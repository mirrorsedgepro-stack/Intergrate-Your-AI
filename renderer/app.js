'use strict';
(() => {
  const api = window.nexus;
  const { fuzzy, mentions, isExeTitle } = window.NexusUtil;
  const IS_MAC = api.platform === 'darwin';
  const IS_WIN = api.platform === 'win32';
  document.body.classList.add(`platform-${api.platform}`);
  // App shortcuts use Cmd on macOS (Ctrl stays with the terminal there: Ctrl+C interrupts) and Ctrl elsewhere.
  const modKey = (e) => (IS_MAC ? e.metaKey : e.ctrlKey);
  const K = (s) => (IS_MAC ? String(s).replace(/Ctrl\+(?!Tab|`)/g, '⌘').replace(/Alt\+/g, '⌥') : s);
  const OS_LABEL = { windows: 'Windows', mac: 'macOS', linux: 'Linux' };
  // On macOS show ⌘/⌥ in every key hint and tooltip, including ones rendered later.
  if (IS_MAC) {
    const fix = (root) => {
      if (!root.querySelectorAll) return;
      for (const el of [root, ...root.querySelectorAll('kbd, [title]')]) {
        if (el.tagName === 'KBD') { if (el.textContent === 'Ctrl') el.textContent = '⌘'; else if (el.textContent === 'Alt') el.textContent = '⌥'; else el.textContent = K(el.textContent); }
        if (el.title && /Ctrl|Alt/.test(el.title)) el.title = K(el.title);
      }
    };
    fix(document.body);
    new MutationObserver((muts) => { for (const m of muts) for (const n of m.addedNodes) if (n.nodeType === 1) fix(n); })
      .observe(document.body, { childList: true, subtree: true });
  }
  const $ = (s, r = document) => r.querySelector(s);
  const esc = (v) => String(v ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const now = () => Date.now();

  // ---------------------------------------------------------------- icons
  const I = {
    claude: '<svg viewBox="0 0 24 24" style="stroke-width:2.4"><path d="M12 3.5v17M3.5 12h17M6 6l12 12M18 6L6 18"/></svg>',
    antigravity: '<img class="logo" src="../assets/google-antigravity.png" alt="" draggable="false">', // selfh.st/icons, CC BY 4.0
    gemini: '<svg viewBox="0 0 24 24"><path class="fill" d="M12 2c.7 5.3 4.7 9.3 10 10-5.3.7-9.3 4.7-10 10-.7-5.3-4.7-9.3-10-10 5.3-.7 9.3-4.7 10-10z"/></svg>',
    shell: '<svg viewBox="0 0 24 24" style="stroke-width:2.2"><path d="M5 7l5 5-5 5M12.5 18H19"/></svg>',
    gitbash: '<svg viewBox="0 0 24 24"><circle cx="6.5" cy="5.5" r="2.2"/><circle cx="6.5" cy="18.5" r="2.2"/><circle cx="17.5" cy="8" r="2.2"/><path d="M6.5 7.7v8.6M17.5 10.2c0 4.3-7 3.6-10 6.3"/></svg>',
    monitor: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="12.5" rx="2.5"/><path d="M8.5 20.5h7M12 16.5v4"/></svg>',
    server: '<svg viewBox="0 0 24 24"><rect x="3.5" y="3.5" width="17" height="7" rx="2.2"/><rect x="3.5" y="13.5" width="17" height="7" rx="2.2"/><path d="M7.5 7h.01M7.5 17h.01M11 7h5M11 17h5"/></svg>',
    x: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>',
    refresh: '<svg viewBox="0 0 24 24"><path d="M20 11a8 8 0 0 0-14.6-4.5M4 4v4h4M4 13a8 8 0 0 0 14.6 4.5M20 20v-4h-4"/></svg>',
    plug: '<svg viewBox="0 0 24 24"><path d="M9 2v5M15 2v5M6 7h12v4a6 6 0 0 1-12 0zM12 17v5"/></svg>',
    more: '<svg viewBox="0 0 24 24"><circle class="fill" cx="5" cy="12" r="1.6"/><circle class="fill" cx="12" cy="12" r="1.6"/><circle class="fill" cx="19" cy="12" r="1.6"/></svg>',
    search: '<svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6.5"/><path d="M16 16l4 4"/></svg>',
    up: '<svg viewBox="0 0 24 24"><path d="M6 15l6-6 6 6"/></svg>',
    down: '<svg viewBox="0 0 24 24"><path d="M6 9l6 6 6-6"/></svg>',
    split: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M12 4v16"/></svg>',
    send: '<svg viewBox="0 0 24 24"><path d="M4 20l16-8L4 4v6l10 2-10 2z"/></svg>',
    gear: '<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/></svg>',
    key: '<svg viewBox="0 0 24 24"><circle cx="7.5" cy="15.5" r="4"/><path d="M10.5 12.5L20 3M16 7l3 3M14 9l2 2"/></svg>',
    box: '<svg viewBox="0 0 24 24"><path d="M21 8l-9-5-9 5 9 5 9-5zM3 8v8l9 5 9-5V8M12 13v8"/></svg>',
    layout: '<svg viewBox="0 0 24 24"><rect x="3" y="3" width="18" height="18" rx="3"/><path d="M12 3v18M3 12h9"/></svg>',
    folder: '<svg viewBox="0 0 24 24"><path d="M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2.5h7.5A2.5 2.5 0 0 1 21 10v7.5a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z"/></svg>',
    bell: '<svg viewBox="0 0 24 24"><path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0"/></svg>',
    save: '<svg viewBox="0 0 24 24"><path d="M5 4h11l3 3v13H5zM8 4v5h7V4M8 20v-6h8v6"/></svg>',
    trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>',
    image: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="9" cy="10" r="2"/><path d="M21 16l-5-5-9 9"/></svg>',
  };

  // ---------------------------------------------------------------- state
  const S = {
    cfg: null,
    mstate: {},           // machineId -> { status, error, tools, detached, hostname }
    tunnels: [],          // active port forwards: { id, machineId, localPort, remoteHost, remotePort }
    sessions: new Map(),  // local id -> session
    byPid: new Map(),     // main-process id -> session
    early: new Map(),     // data that arrived before we mapped the pid
    order: [],
    mru: [],
    layout: 1,
    panes: [null],
    focus: 0,
    paneEls: [],
    winFocused: true,
    targets: new Set(),
    autoReattached: {},
    draft: null,
    split: { x: 0.5, y: 0.5 }, // pane divider positions (fractions)
    restoring: false,
    palMode: null,             // custom palette source (folder picker, workspaces…)
    dirCache: {},              // machineId -> { at, repos, dirs }
    manualDisconnect: {},
    machineRetry: {},
  };
  let uid = 0;
  const baseName = (p) => String(p || '').replace(/[\\/]+$/, '').split(/[\\/]/).pop() || p;
  const shortPath = (p) => (p && p.length > 34 ? '…' + p.slice(-33) : p || '');

  const presetById = (id) => S.cfg.presets.find((p) => p.id === id) || { id, name: id, color: '#9aa3ba', cmd: {} };
  const machineById = (id) => S.cfg.machines.find((m) => m.id === id) || { id, name: id, type: 'local', color: '#646d85' };
  const presetsFor = (m) => S.cfg.presets.filter((p) => !p.only || p.only === m.os);
  const ordered = () => S.order.map((id) => S.sessions.get(id)).filter(Boolean);
  const focused = () => S.sessions.get(S.panes[S.focus]);
  const isVisible = (s) => S.panes.includes(s.id);
  // Brand logos (assets/icons; credits in README). Presets/machines may name one via an `icon` field.
  const LOGO = {
    claude: 'claude.svg', antigravity: 'google-antigravity.png', openclaw: 'openclaw.svg', gitbash: 'git.svg', git: 'git.svg', apple: 'apple.svg', ubuntu: 'ubuntu.svg',
    powershell: 'powershell.svg', terminal: 'terminal.svg', windows: 'windows.svg', linux: 'linux.svg', nvidia: 'nvidia.svg',
  };
  const logo = (k) => `<img class="logo" src="../assets/icons/${LOGO[k]}" alt="" draggable="false">`;
  function presetIcon(presetId, machineId) {
    const key = presetById(presetId).icon || presetId;
    if (key === 'shell') return logo(machineId && machineById(machineId).os === 'windows' ? 'powershell' : 'terminal');
    const own = (o) => Object.prototype.hasOwnProperty.call(o, key); // ids can come from other people's presence
    return own(LOGO) ? logo(key) : own(I) ? I[key] : I.shell;
  }
  const glyph = (presetId, cls = '', machineId = null) => `<span class="glyph ${cls}" style="--c:${presetById(presetId).color}">${presetIcon(presetId, machineId)}</span>`;
  function machineIconKey(m) {
    if (m.icon && LOGO[m.icon]) return m.icon;
    if (m.type === 'local') return m.os === 'mac' ? 'apple' : m.os === 'linux' ? 'linux' : 'windows';
    if (m.type === 'wsl') return /ubuntu/i.test(m.distro || '') ? 'ubuntu' : 'linux';
    // NVIDIA GPU machines (detected by nvidia-smi, or named like a DGX / Spark) get the NVIDIA logo.
    const st = S.mstate[m.id] || {};
    return (st.tools && st.tools['nvidia-smi']) || /spark|dgx/i.test(`${st.hostname || ''} ${m.name}`) ? 'nvidia' : 'linux';
  }
  const mIcon = (m) => logo(machineIconKey(m));
  const presetFromTmux = (name) => { const m = /^nx-([a-z0-9]+)-/.exec(name); return m ? m[1] : 'shell'; };
  const AGENTS = new Set(['claude', 'antigravity', 'openclaw']);
  const isAgent = (s) => AGENTS.has(s.presetId);
  // Executable a preset launches on a machine (keys of the probe's tools map); '' = plain shell, always present.
  const presetCmd = (m, p) => { const c = p.cmd || {}; return c[m.os] != null ? c[m.os] : m.os === 'mac' ? (c.linux || '') : ''; };
  const toolOf = (m, p) => (p.id === 'gitbash' ? 'gitbash' : String(presetCmd(m, p)).split(/\s+/)[0]);
  const isMissing = (m, p) => { const t = toolOf(m, p); return !!t && ((S.mstate[m.id] || {}).tools || {})[t] === false; };

  // ---------------------------------------------------------------- terminal theme
  const THEME = {
    background: '#0b0f18', foreground: '#c8d3f5',
    selectionBackground: 'rgba(122,162,247,0.32)', selectionInactiveBackground: 'rgba(122,162,247,0.16)',
    black: '#1d2233', red: '#f7768e', green: '#9ece6a', yellow: '#e0af68', blue: '#7aa2f7', magenta: '#bb9af7', cyan: '#7dcfff', white: '#a9b1d6',
    brightBlack: '#4e5675', brightRed: '#ff8fa3', brightGreen: '#b9f27c', brightYellow: '#ffc777', brightBlue: '#8db0ff', brightMagenta: '#c9aaff', brightCyan: '#a4e4ff', brightWhite: '#e6ebff',
    scrollbarSliderBackground: 'rgba(148,163,214,0.14)', scrollbarSliderHoverBackground: 'rgba(148,163,214,0.28)', scrollbarSliderActiveBackground: 'rgba(148,163,214,0.36)',
  };

  function termOptions(color, machine) {
    const a = S.cfg.appearance;
    const o = {
      fontFamily: a.fontFamily, fontSize: a.fontSize, lineHeight: a.lineHeight, letterSpacing: 0,
      cursorBlink: true, cursorStyle: 'bar', cursorWidth: 2, cursorInactiveStyle: 'outline',
      allowProposedApi: true, scrollback: 20000, smoothScrollDuration: 60, fastScrollSensitivity: 5,
      drawBoldTextInBrightColors: false, minimumContrastRatio: 1, rescaleOverlappingGlyphs: true,
      theme: { ...THEME, cursor: color, cursorAccent: '#0b0f18' },
    };
    if (IS_WIN && (machine.type === 'local' || machine.type === 'wsl')) o.windowsPty = { backend: 'conpty', buildNumber: 26100 };
    return o;
  }

  // ---------------------------------------------------------------- sessions
  function makeSession(machineId, presetId, extra = {}) {
    const p = presetById(presetId);
    const id = `L${++uid}`;
    const same = ordered().filter((x) => x.machineId === machineId && x.presetId === presetId).length;
    const host = document.createElement('div');
    host.className = 'term-host';
    $('#parking').appendChild(host);
    const term = new Terminal(termOptions(p.color, machineById(machineId)));
    // MSYS bash behind ConPTY mangles bracketed-paste markers into literal "[200~".
    if (presetId === 'gitbash') term.options.ignoreBracketedPasteMode = true;
    const fit = new FitAddon.FitAddon();
    const search = new SearchAddon.SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    // Links in terminal output open on Ctrl/Cmd+click only: output comes from remote machines and agents, and a stray
    // click shouldn't open a page someone else chose.
    term.loadAddon(new WebLinksAddon.WebLinksAddon((e, uri) => {
      if (IS_MAC ? e.metaKey : e.ctrlKey) window.open(uri);
      else toast(`${IS_MAC ? '⌘' : 'Ctrl'}+click to open ${uri.length > 60 ? uri.slice(0, 57) + '…' : uri}`, '', 2500);
    }));
    term.loadAddon(new Unicode11Addon.Unicode11Addon());
    term.unicode.activeVersion = '11';
    term.open(host);
    try {
      const gl = new WebglAddon.WebglAddon();
      gl.onContextLoss(() => gl.dispose());
      term.loadAddon(gl);
    } catch { /* DOM renderer fallback */ }

    const s = {
      id, machineId, presetId, color: p.color, term, fit, search, host,
      name: extra.name || (extra.cwd && extra.cwd !== '~' ? `${p.name} · ${baseName(extra.cwd)}` : same ? `${p.name} ${same + 1}` : p.name),
      tmuxName: extra.attach || null,
      cwd: extra.cwd || null,
      spec: { raw: extra.raw, cmd: extra.cmd, script: extra.script, scriptMachine: extra.scriptMachine, readOnly: extra.readOnly },
      readOnly: !!extra.readOnly, announce: !!extra.announce,
      pid: null, status: 'starting', activity: 'idle', title: '',
      lastData: 0, lastInput: 0, lastPointer: 0, burst: 0, quietUntil: 0, busySince: 0,
      promptLine: null, retry: 0, retryTimer: null,
    };
    wireSession(s);
    S.sessions.set(id, s);
    S.order.push(id);
    return s;
  }

  function wireSession(s) {
    const { term } = s;
    term.onData((d) => {
      s.lastInput = now();
      // Remember where the user's last submitted prompt was, for "Send last reply".
      if (d === '\r') { const b = term.buffer.active; s.promptLine = b.baseY + b.cursorY; }
      if (s.readOnly) return; // watching someone else's session: tmux ignores input anyway
      if (s.status === 'running') api.write(s.pid, d);
      else if (s.status === 'starting') s.pendingInput = (s.pendingInput || '') + d; // sent once connected
      else if ((s.status === 'exited' || s.status === 'disconnected') && d === '\r') { clearTimeout(s.retryTimer); start(s); }
    });
    term.onBinary((d) => { if (s.status === 'running') api.write(s.pid, d); });
    term.onResize(({ cols, rows }) => {
      s.quietUntil = now() + 1200;
      if (s.status === 'running') api.resize(s.pid, cols, rows);
      renderStatus();
    });
    term.onTitleChange((t) => {
      const title = t.replace(/^[\s✳⠂⠐⠈⠁⠄⠠⡀⢀·*]+/, '').trim();
      // PowerShell / cmd under ConPTY title themselves with their executable's path; that's noise next to the name.
      s.title = isExeTitle(title) ? '' : title;
      renderChrome();
    });
    // Bells (and OSC 9 / 777 notifications) are exact "agent is waiting" signals: Claude's Stop/Notification
    // hooks ring one (see "Enable Claude alerts"), so they override the output-volume heuristic.
    const signal = (body) => {
      if (isVisible(s) && S.winFocused) { s.activity = 'idle'; renderChrome(); return; }
      markDone(s, true, body);
      renderChrome();
    };
    term.onBell(() => signal());
    term.parser.registerOscHandler(9, (data) => { if (!/^\d+;/.test(data)) signal(data); return true; });
    term.parser.registerOscHandler(777, (data) => { const [kind, , body] = data.split(';'); if (kind === 'notify') signal(body); return true; });
    // OSC 52 clipboard writes: full-screen apps (Claude Code) copy their own mouse selections this way, through
    // tmux. Writes only; clipboard reads ("?") are ignored so remote programs can't see your clipboard.
    // Accepted only right after you clicked or typed in this pane: otherwise any output (a file being cat'ed, a web
    // page an agent fetched, a hostile remote) could silently swap your clipboard for a command you'd then paste.
    term.parser.registerOscHandler(52, (data) => {
      const b64 = data.slice(data.indexOf(';') + 1);
      if (!b64 || b64 === '?') return true;
      const t = now();
      if (!isVisible(s) || !S.winFocused || (t - s.lastPointer > 3000 && t - s.lastInput > 3000)) {
        toast(`Blocked a clipboard write from ${s.name} (it didn't follow a click or keypress in that pane)`, 'warn', 4000);
        return true;
      }
      if (b64.length > 4 * 1024 * 1024) { toast(`Ignored an oversized clipboard write from ${s.name}`, 'warn'); return true; }
      try {
        const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
        const text = new TextDecoder().decode(bytes);
        if (text) { api.writeClipboard(text); toast(`Copied ${text.length} chars from ${s.name}`, 'ok', 1400); }
      } catch { /* malformed payload */ }
      return true;
    });
    // When the app owns the mouse (Claude's full-screen UI), plain drags go to the app. Shift+drag still
    // selects in Nexus; say so once.
    for (const ev of ['mousedown', 'mouseup']) s.host.addEventListener(ev, () => { s.lastPointer = now(); }, true);
    s.host.addEventListener('mousedown', (e) => {
      if (e.button !== 0 || e.shiftKey || S.mouseTipShown || term.modes.mouseTrackingMode === 'none') return;
      S.mouseTipShown = true;
      toast(`${s.name} is using the mouse: select text in it to copy, or hold Shift and drag to select in Nexus.`, '', 6000);
    }, true);
    term.onSelectionChange(() => {
      if (S.cfg.appearance.copyOnSelect && term.hasSelection()) api.writeClipboard(term.getSelection());
    });
    term.attachCustomKeyEventHandler((e) => termKey(e, s));
    term.textarea.addEventListener('focus', () => {
      const i = S.panes.indexOf(s.id);
      if (i >= 0 && i !== S.focus) { S.focus = i; touchMru(s.id); markPaneFocus(); renderChrome(); }
      clearDone(s);
    });
    s.host.addEventListener('contextmenu', (e) => { e.preventDefault(); termMenu(e, s); });
    // Native paste (macOS Edit menu, middle-click on Linux) with no text: treat it as an image paste.
    term.textarea.addEventListener('paste', (e) => {
      if (e.clipboardData && !e.clipboardData.getData('text/plain')) { e.preventDefault(); pasteInto(s); }
    }, true);
    // Drop files onto a pane: local paths are pasted as-is, remote ones are uploaded first.
    s.host.addEventListener('dragover', (e) => { if (e.dataTransfer.types.includes('Files')) { e.preventDefault(); s.host.classList.add('drop'); } });
    s.host.addEventListener('dragleave', () => s.host.classList.remove('drop'));
    s.host.addEventListener('drop', (e) => {
      e.preventDefault();
      s.host.classList.remove('drop');
      const paths = [...e.dataTransfer.files].map((f) => api.pathForFile(f)).filter(Boolean);
      if (paths.length) attachFiles(s, paths);
    });
  }

  // Quote a path for the session's shell / agent prompt.
  function quotePath(s, p) {
    if (!/[\s'"&()]/.test(p)) return p;
    return machineById(s.machineId).type === 'local' && IS_WIN ? `"${p}"` : `'${p.replace(/'/g, `'\\''`)}'`;
  }

  async function attachFiles(s, paths) {
    if (s.status !== 'running') { toast('Session is not running', 'warn'); return; }
    const remote = machineById(s.machineId).type === 'ssh';
    if (remote) toast(`Uploading ${paths.length} file${paths.length > 1 ? 's' : ''} to ${machineById(s.machineId).name}…`, '', 1800);
    const r = await api.uploadFiles(s.pid, paths);
    if (!r.ok) { toast(r.error, 'bad'); return; }
    s.term.paste(r.paths.map((p) => quotePath(s, p)).join(' ') + ' ');
    s.term.focus();
    if (remote) toast(`Uploaded to ~/.nexus/uploads`, 'ok', 1800);
  }

  async function start(s) {
    const m = machineById(s.machineId);
    s.status = 'starting';
    renderChrome();
    if (m.type === 'ssh' && !s.retry) s.term.write(`\x1b[2m${s.tmuxName ? 'attaching' : 'connecting'} to ${m.name}…\x1b[0m\r\n`);
    const r = await api.create({
      machineId: s.machineId, presetId: s.presetId, cols: s.term.cols, rows: s.term.rows,
      attach: s.tmuxName || undefined, cwd: s.cwd || undefined, ...s.spec,
    });
    if (!S.sessions.has(s.id)) { if (r.ok) api.close(r.id); return; }
    if (!r.ok) {
      // A remote session that dropped keeps retrying quietly; anything else waits for Enter.
      if (s.retry) { s.status = 'disconnected'; scheduleReconnect(s); renderChrome(); return; }
      s.status = 'exited';
      s.term.write(`\r\n\x1b[38;2;247;118;142m✖ ${r.error}\x1b[0m\r\n\x1b[2mEnter to retry · Ctrl+Shift+W to close\x1b[0m\r\n`);
      renderChrome();
      return;
    }
    if (s.retry) s.term.write('\x1b[38;2;115;218;202m✓ Reconnected\x1b[0m\r\n');
    if (s.announce) { s.announce = false; setTimeout(() => announceLaunch(s), 0); }
    s.retry = 0;
    s.pid = r.id;
    if (r.tmuxName) s.tmuxName = r.tmuxName;
    s.status = 'running';
    S.byPid.set(r.id, s);
    const early = S.early.get(r.id);
    if (early) { S.early.delete(r.id); s.term.write(early); }
    if (s.pendingInput) { const p = s.pendingInput; s.pendingInput = ''; setTimeout(() => api.write(s.pid, p), 400); }
    s.quietUntil = now() + 2500;
    renderChrome();
  }

  async function launch(machineId, presetId, extra = {}) {
    const m = machineById(machineId);
    if (m.type === 'ssh' && !m.host) { openSettings(); toast(`Add the address of ${m.name} first.`); return null; }
    if (isMissing(m, presetById(presetId)) && !extra.raw && !extra.script) toast(installHint(m, presetId), 'warn');
    if (!extra.raw && !extra.script && !extra.attach && !(await checkFolderConflict(machineId, presetId, extra.cwd))) return null;
    const s = makeSession(machineId, presetId, { announce: !extra.raw && !extra.script && !extra.attach, ...extra });
    show(s.id);
    await frame();
    try { s.fit.fit(); } catch {}
    start(s);
    return s;
  }

  async function reattach(machineId, name, { focus = true, cwd, label, auto = false } = {}) {
    const existing = ordered().find((s) => s.machineId === machineId && s.tmuxName === name && !s.readOnly);
    if (existing) return show(existing.id);
    let readOnly = false;
    if (!auto) {
      const v = await checkSessionConflict(machineId, name);
      if (!v) return;
      readOnly = v === 'watch';
    }
    const d = ((S.mstate[machineId] || {}).detached || []).find((x) => x.name === name);
    const p = presetById(presetFromTmux(name));
    const s = makeSession(machineId, presetFromTmux(name), {
      attach: name, cwd: cwd || (d && d.cwd) || undefined, readOnly,
      name: readOnly ? `${p.name} (watching)` : label,
    });
    if (focus || !S.panes.some(Boolean)) show(s.id, { focus });
    else renderAll();
    frame().then(() => { try { if (isVisible(s)) s.fit.fit(); } catch {} start(s); });
  }

  async function closeSession(id, opts = {}) {
    const s = S.sessions.get(id);
    if (!s) return;
    S.sessions.delete(id);
    S.order = S.order.filter((x) => x !== id);
    S.mru = S.mru.filter((x) => x !== id);
    S.targets.delete(id);
    clearTimeout(s.retryTimer);
    if (s.pid) { S.byPid.delete(s.pid); api.close(s.pid, opts); }
    else if (opts.kill && s.tmuxName) api.killTmux(s.machineId, s.tmuxName);
    s.term.dispose();
    s.host.remove();
    const i = S.panes.indexOf(id);
    if (i >= 0) S.panes[i] = S.mru.find((x) => !S.panes.includes(x)) || null;
    if (machineById(s.machineId).type === 'ssh') setTimeout(() => refreshMachine(s.machineId), 700);
    renderAll();
    const f = focused();
    if (f) f.term.focus();
  }

  function closeFocused(kill = false) {
    const s = focused();
    if (s) closeSession(s.id, { kill });
  }

  function installHint(m, presetId) {
    const p = presetById(presetId);
    if (m.type === 'ssh') return `${p.name} isn't installed on ${m.name}. Use "Install tools" from the machine's ⋯ menu.`;
    if (m.type === 'wsl') {
      if (presetId === 'claude') return `Claude Code isn't installed in ${m.distro}. In a WSL shell: curl -fsSL https://claude.ai/install.sh | bash`;
      if (presetId === 'antigravity') return `Antigravity isn't installed in ${m.distro}. In a WSL shell: curl -fsSL https://antigravity.google/cli/install.sh | bash`;
      if (presetId === 'openclaw') return `OpenClaw isn't installed in ${m.distro}. In a WSL shell: curl -fsSL https://openclaw.ai/install.sh | bash (it walks you through setup)`;
    }
    if (presetId === 'claude') return IS_WIN ? 'Claude Code isn\'t installed here. In PowerShell: irm https://claude.ai/install.ps1 | iex' : 'Claude Code isn\'t installed here. In a terminal: curl -fsSL https://claude.ai/install.sh | bash';
    if (presetId === 'openclaw') return IS_WIN ? 'OpenClaw isn\'t installed here. In PowerShell: iwr -useb https://openclaw.ai/install.ps1 | iex (it walks you through setup)' : 'OpenClaw isn\'t installed here. In a terminal: curl -fsSL https://openclaw.ai/install.sh | bash (it walks you through setup)';
    if (presetId === 'antigravity') return IS_WIN ? 'Antigravity CLI (agy) isn\'t installed here. In PowerShell: irm https://antigravity.google/cli/install.ps1 | iex' : 'Antigravity CLI (agy) isn\'t installed here. In a terminal: curl -fsSL https://antigravity.google/cli/install.sh | bash';
    return `${p.name} isn't available on ${m.name}.`;
  }

  // ---------------------------------------------------------------- auto-reconnect
  // Dropped remote sessions retry with backoff (2s, 4s, 8s … 30s); tmux kept them running meanwhile.
  function scheduleReconnect(s) {
    clearTimeout(s.retryTimer);
    s.retry = (s.retry || 0) + 1;
    const delay = Math.min(30, 2 ** s.retry) * 1000;
    if (s.retry === 1) s.term.write('\r\n\x1b[38;2;224;175;104m⚡ Connection lost. The session keeps running in tmux; reconnecting automatically (Enter to retry now).\x1b[0m\r\n');
    s.retryTimer = setTimeout(() => {
      if (!S.sessions.has(s.id) || s.status === 'running') return;
      start(s);
    }, delay);
  }

  function scheduleMachineRetry(id) {
    if (S.manualDisconnect[id] || S.machineRetry[id]) return;
    let n = 0;
    const tick = async () => {
      if (S.manualDisconnect[id]) { delete S.machineRetry[id]; return; }
      n += 1;
      const ok = await connectMachine(id, { quiet: true });
      if (ok) { delete S.machineRetry[id]; return; }
      S.machineRetry[id] = setTimeout(tick, Math.min(60, 2 ** n) * 1000);
    };
    S.machineRetry[id] = setTimeout(tick, 2000);
  }

  // ---------------------------------------------------------------- last reply extraction
  // Text the agent printed since the user's last submitted prompt, minus TUI chrome (boxes, hints).
  function lastReply(s) {
    const b = s.term.buffer.active;
    const end = b.length;
    const startLine = s.promptLine != null ? Math.max(0, s.promptLine - 2) : Math.max(0, end - 120);
    const BOX = /^[\s─━│┃╭╮╰╯┌┐└┘═║╔╗╚╝▔▁·•]*$/;
    const CHROME = /^\s*(\? for shortcuts|esc to interrupt|press .* to|⏵⏵|✻|✶|✳|·\s+\w+ing…|auto-accept|bypass permissions)/i;
    let lines = [];
    for (let i = startLine; i < end; i++) {
      const l = b.getLine(i);
      if (!l) continue;
      let t = l.translateToString(true).replace(/^\s*[│┃]\s?/, '').replace(/\s?[│┃]\s*$/, '');
      if (BOX.test(t) && t.trim()) continue;
      if (CHROME.test(t)) continue;
      lines.push(t);
    }
    // Drop the echoed prompt (first lines starting with ">") and the empty input box at the bottom.
    while (lines.length && (/^\s*>\s?/.test(lines[0]) || !lines[0].trim())) lines.shift();
    while (lines.length && (/^\s*>\s*$/.test(lines[lines.length - 1]) || !lines[lines.length - 1].trim())) lines.pop();
    return lines.join('\n').replace(/\n{3,}/g, '\n\n').trim().slice(-20000);
  }

  function sendLastReply(s) {
    const text = lastReply(s);
    if (!text) { toast('Nothing to send yet: no output since your last prompt', 'warn'); return; }
    S.targets = new Set(ordered().filter((o) => o !== s && o.status === 'running' && isAgent(o)).map((o) => o.id));
    openComposer(`Here is the latest output from ${s.name} on ${machineById(s.machineId).name}:\n\n${text}\n`);
  }

  // ---------------------------------------------------------------- activity tracking
  function markDone(s, notify, body) {
    s.activity = 'done';
    if (notify && S.cfg.appearance.notifyWhenDone) {
      const m = machineById(s.machineId);
      api.notify({ title: `${s.name} on ${m.name} is ready`, body: body || s.title || 'Finished - waiting for you', sessionId: s.id });
      api.flash();
    }
  }
  function clearDone(s) {
    if (s.activity === 'done') { s.activity = 'idle'; renderChrome(); }
  }
  setInterval(() => {
    const t = now();
    let dirty = false;
    for (const s of S.sessions.values()) {
      if (s.activity === 'busy' && t - s.lastData > 2500) {
        s.burst = 0;
        dirty = true;
        if (isVisible(s) && S.winFocused) s.activity = 'idle';
        else markDone(s, isAgent(s) && t - s.busySince > 5000);
      } else if (s.activity !== 'busy' && t - s.lastData > 1500) s.burst = 0;
    }
    if (dirty) renderChrome();
  }, 700);

  api.onData((pid, data) => {
    const s = S.byPid.get(pid);
    if (!s) { S.early.set(pid, (S.early.get(pid) || '') + data); return; }
    s.term.write(data);
    const t = now();
    s.lastData = t;
    if (t > s.quietUntil && t - s.lastInput > 500) {
      s.burst += data.length;
      if (s.burst > 400 && s.activity !== 'busy') { s.activity = 'busy'; s.busySince = t; renderChrome(); }
    }
  });

  api.onExit((pid, info) => {
    const s = S.byPid.get(pid);
    if (!s) return;
    S.byPid.delete(pid);
    s.pid = null;
    s.activity = 'idle';
    if (info.disconnected) {
      s.status = 'disconnected';
      if (s.tmuxName) scheduleReconnect(s);
      else s.term.write('\r\n\x1b[38;2;224;175;104m⚡ Connection lost. Press Enter to reconnect.\x1b[0m\r\n');
    } else {
      s.status = 'exited';
      s.term.write(`\r\n\x1b[2m[exited${info.code != null ? ` (${info.code})` : ''}. Enter to restart, Ctrl+Shift+W to close]\x1b[0m\r\n`);
    }
    if (machineById(s.machineId).type === 'ssh') refreshMachine(s.machineId);
    renderChrome();
  });

  // ---------------------------------------------------------------- panes / layout
  function touchMru(id) {
    S.mru = [id, ...S.mru.filter((x) => x !== id)];
  }

  function show(id, { focus = true } = {}) {
    const s = S.sessions.get(id);
    if (!s) return;
    let i = S.panes.indexOf(id);
    if (i < 0) { i = S.focus; S.panes[i] = id; }
    S.focus = i;
    touchMru(id);
    clearDone(s);
    renderAll();
    if (focus) frame().then(() => s.term.focus());
  }

  function setLayout(n) {
    const f = S.panes[S.focus];
    const list = [...new Set([f, ...S.panes, ...S.mru].filter((x) => x && S.sessions.has(x)))].slice(0, n);
    S.layout = n;
    S.panes = Array.from({ length: n }, (_, i) => list[i] || null);
    S.focus = 0;
    renderAll();
    const s = focused();
    if (s) frame().then(() => s.term.focus());
  }
  const cycleLayout = () => setLayout(S.layout === 1 ? 2 : S.layout === 2 ? 4 : 1);

  function splitWith(s) {
    if (S.layout === 1) setLayout(2);
    const i = S.panes.indexOf(s.id);
    if (i < 0) {
      const target = S.panes.findIndex((x, j) => j !== S.focus && !x);
      S.panes[target >= 0 ? target : (S.focus + 1) % S.layout] = s.id;
    }
    show(s.id);
  }

  function moveToNextPane(s) {
    const from = S.panes.indexOf(s.id);
    const to = (Math.max(from, S.focus) + 1) % S.layout;
    if (from >= 0) S.panes[from] = S.panes[to] === s.id ? null : S.panes[to];
    S.panes[to] = s.id;
    S.focus = to;
    renderAll();
    frame().then(() => s.term.focus());
  }

  function movePaneFocus(dir) {
    if (S.layout === 1) return;
    let i = S.focus;
    if (dir === 'ArrowLeft') i = S.layout === 4 ? (i % 2 ? i - 1 : i) : 0;
    if (dir === 'ArrowRight') i = S.layout === 4 ? (i % 2 ? i : i + 1) : 1;
    if (dir === 'ArrowUp' && S.layout === 4) i = i >= 2 ? i - 2 : i;
    if (dir === 'ArrowDown' && S.layout === 4) i = i < 2 ? i + 2 : i;
    S.focus = i;
    markPaneFocus();
    const s = focused();
    if (s) { touchMru(s.id); s.term.focus(); }
    renderChrome();
  }

  function createPaneEl() {
    const el = document.createElement('div');
    el.className = 'pane';
    el.innerHTML = '<div class="pane-head"></div><div class="pane-body"></div>';
    el.addEventListener('mousedown', () => {
      const i = S.paneEls.indexOf(el);
      if (i !== S.focus) { S.focus = i; const s = focused(); if (s) touchMru(s.id); markPaneFocus(); renderChrome(); }
    });
    el.querySelector('.pane-head').addEventListener('click', (e) => {
      const b = e.target.closest('[data-pact]');
      const s = S.sessions.get(S.panes[S.paneEls.indexOf(el)]);
      if (!b || !s) return;
      const act = b.dataset.pact;
      if (act === 'close') closeSession(s.id);
      else if (act === 'search') openSearch(s);
      else if (act === 'reply') sendLastReply(s);
      else if (act === 'split') S.layout === 1 ? splitWith(s) : moveToNextPane(s);
      else if (act === 'menu') { const r = b.getBoundingClientRect(); termMenu({ clientX: r.left, clientY: r.bottom + 4 }, s); }
    });
    el.querySelector('.pane-head').addEventListener('dblclick', (e) => {
      if (e.target.closest('[data-pact]')) return;
      const s = S.sessions.get(S.panes[S.paneEls.indexOf(el)]);
      if (s) { S.focus = S.paneEls.indexOf(el); setLayout(S.layout === 1 ? 2 : 1); }
    });
    return el;
  }

  function renderPanes() {
    const empty = S.order.length === 0;
    $('#welcome').hidden = !empty;
    $('#panes').hidden = empty;
    if (empty) { renderWelcome(); return; }

    S.panes = Array.from({ length: S.layout }, (_, i) => (S.sessions.has(S.panes[i]) ? S.panes[i] : null));
    S.panes = S.panes.map((id, i) => (id && S.panes.indexOf(id) === i ? id : null));
    if (S.focus >= S.layout) S.focus = 0;
    // never leave every pane empty while sessions exist
    S.panes.forEach((id, i) => { if (!id) S.panes[i] = S.mru.concat(S.order).find((x) => !S.panes.includes(x)) || null; });

    const wrap = $('#panes');
    wrap.className = `panes l${S.layout}`;
    while (S.paneEls.length < S.layout) { const el = createPaneEl(); S.paneEls.push(el); wrap.appendChild(el); }
    while (S.paneEls.length > S.layout) {
      const el = S.paneEls.pop();
      el.querySelectorAll('.term-host').forEach((h) => $('#parking').appendChild(h));
      el.remove();
    }
    const parking = $('#parking');
    for (const s of S.sessions.values()) if (!S.panes.includes(s.id) && s.host.parentElement !== parking) parking.appendChild(s.host);

    S.paneEls.forEach((el, i) => {
      const s = S.sessions.get(S.panes[i]);
      const body = el.querySelector('.pane-body');
      body.querySelectorAll('.term-host').forEach((h) => { if (!s || h !== s.host) parking.appendChild(h); });
      body.querySelector('.pane-empty')?.remove();
      if (s) { if (s.host.parentElement !== body) body.appendChild(s.host); }
      else body.insertAdjacentHTML('beforeend', '<div class="pane-empty">Empty pane. Pick a session in the sidebar or press Ctrl+Shift+P</div>');
    });
    applySplit();
    markPaneFocus();
    renderPaneHeads();
    scheduleFit();
    persistSoon();
  }

  // ---------------------------------------------------------------- resizable split
  function applySplit() {
    const wrap = $('#panes');
    const { x, y } = S.split;
    wrap.style.gridTemplateColumns = S.layout === 1 ? '' : `${x}fr ${1 - x}fr`;
    wrap.style.gridTemplateRows = S.layout === 4 ? `${y}fr ${1 - y}fr` : '';
    wrap.querySelectorAll('.gutter').forEach((g) => g.remove());
    if (S.layout === 1) return;
    const mk = (dir) => {
      const g = document.createElement('div');
      g.className = `gutter ${dir}`;
      g.title = 'Drag to resize · double-click to reset';
      g.addEventListener('mousedown', (e) => startDrag(e, dir));
      g.addEventListener('dblclick', () => { S.split[dir === 'v' ? 'x' : 'y'] = 0.5; applySplit(); scheduleFit(); persistSoon(); });
      wrap.appendChild(g);
    };
    mk('v');
    if (S.layout === 4) mk('h');
    requestAnimationFrame(positionGutters);
  }

  function positionGutters() {
    const wrap = $('#panes');
    const r = wrap.getBoundingClientRect();
    const a = S.paneEls[0] && S.paneEls[0].getBoundingClientRect();
    if (!a) return;
    const v = wrap.querySelector('.gutter.v');
    if (v) { v.style.left = `${a.right - r.left}px`; v.style.top = '10px'; v.style.bottom = '10px'; }
    const h = wrap.querySelector('.gutter.h');
    if (h) { h.style.top = `${a.bottom - r.top}px`; h.style.left = '10px'; h.style.right = '10px'; }
  }

  function startDrag(e, dir) {
    e.preventDefault();
    const wrap = $('#panes');
    const r = wrap.getBoundingClientRect();
    document.body.classList.add(dir === 'v' ? 'dragging-v' : 'dragging-h');
    let raf = 0;
    const move = (ev) => {
      const f = dir === 'v' ? (ev.clientX - r.left - 10) / (r.width - 20) : (ev.clientY - r.top - 10) / (r.height - 20);
      S.split[dir === 'v' ? 'x' : 'y'] = Math.min(0.85, Math.max(0.15, f));
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        wrap.style.gridTemplateColumns = `${S.split.x}fr ${1 - S.split.x}fr`;
        if (S.layout === 4) wrap.style.gridTemplateRows = `${S.split.y}fr ${1 - S.split.y}fr`;
        positionGutters();
        scheduleFit();
      });
    };
    const up = () => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      document.body.classList.remove('dragging-v', 'dragging-h');
      persistSoon();
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  }

  function markPaneFocus() {
    S.paneEls.forEach((el, i) => {
      const s = S.sessions.get(S.panes[i]);
      el.classList.toggle('focused', i === S.focus && S.layout > 0);
      el.style.setProperty('--c', s ? s.color : '#646d85');
    });
    renderStatus();
  }

  function renderPaneHeads() {
    S.paneEls.forEach((el, i) => {
      const s = S.sessions.get(S.panes[i]);
      const head = el.querySelector('.pane-head');
      if (!s) { head.innerHTML = '<span class="p-sub">-</span>'; return; }
      const m = machineById(s.machineId);
      const stateChip = s.status === 'disconnected' ? `<span class="chip" style="--mc:#e0af68">${s.retry ? 'reconnecting…' : 'disconnected'}</span>`
        : s.status === 'exited' ? '<span class="chip ghost">exited</span>'
        : s.status === 'starting' ? '<span class="chip ghost">starting…</span>' : '';
      head.innerHTML = `${glyph(s.presetId, 'sm', s.machineId)}<span class="p-title">${esc(s.name)}</span>`
        + `<span class="chip" style="--mc:${m.color}">${esc(m.name)}</span>`
        + (s.cwd && s.cwd !== '~' ? `<span class="chip ghost" title="${esc(s.cwd)}">${I.folder}${esc(shortPath(s.cwd))}</span>` : '')
        + (s.readOnly ? '<span class="chip" style="--mc:#bb9af7">read-only</span>' : '')
        + stateChip
        + `<span class="p-sub" title="${esc(s.tmuxName ? `tmux session ${s.tmuxName}` : '')}">${esc(s.title)}</span>`
        + (isAgent(s) ? `<button class="icon-btn" data-pact="reply" title="Send last reply to another agent (Ctrl+Shift+S)">${I.send}</button>` : '')
        + `<button class="icon-btn" data-pact="search" title="Find (Ctrl+Shift+F)">${I.search}</button>`
        + `<button class="icon-btn" data-pact="split" title="${S.layout === 1 ? 'Split side by side' : 'Move to next pane'}">${I.split}</button>`
        + `<button class="icon-btn" data-pact="menu" title="More">${I.more}</button>`
        + `<button class="icon-btn" data-pact="close" title="${s.tmuxName ? 'Detach (keeps running)' : 'Close'} (Ctrl+Shift+W)">${I.x}</button>`;
    });
  }

  let fitRaf = 0;
  function scheduleFit() {
    cancelAnimationFrame(fitRaf);
    fitRaf = requestAnimationFrame(() => {
      for (const id of S.panes) {
        const s = S.sessions.get(id);
        if (!s) continue;
        try { s.fit.fit(); } catch {}
      }
    });
  }
  // Next paint, but never stall: rAF doesn't fire while the window is minimized or fully covered.
  const frame = () => new Promise((r) => { const t = setTimeout(r, 50); requestAnimationFrame(() => { clearTimeout(t); r(); }); });

  // ---------------------------------------------------------------- chrome: tabs, sidebar, status
  function actClass(s) {
    if (s.status === 'exited' || s.status === 'disconnected') return 'exited';
    return s.activity === 'busy' ? 'busy' : s.activity === 'done' ? 'done' : '';
  }

  function renderTabs() {
    const f = focused();
    $('#tabs').innerHTML = ordered().map((s, i) => {
      const m = machineById(s.machineId);
      const a = actClass(s);
      return `<div class="tab ${f === s ? 'active' : ''}" style="--c:${s.color}" data-s="${s.id}" title="${esc(s.title || s.name)}">`
        + (a ? `<span class="act ${a}"></span>` : glyph(s.presetId, 'sm', s.machineId))
        + `<span class="t-title">${esc(s.name)}</span><span class="t-mach">${esc(m.name)}</span>`
        + (i < 9 ? `<span class="t-num">^${i + 1}</span>` : '')
        + `<span class="t-close" data-close="${s.id}">${I.x}</span></div>`;
    }).join('');
    $('#tabs .tab.active')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }

  function machineCard(m) {
    const st = S.mstate[m.id] || {};
    const status = m.type === 'local' ? 'online' : (st.status || 'offline');
    const sub = m.type === 'local' ? `local · ${OS_LABEL[m.os] || ''}`
      : m.type === 'wsl' ? `WSL · ${m.distro}${st.user ? ` · ${st.user}` : ''}`
      : (m.host ? `${m.user ? m.user + '@' : ''}${m.host}` : 'not configured');
    const sessions = ordered().filter((s) => s.machineId === m.id);
    const openNames = new Set(sessions.map((s) => s.tmuxName).filter(Boolean));
    const detached = (st.detached || []).filter((d) => !openNames.has(d.name));
    const f = focused();
    const tools = st.tools || {};
    const launchers = presetsFor(m).map((p) => {
      const missing = isMissing(m, p);
      return `<button class="launch ${missing ? 'missing' : ''}" style="--c:${p.color}" data-act="launch" data-m="${m.id}" data-p="${p.id}" title="${missing ? `${esc(p.name)} is not installed on ${esc(m.name)}` : `New ${esc(p.name)} on ${esc(m.name)} · Shift+click or right-click to pick a folder`}">${glyph(p.id, '', m.id)}<span>${esc(p.name)}</span></button>`;
    }).join('');
    const items = sessions.map((s) => {
      const i = S.order.indexOf(s.id);
      return `<div class="s-item ${f === s ? 'active' : ''}" style="--c:${s.color}" data-act="show" data-s="${s.id}">${glyph(s.presetId, 'sm', s.machineId)}`
        + `<span class="s-title">${esc(s.name)}${s.title ? `<span style="color:var(--text-3)"> · ${esc(s.title)}</span>` : ''}</span>`
        + othersOnSession(s.machineId, s.tmuxName)
        + `<span class="act ${actClass(s)}"></span>${i < 9 ? `<span class="s-num">^${i + 1}</span>` : ''}`
        + `<button class="s-x" data-act="close" data-s="${s.id}" title="${s.tmuxName ? 'Detach' : 'Close'}">${I.x}</button></div>`;
    }).join('');
    const det = detached.map((d) => `<div class="s-item detached" style="--c:${presetById(presetFromTmux(d.name)).color}" data-act="reattach" data-m="${m.id}" data-name="${esc(d.name)}" title="Reattach">`
      + `${glyph(presetFromTmux(d.name), 'sm', m.id)}<span class="s-title">${esc(d.name)}${d.cwd ? `<span style="color:var(--text-3)"> · ${esc(shortPath(d.cwd))}</span>` : ''}</span>`
      + othersOnSession(m.id, d.name)
      + `<button class="s-x" data-act="kill" data-m="${m.id}" data-name="${esc(d.name)}" title="Kill this tmux session">${I.x}</button></div>`).join('');

    return `<div class="machine" style="--c:${m.color}" data-machine="${m.id}">
      <div class="m-head">
        <div class="m-icon">${mIcon(m)}</div>
        <div class="m-meta"><div class="m-name">${esc(m.name)} <span class="dot ${status}" title="${status}"></span></div><div class="m-sub">${st.status === 'online' && st.route === 'tailscale' ? '<span class="route ts" title="Connected through Tailscale">TS</span>' : st.status === 'online' && st.route === 'jump' ? '<span class="route ts" title="Connected through a jump host">JUMP</span>' : ''}${esc(sub)}</div></div>
        <div class="m-actions">
          ${m.type === 'ssh' ? `<button class="icon-btn" data-act="refresh" data-m="${m.id}" title="${status === 'online' ? 'Refresh' : 'Connect'}">${status === 'online' ? I.refresh : I.plug}</button>` : ''}
          <button class="icon-btn" data-act="mmenu" data-m="${m.id}" title="More">${I.more}</button>
        </div>
      </div>
      ${status === 'error' && st.error ? `<div class="m-error">${esc(st.error)}<br><button data-act="settings">Open settings</button></div>` : ''}
      ${statsHtml(m)}
      <div class="launchers">${launchers}</div>
      ${items ? `<div class="s-list">${items}</div>` : ''}
      ${det ? `<div class="s-label">Detached · still running</div><div class="s-list">${det}</div>` : ''}
    </div>`;
  }

  function renderSidebar() {
    // Remote machines first: that's where the agents usually live.
    const ms = [...S.cfg.machines].sort((a, b) => (a.type === b.type ? 0 : a.type === 'ssh' ? -1 : 1));
    $('#machines').innerHTML = ms.map(machineCard).join('');
  }

  // ---------------------------------------------------------------- machine stats (GPU / RAM / load)
  const gb = (b) => (b / 1024 ** 3).toFixed(b >= 100 * 1024 ** 3 ? 0 : 1);
  function meter(label, pct, value, title = '') {
    const p = Math.max(0, Math.min(100, pct || 0));
    const hot = p >= 85 ? 'hot' : p >= 60 ? 'warm' : '';
    return `<div class="meter ${hot}" title="${esc(title)}"><span class="ml">${label}</span><span class="mv">${value}</span><span class="mb"><i style="width:${p}%"></i></span></div>`;
  }
  // Bytes/second -> "12.3 MB/s" (short, for a meter value).
  const bps = (v) => (v == null ? '-' : v >= 1024 ** 2 ? `${(v / 1024 ** 2).toFixed(v >= 10 * 1024 ** 2 ? 0 : 1)} MB/s` : v >= 1024 ? `${Math.round(v / 1024)} kB/s` : `${Math.round(v)} B/s`);
  // Local and WSL machines have stats too (this computer / the distro), without the "online" gate SSH machines need.
  const statsLive = (m, st) => m.type !== 'ssh' || st.status === 'online';
  function statsHtml(m) {
    const st = S.mstate[m.id] || {};
    const x = st.stats;
    if (!x || !statsLive(m, st)) return '<div class="m-stats" data-stats="' + m.id + '"></div>';
    const parts = [];
    if (x.gpu && x.gpu.error) {
      parts.push(`<div class="meter gpu-err" title="${esc(x.gpu.error)}"><span class="ml">GPU</span><span class="mv">driver unavailable</span><span class="mb"><i style="width:0"></i></span></div>`);
    } else if (x.gpu) {
      parts.push(meter('GPU', x.gpu.util, x.gpu.util != null ? `${x.gpu.util}%` : '-', x.gpu.name));
      if (x.gpu.temp != null) parts.push(meter('TEMP', (x.gpu.temp - 30) / 0.6, `${x.gpu.temp}°C${x.gpu.power != null ? ` · ${Math.round(x.gpu.power)}W` : ''}`, 'GPU temperature / power'));
    }
    if (x.memTotal) {
      const used = x.memTotal - (x.memAvail || 0);
      parts.push(meter('RAM', (used / x.memTotal) * 100, `${gb(used)}/${gb(x.memTotal)} GB`, 'System memory (shared with the GPU on unified-memory machines)'));
    }
    if (x.load != null && x.cpus) parts.push(meter('CPU', (x.load / x.cpus) * 100, `${x.load.toFixed(1)} / ${x.cpus}`, '1-minute load average / cores'));
    if (x.disk && x.disk.total) {
      // Used vs. total of the filesystem the home folder lives on; the bar follows what's left, which is what runs out.
      const pct = ((x.disk.total - (x.disk.avail || 0)) / x.disk.total) * 100;
      parts.push(meter('DISK', pct, `${gb(x.disk.avail || 0)} GB free of ${gb(x.disk.total)}`, `Filesystem of the home folder (${x.disk.mount || '/'}): ${gb(x.disk.used || 0)} GB used`));
    }
    if (x.net) {
      // Throughput since the previous sample (5 s): the bar is log-scaled so 100 kB/s and 1 GB/s are both visible.
      const tot = (x.net.rxBps || 0) + (x.net.txBps || 0);
      const pct = tot > 0 ? Math.min(100, (Math.log10(tot) - 3) / 6 * 100) : 0; // 1 kB/s -> 0%, 1 GB/s -> 100%
      parts.push(meter('NET', Math.max(0, pct), `↓${bps(x.net.rxBps)} ↑${bps(x.net.txBps)}`, `Network in / out (all interfaces); total ${gb(x.net.rx)} GB received, ${gb(x.net.tx)} GB sent since boot`)
        .replace('class="meter', 'class="meter net'));
    }
    return `<div class="m-stats" data-stats="${m.id}">${parts.join('')}</div>`;
  }
  async function pollStats() {
    for (const m of S.cfg.machines) {
      if (!statsLive(m, S.mstate[m.id] || {})) continue;
      const r = await api.stats(m.id);
      if (!r.ok || !r.stats) continue;
      (S.mstate[m.id] = S.mstate[m.id] || {}).stats = r.stats;
      const el = document.querySelector(`[data-stats="${m.id}"]`);
      if (el) el.outerHTML = statsHtml(m);
    }
  }
  setInterval(pollStats, 5000);

  // ---------------------------------------------------------------- session persistence + workspaces
  const persistable = (s) => !s.spec.raw && !s.spec.script;
  function snapshot() {
    const list = ordered().filter(persistable);
    return {
      layout: S.layout,
      split: { ...S.split },
      focus: S.focus,
      sessions: list.map((s) => ({ machineId: s.machineId, presetId: s.presetId, cwd: s.cwd || null, tmuxName: s.tmuxName || null, name: s.name })),
      panes: S.panes.map((id) => list.findIndex((s) => s.id === id)),
    };
  }
  let persistTimer = 0;
  function persistSoon() {
    if (S.restoring || !S.cfg) return;
    clearTimeout(persistTimer);
    persistTimer = setTimeout(() => { S.cfg.lastSession = snapshot(); api.patchConfig({ lastSession: S.cfg.lastSession }); }, 800);
  }

  // Open a snapshot/workspace. resume=true reattaches remote tmux sessions; otherwise starts fresh ones.
  async function openLayout(snap, { resume = false } = {}) {
    if (!snap || !snap.sessions || !snap.sessions.length) return;
    S.restoring = true;
    const created = [];
    for (const it of snap.sessions) {
      const m = S.cfg.machines.find((x) => x.id === it.machineId);
      if (!m || (m.type === 'ssh' && !m.host)) { created.push(null); continue; }
      const extra = { cwd: it.cwd || undefined, name: it.name };
      if (resume && it.tmuxName && m.type === 'ssh') extra.attach = it.tmuxName;
      created.push(makeSession(it.machineId, it.presetId, extra));
    }
    S.layout = [1, 2, 4].includes(snap.layout) ? snap.layout : 1;
    if (snap.split) S.split = { ...S.split, ...snap.split };
    S.panes = Array.from({ length: S.layout }, (_, i) => { const s = created[(snap.panes || [])[i]]; return s ? s.id : null; });
    S.focus = Math.min(snap.focus || 0, S.layout - 1);
    renderAll();
    await frame();
    for (const s of created) if (s) { try { if (isVisible(s)) s.fit.fit(); } catch {} start(s); }
    S.restoring = false;
    const f = focused();
    if (f) { touchMru(f.id); f.term.focus(); }
    persistSoon();
  }

  async function saveWorkspace() {
    const name = await promptText({ title: 'Save workspace', label: 'Name', placeholder: 'e.g. API sprint', value: '' });
    if (!name) return;
    const snap = snapshot();
    if (!snap.sessions.length) { toast('Open some sessions first', 'warn'); return; }
    const ws = (S.cfg.workspaces || []).filter((w) => w.name !== name);
    ws.push({ name, ...snap, sessions: snap.sessions.map(({ tmuxName, ...rest }) => rest) });
    S.cfg.workspaces = ws;
    await api.patchConfig({ workspaces: ws });
    toast(`Saved workspace "${name}"`, 'ok');
  }
  function deleteWorkspace(name) {
    S.cfg.workspaces = (S.cfg.workspaces || []).filter((w) => w.name !== name);
    api.patchConfig({ workspaces: S.cfg.workspaces });
    toast(`Deleted workspace "${name}"`);
  }
  function openWorkspaces() {
    const ws = S.cfg.workspaces || [];
    openPicker({
      placeholder: ws.length ? 'Open a workspace…' : 'No workspaces yet. Arrange your panes, then save one',
      items: () => [
        ...ws.map((w) => ({
          group: 'Workspaces', icon: I.layout, label: w.name,
          sub: w.sessions.map((s) => `${presetById(s.presetId).name}@${machineById(s.machineId).name}${s.cwd ? ` ${baseName(s.cwd)}` : ''}`).join(', '),
          run: () => openLayout(w),
        })),
        { group: 'Manage', icon: I.save, label: 'Save current layout as workspace…', run: saveWorkspace },
        ...ws.map((w) => ({ group: 'Manage', icon: I.trash, label: `Delete workspace "${w.name}"`, run: () => deleteWorkspace(w.name) })),
      ],
    });
  }

  // ---------------------------------------------------------------- folder picker
  function addRecentDir(machineId, cwd) {
    const r = { ...(S.cfg.recentDirs || {}) };
    r[machineId] = [cwd, ...(r[machineId] || []).filter((d) => d !== cwd)].slice(0, 12);
    S.cfg.recentDirs = r;
    api.patchConfig({ recentDirs: r });
  }
  async function loadDirs(m) {
    const c = S.dirCache[m.id];
    if (c && now() - c.at < 60000) return c;
    const r = m.type === 'local' ? await api.localDirs() : await api.dirs(m.id);
    if (!r.ok) { toast(r.error, 'bad'); return { repos: [], dirs: [] }; }
    return (S.dirCache[m.id] = { at: now(), repos: r.repos, dirs: r.dirs });
  }
  async function launchInFolder(machineId, presetId) {
    const m = machineById(machineId);
    const p = presetById(presetId);
    if (m.type === 'ssh' && !m.host) { openSettings(); return; }
    const recent = (S.cfg.recentDirs || {})[m.id] || [];
    let data = { repos: [], dirs: [] };
    const pick = (cwd) => { addRecentDir(m.id, cwd); launch(m.id, p.id, { cwd }); };
    const items = () => {
      const seen = new Set();
      const out = [];
      const add = (group, d, icon = I.folder) => { if (seen.has(d)) return; seen.add(d); out.push({ group, icon, label: d, run: () => pick(d) }); };
      recent.forEach((d) => add('Recent', d));
      data.repos.forEach((d) => add('Projects (git)', d));
      add('Folders', '~');
      data.dirs.forEach((d) => add('Folders', d));
      return out;
    };
    openPicker({
      placeholder: `Folder for ${p.name} on ${m.name}. Type a path and press Enter to use it`,
      items,
      onText: (text) => pick(text.trim()),
    });
    data = await loadDirs(m);
    if (S.palMode && S.palMode.items === items) renderPalette();
  }

  // ---------------------------------------------------------------- prompt modal (text / passphrase)
  function promptText({ title, label, placeholder = '', value = '', password = false, desc = '' }) {
    return new Promise((resolve) => {
      const ov = document.createElement('div');
      ov.className = 'overlay';
      ov.innerHTML = `<div class="modal prompt"><div class="modal-head"><h2>${esc(title)}</h2></div>
        <div class="modal-body">${desc ? `<p class="prompt-desc">${esc(desc)}</p>` : ''}
        <div class="field"><label>${esc(label)}</label><input ${password ? 'type="password"' : ''} placeholder="${esc(placeholder)}" value="${esc(value)}" spellcheck="false"></div></div>
        <div class="modal-foot"><span class="spacer"></span><button class="btn" data-x>Cancel</button><button class="btn primary" data-ok>OK</button></div></div>`;
      document.body.appendChild(ov);
      const input = ov.querySelector('input');
      const done = (v) => { ov.remove(); resolve(v); focused()?.term.focus(); };
      ov.close = () => done(null);
      ov.querySelector('[data-ok]').onclick = () => done(input.value.trim() ? input.value : null);
      ov.querySelector('[data-x]').onclick = () => done(null);
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.preventDefault(); done(input.value.trim() ? input.value : null); }
      });
      setTimeout(() => input.focus(), 0);
    });
  }

  api.onAuthAsk(async (id, info) => {
    const v = await promptText({
      title: `Unlock ${info.keyName}`, label: 'Passphrase', password: true,
      desc: `${info.machine} needs the passphrase for ${info.keyFile}. It is kept in memory only until Nexus closes.`,
    });
    api.answerPassphrase(id, v);
  });

  api.onHostAsk(async (id, info) => {
    const target = info.port && Number(info.port) !== 22 ? `${info.host}:${info.port}` : info.host;
    const ok = await choose({
      title: `Trust ${info.machine}?`,
      desc: `This is the first connection to ${target} and its host key isn't in your known_hosts. Its fingerprint is ${info.fingerprint}. `
        + `To check it, run on that machine: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub (or compare with what plain ssh showed you). `
        + 'If it matches, trust it; Nexus remembers it and warns you if it ever changes.',
      options: [{ label: 'Cancel', value: false, ghost: true }, { label: 'Trust and connect', value: true, primary: true }],
    });
    api.answerHost(id, ok === true);
  });

  api.onUpdateAvailable(({ version, url, download }) => {
    // Windows/Linux: the updater can fetch it (after you say so). macOS: open the release page.
    const run = download ? () => { api.downloadUpdate(); toast(`Downloading Nexus ${version}…`, 'ok', 4000); } : () => window.open(url);
    toast(`Nexus ${version} is available.`, 'ok', 60000, { label: download ? 'Download and install' : 'Download', run });
  });
  api.onUpdateReady((version) => {
    toast(`Nexus ${version} is ready to install.`, 'ok', 60000, { label: 'Restart now', run: () => api.installUpdate() });
  });

  function renderStatus() {
    const ms = S.cfg.machines.map((m) => {
      const st = m.type === 'local' ? 'online' : ((S.mstate[m.id] || {}).status || 'offline');
      return `<span class="st"><span class="dot ${st}"></span>${esc(m.name)}</span>`;
    }).join('');
    const s = focused();
    let info = '';
    if (s) {
      const m = machineById(s.machineId);
      info = `<span class="st" style="color:var(--text-2)">${glyph(s.presetId, 'sm', s.machineId)} ${esc(s.name)} @ ${esc(m.name)}${s.tmuxName ? ` · tmux ${esc(s.tmuxName)}` : ''} · ${s.term.cols}×${s.term.rows}</span>`;
    }
    $('#status-left').innerHTML = ms + info;
    $('#layout-switch').querySelectorAll('button').forEach((b) => b.classList.toggle('active', Number(b.dataset.layout) === S.layout));
  }

  function renderChrome() {
    renderTabs();
    renderSidebar();
    renderPaneHeads();
    renderStatus();
    if (!$('#composer').hidden) renderTargets();
    if (!S.order.length) renderWelcome();
  }
  function renderAll() {
    renderPanes();
    renderChrome();
  }

  function renderWelcome() {
    const ms = [...S.cfg.machines].sort((a, b) => (a.type === b.type ? 0 : a.type === 'ssh' ? -1 : 1));
    const groups = ms.map((m) => {
      const st = S.mstate[m.id] || {};
      const status = m.type === 'local' ? 'online' : (st.status || 'offline');
      const sub = m.type === 'ssh' ? (m.host ? `${m.user ? m.user + '@' : ''}${m.host}` : 'not configured') : m.type === 'wsl' ? `WSL · ${m.distro}` : `this computer · ${OS_LABEL[m.os] || ''}`;
      const cards = presetsFor(m).map((p) => {
        const missing = isMissing(m, p);
        return `<button class="card-launch ${missing ? 'missing' : ''}" style="--c:${p.color}" data-act="launch" data-m="${m.id}" data-p="${p.id}" title="New ${esc(p.name)} on ${esc(m.name)} · Shift+click to pick a folder">
          ${glyph(p.id, 'lg', m.id)}<div><div class="cl-name">${esc(p.name)}</div>${missing ? '<div class="cl-sub">not installed</div>' : ''}</div></button>`;
      }).join('');
      return `<section class="w-machine" style="--c:${m.color}"><div class="w-head"><span class="m-icon">${mIcon(m)}</span><b>${esc(m.name)}</b><span class="dot ${status}" title="${status}"></span><span class="w-sub">${esc(sub)}</span></div><div class="cards">${cards}</div></section>`;
    });
    const unconfigured = S.cfg.machines.find((m) => m.type === 'ssh' && !m.host);
    $('#welcome').innerHTML = `
      <h1>One terminal. Every agent.</h1>
      <div class="lead">Claude, Antigravity, OpenClaw and shells on every machine on your LAN, side by side.</div>
      <div class="w-machines">${groups.join('')}</div>
      ${unconfigured ? `<div class="setup">${I.server}<span><b>${esc(unconfigured.name)}</b> needs an address before it can be used.</span><span class="spacer"></span><button class="btn sm" data-act="settings">Set it up</button></div>` : ''}
      <div class="keys">
        <div><kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>P</kbd></div><div>Command palette: launch or switch anything</div>
        <div><kbd>Ctrl</kbd> <kbd>1</kbd>…<kbd>9</kbd></div><div>Jump to session</div>
        <div><kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>L</kbd></div><div>Cycle layout: single, split, grid</div>
        <div><kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>Enter</kbd></div><div>Broadcast one prompt to many agents</div>
        <div><kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>N</kbd></div><div>Launch an agent in a project folder (or Shift+click a launcher)</div>
        <div><kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>O</kbd></div><div>Workspaces: saved layouts of agents and folders</div>
        <div><kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>S</kbd></div><div>Send an agent's last reply to another agent</div>
        <div><kbd>Ctrl</kbd> <kbd>C</kbd> / <kbd>Ctrl</kbd> <kbd>V</kbd></div><div>Copy / paste text or screenshots; drop files onto a pane to attach them</div>
      </div>`;
  }

  // ---------------------------------------------------------------- machines
  async function connectMachine(id, { quiet = false } = {}) {
    delete S.manualDisconnect[id];
    if (machineById(id).type === 'wsl') { await refreshMachine(id); return (S.mstate[id] || {}).status === 'online'; }
    if (!quiet) { S.mstate[id] = { ...(S.mstate[id] || {}), status: 'connecting', error: null }; renderChrome(); }
    const r = await api.connect(id);
    if (!r.ok) { S.mstate[id] = { ...S.mstate[id], status: 'error', error: r.error }; renderChrome(); return false; }
    await refreshMachine(id);
    return true;
  }

  async function refreshMachine(id) {
    const m = machineById(id);
    if (!(m.type === 'wsl' || (m.type === 'ssh' && (m.host || m.tsHost)))) return;
    const r = await api.probe(id);
    const st = (S.mstate[id] = S.mstate[id] || {});
    if (!r.ok) { st.status = 'error'; st.error = r.error; renderChrome(); return; }
    st.status = 'online';
    st.error = null;
    st.tools = r.tools;
    st.hostname = r.hostname;
    st.home = r.home;
    st.user = r.user;
    setTimeout(ensureHub, 0);
    st.detached = r.sessions.filter((x) => x.name.startsWith('nx-')).sort((a, b) => b.activity - a.activity);
    if (!S.autoReattached[id]) {
      S.autoReattached[id] = true;
      const open = new Set(ordered().map((s) => s.tmuxName));
      const resumable = S.cfg.appearance.restoreSession === false ? [] : st.detached.filter((x) => !x.attached && !open.has(x.name)).slice(0, 8);
      if (resumable.length) {
        resumable.forEach((d, i) => reattach(id, d.name, { focus: i === 0 && !focused(), auto: true }));
        toast(`Resumed ${resumable.length} running session${resumable.length > 1 ? 's' : ''} on ${m.name}`, 'ok');
      }
    }
    renderAll();
  }

  api.onMachineStatus((id, status, error, route) => {
    const st = (S.mstate[id] = S.mstate[id] || {});
    const was = st.status;
    st.status = status;
    st.error = error;
    if (route) st.route = route;
    if (status === 'offline') st.detached = st.detached || [];
    // A machine that was online and dropped (Wi-Fi, reboot) is retried in the background.
    if (was === 'online' && (status === 'offline' || status === 'error')) scheduleMachineRetry(id);
    renderChrome();
  });

  setInterval(async () => {
    for (const m of S.cfg.machines) if (m.type === 'ssh' && (S.mstate[m.id] || {}).status === 'online') refreshMachine(m.id);
    // Track where remote sessions actually are (tmux pane path), so restores and workspaces reopen there.
    let changed = false;
    for (const s of ordered()) {
      if (!s.pid || !s.tmuxName) continue;
      const cwd = await api.cwd(s.pid);
      const home = (S.mstate[s.machineId] || {}).home;
      const tilde = cwd && home && cwd.startsWith(home) ? '~' + cwd.slice(home.length) : cwd;
      if (tilde && tilde !== s.cwd) { s.cwd = tilde; changed = true; }
    }
    if (changed) { renderPaneHeads(); persistSoon(); }
  }, 30000);

  function machineMenu(x, y, m) {
    const st = S.mstate[m.id] || {};
    const items = [{ header: m.name }];
    for (const p of presetsFor(m)) items.push({ label: `New ${p.name}`, glyph: p.id, m: m.id, run: () => launch(m.id, p.id) });
    items.push('sep', { header: 'In a folder' });
    for (const p of presetsFor(m)) items.push({ label: `${p.name} in folder…`, glyph: p.id, m: m.id, run: () => launchInFolder(m.id, p.id) });
    if (m.type === 'ssh') {
      items.push('sep');
      if (st.status === 'online') items.push({ label: 'Disconnect', run: () => { S.manualDisconnect[m.id] = true; api.disconnect(m.id); } });
      else items.push({ label: 'Connect', run: () => connectMachine(m.id) });
      items.push({ label: 'Set up SSH key…', icon: I.key, run: () => setupKey(m.id) });
      items.push({ label: 'Install tools (tmux, Claude, Antigravity, OpenClaw)…', icon: I.box, run: () => bootstrap(m.id) });
      items.push({ label: 'Enable Claude alerts', icon: I.bell, run: () => enableAlerts(m.id) });
      items.push({ label: 'Join Tailscale (sudo tailscale up --ssh)…', icon: I.plug, run: () => launch(m.id, 'shell', { raw: true, cmd: 'sudo tailscale up --ssh && tailscale status | head -5', name: `Tailscale → ${m.name}` }) });
      items.push({ label: 'Share over Tailscale…', icon: I.send, run: openShare });
      items.push('sep', { header: 'Port forwarding' });
      for (const t of S.tunnels.filter((t) => t.machineId === m.id)) {
        items.push({ label: `Open localhost:${t.localPort} → ${t.remoteHost === '127.0.0.1' ? '' : t.remoteHost + ':'}${t.remotePort}`, icon: I.plug, run: () => window.open(`http://localhost:${t.localPort}/`) });
        items.push({ label: `Stop forwarding localhost:${t.localPort}`, icon: I.x, run: () => closeTunnel(t.id) });
      }
      items.push({ label: 'Forward a port…', icon: I.plug, run: () => forwardPort(m.id) });
    }
    items.push('sep', { label: 'Machine settings…', icon: I.gear, run: openSettings });
    showMenu(x, y, items);
  }

  // ---- port forwarding (ssh -L over the machine's connection): a remote web UI as http://localhost:<port> ----
  async function forwardPort(machineId) {
    const m = machineById(machineId);
    const spec = await promptText({
      title: `Forward a port from ${m.name}`, label: 'Remote port (or host:port as seen from that machine)', placeholder: '8888, or 127.0.0.1:18789, or gpu-box2:6006',
      desc: 'The service becomes http://localhost:<port> on this computer, over the SSH connection. Only this computer can use it. The same local port is tried first; otherwise a free one is picked.',
    });
    if (!spec) return;
    const mm = /^(?:([\w.-]+):)?(\d{1,5})$/.exec(spec.trim());
    if (!mm) { toast('Enter a port like 8888 or host:port', 'warn'); return; }
    let r = await api.tunnelOpen({ machineId, remoteHost: mm[1] || '127.0.0.1', remotePort: Number(mm[2]), localPort: Number(mm[2]) });
    if (!r.ok && /already in use/.test(r.error)) r = await api.tunnelOpen({ machineId, remoteHost: mm[1] || '127.0.0.1', remotePort: Number(mm[2]), localPort: 0 });
    if (!r.ok) { toast(`Couldn't forward: ${r.error}`, 'bad'); return; }
    await refreshTunnels();
    toast(`localhost:${r.localPort} → ${m.name}:${r.remotePort}`, 'ok', 15000, { label: 'Open in browser', run: () => window.open(`http://localhost:${r.localPort}/`) });
  }
  async function closeTunnel(id) { await api.tunnelClose(id); await refreshTunnels(); toast('Port forwarding stopped'); }
  async function refreshTunnels() { try { S.tunnels = await api.tunnelList(); } catch { S.tunnels = []; } }

  async function setupKey(machineId) {
    const m = machineById(machineId);
    if (!m.host) { openSettings(); return; }
    await launch('local', 'shell', { script: 'setup-ssh-key', scriptMachine: machineId, name: `SSH key → ${m.name}` });
  }

  async function enableAlerts(machineId) {
    const m = machineById(machineId);
    const r = await api.setupAlerts(machineId);
    if (!r.ok) { toast(`Couldn't enable alerts on ${m.name}: ${r.error}`, 'bad'); return; }
    toast(r.added ? `Claude on ${m.name} will now ring Nexus when it finishes or needs you (restart running Claude sessions)` : `Claude alerts were already enabled on ${m.name}`, 'ok', 6000);
  }

  async function bootstrap(machineId) {
    const m = machineById(machineId);
    const offline = !!S.cfg.offline;
    const ok = await choose({
      title: `Install tools on ${m.name}?`,
      desc: offline
        ? `Offline mode is on: the script only installs from ~/.nexus/offline/ on ${m.name} (tmux via the package manager, claude/agy/openclaw from installers you copied there) and never reaches the internet.`
        : `This runs the vendors' install scripts on ${m.name} (curl … | bash from claude.ai, antigravity.google and openclaw.ai) and may ask for your sudo password. Only continue if ${m.name} is allowed to reach the internet and you trust those sources.`,
      options: [{ label: 'Cancel', value: false, ghost: true }, { label: 'Install', value: true, primary: true }],
    });
    if (!ok) return;
    const r = await api.uploadBootstrap(machineId);
    if (!r.ok) { toast(`Can't reach ${m.name}: ${r.error}`, 'bad'); return; }
    await launch(machineId, 'shell', { raw: true, cmd: `${offline ? 'NEXUS_OFFLINE=1 ' : ''}bash ~/.nexus/bootstrap.sh`, name: 'Install tools' });
  }

  // ---------------------------------------------------------------- copy / paste / keys
  function copySelection(s) {
    // "Select all" includes the empty rows below the prompt: drop trailing blank lines and line-end padding.
    const text = s.term.getSelection().replace(/[ \t]+$/gm, '').replace(/\n+$/, '');
    if (!text) return false;
    api.writeClipboard(text);
    s.term.clearSelection();
    toast(`Copied ${text.length} chars`, 'ok', 1200);
    return true;
  }
  // Text pastes as usual; an image on the clipboard (e.g. Win+Shift+S) is saved as a PNG on the session's
  // machine (uploaded for remotes) and its path is pasted, which Claude and Antigravity read as an image.
  async function pasteInto(s) {
    const text = await api.readClipboard();
    if (text) { s.term.paste(text); return; }
    if (s.status !== 'running') return;
    const r = await api.pasteImage(s.pid);
    if (!r.ok) { toast(r.error, 'bad'); return; }
    if (!r.path) return;
    s.term.paste(quotePath(s, r.path) + ' ');
    toast(machineById(s.machineId).type === 'ssh' ? `Image uploaded to ${machineById(s.machineId).name}` : 'Image saved and attached', 'ok', 1800);
  }

  function termKey(e, s) {
    if (e.type !== 'keydown') return true;
    if (handleShortcut(e)) { e.preventDefault(); e.stopPropagation(); return false; }
    const k = e.key.toLowerCase();
    // macOS: Cmd+C/V go through the Edit menu's native copy/paste, which xterm handles (a clipboard holding only an
    // image is caught by the paste listener below). Other Cmd combos belong to the app, never to the shell.
    if (IS_MAC && e.metaKey) return false;
    if (!IS_MAC && e.ctrlKey && !e.altKey) {
      if (k === 'c' && (e.shiftKey || s.term.hasSelection())) { copySelection(s); e.preventDefault(); return false; }
      if (k === 'v') { e.preventDefault(); pasteInto(s); return false; }
    }
    if (e.shiftKey && e.key === 'Insert') { e.preventDefault(); pasteInto(s); return false; }
    // Shift+Enter = newline without submitting in Claude Code / Antigravity CLI.
    if (e.shiftKey && !e.ctrlKey && !e.altKey && e.key === 'Enter') {
      if (s.status === 'running') api.write(s.pid, '\x1b\r');
      e.preventDefault();
      return false;
    }
    return true;
  }

  function handleShortcut(e) {
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const c = modKey(e), sh = e.shiftKey, a = e.altKey;
    if (c && sh && !a && (k === 'p' || k === 'k')) { openPalette(); return true; }
    if (c && sh && k === 't') { openPalette('new '); return true; }
    if (c && sh && k === 'Enter') { toggleComposer(); return true; }
    if (c && sh && k === 'l') { cycleLayout(); return true; }
    if (c && sh && k === 'w') { closeFocused(); return true; }
    if (c && sh && k === 'f') { const s = focused(); if (s) openSearch(s); return true; }
    if (c && sh && k === 'd') { const s = focused(); if (s) splitWith(s); return true; }
    if (c && sh && k === 's') { const s = focused(); if (s) sendLastReply(s); return true; }
    if (c && sh && k === 'o') { openWorkspaces(); return true; }
    if (c && sh && k === 'm') { toggleTeam(); return true; }
    if (c && sh && k === 'n') { openPalette('in folder '); return true; }
    if (c && !sh && !a && /^[1-9]$/.test(k)) { const s = ordered()[Number(k) - 1]; if (s) show(s.id); return true; }
    if (e.ctrlKey && k === 'Tab') { cycle(sh ? -1 : 1); return true; }
    if (e.ctrlKey && !sh && k === '`') { const id = S.mru[1]; if (id) show(id); return true; }
    if (c && a && k.startsWith('Arrow')) { movePaneFocus(k); return true; }
    if (c && !sh && k === ',') { openSettings(); return true; }
    if (c && !sh && (k === '=' || k === '+')) { bumpFont(1); return true; }
    if (c && !sh && k === '-') { bumpFont(-1); return true; }
    if (c && !sh && k === '0') { bumpFont(0); return true; }
    if (e.key === 'F12') { api.devtools(); return true; }
    if (c && sh && k === 'r') { api.reload(); return true; }
    return false;
  }

  function cycle(dir) {
    const list = ordered();
    if (!list.length) return;
    const i = list.indexOf(focused());
    show(list[(i + dir + list.length) % list.length].id);
  }

  let fontSaveTimer;
  function bumpFont(d) {
    const a = S.cfg.appearance;
    a.fontSize = d === 0 ? 14 : Math.min(28, Math.max(9, a.fontSize + d));
    applyAppearance();
    clearTimeout(fontSaveTimer);
    fontSaveTimer = setTimeout(() => api.saveConfig(S.cfg), 600);
  }
  function applyAppearance() {
    const a = S.cfg.appearance;
    for (const s of S.sessions.values()) {
      s.term.options.fontSize = a.fontSize;
      s.term.options.fontFamily = a.fontFamily;
      s.term.options.lineHeight = a.lineHeight;
    }
    scheduleFit();
  }

  // Open dialogs (palette, settings, share, prompts) own the keyboard: keys never reach the terminal behind them,
  // even when focus was left in it, and Escape closes the topmost one. Each overlay has a .close() method.
  const topOverlay = () => [...document.querySelectorAll('.overlay')].filter((o) => !o.hidden).pop() || null;
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !$('#ctxmenu').hidden) { e.preventDefault(); e.stopPropagation(); return hideMenu(); }
    const dlg = topOverlay();
    if (dlg) {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); dlg.close?.(); return; }
      if (!dlg.contains(e.target)) {
        e.preventDefault();
        e.stopPropagation();
        (dlg.querySelector('input:not([type=hidden]):not([disabled]), textarea, select, button.primary') || dlg).focus();
      }
      return; // no app shortcuts while a dialog is open
    }
    if (e.target.closest && e.target.closest('.xterm')) return; // terminal handles its own
    if (e.key === 'Escape' && !$('#composer').hidden) return closeComposer();
    if (e.target.matches?.('input, textarea, select') && !(e.ctrlKey && e.shiftKey)) return;
    if (handleShortcut(e)) e.preventDefault();
  }, true);

  // ---------------------------------------------------------------- search
  function openSearch(s) {
    const pane = S.paneEls[S.panes.indexOf(s.id)];
    if (!pane) return;
    const body = pane.querySelector('.pane-body');
    let bar = body.querySelector('.searchbar');
    if (!bar) {
      bar = document.createElement('div');
      bar.className = 'searchbar';
      bar.style.top = '6px';
      bar.innerHTML = `<input placeholder="Find in scrollback" spellcheck="false"><button class="icon-btn" data-d="prev">${I.up}</button><button class="icon-btn" data-d="next">${I.down}</button><button class="icon-btn" data-d="close">${I.x}</button>`;
      body.appendChild(bar);
      const input = bar.querySelector('input');
      const close = () => { s.search.clearDecorations?.(); s.term.clearSelection(); bar.remove(); s.term.focus(); };
      input.addEventListener('input', () => s.search.findNext(input.value, { incremental: true }));
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { e.shiftKey ? s.search.findPrevious(input.value) : s.search.findNext(input.value); e.preventDefault(); }
        if (e.key === 'Escape') { close(); e.stopPropagation(); }
      });
      bar.addEventListener('click', (e) => {
        const d = e.target.closest('[data-d]')?.dataset.d;
        if (d === 'next') s.search.findNext(input.value);
        if (d === 'prev') s.search.findPrevious(input.value);
        if (d === 'close') close();
      });
    }
    const sel = s.term.getSelection();
    const input = bar.querySelector('input');
    if (sel && !sel.includes('\n')) input.value = sel;
    input.focus();
    input.select();
  }

  // ---------------------------------------------------------------- context menu
  function showMenu(x, y, items) {
    const el = $('#ctxmenu');
    el.innerHTML = items.map((it, i) => {
      if (it === 'sep') return '<div class="sep"></div>';
      if (it.header) return `<div class="lbl">${esc(it.header)}</div>`;
      const ic = it.glyph ? glyph(it.glyph, 'sm', it.m) : it.icon ? `<span style="width:18px;display:grid;place-items:center;color:var(--text-2)">${it.icon}</span>` : '<span style="width:18px"></span>';
      return `<button class="ci ${it.danger ? 'danger' : ''}" data-i="${i}" ${it.disabled ? 'disabled' : ''}>${ic}<span>${esc(it.label)}</span>${it.kbd ? `<span class="k">${esc(it.kbd)}</span>` : ''}</button>`;
    }).join('');
    el.hidden = false;
    const r = el.getBoundingClientRect();
    el.style.left = `${Math.min(x, innerWidth - r.width - 8)}px`;
    el.style.top = `${Math.min(y, innerHeight - r.height - 8)}px`;
    el.onclick = (e) => {
      const b = e.target.closest('[data-i]');
      if (!b || b.disabled) return;
      hideMenu();
      items[Number(b.dataset.i)].run();
    };
  }
  function hideMenu() { $('#ctxmenu').hidden = true; }
  document.addEventListener('mousedown', (e) => { if (!e.target.closest('#ctxmenu')) hideMenu(); });
  window.addEventListener('blur', hideMenu);

  function termMenu(e, s) {
    const sel = s.term.getSelection();
    const others = ordered().filter((o) => o !== s && o.status === 'running');
    const items = [
      { label: 'Copy', kbd: 'Ctrl+C', disabled: !sel, run: () => copySelection(s) },
      { label: 'Paste', kbd: 'Ctrl+V', run: () => pasteInto(s) },
      { label: 'Select all', run: () => s.term.selectAll() },
      { label: 'Find…', kbd: 'Ctrl+Shift+F', run: () => openSearch(s) },
      { label: 'Send last reply to…', kbd: 'Ctrl+Shift+S', icon: I.send, run: () => sendLastReply(s) },
      { label: 'Clear scrollback', run: () => s.term.clear() },
    ];
    if (others.length) {
      items.push('sep', { header: sel ? 'Send selection to' : 'Send selection to (select text first)' });
      for (const o of others) {
        items.push({
          label: `${o.name} · ${machineById(o.machineId).name}`, glyph: o.presetId, m: o.machineId, disabled: !sel,
          run: () => { o.term.paste(sel); toast(`Sent ${sel.length} chars to ${o.name}`, 'ok'); },
        });
      }
    }
    items.push('sep');
    items.push(S.layout === 1
      ? { label: 'Open side by side', kbd: 'Ctrl+Shift+D', icon: I.split, run: () => splitWith(s) }
      : { label: 'Move to next pane', icon: I.split, run: () => moveToNextPane(s) });
    if (s.tmuxName) {
      items.push({ label: 'Detach (keeps running)', kbd: 'Ctrl+Shift+W', run: () => closeSession(s.id) });
      items.push({ label: 'Kill session', danger: true, run: () => closeSession(s.id, { kill: true }) });
    } else {
      items.push({ label: 'Close', kbd: 'Ctrl+Shift+W', danger: true, run: () => closeSession(s.id) });
    }
    showMenu(e.clientX, e.clientY, items);
  }

  // ---------------------------------------------------------------- palette
  let palItems = [], palSel = 0;

  function paletteSource() {
    const items = [];
    ordered().forEach((s, i) => {
      const m = machineById(s.machineId);
      items.push({ group: 'Sessions', glyph: s.presetId, m: s.machineId, c: s.color, label: s.name, sub: `${m.name}${s.title ? ' · ' + s.title : ''}`, kbd: i < 9 ? `Ctrl+${i + 1}` : '', run: () => show(s.id) });
    });
    const ms = [...S.cfg.machines].sort((a, b) => (a.type === b.type ? 0 : a.type === 'ssh' ? -1 : 1));
    for (const m of ms) {
      for (const p of presetsFor(m)) items.push({ group: 'Launch', glyph: p.id, m: m.id, c: p.color, label: `New ${p.name}`, sub: `on ${m.name}${isMissing(m, p) ? ' · not installed' : ''}`, missing: isMissing(m, p), run: () => launch(m.id, p.id) });
    }
    for (const m of ms) {
      for (const p of presetsFor(m)) items.push({ group: 'Launch in folder', glyph: p.id, m: m.id, c: p.color, label: `${p.name} in folder…`, sub: `on ${m.name}${isMissing(m, p) ? ' · not installed' : ''}`, missing: isMissing(m, p), run: () => launchInFolder(m.id, p.id) });
    }
    for (const w of S.cfg.workspaces || []) {
      items.push({ group: 'Workspaces', icon: I.layout, label: `Open workspace ${w.name}`, sub: `${w.sessions.length} sessions`, run: () => openLayout(w) });
    }
    for (const m of ms) {
      const open = new Set(ordered().map((s) => s.tmuxName));
      for (const d of ((S.mstate[m.id] || {}).detached || []).filter((x) => !open.has(x.name))) {
        const pid = presetFromTmux(d.name);
        items.push({ group: 'Detached', glyph: pid, m: m.id, c: presetById(pid).color, label: `Reattach ${d.name}`, sub: `on ${m.name}`, run: () => reattach(m.id, d.name) });
      }
    }
    const cmd = (label, run, kbd = '', icon = I.layout) => items.push({ group: 'Commands', icon, label, kbd, run });
    cmd('Layout: single', () => setLayout(1));
    cmd('Layout: side by side', () => setLayout(2));
    cmd('Layout: grid of four', () => setLayout(4));
    cmd('Broadcast prompt to sessions', () => openComposer(), 'Ctrl+Shift+Enter', I.send);
    cmd('Workspaces…', openWorkspaces, 'Ctrl+Shift+O', I.layout);
    cmd('Share machines over Tailscale…', openShare, '', I.send);
    cmd('Team chat', () => openTeam(), 'Ctrl+Shift+M', I.send);
    cmd('Save layout as workspace…', saveWorkspace, '', I.save);
    if (focused()) cmd(`Send last reply from ${focused().name} to…`, () => sendLastReply(focused()), 'Ctrl+Shift+S', I.send);
    cmd('Settings', openSettings, 'Ctrl+,', I.gear);
    for (const m of S.cfg.machines.filter((x) => x.type === 'ssh')) {
      cmd(`Connect ${m.name}`, () => connectMachine(m.id), '', I.plug);
      cmd(`Set up SSH key for ${m.name}`, () => setupKey(m.id), '', I.key);
      cmd(`Install tools on ${m.name} (tmux, Claude, Antigravity, OpenClaw)`, () => bootstrap(m.id), '', I.box);
      cmd(`Enable Claude alerts on ${m.name}`, () => enableAlerts(m.id), '', I.bell);
    }
    cmd('Reload window', () => api.reload(), 'Ctrl+Shift+R', I.refresh);
    cmd('Toggle developer tools', () => api.devtools(), 'F12', I.gear);
    return items;
  }

  function highlight(str, offset, hits) {
    let out = '';
    for (let i = 0; i < str.length; i++) out += hits.has(i + offset) ? `<mark>${esc(str[i])}</mark>` : esc(str[i]);
    return out;
  }

  // A picker reuses the palette with its own items; onText receives typed text when nothing is selected.
  function openPicker({ placeholder, items, onText = null }) {
    S.palMode = { placeholder, items, onText };
    openPalette('', true);
  }

  function renderPalette() {
    const q = $('#palette-input').value.trim();
    const src = S.palMode ? S.palMode.items() : paletteSource();
    const onText = S.palMode && S.palMode.onText;
    if (onText && q) src.unshift({ group: 'Use', icon: I.folder, label: q, sub: 'press Enter to use this path', run: () => onText(q), exact: true });
    if (q) {
      palItems = src.map((it) => ({ it, f: it.exact ? { score: -1e6, hits: new Set() } : fuzzy(q, `${it.label} ${it.sub || ''}`) })).filter((x) => x.f)
        .sort((a, b) => b.f.score - a.f.score).map((x) => ({ ...x.it, hits: x.f.hits }));
      // A typed path goes first only if nothing else matches well.
      const ix = palItems.findIndex((x) => x.exact);
      if (ix > 0 && /^[~/]|^[a-z]:[\/]/i.test(q)) palItems.unshift(...palItems.splice(ix, 1));
    } else palItems = src.map((it) => ({ ...it, hits: new Set() }));
    palSel = Math.min(palSel, Math.max(0, palItems.length - 1));
    let lastGroup = null;
    $('#palette-list').innerHTML = palItems.length ? palItems.map((it, i) => {
      const head = !q && it.group !== lastGroup ? `<div class="p-group">${esc(it.group)}</div>` : '';
      lastGroup = it.group;
      const ic = it.glyph ? glyph(it.glyph, '', it.m) : `<span class="glyph" style="--c:#7aa2f7">${it.icon}</span>`;
      const sub = it.sub ? `<small>${highlight(it.sub, it.label.length + 1, it.hits)}</small>` : '';
      return `${head}<div class="p-item ${i === palSel ? 'sel' : ''} ${it.missing ? 'missing' : ''}" data-i="${i}" style="${it.c ? `--c:${it.c}` : ''}">${ic}`
        + `<span class="p-label"><b>${highlight(it.label, 0, it.hits)}</b>${sub}</span>${it.kbd ? `<kbd>${esc(it.kbd)}</kbd>` : ''}</div>`;
    }).join('') : '<div class="p-empty">No matches</div>';
    $('#palette-list .p-item.sel')?.scrollIntoView({ block: 'nearest' });
  }

  function openPalette(prefix = '', keepMode = false) {
    hideMenu();
    if (!keepMode) S.palMode = null;
    $('#palette').hidden = false;
    const input = $('#palette-input');
    input.placeholder = S.palMode ? S.palMode.placeholder : 'Switch session, launch Claude / Antigravity / OpenClaw / Shell on any machine…';
    input.value = prefix;
    palSel = 0;
    renderPalette();
    input.focus();
  }
  function closePalette() {
    $('#palette').hidden = true;
    S.palMode = null;
    focused()?.term.focus();
  }
  function runPalette(i) {
    const it = palItems[i];
    if (!it) return;
    $('#palette').hidden = true;
    S.palMode = null;
    it.run();
  }
  $('#palette-input').addEventListener('input', () => { palSel = 0; renderPalette(); });
  $('#palette-input').addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { palSel = (palSel + 1) % Math.max(1, palItems.length); renderPalette(); e.preventDefault(); }
    else if (e.key === 'ArrowUp') { palSel = (palSel - 1 + palItems.length) % Math.max(1, palItems.length); renderPalette(); e.preventDefault(); }
    else if (e.key === 'Enter') { runPalette(palSel); e.preventDefault(); }
  });
  $('#palette').close = closePalette;
  $('#palette-list').addEventListener('click', (e) => { const el = e.target.closest('[data-i]'); if (el) runPalette(Number(el.dataset.i)); });
  $('#palette-list').addEventListener('mousemove', (e) => {
    const el = e.target.closest('[data-i]');
    if (el && Number(el.dataset.i) !== palSel) { palSel = Number(el.dataset.i); $('#palette-list .sel')?.classList.remove('sel'); el.classList.add('sel'); }
  });
  $('#palette').addEventListener('mousedown', (e) => { if (e.target.id === 'palette') closePalette(); });

  // ---------------------------------------------------------------- composer (broadcast)
  function renderTargets() {
    const live = ordered().filter((s) => s.status === 'running');
    for (const id of [...S.targets]) if (!live.some((s) => s.id === id)) S.targets.delete(id);
    const agents = live.filter(isAgent);
    $('#composer-targets').innerHTML = live.map((s) => `<button class="tchip ${S.targets.has(s.id) ? 'on' : ''}" style="--c:${s.color}" data-s="${s.id}">${glyph(s.presetId, '', s.machineId)}${esc(s.name)} <span style="color:var(--text-3)">${esc(machineById(s.machineId).name)}</span></button>`).join('')
      + (agents.length > 1 ? '<button class="tchip" data-all="agents" style="--c:#bb9af7">All agents</button>' : '')
      + (live.length ? '' : '<span class="hint">No running sessions yet</span>');
  }
  function openComposer(prefill) {
    const c = $('#composer');
    if (typeof prefill === 'string') $('#composer-text').value = prefill;
    if (!S.targets.size && focused()) S.targets.add(focused().id);
    c.hidden = false;
    renderTargets();
    $('#composer-text').focus();
    if (typeof prefill === 'string') $('#composer-text').setSelectionRange(0, 0);
  }
  function closeComposer() {
    $('#composer').hidden = true;
    focused()?.term.focus();
  }
  const toggleComposer = () => ($('#composer').hidden ? openComposer() : closeComposer());
  function sendComposer() {
    const text = $('#composer-text').value;
    if (!text.trim()) return;
    const targets = [...S.targets].map((id) => S.sessions.get(id)).filter((s) => s && s.status === 'running');
    if (!targets.length) { toast('Pick at least one target session', 'warn'); return; }
    const submit = $('#composer-submit').checked;
    for (const s of targets) {
      s.term.paste(text);
      s.lastInput = now();
      if (submit) setTimeout(() => api.write(s.pid, '\r'), 150);
    }
    toast(`Sent to ${targets.map((s) => `${s.name} on ${machineById(s.machineId).name}`).join(', ')}`, 'ok');
    $('#composer-text').value = '';
    closeComposer();
  }
  $('#composer-targets').addEventListener('click', (e) => {
    const all = e.target.closest('[data-all]');
    if (all) {
      const agents = ordered().filter((s) => s.status === 'running' && isAgent(s));
      const allOn = agents.every((s) => S.targets.has(s.id));
      agents.forEach((s) => (allOn ? S.targets.delete(s.id) : S.targets.add(s.id)));
    }
    const b = e.target.closest('[data-s]');
    if (b) S.targets.has(b.dataset.s) ? S.targets.delete(b.dataset.s) : S.targets.add(b.dataset.s);
    renderTargets();
  });
  $('#composer-text').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && e.ctrlKey && !e.shiftKey) { sendComposer(); e.preventDefault(); }
    if (e.key === 'Escape') { closeComposer(); e.preventDefault(); }
  });
  $('#composer-send').addEventListener('click', sendComposer);
  $('#composer-close').addEventListener('click', closeComposer);
  $('#btn-composer').addEventListener('click', toggleComposer);

  // ---------------------------------------------------------------- settings
  function machineForm(m, i) {
    const ssh = m.type === 'ssh';
    return `<div class="m-form" data-mi="${i}" style="--c:${m.color}">
      <div class="m-form-head"><div class="m-icon">${mIcon(m)}</div><b>${esc(m.name)}</b><span class="m-kind">${ssh ? 'ssh · linux' : m.type === 'wsl' ? `wsl · ${esc(m.distro)}` : `this computer · ${OS_LABEL[m.os] || ''}`}</span><span class="spacer"></span>
        ${ssh && S.draft.machines.filter((x) => x.type === 'ssh').length > 1 ? '<button class="btn sm ghost danger" data-sact="remove">Remove</button>' : ''}</div>
      <div class="grid">
        <div class="field c3"><label>Name</label><input data-f="name" value="${esc(m.name)}"></div>
        <div class="field c1"><label>Accent</label><input type="color" data-f="color" value="${esc(m.color)}"></div>
        <div class="field c2"><label>Icon</label><select data-f="icon">${[['', 'Auto'], ['windows', 'Windows'], ['linux', 'Linux'], ['nvidia', 'NVIDIA']]
          .map(([v, l]) => `<option value="${v}" ${(m.icon || '') === v ? 'selected' : ''}>${l}</option>`).join('')}</select></div>
        ${ssh ? `
        <div class="field c2"><label>User</label><input class="mono" data-f="user" value="${esc(m.user)}" placeholder="from ~/.ssh/config"></div>
        <div class="field c3"><label>Host / IP</label><input class="mono" data-f="host" value="${esc(m.host)}" placeholder="192.168.1.50 or linuxbox.local"></div>
        <div class="field c1"><label>Port</label><input class="mono" type="number" data-f="port" value="${esc(m.port || 22)}"></div>
        <div class="field c6"><label>Tailscale address (optional; used when the LAN address can't be reached. Filled in automatically by Share)</label><input class="mono" data-f="tsHost" value="${esc(m.tsHost)}" placeholder="spark1.tail1234.ts.net"></div>
        <div class="field c3"><label>Private key (optional; default tries ~/.ssh/id_ed25519, id_ecdsa, id_rsa, then ssh-agent)</label><input class="mono" data-f="keyPath" value="${esc(m.keyPath)}" placeholder="C:\\Users\\you\\.ssh\\id_ed25519"></div>
        <div class="field c3"><label>Jump host (optional, like ssh -J; blank uses ProxyJump from ~/.ssh/config)</label><input class="mono" data-f="jump" value="${esc(m.jump || '')}" placeholder="user@bastion.example.com:22"></div>` : ''}
      </div>
      ${ssh ? `<div class="m-form-actions">
        <button class="btn sm" data-sact="test">${I.plug} Test connection</button>
        <button class="btn sm" data-sact="key">${I.key} Set up SSH key</button>
        <button class="btn sm" data-sact="bootstrap">${I.box} Install tmux / Claude / Antigravity / OpenClaw</button>
        <button class="btn sm" data-sact="alerts">${I.bell} Enable Claude alerts</button>
      </div><div class="probe-out"></div>` : ''}
    </div>`;
  }

  function renderSettings() {
    const d = S.draft;
    const a = d.appearance;
    $('#settings-body').innerHTML = `
      <div class="section-title">Machines</div>
      ${d.machines.map(machineForm).join('')}
      <button class="btn sm ghost" data-sact="add">+ Add another SSH machine</button>
      <div class="section-title">Appearance &amp; behaviour</div>
      <div class="grid">
        <div class="field c1"><label>Font size</label><input type="number" min="9" max="28" data-a="fontSize" value="${a.fontSize}"></div>
        <div class="field c1"><label>Line height</label><input type="number" step="0.05" min="1" max="2" data-a="lineHeight" value="${a.lineHeight}"></div>
        <div class="field c4"><label>Font family</label><input class="mono" data-a="fontFamily" value="${esc(a.fontFamily)}"></div>
        <label class="check field c3" style="flex-direction:row"><input type="checkbox" data-a="copyOnSelect" ${a.copyOnSelect ? 'checked' : ''}><span>Copy on select</span></label>
        <label class="check field c3" style="flex-direction:row"><input type="checkbox" data-a="notifyWhenDone" ${a.notifyWhenDone ? 'checked' : ''}><span>Notify when a background agent finishes</span></label>
        <div class="field c3"><label>Your name in team chat</label><input data-c="name" value="${esc((d.collab || {}).name || '')}" placeholder="${esc((S.tsUser && S.tsUser.name) || 'from Tailscale')}"></div>
        <div class="field c3"><label>Team chat lives on</label><select data-c="hub"><option value="">Auto (first SSH machine by hostname)</option>${d.machines.filter((m) => m.type === 'ssh').map((m) => { const h = (S.mstate[m.id] || {}).hostname; return h ? `<option value="${esc(h)}" ${(d.collab || {}).hub === h ? 'selected' : ''}>${esc(m.name)} (${esc(h)})</option>` : ''; }).join('')}</select></div>
        <label class="check field c6" style="flex-direction:row"><input type="checkbox" data-c="announce" ${(d.collab || {}).announce !== false ? 'checked' : ''}><span>Post in team chat when I start an agent on a shared machine</span></label>
        <label class="check field c6" style="flex-direction:row"><input type="checkbox" data-a="restoreSession" ${a.restoreSession !== false ? 'checked' : ''}><span>Restore sessions, layout and folders when Nexus starts</span></label>
        <label class="check field c6" style="flex-direction:row"><input type="checkbox" data-a="recordSessions" ${a.recordSessions ? 'checked' : ''}><span>Record remote sessions to <span class="mono">~/.nexus/logs</span> on the machine (audit trail; kept 30 days)</span></label>
        <label class="check field c6" style="flex-direction:row"><input type="checkbox" data-top="offline" ${d.offline ? 'checked' : ''}><span>Offline / air-gapped mode: no update checks, no internet installers (tools install from <span class="mono">~/.nexus/offline/</span> on each machine)</span></label>
      </div>`;
  }

  function openSettings() {
    hideMenu();
    S.draft = JSON.parse(JSON.stringify(S.cfg));
    renderSettings();
    $('#settings').hidden = false;
    $('#settings .modal').focus();
  }
  function closeSettings() {
    $('#settings').hidden = true;
    focused()?.term.focus();
  }
  async function saveSettings() {
    S.cfg = await api.saveConfig(S.draft);
    S.draft = JSON.parse(JSON.stringify(S.cfg));
    TM.live = false; // re-pick the chat hub with the new settings
    ensureHub();
    applyAppearance();
    renderAll();
    for (const m of S.cfg.machines) {
      if (m.type === 'ssh' && m.host && (S.mstate[m.id] || {}).status !== 'online') connectMachine(m.id);
    }
  }

  $('#settings-body').addEventListener('input', (e) => {
    const t = e.target;
    const form = t.closest('[data-mi]');
    if (form && t.dataset.f) {
      const m = S.draft.machines[Number(form.dataset.mi)];
      m[t.dataset.f] = t.dataset.f === 'port' ? Number(t.value) || 22 : t.value.trim();
      if (t.dataset.f === 'color') form.style.setProperty('--c', t.value);
    }
    if (t.dataset.c) {
      const c = (S.draft.collab = S.draft.collab || {});
      c[t.dataset.c] = t.type === 'checkbox' ? t.checked : t.value.trim();
    }
    if (t.dataset.a) {
      const a = S.draft.appearance;
      a[t.dataset.a] = t.type === 'checkbox' ? t.checked : t.type === 'number' ? Number(t.value) : t.value;
    }
    if (t.dataset.top) S.draft[t.dataset.top] = t.type === 'checkbox' ? t.checked : t.value.trim();
  });
  $('#settings-body').addEventListener('click', async (e) => {
    const b = e.target.closest('[data-sact]');
    if (!b) return;
    const act = b.dataset.sact;
    if (act === 'add') {
      S.draft.machines.push({ id: `ssh-${Date.now().toString(36)}`, name: 'New machine', type: 'ssh', os: 'linux', host: '', port: 22, user: '', keyPath: '', color: '#7dcfff' });
      renderSettings();
      return;
    }
    const form = b.closest('[data-mi]');
    const idx = Number(form.dataset.mi);
    const m = S.draft.machines[idx];
    if (act === 'remove') { S.draft.machines.splice(idx, 1); renderSettings(); return; }
    if (!m.host) { toast('Enter the host / IP first', 'warn'); return; }
    await saveSettings();
    if (act === 'test') {
      const out = form.querySelector('.probe-out');
      out.innerHTML = '<span class="hint">Connecting…</span>';
      const ok = await api.connect(m.id);
      const r = ok.ok ? await api.probe(m.id) : ok;
      if (!r.ok) { out.innerHTML = `<span class="chip no">${esc(r.error)}</span>`; return; }
      out.innerHTML = `<span class="chip">connected · ${esc(r.hostname)}</span>`
        + Object.entries(r.tools).map(([k, v]) => `<span class="chip ${v ? '' : 'no'}">${v ? '✓' : '✗'} ${esc(k)}</span>`).join('');
      refreshMachine(m.id);
    } else if (act === 'key') {
      closeSettings();
      setupKey(m.id);
    } else if (act === 'bootstrap') {
      closeSettings();
      bootstrap(m.id);
    } else if (act === 'alerts') {
      enableAlerts(m.id);
    }
  });
  $('#settings-save').addEventListener('click', async () => { await saveSettings(); closeSettings(); toast('Settings saved', 'ok'); });
  $('#settings').addEventListener('click', (e) => { if (e.target.closest('[data-close]') || e.target.id === 'settings') closeSettings(); });
  $('#settings').close = closeSettings;
  $('#settings-open-file').addEventListener('click', () => api.openConfig());
  $('#btn-settings').addEventListener('click', openSettings);

  // ---------------------------------------------------------------- Tailscale sharing
  // Owner: put the SSH machines on the tailnet, lock guests to SSH on them, invite people.
  // Guest: paste an invite code to add those machines (reached via their Tailscale addresses).
  const SH = { open: false, ts: null, overview: null, busy: '', result: null, plan: null, error: '' };
  const sshMachines = () => S.cfg.machines.filter((m) => m.type === 'ssh');
  const tsPeerFor = (m) => {
    const hn = ((S.mstate[m.id] || {}).hostname || '').toLowerCase();
    const peers = (SH.ts && SH.ts.local && SH.ts.local.peers) || [];
    return peers.find((p) => hn && (p.hostName || '').toLowerCase() === hn)
      || peers.find((p) => m.tsHost && (p.dnsName === m.tsHost || p.ips.includes(m.tsHost)));
  };
  const sparkArgs = () => sshMachines().filter((m) => !m.invited)
    .map((m) => ({ machineId: m.id, hostname: (S.mstate[m.id] || {}).hostname || '', tsHost: m.tsHost || '' })).filter((x) => x.hostname || x.tsHost);
  const sshUsersFor = () => [...new Set(sshMachines().map((m) => m.user).filter(Boolean))];

  async function refreshShare({ overview = true } = {}) {
    SH.ts = await api.tsStatus();
    // Remember each machine's tailnet address so Nexus can fall back to it away from the LAN.
    let changed = false;
    for (const m of sshMachines()) {
      const p = tsPeerFor(m);
      if (p && p.dnsName && m.tsHost !== p.dnsName) { m.tsHost = p.dnsName; changed = true; }
    }
    if (changed) S.cfg = await api.saveConfig(S.cfg);
    if (overview && SH.ts.hasToken) {
      const r = await api.tsOverview(sparkArgs());
      SH.overview = r.ok ? r : null;
      SH.error = r.ok ? '' : r.error;
    }
    renderShare();
  }

  function openShare() {
    hideMenu();
    let ov = $('#share');
    if (!ov) {
      ov = document.createElement('div');
      ov.id = 'share';
      ov.className = 'overlay';
      ov.innerHTML = '<div class="modal share" tabindex="-1"><div class="modal-head"><h2>Share your machines</h2><button class="icon-btn" data-sh="close">' + I.x + '</button></div><div class="modal-body" id="share-body"></div></div>';
      document.body.appendChild(ov);
      ov.close = closeShare;
      ov.addEventListener('click', onShareClick);
      ov.addEventListener('mousedown', (e) => { if (e.target === ov) closeShare(); });
    }
    ov.hidden = false;
    ov.querySelector('.modal').focus();
    SH.open = true;
    SH.result = null;
    SH.plan = null;
    renderShare();
    refreshShare();
  }
  function closeShare() {
    const ov = $('#share');
    if (ov) ov.hidden = true;
    SH.open = false;
    focused()?.term.focus();
  }

  const step = (n, title, body, state = '') => `<section class="sh-step ${state}"><div class="sh-num">${state === 'done' ? '✓' : n}</div><div class="sh-main"><h3>${title}</h3>${body}</div></section>`;

  function renderShare() {
    const body = $('#share-body');
    if (!body) return;
    const ts = SH.ts;
    const local = ts && ts.local;
    if (!ts) { body.innerHTML = '<p class="hint">Checking Tailscale…</p>'; return; }

    // 1. This PC on the tailnet
    let pc;
    if (!local.installed) pc = `<p>Tailscale isn't installed on this PC.</p><button class="btn sm primary" data-sh="install">Install Tailscale</button>`;
    else if (!local.running || local.state !== 'Running') pc = `<p>Tailscale is installed but not signed in${local.state ? ` (${esc(local.state)})` : ''}. Open the Tailscale app from the system tray and sign in, then press Refresh.</p>`;
    else pc = `<p>Connected to <b>${esc(local.tailnet || 'your tailnet')}</b> as <span class="mono">${esc((local.self && local.self.dnsName) || '')}</span>.</p>`;
    const pcDone = local.installed && local.state === 'Running';

    // 2. SSH machines on the tailnet
    const rows = sshMachines().map((m) => {
      const p = pcDone ? tsPeerFor(m) : null;
      const st = S.mstate[m.id] || {};
      const right = p
        ? `<span class="chip" style="--mc:${p.online ? '#73daca' : '#646d85'}">${p.online ? 'online' : 'offline'}</span><span class="mono dim">${esc(p.dnsName)}</span>`
        : st.status === 'online'
          ? `<button class="btn sm" data-sh="join" data-m="${m.id}">Join Tailscale</button>`
          : '<span class="dim">connect to it on the LAN first</span>';
      return `<div class="sh-row"><span class="m-icon sm" style="--c:${m.color}">${mIcon(m)}</span><b>${esc(m.name)}</b><span class="dim">${esc(m.user || '')}</span><span class="spacer"></span>${right}</div>`;
    }).join('');
    const sparksDone = pcDone && sshMachines().length && sshMachines().every((m) => tsPeerFor(m));

    // 3. API token
    const token = ts.hasToken
      ? `<p>API token saved, encrypted with your Windows account. <button class="linklike" data-sh="cleartoken">Remove</button></p>`
      : `<p>Nexus needs a Tailscale <b>API access token</b> to send invites and set access rules. Create one at
         <a href="https://login.tailscale.com/admin/settings/keys" target="_blank">login.tailscale.com → Settings → Keys</a> ("Generate access token…"), then paste it here.</p>
         <div class="sh-inline"><input type="password" id="sh-token" placeholder="tskey-api-…" spellcheck="false"><button class="btn sm primary" data-sh="savetoken">Save</button></div>`;

    // 4. Access rules
    let rules = '';
    if (ts.hasToken) {
      if (SH.plan) {
        rules = `<p>Nexus will make these changes to your tailnet:</p><ul class="sh-list">${SH.plan.changes.map((c) => `<li>${esc(c)}</li>`).join('') || '<li>Nothing: already set up.</li>'}</ul>`
          + (SH.plan.missing.length ? `<p class="warn">Not on the tailnet yet: ${SH.plan.missing.map(esc).join(', ')}. Join them first (step 2).</p>` : '')
          + `<div class="sh-inline"><button class="btn sm primary" data-sh="apply" ${SH.plan.changes.length ? '' : 'disabled'}>Apply changes</button><button class="btn sm ghost" data-sh="cancelplan">Cancel</button></div>`;
      } else {
        const tagged = SH.overview && SH.overview.sparks.length && SH.overview.sparks.every((s) => s.device && s.device.tags.includes(SH.overview.tag));
        rules = `<p>Guests get SSH to these machines only, as ${sshUsersFor().map((u) => `<span class="mono">${esc(u)}</span>`).join(' / ')}, through Tailscale SSH: no keys to hand out, and removing a guest cuts access straight away. Your own access stays unchanged.</p>`
          + `<p class="warn small"><b>Guests log in as these same accounts.</b> They can read everything those accounts can: files, SSH keys, Claude / Antigravity / OpenClaw sign-ins, and your tmux sessions (read-only watching is a courtesy, not a lock). They could also leave something behind that keeps running after you remove them. Invite only people you'd trust with that account. Safer: share machines whose User is a dedicated, unprivileged account (no sudo, no personal sign-ins or keys).</p>`
          + `<div class="sh-inline"><button class="btn sm ${tagged ? '' : 'primary'}" data-sh="plan">${tagged ? 'Re-check access rules' : 'Review access rules…'}</button>${tagged ? '<span class="chip" style="--mc:#73daca">Machines tagged and locked down</span>' : ''}</div>`;
      }
    }

    // 5. Guests
    let guests = '';
    if (ts.hasToken) {
      const list = (SH.overview && SH.overview.guests) || [];
      guests = `<div class="sh-inline"><input type="email" id="sh-email" placeholder="friend@example.com" spellcheck="false"><button class="btn sm primary" data-sh="invite">Invite</button></div>`
        + (SH.result ? `<div class="sh-result"><p><b>Invite ready for ${esc(SH.result.email)}</b>. Tailscale has emailed them too. Send them this message:</p>
            <textarea readonly rows="9" id="sh-message">${esc(SH.result.message)}</textarea>
            <div class="sh-inline"><button class="btn sm primary" data-sh="copymsg">Copy message</button><button class="btn sm" data-sh="copycode">Copy invite code only</button></div></div>` : '')
        + (list.length ? `<div class="sh-guests">${list.map((g) => `<div class="sh-row"><b>${esc(g.email)}</b><span class="chip" style="--mc:${g.status === 'active' ? '#73daca' : '#e0af68'}">${esc(g.status)}</span><span class="spacer"></span><button class="btn sm ghost danger" data-sh="remove" data-email="${esc(g.email)}">Remove</button></div>`).join('')}</div>` : '<p class="dim">No guests yet.</p>');
    }

    body.innerHTML = `
      <div class="sh-head"><p class="dim">Invite people to your tailnet so they can use Claude, Antigravity and OpenClaw on your Linux machines from anywhere. They can reach those machines over SSH and nothing else.</p><button class="btn sm ghost" data-sh="refresh">${I.refresh} Refresh</button></div>
      ${SH.error ? `<div class="m-error">${esc(SH.error)}</div>` : ''}
      ${step(1, 'This PC on Tailscale', pc, pcDone ? 'done' : '')}
      ${step(2, 'Machines on your tailnet', `<div class="sh-rows">${rows}</div><p class="dim small">"Join Tailscale" opens a terminal on that machine running <span class="mono">sudo tailscale up --ssh</span>: type the sudo password, then open the login link it prints.</p>`, sparksDone ? 'done' : '')}
      ${step(3, 'Tailscale API token', token, ts.hasToken ? 'done' : '')}
      ${ts.hasToken ? step(4, 'Access rules', rules) : ''}
      ${ts.hasToken ? step(5, 'Guests', guests) : ''}
      <section class="sh-join"><h3>Got an invite code?</h3><p class="dim">Sign in to Tailscale with the invite link first, then paste the code to add the shared machines.</p>
        <div class="sh-inline"><input id="sh-code" placeholder="nexus-invite:…" spellcheck="false"><button class="btn sm" data-sh="joincode">Add machines</button></div></section>`;
  }

  async function busy(label, fn) {
    if (SH.busy) return;
    SH.busy = label;
    try { await fn(); } catch (err) { toast(err.message || String(err), 'bad', 6000); } finally { SH.busy = ''; }
  }

  // Preview + apply a policy change. Returns true when applied (or nothing to do).
  async function applyPlan(args, { confirmText } = {}) {
    const plan = await api.tsPlan({ machines: sparkArgs(), sshUsers: sshUsersFor(), ...args });
    if (!plan.ok) throw new Error(plan.error);
    if (!plan.changes.length) return true;
    if (confirmText && !confirm(`${confirmText}\n\n• ${plan.changes.join('\n• ')}`)) return false;
    const r = await api.tsApply(plan);
    if (!r.ok) throw new Error(r.error);
    return true;
  }

  async function onShareClick(e) {
    const b = e.target.closest('[data-sh]');
    if (!b) return;
    const act = b.dataset.sh;
    if (act === 'close') return closeShare();
    if (act === 'refresh') return busy('refresh', () => refreshShare());
    if (act === 'install') {
      if (IS_MAC) return window.open('https://tailscale.com/download/mac');
      closeShare();
      return launch('local', 'shell', {
        cmd: IS_WIN ? 'winget install -e --id Tailscale.Tailscale' : 'curl -fsSL https://tailscale.com/install.sh | sh && sudo tailscale up',
        name: 'Install Tailscale',
      });
    }
    if (act === 'join') {
      const m = machineById(b.dataset.m);
      closeShare();
      return launch(m.id, 'shell', { raw: true, cmd: 'sudo tailscale up --ssh && tailscale status | head -5', name: `Tailscale → ${m.name}` });
    }
    if (act === 'savetoken') return busy('token', async () => {
      const r = await api.tsSetToken($('#sh-token').value);
      if (!r.ok) throw new Error(r.error);
      toast(`Token saved. ${r.devices} devices on your tailnet.`, 'ok');
      await refreshShare();
    });
    if (act === 'cleartoken') { await api.tsClearToken(); SH.overview = null; return refreshShare(); }
    if (act === 'plan') return busy('plan', async () => {
      const p = await api.tsPlan({ machines: sparkArgs(), sshUsers: sshUsersFor() });
      if (!p.ok) throw new Error(p.error);
      SH.plan = p;
      renderShare();
    });
    if (act === 'cancelplan') { SH.plan = null; return renderShare(); }
    if (act === 'apply') return busy('apply', async () => {
      const r = await api.tsApply(SH.plan);
      if (!r.ok) throw new Error(r.error);
      SH.plan = null;
      toast('Access rules applied and machines tagged', 'ok');
      await refreshShare();
    });
    if (act === 'invite') return busy('invite', async () => {
      const email = $('#sh-email').value.trim().toLowerCase();
      if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) throw new Error('Enter the email address they use (or will use) to sign in to Tailscale');
      const sparks = sshMachines().filter((m) => m.tsHost);
      if (!sparks.length) throw new Error('Put your machines on the tailnet first (step 2)');
      if (!(await applyPlan({ addGuest: email }, { confirmText: `Give ${email} SSH access to your shared machines? This updates your tailnet:` }))) return;
      const inv = await api.tsInvite(email);
      if (!inv.ok) throw new Error(inv.error);
      const code = await api.tsEncodeInvite({
        tailnet: SH.ts.local.tailnet,
        machines: sparks.map((m) => ({ name: m.name, host: m.tsHost, user: m.user, icon: machineIconKey(m) === 'nvidia' ? 'nvidia' : '' })),
      });
      const users = sparks.map((m) => `${m.name}: ${m.user}`).join(', ');
      SH.result = {
        email, code,
        message: `You're invited to use my machines through Nexus.\n\n`
          + `1. Install Tailscale (https://tailscale.com/download) and join my tailnet with this link, signing in as ${email}:\n   ${inv.inviteUrl}\n\n`
          + `2. Install Nexus: https://github.com/mirrorsedgepro-stack/Intergrate-Your-AI/releases\n\n`
          + `3. In Nexus click the Share (people) icon, paste this invite code under "Got an invite code?", and click Add machines:\n   ${code}\n\n`
          + `You'll log in as ${users}. Please don't change other people's sessions or settings.`,
      };
      await refreshShare();
    });
    if (act === 'copymsg') { api.writeClipboard(SH.result.message); return toast('Invitation copied', 'ok'); }
    if (act === 'copycode') { api.writeClipboard(SH.result.code); return toast('Invite code copied', 'ok'); }
    if (act === 'remove') return busy('remove', async () => {
      const email = b.dataset.email;
      const g = ((SH.overview && SH.overview.guests) || []).find((x) => x.email === email) || {};
      if (!confirm(`Remove ${email}?\n\nThey lose SSH access to your machines immediately${g.userId ? ' and are removed from your tailnet (their devices too)' : ''}.`)) return;
      if (!(await applyPlan({ removeGuest: email }))) return;
      const r = await api.tsRevoke({ userId: g.userId, inviteId: g.inviteId });
      if (!r.ok) throw new Error(r.error);
      toast(`${email} removed`, 'ok');
      await refreshShare();
    });
    if (act === 'joincode') return busy('join', async () => {
      const r = await api.tsDecodeInvite($('#sh-code').value);
      if (!r.ok) throw new Error(r.error);
      const fresh = r.machines.filter((x) => !S.cfg.machines.some((m) => m.host === x.host || m.tsHost === x.host));
      if (!fresh.length) { toast('Those machines are already in Nexus', 'ok'); return; }
      // Show exactly where this code points before connecting: Nexus will log in there, run sessions there, and may
      // host team chat there. Only accept codes from someone you know, received over a channel you trust.
      const ok = await choose({
        title: 'Add these machines?',
        desc: `This invite${r.tailnet ? ` (tailnet ${r.tailnet})` : ''} adds: ${fresh.map((x) => `${x.name} = ${x.user || '?'}@${x.host}`).join(', ')}. `
          + 'Nexus will connect to them over Tailscale SSH without offering your SSH keys. Anything you type, paste or drop into their sessions goes to those machines. Only continue if you were expecting this invite.',
        options: [{ label: 'Cancel', value: false, ghost: true }, { label: 'Add machines', value: true, primary: true }],
      });
      if (!ok) return;
      let added = 0;
      for (const x of fresh) {
        S.cfg.machines.push({ id: `ts-${Date.now().toString(36)}${added}`, name: x.name, type: 'ssh', os: 'linux', host: x.host, tsHost: x.host, port: 22, user: x.user, keyPath: '', color: '#76B900', icon: x.icon || '', invited: true });
        added++;
      }
      S.cfg = await api.saveConfig(S.cfg);
      renderAll();
      for (const m of S.cfg.machines) if (m.type === 'ssh' && m.tsHost && (S.mstate[m.id] || {}).status !== 'online') connectMachine(m.id);
      toast(added ? `Added ${added} machine${added > 1 ? 's' : ''}. Connecting over Tailscale…` : 'Those machines are already in Nexus', 'ok');
      closeShare();
    });
  }

  // ---------------------------------------------------------------- team chat + presence
  // Chat and presence live on one "hub" SSH machine (~/.nexus/collab). Everyone's Nexus reads/writes them over its own
  // SSH connection; presence (who has which session open) drives the "someone's already in there" warnings.
  const TM = { hubId: null, live: false, messages: [], seen: new Set(), presence: [], unread: 0, open: false, status: '', starting: false };
  const collabCfg = () => (S.cfg.collab = S.cfg.collab || {});
  const myClient = () => collabCfg().client;
  const myName = () => collabCfg().name || (S.tsUser && S.tsUser.name) || 'Me';
  const me = () => ({ name: myName(), login: (S.tsUser && S.tsUser.login) || '', client: myClient() });
  const hostOf = (machineId) => (S.mstate[machineId] || {}).hostname || '';
  const userOf = (machineId) => (S.mstate[machineId] || {}).user || machineById(machineId).user || '';
  const hueOf = (s) => { let h = 0; for (const c of String(s)) h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
  const avatar = (name, cls = '') => `<span class="ava ${cls}" style="--h:${hueOf(name)}" title="${esc(name)}">${esc(String(name).trim().slice(0, 1).toUpperCase() || '?')}</span>`;
  const others = () => TM.presence.filter((p) => p.client !== myClient());

  async function initTeam() {
    if (!collabCfg().client) {
      collabCfg().client = Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, '0')).join('');
      api.patchConfig({ collab: S.cfg.collab });
    }
    api.tsStatus().then((t) => { S.tsUser = t && t.local && t.local.user; renderTeam(); }).catch(() => {});
    setInterval(heartbeat, 15000);
  }

  // The hub: Settings override (by hostname), else the online SSH machine whose hostname sorts first, so everyone
  // connected to the same machines lands on the same chat without configuring anything.
  function pickHub() {
    const online = S.cfg.machines.filter((m) => m.type === 'ssh' && (S.mstate[m.id] || {}).status === 'online' && hostOf(m.id));
    const want = collabCfg().hub;
    if (want) return online.find((m) => hostOf(m.id) === want) || null;
    return online.sort((a, b) => hostOf(a.id).localeCompare(hostOf(b.id)))[0] || null;
  }
  async function ensureHub() {
    const m = pickHub();
    if (!m || TM.starting || (TM.hubId === m.id && TM.live)) return;
    TM.starting = true;
    const r = await api.collabStart(m.id);
    TM.starting = false;
    if (r.ok) { TM.hubId = m.id; TM.live = true; heartbeat(); }
    renderTeam();
  }
  api.onCollabState(({ machineId, live }) => { if (machineId === TM.hubId) { TM.live = live; renderTeam(); } });

  function mySessions() {
    return ordered().filter((s) => s.tmuxName && s.status === 'running' && machineById(s.machineId).type === 'ssh').map((s) => ({
      host: hostOf(s.machineId), user: userOf(s.machineId), tmux: s.tmuxName, preset: s.presetId, name: s.name, cwd: s.cwd || '',
    }));
  }
  async function heartbeat() {
    if (!TM.live) { ensureHub(); return; }
    const r = await api.collabHeartbeat({ ...me(), status: TM.status, sessions: mySessions() });
    if (r.ok) { TM.presence = r.presence; renderTeam(); renderChrome(); }
  }

  api.onCollabMessage((m) => {
    if (TM.seen.has(m.id)) return;
    TM.seen.add(m.id);
    TM.messages.push(m);
    if (TM.messages.length > 500) TM.messages.splice(0, TM.messages.length - 500);
    const mine = m.from.client === myClient();
    const fresh = Date.now() - m.ts < 60000;
    if (!mine && fresh && m.kind !== 'activity') {
      if (!TM.open) TM.unread++;
      if (mentions(m.text, myName())) {
        api.notify({ title: `${m.from.name} mentioned you`, body: m.text.slice(0, 200) });
        api.flash();
      }
    }
    renderTeam(true);
  });

  async function postTeam(text, kind = 'msg') {
    const r = await api.collabPost({ from: me(), text, kind });
    if (!r.ok) toast(r.error, 'bad');
    return r.ok;
  }
  // Tell the team when an agent starts on a shared machine (so nobody opens a second one in the same folder).
  function announceLaunch(s) {
    if (collabCfg().announce === false || !TM.live || !isAgent(s) || machineById(s.machineId).type !== 'ssh') return;
    postTeam(`started ${presetById(s.presetId).name} in ${s.cwd || '~'} on ${hostOf(s.machineId)}`, 'activity');
  }

  // ---- conflict checks ----
  function whoIn(machineId, tmux) {
    return others().filter((p) => p.sessions.some((x) => x.host === hostOf(machineId) && x.user === userOf(machineId) && x.tmux === tmux)).map((p) => p.name);
  }
  function whoInFolder(machineId, cwd) {
    const norm = (d) => String(d || '').replace(/\/+$/, '') || '~';
    return others().flatMap((p) => p.sessions
      .filter((x) => x.host === hostOf(machineId) && x.user === userOf(machineId) && isAgent({ presetId: x.preset }) && norm(x.cwd) === norm(cwd))
      .map((x) => ({ person: p.name, preset: presetById(x.preset).name })));
  }
  function choose({ title, desc, options }) {
    return new Promise((resolve) => {
      const ov = document.createElement('div');
      ov.className = 'overlay';
      ov.innerHTML = `<div class="modal prompt"><div class="modal-head"><h2>${esc(title)}</h2></div><div class="modal-body"><p class="prompt-desc" style="word-break:normal">${esc(desc)}</p></div>
        <div class="modal-foot"><span class="spacer"></span>${options.map((o, i) => `<button class="btn ${o.primary ? 'primary' : o.ghost ? 'ghost' : ''}" data-i="${i}">${esc(o.label)}</button>`).join('')}</div></div>`;
      document.body.appendChild(ov);
      const done = (v) => { ov.remove(); resolve(v); focused()?.term.focus(); };
      ov.close = () => done(null);
      ov.addEventListener('click', (e) => { const b = e.target.closest('[data-i]'); if (b) done(options[Number(b.dataset.i)].value); else if (e.target === ov) done(null); });
      ov.querySelector('.btn.primary')?.focus();
    });
  }
  // Before attaching to a session someone else is in. Returns 'open' | 'watch' | null (cancel).
  async function checkSessionConflict(machineId, tmux) {
    const people = whoIn(machineId, tmux);
    if (!people.length) return 'open';
    return choose({
      title: `${people.join(', ')} ${people.length > 1 ? 'are' : 'is'} in this session`,
      desc: `${tmux} on ${machineById(machineId).name} is open in ${people.join(' and ')}'s Nexus. If you both type, your keystrokes mix into the same prompt. Watch it read-only, or open it anyway (say so in team chat). Read-only stops your own keystrokes reaching it; it isn't a permission boundary between people sharing an account.`,
      options: [{ label: 'Cancel', value: null, ghost: true }, { label: 'Open anyway', value: 'open' }, { label: 'Watch read-only', value: 'watch', primary: true }],
    });
  }
  // Before starting an agent where someone else's agent already works. Returns true to go ahead.
  async function checkFolderConflict(machineId, presetId, cwd) {
    if (!isAgent({ presetId }) || machineById(machineId).type !== 'ssh') return true;
    const hits = whoInFolder(machineId, cwd || '~');
    if (!hits.length) return true;
    const who = hits.map((h) => `${h.person} (${h.preset})`).join(', ');
    const v = await choose({
      title: 'Someone is already working there',
      desc: `${who} ${hits.length > 1 ? 'are' : 'is'} running an agent in ${cwd || '~'} on ${machineById(machineId).name}. Two agents editing the same files can overwrite each other's changes.`,
      options: [{ label: 'Cancel', value: null, ghost: true }, { label: `Message ${hits[0].person}`, value: 'msg' }, { label: 'Start anyway', value: 'go', primary: true }],
    });
    if (v === 'msg') { openTeam(`@${hits[0].person.split(/\s+/)[0]} `); return false; }
    return v === 'go';
  }

  // ---- chat drawer ----
  function openTeam(prefill) {
    TM.open = true;
    TM.unread = 0;
    document.body.classList.add('team-open');
    $('#team').hidden = false;
    renderTeam(true);
    const input = $('#team-input');
    if (typeof prefill === 'string') { input.value = prefill; }
    input.focus();
    scheduleFit();
    ensureHub();
  }
  function closeTeam() {
    TM.open = false;
    document.body.classList.remove('team-open');
    $('#team').hidden = true;
    renderTeam();
    scheduleFit();
    focused()?.term.focus();
  }
  const toggleTeam = () => (TM.open ? closeTeam() : openTeam());

  const fmtTime = (ts) => new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const linkify = (t) => esc(t).replace(/https?:\/\/[^\s<]+/g, (u) => `<a href="${u}" target="_blank" rel="noopener noreferrer">${u}</a>`)
    .replace(/(^|\s)(@[\w.-]+)/g, '$1<b class="mention">$2</b>');

  function renderTeam(scroll = false) {
    const badge = $('#team-badge');
    if (badge) { badge.textContent = TM.unread > 9 ? '9+' : String(TM.unread); badge.hidden = !TM.unread; }
    const dot = $('#btn-team');
    if (dot) dot.classList.toggle('live', TM.live);
    if (!TM.open) return;
    const hub = TM.hubId ? machineById(TM.hubId) : null;
    $('#team-hub').innerHTML = TM.live && hub ? `<span class="dot online"></span>on ${esc(hub.name)}` : '<span class="dot connecting"></span>connecting…';
    const people = [...TM.presence].sort((a, b) => (a.client === myClient() ? -1 : b.client === myClient() ? 1 : a.name.localeCompare(b.name)));
    $('#team-people').innerHTML = people.length ? people.map((p) => `<div class="tp">
        ${avatar(p.name)}<div class="tp-main"><div class="tp-name">${esc(p.name)}${p.client === myClient() ? ' <span class="dim">(you)</span>' : ''}</div>
        ${p.status ? `<div class="tp-status">${esc(p.status)}</div>` : ''}
        ${p.sessions.map((x) => `<div class="tp-sess">${glyph(x.preset, 'sm')}<span>${esc(presetById(x.preset).name)}${x.cwd ? ` · ${esc(shortPath(x.cwd))}` : ''}</span><span class="dim">${esc(x.host)}</span></div>`).join('')}
        </div></div>`).join('') : '<div class="dim small">Nobody else online.</div>';
    const list = $('#team-msgs');
    const atBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
    // Names in chat are self-declared and anyone who can write to the hub can post: show who a name belongs to (client
    // id, Tailscale login) on hover, and flag a name used from more than one Nexus.
    const clientsByName = new Map();
    for (const x of [...TM.messages.map((m) => m.from), ...TM.presence]) {
      const n = String(x.name || '').trim().toLowerCase();
      if (!clientsByName.has(n)) clientsByName.set(n, new Set());
      if (x.client) clientsByName.get(n).add(x.client);
    }
    const who = (f) => {
      const shared = (clientsByName.get(String(f.name || '').trim().toLowerCase()) || new Set()).size > 1;
      const tip = `Self-declared name${f.login ? ` · Tailscale login ${f.login}` : ''} · device ${String(f.client || '?').slice(0, 6)}${f.client === myClient() ? ' (you)' : ''}`;
      return `<b title="${esc(tip)}">${esc(f.name)}</b>${shared ? ' <span class="chip" style="--mc:#e0af68" title="More than one Nexus uses this name: check the device id on hover">same name, other device</span>' : ''}`;
    };
    let prev = null;
    list.innerHTML = TM.messages.map((m) => {
      if (m.kind === 'activity') { prev = null; return `<div class="tm-act">${avatar(m.from.name, 'xs')}${who(m.from)} ${esc(m.text)} <span class="dim">${fmtTime(m.ts)}</span></div>`; }
      if (m.kind === 'status') { prev = null; return `<div class="tm-act">${avatar(m.from.name, 'xs')}${who(m.from)} ${m.text ? `set status: <i>${esc(m.text)}</i>` : 'cleared their status'} <span class="dim">${fmtTime(m.ts)}</span></div>`; }
      const cont = prev && prev.from.client === m.from.client && m.ts - prev.ts < 5 * 60000;
      prev = m;
      return cont ? `<div class="tm-msg cont"><div class="tm-text">${linkify(m.text)}</div></div>`
        : `<div class="tm-msg">${avatar(m.from.name)}<div class="tm-body"><div class="tm-meta">${who(m.from)}<span class="dim">${fmtTime(m.ts)}</span></div><div class="tm-text">${linkify(m.text)}</div></div></div>`;
    }).join('') || '<div class="tm-empty">No messages yet. Say what you\'re working on so nobody doubles up.<br><span class="dim">Tip: <b>/status Training on the GPU box until 3pm</b> sets your status.</span></div>';
    if (scroll || atBottom) list.scrollTop = list.scrollHeight;
  }

  async function sendTeam() {
    const input = $('#team-input');
    const text = input.value.trim();
    if (!text) return;
    if (!TM.live) { toast('Team chat is not connected yet: connect to a shared machine first', 'warn'); return; }
    const st = /^\/status\b\s*(.*)$/s.exec(text);
    if (st) {
      TM.status = st[1].trim();
      collabCfg().status = TM.status;
      api.patchConfig({ collab: S.cfg.collab });
      if (await postTeam(TM.status, 'status')) { input.value = ''; heartbeat(); }
      return;
    }
    if (await postTeam(text)) input.value = '';
  }

  // Others' presence on my sidebar items: who else is in a session / has a detached one open.
  function othersOnSession(machineId, tmux) {
    const names = tmux ? whoIn(machineId, tmux) : [];
    return names.length ? `<span class="s-who" title="Also open in ${esc(names.join(', '))}'s Nexus">${names.map((n) => avatar(n, 'xs')).join('')}</span>` : '';
  }

  // ---------------------------------------------------------------- toasts
  function toast(msg, kind = '', ms = 3800, action = null) {
    const el = document.createElement('div');
    el.className = `toast ${kind}`;
    if (kind === 'warn') el.style.setProperty('--c', '#e0af68');
    el.textContent = msg;
    if (action) {
      const b = document.createElement('button');
      b.className = 'btn sm primary';
      b.textContent = action.label;
      b.onclick = () => { el.remove(); action.run(); };
      el.appendChild(b);
    }
    $('#toasts').appendChild(el);
    setTimeout(() => { el.style.transition = 'opacity .3s'; el.style.opacity = '0'; setTimeout(() => el.remove(), 300); }, ms);
  }

  // ---------------------------------------------------------------- global wiring
  function onAppClick(e) {
    const b = e.target.closest('[data-act]');
    if (!b) return;
    const act = b.dataset.act;
    e.stopPropagation();
    if (act === 'launch') (e.shiftKey ? launchInFolder : launch)(b.dataset.m, b.dataset.p);
    else if (act === 'show') show(b.dataset.s);
    else if (act === 'close') closeSession(b.dataset.s);
    else if (act === 'reattach') reattach(b.dataset.m, b.dataset.name);
    else if (act === 'kill') {
      if (confirm(`Kill tmux session "${b.dataset.name}"? Anything running in it will stop.`)) api.killTmux(b.dataset.m, b.dataset.name).then(() => refreshMachine(b.dataset.m));
    } else if (act === 'refresh') connectMachine(b.dataset.m);
    else if (act === 'mmenu') { const r = b.getBoundingClientRect(); machineMenu(r.left, r.bottom + 4, machineById(b.dataset.m)); }
    else if (act === 'settings') openSettings();
  }
  $('#machines').addEventListener('click', onAppClick);
  $('#welcome').addEventListener('click', onAppClick);
  $('#machines').addEventListener('contextmenu', (e) => {
    const l = e.target.closest('[data-act="launch"]');
    if (l) { e.preventDefault(); launchInFolder(l.dataset.m, l.dataset.p); return; }
    const item = e.target.closest('[data-act="show"]');
    const card = e.target.closest('[data-machine]');
    e.preventDefault();
    if (item) termMenu(e, S.sessions.get(item.dataset.s));
    else if (card) machineMenu(e.clientX, e.clientY, machineById(card.dataset.machine));
  });
  $('#tabs').addEventListener('click', (e) => {
    const x = e.target.closest('[data-close]');
    if (x) { closeSession(x.dataset.close); return; }
    const t = e.target.closest('.tab');
    if (t) show(t.dataset.s);
  });
  $('#tabs').addEventListener('auxclick', (e) => { const t = e.target.closest('.tab'); if (t && e.button === 1) closeSession(t.dataset.s); });
  $('#tabs').addEventListener('contextmenu', (e) => { const t = e.target.closest('.tab'); if (t) { e.preventDefault(); termMenu(e, S.sessions.get(t.dataset.s)); } });
  $('#tabs').addEventListener('wheel', (e) => { $('#tabs').scrollLeft += e.deltaY; }, { passive: true });
  $('#layout-switch').addEventListener('click', (e) => { const b = e.target.closest('[data-layout]'); if (b) setLayout(Number(b.dataset.layout)); });
  $('#btn-palette').addEventListener('click', () => openPalette());
  $('#btn-workspaces').addEventListener('click', openWorkspaces);
  $('#btn-share').addEventListener('click', openShare);
  $('#btn-team').addEventListener('click', toggleTeam);
  $('#team-close').addEventListener('click', closeTeam);
  $('#team-input').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendTeam(); }
    if (e.key === 'Escape') { e.preventDefault(); closeTeam(); }
  });
  $('#team-send').addEventListener('click', sendTeam);

  api.onFocus((f) => {
    S.winFocused = f;
    if (f) { for (const id of S.panes) { const s = S.sessions.get(id); if (s) clearDone(s); } }
  });
  api.onNotifyClick((id) => show(id));
  new ResizeObserver(() => { scheduleFit(); positionGutters(); }).observe($('#panes'));

  // ---------------------------------------------------------------- boot
  async function init() {
    api.hello();
    S.cfg = await api.getConfig();
    const parking = document.createElement('div');
    parking.id = 'parking';
    document.body.appendChild(parking);
    await Promise.all(['400 14px', '700 14px', 'italic 400 14px'].map((f) => document.fonts.load(`${f} "JetBrains Mono"`))).catch(() => {});

    S.mstate.local = { status: 'online' };
    api.probeLocal().then((t) => { S.mstate.local.tools = t; renderChrome(); if (!S.order.length) renderWelcome(); });
    renderAll();
    // Bring back last time's sessions: remote ones reattach to their tmux session, local ones restart in their folder.
    if (S.cfg.appearance.restoreSession !== false && S.cfg.lastSession) await openLayout(S.cfg.lastSession, { resume: true });
    for (const m of S.cfg.machines) {
      if ((m.type === 'ssh' && (m.host || m.tsHost)) || m.type === 'wsl') if ((S.mstate[m.id] || {}).status !== 'online') connectMachine(m.id);
    }
    api.appInfo().then((i) => { S.version = i.version; });
    TM.status = collabCfg().status || '';
    initTeam();
  }
  init();
})();
