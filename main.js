const { app, BrowserWindow, ipcMain, clipboard, shell, Notification, Menu, safeStorage } = require('electron');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { StringDecoder } = require('string_decoder');
const config = require('./lib/config');
const local = require('./lib/local');
const platform = require('./lib/platform');
const { Machine } = require('./lib/remote');
const tailscale = require('./lib/tailscale');
const collab = require('./lib/collab');

if (!app.requestSingleInstanceLock()) app.quit();
app.setAppUserModelId('com.nexus.terminal');

// In the installed build, files that run outside Electron (PowerShell scripts) live next to app.asar.
const SCRIPTS_DIR = path.join(__dirname, 'scripts').replace(`app.asar${path.sep}`, `app.asar.unpacked${path.sep}`);
const ICON = path.join(__dirname, 'assets', 'icon.png');

let win = null;

// ---- security helpers ----
// Only web links leave the app. Anything else (file:, smb:, ms-*, custom protocol handlers) is a classic Electron
// path to code execution when a page or terminal output gets to choose the URL.
function openExternalSafe(url) {
  let u;
  try { u = new URL(String(url)); } catch { return false; }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') return false;
  shell.openExternal(u.toString()).catch(() => {});
  return true;
}
// IPC is only accepted from our own top-level renderer page (not from an iframe, a navigated page or another window).
const RENDERER_URL = require('url').pathToFileURL(path.join(__dirname, 'renderer', 'index.html')).toString();
function trustedSender(e) {
  try {
    if (!win || win.isDestroyed() || e.sender !== win.webContents) return false;
    const f = e.senderFrame;
    return !!f && f === win.webContents.mainFrame && f.url.split(/[?#]/)[0] === RENDERER_URL;
  } catch { return false; }
}
for (const kind of ['handle', 'on']) {
  const orig = ipcMain[kind].bind(ipcMain);
  ipcMain[kind] = (channel, fn) => orig(channel, (e, ...args) => {
    if (!trustedSender(e)) {
      console.warn(`[nexus] blocked IPC "${channel}" from an untrusted sender`);
      return kind === 'handle' ? Promise.reject(new Error('Untrusted sender')) : undefined;
    }
    return fn(e, ...args);
  });
}
const sessions = new Map(); // id -> { write, resize, close, machineId, tmuxName }
const machines = new Map(); // id -> Machine
let nextId = 1;

function send(ch, ...args) {
  if (win && !win.isDestroyed()) win.webContents.send(ch, ...args);
}

// ---- output batching: coalesce chunks per session into ~4ms frames ----
const pending = new Map();
let flushTimer = null;
function emitData(id, data) {
  if (!data) return;
  pending.set(id, (pending.get(id) || '') + data);
  if (!flushTimer) flushTimer = setTimeout(flush, 4);
}
function flush() {
  flushTimer = null;
  for (const [id, d] of pending) send('session:data', id, d);
  pending.clear();
}

// ---- passphrase prompts (renderer shows a modal, answers over IPC) ----
const asks = new Map();
let askId = 0;
function askPassphrase(keyFile, m) {
  return new Promise((resolve) => {
    const id = ++askId;
    const timer = setTimeout(() => { asks.delete(id); resolve(null); }, 120000);
    asks.set(id, (v) => { clearTimeout(timer); asks.delete(id); resolve(v); });
    send('auth:ask', id, { keyFile, keyName: path.basename(keyFile), machine: m.name });
  });
}
ipcMain.on('auth:answer', (_e, id, value) => { const fn = asks.get(id); if (fn) fn(value || null); });
// First connection to a host key not in ~/.ssh/known_hosts (outside the tailnet): the user checks the fingerprint.
function confirmHost(info, m) {
  return new Promise((resolve) => {
    const id = ++askId;
    const timer = setTimeout(() => { asks.delete(id); resolve(false); }, 120000);
    asks.set(id, (v) => { clearTimeout(timer); asks.delete(id); resolve(v === true); });
    send('auth:host', id, { machine: m.name, address: info.address, host: info.host, port: info.port, fingerprint: info.fingerprint });
  });
}
ipcMain.on('auth:host-answer', (_e, id, ok) => { const fn = asks.get(id); if (fn) fn(ok === true); });

// ---- machines ----
function machineCfg(id) {
  return config.load().machines.find((m) => m.id === id);
}
function getMachine(id) {
  if (!machines.has(id)) {
    const m = new Machine(id, {
      get: () => machineCfg(id),
      known: (k) => config.load().knownHosts[k],
      remember: (k, fp) => { const c = config.load(); c.knownHosts[k] = fp; config.save(c); },
      askPassphrase,
      confirmHost,
    });
    m.on('status', (status, error, route) => {
      send('machine:status', id, status, error, route);
      if (status === 'online' && hub && hub.machineId === id && !hub.ch.stream) startHub(id).catch(() => {});
    });
    machines.set(id, m);
  }
  return machines.get(id);
}

const expandLocal = (p) => (p ? p.replace(/^~(?=[\\/]|$)/, os.homedir()) : p);

// ---- window ----
function createWindow() {
  const cfg = config.load();
  const b = cfg.window || {};
  win = new BrowserWindow({
    width: b.width || 1480,
    height: b.height || 920,
    x: b.x,
    y: b.y,
    minWidth: 860,
    minHeight: 520,
    backgroundColor: '#090c13',
    title: 'Nexus',
    icon: ICON,
    ...(platform.IS_MAC
      ? { titleBarStyle: 'hiddenInset', trafficLightPosition: { x: 14, y: 14 } }
      : { titleBarStyle: 'hidden', titleBarOverlay: { color: '#090c13', symbolColor: '#8b93a8', height: 42 } }),
    show: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webviewTag: false,
      navigateOnDragDrop: false,
      spellcheck: false,
    },
  });
  if (b.maximized) win.maximize();
  win.once('ready-to-show', () => win.show());
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  win.on('close', () => {
    const c = config.load();
    c.window = { ...win.getNormalBounds(), maximized: win.isMaximized() };
    config.save(c);
  });
  win.on('focus', () => send('app:focus', true));
  win.on('blur', () => send('app:focus', false));
  win.webContents.setWindowOpenHandler(({ url }) => { openExternalSafe(url); return { action: 'deny' }; });
  win.webContents.on('will-navigate', (e) => e.preventDefault());
  win.webContents.on('will-redirect', (e) => e.preventDefault());
  win.webContents.on('will-attach-webview', (e) => e.preventDefault());
}

// ---- auto-update (installed builds only; releases on GitHub) ----
async function checkReleaseNotice() {
  try {
    const r = await fetch('https://api.github.com/repos/mirrorsedgepro-stack/Intergrate-Your-AI/releases/latest', { headers: { Accept: 'application/vnd.github+json' } });
    if (!r.ok) return;
    const rel = await r.json();
    const v = String(rel.tag_name || '').replace(/^v/, '');
    const newer = (a, b) => { const x = a.split('.').map(Number), y = b.split('.').map(Number); for (let i = 0; i < 3; i++) { if ((x[i] || 0) !== (y[i] || 0)) return (x[i] || 0) > (y[i] || 0); } return false; };
    if (v && newer(v, app.getVersion())) send('update:available', { version: v, url: rel.html_url });
  } catch { /* offline */ }
}
function setupUpdates() {
  if (!app.isPackaged) return;
  if (platform.IS_MAC) {
    const notice = () => { if (!config.load().offline) checkReleaseNotice(); };
    setTimeout(notice, 8000); setInterval(notice, 6 * 60 * 60 * 1000);
    return;
  }
  let autoUpdater;
  try { ({ autoUpdater } = require('electron-updater')); } catch { return; }
  // Builds are unsigned, so a release is trusted only because it came from the project's GitHub. Don't fetch and
  // stage code silently: announce the version and let the user choose to download it, so an unexpected release is
  // visible before it is installed. Offline mode (Settings) skips the check entirely.
  autoUpdater.autoDownload = false;
  autoUpdater.on('update-available', (info) => send('update:available', { version: info.version, url: `https://github.com/mirrorsedgepro-stack/Intergrate-Your-AI/releases/tag/v${info.version}`, download: true }));
  autoUpdater.on('update-downloaded', (info) => send('update:ready', info.version));
  autoUpdater.on('error', () => {});
  const check = () => { if (!config.load().offline) autoUpdater.checkForUpdates().catch(() => {}); };
  setTimeout(check, 8000);
  setInterval(check, 6 * 60 * 60 * 1000);
  ipcMain.on('update:download', () => autoUpdater.downloadUpdate().catch(() => {}));
  ipcMain.on('update:install', () => autoUpdater.quitAndInstall());
}

Menu.setApplicationMenu(platform.IS_MAC ? Menu.buildFromTemplate([{ role: 'appMenu' }, { role: 'editMenu' }]) : null);

// On Windows, offer each WSL distro as its own machine (once; deleting it in Settings sticks).
async function addWslMachines() {
  const distros = await platform.listWsl();
  if (!distros.length) return;
  const c = config.load();
  c.wslSeen = c.wslSeen || {};
  let changed = false;
  for (const d of distros) {
    if (c.wslSeen[d.name]) continue;
    c.wslSeen[d.name] = true;
    changed = true;
    if (c.machines.some((m) => m.type === 'wsl' && m.distro === d.name)) continue;
    c.machines.push({ id: 'wsl-' + d.name.toLowerCase().replace(/[^a-z0-9]+/g, '-'), name: `WSL · ${d.name}`, type: 'wsl', os: 'linux', distro: d.name, color: '#E95420' });
  }
  if (changed) config.save(c);
}

app.on('web-contents-created', (_e, wc) => {
  // Belt and braces for any future window: no new windows, no navigation away from the app.
  wc.setWindowOpenHandler(({ url }) => { openExternalSafe(url); return { action: 'deny' }; });
  wc.on('will-navigate', (e) => e.preventDefault());
});

app.whenReady().then(async () => {
  // The renderer needs no web permissions (camera, mic, geolocation, notifications, clipboard-read…): notifications
  // and the clipboard go through the main process.
  const { session } = require('electron');
  session.defaultSession.setPermissionRequestHandler((_wc, _perm, cb) => cb(false));
  session.defaultSession.setPermissionCheckHandler(() => false);
  cleanPasteDir();
  await Promise.all([platform.loadLoginShellPath(), addWslMachines().catch(() => {})]);
  createWindow();
  setupUpdates();
});
app.on('second-instance', () => { if (win) { if (win.isMinimized()) win.restore(); win.focus(); } });
app.on('window-all-closed', () => {
  for (const s of sessions.values()) { try { s.close(); } catch {} }
  for (const m of machines.values()) m.disconnect();
  app.quit();
});

// ---- IPC: config / app ----
// The renderer never sees the (encrypted) Tailscale token, nor any key passphrase someone put in config.json by hand.
const publicConfig = () => {
  const c = { ...config.load() };
  if (c.tailscale) c.tailscale = { ...c.tailscale, token: undefined };
  c.machines = (c.machines || []).map((m) => (m.passphrase ? { ...m, passphrase: undefined } : m));
  return c;
};
// Passphrases are per-run secrets (asked in a modal, held in memory). One left in config.json is still honoured for
// compatibility, but is kept out of the renderer and out of any config the renderer writes back.
const withSecrets = (machines, prev) => (machines || []).map((m) => {
  const p = (prev || []).find((x) => x.id === m.id);
  return p && p.passphrase ? { ...m, passphrase: p.passphrase } : { ...m, passphrase: undefined };
});
ipcMain.handle('config:get', () => publicConfig());
// A fresh renderer (first load or reload) owns no sessions: drop any left over. Remote tmux sessions survive.
ipcMain.on('app:hello', () => {
  for (const s of sessions.values()) { try { s.close(); } catch {} }
  sessions.clear();
});
ipcMain.handle('config:save', (_e, cfg) => {
  const prev = config.load();
  config.save({ ...cfg, machines: withSecrets(cfg.machines, prev.machines), knownHosts: prev.knownHosts, window: prev.window, tailscale: prev.tailscale });
  // Drop connections whose address changed so the next use reconnects with new settings.
  for (const [id, m] of machines) {
    const a = prev.machines.find((x) => x.id === id), b = cfg.machines.find((x) => x.id === id);
    if (!b || !a || a.host !== b.host || a.user !== b.user || String(a.port) !== String(b.port) || a.keyPath !== b.keyPath) {
      m.disconnect();
      machines.delete(id);
      send('machine:status', id, 'offline', null);
    }
  }
  return publicConfig();
});
// Merge top-level keys (workspaces, lastSession, recentDirs…) without touching the rest. Only these keys: patching
// knownHosts (pinned SSH host keys), machines or presets (commands) must go through config:save, which keeps knownHosts.
const PATCHABLE = new Set(['workspaces', 'lastSession', 'recentDirs', 'collab']);
ipcMain.handle('config:patch', (_e, patch) => {
  const c = config.load();
  for (const k of Object.keys(patch || {})) if (PATCHABLE.has(k)) c[k] = patch[k];
  config.save(c);
  return true;
});
ipcMain.handle('app:open-config', () => shell.openPath(config.file()));
ipcMain.handle('app:info', () => ({ version: app.getVersion(), packaged: app.isPackaged }));
ipcMain.on('app:devtools', () => win && win.webContents.toggleDevTools());
ipcMain.on('app:reload', () => win && win.webContents.reloadIgnoringCache());
// Electron 44's clipboard is asynchronous (W3C-style read()/readText()/writeText()); readImage() is gone.
ipcMain.handle('clipboard:read', () => clipboard.readText());
ipcMain.on('clipboard:write', (_e, text) => { Promise.resolve(clipboard.writeText(String(text).slice(0, 4 * 1024 * 1024))).catch(() => {}); });
// The first image on the clipboard (a Win+Shift+S / PrtScn screenshot, a copied image) as { data, ext }, or null.
async function clipboardImage() {
  for (const item of await clipboard.read()) {
    const type = item.types.find((t) => t === 'image/png') || item.types.find((t) => /^image\/(jpeg|gif|webp|bmp)$/.test(t));
    if (type) return { data: Buffer.from(await (await item.getType(type)).arrayBuffer()), ext: type === 'image/jpeg' ? 'jpg' : type.slice(6) };
  }
  return null;
}
ipcMain.on('app:notify', (_e, { title, body, sessionId }) => {
  if (!Notification.isSupported()) return;
  const n = new Notification({ title: String(title || '').slice(0, 120), body: String(body || '').slice(0, 300), icon: ICON });
  n.on('click', () => {
    if (!win) return;
    if (win.isMinimized()) win.restore();
    win.show();
    win.focus();
    send('notify:click', sessionId);
  });
  n.show();
});
ipcMain.on('app:flash', () => { if (win && !win.isFocused()) win.flashFrame(true); });

// ---- IPC: machines ----
const safe = (fn) => async (...a) => { try { return { ok: true, ...(await fn(...a)) }; } catch (err) { return { ok: false, error: err.message }; } };
ipcMain.handle('local:probe', () => local.probe());
ipcMain.handle('machine:connect', safe(async (_e, id) => { await getMachine(id).connect(); }));
ipcMain.handle('machine:disconnect', (_e, id) => { getMachine(id).disconnect(); return true; });
ipcMain.handle('machine:probe', safe((_e, id) => {
  const m = machineCfg(id);
  return m && m.type === 'wsl' ? local.wslProbe(m.distro) : getMachine(id).probe();
}));
ipcMain.handle('machine:stats', safe(async (_e, id) => ({ stats: await getMachine(id).stats() })));
ipcMain.handle('machine:dirs', safe((_e, id) => {
  const m = machineCfg(id);
  return m && m.type === 'wsl' ? local.wslDirs(m.distro) : getMachine(id).listDirs();
}));
ipcMain.handle('machine:setup-alerts', safe(async (_e, id) => ({ added: await getMachine(id).setupAlerts() })));
ipcMain.handle('machine:kill-tmux', async (_e, id, name) => { await getMachine(id).killTmux(name); return true; });
ipcMain.handle('machine:upload-bootstrap', safe(async (_e, id) => {
  const content = fs.readFileSync(path.join(__dirname, 'scripts', 'linux-bootstrap.sh'), 'utf8').replace(/\r\n/g, '\n');
  await getMachine(id).upload('~/.nexus/bootstrap.sh', content);
}));
ipcMain.handle('local:dirs', safe(async () => {
  // Folders under the user profile that look like projects (contain .git), plus top-level folders.
  const home = os.homedir();
  const repos = [], dirs = [];
  const skip = new Set(['node_modules', 'AppData', '.git', '.cache', 'Library', 'Applications', 'snap']);
  const walk = (dir, depth) => {
    let entries;
    try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
    if (entries.some((e) => e.name === '.git')) repos.push(dir);
    if (depth >= 3 || repos.length > 300) return;
    for (const e of entries) {
      if (!e.isDirectory() || e.name.startsWith('.') || skip.has(e.name)) continue;
      const p = path.join(dir, e.name);
      if (depth < 2) dirs.push(p);
      walk(p, depth + 1);
    }
  };
  walk(home, 0);
  const tilde = (p) => '~' + p.slice(home.length);
  return { repos: repos.filter((p) => p !== home).map(tilde), dirs: dirs.map(tilde) };
}));

// ---- IPC: Tailscale (sharing SSH machines with guests) ----
// The API token is encrypted with the OS (DPAPI via safeStorage) and never leaves the main process.
// On Linux without a keyring, safeStorage "encrypts" with a hard-coded key (backend basic_text): not a secret.
function secureStorageOk() {
  if (!safeStorage.isEncryptionAvailable()) return false;
  try { return process.platform !== 'linux' || safeStorage.getSelectedStorageBackend() !== 'basic_text'; } catch { return true; }
}
function tsToken() {
  const enc = (config.load().tailscale || {}).token;
  if (!enc || !secureStorageOk()) return null;
  try { return safeStorage.decryptString(Buffer.from(enc, 'base64')); } catch { return null; }
}
function tsApi() {
  const t = tsToken();
  if (!t) throw new Error('Add a Tailscale API access token first');
  return new tailscale.TailscaleApi(t);
}
const matchDevices = (devices, machines, self) => tailscale.matchDevices(devices, machines, self);
const selfName = async () => { try { const l = await tailscale.localStatus(); return (l.self && l.self.dnsName) || ''; } catch { return ''; } };
// The last previewed plan. Applying uses this copy, not whatever the renderer sends back, so only a plan the user saw
// (and which Nexus computed) can change the tailnet.
let lastPlan = null;

ipcMain.handle('ts:status', async () => ({
  local: await tailscale.localStatus(),
  hasToken: !!tsToken(),
  canEncrypt: secureStorageOk(),
}));
ipcMain.handle('ts:set-token', safe(async (_e, token) => {
  const t = String(token || '').trim();
  if (!/^tskey-api-/.test(t)) throw new Error('That does not look like an API access token (it starts with tskey-api-)');
  const devices = await new tailscale.TailscaleApi(t).devices(); // validates the token
  if (!secureStorageOk()) throw new Error('Secure storage (Windows DPAPI, macOS Keychain or a Linux keyring such as gnome-keyring/KWallet) is unavailable; the token was not saved');
  const c = config.load();
  c.tailscale = { ...(c.tailscale || {}), token: safeStorage.encryptString(t).toString('base64') };
  config.save(c);
  return { devices: devices.length };
}));
ipcMain.handle('ts:clear-token', () => {
  const c = config.load();
  if (c.tailscale) delete c.tailscale.token;
  config.save(c);
  return true;
});

// Everything the sharing panel shows: shared devices, guests (policy group), their join state, pending invites.
ipcMain.handle('ts:overview', safe(async (_e, machines) => {
  const api = tsApi();
  const [devices, users, invites, { policy }] = await Promise.all([api.devices(), api.users(), api.invites(), api.getPolicy()]);
  const guests = tailscale.guestsIn(policy).map((email) => {
    const u = users.find((x) => (x.loginName || '').toLowerCase() === email);
    const inv = invites.find((x) => (x.email || '').toLowerCase() === email);
    return { email, userId: u ? u.id : null, status: u ? (u.status || 'active') : inv ? 'invited' : 'not joined', inviteId: inv ? inv.id : null, inviteUrl: inv ? inv.inviteUrl : null, lastSeen: u ? u.lastSeen : null };
  });
  return { sparks: matchDevices(devices, machines, await selfName()), guests, tag: tailscale.SPARK_TAG };
}));

// Preview the policy + tag changes needed to share the machines (no changes made).
ipcMain.handle('ts:plan', safe(async (_e, { machines, sshUsers, addGuest, removeGuest }) => {
  const api = tsApi();
  const [{ policy, etag }, devices] = await Promise.all([api.getPolicy(), api.devices()]);
  let guests = tailscale.guestsIn(policy);
  if (addGuest) guests = [...guests, addGuest.toLowerCase()];
  if (removeGuest) guests = guests.filter((g) => g !== removeGuest.toLowerCase());
  const plan = tailscale.planPolicy(policy, { guests, sshUsers });
  const sparks = matchDevices(devices, machines, await selfName());
  const tagChanges = sparks.filter((s) => s.device && !s.device.tags.includes(tailscale.SPARK_TAG))
    .map((s) => ({ deviceId: s.device.id, hostname: s.hostname, device: s.device.name, tags: [...s.device.tags, tailscale.SPARK_TAG] }));
  // Name the exact tailnet device being tagged (tagging gives guests SSH to it), not just the hostname it reports.
  const changes = [...plan.changes, ...tagChanges.map((t) => `Tag ${t.device || t.hostname} as ${tailscale.SPARK_TAG} (guests get SSH to it)`)];
  const missing = sparks.filter((s) => !s.device).map((s) => s.hostname + (s.ambiguous ? ' (several devices use this hostname: set its Tailscale address in Settings)' : ''));
  lastPlan = { policy: plan.policy, etag, tagChanges };
  return { policy: plan.policy, etag, changes, tagChanges, missing };
}));

// Apply a previewed plan: validate the policy with Tailscale, save it (only if unchanged since the preview), tag devices.
ipcMain.handle('ts:apply', safe(async (_e, req) => {
  if (!lastPlan || !req || req.etag !== lastPlan.etag) throw new Error('Review the access rules again before applying');
  const { policy, etag, tagChanges } = lastPlan;
  lastPlan = null;
  const api = tsApi();
  const v = await api.validatePolicy(policy);
  if (v && v.message) throw new Error(`Tailscale rejected the policy: ${v.message}${v.data ? ' ' + JSON.stringify(v.data) : ''}`);
  await api.setPolicy(policy, etag);
  for (const t of tagChanges || []) await api.setTags(t.deviceId, t.tags);
  return {};
}));

ipcMain.handle('ts:invite', safe(async (_e, email) => {
  const inv = await tsApi().createInvite(email);
  return { inviteUrl: inv && inv.inviteUrl, inviteId: inv && inv.id };
}));
ipcMain.handle('ts:revoke', safe(async (_e, { userId, inviteId }) => {
  const api = tsApi();
  if (inviteId) await api.deleteInvite(inviteId);
  if (userId) await api.deleteUser(userId);
  return {};
}));
ipcMain.handle('ts:encode-invite', (_e, data) => tailscale.encodeInvite(data));
ipcMain.handle('ts:decode-invite', safe(async (_e, code) => {
  let local = null;
  try { local = (await tailscale.localStatus()).tailnet || null; } catch { /* no local client: skip the check */ }
  return tailscale.decodeInvite(code, local);
}));

// ---- IPC: team chat + presence (stored on the hub machine, over the existing SSH connection) ----
let hub = null; // { machineId, ch: CollabHub }
function stopHub() {
  if (hub) { hub.ch.removeAllListeners(); hub.ch.stop(); }
  hub = null;
}
async function startHub(machineId) {
  stopHub();
  const mach = getMachine(machineId);
  const ch = new collab.CollabHub(mach);
  hub = { machineId, ch };
  ch.on('message', (m) => send('collab:message', m));
  ch.on('closed', () => send('collab:state', { machineId, live: false }));
  await ch.start();
  send('collab:state', { machineId, live: true });
}
ipcMain.handle('collab:start', safe(async (_e, machineId) => { await startHub(machineId); }));
ipcMain.handle('collab:stop', () => { stopHub(); return true; });
ipcMain.handle('collab:post', safe(async (_e, { from, text, kind }) => {
  if (!hub) throw new Error('Team chat is not connected');
  const m = collab.makeMessage({ from, text, kind });
  if (!m.text) throw new Error('Empty message');
  await hub.ch.post(m);
  return { id: m.id };
}));
ipcMain.handle('collab:heartbeat', safe(async (_e, presence) => {
  if (!hub) return { presence: [] };
  return { presence: await hub.ch.heartbeat(collab.makePresence(presence)) };
}));

// ---- IPC: sessions ----
ipcMain.handle('session:create', async (_e, spec) => {
  const cfg = config.load();
  const m = cfg.machines.find((x) => x.id === spec.machineId);
  if (!m) return { ok: false, error: 'Unknown machine' };
  const preset = cfg.presets.find((p) => p.id === spec.presetId) || { id: 'shell', cmd: {} };
  const pc = preset.cmd || {};
  const cmd = spec.cmd != null ? spec.cmd : (pc[m.os] != null ? pc[m.os] : m.os === 'mac' ? (pc.linux || '') : '');
  const cols = Math.max(20, spec.cols || 120), rows = Math.max(5, spec.rows || 30);
  const id = `s${nextId++}`;

  try {
    if (m.type === 'local' || m.type === 'wsl') {
      let script = null;
      if (spec.script === 'setup-ssh-key') {
        const target = machineCfg(spec.scriptMachine);
        script = {
          ...(platform.IS_WIN
            ? { file: path.join(SCRIPTS_DIR, 'setup-ssh-key.ps1'), args: ['-HostName', target.host, '-User', target.user || '', '-Port', String(target.port || 22)] }
            : { file: path.join(SCRIPTS_DIR, 'setup-ssh-key.sh'), args: [target.host, target.user || '', String(target.port || 22)] }),
        };
      }
      let cwd = m.type === 'wsl' ? spec.cwd : expandLocal(spec.cwd); // WSL paths are resolved inside the distro
      if (cwd && m.type !== 'wsl' && !fs.existsSync(cwd)) cwd = undefined;
      const p = local.spawn({ cmd, cols, rows, script, cwd, wsl: m.type === 'wsl' ? m.distro : null });
      sessions.set(id, {
        machineId: m.id,
        write: (d) => p.write(d),
        resize: (c, r) => { try { p.resize(c, r); } catch {} },
        close: () => { try { p.kill(); } catch {} },
      });
      p.onData((d) => emitData(id, d));
      p.onExit(({ exitCode }) => {
        flush();
        if (!sessions.has(id)) return;
        sessions.delete(id);
        send('session:exit', id, { code: exitCode });
      });
      return { ok: true, id };
    }

    const mach = getMachine(m.id);
    const tmuxName = spec.raw ? null : (spec.attach || `nx-${preset.id}-${Date.now().toString(36).slice(-4)}${Math.random().toString(36).slice(2, 4)}`);
    const stream = await mach.open({ tmuxName, cmd, cols, rows, cwd: spec.cwd, readOnly: !!spec.readOnly, record: !!(cfg.appearance || {}).recordSessions });
    const dec = new StringDecoder('utf8');
    let exitCode = null;
    sessions.set(id, {
      machineId: m.id,
      tmuxName,
      write: (d) => stream.write(d),
      resize: (c, r) => { try { stream.setWindow(r, c, 0, 0); } catch {} },
      close: () => { try { stream.close(); } catch {} },
    });
    stream.on('data', (b) => emitData(id, dec.write(b)));
    stream.stderr.on('data', (b) => emitData(id, b.toString('utf8')));
    stream.on('exit', (code) => { exitCode = code; });
    stream.on('close', () => {
      flush();
      if (!sessions.has(id)) return;
      sessions.delete(id);
      send('session:exit', id, { code: exitCode, disconnected: exitCode === null });
    });
    return { ok: true, id, tmuxName };
  } catch (err) {
    return { ok: false, error: err.message };
  }
});

ipcMain.on('session:write', (_e, id, data) => { const s = sessions.get(id); if (s) s.write(data); });
ipcMain.on('session:resize', (_e, id, cols, rows) => { const s = sessions.get(id); if (s) s.resize(cols, rows); });
ipcMain.handle('session:close', async (_e, id, opts = {}) => {
  const s = sessions.get(id);
  if (!s) return true;
  sessions.delete(id);
  s.close();
  if (opts.kill && s.tmuxName) await getMachine(s.machineId).killTmux(s.tmuxName);
  return true;
});
ipcMain.handle('session:cwd', async (_e, id) => {
  const s = sessions.get(id);
  if (!s || !s.tmuxName) return null;
  try { return await getMachine(s.machineId).paneCwd(s.tmuxName); } catch { return null; }
});

// Pasted screenshots can hold anything on screen: keep them private to this user and delete them after 7 days.
const pasteDir = () => path.join(app.getPath('temp'), 'nexus-paste');
function cleanPasteDir() {
  try {
    const cutoff = Date.now() - 7 * 24 * 3600 * 1000;
    for (const f of fs.readdirSync(pasteDir())) {
      const p = path.join(pasteDir(), f);
      try { if (fs.statSync(p).mtimeMs < cutoff) fs.unlinkSync(p); } catch { /* in use or gone */ }
    }
  } catch { /* no folder yet */ }
}

// Clipboard image -> a file the session's machine can read; returns its path for pasting.
ipcMain.handle('session:paste-image', safe(async (_e, id) => {
  const s = sessions.get(id);
  if (!s) throw new Error('Session is not running');
  const img = await clipboardImage();
  if (!img || !img.data.length) return { path: null };
  const png = img.data;
  const name = `paste-${new Date().toISOString().replace(/[:.]/g, '-')}.${img.ext}`;
  const type = machineCfg(s.machineId).type;
  if (type === 'local' || type === 'wsl') {
    const dir = pasteDir();
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const p = path.join(dir, name);
    fs.writeFileSync(p, png, { mode: 0o600 });
    return { path: type === 'wsl' ? platform.toWslPath(p) : p };
  }
  return { path: await getMachine(s.machineId).uploadToInbox(name, { buffer: png }) };
}));

// Dropped files -> paths on the session's machine (uploaded to ~/.nexus/uploads for remotes).
ipcMain.handle('session:upload-files', safe(async (_e, id, files) => {
  const s = sessions.get(id);
  if (!s) throw new Error('Session is not running');
  const type = machineCfg(s.machineId).type;
  if (!Array.isArray(files) || !files.length || files.length > 50) throw new Error('Drop between 1 and 50 files');
  // Check every file before uploading any, so a folder in the drop doesn't leave half the files uploaded.
  for (const f of files) {
    const st = fs.statSync(f);
    if (st.isDirectory()) throw new Error(`${path.basename(f)} is a folder; drop files instead`);
    if (type === 'ssh' && st.size > 512 * 1024 * 1024) throw new Error(`${path.basename(f)} is larger than 512 MB`);
  }
  const out = [];
  for (const f of files) {
    if (type === 'local') out.push(f);
    else if (type === 'wsl') out.push(platform.toWslPath(f)); // WSL sees Windows drives under /mnt
    else out.push(await getMachine(s.machineId).uploadToInbox(path.basename(f), { localPath: f }));
  }
  return { paths: out };
}));
