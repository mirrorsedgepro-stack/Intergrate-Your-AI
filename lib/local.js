// Local sessions: this computer's shell (PowerShell / Git Bash on Windows, your login shell on macOS/Linux)
// and, on Windows, WSL distros. All through a pseudo-terminal (ConPTY on Windows).
const pty = require('node-pty');
const os = require('os');
const path = require('path');
const fs = require('fs');
const { execFile } = require('child_process');
const { IS_WIN, extraPaths, which, wslExec } = require('./platform');
const { LINUX_SCRIPT, parseLinux, NetRate } = require('./stats');

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

// ---------------------------------------------------------------- stats for this computer
const run = (file, args, timeout = 6000) => new Promise((resolve) => {
  execFile(file, args, { windowsHide: true, timeout, maxBuffer: 1024 * 1024 }, (err, stdout) => resolve(err ? '' : String(stdout || '')));
});
const localNet = new NetRate();
const wslNets = new Map();
let lastCpu = null;

// CPU busy fraction since the previous call (os.loadavg is always 0 on Windows).
function cpuBusy() {
  const cpus = os.cpus();
  const cur = cpus.reduce((a, c) => { for (const k in c.times) a[k] = (a[k] || 0) + c.times[k]; return a; }, {});
  const total = Object.values(cur).reduce((a, b) => a + b, 0);
  const prev = lastCpu;
  lastCpu = { ...cur, total };
  if (!prev || total <= prev.total) return null;
  return 1 - (cur.idle - prev.idle) / (total - prev.total);
}

// Total bytes received/sent on this computer, as the Linux script reports them.
async function netCounters() {
  if (process.platform === 'linux') {
    try {
      let rx = 0, tx = 0;
      for (const line of fs.readFileSync('/proc/net/dev', 'utf8').split('\n').slice(2)) {
        const a = line.trim().split(/[: ]+/);
        if (a.length < 10 || /^(lo|docker|veth|br-|virbr)/.test(a[0])) continue;
        rx += Number(a[1]); tx += Number(a[9]);
      }
      return { rx, tx };
    } catch { return null; }
  }
  if (IS_WIN) {
    // "Bytes   123   456" (received, sent) in any language: the first row of numbers after the header.
    const out = await run('netstat.exe', ['-e']);
    const m = /^\S+\s+(\d+)\s+(\d+)\s*$/m.exec(out);
    return m ? { rx: Number(m[1]), tx: Number(m[2]) } : null;
  }
  // macOS: sum Ibytes / Obytes over physical interfaces (one <Link#n> row per interface).
  const out = await run('netstat', ['-ibn']);
  let rx = 0, tx = 0;
  for (const line of out.split('\n')) {
    const f = line.trim().split(/\s+/);
    if (f.length < 10 || !/^<Link#/.test(f[2]) || /^(lo|utun|awdl|llw|bridge|vmnet)/.test(f[0])) continue;
    rx += Number(f[6]) || 0; tx += Number(f[9]) || 0;
  }
  return out ? { rx, tx } : null;
}

async function gpu() {
  const smi = which('nvidia-smi') || (IS_WIN && ['C:\\Windows\\System32\\nvidia-smi.exe', 'C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvidia-smi.exe'].find((p) => fs.existsSync(p)));
  if (!smi) return null;
  const out = await run(smi, ['--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total,power.draw', '--format=csv,noheader,nounits'], 4000);
  const line = out.split('\n')[0];
  return line ? parseLinux(`gpu:${line}`).gpu : null;
}

async function diskOf(dir) {
  try {
    const st = await fs.promises.statfs(dir);
    return { total: st.blocks * st.bsize, used: (st.blocks - st.bfree) * st.bsize, avail: st.bavail * st.bsize, mount: IS_WIN ? path.parse(dir).root : dir };
  } catch { return null; }
}

async function stats() {
  const cpus = os.cpus().length;
  const busy = cpuBusy();
  const [g, disk, net] = await Promise.all([gpu(), diskOf(os.homedir()), netCounters()]);
  const out = {
    gpu: g,
    cpus,
    // Present the same way as Linux: a 1-minute load; synthesise one from the busy fraction where loadavg is absent.
    load: IS_WIN ? (busy == null ? null : Number((busy * cpus).toFixed(2))) : os.loadavg()[0],
    memTotal: os.totalmem(),
    memAvail: os.freemem(),
    disk,
    net,
    now: Date.now(),
  };
  return localNet.finish(out);
}

async function wslStats(distro) {
  const { stdout } = await wslExec(distro, LINUX_SCRIPT, 8000);
  if (!wslNets.has(distro)) wslNets.set(distro, new NetRate());
  return wslNets.get(distro).finish(parseLinux(stdout));
}

module.exports = { spawn, probe, wslProbe, wslDirs, stats, wslStats };
