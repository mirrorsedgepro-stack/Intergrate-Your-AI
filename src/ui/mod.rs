//! The window: tabs, machine sidebar, terminal panes, palette, settings, sharing and team chat, drawn with egui.
//! This file holds the app state, the event loop glue and the look; the other files in this folder add behaviour.
mod chrome;
mod fleet;
mod overlays;
mod panes;
mod sessions;
mod share;
mod team;

use crate::backend::{Backend, Event, TunnelInfo};
use crate::config::{Config, Machine, Preset};
use crate::remote::{Status, TmuxSession};
use crate::stats::Stats;
use crate::util::now_ms;
use eframe::egui::{self, Color32, FontFamily, FontId, RichText, Vec2};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

pub use sessions::{Activity, Extra, SessStatus, Session};

// ---------------------------------------------------------------- look
pub const BG: Color32 = Color32::from_rgb(0x09, 0x0c, 0x13);
pub const BG2: Color32 = Color32::from_rgb(0x0d, 0x11, 0x1a);
pub const PANEL: Color32 = Color32::from_rgb(0x11, 0x16, 0x22);
pub const PANEL2: Color32 = Color32::from_rgb(0x16, 0x1c, 0x2b);
pub const PANEL3: Color32 = Color32::from_rgb(0x1c, 0x23, 0x35);
pub const LINE: Color32 = Color32::from_rgb(0x1b, 0x21, 0x31);
pub const LINE2: Color32 = Color32::from_rgb(0x29, 0x31, 0x47);
pub const TEXT: Color32 = Color32::from_rgb(0xd5, 0xdb, 0xeb);
pub const TEXT2: Color32 = Color32::from_rgb(0x9a, 0xa3, 0xba);
pub const TEXT3: Color32 = Color32::from_rgb(0x64, 0x6d, 0x85);
pub const BRAND: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7);
pub const BRAND2: Color32 = Color32::from_rgb(0xbb, 0x9a, 0xf7);
pub const OK: Color32 = Color32::from_rgb(0x73, 0xda, 0xca);
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
pub const BAD: Color32 = Color32::from_rgb(0xf7, 0x76, 0x8e);

/// "#RRGGBB" -> colour (grey when it isn't one).
pub fn hex(s: &str) -> Color32 {
    let h = s.trim_start_matches('#');
    match (h.len(), u32::from_str_radix(h, 16)) { (6, Ok(v)) => Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8), _ => TEXT2 }
}
pub fn to_hex(c: Color32) -> String { format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()) }
/// Mix `c` over the panel colour (for tinted chips and buttons).
pub fn tint(c: Color32, a: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) * a + f32::from(y) * (1.0 - a)) as u8;
    Color32::from_rgb(m(c.r(), PANEL.r()), m(c.g(), PANEL.g()), m(c.b(), PANEL.b()))
}
pub const IS_MAC: bool = cfg!(target_os = "macos");
/// Key hints: ⌘/⌥ on macOS.
pub fn kbd(s: &str) -> String { if IS_MAC { s.replace("Ctrl+Shift+", "⌘⇧").replace("Ctrl+Alt+", "⌘⌥").replace("Ctrl+", "⌘") } else { s.to_string() } }

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG2;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = PANEL2;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = BRAND;
    v.selection.bg_fill = tint(BRAND, 0.45);
    v.selection.stroke = egui::Stroke::new(1.0, TEXT);
    v.window_stroke = egui::Stroke::new(1.0, LINE2);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT2);
    for (w, fill) in [(&mut v.widgets.inactive, PANEL2), (&mut v.widgets.hovered, PANEL3), (&mut v.widgets.active, PANEL3), (&mut v.widgets.open, PANEL3)] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.bg_stroke = egui::Stroke::new(1.0, LINE2);
        w.fg_stroke = egui::Stroke::new(1.0, TEXT);
        w.corner_radius = 6.into();
    }
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(6.0, 5.0);
        s.spacing.button_padding = Vec2::new(8.0, 4.0);
        s.interaction.selectable_labels = false;
        for (ts, size) in [(egui::TextStyle::Body, 13.0), (egui::TextStyle::Button, 13.0), (egui::TextStyle::Small, 11.0), (egui::TextStyle::Heading, 18.0), (egui::TextStyle::Monospace, 12.5)] {
            if let Some(f) = s.text_styles.get_mut(&ts) { f.size = size; }
        }
    });
}

/// The terminal font: the first installed family from the configured list (with its bold face), then egui's bundled
/// monospace, then symbol / CJK fonts from the system so box drawing, spinners and non-Latin text have glyphs.
fn load_fonts(ctx: &egui::Context, families: &str) -> bool {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut defs = egui::FontDefinitions::default();
    let add = |defs: &mut egui::FontDefinitions, name: &str, family: &str, weight: fontdb::Weight| -> bool {
        let q = fontdb::Query { families: &[fontdb::Family::Name(family)], weight, ..Default::default() };
        let Some(id) = db.query(&q) else { return false };
        // An exact family match only: fontdb falls back to other families otherwise.
        if !db.face(id).is_some_and(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(family)) && (weight == fontdb::Weight::NORMAL || f.weight.0 >= 600)) { return false; }
        let Some((bytes, index)) = db.with_face_data(id, |d, i| (d.to_vec(), i)) else { return false };
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        defs.font_data.insert(name.to_string(), Arc::new(data));
        true
    };
    let mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let (mut regular, mut bold) = (Vec::new(), Vec::new());
    for fam in families.split(',').map(|f| f.trim().trim_matches(['"', '\''])).filter(|f| !f.is_empty() && !f.eq_ignore_ascii_case("monospace")) {
        if add(&mut defs, "term-regular", fam, fontdb::Weight::NORMAL) {
            regular.push("term-regular".to_string());
            if add(&mut defs, "term-bold", fam, fontdb::Weight::BOLD) { bold.push("term-bold".to_string()); }
            break;
        }
    }
    // Symbols and CJK fall back through text fonts; emoji get their own family ("term-emoji", used by the terminal for
    // emoji code points only) that starts with the system's colour emoji font, so ✓ ⚠ ▶ stay plain text.
    let group = |defs: &mut egui::FontDefinitions, tag: &str, fams: &[&str]| -> Vec<String> {
        fams.iter().enumerate().filter_map(|(i, fam)| { let name = format!("term-{tag}-{i}"); add(defs, &name, fam, fontdb::Weight::NORMAL).then_some(name) }).collect()
    };
    let symbols = group(&mut defs, "symbol", &["DejaVu Sans Mono", "Menlo", "Consolas", "Segoe UI Symbol", "Apple Symbols", "Noto Sans Symbols2", "Noto Sans Symbols", "DejaVu Sans", "FreeMono"]);
    let emoji = group(&mut defs, "emoji", &["Noto Color Emoji", "Apple Color Emoji", "Segoe UI Emoji"]);
    let rest = group(&mut defs, "rest", &["Noto Emoji", "Symbola", "Noto Sans Mono CJK SC", "Noto Sans CJK SC", "Microsoft YaHei", "PingFang SC"]);
    let fallbacks: Vec<String> = symbols.iter().chain(&rest).cloned().collect();
    let has_bold = !bold.is_empty();
    let chain = |first: Vec<String>| first.into_iter().chain(mono.iter().cloned()).chain(fallbacks.iter().cloned()).collect::<Vec<_>>();
    defs.families.insert(FontFamily::Name(crate::term::EMOJI_FAMILY.into()), chain(emoji.iter().chain(&regular).cloned().collect()));
    if has_bold { bold.extend(regular.iter().cloned()); defs.families.insert(FontFamily::Name("term-bold".into()), chain(bold)); }
    defs.families.insert(FontFamily::Name("term".into()), chain(regular));
    // The UI font also gets the fallbacks (team chat and folder names can hold anything).
    if let Some(p) = defs.families.get_mut(&FontFamily::Proportional) { p.extend(fallbacks.iter().cloned()); }
    ctx.set_fonts(defs);
    has_bold
}

pub struct TermFont { pub id: FontId, pub bold: Option<FontId>, pub cell: Vec2, pub row_h: f32 }

// ---------------------------------------------------------------- state
#[derive(Default, Clone)]
pub struct MState {
    pub status: Option<Status>,
    pub error: Option<String>,
    pub route: Option<&'static str>,
    pub tools: Option<HashMap<String, bool>>,
    pub detached: Vec<TmuxSession>,
    pub hostname: String,
    pub home: String,
    pub user: String,
    pub stats: Option<Stats>,
    /// What each agent session on the machine last reported through its hooks, by tmux session name.
    pub agents: HashMap<String, crate::remote::AgentStatus>,
}

pub type Act = Rc<dyn Fn(&mut App)>;
pub fn act(f: impl Fn(&mut App) + 'static) -> Act { Rc::new(f) }

pub struct Toast { pub msg: String, pub color: Color32, pub until: i64, pub action: Option<(String, Act)> }

/// A launch waiting for a machine to be free.
pub struct Queued { pub id: u64, pub machine_id: String, pub preset_id: String, pub extra: Extra, pub why: String, pub idle_samples: u32 }

pub struct App {
    pub be: Arc<Backend>,
    rx: tokio::sync::mpsc::Receiver<Event>,
    _rt: tokio::runtime::Runtime,
    pub ctx: egui::Context,
    pub cfg: Config,
    pub mstate: HashMap<String, MState>,
    pub tunnels: Vec<TunnelInfo>,
    pub queue: Vec<Queued>,
    pub sessions: HashMap<u64, Session>,
    pub by_pid: HashMap<u64, u64>,
    /// Output that arrived before the session knew its backend id.
    early: HashMap<u64, Vec<u8>>,
    pub order: Vec<u64>,
    pub mru: Vec<u64>,
    pub layout: u8,
    pub panes: Vec<Option<u64>>,
    pub focus: usize,
    pub split: (f32, f32),
    pub win_focused: bool,
    pub targets: HashSet<u64>,
    auto_reattached: HashSet<String>,
    restoring: bool,
    dir_cache: HashMap<String, (i64, crate::remote::Dirs)>,
    manual_disconnect: HashSet<String>,
    /// machine id -> (attempts, next try at)
    machine_retry: HashMap<String, (u32, i64)>,
    uid: u64,
    mouse_tip_shown: bool,
    pub font: TermFont,
    font_cfg: (String, f32, f32),
    pub last_cells: (u16, u16),
    // overlays
    pub palette: Option<overlays::Palette>,
    pub settings: Option<overlays::Settings>,
    pub dialogs: Vec<overlays::Dialog>,
    pub composer: Option<overlays::Composer>,
    pub menu: Option<overlays::Menu>,
    pub share: Option<share::Share>,
    pub fleet: Option<fleet::Fleet>,
    pub replay: Option<fleet::Replay>,
    /// The session the last desktop alert was about, and when (so coming back to the window lands on it).
    last_alert: Option<(u64, i64)>,
    /// Text being composed in an input method (IME), shown at the cursor until it is committed.
    pub preedit: String,
    pub team: team::Team,
    pub toasts: Vec<Toast>,
    // timers (ms since epoch)
    t_activity: i64,
    t_stats: i64,
    t_refresh: i64,
    t_heartbeat: i64,
    persist_at: Option<i64>,
    save_at: Option<i64>,
    stats_busy: bool,
    screenshot: Option<(std::path::PathBuf, i64)>,
    typed: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: tokio::runtime::Runtime, cfg: Config) -> Self {
        let ctx = cc.egui_ctx.clone();
        egui_extras::install_image_loaders(&ctx);
        style(&ctx);
        let (tx, rx) = tokio::sync::mpsc::channel(4096);
        let wake_ctx = ctx.clone();
        let be = Backend::new(rt.handle().clone(), cfg.clone(), tx, Box::new(move || wake_ctx.request_repaint()));
        crate::backend::clean_paste_dir();
        let mut app = Self {
            be, rx, _rt: rt, ctx, cfg, mstate: HashMap::new(), tunnels: vec![], queue: vec![], sessions: HashMap::new(), by_pid: HashMap::new(), early: HashMap::new(),
            order: vec![], mru: vec![], layout: 1, panes: vec![None], focus: 0, split: (0.5, 0.5), win_focused: true, targets: HashSet::new(), auto_reattached: HashSet::new(), restoring: false,
            dir_cache: HashMap::new(), manual_disconnect: HashSet::new(), machine_retry: HashMap::new(), uid: 0, mouse_tip_shown: false,
            font: TermFont { id: FontId::monospace(14.0), bold: None, cell: Vec2::new(8.0, 16.0), row_h: 16.0 }, font_cfg: (String::new(), 0.0, 0.0), last_cells: (120, 30),
            palette: None, settings: None, dialogs: vec![], composer: None, menu: None, share: None, fleet: None, replay: None, last_alert: None, preedit: String::new(), team: team::Team::default(), toasts: vec![],
            t_activity: 0, t_stats: now_ms(), t_refresh: now_ms(), t_heartbeat: now_ms(), persist_at: None, save_at: None, stats_busy: false,
            screenshot: std::env::var_os("NEXUS_SCREENSHOT").map(|p| (p.into(), now_ms() + 3000)), typed: false,
        };
        app.apply_font();
        app.boot();
        app
    }

    /// (Re)load the terminal font when the configured family or size changes, and measure a cell.
    pub fn apply_font(&mut self) {
        let a = &self.cfg.appearance;
        let size = a.font_size.clamp(9.0, 28.0);
        if self.font_cfg.0 != a.font_family {
            let has_bold = load_fonts(&self.ctx, &a.font_family);
            self.font.bold = has_bold.then(|| FontId::new(size, FontFamily::Name("term-bold".into())));
        }
        self.font.id = FontId::new(size, FontFamily::Name("term".into()));
        if let Some(b) = self.font.bold.as_mut() { b.size = size; }
        self.font_cfg = (a.font_family.clone(), size, a.line_height);
        self.font.cell = Vec2::ZERO; // measured on the next frame (fonts are applied between frames)
    }
    fn measure_font(&mut self) {
        if self.font.cell != Vec2::ZERO { return; }
        let id = self.font.id.clone();
        let (w, h) = self.ctx.fonts_mut(|f| (f.glyph_width(&id, 'M'), f.row_height(&id)));
        if w <= 0.0 { return; }
        let ppp = self.ctx.pixels_per_point();
        // Rows sit on whole pixels. Columns are exactly one glyph advance wide (not rounded): a run of text is drawn
        // as one string, and it has to end where the grid says it does.
        let snap = |v: f32| (v * ppp).round().max(1.0) / ppp;
        self.font.row_h = h;
        self.font.cell = Vec2::new(w, snap((self.font_cfg.1 * self.font_cfg.2.clamp(1.0, 2.0)).max(h)));
    }

    // ---------------------------------------------------------------- small helpers
    pub fn machine(&self, id: &str) -> Machine {
        self.cfg.machine(id).cloned().unwrap_or_else(|| Machine { id: id.into(), name: id.into(), kind: "local".into(), color: "#646d85".into(), ..Default::default() })
    }
    pub fn preset(&self, id: &str) -> Preset {
        self.cfg.preset(id).cloned().unwrap_or_else(|| Preset { id: id.into(), name: id.into(), color: "#9aa3ba".into(), ..Default::default() })
    }
    pub fn ms(&mut self, id: &str) -> &mut MState { self.mstate.entry(id.to_string()).or_default() }
    pub fn mst(&self, id: &str) -> MState { self.mstate.get(id).cloned().unwrap_or_default() }
    pub fn status_of(&self, m: &Machine) -> Status { if m.is_local() { Status::Online } else { self.mstate.get(&m.id).and_then(|s| s.status).unwrap_or(Status::Offline) } }
    /// Machines in display order: remote ones first, that's where the agents usually live.
    pub fn machines_sorted(&self) -> Vec<Machine> {
        let mut ms = self.cfg.machines.clone();
        ms.sort_by_key(|m| !m.is_ssh());
        ms
    }
    /// The executable a preset launches on a machine ("" = plain shell, always present).
    pub fn is_missing(&self, m: &Machine, p: &Preset) -> bool {
        let tool = if p.id == "gitbash" { "gitbash".to_string() } else { p.cmd_for(&m.os).split_whitespace().next().unwrap_or_default().to_string() };
        !tool.is_empty() && self.mstate.get(&m.id).and_then(|s| s.tools.as_ref()).and_then(|t| t.get(&tool)) == Some(&false)
    }
    pub fn ordered(&self) -> Vec<u64> { self.order.iter().copied().filter(|id| self.sessions.contains_key(id)).collect() }
    pub fn focused(&self) -> Option<u64> { self.panes.get(self.focus).copied().flatten().filter(|id| self.sessions.contains_key(id)) }
    pub fn is_visible(&self, id: u64) -> bool { self.panes.contains(&Some(id)) }

    pub fn toast(&mut self, msg: impl Into<String>, color: Color32, ms: i64) { self.toasts.push(Toast { msg: msg.into(), color, until: now_ms() + ms, action: None }); }
    pub fn toast_ok(&mut self, msg: impl Into<String>) { self.toast(msg, OK, 3800); }
    pub fn toast_warn(&mut self, msg: impl Into<String>) { self.toast(msg, WARN, 3800); }
    pub fn toast_bad(&mut self, msg: impl Into<String>) { self.toast(msg, BAD, 6000); }
    pub fn toast_action(&mut self, msg: impl Into<String>, ms: i64, label: &str, a: Act) { self.toasts.push(Toast { msg: msg.into(), color: OK, until: now_ms() + ms, action: Some((label.into(), a)) }); }

    /// A desktop notification. With a session, clicking it brings Nexus forward on that session (Linux desktops
    /// report the click; on Windows and macOS coming back to Nexus right after the alert does the same).
    pub fn notify(&mut self, title: &str, body: &str, session: Option<u64>) {
        let (title, body) = (crate::util::clip(title, 120), crate::util::clip(body, 300));
        if let Some(id) = session { self.last_alert = Some((id, now_ms())); }
        let be = self.be.clone();
        std::thread::spawn(move || {
            let mut n = notify_rust::Notification::new();
            n.appname("Nexus").summary(&title).body(&body);
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                if session.is_some() { n.action("default", "Show"); }
                let (Ok(handle), Some(id)) = (n.show(), session) else { return };
                handle.wait_for_action(|action| { if action != "__closed" { be.emit(Event::Call(Box::new(move |app| app.jump_to(id)))); } });
            }
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            { let _ = (&be, n.show()); }
        });
    }
    /// Bring the window forward on a session (from a notification or the fleet view).
    pub fn jump_to(&mut self, id: u64) {
        self.last_alert = None;
        if !self.sessions.contains_key(&id) { return; }
        self.fleet = None;
        self.show(id);
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    pub fn flash(&self) { if !self.win_focused { self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational)); } }

    /// Save the config soon (settings that change while typing or dragging).
    pub fn save_soon(&mut self) { self.save_at = Some(now_ms() + 600); }
    pub fn save_now(&mut self) { self.save_at = None; self.be.save_config(self.cfg.clone()); }
    pub fn persist_soon(&mut self) { if !self.restoring { self.persist_at = Some(now_ms() + 800); } }

    // ---------------------------------------------------------------- boot
    fn boot(&mut self) {
        self.ms("local").status = Some(Status::Online);
        self.be.run(async { crate::local::probe() }, |app, tools| { app.ms("local").tools = Some(tools); });
        // Bring back last time's sessions: remote ones reattach to their tmux session, local ones restart in their folder.
        if self.cfg.appearance.restore_session {
            if let Some(snap) = self.cfg.last_session.clone() { self.open_layout(&snap, true); }
        }
        for m in self.cfg.machines.clone() {
            if (m.is_ssh() && (!m.host.is_empty() || !m.ts_host.is_empty())) || m.is_wsl() { self.connect_machine(&m.id, false); }
        }
        self.team.status = self.cfg.collab.status.clone();
        self.init_team();
        // Debug aid for screenshots: NEXUS_OPEN=palette|settings|share|team|composer|workspaces opens that view at start.
        match std::env::var("NEXUS_OPEN").as_deref() {
            Ok("palette") => self.open_palette(""),
            Ok("settings") => self.open_settings(),
            Ok("share") => self.open_share(),
            Ok("team") => self.open_team(None),
            Ok("composer") => self.open_composer(None),
            Ok("workspaces") => self.open_workspaces(),
            Ok("menu") => self.machine_menu(egui::pos2(240.0, 90.0), "linux"),
            Ok("fleet") => self.open_fleet(),
            Ok("prompts") => self.open_picker(overlays::PalSource::Prompts, ""),
            Ok("replay") => { if let Some(m) = self.cfg.machines.iter().find(|m| m.is_ssh()).map(|m| m.id.clone()) { self.be.run(async { tokio::time::sleep(std::time::Duration::from_millis(1500)).await }, move |app, ()| app.open_replay(&m)); } }
            _ => {}
        }
        self.be.sync_mcp();
        self.be.watch_updates();
    }

    // ---------------------------------------------------------------- events from the backend
    fn drain(&mut self) {
        // Bounded per frame so a flood of output can't starve painting.
        let mut budget = 4 * 1024 * 1024usize;
        while budget > 0 {
            let Ok(ev) = self.rx.try_recv() else { break };
            match ev {
                Event::Data(pid, data) => { budget = budget.saturating_sub(data.len().max(1)); self.on_data(pid, &data); }
                Event::Exit { pid, code, disconnected } => self.on_exit(pid, code, disconnected),
                Event::MachineStatus { id, status, error, route } => self.on_machine_status(&id, status, error, route),
                Event::AskPassphrase { ask, key_file, key_name, machine } => {
                    let be = self.be.clone();
                    self.prompt(&format!("Unlock {key_name}"), &format!("{machine} needs the passphrase for {key_file}. It is kept in memory only until Nexus closes."), "Passphrase", "", "", true,
                        move |_, v| be.answer(ask, v));
                }
                Event::AskHost { ask, info, machine } => {
                    let be = self.be.clone();
                    let target = if info.port == 22 { info.host.clone() } else { format!("{}:{}", info.host, info.port) };
                    self.choose(&format!("Trust {machine}?"),
                        &format!("This is the first connection to {target} and its host key isn't in your known_hosts. Its fingerprint is {}. To check it, run on that machine: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub (or compare with what plain ssh showed you). If it matches, trust it; Nexus remembers it and warns you if it ever changes.", info.fingerprint),
                        &[("Cancel", false), ("Trust and connect", true)], move |_, v| be.answer(ask, (v == Some(1)).then(|| "yes".to_string())));
                }
                Event::CollabMessage(m) => self.on_collab_message(m),
                Event::CollabState { machine_id, live } => { if self.team.hub_id.as_deref() == Some(&machine_id) { self.team.live = live; } }
                Event::McpLaunch { req, machine_id, preset_id, cwd } => self.mcp_launch(req, &machine_id, &preset_id, cwd),
                Event::McpClose { pid, kill } => { if let Some(&id) = self.by_pid.get(&pid) { self.close_session(id, kill); } }
                Event::UpdateAvailable { version, url } => self.on_update(version, url),
                Event::Call(f) => f(self),
            }
        }
        if budget == 0 { self.ctx.request_repaint(); }
    }

    fn on_update(&mut self, version: String, url: String) {
        if cfg!(windows) {
            let v = version.clone();
            self.toast_action(format!("Nexus {version} is available."), 60_000, "Download and install", act(move |app| {
                app.toast_ok(format!("Downloading Nexus {v}…"));
                let (be, v2, url) = (app.be.clone(), v.clone(), url.clone());
                app.be.run(async move { be.download_update(v2).await }, move |app, r| match r {
                    Ok(path) => {
                        app.toast_action("The update is ready to install.", 600_000, "Restart now", act(move |app| {
                            if crate::platform::command(&path).spawn().is_ok() { app.be.shutdown(); app.ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                            else { app.toast_bad("Couldn't start the installer"); }
                        }));
                    }
                    Err(e) => { app.toast_bad(format!("{e}. Opening the release page instead.")); crate::platform::open_external(&url); }
                });
            }));
        } else if crate::backend::installer_name(&version, std::env::consts::OS, std::env::consts::ARCH).is_some() {
            // macOS: the verified disk image is opened for you to drag across. Linux: the verified .deb is installed
            // through the system's password prompt when Nexus itself came from a .deb; otherwise it is left in Downloads.
            let v = version.clone();
            self.toast_action(format!("Nexus {version} is available."), 60_000, "Download", act(move |app| {
                app.toast_ok(format!("Downloading Nexus {v}…"));
                let (be, v2, url) = (app.be.clone(), v.clone(), url.clone());
                app.be.run(async move { be.download_update(v2).await }, move |app, r| match r {
                    Err(e) => { app.toast_bad(format!("{e}. Opening the release page instead.")); crate::platform::open_external(&url); }
                    Ok(path) if IS_MAC => { let _ = open::that_detached(&path); app.toast(format!("Downloaded and checked {}. Quit Nexus, then drag the new one into Applications.", path.display()), OK, 30_000); }
                    Ok(path) => {
                        let from_deb = std::env::current_exe().is_ok_and(|p| p.starts_with("/usr/bin"));
                        if !from_deb { app.toast(format!("Downloaded and checked {}", path.display()), OK, 30_000); return; }
                        app.toast_action("The update is downloaded and checked.", 600_000, "Install", act(move |app| {
                            let (be, path) = (app.be.clone(), path.clone());
                            app.be.run(async move { be.install_deb(path).await }, |app, r| match r {
                                Ok(()) => app.toast("Updated. Restart Nexus to use the new version.", OK, 60_000),
                                Err(e) => app.toast_bad(e),
                            });
                        }));
                    }
                });
            }));
        } else {
            self.toast_action(format!("Nexus {version} is available."), 60_000, "Download", act(move |_| { crate::platform::open_external(&url); }));
        }
    }

    // ---------------------------------------------------------------- timers
    fn tick(&mut self) {
        let t = now_ms();
        if t - self.t_activity > 700 { self.t_activity = t; self.activity_tick(); }
        if t - self.t_stats > 5000 && !self.stats_busy { self.t_stats = t; self.poll_stats(); }
        if t - self.t_refresh > 30_000 { self.t_refresh = t; self.periodic_refresh(); }
        if t - self.t_heartbeat > 15_000 { self.t_heartbeat = t; self.heartbeat(); }
        if self.persist_at.is_some_and(|at| t >= at) { self.persist_at = None; self.cfg.last_session = Some(self.snapshot()); self.be.save_config(self.cfg.clone()); }
        if self.save_at.is_some_and(|at| t >= at) { self.save_now(); }
        self.session_timers(t);
        let due: Vec<String> = self.machine_retry.iter().filter(|(_, (_, at))| t >= *at).map(|(id, _)| id.clone()).collect();
        for id in due { self.machine_retry_tick(&id); }
        self.toasts.retain(|x| x.until > t);
    }

    fn poll_stats(&mut self) {
        // Local and WSL machines have stats too, without the "online" gate SSH machines need.
        let ids: Vec<String> = self.cfg.machines.iter().filter(|m| !m.is_ssh() || self.status_of(m) == Status::Online).map(|m| m.id.clone()).collect();
        self.stats_busy = true;
        let be = self.be.clone();
        self.be.run(async move {
            let mut out = Vec::new();
            for id in ids { if let Ok(Some(s)) = be.stats(&id).await { out.push((id, s)); } }
            out
        }, |app, list| {
            for (id, s) in list { app.ms(&id).stats = Some(s); }
            app.stats_busy = false;
            app.run_queue();
        });
    }

    fn save_window_state(&self) {
        let info = self.ctx.input(|i| i.viewport().clone());
        let prev = self.be.config().window.unwrap_or_default();
        let maximized = info.maximized.unwrap_or(false);
        let mut w = crate::config::WindowCfg { maximized, ..prev };
        // Keep the restored size while maximized, so un-maximizing next time has somewhere to go.
        if let (false, Some(r)) = (maximized || info.minimized.unwrap_or(false), info.inner_rect) { w.width = Some(r.width()); w.height = Some(r.height()); }
        if let (false, Some(r)) = (maximized || info.minimized.unwrap_or(false), info.outer_rect) { w.x = Some(r.left()); w.y = Some(r.top()); }
        self.be.save_window(w);
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let focused = ctx.input(|i| i.focused);
        // Windows and macOS don't say that a notification was clicked: coming back to Nexus within 20 s of one is
        // taken as that click.
        if focused && !self.win_focused && cfg!(any(windows, target_os = "macos")) {
            if let Some((id, at)) = self.last_alert.take() { if now_ms() - at < 20_000 && !self.overlay_open() { self.jump_to(id); } }
        }
        self.win_focused = focused;
        self.drain();
        self.tick();
        if ctx.input(|i| i.viewport().close_requested()) {
            self.cfg.last_session = Some(self.snapshot());
            self.be.save_config(self.cfg.clone());
            self.save_window_state();
            self.be.shutdown();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.measure_font();
        self.handle_keyboard();
        self.handle_drops();
        self.draw_titlebar(ui);
        self.draw_statusbar(ui);
        self.draw_sidebar(ui);
        self.draw_team(ui);
        self.draw_composer(ui);
        self.replay_tick();
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG)).show(ui, |ui| {
            if self.ordered().is_empty() { self.draw_welcome(ui); } else { self.draw_panes(ui); }
        });
        self.draw_overlays();
        self.draw_toasts();
        if let Some((path, at)) = self.screenshot.clone() {
            // Debug aid (NEXUS_SCREENSHOT=file.png): save a picture of the window a moment after start, then quit.
            // NEXUS_TYPE=text: typed into the focused session a second before the picture is taken.
            if let (Some(text), Some(id)) = (std::env::var("NEXUS_TYPE").ok().filter(|_| now_ms() >= at - 1200 && !self.typed), self.focused()) {
                self.typed = true;
                self.input(id, text.replace("\\r", "\r").as_bytes());
            }
            if now_ms() >= at {
                self.screenshot = None;
                let ctx = self.ctx.clone();
                self.ctx.request_screenshot(move |img| {
                    let _ = image::save_buffer(&path, img.as_raw(), img.width() as u32, img.height() as u32, image::ColorType::Rgba8);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                });
            }
        }
    }
}

// ---------------------------------------------------------------- widgets shared by the views
pub fn dot(ui: &mut egui::Ui, status: Status) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
    let c = match status { Status::Online => OK, Status::Connecting => WARN, Status::Error => BAD, Status::Offline => TEXT3 };
    ui.painter().circle_filled(r.center(), 3.5, c);
    resp.on_hover_text(status.as_str())
}
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + Vec2::new(14.0, 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, rect.height() / 2.0, tint(color, 0.16));
    ui.painter().galley(rect.min + Vec2::new(7.0, 2.0), galley, color);
    resp
}
pub fn key_hint(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new().fill(PANEL2).stroke(egui::Stroke::new(1.0, LINE2)).corner_radius(4).inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| ui.label(RichText::new(kbd(text)).monospace().size(10.5).color(TEXT2)));
}
/// Line icons (the same drawings the Electron renderer used), as white SVGs tinted when drawn.
const ICONS: &[(&str, &str)] = &[
    ("x", r#"<path d="M6 6l12 12M18 6L6 18"/>"#),
    ("refresh", r#"<path d="M20 11a8 8 0 0 0-14.6-4.5M4 4v4h4M4 13a8 8 0 0 0 14.6 4.5M20 20v-4h-4"/>"#),
    ("plug", r#"<path d="M9 2v5M15 2v5M6 7h12v4a6 6 0 0 1-12 0zM12 17v5"/>"#),
    ("more", r##"<circle fill="#fff" stroke="none" cx="5" cy="12" r="1.6"/><circle fill="#fff" stroke="none" cx="12" cy="12" r="1.6"/><circle fill="#fff" stroke="none" cx="19" cy="12" r="1.6"/>"##),
    ("search", r#"<circle cx="11" cy="11" r="6.5"/><path d="M16 16l4 4"/>"#),
    ("up", r#"<path d="M6 15l6-6 6 6"/>"#),
    ("down", r#"<path d="M6 9l6 6 6-6"/>"#),
    ("split", r#"<rect x="3" y="4" width="18" height="16" rx="2.5"/><path d="M12 4v16"/>"#),
    ("send", r#"<path d="M4 20l16-8L4 4v6l10 2-10 2z"/>"#),
    ("gear", r#"<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1"/>"#),
    ("key", r#"<circle cx="7.5" cy="15.5" r="4"/><path d="M10.5 12.5L20 3M16 7l3 3M14 9l2 2"/>"#),
    ("clock", r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#),
    ("box", r#"<path d="M21 8l-9-5-9 5 9 5 9-5zM3 8v8l9 5 9-5V8M12 13v8"/>"#),
    ("layout", r#"<rect x="3" y="3" width="18" height="18" rx="3"/><path d="M12 3v18M3 12h9"/>"#),
    ("folder", r#"<path d="M3 7.5A2.5 2.5 0 0 1 5.5 5H9l2 2.5h7.5A2.5 2.5 0 0 1 21 10v7.5a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 17.5z"/>"#),
    ("bell", r#"<path d="M6 16V11a6 6 0 0 1 12 0v5l1.5 2h-15zM10 20.5a2 2 0 0 0 4 0"/>"#),
    ("save", r#"<path d="M5 4h11l3 3v13H5zM8 4v5h7V4M8 20v-6h8v6"/>"#),
    ("trash", r#"<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>"#),
    ("chat", r#"<path d="M4 5.5h16v10H9l-5 4z"/><path d="M8 9.5h8M8 12.5h5"/>"#),
    ("people", r#"<circle cx="9" cy="8" r="3.2"/><path d="M3.5 19c.6-3.2 2.8-5 5.5-5s4.9 1.8 5.5 5"/><circle cx="17" cy="9" r="2.4"/><path d="M15.5 14.2c2.6.1 4.4 1.7 5 4.8"/>"#),
    ("layout1", r#"<rect x="4" y="4" width="16" height="16" rx="3"/>"#),
    ("layout2", r#"<rect x="3.5" y="4" width="7" height="16" rx="2.4"/><rect x="13.5" y="4" width="7" height="16" rx="2.4"/>"#),
    ("layout4", r#"<rect x="3.5" y="3.5" width="7" height="7" rx="2"/><rect x="13.5" y="3.5" width="7" height="7" rx="2"/><rect x="3.5" y="13.5" width="7" height="7" rx="2"/><rect x="13.5" y="13.5" width="7" height="7" rx="2"/>"#),
    ("shell", r#"<path d="M5 7l5 5-5 5M12.5 18H19"/>"#),
    ("inbox", r#"<path d="M4 13l2.5-8h11L20 13v6H4zM4 13h5l1 2.5h4l1-2.5h5"/>"#),
    ("play", r#"<path d="M7 5l12 7-12 7z"/>"#),
    ("pause", r#"<path d="M8 5v14M16 5v14"/>"#),
    ("branch", r#"<circle cx="7" cy="5" r="2"/><circle cx="7" cy="19" r="2"/><circle cx="17" cy="8" r="2"/><path d="M7 7v10M17 10c0 5-10 2-10 7"/>"#),
    ("book", r#"<path d="M5 4h11a3 3 0 0 1 3 3v13H8a3 3 0 0 1-3-3zM5 17a3 3 0 0 1 3-3h11"/>"#),
];
pub fn icon_src(name: &str) -> egui::ImageSource<'static> {
    static CACHE: std::sync::LazyLock<HashMap<&'static str, (String, &'static [u8])>> = std::sync::LazyLock::new(|| {
        ICONS.iter().map(|(name, body)| {
            let svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="48" height="48" fill="none" stroke="#fff" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"##);
            (*name, (format!("bytes://nexus-icon-{name}.svg"), &*svg.into_bytes().leak()))
        }).collect()
    });
    let (uri, bytes) = CACHE.get(name).or_else(|| CACHE.get("shell")).expect("icon");
    egui::ImageSource::Bytes { uri: uri.clone().into(), bytes: (*bytes).into() }
}
pub fn icon(ui: &mut egui::Ui, name: &str, size: f32, color: Color32) -> egui::Response {
    ui.add(egui::Image::new(icon_src(name)).fit_to_exact_size(Vec2::splat(size)).tint(color))
}
/// A flat icon button.
pub fn icon_btn(ui: &mut egui::Ui, name: &str, tip: &str) -> egui::Response { icon_btn_c(ui, name, tip, TEXT2, 15.0) }
pub fn icon_btn_c(ui: &mut egui::Ui, name: &str, tip: &str, color: Color32, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size + 9.0), egui::Sense::click());
    if resp.hovered() { ui.painter().rect_filled(rect, 6.0, PANEL3); }
    egui::Image::new(icon_src(name)).tint(if resp.hovered() { TEXT } else { color }).paint_at(ui, egui::Rect::from_center_size(rect.center(), Vec2::splat(size)));
    if tip.is_empty() { resp } else { resp.on_hover_text(kbd(tip)) }
}
/// A button with a line icon and a label.
pub fn icon_text_btn(ui: &mut egui::Ui, name: &str, text: &str, framed: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), TEXT);
    let pad = if framed { Vec2::new(9.0, 5.0) } else { Vec2::new(4.0, 4.0) };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(14.0 + 6.0 + galley.size().x, galley.size().y.max(14.0)) + pad * 2.0, egui::Sense::click());
    if framed || resp.hovered() { ui.painter().rect(rect, 6.0, if resp.hovered() { PANEL3 } else { PANEL2 }, egui::Stroke::new(1.0, if framed { LINE2 } else { Color32::TRANSPARENT }), egui::StrokeKind::Inside); }
    egui::Image::new(icon_src(name)).tint(TEXT2).paint_at(ui, egui::Rect::from_min_size(egui::pos2(rect.left() + pad.x, rect.center().y - 7.0), Vec2::splat(14.0)));
    ui.painter().galley(egui::pos2(rect.left() + pad.x + 20.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    resp
}
/// A launcher pill: logo + name (+ a small second line), sized up front so rows of them wrap.
#[allow(clippy::too_many_arguments)]
pub fn pill(ui: &mut egui::Ui, logo_key: &str, text: &str, sub: &str, color: Color32, faded: bool, icon_size: f32, text_size: f32, pad: Vec2, outline: bool) -> egui::Response {
    let fg = if faded { TEXT3 } else { TEXT };
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(text_size), fg);
    let sub_g = (!sub.is_empty()).then(|| ui.painter().layout_no_wrap(sub.to_string(), FontId::proportional(10.5), TEXT3));
    let text_w = galley.size().x.max(sub_g.as_ref().map_or(0.0, |g| g.size().x));
    let text_h = galley.size().y + sub_g.as_ref().map_or(0.0, |g| g.size().y);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(icon_size + 6.0 + text_w, icon_size.max(text_h)) + pad * 2.0, egui::Sense::click());
    let a = if faded { 0.04 } else if resp.hovered() { 0.2 } else { 0.11 };
    ui.painter().rect(rect, 8.0, tint(color, a), egui::Stroke::new(1.0, if outline { tint(color, 0.28) } else { Color32::TRANSPARENT }), egui::StrokeKind::Inside);
    let img = egui::Rect::from_min_size(egui::pos2(rect.left() + pad.x, rect.center().y - icon_size / 2.0), Vec2::splat(icon_size));
    match logo(logo_key) { Some(src) => egui::Image::new(src).tint(if faded { Color32::from_white_alpha(110) } else { Color32::WHITE }).paint_at(ui, img), None => egui::Image::new(icon_src("shell")).tint(TEXT2).paint_at(ui, img) }
    let x = img.right() + 6.0;
    let y = rect.center().y - text_h / 2.0;
    let gh = galley.size().y;
    ui.painter().galley(egui::pos2(x, y), galley, fg);
    if let Some(g) = sub_g { ui.painter().galley(egui::pos2(x, y + gh), g, TEXT3); }
    resp
}
pub fn primary_btn(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).color(BG).strong()).fill(BRAND))
}
pub fn dim(text: impl Into<String>) -> RichText { RichText::new(text).color(TEXT3) }

/// Brand logos (assets/icons; credits in README). Presets/machines may name one via an `icon` field.
pub fn logo(key: &str) -> Option<egui::ImageSource<'static>> {
    Some(match key {
        "claude" => egui::include_image!("../../assets/icons/claude.svg"),
        "antigravity" => egui::include_image!("../../assets/icons/google-antigravity.png"),
        "openclaw" => egui::include_image!("../../assets/icons/openclaw.svg"),
        "gitbash" | "git" => egui::include_image!("../../assets/icons/git.svg"),
        "apple" => egui::include_image!("../../assets/icons/apple.svg"),
        "ubuntu" => egui::include_image!("../../assets/icons/ubuntu.svg"),
        "powershell" => egui::include_image!("../../assets/icons/powershell.svg"),
        "terminal" => egui::include_image!("../../assets/icons/terminal.svg"),
        "windows" => egui::include_image!("../../assets/icons/windows.svg"),
        "linux" => egui::include_image!("../../assets/icons/linux.svg"),
        "nvidia" => egui::include_image!("../../assets/icons/nvidia.svg"),
        _ => return None,
    })
}
pub fn logo_img(ui: &mut egui::Ui, key: &str, size: f32) {
    match logo(key) {
        Some(src) => { ui.add(egui::Image::new(src).fit_to_exact_size(Vec2::splat(size))); }
        None => { icon(ui, "shell", size, TEXT2); }
    }
}

impl App {
    /// The logo for a preset on a machine (the plain shell is PowerShell on Windows, a terminal elsewhere).
    pub fn preset_icon(&self, preset_id: &str, machine_id: Option<&str>) -> String {
        let p = self.preset(preset_id);
        let key = if p.icon.is_empty() { preset_id.to_string() } else { p.icon };
        if key == "shell" { return if machine_id.is_some_and(|m| self.machine(m).os == "windows") { "powershell" } else { "terminal" }.into(); }
        if logo(&key).is_some() { key } else { "terminal".into() }
    }
    pub fn machine_icon(&self, m: &Machine) -> &'static str {
        match m.icon.as_str() { "windows" => return "windows", "linux" => return "linux", "nvidia" => return "nvidia", _ => {} }
        if m.is_local() { return match m.os.as_str() { "mac" => "apple", "linux" => "linux", _ => "windows" }; }
        if m.is_wsl() { return if m.distro.to_lowercase().contains("ubuntu") { "ubuntu" } else { "linux" }; }
        // NVIDIA GPU machines (detected by nvidia-smi, or named like a DGX / Spark) get the NVIDIA logo.
        let st = self.mst(&m.id);
        let named = format!("{} {}", st.hostname, m.name).to_lowercase();
        if st.tools.as_ref().and_then(|t| t.get("nvidia-smi")) == Some(&true) || named.contains("spark") || named.contains("dgx") { "nvidia" } else { "linux" }
    }
}
