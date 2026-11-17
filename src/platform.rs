//! OS differences in one place: which OS we're on, finding programs, the login-shell PATH on macOS/Linux, and WSL.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const IS_WIN: bool = cfg!(windows);
pub const IS_MAC: bool = cfg!(target_os = "macos");

/// Nexus's machine "os" values: windows | mac | linux.
pub fn platform_os() -> &'static str { if IS_WIN { "windows" } else if IS_MAC { "mac" } else { "linux" } }

pub fn home() -> PathBuf { dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")) }

/// `~` and `~/x` -> the home folder.
pub fn expand_home(p: &str) -> PathBuf {
    let p = p.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(p);
    if p == "~" { return home(); }
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) { Some(rest) => home().join(rest), None => PathBuf::from(p) }
}

/// A Command that never flashes a console window on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub struct Output { pub code: i32, pub stdout: Vec<u8>, pub stderr: String, pub missing: bool }

/// Run a program to completion with a timeout (killed when it runs over). Blocking: call from a worker thread.
pub fn run(mut cmd: Command, stdin: Option<&[u8]>, timeout: Duration) -> Output {
    use std::io::{Read, Write};
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Output { code: 1, stdout: vec![], stderr: e.to_string(), missing: e.kind() == std::io::ErrorKind::NotFound },
    };
    if let (Some(data), Some(mut si)) = (stdin, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || { let _ = si.write_all(&data); });
    }
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = so.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let t_err = std::thread::spawn(move || { let mut b = Vec::new(); if let Some(s) = se.as_mut() { let _ = s.read_to_end(&mut b); } b });
    let start = std::time::Instant::now();
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(1),
            Ok(None) if start.elapsed() > timeout => { let _ = child.kill(); let _ = child.wait(); break 124; }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => break 1,
        }
    };
    Output { code, stdout: t_out.join().unwrap_or_default(), stderr: String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned(), missing: false }
}
pub fn run_text(cmd: Command, timeout: Duration) -> String {
    let o = run(cmd, None, timeout);
    if o.code == 0 { String::from_utf8_lossy(&o.stdout).into_owned() } else { String::new() }
}

/// User-level install dirs (Claude/Antigravity installers, npm globals, Homebrew) that GUI-launched apps often lack on PATH.
pub fn extra_paths() -> Vec<PathBuf> {
    let h = home();
    if IS_WIN {
        vec![
            h.join(".local").join("bin"),
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("agy").join("bin"),
            PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("npm"),
        ]
    } else {
        vec![h.join(".local/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")]
    }
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else { return false };
    if !md.is_file() { return false; }
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; md.permissions().mode() & 0o111 != 0 }
    #[cfg(not(unix))]
    { true }
}

/// Locate an executable on PATH (+ extra dirs) without spawning anything.
pub fn which(cmd: &str) -> Option<PathBuf> { which_in(cmd, &std::env::var_os("PATH").unwrap_or_default()) }
pub fn which_in(cmd: &str, env_path: &std::ffi::OsStr) -> Option<PathBuf> {
    let exts: Vec<String> = if IS_WIN {
        let mut v: Vec<String> = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into()).split(';').map(str::to_lowercase).collect();
        v.push(String::new());
        v
    } else { vec![String::new()] };
    for d in std::env::split_paths(env_path).chain(extra_paths()) {
        if d.as_os_str().is_empty() { continue; }
        for e in &exts {
            let p = d.join(format!("{cmd}{e}"));
            if is_executable(&p) { return Some(p); }
        }
    }
    None
}

/// macOS/Linux apps started from Finder or a desktop launcher get a minimal PATH (/usr/bin:/bin…), so `claude`
/// installed in ~/.local/bin or Homebrew isn't found. Ask the user's login shell for its PATH once at startup
/// (before any other thread exists: it changes this process's environment).
pub fn load_login_shell_path() -> bool {
    if IS_WIN { return false; }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| if IS_MAC { "/bin/zsh" } else { "/bin/bash" }.into());
    let mut c = command(&shell);
    c.args(["-ilc", r#"printf "__NEXUS_PATH__%s__NEXUS_PATH__" "$PATH""#]).env("DISABLE_AUTO_UPDATE", "true");
    let out = run(c, None, Duration::from_secs(6));
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(found) = text.split("__NEXUS_PATH__").nth(1).filter(|s| !s.is_empty()) else { return false };
    let mut seen = std::collections::HashSet::new();
    let cur = std::env::var("PATH").unwrap_or_default();
    let merged: Vec<&str> = found.split(':').chain(cur.split(':')).filter(|p| !p.is_empty() && seen.insert(*p)).collect();
    std::env::set_var("PATH", merged.join(":"));
    true
}

// ---------------------------------------------------------------- WSL (Windows only)
/// C:\Users\me\x -> /mnt/c/Users/me/x (WSL's default automount).
pub fn to_wsl_path(p: &str) -> String {
    let b = p.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') {
        format!("/mnt/{}/{}", (b[0] as char).to_ascii_lowercase(), p[3..].replace('\\', "/"))
    } else { p.replace('\\', "/") }
}

#[derive(Debug, PartialEq)]
pub struct WslDistro { pub name: String, pub is_default: bool }

/// `wsl.exe -l` prints UTF-16LE; the default distro is marked "(Default)". Docker's internal distros are skipped.
pub fn parse_wsl_list(buf: &[u8]) -> Vec<WslDistro> {
    let units: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = String::from_utf16_lossy(&units).replace(['\u{0}', '\u{feff}'], "");
    let mut out = Vec::new();
    for raw in text.lines().skip(1) {
        let line = raw.trim();
        if line.is_empty() { continue; }
        let low = line.to_lowercase();
        let is_default = ["(default)", "(par défaut)", "(standard)"].iter().any(|m| low.contains(m));
        let name = if line.ends_with(')') { line.rfind('(').map_or(line, |i| line[..i].trim_end()) } else { line };
        if name.is_empty() || name.to_lowercase().starts_with("docker-desktop") { continue; }
        out.push(WslDistro { name: name.to_string(), is_default });
    }
    out
}

pub fn list_wsl() -> Vec<WslDistro> {
    if !IS_WIN { return vec![]; }
    let mut c = command("wsl.exe");
    c.arg("-l");
    let o = run(c, None, Duration::from_secs(8));
    if o.code == 0 { parse_wsl_list(&o.stdout) } else { vec![] }
}

/// Run a bash script inside a distro (login shell so ~/.local/bin, nvm etc. are on PATH).
pub fn wsl_exec(distro: &str, script: &str, timeout: Duration) -> (i32, String, String) {
    let mut c = command("wsl.exe");
    c.args(["-d", distro, "-e", "bash", "-lc", script]);
    let o = run(c, None, timeout);
    (o.code, String::from_utf8_lossy(&o.stdout).into_owned(), o.stderr)
}

/// Only web links leave the app: terminal output and chat come from other machines and people, and anything else
/// (file:, smb:, custom protocol handlers) is a path to running something the user didn't choose.
pub fn open_external(url: &str) -> bool {
    let ok = (url.starts_with("https://") || url.starts_with("http://")) && !url.chars().any(|c| c.is_control() || c.is_whitespace());
    if ok { let _ = open::that_detached(url); }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_paths() {
        assert_eq!(to_wsl_path(r"C:\Users\me\shot 1.png"), "/mnt/c/Users/me/shot 1.png");
        assert_eq!(to_wsl_path("D:/x/y"), "/mnt/d/x/y");
        assert_eq!(to_wsl_path("/home/me/x"), "/home/me/x");
    }
    #[test]
    fn wsl_list_utf16_default_and_docker() {
        let text = "Windows Subsystem for Linux Distributions:\r\nUbuntu (Default)\r\nDebian\r\ndocker-desktop\r\ndocker-desktop-data\r\n";
        let buf: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse_wsl_list(&buf), vec![WslDistro { name: "Ubuntu".into(), is_default: true }, WslDistro { name: "Debian".into(), is_default: false }]);
    }
    #[cfg(unix)]
    #[test]
    fn which_finds_executables_on_a_given_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nexus-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nexus-tool"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.join("nexus-data"), "x").unwrap();
        std::fs::set_permissions(dir.join("nexus-tool"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which_in("nexus-tool", dir.as_os_str()), Some(dir.join("nexus-tool")));
        assert_eq!(which_in("nexus-data", dir.as_os_str()), None, "not executable");
        assert_eq!(which_in("nexus-missing", dir.as_os_str()), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn only_web_links_open() {
        assert!(!open_external("file:///etc/passwd"));
        assert!(!open_external("ms-settings:foo"));
        assert!(!open_external("https://a.b/ c"));
    }
    #[test]
    fn run_times_out_and_reports_missing_programs() {
        let o = run(command("nexus-definitely-not-a-program"), None, Duration::from_secs(1));
        assert!(o.missing);
        #[cfg(unix)]
        {
            let mut c = command("sh"); c.args(["-c", "cat; exec sleep 5"]);
            let o = run(c, Some(b"hi"), Duration::from_millis(300));
            assert_eq!(o.code, 124);
            assert_eq!(o.stdout, b"hi");
        }
    }
}
