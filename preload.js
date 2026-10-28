const { contextBridge, ipcRenderer, webUtils } = require('electron');

// Paths that came out of a real drag-and-drop (webUtils.getPathForFile) in this renderer. upload-files only accepts
// these, so a compromised page can't name an arbitrary local file (~/.ssh/id_ed25519) to ship to a remote machine.
const dropped = new Set();
const pathForFile = (file) => { const p = webUtils.getPathForFile(file); if (p) dropped.add(p); return p; };

const on = (ch) => (fn) => {
  const h = (_e, ...a) => fn(...a);
  ipcRenderer.on(ch, h);
  return () => ipcRenderer.removeListener(ch, h);
};

contextBridge.exposeInMainWorld('nexus', {
  platform: process.platform,
  hello: () => ipcRenderer.send('app:hello'),
  getConfig: () => ipcRenderer.invoke('config:get'),
  saveConfig: (cfg) => ipcRenderer.invoke('config:save', cfg),
  patchConfig: (patch) => ipcRenderer.invoke('config:patch', patch),
  openConfig: () => ipcRenderer.invoke('app:open-config'),
  appInfo: () => ipcRenderer.invoke('app:info'),
  devtools: () => ipcRenderer.send('app:devtools'),
  reload: () => ipcRenderer.send('app:reload'),
  notify: (n) => ipcRenderer.send('app:notify', n),
  flash: () => ipcRenderer.send('app:flash'),
  readClipboard: () => ipcRenderer.invoke('clipboard:read'),
  writeClipboard: (t) => ipcRenderer.send('clipboard:write', t),
  pathForFile,
  downloadUpdate: () => ipcRenderer.send('update:download'),
  installUpdate: () => ipcRenderer.send('update:install'),
  answerPassphrase: (id, value) => ipcRenderer.send('auth:answer', id, value),
  answerHost: (id, ok) => ipcRenderer.send('auth:host-answer', id, ok === true),

  probeLocal: () => ipcRenderer.invoke('local:probe'),
  localDirs: () => ipcRenderer.invoke('local:dirs'),
  connect: (id) => ipcRenderer.invoke('machine:connect', id),
  disconnect: (id) => ipcRenderer.invoke('machine:disconnect', id),
  probe: (id) => ipcRenderer.invoke('machine:probe', id),
  stats: (id) => ipcRenderer.invoke('machine:stats', id),
  dirs: (id) => ipcRenderer.invoke('machine:dirs', id),
  setupAlerts: (id) => ipcRenderer.invoke('machine:setup-alerts', id),
  killTmux: (id, name) => ipcRenderer.invoke('machine:kill-tmux', id, name),
  uploadBootstrap: (id) => ipcRenderer.invoke('machine:upload-bootstrap', id),

  create: (spec) => ipcRenderer.invoke('session:create', spec),
  write: (id, data) => ipcRenderer.send('session:write', id, data),
  resize: (id, cols, rows) => ipcRenderer.send('session:resize', id, cols, rows),
  close: (id, opts) => ipcRenderer.invoke('session:close', id, opts),
  cwd: (id) => ipcRenderer.invoke('session:cwd', id),
  pasteImage: (id) => ipcRenderer.invoke('session:paste-image', id),
  uploadFiles: (id, paths) => ipcRenderer.invoke('session:upload-files', id, (paths || []).filter((p) => dropped.has(p))),

  tsStatus: () => ipcRenderer.invoke('ts:status'),
  tsSetToken: (t) => ipcRenderer.invoke('ts:set-token', t),
  tsClearToken: () => ipcRenderer.invoke('ts:clear-token'),
  tsOverview: (machines) => ipcRenderer.invoke('ts:overview', machines),
  tsPlan: (args) => ipcRenderer.invoke('ts:plan', args),
  tsApply: (plan) => ipcRenderer.invoke('ts:apply', plan),
  tsInvite: (email) => ipcRenderer.invoke('ts:invite', email),
  tsRevoke: (args) => ipcRenderer.invoke('ts:revoke', args),
  tsEncodeInvite: (data) => ipcRenderer.invoke('ts:encode-invite', data),
  tsDecodeInvite: (code) => ipcRenderer.invoke('ts:decode-invite', code),

  collabStart: (machineId) => ipcRenderer.invoke('collab:start', machineId),
  collabStop: () => ipcRenderer.invoke('collab:stop'),
  collabPost: (msg) => ipcRenderer.invoke('collab:post', msg),
  collabHeartbeat: (presence) => ipcRenderer.invoke('collab:heartbeat', presence),
  onCollabMessage: on('collab:message'),
  onCollabState: on('collab:state'),

  onData: on('session:data'),
  onExit: on('session:exit'),
  onMachineStatus: on('machine:status'),
  onNotifyClick: on('notify:click'),
  onFocus: on('app:focus'),
  onAuthAsk: on('auth:ask'),
  onHostAsk: on('auth:host'),
  onUpdateReady: on('update:ready'),
  onUpdateAvailable: on('update:available'),
});
