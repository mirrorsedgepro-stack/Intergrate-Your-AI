// Persistent settings: machines, presets, appearance. Stored as config.json in the app data folder (%APPDATA%\Nexus,~/Library/Application Support/Nexus, ~/.config/Nexus).
const fs = require('fs');
const path = require('path');
const { platformOs } = require('./platform');
// Electron is required lazily so the pure helpers (merge, migrate) can be unit-tested in plain Node.

const DEFAULTS = {
  version: 1,
  appearance: {
    fontSize: 14,
    fontFamily: '"JetBrains Mono", "Cascadia Code", Menlo, Consolas, monospace',
    lineHeight: 1.15,
    copyOnSelect: false,
    notifyWhenDone: true,
    restoreSession: true,
  },
  presets: [
    { id: 'claude', name: 'Claude', color: '#E08A62', cmd: { windows: 'claude', linux: 'claude' } },
    { id: 'antigravity', name: 'Antigravity', color: '#7BA7FF', cmd: { windows: 'agy', linux: 'agy' } },
    { id: 'openclaw', name: 'OpenClaw', color: '#FF5C5C', cmd: { windows: 'openclaw tui', linux: 'openclaw tui' } },
    { id: 'shell',  name: 'Shell',  color: '#73DACA', cmd: { windows: '', linux: '' } },
    { id: 'gitbash', name: 'Git Bash', color: '#E0AF68', only: 'windows', cmd: { windows: '__gitbash__' } },
  ],
  machines: [
    { id: 'local', name: platformOs() === 'mac' ? 'This Mac' : 'This PC', type: 'local', os: platformOs(), color: '#7AA2F7' },
    { id: 'linux', name: 'Linux box', type: 'ssh', os: 'linux', host: '', port: 22, user: '', keyPath: '', color: '#BB9AF7' },
  ],
  knownHosts: {},
  workspaces: [],
  recentDirs: {},
  lastSession: null,
};

function file() {
  return path.join(require('electron').app.getPath('userData'), 'config.json');
}

function merge(base, over) {
  if (Array.isArray(base) || typeof base !== 'object' || base === null) return over === undefined ? base : over;
  const out = { ...base };
  for (const k of Object.keys(over || {})) out[k] = merge(base[k], over[k]);
  return out;
}

let cache = null;

// Built-in presets added after the first release. Each is added to existing configs once (recorded in
// presetsAdded), so deleting one in the config file sticks.
const ADDED_PRESETS = ['openclaw'];

// Bring an older config up to date in place. Returns true if anything changed.
function migrate(cfg) {
  let changed = false;
  // Gemini CLI became the Antigravity CLI (`agy`): swap the old default preset in place.
  const i = cfg.presets.findIndex((p) => p.id === 'gemini' && (p.cmd || {}).linux === 'gemini');
  if (i >= 0 && !cfg.presets.some((p) => p.id === 'antigravity')) {
    cfg.presets[i] = { ...DEFAULTS.presets.find((p) => p.id === 'antigravity') };
    changed = true;
  }
  const added = Array.isArray(cfg.presetsAdded) ? cfg.presetsAdded : [];
  for (const id of ADDED_PRESETS) {
    if (added.includes(id)) continue;
    added.push(id);
    changed = true;
    if (cfg.presets.some((p) => p.id === id)) continue;
    // Agents go before the shells, after the last built-in agent the user still has.
    const at = cfg.presets.findIndex((p) => p.id === 'shell');
    cfg.presets.splice(at < 0 ? cfg.presets.length : at, 0, { ...DEFAULTS.presets.find((p) => p.id === id) });
  }
  cfg.presetsAdded = added;
  return changed;
}

function load() {
  if (cache) return cache;
  try {
    cache = merge(DEFAULTS, JSON.parse(fs.readFileSync(file(), 'utf8')));
    if (migrate(cache)) save(cache);
    // The local machine is always this computer, whatever OS the config was written on.
    for (const m of cache.machines) if (m.type === 'local') m.os = platformOs();
  } catch {
    cache = JSON.parse(JSON.stringify(DEFAULTS));
    migrate(cache);
    save(cache);
  }
  return cache;
}

function save(cfg) {
  cache = cfg;
  fs.mkdirSync(path.dirname(file()), { recursive: true });
  fs.writeFileSync(file(), JSON.stringify(cfg, null, 2));
  return cfg;
}

module.exports = { load, save, file, merge, migrate, DEFAULTS, ADDED_PRESETS };
