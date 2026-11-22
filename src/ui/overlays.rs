//! Things that float over the panes: command palette and pickers, prompts, settings, the broadcast composer,
//! context menus and toasts. Also the keyboard: app shortcuts first, then the focused terminal.
use super::sessions::is_agent;
use super::*;
use crate::util::{base_name, fuzzy};
use egui::{Key, Modifiers};

// ---------------------------------------------------------------- palette
pub enum Icon { Preset(String, Option<String>), Glyph(&'static str) }
pub struct PalItem {
    pub group: &'static str,
    pub label: String,
    pub sub: String,
    pub kbd: String,
    pub icon: Icon,
    pub color: Option<Color32>,
    pub missing: bool,
    /// The text the user typed, offered as-is (a folder path).
    pub exact: bool,
    pub run: Act,
}
#[derive(Clone, PartialEq)]
pub enum PalSource { Main, Workspaces, Folder { machine_id: String, preset_id: String, worktree: bool }, Queue, Prompts }
pub struct Palette { input: String, sel: usize, source: PalSource, focus: bool, scroll: bool }

// ---------------------------------------------------------------- dialogs
pub enum DialogKind { Choose(Vec<(String, bool)>), Prompt { label: String, placeholder: String, value: String, password: bool } }
pub struct Dialog { title: String, desc: String, kind: DialogKind, on: Option<Box<dyn FnOnce(&mut App, DialogResult)>>, focus: bool }
pub enum DialogResult { Choice(Option<usize>), Text(Option<String>) }

pub struct Settings { pub draft: Config, probe_out: HashMap<String, Result<(String, Vec<(String, bool)>), String>> }
pub struct Composer { text: String, submit: bool, focus: bool }

pub enum MenuItem { Sep, Header(String), Item { label: String, kbd: String, icon: Option<Icon>, danger: bool, disabled: bool, run: Act } }
pub struct Menu { pub pos: egui::Pos2, pub items: Vec<MenuItem>, opened: i64 }
pub fn mi(label: impl Into<String>, run: Act) -> MenuItem { MenuItem::Item { label: label.into(), kbd: String::new(), icon: None, danger: false, disabled: false, run } }
impl MenuItem {
    pub fn kbd(mut self, k: &str) -> Self { if let Self::Item { kbd, .. } = &mut self { *kbd = k.into(); } self }
    pub fn icon(mut self, i: Icon) -> Self { if let Self::Item { icon, .. } = &mut self { *icon = Some(i); } self }
    pub fn danger(mut self) -> Self { if let Self::Item { danger, .. } = &mut self { *danger = true; } self }
    pub fn disabled(mut self, d: bool) -> Self { if let Self::Item { disabled, .. } = &mut self { *disabled = d; } self }
}

/// A dialog: the background colour with a hairline border and a soft shadow, over the dimmed window.
pub(super) fn modal_frame() -> egui::Frame {
    egui::Frame::new().fill(Color32::from_rgb(0x0f, 0x0f, 0x0f)).stroke(egui::Stroke::new(1.0, LINE2)).corner_radius(12).inner_margin(20)
        .shadow(egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(160) })
}
pub(super) fn section(ui: &mut egui::Ui, title: &str) { ui.add_space(14.0); ui.label(RichText::new(title).size(13.0).family(medium()).color(TEXT)); ui.add_space(3.0); }
fn field(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) { ui.vertical(|ui| { ui.spacing_mut().item_spacing.y = 4.0; ui.label(RichText::new(label).size(11.5).color(TEXT2)); add(ui); }); }

impl App {
    /// Is the keyboard in one of the text boxes beside the panes (composer, team chat, find)? Otherwise it is the
    /// focused terminal's, whatever button was clicked last.
    pub fn text_has_focus(&self) -> bool {
        let Some(f) = self.ctx.memory(|m| m.focused()) else { return false };
        f == egui::Id::unique("composer-text") || f == egui::Id::unique("team-input") || self.panes.iter().flatten().any(|id| f == egui::Id::unique(("search", *id)))
    }
    pub fn overlay_open(&self) -> bool { !self.dialogs.is_empty() || self.palette.is_some() || self.settings.is_some() || self.share.is_some() || self.fleet.is_some() || self.replay.is_some() }

    // ---------------------------------------------------------------- dialogs
    pub fn choose(&mut self, title: &str, desc: &str, options: &[(&str, bool)], on: impl FnOnce(&mut App, Option<usize>) + 'static) {
        self.menu = None;
        self.dialogs.push(Dialog { title: title.into(), desc: desc.into(), kind: DialogKind::Choose(options.iter().map(|(l, p)| ((*l).to_string(), *p)).collect()), focus: true,
            on: Some(Box::new(move |app, r| on(app, if let DialogResult::Choice(c) = r { c } else { None }))) });
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prompt(&mut self, title: &str, desc: &str, label: &str, placeholder: &str, value: &str, password: bool, on: impl FnOnce(&mut App, Option<String>) + 'static) {
        self.menu = None;
        self.dialogs.push(Dialog { title: title.into(), desc: desc.into(), kind: DialogKind::Prompt { label: label.into(), placeholder: placeholder.into(), value: value.into(), password }, focus: true,
            on: Some(Box::new(move |app, r| on(app, if let DialogResult::Text(t) = r { t } else { None }))) });
    }
    fn draw_dialog(&mut self) {
        let Some(mut d) = self.dialogs.pop() else { return };
        let mut result: Option<DialogResult> = None;
        let esc = self.ctx.input(|i| i.key_pressed(Key::Escape));
        egui::Modal::new(egui::Id::unique(("dialog", self.dialogs.len()))).frame(modal_frame()).show(&self.ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(&d.title).size(16.0).family(medium()));
            ui.add_space(6.0);
            if !d.desc.is_empty() { ui.label(RichText::new(&d.desc).color(TEXT2)); ui.add_space(8.0); }
            match &mut d.kind {
                DialogKind::Prompt { label, placeholder, value, password } => {
                    ui.label(RichText::new(label.as_str()).size(11.0).color(TEXT2));
                    let r = ui.add(egui::TextEdit::singleline(value).hint_text(placeholder.as_str()).password(*password).desired_width(f32::INFINITY));
                    if std::mem::take(&mut d.focus) { r.request_focus(); }
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_btn(ui, "OK").clicked() || enter { result = Some(DialogResult::Text(Some(value.clone()).filter(|v| !v.trim().is_empty()))); }
                        if ui.button("Cancel").clicked() { result = Some(DialogResult::Text(None)); }
                    });
                }
                DialogKind::Choose(options) => {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (i, (label, primary)) in options.iter().enumerate().rev() {
                            // The confirming button of something that can't be undone is the destructive variant.
                            let destroys = ["remove", "delete", "kill", "discard", "revoke", "forget", "close "].iter().any(|w| label.to_lowercase().starts_with(w));
                            let clicked = if *primary && destroys { danger_btn(ui, label).clicked() } else if *primary { primary_btn(ui, label).clicked() } else { ui.button(label.as_str()).clicked() };
                            if clicked { result = Some(DialogResult::Choice(Some(i))); }
                        }
                    });
                }
            }
        });
        if esc && result.is_none() { result = Some(if matches!(d.kind, DialogKind::Prompt { .. }) { DialogResult::Text(None) } else { DialogResult::Choice(None) }); }
        match result {
            Some(r) => { if let Some(on) = d.on.take() { on(self, r); } if !self.overlay_open() { self.focus_terminal(); } }
            None => self.dialogs.push(d),
        }
    }

    // ---------------------------------------------------------------- palette
    pub fn open_palette(&mut self, prefix: &str) { self.open_picker(PalSource::Main, prefix); }
    pub fn open_picker(&mut self, source: PalSource, prefix: &str) {
        self.menu = None;
        self.palette = Some(Palette { input: prefix.into(), sel: 0, source, focus: true, scroll: false });
    }
    pub fn open_workspaces(&mut self) { self.open_picker(PalSource::Workspaces, ""); }

    /// Launch in a project folder: recent folders, git projects and top-level folders on that machine, or any typed path.
    pub fn launch_in_folder(&mut self, machine_id: &str, preset_id: &str) { self.pick_launch_folder(machine_id, preset_id, false); }
    /// Like launch_in_folder, but the agent gets its own git worktree of the chosen repository.
    pub fn launch_worktree(&mut self, machine_id: &str, preset_id: &str) { self.pick_launch_folder(machine_id, preset_id, true); }
    fn pick_launch_folder(&mut self, machine_id: &str, preset_id: &str, worktree: bool) {
        let m = self.machine(machine_id);
        if m.is_ssh() && m.host.is_empty() && m.ts_host.is_empty() { self.open_settings(); return; }
        self.open_picker(PalSource::Folder { machine_id: machine_id.into(), preset_id: preset_id.into(), worktree }, "");
        if self.dir_cache.get(machine_id).is_some_and(|(at, _)| now_ms() - at < 60_000) { return; }
        let (be, mid) = (self.be.clone(), machine_id.to_string());
        let mid2 = mid.clone();
        self.be.run(async move { be.dirs(&mid).await }, move |app, r| match r {
            Ok(d) => { app.dir_cache.insert(mid2, (now_ms(), d)); }
            Err(e) => app.toast_bad(e),
        });
    }
    fn pick_folder(&mut self, machine_id: &str, preset_id: &str, cwd: &str, worktree: bool) {
        let list = self.cfg.recent_dirs.entry(machine_id.to_string()).or_default();
        list.retain(|d| d != cwd);
        list.insert(0, cwd.to_string());
        list.truncate(12);
        self.save_now();
        if worktree { self.launch_in_worktree(machine_id, preset_id, cwd); } else { self.launch(machine_id, preset_id, Extra { cwd: Some(cwd.into()), ..Default::default() }); }
    }

    // ---------------------------------------------------------------- prompt library
    pub fn save_prompt(&mut self, text: String) {
        if text.trim().is_empty() { self.toast_warn("Write the prompt first"); return; }
        self.prompt("Save prompt", "Saved prompts are in the palette (type \"prompt\") and fill the broadcast box when picked.", "Name", "e.g. Review this branch", "", false, move |app, name| {
            let Some(name) = name.map(|n| crate::util::clip(n.trim(), 60)) else { return };
            app.cfg.prompts.retain(|p| p.name != name);
            app.cfg.prompts.push(crate::config::SavedPrompt { name: name.clone(), text: text.clone() });
            app.save_now();
            app.toast_ok(format!("Saved prompt \"{name}\""));
        });
    }
    fn delete_prompt(&mut self, name: &str) {
        self.cfg.prompts.retain(|p| p.name != name);
        self.save_now();
        self.toast(format!("Deleted prompt \"{name}\""), TEXT2, 3800);
    }
    fn prompt_items(&self, group: &'static str, items: &mut Vec<PalItem>) {
        for p in &self.cfg.prompts {
            let text = p.text.clone();
            let sub = crate::util::clip(&p.text.split_whitespace().collect::<Vec<_>>().join(" "), 90);
            items.push(PalItem { group, label: format!("Prompt: {}", p.name), sub, kbd: String::new(), icon: Icon::Glyph("book"), color: None, missing: false, exact: false, run: act(move |app| app.open_composer(Some(text.clone()))) });
        }
    }

    fn palette_items(&self, source: &PalSource) -> (String, Vec<PalItem>) {
        let item = |group, label: String, sub: String, kbd: &str, icon, color, run| PalItem { group, label, sub, kbd: kbd.into(), icon, color, missing: false, exact: false, run };
        let mut items = Vec::new();
        match source {
            PalSource::Queue => return (if self.queue.is_empty() { "Nothing is queued. Launching on a claimed machine offers \"Start when free\".".into() } else { "Queued launches (select to cancel)…".into() }, self.queue_items()),
            PalSource::Workspaces => {
                let ws = &self.cfg.workspaces;
                for w in ws {
                    let (name, snap) = (w.name.clone().unwrap_or_default(), w.clone());
                    let sub = w.sessions.iter().map(|s| format!("{}@{}{}", self.preset(&s.preset_id).name, self.machine(&s.machine_id).name, s.cwd.as_ref().map(|c| format!(" {}", base_name(c))).unwrap_or_default())).collect::<Vec<_>>().join(", ");
                    items.push(item("Workspaces", name, sub, "", Icon::Glyph("layout"), None, act(move |app| app.open_layout(&snap, false))));
                }
                items.push(item("Manage", "Save current layout as workspace…".into(), String::new(), "", Icon::Glyph("save"), None, act(App::save_workspace)));
                items.push(item("Manage", "Export workspaces and presets to a file…".into(), String::new(), "", Icon::Glyph("send"), None, act(App::export_workspaces)));
                items.push(item("Manage", "Import workspaces and presets from a file…".into(), String::new(), "", Icon::Glyph("folder"), None, act(App::import_workspaces)));
                for w in ws { let name = w.name.clone().unwrap_or_default(); let n2 = name.clone(); items.push(item("Manage", format!("Delete workspace \"{name}\""), String::new(), "", Icon::Glyph("trash"), None, act(move |app| app.delete_workspace(&n2)))); }
                return (if ws.is_empty() { "No workspaces yet. Arrange your panes, then save one".into() } else { "Open a workspace…".into() }, items);
            }
            PalSource::Prompts => {
                self.prompt_items("Prompts", &mut items);
                for p in &self.cfg.prompts { let n = p.name.clone(); items.push(item("Manage", format!("Delete prompt \"{}\"", p.name), String::new(), "", Icon::Glyph("trash"), None, act(move |app| app.delete_prompt(&n)))); }
                return (if self.cfg.prompts.is_empty() { "No saved prompts yet. Write one in the broadcast box and choose \"Save as prompt\"".into() } else { "Pick a prompt to put in the broadcast box…".into() }, items);
            }
            PalSource::Folder { machine_id, preset_id, worktree } => {
                let (m, p) = (self.machine(machine_id), self.preset(preset_id));
                let wt = *worktree;
                let mut seen = HashSet::new();
                let mut add = |items: &mut Vec<PalItem>, group, d: &str| {
                    if !seen.insert(d.to_string()) { return; }
                    let (mid, pid, dir) = (machine_id.clone(), preset_id.clone(), d.to_string());
                    items.push(item(group, d.to_string(), String::new(), "", Icon::Glyph(if wt { "branch" } else { "folder" }), None, act(move |app| app.pick_folder(&mid, &pid, &dir, wt))));
                };
                for d in self.cfg.recent_dirs.get(machine_id).into_iter().flatten() { add(&mut items, "Recent", d); }
                let data = self.dir_cache.get(machine_id).map(|x| x.1.clone()).unwrap_or_default();
                for d in &data.repos { add(&mut items, "Projects (git)", d); }
                if wt { return (format!("Repository to branch off for {} on {}. Type a path and press Enter to use it", p.name, m.name), items); }
                add(&mut items, "Folders", "~");
                for d in &data.dirs { add(&mut items, "Folders", d); }
                return (format!("Folder for {} on {}. Type a path and press Enter to use it", p.name, m.name), items);
            }
            PalSource::Main => {}
        }
        for (i, id) in self.ordered().into_iter().enumerate() {
            let s = &self.sessions[&id];
            let sub = format!("{}{}", self.machine(&s.machine_id).name, if s.title.is_empty() { String::new() } else { format!(" · {}", s.title) });
            items.push(item("Sessions", s.name.clone(), sub, &if i < 9 { format!("Ctrl+{}", i + 1) } else { String::new() }, Icon::Preset(s.preset_id.clone(), Some(s.machine_id.clone())), Some(s.color), act(move |app| app.show(id))));
        }
        let ms = self.machines_sorted();
        for (group, folder) in [("Launch", false), ("Launch in folder", true)] {
            for m in &ms {
                for p in self.cfg.presets_for(m) {
                    let missing = self.is_missing(m, p);
                    let (mid, pid) = (m.id.clone(), p.id.clone());
                    let mut it = item(group, if folder { format!("{} in folder…", p.name) } else { format!("New {}", p.name) }, format!("on {}{}", m.name, if missing { " · not installed" } else { "" }), "",
                        Icon::Preset(p.id.clone(), Some(m.id.clone())), Some(hex(&p.color)), act(move |app| if folder { app.launch_in_folder(&mid, &pid) } else { app.launch(&mid, &pid, Extra::default()) }));
                    it.missing = missing;
                    items.push(it);
                }
            }
        }
        for m in &ms {
            for p in self.cfg.presets_for(m).filter(|p| is_agent(&p.id)) {
                let (mid, pid) = (m.id.clone(), p.id.clone());
                items.push(item("Launch in a new git worktree", format!("{} in a new worktree…", p.name), format!("on {}", m.name), "", Icon::Preset(p.id.clone(), Some(m.id.clone())), Some(hex(&p.color)), act(move |app| app.launch_worktree(&mid, &pid))));
            }
        }
        self.prompt_items("Prompts", &mut items);
        for w in &self.cfg.workspaces {
            let snap = w.clone();
            items.push(item("Workspaces", format!("Open workspace {}", w.name.clone().unwrap_or_default()), format!("{} sessions", w.sessions.len()), "", Icon::Glyph("layout"), None, act(move |app| app.open_layout(&snap, false))));
        }
        let open: HashSet<&String> = self.sessions.values().filter_map(|s| s.tmux_name.as_ref()).collect();
        for m in &ms {
            for d in self.mst(&m.id).detached.iter().filter(|d| !open.contains(&d.name)) {
                let pid = sessions::preset_from_tmux(&d.name);
                let (mid, name) = (m.id.clone(), d.name.clone());
                items.push(item("Detached", format!("Reattach {}", d.name), format!("on {}", m.name), "", Icon::Preset(pid.clone(), Some(m.id.clone())), Some(hex(&self.preset(&pid).color)), act(move |app| app.reattach(&mid, &name, true, false, false))));
            }
        }
        let mut cmd = |label: &str, kbd: &str, glyph: &'static str, run: Act| items.push(item("Commands", label.to_string(), String::new(), kbd, Icon::Glyph(glyph), None, run));
        cmd("Layout: single", "", "layout", act(|a| a.set_layout(1)));
        cmd("Layout: side by side", "", "layout", act(|a| a.set_layout(2)));
        cmd("Layout: grid of four", "", "layout", act(|a| a.set_layout(4)));
        cmd("Fleet: every agent, and the ones waiting for you", "Ctrl+Shift+A", "inbox", act(App::open_fleet));
        cmd("Broadcast prompt to sessions", "Ctrl+Shift+Enter", "send", act(|a| a.open_composer(None)));
        cmd("Prompt library…", "", "book", act(|a| a.open_picker(PalSource::Prompts, "")));
        cmd("Workspaces…", "Ctrl+Shift+O", "layout", act(App::open_workspaces));
        cmd("Share machines over Tailscale…", "", "people", act(App::open_share));
        cmd("Team chat", "Ctrl+Shift+M", "chat", act(|a| a.open_team(None)));
        cmd("Save layout as workspace…", "", "save", act(App::save_workspace));
        cmd("Queue: launches waiting for a free machine…", "", "clock", act(|a| a.open_picker(PalSource::Queue, "")));
        cmd("Export workspaces and presets…", "", "send", act(App::export_workspaces));
        cmd("Import workspaces and presets…", "", "folder", act(App::import_workspaces));
        cmd("Publish my presets to the team", "", "send", act(App::publish_presets));
        cmd("Import team presets…", "", "folder", act(App::import_team_presets));
        if let Some(f) = self.focused() { cmd(&format!("Send last reply from {} to…", self.sessions[&f].name), "Ctrl+Shift+S", "send", act(move |a| a.send_last_reply(f))); }
        cmd("Settings", "Ctrl+,", "gear", act(App::open_settings));
        for m in self.cfg.machines.iter().filter(|m| m.is_ssh()) {
            let id = m.id.clone();
            let (a, b, c, d) = (id.clone(), id.clone(), id.clone(), id);
            cmd(&format!("Connect {}", m.name), "", "plug", act(move |x| x.connect_machine(&a, false)));
            cmd(&format!("Set up SSH key for {}", m.name), "", "key", act(move |x| x.setup_key(&b)));
            cmd(&format!("Install tools on {} (tmux, Claude, Antigravity, OpenClaw)", m.name), "", "box", act(move |x| x.bootstrap(&c)));
            cmd(&format!("Enable Claude alerts on {}", m.name), "", "bell", act(move |x| x.enable_alerts(&d)));
            let e = m.id.clone();
            cmd(&format!("Replay recorded sessions on {}…", m.name), "", "play", act(move |x| x.open_replay(&e)));
        }
        ("Switch session, launch Claude / Antigravity / OpenClaw / Shell on any machine…".into(), items)
    }

    fn draw_palette(&mut self) {
        let Some(mut pal) = self.palette.take() else { return };
        let (placeholder, mut src) = self.palette_items(&pal.source);
        let q = pal.input.trim().to_string();
        if let (PalSource::Folder { machine_id, preset_id, worktree }, false) = (&pal.source, q.is_empty()) {
            let (mid, pid, text, wt) = (machine_id.clone(), preset_id.clone(), q.clone(), *worktree);
            src.insert(0, PalItem { group: "Use", label: q.clone(), sub: "press Enter to use this path".into(), kbd: String::new(), icon: Icon::Glyph("folder"), color: None, missing: false, exact: true, run: act(move |app| app.pick_folder(&mid, &pid, &text, wt)) });
        }
        let mut items: Vec<(PalItem, HashSet<usize>)> = if q.is_empty() { src.into_iter().map(|i| (i, HashSet::new())).collect() } else {
            let mut v: Vec<(f64, PalItem, HashSet<usize>)> = src.into_iter().filter_map(|it| {
                if it.exact { return Some((-1e6, it, HashSet::new())); }
                let (score, hits) = fuzzy(&q, &format!("{} {}", it.label, it.sub))?;
                Some((score, it, hits))
            }).collect();
            v.sort_by(|a, b| b.0.total_cmp(&a.0));
            // A typed path goes first when it looks like one.
            if let Some(ix) = v.iter().position(|x| x.1.exact) { if ix > 0 && (q.starts_with(['~', '/']) || q.as_bytes().get(1) == Some(&b':')) { let x = v.remove(ix); v.insert(0, x); } }
            v.into_iter().map(|(_, i, h)| (i, h)).collect()
        };
        let n = items.len();
        pal.sel = pal.sel.min(n.saturating_sub(1));
        let (mut run, mut close) = (None, false);
        // Keys the list owns (taken before the text box sees them).
        self.ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) && n > 0 { pal.sel = (pal.sel + 1) % n; pal.scroll = true; }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) && n > 0 { pal.sel = (pal.sel + n - 1) % n; pal.scroll = true; }
            if i.consume_key(Modifiers::NONE, Key::Enter) { run = Some(pal.sel); }
            if i.consume_key(Modifiers::NONE, Key::Escape) { close = true; }
        });
        let screen = self.ctx.content_rect();
        let backdrop = egui::Area::new(egui::Id::unique("pal-back")).order(egui::Order::Middle).fixed_pos(screen.min).show(&self.ctx, |ui| {
            let r = ui.allocate_response(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(120));
            r
        }).inner;
        if backdrop.clicked() { close = true; }
        let width = 640.0f32.min(screen.width() - 40.0);
        egui::Area::new(egui::Id::unique("palette")).order(egui::Order::Foreground).fixed_pos(egui::pos2(screen.center().x - width / 2.0, screen.top() + 70.0)).show(&self.ctx, |ui| {
            modal_frame().inner_margin(8).show(ui, |ui| {
                ui.set_width(width);
                let before = pal.input.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut pal.input).hint_text(placeholder).desired_width(f32::INFINITY).frame(egui::Frame::NONE).font(FontId::proportional(15.0)).margin(Vec2::new(8.0, 8.0)));
                if std::mem::take(&mut pal.focus) || !r.has_focus() { r.request_focus(); }
                if pal.input != before { pal.sel = 0; pal.scroll = true; }
                ui.separator();
                let list_h = (screen.height() - 220.0).clamp(120.0, 460.0);
                // A floating window keeps its first frame's height and a scroll area won't outgrow that by itself:
                // min_scrolled_height lets a list that fills in later (folders arriving from a machine) grow.
                egui::ScrollArea::vertical().max_height(list_h).min_scrolled_height(list_h).show(ui, |ui| {
                    if items.is_empty() { ui.add_space(14.0); ui.vertical_centered(|ui| ui.label(dim("No matches"))); ui.add_space(14.0); }
                    let mut last_group = "";
                    for (i, (it, hits)) in items.iter().enumerate() {
                        if q.is_empty() && it.group != last_group { ui.add_space(4.0); ui.label(RichText::new(it.group.to_uppercase()).size(10.0).color(TEXT3)); }
                        last_group = it.group;
                        let sel = i == pal.sel;
                        let resp = egui::Frame::new().fill(if sel { PANEL3 } else { Color32::TRANSPARENT }).corner_radius(7).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                match &it.icon {
                                    Icon::Preset(p, m) => logo_img(ui, &self.preset_icon(p, m.as_deref()), 16.0),
                                    Icon::Glyph(g) => { icon(ui, g, 16.0, BRAND); }
                                }
                                let base = if it.missing { TEXT3 } else { TEXT };
                                let mut job = egui::text::LayoutJob::default();
                                for (ci, ch) in it.label.chars().enumerate() {
                                    job.append(&ch.to_string(), 0.0, egui::TextFormat { font_id: FontId::proportional(13.0), color: if hits.contains(&ci) { it.color.unwrap_or(BRAND) } else { base }, ..Default::default() });
                                }
                                let off = it.label.chars().count() + 1;
                                if !it.sub.is_empty() { job.append("  ", 0.0, egui::TextFormat::default()); }
                                for (ci, ch) in it.sub.chars().enumerate() {
                                    job.append(&ch.to_string(), 0.0, egui::TextFormat { font_id: FontId::proportional(11.5), color: if hits.contains(&(ci + off)) { TEXT } else { TEXT3 }, ..Default::default() });
                                }
                                ui.add(egui::Label::new(job).truncate());
                                if !it.kbd.is_empty() { ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| key_hint(ui, &it.kbd)); }
                            });
                        }).response.interact(egui::Sense::click());
                        if sel && pal.scroll { resp.scroll_to_me(None); }
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) { pal.sel = i; }
                        if resp.clicked() { run = Some(i); }
                    }
                    pal.scroll = false;
                });
            });
        });
        match run.filter(|i| *i < n) {
            Some(i) => { let a = items.swap_remove(i).0.run; self.focus_terminal(); a(self); }
            None if close => self.focus_terminal(),
            None => { if self.palette.is_none() { self.palette = Some(pal); } }
        }
    }

    // ---------------------------------------------------------------- composer (broadcast)
    pub fn open_composer(&mut self, prefill: Option<String>) {
        if self.targets.is_empty() { if let Some(f) = self.focused() { self.targets.insert(f); } }
        let c = self.composer.get_or_insert(Composer { text: String::new(), submit: true, focus: true });
        if let Some(p) = prefill { c.text = p; }
        c.focus = true;
    }
    pub fn close_composer(&mut self) { self.composer = None; self.focus_terminal(); }
    pub fn toggle_composer(&mut self) { if self.composer.is_some() { self.close_composer(); } else { self.open_composer(None); } }
    fn send_composer(&mut self) {
        let Some(c) = self.composer.as_ref() else { return };
        let (text, submit) = (c.text.clone(), c.submit);
        if text.trim().is_empty() { return; }
        let targets: Vec<u64> = self.targets.iter().copied().filter(|id| self.sessions.get(id).is_some_and(|s| s.status == SessStatus::Running)).collect();
        if targets.is_empty() { self.toast_warn("Pick at least one target session"); return; }
        let mut names = Vec::new();
        for id in &targets {
            self.paste_text(*id, &text);
            let s = &self.sessions[id];
            names.push(format!("{} on {}", s.name, self.machine(&s.machine_id).name));
            if let (true, Some(pid)) = (submit, s.pid) {
                // A moment later, so the agent has taken the paste before Enter arrives.
                let be = self.be.clone();
                self.be.rt.spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(150)).await; be.write(pid, b"\r"); });
            }
        }
        self.toast_ok(format!("Sent to {}", names.join(", ")));
        self.close_composer();
    }
    pub(super) fn draw_composer(&mut self, ui: &mut egui::Ui) {
        let Some(mut c) = self.composer.take() else { return };
        let live: Vec<u64> = self.ordered().into_iter().filter(|id| self.sessions[id].status == SessStatus::Running).collect();
        self.targets.retain(|id| live.contains(id));
        let (mut send, mut close, mut library, mut save) = (false, false, false, false);
        egui::Panel::bottom("composer").frame(egui::Frame::new().fill(PANEL).inner_margin(10)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Broadcast prompt").strong());
                for id in &live {
                    let s = &self.sessions[id];
                    let on = self.targets.contains(id);
                    let text = RichText::new(format!("{} · {}", s.name, self.machine(&s.machine_id).name)).size(11.5).color(if on { s.color } else { TEXT2 });
                    if ui.add(egui::Button::new(text).fill(if on { tint(s.color, 0.2) } else { PANEL2 }).corner_radius(10)).clicked() { if on { self.targets.remove(id); } else { self.targets.insert(*id); } }
                }
                let agents: Vec<u64> = live.iter().copied().filter(|id| is_agent(&self.sessions[id].preset_id)).collect();
                if agents.len() > 1 && ui.add(egui::Button::new(RichText::new("All agents").size(11.5).color(BRAND2)).corner_radius(10)).clicked() {
                    if agents.iter().all(|a| self.targets.contains(a)) { for a in &agents { self.targets.remove(a); } } else { self.targets.extend(agents); }
                }
                if live.is_empty() { ui.label(dim("No running sessions yet")); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_btn(ui, "x", "Close (Esc)").clicked() { close = true; } });
            });
            let id = egui::Id::unique("composer-text");
            if ui.memory(|m| m.has_focus(id)) {
                ui.input_mut(|i| { if i.consume_key(Modifiers::COMMAND, Key::Enter) { send = true; } if i.consume_key(Modifiers::NONE, Key::Escape) { close = true; } });
            }
            let r = ui.add(egui::TextEdit::multiline(&mut c.text).id(id).desired_rows(4).desired_width(f32::INFINITY).hint_text(format!("Write once, send to every selected session.   {} to send", kbd("Ctrl+Enter"))));
            if std::mem::take(&mut c.focus) { r.request_focus(); }
            ui.horizontal(|ui| {
                switch(ui, &mut c.submit, "Press Enter after sending");
                if icon_text_btn(ui, "book", "Prompts", false).on_hover_text("Fill this box from a saved prompt").clicked() { library = true; }
                if icon_text_btn(ui, "save", "Save as prompt", false).on_hover_text("Keep this text in the prompt library").clicked() { save = true; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if primary_btn(ui, "Send").clicked() { send = true; } key_hint(ui, "Ctrl+Enter"); });
            });
        });
        let text = c.text.clone();
        self.composer = Some(c);
        if send { self.send_composer(); } else if close { self.close_composer(); }
        else if library { self.open_picker(PalSource::Prompts, ""); } else if save { self.save_prompt(text); }
    }

    // ---------------------------------------------------------------- team presets
    /// Team presets live on the chat hub (~/.nexus/collab/presets.json) so the whole team launches the same commands.
    pub fn publish_presets(&mut self) {
        let (be, presets) = (self.be.clone(), self.cfg.presets.clone());
        self.be.run(async move { be.collab_presets_put(presets).await }, |app, r| match r {
            Err(e) => app.toast_bad(e),
            Ok(n) => { app.toast_ok(format!("Published {n} presets to the team")); app.post_team(format!("published their launch presets ({} → Import team presets)", kbd("Ctrl+Shift+P")), "activity", |_| {}); }
        });
    }
    pub fn import_team_presets(&mut self) {
        let be = self.be.clone();
        self.be.run(async move { be.collab_presets_get().await }, |app, r| {
            let list = match r { Ok(l) => l, Err(e) => { app.toast_bad(e); return; } };
            let fresh = app.fresh_presets(list);
            if fresh.is_empty() { app.toast_ok("No team presets you don't already have"); return; }
            let desc = format!("New: {}. Anyone who can write to the hub machine could have put these here; they are commands that will run on your machines.", sessions::describe_presets(&fresh));
            app.choose("Import team presets?", &desc, &[("Cancel", false), ("Import", true)], move |app, v| {
                if v != Some(1) { return; }
                app.cfg.presets.extend(fresh.iter().cloned());
                app.save_now();
                app.toast_ok(format!("Imported {} team preset{}", fresh.len(), if fresh.len() == 1 { "" } else { "s" }));
            });
        });
    }

    // ---------------------------------------------------------------- settings
    pub fn open_settings(&mut self) { self.menu = None; self.settings = Some(Settings { draft: self.cfg.clone(), probe_out: HashMap::new() }); }
    fn close_settings(&mut self) { self.settings = None; self.focus_terminal(); }
    /// The draft becomes the config (keeping what changed behind the dialog's back: sessions, recent folders…).
    fn save_settings(&mut self) {
        let Some(s) = self.settings.as_ref() else { return };
        let d = s.draft.clone();
        self.cfg.machines = d.machines;
        self.cfg.appearance = d.appearance;
        self.cfg.offline = d.offline;
        self.cfg.mcp = d.mcp;
        self.cfg.push = crate::config::PushCfg { url: d.push.url.trim().to_string(), ..d.push };
        let (client, status) = (self.cfg.collab.client.clone(), self.cfg.collab.status.clone());
        self.cfg.collab = crate::config::CollabCfg { client, status, ..d.collab };
        self.save_now();
        self.apply_font();
        self.reset_hub();
        for m in self.cfg.machines.clone() { if m.is_ssh() && !m.host.is_empty() && self.status_of(&m) != Status::Online { self.connect_machine(&m.id, false); } }
    }
    fn draw_settings(&mut self) {
        let Some(mut st) = self.settings.take() else { return };
        enum Do { None, Save, Cancel, OpenFile, Add, Remove(usize), Test(String), Key(String), Bootstrap(String), Alerts(String), PushTest(String) }
        let mut todo = Do::None;
        let esc = self.dialogs.is_empty() && self.ctx.input(|i| i.key_pressed(Key::Escape));
        let screen = self.ctx.content_rect();
        egui::Modal::new(egui::Id::unique("settings")).frame(modal_frame()).show(&self.ctx, |ui| {
            ui.set_width(760.0f32.min(screen.width() - 60.0));
            ui.horizontal(|ui| { ui.label(RichText::new("Settings").size(16.0).family(medium())); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if icon_btn(ui, "x", "Close").clicked() { todo = Do::Cancel; } }); });
            egui::ScrollArea::vertical().max_height(screen.height() - 190.0).min_scrolled_height((screen.height() - 190.0).max(120.0)).show(ui, |ui| {
                section(ui, "Machines");
                let ssh_count = st.draft.machines.iter().filter(|m| m.is_ssh()).count();
                for (idx, m) in st.draft.machines.iter_mut().enumerate() {
                    let ssh = m.is_ssh();
                    egui::Frame::new().fill(PANEL2).corner_radius(10).inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            logo_img(ui, self.machine_icon(m), 18.0);
                            ui.label(RichText::new(&m.name).strong());
                            ui.label(dim(if ssh { "ssh · linux".to_string() } else if m.is_wsl() { format!("wsl · {}", m.distro) } else { "this computer".to_string() }).size(11.0));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ssh && ssh_count > 1 && ui.add(egui::Button::new(RichText::new("Remove").color(BAD)).frame(false)).clicked() { todo = Do::Remove(idx); } });
                        });
                        ui.horizontal(|ui| {
                            field(ui, "Name", |ui| { ui.add(egui::TextEdit::singleline(&mut m.name).desired_width(200.0)); });
                            field(ui, "Accent", |ui| { let c = hex(&m.color); let mut rgb = [c.r(), c.g(), c.b()]; if ui.color_edit_button_srgb(&mut rgb).changed() { m.color = to_hex(Color32::from_rgb(rgb[0], rgb[1], rgb[2])); } });
                            field(ui, "Icon", |ui| {
                                egui::ComboBox::from_id_salt(("icon", idx)).selected_text(match m.icon.as_str() { "windows" => "Windows", "linux" => "Linux", "nvidia" => "NVIDIA", _ => "Auto" }).show_ui(ui, |ui| {
                                    for (v, l) in [("", "Auto"), ("windows", "Windows"), ("linux", "Linux"), ("nvidia", "NVIDIA")] { ui.selectable_value(&mut m.icon, v.to_string(), l); }
                                });
                            });
                        });
                        if !ssh { return; }
                        let mono = |s: &mut String, hint: &str, w: f32, ui: &mut egui::Ui| { let r = ui.add(egui::TextEdit::singleline(s).hint_text(hint).font(egui::TextStyle::Monospace).desired_width(w)); if r.changed() { *s = s.trim().to_string(); } };
                        ui.horizontal(|ui| {
                            field(ui, "User", |ui| mono(&mut m.user, "from ~/.ssh/config", 150.0, ui));
                            field(ui, "Host / IP", |ui| mono(&mut m.host, "192.168.1.50 or linuxbox.local", 260.0, ui));
                            field(ui, "Port", |ui| { let mut p = if m.port == 0 { 22 } else { m.port }; if ui.add(egui::DragValue::new(&mut p).range(1..=65535)).changed() { m.port = p; } });
                        });
                        field(ui, "Tailscale address (optional; used when the LAN address can't be reached. Filled in automatically by Share)", |ui| mono(&mut m.ts_host, "spark1.tail1234.ts.net", 480.0, ui));
                        field(ui, "Private key (optional; default tries ~/.ssh/id_ed25519, id_ecdsa, id_rsa, then ssh-agent)", |ui| mono(&mut m.key_path, "~/.ssh/id_ed25519", 480.0, ui));
                        field(ui, "Jump host (optional, like ssh -J; blank uses ProxyJump from ~/.ssh/config)", |ui| mono(&mut m.jump, "user@bastion.example.com:22", 480.0, ui));
                        ui.horizontal_wrapped(|ui| {
                            if icon_text_btn(ui, "plug", "Test connection", true).clicked() { todo = Do::Test(m.id.clone()); }
                            if icon_text_btn(ui, "key", "Set up SSH key", true).clicked() { todo = Do::Key(m.id.clone()); }
                            if icon_text_btn(ui, "box", "Install tmux / Claude / Antigravity / OpenClaw", true).clicked() { todo = Do::Bootstrap(m.id.clone()); }
                            if icon_text_btn(ui, "bell", "Enable Claude alerts", true).clicked() { todo = Do::Alerts(m.id.clone()); }
                        });
                        match st.probe_out.get(&m.id) {
                            Some(Err(e)) => { chip(ui, e, BAD); }
                            Some(Ok((host, tools))) if host.is_empty() && tools.is_empty() => { ui.label(dim("Connecting…")); }
                            Some(Ok((host, tools))) => { ui.horizontal_wrapped(|ui| { chip(ui, &format!("connected · {host}"), OK); for (k, v) in tools { chip(ui, &format!("{} {k}", if *v { "✓" } else { "✗" }), if *v { TEXT2 } else { BAD }); } }); }
                            None => {}
                        }
                    });
                    ui.add_space(4.0);
                }
                if ui.add(egui::Button::new("+ Add another SSH machine").frame(false)).clicked() { todo = Do::Add; }

                section(ui, "Appearance & behaviour");
                let d = &mut st.draft;
                ui.horizontal(|ui| {
                    field(ui, "Font size", |ui| { ui.add(egui::DragValue::new(&mut d.appearance.font_size).range(9.0..=28.0).speed(0.25)); });
                    field(ui, "Line height", |ui| { ui.add(egui::DragValue::new(&mut d.appearance.line_height).range(1.0..=2.0).speed(0.01)); });
                    field(ui, "Font family", |ui| { ui.add(egui::TextEdit::singleline(&mut d.appearance.font_family).font(egui::TextStyle::Monospace).desired_width(440.0)); });
                });
                ui.horizontal(|ui| {
                    field(ui, "Your name in team chat", |ui| { ui.add(egui::TextEdit::singleline(&mut d.collab.name).hint_text(self.team.ts_user.as_ref().map_or("from Tailscale", |u| u.name.as_str())).desired_width(240.0)); });
                    field(ui, "Team chat lives on", |ui| {
                        let hubs: Vec<(String, String)> = d.machines.iter().filter(|m| m.is_ssh()).filter_map(|m| { let h = self.host_of(&m.id); (!h.is_empty()).then(|| (h.clone(), format!("{} ({h})", m.name))) }).collect();
                        let cur = hubs.iter().find(|(h, _)| *h == d.collab.hub).map_or("Auto (first SSH machine by hostname)".to_string(), |x| x.1.clone());
                        egui::ComboBox::from_id_salt("hub").selected_text(cur).show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.collab.hub, String::new(), "Auto (first SSH machine by hostname)");
                            for (h, label) in hubs { ui.selectable_value(&mut d.collab.hub, h, label); }
                        });
                    });
                });
                switch(ui, &mut d.appearance.copy_on_select, "Copy on select");
                switch(ui, &mut d.appearance.notify_when_done, "Notify when a background agent finishes");
                switch(ui, &mut d.collab.announce, "Post in team chat when I start an agent on a shared machine");
                field(ui, "Phone alerts: an ntfy topic address (ntfy.sh or your own server). Told when an agent finishes or needs you while Nexus isn't in front. Anyone who knows the address can read the alerts, so pick a long random topic.", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut d.push.url).hint_text("https://ntfy.sh/nexus-7f3k2m9x1q").font(egui::TextStyle::Monospace).desired_width(420.0));
                        if ui.add_enabled(!d.push.url.trim().is_empty(), egui::Button::new("Send a test")).clicked() { todo = Do::PushTest(d.push.url.trim().to_string()); }
                    });
                });
                if !d.push.url.trim().is_empty() { switch(ui, &mut d.push.include_text, "Include what the agent said in phone alerts (off: only which session is waiting)"); }
                switch(ui, &mut d.appearance.restore_session, "Restore sessions, layout and folders when Nexus starts");
                switch(ui, &mut d.appearance.record_sessions, "Record remote sessions to ~/.nexus/logs on the machine (audit trail; kept 30 days)");
                switch(ui, &mut d.mcp.enabled, "Let MCP clients on this PC control Nexus (list machines, launch agents, send text, read output)");
                if d.mcp.enabled {
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        let cmd = format!("claude mcp add nexus -- \"{}\"", mcp_bridge_path());
                        ui.label(RichText::new(&cmd).monospace().size(11.0).color(TEXT2));
                        if ui.small_button("Copy").clicked() { ui.ctx().copy_text(cmd); }
                    });
                }
                switch(ui, &mut d.offline, "Offline / air-gapped mode: no update checks, no internet installers (tools install from ~/.nexus/offline/ on each machine)");
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new("Open config file").frame(false)).clicked() { todo = Do::OpenFile; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if primary_btn(ui, "Save").clicked() { todo = Do::Save; } if ui.button("Cancel").clicked() { todo = Do::Cancel; } });
            });
        });
        self.settings = Some(st);
        if esc && matches!(todo, Do::None) { todo = Do::Cancel; }
        let needs_host = |app: &mut App, id: &str| -> bool {
            let ok = app.settings.as_ref().and_then(|s| s.draft.machine(id)).is_some_and(|m| !m.host.is_empty());
            if ok { app.save_settings(); } else { app.toast_warn("Enter the host / IP first"); }
            ok
        };
        match todo {
            Do::None => {}
            Do::Save => { self.save_settings(); self.close_settings(); self.toast_ok("Settings saved"); }
            Do::Cancel => self.close_settings(),
            Do::OpenFile => { let _ = open::that_detached(crate::config::file()); }
            Do::Add => { if let Some(s) = self.settings.as_mut() { s.draft.machines.push(Machine { id: format!("ssh-{}", crate::remote::radix36(now_ms() as u64)), name: "New machine".into(), kind: "ssh".into(), os: "linux".into(), port: 22, color: "#7dcfff".into(), ..Default::default() }); } }
            Do::Remove(i) => { if let Some(s) = self.settings.as_mut() { s.draft.machines.remove(i); } }
            Do::Test(id) => if needs_host(self, &id) {
                if let Some(s) = self.settings.as_mut() { s.probe_out.insert(id.clone(), Ok((String::new(), vec![]))); }
                let (be, mid) = (self.be.clone(), id.clone());
                self.be.run(async move { be.machine(&mid).connect().await?; be.probe(&mid).await }, move |app, r| {
                    let out = r.map(|p| { let mut t: Vec<(String, bool)> = p.tools.into_iter().collect(); t.sort(); (p.hostname, t) });
                    if let Some(s) = app.settings.as_mut() { s.probe_out.insert(id.clone(), out); }
                    app.refresh_machine(&id);
                });
            },
            Do::Key(id) => if needs_host(self, &id) { self.close_settings(); self.setup_key(&id); },
            Do::Bootstrap(id) => if needs_host(self, &id) { self.close_settings(); self.bootstrap(&id); },
            Do::Alerts(id) => if needs_host(self, &id) { self.enable_alerts(&id); },
            Do::PushTest(url) => {
                let be = self.be.clone();
                self.be.run(async move { be.push_test(url).await }, |app, r| match r { Ok(()) => app.toast_ok("Sent. It should reach your phone in a moment."), Err(e) => app.toast_bad(format!("Couldn't send: {e}")) });
            }
        }
    }

    // ---------------------------------------------------------------- context menu
    pub fn show_menu(&mut self, pos: egui::Pos2, items: Vec<MenuItem>) { self.menu = Some(Menu { pos, items, opened: now_ms() }); }
    fn draw_menu(&mut self) {
        let Some(menu) = self.menu.take() else { return };
        let mut run: Option<Act> = None;
        let screen = self.ctx.content_rect();
        let area = egui::Area::new(egui::Id::unique("ctxmenu")).order(egui::Order::Tooltip).pivot(egui::Align2::LEFT_TOP).fixed_pos(menu.pos).constrain_to(screen.shrink(6.0)).show(&self.ctx, |ui| {
            egui::Frame::new().fill(PANEL).stroke(egui::Stroke::new(1.0, LINE2)).corner_radius(10).inner_margin(5)
                .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) }).show(ui, |ui| {
                ui.set_width(248.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.spacing_mut().interact_size.y = 18.0;
                for it in &menu.items {
                    match it {
                        MenuItem::Sep => { ui.add_space(3.0); ui.separator(); ui.add_space(3.0); }
                        MenuItem::Header(h) => { ui.add_space(2.0); ui.horizontal(|ui| { ui.add_space(7.0); ui.label(RichText::new(h).size(11.0).family(medium()).color(TEXT3)); }); }
                        MenuItem::Item { label, kbd: k, icon, danger, disabled, run: a } => {
                            let color = if *disabled { TEXT3 } else if *danger { BAD } else { TEXT };
                            let resp = ui.add_enabled_ui(!*disabled, |ui| {
                                let r = egui::Frame::new().corner_radius(6).inner_margin(egui::Margin::symmetric(7, 4)).show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        match icon {
                                            Some(Icon::Preset(p, m)) => logo_img(ui, &self.preset_icon(p, m.as_deref()), 14.0),
                                            Some(Icon::Glyph(g)) => { super::icon(ui, g, 14.0, TEXT2); }
                                            None => { ui.add_space(14.0 + ui.spacing().item_spacing.x); }
                                        }
                                        ui.label(RichText::new(label).color(color));
                                        if !k.is_empty() { ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(kbd(k)).monospace().size(10.5).color(TEXT3))); }
                                    });
                                }).response.interact(egui::Sense::click());
                                if r.hovered() { ui.painter().rect_filled(r.rect, 6.0, Color32::from_white_alpha(14)); }
                                r
                            }).inner;
                            if resp.clicked() { run = Some(a.clone()); }
                        }
                    }
                }
            });
        }).response;
        let outside = self.ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.rect.contains(p)));
        let close = run.is_some() || (outside && now_ms() - menu.opened > 150) || self.ctx.input(|i| i.key_pressed(Key::Escape)) || !self.win_focused;
        if let Some(a) = run { a(self); }
        if !close && self.menu.is_none() { self.menu = Some(menu); }
    }

    pub(super) fn draw_overlays(&mut self) {
        self.draw_settings();
        self.draw_share();
        self.draw_fleet();
        self.draw_replay();
        self.draw_palette();
        self.draw_dialog();
        self.draw_menu();
    }

    pub(super) fn draw_toasts(&mut self) {
        if self.toasts.is_empty() { return; }
        let mut run: Option<(usize, Act)> = None;
        let screen = self.ctx.content_rect();
        egui::Area::new(egui::Id::unique("toasts")).order(egui::Order::Tooltip).pivot(egui::Align2::RIGHT_BOTTOM).fixed_pos(screen.right_bottom() - Vec2::new(16.0, 40.0)).show(&self.ctx, |ui| {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                for (i, t) in self.toasts.iter().enumerate().rev() {
                    egui::Frame::new().fill(PANEL).stroke(egui::Stroke::new(1.0, LINE2)).corner_radius(10).inner_margin(egui::Margin::symmetric(14, 10)).shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(140) }).show(ui, |ui| {
                        ui.set_max_width(440.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("●").size(9.0).color(t.color));
                            ui.label(&t.msg);
                            if let Some((label, a)) = &t.action { if primary_btn(ui, label).clicked() { run = Some((i, a.clone())); } }
                        });
                    });
                }
            });
        });
        if let Some((i, a)) = run { self.toasts.remove(i); a(self); }
    }

    // ---------------------------------------------------------------- keyboard
    /// App shortcuts. They use Cmd on macOS (Ctrl stays with the terminal there: Ctrl+C interrupts) and Ctrl elsewhere.
    fn shortcut(&mut self, key: Key, m: Modifiers) -> bool {
        let (c, sh, a) = (m.command, m.shift, m.alt);
        match key {
            Key::P | Key::K if c && sh && !a => self.open_palette(""),
            Key::T if c && sh => self.open_palette("new "),
            Key::Enter if c && sh => self.toggle_composer(),
            Key::L if c && sh => self.cycle_layout(),
            Key::W if c && sh => self.close_focused(),
            Key::F if c && sh => { if let Some(id) = self.focused() { self.open_search(id); } }
            Key::D if c && sh => { if let Some(id) = self.focused() { self.split_with(id); } }
            Key::S if c && sh => { if let Some(id) = self.focused() { self.send_last_reply(id); } }
            Key::O if c && sh => self.open_workspaces(),
            Key::M if c && sh => self.toggle_team(),
            Key::A if c && sh => self.open_fleet(),
            Key::N if c && sh => self.open_palette("in folder "),
            Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9 if c && !sh && !a => {
                let n = key.name().parse::<usize>().unwrap_or(1);
                if let Some(id) = self.ordered().get(n - 1).copied() { self.show(id); }
            }
            Key::Tab if m.ctrl => self.cycle(if sh { -1 } else { 1 }),
            Key::Backtick if m.ctrl && !sh => { if let Some(id) = self.mru.get(1).copied() { self.show(id); } }
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown if c && a => self.move_pane_focus(key),
            Key::Comma if c && !sh => self.open_settings(),
            Key::Equals | Key::Plus if c && !sh => self.bump_font(1),
            Key::Minus if c && !sh => self.bump_font(-1),
            Key::Num0 if c && !sh => self.bump_font(0),
            _ => return false,
        }
        true
    }

    pub(super) fn handle_keyboard(&mut self) {
        self.ctx.options_mut(|o| o.zoom_with_keyboard = false);
        // Open dialogs (palette, settings, share, prompts) own the keyboard: keys never reach the terminal behind them.
        if self.overlay_open() || self.menu.is_some() { return; }
        let text_focus = self.text_has_focus();
        let events = self.ctx.input(|i| i.events.clone());
        let target = self.focused();
        for ev in events {
            match ev {
                egui::Event::Key { key, pressed: true, modifiers, .. } => {
                    // In a text box only the Ctrl+Shift combos are app shortcuts; the rest is typing.
                    if (!text_focus || (modifiers.command && modifiers.shift)) && self.shortcut(key, modifiers) { continue; }
                    if text_focus { continue; }
                    let Some(id) = target else { continue };
                    if IS_MAC && modifiers.mac_cmd { continue; } // other Cmd combos belong to the app, never to the shell
                    if key == Key::PageUp && modifiers.shift { if let Some(s) = self.sessions.get_mut(&id) { s.term.scroll(i32::from(s.term.rows) - 1); } continue; }
                    if key == Key::PageDown && modifiers.shift { if let Some(s) = self.sessions.get_mut(&id) { s.term.scroll(1 - i32::from(s.term.rows)); } continue; }
                    if let Some(bytes) = self.sessions.get(&id).and_then(|s| s.term.key_bytes(key, modifiers)) { self.input(id, &bytes); }
                }
                egui::Event::Text(t) if !text_focus => {
                    let m = self.ctx.input(|i| i.modifiers);
                    // Alt+key was already sent as ESC+key; Ctrl/Cmd combos are never text (Ctrl+Alt is AltGr: that is text).
                    if (m.alt && !m.ctrl && !IS_MAC) || (m.command && !m.alt) { continue; }
                    if let Some(id) = target { self.input(id, t.as_bytes()); }
                }
                egui::Event::Ime(egui::ImeEvent::Commit(t)) if !text_focus => { self.preedit.clear(); if let Some(id) = target { self.input(id, t.as_bytes()); } }
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) if !text_focus => self.preedit = text,
                // Ctrl+C copies when text is selected, otherwise it interrupts (on macOS Ctrl+C is always the terminal's).
                egui::Event::Copy if !text_focus => { if let Some(id) = target { if !self.copy_selection(id) && !IS_MAC { self.input(id, b"\x03"); } } }
                egui::Event::Cut if !text_focus && !IS_MAC => { if let Some(id) = target { self.input(id, b"\x18"); } }
                egui::Event::Paste(text) if !text_focus => { if let Some(id) = target { self.paste_text(id, &text); } }
                egui::Event::PasteImage(img) if !text_focus => { if let Some(id) = target { self.paste_image(id, &img); } }
                _ => {}
            }
        }
    }
}

/// Where the MCP stdio bridge lives: beside the app's own executable.
pub fn mcp_bridge_path() -> String {
    let name = if cfg!(windows) { "nexus-mcp.exe" } else { "nexus-mcp" };
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(name))).map_or(name.to_string(), |p| p.to_string_lossy().into_owned())
}
