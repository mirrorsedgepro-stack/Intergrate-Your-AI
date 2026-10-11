// Local sessions: this computer's shell (PowerShell / Git Bash on Windows, your login shell on macOS/Linux)
// and, on Windows, WSL distros. All through a pseudo-terminal (ConPTY on Windows).
const pty = require('node-pty');
const os = require('os');
const path = require('path');
const fs = require('fs');
const { IS_WIN, extraPaths, which, wslExec } = require('./platform');

const GIT_BASH = IS_WIN
  ? ['C:\\Program Files\\Git\\bin\\bash.exe', 'C:\\Program Files (x86)\\Git\\bin\\bash.exe'].find((p) => fs.existsSync(p))
  : null;

async function probe() {
  return {
    claude: !!which('claude'),
    agy: !!which('agy'),
    openclaw: !!which('openclaw'),
    pwsh: IS_WIN && !!which('pwsh'),
    gitbash: !!GIT_BASH,
    shell: true,
  };
}

const shq = (s) => `'${String(s).replace(/'/g, `'\\''`)}'`;

function baseEnv() {
  const env = { ...process.env, TERM: 'xterm-256color', COLORTERM: 'truecolor', TERM_PROGRAM: 'Nexus' };
  const key = Object.keys(env).find((k) => k.toLowerCase() === 'path') || 'PATH';
  env[key] = [...extraPaths(), env[key]].join(path.delimiter);
  return env;
}

// cmd: '' -> plain shell, '__gitbash__' -> Git Bash, anything else -> run it, then drop into the shell when it exits.
// wsl: distro name -> run inside that WSL distro instead of on Windows.
function spawn({ cmd, cols, rows, cwd, script, wsl }) {
  const env = baseEnv();
  const opts = { name: 'xterm-256color', cols, rows, cwd: os.homedir(), env };

  if (wsl) {
    const inner = cmd ? `${cmd}; exec "$SHELL" -l` : 'exec "$SHELL" -l';
    return pty.spawn('wsl.exe', ['-d', wsl, '--cd', cwd || '~', '-e', 'bash', '-lic', inner], { ...opts, useConpty: true });
  }

  if (cwd && fs.existsSync(cwd)) opts.cwd = cwd;

  if (IS_WIN) {
    const ps = which('pwsh') || 'powershell.exe';
    const base = ['-NoLogo', '-ExecutionPolicy', 'Bypass'];
    opts.useConpty = true;
    if (script) return pty.spawn(ps, [...base, '-NoExit', '-File', script.file, ...(script.args || [])], opts);
    if (cmd === '__gitbash__') {
      if (!GIT_BASH) throw new Error('Git Bash not found');
      return pty.spawn(GIT_BASH, ['--login', '-i'], opts);
    }
    if (!cmd) return pty.spawn(ps, base, opts);
    return pty.spawn(ps, [...base, '-NoExit', '-Command', cmd], opts);
  }

  // macOS / Linux: the user's login shell.
  const shell = process.env.SHELL || (process.platform === 'darwin' ? '/bin/zsh' : '/bin/bash');
  if (script) return pty.spawn(shell, ['-lic', `bash ${[script.file, ...(script.args || [])].map(shq).join(' ')}; exec ${shq(shell)} -l`], opts);
  if (!cmd) return pty.spawn(shell, ['-l'], opts);
  return pty.spawn(shell, ['-lic', `${cmd}; exec ${shq(shell)} -l`], opts);
}

// ---------------------------------------------------------------- WSL machines
async function wslProbe(distro) {
  const script = 'for t in tmux claude agy openclaw node git; do if command -v $t >/dev/null 2>&1; then echo "tool:$t=1"; else echo "tool:$t=0"; fi; done; '
    + 'echo "host:$(hostname)"; echo "home:$HOME"; echo "user:$(id -un)"';
  const { stdout, code, stderr } = await wslExec(distro, script);
  if (code && !stdout) throw new Error(stderr.trim() || `WSL distro ${distro} did not start`);
  const tools = {};
  let hostname = '', home = '', user = '';
  for (const line of stdout.split(/\r?\n/)) {
    let mm;
    if ((mm = line.match(/^tool:([\w-]+)=([01])/))) tools[mm[1]] = mm[2] === '1';
    else if ((mm = line.match(/^host:(.*)/))) hostname = mm[1].trim();
    else if ((mm = line.match(/^home:(.*)/))) home = mm[1].trim();
    else if ((mm = line.match(/^user:(.*)/))) user = mm[1].trim();
  }
  return { tools, hostname, home, user, sessions: [] };
}

async function wslDirs(distro) {
  const script = 'cd ~ || exit 0; '
    + 'find . -maxdepth 4 \\( -name node_modules -o -name .cache -o -name .local -o -name .npm -o -name .nvm -o -name snap \\) -prune -o -name .git -print 2>/dev/null | head -n 300 | sed "s#/\\.git\\$##; s#^\\.#repo:~#"; '
    + 'find . -mindepth 1 -maxdepth 2 -type d -not -path "*/.*" -not -path "*/node_modules*" 2>/dev/null | head -n 400 | sed "s#^\\.#dir:~#"';
  const { stdout } = await wslExec(distro, script);
  const repos = [], dirs = [];
  for (const line of stdout.split(/\r?\n/)) {
    if (line.startsWith('repo:') && !/\/\./.test(line.slice(5))) repos.push(line.slice(5));
    else if (line.startsWith('dir:')) dirs.push(line.slice(4));
  }
  return { repos, dirs };
}

module.exports = { spawn, probe, wslProbe, wslDirs };
