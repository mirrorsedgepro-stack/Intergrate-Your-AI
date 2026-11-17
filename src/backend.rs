//! The app's engine: sessions (local PTYs and SSH channels), machines, port forwards, team chat, the MCP server,
//! Tailscale sharing and update checks. It runs on a tokio runtime and talks to the UI through one event channel;
//! the UI calls it directly (every method is cheap or async).
use crate::collab::{self, Message, Presence};
use crate::config::{self, Config, Preset};
use crate::local::{self, SpawnSpec};
use crate::mcp::{self, McpServer};
use crate::platform::{self, IS_WIN};
use crate::remote::{self, Machine, Status, Stdin, Store, TrustStore, UnknownHost};
use crate::stats::Stats;
use crate::tailscale::{self, Credential, LocalStatus, PlanOpts, SparkArg, SparkMatch, TailscaleApi};
use crate::util::now_ms;
use russh::ChannelMsg;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub const REPO: &str = "mirrorsedgepro-stack/Intergrate-Your-AI";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Recent output kept per session, for the MCP read_output tool.
const OUT_KEEP: usize = 64 * 1024;

pub type AppFn = Box<dyn FnOnce(&mut crate::ui::App) + Send>;

pub enum Event {
    Data(u64, Vec<u8>),
    Exit { pid: u64, code: Option<i32>, disconnected: bool },
    MachineStatus { id: String, status: Status, error: Option<String>, route: Option<&'static str> },
    AskPassphrase { ask: u64, key_file: String, key_name: String, machine: String },
    AskHost { ask: u64, info: UnknownHost, machine: String },
    CollabMessage(Message),
    CollabState { machine_id: String, live: bool },
    McpLaunch { req: u64, machine_id: String, preset_id: String, cwd: Option<String> },
    McpClose { pid: u64, kill: bool },
    UpdateAvailable { version: String, url: String },
    /// Run something on the UI thread with the app state (how async results come home).
    Call(AppFn),
}

#[derive(Default, Clone, Debug)]
pub struct SessionSpec {
    pub machine_id: String,
    pub preset_id: String,
    pub cols: u16,
    pub rows: u16,
    /// Reattach to this tmux session instead of starting a new one.
    pub attach: Option<String>,
    pub cwd: Option<String>,
    /// A one-off command outside tmux (installers, account setup).
    pub raw: bool,
    pub cmd: Option<String>,
    pub script: Option<String>,
    pub script_machine: Option<String>,
    pub read_only: bool,
}

enum RCmd { Write(Vec<u8>), Resize(u16, u16), Close }
enum Io {
    /// `input` feeds a writer thread, so a program that has stopped reading can never stall the UI.
    Local { input: Mutex<std::sync::mpsc::Sender<Vec<u8>>>, master: Mutex<Box<dyn portable_pty::MasterPty + Send>>, killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>> },
    Remote(mpsc::UnboundedSender<RCmd>),
}
type Recent = Arc<Mutex<Vec<u8>>>;
struct Sess { machine_id: String, preset_id: String, cwd: Option<String>, tmux_name: Option<String>, io: Io, recent: Recent }
fn keep(recent: &Recent, data: &[u8]) {
    let mut r = recent.lock().unwrap();
    r.extend_from_slice(data);
    if r.len() > OUT_KEEP * 2 { let cut = r.len() - OUT_KEEP; r.drain(..cut); }
}
impl Sess {
    fn recent(&self) -> String { let r = self.recent.lock().unwrap(); String::from_utf8_lossy(&r[r.len().saturating_sub(OUT_KEEP)..]).into_owned() }
}

struct Tunnel { machine_id: String, local_port: u16, remote_host: String, remote_port: u16, task: tokio::task::JoinHandle<()>, conns: Arc<AtomicUsize> }
#[derive(Clone, Debug)]
pub struct TunnelInfo { pub id: String, pub machine_id: String, pub local_port: u16, pub remote_host: String, pub remote_port: u16 }

struct Hub { machine_id: String, task: Option<tokio::task::JoinHandle<()>>, live: bool }
struct Plan { policy: Value, etag: Option<String>, tag_changes: Vec<(String, Vec<String>)> }

#[derive(Clone, Debug, Default)]
pub struct TsStatus { pub local: LocalStatus, pub has_token: bool, pub token_kind: Option<&'static str>, pub can_encrypt: bool }
#[derive(Clone, Debug, Default)]
pub struct Guest { pub email: String, pub user_id: Option<String>, pub status: String, pub invite_id: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct Overview { pub sparks: Vec<SparkMatch>, pub guests: Vec<Guest> }
#[derive(Clone, Debug, Default)]
pub struct PlanView { pub etag: Option<String>, pub changes: Vec<String>, pub missing: Vec<String> }
#[derive(Clone, Debug, Default)]
pub struct PlanArgs { pub machines: Vec<SparkArg>, pub ssh_users: Vec<String>, pub add_guest: Option<String>, pub remove_guest: Option<String>, pub per_guest_users: bool }

pub struct Backend {
    pub rt: tokio::runtime::Handle,
    tx: mpsc::Sender<Event>,
    wake: Box<dyn Fn() + Send + Sync>,
    me: Weak<Backend>,
    cfg: Mutex<Config>,
    sessions: Mutex<HashMap<u64, Arc<Sess>>>,
    machines: Mutex<HashMap<String, Arc<Machine>>>,
    asks: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_id: AtomicU64,
    tunnels: Mutex<HashMap<String, Tunnel>>,
    hub: Mutex<Option<Hub>>,
    mcp: Mutex<Option<McpServer>>,
    last_stats: Mutex<HashMap<String, Stats>>,
    launch_waits: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    ts_client: Mutex<Option<(String, Arc<TailscaleApi>)>>,
    last_plan: Mutex<Option<Plan>>,
}

struct Trust(Weak<Backend>);
impl TrustStore for Trust {
    fn known(&self, key: &str) -> Option<String> { self.0.upgrade()?.cfg.lock().unwrap().known_hosts.get(key).cloned() }
    fn remember(&self, key: &str, fp: &str) {
        if let Some(b) = self.0.upgrade() { let mut c = b.cfg.lock().unwrap(); c.known_hosts.insert(key.into(), fp.into()); config::save(&c); }
    }
}

fn keyring_entry() -> Result<keyring::Entry, String> { keyring::Entry::new("Nexus", "tailscale-credential").map_err(|e| e.to_string()) }

impl Backend {
    pub fn new(rt: tokio::runtime::Handle, cfg: Config, tx: mpsc::Sender<Event>, wake: Box<dyn Fn() + Send + Sync>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            rt, tx, wake, me: me.clone(), cfg: Mutex::new(cfg), sessions: Mutex::default(), machines: Mutex::default(), asks: Mutex::default(), next_id: AtomicU64::new(1),
            tunnels: Mutex::default(), hub: Mutex::default(), mcp: Mutex::new(None), last_stats: Mutex::default(), launch_waits: Mutex::default(), ts_client: Mutex::new(None), last_plan: Mutex::new(None),
        })
    }
    fn arc(&self) -> Arc<Self> { self.me.upgrade().expect("backend alive") }
    fn id(&self) -> u64 { self.next_id.fetch_add(1, Ordering::SeqCst) }

    /// Send an event to the UI from anywhere (never blocks; a full queue is handed to a task).
    pub fn emit(&self, ev: Event) {
        if let Err(mpsc::error::TrySendError::Full(ev)) = self.tx.try_send(ev) {
            let tx = self.tx.clone();
            self.rt.spawn(async move { let _ = tx.send(ev).await; });
        }
        (self.wake)();
    }
    /// Run `f` on the backend, then `done` on the UI thread with its result.
    pub fn run<T, F, D>(&self, f: F, done: D)
    where T: Send + 'static, F: std::future::Future<Output = T> + Send + 'static, D: FnOnce(&mut crate::ui::App, T) + Send + 'static {
        let me = self.arc();
        self.rt.spawn(async move { let v = f.await; me.emit(Event::Call(Box::new(move |app| done(app, v)))); });
    }
    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
        self.rt.spawn_blocking(f).await.map_err(|e| e.to_string())
    }

    // ---------------------------------------------------------------- config
    pub fn config(&self) -> Config { self.cfg.lock().unwrap().clone() }
    fn machine_cfg(&self, id: &str) -> Option<config::Machine> { self.cfg.lock().unwrap().machine(id).cloned() }

    /// The UI's copy becomes the config, except for what only the backend owns: pinned host keys, the window
    /// geometry and hand-written key passphrases. Connections whose address changed are dropped so the next use
    /// reconnects with the new settings.
    pub fn save_config(&self, mut new: Config) {
        let prev = {
            let mut cur = self.cfg.lock().unwrap();
            new.known_hosts = cur.known_hosts.clone();
            new.window = cur.window.clone();
            for m in &mut new.machines { m.passphrase = cur.machine(&m.id).and_then(|p| p.passphrase.clone()); }
            config::save(&new);
            std::mem::replace(&mut *cur, new.clone())
        };
        let stale: Vec<String> = self.machines.lock().unwrap().keys().filter(|id| {
            match (prev.machine(id), new.machine(id)) {
                (Some(a), Some(b)) => a.host != b.host || a.user != b.user || a.port_or_default() != b.port_or_default() || a.key_path != b.key_path || a.jump != b.jump,
                _ => true,
            }
        }).cloned().collect();
        for id in stale {
            if let Some(m) = self.machines.lock().unwrap().remove(&id) { m.disconnect(); }
            self.emit(Event::MachineStatus { id, status: Status::Offline, error: None, route: None });
        }
        self.sync_mcp();
    }
    pub fn save_window(&self, w: config::WindowCfg) { let mut c = self.cfg.lock().unwrap(); c.window = Some(w); config::save(&c); }

    /// On Windows, offer each WSL distro as its own machine (once; deleting it in Settings sticks).
    pub fn add_wsl_machines(cfg: &mut Config) -> bool {
        let mut changed = false;
        for d in platform::list_wsl() {
            if cfg.wsl_seen.insert(d.name.clone(), true).is_some() { continue; }
            changed = true;
            if cfg.machines.iter().any(|m| m.is_wsl() && m.distro == d.name) { continue; }
            let slug: String = d.name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            cfg.machines.push(config::Machine { id: format!("wsl-{slug}"), name: format!("WSL · {}", d.name), kind: "wsl".into(), os: "linux".into(), distro: d.name, color: "#E95420".into(), ..Default::default() });
        }
        changed
    }

    // ---------------------------------------------------------------- prompts (the UI shows a modal, answers by id)
    async fn ask(&self, make: impl FnOnce(u64) -> Event) -> Option<String> {
        let id = self.id();
        let (tx, rx) = oneshot::channel();
        self.asks.lock().unwrap().insert(id, tx);
        self.emit(make(id));
        let r = tokio::time::timeout(Duration::from_secs(120), rx).await.ok().and_then(Result::ok).flatten();
        self.asks.lock().unwrap().remove(&id);
        r
    }
    pub fn answer(&self, ask: u64, value: Option<String>) { if let Some(tx) = self.asks.lock().unwrap().remove(&ask) { let _ = tx.send(value); } }

    // ---------------------------------------------------------------- machines
    pub fn machine(&self, id: &str) -> Arc<Machine> {
        if let Some(m) = self.machines.lock().unwrap().get(id) { return m.clone(); }
        let (w1, w2, w3, w4) = (self.me.clone(), self.me.clone(), self.me.clone(), self.me.clone());
        let (id1, id2) = (id.to_string(), id.to_string());
        let store = Store {
            get: Box::new(move || w1.upgrade()?.machine_cfg(&id1)),
            trust: Arc::new(Trust(self.me.clone())),
            ask_passphrase: Box::new(move |key_file: PathBuf, machine: String| {
                let w = w2.clone();
                Box::pin(async move {
                    let b = w.upgrade()?;
                    b.ask(|ask| Event::AskPassphrase { ask, key_name: remote::key_file_name(&key_file), key_file: key_file.to_string_lossy().into_owned(), machine }).await
                })
            }),
            // First connection to a host key not in ~/.ssh/known_hosts (outside the tailnet): the user checks the fingerprint.
            confirm_host: Box::new(move |info: UnknownHost, machine: String| {
                let w = w3.clone();
                Box::pin(async move { match w.upgrade() { Some(b) => b.ask(|ask| Event::AskHost { ask, info, machine }).await.is_some(), None => false } })
            }),
            on_status: Box::new(move |status, error, route| {
                let Some(b) = w4.upgrade() else { return };
                b.emit(Event::MachineStatus { id: id2.clone(), status, error, route });
                let restart = status == Status::Online && b.hub.lock().unwrap().as_ref().is_some_and(|h| h.machine_id == id2 && !h.live);
                if restart { let (b2, id) = (b.clone(), id2.clone()); b.rt.spawn(async move { let _ = b2.start_hub(&id).await; }); }
            }),
        };
        let m = Machine::new(store);
        self.machines.lock().unwrap().insert(id.to_string(), m.clone());
        m
    }
    pub fn machine_status(&self, id: &str) -> Status { self.machines.lock().unwrap().get(id).map_or(Status::Offline, |m| m.status()) }
    pub fn disconnect(&self, id: &str) { if let Some(m) = self.machines.lock().unwrap().get(id) { m.disconnect(); } }

    pub async fn probe(&self, id: &str) -> Result<remote::Probe, String> {
        match self.machine_cfg(id) {
            Some(m) if m.is_wsl() => self.blocking(move || local::wsl_probe(&m.distro)).await?,
            _ => self.machine(id).probe().await,
        }
    }
    pub async fn stats(&self, id: &str) -> Result<Option<Stats>, String> {
        let m = self.machine_cfg(id).ok_or("Unknown machine")?;
        let stats = if m.is_local() { Some(self.blocking(|| local::STATS.lock().unwrap().sample()).await?) }
            else if m.is_wsl() { Some(self.blocking(move || local::STATS.lock().unwrap().wsl_sample(&m.distro)).await?) }
            else { self.machine(id).stats().await? };
        if let Some(s) = &stats { self.last_stats.lock().unwrap().insert(id.to_string(), s.clone()); }
        Ok(stats)
    }
    pub async fn dirs(&self, id: &str) -> Result<remote::Dirs, String> {
        match self.machine_cfg(id) {
            Some(m) if m.is_local() => self.blocking(local::local_dirs).await,
            Some(m) if m.is_wsl() => self.blocking(move || local::wsl_dirs(&m.distro)).await,
            _ => self.machine(id).list_dirs().await,
        }
    }
    pub async fn upload_bootstrap(&self, id: &str) -> Result<(), String> {
        self.machine(id).upload("~/.nexus/bootstrap.sh", &local::BOOTSTRAP_SH.replace("\r\n", "\n")).await
    }

    // ---------------------------------------------------------------- sessions
    pub async fn create_session(&self, spec: SessionSpec) -> Result<(u64, Option<String>), String> {
        let cfg = self.config();
        let m = cfg.machine(&spec.machine_id).cloned().ok_or("Unknown machine")?;
        let shell = Preset { id: "shell".into(), ..Default::default() };
        let preset = cfg.preset(&spec.preset_id).unwrap_or(&shell);
        let cmd = spec.cmd.clone().unwrap_or_else(|| preset.cmd_for(&m.os));
        let (cols, rows) = (spec.cols.max(20), spec.rows.max(5));
        let id = self.id();

        if !m.is_ssh() {
            let mut script = None;
            if spec.script.as_deref() == Some("setup-ssh-key") {
                let t = spec.script_machine.as_deref().and_then(|x| cfg.machine(x)).ok_or("Unknown machine")?;
                let file = local::setup_key_script()?;
                let port = t.port_or_default().to_string();
                script = Some((file, if IS_WIN { vec!["-HostName".into(), t.host.clone(), "-User".into(), t.user.clone(), "-Port".into(), port] } else { vec![t.host.clone(), t.user.clone(), port] }));
            }
            // WSL paths are resolved inside the distro.
            let sp = SpawnSpec { cmd, cols, rows, cwd: spec.cwd.clone(), script, wsl: m.is_wsl().then(|| m.distro.clone()) };
            let pty = self.blocking(move || local::spawn(&sp)).await??;
            let local::Pty { master, mut reader, mut writer, mut child } = pty;
            let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
            std::thread::spawn(move || { for data in input_rx { if std::io::Write::write_all(&mut writer, &data).and_then(|()| std::io::Write::flush(&mut writer)).is_err() { break; } } });
            let recent = Recent::default();
            let sess = Arc::new(Sess {
                machine_id: m.id.clone(), preset_id: preset.id.clone(), cwd: spec.cwd.clone(), tmux_name: None, recent: recent.clone(),
                io: Io::Local { input: Mutex::new(input), master: Mutex::new(master), killer: Mutex::new(child.clone_killer()) },
            });
            self.sessions.lock().unwrap().insert(id, sess);
            // The reader holds no reference to the session: on Windows the ConPTY only reports end-of-file once the
            // session (and with it the pty) has been dropped.
            let (me, tx) = (self.arc(), self.tx.clone());
            let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 32 * 1024];
                loop {
                    match std::io::Read::read(&mut reader, &mut buf) {
                        Ok(n) if n > 0 => {
                            keep(&recent, &buf[..n]);
                            // Blocks when the UI is behind: the PTY (and the program writing to it) then waits too.
                            if tx.blocking_send(Event::Data(id, buf[..n].to_vec())).is_err() { break; }
                            (me.wake)();
                        }
                        _ => break,
                    }
                }
                let _ = done_tx.send(());
            });
            let me = self.arc();
            std::thread::spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code() as i32);
                // Let the reader drain what the program printed last (on Windows it can't finish until the session is dropped).
                let _ = done_rx.recv_timeout(Duration::from_millis(if IS_WIN { 80 } else { 400 }));
                if me.sessions.lock().unwrap().remove(&id).is_some() { me.emit(Event::Exit { pid: id, code, disconnected: false }); }
            });
            return Ok((id, None));
        }

        let mach = self.machine(&m.id);
        let tmux_name = if spec.raw { None } else {
            Some(spec.attach.clone().unwrap_or_else(|| {
                let t = remote::radix36(now_ms() as u64);
                format!("nx-{}-{}{}", preset.id, &t[t.len().saturating_sub(4)..], remote::radix36(u64::from(rand::random::<u16>()) % 1296 + 36).chars().take(2).collect::<String>())
            }))
        };
        let ch = mach.open(tmux_name.as_deref(), &cmd, cols, rows, spec.cwd.as_deref().unwrap_or(""), spec.read_only, cfg.appearance.record_sessions).await?;
        let (ctl, mut rx) = mpsc::unbounded_channel();
        let recent = Recent::default();
        let sess = Arc::new(Sess { machine_id: m.id.clone(), preset_id: preset.id.clone(), cwd: spec.cwd.clone(), tmux_name: tmux_name.clone(), io: Io::Remote(ctl), recent: recent.clone() });
        self.sessions.lock().unwrap().insert(id, sess);
        let me = self.arc();
        self.rt.spawn(async move {
            let (mut rd, wr) = ch.split();
            let mut code = None;
            loop {
                tokio::select! {
                    msg = rd.wait() => match msg {
                        Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                            keep(&recent, &data);
                            if me.tx.send(Event::Data(id, data.to_vec())).await.is_err() { break; }
                            (me.wake)();
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status as i32),
                        Some(ChannelMsg::Close) | None => break,
                        Some(_) => {}
                    },
                    cmd = rx.recv() => match cmd {
                        Some(RCmd::Write(b)) => { let _ = wr.data(&b[..]).await; }
                        Some(RCmd::Resize(c, r)) => { let _ = wr.window_change(u32::from(c), u32::from(r), 0, 0).await; }
                        Some(RCmd::Close) | None => { let _ = wr.close().await; break; }
                    },
                }
            }
            if me.sessions.lock().unwrap().remove(&id).is_some() { me.emit(Event::Exit { pid: id, code, disconnected: code.is_none() }); }
        });
        Ok((id, tmux_name))
    }

    fn sess(&self, id: u64) -> Option<Arc<Sess>> { self.sessions.lock().unwrap().get(&id).cloned() }
    pub fn write(&self, id: u64, data: &[u8]) {
        let Some(s) = self.sess(id) else { return };
        match &s.io {
            Io::Local { input, .. } => { let _ = input.lock().unwrap().send(data.to_vec()); }
            Io::Remote(tx) => { let _ = tx.send(RCmd::Write(data.to_vec())); }
        }
    }
    pub fn resize(&self, id: u64, cols: u16, rows: u16) {
        let Some(s) = self.sess(id) else { return };
        match &s.io {
            Io::Local { master, .. } => { let _ = master.lock().unwrap().resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }); }
            Io::Remote(tx) => { let _ = tx.send(RCmd::Resize(cols, rows)); }
        }
    }
    fn end(s: Arc<Sess>) {
        let killer = match &s.io { Io::Local { killer, .. } => Some(killer.lock().unwrap().clone_killer()), Io::Remote(tx) => { let _ = tx.send(RCmd::Close); None } };
        // Killing waits for the program to go; do that off the UI thread, and only let go of the pty afterwards
        // (dropping its writer types newline + end-of-file, which must not reach a program that is still alive).
        if let Some(mut k) = killer { std::thread::spawn(move || { let _ = k.kill(); drop(s); }); }
    }
    pub fn close(&self, id: u64, kill: bool) {
        let Some(s) = self.sessions.lock().unwrap().remove(&id) else { return };
        if let (true, Some(name)) = (kill, s.tmux_name.clone()) { self.kill_tmux(&s.machine_id, &name); }
        Self::end(s);
    }
    pub fn kill_tmux(&self, machine_id: &str, name: &str) {
        let (m, name) = (self.machine(machine_id), name.to_string());
        self.rt.spawn(async move { m.kill_tmux(&name).await; });
    }
    pub async fn agent_status(&self, id: u64) -> Option<remote::AgentStatus> {
        let s = self.sess(id)?;
        self.machine(&s.machine_id).agent_status(s.tmux_name.as_deref()?).await
    }
    pub async fn session_cwd(&self, id: u64) -> Option<String> {
        let s = self.sess(id)?;
        self.machine(&s.machine_id).pane_cwd(s.tmux_name.as_deref()?).await
    }

    /// Clipboard image (already PNG-encoded by the UI thread) -> a file the session's machine can read.
    pub async fn paste_image(&self, id: u64, png: Vec<u8>) -> Result<String, String> {
        let s = self.sess(id).ok_or("Session is not running")?;
        let name = format!("paste-{}.png", chrono::Local::now().format("%Y-%m-%dT%H-%M-%S-%3f"));
        let m = self.machine_cfg(&s.machine_id).ok_or("Unknown machine")?;
        if m.is_ssh() { return self.machine(&m.id).upload_to_inbox(&name, Stdin::Bytes(png)).await; }
        let p = paste_dir().join(name);
        write_private(&p, &png)?;
        let p = p.to_string_lossy().into_owned();
        Ok(if m.is_wsl() { platform::to_wsl_path(&p) } else { p })
    }

    /// Dropped files -> paths on the session's machine (uploaded to ~/.nexus/uploads for remotes).
    pub async fn upload_files(&self, id: u64, files: Vec<PathBuf>) -> Result<Vec<String>, String> {
        let s = self.sess(id).ok_or("Session is not running")?;
        let m = self.machine_cfg(&s.machine_id).ok_or("Unknown machine")?;
        if files.is_empty() || files.len() > 50 { return Err("Drop between 1 and 50 files".into()); }
        let name = |f: &PathBuf| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // Check every file before uploading any, so a folder in the drop doesn't leave half the files uploaded.
        for f in &files {
            let md = std::fs::metadata(f).map_err(|e| format!("{}: {e}", name(f)))?;
            if md.is_dir() { return Err(format!("{} is a folder; drop files instead", name(f))); }
            if m.is_ssh() && md.len() > 512 * 1024 * 1024 { return Err(format!("{} is larger than 512 MB", name(f))); }
        }
        let mut out = Vec::new();
        for f in files {
            let p = f.to_string_lossy().into_owned();
            out.push(if m.is_local() { p } else if m.is_wsl() { platform::to_wsl_path(&p) } // WSL sees Windows drives under /mnt
                else { self.machine(&m.id).upload_to_inbox(&name(&f), Stdin::File(f)).await? });
        }
        Ok(out)
    }

    /// Stop everything (window closing).
    pub fn shutdown(&self) {
        let all: Vec<Arc<Sess>> = self.sessions.lock().unwrap().drain().map(|x| x.1).collect();
        for s in all { Self::end(s); }
        for (_, t) in self.tunnels.lock().unwrap().drain() { t.task.abort(); }
        self.mcp.lock().unwrap().take();
        self.stop_hub();
        for m in self.machines.lock().unwrap().values() { m.disconnect(); }
    }

    // ---------------------------------------------------------------- port forwarding (ssh -L over the machine's connection)
    // A remote web UI (OpenClaw gateway, Jupyter, TensorBoard, a dev server) becomes http://localhost:<port> on this
    // computer. One listener per tunnel, bound to loopback only, so nothing else on the LAN can ride it.
    pub async fn tunnel_open(&self, machine_id: &str, remote_host: &str, remote_port: u16, local_port: u16) -> Result<TunnelInfo, String> {
        if !self.machine_cfg(machine_id).is_some_and(|m| m.is_ssh()) { return Err("Port forwarding needs an SSH machine".into()); }
        let host = remote_host.trim();
        if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) { return Err("Remote host must be a hostname or IP".into()); }
        if remote_port == 0 { return Err("Remote port must be 1-65535".into()); }
        let mach = self.machine(machine_id);
        mach.connect().await?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", local_port)).await
            .map_err(|e| if e.kind() == std::io::ErrorKind::AddrInUse { format!("Port {local_port} is already in use on this computer") } else { e.to_string() })?;
        let local_port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let conns = Arc::new(AtomicUsize::new(0));
        let (host2, conns2) = (host.to_string(), conns.clone());
        // Kept across reconnects: new connections use whatever connection the machine has.
        let task = self.rt.spawn(async move {
            while let Ok((mut sock, peer)) = listener.accept().await {
                let (mach, host, conns) = (mach.clone(), host2.clone(), conns2.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = mach.forward(&host, remote_port, peer.port()).await else { return };
                    conns.fetch_add(1, Ordering::SeqCst);
                    let _ = tokio::io::copy_bidirectional(&mut sock, &mut stream).await;
                    conns.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        let id = format!("t{}", self.id());
        self.tunnels.lock().unwrap().insert(id.clone(), Tunnel { machine_id: machine_id.into(), local_port, remote_host: host.into(), remote_port, task, conns });
        Ok(TunnelInfo { id, machine_id: machine_id.into(), local_port, remote_host: host.into(), remote_port })
    }
    pub fn tunnel_close(&self, id: &str) { if let Some(t) = self.tunnels.lock().unwrap().remove(id) { t.task.abort(); } }
    pub fn tunnels(&self) -> Vec<TunnelInfo> {
        let mut v: Vec<TunnelInfo> = self.tunnels.lock().unwrap().iter().map(|(id, t)| { let _ = t.conns.load(Ordering::SeqCst); TunnelInfo { id: id.clone(), machine_id: t.machine_id.clone(), local_port: t.local_port, remote_host: t.remote_host.clone(), remote_port: t.remote_port } }).collect();
        v.sort_by_key(|t| t.local_port);
        v
    }

    // ---------------------------------------------------------------- team chat + presence (on the hub machine, over SSH)
    pub fn stop_hub(&self) { if let Some(h) = self.hub.lock().unwrap().take() { if let Some(t) = h.task { t.abort(); } } }
    pub async fn start_hub(&self, machine_id: &str) -> Result<(), String> {
        self.stop_hub();
        *self.hub.lock().unwrap() = Some(Hub { machine_id: machine_id.into(), task: None, live: false });
        let mach = self.machine(machine_id);
        mach.connect().await?;
        mach.exec(&collab::prepare_cmd(), Stdin::None, Duration::from_secs(20)).await?;
        let ch = mach.exec_stream(&collab::tail_cmd()).await?;
        let (me, mid) = (self.arc(), machine_id.to_string());
        let task = self.rt.spawn(async move {
            let (mut rd, _wr) = ch.split();
            let mut buf = collab::LineBuf::default();
            while let Some(msg) = rd.wait().await {
                match msg {
                    ChannelMsg::Data { data } => for m in buf.push(&data) { me.emit(Event::CollabMessage(m)); },
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            if let Some(h) = me.hub.lock().unwrap().as_mut().filter(|h| h.machine_id == mid) { h.live = false; }
            me.emit(Event::CollabState { machine_id: mid, live: false });
        });
        if let Some(h) = self.hub.lock().unwrap().as_mut().filter(|h| h.machine_id == machine_id) { h.task = Some(task); h.live = true; }
        self.emit(Event::CollabState { machine_id: machine_id.into(), live: true });
        Ok(())
    }
    fn hub_machine(&self) -> Result<Arc<Machine>, String> {
        let id = self.hub.lock().unwrap().as_ref().map(|h| h.machine_id.clone()).ok_or("Team chat is not connected")?;
        Ok(self.machine(&id))
    }
    pub async fn collab_post(&self, from: collab::From, text: String, kind: &str) -> Result<(), String> {
        let m = collab::make_message(&from, &text, kind);
        if m.text.is_empty() { return Err("Empty message".into()); }
        let line = format!("{}\n", serde_json::to_string(&m).map_err(|e| e.to_string())?);
        let r = self.hub_machine()?.exec(&collab::post_cmd(), Stdin::Bytes(line.into_bytes()), Duration::from_secs(20)).await?;
        if r.code.is_some_and(|c| c != 0) { return Err(if r.stderr.is_empty() { "could not post".into() } else { r.stderr }); }
        Ok(())
    }
    /// Write our heartbeat and return everyone's fresh presence (including ours).
    pub async fn collab_heartbeat(&self, presence: Value) -> Result<Vec<Presence>, String> {
        let Ok(m) = self.hub_machine() else { return Ok(vec![]) };
        let p = collab::make_presence(&presence);
        let body = serde_json::to_vec(&p).map_err(|e| e.to_string())?;
        Ok(collab::parse_presence_list(&m.exec(&collab::heartbeat_cmd(&p.client), Stdin::Bytes(body), Duration::from_secs(10)).await?.stdout))
    }
    /// Team presets: one JSON file on the hub so everyone launches the same commands. Untrusted content: presets are
    /// commands, so the importer reviews and confirms them in the UI before they become theirs.
    pub async fn collab_presets_get(&self) -> Result<Vec<Value>, String> {
        let r = self.hub_machine()?.exec(collab::PRESETS_GET_CMD, Stdin::None, Duration::from_secs(20)).await?;
        Ok(serde_json::from_str::<Value>(&r.stdout).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().into_iter().take(50).collect())
    }
    pub async fn collab_presets_put(&self, presets: Vec<Preset>) -> Result<usize, String> {
        let clean: Vec<Preset> = presets.into_iter().take(50).collect();
        let body = serde_json::to_vec_pretty(&clean).map_err(|e| e.to_string())?;
        let r = self.hub_machine()?.exec(collab::PRESETS_PUT_CMD, Stdin::Bytes(body), Duration::from_secs(20)).await?;
        if r.code.is_some_and(|c| c != 0) { return Err(if r.stderr.is_empty() { "could not publish".into() } else { r.stderr }); }
        Ok(clean.len())
    }

    // ---------------------------------------------------------------- MCP server (opt-in): local agents orchestrate Nexus
    pub fn mcp_running(&self) -> bool { self.mcp.lock().unwrap().is_some() }
    pub fn sync_mcp(&self) {
        let on = self.cfg.lock().unwrap().mcp.enabled;
        if !on { self.mcp.lock().unwrap().take(); return; }
        if self.mcp_running() { return; }
        let weak = self.me.clone();
        let handler: mcp::Handler = Arc::new(move |call| {
            let weak = weak.clone();
            Box::pin(async move { match weak.upgrade() { Some(b) => b.mcp_call(call).await, None => Err("Nexus is closing".into()) } })
        });
        let me = self.arc();
        self.rt.spawn(async move { if let Ok(srv) = McpServer::start(VERSION.into(), handler).await { *me.mcp.lock().unwrap() = Some(srv); } });
    }
    pub(crate) async fn mcp_call(&self, call: mcp::Call) -> Result<Value, String> {
        let no = |id: &str| format!("No session {id}");
        let find = |id: &str| -> Result<(u64, Arc<Sess>), String> { let n: u64 = id.trim_start_matches('s').parse().map_err(|_| no(id))?; Ok((n, self.sess(n).ok_or_else(|| no(id))?)) };
        match call {
            mcp::Call::ListMachines => {
                let (cfg, stats) = (self.config(), self.last_stats.lock().unwrap().clone());
                Ok(Value::Array(cfg.machines.iter().map(|m| json!({
                    "id": m.id, "name": m.name, "type": m.kind, "os": m.os, "host": if m.host.is_empty() { Value::Null } else { json!(m.host) },
                    "status": if m.is_local() { "online" } else { self.machine_status(&m.id).as_str() }, "stats": stats.get(&m.id),
                })).collect()))
            }
            mcp::Call::ListSessions => Ok(Value::Array(self.sessions.lock().unwrap().iter().map(|(id, s)| json!({
                "id": format!("s{id}"), "machineId": s.machine_id, "preset": s.preset_id, "cwd": s.cwd, "tmux": s.tmux_name,
                "lastLine": mcp::plain(&s.recent()).trim().lines().last().unwrap_or_default(),
            })).collect())),
            mcp::Call::Launch { machine, preset, cwd } => {
                let cfg = self.config();
                let m = cfg.machines.iter().find(|x| x.id == machine || x.name.eq_ignore_ascii_case(&machine)).ok_or(format!("No machine \"{machine}\""))?;
                if cfg.preset(&preset).is_none() { return Err(format!("No preset \"{preset}\"")); }
                let req = self.id();
                let (tx, rx) = oneshot::channel();
                self.launch_waits.lock().unwrap().insert(req, tx);
                self.emit(Event::McpLaunch { req, machine_id: m.id.clone(), preset_id: preset, cwd });
                let r = tokio::time::timeout(Duration::from_secs(30), rx).await;
                self.launch_waits.lock().unwrap().remove(&req);
                r.map_err(|_| "Nexus did not start the session (is a dialog open?)".to_string())?.map_err(|_| "Launch was cancelled in Nexus".to_string())?
            }
            mcp::Call::Send { session, text } => { let (id, _) = find(&session)?; self.write(id, text.as_bytes()); Ok(Value::Null) }
            mcp::Call::ReadOutput { session } => Ok(json!(find(&session)?.1.recent())),
            mcp::Call::Close { session, kill } => { let (pid, _) = find(&session)?; self.emit(Event::McpClose { pid, kill }); Ok(Value::Null) }
        }
    }
    pub fn mcp_launched(&self, req: u64, result: Result<Value, String>) { if let Some(tx) = self.launch_waits.lock().unwrap().remove(&req) { let _ = tx.send(result); } }

    // ---------------------------------------------------------------- Tailscale (sharing SSH machines with guests)
    // The credential lives in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service) and never
    // reaches config.json or the UI.
    fn ts_token() -> Option<String> { keyring_entry().ok()?.get_password().ok().filter(|t| !t.is_empty()) }
    fn ts_api(&self) -> Result<Arc<TailscaleApi>, String> {
        let t = Self::ts_token().ok_or("Add a Tailscale API access token or OAuth client first")?;
        let mut c = self.ts_client.lock().unwrap();
        // One client per saved credential, so an OAuth client's 1-hour access token is reused across calls.
        if let Some((raw, api)) = c.as_ref() { if *raw == t { return Ok(api.clone()); } }
        let api = Arc::new(TailscaleApi::new(tailscale::parse_credential(&t).unwrap_or(Credential::Api { token: t.clone() }), tailscale::ureq_http()));
        *c = Some((t, api.clone()));
        Ok(api)
    }
    pub async fn ts_status(&self) -> TsStatus {
        self.blocking(|| {
            let t = Self::ts_token();
            TsStatus { local: tailscale::local_status(), token_kind: t.as_deref().and_then(tailscale::parse_credential).map(|c| c.kind()), has_token: t.is_some(), can_encrypt: keyring::Entry::store_status().is_ok() }
        }).await.unwrap_or_default()
    }
    pub async fn ts_set_token(&self, token: String) -> Result<usize, String> {
        self.blocking(move || {
            let t = token.trim().to_string();
            let cred = tailscale::parse_credential(&t).ok_or("That does not look like a Tailscale credential: paste an API access token (tskey-api-…) or an OAuth client secret (tskey-client-…, optionally as clientId:secret)")?;
            let devices = TailscaleApi::new(cred, tailscale::ureq_http()).devices()?; // validates the credential (and, for OAuth, its scopes)
            keyring_entry().and_then(|e| e.set_password(&t).map_err(|e| e.to_string()))
                .map_err(|e| format!("Secure storage (Windows Credential Manager, macOS Keychain or a Linux keyring such as gnome-keyring/KWallet) is unavailable; the token was not saved ({e})"))?;
            Ok(devices.len())
        }).await?
    }
    pub async fn ts_clear_token(&self) {
        *self.ts_client.lock().unwrap() = None;
        let _ = self.blocking(|| { if let Ok(e) = keyring_entry() { let _ = e.delete_credential(); } }).await;
    }
    /// Everything the sharing panel shows: shared devices, guests (policy group), their join state, pending invites.
    pub async fn ts_overview(&self, machines: Vec<SparkArg>) -> Result<Overview, String> {
        let api = self.ts_api()?;
        self.blocking(move || {
            let (devices, users, invites, (policy, _)) = (api.devices()?, api.users()?, api.invites()?, api.get_policy()?);
            let low = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_lowercase();
            let guests = tailscale::guests_in(&policy).into_iter().map(|email| {
                let u = users.iter().find(|x| low(x, "loginName") == email);
                let inv = invites.iter().find(|x| low(x, "email") == email);
                let ids = |v: Option<&Value>| v.and_then(|v| v["id"].as_str().map(String::from));
                Guest { status: match (u, inv) { (Some(u), _) => u["status"].as_str().unwrap_or("active").to_string(), (None, Some(_)) => "invited".into(), _ => "not joined".into() }, user_id: ids(u), invite_id: ids(inv), email }
            }).collect();
            Ok(Overview { sparks: tailscale::match_devices(&devices, &machines, &self_name()), guests })
        }).await?
    }
    /// Preview the policy + tag changes needed to share the machines (no changes made). The plan is kept here:
    /// applying uses this copy, so only a plan the user saw (and which Nexus computed) can change the tailnet.
    pub async fn ts_plan(&self, a: PlanArgs) -> Result<PlanView, String> {
        let api = self.ts_api()?;
        let (view, plan) = self.blocking(move || -> Result<(PlanView, Plan), String> {
            let ((policy, etag), devices) = (api.get_policy()?, api.devices()?);
            let mut guests = tailscale::guests_in(&policy);
            if let Some(g) = &a.add_guest { guests.push(g.to_lowercase()); }
            if let Some(g) = &a.remove_guest { let g = g.to_lowercase(); guests.retain(|x| *x != g); }
            let (new_policy, mut changes) = tailscale::plan_policy(&policy, &PlanOpts { guests, ssh_users: a.ssh_users.clone(), per_guest_users: a.per_guest_users });
            let sparks = tailscale::match_devices(&devices, &a.machines, &self_name());
            let mut tag_changes = Vec::new();
            for s in &sparks {
                let Some(d) = s.device.as_ref().filter(|d| !d.tags.iter().any(|t| t == tailscale::SPARK_TAG)) else { continue };
                // Name the exact tailnet device being tagged (tagging gives guests SSH to it), not just the hostname it reports.
                changes.push(format!("Tag {} as {} (guests get SSH to it)", if d.name.is_empty() { &s.hostname } else { &d.name }, tailscale::SPARK_TAG));
                tag_changes.push((d.id.clone(), d.tags.iter().cloned().chain([tailscale::SPARK_TAG.to_string()]).collect()));
            }
            let missing = sparks.iter().filter(|s| s.device.is_none()).map(|s| format!("{}{}", s.hostname, if s.ambiguous { " (several devices use this hostname: set its Tailscale address in Settings)" } else { "" })).collect();
            Ok((PlanView { etag: etag.clone(), changes, missing }, Plan { policy: new_policy, etag, tag_changes }))
        }).await??;
        *self.last_plan.lock().unwrap() = Some(plan);
        Ok(view)
    }
    /// Apply a previewed plan: validate the policy with Tailscale, save it (only if unchanged since the preview), tag devices.
    pub async fn ts_apply(&self, etag: Option<String>) -> Result<(), String> {
        let plan = self.last_plan.lock().unwrap().take().filter(|p| p.etag == etag).ok_or("Review the access rules again before applying")?;
        let api = self.ts_api()?;
        self.blocking(move || {
            let v = api.validate_policy(&plan.policy)?;
            if let Some(msg) = v["message"].as_str() { return Err(format!("Tailscale rejected the policy: {msg}{}", v.get("data").map(|d| format!(" {d}")).unwrap_or_default())); }
            api.set_policy(&plan.policy, plan.etag.as_deref())?;
            for (device, tags) in &plan.tag_changes { api.set_tags(device, tags)?; }
            Ok(())
        }).await?
    }
    pub async fn ts_invite(&self, email: String) -> Result<String, String> {
        let api = self.ts_api()?;
        self.blocking(move || Ok(api.create_invite(&email)?["inviteUrl"].as_str().unwrap_or_default().to_string())).await?
    }
    pub async fn ts_revoke(&self, user_id: Option<String>, invite_id: Option<String>) -> Result<(), String> {
        let api = self.ts_api()?;
        self.blocking(move || {
            if let Some(i) = invite_id { api.delete_invite(&i)?; }
            if let Some(u) = user_id { api.delete_user(&u)?; }
            Ok(())
        }).await?
    }
    pub async fn ts_decode_invite(&self, code: String) -> Result<tailscale::Invite, String> {
        self.blocking(move || tailscale::decode_invite(&code, tailscale::local_status().tailnet.as_deref())).await?
    }

    // ---------------------------------------------------------------- updates
    /// Builds are unsigned, so a release is trusted only because it came from the project's GitHub. Nothing is
    /// fetched or staged silently: Nexus announces the version and the user chooses what to do.
    pub fn watch_updates(&self) {
        if cfg!(debug_assertions) || std::env::var_os("NEXUS_NO_UPDATE_CHECK").is_some() { return; }
        let me = self.arc();
        self.rt.spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await;
            loop {
                if !me.cfg.lock().unwrap().offline {
                    if let Ok(Some((version, url))) = me.blocking(latest_release).await { me.emit(Event::UpdateAvailable { version, url }); }
                }
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }
    /// Windows: download the release's installer, check it against the SHA-256 GitHub publishes for the asset, and
    /// start it. Returns the installer's path; the caller closes Nexus so the installer can replace it.
    pub async fn download_update(&self, version: String) -> Result<PathBuf, String> { self.blocking(move || fetch_installer(&version)).await? }
}

fn self_name() -> String { tailscale::local_status().self_peer.map(|p| p.dns_name).unwrap_or_default() }

fn github_agent() -> ureq::Agent { ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).build().into() }
fn release_json(path: &str) -> Option<Value> {
    let mut res = github_agent().get(format!("https://api.github.com/repos/{REPO}/releases/{path}")).header("Accept", "application/vnd.github+json").header("User-Agent", "Nexus").call().ok()?;
    serde_json::from_str(&res.body_mut().read_to_string().ok()?).ok()
}
pub fn newer(a: &str, b: &str) -> bool {
    let p = |s: &str| -> Vec<u64> { s.split('.').map(|x| x.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).chain([0, 0, 0]).take(3).collect() };
    p(a) > p(b)
}
fn latest_release() -> Option<(String, String)> {
    let rel = release_json("latest")?;
    let v = rel["tag_name"].as_str()?.trim_start_matches('v').to_string();
    (!v.is_empty() && newer(&v, VERSION)).then(|| (v, rel["html_url"].as_str().unwrap_or_default().to_string()))
}
fn fetch_installer(version: &str) -> Result<PathBuf, String> {
    use sha2::Digest;
    let rel = release_json(&format!("tags/v{version}")).ok_or("Couldn't read the release from GitHub")?;
    let name = format!("Nexus-Setup-{version}.exe");
    let asset = rel["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name.as_str())).ok_or(format!("Release {version} has no {name}"))?;
    let want = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).ok_or("GitHub did not publish a checksum for the installer; download it from the release page instead")?.to_lowercase();
    let url = asset["browser_download_url"].as_str().filter(|u| u.starts_with(&format!("https://github.com/{REPO}/releases/download/"))).ok_or("Unexpected download address")?;
    let mut res = github_agent().get(url).header("User-Agent", "Nexus").call().map_err(|e| e.to_string())?;
    let bytes = res.body_mut().with_config().limit(512 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())?;
    let got: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if got != want { return Err("The downloaded installer does not match the checksum GitHub published for it; it was not run".into()); }
    let path = std::env::temp_dir().join(name);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Pasted screenshots can hold anything on screen: keep them private to this user and delete them after 7 days.
fn paste_dir() -> PathBuf { std::env::temp_dir().join("nexus-paste") }
pub fn clean_paste_dir() {
    let Ok(rd) = std::fs::read_dir(paste_dir()) else { return };
    let week = Duration::from_secs(7 * 24 * 3600);
    for e in rd.flatten() {
        if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > week) { let _ = std::fs::remove_file(e.path()); }
    }
}
fn write_private(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; let _ = std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)); }
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).and_then(|mut f| f.write_all(data)).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    { std::fs::write(path, data).map_err(|e| e.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(newer("2.0.1", "2.0.0"));
        assert!(newer("2.1", "2.0.9"));
        assert!(!newer("2.0.0", "2.0.0"));
        assert!(!newer("1.9.9", "2.0.0"));
        assert!(newer("10.0.0", "9.9.9"));
        assert!(!newer("2.0.0-beta", "2.0.0"));
    }
}
