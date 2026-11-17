//! Remote machines over SSH (russh). One connection per machine, one channel per session.
//! Sessions run inside a dedicated tmux server (-L nexus) so they survive app restarts and dropped links.
use crate::config;
use crate::sshconfig::{check_known_hosts, fingerprint, parse_jump, ssh_config_for, ssh_dir, HostConfig, Jump, Verdict};
use crate::stats::{parse_linux, NetRate, Stats, LINUX_SCRIPT};
use crate::tailscale::is_tailnet_address;
use russh::client::{self, AuthResult};
use russh::keys::{load_secret_key, PrivateKey, PrivateKeyWithHashAlg};
use russh::ChannelMsg;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

pub const TMUX: &str = "tmux -L nexus -f ~/.nexus/tmux.conf";

/// Invisible tmux: no status bar, no mouse capture (so the app's own selection/copy works),
/// and no alternate screen (so output lands in the app's scrollback). Bells pass through to Nexus.
pub const TMUX_CONF: &str = r#"# Managed by Nexus - overwritten on connect
set -g status off
set -g mouse off
set -g escape-time 0
set -g history-limit 50000
set -g focus-events on
set -g set-clipboard on
set -g allow-passthrough on
set -g extended-keys on
set -g default-terminal "tmux-256color"
set -ga terminal-overrides ',xterm*:smcup@:rmcup@,xterm-256color:Tc'
# Forward apps' OSC 52 clipboard writes to Nexus (Ms for older tmux, terminal-features for 3.2+).
set -ga terminal-overrides ',xterm*:Ms=\E]52;%p1%s;%p2%s\007'
set -as terminal-features ',xterm*:clipboard'
set -g set-titles on
set -g set-titles-string '#T'
set-environment -g COLORTERM truecolor
set -g remain-on-exit off
set -g monitor-bell on
set -g bell-action any
set -g visual-bell off
"#;

/// Claude Code hooks for "Enable Claude alerts". Each Stop / Notification event rings the terminal bell (which travels
/// through tmux to Nexus: an exact "waiting for you" signal) and writes what happened to ~/.nexus/agent/<tmux session>.json
/// (the event, Claude's notification text or its last reply, the folder), which Nexus reads to show *why* it stopped.
pub const HOOK_SH: &str = r#"#!/usr/bin/env bash
# Managed by Nexus (Enable Claude alerts). Reads the hook event from stdin, rings the bell, records a status file.
printf '\a' > /dev/tty 2>/dev/null || true
d="$HOME/.nexus/agent"; umask 077; mkdir -p "$d"
s=$( [ -n "$TMUX_PANE" ] && tmux -L nexus display-message -p -t "$TMUX_PANE" '#S' 2>/dev/null )
[ -n "$s" ] || s="pid-$PPID"
# The event JSON arrives on stdin; the python below is fed through a heredoc, so hand it over in the environment.
NEXUS_EV="$(cat 2>/dev/null)"; export NEXUS_EV
python3 - "$d/$(printf %s "$s" | tr -c 'A-Za-z0-9._-' '_').json" <<'PY' 2>/dev/null || true
import json, os, sys, time
out = sys.argv[1]
try: ev = json.loads(os.environ.get('NEXUS_EV') or '{}')
except Exception: ev = {}
rec = {'ts': int(time.time() * 1000), 'event': ev.get('hook_event_name') or 'Stop', 'cwd': ev.get('cwd') or os.getcwd(),
       'message': (ev.get('message') or ev.get('title') or '')[:300], 'summary': ''}
tp = ev.get('transcript_path')
if rec['event'] == 'Stop' and tp and os.path.isfile(tp):
    try:
        with open(tp, 'rb') as f:
            f.seek(0, 2); n = f.tell(); f.seek(max(0, n - 200000)); tail = f.read().decode('utf-8', 'replace').splitlines()
        for line in reversed(tail):
            try: m = json.loads(line)
            except Exception: continue
            if m.get('type') != 'assistant': continue
            parts = (m.get('message') or {}).get('content') or []
            text = ' '.join(p.get('text', '') for p in parts if isinstance(p, dict) and p.get('type') == 'text').strip()
            if text: rec['summary'] = text[:400]; break
    except Exception: pass
tmp = out + '.tmp'
with open(tmp, 'w') as f: json.dump(rec, f)
os.replace(tmp, out)
PY
"#;

pub const ALERTS_PY: &str = r#"
import json, os
p = os.path.expanduser('~/.claude/settings.json')
os.makedirs(os.path.dirname(p), exist_ok=True)
try:
    with open(p) as f: s = json.load(f)
except Exception:
    s = {}
old = "printf '\\a' > /dev/tty 2>/dev/null || true"
cmd = 'bash "$HOME/.nexus/claude-hook.sh"'
hooks = s.setdefault('hooks', {})
added = 0
for ev in ('Stop', 'Notification'):
    lst = hooks.setdefault(ev, [])
    # Replace the bell-only hook from older Nexus versions.
    for e in lst:
        for h in (e.get('hooks') or []):
            if h.get('command') == old: h['command'] = cmd; added += 1
    if not any(h.get('command') == cmd for e in lst for h in (e.get('hooks') or [])):
        lst.append({'hooks': [{'type': 'command', 'command': cmd}]})
        added += 1
with open(p, 'w') as f: json.dump(s, f, indent=2)
print('alerts:' + str(added))
"#;

pub fn shq(s: &str) -> String { format!("'{}'", s.replace('\'', "'\\''")) }
/// Quote a remote path but keep a leading ~ expandable.
pub fn sh_path(p: &str) -> String {
    if p == "~" { "\"$HOME\"".into() } else if let Some(rest) = p.strip_prefix("~/") { format!("\"$HOME\"/{}", shq(rest)) } else { shq(p) }
}

/// Remote command for an interactive session. The agent runs in a login+interactive bash (so nvm /
/// ~/.local/bin PATH entries load); when it exits you drop into your normal shell instead of losing the pane.
/// record: also pipe everything the pane prints into ~/.nexus/logs/<tmuxName>-<date>.log on the machine (an audit
/// trail of what each agent did; pipe-pane -o makes it idempotent across reattaches).
pub fn session_command(tmux_name: Option<&str>, cmd: &str, tmux: &str, cwd: &str, read_only: bool, record: bool) -> String {
    let name = tmux_name.filter(|n| !n.is_empty());
    // Watch someone else's session without being able to type into it.
    if let (Some(n), true) = (name, read_only) { return format!("exec {tmux} attach-session -r -t {}", shq(n)); }
    let cd = if cwd.is_empty() { String::new() } else { format!("cd {} 2>/dev/null; ", sh_path(cwd)) };
    let inner = if cmd.is_empty() { format!("{cd}exec \"$SHELL\" -l") } else { format!("{cd}{cmd}; exec \"$SHELL\" -l") };
    let Some(n) = name else { return format!("exec bash -ilc {}", shq(&inner)) };
    let run = if cmd.is_empty() { "\"$SHELL\" -l".to_string() } else { format!("bash -ilc {}", shq(&inner)) };
    let dir = if cwd.is_empty() { String::new() } else { format!("-c {} ", sh_path(cwd)) };
    let fallback = format!("else printf '\\033[33m[nexus] tmux is not installed here - this session will not survive disconnects.\\033[0m\\r\\n'; exec bash -ilc {}; fi", shq(&inner));
    if !record { return format!("if command -v tmux >/dev/null 2>&1; then exec {tmux} new-session -A {dir}-s {} {}; {fallback}", shq(n), shq(&run)); }
    let log = format!("\"$HOME\"/.nexus/logs/{}-$(date +%Y%m%d).log", shq(n));
    format!(
        "if command -v tmux >/dev/null 2>&1; then {tmux} new-session -A -d {dir}-s {n} {run} && (umask 077; mkdir -p \"$HOME\"/.nexus/logs; {tmux} pipe-pane -o -t {n} {pipe}); exec {tmux} attach-session -t {n}; {fallback}",
        n = shq(n), run = shq(&run), pipe = shq(&format!("exec cat >> {log}"))
    )
}

// ---------------------------------------------------------------- host keys
#[derive(Clone, Debug, PartialEq)]
pub struct UnknownHost { pub key: String, pub fingerprint: String, pub host: String, pub port: u16, pub address: String }

/// Nexus's own trust-on-first-use list (config.json "knownHosts"), keyed per address:port.
pub trait TrustStore: Send + Sync {
    fn known(&self, key: &str) -> Option<String>;
    fn remember(&self, key: &str, fp: &str);
}
#[derive(Default, Debug)]
pub struct VerifyState { pub host_key_error: Option<String>, pub unknown_host: Option<UnknownHost> }

pub struct HostCheck { pub host: String, pub port: u16, pub address: String, pub known_files: Vec<PathBuf>, /** Can the app ask the user to confirm a new fingerprint? */ pub can_confirm: bool }

/// Host-key check for one hop. Trust ~/.ssh/known_hosts first (what `ssh` uses); fall back to Nexus's own
/// trust-on-first-use list, keyed per address (Tailscale SSH answers with its own host key, different from sshd's).
pub fn verify_host(c: &HostCheck, blob: &[u8], trust: &dyn TrustStore, st: &mut VerifyState) -> bool {
    let fp = fingerprint(blob);
    match check_known_hosts(&c.known_files, &c.host, c.port, blob) {
        Verdict::Match => return true,
        Verdict::Revoked | Verdict::Mismatch => {
            let entry = if c.port == 22 { c.host.clone() } else { format!("[{}]:{}", c.host, c.port) };
            st.host_key_error = Some(format!("Host key for {}:{} does not match ~/.ssh/known_hosts (got {fp}). If the machine was reinstalled, remove the old entry with: ssh-keygen -R {entry}", c.host, c.port));
            return false;
        }
        Verdict::Unknown => {}
    }
    let k = format!("{}:{}", c.address, c.port);
    match trust.known(&k) {
        None => {
            // Tailnet peers are already authenticated by WireGuard: pin silently. Anywhere else (LAN, internet) a first
            // connection could be a man in the middle, so the user confirms the fingerprint once (if the app can ask).
            if is_tailnet_address(&c.address) || is_tailnet_address(&c.host) || !c.can_confirm { trust.remember(&k, &fp); return true; }
            st.host_key_error = Some(format!("{k} presented a host key Nexus hasn't seen before ({fp}). It was not trusted."));
            st.unknown_host = Some(UnknownHost { key: k, fingerprint: fp, host: c.host.clone(), port: c.port, address: c.address.clone() });
            false
        }
        Some(known) if known != fp => {
            st.host_key_error = Some(format!("Host key for {k} changed (expected {known}, got {fp}). Remove it under \"knownHosts\" in the config if this is expected."));
            false
        }
        Some(_) => true,
    }
}

struct Client { check: HostCheck, trust: Arc<dyn TrustStore>, verify: Arc<Mutex<VerifyState>>, on_drop: Option<Arc<dyn Fn() + Send + Sync>> }
impl client::Handler for Client {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let blob = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.to_bytes().ok(),
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => cert.to_bytes().ok(),
        };
        let Some(blob) = blob else { return Ok(false) };
        Ok(verify_host(&self.check, &blob, self.trust.as_ref(), &mut self.verify.lock().unwrap()))
    }
    async fn disconnected(&mut self, reason: client::DisconnectReason<Self::Error>) -> Result<(), Self::Error> {
        if let Some(f) = &self.on_drop { f(); }
        match reason { client::DisconnectReason::ReceivedDisconnect(_) => Ok(()), client::DisconnectReason::Error(e) => Err(e) }
    }
}
type Handle = client::Handle<Client>;

// ---------------------------------------------------------------- errors
#[derive(Debug, Clone, Default)]
pub struct ConnError { pub msg: String, pub auth_failed: bool, pub unreachable: bool, pub unknown_host: Option<UnknownHost> }
impl ConnError { fn plain(msg: impl Into<String>) -> Self { Self { msg: msg.into(), ..Default::default() } } }
impl std::fmt::Display for ConnError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.msg) } }

enum Fail { Auth, Timeout, NotFound, Io(std::io::Error), Other(String) }

fn friendly(f: &Fail, user: &str, host: &str, port: u16, tried: &str) -> (String, bool) {
    use std::io::ErrorKind as K;
    match f {
        Fail::Auth => (format!("Authentication failed for {}@{host} (tried {tried}). Use \"Set up SSH key\" in Settings.", if user.is_empty() { "?" } else { user }), false),
        Fail::Timeout => (format!("Timed out reaching {host}."), true),
        Fail::NotFound if is_tailnet_address(host) => (format!("Can't find {host} on Tailscale. Is Tailscale running on this PC and signed in to the tailnet this machine is on?"), true),
        Fail::NotFound => (format!("Host \"{host}\" not found."), true),
        Fail::Io(e) => match e.kind() {
            K::ConnectionRefused => (format!("{host}:{port} refused the connection. Is sshd running?"), true),
            K::TimedOut => (format!("Timed out reaching {host}."), true),
            K::HostUnreachable | K::NetworkUnreachable => (format!("{host} is unreachable."), true),
            _ => (e.to_string(), false),
        },
        Fail::Other(m) => (m.clone(), false),
    }
}

// ---------------------------------------------------------------- the app's side
pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status { Offline, Connecting, Online, Error }
impl Status { pub fn as_str(self) -> &'static str { match self { Self::Offline => "offline", Self::Connecting => "connecting", Self::Online => "online", Self::Error => "error" } } }

/// What a Machine needs from the app: its settings, the pinned host keys, and a way to ask the user things.
pub struct Store {
    pub get: Box<dyn Fn() -> Option<config::Machine> + Send + Sync>,
    pub trust: Arc<dyn TrustStore>,
    pub ask_passphrase: Box<dyn Fn(PathBuf, String) -> BoxFut<Option<String>> + Send + Sync>,
    pub confirm_host: Box<dyn Fn(UnknownHost, String) -> BoxFut<bool> + Send + Sync>,
    pub on_status: Box<dyn Fn(Status, Option<String>, Option<&'static str>) + Send + Sync>,
}

#[derive(Default)]
struct State {
    status: Option<Status>,
    route: Option<&'static str>,
    home_dir: Option<String>,
    passphrases: HashMap<PathBuf, String>,
    declined_hosts: HashSet<String>,
    encrypted_keys: Vec<PathBuf>,
    tried_keys: String,
}
struct Conn { handle: Arc<Handle>, jumps: Vec<Arc<Handle>> }

pub struct Machine {
    store: Store,
    state: Mutex<State>,
    conn: Mutex<Option<Arc<Conn>>>,
    connecting: tokio::sync::Mutex<()>,
    net_rate: Mutex<NetRate>,
    /// Bumped per connection, so a late "disconnected" from an old connection can't mark a new one offline.
    epoch: std::sync::atomic::AtomicU64,
}

#[derive(Default, Debug, Clone)]
pub struct TmuxSession { pub name: String, pub attached: u32, pub activity: i64, pub cwd: String }
#[derive(Default, Debug, Clone)]
pub struct Probe { pub tools: HashMap<String, bool>, pub sessions: Vec<TmuxSession>, pub hostname: String, pub home: String, pub user: String }
#[derive(Default, Debug, Clone)]
pub struct Dirs { pub repos: Vec<String>, pub dirs: Vec<String> }
#[derive(Default, Debug, Clone)]
pub struct AgentStatus { pub ts: i64, pub event: String, pub message: String, pub summary: String }
pub struct ExecOut { pub code: Option<u32>, pub stdout: String, pub stderr: String }
pub enum Stdin { None, Bytes(Vec<u8>), File(PathBuf) }

pub fn parse_probe(stdout: &str) -> Probe {
    let mut p = Probe::default();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(t) = line.strip_prefix("tool:") {
            if let Some((name, v)) = t.split_once('=') { p.tools.insert(name.to_string(), v.starts_with('1')); }
        } else if let Some(s) = line.strip_prefix("sess:") {
            let f: Vec<&str> = s.splitn(4, '|').collect();
            if let (4, Ok(att), Ok(act)) = (f.len(), f.get(1).unwrap_or(&"").parse::<u32>(), f.get(2).unwrap_or(&"").parse::<i64>()) {
                p.sessions.push(TmuxSession { name: f[0].into(), attached: att, activity: act * 1000, cwd: f[3].into() });
            }
        } else if let Some(v) = line.strip_prefix("host:") { p.hostname = v.trim().into(); }
        else if let Some(v) = line.strip_prefix("home:") { p.home = v.trim().into(); }
        else if let Some(v) = line.strip_prefix("user:") { p.user = v.trim().into(); }
    }
    // Show paths under $HOME as ~/..., which is also how Nexus stores them.
    if !p.home.is_empty() { for s in &mut p.sessions { if let Some(rest) = s.cwd.strip_prefix(&p.home) { s.cwd = format!("~{rest}"); } } }
    p
}
pub fn parse_dirs(stdout: &str) -> Dirs {
    let mut d = Dirs::default();
    for line in stdout.lines() {
        let line = line.trim_end_matches('\r');
        // tool checkouts like ~/.oh-my-zsh are not projects
        if let Some(r) = line.strip_prefix("repo:") { if !r.contains("/.") { d.repos.push(r.into()); } }
        else if let Some(x) = line.strip_prefix("dir:") { d.dirs.push(x.into()); }
    }
    d
}
/// Project folders for the "launch in folder" picker: git repos (depth 4) and plain folders (depth 2).
pub const DIRS_SCRIPT: &str = concat!(
    "cd ~ || exit 0; ",
    r#"find . -maxdepth 4 \( -name node_modules -o -name .cache -o -name .local -o -name .npm -o -name .nvm -o -name snap -o -name .nexus \) -prune -o -name .git -print 2>/dev/null | head -n 300 | sed "s#/\.git\$##; s#^\.#repo:~#"; "#,
    r#"find . -mindepth 1 -maxdepth 2 -type d -not -path "*/.*" -not -path "*/node_modules*" 2>/dev/null | head -n 400 | sed "s#^\.#dir:~#""#
);
pub const PROBE_TOOLS: &str = "for t in tmux claude agy openclaw node git python3 nvidia-smi; do if command -v $t >/dev/null 2>&1; then echo \"tool:$t=1\"; else echo \"tool:$t=0\"; fi; done; ";

fn os_user() -> String { std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default() }
#[derive(Clone, Debug)]
pub struct LogFile { pub name: String, pub size: u64 }
/// How much of a recording the replay viewer loads (the newest part).
pub const LOG_MAX: usize = 6 * 1024 * 1024;
const AGENTS_CMD: &str = r#"cd "$HOME"/.nexus/agent 2>/dev/null && for f in nx-*.json; do [ -f "$f" ] && { printf '%s\t' "${f%.json}"; tr -d '\n' < "$f"; echo; }; done; true"#;
const LOGS_CMD: &str = r#"cd "$HOME"/.nexus/logs 2>/dev/null || exit 0; ls -t -- *.log 2>/dev/null | head -n 200 | while IFS= read -r f; do printf '%s\t%s\n' "$(wc -c < "$f" | tr -d ' ')" "$f"; done"#;

pub fn parse_agent(json: &str) -> Option<AgentStatus> {
    let r: serde_json::Value = serde_json::from_str(json).ok().filter(serde_json::Value::is_object)?;
    let s = |k: &str, n: usize| crate::util::clip(r[k].as_str().unwrap_or_default(), n);
    Some(AgentStatus { ts: r["ts"].as_f64().unwrap_or(0.0) as i64, event: s("event", 24), message: s("message", 300), summary: s("summary", 400) })
}

/// A branch name git accepts and that is safe to use as a folder name.
pub fn valid_branch(b: &str) -> bool {
    !b.is_empty() && b.len() <= 80 && b.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c)) && !b.starts_with(['-', '/', '.']) && !b.ends_with(['/', '.']) && !b.contains("..") && !b.contains("//") && !b.ends_with(".lock")
}
pub fn worktree_slug(branch: &str) -> String { branch.replace('/', "-") }
/// Shell script (POSIX) that adds a worktree for `branch` (created if new) in <repo>-worktrees/<branch> beside the
/// repository holding `cwd`, and prints the new folder.
pub fn worktree_script(cwd: &str, branch: &str) -> String {
    format!(
        "set -e; cd {}; top=$(git rev-parse --show-toplevel); dest=\"$(dirname \"$top\")/$(basename \"$top\")-worktrees/\"{}; mkdir -p \"$(dirname \"$dest\")\"; \
         git worktree add -b {b} \"$dest\" >/dev/null 2>&1 || git worktree add \"$dest\" {b} >/dev/null; printf '%s' \"$dest\"",
        sh_path(cwd), shq(&worktree_slug(branch)), b = shq(branch))
}

fn safe_name(s: &str, keep: &str) -> String { s.chars().map(|c| if c.is_ascii_alphanumeric() || keep.contains(c) { c } else { '_' }).collect() }

struct Hop { host: String, port: u16, user: String, address: String, sc: HostConfig }

impl Machine {
    pub fn new(store: Store) -> Arc<Self> {
        Arc::new(Self { store, state: Mutex::default(), conn: Mutex::new(None), connecting: tokio::sync::Mutex::new(()), net_rate: Mutex::default(), epoch: 0.into() })
    }
    pub fn status(&self) -> Status { self.state.lock().unwrap().status.unwrap_or(Status::Offline) }
    pub fn is_online(&self) -> bool { self.status() == Status::Online && self.conn.lock().unwrap().is_some() }
    fn set_status(&self, status: Status, error: Option<String>) {
        let route = { let mut st = self.state.lock().unwrap(); st.status = Some(status); st.route };
        (self.store.on_status)(status, error, route);
    }
    fn handle(&self) -> Result<Arc<Handle>, String> {
        self.conn.lock().unwrap().as_ref().map(|c| c.handle.clone()).ok_or_else(|| "Not connected".to_string())
    }

    /// Usable private keys for one hop, in order. Machines added from someone else's invite code use Tailscale SSH
    /// ("none" auth): don't offer them your keys, which would tell that host which public keys you hold (and let it
    /// try them against other servers' prompts).
    fn load_keys(&self, m: &config::Machine, sc: &HostConfig) -> Vec<(String, PrivateKey)> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if !m.key_path.is_empty() { candidates.push(crate::platform::expand_home(&m.key_path)); }
        if !m.invited {
            candidates.extend(sc.identity_files.iter().cloned());
            candidates.extend(["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|f| ssh_dir().join(f)));
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut st = self.state.lock().unwrap();
        for f in candidates.into_iter().filter(|f| seen.insert(f.clone())) {
            if !f.exists() { continue; }
            let pass = st.passphrases.get(&f).cloned().or_else(|| m.passphrase.clone());
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match load_secret_key(&f, None) {
                Ok(k) => out.push((name, k)),
                Err(russh::keys::Error::KeyIsEncrypted) => match pass.as_deref().map(|p| load_secret_key(&f, Some(p))) {
                    Some(Ok(k)) => out.push((name, k)),
                    // wrong passphrase: ask again next time
                    _ => { st.passphrases.remove(&f); if !st.encrypted_keys.contains(&f) { st.encrypted_keys.push(f); } }
                },
                Err(_) => {}
            }
        }
        out
    }

    /// Authenticate one hop, in order: "none" (Tailscale SSH), every usable key, then ssh-agent.
    async fn authenticate(&self, h: &mut Handle, user: &str, m: &config::Machine, sc: &HostConfig, record: bool) -> Result<(), Fail> {
        let other = |e: russh::Error| Fail::Other(e.to_string());
        if matches!(h.authenticate_none(user).await.map_err(other)?, AuthResult::Success) { return Ok(()); }
        let mut tried: Vec<String> = Vec::new();
        for (name, key) in self.load_keys(m, sc) {
            tried.push(name);
            let hash = h.best_supported_rsa_hash().await.ok().flatten().flatten();
            match h.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await {
                Ok(AuthResult::Success) => return Ok(()),
                Ok(_) => {}
                Err(e) => return Err(other(e)),
            }
        }
        if !m.invited && agent_auth(h, user, &mut tried).await { return Ok(()); }
        if record { self.state.lock().unwrap().tried_keys = if tried.is_empty() { "no usable keys".into() } else { tried.join(", ") }; }
        Err(Fail::Auth)
    }

    /// One SSH handshake + authentication over `stream`.
    async fn establish<S>(self: &Arc<Self>, stream: S, hop: &Hop, m: &config::Machine, verify: &Arc<Mutex<VerifyState>>, is_final: bool) -> Result<Handle, Fail>
    where S: AsyncRead + AsyncWrite + Unpin + Send + 'static {
        let cfg = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(15)), keepalive_max: 4, inactivity_timeout: None, ..Default::default() });
        let mut known_files = hop.sc.known_hosts_files.clone();
        known_files.push(ssh_dir().join("known_hosts"));
        let on_drop: Option<Arc<dyn Fn() + Send + Sync>> = if is_final {
            let (me, epoch) = (Arc::downgrade(self), self.epoch.load(std::sync::atomic::Ordering::SeqCst));
            Some(Arc::new(move || { if let Some(me) = me.upgrade() { me.dropped(epoch); } }))
        } else { None };
        let handler = Client { check: HostCheck { host: hop.host.clone(), port: hop.port, address: hop.address.clone(), known_files, can_confirm: true }, trust: self.store.trust.clone(), verify: verify.clone(), on_drop };
        let mut h = client::connect_stream(cfg, stream, handler).await.map_err(|e| match e { russh::Error::IO(io) => Fail::Io(io), e => Fail::Other(e.to_string()) })?;
        // Jump hosts use your own keys even when the machine itself came from an invite.
        let jm;
        let m = if is_final { m } else { jm = config::Machine { key_path: String::new(), invited: false, ..m.clone() }; &jm };
        self.authenticate(&mut h, &hop.user, m, &hop.sc, is_final).await?;
        Ok(h)
    }

    fn dropped(&self, epoch: u64) {
        if self.epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch { return; }
        let had = self.conn.lock().unwrap().take().is_some();
        if had && self.status() == Status::Online { self.set_status(Status::Offline, None); }
    }

    /// One connection attempt to one address (LAN name/IP, or the machine's Tailscale address), through the jump
    /// chain if there is one (ssh -J): each hop is a full SSH connection whose forwarded channel carries the next.
    async fn attempt(self: &Arc<Self>, m: &config::Machine, address: &str, quick: bool) -> Result<(), ConnError> {
        // Blank Nexus fields fall back to ~/.ssh/config, exactly like the ssh command does.
        let sc = ssh_config_for(address);
        let target = Hop {
            host: sc.hostname.clone().unwrap_or_else(|| address.to_string()),
            port: if m.port != 0 { m.port } else { sc.port.unwrap_or(22) },
            user: if m.user.is_empty() { sc.user.clone().unwrap_or_else(os_user) } else { m.user.clone() },
            address: address.to_string(),
            sc,
        };
        // The jump chain: the machine's own "Jump host" field, else ProxyJump from ~/.ssh/config.
        let jumps: Vec<Jump> = if !m.jump.is_empty() { m.jump.split(',').filter_map(parse_jump).collect() } else if m.invited { vec![] } else { target.sc.jumps.clone() };
        let mut hops: Vec<Hop> = jumps.iter().map(|j| {
            let sc = ssh_config_for(&j.host);
            Hop { host: sc.hostname.clone().unwrap_or_else(|| j.host.clone()), port: j.port.or(sc.port).unwrap_or(22), user: j.user.clone().or_else(|| sc.user.clone()).unwrap_or_else(os_user), address: j.host.clone(), sc }
        }).collect();
        let via_jump = !hops.is_empty();
        hops.push(target);
        { let mut st = self.state.lock().unwrap(); st.encrypted_keys.clear(); st.tried_keys = "no usable keys".into(); }
        self.epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let verify = Arc::new(Mutex::new(VerifyState::default()));
        let limit = Duration::from_secs(if quick { 5 } else { 12 });
        let last = hops.len() - 1;
        let run = async {
            let mut chain: Vec<Arc<Handle>> = Vec::new();
            for (i, hop) in hops.iter().enumerate() {
                let label = |f: Fail| if i < last { Fail::Other(format!("Jump host {}@{}:{}: {}", hop.user, hop.host, hop.port, verify.lock().unwrap().host_key_error.clone().unwrap_or_else(|| friendly(&f, &hop.user, &hop.host, hop.port, "your keys").0))) } else { f };
                let h = match chain.last() {
                    None => {
                        let addr = tokio::net::lookup_host((hop.host.as_str(), hop.port)).await.ok().and_then(|mut a| a.next()).ok_or(Fail::NotFound).map_err(label)?;
                        let sock = tokio::net::TcpStream::connect(addr).await.map_err(|e| label(Fail::Io(e)))?;
                        let _ = sock.set_nodelay(true);
                        self.establish(sock, hop, m, &verify, i == last).await.map_err(label)?
                    }
                    Some(prev) => {
                        let ch = prev.channel_open_direct_tcpip(hop.host.clone(), u32::from(hop.port), "127.0.0.1", 0).await
                            .map_err(|e| Fail::Other(format!("Jump host {} could not reach {}:{}: {e}", hops[i - 1].host, hop.host, hop.port)))?;
                        self.establish(ch.into_stream(), hop, m, &verify, i == last).await.map_err(label)?
                    }
                };
                chain.push(Arc::new(h));
            }
            Ok::<_, Fail>(chain)
        };
        let result = match tokio::time::timeout(limit, run).await { Ok(r) => r, Err(_) => Err(Fail::Timeout) };
        let mut chain = match result {
            Ok(c) => c,
            Err(f) => {
                let v = std::mem::take(&mut *verify.lock().unwrap());
                let tried = self.state.lock().unwrap().tried_keys.clone();
                let (msg, unreachable) = friendly(&f, &hops[last].user, address, hops[last].port, &tried);
                return Err(ConnError { msg: v.host_key_error.clone().unwrap_or(msg), auth_failed: matches!(f, Fail::Auth), unreachable: unreachable && v.host_key_error.is_none(), unknown_host: v.unknown_host });
            }
        };
        let handle = chain.pop().expect("at least the target hop");
        let route = if is_tailnet_address(address) { "tailscale" } else if via_jump { "jump" } else { "lan" };
        *self.conn.lock().unwrap() = Some(Arc::new(Conn { handle, jumps: chain }));
        { let mut st = self.state.lock().unwrap(); st.home_dir = None; st.route = Some(route); }
        // ~/.nexus holds pasted screenshots, uploads and team chat: private to this account.
        let setup = format!("umask 077; mkdir -p ~/.nexus/uploads && chmod 700 ~/.nexus ~/.nexus/uploads; cat > ~/.nexus/tmux.conf && ({TMUX} source-file ~/.nexus/tmux.conf >/dev/null 2>&1 || true); find ~/.nexus/uploads -type f -mtime +7 -delete 2>/dev/null; find ~/.nexus/logs -type f -mtime +30 -delete 2>/dev/null; true");
        let _ = self.exec(&setup, Stdin::Bytes(TMUX_CONF.as_bytes().to_vec()), Duration::from_secs(20)).await;
        self.set_status(Status::Online, None);
        Ok(())
    }

    /// Try the LAN address, then the machine's Tailscale address (away from home, or a guest's only route).
    async fn attempt_all(self: &Arc<Self>, m: &config::Machine) -> Result<(), ConnError> {
        let mut addresses: Vec<&str> = Vec::new();
        for a in [m.host.as_str(), m.ts_host.as_str()] { if !a.is_empty() && !addresses.contains(&a) { addresses.push(a); } }
        let mut last = ConnError::plain("No host configured for this machine. Open Settings to add one.");
        for (i, a) in addresses.iter().enumerate() {
            match self.attempt(m, a, i + 1 < addresses.len()).await {
                Ok(()) => return Ok(()),
                Err(e) => { let stop = !e.unreachable; last = e; if stop { break; } } // auth / host-key problems are real answers, not routing ones
            }
        }
        self.set_status(Status::Error, Some(last.msg.clone()));
        Err(last)
    }

    /// One attempt round, plus a single retry after the user trusts a new host key.
    async fn round(self: &Arc<Self>, m: &config::Machine) -> Result<(), ConnError> {
        let err = match self.attempt_all(m).await { Ok(()) => return Ok(()), Err(e) => e };
        let Some(u) = err.unknown_host.clone() else { return Err(err) };
        let seen = format!("{} {}", u.key, u.fingerprint);
        if self.state.lock().unwrap().declined_hosts.contains(&seen) { return Err(err); } // asked once this run already; don't nag on auto-reconnect
        if !(self.store.confirm_host)(u.clone(), m.name.clone()).await { self.state.lock().unwrap().declined_hosts.insert(seen); return Err(err); }
        self.store.trust.remember(&u.key, &u.fingerprint);
        self.set_status(Status::Connecting, None);
        self.attempt_all(m).await
    }

    pub async fn connect(self: &Arc<Self>) -> Result<(), String> {
        if self.is_online() { return Ok(()); }
        let _guard = self.connecting.lock().await;
        if self.is_online() { return Ok(()); }
        let m = (self.store.get)().filter(|m| !m.host.is_empty() || !m.ts_host.is_empty()).ok_or("No host configured for this machine. Open Settings to add one.")?;
        self.set_status(Status::Connecting, None);
        let err = match self.round(&m).await { Ok(()) => return Ok(()), Err(e) => e };
        // Passphrase-protected keys: ask once, then retry with them unlocked.
        let encrypted = self.state.lock().unwrap().encrypted_keys.clone();
        if !err.auth_failed || encrypted.is_empty() { return Err(err.msg); }
        let mut unlocked = false;
        for f in encrypted {
            if let Some(pass) = (self.store.ask_passphrase)(f.clone(), m.name.clone()).await.filter(|p| !p.is_empty()) {
                self.state.lock().unwrap().passphrases.insert(f, pass);
                unlocked = true;
            }
        }
        if !unlocked { return Err(err.msg); }
        self.set_status(Status::Connecting, None);
        self.round(&m).await.map_err(|e| e.msg)
    }

    pub fn disconnect(self: &Arc<Self>) {
        let Some(c) = self.conn.lock().unwrap().take() else { return };
        if self.status() == Status::Online { self.set_status(Status::Offline, None); }
        tokio::spawn(async move {
            let _ = c.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            for j in &c.jumps { let _ = j.disconnect(russh::Disconnect::ByApplication, "", "en").await; }
        });
    }

    /// Run a command with no pty; resolves with stdout/stderr/code.
    pub async fn exec(&self, cmd: &str, stdin: Stdin, timeout: Duration) -> Result<ExecOut, String> {
        let h = self.handle()?;
        let run = async {
            let ch = h.channel_open_session().await.map_err(|e| e.to_string())?;
            ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
            let (mut rd, wr) = ch.split();
            let feed = tokio::spawn(async move {
                match stdin {
                    Stdin::None => {}
                    Stdin::Bytes(b) => { let _ = wr.data(&b[..]).await; }
                    Stdin::File(p) => { if let Ok(f) = tokio::fs::File::open(p).await { let _ = wr.data(f).await; } }
                }
                let _ = wr.eof().await;
            });
            let (mut out, mut err, mut code) = (Vec::new(), Vec::new(), None);
            while let Some(msg) = rd.wait().await {
                match msg {
                    ChannelMsg::Data { data } => out.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => err.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            feed.abort();
            Ok::<_, String>(ExecOut { code, stdout: String::from_utf8_lossy(&out).into_owned(), stderr: String::from_utf8_lossy(&err).into_owned() })
        };
        tokio::time::timeout(timeout, run).await.map_err(|_| "Remote command timed out".to_string())?
    }
    async fn sh(&self, cmd: &str, secs: u64) -> Result<ExecOut, String> { self.exec(cmd, Stdin::None, Duration::from_secs(secs)).await }

    pub async fn home(&self) -> Result<String, String> {
        if let Some(h) = self.state.lock().unwrap().home_dir.clone() { return Ok(h); }
        let h = self.sh("printf %s \"$HOME\"", 20).await?.stdout.trim().to_string();
        self.state.lock().unwrap().home_dir = Some(h.clone());
        Ok(h)
    }

    pub async fn probe(self: &Arc<Self>) -> Result<Probe, String> {
        self.connect().await?;
        let script = format!("{PROBE_TOOLS}{TMUX} ls -F 'sess:#{{session_name}}|#{{session_attached}}|#{{session_activity}}|#{{pane_current_path}}' 2>/dev/null; echo \"host:$(hostname)\"; echo \"home:$HOME\"; echo \"user:$(id -un)\"");
        let p = parse_probe(&self.sh(&format!("bash -ilc {} 2>/dev/null </dev/null", shq(&script)), 20).await?.stdout);
        if !p.home.is_empty() { self.state.lock().unwrap().home_dir = Some(p.home.clone()); }
        Ok(p)
    }

    /// GPU / CPU / memory / disk / network snapshot for the machine card.
    pub async fn stats(&self) -> Result<Option<Stats>, String> {
        if self.conn.lock().unwrap().is_none() { return Ok(None); }
        let out = self.sh(LINUX_SCRIPT, 8).await?;
        Ok(Some(self.net_rate.lock().unwrap().finish(parse_linux(&out.stdout))))
    }

    pub async fn list_dirs(self: &Arc<Self>) -> Result<Dirs, String> {
        self.connect().await?;
        Ok(parse_dirs(&self.sh(DIRS_SCRIPT, 15).await?.stdout))
    }

    pub async fn pane_cwd(&self, tmux_name: &str) -> Option<String> {
        let out = self.sh(&format!("{TMUX} display-message -p -t {} '#{{pane_current_path}}' 2>/dev/null", shq(tmux_name)), 5).await.ok()?;
        Some(out.stdout.trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Upload bytes or a local file into ~/.nexus/uploads; returns the absolute remote path.
    pub async fn upload_to_inbox(self: &Arc<Self>, name: &str, content: Stdin) -> Result<String, String> {
        self.connect().await?;
        let home = self.home().await?;
        let safe: Vec<char> = safe_name(name, "_.-").chars().collect();
        let safe: String = safe[safe.len().saturating_sub(80)..].iter().collect();
        let abs = format!("{home}/.nexus/uploads/{}-{}", radix36(crate::util::now_ms() as u64), if safe.is_empty() { "file" } else { &safe });
        let r = self.exec(&format!("umask 077; mkdir -p ~/.nexus/uploads && chmod 700 ~/.nexus/uploads && cat > {}", shq(&abs)), content, Duration::from_secs(1800)).await?;
        if r.code != Some(0) { return Err(if r.stderr.trim().is_empty() { "upload failed".into() } else { r.stderr.trim().to_string() }); }
        Ok(abs)
    }

    pub async fn setup_alerts(self: &Arc<Self>) -> Result<u32, String> {
        self.connect().await?;
        self.exec("umask 077; mkdir -p ~/.nexus && cat > ~/.nexus/claude-hook.sh && chmod 700 ~/.nexus/claude-hook.sh", Stdin::Bytes(HOOK_SH.as_bytes().to_vec()), Duration::from_secs(20)).await?;
        let r = self.exec("python3 - 2>&1", Stdin::Bytes(ALERTS_PY.as_bytes().to_vec()), Duration::from_secs(20)).await?;
        r.stdout.split("alerts:").nth(1).and_then(|s| s.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
            .ok_or_else(|| [r.stdout.trim(), r.stderr.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or("python3 is required on the remote").to_string())
    }

    /// Interactive channel with a pty. tmux_name -> persistent session; otherwise a plain one-shot login shell.
    #[allow(clippy::too_many_arguments)]
    pub async fn open(self: &Arc<Self>, tmux_name: Option<&str>, cmd: &str, cols: u16, rows: u16, cwd: &str, read_only: bool, record: bool) -> Result<russh::Channel<client::Msg>, String> {
        self.connect().await?;
        let full = session_command(tmux_name, cmd, TMUX, cwd, read_only, record);
        let ch = self.handle()?.channel_open_session().await.map_err(|e| e.to_string())?;
        ch.request_pty(false, "xterm-256color", u32::from(cols), u32::from(rows), 0, 0, &[]).await.map_err(|e| e.to_string())?;
        ch.exec(true, full).await.map_err(|e| e.to_string())?;
        Ok(ch)
    }

    /// A long-running command with no pty whose output is read as it arrives (the team chat's `tail -F`).
    pub async fn exec_stream(&self, cmd: &str) -> Result<russh::Channel<client::Msg>, String> {
        let ch = self.handle()?.channel_open_session().await.map_err(|e| e.to_string())?;
        ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
        Ok(ch)
    }

    /// What the agent in this tmux session last reported through its hooks (see HOOK_SH), or None.
    pub async fn agent_status(&self, tmux_name: &str) -> Option<AgentStatus> {
        let out = self.sh(&format!("cat \"$HOME\"/.nexus/agent/{}.json 2>/dev/null", shq(&safe_name(tmux_name, "._-"))), 5).await.ok()?;
        parse_agent(&out.stdout)
    }
    /// The last hook report of every Nexus tmux session on this machine (open here or not), by session name.
    pub async fn agent_statuses(&self) -> HashMap<String, AgentStatus> {
        let Ok(out) = self.sh(AGENTS_CMD, 8).await else { return HashMap::new() };
        out.stdout.lines().filter_map(|l| { let (name, json) = l.split_once('\t')?; Some((name.to_string(), parse_agent(json)?)) }).collect()
    }

    /// Recorded sessions (~/.nexus/logs), newest first.
    pub async fn list_logs(&self) -> Result<Vec<LogFile>, String> {
        let out = self.sh(LOGS_CMD, 20).await?;
        Ok(out.stdout.lines().filter_map(|l| { let (size, name) = l.split_once('\t')?; Some(LogFile { name: name.to_string(), size: size.trim().parse().ok()? }) }).collect())
    }
    /// The (end of a) recording: what the pane printed, escape codes included.
    pub async fn read_log(&self, name: &str) -> Result<Vec<u8>, String> {
        if name.is_empty() || name.starts_with('.') || name != safe_name(name, "._-") { return Err("Not a recording".into()); }
        let out = self.exec(&format!("tail -c {LOG_MAX} \"$HOME\"/.nexus/logs/{}", shq(name)), Stdin::None, Duration::from_secs(90)).await?;
        if out.code.is_some_and(|c| c != 0) { return Err(if out.stderr.is_empty() { "Couldn't read the recording".into() } else { out.stderr }); }
        Ok(out.stdout.into_bytes())
    }
    /// `git worktree add` beside the repository that holds `cwd`; returns the new folder.
    pub async fn add_worktree(&self, cwd: &str, branch: &str) -> Result<String, String> {
        let out = self.sh(&worktree_script(cwd, branch), 60).await?;
        let path = out.stdout.trim().to_string();
        if out.code == Some(0) && !path.is_empty() { Ok(path) } else { Err(crate::util::clip(out.stderr.trim().lines().last().unwrap_or("git worktree add failed"), 300)) }
    }

    pub async fn kill_tmux(&self, name: &str) { let _ = self.sh(&format!("{TMUX} kill-session -t {} 2>/dev/null", shq(name)), 20).await; }

    pub async fn upload(self: &Arc<Self>, remote_path: &str, content: &str) -> Result<(), String> {
        self.connect().await?;
        let dir = remote_path.rsplit_once('/').map_or("", |x| x.0);
        let r = self.exec(&format!("mkdir -p {} && cat > {p} && chmod +x {p}", sh_path(dir), p = sh_path(remote_path)), Stdin::Bytes(content.as_bytes().to_vec()), Duration::from_secs(20)).await?;
        if r.code.is_some_and(|c| c != 0) { return Err(if r.stderr.is_empty() { "upload failed".into() } else { r.stderr }); }
        Ok(())
    }

    /// A byte stream to host:port as seen from the machine (ssh -L), over the existing connection.
    pub async fn forward(&self, host: &str, port: u16, origin_port: u16) -> Result<russh::ChannelStream<client::Msg>, String> {
        let ch = self.handle()?.channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", u32::from(origin_port)).await.map_err(|e| e.to_string())?;
        Ok(ch.into_stream())
    }
}

pub fn radix36(mut n: u64) -> String {
    let d = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut v = Vec::new();
    loop { v.push(d[(n % 36) as usize]); n /= 36; if n == 0 { break; } }
    v.reverse();
    String::from_utf8(v).unwrap_or_default()
}

/// Offer every identity the running ssh-agent holds (the Windows OpenSSH agent pipe, or $SSH_AUTH_SOCK).
async fn agent_auth(h: &mut Handle, user: &str, tried: &mut Vec<String>) -> bool {
    use russh::keys::agent::{client::AgentClient, AgentIdentity};
    #[cfg(windows)]
    let agent = { let pipe = r"\\.\pipe\openssh-ssh-agent"; if std::path::Path::new(pipe).exists() { AgentClient::connect_named_pipe(pipe).await.ok().map(AgentClient::dynamic) } else { None } };
    #[cfg(unix)]
    let agent = if std::env::var_os("SSH_AUTH_SOCK").is_some() { AgentClient::connect_env().await.ok().map(AgentClient::dynamic) } else { None };
    let Some(mut agent) = agent else { return false };
    tried.push("ssh-agent".into());
    let Ok(ids) = agent.request_identities().await else { return false };
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else { continue };
        let hash = h.best_supported_rsa_hash().await.ok().flatten().flatten();
        if h.authenticate_publickey_with(user, key, hash, &mut agent).await.is_ok_and(|r| r.success()) { return true; }
    }
    false
}

pub fn key_file_name(p: &Path) -> String { p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{command, run};

    #[test]
    fn branch_names_and_worktree_script() {
        for ok in ["main", "feature/login", "nexus/claude-ab12", "v1.2_x"] { assert!(valid_branch(ok), "{ok}"); }
        for bad in ["", "-x", "/x", "x/", "a..b", "a b", "a;b", "$(id)", "x.lock", ".hidden", "a//b", "a'b"] { assert!(!valid_branch(bad), "{bad}"); }
        let s = worktree_script("~/code/my app", "feature/login");
        assert!(s.contains("cd \"$HOME\"/'code/my app'") && s.contains("-worktrees/\"'feature-login'") && s.contains("git worktree add -b 'feature/login' \"$dest\""), "{s}");
        let a = parse_agent(r#"{"ts": 12.0, "event": "Stop", "summary": "ok"}"#).unwrap();
        assert_eq!((a.ts, a.event.as_str(), a.summary.as_str(), a.message.as_str()), (12, "Stop", "ok", ""));
        assert!(parse_agent("[]").is_none() && parse_agent("").is_none());
    }

    #[test]
    fn quoting() {
        assert_eq!(shq("it's"), r"'it'\''s'");
        assert_eq!(sh_path("~"), "\"$HOME\"");
        assert_eq!(sh_path("~/code/my app"), "\"$HOME\"/'code/my app'");
        assert_eq!(sh_path("/srv/x"), "'/srv/x'");
    }
    #[test]
    fn session_command_plain_login_shell_and_read_only() {
        assert_eq!(session_command(None, "claude", TMUX, "", false, false), r#"exec bash -ilc 'claude; exec "$SHELL" -l'"#);
        assert_eq!(session_command(Some("nx-a"), "claude", "tmux", "", true, false), "exec tmux attach-session -r -t 'nx-a'");
        let rec = session_command(Some("nx-a"), "", "tmux", "~/p", false, true);
        assert!(rec.contains("pipe-pane -o -t 'nx-a'") && rec.contains("attach-session -t 'nx-a'") && rec.contains(".nexus/logs/") && rec.contains("-$(date +%Y%m%d).log"), "{rec}");
    }
    #[cfg(unix)]
    #[test]
    fn session_command_runs_the_agent_in_the_folder_with_nested_quotes_intact() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nexus-cmd-{}", std::process::id()));
        let work = dir.join("my 'proj'");
        std::fs::create_dir_all(&work).unwrap();
        // Fake tmux: print -c, then run the session command the way tmux would (sh -c).
        std::fs::write(dir.join("tmux"), "#!/bin/bash\nwhile [ $# -gt 1 ]; do [ \"$1\" = -c ] && echo \"start-dir=$2\"; shift; done\nexec sh -c \"$1\"\n").unwrap();
        std::fs::set_permissions(dir.join("tmux"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let cmd = session_command(Some("nx-claude-test"), "pwd; echo \"it's quoted\"", "tmux", &work.to_string_lossy(), false, false);
        let mut c = command("/bin/bash");
        c.args(["-c", &cmd]).env("PATH", format!("{}:{}", dir.display(), std::env::var("PATH").unwrap())).env("SHELL", "/usr/bin/true").env("HOME", &dir);
        let out = String::from_utf8(run(c, Some(b""), Duration::from_secs(20)).stdout).unwrap();
        assert!(out.contains("start-dir=") && out.lines().any(|l| l.starts_with("start-dir=") && l.ends_with("my 'proj'")), "{out}");
        assert!(out.contains("my 'proj'\n") && out.contains("it's quoted"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn python() -> Option<&'static str> { ["python3", "python"].into_iter().find(|p| { let mut c = command(p); c.arg("--version"); run(c, None, Duration::from_secs(5)).code == 0 }) }
    fn home_dir(tag: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("nexus-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(h.join(".claude")).unwrap();
        h
    }
    #[test]
    fn claude_alert_hooks_added_once_existing_settings_preserved_old_hook_upgraded() {
        let Some(py) = python() else { return };
        let home = home_dir("alerts");
        let file = home.join(".claude/settings.json");
        std::fs::write(&file, r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]}]}}"#).unwrap();
        let go = || { let mut c = command(py); c.arg("-").env("HOME", &home).env("USERPROFILE", &home); String::from_utf8(run(c, Some(ALERTS_PY.as_bytes()), Duration::from_secs(20)).stdout).unwrap() };
        assert!(go().contains("alerts:2"));
        assert!(go().contains("alerts:0"));
        let s: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(s["model"], "opus");
        assert_eq!((s["hooks"]["Stop"].as_array().unwrap().len(), s["hooks"]["Notification"].as_array().unwrap().len()), (2, 1));
        assert_eq!(s["hooks"]["Stop"][1]["hooks"][0]["command"], "bash \"$HOME/.nexus/claude-hook.sh\"");
        // The bell-only hook from older Nexus versions is upgraded in place.
        let old = r"printf '\a' > /dev/tty 2>/dev/null || true";
        std::fs::write(&file, serde_json::json!({ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": old }] }], "Notification": [{ "hooks": [{ "type": "command", "command": old }] }] } }).to_string()).unwrap();
        assert!(go().contains("alerts:2"));
        let s: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "bash \"$HOME/.nexus/claude-hook.sh\"");
        assert!(HOOK_SH.starts_with("#!/usr/bin/env bash"));
        let _ = std::fs::remove_dir_all(&home);
    }
    #[cfg(unix)]
    #[test]
    fn claude_hook_script_records_the_event_and_the_last_reply() {
        if python().is_none() { return; }
        let home = home_dir("hook");
        let sh = home.join("hook.sh");
        std::fs::write(&sh, HOOK_SH).unwrap();
        let transcript = home.join("t.jsonl");
        std::fs::write(&transcript, [
            r#"{"type":"user","message":{"content":"hi"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done: 3 files changed."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
        ].join("\n")).unwrap();
        let fire = |ev: serde_json::Value| { let mut c = command("/bin/bash"); c.arg(&sh).env("HOME", &home).env("TMUX_PANE", ""); run(c, Some(ev.to_string().as_bytes()), Duration::from_secs(20)); };
        fire(serde_json::json!({ "hook_event_name": "Stop", "cwd": "/srv/proj", "transcript_path": transcript }));
        let files: Vec<_> = std::fs::read_dir(home.join(".nexus/agent")).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "json")).collect();
        assert_eq!(files.len(), 1);
        let rec = |p: &Path| -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() };
        let r = rec(&files[0].path());
        assert_eq!((r["event"].as_str(), r["cwd"].as_str(), r["summary"].as_str()), (Some("Stop"), Some("/srv/proj"), Some("Done: 3 files changed.")));
        fire(serde_json::json!({ "hook_event_name": "Notification", "message": "Claude needs your permission to use Bash" }));
        let r = rec(&files[0].path());
        assert_eq!(r["event"], "Notification");
        assert!(r["message"].as_str().unwrap().contains("permission"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[derive(Default)]
    struct Pins(Mutex<HashMap<String, String>>);
    impl TrustStore for Pins {
        fn known(&self, k: &str) -> Option<String> { self.0.lock().unwrap().get(k).cloned() }
        fn remember(&self, k: &str, fp: &str) { self.0.lock().unwrap().insert(k.into(), fp.into()); }
    }
    #[test]
    fn first_seen_host_keys_confirmed_off_the_tailnet_pinned_silently_on_it() {
        let blob = crate::sshconfig::test_blob("ssh-ed25519", 9);
        let pins = Pins::default();
        let check = |host: &str, can_confirm| HostCheck { host: host.into(), port: 22, address: host.into(), known_files: vec![PathBuf::from("/nonexistent/known_hosts")], can_confirm };
        let mut st = VerifyState::default();
        assert!(!verify_host(&check("nexus-test-host.invalid", true), &blob, &pins, &mut st), "LAN host not trusted without asking");
        let u = st.unknown_host.clone().unwrap();
        assert_eq!(u.key, "nexus-test-host.invalid:22");
        assert!(u.fingerprint.starts_with("SHA256:"));
        assert!(pins.0.lock().unwrap().is_empty());

        assert!(verify_host(&check("box.tail1.ts.net", true), &blob, &pins, &mut VerifyState::default()), "tailnet host pinned");
        assert!(pins.known("box.tail1.ts.net:22").is_some());

        pins.remember(&u.key, &u.fingerprint); // the user trusted it
        assert!(verify_host(&check("nexus-test-host.invalid", true), &blob, &pins, &mut VerifyState::default()));
        // …and a different key from the same address is refused from then on.
        let mut st = VerifyState::default();
        assert!(!verify_host(&check("nexus-test-host.invalid", true), &crate::sshconfig::test_blob("ssh-ed25519", 10), &pins, &mut st));
        assert!(st.host_key_error.unwrap().contains("changed") && st.unknown_host.is_none());

        assert!(verify_host(&check("other-host.invalid", false), &blob, &pins, &mut VerifyState::default()), "no prompt available: old trust-on-first-use");
    }
    #[test]
    fn probe_and_dirs_output() {
        let p = parse_probe("tool:tmux=1\ntool:nvidia-smi=0\nsess:nx-claude-ab12|0|1700000000|/home/me/proj\nsess:bad\nhost:spark1\nhome:/home/me\nuser:me\n");
        assert_eq!((p.tools["tmux"], p.tools["nvidia-smi"]), (true, false));
        assert_eq!(p.sessions.len(), 1);
        assert_eq!((p.sessions[0].cwd.as_str(), p.sessions[0].activity), ("~/proj", 1_700_000_000_000));
        assert_eq!((p.hostname.as_str(), p.user.as_str()), ("spark1", "me"));
        let d = parse_dirs("repo:~/code/api\nrepo:~/.oh-my-zsh\ndir:~/code\n");
        assert_eq!((d.repos, d.dirs), (vec!["~/code/api".to_string()], vec!["~/code".to_string()]));
        assert_eq!(radix36(35), "z");
        assert_eq!(radix36(36), "10");
    }
}
