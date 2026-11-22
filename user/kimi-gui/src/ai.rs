//! AI Studio client: talks to a guest `llama-server` (OpenAI-compatible API)
//! over SYS_NET on 127.0.0.1. Message state, streaming replay and input come
//! from the shared `chat::Chat`; this module owns the llama-server lifecycle
//! and the HTTP/JSON request format.

use libkimi as k;
use crate::chat::{self, Chat};

pub const PORT: u64 = 8080;

pub struct Ai {
    pub server_slot: Option<u64>,
    pub server_started: bool,
    pub model: [u8; 48],
    pub model_len: usize,
    pub server_log: [u8; 512],
    pub server_log_len: usize,
    pub chat: Chat,
}

impl Ai {
    pub fn new() -> Self {
        Self {
            server_slot: None,
            server_started: false,
            model: [0; 48],
            model_len: 0,
            server_log: [0; 512],
            server_log_len: 0,
            chat: Chat::new(),
        }
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

/// Build the HTTP POST (headers + JSON) for /v1/chat/completions, streaming.
pub fn build_request(chat: &Chat, out: &mut [u8]) -> usize {
    let mut body = [0u8; 4096];
    let mut bn = 0usize;
    append(&mut body, &mut bn, b"{\"model\":\"local\",\"stream\":true,\"max_tokens\":256,\"messages\":[");
    append(&mut body, &mut bn, b"{\"role\":\"system\",\"content\":\"You are a helpful on-device assistant. Keep replies short.\"}");
    for i in 0..chat.msg_count {
        let m = &chat.msgs[i];
        append(&mut body, &mut bn, b",{\"role\":\"");
        append(&mut body, &mut bn, if m.role == chat::ROLE_USER { b"user" } else { b"assistant" });
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

/// Open the streaming chat connection and send the request. Returns the slot.
pub fn begin_chat(ai: &Ai) -> Option<u64> {
    let slot = k::net_tcp_connect(0x7F00_0001, PORT as u16, 1500)?;
    let mut req = [0u8; 4600];
    let n = build_request(&ai.chat, &mut req);
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

// ---- host test hooks -------------------------------------------------------

#[cfg(not(target_os = "none"))]
pub fn test_request(out: &mut [u8]) -> usize {
    let mut chat = Chat::new();
    chat.push_msg(chat::ROLE_USER, b"hello \"world\"\nline2");
    build_request(&chat, out)
}

#[cfg(not(target_os = "none"))]
pub fn test_sse(chunk: &str, out: &mut [u8]) -> u64 {
    let mut carry = [0u8; 512];
    let mut cl = 0usize;
    let mut ol = 0usize;
    let done = chat::feed_chunk(chunk.as_bytes(), &mut carry, &mut cl, out, &mut ol);
    ((done as u64) << 32) | ol as u64
}

#[cfg(not(target_os = "none"))]
pub fn test_chat(prompt: &str, out: &mut [u8]) -> u64 {
    let mut chat = Chat::new();
    chat.push_msg(chat::ROLE_USER, prompt.as_bytes());
    let Some(slot) = k::net_tcp_connect(0x7F00_0001, PORT as u16, 1500) else { return 0 };
    let mut req = [0u8; 4600];
    let n = build_request(&chat, &mut req);
    let mut sent = 0usize;
    while sent < n {
        let w = k::net_tcp_send(slot, &req[sent..]);
        if w == 0 { k::net_tcp_close(slot); return 0; }
        sent += w;
    }
    let mut carry = [0u8; 512];
    let mut cl = 0usize;
    let mut pl = 0usize;
    let mut buf = [0u8; 1024];
    let mut idle = 0u32;
    loop {
        let r = k::net_tcp_recv(slot, &mut buf, 60);
        if r > 0 {
            idle = 0;
            if chat::feed_chunk(&buf[..r], &mut carry, &mut cl, out, &mut pl) {
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