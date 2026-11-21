//! AI Studio client: talks to a guest `llama-server` (OpenAI-compatible API)
//! over SYS_NET on 127.0.0.1. Builds chat requests and parses streaming SSE
//! deltas into the running assistant reply. no_std, no allocator.

use libkimi as k;

pub const PORT: u64 = 8080;
const MAX_MSGS: usize = 12;
const MSG_CAP: usize = 384;

#[derive(Clone, Copy)]
pub struct Msg {
    pub role: u8, // 0 user, 1 assistant, 2 system
    pub text: [u8; MSG_CAP],
    pub len: usize,
}

pub const EMPTY_MSG: Msg = Msg { role: 0, text: [0; MSG_CAP], len: 0 };

pub struct Ai {
    pub server_slot: Option<u64>,
    pub server_started: bool,
    pub model: [u8; 48],
    pub model_len: usize,
    pub msgs: [Msg; MAX_MSGS],
    pub msg_count: usize,
    pub input: [u8; 160],
    pub input_len: usize,
    pub net_slot: Option<u64>,
    pub streaming: bool,
    pub partial: [u8; MSG_CAP],
    pub partial_len: usize,
    pub status: [u8; 64],
    pub status_len: usize,
    pub scroll: usize,
    pub server_log: [u8; 512],
    pub server_log_len: usize,
}

impl Ai {
    pub fn new() -> Self {
        Self {
            server_slot: None,
            server_started: false,
            model: [0; 48],
            model_len: 0,
            msgs: [EMPTY_MSG; MAX_MSGS],
            msg_count: 0,
            input: [0; 160],
            input_len: 0,
            net_slot: None,
            streaming: false,
            partial: [0; MSG_CAP],
            partial_len: 0,
            status: [0; 64],
            status_len: 0,
            scroll: 0,
            server_log: [0; 512],
            server_log_len: 0,
        }
    }

    pub fn set_status(&mut self, s: &str) {
        let b = s.as_bytes();
        self.status_len = b.len().min(64);
        self.status[..self.status_len].copy_from_slice(&b[..self.status_len]);
    }

    pub fn push_msg(&mut self, role: u8, text: &[u8]) {
        if self.msg_count >= MAX_MSGS {
            // drop the oldest pair to keep a rolling window
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
}

fn append(dst: &mut [u8], n: &mut usize, src: &[u8]) {
    let take = src.len().min(dst.len() - *n);
    dst[*n..*n + take].copy_from_slice(&src[..take]);
    *n += take;
}

fn json_escape(dst: &mut [u8], n: &mut usize, src: &[u8]) {
    for &b in src {
        match b {
            b'"' => append(dst, n, b"\\\""),
            b'\\' => append(dst, n, b"\\\\"),
            b'\n' => append(dst, n, b"\\n"),
            b'\r' => append(dst, n, b"\\r"),
            b'\t' => append(dst, n, b"\\t"),
            0x20..=0x7e => {
                if *n < dst.len() {
                    dst[*n] = b;
                    *n += 1;
                }
            }
            _ => {}
        }
    }
}

/// Build the full HTTP POST (headers + JSON body) for /v1/chat/completions with
/// streaming. Returns the byte length written into `out`.
pub fn build_request(ai: &Ai, out: &mut [u8]) -> usize {
    let mut body = [0u8; 4096];
    let mut bn = 0usize;
    append(&mut body, &mut bn, b"{\"model\":\"local\",\"stream\":true,\"max_tokens\":256,\"messages\":[");
    append(&mut body, &mut bn, b"{\"role\":\"system\",\"content\":\"You are a helpful on-device assistant. Keep replies short.\"}");
    for i in 0..ai.msg_count {
        let m = &ai.msgs[i];
        append(&mut body, &mut bn, b",{\"role\":\"");
        append(&mut body, &mut bn, if m.role == 0 { b"user" } else { b"assistant" });
        append(&mut body, &mut bn, b"\",\"content\":\"");
        json_escape(&mut body, &mut bn, &m.text[..m.len]);
        append(&mut body, &mut bn, b"\"}");
    }
    append(&mut body, &mut bn, b"]}");

    let mut n = 0usize;
    append(out, &mut n, b"POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nContent-Length: ");
    let mut num = [0u8; 20];
    let mut i = num.len();
    let mut v = bn as u64;
    loop {
        i -= 1;
        num[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    append(out, &mut n, &num[i..]);
    append(out, &mut n, b"\r\nConnection: close\r\n\r\n");
    append(out, &mut n, &body[..bn]);
    n
}

/// Extract the assistant text delta from one SSE `data: {json}` line. Appends
/// the decoded content to `out`. Returns true if the stream is done ([DONE]).
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
    // find "content":"..." within choices[0].delta
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
                    let e = data[j + 1];
                    let c = match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        _ => e,
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

/// Feed a raw socket chunk (may contain partial lines) into the SSE parser.
/// `carry` holds an incomplete trailing line between calls. Appends decoded
/// text to `out`. Returns true when [DONE] seen.
pub fn feed_chunk(chunk: &[u8], carry: &mut [u8; 512], carry_len: &mut usize, out: &mut [u8], out_len: &mut usize) -> bool {
    let mut done = false;
    let mut start = 0usize;
    for i in 0..chunk.len() {
        if chunk[i] == b'\n' {
            let mut line = [0u8; 512];
            let mut ln = 0usize;
            append(&mut line, &mut ln, &carry[..*carry_len]);
            append(&mut line, &mut ln, &chunk[start..i]);
            *carry_len = 0;
            // trim trailing \r
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

/// Open the streaming chat connection and send the request. Returns the net
/// slot on success.
pub fn begin_chat(ai: &Ai) -> Option<u64> {
    let slot = k::net_tcp_connect(0x7F00_0001, PORT as u16, 1500)?;
    let mut req = [0u8; 4600];
    let n = build_request(ai, &mut req);
    let mut sent = 0usize;
    while sent < n {
        let w = k::net_tcp_send(slot, &req[sent..]);
        if w == 0 {
            k::net_tcp_close(slot);
            return None;
        }
        sent += w;
    }
    Some(slot)
}

#[cfg(not(target_os = "none"))]
pub fn test_request(out: &mut [u8]) -> usize {
    let mut ai = Ai::new();
    ai.push_msg(0, b"hello \"world\"\nline2");
    build_request(&ai, out)
}

#[cfg(not(target_os = "none"))]
pub fn test_sse(chunk: &str, out: &mut [u8]) -> u64 {
    let mut carry = [0u8; 512];
    let mut cl = 0usize;
    let mut ol = 0usize;
    let done = feed_chunk(chunk.as_bytes(), &mut carry, &mut cl, out, &mut ol);
    ((done as u64) << 32) | ol as u64
}

/// Host-only end-to-end: send `prompt` to a running llama-server and accumulate
/// the streamed reply into `out` (SYS_NET routed to real sockets by the
/// harness). Returns the reply length (0 = connect failed).
#[cfg(not(target_os = "none"))]
pub fn test_chat(prompt: &str, out: &mut [u8]) -> u64 {
    let mut ai = Ai::new();
    ai.push_msg(0, prompt.as_bytes());
    let Some(slot) = begin_chat(&ai) else { return 0 };
    let mut carry = [0u8; 512];
    let mut cl = 0usize;
    let mut pl = 0usize;
    let mut buf = [0u8; 1024];
    let mut idle = 0u32;
    loop {
        let n = k::net_tcp_recv(slot, &mut buf, 60);
        if n > 0 {
            idle = 0;
            if feed_chunk(&buf[..n], &mut carry, &mut cl, out, &mut pl) {
                break;
            }
        } else {
            idle += 1;
            if idle > 200 {
                break;
            }
        }
    }
    k::net_tcp_close(slot);
    pl as u64
}