// OS differences in one place: which OS we're on, finding programs, the login-shell PATH on macOS/Linux, and WSL.
const { execFile } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const IS_WIN = process.platform === 'win32';
const IS_MAC = process.platform === 'darwin';

// Nexus's machine "os" values: windows | mac | linux.
function platformOs(p = process.platform) {
  return p === 'win32' ? 'windows' : p === 'darwin' ? 'mac' : 'linux';
}

// User-level install dirs (Claude/Antigravity installers, npm globals, Homebrew) that GUI-launched apps often lack on PATH.
function extraPaths() {
  const home = os.homedir();
  if (IS_WIN) return [path.join(home, '.local', 'bin'), path.join(process.env.LOCALAPPDATA || '', 'agy', 'bin'), path.join(process.env.APPDATA || '', 'npm')];
  return [path.join(home, '.local', 'bin'), '/opt/homebrew/bin', '/usr/local/bin'];
}

// Locate an executable on PATH (+ extra dirs) without spawning anything.
function which(cmd, envPath = process.env.PATH || process.env.Path || '') {
  const dirs = [...envPath.split(path.delimiter), ...extraPaths()].filter(Boolean);
  const exts = IS_WIN ? (process.env.PATHEXT || '.EXE;.CMD;.BAT;.COM').split(';').map((e) => e.toLowerCase()).concat(['']) : [''];
  for (const d of dirs) {
    for (const e of exts) {
      const p = path.join(d, cmd + e);
      try {
        const st = fs.statSync(p);
        if (st.isFile() && (IS_WIN || (st.mode & 0o111))) return p;
      } catch { /* not here */ }
    }
  }
  return null;
}

// macOS/Linux apps started from Finder or a desktop launcher get a minimal PATH (/usr/bin:/bin…), so `claude`
// installed in ~/.local/bin or Homebrew isn't found. Ask the user's login shell for its PATH once at startup.
function loadLoginShellPath() {
  if (IS_WIN) return Promise.resolve(false);
  const shell = process.env.SHELL || (IS_MAC ? '/bin/zsh' : '/bin/bash');
  return new Promise((resolve) => {
    execFile(shell, ['-ilc', 'printf "__NEXUS_PATH__%s__NEXUS_PATH__" "$PATH"'], { timeout: 6000, env: { ...process.env, DISABLE_AUTO_UPDATE: 'true' } }, (err, stdout) => {
      const m = /__NEXUS_PATH__(.*?)__NEXUS_PATH__/s.exec(stdout || '');
      if (m && m[1]) { process.env.PATH = [...new Set([...m[1].split(':'), ...(process.env.PATH || '').split(':')])].filter(Boolean).join(':'); resolve(true); }
      else resolve(false);
    });
  });
}

// ---------------------------------------------------------------- WSL (Windows only)
// C:\Users\me\x -> /mnt/c/Users/me/x (WSL's default automount).
function toWslPath(p) {
  const m = /^([A-Za-z]):[\\/](.*)$/.exec(p || '');
  return m ? `/mnt/${m[1].toLowerCase()}/${m[2].replace(/\\/g, '/')}` : String(p || '').replace(/\\/g, '/');
}

// `wsl.exe -l` prints UTF-16LE; the default distro is marked "(Default)". Docker's internal distros are skipped.
function parseWslList(buf) {
  const text = Buffer.isBuffer(buf) ? buf.toString('utf16le') : String(buf);
  const out = [];
  for (const raw of text.replace(/\u0000/g, '').split(/\r?\n/).slice(1)) {
    const line = raw.trim();
    if (!line) continue;
    const isDefault = /\(Default\)|\(Par défaut\)|\(Standard\)/i.test(line);
    const name = line.replace(/\s*\(.*\)\s*$/, '').trim();
    if (!name || /^docker-desktop/i.test(name)) continue;
    out.push({ name, isDefault });
  }
  return out;
}

function listWsl() {
  if (!IS_WIN) return Promise.resolve([]);
  return new Promise((resolve) => {
    execFile('wsl.exe', ['-l'], { encoding: 'buffer', windowsHide: true, timeout: 8000 }, (err, stdout) => {
      resolve(err ? [] : parseWslList(stdout));
    });
  });
}

// Run a bash script inside a distro (login shell so ~/.local/bin, nvm etc. are on PATH).
function wslExec(distro, script, timeout = 20000) {
  return new Promise((resolve) => {
    execFile('wsl.exe', ['-d', distro, '-e', 'bash', '-lc', script], { windowsHide: true, timeout, maxBuffer: 4 * 1024 * 1024 }, (err, stdout, stderr) => {
      resolve({ code: err ? (err.code || 1) : 0, stdout: String(stdout || ''), stderr: String(stderr || '') });
    });
  });
}

module.exports = { IS_WIN, IS_MAC, platformOs, extraPaths, which, loadLoginShellPath, toWslPath, parseWslList, listWsl, wslExec };
