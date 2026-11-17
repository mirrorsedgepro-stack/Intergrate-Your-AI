//! The panes: layout and dividers, pane headers, and the terminal itself (painting, mouse, wheel, find, context menu).
use super::overlays::{mi, Icon, MenuItem};
use super::sessions::is_agent;
use super::*;
use crate::term::MouseKind;
use crate::util::short_path;
use egui::{Pos2, Rect};

const PAD: f32 = 10.0;
const GUTTER: f32 = 8.0;
const HEAD: f32 = 32.0;

/// A web link under a column of a line of terminal text.
pub fn url_at(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let text: String = chars.iter().collect();
    for scheme in ["https://", "http://"] {
        let mut from = 0;
        while let Some(i) = text[from..].find(scheme) {
            let start_byte = from + i;
            let start = text[..start_byte].chars().count();
            let mut end = start;
            while end < chars.len() && !chars[end].is_whitespace() && !"<>\"'`│┃".contains(chars[end]) { end += 1; }
            while end > start && ".,;:)]}".contains(chars[end - 1]) { end -= 1; }
            if (start..end).contains(&col) && end - start > scheme.len() { return Some(chars[start..end].iter().collect()); }
            from = start_byte + scheme.len();
        }
    }
    None
}

impl App {
    /// Files dropped on the window go to the pane under the pointer (or the focused one).
    pub(super) fn handle_drops(&mut self) {
        let paths: Vec<std::path::PathBuf> = self.ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| p.is_absolute()).collect());
        if paths.is_empty() || self.overlay_open() { return; }
        let pos = self.ctx.input(|i| i.pointer.latest_pos());
        let rects = self.pane_rects(self.ctx.content_rect()); // close enough: only used to tell panes apart
        let target = pos.and_then(|p| rects.iter().position(|r| r.contains(p))).and_then(|i| self.panes.get(i).copied().flatten()).or(self.focused());
        if let Some(id) = target { self.attach_files(id, paths); }
    }

    fn pane_rects(&self, area: Rect) -> Vec<Rect> {
        let a = area.shrink(PAD);
        let x = a.left() + (a.width() - GUTTER) * self.split.0;
        let y = a.top() + (a.height() - GUTTER) * self.split.1;
        match self.layout {
            2 => vec![Rect::from_min_max(a.min, Pos2::new(x, a.bottom())), Rect::from_min_max(Pos2::new(x + GUTTER, a.top()), a.max)],
            4 => vec![
                Rect::from_min_max(a.min, Pos2::new(x, y)), Rect::from_min_max(Pos2::new(x + GUTTER, a.top()), Pos2::new(a.right(), y)),
                Rect::from_min_max(Pos2::new(a.left(), y + GUTTER), Pos2::new(x, a.bottom())), Rect::from_min_max(Pos2::new(x + GUTTER, y + GUTTER), a.max),
            ],
            _ => vec![a],
        }
    }

    pub(super) fn draw_panes(&mut self, ui: &mut egui::Ui) {
        self.normalize_panes();
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        let rects = self.pane_rects(area);
        // Dividers: drag to resize, double-click to reset.
        if self.layout > 1 {
            let a = area.shrink(PAD);
            let gutter = |app: &mut App, ui: &mut egui::Ui, vertical: bool| {
                let r = if vertical { Rect::from_min_max(Pos2::new(rects[0].right(), a.top()), Pos2::new(rects[0].right() + GUTTER, a.bottom())) } else { Rect::from_min_max(Pos2::new(a.left(), rects[0].bottom()), Pos2::new(a.right(), rects[0].bottom() + GUTTER)) };
                let resp = ui.interact(r, egui::Id::unique(("gutter", vertical)), egui::Sense::click_and_drag()).on_hover_text("Drag to resize · double-click to reset");
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
                    ui.painter().rect_filled(r.shrink2(if vertical { Vec2::new(3.0, 0.0) } else { Vec2::new(0.0, 3.0) }), 1.0, LINE2);
                }
                if let (true, Some(p)) = (resp.dragged(), resp.interact_pointer_pos()) {
                    let f = if vertical { (p.x - a.left()) / a.width() } else { (p.y - a.top()) / a.height() }.clamp(0.15, 0.85);
                    if vertical { app.split.0 = f; } else { app.split.1 = f; }
                }
                if resp.double_clicked() { if vertical { app.split.0 = 0.5; } else { app.split.1 = 0.5; } }
                if resp.drag_stopped() || resp.double_clicked() { app.persist_soon(); }
            };
            gutter(self, ui, true);
            if self.layout == 4 { gutter(self, ui, false); }
        }
        for (i, rect) in rects.into_iter().enumerate() { self.draw_pane(ui, i, rect); }
    }

    fn draw_pane(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect) {
        let sid = self.panes.get(i).copied().flatten();
        let focused = i == self.focus;
        let color = sid.and_then(|id| self.sessions.get(&id)).map_or(TEXT3, |s| s.color);
        ui.painter().rect(rect, 10.0, crate::term::BG, egui::Stroke::new(1.0, if focused && self.layout > 1 { tint(color, 0.7) } else { LINE }), egui::StrokeKind::Inside);
        let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEAD));
        let body = Rect::from_min_max(Pos2::new(rect.left() + 8.0, rect.top() + HEAD), Pos2::new(rect.right() - 6.0, rect.bottom() - 6.0));
        ui.painter().hline(rect.x_range(), head.bottom(), egui::Stroke::new(1.0, LINE));
        let Some(id) = sid else {
            ui.painter().text(head.left_center() + Vec2::new(10.0, 0.0), egui::Align2::LEFT_CENTER, "-", FontId::proportional(12.0), TEXT3);
            ui.painter().text(body.center(), egui::Align2::CENTER_CENTER, format!("Empty pane. Pick a session in the sidebar or press {}", kbd("Ctrl+Shift+P")), FontId::proportional(13.0), TEXT3);
            if ui.interact(rect, egui::Id::unique(("empty-pane", i)), egui::Sense::click()).clicked() { self.focus_pane(i); }
            return;
        };
        self.pane_head(ui, i, id, head);
        self.pane_terminal(ui, i, id, body);
        self.pane_search(ui, id, body);
    }

    fn pane_head(&mut self, ui: &mut egui::Ui, i: usize, id: u64, head: Rect) {
        let mut todo: Option<Act> = None;
        let bg = ui.interact(head, egui::Id::unique(("pane-head", i)), egui::Sense::click());
        ui.scope_builder(egui::UiBuilder::new().max_rect(head.shrink2(Vec2::new(10.0, 0.0))), |ui| {
            let Some(s) = self.sessions.get(&id) else { return };
            let m = self.machine(&s.machine_id);
            ui.horizontal_centered(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if icon_btn(ui, "x", &format!("{} (Ctrl+Shift+W)", if s.tmux_name.is_some() { "Detach (keeps running)" } else { "Close" })).clicked() { todo = Some(act(move |a| a.close_session(id, false))); }
                    let r = icon_btn(ui, "more", "More");
                    if r.clicked() { let pos = r.rect.left_bottom() + Vec2::new(0.0, 4.0); todo = Some(act(move |a| a.term_menu(pos, id))); }
                    let single = self.layout == 1;
                    if icon_btn(ui, "split", if single { "Split side by side" } else { "Move to next pane" }).clicked() { todo = Some(act(move |a| if single { a.split_with(id) } else { a.move_to_next_pane(id) })); }
                    if icon_btn(ui, "search", "Find (Ctrl+Shift+F)").clicked() { todo = Some(act(move |a| a.open_search(id))); }
                    if is_agent(&s.preset_id) && icon_btn(ui, "send", "Send last reply to another agent (Ctrl+Shift+S)").clicked() { todo = Some(act(move |a| a.send_last_reply(id))); }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        // A narrow pane cuts the details off rather than drawing them under the buttons.
                        ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                        logo_img(ui, &self.preset_icon(&s.preset_id, Some(&s.machine_id)), 15.0);
                        ui.label(RichText::new(&s.name).strong());
                        chip(ui, &m.name, hex(&m.color));
                        if let Some(c) = s.cwd.as_deref().filter(|c| *c != "~") { chip(ui, &short_path(c), TEXT2).on_hover_text(c); }
                        if s.read_only { chip(ui, "read-only", BRAND2); }
                        if let (Some(a), Activity::Done) = (&s.agent, s.activity) {
                            let ask = a.event == "Notification";
                            let full = if ask { &a.message } else { &a.summary };
                            let short = crate::util::clip(if full.is_empty() { if ask { "needs input" } else { "finished" } } else { full }, 60);
                            chip(ui, &format!("{} {short}", if ask { "⚠" } else { "✓" }), if ask { WARN } else { OK }).on_hover_text(full);
                        }
                        match s.status {
                            SessStatus::Disconnected => { chip(ui, if s.retry > 0 { "reconnecting…" } else { "disconnected" }, WARN); }
                            SessStatus::Exited => { chip(ui, "exited", TEXT3); }
                            SessStatus::Starting => { chip(ui, "starting…", TEXT3); }
                            SessStatus::Running => {}
                        }
                        let r = ui.add(egui::Label::new(dim(&s.title).size(12.0)).truncate());
                        if let Some(t) = &s.tmux_name { r.on_hover_text(format!("tmux session {t}")); }
                    });
                });
            });
        });
        if bg.clicked() { self.focus_pane(i); }
        if bg.double_clicked() { self.focus_pane(i); self.set_layout(if self.layout == 1 { 2 } else { 1 }); }
        if let Some(a) = todo { a(self); }
    }

    fn pane_terminal(&mut self, ui: &mut egui::Ui, i: usize, id: u64, body: Rect) {
        let cell = self.font.cell;
        if cell.x <= 0.0 || body.width() < cell.x * 2.0 || body.height() < cell.y { return; }
        let cols = ((body.width() / cell.x).floor() as u16).clamp(2, 1000);
        let rows = ((body.height() / cell.y).floor() as u16).clamp(1, 500);
        let rect = Rect::from_min_size(body.min, Vec2::new(f32::from(cols) * cell.x, f32::from(rows) * cell.y));
        let is_focus = i == self.focus;
        let now = now_ms();
        let widget = egui::Id::unique(("term", i));
        let resp = ui.interact(body, widget, egui::Sense::click_and_drag());
        let owns_kb = is_focus && !self.overlay_open() && self.menu.is_none() && self.win_focused && !self.text_has_focus();
        if owns_kb {
            // Tab, arrows and Escape belong to the program in the terminal, not to moving focus between widgets.
            resp.request_focus();
            ui.memory_mut(|m| m.set_focus_lock_filter(widget, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true }));
        }
        let (pos, pressed, down, released, mods, wheel) = ui.input(|inp| (inp.pointer.latest_pos(), inp.pointer.primary_pressed(), inp.pointer.primary_down(), inp.pointer.primary_released(), inp.modifiers, inp.smooth_scroll_delta.y));
        let over = resp.contains_pointer();
        if over { ui.ctx().set_cursor_icon(egui::CursorIcon::Text); }
        let mut out: Vec<Vec<u8>> = Vec::new();
        let (mut copy_on_select, mut open_url, mut tip, mut menu_at) = (false, None, false, None);
        let mut resized = None;
        if let Some(s) = self.sessions.get_mut(&id) {
            if s.term.resize(cols, rows) { s.quiet_until = now + 1200; resized = s.pid; }
            if let Some(p) = pos {
                let (col, row, side) = s.term.cell_at(rect, cell, Pos2::new(p.x.clamp(rect.left(), rect.right() - 1.0), p.y.clamp(rect.top(), rect.bottom() - 1.0)));
                let app_mouse = s.term.mouse_mode() && !mods.shift;
                if pressed && over {
                    s.last_pointer = now;
                    if app_mouse {
                        // The program owns the mouse (Claude's full-screen UI): plain clicks and drags go to it.
                        out.extend(s.term.mouse_report(MouseKind::Press, 0, col, row, mods));
                        s.mouse_cell = Some((col, row));
                        tip = true;
                    } else if mods.command { open_url = url_at(&s.term.visible_line(row), col as usize); }
                    else {
                        let count = if now - s.click.0 < 400 && s.click.1 == (col, row) { s.click.2 % 3 + 1 } else { 1 };
                        s.click = (now, (col, row), count);
                        s.term.start_selection(col, row, side, count);
                    }
                } else if down && s.mouse_cell.is_some() {
                    if s.mouse_cell != Some((col, row)) { out.extend(s.term.mouse_report(MouseKind::Drag, 0, col, row, mods)); s.mouse_cell = Some((col, row)); }
                } else if down && s.term.is_dragging() {
                    s.term.drag_selection(col, row, side);
                    // Dragging past the edge scrolls into the scrollback.
                    if p.y < rect.top() { s.term.scroll(1); } else if p.y > rect.bottom() { s.term.scroll(-1); }
                    ui.ctx().request_repaint();
                }
                if released {
                    if s.mouse_cell.take().is_some() { out.extend(s.term.mouse_report(MouseKind::Release, 0, col, row, mods)); s.last_pointer = now; }
                    else if s.term.is_dragging() { s.term.end_selection(); s.last_pointer = now; copy_on_select = s.term.has_selection(); }
                }
                if over && wheel != 0.0 {
                    s.scroll_acc += wheel;
                    let lines = (s.scroll_acc / cell.y).trunc() as i32;
                    if lines != 0 {
                        s.scroll_acc -= lines as f32 * cell.y;
                        if app_mouse { for _ in 0..lines.unsigned_abs().min(20) { out.extend(s.term.mouse_report(MouseKind::Press, if lines > 0 { 64 } else { 65 }, col, row, mods)); } }
                        else if let Some(keys) = s.term.wheel_as_arrows(lines) { out.push(keys); }
                        else { s.term.scroll(lines); }
                    }
                }
            } else if released { s.mouse_cell = None; s.term.end_selection(); }
            if resp.secondary_clicked() { s.last_pointer = now; menu_at = resp.interact_pointer_pos(); }

            let blink_on = now - s.last_input < 600 || (now / 530) % 2 == 0;
            s.term.paint(&ui.painter_at(body), rect, &self.font.id, self.font.bold.as_ref(), cell, self.font.row_h, owns_kb, s.color, blink_on);
            if owns_kb {
                // Input methods (Chinese, Japanese, Korean, dead keys…): tell the system where the cursor is so its
                // candidate window sits there, and show the text being composed until it is committed.
                if let Some((col, row)) = s.term.cursor_cell() {
                    let at = rect.min + Vec2::new(f32::from(col) * cell.x, f32::from(row) * cell.y);
                    let cur = Rect::from_min_size(at, Vec2::new(1.0, cell.y));
                    ui.ctx().output_mut(|o| o.ime = Some(egui::output::IMEOutput { purpose: egui::IMEPurpose::Terminal, rect: body, cursor_rect: cur, should_interrupt_composition: false }));
                    if !self.preedit.is_empty() {
                        let g = ui.painter().layout_no_wrap(self.preedit.clone(), self.font.id.clone(), crate::term::FG);
                        let r = Rect::from_min_size(at, Vec2::new(g.size().x, cell.y));
                        ui.painter().rect_filled(r, 0.0, PANEL3);
                        ui.painter().hline(r.x_range(), r.bottom() - 1.0, egui::Stroke::new(1.0, s.color));
                        ui.painter().galley(at, g, crate::term::FG);
                    }
                }
            }
            if s.term.scrolled_back() { ui.painter().text(body.right_bottom() - Vec2::new(6.0, 4.0), egui::Align2::RIGHT_BOTTOM, "↑ scrollback", FontId::proportional(10.5), TEXT3); }
        }
        if is_focus { self.last_cells = (cols, rows); }
        if let Some(pid) = resized { self.be.resize(pid, cols, rows); }
        if pressed && over { self.focus_pane(i); self.menu = None; self.focus_terminal(); }
        let sent = !out.is_empty();
        for b in out { if let Some(pid) = self.sessions.get(&id).and_then(|s| s.pid) { self.be.write(pid, &b); } }
        if sent { if let Some(s) = self.sessions.get_mut(&id) { s.last_input = now; } }
        if tip && !self.mouse_tip_shown {
            // Plain drags go to the program; Shift+drag still selects in Nexus. Say so once.
            self.mouse_tip_shown = true;
            let name = self.sessions.get(&id).map(|s| s.name.clone()).unwrap_or_default();
            self.toast(format!("{name} is using the mouse: select text in it to copy, or hold Shift and drag to select in Nexus."), TEXT2, 6000);
        }
        // Links in terminal output open on Ctrl/Cmd+click only: output comes from remote machines and agents, and a stray
        // click shouldn't open a page someone else chose.
        if let Some(url) = open_url { crate::platform::open_external(&url); }
        if copy_on_select && self.cfg.appearance.copy_on_select { if let Some(t) = self.sessions.get(&id).and_then(|s| s.term.selection_text()) { self.ctx.copy_text(t); } }
        if let Some(p) = menu_at { self.focus_pane(i); self.term_menu(p, id); }
    }

    // ---------------------------------------------------------------- find in scrollback
    pub fn open_search(&mut self, id: u64) {
        if !self.is_visible(id) { self.show(id); }
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let sel = s.term.selection_text().filter(|t| !t.contains('\n'));
        if s.search.is_none() || sel.is_some() { s.search = Some(sel.unwrap_or_default()); }
        s.search_focus = true;
    }
    fn pane_search(&mut self, ui: &mut egui::Ui, id: u64, body: Rect) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        let Some(mut q) = s.search.take() else { return };
        let (mut find, mut close) = (None::<bool>, false);
        let r = Rect::from_min_size(Pos2::new(body.right() - 330.0, body.top() + 6.0), Vec2::new(322.0, 32.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
            egui::Frame::new().fill(PANEL2).stroke(egui::Stroke::new(1.0, LINE2)).corner_radius(8).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let te = egui::Id::unique(("search", id));
                    if ui.memory(|m| m.has_focus(te)) {
                        ui.input_mut(|i| {
                            if i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter) || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) { find = Some(false); }
                            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) { close = true; }
                        });
                        if find.is_some() && ui.input(|i| i.modifiers.shift) { find = Some(true); }
                    }
                    let resp = ui.add(egui::TextEdit::singleline(&mut q).id(te).hint_text("Find in scrollback").desired_width(200.0));
                    if std::mem::take(&mut s.search_focus) { resp.request_focus(); }
                    if resp.changed() { s.term.clear_selection(); find = Some(false); }
                    if icon_btn(ui, "up", "Previous").clicked() { find = Some(false); }
                    if icon_btn(ui, "down", "Next").clicked() { find = Some(true); }
                    if icon_btn(ui, "x", "Close").clicked() { close = true; }
                });
            });
        });
        // Searching runs from the newest output upwards, like scrolling back.
        if let Some(forward) = find { s.term.find(&q, forward); }
        if close { s.term.clear_selection(); self.focus_terminal(); } else { s.search = Some(q); }
    }

    // ---------------------------------------------------------------- context menu
    pub fn term_menu(&mut self, pos: Pos2, id: u64) {
        let Some(s) = self.sessions.get(&id) else { return };
        let sel = s.term.selection_text();
        let has_sel = sel.is_some();
        let mut items = vec![
            mi("Copy", act(move |a| { a.copy_selection(id); })).kbd("Ctrl+C").disabled(!has_sel),
            mi("Paste", act(move |a| { a.ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste); a.show(id); })).kbd("Ctrl+V"),
            mi("Select all", act(move |a| { if let Some(s) = a.sessions.get_mut(&id) { s.term.select_all(); } })),
            mi("Find…", act(move |a| a.open_search(id))).kbd("Ctrl+Shift+F"),
            mi("Send last reply to…", act(move |a| a.send_last_reply(id))).kbd("Ctrl+Shift+S").icon(Icon::Glyph("send")),
            mi("Clear scrollback", act(move |a| { if let Some(s) = a.sessions.get_mut(&id) { s.term.clear_scrollback(); } })),
        ];
        let others: Vec<&Session> = self.ordered().into_iter().filter(|o| *o != id).filter_map(|o| self.sessions.get(&o)).filter(|o| o.status == SessStatus::Running).collect();
        if !others.is_empty() {
            items.push(MenuItem::Sep);
            items.push(MenuItem::Header(if has_sel { "Send selection to".into() } else { "Send selection to (select text first)".into() }));
            for o in others {
                let (oid, text, name) = (o.id, sel.clone().unwrap_or_default(), o.name.clone());
                items.push(mi(format!("{} · {}", o.name, self.machine(&o.machine_id).name), act(move |a| { a.paste_text(oid, &text); a.toast_ok(format!("Sent {} chars to {name}", text.chars().count())); }))
                    .icon(Icon::Preset(o.preset_id.clone(), Some(o.machine_id.clone()))).disabled(!has_sel));
            }
        }
        items.push(MenuItem::Sep);
        items.push(if self.layout == 1 { mi("Open side by side", act(move |a| a.split_with(id))).kbd("Ctrl+Shift+D").icon(Icon::Glyph("split")) } else { mi("Move to next pane", act(move |a| a.move_to_next_pane(id))).icon(Icon::Glyph("split")) });
        if s.tmux_name.is_some() {
            items.push(mi("Detach (keeps running)", act(move |a| a.close_session(id, false))).kbd("Ctrl+Shift+W"));
            items.push(mi("Kill session", act(move |a| a.close_session(id, true))).danger());
        } else { items.push(mi("Close", act(move |a| a.close_session(id, false))).kbd("Ctrl+Shift+W").danger()); }
        self.show_menu(pos, items);
    }
}

#[cfg(test)]
mod tests {
    use super::url_at;

    #[test]
    fn links_under_the_pointer() {
        let line = "see https://example.com/docs?a=1), then http://localhost:8080/ │";
        assert_eq!(url_at(line, 6).as_deref(), Some("https://example.com/docs?a=1"));
        assert_eq!(url_at(line, 45).as_deref(), Some("http://localhost:8080/"));
        assert_eq!(url_at(line, 1), None);
        assert_eq!(url_at("file:///etc/passwd", 3), None);
    }
}
