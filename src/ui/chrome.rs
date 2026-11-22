//! The frame around the panes: tabs and title actions, the machine sidebar with its stats, the status bar and the
//! welcome screen.
use super::overlays::{mi, Icon, MenuItem};
use super::sessions::preset_from_tmux;
use super::team::avatar;
use super::*;
use crate::util::{bps, fmt_time, gb, short_path};

fn os_label(os: &str) -> &'static str { match os { "windows" => "Windows", "mac" => "macOS", "linux" => "Linux", _ => "" } }

fn meter(ui: &mut egui::Ui, label: &str, pct: f64, value: &str, tip: &str) {
    let p = pct.clamp(0.0, 100.0) as f32;
    let color = if p >= 85.0 { BAD } else if p >= 60.0 { WARN } else { BRAND };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 15.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.text(rect.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(9.5), TEXT3);
    painter.text(rect.right_center() - Vec2::new(54.0, 0.0), egui::Align2::RIGHT_CENTER, value, FontId::proportional(10.5), TEXT2);
    let bar = egui::Rect::from_min_size(egui::pos2(rect.right() - 48.0, rect.center().y - 2.0), Vec2::new(48.0, 4.0));
    painter.rect_filled(bar, 2.0, LINE2);
    painter.rect_filled(egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * p / 100.0, 4.0)), 2.0, color);
    if !tip.is_empty() { resp.on_hover_text(tip); }
}

impl App {
    fn act_color(s: &Session) -> Option<Color32> {
        if matches!(s.status, SessStatus::Exited | SessStatus::Disconnected) { return Some(TEXT3); }
        match s.activity { Activity::Busy => Some(WARN), Activity::Done => Some(OK), Activity::Idle => None }
    }

    // ---------------------------------------------------------------- title bar: brand, tabs, actions
    pub(super) fn draw_titlebar(&mut self, ui: &mut egui::Ui) {
        let mut todo: Vec<Act> = Vec::new();
        egui::Panel::top("titlebar").exact_size(42.0).frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(10, 0))).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("◆").color(BRAND).size(15.0));
                ui.label(RichText::new("nexus").strong().size(14.0));
                ui.add_space(8.0);
                // Right side first, so the tabs get what is left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (n, name, tip) in [(4u8, "layout4", "Grid"), (2, "layout2", "Side by side"), (1, "layout1", "Single")] {
                        if icon_btn_c(ui, name, &format!("{tip} · Layout (Ctrl+Shift+L)"), if self.layout == n { BRAND } else { TEXT3 }, 15.0).clicked() { todo.push(act(move |a| a.set_layout(n))); }
                    }
                    ui.add_space(6.0);
                    if icon_btn(ui, "search", "Command palette (Ctrl+Shift+P)").clicked() { todo.push(act(|a| a.open_palette(""))); }
                    let waiting = self.waiting_count();
                    let r = icon_btn_c(ui, "inbox", "Fleet: every agent, and the ones waiting for you (Ctrl+Shift+A)", if waiting > 0 { WARN } else { TEXT2 }, 15.0);
                    if waiting > 0 { ui.painter().text(r.rect.right_top() + Vec2::new(-2.0, 3.0), egui::Align2::CENTER_CENTER, if waiting > 9 { "9+".to_string() } else { waiting.to_string() }, FontId::proportional(9.5), WARN); }
                    if r.clicked() { todo.push(act(App::open_fleet)); }
                    if icon_btn(ui, "layout", "Workspaces (Ctrl+Shift+O)").clicked() { todo.push(act(App::open_workspaces)); }
                    if icon_btn(ui, "people", "Share machines over Tailscale").clicked() { todo.push(act(App::open_share)); }
                    let r = icon_btn_c(ui, "chat", "Team chat (Ctrl+Shift+M)", if self.team.live { OK } else { TEXT2 }, 15.0);
                    if self.team.unread > 0 { ui.painter().text(r.rect.right_top() + Vec2::new(-2.0, 3.0), egui::Align2::CENTER_CENTER, if self.team.unread > 9 { "9+".to_string() } else { self.team.unread.to_string() }, FontId::proportional(9.5), BAD); }
                    if r.clicked() { todo.push(act(App::toggle_team)); }
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        egui::ScrollArea::horizontal().id_salt("tabs").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                            let focused = self.focused();
                            for (i, id) in self.ordered().into_iter().enumerate() {
                                let s = &self.sessions[&id];
                                let active = focused == Some(id);
                                // The tab senses clicks underneath its contents (UiBuilder::sense), so its close button
                                // still gets its own. Sensing on the frame's response afterwards would cover the button.
                                let resp = ui.scope_builder(egui::UiBuilder::new().id_salt(("tab", id)).sense(egui::Sense::click()), |ui| { egui::Frame::new().fill(if active { PANEL2 } else { Color32::TRANSPARENT }).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        match Self::act_color(s) {
                                            Some(c) => { let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover()); ui.painter().circle_filled(r.center(), 4.0, c); }
                                            None => logo_img(ui, &self.preset_icon(&s.preset_id, Some(&s.machine_id)), 14.0),
                                        }
                                        ui.label(RichText::new(&s.name).color(if active { TEXT } else { TEXT2 }));
                                        ui.label(dim(self.machine(&s.machine_id).name).size(11.0));
                                        if i < 9 { ui.label(RichText::new(format!("^{}", i + 1)).monospace().size(9.5).color(TEXT3)); }
                                        if icon_btn_c(ui, "x", "", TEXT3, 10.0).clicked() { todo.push(act(move |a| a.close_session(id, false))); }
                                    });
                                }); }).response;
                                if active { ui.painter().hline(resp.rect.x_range(), resp.rect.bottom() - 1.0, egui::Stroke::new(2.0, s.color)); }
                                let resp = resp.on_hover_text(if s.title.is_empty() { s.name.clone() } else { s.title.clone() });
                                if resp.clicked() { todo.push(act(move |a| a.show(id))); }
                                if resp.middle_clicked() { todo.push(act(move |a| a.close_session(id, false))); }
                                if resp.secondary_clicked() { let pos = resp.interact_pointer_pos().unwrap_or(resp.rect.left_bottom()); todo.push(act(move |a| a.term_menu(pos, id))); }
                            }
                        });
                    });
                });
            });
        });
        for a in todo { a(self); }
    }

    // ---------------------------------------------------------------- status bar
    pub(super) fn draw_statusbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("statusbar").exact_size(26.0).frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(10, 0))).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                for m in &self.cfg.machines { dot(ui, self.status_of(m)); ui.label(RichText::new(&m.name).size(11.0).color(TEXT3)); ui.add_space(6.0); }
                if let Some(s) = self.focused().and_then(|id| self.sessions.get(&id)) {
                    let tmux = s.tmux_name.as_ref().map(|t| format!(" · tmux {t}")).unwrap_or_default();
                    ui.label(RichText::new(format!("{} @ {}{tmux} · {}×{}", s.name, self.machine(&s.machine_id).name, s.term.cols, s.term.rows)).size(11.0).color(TEXT2));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.available_width() < 520.0 { return; }
                    for (k, what) in [("Ctrl+Shift+Enter", "broadcast"), ("Ctrl+Shift+A", "fleet"), ("Ctrl+1-9", "switch"), ("Ctrl+Shift+P", "palette")] {
                        ui.label(RichText::new(what).size(11.0).color(TEXT3));
                        key_hint(ui, k);
                        ui.add_space(4.0);
                    }
                });
            });
        });
    }

    // ---------------------------------------------------------------- sidebar
    pub(super) fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        let mut todo: Vec<Act> = Vec::new();
        egui::Panel::left("sidebar").exact_size(264.0).frame(egui::Frame::new().fill(BG2).inner_margin(8)).show(ui, |ui| {
            egui::Panel::bottom("sidebar-foot").frame(egui::Frame::new()).show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if icon_text_btn(ui, "send", "Broadcast", false).on_hover_text(kbd("Composer (Ctrl+Shift+Enter)")).clicked() { todo.push(act(App::toggle_composer)); }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_text_btn(ui, "gear", "Settings", false).on_hover_text(kbd("Settings (Ctrl+,)")).clicked() { todo.push(act(App::open_settings)); } });
                });
            });
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for m in self.machines_sorted() { self.machine_card(ui, &m, &mut todo); ui.add_space(6.0); }
            });
        });
        for a in todo { a(self); }
    }

    fn machine_card(&self, ui: &mut egui::Ui, m: &Machine, todo: &mut Vec<Act>) {
        let st = self.mst(&m.id);
        let status = self.status_of(m);
        let color = hex(&m.color);
        let mid = m.id.clone();
        // The card senses (right-)clicks underneath everything in it: launchers, session rows and the menu button come first.
        let card = ui.scope_builder(egui::UiBuilder::new().id_salt(("machine-card", &m.id)).sense(egui::Sense::click()), |ui| { egui::Frame::new().fill(PANEL).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(11).inner_margin(9).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                logo_img(ui, self.machine_icon(m), 22.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.horizontal(|ui| { ui.label(RichText::new(&m.name).strong()); dot(ui, status); });
                    ui.horizontal(|ui| {
                        if status == Status::Online { match st.route { Some("tailscale") => { chip(ui, "TS", OK).on_hover_text("Connected through Tailscale"); } Some("jump") => { chip(ui, "JUMP", OK).on_hover_text("Connected through a jump host"); } _ => {} } }
                        let sub = if m.is_local() { format!("local · {}", os_label(&m.os)) }
                            else if m.is_wsl() { format!("WSL · {}{}", m.distro, if st.user.is_empty() { String::new() } else { format!(" · {}", st.user) }) }
                            else if m.host.is_empty() && m.ts_host.is_empty() { "not configured".into() }
                            else { format!("{}{}", if m.user.is_empty() { String::new() } else { format!("{}@", m.user) }, if m.host.is_empty() { &m.ts_host } else { &m.host }) };
                        ui.add(egui::Label::new(dim(sub).size(11.0)).truncate());
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = icon_btn(ui, "more", "More");
                    if r.clicked() { let (id, pos) = (mid.clone(), r.rect.left_bottom() + Vec2::new(0.0, 4.0)); todo.push(act(move |a| a.machine_menu(pos, &id))); }
                    if m.is_ssh() && icon_btn(ui, if status == Status::Online { "refresh" } else { "plug" }, if status == Status::Online { "Refresh" } else { "Connect" }).clicked() { let id = mid.clone(); todo.push(act(move |a| a.connect_machine(&id, false))); }
                });
            });
            if let (Status::Error, Some(e)) = (status, &st.error) {
                egui::Frame::new().fill(tint(BAD, 0.12)).corner_radius(7).inner_margin(6).show(ui, |ui| {
                    ui.label(RichText::new(e).size(11.5).color(BAD));
                    if ui.small_button("Open settings").clicked() { todo.push(act(App::open_settings)); }
                });
            }
            // "Claimed by Sam until 15:00 · fine-tuning" (others' claims and your own) and anything queued for this machine.
            if m.is_ssh() {
                let note = |n: &str| if n.is_empty() { String::new() } else { format!(" · {n}") };
                if let Some(c) = self.my_claim_on(&m.id) { ui.label(RichText::new(format!("Yours until {}{}", fmt_time(c.until), note(&c.note))).size(11.0).color(OK)).on_hover_text(format!("/release {} to give it up", m.name)); }
                for c in self.claimed_by(&m.id) { ui.label(RichText::new(format!("{} until {}{}", c.person, fmt_time(c.until), note(&c.note))).size(11.0).color(WARN)); }
                let q = self.queue.iter().filter(|x| x.machine_id == m.id).count();
                if q > 0 { ui.label(RichText::new(format!("{q} launch{} queued until free", if q > 1 { "es" } else { "" })).size(11.0).color(TEXT2)); }
            }
            if let Some(x) = st.stats.as_ref().filter(|_| !m.is_ssh() || status == Status::Online) { self.stats_meters(ui, x); }

            // Launchers: click to start, Shift+click or right-click to pick a folder.
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                for p in self.cfg.presets_for(m) {
                    let missing = self.is_missing(m, p);
                    let resp = pill(ui, &self.preset_icon(&p.id, Some(&m.id)), &p.name, "", hex(&p.color), missing, 14.0, 12.0, Vec2::new(7.0, 4.0), false);
                    let resp = resp.on_hover_text(if missing { format!("{} is not installed on {}", p.name, m.name) } else { format!("New {} on {} · Shift+click or right-click to pick a folder", p.name, m.name) });
                    let (a, b) = (mid.clone(), p.id.clone());
                    if resp.secondary_clicked() || (resp.clicked() && ui.input(|i| i.modifiers.shift)) { todo.push(act(move |x| x.launch_in_folder(&a, &b))); }
                    else if resp.clicked() { todo.push(act(move |x| x.launch(&a, &b, Extra::default()))); }
                }
            });

            let focused = self.focused();
            let open: Vec<&Session> = self.ordered().into_iter().filter_map(|id| self.sessions.get(&id)).filter(|s| s.machine_id == m.id).collect();
            let open_names: HashSet<&String> = open.iter().filter_map(|s| s.tmux_name.as_ref()).collect();
            let row = |ui: &mut egui::Ui, active: bool, add: &mut dyn FnMut(&mut egui::Ui)| -> egui::Response {
                ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
                    egui::Frame::new().fill(if active { PANEL3 } else { Color32::TRANSPARENT }).corner_radius(6).inner_margin(egui::Margin::symmetric(5, 3)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| add(ui));
                    });
                }).response
            };
            for s in &open {
                let id = s.id;
                let i = self.order.iter().position(|x| *x == id).unwrap_or(99);
                let mut close = false;
                let resp = row(ui, focused == Some(id), &mut |ui| {
                    logo_img(ui, &self.preset_icon(&s.preset_id, Some(&s.machine_id)), 14.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_btn_c(ui, "x", "", TEXT3, 10.0).on_hover_text(if s.tmux_name.is_some() { "Detach" } else { "Close" }).clicked() { close = true; }
                        if i < 9 { ui.label(RichText::new(format!("^{}", i + 1)).monospace().size(9.5).color(TEXT3)); }
                        if let Some(c) = Self::act_color(s) { let (r, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover()); ui.painter().circle_filled(r.center(), 3.5, c); }
                        if let Some(t) = &s.tmux_name { let names = self.who_in(&s.machine_id, t); for n in &names { avatar(ui, n, 13.0).on_hover_text(format!("Also open in {}'s Nexus", names.join(", "))); } }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(if s.title.is_empty() { s.name.clone() } else { format!("{} · {}", s.name, s.title) }).size(12.0)).truncate());
                        });
                    });
                });
                if close { todo.push(act(move |a| a.close_session(id, false))); }
                else if resp.clicked() { todo.push(act(move |a| a.show(id))); }
                else if resp.secondary_clicked() { let pos = resp.interact_pointer_pos().unwrap_or(resp.rect.left_bottom()); todo.push(act(move |a| a.term_menu(pos, id))); }
            }
            let detached: Vec<&crate::remote::TmuxSession> = st.detached.iter().filter(|d| !open_names.contains(&d.name)).collect();
            if !detached.is_empty() { ui.label(RichText::new("DETACHED · STILL RUNNING").size(9.5).color(TEXT3)); }
            for d in detached {
                let pid = preset_from_tmux(&d.name);
                let mut kill = false;
                let resp = row(ui, false, &mut |ui| {
                    logo_img(ui, &self.preset_icon(&pid, Some(&m.id)), 14.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_btn_c(ui, "x", "", TEXT3, 10.0).on_hover_text("Kill this tmux session").clicked() { kill = true; }
                        for n in self.who_in(&m.id, &d.name) { avatar(ui, &n, 13.0); }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(if d.cwd.is_empty() { d.name.clone() } else { format!("{} · {}", d.name, short_path(&d.cwd)) }).size(12.0).color(TEXT2)).truncate());
                        });
                    });
                }).on_hover_text("Reattach");
                let (a, name) = (mid.clone(), d.name.clone());
                if kill {
                    todo.push(act(move |x| {
                        let (a, name) = (a.clone(), name.clone());
                        x.choose(&format!("Kill tmux session \"{name}\"?"), "Anything running in it will stop.", &[("Cancel", false), ("Kill session", true)], move |x, v| {
                            if v != Some(1) { return; }
                            let (be, a2, n2) = (x.be.clone(), a.clone(), name.clone());
                            x.be.run(async move { be.machine(&a2).kill_tmux(&n2).await }, move |x, ()| x.refresh_machine(&a));
                        });
                    }));
                } else if resp.clicked() { todo.push(act(move |x| x.reattach(&a, &name, true, false, false))); }
            }
        }); }).response;
        // A coloured edge in the machine's accent.
        ui.painter().rect_filled(egui::Rect::from_min_max(card.rect.left_top() + Vec2::new(0.0, 10.0), card.rect.left_bottom() + Vec2::new(2.0, -10.0)), 1.0, color);
        if card.secondary_clicked() { if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) { todo.push(act(move |a| a.machine_menu(pos, &mid))); } }
    }

    /// GPU / RAM / CPU / disk / network, refreshed every 5 s.
    fn stats_meters(&self, ui: &mut egui::Ui, x: &Stats) {
        ui.add_space(2.0);
        ui.spacing_mut().item_spacing.y = 1.0;
        if let Some(g) = &x.gpu {
            if let Some(e) = &g.error { meter(ui, "GPU", 0.0, "driver unavailable", e); }
            else {
                meter(ui, "GPU", g.util.unwrap_or(0.0), &g.util.map_or("-".into(), |u| format!("{u:.0}%")), &g.name);
                if let (Some(used), Some(total)) = (g.mem_used, g.mem_total.filter(|t| *t > 0.0)) { meter(ui, "VRAM", used / total * 100.0, &format!("{:.1}/{:.0} GB", used / 1024.0, total / 1024.0), "GPU memory in use"); }
                if let Some(t) = g.temp { meter(ui, "TEMP", (t - 30.0) / 0.6, &format!("{t:.0}°C{}", g.power.map(|p| format!(" · {p:.0}W")).unwrap_or_default()), "GPU temperature / power"); }
            }
        }
        if let Some(total) = x.mem_total.filter(|t| *t > 0.0) {
            let used = total - x.mem_avail.unwrap_or(0.0);
            meter(ui, "RAM", used / total * 100.0, &format!("{}/{} GB", gb(used), gb(total)), "System memory (shared with the GPU on unified-memory machines)");
        }
        if let (Some(load), Some(cpus)) = (x.load, x.cpus.filter(|c| *c > 0.0)) { meter(ui, "CPU", load / cpus * 100.0, &format!("{load:.1} / {cpus:.0}"), "1-minute load average / cores"); }
        if let Some(d) = x.disk.as_ref().filter(|d| d.total > 0.0) {
            // Used vs. total of the filesystem the home folder lives on; the bar follows what's left, which is what runs out.
            meter(ui, "DISK", (d.total - d.avail) / d.total * 100.0, &format!("{} GB free of {}", gb(d.avail), gb(d.total)), &format!("Filesystem of the home folder ({}): {} GB used", if d.mount.is_empty() { "/" } else { &d.mount }, gb(d.used)));
        }
        if let Some(n) = &x.net {
            // Throughput since the previous sample (5 s): the bar is log-scaled so 100 kB/s and 1 GB/s are both visible.
            let tot = n.rx_bps.unwrap_or(0.0) + n.tx_bps.unwrap_or(0.0);
            let pct = if tot > 0.0 { ((tot.log10() - 3.0) / 6.0 * 100.0).clamp(0.0, 59.0) } else { 0.0 }; // 1 kB/s -> 0%, 1 GB/s -> full; never "hot"
            meter(ui, "NET", pct, &format!("↓{} ↑{}", bps(n.rx_bps), bps(n.tx_bps)), &format!("Network in / out (all interfaces); total {} GB received, {} GB sent since boot", gb(n.rx), gb(n.tx)));
        }
    }

    pub fn machine_menu(&mut self, pos: egui::Pos2, machine_id: &str) {
        let m = self.machine(machine_id);
        let id = m.id.clone();
        let mut items = vec![MenuItem::Header(m.name.clone())];
        for p in self.cfg.presets_for(&m) { let (a, b) = (id.clone(), p.id.clone()); items.push(mi(format!("New {}", p.name), act(move |x| x.launch(&a, &b, Extra::default()))).icon(Icon::Preset(p.id.clone(), Some(id.clone())))); }
        items.push(MenuItem::Sep);
        items.push(MenuItem::Header("In a folder".into()));
        for p in self.cfg.presets_for(&m) { let (a, b) = (id.clone(), p.id.clone()); items.push(mi(format!("{} in folder…", p.name), act(move |x| x.launch_in_folder(&a, &b))).icon(Icon::Preset(p.id.clone(), Some(id.clone())))); }
        let agents: Vec<&Preset> = self.cfg.presets_for(&m).filter(|p| super::sessions::is_agent(&p.id)).collect();
        if !agents.is_empty() {
            items.push(MenuItem::Sep);
            items.push(MenuItem::Header("In a new git worktree".into()));
            for p in agents { let (a, b) = (id.clone(), p.id.clone()); items.push(mi(format!("{} on its own branch…", p.name), act(move |x| x.launch_worktree(&a, &b))).icon(Icon::Glyph("branch"))); }
        }
        if m.is_ssh() {
            items.push(MenuItem::Sep);
            let a = id.clone();
            if self.status_of(&m) == Status::Online { items.push(mi("Disconnect", act(move |x| x.disconnect_machine(&a)))); } else { items.push(mi("Connect", act(move |x| x.connect_machine(&a, false)))); }
            let (a, b, c, d) = (id.clone(), id.clone(), id.clone(), id.clone());
            items.push(mi("Set up SSH key…", act(move |x| x.setup_key(&a))).icon(Icon::Glyph("key")));
            items.push(mi("Install tools (tmux, Claude, Antigravity, OpenClaw)…", act(move |x| x.bootstrap(&b))).icon(Icon::Glyph("box")));
            items.push(mi("Enable Claude alerts", act(move |x| x.enable_alerts(&c))).icon(Icon::Glyph("bell")));
            let e = id.clone();
            items.push(mi("Replay recorded sessions…", act(move |x| x.open_replay(&e))).icon(Icon::Glyph("play")));
            let name = m.name.clone();
            items.push(mi("Join Tailscale (sudo tailscale up --ssh)…", act(move |x| x.launch(&d, "shell", Extra { raw: true, cmd: Some("sudo tailscale up --ssh && tailscale status | head -5".into()), name: Some(format!("Tailscale → {name}")), ..Default::default() }))).icon(Icon::Glyph("plug")));
            items.push(mi("Share over Tailscale…", act(App::open_share)).icon(Icon::Glyph("people")));
            items.push(MenuItem::Sep);
            items.push(MenuItem::Header("Port forwarding".into()));
            self.tunnels = self.be.tunnels();
            for t in self.tunnels.iter().filter(|t| t.machine_id == id) {
                let (url, tid) = (format!("http://localhost:{}/", t.local_port), t.id.clone());
                items.push(mi(format!("Open localhost:{} → {}{}", t.local_port, if t.remote_host == "127.0.0.1" { String::new() } else { format!("{}:", t.remote_host) }, t.remote_port), act(move |_| { crate::platform::open_external(&url); })).icon(Icon::Glyph("plug")));
                items.push(mi(format!("Stop forwarding localhost:{}", t.local_port), act(move |x| { x.be.tunnel_close(&tid); x.tunnels = x.be.tunnels(); x.toast("Port forwarding stopped", TEXT2, 3800); })).icon(Icon::Glyph("x")));
            }
            let a = id.clone();
            items.push(mi("Forward a port…", act(move |x| x.forward_port(&a))).icon(Icon::Glyph("plug")));
        }
        items.push(MenuItem::Sep);
        items.push(mi("Machine settings…", act(App::open_settings)).icon(Icon::Glyph("gear")));
        self.show_menu(pos, items);
    }

    // ---------------------------------------------------------------- welcome (no sessions yet)
    pub(super) fn draw_welcome(&mut self, ui: &mut egui::Ui) {
        let mut todo: Vec<Act> = Vec::new();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width().min(820.0);
            ui.vertical_centered(|ui| {
                ui.set_max_width(width);
                ui.add_space(40.0);
                ui.label(RichText::new("One terminal. Every agent.").size(28.0).strong());
                ui.label(RichText::new("Claude, Antigravity, OpenClaw and shells on every machine on your LAN, side by side.").color(TEXT2));
                ui.add_space(18.0);
                for m in self.machines_sorted() {
                    egui::Frame::new().fill(PANEL).stroke(egui::Stroke::new(1.0, LINE)).corner_radius(12).inner_margin(12).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            logo_img(ui, self.machine_icon(&m), 20.0);
                            ui.label(RichText::new(&m.name).strong());
                            dot(ui, self.status_of(&m));
                            let sub = if m.is_ssh() { if m.host.is_empty() { "not configured".to_string() } else { format!("{}{}", if m.user.is_empty() { String::new() } else { format!("{}@", m.user) }, m.host) } }
                                else if m.is_wsl() { format!("WSL · {}", m.distro) } else { format!("this computer · {}", os_label(&m.os)) };
                            ui.label(dim(sub));
                        });
                        ui.horizontal_wrapped(|ui| {
                            for p in self.cfg.presets_for(&m) {
                                let missing = self.is_missing(&m, p);
                                let resp = pill(ui, &self.preset_icon(&p.id, Some(&m.id)), &p.name, if missing { "not installed" } else { "" }, hex(&p.color), missing, 22.0, 13.5, Vec2::new(13.0, 10.0), true)
                                    .on_hover_text(format!("New {} on {} · Shift+click to pick a folder", p.name, m.name));
                                let (a, b) = (m.id.clone(), p.id.clone());
                                if resp.secondary_clicked() || (resp.clicked() && ui.input(|i| i.modifiers.shift)) { todo.push(act(move |x| x.launch_in_folder(&a, &b))); }
                                else if resp.clicked() { todo.push(act(move |x| x.launch(&a, &b, Extra::default()))); }
                            }
                        });
                    });
                    ui.add_space(8.0);
                }
                if let Some(u) = self.cfg.machines.iter().find(|m| m.is_ssh() && m.host.is_empty() && m.ts_host.is_empty()) {
                    egui::Frame::new().fill(tint(WARN, 0.1)).corner_radius(10).inner_margin(10).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&u.name).strong());
                            ui.label("needs an address before it can be used.");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ui.button("Set it up").clicked() { todo.push(act(App::open_settings)); } });
                        });
                    });
                }
                ui.add_space(14.0);
                egui::Grid::new("keys").num_columns(2).spacing([16.0, 7.0]).show(ui, |ui| {
                    for (k, what) in [
                        ("Ctrl+Shift+P", "Command palette: launch or switch anything"), ("Ctrl+1…9", "Jump to session"), ("Ctrl+Shift+L", "Cycle layout: single, split, grid"),
                        ("Ctrl+Shift+A", "Fleet: every agent at a glance, and answer the ones waiting for you"),
                        ("Ctrl+Shift+Enter", "Broadcast one prompt to many agents"), ("Ctrl+Shift+N", "Launch an agent in a project folder (or Shift+click a launcher)"),
                        ("Ctrl+Shift+O", "Workspaces: saved layouts of agents and folders"), ("Ctrl+Shift+S", "Send an agent's last reply to another agent"),
                        ("Ctrl+C / Ctrl+V", "Copy / paste text or screenshots; drop files onto a pane to attach them"),
                    ] { ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| key_hint(ui, k)); ui.label(RichText::new(what).color(TEXT2)); ui.end_row(); }
                });
                ui.add_space(30.0);
            });
        });
        for a in todo { a(self); }
    }
}
