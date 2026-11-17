//! The fleet view: every agent on every machine at a glance, with the ones waiting for you first and answerable in
//! place. Also the replay viewer for recorded sessions.
use super::overlays::{modal_frame, section};
use super::sessions::{is_agent, preset_from_tmux};
use super::*;
use crate::remote::{AgentStatus, LogFile, LOG_MAX};
use crate::term::Terminal;
use crate::util::{clip, short_path};
use egui::{Key, Pos2, Rect};

#[derive(Default)]
pub struct Fleet { replies: HashMap<u64, String> }

/// How a session looks from the outside.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Look { NeedsYou, Finished, Working, Idle, Starting, Reconnecting, Exited }
impl Look {
    fn chip(self) -> (&'static str, Color32) {
        match self {
            Self::NeedsYou => ("needs you", WARN), Self::Finished => ("finished", OK), Self::Working => ("working", BRAND), Self::Idle => ("idle", TEXT3),
            Self::Starting => ("starting", TEXT3), Self::Reconnecting => ("reconnecting", WARN), Self::Exited => ("exited", TEXT3),
        }
    }
}

/// Does a notification's text say the agent is asking for something (rather than done)?
pub fn asks(why: &str) -> bool {
    let l = why.to_lowercase();
    ["permission", "needs your", "waiting for", "approve", "confirm"].iter().any(|k| l.contains(k))
}
/// A session that isn't open in Nexus, judged by its last hook report and when tmux last saw output from it.
pub fn detached_look(st: Option<&AgentStatus>, activity_ms: i64, now: i64) -> (Look, String) {
    match st {
        // Output well after the report means it has been set to work again since.
        Some(a) if activity_ms <= a.ts + 8000 => if a.event == "Notification" { (Look::NeedsYou, a.message.clone()) } else { (Look::Finished, a.summary.clone()) },
        _ => (if now - activity_ms < 90_000 { Look::Working } else { Look::Idle }, String::new()),
    }
}

/// A recorded session being played back.
pub struct Replay {
    machine_id: String,
    logs: Option<Result<Vec<LogFile>, String>>,
    sel: Option<String>,
    loading: bool,
    data: Vec<u8>,
    term: Terminal,
    /// Bytes of `data` the emulator has seen, and where playback is.
    fed: usize,
    pos: usize,
    playing: bool,
    speed: f32,
    cols: u16,
    rows: u16,
    last: i64,
    carry: f32,
    scroll_acc: f32,
}

impl App {
    pub fn look(&self, s: &Session) -> Look {
        match (s.status, s.activity) {
            (SessStatus::Starting, _) => Look::Starting,
            (SessStatus::Disconnected, _) => Look::Reconnecting,
            (SessStatus::Exited, _) => Look::Exited,
            (_, Activity::Busy) => Look::Working,
            (_, Activity::Idle) => Look::Idle,
            (_, Activity::Done) => if s.agent.as_ref().is_some_and(|a| a.event == "Notification") || s.why.as_deref().is_some_and(asks) { Look::NeedsYou } else { Look::Finished },
        }
    }
    /// Sessions that stopped and are waiting for you: the ones asking a question first.
    pub fn waiting(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.ordered().into_iter().filter(|id| { let s = &self.sessions[id]; s.activity == Activity::Done && s.status == SessStatus::Running }).collect();
        v.sort_by_key(|id| self.look(&self.sessions[id]) != Look::NeedsYou);
        v
    }
    /// How many agents want you: waiting ones open here, plus ones on the machines that asked something and aren't open.
    pub fn waiting_count(&self) -> usize {
        let open: HashSet<&String> = self.sessions.values().filter_map(|s| s.tmux_name.as_ref()).collect();
        let now = now_ms();
        let away = self.mstate.values().flat_map(|st| st.detached.iter().filter(|d| !open.contains(&d.name) && detached_look(st.agents.get(&d.name), d.activity, now).0 == Look::NeedsYou)).count();
        self.waiting().len() + away
    }
    fn why(s: &Session) -> String {
        match &s.agent {
            Some(a) if a.event == "Notification" && !a.message.is_empty() => a.message.clone(),
            Some(a) if !a.summary.is_empty() => a.summary.clone(),
            _ => s.why.clone().unwrap_or_default(),
        }
    }

    pub fn open_fleet(&mut self) {
        self.menu = None;
        self.palette = None;
        self.fleet = Some(Fleet::default());
        for m in self.cfg.machines.clone() { if m.is_ssh() && self.status_of(&m) == Status::Online { self.refresh_agents(&m.id); } }
    }
    /// Keys for a waiting agent ("1" picks the first choice of a permission prompt), without leaving the fleet view.
    fn fleet_keys(&mut self, id: u64, keys: &[u8]) { self.input(id, keys); self.clear_done(id); }
    fn fleet_reply(&mut self, id: u64, text: &str) {
        if text.trim().is_empty() { return; }
        self.paste_text(id, text);
        if let Some(pid) = self.sessions.get(&id).and_then(|s| s.pid) {
            // A moment later, so the agent has taken the paste before Enter arrives.
            let be = self.be.clone();
            self.be.rt.spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(150)).await; be.write(pid, b"\r"); });
        }
        self.clear_done(id);
    }

    /// A small live picture of a session's screen: the last rows in use, in a tiny version of the terminal font.
    fn preview(&self, ui: &egui::Ui, s: &Session, rect: Rect, small: &FontId, cell: Vec2) {
        ui.painter().rect(rect, 6.0, crate::term::BG, egui::Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
        let inner = rect.shrink(4.0);
        let used = f32::from(s.term.content_rows()) * cell.y;
        // More rows than fit: show the last ones, whole rows only.
        let (top, clip) = if used > inner.height() { (inner.bottom() - used, Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - (inner.height() / cell.y).floor() * cell.y), inner.max)) } else { (inner.top(), inner) };
        s.term.paint(&ui.painter_at(clip), Rect::from_min_size(Pos2::new(inner.left(), top), inner.size()), small, None, cell, cell.y, false, s.color, false);
    }

    pub(super) fn draw_fleet(&mut self) {
        let Some(mut f) = self.fleet.take() else { return };
        let mut todo: Vec<Act> = Vec::new();
        let top = self.dialogs.is_empty() && self.palette.is_none();
        let mut close = false;
        let screen = self.ctx.content_rect();
        let small = FontId::new(8.5, FontFamily::Name("term".into()));
        let (cw, ch) = self.ctx.fonts_mut(|x| (x.glyph_width(&small, 'M'), x.row_height(&small)));
        let cell = Vec2::new(cw.max(1.0), ch.max(1.0));
        let (waiting, all, now) = (self.waiting(), self.ordered(), now_ms());
        let modal = egui::Modal::new(egui::Id::unique("fleet")).frame(modal_frame()).show(&self.ctx, |ui| {
            let width = (screen.width() - 80.0).clamp(320.0, 1180.0);
            ui.set_width(width);
            // A modal keeps the size of its first frame unless told otherwise; this one is a full-height dashboard.
            ui.set_min_height((screen.height() - 110.0).max(200.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Fleet").size(16.0).strong());
                let working = all.iter().filter(|id| self.look(&self.sessions[id]) == Look::Working).count();
                ui.label(dim(format!("{} session{} · {working} working · {} waiting for you", all.len(), if all.len() == 1 { "" } else { "s" }, waiting.len())));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_btn(ui, "x", "Close (Esc)").clicked() { close = true; } });
            });
            egui::ScrollArea::vertical().max_height(screen.height() - 150.0).auto_shrink([false, false]).show(ui, |ui| {
                if all.is_empty() { ui.add_space(10.0); ui.label(dim("No sessions are open in this window. Launch an agent from the sidebar and it shows up here.")); }
                if !waiting.is_empty() { section(ui, "Waiting for you"); }
                for id in &waiting { self.fleet_waiting(ui, &mut f, *id, &small, cell, &mut todo); ui.add_space(4.0); }

                if !all.is_empty() { section(ui, "All sessions"); }
                let (card, gap) = (Vec2::new(272.0, 9.0 * cell.y + 62.0), 8.0);
                let per_row = (((width + gap) / (card.x + gap)).floor() as usize).max(1);
                for row in all.chunks(per_row) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for id in row { self.fleet_card(ui, *id, card, &small, cell, &mut todo); }
                    });
                    ui.add_space(gap - 5.0);
                }

                // Sessions still running on the machines that aren't open in this window.
                let open: HashSet<&String> = self.sessions.values().filter_map(|s| s.tmux_name.as_ref()).collect();
                for m in self.machines_sorted() {
                    let st = self.mst(&m.id);
                    let det: Vec<&crate::remote::TmuxSession> = st.detached.iter().filter(|d| !open.contains(&d.name)).collect();
                    if det.is_empty() { continue; }
                    section(ui, &format!("Still running on {} (not open here)", m.name));
                    for d in det {
                        let (look, text) = detached_look(st.agents.get(&d.name), d.activity, now);
                        let pid = preset_from_tmux(&d.name);
                        ui.horizontal(|ui| {
                            logo_img(ui, &self.preset_icon(&pid, Some(&m.id)), 14.0);
                            ui.label(RichText::new(&d.name).size(12.0));
                            let (label, color) = look.chip();
                            if is_agent(&pid) || look == Look::Working { chip(ui, label, color); }
                            if !d.cwd.is_empty() { ui.label(dim(short_path(&d.cwd)).size(11.0)); }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let (a, n) = (m.id.clone(), d.name.clone());
                                if ui.small_button("Open").on_hover_text("Reattach in this window").clicked() { todo.push(act(move |x| { x.fleet = None; x.reattach(&a, &n, true, false, false); })); }
                                let (a, n) = (m.id.clone(), d.name.clone());
                                if ui.small_button("Watch").on_hover_text("Open read-only").clicked() { todo.push(act(move |x| { x.fleet = None; x.reattach(&a, &n, true, false, true); })); }
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| { ui.add(egui::Label::new(RichText::new(clip(&text, 160)).size(11.5).color(TEXT2)).truncate()); });
                            });
                        });
                    }
                }
            });
        });
        if top && modal.should_close() { close = true; }
        if !close && self.fleet.is_none() { self.fleet = Some(f); }
        for a in todo { a(self); }
        if close { self.fleet = None; self.focus_terminal(); }
    }

    fn fleet_waiting(&self, ui: &mut egui::Ui, f: &mut Fleet, id: u64, small: &FontId, cell: Vec2, todo: &mut Vec<Act>) {
        let s = &self.sessions[&id];
        let (label, color) = self.look(s).chip();
        egui::Frame::new().fill(PANEL2).stroke(egui::Stroke::new(1.0, tint(color, 0.45))).corner_radius(10).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new((ui.available_width() * 0.5).clamp(200.0, 470.0), 11.0 * cell.y + 8.0), egui::Sense::click());
                self.preview(ui, s, rect, small, cell);
                if resp.on_hover_text("Open this session").clicked() { todo.push(act(move |a| a.jump_to(id))); }
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        logo_img(ui, &self.preset_icon(&s.preset_id, Some(&s.machine_id)), 15.0);
                        ui.label(RichText::new(&s.name).strong());
                        let m = self.machine(&s.machine_id);
                        chip(ui, &m.name, hex(&m.color));
                        chip(ui, label, color);
                    });
                    let why = Self::why(s);
                    if !why.is_empty() { ui.add(egui::Label::new(RichText::new(clip(&why, 320)).color(TEXT2)).wrap()); }
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.label(dim("Answer:").size(11.0));
                        for (text, keys, tip) in [("1", "1", "Press 1 (the first choice, usually Yes)"), ("2", "2", "Press 2"), ("3", "3", "Press 3"), ("Enter", "\r", "Press Enter"), ("Esc", "\x1b", "Press Escape (usually No / cancel)")] {
                            if ui.add(egui::Button::new(RichText::new(text).monospace().size(11.5))).on_hover_text(tip).clicked() { todo.push(act(move |a| a.fleet_keys(id, keys.as_bytes()))); }
                        }
                    });
                    ui.horizontal(|ui| {
                        let reply = f.replies.entry(id).or_default();
                        let r = ui.add(egui::TextEdit::singleline(reply).hint_text("Or type a reply and press Enter").desired_width((ui.available_width() - 130.0).max(120.0)));
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (primary_btn(ui, "Send").clicked() || enter) && !reply.trim().is_empty() { let text = std::mem::take(reply); todo.push(act(move |a| a.fleet_reply(id, &text))); }
                        if ui.button("Open").clicked() { todo.push(act(move |a| a.jump_to(id))); }
                    });
                });
            });
        });
    }

    fn fleet_card(&self, ui: &mut egui::Ui, id: u64, size: Vec2, small: &FontId, cell: Vec2, todo: &mut Vec<Act>) {
        let s = &self.sessions[&id];
        let (label, color) = self.look(s).chip();
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let hot = resp.hovered();
        let p = ui.painter();
        p.rect(rect, 9.0, if hot { PANEL3 } else { PANEL2 }, egui::Stroke::new(1.0, if self.look(s) == Look::NeedsYou { tint(WARN, 0.6) } else { LINE }), egui::StrokeKind::Inside);
        let pad = 8.0;
        let head = Pos2::new(rect.left() + pad, rect.top() + 14.0);
        if let Some(src) = logo(&self.preset_icon(&s.preset_id, Some(&s.machine_id))) { egui::Image::new(src).paint_at(ui, Rect::from_center_size(head + Vec2::new(7.0, 0.0), Vec2::splat(14.0))); }
        let chip_g = p.layout_no_wrap(label.to_string(), FontId::proportional(10.5), color);
        let chip_r = Rect::from_min_size(Pos2::new(rect.right() - pad - chip_g.size().x - 12.0, head.y - 8.0), chip_g.size() + Vec2::new(12.0, 4.0));
        p.rect_filled(chip_r, chip_r.height() / 2.0, tint(color, 0.16));
        p.galley(chip_r.min + Vec2::new(6.0, 2.0), chip_g, color);
        let name_clip = Rect::from_min_max(Pos2::new(head.x + 20.0, rect.top()), Pos2::new(chip_r.left() - 6.0, rect.top() + 28.0));
        p.with_clip_rect(name_clip).text(Pos2::new(head.x + 20.0, head.y), egui::Align2::LEFT_CENTER, &s.name, FontId::proportional(12.5), TEXT);
        let shot = Rect::from_min_max(Pos2::new(rect.left() + pad, rect.top() + 28.0), Pos2::new(rect.right() - pad, rect.bottom() - 24.0));
        self.preview(ui, s, shot, small, cell);
        let foot = format!("{}{}", self.machine(&s.machine_id).name, s.cwd.as_deref().filter(|c| *c != "~").map(|c| format!(" · {}", short_path(c))).unwrap_or_default());
        ui.painter().with_clip_rect(rect.shrink(pad)).text(Pos2::new(rect.left() + pad, rect.bottom() - 12.0), egui::Align2::LEFT_CENTER, foot, FontId::proportional(10.5), TEXT3);
        let why = Self::why(s);
        let resp = if why.is_empty() { resp } else { resp.on_hover_text(clip(&why, 400)) };
        if resp.clicked() { todo.push(act(move |a| a.jump_to(id))); }
    }

    // ---------------------------------------------------------------- replay of recorded sessions
    pub fn open_replay(&mut self, machine_id: &str) {
        self.menu = None;
        let (cols, rows) = self.last_cells;
        self.replay = Some(Replay {
            machine_id: machine_id.into(), logs: None, sel: None, loading: false, data: vec![], term: Terminal::new(cols, rows), fed: 0, pos: 0, playing: false, speed: 4.0,
            cols, rows, last: now_ms(), carry: 0.0, scroll_acc: 0.0,
        });
        let (be, mid) = (self.be.clone(), machine_id.to_string());
        let mid2 = mid.clone();
        self.be.run(async move { be.machine(&mid).list_logs().await }, move |app, r| {
            // Debug aid for screenshots (NEXUS_OPEN=replay): open the newest recording straight away.
            let first = r.as_ref().ok().and_then(|l| l.first()).map(|l| l.name.clone()).filter(|_| std::env::var("NEXUS_OPEN").as_deref() == Ok("replay"));
            if let Some(rp) = app.replay.as_mut().filter(|rp| rp.machine_id == mid2) { rp.logs = Some(r); if let Some(n) = first { app.replay_load(n); } }
        });
    }
    fn replay_load(&mut self, name: String) {
        let Some(rp) = self.replay.as_mut() else { return };
        rp.sel = Some(name.clone());
        rp.loading = true;
        rp.playing = false;
        let (be, mid, n2) = (self.be.clone(), rp.machine_id.clone(), name.clone());
        self.be.run(async move { be.machine(&mid).read_log(&n2).await }, move |app, r| {
            let Some(rp) = app.replay.as_mut().filter(|rp| rp.sel.as_deref() == Some(&name)) else { return };
            rp.loading = false;
            match r {
                // Opens on the last screen; drag the slider back (or press play at the start) to watch it unfold.
                Ok(data) => { rp.pos = data.len(); rp.data = data; rp.fed = 0; rp.term = Terminal::new(rp.cols, rp.rows); }
                Err(e) => { rp.sel = None; app.toast_bad(e); }
            }
        });
    }
    /// Advance playback and bring the emulator to the playback position (rewinding starts over from the first byte).
    pub(super) fn replay_tick(&mut self) {
        let Some(r) = self.replay.as_mut() else { return };
        let t = now_ms();
        if r.playing {
            r.carry += (t - r.last).clamp(0, 200) as f32 / 1000.0 * 2500.0 * r.speed;
            let n = r.carry as usize;
            r.carry -= n as f32;
            r.pos = (r.pos + n).min(r.data.len());
            if r.pos >= r.data.len() { r.playing = false; }
            self.ctx.request_repaint();
        }
        r.last = t;
        if r.term.cols != r.cols || r.term.rows != r.rows || r.pos < r.fed { r.term = Terminal::new(r.cols, r.rows); r.fed = 0; }
        if r.pos > r.fed {
            // Bounded per frame, so a long rewind can't freeze the window.
            let end = r.pos.min(r.fed + 1_500_000);
            let _ = r.term.feed(&r.data[r.fed..end]);
            r.fed = end;
            if end < r.pos { self.ctx.request_repaint(); }
        }
    }

    pub(super) fn draw_replay(&mut self) {
        let Some(mut r) = self.replay.take() else { return };
        let top = self.dialogs.is_empty() && self.palette.is_none();
        let (mut close, mut load) = (false, None::<String>);
        let screen = self.ctx.content_rect();
        let cell = self.font.cell;
        let name = self.machine(&r.machine_id).name;
        let recording = self.cfg.appearance.record_sessions;
        let modal = egui::Modal::new(egui::Id::unique("replay")).frame(modal_frame()).show(&self.ctx, |ui| {
            let width = (screen.width() - 60.0).clamp(320.0, 1280.0);
            let height = (screen.height() - 150.0).max(200.0);
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Recorded sessions on {name}")).size(16.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_btn(ui, "x", "Close (Esc)").clicked() { close = true; } });
            });
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(250.0);
                    egui::ScrollArea::vertical().id_salt("replay-list").max_height(height).auto_shrink([false, false]).show(ui, |ui| match &r.logs {
                        None => { ui.label(dim("Loading…")); }
                        Some(Err(e)) => { ui.label(RichText::new(e).color(BAD)); }
                        Some(Ok(list)) if list.is_empty() => {
                            ui.label(dim(if recording { "Nothing recorded yet. Sessions started from now on are recorded." } else { "Nothing recorded. Turn on \"Record remote sessions\" in Settings; sessions started after that are kept for 30 days." }));
                        }
                        Some(Ok(list)) => for l in list {
                            let on = r.sel.as_deref() == Some(&l.name);
                            let size = if l.size >= 1 << 20 { format!("{:.1} MB", l.size as f64 / f64::from(1 << 20)) } else { format!("{} kB", l.size.div_ceil(1024)) };
                            let text = format!("{}\n{size}", l.name.trim_end_matches(".log"));
                            if ui.add(egui::Button::selectable(on, RichText::new(text).size(11.5)).min_size(Vec2::new(240.0, 0.0))).clicked() && !on { load = Some(l.name.clone()); }
                        },
                    });
                });
                ui.vertical(|ui| {
                    let len = r.data.len();
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(len > 0, |ui| {
                            if icon_btn(ui, if r.playing { "pause" } else { "play" }, if r.playing { "Pause" } else { "Play" }).clicked() {
                                r.playing = !r.playing;
                                if r.playing && r.pos >= len { r.pos = 0; }
                            }
                            let mut at = r.pos as f32;
                            ui.spacing_mut().slider_width = (ui.available_width() - 330.0).max(120.0);
                            if ui.add(egui::Slider::new(&mut at, 0.0..=len.max(1) as f32).show_value(false)).changed() { r.pos = (at as usize).min(len); }
                            ui.label(dim(format!("{:>3}%", (r.pos * 100).checked_div(len).unwrap_or(0))).monospace());
                        });
                        egui::ComboBox::from_id_salt("replay-speed").width(54.0).selected_text(format!("{}×", r.speed)).show_ui(ui, |ui| { for s in [1.0, 4.0, 16.0, 64.0] { ui.selectable_value(&mut r.speed, s, format!("{s}×")); } });
                        ui.add(egui::DragValue::new(&mut r.cols).range(20..=400)).on_hover_text("Columns. Set this to the width the session was recorded at if full-screen programs look scrambled.");
                        ui.label(dim("×"));
                        ui.add(egui::DragValue::new(&mut r.rows).range(5..=200)).on_hover_text("Rows");
                    });
                    let size = Vec2::new(ui.available_width(), (height - 34.0).max(80.0));
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.painter().rect(rect, 8.0, crate::term::BG, egui::Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
                    let inner = rect.shrink(6.0);
                    if r.loading { ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "Loading the recording…", FontId::proportional(13.0), TEXT3); }
                    else if len == 0 { ui.painter().text(inner.center(), egui::Align2::CENTER_CENTER, "Pick a recording on the left", FontId::proportional(13.0), TEXT3); }
                    else if cell.x > 0.0 {
                        let wheel = if resp.contains_pointer() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
                        r.scroll_acc += wheel;
                        let lines = (r.scroll_acc / cell.y).trunc() as i32;
                        if lines != 0 { r.scroll_acc -= lines as f32 * cell.y; r.term.scroll(lines); }
                        r.term.paint(&ui.painter_at(inner), inner, &self.font.id, self.font.bold.as_ref(), cell, self.font.row_h, false, BRAND, false);
                        if r.sel.as_ref().and_then(|n| r.logs.as_ref()?.as_ref().ok()?.iter().find(|l| &l.name == n)).is_some_and(|l| l.size as usize > LOG_MAX) {
                            ui.painter().text(inner.right_bottom(), egui::Align2::RIGHT_BOTTOM, format!("showing the last {} MB", LOG_MAX >> 20), FontId::proportional(10.5), TEXT3);
                        }
                    }
                });
            });
        });
        if top && modal.should_close() { close = true; }
        if !close && self.replay.is_none() { self.replay = Some(r); }
        if let Some(n) = load { self.replay_load(n); }
        if close { self.replay = None; self.focus_terminal(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_that_are_not_open_are_judged_by_their_last_report() {
        let st = |event: &str, ts| AgentStatus { ts, event: event.into(), message: "Claude needs your permission to use Bash".into(), summary: "All tests pass.".into() };
        let now = 1_000_000;
        assert_eq!(detached_look(Some(&st("Notification", 900_000)), 901_000, now), (Look::NeedsYou, "Claude needs your permission to use Bash".into()));
        assert_eq!(detached_look(Some(&st("Stop", 900_000)), 899_000, now).0, Look::Finished);
        // Output long after the report: it was answered and is working again (or idle since).
        assert_eq!(detached_look(Some(&st("Notification", 500_000)), 990_000, now).0, Look::Working);
        assert_eq!(detached_look(Some(&st("Stop", 100_000)), 300_000, now).0, Look::Idle);
        assert_eq!(detached_look(None, 995_000, now).0, Look::Working);
        assert!(asks("Claude needs your permission to use Bash") && asks("Claude is waiting for your input") && !asks("Finished: all tests pass"));
    }
}
