//! ANSI/VT terminal emulator for the in-GUI Terminal.
//!
//! This is deliberately closer to a real terminal than a line-oriented text
//! widget: it maintains an 80x26 cell grid, parses CSI/OSC escape sequences,
//! supports the alternate screen and scrollback, and can attach the grid to a
//! pseudoterminal hosted by the Android libOS runtime.  That is the minimum
//! needed for full-screen TUIs such as opencode, vim, htop or lazygit.
//!
//! The emulator is allocation-free.  The screen and scrollback live in fixed
//! static arrays so it works on bare metal, iOS and Android without changing
//! the userland's `no_std` contract.

use crate::draw::Screen;
use crate::font_data;
use crate::theme;
use libkimi as k;

pub const COLS: usize = 80;
pub const ROWS: usize = 26;
const MAX_ROWS: usize = 40;
const SCROLLBACK_LINES: usize = 256;
const RESPONSE_CAP: usize = 256;
const TITLE_CAP: usize = 96;

const DEFAULT_FG: u32 = 0x00e8e8ef;
const DEFAULT_BG: u32 = 0x000b0b10;
const CURSOR_FG: u32 = 0x000b0b10;

const FLAG_BOLD: u8 = 0x01;
const FLAG_UNDERLINE: u8 = 0x02;
const FLAG_REVERSE: u8 = 0x04;

#[derive(Clone, Copy)]
struct Cell {
    ch: u32,
    fg: u32,
    bg: u32,
    flags: u8,
}

const BLANK: Cell = Cell {
    // Zeroed cells keep the large grids in .bss instead of bloating the ELF
    // with hundreds of kilobytes of literal spaces.
    ch: 0,
    fg: 0,
    bg: 0,
    flags: 0,
};
const BLANK_ROW: [Cell; COLS] = [BLANK; COLS];

#[allow(static_mut_refs)]
static mut MAIN_GRID: [[Cell; COLS]; MAX_ROWS] = [[BLANK; COLS]; MAX_ROWS];
#[allow(static_mut_refs)]
static mut ALT_GRID: [[Cell; COLS]; MAX_ROWS] = [[BLANK; COLS]; MAX_ROWS];
#[allow(static_mut_refs)]
static mut HISTORY: [[Cell; COLS]; SCROLLBACK_LINES] = [[BLANK; COLS]; SCROLLBACK_LINES];
static mut HISTORY_HEAD: usize = 0;
static mut HISTORY_COUNT: usize = 0;
static mut RESPONSE: [u8; RESPONSE_CAP] = [0; RESPONSE_CAP];
static mut RESPONSE_LEN: usize = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseState {
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
    Charset,
}

pub struct Terminal {
    /// Local line editor, used when no guest PTY is attached.
    pub input: [u8; 192],
    pub input_len: usize,
    pub ctrl: bool,
    pub title: [u8; TITLE_CAP],
    pub title_len: usize,
    pub guest: u64,
    pub guest_exit: Option<u64>,
    pub scroll_offset: usize,
    parser: ParseState,
    params: [i32; 16],
    param_count: usize,
    private_marker: u8,
    osc: [u8; 256],
    osc_len: usize,
    utf8: [u8; 4],
    utf8_len: usize,
    utf8_need: usize,
    cx: usize,
    cy: usize,
    saved_cx: usize,
    saved_cy: usize,
    scroll_top: usize,
    scroll_bottom: usize,
    fg: u32,
    bg: u32,
    flags: u8,
    autowrap: bool,
    insert_mode: bool,
    origin_mode: bool,
    cursor_visible: bool,
    using_alt: bool,
    bracketed_paste: bool,
    app_cursor: bool,
    wrap_pending: bool,
    dirty: bool,
}

impl Terminal {
    pub fn new() -> Self {
        let mut term = Self {
            input: [0; 192],
            input_len: 0,
            ctrl: false,
            title: [0; TITLE_CAP],
            title_len: 0,
            guest: 0,
            guest_exit: None,
            scroll_offset: 0,
            parser: ParseState::Ground,
            params: [0; 16],
            param_count: 0,
            private_marker: 0,
            osc: [0; 256],
            osc_len: 0,
            utf8: [0; 4],
            utf8_len: 0,
            utf8_need: 0,
            cx: 0,
            cy: 0,
            saved_cx: 0,
            saved_cy: 0,
            scroll_top: 0,
            scroll_bottom: ROWS - 1,
            fg: 0,
            bg: 0,
            flags: 0,
            autowrap: true,
            insert_mode: false,
            origin_mode: false,
            cursor_visible: true,
            using_alt: false,
            bracketed_paste: false,
            app_cursor: false,
            wrap_pending: false,
            dirty: true,
        };
        term.reset();
        term.feed(b"KimiOS Terminal\r\nVT100/ANSI + PTY ready.\r\nType a command, or `opencode` to start a full-screen TUI.\r\n");
        term
    }

    pub fn is_guest(&self) -> bool { self.guest != 0 }
    fn set_title(&mut self, data: &[u8]) {
        // OSC 0 / OSC 2 payload is "title", sometimes preceded by "0;".
        let text = if data.len() > 2 && (data[0] == b'0' || data[0] == b'2') && data[1] == b';' {
            &data[2..]
        } else {
            data
        };
        self.title_len = text.len().min(TITLE_CAP);
        self.title[..self.title_len].copy_from_slice(&text[..self.title_len]);
    }

    #[allow(static_mut_refs)]
    fn reset(&mut self) {
        unsafe {
            for row in MAIN_GRID.iter_mut() { *row = BLANK_ROW; }
            for row in ALT_GRID.iter_mut() { *row = BLANK_ROW; }
            HISTORY_HEAD = 0;
            HISTORY_COUNT = 0;
            RESPONSE_LEN = 0;
        }
        self.cx = 0;
        self.cy = 0;
        self.saved_cx = 0;
        self.saved_cy = 0;
        self.scroll_top = 0;
        self.scroll_bottom = ROWS - 1;
        self.fg = 0;
        self.bg = 0;
        self.flags = 0;
        self.autowrap = true;
        self.insert_mode = false;
        self.origin_mode = false;
        self.cursor_visible = true;
        self.using_alt = false;
        self.bracketed_paste = false;
        self.app_cursor = false;
        self.wrap_pending = false;
        self.scroll_offset = 0;
        self.parser = ParseState::Ground;
        self.param_count = 0;
        self.private_marker = 0;
        self.osc_len = 0;
        self.utf8_len = 0;
        self.utf8_need = 0;
        self.dirty = true;
    }

    #[allow(static_mut_refs)]
    fn grid(&self) -> &'static [[Cell; COLS]; MAX_ROWS] {
        unsafe { if self.using_alt { &ALT_GRID } else { &MAIN_GRID } }
    }

    #[allow(static_mut_refs)]
    fn grid_mut(&mut self) -> &'static mut [[Cell; COLS]; MAX_ROWS] {
        unsafe { if self.using_alt { &mut ALT_GRID } else { &mut MAIN_GRID } }
    }

    fn active_rows(&self) -> usize { ROWS }

    fn clear_row(grid: &mut [[Cell; COLS]; MAX_ROWS], row: usize) {
        if row < MAX_ROWS { grid[row] = BLANK_ROW; }
    }

    #[allow(static_mut_refs)]
    fn push_history(&mut self, line: &[Cell; COLS]) {
        if self.using_alt { return; }
        unsafe {
            let slot = HISTORY_HEAD;
            HISTORY[slot] = *line;
            HISTORY_HEAD = (HISTORY_HEAD + 1) % SCROLLBACK_LINES;
            if HISTORY_COUNT < SCROLLBACK_LINES { HISTORY_COUNT += 1; }
        }
    }

    #[allow(static_mut_refs)]
    fn history_line(&self, index: usize) -> Option<[Cell; COLS]> {
        unsafe {
            if index >= HISTORY_COUNT { return None; }
            let start = (HISTORY_HEAD + SCROLLBACK_LINES - HISTORY_COUNT) % SCROLLBACK_LINES;
            let slot = (start + index) % SCROLLBACK_LINES;
            Some(HISTORY[slot])
        }
    }

    fn line_at(&self, visible_index: usize) -> [Cell; COLS] {
        if self.scroll_offset == 0 {
            self.grid()[visible_index.min(MAX_ROWS - 1)]
        } else {
            let total = unsafe { HISTORY_COUNT } + self.active_rows();
            let start = total.saturating_sub(self.active_rows() + self.scroll_offset);
            let index = start + visible_index;
            if index < unsafe { HISTORY_COUNT } {
                self.history_line(index).unwrap_or(BLANK_ROW)
            } else {
                let row = index - unsafe { HISTORY_COUNT };
                self.grid()[row.min(MAX_ROWS - 1)]
            }
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        let max = unsafe { HISTORY_COUNT };
        self.scroll_offset = (self.scroll_offset + lines).min(max);
        self.dirty = true;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.dirty = true;
    }

    fn scroll_to_bottom(&mut self) {
        if self.scroll_offset != 0 { self.scroll_offset = 0; self.dirty = true; }
    }

    pub fn clear(&mut self) {
        self.parser = ParseState::Ground;
        self.param_count = 0;
        self.using_alt = false;
        self.cx = 0;
        self.cy = 0;
        self.scroll_top = 0;
        self.scroll_bottom = ROWS - 1;
        unsafe {
            for row in MAIN_GRID.iter_mut() { *row = BLANK_ROW; }
            for row in ALT_GRID.iter_mut() { *row = BLANK_ROW; }
            HISTORY_HEAD = 0;
            HISTORY_COUNT = 0;
        }
        self.scroll_offset = 0;
        self.dirty = true;
    }

    // -- parser -------------------------------------------------------------

    pub fn feed_text(&mut self, text: &str) { self.feed(text.as_bytes()); }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes { self.feed_byte(byte); }
        self.dirty = true;
    }

    fn feed_byte(&mut self, byte: u8) {
        if self.utf8_need != 0 {
            if byte & 0xc0 == 0x80 {
                self.utf8[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    let cp = core::str::from_utf8(&self.utf8[..self.utf8_len])
                        .ok()
                        .and_then(|s| s.chars().next())
                        .map(|c| c as u32)
                        .unwrap_or(b'?' as u32);
                    self.utf8_len = 0;
                    self.utf8_need = 0;
                    self.put_char(cp);
                }
                return;
            }
            // Truncated sequence: emit replacement and reprocess this byte.
            self.utf8_len = 0;
            self.utf8_need = 0;
            self.put_char(b'?' as u32);
        }
        if byte >= 0xc2 {
            self.utf8[0] = byte;
            self.utf8_len = 1;
            self.utf8_need = if byte < 0xe0 { 2 } else if byte < 0xf0 { 3 } else { 4 };
            return;
        }
        if byte >= 0x80 { self.put_char(b'?' as u32); return; }

        match self.parser {
            ParseState::Ground => self.ground(byte),
            ParseState::Escape => self.escape(byte),
            ParseState::Csi => self.csi(byte),
            ParseState::Osc => self.osc(byte),
            ParseState::OscEscape => {
                if byte == b'\\' {
                    let len = self.osc_len;
                    let mut tmp = [0u8; 256];
                    tmp[..len].copy_from_slice(&self.osc[..len]);
                    self.set_title(&tmp[..len]);
                    self.osc_len = 0;
                    self.parser = ParseState::Ground;
                } else {
                    self.osc_push(byte);
                    self.parser = ParseState::Osc;
                }
            }
            ParseState::Charset => self.parser = ParseState::Ground,
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            0x07 => {}
            0x08 => {
                if self.cx > 0 { self.cx -= 1; }
                self.wrap_pending = false;
            }
            0x09 => self.tab_forward(1),
            0x0a | 0x0b | 0x0c => {
                self.linefeed();
                self.wrap_pending = false;
            }
            0x0d => {
                self.cx = 0;
                self.wrap_pending = false;
            }
            0x1b => {
                self.parser = ParseState::Escape;
                self.param_count = 0;
                self.private_marker = 0;
            }
            0x00..=0x1f | 0x7f => {}
            _ => self.put_char(byte as u32),
        }
    }

    fn escape(&mut self, byte: u8) {
        self.parser = ParseState::Ground;
        match byte {
            b'[' => { self.parser = ParseState::Csi; self.param_count = 0; self.private_marker = 0; }
            b']' => { self.parser = ParseState::Osc; self.osc_len = 0; }
            b'(' | b')' | b'*' | b'+' => self.parser = ParseState::Charset,
            b'7' => { self.saved_cx = self.cx; self.saved_cy = self.cy; }
            b'8' => { self.cx = self.saved_cx.min(COLS - 1); self.cy = self.saved_cy.min(ROWS - 1); }
            b'D' => self.linefeed(),
            b'E' => { self.cx = 0; self.linefeed(); }
            b'M' => self.reverse_index(),
            b'c' => self.reset(),
            b'=' | b'>' => {}
            _ => {}
        }
    }

    fn osc_push(&mut self, byte: u8) {
        if self.osc_len < self.osc.len() {
            self.osc[self.osc_len] = byte;
            self.osc_len += 1;
        }
    }

    fn osc(&mut self, byte: u8) {
        match byte {
            0x07 => {
                let len = self.osc_len;
                let mut tmp = [0u8; 256];
                tmp[..len].copy_from_slice(&self.osc[..len]);
                self.set_title(&tmp[..len]);
                self.osc_len = 0;
                self.parser = ParseState::Ground;
            }
            0x1b => self.parser = ParseState::OscEscape,
            _ => self.osc_push(byte),
        }
    }

    fn param(&self, index: usize, default: i32) -> i32 {
        if index < self.param_count && self.params[index] > 0 {
            self.params[index]
        } else {
            default
        }
    }

    fn current_param_mut(&mut self) -> &mut i32 {
        if self.param_count == 0 {
            self.param_count = 1;
            self.params[0] = 0;
        }
        &mut self.params[self.param_count - 1]
    }

    fn csi(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                let p = self.current_param_mut();
                *p = p.saturating_mul(10).saturating_add((byte - b'0') as i32);
            }
            b';' => {
                if self.param_count < 16 { self.param_count += 1; self.params[self.param_count - 1] = 0; }
            }
            b'?' | b'>' | b'!' => self.private_marker = byte,
            b' '..=b'/' => {}
            0x40..=0x7e => self.csi_dispatch(byte),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, final_byte: u8) {
        let private = self.private_marker;
        let count = self.param_count;
        let mut params = [0i32; 16];
        params[..count].copy_from_slice(&self.params[..count]);

        match final_byte {
            b'A' => self.cy = self.cy.saturating_sub(self.param(0, 1).max(1) as usize),
            b'B' | b'e' => self.move_down(self.param(0, 1).max(1) as usize),
            b'C' | b'a' => self.cx = (self.cx + self.param(0, 1).max(1) as usize).min(COLS - 1),
            b'D' => self.cx = self.cx.saturating_sub(self.param(0, 1).max(1) as usize),
            b'E' => { self.cx = 0; self.move_down(self.param(0, 1).max(1) as usize); }
            b'F' => { self.cx = 0; self.cy = self.cy.saturating_sub(self.param(0, 1).max(1) as usize); }
            b'G' | b'`' => self.cx = (self.param(0, 1).max(1) as usize - 1).min(COLS - 1),
            b'd' => self.cy = (self.param(0, 1).max(1) as usize - 1).min(self.scroll_bottom),
            b'H' | b'f' => {
                self.cy = (self.param(0, 1).max(1) as usize - 1)
                    .min(self.scroll_bottom);
                self.cx = (self.param(1, 1).max(1) as usize - 1).min(COLS - 1);
                self.wrap_pending = false;
            }
            b'J' => self.erase_display(self.param(0, 0)),
            b'K' => self.erase_line(self.param(0, 0)),
            b'L' => self.insert_lines(self.param(0, 1).max(1) as usize),
            b'M' => self.delete_lines(self.param(0, 1).max(1) as usize),
            b'P' => self.delete_chars(self.param(0, 1).max(1) as usize),
            b'@' => self.insert_chars(self.param(0, 1).max(1) as usize),
            b'X' => self.erase_chars(self.param(0, 1).max(1) as usize),
            b'S' => self.scroll_up_region(self.param(0, 1).max(1) as usize),
            b'T' => self.scroll_down_region(self.param(0, 1).max(1) as usize),
            b'm' => self.select_graphic_rendition(&params[..count]),
            b'r' => {
                let top = self.param(0, 1).max(1) as usize - 1;
                let bottom = self.param(1, ROWS as i32).max(1) as usize - 1;
                if top < bottom && bottom < ROWS {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.cx = 0;
                    self.cy = if self.origin_mode { top } else { 0 };
                }
            }
            b's' => { self.saved_cx = self.cx; self.saved_cy = self.cy; }
            b'u' => { self.cx = self.saved_cx.min(COLS - 1); self.cy = self.saved_cy.min(ROWS - 1); }
            b'h' => self.set_modes(private, true),
            b'l' => self.set_modes(private, false),
            b'n' => self.report_status(private, &params[..count]),
            b'c' => self.queue_response(b"\x1b[?1;2c"),
            b'Z' => self.tab_backward(self.param(0, 1).max(1) as usize),
            b'I' => self.tab_forward(self.param(0, 1).max(1) as usize),
            b'b' => {
                let n = self.param(0, 1).max(1) as usize;
                let ch = self.last_char();
                for _ in 0..n { self.put_char(ch); }
            }
            b'g' => {}
            b't' => {}
            _ => {}
        }
        self.parser = ParseState::Ground;
        self.param_count = 0;
        self.private_marker = 0;
    }

    fn set_modes(&mut self, private: u8, enabled: bool) {
        if private != b'?' {
            if self.param_count > 0 {
                match self.params[0] {
                    4 => self.insert_mode = enabled,
                    20 => {}
                    _ => {}
                }
            }
            return;
        }
        for i in 0..self.param_count.max(1) {
            match self.param(i, 0) {
                1 => self.app_cursor = enabled,
                7 => self.autowrap = enabled,
                25 => self.cursor_visible = enabled,
                47 | 1047 => self.set_alt_screen(enabled),
                1049 => {
                    if enabled {
                        self.saved_cx = self.cx;
                        self.saved_cy = self.cy;
                        self.set_alt_screen(true);
                    } else {
                        self.set_alt_screen(false);
                        self.cx = self.saved_cx.min(COLS - 1);
                        self.cy = self.saved_cy.min(ROWS - 1);
                    }
                }
                1000 | 1002 | 1003 | 1006 => {}
                2004 => self.bracketed_paste = enabled,
                _ => {}
            }
        }
    }

    fn set_alt_screen(&mut self, enabled: bool) {
        if self.using_alt == enabled { return; }
        self.using_alt = enabled;
        if enabled {
            unsafe { for row in ALT_GRID.iter_mut() { *row = BLANK_ROW; } }
            self.cx = 0;
            self.cy = 0;
            self.scroll_top = 0;
            self.scroll_bottom = ROWS - 1;
        }
        self.scroll_offset = 0;
        self.wrap_pending = false;
        self.dirty = true;
    }

    fn report_status(&mut self, private: u8, params: &[i32]) {
        let code = if !params.is_empty() { params[0] } else { 0 };
        match code {
            5 => self.queue_response(b"\x1b[0n"),
            6 => {
                let mut buf = [0u8; 32];
                let prefix: &[u8] = if private == b'?' { b"\x1b[?" } else { b"\x1b[" };
                let mut n = 0;
                for &b in prefix { buf[n] = b; n += 1; }
                n += put_num(&mut buf[n..], self.cy as u64 + 1);
                buf[n] = b';'; n += 1;
                n += put_num(&mut buf[n..], self.cx as u64 + 1);
                buf[n] = b'R'; n += 1;
                self.queue_response(&buf[..n]);
            }
            _ => {}
        }
    }

    fn queue_response(&mut self, bytes: &[u8]) {
        unsafe {
            let n = bytes.len().min(RESPONSE_CAP - RESPONSE_LEN);
            RESPONSE[RESPONSE_LEN..RESPONSE_LEN + n].copy_from_slice(&bytes[..n]);
            RESPONSE_LEN += n;
        }
    }

    #[allow(static_mut_refs)]
    pub fn take_response(&mut self, out: &mut [u8]) -> usize {
        unsafe {
            let n = RESPONSE_LEN.min(out.len());
            out[..n].copy_from_slice(&RESPONSE[..n]);
            if n == RESPONSE_LEN {
                RESPONSE_LEN = 0;
            } else {
                RESPONSE.copy_within(n..RESPONSE_LEN, 0);
                RESPONSE_LEN -= n;
            }
            n
        }
    }

    fn select_graphic_rendition(&mut self, params: &[i32]) {
        let mut i = 0usize;
        if params.is_empty() {
            self.fg = 0; self.bg = 0; self.flags = 0;
            return;
        }
        while i < params.len() {
            match params[i] {
                0 => { self.fg = 0; self.bg = 0; self.flags = 0; }
                1 => self.flags |= FLAG_BOLD,
                4 => self.flags |= FLAG_UNDERLINE,
                7 => self.flags |= FLAG_REVERSE,
                22 => self.flags &= !FLAG_BOLD,
                24 => self.flags &= !FLAG_UNDERLINE,
                27 => self.flags &= !FLAG_REVERSE,
                30..=37 => self.fg = ansi_color(params[i] as u32 - 30),
                39 => self.fg = 0,
                40..=47 => self.bg = ansi_color(params[i] as u32 - 40),
                49 => self.bg = 0,
                90..=97 => self.fg = ansi_color(params[i] as u32 - 90 + 8),
                100..=107 => self.bg = ansi_color(params[i] as u32 - 100 + 8),
                38 | 48 => {
                    let is_fg = params[i] == 38;
                    if i + 1 < params.len() && params[i + 1] == 5 && i + 2 < params.len() {
                        let color = ansi_color(params[i + 2].clamp(0, 255) as u32);
                        if is_fg { self.fg = color; } else { self.bg = color; }
                        i += 2;
                    } else if i + 1 < params.len() && params[i + 1] == 2 && i + 4 < params.len() {
                        let color = ((params[i + 2].clamp(0, 255) as u32) << 16)
                            | ((params[i + 3].clamp(0, 255) as u32) << 8)
                            | params[i + 4].clamp(0, 255) as u32;
                        if is_fg { self.fg = color; } else { self.bg = color; }
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    // -- writing ------------------------------------------------------------

    fn last_char(&self) -> u32 {
        if self.cx > 0 { self.grid()[self.cy][self.cx - 1].ch } else { b' ' as u32 }
    }

    fn put_char(&mut self, ch: u32) {
        let width = char_width(ch);
        if width == 0 { return; }
        self.scroll_to_bottom();
        if self.wrap_pending && self.autowrap {
            self.cx = 0;
            self.linefeed();
            self.wrap_pending = false;
        }
        if self.cx + width > COLS {
            if self.autowrap {
                self.cx = 0;
                self.linefeed();
            } else {
                self.cx = COLS.saturating_sub(width);
            }
        }
        let cy = self.cy;
        let cx = self.cx;
        if self.insert_mode {
            self.insert_chars(width);
        }
        let fg = self.fg;
        let bg = self.bg;
        let flags = self.flags;
        let grid = self.grid_mut();
        grid[cy][cx] = Cell { ch, fg, bg, flags };
        if width == 2 && cx + 1 < COLS {
            grid[cy][cx + 1] = Cell { ch: 0, fg, bg, flags };
        }
        if cx + width >= COLS {
            self.cx = COLS - 1;
            self.wrap_pending = true;
        } else {
            self.cx += width;
        }
        self.dirty = true;
    }

    fn linefeed(&mut self) {
        if self.cy == self.scroll_bottom {
            self.scroll_up_region(1);
        } else if self.cy + 1 < ROWS {
            self.cy += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cy == self.scroll_top {
            self.scroll_down_region(1);
        } else if self.cy > 0 {
            self.cy -= 1;
        }
    }

    fn move_down(&mut self, n: usize) {
        self.cy = (self.cy + n).min(self.scroll_bottom);
        self.wrap_pending = false;
    }

    fn tab_forward(&mut self, n: usize) {
        for _ in 0..n {
            let next = (self.cx / 8 + 1) * 8;
            self.cx = next.min(COLS - 1);
        }
    }

    fn tab_backward(&mut self, n: usize) {
        for _ in 0..n {
            self.cx = if self.cx == 0 { 0 } else { ((self.cx - 1) / 8) * 8 };
        }
    }

    fn scroll_up_region(&mut self, n: usize) {
        let top = self.scroll_top.min(ROWS - 1);
        let bottom = self.scroll_bottom.min(ROWS - 1);
        if top >= bottom { return; }
        for _ in 0..n {
            let grid = self.grid_mut();
            if self.scroll_top == 0 && !self.using_alt {
                let line = grid[top];
                self.push_history(&line);
            }
            for row in self.scroll_top..self.scroll_bottom {
                grid[row] = grid[row + 1];
            }
            Self::clear_row(grid, self.scroll_bottom);
        }
        self.dirty = true;
    }

    fn scroll_down_region(&mut self, n: usize) {
        let top = self.scroll_top.min(ROWS - 1);
        let bottom = self.scroll_bottom.min(ROWS - 1);
        if top >= bottom { return; }
        for _ in 0..n {
            let grid = self.grid_mut();
            for row in (top + 1..=bottom).rev() {
                grid[row] = grid[row - 1];
            }
            Self::clear_row(grid, top);
        }
        self.dirty = true;
    }

    fn erase_display(&mut self, mode: i32) {
        let cy = self.cy;
        let cx = self.cx;
        let grid = self.grid_mut();
        match mode {
            0 => {
                for col in cx..COLS { grid[cy][col] = BLANK; }
                for row in cy + 1..ROWS { Self::clear_row(grid, row); }
            }
            1 => {
                for row in 0..cy { Self::clear_row(grid, row); }
                for col in 0..=cx.min(COLS - 1) { grid[cy][col] = BLANK; }
            }
            2 => {
                for row in 0..ROWS { Self::clear_row(grid, row); }
                self.scroll_offset = 0;
            }
            3 => unsafe { HISTORY_HEAD = 0; HISTORY_COUNT = 0; self.scroll_offset = 0; },
            _ => {}
        }
        self.dirty = true;
    }

    fn erase_line(&mut self, mode: i32) {
        let cy = self.cy;
        let cx = self.cx;
        let grid = self.grid_mut();
        match mode {
            0 => for col in cx..COLS { grid[cy][col] = BLANK; },
            1 => for col in 0..=cx.min(COLS - 1) { grid[cy][col] = BLANK; },
            2 => Self::clear_row(grid, cy),
            _ => {}
        }
        self.dirty = true;
    }

    fn insert_lines(&mut self, n: usize) {
        if self.cy < self.scroll_top || self.cy > self.scroll_bottom { return; }
        for _ in 0..n {
            let grid = self.grid_mut();
            for row in (self.cy + 1..=self.scroll_bottom).rev() {
                grid[row] = grid[row - 1];
            }
            Self::clear_row(grid, self.cy);
        }
        self.dirty = true;
    }

    fn delete_lines(&mut self, n: usize) {
        if self.cy < self.scroll_top || self.cy > self.scroll_bottom { return; }
        for _ in 0..n {
            let grid = self.grid_mut();
            for row in self.cy..self.scroll_bottom {
                grid[row] = grid[row + 1];
            }
            Self::clear_row(grid, self.scroll_bottom);
        }
        self.dirty = true;
    }

    fn delete_chars(&mut self, n: usize) {
        let cy = self.cy;
        let cx = self.cx;
        let grid = self.grid_mut();
        let n = n.min(COLS - cx);
        for col in cx..COLS - n { grid[cy][col] = grid[cy][col + n]; }
        for col in COLS - n..COLS { grid[cy][col] = BLANK; }
        self.dirty = true;
    }

    fn insert_chars(&mut self, n: usize) {
        let cy = self.cy;
        let cx = self.cx;
        let grid = self.grid_mut();
        let n = n.min(COLS - cx);
        for col in (cx + n..COLS).rev() { grid[cy][col] = grid[cy][col - n]; }
        for col in cx..cx + n { grid[cy][col] = BLANK; }
        self.dirty = true;
    }

    fn erase_chars(&mut self, n: usize) {
        let cy = self.cy;
        let cx = self.cx;
        let grid = self.grid_mut();
        for col in cx..(cx + n).min(COLS) { grid[cy][col] = BLANK; }
        self.dirty = true;
    }

    // -- guest PTY ----------------------------------------------------------

    pub fn start_guest(&mut self, command: &str) -> bool {
        if self.guest != 0 { self.close_guest(); }
        self.clear();
        match k::term_start(command, COLS as u64, ROWS as u64) {
            Some(id) => {
                self.guest = id;
                self.guest_exit = None;
                self.feed_text("KimiOS: PTY attached\r\n");
                true
            }
            None => {
                self.feed_text("KimiOS: PTY unavailable here. On Android, install a guest package first (apkg install nodejs npm).\r\n");
                false
            }
        }
    }

    pub fn poll_guest(&mut self) -> bool {
        if self.guest == 0 { return false; }
        let mut changed = false;
        let mut buf = [0u8; 2048];
        loop {
            match k::term_read(self.guest, &mut buf) {
                Some(0) => break,
                Some(n) => { self.feed(&buf[..n]); changed = true; if n < buf.len() { break; } }
                None => break,
            }
        }
        let mut response = [0u8; RESPONSE_CAP];
        let n = self.take_response(&mut response);
        if n > 0 { k::term_write(self.guest, &response[..n]); }
        if self.guest_exit.is_none() && !k::term_alive(self.guest) {
            if let Some(code) = k::term_close(self.guest) {
                self.guest_exit = Some(code);
                self.feed_text("\r\n[process exited]\r\n");
            }
            self.guest = 0;
            changed = true;
        }
        if changed { self.dirty = true; }
        changed
    }

    pub fn send_bytes(&mut self, bytes: &[u8]) -> bool {
        if self.guest != 0 {
            self.scroll_to_bottom();
            k::term_write(self.guest, bytes) > 0
        } else {
            false
        }
    }

    pub fn close_guest(&mut self) {
        if self.guest != 0 {
            if let Some(code) = k::term_close(self.guest) {
                self.guest_exit = Some(code);
            }
            self.guest = 0;
        }
    }

    // -- rendering ----------------------------------------------------------

    pub fn render(&self, screen: &Screen, x: i64, y: i64, cell_w: i64, cell_h: i64, font_size: i64) {
        let width = cell_w * COLS as i64;
        let height = cell_h * ROWS as i64;
        screen.fill(x, y, width, height, DEFAULT_BG);

        for row in 0..ROWS {
            let line = self.line_at(row);
            let mut col = 0usize;
            while col < COLS {
                let cell = line[col];
                if cell.ch == 0 { col += 1; continue; }
                let mut fg = if cell.fg == 0 { DEFAULT_FG } else { cell.fg };
                let mut bg = if cell.bg == 0 { DEFAULT_BG } else { cell.bg };
                if cell.flags & FLAG_REVERSE != 0 { core::mem::swap(&mut fg, &mut bg); }
                if bg != DEFAULT_BG {
                    screen.fill(x + col as i64 * cell_w, y + row as i64 * cell_h, cell_w, cell_h, bg);
                }
                if cell.ch != b' ' as u32 {
                    draw_cell(screen, x + col as i64 * cell_w, y + row as i64 * cell_h, cell.ch, font_size, fg);
                }
                if cell.flags & FLAG_UNDERLINE != 0 {
                    screen.fill(x + col as i64 * cell_w, y + (row as i64 + 1) * cell_h - 1, cell_w, 1, fg);
                }
                col += if char_width(cell.ch) == 2 { 2 } else { 1 };
            }
        }

        if self.cursor_visible && self.scroll_offset == 0 {
            let cx = x + self.cx as i64 * cell_w;
            let cy = y + self.cy as i64 * cell_h;
            screen.fill(cx, cy, cell_w, cell_h, theme::RED());
            let line = self.line_at(self.cy);
            let cell = line[self.cx.min(COLS - 1)];
            if cell.ch != b' ' as u32 {
                draw_cell(screen, cx, cy, cell.ch, font_size, CURSOR_FG);
            }
        }
    }
}

fn draw_cell(screen: &Screen, x: i64, y: i64, ch: u32, size: i64, color: u32) {
    let mut buf = [0u8; 4];
    let s = char::from_u32(ch).unwrap_or('?').encode_utf8(&mut buf);
    screen.font_text(x, y, s, size, color, &font_data::MONO);
}

fn put_num(out: &mut [u8], mut value: u64) -> usize {
    let mut tmp = [0u8; 20];
    let mut n = 0;
    if value == 0 { out[0] = b'0'; return 1; }
    while value > 0 { tmp[n] = b'0' + (value % 10) as u8; value /= 10; n += 1; }
    for i in 0..n { out[i] = tmp[n - 1 - i]; }
    n
}

fn char_width(ch: u32) -> usize {
    if ch == 0 { return 0; }
    if (0x0300..=0x036f).contains(&ch)
        || (0x1ab0..=0x1aff).contains(&ch)
        || (0x1dc0..=0x1dff).contains(&ch)
        || (0x20d0..=0x20ff).contains(&ch)
        || (0xfe00..=0xfe0f).contains(&ch)
    {
        return 0;
    }
    if (0x1100..=0x115f).contains(&ch)
        || (0x2e80..=0xa4cf).contains(&ch)
        || (0xac00..=0xd7a3).contains(&ch)
        || (0xf900..=0xfaff).contains(&ch)
        || (0xfe10..=0xfe19).contains(&ch)
        || (0xfe30..=0xfe6f).contains(&ch)
        || (0xff00..=0xff60).contains(&ch)
        || (0xffe0..=0xffe6).contains(&ch)
        || (0x1f300..=0x1faff).contains(&ch)
        || (0x20000..=0x3fffd).contains(&ch)
    {
        2
    } else {
        1
    }
}

fn ansi_color(index: u32) -> u32 {
    const BASE: [u32; 16] = [
        0x001b1b22, 0x00e05c5c, 0x0060c878, 0x00d8b45a,
        0x006a9ce8, 0x00c07ad8, 0x005ac8c8, 0x00d8d8e0,
        0x00606070, 0x00ff8080, 0x0088e0a0, 0x00f0d080,
        0x0090b8ff, 0x00e0a0ff, 0x0080e0e0, 0x00ffffff,
    ];
    if index < 16 { return BASE[index as usize]; }
    if index < 232 {
        let n = index - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let scale = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
        return (scale(r) << 16) | (scale(g) << 8) | scale(b);
    }
    let v = 8 + (index - 232).min(23) * 10;
    (v << 16) | (v << 8) | v
}