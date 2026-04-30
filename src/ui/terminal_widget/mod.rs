pub mod view;
pub use view::TerminalView;

use std::sync::Arc;

use std::ops::RangeInclusive;

use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use parking_lot::FairMutex;
use tokio::sync::mpsc::UnboundedSender;

use crate::proto::ssh::{SessionCommand, SessionEvent, SessionHandle, TunnelStatusMap};

const SCROLLBACK_LINES: usize = 10_000;

#[derive(Clone)]
struct UiListener {
    pty_tx: UnboundedSender<SessionCommand>,
}

impl EventListener for UiListener {
    fn send_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(text) = event {
            let _ = self.pty_tx.send(SessionCommand::Input(text.into_bytes()));
        }
    }
}

pub struct TerminalEmulator {
    term: Arc<FairMutex<Term<UiListener>>>,
    parser: Processor,
    cmd_tx: UnboundedSender<SessionCommand>,
    events_rx: tokio::sync::mpsc::UnboundedReceiver<SessionEvent>,
    tunnels: TunnelStatusMap,
    pub cols: u16,
    pub rows: u16,
    pub closed: Option<String>,
    pub find: FindState,
}

pub type FindMatch = RangeInclusive<Point>;

#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub just_opened: bool,
    pub matches: Vec<FindMatch>,
    pub current: Option<usize>,
    pub last_key: Option<String>,
}

impl FindState {
    pub fn close(&mut self) {
        self.open = false;
        self.matches.clear();
        self.current = None;
        self.last_key = None;
    }
}

impl TerminalEmulator {
    pub fn new(handle: SessionHandle, cols: u16, rows: u16) -> Self {
        let listener = UiListener {
            pty_tx: handle.commands.clone(),
        };
        let mut config = TermConfig::default();
        config.scrolling_history = SCROLLBACK_LINES;
        let size = TermSize::new(cols as usize, rows as usize);
        let term = Term::new(config, &size, listener);
        Self {
            term: Arc::new(FairMutex::new(term)),
            parser: Processor::new(),
            cmd_tx: handle.commands,
            events_rx: handle.events,
            tunnels: handle.tunnels,
            cols,
            rows,
            closed: None,
            find: FindState::default(),
        }
    }

    pub fn tunnels(&self) -> &TunnelStatusMap {
        &self.tunnels
    }

    pub fn pump(&mut self) -> bool {
        let mut new_data = false;
        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                SessionEvent::Output(bytes) => {
                    let mut term = self.term.lock();
                    self.parser.advance(&mut *term, &bytes);
                    new_data = true;
                }
                SessionEvent::Closed(reason) => {
                    self.closed = Some(reason.unwrap_or_else(|| "session closed".to_string()));
                    new_data = true;
                }
            }
        }
        new_data
    }

    pub fn send_input(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            let _ = self.cmd_tx.send(SessionCommand::Input(bytes));
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let size = TermSize::new(cols as usize, rows as usize);
        self.term.lock().resize(size);
        let _ = self.cmd_tx.send(SessionCommand::Resize { cols, rows });
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cursor_line = content.cursor.point.line.0 + content.display_offset as i32;
        let cursor_col = content.cursor.point.column.0;

        let cols = term.columns();
        let lines = term.screen_lines();
        let mut rows: Vec<Vec<RenderCell>> = (0..lines).map(|_| Vec::with_capacity(cols)).collect();

        let selection_range = content.selection;

        for indexed in content.display_iter {
            let line_idx = (indexed.point.line.0 + content.display_offset as i32) as usize;
            if line_idx >= rows.len() {
                continue;
            }
            let selected = selection_range
                .map(|r| r.contains(indexed.point))
                .unwrap_or(false);
            rows[line_idx].push(RenderCell {
                ch: indexed.cell.c,
                fg: resolve_color(indexed.cell.fg, true),
                bg: resolve_color(indexed.cell.bg, false),
                bold: indexed.cell.flags.contains(Flags::BOLD),
                italic: indexed.cell.flags.contains(Flags::ITALIC),
                underline: indexed.cell.flags.intersects(Flags::ALL_UNDERLINES),
                selected,
            });
        }

        TerminalSnapshot {
            rows,
            cursor: (cursor_line as usize, cursor_col),
            cursor_visible: !matches!(
                content.cursor.shape,
                alacritty_terminal::vte::ansi::CursorShape::Hidden
            ),
            display_offset: content.display_offset,
            history_size: term.history_size(),
        }
    }

    pub fn scroll(&mut self, delta_lines: i32) {
        if delta_lines == 0 {
            return;
        }
        self.term.lock().scroll_display(Scroll::Delta(delta_lines));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    pub fn begin_selection(&mut self, line: i32, col: usize, side: Side) {
        let mut term = self.term.lock();
        let cols = term.columns();
        let column = Column(col.min(cols.saturating_sub(1)));
        let point = Point::new(Line(line), column);
        term.selection = Some(Selection::new(SelectionType::Simple, point, side));
    }

    pub fn begin_semantic_selection(&mut self, line: i32, col: usize) {
        let mut term = self.term.lock();
        let cols = term.columns();
        let column = Column(col.min(cols.saturating_sub(1)));
        let point = Point::new(Line(line), column);
        term.selection = Some(Selection::new(SelectionType::Semantic, point, Side::Left));
    }

    pub fn update_selection(&mut self, line: i32, col: usize, side: Side) {
        let mut term = self.term.lock();
        let cols = term.columns();
        let column = Column(col.min(cols.saturating_sub(1)));
        let point = Point::new(Line(line), column);
        if let Some(selection) = term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.lock().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string()
    }

    pub fn open_find(&mut self) {
        self.find.open = true;
        self.find.just_opened = true;
    }

    pub fn close_find(&mut self) {
        self.find.close();
    }

    pub fn recompute_find_matches(&mut self) {
        let key = self.find.query.clone();
        if self.find.last_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.find.last_key = Some(key.clone());
        self.rebuild_find_matches(&key);
        if !self.find.matches.is_empty() {
            self.find.current = Some(0);
        }
    }

    pub fn refresh_find_matches(&mut self) {
        if !self.find.open || self.find.query.is_empty() {
            return;
        }
        let key = self.find.query.clone();
        let prev_anchor = self
            .find
            .current
            .and_then(|i| self.find.matches.get(i))
            .map(|r| *r.start());
        self.rebuild_find_matches(&key);
        self.find.current = if self.find.matches.is_empty() {
            None
        } else if let Some(anchor) = prev_anchor {
            Some(
                self.find
                    .matches
                    .iter()
                    .position(|r| *r.start() == anchor)
                    .unwrap_or_else(|| {
                        self.find
                            .matches
                            .iter()
                            .rposition(|r| *r.start() <= anchor)
                            .unwrap_or(0)
                    }),
            )
        } else {
            Some(0)
        };
    }

    fn rebuild_find_matches(&mut self, key: &str) {
        self.find.matches.clear();
        self.find.current = None;
        if key.is_empty() {
            return;
        }

        let case_insensitive = !key.chars().any(|c| c.is_uppercase());
        let needle: String = if case_insensitive {
            key.to_lowercase()
        } else {
            key.to_string()
        };

        let term = self.term.lock();
        let grid = term.grid();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        let cols = grid.columns();

        for line_idx in top..=bottom {
            let line = Line(line_idx);
            let row = &grid[line];
            let mut line_text = String::with_capacity(cols);
            let mut char_to_col: Vec<usize> = Vec::with_capacity(cols);
            for c in 0..cols {
                let cell = &row[Column(c)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                if case_insensitive {
                    for lower in cell.c.to_lowercase() {
                        line_text.push(lower);
                        char_to_col.push(c);
                    }
                } else {
                    line_text.push(cell.c);
                    char_to_col.push(c);
                }
            }

            let needle_char_len = needle.chars().count();
            let mut search_from = 0usize;
            while let Some(rel) = line_text[search_from..].find(&needle) {
                let byte_start = search_from + rel;
                let char_start = line_text[..byte_start].chars().count();
                let char_end = char_start + needle_char_len.saturating_sub(1);
                if let (Some(&sc), Some(&ec)) =
                    (char_to_col.get(char_start), char_to_col.get(char_end))
                {
                    self.find.matches.push(
                        Point::new(line, Column(sc))..=Point::new(line, Column(ec)),
                    );
                }
                search_from = byte_start + needle.len().max(1);
                if search_from >= line_text.len() {
                    break;
                }
            }
        }
    }

    pub fn find_goto(&mut self, next: bool) {
        if self.find.matches.is_empty() {
            return;
        }
        let n = self.find.matches.len();
        let idx = match self.find.current {
            Some(i) => {
                if next {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                }
            }
            None => 0,
        };
        self.find.current = Some(idx);
        let point = *self.find.matches[idx].start();
        self.term.lock().scroll_to_point(point);
    }

    pub fn find_scroll_to_current(&mut self) {
        if let Some(idx) = self.find.current
            && let Some(m) = self.find.matches.get(idx)
        {
            let point = *m.start();
            self.term.lock().scroll_to_point(point);
        }
    }

    pub fn display_offset(&self) -> usize {
        self.term.lock().grid().display_offset()
    }
}

pub struct TerminalSnapshot {
    pub rows: Vec<Vec<RenderCell>>,
    pub cursor: (usize, usize),
    pub cursor_visible: bool,
    pub display_offset: usize,
    pub history_size: usize,
}

#[derive(Clone, Copy)]
pub struct RenderCell {
    pub ch: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub selected: bool,
}

fn resolve_color(color: Color, foreground: bool) -> [u8; 3] {
    match color {
        Color::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
        Color::Named(named) => named_color(named, foreground),
        Color::Indexed(idx) => indexed_color(idx, foreground),
    }
}

fn named_color(c: NamedColor, foreground: bool) -> [u8; 3] {
    match c {
        NamedColor::Foreground => [0xea, 0xea, 0xea],
        NamedColor::Background => [0x12, 0x12, 0x14],
        NamedColor::Black | NamedColor::DimBlack => [0x00, 0x00, 0x00],
        NamedColor::Red | NamedColor::DimRed => [0xcd, 0x31, 0x31],
        NamedColor::Green | NamedColor::DimGreen => [0x0d, 0xbc, 0x79],
        NamedColor::Yellow | NamedColor::DimYellow => [0xe5, 0xe5, 0x10],
        NamedColor::Blue | NamedColor::DimBlue => [0x24, 0x72, 0xc8],
        NamedColor::Magenta | NamedColor::DimMagenta => [0xbc, 0x3f, 0xbc],
        NamedColor::Cyan | NamedColor::DimCyan => [0x11, 0xa8, 0xcd],
        NamedColor::White | NamedColor::DimWhite => [0xe5, 0xe5, 0xe5],
        NamedColor::BrightBlack => [0x66, 0x66, 0x66],
        NamedColor::BrightRed => [0xf1, 0x4c, 0x4c],
        NamedColor::BrightGreen => [0x23, 0xd1, 0x8b],
        NamedColor::BrightYellow => [0xf5, 0xf5, 0x43],
        NamedColor::BrightBlue => [0x3b, 0x8e, 0xea],
        NamedColor::BrightMagenta => [0xd6, 0x70, 0xd6],
        NamedColor::BrightCyan => [0x29, 0xb8, 0xdb],
        NamedColor::BrightWhite => [0xff, 0xff, 0xff],
        NamedColor::Cursor => [0xea, 0xea, 0xea],
        NamedColor::DimForeground => [0xa0, 0xa0, 0xa0],
        NamedColor::BrightForeground => {
            if foreground {
                [0xff, 0xff, 0xff]
            } else {
                [0xea, 0xea, 0xea]
            }
        }
    }
}

fn indexed_color(idx: u8, foreground: bool) -> [u8; 3] {
    if idx < 16 {
        let named = match idx {
            0 => NamedColor::Black,
            1 => NamedColor::Red,
            2 => NamedColor::Green,
            3 => NamedColor::Yellow,
            4 => NamedColor::Blue,
            5 => NamedColor::Magenta,
            6 => NamedColor::Cyan,
            7 => NamedColor::White,
            8 => NamedColor::BrightBlack,
            9 => NamedColor::BrightRed,
            10 => NamedColor::BrightGreen,
            11 => NamedColor::BrightYellow,
            12 => NamedColor::BrightBlue,
            13 => NamedColor::BrightMagenta,
            14 => NamedColor::BrightCyan,
            _ => NamedColor::BrightWhite,
        };
        return named_color(named, foreground);
    }
    if idx < 232 {
        let i = idx - 16;
        let r = (i / 36) % 6;
        let g = (i / 6) % 6;
        let b = i % 6;
        let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        return [scale(r), scale(g), scale(b)];
    }
    let v = 8 + (idx - 232) * 10;
    [v, v, v]
}
