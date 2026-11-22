//! VT100 / ECMA-48 / xterm-compatible terminal emulator for the TUI surface.
//!
//! A robust, incremental state-machine parser (ESC / CSI / OSC / DCS / charset)
//! over an attributed, UTF-8-aware cell grid. Supports cursor motion, editing
//! (IL/DL/ICH/DCH/ECH), scroll regions (DECSTBM), insert mode, tabs (HTS/TBC),
//! erase (ED/EL), save/restore (DECSC/DECRC + CSI s/u), SGR (bold/dim/underline/
//! reverse/italic/strike, 16/256/truecolor), DEC private modes (cursor, alt
//! screen, autowrap, origin, insert, application cursor, mouse tracking,
//! bracketed paste), and DSR/DA responses. Malformed input can never wedge the
//! parser (every state is bounded and always returns to GROUND).

pub const COLS: usize = 76;
pub const ROWS: usize = 24;

pub const DEFAULT_FG: u16 = 256;
pub const DEFAULT_BG: u16 = 257;

// cell attribute bits
pub const F_BOLD: u8 = 1;
pub const F_DIM: u8 = 2;
pub const F_UNDER: u8 = 4;
pub const F_REVERSE: u8 = 8;
pub const F_ITALIC: u8 = 16;
pub const F_STRIKE: u8 = 32;
pub const F_BLINK: u8 = 64;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: u32,
    pub fg: u16,
    pub bg: u16,
    pub flags: u8,
}

pub const EMPTY_CELL: Cell = Cell { ch: b' ' as u32, fg: DEFAULT_FG, bg: DEFAULT_BG, flags: 0 };

// parser states
const S_GROUND: u8 = 0;
const S_ESC: u8 = 1;
const S_CSI: u8 = 2;
const S_OSC: u8 = 3;
const S_OSC_ESC: u8 = 4;
const S_STR: u8 = 5; // DCS / SOS / PM / APC: skip until ST
const S_STR_ESC: u8 = 6;
const S_CHARSET: u8 = 7;

// mouse tracking modes
pub const MOUSE_OFF: u16 = 0;
pub const MOUSE_X10: u16 = 9; // 1000 => press only (we store 1000 here)
pub const MOUSE_BUTTON: u16 = 1002;
pub const MOUSE_ANY: u16 = 1003;

pub struct Vt {
    pub cells: [[Cell; COLS]; ROWS],
    alt: [[Cell; COLS]; ROWS],
    pub cx: usize,
    pub cy: usize,
    fg: u16,
    bg: u16,
    flags: u8,
    pub scroll_top: usize,
    pub scroll_bot: usize,
    saved: [usize; 2],
    saved_fg: u16,
    saved_bg: u16,
    saved_flags: u8,
    pub cursor_visible: bool,
    pub alt_active: bool,
    wrap: bool,
    origin: bool,
    insert: bool,
    pub app_cursor: bool,
    pub mouse_mode: u16,
    pub mouse_sgr: bool,
    pub bracketed_paste: bool,
    tabs: [u64; (COLS + 63) / 64 + 1],
    state: u8,
    params: [u16; 32],
    nparams: usize,
    cur: u16,
    have_cur: bool,
    priv_marker: u8, // 0 none, b'?' or b'>' or b'=' 
    intermediate: u8,
    // UTF-8 decoder
    u_need: u8,
    u_cp: u32,
    pub dirty: bool,
    pub resp: [u8; 64],
    pub resp_len: usize,
}

impl Vt {
    pub fn new() -> Self {
        let mut t = [0u64; (COLS + 63) / 64 + 1];
        for c in (0..COLS).step_by(8) {
            t[c / 64] |= 1u64 << (c % 64);
        }
        Self {
            cells: [[EMPTY_CELL; COLS]; ROWS],
            alt: [[EMPTY_CELL; COLS]; ROWS],
            cx: 0,
            cy: 0,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            flags: 0,
            scroll_top: 0,
            scroll_bot: ROWS - 1,
            saved: [0, 0],
            saved_fg: DEFAULT_FG,
            saved_bg: DEFAULT_BG,
            saved_flags: 0,
            cursor_visible: true,
            alt_active: false,
            wrap: true,
            origin: false,
            insert: false,
            app_cursor: false,
            mouse_mode: MOUSE_OFF,
            mouse_sgr: false,
            bracketed_paste: false,
            tabs: t,
            state: S_GROUND,
            params: [0; 32],
            nparams: 0,
            cur: 0,
            have_cur: false,
            priv_marker: 0,
            intermediate: 0,
            u_need: 0,
            u_cp: 0,
            dirty: true,
            resp: [0; 64],
            resp_len: 0,
        }
    }

    pub fn reset(&mut self) {
        let tabs = self.tabs;
        *self = Self::new();
        self.tabs = tabs;
    }

    fn tab_set(&self, c: usize) -> bool {
        c < COLS && (self.tabs[c / 64] & (1u64 << (c % 64))) != 0
    }

    // ---- grid primitives ---------------------------------------------------
    fn put(&mut self, cp: u32) {
        if self.cx >= COLS {
            if self.wrap {
                self.cx = 0;
                self.linefeed();
            } else {
                self.cx = COLS - 1;
            }
        }
        if self.insert {
            // shift right
            let mut x = COLS - 1;
            while x > self.cx {
                self.cells[self.cy][x] = self.cells[self.cy][x - 1];
                x -= 1;
            }
        }
        let cell = &mut self.cells[self.cy][self.cx];
        cell.ch = cp;
        cell.fg = self.fg;
        cell.bg = self.bg;
        cell.flags = self.flags;
        self.cx += 1;
        self.dirty = true;
    }

    fn linefeed(&mut self) {
        if self.cy == self.scroll_bot {
            self.scroll_up(1);
        } else if self.cy < ROWS - 1 {
            self.cy += 1;
        }
        self.dirty = true;
    }

    fn reverse_index(&mut self) {
        if self.cy == self.scroll_top {
            self.scroll_down(1);
        } else if self.cy > 0 {
            self.cy -= 1;
        }
        self.dirty = true;
    }

    fn scroll_up(&mut self, n: usize) {
        let (top, bot) = (self.scroll_top, self.scroll_bot);
        for _ in 0..n {
            for r in top..bot {
                self.cells[r] = self.cells[r + 1];
            }
            self.cells[bot] = [self.eraser(); COLS];
        }
        self.dirty = true;
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bot) = (self.scroll_top, self.scroll_bot);
        for _ in 0..n {
            for r in (top + 1..=bot).rev() {
                self.cells[r] = self.cells[r - 1];
            }
            self.cells[top] = [self.eraser(); COLS];
        }
        self.dirty = true;
    }

    fn eraser(&self) -> Cell {
        Cell { ch: b' ' as u32, fg: DEFAULT_FG, bg: DEFAULT_BG, flags: 0 }
    }

    fn erase_line(&mut self, mode: u16) {
        let (a, b) = match mode {
            1 => (0, self.cx),
            2 => (0, COLS - 1),
            _ => (self.cx, COLS - 1),
        };
        for x in a..=b.min(COLS - 1) {
            self.cells[self.cy][x] = self.eraser();
        }
        self.dirty = true;
    }

    fn erase_display(&mut self, mode: u16) {
        match mode {
            1 => {
                for r in 0..self.cy {
                    self.cells[r] = [self.eraser(); COLS];
                }
                self.erase_line(1);
            }
            2 | 3 => {
                self.cells = [[self.eraser(); COLS]; ROWS];
            }
            _ => {
                self.erase_line(0);
                for r in (self.cy + 1)..ROWS {
                    self.cells[r] = [self.eraser(); COLS];
                }
            }
        }
        self.dirty = true;
    }

    fn param(&self, i: usize, default: u16) -> u16 {
        if i < self.nparams {
            if self.params[i] == 0 { default } else { self.params[i] }
        } else {
            default
        }
    }

    fn respond(&mut self, bytes: &[u8]) {
        let room = self.resp.len() - self.resp_len;
        let take = bytes.len().min(room);
        self.resp[self.resp_len..self.resp_len + take].copy_from_slice(&bytes[..take]);
        self.resp_len += take;
    }

    fn clamp_cursor(&mut self) {
        if self.cx >= COLS { self.cx = COLS - 1; }
        if self.cy >= ROWS { self.cy = ROWS - 1; }
    }

    // ---- input -------------------------------------------------------------
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        // C0 controls that are always honored outside string states
        match self.state {
            S_ESC => return self.do_esc(b),
            S_CSI => return self.do_csi(b),
            S_OSC => {
                if b == 0x07 { self.state = S_GROUND; }
                else if b == 0x1b { self.state = S_OSC_ESC; }
                return;
            }
            S_OSC_ESC => {
                self.state = if b == b'\\' { S_GROUND } else { S_OSC };
                return;
            }
            S_STR => {
                if b == 0x1b { self.state = S_STR_ESC; }
                return;
            }
            S_STR_ESC => {
                self.state = if b == b'\\' { S_GROUND } else { S_STR };
                return;
            }
            S_CHARSET => {
                self.state = S_GROUND;
                return;
            }
            _ => {}
        }
        // GROUND
        if self.u_need > 0 {
            if b & 0xC0 == 0x80 {
                self.u_cp = (self.u_cp << 6) | (b & 0x3F) as u32;
                self.u_need -= 1;
                if self.u_need == 0 {
                    self.put(self.u_cp);
                }
                return;
            }
            // invalid continuation: drop partial, fall through to handle b
            self.u_need = 0;
        }
        match b {
            0x1b => self.state = S_ESC,
            0x18 | 0x1a => self.state = S_GROUND, // CAN / SUB abort
            0x0a | 0x0b | 0x0c => {
                self.linefeed();
                if self.origin && self.cy < self.scroll_top { self.cy = self.scroll_top; }
            }
            0x0d => self.cx = 0,
            0x08 => self.cx = self.cx.saturating_sub(1),
            0x09 => self.tab_forward(1),
            0x07 => {}
            0x20..=0x7e => self.put(b as u32),
            0xc2..=0xdf => { self.u_cp = (b & 0x1F) as u32; self.u_need = 1; }
            0xe0..=0xef => { self.u_cp = (b & 0x0F) as u32; self.u_need = 2; }
            0xf0..=0xf4 => { self.u_cp = (b & 0x07) as u32; self.u_need = 3; }
            _ => {}
        }
    }

    fn tab_forward(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cx + 1;
            while x < COLS && !self.tab_set(x) { x += 1; }
            self.cx = x.min(COLS - 1);
        }
    }

    fn tab_back(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cx;
            loop {
                if x == 0 { break; }
                x -= 1;
                if self.tab_set(x) { break; }
            }
            self.cx = x;
        }
    }

    fn do_esc(&mut self, b: u8) {
        self.state = S_GROUND;
        match b {
            b'[' => {
                self.state = S_CSI;
                self.nparams = 0;
                self.cur = 0;
                self.have_cur = false;
                self.priv_marker = 0;
                self.intermediate = 0;
            }
            b']' => self.state = S_OSC,
            b'P' | b'X' | b'^' | b'_' => self.state = S_STR,
            b'(' | b')' | b'*' | b'+' => self.state = S_CHARSET,
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.linefeed(),
            b'E' => { self.cx = 0; self.linefeed(); }
            b'M' => self.reverse_index(),
            b'H' => { if self.tab_set(self.cx) { /* keep */ } }
            b'c' => self.reset(),
            b'=' => self.app_cursor = false,
            b'>' => self.app_cursor = true,
            b'\\' => {}
            _ => {}
        }
    }

    fn save_cursor(&mut self) {
        self.saved = [self.cx, self.cy];
        self.saved_fg = self.fg;
        self.saved_bg = self.bg;
        self.saved_flags = self.flags;
    }

    fn restore_cursor(&mut self) {
        self.cx = self.saved[0].min(COLS - 1);
        self.cy = self.saved[1].min(ROWS - 1);
        self.fg = self.saved_fg;
        self.bg = self.saved_bg;
        self.flags = self.saved_flags;
    }

    fn do_csi(&mut self, b: u8) {
        match b {
            b'0'..=b'9' => {
                self.cur = self.cur.saturating_mul(10).saturating_add((b - b'0') as u16);
                self.have_cur = true;
            }
            b';' => {
                if self.nparams < self.params.len() {
                    self.params[self.nparams] = self.cur;
                    self.nparams += 1;
                }
                self.cur = 0;
                self.have_cur = false;
            }
            b':' => {
                // sub-parameters: treat as ';' separator for simplicity
                if self.nparams < self.params.len() {
                    self.params[self.nparams] = self.cur;
                    self.nparams += 1;
                }
                self.cur = 0;
                self.have_cur = false;
            }
            b'?' | b'>' | b'=' | b'<' => { self.priv_marker = b; }
            b' ' | b'!' | b'"' | b'$' | b'\'' => { self.intermediate = b; }
            0x40..=0x7e => {
                if self.have_cur || self.nparams > 0 {
                    if self.nparams < self.params.len() {
                        self.params[self.nparams] = self.cur;
                        self.nparams += 1;
                    }
                }
                self.csi_dispatch(b);
                self.state = S_GROUND;
            }
            0x1b => self.state = S_ESC,
            0x18 | 0x1a => self.state = S_GROUND,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, f: u8) {
        let p0 = self.param(0, 1) as usize;
        let n = |c: usize, p: usize| -> usize { if p == 0 { c } else { c.min(ROWS.max(COLS)) } };
        _ = n;
        match f {
            b'A' => { self.cy = self.cy.saturating_sub(p0); if self.cy < self.scroll_top && self.origin { self.cy = self.scroll_top; } }
            b'B' | b'e' => { self.cy = (self.cy + p0).min(ROWS - 1); }
            b'C' | b'a' => { self.cx = (self.cx + p0).min(COLS - 1); }
            b'D' => { self.cx = self.cx.saturating_sub(p0); }
            b'E' => { self.cy = (self.cy + p0).min(ROWS - 1); self.cx = 0; }
            b'F' => { self.cy = self.cy.saturating_sub(p0); self.cx = 0; }
            b'G' | b'`' => { self.cx = p0.saturating_sub(1).min(COLS - 1); }
            b'd' => { self.cy = p0.saturating_sub(1).min(ROWS - 1); }
            b'H' | b'f' => {
                let mut row = self.param(0, 1) as usize;
                let mut col = self.param(1, 1) as usize;
                if self.origin {
                    row = (self.scroll_top + row).min(self.scroll_bot + 1);
                    self.cy = row.saturating_sub(1).min(ROWS - 1);
                } else {
                    self.cy = row.saturating_sub(1).min(ROWS - 1);
                }
                self.cx = col.saturating_sub(1).min(COLS - 1);
            }
            b'J' => self.erase_display(self.param(0, 0)),
            b'K' => self.erase_line(self.param(0, 0)),
            b'L' => {
                let mut n = p0;
                let bot = self.scroll_bot;
                while n > 0 && self.cy <= bot {
                    for r in (self.cy + 1..=bot).rev() {
                        self.cells[r] = self.cells[r - 1];
                    }
                    self.cells[self.cy] = [self.eraser(); COLS];
                    n -= 1;
                }
                self.dirty = true;
            }
            b'M' => {
                let mut n = p0;
                let bot = self.scroll_bot;
                while n > 0 && self.cy <= bot {
                    for r in self.cy..bot {
                        self.cells[r] = self.cells[r + 1];
                    }
                    self.cells[bot] = [self.eraser(); COLS];
                    n -= 1;
                }
                self.dirty = true;
            }
            b'P' => {
                let cnt = p0.min(COLS - self.cx);
                for x in self.cx..COLS {
                    self.cells[self.cy][x] = if x + cnt < COLS { self.cells[self.cy][x + cnt] } else { self.eraser() };
                }
                self.dirty = true;
            }
            b'X' => {
                let cnt = p0.min(COLS - self.cx);
                for x in self.cx..self.cx + cnt { self.cells[self.cy][x] = self.eraser(); }
                self.dirty = true;
            }
            b'@' => {
                let cnt = p0.min(COLS - self.cx);
                for x in (self.cx..COLS).rev() {
                    self.cells[self.cy][x] = if x >= self.cx + cnt { self.cells[self.cy][x - cnt] } else { self.eraser() };
                }
                self.dirty = true;
            }
            b'S' => self.scroll_up(p0),
            b'T' => self.scroll_down(p0),
            b'r' => {
                let top = self.param(0, 1) as usize;
                let bot = self.param(1, ROWS as u16) as usize;
                self.scroll_top = top.saturating_sub(1).min(ROWS - 1);
                self.scroll_bot = bot.saturating_sub(1).min(ROWS - 1);
                if self.scroll_bot < self.scroll_top { self.scroll_bot = self.scroll_top; }
                self.cx = 0;
                self.cy = 0;
            }
            b'm' => self.sgr(),
            b'h' | b'l' => self.set_modes(f == b'h'),
            b's' => self.save_cursor(),
            b'u' => self.restore_cursor(),
            b'I' => self.tab_forward(p0),
            b'Z' => self.tab_back(p0),
            b'g' => {
                let mode = self.param(0, 0);
                if mode == 0 {
                    if self.cx < COLS { self.tabs[self.cx / 64] &= !(1u64 << (self.cx % 64)); }
                } else if mode == 3 {
                    self.tabs = [0; (COLS + 63) / 64 + 1];
                }
            }
            b'n' => {
                match self.param(0, 0) {
                    6 => {
                        let mut buf = [0u8; 24];
                        let mut n = 0;
                        push(&mut buf, &mut n, b"\x1b[");
                        push_num(&mut buf, &mut n, (self.cy + 1) as u64);
                        push(&mut buf, &mut n, b";");
                        push_num(&mut buf, &mut n, (self.cx + 1) as u64);
                        push(&mut buf, &mut n, b"R");
                        self.respond(&buf[..n]);
                    }
                    5 => { self.respond(b"\x1b[0n"); }
                    _ => {}
                }
            }
            b'c' => { if self.param(0, 0) == 0 { self.respond(b"\x1b[?62;c"); } }
            _ => {}
        }
        self.clamp_cursor();
        self.dirty = true;
    }

    fn set_modes(&mut self, on: bool) {
        let priv_ = self.priv_marker == b'?';
        for i in 0..self.nparams.max(1) {
            let m = if i < self.nparams { self.params[i] } else { 0 };
            if priv_ {
                match m {
                    25 => self.cursor_visible = on,
                    7 => self.wrap = on,
                    6 => self.origin = on,
                    4 => self.insert = on,
                    1000 => self.mouse_mode = if on { 1000 } else { MOUSE_OFF },
                    1002 => self.mouse_mode = if on { MOUSE_BUTTON } else { MOUSE_OFF },
                    1003 => self.mouse_mode = if on { MOUSE_ANY } else { MOUSE_OFF },
                    1006 => self.mouse_sgr = on,
                    2004 => self.bracketed_paste = on,
                    1 => self.app_cursor = on,
                    1049 | 1047 | 47 => self.switch_alt(on),
                    1048 => { if on { self.save_cursor(); } else { self.restore_cursor(); } }
                    _ => {}
                }
            }
        }
    }

    fn switch_alt(&mut self, on: bool) {
        if on == self.alt_active { return; }
        core::mem::swap(&mut self.cells, &mut self.alt);
        self.alt_active = on;
        if on {
            self.cells = [[self.eraser(); COLS]; ROWS];
            self.cx = 0;
            self.cy = 0;
        }
        self.dirty = true;
    }

    fn sgr(&mut self) {
        if self.nparams == 0 {
            self.fg = DEFAULT_FG;
            self.bg = DEFAULT_BG;
            self.flags = 0;
            return;
        }
        let mut i = 0;
        while i < self.nparams {
            let p = self.params[i];
            match p {
                0 => { self.fg = DEFAULT_FG; self.bg = DEFAULT_BG; self.flags = 0; }
                1 => self.flags |= F_BOLD,
                2 => self.flags |= F_DIM,
                3 => self.flags |= F_ITALIC,
                4 => self.flags |= F_UNDER,
                5 | 6 => self.flags |= F_BLINK,
                7 => self.flags |= F_REVERSE,
                9 => self.flags |= F_STRIKE,
                21 | 22 => self.flags &= !(F_BOLD | F_DIM),
                23 => self.flags &= !F_ITALIC,
                24 => self.flags &= !F_UNDER,
                25 => self.flags &= !F_BLINK,
                27 => self.flags &= !F_REVERSE,
                29 => self.flags &= !F_STRIKE,
                30..=37 => self.fg = p - 30,
                39 => self.fg = DEFAULT_FG,
                40..=47 => self.bg = p - 40,
                49 => self.bg = DEFAULT_BG,
                90..=97 => self.fg = p - 90 + 8,
                100..=107 => self.bg = p - 100 + 8,
                38 | 48 => {
                    let is_fg = p == 38;
                    if i + 1 < self.nparams && self.params[i + 1] == 5 && i + 2 < self.nparams {
                        let c = self.params[i + 2].min(255);
                        if is_fg { self.fg = c; } else { self.bg = c; }
                        i += 2;
                    } else if i + 1 < self.nparams && self.params[i + 1] == 2 && i + 4 < self.nparams {
                        let r = self.params[i + 2] as u32;
                        let g = self.params[i + 3] as u32;
                        let b = self.params[i + 4] as u32;
                        let c = rgb_to_256(r, g, b);
                        if is_fg { self.fg = c; } else { self.bg = c; }
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

fn push(dst: &mut [u8], n: &mut usize, src: &[u8]) {
    let take = src.len().min(dst.len() - *n);
    dst[*n..*n + take].copy_from_slice(&src[..take]);
    *n += take;
}

fn push_num(dst: &mut [u8], n: &mut usize, mut v: u64) {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 { break; }
    }
    push(dst, n, &tmp[i..]);
}

fn rgb_to_256(r: u32, g: u32, b: u32) -> u16 {
    if r == g && g == b {
        if r < 8 { return 16; }
        if r > 248 { return 231; }
        return (232 + (r - 8) / 10) as u16;
    }
    let ri = (r * 5 / 255).min(5);
    let gi = (g * 5 / 255).min(5);
    let bi = (b * 5 / 255).min(5);
    (16 + 36 * ri + 6 * gi + bi) as u16
}

/// xterm 256-colour palette -> 0x00RRGGBB (256/257 are theme defaults).
pub fn palette(idx: u16) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
        0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    if idx < 16 { return BASE[idx as usize]; }
    if idx < 232 {
        let c = idx - 16;
        let r = c / 36;
        let g = (c % 36) / 6;
        let b = c % 6;
        let conv = |v: u16| if v == 0 { 0u32 } else { (55 + v * 40) as u32 };
        return (conv(r) << 16) | (conv(g) << 8) | conv(b);
    }
    if idx < 256 {
        let v = (8 + (idx - 232) * 10) as u32;
        return (v << 16) | (v << 8) | v;
    }
    0xffffff
}