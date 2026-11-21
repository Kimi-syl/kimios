//! Minimal VT100/xterm terminal emulator for the TUI surface. Parses the byte
//! stream from a PTY guest session into an attributed cell grid (cursor, SGR
//! colors, erase/scroll, alternate screen). Enough for sh/top/vi/htop.

pub const COLS: usize = 76;
pub const ROWS: usize = 22;

pub const DEFAULT_FG: u16 = 256;
pub const DEFAULT_BG: u16 = 257;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: u8,
    pub fg: u16,
    pub bg: u16,
    pub flags: u8, // bit0 bold, bit1 reverse, bit2 underline
}

pub const EMPTY_CELL: Cell = Cell { ch: b' ', fg: DEFAULT_FG, bg: DEFAULT_BG, flags: 0 };

const GROUND: u8 = 0;
const ESC: u8 = 1;
const CSI: u8 = 2;
const OSC: u8 = 3;
const CHARSET: u8 = 4;
const OSC_ESC: u8 = 5;

pub struct Vt {
    pub cells: [[Cell; COLS]; ROWS],
    alt: [[Cell; COLS]; ROWS],
    pub cx: usize,
    pub cy: usize,
    fg: u16,
    bg: u16,
    flags: u8,
    scroll_top: usize,
    scroll_bot: usize,
    saved_cx: usize,
    saved_cy: usize,
    pub cursor_visible: bool,
    pub alt_active: bool,
    wrap: bool,
    state: u8,
    params: [u16; 16],
    nparams: usize,
    cur: u16,
    started: bool,
    priv_mode: bool,
    pub dirty: bool,
    // Responses the emulator must send back to the child (DSR / DA).
    pub resp: [u8; 64],
    pub resp_len: usize,
}

impl Vt {
    pub fn new() -> Self {
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
            saved_cx: 0,
            saved_cy: 0,
            cursor_visible: true,
            alt_active: false,
            wrap: true,
            state: GROUND,
            params: [0; 16],
            nparams: 0,
            cur: 0,
            started: false,
            priv_mode: false,
            dirty: true,
            resp: [0; 64],
            resp_len: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn put(&mut self, ch: u8) {
        if self.cx >= COLS {
            if self.wrap {
                self.cx = 0;
                self.linefeed();
            } else {
                self.cx = COLS - 1;
            }
        }
        let cell = &mut self.cells[self.cy][self.cx];
        cell.ch = ch;
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

    fn scroll_up(&mut self, n: usize) {
        let top = self.scroll_top;
        let bot = self.scroll_bot;
        for _ in 0..n {
            for r in top..bot {
                self.cells[r] = self.cells[r + 1];
            }
            self.cells[bot] = [EMPTY_CELL; COLS];
        }
        self.dirty = true;
    }

    fn scroll_down(&mut self, n: usize) {
        let top = self.scroll_top;
        let bot = self.scroll_bot;
        for _ in 0..n {
            for r in (top + 1..=bot).rev() {
                self.cells[r] = self.cells[r - 1];
            }
            self.cells[top] = [EMPTY_CELL; COLS];
        }
        self.dirty = true;
    }

    fn erase_line(&mut self, mode: u16) {
        let (a, b) = match mode {
            1 => (0, self.cx),
            2 => (0, COLS - 1),
            _ => (self.cx, COLS - 1),
        };
        for x in a..=b.min(COLS - 1) {
            self.cells[self.cy][x] = EMPTY_CELL;
        }
        self.dirty = true;
    }

    fn erase_display(&mut self, mode: u16) {
        match mode {
            1 => {
                for r in 0..self.cy {
                    self.cells[r] = [EMPTY_CELL; COLS];
                }
                self.erase_line(1);
            }
            2 | 3 => {
                self.cells = [[EMPTY_CELL; COLS]; ROWS];
            }
            _ => {
                self.erase_line(0);
                for r in (self.cy + 1)..ROWS {
                    self.cells[r] = [EMPTY_CELL; COLS];
                }
            }
        }
        self.dirty = true;
    }

    fn param(&self, i: usize, default: u16) -> u16 {
        if i < self.nparams && self.params[i] != 0 {
            self.params[i]
        } else if i < self.nparams {
            // an explicit 0 is meaningful for some commands; treat 0 as 0 only
            // when it was actually typed, else default
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

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            ESC => {
                self.state = GROUND;
                match b {
                    b'[' => {
                        self.state = CSI;
                        self.nparams = 0;
                        self.cur = 0;
                        self.started = false;
                        self.priv_mode = false;
                    }
                    b']' => self.state = OSC,
                    b'(' | b')' | b'*' | b'+' => self.state = CHARSET,
                    b'7' => {
                        self.saved_cx = self.cx;
                        self.saved_cy = self.cy;
                    }
                    b'8' => {
                        self.cx = self.saved_cx.min(COLS - 1);
                        self.cy = self.saved_cy.min(ROWS - 1);
                    }
                    b'M' => {
                        if self.cy == self.scroll_top {
                            self.scroll_down(1);
                        } else if self.cy > 0 {
                            self.cy -= 1;
                        }
                    }
                    b'c' => self.reset(),
                    _ => {}
                }
                return;
            }
            CSI => {
                match b {
                    b'0'..=b'9' => {
                        self.cur = self.cur.saturating_mul(10).saturating_add((b - b'0') as u16);
                        self.started = true;
                    }
                    b';' => {
                        if self.nparams < self.params.len() {
                            self.params[self.nparams] = self.cur;
                            self.nparams += 1;
                        }
                        self.cur = 0;
                        self.started = false;
                    }
                    b'?' => self.priv_mode = true,
                    b' ' | b'!' | b'"' | b'$' | b'>' | b'<' | b'=' => {}
                    0x40..=0x7e => {
                        if self.started || self.nparams > 0 {
                            if self.nparams < self.params.len() {
                                self.params[self.nparams] = self.cur;
                                self.nparams += 1;
                            }
                        }
                        self.csi_dispatch(b);
                        self.state = GROUND;
                    }
                    0x1b => self.state = ESC,
                    _ => {}
                }
                return;
            }
            OSC => {
                if b == 0x07 {
                    self.state = GROUND;
                } else if b == 0x1b {
                    self.state = OSC_ESC;
                }
                return;
            }
            OSC_ESC => {
                self.state = if b == b'\\' { GROUND } else { OSC };
                return;
            }
            CHARSET => {
                self.state = GROUND;
                return;
            }
            _ => {}
        }
        // GROUND
        match b {
            0x1b => self.state = ESC,
            0x0a | 0x0b | 0x0c => self.linefeed(),
            0x0d => self.cx = 0,
            0x08 => self.cx = self.cx.saturating_sub(1),
            0x09 => {
                self.cx = ((self.cx / 8) + 1) * 8;
                if self.cx >= COLS {
                    self.cx = COLS - 1;
                }
            }
            0x07 => {}
            0x20..=0x7e => self.put(b),
            _ => {
                // Non-ASCII (UTF-8 lead/continuation): render a placeholder for
                // lead bytes, drop continuation bytes, to keep the grid stable.
                if b >= 0xc0 {
                    self.put(b'?');
                }
            }
        }
    }

    fn csi_dispatch(&mut self, f: u8) {
        let p0 = self.param(0, 1);
        match f {
            b'A' => self.cy = self.cy.saturating_sub(p0 as usize),
            b'B' => self.cy = (self.cy + p0 as usize).min(ROWS - 1),
            b'C' => self.cx = (self.cx + p0 as usize).min(COLS - 1),
            b'D' => self.cx = self.cx.saturating_sub(p0 as usize),
            b'E' => {
                self.cy = (self.cy + p0 as usize).min(ROWS - 1);
                self.cx = 0;
            }
            b'F' => {
                self.cy = self.cy.saturating_sub(p0 as usize);
                self.cx = 0;
            }
            b'G' => self.cx = (p0 as usize).saturating_sub(1).min(COLS - 1),
            b'd' => self.cy = (p0 as usize).saturating_sub(1).min(ROWS - 1),
            b'H' | b'f' => {
                let row = self.param(0, 1) as usize;
                let col = self.param(1, 1) as usize;
                self.cy = row.saturating_sub(1).min(ROWS - 1);
                self.cx = col.saturating_sub(1).min(COLS - 1);
            }
            b'J' => self.erase_display(self.param(0, 0)),
            b'K' => self.erase_line(self.param(0, 0)),
            b'L' => {
                let n = p0 as usize;
                for _ in 0..n {
                    for r in (self.cy + 1..=self.scroll_bot).rev() {
                        self.cells[r] = self.cells[r - 1];
                    }
                    self.cells[self.cy] = [EMPTY_CELL; COLS];
                }
                self.dirty = true;
            }
            b'M' => {
                let n = p0 as usize;
                for _ in 0..n {
                    for r in self.cy..self.scroll_bot {
                        self.cells[r] = self.cells[r + 1];
                    }
                    self.cells[self.scroll_bot] = [EMPTY_CELL; COLS];
                }
                self.dirty = true;
            }
            b'P' => {
                let n = (p0 as usize).min(COLS - self.cx);
                for x in self.cx..COLS {
                    self.cells[self.cy][x] = if x + n < COLS {
                        self.cells[self.cy][x + n]
                    } else {
                        EMPTY_CELL
                    };
                }
                self.dirty = true;
            }
            b'X' => {
                let n = (p0 as usize).min(COLS - self.cx);
                for x in self.cx..self.cx + n {
                    self.cells[self.cy][x] = EMPTY_CELL;
                }
                self.dirty = true;
            }
            b'@' => {
                let n = (p0 as usize).min(COLS - self.cx);
                for x in (self.cx..COLS).rev() {
                    self.cells[self.cy][x] = if x >= self.cx + n {
                        self.cells[self.cy][x - n]
                    } else {
                        EMPTY_CELL
                    };
                }
                self.dirty = true;
            }
            b'S' => self.scroll_up(p0 as usize),
            b'T' => self.scroll_down(p0 as usize),
            b'r' => {
                let top = self.param(0, 1) as usize;
                let bot = self.param(1, ROWS as u16) as usize;
                self.scroll_top = top.saturating_sub(1).min(ROWS - 1);
                self.scroll_bot = bot.saturating_sub(1).min(ROWS - 1);
                if self.scroll_bot < self.scroll_top {
                    self.scroll_bot = self.scroll_top;
                }
                self.cx = 0;
                self.cy = 0;
            }
            b'm' => self.sgr(),
            b'h' | b'l' => {
                let on = f == b'h';
                if self.priv_mode {
                    for i in 0..self.nparams {
                        match self.params[i] {
                            25 => self.cursor_visible = on,
                            7 => self.wrap = on,
                            1049 | 1047 | 47 => self.switch_alt(on),
                            1048 => {
                                if on {
                                    self.saved_cx = self.cx;
                                    self.saved_cy = self.cy;
                                } else {
                                    self.cx = self.saved_cx.min(COLS - 1);
                                    self.cy = self.saved_cy.min(ROWS - 1);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            b's' => {
                self.saved_cx = self.cx;
                self.saved_cy = self.cy;
            }
            b'u' => {
                self.cx = self.saved_cx.min(COLS - 1);
                self.cy = self.saved_cy.min(ROWS - 1);
            }
            b'n' => {
                if self.param(0, 0) == 6 {
                    let mut buf = [0u8; 24];
                    let mut n = 0;
                    push(&mut buf, &mut n, b"\x1b[");
                    push_num(&mut buf, &mut n, (self.cy + 1) as u64);
                    push(&mut buf, &mut n, b";");
                    push_num(&mut buf, &mut n, (self.cx + 1) as u64);
                    push(&mut buf, &mut n, b"R");
                    self.respond(&buf[..n]);
                }
            }
            b'c' => {
                if self.param(0, 0) == 0 {
                    self.respond(b"\x1b[?6c");
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn switch_alt(&mut self, on: bool) {
        if on == self.alt_active {
            return;
        }
        if on {
            core::mem::swap(&mut self.cells, &mut self.alt);
            self.alt_active = true;
            self.cells = [[EMPTY_CELL; COLS]; ROWS];
            self.cx = 0;
            self.cy = 0;
        } else {
            core::mem::swap(&mut self.cells, &mut self.alt);
            self.alt_active = false;
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
                0 => {
                    self.fg = DEFAULT_FG;
                    self.bg = DEFAULT_BG;
                    self.flags = 0;
                }
                1 => self.flags |= 1,
                4 => self.flags |= 4,
                7 => self.flags |= 2,
                22 => self.flags &= !1,
                24 => self.flags &= !4,
                27 => self.flags &= !2,
                30..=37 => self.fg = (p - 30) as u16,
                39 => self.fg = DEFAULT_FG,
                40..=47 => self.bg = (p - 40) as u16,
                49 => self.bg = DEFAULT_BG,
                90..=97 => self.fg = (p - 90 + 8) as u16,
                100..=107 => self.bg = (p - 100 + 8) as u16,
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
        if v == 0 {
            break;
        }
    }
    push(dst, n, &tmp[i..]);
}

fn rgb_to_256(r: u32, g: u32, b: u32) -> u16 {
    // grayscale ramp
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return (232 + (r - 8) / 10) as u16;
    }
    let ri = (r * 5 / 255).min(5);
    let gi = (g * 5 / 255).min(5);
    let bi = (b * 5 / 255).min(5);
    (16 + 36 * ri + 6 * gi + bi) as u16
}

/// xterm 256-color palette -> 0x00RRGGBB. Indices 256/257 are handled by the
/// caller (theme defaults).
pub fn palette(idx: u16) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
        0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
    ];
    if idx < 16 {
        return BASE[idx as usize];
    }
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