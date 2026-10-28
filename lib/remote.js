// Remote machines over SSH (ssh2). One connection per machine, one channel per session.
// Sessions run inside a dedicated tmux server (-L nexus) so they survive app restarts and dropped links.
const { Client, utils } = require('ssh2');
const { EventEmitter } = require('events');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { SSH_DIR, expandHome, sshConfigFor, checkKnownHosts, fingerprint } = require('./sshconfig');
const { isTailnetAddress } = require('./tailscale');
const { LINUX_SCRIPT, parseLinux, NetRate } = require('./stats');

const TMUX = 'tmux -L nexus -f ~/.nexus/tmux.conf';

// Invisible tmux: no status bar, no mouse capture (so the app's own selection/copy works),
// and no alternate screen (so output lands in the app's scrollback). Bells pass through to Nexus.
const TMUX_CONF = `# Managed by Nexus - overwritten on connect
set -g status off
set -g mouse off
set -g escape-time 0
set -g history-limit 50000
set -g focus-events on
set -g set-clipboard on
set -g allow-passthrough on
set -g extended-keys on
set -g default-terminal "tmux-256color"
set -ga terminal-overrides ',xterm*:smcup@:rmcup@,xterm-256color:Tc'
# Forward apps' OSC 52 clipboard writes to Nexus (Ms for older tmux, terminal-features for 3.2+).
set -ga terminal-overrides ',xterm*:Ms=\\E]52;%p1%s;%p2%s\\007'
set -as terminal-features ',xterm*:clipboard'
set -g set-titles on
set -g set-titles-string '#T'
set-environment -g COLORTERM truecolor
set -g remain-on-exit off
set -g monitor-bell on
set -g bell-action any
set -g visual-bell off
`;

// Claude Code hooks that ring the terminal bell when Claude stops or needs input. The bell travels
// through tmux to Nexus, which turns it into an exact "waiting for you" signal.
const ALERTS_PY = `
import json, os
p = os.path.expanduser('~/.claude/settings.json')
os.makedirs(os.path.dirname(p), exist_ok=True)
try:
    with open(p) as f: s = json.load(f)
except Exception:
    s = {}
cmd = "printf '\\\\a' > /dev/tty 2>/dev/null || true"
hooks = s.setdefault('hooks', {})
added = 0
for ev in ('Stop', 'Notification'):
    lst = hooks.setdefault(ev, [])
    if not any(h.get('command') == cmd for e in lst for h in (e.get('hooks') or [])):
        lst.append({'hooks': [{'type': 'command', 'command': cmd}]})
        added += 1
with open(p, 'w') as f: json.dump(s, f, indent=2)
print('alerts:' + str(added))
`;

const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;
// Quote a remote path but keep a leading ~ expandable.
const shPath = (p) => (p === '~' ? '"$HOME"' : p.startsWith('~/') ? `"$HOME"/${shq(p.slice(2))}` : shq(p));
const AGENT_PIPE = '\\\\.\\pipe\\openssh-ssh-agent';

// Remote command for an interactive session. The agent runs in a login+interactive bash (so nvm /
// ~/.local/bin PATH entries load); when it exits you drop into your normal shell instead of losing the pane.
// record: also pipe everything the pane prints into ~/.nexus/logs/<tmuxName>-<date>.log on the machine (an audit
// trail of what each agent did; pipe-pane -o makes it idempotent across reattaches).
function sessionCommand(tmuxName, cmd, tmux = TMUX, cwd = '', readOnly = false, record = false) {
  // Watch someone else's session without being able to type into it.
  if (tmuxName && readOnly) return `exec ${tmux} attach-session -r -t ${shq(tmuxName)}`;
  const cd = cwd ? `cd ${shPath(cwd)} 2>/dev/null; ` : '';
  const inner = cmd ? `${cd}${cmd}; exec "$SHELL" -l` : `${cd}exec "$SHELL" -l`;
  if (!tmuxName) return `exec bash -ilc ${shq(inner)}`;
  const run = cmd ? `bash -ilc ${shq(inner)}` : '"$SHELL" -l';
  const dir = cwd ? `-c ${shPath(cwd)} ` : '';
  const fallback = `else printf '\\033[33m[nexus] tmux is not installed here - this session will not survive disconnects.\\033[0m\\r\\n'; exec bash -ilc ${shq(inner)}; fi`;
  if (!record) return `if command -v tmux >/dev/null 2>&1; then exec ${tmux} new-session -A ${dir}-s ${shq(tmuxName)} ${shq(run)}; ${fallback}`;
  const log = `"$HOME"/.nexus/logs/${shq(tmuxName)}-$(date +%Y%m%d).log`;
  return `if command -v tmux >/dev/null 2>&1; then ${tmux} new-session -A -d ${dir}-s ${shq(tmuxName)} ${shq(run)} && `
    + `(umask 077; mkdir -p "$HOME"/.nexus/logs; ${tmux} pipe-pane -o -t ${shq(tmuxName)} ${shq(`exec cat >> ${log}`)}); `
    + `exec ${tmux} attach-session -t ${shq(tmuxName)}; ${fallback}`;
}

function friendly(err, m) {
  const msg = String(err && err.message || err);
  if (/authentication methods failed/i.test(msg)) return `Authentication failed for ${m.user || '?'}@${m.host} (tried ${m.triedKeys || 'no keys'}). Use "Set up SSH key" in Settings.`;
  if (err && err.code === 'ECONNREFUSED') return `${m.host}:${m.port || 22} refused the connection. Is sshd running?`;
  if (err && (err.code === 'ETIMEDOUT' || err.level === 'client-timeout')) return `Timed out reaching ${m.host}.`;
  if (err && (err.code === 'ENOTFOUND' || err.code === 'EAI_AGAIN') && isTailnetAddress(m.host)) {
    return `Can't find ${m.host} on Tailscale. Is Tailscale running on this PC and signed in to the tailnet this machine is on?`;
  }
  if (err && err.code === 'ENOTFOUND') return `Host "${m.host}" not found.`;
  if (err && err.code === 'EHOSTUNREACH') return `${m.host} is unreachable.`;
  return msg;
}

class Machine extends EventEmitter {
  // store: { get(), known(key), remember(key, fp), askPassphrase?(keyFile, machineCfg) -> Promise<string|null>,
  //          confirmHost?({ key, fingerprint, host, port, address }, machineCfg) -> Promise<boolean> }
  constructor(id, store) {
    super();
    this.id = id;
    this.store = store;
    this.status = 'offline';
    this.error = null;
    this.conn = null;
    this.connecting = null;
    this.passphrases = {};
    this.declinedHosts = new Set();
    this.encryptedKeys = [];
  }

  setStatus(status, error = null) {
    this.status = status;
    this.error = error;
    this.emit('status', status, error, this.route || null);
  }

  // address: the host actually dialled (LAN name/IP, or the machine's Tailscale address).
  authOptions(m, address = m.host, quick = false) {
    // Blank Nexus fields fall back to ~/.ssh/config, exactly like the ssh command does.
    const sc = sshConfigFor(address);
    const port = Number(m.port) || sc.port || 22;
    const username = m.user || sc.user || os.userInfo().username;
    const host = sc.hostname || address;
    const knownFiles = [...sc.knownHostsFiles, path.join(SSH_DIR, 'known_hosts')];
    const opts = {
      host,
      port,
      username,
      // With a fallback address still to try, give up on an unreachable one quickly.
      readyTimeout: quick ? 5000 : 12000,
      keepaliveInterval: 15000,
      keepaliveCountMax: 4,
      hostVerifier: (blob) => {
        // Trust ~/.ssh/known_hosts first (what `ssh` uses); fall back to Nexus's own trust-on-first-use list.
        const verdict = checkKnownHosts(knownFiles, host, port, blob);
        const fp = fingerprint(blob);
        if (verdict === 'match') return true;
        if (verdict === 'revoked' || verdict === 'mismatch') {
          this.hostKeyError = `Host key for ${host}:${port} does not match ~/.ssh/known_hosts (got ${fp}). `
            + 'If the machine was reinstalled, remove the old entry with: ssh-keygen -R ' + (port === 22 ? host : `[${host}]:${port}`);
          return false;
        }
        // Pinned per address: Tailscale SSH answers with its own host key, different from sshd's on the LAN.
        const k = `${address}:${port}`;
        const known = this.store.known(k);
        if (!known) {
          // Tailnet peers are already authenticated by WireGuard: pin silently. Anywhere else (LAN, internet) a first
          // connection could be a man in the middle, so the user confirms the fingerprint once (if the app can ask).
          if (isTailnetAddress(address) || isTailnetAddress(host) || !this.store.confirmHost) { this.store.remember(k, fp); return true; }
          this.unknownHost = { key: k, fingerprint: fp, host, port, address };
          this.hostKeyError = `${k} presented a host key Nexus hasn't seen before (${fp}). It was not trusted.`;
          return false;
        }
        if (known !== fp) {
          this.hostKeyError = `Host key for ${k} changed (expected ${known}, got ${fp}). Remove it under "knownHosts" in the config if this is expected.`;
          return false;
        }
        return true;
      },
    };
    // Try every usable key in order: explicit key, ~/.ssh/config IdentityFiles, then the defaults; then ssh-agent.
    // Machines added from someone else's invite code use Tailscale SSH ("none" auth): don't offer them your keys, which
    // would tell that host which public keys you hold (and let it try them against other servers' prompts).
    const candidates = m.invited ? (m.keyPath ? [expandHome(m.keyPath)] : []) : [...new Set([
      ...(m.keyPath ? [expandHome(m.keyPath)] : []),
      ...sc.identityFiles,
      ...['id_ed25519', 'id_ecdsa', 'id_rsa'].map((f) => path.join(SSH_DIR, f)),
    ])];
    // "none" first: Tailscale SSH admits tailnet identities its policy allows without any key; OpenSSH refuses it.
    const methods = [{ type: 'none', username }];
    const tried = [];
    this.encryptedKeys = [];
    for (const f of candidates) {
      if (!fs.existsSync(f)) continue;
      const parsed = utils.parseKey(fs.readFileSync(f), this.passphrases[f] || m.passphrase || undefined);
      if (parsed instanceof Error) {
        if (/encrypt|passphrase|bad decrypt/i.test(parsed.message)) {
          if (this.passphrases[f]) delete this.passphrases[f]; // wrong passphrase: ask again next time
          this.encryptedKeys.push(f);
        }
        continue;
      }
      methods.push({ type: 'publickey', username, key: Array.isArray(parsed) ? parsed[0] : parsed });
      tried.push(path.basename(f));
    }
    const agent = process.platform === 'win32' ? (fs.existsSync(AGENT_PIPE) && AGENT_PIPE) : process.env.SSH_AUTH_SOCK;
    if (agent && !m.invited) { methods.push({ type: 'agent', username, agent }); tried.push('ssh-agent'); }
    this.triedKeys = tried.join(', ') || 'no usable keys';
    opts.authHandler = () => methods.shift() || false;
    return opts;
  }

  // One connection attempt to one address. Rejects with err.authFailed when the server refused every key,
  // err.unreachable when the address couldn't be reached at all.
  attempt(m, address = m.host, quick = false) {
    this.hostKeyError = null;
    this.unknownHost = null;
    return new Promise((resolve, reject) => {
      const conn = new Client();
      let settled = false;
      const fail = (err) => {
        if (settled) return;
        settled = true;
        const msg = this.hostKeyError || friendly(err, { ...m, host: address, triedKeys: this.triedKeys });
        const e = new Error(msg);
        e.authFailed = /authentication methods failed/i.test(String(err && err.message || err));
        e.unknownHost = this.unknownHost;
        e.unreachable = !this.hostKeyError && !e.authFailed
          && (['ECONNREFUSED', 'ETIMEDOUT', 'EHOSTUNREACH', 'ENETUNREACH', 'ENOTFOUND', 'EAI_AGAIN'].includes(err && err.code) || (err && err.level) === 'client-timeout');
        reject(e);
      };
      conn.on('ready', async () => {
        this.conn = conn;
        settled = true;
        this.homeDir = null;
        this.address = address;
        this.route = isTailnetAddress(address) ? 'tailscale' : 'lan';
        try {
          // ~/.nexus holds pasted screenshots, uploads and team chat: private to this account.
          await this.exec(`umask 077; mkdir -p ~/.nexus/uploads && chmod 700 ~/.nexus ~/.nexus/uploads; cat > ~/.nexus/tmux.conf && (${TMUX} source-file ~/.nexus/tmux.conf >/dev/null 2>&1 || true); `
            + 'find ~/.nexus/uploads -type f -mtime +7 -delete 2>/dev/null; find ~/.nexus/logs -type f -mtime +30 -delete 2>/dev/null; true', TMUX_CONF);
        } catch {}
        this.setStatus('online');
        resolve();
      });
      conn.on('error', fail);
      conn.on('close', () => {
        if (this.conn === conn) this.conn = null;
        if (!settled) return fail(new Error('Connection closed'));
        if (this.status === 'online') this.setStatus('offline');
      });
      try { conn.connect(this.authOptions(m, address, quick)); } catch (err) { fail(err); }
    });
  }

  // Try the LAN address, then the machine's Tailscale address (away from home, or a guest's only route).
  async attemptAll(m) {
    const addresses = [...new Set([m.host, m.tsHost].filter(Boolean))];
    let last;
    for (let i = 0; i < addresses.length; i++) {
      try {
        await this.attempt(m, addresses[i], i < addresses.length - 1);
        return;
      } catch (err) {
        last = err;
        if (!err.unreachable) break; // auth / host-key problems are real answers, not routing ones
      }
    }
    this.setStatus('error', last.message);
    throw last;
  }

  connect() {
    if (this.status === 'online' && this.conn) return Promise.resolve();
    if (this.connecting) return this.connecting;
    const m = this.store.get();
    if (!m || !(m.host || m.tsHost)) return Promise.reject(new Error('No host configured for this machine. Open Settings to add one.'));
    this.setStatus('connecting');
    // One attempt round, plus a single retry after the user trusts a new host key.
    const round = async () => {
      try {
        await this.attemptAll(m);
      } catch (err) {
        const u = err.unknownHost;
        if (!u) throw err;
        const seen = `${u.key} ${u.fingerprint}`;
        if (this.declinedHosts.has(seen)) throw err; // asked once this run already; don't nag on auto-reconnect
        if (!(await this.store.confirmHost(u, m))) { this.declinedHosts.add(seen); throw err; }
        this.store.remember(u.key, u.fingerprint);
        this.setStatus('connecting');
        await this.attemptAll(m);
      }
    };
    this.connecting = (async () => {
      try {
        await round();
      } catch (err) {
        // Passphrase-protected keys: ask once, then retry with them unlocked.
        if (!err.authFailed || !this.encryptedKeys.length || !this.store.askPassphrase) throw err;
        let unlocked = false;
        for (const f of this.encryptedKeys) {
          const pass = await this.store.askPassphrase(f, m);
          if (pass) { this.passphrases[f] = pass; unlocked = true; }
        }
        if (!unlocked) throw err;
        this.setStatus('connecting');
        await round();
      }
    })().finally(() => { this.connecting = null; });
    return this.connecting;
  }

  disconnect() {
    if (this.conn) this.conn.end();
  }

  // Run a command with no pty; resolves with stdout/stderr/code.
  exec(cmd, stdin, timeoutMs = 20000) {
    return new Promise((resolve, reject) => {
      if (!this.conn) return reject(new Error('Not connected'));
      this.conn.exec(cmd, (err, stream) => {
        if (err) return reject(err);
        let out = '', errOut = '', code = null;
        const t = setTimeout(() => { stream.close(); reject(new Error('Remote command timed out')); }, timeoutMs);
        stream.on('data', (d) => { out += d; });
        stream.stderr.on('data', (d) => { errOut += d; });
        stream.on('exit', (c) => { code = c; });
        stream.on('close', () => { clearTimeout(t); resolve({ code, stdout: out, stderr: errOut }); });
        if (stdin != null) stream.end(stdin); else stream.end();
      });
    });
  }

  async home() {
    if (!this.homeDir) this.homeDir = (await this.exec('printf %s "$HOME"')).stdout.trim();
    return this.homeDir;
  }

  async probe() {
    await this.connect();
    const script = `for t in tmux claude agy openclaw node git python3 nvidia-smi; do if command -v $t >/dev/null 2>&1; then echo "tool:$t=1"; else echo "tool:$t=0"; fi; done; `
      + `${TMUX} ls -F 'sess:#{session_name}|#{session_attached}|#{session_activity}|#{pane_current_path}' 2>/dev/null; echo "host:$(hostname)"; echo "home:$HOME"; echo "user:$(id -un)"`;
    const { stdout } = await this.exec(`bash -ilc ${shq(script)} 2>/dev/null </dev/null`);
    const tools = {}, sessions = [];
    let hostname = '', home = '', user = '';
    for (const line of stdout.split(/\r?\n/)) {
      let mm;
      if ((mm = line.match(/^tool:([\w-]+)=([01])/))) tools[mm[1]] = mm[2] === '1';
      else if ((mm = line.match(/^sess:([^|]+)\|(\d+)\|(\d+)\|(.*)$/))) sessions.push({ name: mm[1], attached: Number(mm[2]), activity: Number(mm[3]) * 1000, cwd: mm[4] });
      else if ((mm = line.match(/^host:(.*)/))) hostname = mm[1].trim();
      else if ((mm = line.match(/^home:(.*)/))) home = mm[1].trim();
      else if ((mm = line.match(/^user:(.*)/))) user = mm[1].trim();
    }
    if (home) this.homeDir = home;
    // Show paths under $HOME as ~/..., which is also how Nexus stores them.
    for (const s of sessions) if (home && s.cwd.startsWith(home)) s.cwd = '~' + s.cwd.slice(home.length);
    return { tools, sessions, hostname, home, user };
  }

  // GPU / CPU / memory / disk / network snapshot for the machine card.
  async stats() {
    if (!this.conn) return null;
    const { stdout } = await this.exec(LINUX_SCRIPT, null, 8000);
    this.netRate = this.netRate || new NetRate();
    return this.netRate.finish(parseLinux(stdout));
  }

  // Project folders for the "launch in folder" picker: git repos (depth 4) and plain folders (depth 2).
  async listDirs() {
    await this.connect();
    const script = 'cd ~ || exit 0; '
      + 'find . -maxdepth 4 \\( -name node_modules -o -name .cache -o -name .local -o -name .npm -o -name .nvm -o -name snap -o -name .nexus \\) -prune -o -name .git -print 2>/dev/null | head -n 300 | sed "s#/\\.git\\$##; s#^\\.#repo:~#"; '
      + 'find . -mindepth 1 -maxdepth 2 -type d -not -path "*/.*" -not -path "*/node_modules*" 2>/dev/null | head -n 400 | sed "s#^\\.#dir:~#"';
    const { stdout } = await this.exec(script, null, 15000);
    const repos = [], dirs = [];
    const hidden = (p) => /\/\./.test(p); // tool checkouts like ~/.oh-my-zsh are not projects
    for (const line of stdout.split(/\r?\n/)) {
      if (line.startsWith('repo:') && !hidden(line.slice(5))) repos.push(line.slice(5));
      else if (line.startsWith('dir:')) dirs.push(line.slice(4));
    }
    return { repos, dirs };
  }

  async paneCwd(tmuxName) {
    if (!this.conn || !tmuxName) return null;
    const { stdout } = await this.exec(`${TMUX} display-message -p -t ${shq(tmuxName)} '#{pane_current_path}' 2>/dev/null`, null, 5000);
    return stdout.trim() || null;
  }

  sftp() {
    return new Promise((resolve, reject) => this.conn.sftp((err, s) => (err ? reject(err) : resolve(s))));
  }

  // Upload a buffer or a local file into ~/.nexus/uploads; returns the absolute remote path.
  async uploadToInbox(name, { buffer, localPath }) {
    await this.connect();
    const home = await this.home();
    const safe = String(name).replace(/[^\w.\-]+/g, '_').slice(-80) || 'file';
    const abs = `${home}/.nexus/uploads/${Date.now().toString(36)}-${safe}`;
    await this.exec('umask 077; mkdir -p ~/.nexus/uploads && chmod 700 ~/.nexus/uploads');
    const sftp = await this.sftp();
    try {
      await new Promise((resolve, reject) => {
        const done = (e) => (e ? reject(e) : resolve());
        if (buffer) sftp.writeFile(abs, buffer, done); else sftp.fastPut(localPath, abs, done);
      });
    } finally { sftp.end(); }
    return abs;
  }

  async setupAlerts() {
    await this.connect();
    const r = await this.exec('python3 - 2>&1', ALERTS_PY);
    const mm = /alerts:(\d+)/.exec(r.stdout);
    if (!mm) throw new Error(r.stdout.trim() || r.stderr.trim() || 'python3 is required on the remote');
    return Number(mm[1]);
  }

  // Interactive channel with a pty. tmuxName -> persistent session; otherwise a plain one-shot login shell.
  async open({ tmuxName, cmd, cols, rows, cwd, readOnly, record }) {
    await this.connect();
    const full = sessionCommand(tmuxName, cmd, TMUX, cwd, readOnly, !!record);
    return new Promise((resolve, reject) => {
      this.conn.exec(full, { pty: { term: 'xterm-256color', cols, rows, width: 0, height: 0 } }, (err, stream) => {
        if (err) reject(err); else resolve(stream);
      });
    });
  }

  async killTmux(name) {
    if (!this.conn) return;
    await this.exec(`${TMUX} kill-session -t ${shq(name)} 2>/dev/null`).catch(() => {});
  }

  async upload(remotePath, content) {
    await this.connect();
    const dir = remotePath.replace(/\/[^/]*$/, '');
    const r = await this.exec(`mkdir -p ${shPath(dir)} && cat > ${shPath(remotePath)} && chmod +x ${shPath(remotePath)}`, content);
    if (r.code) throw new Error(r.stderr || 'upload failed');
  }
}

module.exports = { Machine, sessionCommand, shq, shPath, sshConfigFor, ALERTS_PY };
