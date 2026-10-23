// Team chat + presence, stored on one "hub" SSH machine under ~/.nexus/collab and reached over each Nexus's existing SSH
// connection: no extra server, no extra ports, and only people who can reach those machines can read it.
//   chat.jsonl          one JSON message per line, appended atomically (lines are kept well under PIPE_BUF)
//   presence/<id>.json  each Nexus's heartbeat: who, status line, which sessions it has open
const { EventEmitter } = require('events');
const crypto = require('crypto');

const DIR = '~/.nexus/collab';
const MAX_TEXT = 1500;
const PRESENCE_TTL_MIN = 2;

// ---------------------------------------------------------------- pure helpers (tested)
const clean = (s, n) => String(s == null ? '' : s).replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, '').slice(0, n);

function makeMessage({ from, text, kind = 'msg' }) {
  return {
    id: crypto.randomBytes(6).toString('hex'),
    ts: Date.now(),
    kind: ['msg', 'status', 'activity'].includes(kind) ? kind : 'msg',
    from: { name: clean(from && from.name, 40) || 'someone', login: clean(from && from.login, 80), client: clean(from && from.client, 24) },
    text: clean(text, MAX_TEXT).trim(),
  };
}

// Parse one chat.jsonl line; anything malformed (or hand-edited badly) is dropped rather than trusted.
function parseLine(line) {
  try {
    const m = JSON.parse(line);
    if (!m || typeof m.id !== 'string' || typeof m.ts !== 'number' || typeof m.text !== 'string' || !m.from) return null;
    return { ...makeMessage({ from: m.from, text: m.text, kind: m.kind }), id: clean(m.id, 24), ts: m.ts };
  } catch { return null; }
}

function makePresence({ client, name, login, status, sessions }) {
  return {
    client: clean(client, 24),
    name: clean(name, 40),
    login: clean(login, 80),
    status: clean(status, 140),
    ts: Date.now(),
    sessions: (sessions || []).slice(0, 40).map((s) => ({
      host: clean(s.host, 64), user: clean(s.user, 32), tmux: clean(s.tmux, 64),
      // Others' presence is untrusted: keep the preset to a plain id (it picks an icon and a lookup key in the UI).
      preset: /^[a-z0-9-]{1,24}$/.test(String(s.preset || '')) ? s.preset : 'shell', name: clean(s.name, 60), cwd: clean(s.cwd, 200),
    })),
  };
}

const normDir = (d) => String(d || '').replace(/\/+$/, '') || '~';

// What would collide if *client* opened `target` ({host, user, tmux?, cwd?, preset?})?
//   sameSession: other people attached to that exact tmux session
//   sameFolder:  other people running an agent in the same folder on the same machine/account
function conflicts(presence, client, target, agentPresets = ['claude', 'antigravity', 'openclaw']) {
  const out = { sameSession: [], sameFolder: [] };
  for (const p of presence) {
    if (!p || p.client === client) continue;
    for (const s of p.sessions || []) {
      if (s.host !== target.host || s.user !== target.user) continue;
      if (target.tmux && s.tmux === target.tmux) out.sameSession.push({ person: p.name, session: s });
      else if (!target.tmux && target.cwd && agentPresets.includes(s.preset) && normDir(s.cwd) === normDir(target.cwd)) {
        out.sameFolder.push({ person: p.name, session: s });
      }
    }
  }
  return out;
}

// ---------------------------------------------------------------- hub connection

class CollabHub extends EventEmitter {
  constructor(machine) {
    super();
    this.machine = machine; // a connected lib/remote Machine
    this.stream = null;
    this.buf = '';
  }

  async start() {
    this.stop();
    await this.machine.connect();
    // Keep the chat log bounded (last 2000 messages) and drop week-old presence files.
    await this.machine.exec(`mkdir -p ${DIR}/presence && touch ${DIR}/chat.jsonl && `
      + `if [ $(wc -c < ${DIR}/chat.jsonl) -gt 2000000 ]; then tail -n 2000 ${DIR}/chat.jsonl > ${DIR}/chat.tmp && mv -f ${DIR}/chat.tmp ${DIR}/chat.jsonl; fi; `
      + `find ${DIR}/presence -name '*.json' -mmin +10080 -delete 2>/dev/null; true`);
    await new Promise((resolve, reject) => {
      this.machine.conn.exec(`exec tail -n 300 -F ${DIR}/chat.jsonl 2>/dev/null`, (err, stream) => {
        if (err) return reject(err);
        this.stream = stream;
        stream.on('data', (d) => this.onData(d));
        stream.on('close', () => { if (this.stream === stream) { this.stream = null; this.emit('closed'); } });
        resolve();
      });
    });
  }

  onData(d) {
    this.buf += d.toString('utf8');
    let i;
    while ((i = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, i);
      this.buf = this.buf.slice(i + 1);
      const m = parseLine(line);
      if (m) this.emit('message', m);
    }
  }

  stop() {
    if (this.stream) { try { this.stream.close(); } catch {} }
    this.stream = null;
    this.buf = '';
  }

  async post(msg) {
    const line = JSON.stringify(msg) + '\n';
    const r = await this.machine.exec(`cat >> ${DIR}/chat.jsonl`, line);
    if (r.code) throw new Error(r.stderr || 'could not post');
  }

  // Write our heartbeat and return everyone's fresh presence (including ours).
  async heartbeat(presence) {
    const id = presence.client.replace(/[^a-z0-9]/gi, '') || 'anon'; // safe as a bare path segment
    const f = `${DIR}/presence/${id}.json`;
    const { stdout } = await this.machine.exec(
      `mkdir -p ${DIR}/presence && cat > ${f}.tmp && mv -f ${f}.tmp ${f} && `
        + `find ${DIR}/presence -name '*.json' -mmin -${PRESENCE_TTL_MIN} -exec sh -c 'cat "$1"; echo' _ {} \\;`,
      JSON.stringify(presence), 10000);
    const list = [];
    for (const line of stdout.split('\n')) {
      try { const p = JSON.parse(line); if (p && p.client) list.push(makePresence(p)); } catch {}
    }
    return list;
  }
}

module.exports = { CollabHub, makeMessage, parseLine, makePresence, conflicts, MAX_TEXT };
