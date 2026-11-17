//! Team chat + presence. Chat and presence live on one "hub" SSH machine (~/.nexus/collab). Everyone's Nexus
//! reads/writes them over its own SSH connection; presence (who has which session open) drives the "someone's already
//! in there" warnings, machine claims and the launch queue.
use super::sessions::is_agent;
use super::*;
use crate::collab::{Claim, From, Message, Presence};
use crate::util::{base_name, claims_on, fmt_time, mentions, parse_claim, short_path, ClaimHit};

#[derive(Default)]
pub struct Team {
    pub hub_id: Option<String>,
    pub live: bool,
    messages: Vec<Message>,
    seen: HashSet<String>,
    pub presence: Vec<Presence>,
    pub unread: usize,
    pub open: bool,
    pub status: String,
    starting: bool,
    /// My own claims (carried in my presence).
    pub claims: Vec<Claim>,
    input: String,
    focus_input: bool,
    pub ts_user: Option<crate::tailscale::TsUser>,
}

/// % utilisation below which a GPU counts as free, for two samples in a row.
const GPU_IDLE: f64 = 20.0;

fn hue_color(name: &str) -> Color32 {
    let h = name.chars().fold(0u32, |h, c| (h * 31 + c as u32) % 360);
    egui::ecolor::Hsva::new(h as f32 / 360.0, 0.55, 0.85, 1.0).into()
}
pub fn avatar(ui: &mut egui::Ui, name: &str, size: f32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), size / 2.0, hue_color(name));
    let initial = name.trim().chars().next().map_or("?".to_string(), |c| c.to_uppercase().to_string());
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, initial, FontId::proportional(size * 0.55), BG);
    resp.on_hover_text(name)
}

impl App {
    fn my_client(&self) -> &str { &self.cfg.collab.client }
    pub fn my_name(&self) -> String {
        if !self.cfg.collab.name.is_empty() { self.cfg.collab.name.clone() } else { self.team.ts_user.as_ref().map(|u| u.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Me".into()) }
    }
    fn me(&self) -> From { From { name: self.my_name(), login: self.team.ts_user.as_ref().map(|u| u.login.clone()).unwrap_or_default(), client: self.my_client().to_string() } }
    pub fn host_of(&self, machine_id: &str) -> String { self.mstate.get(machine_id).map(|s| s.hostname.clone()).unwrap_or_default() }
    fn user_of(&self, machine_id: &str) -> String {
        self.mstate.get(machine_id).map(|s| s.user.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| self.machine(machine_id).user)
    }

    pub(super) fn init_team(&mut self) {
        if self.cfg.collab.client.is_empty() {
            self.cfg.collab.client = (0..8).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
            self.save_now();
        }
        let be = self.be.clone();
        self.be.run(async move { be.ts_status().await }, |app, t| app.team.ts_user = t.local.user);
    }

    /// The hub: Settings override (by hostname), else the online SSH machine whose hostname sorts first, so everyone
    /// connected to the same machines lands on the same chat without configuring anything.
    fn pick_hub(&self) -> Option<String> {
        let mut online: Vec<(String, String)> = self.cfg.machines.iter().filter(|m| m.is_ssh() && self.status_of(m) == Status::Online).map(|m| (self.host_of(&m.id), m.id.clone())).filter(|(h, _)| !h.is_empty()).collect();
        let want = &self.cfg.collab.hub;
        if !want.is_empty() { return online.into_iter().find(|(h, _)| h == want).map(|x| x.1); }
        online.sort();
        online.into_iter().next().map(|x| x.1)
    }
    pub fn ensure_hub(&mut self) {
        let Some(id) = self.pick_hub() else { return };
        if self.team.starting || (self.team.hub_id.as_deref() == Some(&id) && self.team.live) { return; }
        self.team.starting = true;
        let (be, mid) = (self.be.clone(), id.clone());
        self.be.run(async move { be.start_hub(&mid).await }, move |app, r| {
            app.team.starting = false;
            if r.is_ok() { app.team.hub_id = Some(id); app.team.live = true; app.heartbeat(); }
        });
    }
    /// Re-pick the hub (after Settings changed).
    pub fn reset_hub(&mut self) { self.team.live = false; self.ensure_hub(); }

    pub(super) fn heartbeat(&mut self) {
        if !self.team.live { self.ensure_hub(); return; }
        let t = now_ms();
        self.team.claims.retain(|c| c.until > t);
        let sessions: Vec<serde_json::Value> = self.sessions.values().filter(|s| s.tmux_name.is_some() && s.status == SessStatus::Running && self.machine(&s.machine_id).is_ssh())
            .map(|s| serde_json::json!({ "host": self.host_of(&s.machine_id), "user": self.user_of(&s.machine_id), "tmux": s.tmux_name, "preset": s.preset_id, "name": s.name, "cwd": s.cwd.clone().unwrap_or_default() })).collect();
        let me = self.me();
        let presence = serde_json::json!({ "client": me.client, "name": me.name, "login": me.login, "status": self.team.status, "sessions": sessions, "claims": self.team.claims });
        let be = self.be.clone();
        self.be.run(async move { be.collab_heartbeat(presence).await }, |app, r| { if let Ok(p) = r { app.team.presence = p; } });
    }

    pub(super) fn on_collab_message(&mut self, m: Message) {
        if !self.team.seen.insert(m.id.clone()) { return; }
        let mine = m.from.client == self.my_client();
        let fresh = now_ms() - m.ts < 60_000;
        if !mine && fresh && m.kind != "activity" {
            if !self.team.open { self.team.unread += 1; }
            if mentions(&m.text, &self.my_name()) { self.notify(&format!("{} mentioned you", m.from.name), &crate::util::clip(&m.text, 200), None); self.flash(); }
        }
        self.team.messages.push(m);
        let n = self.team.messages.len();
        if n > 500 { self.team.messages.drain(..n - 500); }
    }

    pub fn post_team(&mut self, text: String, kind: &'static str, then: impl FnOnce(&mut App) + Send + 'static) {
        let (be, me) = (self.be.clone(), self.me());
        self.be.run(async move { be.collab_post(me, text, kind).await }, move |app, r| match r { Ok(()) => then(app), Err(e) => app.toast_bad(e) });
    }
    /// Tell the team when an agent starts on a shared machine (so nobody opens a second one in the same folder).
    pub(super) fn announce_launch(&mut self, id: u64) {
        let Some(s) = self.sessions.get(&id) else { return };
        if !self.cfg.collab.announce || !self.team.live || !is_agent(&s.preset_id) || !self.machine(&s.machine_id).is_ssh() { return; }
        let text = format!("started {} in {} on {}", self.preset(&s.preset_id).name, s.cwd.as_deref().unwrap_or("~"), self.host_of(&s.machine_id));
        self.post_team(text, "activity", |_| {});
    }

    // ---- conflict checks (each returns true when it took over: a dialog is open, or the launch must not go ahead) ----
    pub fn who_in(&self, machine_id: &str, tmux: &str) -> Vec<String> {
        let (host, user) = (self.host_of(machine_id), self.user_of(machine_id));
        crate::collab::conflicts(&self.team.presence, self.my_client(), &host, &user, Some(tmux), None).same_session.into_iter().map(|x| x.0).collect()
    }
    /// Before attaching to a session someone else is in.
    pub(super) fn check_session_conflict(&mut self, machine_id: &str, tmux: &str) -> bool {
        let people = self.who_in(machine_id, tmux);
        if people.is_empty() { return false; }
        let (mid, name) = (machine_id.to_string(), tmux.to_string());
        let who = people.join(", ");
        self.choose(&format!("{who} {} in this session", if people.len() > 1 { "are" } else { "is" }),
            &format!("{tmux} on {} is open in {}'s Nexus. If you both type, your keystrokes mix into the same prompt. Watch it read-only, or open it anyway (say so in team chat). Read-only stops your own keystrokes reaching it; it isn't a permission boundary between people sharing an account.", self.machine(machine_id).name, people.join(" and ")),
            &[("Cancel", false), ("Open anyway", false), ("Watch read-only", true)], move |app, v| match v {
                Some(1) => app.reattach(&mid, &name, true, true, false),
                Some(2) => app.reattach(&mid, &name, true, true, true),
                _ => {}
            });
        true
    }
    /// Before starting an agent where someone else's agent already works.
    pub(super) fn check_folder_conflict(&mut self, machine_id: &str, preset_id: &str, extra: Extra, interactive: bool) -> bool {
        if !is_agent(preset_id) || !self.machine(machine_id).is_ssh() { return false; }
        let cwd = extra.cwd.clone().unwrap_or_else(|| "~".into());
        let hits = crate::collab::conflicts(&self.team.presence, self.my_client(), &self.host_of(machine_id), &self.user_of(machine_id), None, Some(&cwd)).same_folder;
        if hits.is_empty() { return false; }
        if !interactive { return true; }
        let who = hits.iter().map(|(p, s)| format!("{p} ({})", self.preset(&s.preset).name)).collect::<Vec<_>>().join(", ");
        let first = hits[0].0.clone();
        let (mid, pid) = (machine_id.to_string(), preset_id.to_string());
        self.choose("Someone is already working there",
            &format!("{who} {} running an agent in {cwd} on {}. Two agents editing the same files can overwrite each other's changes.", if hits.len() > 1 { "are" } else { "is" }, self.machine(machine_id).name),
            &[("Cancel", false), (&format!("Message {first}"), false), ("Start anyway", true)], move |app, v| match v {
                Some(1) => app.open_team(Some(format!("@{} ", first.split_whitespace().next().unwrap_or_default()))),
                Some(2) => app.launch_stage(&mid, &pid, extra, 1, None),
                _ => {}
            });
        true
    }
    /// Someone claimed the machine (/claim): ask before starting an agent there, or queue it until the machine is free.
    pub(super) fn check_claim(&mut self, machine_id: &str, preset_id: &str, extra: Extra, interactive: bool) -> bool {
        if !is_agent(preset_id) || !self.machine(machine_id).is_ssh() { return false; }
        let Some(c) = self.claimed_by(machine_id).into_iter().next() else { return false };
        if !interactive { return true; }
        let (mid, pid, person) = (machine_id.to_string(), preset_id.to_string(), c.person.clone());
        self.choose(&format!("{} is claimed", self.machine(machine_id).name),
            &format!("{} claimed it until {}{}. Starting {} now competes for its GPU and memory.", c.person, fmt_time(c.until), if c.note.is_empty() { String::new() } else { format!(" ({})", c.note) }, self.preset(preset_id).name),
            &[("Cancel", false), (&format!("Message {}", c.person), false), ("Start when free", false), ("Start anyway", true)], move |app, v| match v {
                Some(1) => app.open_team(Some(format!("@{} ", person.split_whitespace().next().unwrap_or_default()))),
                Some(2) => app.enqueue(&mid, &pid, extra, format!("until {person}'s claim ends")),
                Some(3) => app.launch_stage(&mid, &pid, extra, 2, None),
                _ => {}
            });
        true
    }

    // ---- claims ----
    /// Claims on a machine by other people (not expired), for cards and launch checks.
    pub fn claimed_by(&self, machine_id: &str) -> Vec<ClaimHit> { claims_on(&self.team.presence, &self.host_of(machine_id), Some(self.my_client()), now_ms()) }
    pub fn my_claim_on(&self, machine_id: &str) -> Option<&Claim> { let h = self.host_of(machine_id); self.team.claims.iter().find(|c| c.host == h && c.until > now_ms()) }
    fn machine_by_host_or_name(&self, x: &str) -> Option<Machine> {
        self.cfg.machines.iter().find(|m| m.is_ssh() && (self.host_of(&m.id).eq_ignore_ascii_case(x) || m.name.eq_ignore_ascii_case(x))).cloned()
    }

    // ---- launch queue: start an agent once a machine is unclaimed and its GPU is idle ----
    fn enqueue(&mut self, machine_id: &str, preset_id: &str, extra: Extra, why: String) {
        self.uid += 1;
        self.toast(format!("Queued {} on {} {why}. Nexus starts it when the machine is free (see {} → Queue).", self.preset(preset_id).name, self.machine(machine_id).name, kbd("Ctrl+Shift+P")), OK, 6000);
        self.queue.push(Queued { id: self.uid, machine_id: machine_id.into(), preset_id: preset_id.into(), extra, why, idle_samples: 0 });
    }
    fn machine_free(&self, machine_id: &str) -> bool {
        if !self.claimed_by(machine_id).is_empty() { return false; }
        match self.mstate.get(machine_id).and_then(|s| s.stats.as_ref()).and_then(|s| s.gpu.as_ref()) {
            Some(g) if g.error.is_none() => g.util.is_none_or(|u| u < GPU_IDLE),
            _ => true,
        }
    }
    pub(super) fn run_queue(&mut self) {
        let mut ready = Vec::new();
        for i in 0..self.queue.len() {
            let mid = self.queue[i].machine_id.clone();
            if self.status_of(&self.machine(&mid)) != Status::Online { continue; }
            let free = self.machine_free(&mid);
            let q = &mut self.queue[i];
            q.idle_samples = if free { q.idle_samples + 1 } else { 0 };
            if q.idle_samples >= 2 { ready.push(q.id); }
        }
        for id in ready {
            let Some(i) = self.queue.iter().position(|q| q.id == id) else { continue };
            let q = self.queue.remove(i);
            let (m, p) = (self.machine(&q.machine_id), self.preset(&q.preset_id));
            self.notify(&format!("{} is free", m.name), &format!("Starting {}{}", p.name, q.extra.cwd.as_ref().map(|c| format!(" in {c}")).unwrap_or_default()), None);
            self.toast(format!("{} is free: starting {}", m.name, p.name), OK, 5000);
            self.launch(&q.machine_id, &q.preset_id, Extra { queued: true, ..q.extra });
        }
    }
    pub fn queue_items(&self) -> Vec<overlays::PalItem> {
        self.queue.iter().map(|q| {
            let id = q.id;
            overlays::PalItem {
                group: "Queue", label: format!("{} on {}{}", self.preset(&q.preset_id).name, self.machine(&q.machine_id).name, q.extra.cwd.as_ref().map(|c| format!(" · {}", base_name(c))).unwrap_or_default()),
                sub: format!("waiting {} · cancel", q.why), kbd: String::new(), icon: overlays::Icon::Preset(q.preset_id.clone(), Some(q.machine_id.clone())), color: Some(hex(&self.preset(&q.preset_id).color)), missing: false, exact: false,
                run: act(move |app| { app.queue.retain(|x| x.id != id); app.toast("Removed from the queue", TEXT2, 3800); }),
            }
        }).collect()
    }

    // ---- chat drawer ----
    pub fn open_team(&mut self, prefill: Option<String>) {
        self.team.open = true;
        self.team.unread = 0;
        if let Some(p) = prefill { self.team.input = p; }
        self.team.focus_input = true;
        self.ensure_hub();
    }
    pub fn close_team(&mut self) { self.team.open = false; self.focus_terminal(); }
    pub fn toggle_team(&mut self) { if self.team.open { self.close_team(); } else { self.open_team(None); } }

    fn send_team(&mut self) {
        let text = self.team.input.trim().to_string();
        if text.is_empty() { return; }
        if !self.team.live { self.toast_warn("Team chat is not connected yet: connect to a shared machine first"); return; }
        let done = |app: &mut App| { app.team.input.clear(); app.heartbeat(); };
        if let Some(rest) = text.strip_prefix("/status").filter(|r| r.is_empty() || r.starts_with(char::is_whitespace)) {
            self.team.status = rest.trim().to_string();
            self.cfg.collab.status = self.team.status.clone();
            self.save_now();
            self.post_team(self.team.status.clone(), "status", done);
        } else if text.starts_with("/claim") {
            let Some(mut c) = parse_claim(&text, now_ms()) else { self.toast_warn("Usage: /claim <machine> <2h | 45m | 15:00> [what for]"); return };
            let Some(m) = self.machine_by_host_or_name(&c.host) else { self.toast_warn(format!("No machine called \"{}\". Use a machine's name or hostname", c.host)); return };
            let host = self.host_of(&m.id);
            if !host.is_empty() { c.host = host; }
            self.team.claims.retain(|x| x.host != c.host);
            self.team.claims.push(Claim { host: c.host, until: c.until, note: c.note.clone() });
            self.post_team(format!("claimed {} until {}{}", m.name, fmt_time(c.until), if c.note.is_empty() { String::new() } else { format!(" · {}", c.note) }), "activity", done);
        } else if let Some(target) = text.strip_prefix("/release").map(str::trim).filter(|t| !t.is_empty()).and_then(|t| t.split_whitespace().next()) {
            let m = self.machine_by_host_or_name(target);
            let host = m.as_ref().map_or(target.to_string(), |m| self.host_of(&m.id));
            self.team.claims.retain(|x| x.host != host);
            self.post_team(format!("released {}", m.map_or(target.to_string(), |m| m.name)), "activity", done);
        } else { self.post_team(text, "msg", |app| app.team.input.clear()); }
    }

    pub(super) fn draw_team(&mut self, ui: &mut egui::Ui) {
        if !self.team.open { return; }
        let mut send = false;
        let mut close = false;
        egui::Panel::right("team").exact_size(330.0).frame(egui::Frame::new().fill(PANEL).inner_margin(10)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Team").strong());
                let hub = self.team.hub_id.as_ref().map(|id| self.machine(id).name);
                match hub.filter(|_| self.team.live) {
                    Some(name) => { dot(ui, Status::Online); ui.label(dim(format!("on {name}"))); }
                    None => { dot(ui, Status::Connecting); ui.label(dim("connecting…")); }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_btn(ui, "x", "Close (Esc)").clicked() { close = true; } });
            });
            ui.separator();
            // People: who is online, their status and which agent sessions they have open.
            let me = self.my_client().to_string();
            let mut people = self.team.presence.clone();
            people.sort_by(|a, b| (a.client != me, a.name.to_lowercase()).cmp(&(b.client != me, b.name.to_lowercase())));
            if people.is_empty() { ui.label(dim("Nobody else online.").size(11.0)); }
            egui::ScrollArea::vertical().id_salt("people").max_height(170.0).show(ui, |ui| {
                for p in &people {
                    ui.horizontal(|ui| {
                        avatar(ui, &p.name, 22.0);
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| { ui.label(RichText::new(&p.name).strong()); if p.client == me { ui.label(dim("(you)")); } });
                            if !p.status.is_empty() { ui.label(RichText::new(&p.status).color(TEXT2).size(11.5)); }
                            for x in &p.sessions {
                                ui.horizontal(|ui| {
                                    logo_img(ui, &self.preset_icon(&x.preset, None), 13.0);
                                    ui.label(RichText::new(format!("{}{}", self.preset(&x.preset).name, if x.cwd.is_empty() { String::new() } else { format!(" · {}", short_path(&x.cwd)) })).size(11.5));
                                    ui.label(dim(&x.host).size(11.0));
                                });
                            }
                        });
                    });
                }
            });
            ui.separator();
            // Compose box at the bottom, messages fill the rest.
            egui::Panel::bottom("team-compose").frame(egui::Frame::new()).show(ui, |ui| {
                let id = egui::Id::unique("team-input");
                let focused = ui.memory(|m| m.has_focus(id));
                if focused && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) { send = true; }
                ui.add_space(4.0);
                let r = ui.add(egui::TextEdit::multiline(&mut self.team.input).id(id).desired_rows(2).desired_width(f32::INFINITY).hint_text("Message the team · @name to ping · /status to set yours"));
                if std::mem::take(&mut self.team.focus_input) { r.request_focus(); }
                ui.horizontal(|ui| { ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if primary_btn(ui, "Send").clicked() { send = true; } }); });
            });
            // Names in chat are self-declared and anyone who can write to the hub can post: show who a name belongs to
            // (client id, Tailscale login) on hover, and flag a name used from more than one Nexus.
            let mut by_name: HashMap<String, HashSet<String>> = HashMap::new();
            for (n, c) in self.team.messages.iter().map(|m| (&m.from.name, &m.from.client)).chain(self.team.presence.iter().map(|p| (&p.name, &p.client))) {
                if !c.is_empty() { by_name.entry(n.trim().to_lowercase()).or_default().insert(c.clone()); }
            }
            let who = |ui: &mut egui::Ui, f: &From| {
                let tip = format!("Self-declared name{} · device {}{}", if f.login.is_empty() { String::new() } else { format!(" · Tailscale login {}", f.login) }, crate::util::clip(&f.client, 6), if f.client == me { " (you)" } else { "" });
                ui.label(RichText::new(&f.name).strong()).on_hover_text(tip);
                if by_name.get(&f.name.trim().to_lowercase()).is_some_and(|s| s.len() > 1) { chip(ui, "same name, other device", WARN).on_hover_text("More than one Nexus uses this name: check the device id on hover"); }
            };
            egui::ScrollArea::vertical().id_salt("msgs").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                if self.team.messages.is_empty() {
                    ui.add_space(20.0);
                    ui.label(dim("No messages yet. Say what you're working on so nobody doubles up."));
                    ui.label(dim("Tips: /status Training on the GPU box until 3pm sets your status · /claim gpu-box 2h fine-tuning claims a machine (/release gpu-box when done).").size(11.0));
                }
                let mut prev: Option<&Message> = None;
                for m in &self.team.messages {
                    if m.kind != "msg" {
                        prev = None;
                        ui.horizontal_wrapped(|ui| {
                            avatar(ui, &m.from.name, 14.0);
                            who(ui, &m.from);
                            let text = if m.kind == "status" { if m.text.is_empty() { "cleared their status".to_string() } else { format!("set status: {}", m.text) } } else { m.text.clone() };
                            ui.label(RichText::new(text).color(TEXT2).size(12.0));
                            ui.label(dim(fmt_time(m.ts)).size(10.5));
                        });
                        continue;
                    }
                    let cont = prev.is_some_and(|p| p.from.client == m.from.client && m.ts - p.ts < 5 * 60_000);
                    prev = Some(m);
                    if !cont {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| { avatar(ui, &m.from.name, 20.0); who(ui, &m.from); ui.label(dim(fmt_time(m.ts)).size(10.5)); });
                    }
                    ui.horizontal(|ui| { ui.add_space(26.0); message_text(ui, &m.text); });
                }
            });
        });
        if close || (self.team.open && self.dialogs.is_empty() && self.palette.is_none() && self.ctx.memory(|m| m.has_focus(egui::Id::unique("team-input"))) && self.ctx.input(|i| i.key_pressed(egui::Key::Escape))) { self.close_team(); }
        if send { self.send_team(); self.team.focus_input = true; }
    }
}

/// Chat text with links (web links only) and @mentions picked out.
fn message_text(ui: &mut egui::Ui, text: &str) {
    ui.vertical(|ui| {
        for line in text.lines() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for word in line.split(' ') {
                    if word.starts_with("https://") || word.starts_with("http://") {
                        if ui.link(word).clicked() { crate::platform::open_external(word); }
                    } else if word.len() > 1 && word.starts_with('@') { ui.label(RichText::new(word).color(BRAND).strong()); }
                    else { ui.label(word); }
                }
            });
        }
    });
}
