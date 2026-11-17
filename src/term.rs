//! The terminal emulator: alacritty_terminal keeps the grid, scrollback and modes; this file feeds it, turns keys
//! and mouse actions into the bytes a terminal sends, and paints the grid with egui.
use alacritty_terminal::event::{Event as AEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb};
use eframe::egui::{self, Color32, FontId, Key, Modifiers, Pos2, Rect, Stroke, Vec2};
use std::sync::{Arc, Mutex};

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0f, 0x18);
pub const FG: Color32 = Color32::from_rgb(0xc8, 0xd3, 0xf5);
const SELECTION: Color32 = Color32::from_rgb(0x2b, 0x3b, 0x62);
/// black, red, green, yellow, blue, magenta, cyan, white, then the bright eight.
const ANSI: [u32; 16] = [0x1d2233, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x4e5675, 0xff8fa3, 0xb9f27c, 0xffc777, 0x8db0ff, 0xc9aaff, 0xa4e4ff, 0xe6ebff];

const fn hex(c: u32) -> Color32 { Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8) }
fn indexed(i: u8) -> Color32 {
    match i {
        0..=15 => hex(ANSI[i as usize]),
        16..=231 => { let n = i - 16; let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x }; Color32::from_rgb(v(n / 36), v(n / 6 % 6), v(n % 6)) }
        _ => { let g = 8 + 10 * (i - 232); Color32::from_rgb(g, g, g) }
    }
}
fn dim(c: Color32) -> Color32 { Color32::from_rgb((f32::from(c.r()) * 0.66) as u8, (f32::from(c.g()) * 0.66) as u8, (f32::from(c.b()) * 0.66) as u8) }
fn named(n: NamedColor) -> Color32 {
    let i = n as usize;
    match n {
        NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => FG,
        NamedColor::Background => BG,
        NamedColor::DimForeground => dim(FG),
        _ if i < 16 => hex(ANSI[i]),
        _ => dim(hex(ANSI[(i - NamedColor::DimBlack as usize).min(7)])),
    }
}

#[derive(Clone)]
struct Listener(Arc<Mutex<Vec<AEvent>>>);
impl EventListener for Listener { fn send_event(&self, e: AEvent) { self.0.lock().unwrap().push(e); } }

struct Size { cols: usize, rows: usize }
impl Dimensions for Size {
    fn total_lines(&self) -> usize { self.rows }
    fn screen_lines(&self) -> usize { self.rows }
    fn columns(&self) -> usize { self.cols }
}

/// What a chunk of output asked the app to do.
#[derive(Debug, PartialEq)]
pub enum TermOut {
    Title(String),
    Bell,
    /// OSC 9 / OSC 777 "notify": an exact "the agent is waiting" signal with a message.
    Notify(String),
    /// OSC 52 clipboard write (the app decides whether to honour it).
    Clipboard(String),
    /// Bytes to send back to the program (answers to terminal queries).
    Reply(Vec<u8>),
}

/// Finds OSC 9 / 777 notifications in the output stream (alacritty_terminal has no hook for them). The bytes still go
/// to the emulator, which ignores those sequences.
#[derive(Default)]
struct OscScan { state: u8, buf: Vec<u8> }
impl OscScan {
    fn feed(&mut self, data: &[u8], out: &mut Vec<TermOut>) {
        for &b in data {
            match (self.state, b) {
                (0, 0x1b) => self.state = 1,
                (1, b']') => { self.state = 2; self.buf.clear(); }
                (1, _) => self.state = 0,
                (2, 0x07) => { self.done(out); self.state = 0; }
                (2, 0x1b) => self.state = 3,
                (2, _) => { if self.buf.len() < 4096 { self.buf.push(b); } }
                (3, _) => { self.done(out); self.state = if b == 0x1b { 1 } else { 0 }; }
                _ => self.state = 0,
            }
        }
    }
    fn done(&mut self, out: &mut Vec<TermOut>) {
        let s = String::from_utf8_lossy(&self.buf);
        if let Some(data) = s.strip_prefix("9;") {
            // "9;4;…" and friends are progress reports (ConEmu), not messages.
            let numeric = data.split_once(';').is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()));
            if !numeric { out.push(TermOut::Notify(data.to_string())); }
        } else if let Some(data) = s.strip_prefix("777;") {
            let mut p = data.splitn(3, ';');
            if p.next() == Some("notify") { out.push(TermOut::Notify(p.nth(1).unwrap_or_default().to_string())); }
        }
    }
}

pub struct Terminal {
    term: alacritty_terminal::Term<Listener>,
    parser: Processor,
    events: Arc<Mutex<Vec<AEvent>>>,
    osc: OscScan,
    pub cols: u16,
    pub rows: u16,
    search: Option<(String, RegexSearch)>,
    /// Where the left button went down, while a selection drag is in progress.
    dragging: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MouseKind { Press, Release, Drag }

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let cfg = Config { scrolling_history: 20_000, ..Default::default() };
        let term = alacritty_terminal::Term::new(cfg, &Size { cols: cols as usize, rows: rows as usize }, Listener(events.clone()));
        Self { term, parser: Processor::new(), events, osc: OscScan::default(), cols, rows, search: None, dragging: false }
    }

    /// Feed program output; returns what it asked for (title changes, bells, clipboard writes, query replies).
    pub fn feed(&mut self, data: &[u8]) -> Vec<TermOut> {
        let mut out = Vec::new();
        self.osc.feed(data, &mut out);
        self.parser.advance(&mut self.term, data);
        for e in std::mem::take(&mut *self.events.lock().unwrap()) {
            match e {
                AEvent::Title(t) => out.push(TermOut::Title(t)),
                AEvent::ResetTitle => out.push(TermOut::Title(String::new())),
                AEvent::Bell => out.push(TermOut::Bell),
                AEvent::ClipboardStore(_, text) => out.push(TermOut::Clipboard(text)),
                AEvent::PtyWrite(s) => out.push(TermOut::Reply(s.into_bytes())),
                AEvent::ColorRequest(i, fmt) => {
                    let c = self.term.colors()[i].map_or_else(|| if i < 256 { indexed(i as u8) } else if i == NamedColor::Background as usize { BG } else { FG }, |c| Color32::from_rgb(c.r, c.g, c.b));
                    out.push(TermOut::Reply(fmt(Rgb { r: c.r(), g: c.g(), b: c.b() }).into_bytes()));
                }
                AEvent::TextAreaSizeRequest(fmt) => out.push(TermOut::Reply(fmt(alacritty_terminal::event::WindowSize { num_lines: self.rows, num_cols: self.cols, cell_width: 8, cell_height: 16 }).into_bytes())),
                // Clipboard reads are never answered: remote programs can't see your clipboard.
                _ => {}
            }
        }
        out
    }
    pub fn write_str(&mut self, s: &str) { let _ = self.feed(s.as_bytes()); }

    pub fn resize(&mut self, cols: u16, rows: u16) -> bool {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if cols == self.cols && rows == self.rows { return false; }
        self.cols = cols;
        self.rows = rows;
        self.term.resize(Size { cols: cols as usize, rows: rows as usize });
        true
    }
    pub fn mode(&self) -> TermMode { *self.term.mode() }
    pub fn mouse_mode(&self) -> bool { self.mode().intersects(TermMode::MOUSE_MODE) }
    pub fn scroll(&mut self, lines: i32) { self.term.scroll_display(Scroll::Delta(lines)); }
    pub fn scroll_to_bottom(&mut self) { self.term.scroll_display(Scroll::Bottom); }
    pub fn scrolled_back(&self) -> bool { self.term.grid().display_offset() > 0 }

    // ---------------------------------------------------------------- selection / text
    pub fn has_selection(&self) -> bool { self.term.selection.as_ref().is_some_and(|s| !s.is_empty()) }
    pub fn selection_text(&self) -> Option<String> { self.term.selection_to_string().filter(|s| !s.is_empty()) }
    pub fn clear_selection(&mut self) { self.term.selection = None; }
    pub fn select_all(&mut self) {
        let mut s = Selection::new(SelectionType::Simple, Point::new(self.term.topmost_line(), Column(0)), Side::Left);
        s.update(Point::new(self.term.bottommost_line(), self.term.last_column()), Side::Right);
        self.term.selection = Some(s);
    }
    pub fn clear_scrollback(&mut self) { self.term.grid_mut().clear_history(); }

    /// The cursor's line counted from the top of the scrollback (for "since my last prompt").
    pub fn cursor_abs_line(&self) -> usize { self.term.history_size() + self.term.grid().cursor.point.line.0.max(0) as usize }
    pub fn total_lines(&self) -> usize { self.term.total_lines() }
    /// Text of lines [from, total) counted from the top of the scrollback, right-trimmed.
    pub fn lines_from(&self, from: usize) -> Vec<String> {
        let hist = self.term.history_size() as i32;
        (from as i32..self.term.total_lines() as i32).map(|i| {
            let l = Line(i - hist);
            self.term.bounds_to_string(Point::new(l, Column(0)), Point::new(l, self.term.last_column())).trim_end().to_string()
        }).collect()
    }

    /// Text of one visible row.
    pub fn visible_line(&self, row: u16) -> String {
        let l = Line(i32::from(row) - self.term.grid().display_offset() as i32);
        self.term.bounds_to_string(Point::new(l, Column(0)), Point::new(l, self.term.last_column()))
    }

    /// Find the next/previous match and select it. Returns false when there is none.
    pub fn find(&mut self, query: &str, forward: bool) -> bool {
        if query.is_empty() { self.search = None; self.clear_selection(); return false; }
        if self.search.as_ref().is_none_or(|(q, _)| q != query) {
            let Ok(re) = RegexSearch::new(&regex::escape(query)) else { return false };
            self.search = Some((query.to_string(), re));
        }
        let cur = self.term.selection.as_ref().and_then(|s| s.to_range(&self.term));
        let origin = match (&cur, forward) {
            (Some(r), true) => r.end.add(&self.term, alacritty_terminal::index::Boundary::None, 1),
            (Some(r), false) => r.start.sub(&self.term, alacritty_terminal::index::Boundary::None, 1),
            (None, _) => Point::new(Line(self.rows as i32 - 1 - self.term.grid().display_offset() as i32), self.term.last_column()),
        };
        let (dir, side) = if forward { (Direction::Right, Side::Left) } else { (Direction::Left, Side::Right) };
        let Some((_, re)) = self.search.as_mut() else { return false };
        let Some(m) = self.term.search_next(re, origin, dir, side, None) else { return false };
        let mut s = Selection::new(SelectionType::Simple, *m.start(), Side::Left);
        s.update(*m.end(), Side::Right);
        self.term.selection = Some(s);
        // Bring the match into view (a third of the way down).
        let (line, off) = (m.start().line.0, self.term.grid().display_offset() as i32);
        if line < -off || line >= self.rows as i32 - off { self.term.scroll_display(Scroll::Delta(-off - line + i32::from(self.rows) / 3)); }
        true
    }

    // ---------------------------------------------------------------- input encoding
    /// Text pasted into the terminal (bracketed when the program asked for it).
    pub fn paste_bytes(&self, text: &str, ignore_bracketed: bool) -> Vec<u8> {
        if self.mode().contains(TermMode::BRACKETED_PASTE) && !ignore_bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace(['\x1b', '\x03'], "")).into_bytes()
        } else { text.replace("\r\n", "\r").replace('\n', "\r").into_bytes() }
    }

    /// The bytes a key press sends, or None when the key isn't the terminal's (plain text arrives as text events).
    pub fn key_bytes(&self, key: Key, m: Modifiers) -> Option<Vec<u8>> {
        let app = self.mode().contains(TermMode::APP_CURSOR);
        // xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4).
        let mods = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
        let csi = |ch: char| if mods > 1 { format!("\x1b[1;{mods}{ch}") } else if app { format!("\x1bO{ch}") } else { format!("\x1b[{ch}") };
        let tilde = |n: u8| if mods > 1 { format!("\x1b[{n};{mods}~") } else { format!("\x1b[{n}~") };
        let ss3 = |ch: char| if mods > 1 { format!("\x1b[1;{mods}{ch}") } else { format!("\x1bO{ch}") };
        let s = match key {
            Key::Enter => if m.alt || (m.shift && !m.ctrl) { "\x1b\r".into() } else { "\r".into() }, // Shift+Enter: newline without submitting (Claude Code, agy)
            Key::Tab => if m.shift { "\x1b[Z".into() } else { "\t".into() },
            Key::Backspace => if m.alt { "\x1b\x7f".into() } else if m.ctrl { "\x08".into() } else { "\x7f".into() },
            Key::Escape => "\x1b".into(),
            Key::ArrowUp => csi('A'), Key::ArrowDown => csi('B'), Key::ArrowRight => csi('C'), Key::ArrowLeft => csi('D'),
            Key::Home => csi('H'), Key::End => csi('F'),
            Key::Insert => tilde(2), Key::Delete => tilde(3), Key::PageUp => tilde(5), Key::PageDown => tilde(6),
            Key::F1 => ss3('P'), Key::F2 => ss3('Q'), Key::F3 => ss3('R'), Key::F4 => ss3('S'),
            Key::F5 => tilde(15), Key::F6 => tilde(17), Key::F7 => tilde(18), Key::F8 => tilde(19), Key::F9 => tilde(20), Key::F10 => tilde(21), Key::F11 => tilde(23), Key::F12 => tilde(24),
            Key::Space if m.ctrl => "\0".into(),
            _ => {
                let name = key.name();
                let ch = if name.len() == 1 { name.chars().next() } else { match key { Key::OpenBracket => Some('['), Key::CloseBracket => Some(']'), Key::Backslash => Some('\\'), Key::Slash => Some('/'), Key::Minus => Some('-'), _ => None } }?;
                if m.ctrl && !m.alt {
                    // Ctrl+letter and friends: the control characters.
                    let b = match ch { 'A'..='Z' => ch as u8 - b'A' + 1, '[' => 0x1b, '\\' => 0x1c, ']' => 0x1d, '/' | '-' => 0x1f, '2' => 0, '6' => 0x1e, _ => return None };
                    return Some(vec![b]);
                }
                if m.alt && !m.ctrl && !cfg!(target_os = "macos") {
                    // Alt as Meta: ESC, then the key.
                    let c = if m.shift { ch } else { ch.to_ascii_lowercase() };
                    return Some(format!("\x1b{c}").into_bytes());
                }
                return None;
            }
        };
        Some(s.into_bytes())
    }

    /// A mouse report for programs that asked for the mouse (Claude Code's full-screen UI), or None.
    /// button: 0 left, 1 middle, 2 right, 64/65 wheel up/down.
    pub fn mouse_report(&self, kind: MouseKind, button: u8, col: u16, row: u16, m: Modifiers) -> Option<Vec<u8>> {
        let mode = self.mode();
        if !mode.intersects(TermMode::MOUSE_MODE) { return None; }
        if kind == MouseKind::Drag && !mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) { return None; }
        let mut b = u32::from(button) + 4 * u32::from(m.shift) + 8 * u32::from(m.alt) + 16 * u32::from(m.ctrl);
        if kind == MouseKind::Drag { b += 32; }
        let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
        if mode.contains(TermMode::SGR_MOUSE) {
            return Some(format!("\x1b[<{b};{x};{y}{}", if kind == MouseKind::Release { 'm' } else { 'M' }).into_bytes());
        }
        if x > 223 || y > 223 { return None; }
        let b = if kind == MouseKind::Release { 3 + (b & !3) } else { b };
        Some(vec![0x1b, b'[', b'M', (32 + b) as u8, (32 + x) as u8, (32 + y) as u8])
    }

    /// Wheel over the alternate screen without mouse reporting: programs like less get arrow keys instead.
    pub fn wheel_as_arrows(&self, lines: i32) -> Option<Vec<u8>> {
        let mode = self.mode();
        if !mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) || mode.intersects(TermMode::MOUSE_MODE) { return None; }
        let key = if mode.contains(TermMode::APP_CURSOR) { if lines > 0 { "\x1bOA" } else { "\x1bOB" } } else if lines > 0 { "\x1b[A" } else { "\x1b[B" };
        Some(key.repeat(lines.unsigned_abs() as usize).into_bytes())
    }

    // ---------------------------------------------------------------- pointer -> grid
    pub fn cell_at(&self, rect: Rect, cell: Vec2, pos: Pos2) -> (u16, u16, Side) {
        let fx = ((pos.x - rect.left()) / cell.x).max(0.0);
        let col = (fx as u16).min(self.cols.saturating_sub(1));
        let row = (((pos.y - rect.top()) / cell.y).max(0.0) as u16).min(self.rows.saturating_sub(1));
        (col, row, if fx.fract() < 0.5 { Side::Left } else { Side::Right })
    }
    fn point(&self, col: u16, row: u16) -> Point { Point::new(Line(i32::from(row) - self.term.grid().display_offset() as i32), Column(col as usize)) }
    /// clicks: 1 = character, 2 = word, 3 = line.
    pub fn start_selection(&mut self, col: u16, row: u16, side: Side, clicks: u8) {
        let ty = match clicks { 2 => SelectionType::Semantic, 3 => SelectionType::Lines, _ => SelectionType::Simple };
        self.term.selection = Some(Selection::new(ty, self.point(col, row), side));
        self.dragging = true;
    }
    pub fn drag_selection(&mut self, col: u16, row: u16, side: Side) {
        if !self.dragging { return; }
        let p = self.point(col, row);
        if let Some(s) = self.term.selection.as_mut() { s.update(p, side); }
    }
    pub fn end_selection(&mut self) {
        self.dragging = false;
        if self.term.selection.as_ref().is_some_and(Selection::is_empty) { self.term.selection = None; }
    }
    pub fn is_dragging(&self) -> bool { self.dragging }

    // ---------------------------------------------------------------- painting
    fn color(&self, c: Color) -> Color32 {
        let over = |i: usize| self.term.colors()[i].map(|c| Color32::from_rgb(c.r, c.g, c.b));
        match c {
            Color::Spec(c) => Color32::from_rgb(c.r, c.g, c.b),
            Color::Named(n) => over(n as usize).unwrap_or_else(|| named(n)),
            Color::Indexed(i) => over(i as usize).unwrap_or_else(|| indexed(i)),
        }
    }

    /// Paint the visible grid into `rect`. `cell` is one character cell; `accent` colours the cursor.
    pub fn paint(&self, painter: &egui::Painter, rect: Rect, font: &FontId, bold: Option<&FontId>, cell: Vec2, row_h: f32, focused: bool, accent: Color32, blink_on: bool) {
        let content = self.term.renderable_content();
        let off = content.display_offset as i32;
        let y_pad = ((cell.y - row_h) / 2.0).max(0.0);
        let origin = rect.left_top();
        // Runs of cells with the same look are drawn as one string; anything that isn't plain ASCII is placed cell by cell
        // so fallback fonts with other widths can't shift the rest of the line.
        struct Run { row: i32, col: usize, text: String, fg: Color32, bold: bool, last: usize }
        let mut run: Option<Run> = None;
        let flush = |r: Option<Run>| {
            let Some(r) = r else { return };
            let pos = origin + Vec2::new(r.col as f32 * cell.x, r.row as f32 * cell.y + y_pad);
            // Braille patterns (spinners, sparklines) are drawn as dots: few fonts carry them.
            if let Some(bits) = r.text.chars().next().map(|c| c as u32).filter(|c| (0x2800..=0x28ff).contains(c)).map(|c| c - 0x2800) {
                let cell_min = origin + Vec2::new(r.col as f32 * cell.x, r.row as f32 * cell.y);
                for bit in 0..8u32 {
                    if bits & (1 << bit) == 0 { continue; }
                    let (cx, cy) = match bit { 0 => (0, 0), 1 => (0, 1), 2 => (0, 2), 3 => (1, 0), 4 => (1, 1), 5 => (1, 2), 6 => (0, 3), _ => (1, 3) };
                    painter.circle_filled(cell_min + Vec2::new(cell.x * (0.3 + 0.4 * cx as f32), cell.y * (0.2 + 0.2 * cy as f32)), (cell.x * 0.11).max(0.8), r.fg);
                }
                return;
            }
            match (r.bold, bold) {
                (true, Some(b)) => { painter.text(pos, egui::Align2::LEFT_TOP, r.text, b.clone(), r.fg); }
                (true, None) => {
                    // No bold face in this font: double-strike, as terminals have always done.
                    painter.text(pos, egui::Align2::LEFT_TOP, &r.text, font.clone(), r.fg);
                    painter.text(pos + Vec2::new(0.6, 0.0), egui::Align2::LEFT_TOP, r.text, font.clone(), r.fg);
                }
                _ => { painter.text(pos, egui::Align2::LEFT_TOP, r.text, font.clone(), r.fg); }
            }
        };
        for ind in content.display_iter {
            let (c, p) = (ind.cell, ind.point);
            let row = p.line.0 + off;
            if row < 0 || row >= i32::from(self.rows) { continue; }
            let col = p.column.0;
            if c.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) { continue; }
            let (mut fg, mut bg) = (self.color(c.fg), self.color(c.bg));
            if c.flags.contains(Flags::INVERSE) { std::mem::swap(&mut fg, &mut bg); }
            if c.flags.contains(Flags::DIM) { fg = dim(fg); }
            let selected = content.selection.is_some_and(|s| s.contains(p));
            if selected { bg = SELECTION; }
            let w = if c.flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 };
            let x = origin.x + col as f32 * cell.x;
            let y = origin.y + row as f32 * cell.y;
            if bg != BG { painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell.x * w + 0.5, cell.y + 0.5)), 0.0, bg); }
            if c.flags.intersects(Flags::ALL_UNDERLINES) { painter.line_segment([Pos2::new(x, y + cell.y - 1.5), Pos2::new(x + cell.x * w, y + cell.y - 1.5)], Stroke::new(1.0, fg)); }
            if c.flags.contains(Flags::STRIKEOUT) { painter.line_segment([Pos2::new(x, y + cell.y / 2.0), Pos2::new(x + cell.x * w, y + cell.y / 2.0)], Stroke::new(1.0, fg)); }
            let ch = c.c;
            if ch == ' ' || ch == '\0' || c.flags.contains(Flags::HIDDEN) { continue; }
            let is_bold = c.flags.contains(Flags::BOLD);
            let simple = ch.is_ascii() && c.zerowidth().is_none();
            match run.as_mut() {
                Some(r) if simple && r.row == row && r.fg == fg && r.bold == is_bold && col > r.last && col - r.last <= 8 => {
                    for _ in r.last + 1..col { r.text.push(' '); }
                    r.text.push(ch);
                    r.last = col;
                }
                _ => {
                    flush(run.take());
                    let mut text = ch.to_string();
                    if let Some(z) = c.zerowidth() { text.extend(z); }
                    let r = Run { row, col, text, fg, bold: is_bold, last: col };
                    if simple { run = Some(r); } else { flush(Some(r)); }
                }
            }
        }
        flush(run.take());

        // Cursor: a bar while typing here, an outline when the pane isn't focused.
        let cur = content.cursor;
        let row = cur.point.line.0 + off;
        if cur.shape != CursorShape::Hidden && content.mode.contains(TermMode::SHOW_CURSOR) && (0..i32::from(self.rows)).contains(&row) {
            let r = Rect::from_min_size(origin + Vec2::new(cur.point.column.0 as f32 * cell.x, row as f32 * cell.y), cell);
            if !focused { painter.rect_stroke(r.shrink(0.5), 0.0, Stroke::new(1.0, accent.gamma_multiply(0.7)), egui::StrokeKind::Inside); }
            else if blink_on {
                match cur.shape {
                    CursorShape::Block => { painter.rect_filled(r, 0.0, accent.gamma_multiply(0.55)); }
                    CursorShape::Underline => { painter.rect_filled(Rect::from_min_max(Pos2::new(r.left(), r.bottom() - 2.0), r.max), 0.0, accent); }
                    _ => { painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(2.0, cell.y)), 0.0, accent); }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Modifiers { Modifiers::NONE }
    #[test]
    fn output_lands_in_the_grid_and_events_come_out() {
        let mut t = Terminal::new(40, 5);
        let out = t.feed(b"\x1b]0;my title\x07hello \x1b[31mworld\x1b[0m\r\n\x07");
        assert!(out.contains(&TermOut::Title("my title".into())) && out.contains(&TermOut::Bell));
        assert_eq!(t.lines_from(0)[0], "hello world");
        // Device attributes query gets an answer for the program.
        assert!(t.feed(b"\x1b[c").iter().any(|o| matches!(o, TermOut::Reply(b) if b.starts_with(b"\x1b[?"))));
        // OSC 52 write is surfaced; a read ("?") is not answered.
        assert!(t.feed(b"\x1b]52;c;aGk=\x07").contains(&TermOut::Clipboard("hi".into())));
        assert!(t.feed(b"\x1b]52;c;?\x07").is_empty());
    }
    #[test]
    fn osc_9_and_777_notifications_even_split_across_chunks() {
        let mut t = Terminal::new(40, 5);
        assert!(t.feed(b"\x1b]9;Build fini").is_empty());
        assert_eq!(t.feed(b"shed\x07"), [TermOut::Notify("Build finished".into())]);
        assert_eq!(t.feed(b"\x1b]777;notify;Claude;needs input\x1b\\"), [TermOut::Notify("needs input".into())]);
        assert!(t.feed(b"\x1b]9;4;1;50\x07").is_empty(), "progress reports are not messages");
    }
    #[test]
    fn keys_become_terminal_bytes() {
        let mut t = Terminal::new(40, 5);
        let k = |t: &Terminal, key, m| t.key_bytes(key, m).map(|b| String::from_utf8(b).unwrap());
        assert_eq!(k(&t, Key::ArrowUp, none()).unwrap(), "\x1b[A");
        assert_eq!(k(&t, Key::ArrowLeft, Modifiers::CTRL).unwrap(), "\x1b[1;5D");
        assert_eq!(k(&t, Key::C, Modifiers::CTRL).unwrap(), "\x03");
        assert_eq!(k(&t, Key::Enter, Modifiers::SHIFT).unwrap(), "\x1b\r");
        assert_eq!(k(&t, Key::Tab, Modifiers::SHIFT).unwrap(), "\x1b[Z");
        assert_eq!(k(&t, Key::Delete, none()).unwrap(), "\x1b[3~");
        assert_eq!(k(&t, Key::F5, none()).unwrap(), "\x1b[15~");
        assert_eq!(k(&t, Key::A, none()), None, "plain letters arrive as text");
        t.feed(b"\x1b[?1h"); // application cursor keys
        assert_eq!(k(&t, Key::ArrowUp, none()).unwrap(), "\x1bOA");
    }
    #[test]
    fn paste_is_bracketed_only_when_asked_and_cannot_break_out() {
        let mut t = Terminal::new(40, 5);
        assert_eq!(t.paste_bytes("a\nb", false), b"a\rb");
        t.feed(b"\x1b[?2004h");
        assert_eq!(t.paste_bytes("x\x1b[201~rm -rf", false), b"\x1b[200~x[201~rm -rf\x1b[201~");
        assert_eq!(t.paste_bytes("a\nb", true), b"a\rb", "Git Bash: brackets off");
    }
    #[test]
    fn mouse_reports_follow_the_mode() {
        let mut t = Terminal::new(40, 5);
        assert_eq!(t.mouse_report(MouseKind::Press, 0, 3, 1, none()), None);
        t.feed(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(t.mouse_report(MouseKind::Press, 0, 3, 1, none()).unwrap(), b"\x1b[<0;4;2M");
        assert_eq!(t.mouse_report(MouseKind::Release, 0, 3, 1, none()).unwrap(), b"\x1b[<0;4;2m");
        assert_eq!(t.mouse_report(MouseKind::Drag, 0, 3, 1, none()), None, "no motion reporting asked");
        assert_eq!(t.mouse_report(MouseKind::Press, 64, 0, 0, none()).unwrap(), b"\x1b[<64;1;1M");
    }
    #[test]
    fn selection_search_and_scrollback() {
        let mut t = Terminal::new(20, 4);
        for i in 0..30 { t.feed(format!("line {i} needle{i}\r\n").as_bytes()); }
        assert!(t.total_lines() > 4);
        assert!(t.find("needle7", false));
        assert_eq!(t.selection_text().unwrap(), "needle7");
        assert!(t.scrolled_back());
        assert!(!t.find("nope-not-here", false));
        t.clear_selection();
        t.scroll_to_bottom();
        t.start_selection(0, 0, Side::Left, 3);
        t.end_selection();
        assert!(t.selection_text().unwrap().starts_with("line 2"));
        t.select_all();
        assert!(t.selection_text().unwrap().contains("line 0 needle0"));
        t.clear_scrollback();
        assert_eq!(t.total_lines(), 4);
        assert!(t.resize(30, 6) && !t.resize(30, 6));
    }
}
