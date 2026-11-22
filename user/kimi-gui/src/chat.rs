//! Shared native chat layer: message state, line-edited input, a streaming SSE
//! pump (OpenAI/llama-server style), and a reusable chat view. AI Studio uses
//! it for llama-server; the native OpenCode client will reuse the same state +
//! view with its own request/event format.

use libkimi as k;
use crate::draw::Screen;
use crate::theme::{BLACK, SURFACE_HI, OUTLINE, TEXT, MUTED, RED, RED_DIM, GREEN};

pub const MAX_MSGS: usize = 24;
pub const MSG_CAP: usize = 512;
pub const INPUT_CAP: usize = 200;

pub const ROLE_USER: u8 = 0;
pub const ROLE_AI: u8 = 1;
pub const ROLE_SYSTEM: u8 = 2;

#[derive(Clone, Copy)]
pub struct Msg {
    pub role: u8,
    pub text: [u8; MSG_CAP],
    pub len: usize,
}
pub const EMPTY_MSG: Msg = Msg { role: 0, text: [0; MSG_CAP], len: 0 };

pub struct Chat {
    pub msgs: [Msg; MAX_MSGS],
    pub msg_count: usize,
    pub input: [u8; INPUT_CAP],
    pub input_len: usize,
    pub partial: [u8; MSG_CAP],
    pub partial_len: usize,
    pub streaming: bool,
    pub net: Option<u64>,
    pub carry: [u8; 512],
    pub carry_len: usize,
    pub status: [u8; 64],
    pub status_len: usize,
}

impl Chat {
    pub fn new() -> Self {
        Self {
            msgs: [EMPTY_MSG; MAX_MSGS],
            msg_count: 0,
            input: [0; INPUT_CAP],
            input_len: 0,
            partial: [0; MSG_CAP],
            partial_len: 0,
            streaming: false,
            net: None,
            carry: [0; 512],
            carry_len: 0,
            status: [0; 64],
            status_len: 0,
        }
    }

    pub fn set_status(&mut self, s: &str) {
        let b = s.as_bytes();
        self.status_len = b.len().min(64);
        self.status[..self.status_len].copy_from_slice(&b[..self.status_len]);
    }

    pub fn push_msg(&mut self, role: u8, text: &[u8]) {
        if self.msg_count >= MAX_MSGS {
            for i in 2..MAX_MSGS {
                self.msgs[i - 2] = self.msgs[i];
            }
            self.msg_count = MAX_MSGS - 2;
        }
        let m = &mut self.msgs[self.msg_count];
        m.role = role;
        m.len = text.len().min(MSG_CAP);
        m.text[..m.len].copy_from_slice(&text[..m.len]);
        self.msg_count += 1;
    }

    /// Start a streaming reply on an already-connected socket.
    pub fn begin(&mut self, slot: u64) {
        self.net = Some(slot);
        self.streaming = true;
        self.partial_len = 0;
        self.carry_len = 0;
        self.set_status("Thinking...");
    }

    /// Drain the socket once (non-blocking-ish) and parse SSE deltas into the
    /// running assistant reply. Returns true when something changed.
    pub fn pump(&mut self) -> bool {
        let Some(slot) = self.net else {
            self.streaming = false;
            return false;
        };
        let mut buf = [0u8; 1024];
        let n = k::net_tcp_recv(slot, &mut buf, 12);
        if n == 0 {
            return false;
        }
        let done = feed_chunk(
            &buf[..n],
            &mut self.carry,
            &mut self.carry_len,
            &mut self.partial,
            &mut self.partial_len,
        );
        if done {
            let mut tmp = [0u8; MSG_CAP];
            let pl = self.partial_len.min(MSG_CAP);
            tmp[..pl].copy_from_slice(&self.partial[..pl]);
            self.push_msg(ROLE_AI, &tmp[..pl]);
            self.partial_len = 0;
            self.streaming = false;
            self.net = None;
            k::net_tcp_close(slot);
            self.set_status("Done");
        }
        true
    }

    /// Copy the current input line out and clear it (call on submit).
    pub fn take_input(&mut self, out: &mut [u8]) -> usize {
        let n = self.input_len.min(self.input.len()).min(out.len());
        out[..n].copy_from_slice(&self.input[..n]);
        self.input_len = 0;
        n
    }

    /// Cancel any in-flight stream.
    pub fn abort(&mut self) {
        if let Some(slot) = self.net {
            k::net_tcp_close(slot);
        }
        self.net = None;
        self.streaming = false;
    }
}

/// Line-edit a keystroke into the input buffer. Returns true on Enter (submit).
pub fn key(chat: &mut Chat, code: u16, shift: bool) -> bool {
    match crate::apps::key_event(code, shift) {
        Some((0, true)) => {
            chat.input_len = chat.input_len.saturating_sub(1);
            false
        }
        Some((b'\n', _)) => true,
        Some((byte, _)) => {
            if chat.input_len < chat.input.len() {
                chat.input[chat.input_len] = byte;
                chat.input_len += 1;
            }
            false
        }
        None => false,
    }
}

// ---- SSE parsing (OpenAI/llama: choices[].delta.content) -------------------

fn append(dst: &mut [u8], n: &mut usize, src: &[u8]) {
    let take = src.len().min(dst.len() - *n);
    dst[*n..*n + take].copy_from_slice(&src[..take]);
    *n += take;
}

pub fn parse_sse_line(line: &[u8], out: &mut [u8], out_len: &mut usize) -> bool {
    let data = if line.starts_with(b"data:") {
        let mut s = 5;
        while s < line.len() && line[s] == b' ' {
            s += 1;
        }
        &line[s..]
    } else {
        return false;
    };
    if data.starts_with(b"[DONE]") {
        return true;
    }
    let key = b"\"content\":\"";
    let mut i = 0;
    while i + key.len() <= data.len() {
        if &data[i..i + key.len()] == key {
            let mut j = i + key.len();
            while j < data.len() {
                let b = data[j];
                if b == b'"' {
                    break;
                }
                if b == b'\\' && j + 1 < data.len() {
                    let c = match data[j + 1] {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        e => e,
                    };
                    if *out_len < out.len() {
                        out[*out_len] = c;
                        *out_len += 1;
                    }
                    j += 2;
                } else {
                    if *out_len < out.len() {
                        out[*out_len] = b;
                        *out_len += 1;
                    }
                    j += 1;
                }
            }
            return false;
        }
        i += 1;
    }
    false
}

pub fn feed_chunk(
    chunk: &[u8],
    carry: &mut [u8; 512],
    carry_len: &mut usize,
    out: &mut [u8],
    out_len: &mut usize,
) -> bool {
    let mut done = false;
    let mut start = 0usize;
    for i in 0..chunk.len() {
        if chunk[i] == b'\n' {
            let mut line = [0u8; 512];
            let mut ln = 0usize;
            append(&mut line, &mut ln, &carry[..*carry_len]);
            append(&mut line, &mut ln, &chunk[start..i]);
            *carry_len = 0;
            let end = if ln > 0 && line[ln - 1] == b'\r' { ln - 1 } else { ln };
            if parse_sse_line(&line[..end], out, out_len) {
                done = true;
            }
            start = i + 1;
        }
    }
    let rem = &chunk[start..];
    let take = rem.len().min(carry.len() - *carry_len);
    carry[*carry_len..*carry_len + take].copy_from_slice(&rem[..take]);
    *carry_len += take;
    done
}

// ---- reusable view ---------------------------------------------------------

fn lines(len: usize, cols: usize) -> usize {
    if len == 0 { 1 } else { (len + cols - 1) / cols }
}

fn emit(screen: &Screen, y: &mut i64, no: &mut usize, skip: usize, max_y: i64, text: &[u8], color: u32) {
    if *no >= skip && *y + 13 <= max_y {
        screen.text_mono(20, *y, crate::apps::slice_str(text), 1, color);
        *y += 13;
    }
    *no += 1;
}

/// Render the message history + streaming partial + input line.
/// `top` is where messages start; `max_y` bounds them; input goes at `max_y`.
pub fn draw_body(screen: &Screen, chat: &Chat, top: i64, max_y: i64, osk: bool, hint: &str) {
    let width = screen.width();
    let cols = 60usize;
    let maxlines = ((max_y - top) / 13) as usize;
    let mut total = 0usize;
    for i in 0..chat.msg_count {
        total += 1 + lines(chat.msgs[i].len, cols);
    }
    if chat.streaming {
        total += 1 + lines(chat.partial_len, cols);
    }
    let skip = total.saturating_sub(maxlines);
    let mut y = top;
    let mut no = 0usize;
    for i in 0..chat.msg_count {
        let m = &chat.msgs[i];
        let (tag, color) = match m.role {
            ROLE_USER => ("You", TEXT()),
            ROLE_SYSTEM => ("Sys", MUTED()),
            _ => ("AI", GREEN()),
        };
        emit(screen, &mut y, &mut no, skip, max_y, tag.as_bytes(), RED());
        let mut off = 0usize;
        loop {
            let end = (off + cols).min(m.len);
            emit(screen, &mut y, &mut no, skip, max_y, &m.text[off..end], color);
            off = end;
            if off >= m.len {
                break;
            }
        }
    }
    if chat.streaming {
        emit(screen, &mut y, &mut no, skip, max_y, b"AI", RED());
        let mut off = 0usize;
        loop {
            let end = (off + cols).min(chat.partial_len);
            emit(screen, &mut y, &mut no, skip, max_y, &chat.partial[off..end], GREEN());
            off = end;
            if off >= chat.partial_len {
                break;
            }
        }
    }
    if chat.msg_count == 0 && !chat.streaming {
        screen.text_centered(width / 2, top + 90, "Local AI chat", 2, MUTED());
        screen.text_centered(width / 2, top + 122, hint, 1, MUTED());
    }

    let iy = max_y + 2;
    screen.fill(20, iy - 6, width - 40, 1, OUTLINE());
    screen.text_mono(20, iy, ">", 2, RED());
    let shown = chat.input_len.min(48);
    screen.text_mono(44, iy, crate::apps::slice_str(&chat.input[..shown]), 2, TEXT());
    if chat.input_len == 0 {
        screen.text_mono(44, iy, "ask anything...", 2, 0x0050_505a);
    }
    let _ = osk;
    let _ = SURFACE_HI;
}