//! Local sessions: this computer's shell (PowerShell / Git Bash on Windows, your login shell on macOS/Linux)
//! and, on Windows, WSL distros. All through a pseudo-terminal (ConPTY on Windows).
use crate::platform::{self, extra_paths, which, wsl_exec, IS_MAC, IS_WIN};
use crate::remote::{parse_dirs, parse_probe, shq, Dirs, Probe};
use crate::stats::{parse_gpu_csv, parse_linux, Disk, Gpu, Net, NetRate, Stats, LINUX_SCRIPT};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub fn git_bash() -> Option<PathBuf> {
    if !IS_WIN { return None; }
    [r"C:\Program Files\Git\bin\bash.exe", r"C:\Program Files (x86)\Git\bin\bash.exe"].iter().map(PathBuf::from).find(|p| p.exists())
}

pub fn probe() -> HashMap<String, bool> {
    [("claude", which("claude").is_some()), ("agy", which("agy").is_some()), ("openclaw", which("openclaw").is_some()),
     ("pwsh", IS_WIN && which("pwsh").is_some()), ("gitbash", git_bash().is_some()), ("shell", true)]
        .into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

#[derive(Default)]
pub struct SpawnSpec {
    /// "" -> plain shell, "__gitbash__" -> Git Bash, anything else -> run it, then drop into the shell when it exits.
    pub cmd: String,
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
    /// A script file to run (with arguments) before dropping into the shell.
    pub script: Option<(PathBuf, Vec<String>)>,
    /// Distro name -> run inside that WSL distro instead of on Windows.
    pub wsl: Option<String>,
}

pub struct Pty {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub reader: Box<dyn std::io::Read + Send>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
}

fn login_shell() -> String { std::env::var("SHELL").unwrap_or_else(|_| if IS_MAC { "/bin/zsh" } else { "/bin/bash" }.into()) }

/// The program and arguments for a local session (separate from spawning, so it can be tested).
pub fn command_line(s: &SpawnSpec, shell: &str) -> Result<(String, Vec<String>), String> {
    let v = |a: &[&str]| a.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
    if let Some(distro) = &s.wsl {
        let inner = if s.cmd.is_empty() { "exec \"$SHELL\" -l".to_string() } else { format!("{}; exec \"$SHELL\" -l", s.cmd) };
        return Ok(("wsl.exe".into(), v(&["-d", distro, "--cd", s.cwd.as_deref().filter(|c| !c.is_empty()).unwrap_or("~"), "-e", "bash", "-lic", &inner])));
    }
    if IS_WIN {
        let ps = which("pwsh").map_or_else(|| "powershell.exe".to_string(), |p| p.to_string_lossy().into_owned());
        let mut args = v(&["-NoLogo", "-ExecutionPolicy", "Bypass"]);
        if let Some((file, extra)) = &s.script {
            args.extend(v(&["-NoExit", "-File"]));
            args.push(file.to_string_lossy().into_owned());
            args.extend(extra.iter().cloned());
        } else if s.cmd == "__gitbash__" {
            return Ok((git_bash().ok_or("Git Bash not found")?.to_string_lossy().into_owned(), v(&["--login", "-i"])));
        } else if !s.cmd.is_empty() {
            args.extend(v(&["-NoExit", "-Command", &s.cmd]));
        }
        return Ok((ps, args));
    }
    // macOS / Linux: the user's login shell.
    let shell = shell.to_string();
    let args = if let Some((file, extra)) = &s.script {
        let words: Vec<String> = std::iter::once(file.to_string_lossy().into_owned()).chain(extra.iter().cloned()).map(|w| shq(&w)).collect();
        v(&["-lic", &format!("bash {}; exec {} -l", words.join(" "), shq(&shell))])
    } else if s.cmd.is_empty() { v(&["-l"]) } else { v(&["-lic", &format!("{}; exec {} -l", s.cmd, shq(&shell))]) };
    Ok((shell, args))
}

pub fn spawn(s: &SpawnSpec) -> Result<Pty, String> {
    let (program, args) = command_line(s, &login_shell())?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "Nexus");
    let path = std::env::join_paths(extra_paths().into_iter().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap_or_default();
    cmd.env("PATH", path);
    let cwd = s.cwd.as_deref().filter(|_| s.wsl.is_none()).map(platform::expand_home).filter(|p| p.is_dir()).unwrap_or_else(platform::home);
    cmd.cwd(cwd);
    let pair = native_pty_system().openpty(PtySize { rows: s.rows.max(5), cols: s.cols.max(20), pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())?;
    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);
    Ok(Pty { reader: pair.master.try_clone_reader().map_err(|e| e.to_string())?, writer: pair.master.take_writer().map_err(|e| e.to_string())?, master: pair.master, child })
}

// ---------------------------------------------------------------- WSL machines
pub fn wsl_probe(distro: &str) -> Result<Probe, String> {
    let script = "for t in tmux claude agy openclaw node git; do if command -v $t >/dev/null 2>&1; then echo \"tool:$t=1\"; else echo \"tool:$t=0\"; fi; done; echo \"host:$(hostname)\"; echo \"home:$HOME\"; echo \"user:$(id -un)\"";
    let (code, stdout, stderr) = wsl_exec(distro, script, Duration::from_secs(20));
    if code != 0 && stdout.is_empty() { return Err(if stderr.trim().is_empty() { format!("WSL distro {distro} did not start") } else { stderr.trim().to_string() }); }
    let mut p = parse_probe(&stdout);
    p.sessions.clear();
    Ok(p)
}
pub fn wsl_dirs(distro: &str) -> Dirs { parse_dirs(&wsl_exec(distro, &crate::remote::DIRS_SCRIPT.replace(" -o -name .nexus", ""), Duration::from_secs(20)).1) }

/// Folders under the user profile that look like projects (contain .git), plus top-level folders.
pub fn local_dirs() -> Dirs {
    const SKIP: &[&str] = &["node_modules", "AppData", ".git", ".cache", "Library", "Applications", "snap"];
    fn walk(dir: &Path, depth: usize, repos: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let entries: Vec<_> = rd.filter_map(Result::ok).collect();
        if entries.iter().any(|e| e.file_name() == ".git") { repos.push(dir.to_path_buf()); }
        if depth >= 3 || repos.len() > 300 { return; }
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIP.contains(&name.as_str()) || !e.file_type().is_ok_and(|t| t.is_dir()) { continue; }
            if depth < 2 { dirs.push(e.path()); }
            walk(&e.path(), depth + 1, repos, dirs);
        }
    }
    let home = platform::home();
    let (mut repos, mut dirs) = (Vec::new(), Vec::new());
    walk(&home, 0, &mut repos, &mut dirs);
    let tilde = |p: &PathBuf| format!("~{}", p.to_string_lossy().strip_prefix(&*home.to_string_lossy()).unwrap_or_default());
    Dirs { repos: repos.iter().filter(|p| **p != home).map(tilde).collect(), dirs: dirs.iter().map(tilde).collect() }
}

// ---------------------------------------------------------------- stats for this computer
fn gpu() -> Option<Gpu> {
    let smi = which("nvidia-smi").or_else(|| [r"C:\Windows\System32\nvidia-smi.exe", r"C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe"].iter().map(PathBuf::from).find(|p| IS_WIN && p.exists()))?;
    let mut c = platform::command(smi);
    c.args(["--query-gpu=name,utilization.gpu,temperature.gpu,memory.used,memory.total,power.draw", "--format=csv,noheader,nounits"]);
    let out = platform::run_text(c, Duration::from_secs(4));
    out.lines().next().filter(|l| !l.trim().is_empty()).map(parse_gpu_csv)
}

/// Collectors that keep state between samples (CPU usage and network throughput are rates).
pub struct LocalStats { sys: Option<sysinfo::System>, net: NetRate, wsl: HashMap<String, NetRate> }
impl Default for LocalStats { fn default() -> Self { Self { sys: sysinfo::System::new().ok(), net: NetRate::default(), wsl: HashMap::new() } } }

impl LocalStats {
    /// Blocking (runs nvidia-smi): call from a worker thread.
    pub fn sample(&mut self) -> Stats {
        let mut out = Stats { gpu: gpu(), now: Some(crate::util::now_ms() as f64), ..Default::default() };
        if let Some(sys) = self.sys.as_mut() {
            sys.refresh_memory();
            sys.refresh_cpu_usage();
            let cpus = sys.cpus().len() as f64;
            out.cpus = Some(cpus);
            out.mem_total = Some(sys.total_memory() as f64);
            out.mem_avail = Some(sys.available_memory() as f64);
            // Present the same way as Linux: a 1-minute load; synthesise one from the busy fraction where loadavg is absent.
            out.load = if IS_WIN { Some((f64::from(sys.global_cpu_usage()) / 100.0 * cpus * 100.0).round() / 100.0) } else { sysinfo::System::load_average().ok().map(|l| l.one) };
        }
        let home = platform::home();
        if let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() {
            // The filesystem the home folder lives on: the mount point that is the longest prefix of it.
            if let Some(d) = disks.iter().filter(|d| home.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len()) {
                let (total, avail) = (d.total_space() as f64, d.available_space() as f64);
                out.disk = Some(Disk { total, used: total - avail, avail, mount: d.mount_point().to_string_lossy().into_owned() });
            }
        }
        if let Ok(nets) = sysinfo::Networks::new_with_refreshed_list() {
            let skip = ["lo", "docker", "veth", "br-", "virbr", "utun", "awdl", "llw", "bridge", "vmnet", "Loopback"];
            let (mut rx, mut tx, mut any) = (0.0, 0.0, false);
            for (name, data) in &nets {
                let name = name.to_string_lossy();
                if skip.iter().any(|s| name.starts_with(s)) { continue; }
                rx += data.total_received() as f64;
                tx += data.total_transmitted() as f64;
                any = true;
            }
            if any { out.net = Some(Net { rx, tx, ..Default::default() }); }
        }
        self.net.finish(out)
    }
    pub fn wsl_sample(&mut self, distro: &str) -> Stats {
        let (_, stdout, _) = wsl_exec(distro, LINUX_SCRIPT, Duration::from_secs(8));
        self.wsl.entry(distro.to_string()).or_default().finish(parse_linux(&stdout))
    }
}
pub static STATS: std::sync::LazyLock<Mutex<LocalStats>> = std::sync::LazyLock::new(Mutex::default);

// ---------------------------------------------------------------- helper scripts
// Shipped inside the executable and written out when first needed, so there is nothing to install beside it.
pub const BOOTSTRAP_SH: &str = include_str!("../scripts/linux-bootstrap.sh");
const SETUP_KEY_SH: &str = include_str!("../scripts/setup-ssh-key.sh");
const SETUP_KEY_PS1: &str = include_str!("../scripts/setup-ssh-key.ps1");

/// The "Set up SSH key" helper for this OS, as a file the local shell can run.
/// `git worktree add` on this computer (see remote::worktree_script for the layout).
pub fn add_worktree(cwd: &str, branch: &str) -> Result<String, String> {
    let git = |dir: &std::path::Path, args: &[&std::ffi::OsStr]| -> Result<String, String> {
        let mut c = crate::platform::command("git");
        c.current_dir(dir).args(args);
        let o = crate::platform::run(c, None, Duration::from_secs(60));
        if o.code == 0 { Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()) } else { Err(crate::util::clip(o.stderr.trim().lines().last().unwrap_or("git failed (is it installed?)"), 300)) }
    };
    let os = std::ffi::OsStr::new;
    let dir = match cwd.strip_prefix('~') { Some(rest) => dirs::home_dir().ok_or("No home folder")?.join(rest.trim_start_matches(['/', '\\'])), None => PathBuf::from(cwd) };
    let top = PathBuf::from(git(&dir, &[os("rev-parse"), os("--show-toplevel")])?);
    let name = top.file_name().ok_or("Not a repository folder")?.to_string_lossy().into_owned();
    let dest = top.parent().ok_or("Not a repository folder")?.join(format!("{name}-worktrees")).join(crate::remote::worktree_slug(branch));
    if let Some(p) = dest.parent() { std::fs::create_dir_all(p).map_err(|e| e.to_string())?; }
    git(&top, &[os("worktree"), os("add"), os("-b"), os(branch), dest.as_os_str()]).or_else(|_| git(&top, &[os("worktree"), os("add"), dest.as_os_str(), os(branch)]))?;
    Ok(dest.to_string_lossy().into_owned())
}

pub fn setup_key_script() -> Result<PathBuf, String> {
    let (name, text) = if IS_WIN { ("setup-ssh-key.ps1", SETUP_KEY_PS1) } else { ("setup-ssh-key.sh", SETUP_KEY_SH) };
    let dir = crate::config::dir().join("scripts");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    let text = if IS_WIN { text.to_string() } else { text.replace("\r\n", "\n") };
    if std::fs::read_to_string(&path).ok().as_deref() != Some(&text) { std::fs::write(&path, text).map_err(|e| e.to_string())?; }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_stats_sample_has_the_basics() {
        let s = LocalStats::default().sample();
        assert!(s.cpus.unwrap_or(0.0) >= 1.0);
        assert!(s.mem_total.unwrap_or(0.0) > 0.0);
        assert!(s.now.is_some());
    }
    #[cfg(unix)]
    #[test]
    fn unix_command_lines() {
        let sh = "/bin/bash";
        let (p, a) = command_line(&SpawnSpec { cmd: "claude".into(), ..Default::default() }, sh).unwrap();
        assert_eq!((p.as_str(), a), ("/bin/bash", vec!["-lic".to_string(), "claude; exec '/bin/bash' -l".to_string()]));
        assert_eq!(command_line(&SpawnSpec::default(), sh).unwrap().1, ["-l"]);
        let (_, a) = command_line(&SpawnSpec { script: Some((PathBuf::from("/x/set up.sh"), vec!["host".into(), String::new(), "22".into()])), ..Default::default() }, sh).unwrap();
        assert_eq!(a[1], "bash '/x/set up.sh' 'host' '' '22'; exec '/bin/bash' -l");
        let (p, a) = command_line(&SpawnSpec { cmd: "agy".into(), wsl: Some("Ubuntu".into()), ..Default::default() }, sh).unwrap();
        assert_eq!(p, "wsl.exe");
        assert_eq!(a, ["-d", "Ubuntu", "--cd", "~", "-e", "bash", "-lic", "agy; exec \"$SHELL\" -l"]);
    }
    #[cfg(unix)]
    #[test]
    fn a_local_pty_runs_a_command_and_reports_its_output() {
        use std::io::Read;
        let mut pty = spawn(&SpawnSpec { cmd: "echo nexus-$((40+2)); exit 0".into(), cols: 80, rows: 24, ..Default::default() }).unwrap();
        let mut out = String::new();
        let mut buf = [0u8; 4096];
        while !out.contains("nexus-42") { match pty.reader.read(&mut buf) { Ok(n) if n > 0 => out.push_str(&String::from_utf8_lossy(&buf[..n])), _ => break } }
        assert!(out.contains("nexus-42"), "{out}");
        let _ = pty.child.kill();
    }
}
