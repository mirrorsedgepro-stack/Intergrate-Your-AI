const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { execFileSync } = require('child_process');
const { sessionCommand, shq, shPath } = require('../lib/remote');
const { merge, migrate, DEFAULTS } = require('../lib/config');

// A POSIX bash to run generated commands (Linux CI, or Git Bash on Windows).
const BASH = process.platform === 'win32'
  ? ['C:\\Program Files\\Git\\bin\\bash.exe'].find((p) => fs.existsSync(p))
  : '/bin/bash';

test('shq / shPath quoting', () => {
  assert.strictEqual(shq("it's"), `'it'\\''s'`);
  assert.strictEqual(shPath('~'), '"$HOME"');
  assert.strictEqual(shPath('~/code/my app'), `"$HOME"/'code/my app'`);
  assert.strictEqual(shPath('/srv/x'), `'/srv/x'`);
});

test('sessionCommand runs the agent in the folder via tmux, with nested quotes intact', { skip: !BASH && 'no bash available' }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'nexus-cmd-'));
  const work = path.join(dir, "my 'proj'");
  fs.mkdirSync(work);
  // Fake tmux: print -c, then run the session command the way tmux would (sh -c).
  const fake = path.join(dir, 'tmux');
  fs.writeFileSync(fake, '#!/bin/bash\nwhile [ $# -gt 1 ]; do [ "$1" = -c ] && echo "start-dir=$2"; shift; done\nexec sh -c "$1"\n', { mode: 0o755 });
  const posix = (p) => (process.platform === 'win32' ? p.replace(/^([A-Za-z]):/, (_, d) => `/${d.toLowerCase()}`).replace(/\\/g, '/') : p);
  const cmd = sessionCommand('nx-claude-test', `pwd; echo "it's quoted"`, 'tmux', posix(work));
  const out = execFileSync(BASH, ['-c', cmd], {
    env: { ...process.env, PATH: `${posix(dir)}:${process.env.PATH}`, SHELL: '/usr/bin/true', HOME: posix(dir) },
    input: '',
    encoding: 'utf8',
  });
  assert.match(out, /start-dir=.*my 'proj'/);
  assert.match(out, /my 'proj'\n/);
  assert.match(out, /it's quoted/);
});

test('sessionCommand without tmux name is a plain login shell', () => {
  assert.match(sessionCommand(null, 'claude'), /^exec bash -ilc 'claude; exec "\$SHELL" -l'$/);
});

test('config migrate swaps the old Gemini preset for Antigravity once', () => {
  const cfg = JSON.parse(JSON.stringify(DEFAULTS));
  cfg.presets = cfg.presets.filter((p) => p.id !== 'antigravity');
  cfg.presets.splice(1, 0, { id: 'gemini', name: 'Gemini', cmd: { windows: 'gemini', linux: 'gemini' } });
  assert.strictEqual(migrate(cfg), true);
  assert.strictEqual(cfg.presets[1].id, 'antigravity');
  assert.strictEqual(cfg.presets[1].cmd.linux, 'agy');
  assert.strictEqual(migrate(cfg), false);
});

test('config migrate adds OpenClaw to an older config once, before the shells', () => {
  // A v1.3.0 config: saved presets without OpenClaw, merged over the current defaults (arrays are replaced).
  const old = { presets: DEFAULTS.presets.filter((p) => p.id !== 'openclaw') };
  const cfg = merge(JSON.parse(JSON.stringify(DEFAULTS)), JSON.parse(JSON.stringify(old)));
  assert.strictEqual(migrate(cfg), true);
  const ids = cfg.presets.map((p) => p.id);
  assert.deepStrictEqual(ids.slice(0, 4), ['claude', 'antigravity', 'openclaw', 'shell']);
  assert.strictEqual(cfg.presets[2].cmd.linux, 'openclaw tui');
  assert.strictEqual(migrate(cfg), false);
  // Deleted by the user afterwards: it stays deleted.
  cfg.presets = cfg.presets.filter((p) => p.id !== 'openclaw');
  assert.strictEqual(migrate(cfg), false);
  assert.ok(!cfg.presets.some((p) => p.id === 'openclaw'));
  // A fresh config already has it and is only marked.
  const fresh = JSON.parse(JSON.stringify(DEFAULTS));
  migrate(fresh);
  assert.strictEqual(fresh.presets.filter((p) => p.id === 'openclaw').length, 1);
});

test('Claude alert hooks: added once, existing settings preserved', () => {
  const { ALERTS_PY } = require('../lib/remote');
  const py = ['python3', 'python'].find((p) => { try { execFileSync(p, ['--version'], { stdio: 'ignore' }); return true; } catch { return false; } });
  if (!py) return;
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'nexus-alerts-'));
  fs.mkdirSync(path.join(home, '.claude'));
  const file = path.join(home, '.claude', 'settings.json');
  fs.writeFileSync(file, JSON.stringify({ model: 'opus', hooks: { Stop: [{ hooks: [{ type: 'command', command: 'echo mine' }] }] } }));
  const run = () => execFileSync(py, ['-'], { input: ALERTS_PY, env: { ...process.env, HOME: home, USERPROFILE: home }, encoding: 'utf8' });
  assert.match(run(), /alerts:2/);
  assert.match(run(), /alerts:0/);
  const s = JSON.parse(fs.readFileSync(file, 'utf8'));
  assert.strictEqual(s.model, 'opus');
  assert.strictEqual(s.hooks.Stop.length, 2);
  assert.strictEqual(s.hooks.Notification.length, 1);
  assert.strictEqual(s.hooks.Stop[1].hooks[0].command, String.raw`printf '\a' > /dev/tty 2>/dev/null || true`);
});

test('first-seen host keys: confirmed off the tailnet, pinned silently on it', () => {
  const crypto = require('crypto');
  const { Machine } = require('../lib/remote');
  const t = Buffer.from('ssh-ed25519'), k = crypto.randomBytes(32);
  const len = (n) => { const b = Buffer.alloc(4); b.writeUInt32BE(n); return b; };
  const blob = Buffer.concat([len(t.length), t, len(k.length), k]);
  const pinned = {};
  const store = { get: () => null, known: (x) => pinned[x], remember: (x, fp) => { pinned[x] = fp; }, confirmHost: async () => true };
  const m = { host: 'nexus-test-host.invalid', user: 'u', port: 22 };

  const mach = new Machine('t', store);
  assert.strictEqual(mach.authOptions(m, m.host).hostVerifier(blob), false, 'LAN host not trusted without asking');
  assert.strictEqual(mach.unknownHost.key, 'nexus-test-host.invalid:22');
  assert.match(mach.unknownHost.fingerprint, /^SHA256:/);
  assert.deepStrictEqual(pinned, {});

  assert.strictEqual(mach.authOptions(m, 'box.tail1.ts.net').hostVerifier(blob), true, 'tailnet host pinned');
  assert.ok(pinned['box.tail1.ts.net:22']);

  pinned['nexus-test-host.invalid:22'] = mach.unknownHost.fingerprint; // the user trusted it
  assert.strictEqual(mach.authOptions(m, m.host).hostVerifier(blob), true);

  const legacy = new Machine('l', { ...store, confirmHost: undefined });
  assert.strictEqual(legacy.authOptions({ ...m, host: 'other-host.invalid' }, 'other-host.invalid').hostVerifier(blob), true, 'no prompt available: old trust-on-first-use');
});
