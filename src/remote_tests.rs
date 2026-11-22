//! End-to-end tests of the SSH layer against a real SSH server running in this process (russh's server side).
//! It authenticates one public key and runs the requested commands locally, with a pty when one was asked for, so
//! the tests exercise the same paths as a Linux machine: host-key trust, key auth, exec with stdin, uploads,
//! tmux-backed sessions that survive a dropped connection, port forwarding and reconnects.
#![cfg(unix)]
use crate::backend::{Backend, Event, SessionSpec};
use crate::config::{self, Config};
use crate::remote::*;
use russh::keys::ssh_key::LineEnding;
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

enum Proc { Pipe(Option<std::process::ChildStdin>), Pty(Box<dyn Write + Send>, Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>) }

#[derive(Clone)]
struct Srv { allowed: PublicKey, home: PathBuf, pty: Arc<Mutex<HashMap<ChannelId, (u16, u16)>>>, procs: Arc<Mutex<HashMap<ChannelId, Proc>>> }
impl server::Server for Srv {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self { Self { pty: Arc::default(), procs: Arc::default(), ..self.clone() } }
}
impl Srv {
    fn envs(&self) -> Vec<(&'static str, String)> {
        let h = self.home.to_string_lossy().into_owned();
        vec![("HOME", h.clone()), ("TMUX_TMPDIR", h), ("SHELL", "/bin/sh".into()), ("TERM", "xterm-256color".into())]
    }
}
impl server::Handler for Srv {
    type Error = russh::Error;
    async fn auth_publickey(&mut self, _user: &str, key: server::VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        Ok(if key.public_key.key_data() == self.allowed.key_data() { Auth::Accept } else { Auth::reject() })
    }
    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> { Ok(if password == PASSWORD { Auth::Accept } else { Auth::reject() }) }
    async fn channel_open_session(&mut self, _channel: Channel<Msg>, reply: server::ChannelOpenHandle, _s: &mut Session) -> Result<(), Self::Error> { reply.accept().await; Ok(()) }
    async fn pty_request(&mut self, channel: ChannelId, _term: &str, cols: u32, rows: u32, _pw: u32, _ph: u32, _modes: &[(russh::Pty, u32)], s: &mut Session) -> Result<(), Self::Error> {
        self.pty.lock().unwrap().insert(channel, (cols as u16, rows as u16));
        s.channel_success(channel)
    }
    async fn window_change_request(&mut self, channel: ChannelId, cols: u32, rows: u32, _pw: u32, _ph: u32, _s: &mut Session) -> Result<(), Self::Error> {
        if let Some(Proc::Pty(_, master, _)) = self.procs.lock().unwrap().get(&channel) { let _ = master.resize(portable_pty::PtySize { rows: rows as u16, cols: cols as u16, pixel_width: 0, pixel_height: 0 }); }
        Ok(())
    }
    async fn exec_request(&mut self, channel: ChannelId, data: &[u8], s: &mut Session) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        let (handle, rt) = (s.handle(), tokio::runtime::Handle::current());
        let finish = move |handle: server::Handle, rt: tokio::runtime::Handle, code: u32| rt.block_on(async { let _ = handle.exit_status_request(channel, code).await; let _ = handle.eof(channel).await; let _ = handle.close(channel).await; });
        if let Some((cols, rows)) = self.pty.lock().unwrap().get(&channel).copied() {
            let pair = portable_pty::native_pty_system().openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).unwrap();
            let mut cb = portable_pty::CommandBuilder::new(login_shell());
            cb.args(["-c", &cmd]);
            for (k, v) in self.envs() { cb.env(k, v); }
            cb.cwd(&self.home);
            let mut child = pair.slave.spawn_command(cb).unwrap();
            drop(pair.slave);
            let mut reader = pair.master.try_clone_reader().unwrap();
            self.procs.lock().unwrap().insert(channel, Proc::Pty(pair.master.take_writer().unwrap(), pair.master, child.clone_killer()));
            let procs = self.procs.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) { if n == 0 || rt.block_on(handle.data(channel, buf[..n].to_vec())).is_err() { break; } }
                let code = child.wait().map(|st| st.exit_code()).unwrap_or(1);
                procs.lock().unwrap().remove(&channel);
                finish(handle, rt, code);
            });
        } else {
            let mut child = std::process::Command::new(login_shell()).args(["-c", &cmd]).envs(self.envs()).current_dir(&self.home)
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
            self.procs.lock().unwrap().insert(channel, Proc::Pipe(child.stdin.take()));
            let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
            let (h2, rt2) = (handle.clone(), rt.clone());
            let t_err = std::thread::spawn(move || { let mut buf = [0u8; 8192]; while let Ok(n) = err.read(&mut buf) { if n == 0 || rt2.block_on(h2.extended_data(channel, 1, buf[..n].to_vec())).is_err() { break; } } });
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = out.read(&mut buf) { if n == 0 || rt.block_on(handle.data(channel, buf[..n].to_vec())).is_err() { break; } }
                let _ = t_err.join();
                let code = child.wait().ok().and_then(|st| st.code()).unwrap_or(1) as u32;
                finish(handle, rt, code);
            });
        }
        s.channel_success(channel)
    }
    async fn data(&mut self, channel: ChannelId, data: &[u8], _s: &mut Session) -> Result<(), Self::Error> {
        match self.procs.lock().unwrap().get_mut(&channel) {
            Some(Proc::Pipe(Some(stdin))) => { let _ = stdin.write_all(data); }
            Some(Proc::Pty(w, _, _)) => { let _ = w.write_all(data); let _ = w.flush(); }
            _ => {}
        }
        Ok(())
    }
    async fn channel_eof(&mut self, channel: ChannelId, _s: &mut Session) -> Result<(), Self::Error> {
        if let Some(Proc::Pipe(stdin)) = self.procs.lock().unwrap().get_mut(&channel) { stdin.take(); }
        Ok(())
    }
    async fn channel_close(&mut self, channel: ChannelId, _s: &mut Session) -> Result<(), Self::Error> {
        // Closing the channel hangs up the program on its pty, as sshd does.
        // (portable-pty's writer types newline + EOF into the pty when dropped, which sshd doesn't do: leak it instead.)
        let proc = self.procs.lock().unwrap().remove(&channel);
        if let Some(Proc::Pty(writer, _master, mut killer)) = proc { let _ = killer.kill(); std::mem::forget(writer); }
        Ok(())
    }
    async fn channel_open_direct_tcpip(&mut self, channel: Channel<Msg>, host: &str, port: u32, _oa: &str, _op: u32, reply: server::ChannelOpenHandle, _s: &mut Session) -> Result<(), Self::Error> {
        match tokio::net::TcpStream::connect((host, port as u16)).await {
            Ok(mut tcp) => { reply.accept().await; tokio::spawn(async move { let mut st = channel.into_stream(); let _ = tokio::io::copy_bidirectional(&mut st, &mut tcp).await; }); }
            Err(_) => drop(reply),
        }
        Ok(())
    }
}

/// The account password the test server accepts (only "Set up SSH key" ever offers one).
const PASSWORD: &str = "nexus-test-password";

struct TestServer { port: u16, home: PathBuf, key_file: PathBuf, stop: russh::server::RunningServerHandle }

/// Start a server with a fresh host key; `home` is the fake home folder its commands run in.
async fn start_server(tag: &str, client: Option<&PrivateKey>) -> TestServer {
    let home = std::env::temp_dir().join(format!("nexus-sshd-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let rng = || russh::keys::key::safe_rng();
    let own;
    let client = match client { Some(k) => k, None => { own = PrivateKey::random(&mut rng(), Algorithm::Ed25519).unwrap(); &own } };
    let key_file = home.join("client_key");
    std::fs::write(&key_file, client.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
    let cfg = Arc::new(server::Config { keys: vec![PrivateKey::random(&mut rng(), Algorithm::Ed25519).unwrap()], auth_rejection_time: Duration::from_millis(10), auth_rejection_time_initial: Some(Duration::ZERO), ..Default::default() });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut srv = Srv { allowed: client.public_key().clone(), home: home.clone(), pty: Arc::default(), procs: Arc::default() };
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let running = srv.run_on_socket(cfg, &listener);
        let _ = tx.send(running.handle());
        let _ = running.await;
    });
    TestServer { port, home, key_file, stop: rx.await.unwrap() }
}

#[derive(Default)]
struct Pins(Mutex<HashMap<String, String>>);
impl TrustStore for Pins {
    fn known(&self, k: &str) -> Option<String> { self.0.lock().unwrap().get(k).cloned() }
    fn remember(&self, k: &str, fp: &str) { self.0.lock().unwrap().insert(k.into(), fp.into()); }
}
struct Rig { machine: Arc<Machine>, pins: Arc<Pins>, asked: Arc<Mutex<Vec<String>>>, statuses: Arc<Mutex<Vec<(Status, Option<&'static str>)>>> }
fn rig(port: u16, key_file: PathBuf, trust_new_hosts: bool) -> Rig {
    let (pins, asked, statuses) = (Arc::new(Pins::default()), Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
    let (a2, s2) = (asked.clone(), statuses.clone());
    let cfg = config::Machine { id: "t".into(), name: "Test box".into(), kind: "ssh".into(), os: "linux".into(), host: "127.0.0.1".into(), port, user: "tester".into(), key_path: key_file.to_string_lossy().into_owned(), ..Default::default() };
    let machine = Machine::new(Store {
        get: Box::new(move || Some(cfg.clone())),
        trust: pins.clone(),
        ask_passphrase: Box::new(|_, _| Box::pin(async { None })),
        confirm_host: Box::new(move |u, _| { a2.lock().unwrap().push(u.fingerprint); Box::pin(async move { trust_new_hosts }) }),
        on_status: Box::new(move |st, _, route| s2.lock().unwrap().push((st, route))),
    });
    Rig { machine, pins, asked, statuses }
}
fn runtime() -> tokio::runtime::Runtime { tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap() }
const T: Duration = Duration::from_secs(20);

#[test]
fn connect_trust_exec_upload_forward_and_refusals() {
    runtime().block_on(async {
        let srv = start_server("basic", None).await;
        let r = rig(srv.port, srv.key_file.clone(), true);
        r.machine.connect().await.expect("connects with the key after the fingerprint is confirmed");
        assert_eq!(r.asked.lock().unwrap().len(), 1, "a new host key outside the tailnet is confirmed once");
        assert!(r.asked.lock().unwrap()[0].starts_with("SHA256:"));
        assert!(r.pins.known(&format!("127.0.0.1:{}", srv.port)).is_some(), "…and pinned");
        assert_eq!(r.statuses.lock().unwrap().last().copied(), Some((Status::Online, Some("lan"))));
        assert!(srv.home.join(".nexus/tmux.conf").exists(), "connect writes the tmux config");

        // exec: stdout, stderr, stdin and the exit status all arrive.
        let out = r.machine.exec("echo hi; cat; echo oops >&2; exit 3", Stdin::Bytes(b"from stdin".to_vec()), T).await.unwrap();
        assert_eq!((out.stdout.as_str(), out.stderr.as_str(), out.code), ("hi\nfrom stdin", "oops\n", Some(3)));
        assert_eq!(r.machine.home().await.unwrap(), srv.home.to_string_lossy());
        assert!(r.machine.exec("sleep 5", Stdin::None, Duration::from_millis(300)).await.is_err_and(|e| e.contains("timed out")));

        // probe and stats parse what a real shell prints.
        let p = r.machine.probe().await.unwrap();
        assert!(!p.hostname.is_empty() && p.tools.contains_key("tmux") && p.home == srv.home.to_string_lossy());
        let st = r.machine.stats().await.unwrap().unwrap();
        if cfg!(target_os = "linux") { assert!(st.cpus.unwrap_or(0.0) >= 1.0 && st.mem_total.is_some()); } // the stats script reads /proc

        // Uploads land in the private inbox, byte for byte (a buffer and a streamed file).
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let path = r.machine.upload_to_inbox("shot one.png", Stdin::Bytes(big.clone())).await.unwrap();
        assert!(path.starts_with(&format!("{}/.nexus/uploads/", srv.home.display())) && path.ends_with("-shot_one.png"), "{path}");
        assert_eq!(std::fs::read(&path).unwrap(), big);
        let local = srv.home.join("local.bin");
        std::fs::write(&local, &big).unwrap();
        let path2 = r.machine.upload_to_inbox("local.bin", Stdin::File(local)).await.unwrap();
        assert_eq!(std::fs::read(&path2).unwrap(), big);
        r.machine.upload("~/.nexus/bootstrap.sh", "#!/bin/sh\necho boot\n").await.unwrap();
        assert_eq!(r.machine.exec("~/.nexus/bootstrap.sh", Stdin::None, T).await.unwrap().stdout, "boot\n");

        // Fleet view: the hook reports of every Nexus session come back by session name (other files are ignored).
        std::fs::create_dir_all(srv.home.join(".nexus/agent")).unwrap();
        std::fs::write(srv.home.join(".nexus/agent/nx-claude-ab12.json"), r#"{"ts": 1700000000000, "event": "Notification", "message": "Claude needs your permission to use Bash", "summary": ""}"#).unwrap();
        std::fs::write(srv.home.join(".nexus/agent/nx-claude-cd34.json"), "{\"ts\": 5, \"event\": \"Stop\",\n \"summary\": \"Done.\"}\n").unwrap();
        std::fs::write(srv.home.join(".nexus/agent/pid-77.json"), "{}").unwrap();
        std::fs::write(srv.home.join(".nexus/agent/nx-broken.json"), "not json").unwrap();
        let agents = r.machine.agent_statuses().await;
        assert_eq!(agents.len(), 2, "{agents:?}");
        assert_eq!((agents["nx-claude-ab12"].event.as_str(), agents["nx-claude-ab12"].ts), ("Notification", 1_700_000_000_000));
        assert_eq!(agents["nx-claude-cd34"].summary, "Done.");

        // Replay: recordings are listed newest first and read back as they were written; nothing else can be read.
        assert!(r.machine.list_logs().await.unwrap().is_empty());
        std::fs::create_dir_all(srv.home.join(".nexus/logs")).unwrap();
        let rec = "\x1b[1mhello\x1b[0m \u{1f600}\r\n".repeat(50);
        std::fs::write(srv.home.join(".nexus/logs/nx-claude-ab12-20260101.log"), &rec).unwrap();
        std::fs::write(srv.home.join(".nexus/logs/notes.txt"), "x").unwrap();
        let logs = r.machine.list_logs().await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!((logs[0].name.as_str(), logs[0].size as usize), ("nx-claude-ab12-20260101.log", rec.len()));
        assert_eq!(r.machine.read_log(&logs[0].name).await.unwrap(), rec.as_bytes());
        for bad in ["../../etc/passwd", "a b.log", "", ".hidden", "x;id.log"] { assert!(r.machine.read_log(bad).await.is_err(), "{bad}"); }
        assert!(r.machine.read_log("missing.log").await.is_err());

        // Worktrees: a new branch in <repo>-worktrees/<branch> beside the repository, from any folder inside it.
        if crate::platform::run_text({ let mut c = crate::platform::command("git"); c.arg("--version"); c }, T).contains("git version") {
            let repo = srv.home.join("proj");
            std::fs::create_dir_all(repo.join("src")).unwrap();
            let sh = |cmd: &str| { let mut c = crate::platform::command("sh"); c.arg("-c").arg(cmd).current_dir(&repo); assert_eq!(crate::platform::run(c, None, T).code, 0, "{cmd}"); };
            sh("git init -q . && git -c user.email=a@b -c user.name=t commit -q --allow-empty -m init");
            let wt = r.machine.add_worktree("~/proj/src", "feature/login").await.unwrap();
            assert!(wt.ends_with("/proj-worktrees/feature-login"), "{wt}");
            assert!(std::path::Path::new(&wt).join(".git").exists());
            sh("git rev-parse --verify -q feature/login >/dev/null");
            // The same branch again can't be checked out twice; an existing branch elsewhere is checked out, not recreated.
            assert!(r.machine.add_worktree("~/proj", "feature/login").await.is_err());
            sh("git branch existing");
            let wt2 = crate::local::add_worktree(&repo.to_string_lossy(), "existing").unwrap();
            assert!(wt2.ends_with("proj-worktrees/existing") && std::path::Path::new(&wt2).join(".git").exists(), "{wt2}");
            assert!(r.machine.add_worktree("~", "x").await.is_err(), "not a repository");
        }

        // Port forwarding: bytes reach a listener on the "remote" side and come back.
        let echo = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        tokio::spawn(async move { while let Ok((mut s, _)) = echo.accept().await { tokio::spawn(async move { let (mut rd, mut wr) = s.split(); let _ = tokio::io::copy(&mut rd, &mut wr).await; }); } });
        let mut stream = r.machine.forward("127.0.0.1", echo_port, 0).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, b"ping through ssh").await.unwrap();
        let mut buf = [0u8; 16];
        tokio::time::timeout(T, tokio::io::AsyncReadExt::read_exact(&mut stream, &mut buf)).await.unwrap().unwrap();
        assert_eq!(&buf, b"ping through ssh");

        // Disconnect is reported; reconnecting doesn't ask about the (now pinned) host again.
        r.machine.disconnect();
        assert_eq!(r.statuses.lock().unwrap().last().map(|s| s.0), Some(Status::Offline));
        r.machine.connect().await.unwrap();
        assert_eq!(r.asked.lock().unwrap().len(), 1);

        // The server going away marks the machine offline without anyone asking.
        srv.stop.shutdown("bye".into());
        let gone = async { while r.machine.is_online() { tokio::time::sleep(Duration::from_millis(50)).await; } };
        tokio::time::timeout(T, gone).await.expect("a dropped connection is noticed");

        // A key the server doesn't know: a clear message naming what was tried.
        let srv2 = start_server("auth", None).await;
        let stranger = srv2.home.join("stranger");
        std::fs::write(&stranger, PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).unwrap().to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        let r2 = rig(srv2.port, stranger, true);
        let err = r2.machine.connect().await.unwrap_err();
        assert!(err.contains("Authentication failed for tester@127.0.0.1 (tried stranger"), "{err}");
        assert_eq!(r2.statuses.lock().unwrap().last().map(|s| s.0), Some(Status::Error));

        // Declining the fingerprint: not connected, nothing pinned, and no second prompt on the retry.
        let r3 = rig(srv2.port, srv2.key_file.clone(), false);
        assert!(r3.machine.connect().await.unwrap_err().contains("hasn't seen before"));
        assert!(r3.machine.connect().await.is_err());
        assert_eq!(r3.asked.lock().unwrap().len(), 1);
        assert!(r3.pins.0.lock().unwrap().is_empty());

        // A host key that changed since it was pinned is refused outright.
        let r4 = rig(srv2.port, srv2.key_file.clone(), true);
        r4.pins.remember(&format!("127.0.0.1:{}", srv2.port), "SHA256:somethingelse");
        let err = r4.machine.connect().await.unwrap_err();
        assert!(err.contains("changed (expected SHA256:somethingelse"), "{err}");
        assert!(r4.asked.lock().unwrap().is_empty());

        // Nothing listening: a routing answer, not an authentication one.
        let dead = rig(1, srv2.key_file.clone(), true);
        assert!(dead.machine.connect().await.unwrap_err().contains("refused the connection"));
    });
}

/// Feeds a session's output from the backend's event stream into a terminal emulator until its screen shows `needle`.
async fn wait_for(rx: &mut tokio::sync::mpsc::Receiver<Event>, pid: u64, needle: &str, term: &mut crate::term::Terminal) {
    let screen = |t: &crate::term::Terminal| t.lines_from(0).join("\n");
    let run = async {
        while !screen(term).contains(needle) {
            match rx.recv().await { Some(Event::Data(id, d)) if id == pid => { term.feed(&d); } Some(_) => {} None => break }
        }
    };
    if tokio::time::timeout(T, run).await.is_err() { panic!("never saw {needle:?} on screen:\n{}", screen(term)); }
}

#[test]
fn tmux_sessions_survive_detach_and_can_be_reattached_killed_and_watched() {
    if crate::platform::which("tmux").is_none() { eprintln!("tmux is not installed: skipping"); return; }
    let rt = runtime();
    let handle = rt.handle().clone();
    rt.block_on(async {
        let srv = start_server("tmux", None).await;
        let mut cfg = Config::default();
        cfg.machines[1] = config::Machine { id: "linux".into(), name: "Test box".into(), kind: "ssh".into(), os: "linux".into(), host: "127.0.0.1".into(), port: srv.port, user: "tester".into(), key_path: srv.key_file.to_string_lossy().into_owned(), ..Default::default() };
        cfg.presets.push(config::Preset { id: "counter".into(), name: "Counter".into(), cmd: [("linux".to_string(), "echo started-$((20+22)); cat".to_string())].into(), ..Default::default() });
        // Trust the test server up front (the prompt path is covered by the other test).
        std::env::set_var("NEXUS_CONFIG_DIR", &srv.home);
        let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
        let be = Backend::new(handle, cfg, tx, Box::new(|| {}));
        let b2 = be.clone();
        let answer = tokio::spawn(async move {
            // Answer the "trust this host?" prompt the way the UI would, and keep the other events for the test.
            let mut kept = Vec::new();
            while let Some(ev) = rx.recv().await {
                if let Event::AskHost { ask, .. } = ev { b2.answer(ask, Some("yes".into())); break; }
                kept.push(ev);
            }
            (rx, kept)
        });
        be.machine("linux").connect().await.unwrap();
        let (mut rx, _) = answer.await.unwrap();

        let spec = SessionSpec { machine_id: "linux".into(), preset_id: "counter".into(), cols: 100, rows: 30, cwd: Some("~".into()), ..Default::default() };
        let (pid, name) = be.create_session(spec.clone()).await.unwrap();
        let name = name.expect("remote sessions get a tmux name");
        assert!(name.starts_with("nx-counter-"), "{name}");
        let mut seen = crate::term::Terminal::new(100, 30);
        wait_for(&mut rx, pid, "started-42", &mut seen).await;
        be.resize(pid, 90, 25);
        seen.resize(90, 25);
        be.write(pid, b"typed into the session\n");
        wait_for(&mut rx, pid, "typed into the session\ntyped into the session", &mut seen).await; // echoed by the tty, then by cat

        // The session shows up in the probe, with its folder under ~.
        let p = be.probe("linux").await.unwrap();
        let s = p.sessions.iter().find(|s| s.name == name).expect("listed by tmux");
        assert_eq!((s.cwd.as_str(), s.attached), ("~", 1));
        assert_eq!(be.session_cwd(pid).await.as_deref(), Some(&*srv.home.to_string_lossy()));

        // Detach (close without kill): the program keeps running, and reattaching shows it again.
        be.close(pid, false);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let after = be.probe("linux").await.unwrap().sessions;
        assert!(after.iter().any(|s| s.name == name && s.attached == 0), "still running, nobody attached: {after:?}");
        let (pid2, name2) = be.create_session(SessionSpec { attach: Some(name.clone()), ..spec.clone() }).await.unwrap();
        assert_eq!(name2.as_deref(), Some(name.as_str()));
        let mut seen2 = crate::term::Terminal::new(100, 30);
        wait_for(&mut rx, pid2, "typed into the session", &mut seen2).await; // tmux redraws what was on screen
        be.write(pid2, b"after reattach\n");
        wait_for(&mut rx, pid2, "after reattach\nafter reattach", &mut seen2).await;

        // Watching read-only: output arrives, keystrokes don't.
        let (pid3, _) = be.create_session(SessionSpec { attach: Some(name.clone()), read_only: true, ..spec.clone() }).await.unwrap();
        let mut seen3 = crate::term::Terminal::new(100, 30);
        wait_for(&mut rx, pid3, "after reattach", &mut seen3).await;
        be.write(pid3, b"SHOULD-NOT-APPEAR\n");
        be.write(pid2, b"owner still types\n");
        wait_for(&mut rx, pid3, "owner still types\nowner still types", &mut seen3).await;
        assert!(!seen3.lines_from(0).join("\n").contains("SHOULD-NOT-APPEAR"));
        be.close(pid3, false);

        // Kill: the tmux session is gone.
        be.close(pid2, true);
        let gone = async { loop { if !be.probe("linux").await.unwrap().sessions.iter().any(|s| s.name == name) { break; } tokio::time::sleep(Duration::from_millis(100)).await; } };
        tokio::time::timeout(T, gone).await.expect("killed");

        // A raw session (installers, account setup) runs outside tmux and reports its exit.
        let (pid4, none) = be.create_session(SessionSpec { machine_id: "linux".into(), preset_id: "shell".into(), cols: 80, rows: 24, raw: true, cmd: Some("echo raw-$((1+1)); exit".into()), ..Default::default() }).await.unwrap();
        assert!(none.is_none());
        let exited = async { loop { if let Some(Event::Exit { pid, disconnected, .. }) = rx.recv().await { if pid == pid4 { break disconnected; } } } };
        assert!(!tokio::time::timeout(T, exited).await.unwrap(), "a normal exit is not a dropped connection");

        // Team chat on the hub: a posted message comes back through the tail, presence round-trips.
        be.start_hub("linux").await.unwrap();
        be.collab_post(crate::collab::From { name: "Frenchy".into(), login: String::new(), client: "abc123".into() }, "GPU is free".into(), "msg").await.unwrap();
        let got = async { loop { if let Some(Event::CollabMessage(m)) = rx.recv().await { break m; } } };
        let m = tokio::time::timeout(T, got).await.unwrap();
        assert_eq!((m.text.as_str(), m.from.name.as_str()), ("GPU is free", "Frenchy"));
        let pres = be.collab_heartbeat(serde_json::json!({ "client": "abc123", "name": "Frenchy", "status": "testing", "sessions": [], "claims": [{ "host": "box", "until": 9_999_999_999_999i64, "note": "" }] })).await.unwrap();
        assert_eq!((pres.len(), pres[0].status.as_str(), pres[0].claims.len()), (1, "testing", 1));
        assert_eq!(be.collab_presets_put(config::default_presets()).await.unwrap(), 5);
        assert_eq!(be.collab_presets_get().await.unwrap().len(), 5);

        // A forwarded port reaches a service on the machine.
        let echo = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        tokio::spawn(async move { while let Ok((mut s, _)) = echo.accept().await { tokio::spawn(async move { let (mut rd, mut wr) = s.split(); let _ = tokio::io::copy(&mut rd, &mut wr).await; }); } });
        let t = be.tunnel_open("linux", "127.0.0.1", echo_port, 0).await.unwrap();
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", t.local_port)).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut c, b"via tunnel").await.unwrap();
        let mut buf = [0u8; 10];
        tokio::time::timeout(T, tokio::io::AsyncReadExt::read_exact(&mut c, &mut buf)).await.unwrap().unwrap();
        assert_eq!(&buf, b"via tunnel");
        assert!(be.tunnel_open("linux", "bad host!", 80, 0).await.is_err());
        be.tunnel_close(&t.id);
        assert!(be.tunnels().is_empty());

        be.shutdown();
        let _ = be.machine("linux").exec(&format!("{TMUX} kill-server"), Stdin::None, T).await;
        srv.stop.shutdown("done".into());
    });
}

#[test]
fn local_sessions_run_report_exit_and_feed_the_mcp_tools() {
    let rt = runtime();
    let handle = rt.handle().clone();
    rt.block_on(async {
        if std::env::var_os("NEXUS_CONFIG_DIR").is_none() { std::env::set_var("NEXUS_CONFIG_DIR", std::env::temp_dir().join(format!("nexus-cfg-{}", std::process::id()))); }
        let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
        let be = Backend::new(handle, Config::default(), tx, Box::new(|| {}));
        // Whatever the login shell and its prompt are, the session runs a quiet /bin/sh: no echo, no prompt.
        let quiet = Some("stty -echo; PS1= PS2= exec /bin/sh".to_string());
        let spec = SessionSpec { machine_id: "local".into(), preset_id: "shell".into(), cols: 80, rows: 24, cwd: Some("/tmp".into()), cmd: quiet.clone(), ..Default::default() };
        let (pid, tmux) = be.create_session(spec).await.unwrap();
        assert!(tmux.is_none(), "local sessions have no tmux");
        let mut term = crate::term::Terminal::new(80, 24);
        be.write(pid, b"echo local-$((40+2)); basename \"$PWD\"\n");
        wait_for(&mut rx, pid, "local-42\ntmp", &mut term).await;
        be.resize(pid, 100, 30);
        be.write(pid, b"stty size\n");
        wait_for(&mut rx, pid, "30 100", &mut term).await;

        // What the MCP tools see: the session list with its last line, and the recent output as plain text.
        let list = be.mcp_call(crate::mcp::Call::ListSessions).await.unwrap();
        assert_eq!((list[0]["id"].as_str(), list[0]["preset"].as_str(), list[0]["machineId"].as_str()), (Some(&*format!("s{pid}")), Some("shell"), Some("local")));
        be.mcp_call(crate::mcp::Call::Send { session: format!("s{pid}"), text: "echo from-mcp\n".into() }).await.unwrap();
        wait_for(&mut rx, pid, "from-mcp", &mut term).await;
        let out = be.mcp_call(crate::mcp::Call::ReadOutput { session: format!("s{pid}") }).await.unwrap();
        assert!(crate::mcp::plain(out.as_str().unwrap()).contains("local-42"));
        assert!(be.mcp_call(crate::mcp::Call::Send { session: "s999".into(), text: String::new() }).await.is_err());
        let machines = be.mcp_call(crate::mcp::Call::ListMachines).await.unwrap();
        assert_eq!(machines[0]["status"], "online");

        // The program exiting is reported with its code; closing a session reports nothing.
        be.write(pid, b"exit 7\n");
        let exited = async { loop { if let Some(Event::Exit { pid: p, code, disconnected }) = rx.recv().await { if p == pid { break (code, disconnected); } } } };
        assert_eq!(tokio::time::timeout(T, exited).await.unwrap(), (Some(7), false));
        let (pid2, _) = be.create_session(SessionSpec { machine_id: "local".into(), preset_id: "shell".into(), cols: 80, rows: 24, cmd: quiet, ..Default::default() }).await.unwrap();
        be.close(pid2, false);
        assert!(be.mcp_call(crate::mcp::Call::ListSessions).await.unwrap().as_array().unwrap().is_empty());
    });
}

/// The shell the test server runs every command through, as sshd does with the account's login shell. Set
/// NEXUS_TEST_LOGIN_SHELL to a fish (or csh, zsh…) binary to run these tests against an account that uses it.
fn login_shell() -> String { std::env::var("NEXUS_TEST_LOGIN_SHELL").unwrap_or_else(|_| "/bin/bash".into()) }

/// Not a test: keeps the in-process SSH server up so the app itself can be pointed at it (screenshots, hand checks).
///   NEXUS_SERVE_SECS=60 NEXUS_SERVE_INFO=/tmp/serve.json cargo test serve_for_hand_checks -- --ignored
/// The info file gets the port, the client key and the host-key pin for config.json's knownHosts.
#[test]
#[ignore]
fn serve_for_hand_checks() {
    runtime().block_on(async {
        let srv = start_server("serve", None).await;
        let r = rig(srv.port, srv.key_file.clone(), true);
        r.machine.connect().await.unwrap();
        let pin_key = format!("127.0.0.1:{}", srv.port);
        let info = serde_json::json!({ "port": srv.port, "key": srv.key_file, "home": srv.home, "pinKey": pin_key, "pin": r.pins.known(&pin_key) });
        std::fs::write(std::env::var("NEXUS_SERVE_INFO").unwrap_or("/tmp/nexus-serve.json".into()), info.to_string()).unwrap();
        tokio::time::sleep(Duration::from_secs(std::env::var("NEXUS_SERVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60))).await;
    });
}

#[test]
fn set_up_key_signs_in_with_the_password_once_and_installs_the_device_key() {
    use std::os::unix::fs::PermissionsExt;
    runtime().block_on(async {
        let srv = start_server("setup-key", None).await;
        // This device's key is made on first use, kept private, and reused afterwards.
        let dir = srv.home.join("device").join(".ssh");
        let public = device_key_in(&dir).unwrap();
        assert!(public.starts_with("ssh-ed25519 "), "{public}");
        assert_eq!(device_key_in(&dir).unwrap(), public);
        assert_eq!(std::fs::metadata(dir.join("id_ed25519")).unwrap().permissions().mode() & 0o777, 0o600);

        // The server doesn't know that key yet: signing in fails, and a wrong password doesn't help.
        let r = rig(srv.port, dir.join("id_ed25519"), true);
        assert!(r.machine.connect().await.is_err());
        assert!(r.machine.install_key_as(public.clone(), "wrong".into()).await.is_err());
        assert!(!srv.home.join(".ssh/authorized_keys").exists());

        // The right one gets in, and the key is added exactly once however often it is set up.
        assert_eq!(r.machine.install_key_as(public.clone(), PASSWORD.into()).await.unwrap(), public);
        assert!(r.machine.is_online());
        r.machine.install_key_as(public.clone(), PASSWORD.into()).await.unwrap();
        assert_eq!(std::fs::read_to_string(srv.home.join(".ssh/authorized_keys")).unwrap(), format!("{public}\n"));
        assert_eq!(std::fs::metadata(srv.home.join(".ssh/authorized_keys")).unwrap().permissions().mode() & 0o077, 0, "private to the account");

        // The password was for that connection only: without it (this test server never learns new keys) sign-in fails again.
        r.machine.disconnect();
        assert!(r.machine.connect().await.is_err());
        srv.stop.shutdown("done".into());
    });
}
