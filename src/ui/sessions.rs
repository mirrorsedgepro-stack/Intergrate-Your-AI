//! Sessions and machines: launching, reattaching, reconnecting, activity tracking, restore and workspaces.
use super::*;
use crate::backend::SessionSpec;
use crate::config::{SnapSession, Snapshot, Split};
use crate::remote::AgentStatus;
use crate::term::{TermOut, Terminal};
use crate::util::{base_name, clip};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SessStatus { Starting, Running, Exited, Disconnected }
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Activity { Idle, Busy, Done }

/// How a session is to be created.
#[derive(Default, Clone)]
pub struct Extra {
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub attach: Option<String>,
    pub raw: bool,
    pub cmd: Option<String>,
    pub script: Option<String>,
    pub script_machine: Option<String>,
    pub read_only: bool,
    /// Already waited in the queue: don't ask about claims again.
    pub queued: bool,
}

pub struct Session {
    pub id: u64,
    pub machine_id: String,
    pub preset_id: String,
    pub color: Color32,
    pub name: String,
    pub tmux_name: Option<String>,
    pub cwd: Option<String>,
    pub spec: Extra,
    pub read_only: bool,
    announce: bool,
    /// The backend's id while it runs.
    pub pid: Option<u64>,
    pub status: SessStatus,
    pub activity: Activity,
    pub title: String,
    pub term: Terminal,
    last_data: i64,
    pub last_input: i64,
    pub last_pointer: i64,
    burst: usize,
    pub quiet_until: i64,
    busy_since: i64,
    /// Where the user's last submitted prompt was, for "Send last reply".
    prompt_line: Option<usize>,
    pub retry: u32,
    retry_at: Option<i64>,
    pending_input: Vec<u8>,
    pending_at: Option<i64>,
    pub agent: Option<AgentStatus>,
    /// Why it is waiting (what the bell, hook or notification said), for the fleet view.
    pub why: Option<String>,
    pub search: Option<String>,
    pub search_focus: bool,
    /// Wheel movement not yet turned into whole lines.
    pub scroll_acc: f32,
    /// The cell last reported to a program that owns the mouse, while the button is held.
    pub mouse_cell: Option<(u16, u16)>,
    /// Last press (time, cell, count) for double / triple click.
    pub click: (i64, (u16, u16), u8),
}

const AGENTS: &[&str] = &["claude", "antigravity", "openclaw"];
pub fn is_agent(preset_id: &str) -> bool { AGENTS.contains(&preset_id) }
pub fn preset_from_tmux(name: &str) -> String {
    name.strip_prefix("nx-").and_then(|r| r.split('-').next()).filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())).unwrap_or("shell").to_string()
}

impl App {
    // ---------------------------------------------------------------- sessions
    pub fn make_session(&mut self, machine_id: &str, preset_id: &str, extra: Extra, announce: bool) -> u64 {
        let p = self.preset(preset_id);
        self.uid += 1;
        let id = self.uid;
        let same = self.sessions.values().filter(|x| x.machine_id == machine_id && x.preset_id == preset_id).count();
        let name = extra.name.clone().unwrap_or_else(|| match extra.cwd.as_deref() {
            Some(c) if c != "~" => format!("{} · {}", p.name, base_name(c)),
            _ if same > 0 => format!("{} {}", p.name, same + 1),
            _ => p.name.clone(),
        });
        let s = Session {
            id, machine_id: machine_id.into(), preset_id: preset_id.into(), color: hex(&p.color), name, tmux_name: extra.attach.clone(), cwd: extra.cwd.clone(),
            read_only: extra.read_only, announce, spec: extra, pid: None, status: SessStatus::Starting, activity: Activity::Idle, title: String::new(),
            term: Terminal::new(self.last_cells.0, self.last_cells.1),
            last_data: 0, last_input: 0, last_pointer: 0, burst: 0, quiet_until: 0, busy_since: 0, prompt_line: None, retry: 0, retry_at: None, pending_input: vec![], pending_at: None,
            agent: None, why: None, search: None, search_focus: false, scroll_acc: 0.0, mouse_cell: None, click: (0, (0, 0), 0),
        };
        self.sessions.insert(id, s);
        self.order.push(id);
        id
    }

    pub fn start(&mut self, id: u64) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let m = self.cfg.machine(&s.machine_id).cloned().unwrap_or_default();
        s.status = SessStatus::Starting;
        s.retry_at = None;
        if m.is_ssh() && s.retry == 0 { s.term.write_str(&format!("\x1b[2m{} to {}…\x1b[0m\r\n", if s.tmux_name.is_some() { "attaching" } else { "connecting" }, m.name)); }
        let spec = SessionSpec {
            machine_id: s.machine_id.clone(), preset_id: s.preset_id.clone(), cols: s.term.cols, rows: s.term.rows, attach: s.tmux_name.clone(), cwd: s.cwd.clone(),
            raw: s.spec.raw, cmd: s.spec.cmd.clone(), script: s.spec.script.clone(), script_machine: s.spec.script_machine.clone(), read_only: s.spec.read_only,
        };
        let be = self.be.clone();
        self.be.run(async move { be.create_session(spec).await }, move |app, r| app.started(id, r));
    }

    fn started(&mut self, id: u64, r: Result<(u64, Option<String>), String>) {
        let Some(s) = self.sessions.get_mut(&id) else { if let Ok((pid, _)) = r { self.be.close(pid, false); } return };
        let (pid, tmux) = match r {
            Ok(v) => v,
            Err(e) => {
                // A remote session that dropped keeps retrying quietly; anything else waits for Enter.
                if s.retry > 0 { s.status = SessStatus::Disconnected; self.schedule_reconnect(id); return; }
                s.status = SessStatus::Exited;
                s.term.write_str(&format!("\r\n\x1b[38;2;247;118;142m✖ {e}\x1b[0m\r\n\x1b[2mEnter to retry · {} to close\x1b[0m\r\n", kbd("Ctrl+Shift+W")));
                return;
            }
        };
        if s.retry > 0 { s.term.write_str("\x1b[38;2;115;218;202m✓ Reconnected\x1b[0m\r\n"); }
        let announce = std::mem::take(&mut s.announce);
        s.retry = 0;
        s.pid = Some(pid);
        if tmux.is_some() { s.tmux_name = tmux; }
        s.status = SessStatus::Running;
        s.quiet_until = now_ms() + 2500;
        // Typed before it connected: sent once the program is up.
        if !s.pending_input.is_empty() { s.pending_at = Some(now_ms() + 400); }
        let (cols, rows) = (s.term.cols, s.term.rows);
        self.by_pid.insert(pid, id);
        self.be.resize(pid, cols, rows);
        if let Some(early) = self.early.remove(&pid) { self.on_data(pid, &early); }
        if announce { self.announce_launch(id); }
        self.persist_soon();
    }

    /// Start a session (asking first when someone else is working there, or the machine is claimed).
    pub fn launch(&mut self, machine_id: &str, preset_id: &str, extra: Extra) { self.launch_stage(machine_id, preset_id, extra, 0, None); }
    pub(super) fn launch_stage(&mut self, machine_id: &str, preset_id: &str, extra: Extra, stage: u8, done: Option<Box<dyn FnOnce(&mut App, Option<u64>)>>) {
        let m = self.machine(machine_id);
        let plain = !extra.raw && extra.script.is_none() && extra.attach.is_none();
        if stage == 0 {
            if m.is_ssh() && !m.has_address() { self.open_settings(); self.toast(format!("Add the address of {} first.", m.name), TEXT2, 3800); if let Some(d) = done { d(self, None); } return; }
            if self.is_missing(&m, &self.preset(preset_id)) && !extra.raw && extra.script.is_none() { let hint = self.install_hint(&m, preset_id); self.toast_warn(hint); }
        }
        if stage == 0 && plain && self.check_folder_conflict(machine_id, preset_id, extra.clone(), done.is_none()) { if let Some(d) = done { d(self, None); } return; }
        if stage <= 1 && plain && !extra.queued && self.check_claim(machine_id, preset_id, extra.clone(), done.is_none()) { if let Some(d) = done { d(self, None); } return; }
        let id = self.make_session(machine_id, preset_id, extra, plain);
        self.show(id);
        self.start(id);
        if let Some(d) = done { d(self, Some(id)); }
    }

    pub fn reattach(&mut self, machine_id: &str, name: &str, focus: bool, auto: bool, read_only: bool) {
        if let Some(existing) = self.sessions.values().find(|s| s.machine_id == machine_id && s.tmux_name.as_deref() == Some(name) && !s.read_only).map(|s| s.id) { self.show(existing); return; }
        if !auto && !read_only && self.check_session_conflict(machine_id, name) { return; }
        let d = self.mst(machine_id).detached.into_iter().find(|x| x.name == name);
        let preset = preset_from_tmux(name);
        let p = self.preset(&preset);
        let id = self.make_session(machine_id, &preset, Extra { attach: Some(name.into()), cwd: d.map(|d| d.cwd).filter(|c| !c.is_empty()), read_only, name: read_only.then(|| format!("{} (watching)", p.name)), ..Default::default() }, false);
        if focus || !self.panes.iter().any(Option::is_some) { self.show(id); }
        self.start(id);
    }

    pub fn close_session(&mut self, id: u64, kill: bool) {
        let Some(s) = self.sessions.remove(&id) else { return };
        self.order.retain(|x| *x != id);
        self.mru.retain(|x| *x != id);
        self.targets.remove(&id);
        match (s.pid, &s.tmux_name) {
            (Some(pid), _) => { self.by_pid.remove(&pid); self.be.close(pid, kill); }
            (None, Some(name)) if kill => self.be.kill_tmux(&s.machine_id, name),
            _ => {}
        }
        if let Some(i) = self.panes.iter().position(|p| *p == Some(id)) { self.panes[i] = self.mru.iter().copied().find(|x| !self.panes.contains(&Some(*x))); }
        if self.machine(&s.machine_id).is_ssh() { let mid = s.machine_id.clone(); self.be.run(async { tokio::time::sleep(std::time::Duration::from_millis(700)).await }, move |app, ()| app.refresh_machine(&mid)); }
        self.persist_soon();
    }
    pub fn close_focused(&mut self) { if let Some(id) = self.focused() { self.close_session(id, false); } }

    fn install_hint(&self, m: &Machine, preset_id: &str) -> String {
        let p = self.preset(preset_id);
        let win = cfg!(windows);
        if m.is_ssh() { return format!("{} isn't installed on {}. Use \"Install tools\" from the machine's ⋯ menu.", p.name, m.name); }
        let (what, sh, ps) = match preset_id {
            "claude" => ("Claude Code", "curl -fsSL https://claude.ai/install.sh | bash", "irm https://claude.ai/install.ps1 | iex"),
            "antigravity" => ("Antigravity CLI (agy)", "curl -fsSL https://antigravity.google/cli/install.sh | bash", "irm https://antigravity.google/cli/install.ps1 | iex"),
            "openclaw" => ("OpenClaw", "curl -fsSL https://openclaw.ai/install.sh | bash (it walks you through setup)", "iwr -useb https://openclaw.ai/install.ps1 | iex (it walks you through setup)"),
            _ => return format!("{} isn't available on {}.", p.name, m.name),
        };
        if m.is_wsl() { format!("{what} isn't installed in {}. In a WSL shell: {sh}", m.distro) }
        else if win { format!("{what} isn't installed here. In PowerShell: {ps}") } else { format!("{what} isn't installed here. In a terminal: {sh}") }
    }

    // ---------------------------------------------------------------- auto-reconnect
    /// Dropped remote sessions retry with backoff (2s, 4s, 8s … 30s); tmux kept them running meanwhile.
    fn schedule_reconnect(&mut self, id: u64) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        s.retry += 1;
        if s.retry == 1 { s.term.write_str("\r\n\x1b[38;2;224;175;104m⚡ Connection lost. The session keeps running in tmux; reconnecting automatically (Enter to retry now).\x1b[0m\r\n"); }
        s.retry_at = Some(now_ms() + i64::from(2u32.saturating_pow(s.retry).min(30)) * 1000);
    }
    pub(super) fn session_timers(&mut self, t: i64) {
        let due: Vec<u64> = self.sessions.values().filter(|s| s.retry_at.is_some_and(|at| t >= at) && s.status != SessStatus::Running && s.status != SessStatus::Starting).map(|s| s.id).collect();
        for id in due { self.start(id); }
        let be = self.be.clone();
        for s in self.sessions.values_mut() {
            if let (Some(at), Some(pid)) = (s.pending_at, s.pid) { if t >= at { s.pending_at = None; be.write(pid, &std::mem::take(&mut s.pending_input)); } }
        }
    }
    fn schedule_machine_retry(&mut self, id: &str) {
        if self.manual_disconnect.contains(id) || self.machine_retry.contains_key(id) { return; }
        self.machine_retry.insert(id.to_string(), (0, now_ms() + 2000));
    }
    pub(super) fn machine_retry_tick(&mut self, id: &str) {
        let Some((n, _)) = self.machine_retry.get(id).copied() else { return };
        if self.manual_disconnect.contains(id) || self.cfg.machine(id).is_none() { self.machine_retry.remove(id); return; }
        let n = n + 1;
        // Parked far ahead while this attempt runs; the result reschedules or clears it.
        self.machine_retry.insert(id.to_string(), (n, i64::MAX));
        let (be, mid) = (self.be.clone(), id.to_string());
        let mid2 = mid.clone();
        self.be.run(async move { be.machine(&mid).connect().await.is_ok() }, move |app, ok| {
            if ok { app.machine_retry.remove(&mid2); app.refresh_machine(&mid2); }
            else if app.machine_retry.contains_key(&mid2) { app.machine_retry.insert(mid2, (n, now_ms() + i64::from(2u32.saturating_pow(n).min(60)) * 1000)); }
        });
    }

    // ---------------------------------------------------------------- last reply extraction
    /// Text the agent printed since the user's last submitted prompt, minus TUI chrome (boxes, hints).
    fn last_reply(&self, id: u64) -> String {
        let Some(s) = self.sessions.get(&id) else { return String::new() };
        let end = s.term.total_lines();
        let from = s.prompt_line.map_or(end.saturating_sub(120), |p| p.saturating_sub(2)).min(end);
        let is_box = |t: &str| !t.trim().is_empty() && t.chars().all(|c| c.is_whitespace() || "─━│┃╭╮╰╯┌┐└┘═║╔╗╚╝▔▁·•".contains(c));
        let is_chrome = |t: &str| {
            let l = t.trim_start().to_lowercase();
            ["? for shortcuts", "esc to interrupt", "⏵⏵", "✻", "✶", "✳", "auto-accept", "bypass permissions"].iter().any(|p| l.starts_with(p))
                || (l.starts_with("press ") && l.contains(" to")) || (l.starts_with('·') && l.contains("ing…"))
        };
        let mut lines: Vec<String> = s.term.lines_from(from).into_iter().filter_map(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix(['│', '┃']).map_or(l.as_str(), |r| r.strip_prefix(' ').unwrap_or(r));
            let t = t.trim_end().strip_suffix(['│', '┃']).map_or(t, |r| r.strip_suffix(' ').unwrap_or(r));
            (!is_box(t) && !is_chrome(t)).then(|| t.to_string())
        }).collect();
        // Drop the echoed prompt (first lines starting with ">") and the empty input box at the bottom.
        while lines.first().is_some_and(|l| l.trim_start().starts_with('>') || l.trim().is_empty()) { lines.remove(0); }
        while lines.last().is_some_and(|l| l.trim() == ">" || l.trim().is_empty()) { lines.pop(); }
        let mut out = String::new();
        let mut blanks = 0;
        for l in lines { if l.trim().is_empty() { blanks += 1; if blanks > 1 { continue; } } else { blanks = 0; } out.push_str(&l); out.push('\n'); }
        let chars: Vec<char> = out.trim().chars().collect();
        chars[chars.len().saturating_sub(20_000)..].iter().collect()
    }
    pub fn send_last_reply(&mut self, id: u64) {
        let text = self.last_reply(id);
        if text.is_empty() { self.toast_warn("Nothing to send yet: no output since your last prompt"); return; }
        let Some(s) = self.sessions.get(&id) else { return };
        let intro = format!("Here is the latest output from {} on {}:\n\n{text}\n", s.name, self.machine(&s.machine_id).name);
        self.targets = self.sessions.values().filter(|o| o.id != id && o.status == SessStatus::Running && is_agent(&o.preset_id)).map(|o| o.id).collect();
        self.open_composer(Some(intro));
    }

    // ---------------------------------------------------------------- activity tracking
    fn mark_done(&mut self, id: u64, notify: bool, body: Option<String>) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        s.activity = Activity::Done;
        s.why = body.clone().filter(|b| !b.is_empty());
        if notify && self.cfg.appearance.notify_when_done {
            let (name, title, mid) = (s.name.clone(), s.title.clone(), s.machine_id.clone());
            let body = body.filter(|b| !b.is_empty()).unwrap_or(if title.is_empty() { "Finished - waiting for you".into() } else { title });
            let head = format!("{name} on {} is ready", self.machine(&mid).name);
            self.notify(&head, &body, Some(id));
            // The phone hears about it only while Nexus isn't in front of you.
            if !self.win_focused { self.be.push(head, if self.cfg.push.include_text { body } else { "Waiting for you".into() }); }
            self.flash();
        }
    }
    pub fn clear_done(&mut self, id: u64) {
        if let Some(s) = self.sessions.get_mut(&id) { if s.activity == Activity::Done { s.activity = Activity::Idle; s.agent = None; s.why = None; } }
    }
    pub(super) fn activity_tick(&mut self) {
        let t = now_ms();
        let mut done = Vec::new();
        for s in self.sessions.values_mut() {
            if s.activity == Activity::Busy && t - s.last_data > 2500 {
                s.burst = 0;
                if self.panes.contains(&Some(s.id)) && self.win_focused { s.activity = Activity::Idle; } else { done.push((s.id, is_agent(&s.preset_id) && t - s.busy_since > 5000)); }
            } else if s.activity != Activity::Busy && t - s.last_data > 1500 { s.burst = 0; }
        }
        for (id, notify) in done { self.mark_done(id, notify, None); }
    }
    /// Bells (and OSC 9 / 777 notifications) are exact "agent is waiting" signals: Claude's Stop/Notification
    /// hooks ring one (see "Enable Claude alerts"), so they override the output-volume heuristic.
    fn signal(&mut self, id: u64, body: Option<String>) {
        if self.is_visible(id) && self.win_focused { if let Some(s) = self.sessions.get_mut(&id) { s.activity = Activity::Idle; } return; }
        self.mark_done(id, true, body);
    }

    pub(super) fn on_data(&mut self, pid: u64, data: &[u8]) {
        let Some(&id) = self.by_pid.get(&pid) else { self.early.entry(pid).or_default().extend_from_slice(data); return };
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let outs = s.term.feed(data);
        let t = now_ms();
        s.last_data = t;
        if t > s.quiet_until && t - s.last_input > 500 {
            s.burst += data.len();
            if s.burst > 400 && s.activity != Activity::Busy { s.activity = Activity::Busy; s.busy_since = t; }
        }
        for o in outs {
            match o {
                TermOut::Reply(b) => self.be.write(pid, &b),
                TermOut::Title(title) => {
                    let title = title.trim_start_matches(|c: char| c.is_whitespace() || "✳⠂⠐⠈⠁⠄⠠⡀⢀·*".contains(c)).trim().to_string();
                    // PowerShell / cmd under ConPTY title themselves with their executable's path; that's noise next to the name.
                    if let Some(s) = self.sessions.get_mut(&id) { s.title = if crate::util::is_exe_title(&title) { String::new() } else { title }; }
                }
                TermOut::Notify(body) => self.signal(id, Some(body)),
                TermOut::Bell => self.on_bell(id),
                TermOut::Clipboard(text) => self.on_osc52(id, text),
            }
        }
    }

    /// A bell from a remote agent session: ask what the hook recorded (needs permission? finished with what?) so the
    /// notification and the pane header say why it stopped, not just that it did.
    fn on_bell(&mut self, id: u64) {
        let Some(s) = self.sessions.get(&id) else { return };
        let (Some(pid), true, true) = (s.pid, s.tmux_name.is_some(), is_agent(&s.preset_id)) else { self.signal(id, None); return };
        let be = self.be.clone();
        self.be.run(async move { be.agent_status(pid).await }, move |app, st| {
            match st.filter(|st| now_ms() - st.ts < 60_000) {
                Some(st) => {
                    let why = if st.event == "Notification" { if st.message.is_empty() { "needs your input".into() } else { st.message.clone() } } else if st.summary.is_empty() { "Finished".into() } else { format!("Finished: {}", st.summary) };
                    if let Some(s) = app.sessions.get_mut(&id) { s.agent = Some(st); }
                    app.signal(id, Some(why));
                }
                None => app.signal(id, None),
            }
        });
    }

    /// OSC 52 clipboard writes: full-screen apps (Claude Code) copy their own mouse selections this way, through
    /// tmux. Accepted only right after you clicked or typed in this pane: otherwise any output (a file being cat'ed,
    /// a web page an agent fetched, a hostile remote) could silently swap your clipboard for a command you'd then paste.
    fn on_osc52(&mut self, id: u64, text: String) {
        let Some(s) = self.sessions.get(&id) else { return };
        let (t, name) = (now_ms(), s.name.clone());
        if !self.is_visible(id) || !self.win_focused || (t - s.last_pointer > 3000 && t - s.last_input > 3000) {
            self.toast(format!("Blocked a clipboard write from {name} (it didn't follow a click or keypress in that pane)"), WARN, 4000);
        } else if text.len() > 4 * 1024 * 1024 { self.toast_warn(format!("Ignored an oversized clipboard write from {name}")); }
        else if !text.is_empty() { self.toast(format!("Copied {} chars from {name}", text.chars().count()), OK, 1400); self.ctx.copy_text(text); }
    }

    pub(super) fn on_exit(&mut self, pid: u64, code: Option<i32>, disconnected: bool) {
        let Some(id) = self.by_pid.remove(&pid) else { return };
        let Some(s) = self.sessions.get_mut(&id) else { return };
        s.pid = None;
        s.activity = Activity::Idle;
        let (mid, has_tmux) = (s.machine_id.clone(), s.tmux_name.is_some());
        if disconnected {
            s.status = SessStatus::Disconnected;
            if has_tmux { self.schedule_reconnect(id); } else { s.term.write_str("\r\n\x1b[38;2;224;175;104m⚡ Connection lost. Press Enter to reconnect.\x1b[0m\r\n"); }
        } else {
            s.status = SessStatus::Exited;
            s.term.write_str(&format!("\r\n\x1b[2m[exited{}. Enter to restart, {} to close]\x1b[0m\r\n", code.map(|c| format!(" ({c})")).unwrap_or_default(), kbd("Ctrl+Shift+W")));
        }
        if self.machine(&mid).is_ssh() { self.refresh_machine(&mid); }
    }

    /// Keystrokes and pasted text for a session.
    pub fn input(&mut self, id: u64, data: &[u8]) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        s.last_input = now_ms();
        if data == b"\r" { s.prompt_line = Some(s.term.cursor_abs_line()); }
        if s.read_only { return; } // watching someone else's session: tmux ignores input anyway
        s.term.scroll_to_bottom();
        match (s.status, s.pid) {
            (SessStatus::Running, Some(pid)) => self.be.write(pid, data),
            (SessStatus::Starting, _) => s.pending_input.extend_from_slice(data),
            (SessStatus::Exited | SessStatus::Disconnected, _) if data == b"\r" => self.start(id),
            _ => {}
        }
    }
    pub fn paste_text(&mut self, id: u64, text: &str) {
        let Some(s) = self.sessions.get(&id) else { return };
        // MSYS bash behind ConPTY mangles bracketed-paste markers into literal "[200~".
        let bytes = s.term.paste_bytes(text, s.preset_id == "gitbash");
        self.input(id, &bytes);
    }
    /// Quote a path for the session's shell / agent prompt.
    fn quote_path(&self, id: u64, p: &str) -> String {
        if !p.chars().any(|c| c.is_whitespace() || "'\"&()".contains(c)) { return p.to_string(); }
        let local_win = cfg!(windows) && self.sessions.get(&id).is_some_and(|s| self.machine(&s.machine_id).is_local());
        if local_win { format!("\"{p}\"") } else { crate::remote::shq(p) }
    }
    /// Drop files onto a pane: local paths are pasted as-is, remote ones are uploaded first.
    pub fn attach_files(&mut self, id: u64, paths: Vec<std::path::PathBuf>) {
        let Some(s) = self.sessions.get(&id) else { return };
        let (Some(pid), SessStatus::Running) = (s.pid, s.status) else { self.toast_warn("Session is not running"); return };
        let m = self.machine(&s.machine_id);
        if m.is_ssh() { self.toast(format!("Uploading {} file{} to {}…", paths.len(), if paths.len() > 1 { "s" } else { "" }, m.name), TEXT2, 1800); }
        let be = self.be.clone();
        self.be.run(async move { be.upload_files(pid, paths).await }, move |app, r| match r {
            Err(e) => app.toast_bad(e),
            Ok(list) => {
                let text = list.iter().map(|p| app.quote_path(id, p)).collect::<Vec<_>>().join(" ") + " ";
                app.paste_text(id, &text);
                if m.is_ssh() { app.toast("Uploaded to ~/.nexus/uploads", OK, 1800); }
            }
        });
    }
    /// An image on the clipboard (e.g. Win+Shift+S) is saved as a PNG on the session's machine (uploaded for
    /// remotes) and its path is pasted, which Claude and Antigravity read as an image.
    pub fn paste_image(&mut self, id: u64, img: &egui::ColorImage) {
        let Some(s) = self.sessions.get(&id) else { return };
        let (Some(pid), SessStatus::Running) = (s.pid, s.status) else { return };
        let mut png = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut png);
        if image::ImageEncoder::write_image(enc, img.as_raw(), img.width() as u32, img.height() as u32, image::ExtendedColorType::Rgba8).is_err() { return; }
        let (be, ssh, mname) = (self.be.clone(), self.machine(&s.machine_id).is_ssh(), self.machine(&s.machine_id).name);
        self.be.run(async move { be.paste_image(pid, png).await }, move |app, r| match r {
            Err(e) => app.toast_bad(e),
            Ok(path) => {
                let text = app.quote_path(id, &path) + " ";
                app.paste_text(id, &text);
                app.toast(if ssh { format!("Image uploaded to {mname}") } else { "Image saved and attached".into() }, OK, 1800);
            }
        });
    }
    pub fn copy_selection(&mut self, id: u64) -> bool {
        let Some(s) = self.sessions.get_mut(&id) else { return false };
        // "Select all" includes the empty rows below the prompt: drop trailing blank lines and line-end padding.
        let Some(text) = s.term.selection_text().map(|t| t.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_end_matches('\n').to_string()).filter(|t| !t.is_empty()) else { return false };
        s.term.clear_selection();
        self.toast(format!("Copied {} chars", text.chars().count()), OK, 1200);
        self.ctx.copy_text(text);
        true
    }

    // ---------------------------------------------------------------- panes / layout
    fn touch_mru(&mut self, id: u64) { self.mru.retain(|x| *x != id); self.mru.insert(0, id); }
    pub fn show(&mut self, id: u64) {
        if !self.sessions.contains_key(&id) { return; }
        let i = self.panes.iter().position(|p| *p == Some(id)).unwrap_or_else(|| { let i = self.focus.min(self.panes.len() - 1); self.panes[i] = Some(id); i });
        self.focus = i;
        self.touch_mru(id);
        self.clear_done(id);
        self.focus_terminal();
        self.persist_soon();
    }
    pub fn focus_pane(&mut self, i: usize) {
        if i >= self.panes.len() { return; }
        self.focus = i;
        if let Some(id) = self.panes[i] { self.touch_mru(id); self.clear_done(id); }
    }
    /// Give the keyboard back to the terminal (drop focus from any text box).
    pub fn focus_terminal(&mut self) { self.ctx.memory_mut(|m| { if let Some(f) = m.focused() { m.surrender_focus(f); } }); }
    pub fn set_layout(&mut self, n: u8) {
        let n = if [1, 2, 4].contains(&n) { n } else { 1 };
        let mut list: Vec<u64> = Vec::new();
        for id in self.focused().into_iter().chain(self.panes.iter().flatten().copied()).chain(self.mru.iter().copied()) {
            if self.sessions.contains_key(&id) && !list.contains(&id) { list.push(id); }
        }
        self.layout = n;
        self.panes = (0..n as usize).map(|i| list.get(i).copied()).collect();
        self.focus = 0;
        self.persist_soon();
    }
    pub fn cycle_layout(&mut self) { self.set_layout(match self.layout { 1 => 2, 2 => 4, _ => 1 }); }
    pub fn split_with(&mut self, id: u64) {
        if self.layout == 1 { self.set_layout(2); }
        if !self.panes.contains(&Some(id)) {
            let target = self.panes.iter().enumerate().position(|(j, p)| j != self.focus && p.is_none()).unwrap_or((self.focus + 1) % self.panes.len());
            self.panes[target] = Some(id);
        }
        self.show(id);
    }
    pub fn move_to_next_pane(&mut self, id: u64) {
        let n = self.panes.len();
        let from = self.panes.iter().position(|p| *p == Some(id));
        let to = (from.unwrap_or(0).max(self.focus) + 1) % n;
        if let Some(f) = from { self.panes[f] = if self.panes[to] == Some(id) { None } else { self.panes[to] }; }
        self.panes[to] = Some(id);
        self.focus = to;
        self.persist_soon();
    }
    pub fn move_pane_focus(&mut self, key: egui::Key) {
        if self.layout == 1 { return; }
        let mut i = self.focus;
        let four = self.layout == 4;
        match key {
            egui::Key::ArrowLeft => i = if four { i - i % 2 } else { 0 },
            egui::Key::ArrowRight => i = if four { i - i % 2 + 1 } else { 1 },
            egui::Key::ArrowUp if four && i >= 2 => i -= 2,
            egui::Key::ArrowDown if four && i < 2 => i += 2,
            _ => {}
        }
        self.focus_pane(i);
    }
    pub fn cycle(&mut self, dir: i32) {
        let list = self.ordered();
        if list.is_empty() { return; }
        let i = self.focused().and_then(|f| list.iter().position(|x| *x == f)).unwrap_or(0) as i32;
        self.show(list[((i + dir).rem_euclid(list.len() as i32)) as usize]);
    }
    /// Keep the pane list consistent with the sessions that exist: no stale or duplicate ids, and never every pane
    /// empty while sessions exist.
    pub(super) fn normalize_panes(&mut self) {
        let n = self.layout as usize;
        self.panes.resize(n, None);
        for i in 0..n {
            let ok = self.panes[i].is_some_and(|id| self.sessions.contains_key(&id) && self.panes.iter().position(|p| *p == Some(id)) == Some(i));
            if !ok { self.panes[i] = None; }
        }
        if self.focus >= n { self.focus = 0; }
        for i in 0..n {
            if self.panes[i].is_none() { self.panes[i] = self.mru.iter().chain(self.order.iter()).copied().find(|x| self.sessions.contains_key(x) && !self.panes.contains(&Some(*x))); }
        }
    }
    pub fn bump_font(&mut self, d: i32) {
        let a = &mut self.cfg.appearance;
        a.font_size = if d == 0 { 14.0 } else { (a.font_size + d as f32).clamp(9.0, 28.0) };
        self.apply_font();
        self.save_soon();
    }

    // ---------------------------------------------------------------- session persistence + workspaces
    pub fn snapshot(&self) -> Snapshot {
        let list: Vec<&Session> = self.ordered().into_iter().filter_map(|id| self.sessions.get(&id)).filter(|s| !s.spec.raw && s.spec.script.is_none()).collect();
        Snapshot {
            name: None, layout: self.layout, split: Split { x: self.split.0, y: self.split.1 }, focus: self.focus,
            sessions: list.iter().map(|s| SnapSession { machine_id: s.machine_id.clone(), preset_id: s.preset_id.clone(), cwd: s.cwd.clone(), tmux_name: s.tmux_name.clone(), name: Some(s.name.clone()) }).collect(),
            panes: self.panes.iter().map(|p| p.and_then(|id| list.iter().position(|s| s.id == id)).map_or(-1, |i| i as i64)).collect(),
        }
    }
    /// Open a snapshot/workspace. resume=true reattaches remote tmux sessions; otherwise starts fresh ones.
    pub fn open_layout(&mut self, snap: &Snapshot, resume: bool) {
        if snap.sessions.is_empty() { return; }
        self.restoring = true;
        let mut created: Vec<Option<u64>> = Vec::new();
        for it in &snap.sessions {
            let Some(m) = self.cfg.machine(&it.machine_id).cloned().filter(|m| !(m.is_ssh() && !m.has_address())) else { created.push(None); continue };
            let attach = if resume && m.is_ssh() { it.tmux_name.clone() } else { None };
            created.push(Some(self.make_session(&it.machine_id, &it.preset_id, Extra { cwd: it.cwd.clone(), name: it.name.clone(), attach, ..Default::default() }, false)));
        }
        self.layout = if [1, 2, 4].contains(&snap.layout) { snap.layout } else { 1 };
        self.split = (snap.split.x.clamp(0.15, 0.85), snap.split.y.clamp(0.15, 0.85));
        self.panes = (0..self.layout as usize).map(|i| snap.panes.get(i).and_then(|&p| usize::try_from(p).ok()).and_then(|p| created.get(p).copied().flatten())).collect();
        self.focus = snap.focus.min(self.layout as usize - 1);
        for id in created.into_iter().flatten() { self.start(id); }
        self.restoring = false;
        if let Some(f) = self.focused() { self.touch_mru(f); }
        self.persist_soon();
    }
    pub fn save_workspace(&mut self) {
        self.prompt("Save workspace", "", "Name", "e.g. API sprint", "", false, |app, name| {
            let Some(name) = name else { return };
            let mut snap = app.snapshot();
            if snap.sessions.is_empty() { app.toast_warn("Open some sessions first"); return; }
            for s in &mut snap.sessions { s.tmux_name = None; }
            snap.name = Some(name.clone());
            app.cfg.workspaces.retain(|w| w.name.as_deref() != Some(&name));
            app.cfg.workspaces.push(snap);
            app.save_now();
            app.toast_ok(format!("Saved workspace \"{name}\""));
        });
    }
    pub fn delete_workspace(&mut self, name: &str) {
        self.cfg.workspaces.retain(|w| w.name.as_deref() != Some(name));
        self.save_now();
        self.toast(format!("Deleted workspace \"{name}\""), TEXT2, 3800);
    }

    // ---- export / import (hand your layouts and launch commands to a teammate) ----
    // The file carries workspaces, presets and a machine index (name/host) so sessions can be re-pointed at the
    // importer's own machines. No addresses are required to match: unmatched sessions are skipped, not guessed.
    pub fn export_workspaces(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Export workspaces and presets").set_file_name("nexus-workspaces.json").add_filter("JSON", &["json"]).save_file() else { return };
        let c = &self.cfg;
        let data = serde_json::json!({
            "nexus": 1, "exportedAt": chrono::Utc::now().to_rfc3339(), "presets": c.presets, "workspaces": c.workspaces,
            "machines": c.machines.iter().map(|m| serde_json::json!({ "id": m.id, "name": m.name, "type": m.kind, "host": m.host, "tsHost": m.ts_host, "tgHost": m.tg_host, "hostname": self.mst(&m.id).hostname })).collect::<Vec<_>>(),
        });
        match std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap_or_default()) {
            Ok(()) => self.toast_ok(format!("Exported {} workspace{} and {} presets", c.workspaces.len(), if c.workspaces.len() == 1 { "" } else { "s" }, c.presets.len())),
            Err(e) => self.toast_bad(e.to_string()),
        }
    }
    pub fn import_workspaces(&mut self) {
        let Some(path) = rfd::FileDialog::new().set_title("Import workspaces and presets").add_filter("JSON", &["json"]).pick_file() else { return };
        let data = match std::fs::metadata(&path).map(|m| m.len()) {
            Ok(n) if n > 4 * 1024 * 1024 => { self.toast_bad("That file is too large to be a Nexus export"); return; }
            _ => std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).filter(|d| d["nexus"] == 1),
        };
        let Some(data) = data else { self.toast_bad("Not a Nexus export file"); return };
        // Map a machine from the export onto one of ours: same id, else same address, else same name.
        let s = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
        let mut map: HashMap<String, String> = HashMap::new();
        for x in data["machines"].as_array().into_iter().flatten() {
            let (id, kind, host, ts, tg, name) = (s(x, "id"), s(x, "type"), s(x, "host"), s(x, "tsHost"), s(x, "tgHost"), s(x, "name"));
            let ms = &self.cfg.machines;
            let hit = ms.iter().find(|m| m.id == id && (m.kind == kind || kind.is_empty()))
                .or_else(|| ms.iter().find(|m| !host.is_empty() && (m.host == host || m.ts_host == host || (!ts.is_empty() && m.ts_host == ts))).or_else(|| ms.iter().find(|m| !tg.is_empty() && m.tg_host == tg)))
                .or_else(|| ms.iter().find(|m| !name.is_empty() && m.name == name))
                .or_else(|| ms.iter().find(|m| kind == "local" && m.is_local()));
            if let Some(m) = hit { map.insert(id, m.id.clone()); }
        }
        let presets = self.fresh_presets(data["presets"].as_array().cloned().unwrap_or_default());
        let mut ws: Vec<Snapshot> = Vec::new();
        for w in data["workspaces"].as_array().into_iter().flatten() {
            let Ok(mut snap) = serde_json::from_value::<Snapshot>(w.clone()) else { continue };
            let Some(name) = snap.name.clone().filter(|n| !n.is_empty()) else { continue };
            let mapped: Vec<Option<SnapSession>> = snap.sessions.iter().map(|x| map.get(&x.machine_id).map(|id| SnapSession { machine_id: id.clone(), ..x.clone() })).collect();
            let kept_index = |i: i64| -> i64 { usize::try_from(i).ok().filter(|&i| mapped.get(i).is_some_and(Option::is_some)).map_or(-1, |i| mapped[..i].iter().flatten().count() as i64) };
            snap.panes = snap.panes.iter().map(|&i| kept_index(i)).collect();
            snap.sessions = mapped.iter().flatten().cloned().collect();
            if snap.sessions.is_empty() { continue; }
            snap.name = Some(clip(&name, 60));
            ws.push(snap);
        }
        if ws.is_empty() && presets.is_empty() { self.toast_warn("Nothing in that file matches your machines, and it has no new presets"); return; }
        // Presets are commands that will run on your machines: show them before adding.
        let names = ws.iter().filter_map(|w| w.name.clone()).collect::<Vec<_>>().join(", ");
        let desc = format!("{} workspace{} ({}){}", ws.len(), if ws.len() == 1 { "" } else { "s" }, if names.is_empty() { "none" } else { &names },
            if presets.is_empty() { ". No new presets.".to_string() } else { format!(", and {} new preset{}: {}. Presets are commands that run on your machines: only import ones you understand.", presets.len(), if presets.len() == 1 { "" } else { "s" }, describe_presets(&presets)) });
        self.choose("Import?", &desc, &[("Cancel", false), ("Import", true)], move |app, v| {
            if v != Some(1) { return; }
            app.cfg.workspaces.retain(|w| !ws.iter().any(|x| x.name == w.name));
            app.cfg.workspaces.extend(ws.iter().cloned());
            app.cfg.presets.extend(presets.iter().cloned());
            app.save_now();
            app.toast_ok(format!("Imported {} workspace{}{}", ws.len(), if ws.len() == 1 { "" } else { "s" }, if presets.is_empty() { String::new() } else { format!(" and {} presets", presets.len()) }));
        });
    }
    /// Presets from a file or the team hub, reduced to safe fields, minus the ones we already have.
    pub(super) fn fresh_presets(&self, list: Vec<serde_json::Value>) -> Vec<Preset> {
        list.iter().filter_map(|p| {
            let id = p["id"].as_str().filter(|id| (1..=24).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))?;
            if self.cfg.presets.iter().any(|q| q.id == id) { return None; }
            let color = p["color"].as_str().filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())).unwrap_or("#9aa3ba");
            Some(Preset {
                id: id.into(), name: clip(p["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(id), 40), color: color.into(), icon: clip(p["icon"].as_str().unwrap_or_default(), 24),
                cmd: p["cmd"].as_object().map(|o| o.iter().filter(|(k, _)| ["windows", "linux", "mac"].contains(&k.as_str())).map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), String::from))).collect()).unwrap_or_default(),
                only: p["only"].as_str().map(String::from),
            })
        }).collect()
    }

    // ---------------------------------------------------------------- machines
    pub fn connect_machine(&mut self, id: &str, quiet: bool) {
        self.manual_disconnect.remove(id);
        if self.machine(id).is_wsl() { self.refresh_machine(id); return; }
        if !quiet { let st = self.ms(id); st.status = Some(Status::Connecting); st.error = None; }
        let (be, mid) = (self.be.clone(), id.to_string());
        let mid2 = mid.clone();
        self.be.run(async move { be.machine(&mid).connect().await }, move |app, r| match r {
            Err(e) => { let st = app.ms(&mid2); st.status = Some(Status::Error); st.error = Some(e); }
            Ok(()) => app.refresh_machine(&mid2),
        });
    }
    pub fn refresh_machine(&mut self, id: &str) {
        let m = self.machine(id);
        if !(m.is_wsl() || (m.is_ssh() && m.has_address())) || self.cfg.machine(id).is_none() { return; }
        let (be, mid) = (self.be.clone(), id.to_string());
        let mid2 = mid.clone();
        self.be.run(async move { be.probe(&mid).await }, move |app, r| {
            let id = mid2;
            let p = match r { Ok(p) => p, Err(e) => { let st = app.ms(&id); st.status = Some(Status::Error); st.error = Some(e); return; } };
            let st = app.ms(&id);
            st.status = Some(Status::Online);
            st.error = None;
            st.tools = Some(p.tools);
            st.hostname = p.hostname;
            st.home = p.home;
            st.user = p.user;
            let mut det: Vec<TmuxSession> = p.sessions.into_iter().filter(|x| x.name.starts_with("nx-")).collect();
            det.sort_by_key(|x| std::cmp::Reverse(x.activity));
            st.detached = det.clone();
            app.refresh_agents(&id);
            app.ensure_hub();
            if app.auto_reattached.insert(id.clone()) && app.cfg.appearance.restore_session {
                let open: HashSet<String> = app.sessions.values().filter_map(|s| s.tmux_name.clone()).collect();
                let resumable: Vec<String> = det.iter().filter(|x| x.attached == 0 && !open.contains(&x.name)).take(8).map(|x| x.name.clone()).collect();
                let had_focus = app.focused().is_some();
                for (i, name) in resumable.iter().enumerate() { app.reattach(&id, name, i == 0 && !had_focus, true, false); }
                if !resumable.is_empty() { app.toast_ok(format!("Resumed {} running session{} on {}", resumable.len(), if resumable.len() > 1 { "s" } else { "" }, app.machine(&id).name)); }
            }
        });
    }
    pub(super) fn on_machine_status(&mut self, id: &str, status: Status, error: Option<String>, route: Option<&'static str>) {
        let st = self.ms(id);
        let was = st.status;
        st.status = Some(status);
        st.error = error;
        if route.is_some() { st.route = route; }
        // A machine that was online and dropped (Wi-Fi, reboot) is retried in the background.
        if was == Some(Status::Online) && matches!(status, Status::Offline | Status::Error) { self.schedule_machine_retry(id); }
    }
    /// What the agents on a machine last reported (also for sessions that aren't open here): the fleet view's data.
    pub fn refresh_agents(&mut self, id: &str) {
        let (be, mid) = (self.be.clone(), id.to_string());
        let mid2 = mid.clone();
        self.be.run(async move { be.agent_statuses(&mid).await }, move |app, map| { app.ms(&mid2).agents = map; });
    }
    /// Launch an agent in its own git worktree: pick the repository folder, name the branch, start there.
    pub fn launch_in_worktree(&mut self, machine_id: &str, preset_id: &str, cwd: &str) {
        let (mid, pid, cwd) = (machine_id.to_string(), preset_id.to_string(), cwd.to_string());
        let t = crate::remote::radix36(now_ms() as u64);
        let suggestion = format!("nexus/{preset_id}-{}", &t[t.len().saturating_sub(4)..]);
        self.prompt("New git worktree", &format!("A separate working folder and branch for this agent, beside the repository ({}-worktrees/<branch>), so agents working on the same project can't overwrite each other's files. An existing branch is checked out instead of created.", crate::util::base_name(&cwd)),
            "Branch", "feature/login", &suggestion, false, move |app, branch| {
                let Some(branch) = branch.map(|b| b.trim().to_string()) else { return };
                if !crate::remote::valid_branch(&branch) { app.toast_warn("Branch names use letters, digits, . _ - and /"); return; }
                let (be, mid2, cwd2, b2) = (app.be.clone(), mid.clone(), cwd.clone(), branch.clone());
                let (mid3, pid3) = (mid.clone(), pid.clone());
                app.be.run(async move { be.add_worktree(&mid2, cwd2, b2).await }, move |app, r| match r {
                    Err(e) => app.toast_bad(format!("Couldn't create the worktree: {e}")),
                    Ok(path) => {
                        app.toast_ok(format!("Worktree {branch} is at {path}"));
                        app.launch(&mid3, &pid3, Extra { cwd: Some(path), name: Some(format!("{} · {branch}", app.preset(&pid3).name)), ..Default::default() });
                    }
                });
            });
    }

    pub fn disconnect_machine(&mut self, id: &str) { self.manual_disconnect.insert(id.to_string()); self.be.disconnect(id); }

    pub(super) fn periodic_refresh(&mut self) {
        for m in self.cfg.machines.clone() { if m.is_ssh() && self.status_of(&m) == Status::Online { self.refresh_machine(&m.id); } }
        // Track where remote sessions actually are (tmux pane path), so restores and workspaces reopen there.
        let list: Vec<(u64, u64)> = self.sessions.values().filter(|s| s.tmux_name.is_some()).filter_map(|s| Some((s.id, s.pid?))).collect();
        let be = self.be.clone();
        self.be.run(async move {
            let mut out = Vec::new();
            for (id, pid) in list { if let Some(cwd) = be.session_cwd(pid).await { out.push((id, cwd)); } }
            out
        }, |app, list| {
            let mut changed = false;
            for (id, cwd) in list {
                let Some(mid) = app.sessions.get(&id).map(|s| s.machine_id.clone()) else { continue };
                let home = app.mst(&mid).home;
                let tilde = match cwd.strip_prefix(&home) { Some(rest) if !home.is_empty() => format!("~{rest}"), _ => cwd };
                if let Some(s) = app.sessions.get_mut(&id) { if s.cwd.as_deref() != Some(&tilde) { s.cwd = Some(tilde); changed = true; } }
            }
            if changed { app.persist_soon(); }
        });
    }

    // ---- machine actions ----
    pub fn setup_key(&mut self, machine_id: &str) {
        let m = self.machine(machine_id);
        if m.host.is_empty() { self.open_settings(); return; }
        self.launch("local", "shell", Extra { script: Some("setup-ssh-key".into()), script_machine: Some(machine_id.into()), name: Some(format!("SSH key → {}", m.name)), ..Default::default() });
    }
    pub fn enable_alerts(&mut self, machine_id: &str) {
        let (be, mid, name) = (self.be.clone(), machine_id.to_string(), self.machine(machine_id).name);
        self.be.run(async move { be.machine(&mid).setup_alerts().await }, move |app, r| match r {
            Err(e) => app.toast_bad(format!("Couldn't enable alerts on {name}: {e}")),
            Ok(0) => app.toast(format!("Claude alerts were already enabled on {name}"), OK, 6000),
            Ok(_) => app.toast(format!("Claude on {name} will now ring Nexus when it finishes or needs you (restart running Claude sessions)"), OK, 6000),
        });
    }
    pub fn bootstrap(&mut self, machine_id: &str) {
        let (m, offline, mid) = (self.machine(machine_id), self.cfg.offline, machine_id.to_string());
        let desc = if offline { format!("Offline mode is on: the script only installs from ~/.nexus/offline/ on {0} (tmux via the package manager, claude/agy/openclaw from installers you copied there) and never reaches the internet.", m.name) }
            else { format!("This runs the vendors' install scripts on {0} (curl … | bash from claude.ai, antigravity.google and openclaw.ai) and may ask for your sudo password. Only continue if {0} is allowed to reach the internet and you trust those sources.", m.name) };
        self.choose(&format!("Install tools on {}?", m.name), &desc, &[("Cancel", false), ("Install", true)], move |app, v| {
            if v != Some(1) { return; }
            let (be, mid2, name) = (app.be.clone(), mid.clone(), m.name.clone());
            let mid3 = mid.clone();
            app.be.run(async move { be.upload_bootstrap(&mid2).await }, move |app, r| match r {
                Err(e) => app.toast_bad(format!("Can't reach {name}: {e}")),
                Ok(()) => app.launch(&mid3, "shell", Extra { raw: true, cmd: Some(format!("{}bash ~/.nexus/bootstrap.sh", if offline { "NEXUS_OFFLINE=1 " } else { "" })), name: Some("Install tools".into()), ..Default::default() }),
            });
        });
    }
    /// Port forwarding (ssh -L over the machine's connection): a remote web UI as http://localhost:<port>.
    pub fn forward_port(&mut self, machine_id: &str) {
        let (m, mid) = (self.machine(machine_id), machine_id.to_string());
        self.prompt(&format!("Forward a port from {}", m.name),
            "The service becomes http://localhost:<port> on this computer, over the SSH connection. Only this computer can use it. The same local port is tried first; otherwise a free one is picked.",
            "Remote port (or host:port as seen from that machine)", "8888, or 127.0.0.1:18789, or gpu-box2:6006", "", false, move |app, spec| {
                let Some(spec) = spec else { return };
                let spec = spec.trim();
                let (host, port) = spec.rsplit_once(':').map_or(("127.0.0.1", spec), |(h, p)| (h, p));
                let Some(port) = port.parse::<u16>().ok().filter(|p| *p > 0 && host.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)) && !host.is_empty()) else { app.toast_warn("Enter a port like 8888 or host:port"); return };
                let (be, mid2, host, name) = (app.be.clone(), mid.clone(), host.to_string(), m.name.clone());
                app.be.run(async move {
                    match be.tunnel_open(&mid2, &host, port, port).await {
                        Err(e) if e.contains("already in use") => be.tunnel_open(&mid2, &host, port, 0).await,
                        r => r,
                    }
                }, move |app, r| match r {
                    Err(e) => app.toast_bad(format!("Couldn't forward: {e}")),
                    Ok(t) => {
                        app.tunnels = app.be.tunnels();
                        let url = format!("http://localhost:{}/", t.local_port);
                        app.toast_action(format!("localhost:{} → {name}:{}", t.local_port, t.remote_port), 15_000, "Open in browser", act(move |_| { crate::platform::open_external(&url); }));
                    }
                });
            });
    }

    /// An MCP client (Claude Code on this PC) asked Nexus to start a session.
    pub(super) fn mcp_launch(&mut self, req: u64, machine_id: &str, preset_id: &str, cwd: Option<String>) {
        let (mid, pid) = (machine_id.to_string(), preset_id.to_string());
        self.launch_stage(machine_id, preset_id, Extra { cwd, ..Default::default() }, 0, Some(Box::new(move |app, id| {
            let Some(id) = id else { app.be.mcp_launched(req, Err("Launch was cancelled in Nexus".into())); return };
            // Reply once it is running (or has failed), polling its status for up to 20 s.
            fn wait(app: &mut App, req: u64, id: u64, mid: String, pid: String, tries: u32) {
                match app.sessions.get(&id) {
                    Some(s) if s.status == SessStatus::Starting && tries < 100 => app.be.run(async { tokio::time::sleep(std::time::Duration::from_millis(200)).await }, move |app, ()| wait(app, req, id, mid, pid, tries + 1)),
                    Some(s) if s.pid.is_some() => {
                        let v = serde_json::json!({ "id": format!("s{}", s.pid.unwrap_or(0)), "name": s.name, "machineId": mid, "preset": pid, "cwd": s.cwd });
                        app.toast(format!("An MCP client started {} on {}", app.preset(&pid).name, app.machine(&mid).name), OK, 4000);
                        app.be.mcp_launched(req, Ok(v));
                    }
                    _ => app.be.mcp_launched(req, Err("The session did not start".into())),
                }
            }
            wait(app, req, id, mid, pid, 0);
        })));
    }
}

pub fn describe_presets(list: &[Preset]) -> String {
    list.iter().map(|p| { let c: Vec<&str> = p.cmd.values().map(String::as_str).filter(|c| !c.is_empty()).collect(); format!("{} = {}", p.name, if c.is_empty() { "(shell)".into() } else { c.join(" | ") }) }).collect::<Vec<_>>().join("; ")
}
