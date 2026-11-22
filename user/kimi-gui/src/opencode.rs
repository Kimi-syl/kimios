//! Native OpenCode client: drives a local `opencode serve` over SYS_NET using
//! its HTTP API (Basic auth). Creates a session, sends a prompt, and parses the
//! session messages into the shared `chat::Chat`. no_std, fixed buffers.

use libkimi as k;
use crate::chat::{self, Chat};

pub const PORT: u64 = 4199;

// ---- base64 (for the Basic auth header) -----------------------------------

fn b64(dst: &mut [u8], n: &mut usize, src: &[u8]) {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut i = 0;
    while i + 3 <= src.len() {
        let v = ((src[i] as u32) << 16) | ((src[i + 1] as u32) << 8) | src[i + 2] as u32;
        for s in [18, 12, 6, 0] {
            if *n < dst.len() {
                dst[*n] = T[((v >> s) & 63) as usize];
                *n += 1;
            }
        }
        i += 3;
    }
    let rem = src.len() - i;
    if rem == 1 {
        let v = (src[i] as u32) << 16;
        if *n < dst.len() { dst[*n] = T[((v >> 18) & 63) as usize]; *n += 1; }
        if *n < dst.len() { dst[*n] = T[((v >> 12) & 63) as usize]; *n += 1; }
        if *n < dst.len() { dst[*n] = b'='; *n += 1; }
        if *n < dst.len() { dst[*n] = b'='; *n += 1; }
    } else if rem == 2 {
        let v = ((src[i] as u32) << 16) | ((src[i + 1] as u32) << 8);
        if *n < dst.len() { dst[*n] = T[((v >> 18) & 63) as usize]; *n += 1; }
        if *n < dst.len() { dst[*n] = T[((v >> 12) & 63) as usize]; *n += 1; }
        if *n < dst.len() { dst[*n] = T[((v >> 6) & 63) as usize]; *n += 1; }
        if *n < dst.len() { dst[*n] = b'='; *n += 1; }
    }
}

fn append(dst: &mut [u8], n: &mut usize, src: &[u8]) {
    let take = src.len().min(dst.len() - *n);
    dst[*n..*n + take].copy_from_slice(&src[..take]);
    *n += take;
}

fn auth_header(out: &mut [u8], n: &mut usize, password: &str) {
    let mut cred = [0u8; 96];
    let mut cn = 0usize;
    append(&mut cred, &mut cn, b"opencode:");
    append(&mut cred, &mut cn, password.as_bytes());
    let mut enc = [0u8; 160];
    let mut en = 0usize;
    b64(&mut enc, &mut en, &cred[..cn]);
    append(out, n, b"Authorization: Basic ");
    append(out, n, &enc[..en]);
    append(out, n, b"\r\n");
}

/// POST /api/session {title} with Basic auth.
pub fn build_session_req(password: &str, out: &mut [u8]) -> usize {
    let body = b"{\"title\":\"kimi\"}";
    let mut n = 0usize;
    append(out, &mut n, b"POST /api/session HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nConnection: close\r\n");
    auth_header(out, &mut n, password);
    let mut cl = [0u8; 24];
    let mut cln = 0usize;
    append(&mut cl, &mut cln, b"Content-Length: ");
    append_num(&mut cl, &mut cln, body.len() as u64);
    append(&mut cl, &mut cln, b"\r\n\r\n");
    append(out, &mut n, &cl[..cln]);
    append(out, &mut n, body);
    n
}

/// POST /api/session/{sid}/prompt {text} with Basic auth.
pub fn build_prompt_req(password: &str, sid: &str, text: &str, out: &mut [u8]) -> usize {
    // JSON body {"text":"..."} with escaping
    let mut body = [0u8; 512];
    let mut bn = 0usize;
    append(&mut body, &mut bn, b"{\"text\":\"");
    json_escape(&mut body, &mut bn, text.as_bytes());
    append(&mut body, &mut bn, b"\"}");
    let mut n = 0usize;
    append(out, &mut n, b"POST /api/session/");
    append(out, &mut n, sid.as_bytes());
    append(out, &mut n, b"/prompt HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nConnection: close\r\n");
    auth_header(out, &mut n, password);
    let mut cl = [0u8; 24];
    let mut cln = 0usize;
    append(&mut cl, &mut cln, b"Content-Length: ");
    append_num(&mut cl, &mut cln, bn as u64);
    append(&mut cl, &mut cln, b"\r\n\r\n");
    append(out, &mut n, &cl[..cln]);
    append(out, &mut n, &body[..bn]);
    n
}

/// GET /api/session/{sid}/message with Basic auth.
pub fn build_messages_req(password: &str, sid: &str, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    append(out, &mut n, b"GET /api/session/");
    append(out, &mut n, sid.as_bytes());
    append(out, &mut n, b"/message HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    auth_header(out, &mut n, password);
    append(out, &mut n, b"\r\n");
    n
}

fn append_num(dst: &mut [u8], n: &mut usize, mut v: u64) {
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
    append(dst, n, &tmp[i..]);
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

// ---- transport ------------------------------------------------------------

fn send_all(slot: u64, req: &[u8]) -> bool {
    let mut sent = 0usize;
    while sent < req.len() {
        let w = k::net_tcp_send(slot, &req[sent..]);
        if w == 0 {
            return false;
        }
        sent += w;
    }
    true
}

/// Read the whole response (until close) and return the body slice bounds
/// within `buf`. Returns (body_start, body_len).
fn read_body(slot: u64, buf: &mut [u8]) -> (usize, usize) {
    let mut total = 0usize;
    let mut tmp = [0u8; 4096];
    let mut idle = 0u32;
    loop {
        let n = k::net_tcp_recv(slot, &mut tmp, 250);
        if n == 0 {
            // Wait (bounded) for the first byte; once we have data, a short
            // idle means the response is complete.
            idle += 1;
            if total == 0 && idle < 40 {
                continue;
            }
            break;
        }
        idle = 0;
        let take = n.min(buf.len() - total);
        buf[total..total + take].copy_from_slice(&tmp[..take]);
        total += take;
        if total >= buf.len() {
            break;
        }
    }
    // find "\r\n\r\n"
    let mut i = 0;
    while i + 4 <= total {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            let start = i + 4;
            return (start, total - start);
        }
        i += 1;
    }
    (total, 0)
}

/// Connect + send `req` + read body into `body`; returns body length.
fn request(password_req: &[u8], body: &mut [u8], _pw: &str) -> usize {
    let Some(slot) = k::net_tcp_connect(0x7F00_0001, PORT as u16, 1500) else { return 0 };
    if !send_all(slot, password_req) {
        k::net_tcp_close(slot);
        return 0;
    }
    let (s, l) = read_body(slot, body);
    k::net_tcp_close(slot);
    // move body to front
    if s > 0 && l > 0 {
        body.copy_within(s..s + l, 0);
    }
    l
}

// ---- JSON extraction ------------------------------------------------------

/// Find `"key":"` and return the value bytes (until the closing quote).
fn find_str<'a>(data: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut pat = [0u8; 48];
    let mut pn = 0usize;
    if key.len() + 4 > pat.len() { return None; }
    pat[0] = b'"';
    pat[1..1 + key.len()].copy_from_slice(key);
    pn = 1 + key.len();
    pat[pn] = b'"'; pn += 1;
    pat[pn] = b':'; pn += 1;
    let mut i = 0;
    while i + pn <= data.len() {
        if &data[i..i + pn] == &pat[..pn] {
            let mut j = i + pn;
            while j < data.len() && (data[j] == b' ') { j += 1; }
            if j >= data.len() || data[j] != b'"' { return None; }
            j += 1;
            let start = j;
            while j < data.len() && data[j] != b'"' {
                if data[j] == b'\\' { j += 1; }
                j += 1;
            }
            return Some(&data[start..j.min(data.len())]);
        }
        i += 1;
    }
    None
}

pub fn parse_session_id(data: &[u8], out: &mut [u8]) -> usize {
    match find_str(data, b"id") {
        Some(v) if v.starts_with(b"ses_") => {
            let n = v.len().min(out.len());
            out[..n].copy_from_slice(&v[..n]);
            n
        }
        _ => 0,
    }
}

/// Parse the /message JSON into chat messages: user text, assistant text parts
/// (+ tool names), and idle errors.
fn match_bracket(data: &[u8], start: usize) -> usize {
    if start >= data.len() { return data.len(); }
    let open = data[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0i32;
    let mut i = start;
    let mut in_str = false;
    while i < data.len() {
        let c = data[i];
        if in_str {
            if c == b'\\' { i += 1; }
            else if c == b'"' { in_str = false; }
        } else if c == b'"' {
            in_str = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 { return i + 1; }
        }
        i += 1;
    }
    data.len()
}

fn handle_message(obj: &[u8], chat: &mut Chat) {
    let Some(ty) = find_str(obj, b"type") else { return };
    if ty == b"user" {
        if let Some(v) = find_str(obj, b"text") { chat.push_msg(chat::ROLE_USER, v); }
    } else if ty == b"assistant" {
        let mut buf = [0u8; chat::MSG_CAP];
        let mut bn = 0usize;
        let mut any = false;
        if let Some(cp) = find_from(obj, b"\"content\":[", 0) {
            let arr = cp + 10;
            let arr_end = match_bracket(obj, arr);
            let mut j = arr + 1;
            while j < arr_end {
                while j < arr_end && obj[j] != b'{' { j += 1; }
                if j >= arr_end { break; }
                let pe = match_bracket(obj, j);
                let part = &obj[j..pe.min(arr_end)];
                if let Some(pt) = find_str(part, b"type") {
                    if pt == b"text" {
                        if let Some(v) = find_str(part, b"text") { append(&mut buf, &mut bn, v); any = true; }
                    } else if pt == b"tool" {
                        if let Some(v) = find_str(part, b"name") {
                            append(&mut buf, &mut bn, b"\n[tool: ");
                            append(&mut buf, &mut bn, v);
                            append(&mut buf, &mut bn, b"]");
                            any = true;
                        }
                    }
                }
                j = pe;
            }
        }
        if any { chat.push_msg(chat::ROLE_AI, &buf[..bn]); }
    } else if ty == b"idle" {
        if let Some(v) = find_str(obj, b"message") {
            if v.len() > 6 { chat.push_msg(chat::ROLE_SYSTEM, v); }
        }
    }
}

pub fn parse_messages(data: &[u8], chat: &mut Chat) {
    let Some(dp) = find_from(data, b"\"data\":[", 0) else { return };
    let arr = dp + 7;
    let end = match_bracket(data, arr);
    let mut i = arr + 1;
    while i < end {
        while i < end && data[i] != b'{' { i += 1; }
        if i >= end { break; }
        let oe = match_bracket(data, i);
        handle_message(&data[i..oe.min(end)], chat);
        i = oe;
    }
}

fn find_from(data: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    if from >= data.len() || pat.len() > data.len() { return None; }
    let mut i = from;
    while i + pat.len() <= data.len() {
        if &data[i..i + pat.len()] == pat {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ---- host test hook -------------------------------------------------------

#[cfg(not(target_os = "none"))]
pub fn test_opencode(password: &str, prompt: &str, out: &mut [u8]) -> u64 {
    let mut n = 0usize;
    // create session
    let mut req = [0u8; 512];
    let rl = build_session_req(password, &mut req);
    let mut body = [0u8; 4096];
    let bl = request(&req[..rl], &mut body, password);
    let mut sid = [0u8; 48];
    let sn = parse_session_id(&body[..bl], &mut sid);
    append(out, &mut n, b"[session id len=");
    append_num(out, &mut n, sn as u64);
    append(out, &mut n, b"]\n");
    if sn == 0 {
        return n as u64;
    }
    let sid = core::str::from_utf8(&sid[..sn]).unwrap_or("");
    // prompt
    append(out, &mut n, b"[sid=");
    append(out, &mut n, sid.as_bytes());
    append(out, &mut n, b"]\n");
    let rl = build_prompt_req(password, sid, prompt, &mut req);
    k::sleep_ms(120);
    let mut bl = 0usize;
    for attempt in 0..3 {
        let Some(slot) = k::net_tcp_connect(0x7F00_0001, PORT as u16, 1500) else {
            append(out, &mut n, b"[prompt CONNECTFAIL]\n");
            break;
        };
        let _ = send_all(slot, &req[..rl]);
        let (_s, l) = read_body(slot, &mut body);
        k::net_tcp_close(slot);
        append(out, &mut n, b"[prompt try=");
        append_num(out, &mut n, attempt as u64);
        append(out, &mut n, b" read=");
        append_num(out, &mut n, l as u64);
        append(out, &mut n, b"]\n");
        if l > 0 { bl = l; break; }
        k::sleep_ms(200);
    }
    k::sleep_ms(6000);
    // messages
    let rl = build_messages_req(password, sid, &mut req);
    let bl = request(&req[..rl], &mut body, password);
    let mut chat = Chat::new();
    parse_messages(&body[..bl], &mut chat);
    append(out, &mut n, b"[messages parsed=");
    append_num(out, &mut n, chat.msg_count as u64);
    append(out, &mut n, b"]\n");
    for i in 0..chat.msg_count {
        let m = &chat.msgs[i];
        append(out, &mut n, if m.role == chat::ROLE_USER { b"You: " } else if m.role == chat::ROLE_AI { b"AI: " } else { b"Sys: " });
        let take = m.len.min(out.len() - n);
        append(out, &mut n, &m.text[..take]);
        append(out, &mut n, b"\n");
    }
    n as u64
}