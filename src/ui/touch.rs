//! Phones and touch screens: the narrow layout, the row of terminal keys above the on-screen keyboard, the system
//! clipboard and the Back button on Android.
use super::*;
use egui::{Key, Modifiers};

/// Below this width (in points) the window is laid out for a phone: one pane, no status bar, and the machine list
/// as a drawer. Desktop windows can't get this narrow; tablets and unfolded phones get the full layout.
pub const COMPACT_WIDTH: f32 = 700.0;

#[derive(Clone, Copy)]
enum Cap { Key(Key), Text(&'static str), Ctrl, Alt, Paste, Keyboard }

/// What an on-screen keyboard lacks, in the order a shell and the agents' prompts need them.
const CAPS: &[(&str, Cap)] = &[
    ("esc", Cap::Key(Key::Escape)), ("tab", Cap::Key(Key::Tab)), ("ctrl", Cap::Ctrl), ("alt", Cap::Alt),
    ("←", Cap::Key(Key::ArrowLeft)), ("↓", Cap::Key(Key::ArrowDown)), ("↑", Cap::Key(Key::ArrowUp)), ("→", Cap::Key(Key::ArrowRight)),
    ("paste", Cap::Paste), ("-", Cap::Text("-")), ("/", Cap::Text("/")), ("|", Cap::Text("|")), ("~", Cap::Text("~")),
    ("home", Cap::Key(Key::Home)), ("end", Cap::Key(Key::End)), ("pgup", Cap::Key(Key::PageUp)), ("pgdn", Cap::Key(Key::PageDown)),
    ("keyboard", Cap::Keyboard),
];

/// The control character Ctrl makes of a typed character (Ctrl+C -> 0x03), where there is one.
pub fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '@' | ' ' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' | '/' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Typed text with the keys row's Ctrl and Alt applied: Ctrl turns a single character into its control character,
/// Alt puts ESC in front (what a terminal sends for Alt+key).
pub fn sticky_bytes(text: &str, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 1);
    if alt { out.push(0x1b); }
    let mut chars = text.chars();
    match (ctrl, chars.next(), chars.next()) {
        (true, Some(c), None) if ctrl_byte(c).is_some() => out.extend(ctrl_byte(c)),
        _ => out.extend_from_slice(text.as_bytes()),
    }
    out
}

fn key_cap(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(label.to_string(), FontId::monospace(13.0), if on { BG } else { TEXT });
    let (rect, resp) = ui.allocate_exact_size(Vec2::new((galley.size().x + 18.0).max(38.0), 32.0), egui::Sense::click());
    let fill = if on { BRAND } else if resp.is_pointer_button_down_on() { PANEL3 } else { PANEL2 };
    ui.painter().rect(rect, 7.0, fill, egui::Stroke::new(1.0, LINE2), egui::StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, TEXT);
    resp
}

impl App {
    pub(super) fn set_compact(&mut self, compact: bool) {
        self.compact = compact;
        if compact && self.layout != 1 { self.set_layout(1); }
        if !compact { self.drawer = false; }
    }

    /// The machine list on a phone: opened from the title bar, closed by picking a session.
    pub fn toggle_drawer(&mut self) {
        self.drawer = !self.drawer;
        self.menu = None;
        if self.drawer && self.team.open { self.toggle_team(); }
    }

    /// Paste the system clipboard into whatever has the keyboard.
    pub fn request_paste(&mut self) {
        #[cfg(not(target_os = "android"))]
        self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        #[cfg(target_os = "android")]
        match crate::android::get_clipboard() {
            Some(text) => { self.pending_events.push(egui::Event::Paste(text)); self.ctx.request_repaint(); }
            None => self.toast_warn("There is no text on the clipboard"),
        }
    }

    /// On Android egui keeps copied text inside the app: hand it to the system clipboard as well.
    pub(super) fn sync_clipboard(&mut self) {
        #[cfg(target_os = "android")]
        {
            let copied = self.ctx.output(|o| o.commands.iter().rev().find_map(|c| match c { egui::OutputCommand::CopyText(t) => Some(t.clone()), _ => None }));
            if let Some(text) = copied { if !crate::android::set_clipboard(&text) { self.toast_warn("Couldn't reach the clipboard"); } }
        }
    }

    /// Before each frame on Android: keep clear of the system bars and the on-screen keyboard, give the Back button
    /// a meaning, and deliver pasted text.
    #[cfg(target_os = "android")]
    pub(super) fn android_input(&mut self, raw: &mut egui::RawInput) {
        let ppp = self.ctx.pixels_per_point().max(0.5);
        let (left, top, right, bottom) = crate::android::insets();
        raw.safe_area_insets = Some(egui::SafeAreaInsets(egui::epaint::MarginF32 { left: left / ppp, right: right / ppp, top: top / ppp, bottom: bottom / ppp }));
        // Back closes whatever is open on top (the same as Escape); with nothing open it shows or hides the machine list.
        let overlay = self.overlay_open() || self.menu.is_some();
        let mut back = false;
        raw.events.retain_mut(|e| {
            let egui::Event::Key { key, pressed, .. } = e else { return true };
            if *key != Key::BrowserBack { return true; }
            if overlay { *key = Key::Escape; return true; }
            back |= *pressed;
            false
        });
        if back {
            if self.team.open { self.toggle_team(); } else if self.compact { self.toggle_drawer(); }
        }
        raw.events.append(&mut self.pending_events);
    }

    /// The keys an on-screen keyboard doesn't have. Ctrl and Alt stay down for the next key only.
    pub(super) fn draw_keys_row(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.focused() else { return };
        let mut pressed: Option<Cap> = None;
        egui::Panel::bottom("keys-row").exact_size(42.0).frame(egui::Frame::new().fill(BG2).inner_margin(egui::Margin::symmetric(6, 5))).show(ui, |ui| {
            egui::ScrollArea::horizontal().id_salt("keys-row").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    for (label, cap) in CAPS {
                        let on = match cap { Cap::Ctrl => self.sticky_ctrl, Cap::Alt => self.sticky_alt, Cap::Keyboard => !self.kb_hidden, _ => false };
                        if key_cap(ui, label, on).clicked() { pressed = Some(*cap); }
                    }
                });
            });
        });
        let Some(cap) = pressed else { return };
        let (ctrl, alt) = (self.sticky_ctrl, self.sticky_alt);
        match cap {
            Cap::Ctrl => self.sticky_ctrl = !ctrl,
            Cap::Alt => self.sticky_alt = !alt,
            Cap::Keyboard => self.kb_hidden = !self.kb_hidden,
            Cap::Paste => self.request_paste(),
            Cap::Key(key) => {
                let mods = Modifiers { ctrl, alt, ..Modifiers::NONE };
                if let Some(bytes) = self.sessions.get(&id).and_then(|s| s.term.key_bytes(key, mods)) { self.input(id, &bytes); }
                (self.sticky_ctrl, self.sticky_alt) = (false, false);
            }
            Cap::Text(text) => {
                self.input(id, &sticky_bytes(text, ctrl, alt));
                (self.sticky_ctrl, self.sticky_alt) = (false, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_ctrl_and_alt_make_the_bytes_a_keyboard_would() {
        assert_eq!(sticky_bytes("c", true, false), [0x03]);
        assert_eq!(sticky_bytes("C", true, false), [0x03]);
        assert_eq!(sticky_bytes("[", true, false), [0x1b]);
        assert_eq!(sticky_bytes(" ", true, false), [0x00]);
        assert_eq!(sticky_bytes("b", false, true), [0x1b, b'b']);
        assert_eq!(sticky_bytes("x", true, true), [0x1b, 0x18]);
        // Nothing to make of these: they are typed as they are.
        assert_eq!(sticky_bytes("é", true, false), "é".as_bytes());
        assert_eq!(sticky_bytes("ls", true, false), b"ls");
        assert_eq!(sticky_bytes("d", false, false), b"d");
    }

    #[test]
    fn the_keys_row_has_each_label_once() {
        let mut seen = std::collections::HashSet::new();
        assert!(CAPS.iter().all(|(label, _)| seen.insert(*label)));
    }
}
