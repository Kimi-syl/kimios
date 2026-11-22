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
    if !password.is_empty() { auth_header(out, &mut n, password); }
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
    if !password.is_empty() { auth_header(out, &mut n, password); }
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
    if !password.is_empty() { auth_header(out, &mut n, password); }
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

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() { return false; }
    hay.windows(needle.len()).any(|w| w == needle)
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
// ===================== optional OpenCode app (engine on demand) =============
// The 219 MB engine is NOT bundled; the app downloads it into the sysroot on
// demand (fetch_url) and extracts it with the static busybox `tar` through the
// guest loader.

pub const OC_URL: &str =
    "https://github.com/anomalyco/opencode/releases/latest/download/opencode-linux-arm64-musl.tar.gz";

pub struct Oc {
    pub server_slot: Option<u64>,
    pub started: bool,
    pub installed: bool,
    pub pw: [u8; 64],
    pub pw_len: usize,
    pub logscan: usize,
    pub logbuf: [u8; 2048],
    pub session: [u8; 48],
    pub session_len: usize,
    pub chat: Chat,
    pub status: [u8; 64],
    pub status_len: usize,
    pub pending: bool,
    pub last_poll: u64,
    pub ready: bool,
    pub models: [ModelEntry; 32],
    pub model_count: usize,
    pub sel_model: usize,
    pub picker: bool,
    pub installing: bool,
    pub stage: u8,
    pub dldone: u64,
    pub dltotal: u64,
    pub auto_fetched: bool,
    pub has_model: bool,
}

impl Oc {
    pub fn new() -> Self {
        Self {
            server_slot: None,
            started: false,
            installed: false,
            pw: [0; 64],
            pw_len: 0,
            logscan: 0,
            logbuf: [0; 2048],
            session: [0; 48],
            session_len: 0,
            chat: Chat::new(),
            status: [0; 64],
            status_len: 0,
            pending: false,
            last_poll: 0,
            ready: false,
            models: [EMPTY_MODEL; 32],
            model_count: 0,
            sel_model: 0,
            picker: false,
            installing: false,
            stage: 0,
            dldone: 0,
            dltotal: 0,
            auto_fetched: false,
            has_model: false,
        }
    }

    pub fn set_status(&mut self, s: &str) {
        let b = s.as_bytes();
        self.status_len = b.len().min(64);
        self.status[..self.status_len].copy_from_slice(&b[..self.status_len]);
    }

    pub fn refresh_installed(&mut self) {
        self.installed = k::stat("/disk/alpine/root/usr/bin/opencode").is_some()
            || k::stat("/disk/alpine/root/bin/opencode").is_some();
        if self.installed && self.status_len == 0 {
            self.set_status("Tap Start to run opencode serve");
        }
    }

    /// On-demand install: download the release tarball, extract the binary into
    /// the sysroot's usr/bin (blocking; large). Returns installed.
    pub fn install(&mut self) -> bool {
        let _ = k::mkdir("/disk/opencode");
        let _ = k::mkdir("/disk/alpine/root/usr/bin");
        self.set_status("Downloading OpenCode (large)...");
        if k::fetch_url(OC_URL, "/disk/opencode/oc.tar.gz").is_none() {
            self.set_status("Download failed (network?)");
            return false;
        }
        // Validate: gzip magic + plausible size (compressed ~78 MB).
        let mut ok = false;
        if let Some((_, size)) = k::stat("/disk/opencode/oc.tar.gz") {
            if size > 40_000_000 {
                if let Some(h) = k::open("/disk/opencode/oc.tar.gz") {
                    let mut m = [0u8; 2];
                    let n = k::read_file(h, 0, &mut m);
                    k::close(h);
                    ok = n == 2 && m[0] == 0x1f && m[1] == 0x8b;
                }
            }
        }
        if !ok {
            self.set_status("Download incomplete - retry");
            return false;
        }
        self.set_status("Extracting...");
        let mut sink = [0u8; 256];
        // relative to sysroot cwd: ../../opencode = <diskRoot>/opencode
        let _ = k::linux_run(
            "busybox",
            "tar xzf ../../opencode/oc.tar.gz -C ../../alpine/root/usr/bin",
            &mut sink,
        );
        // The engine is a normal dynamic musl ELF but needs libstdc++/libgcc;
        // resolve+install them (with their deps) from the Alpine index so they
        // are guaranteed present regardless of what else is installed.
        self.set_status("Installing runtime libs...");
        let _ = k::alpine::update("");
        let _ = k::alpine::install("libstdc++");
        let _ = k::alpine::install("libgcc");
        self.set_status("Finishing...");
        self.refresh_installed();
        if self.installed {
            self.set_status("Installed - tap Start");
        } else {
            self.set_status("Extract failed");
        }
        self.installed
    }

    /// Write an opencode config so providers initialize (local llama-server
    /// provider; the built-in free provider also appears) into the guest HOME.
    fn write_config() {
        let _ = k::mkdir("/disk/home");
        let _ = k::mkdir("/disk/home/.config");
        let _ = k::mkdir("/disk/home/.config/opencode");
        let cfg: &[u8] = b"{\"$schema\":\"https://opencode.ai/config.json\",\"providers\":{\"local\":{\"name\":\"Local\",\"package\":\"@opencode/ai/providers/openai-compatible\",\"settings\":{\"baseURL\":\"http://127.0.0.1:8080/v1\",\"apiKey\":\"x\"},\"models\":{\"local\":{\"modelID\":\"local\",\"name\":\"Local llama-server\"}}}}}";
        if let Some(h) = k::create("/disk/home/.config/opencode/opencode.json") {
            let _ = k::write_file(h, 0, cfg);
            k::close(h);
        }
    }

    pub fn start_server(&mut self) {
        if !self.installed {
            self.refresh_installed();
            if !self.installed {
                self.set_status("Install OpenCode first");
                return;
            }
        }
        Self::write_config();
        match k::proc_spawn("opencode", "serve --port 4199", 0, 0) {
            Some(slot) => {
                self.server_slot = Some(slot);
                self.started = true;
                self.pw_len = 0;
                self.logscan = 0;
                self.ready = false;
                self.auto_fetched = false;
                self.set_status("Starting server...");
            }
            None => self.set_status("opencode not runnable yet"),
        }
    }

    pub fn stop_server(&mut self) {
        if let Some(s) = self.server_slot {
            let _ = k::proc_kill(s);
            let _ = k::proc_close(s);
        }
        self.server_slot = None;
        self.started = false;
        self.set_status("Server stopped");
    }

    /// Drain the server log, extracting the auto-generated password.
    pub fn pump_log(&mut self) -> bool {
        let Some(slot) = self.server_slot else { return false };
        let mut tmp = [0u8; 512];
        let mut changed = false;
        loop {
            let Some(n) = k::proc_read(slot, &mut tmp) else { break };
            if n == 0 { break; }
            for &b in &tmp[..n] {
                if self.logscan < self.logbuf.len() {
                    self.logbuf[self.logscan] = b;
                    self.logscan += 1;
                }
            }
            changed = true;
            if n < tmp.len() { break; }
        }
        // Detect readiness: either a printed password (secured) or "listening"/"unsecured".
        {
            let buf = &self.logbuf[..self.logscan];
            if contains(buf, b"listening on") || contains(buf, b"unsecured") || self.pw_len > 0 {
                self.ready = true;
            }
        }
        // scan for "server password "
        if self.pw_len == 0 {
            let key = b"server password ";
            let buf = &self.logbuf[..self.logscan];
            let mut i = 0;
            while i + key.len() <= buf.len() {
                if &buf[i..i + key.len()] == key {
                    let mut j = i + key.len();
                    let mut pn = 0usize;
                    while j < buf.len() && pn < 63 {
                        let c = buf[j];
                        if c == b'\n' || c == b'\r' || c == b' ' { break; }
                        self.pw[pn] = c;
                        pn += 1;
                        j += 1;
                    }
                    self.pw_len = pn;
                    if pn > 0 { self.set_status("Server ready"); }
                    break;
                }
                i += 1;
            }
        }
        if let Some(Some(_)) = k::proc_status(slot) {
            self.server_slot = None;
            self.started = false;
            self.set_status("Server exited");
            changed = true;
        }
        changed
    }

    fn pw_str(&self) -> &str {
        core::str::from_utf8(&self.pw[..self.pw_len]).unwrap_or("")
    }

    /// Ensure a session exists; create one if needed. Returns true on success.
    fn ensure_session(&mut self) -> bool {
        if self.session_len > 0 {
            return true;
        }
        let pw = self.pw_str();
        let mut req = [0u8; 512];
        let rl = build_session_req(pw, &mut req);
        let mut body = [0u8; 4096];
        let bl = http_transport(&req[..rl], &mut body);
        let mut sid = [0u8; 48];
        let sn = parse_session_id(&body[..bl], &mut sid);
        if sn == 0 {
            self.set_status("Session create failed");
            return false;
        }
        self.session[..sn].copy_from_slice(&sid[..sn]);
        self.session_len = sn;
        true
    }

    /// Send a prompt (creating the session first). Pushes the user msg locally.
    pub fn send(&mut self) {
        if self.server_slot.is_none() || !self.ready {
            self.set_status("Start the server first");
            return;
        }
        if !self.has_model {
            self.set_status("Tap Model to choose one first");
            return;
        }
        let mut text = [0u8; 200];
        let n = self.chat.take_input(&mut text);
        if n == 0 {
            return;
        }
        if !self.ensure_session() {
            return;
        }
        self.chat.push_msg(chat::ROLE_USER, &text[..n]);
        let pw = self.pw_str();
        let sid = core::str::from_utf8(&self.session[..self.session_len]).unwrap_or("");
        let tstr = core::str::from_utf8(&text[..n]).unwrap_or("");
        let mut req = [0u8; 768];
        let rl = build_prompt_req(pw, sid, tstr, &mut req);
        let mut body = [0u8; 512];
        // retry a few times (server slow to accept on fresh connections)
        for _ in 0..3 {
            let l = http_transport(&req[..rl], &mut body);
            if l > 0 { break; }
            k::sleep_ms(150);
        }
        self.pending = true;
        self.last_poll = 0;
        self.chat.set_status("Thinking...");
    }

    /// Poll the session messages and rebuild the chat. Returns true on change.
    pub fn poll(&mut self) -> bool {
        if !self.pending || self.session_len == 0 {
            return false;
        }
        let pw = self.pw_str();
        let sid = core::str::from_utf8(&self.session[..self.session_len]).unwrap_or("");
        let mut req = [0u8; 256];
        let rl = build_messages_req(pw, sid, &mut req);
        let mut body = [0u8; 8192];
        let bl = http_transport(&req[..rl], &mut body);
        if bl == 0 {
            return false;
        }
        self.chat.msg_count = 0;
        self.chat.partial_len = 0;
        parse_messages(&body[..bl], &mut self.chat);
        // done when an assistant reply or a terminal idle shows up
        let mut done = false;
        for i in 0..self.chat.msg_count {
            let r = self.chat.msgs[i].role;
            if r == chat::ROLE_AI || r == chat::ROLE_SYSTEM { done = true; }
        }
        if done {
            self.pending = false;
            self.chat.set_status("Done");
        }
        true
    }
}

/// Convenience: connect, send a prepared request, read the body. Returns body len.
fn http_transport(req: &[u8], body: &mut [u8]) -> usize {
    let Some(slot) = k::net_tcp_connect(0x7F00_0001, PORT as u16, 1500) else { return 0 };
    if !send_all(slot, req) {
        k::net_tcp_close(slot);
        return 0;
    }
    let (s, l) = read_body(slot, body);
    k::net_tcp_close(slot);
    if s > 0 && l > 0 {
        body.copy_within(s..s + l, 0);
    }
    l
}

impl Oc {
    /// Begin an on-demand download (non-blocking). Progress is polled in
    /// pump_install()/drawn each frame, so the bar reflects REAL bytes.
    pub fn start_install(&mut self) {
        if self.installing { return; }
        let _ = k::mkdir("/disk/opencode");
        let _ = k::mkdir("/disk/alpine/root/usr/bin");
        self.dldone = 0;
        self.dltotal = 0;
        if k::fetch_async(OC_URL, "/disk/opencode/oc.tar.gz") {
            self.installing = true;
            self.stage = 1;
            self.set_status("Downloading...");
        } else {
            self.set_status("Download start failed");
        }
    }

    /// Advance the install state machine. Returns true when the display changed.
    pub fn pump_install(&mut self) -> bool {
        if !self.installing {
            return false;
        }
        if self.stage == 1 {
            let mut p = [0u64; 3];
            if k::fetch_progress(&mut p) {
                self.dldone = p[0];
                self.dltotal = p[1];
                match p[2] {
                    2 => self.stage = 2,
                    3 => {
                        self.set_status("Download failed");
                        self.installing = false;
                        self.stage = 5;
                        return true;
                    }
                    _ => {}
                }
            }
            if self.stage == 1 {
                return true;
            }
        }
        if self.stage == 2 {
            self.set_status("Extracting...");
            let mut ok = false;
            if let Some((_, size)) = k::stat("/disk/opencode/oc.tar.gz") {
                if size > 40_000_000 {
                    if let Some(h) = k::open("/disk/opencode/oc.tar.gz") {
                        let mut m = [0u8; 2];
                        let n = k::read_file(h, 0, &mut m);
                        k::close(h);
                        ok = n == 2 && m[0] == 0x1f && m[1] == 0x8b;
                    }
                }
            }
            if !ok {
                self.set_status("Download incomplete - retry");
                self.installing = false;
                self.stage = 5;
                return true;
            }
            let mut sink = [0u8; 256];
            let _ = k::linux_run("busybox", "tar xzf ../../opencode/oc.tar.gz -C ../../alpine/root/usr/bin", &mut sink);
            self.set_status("Installing runtime libs...");
            let _ = k::alpine::update("");
            let _ = k::alpine::install("libstdc++");
            let _ = k::alpine::install("libgcc");
            self.refresh_installed();
            self.installing = false;
            self.stage = 4;
            self.set_status(if self.installed { "Installed - tap Start" } else { "Extract failed" });
            return true;
        }
        false
    }
}

// ===================== model list / selection ==============================

#[derive(Clone, Copy)]
pub struct ModelEntry {
    pub name: [u8; 40],
    pub name_len: usize,
    pub provider: [u8; 24],
    pub provider_len: usize,
    pub model: [u8; 40],
    pub model_len: usize,
}
pub const EMPTY_MODEL: ModelEntry = ModelEntry {
    name: [0; 40], name_len: 0, provider: [0; 24], provider_len: 0, model: [0; 40], model_len: 0,
};
impl ModelEntry {
    pub fn name_str(&self) -> &str { core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("") }
    pub fn provider_str(&self) -> &str { core::str::from_utf8(&self.provider[..self.provider_len]).unwrap_or("") }
    pub fn model_str(&self) -> &str { core::str::from_utf8(&self.model[..self.model_len]).unwrap_or("") }
}

pub fn build_models_req(password: &str, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    append(out, &mut n, b"GET /api/model HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if !password.is_empty() { auth_header(out, &mut n, password); }
    append(out, &mut n, b"\r\n");
    n
}

pub fn build_set_model_req(password: &str, sid: &str, provider: &str, model: &str, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    append(out, &mut n, b"POST /api/session/");
    append(out, &mut n, sid.as_bytes());
    append(out, &mut n, b"/model HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nConnection: close\r\n");
    if !password.is_empty() { auth_header(out, &mut n, password); }
    let mut body = [0u8; 256];
    let mut bn = 0usize;
    append(&mut body, &mut bn, b"{\"model\":{\"id\":\"");
    json_escape(&mut body, &mut bn, model.as_bytes());
    append(&mut body, &mut bn, b"\",\"providerID\":\"");
    json_escape(&mut body, &mut bn, provider.as_bytes());
    append(&mut body, &mut bn, b"\"}}");
    let mut cl = [0u8; 24]; let mut cln = 0usize;
    append(&mut cl, &mut cln, b"Content-Length: ");
    append_num(&mut cl, &mut cln, bn as u64);
    append(&mut cl, &mut cln, b"\r\n\r\n");
    append(out, &mut n, &cl[..cln]);
    append(out, &mut n, &body[..bn]);
    n
}

/// Parse GET /api/model -> entries (name, providerID, modelID).
pub fn parse_models(data: &[u8], out: &mut [ModelEntry]) -> usize {
    let Some(dp) = find_from(data, b"\"data\":[", 0) else { return 0 };
    let arr = dp + 7;
    let end = match_bracket(data, arr);
    let mut count = 0usize;
    let mut i = arr + 1;
    while i < end && count < out.len() {
        while i < end && data[i] != b'{' { i += 1; }
        if i >= end { break; }
        let oe = match_bracket(data, i);
        let obj = &data[i..oe.min(end)];
        i = oe;
        let model = find_str(obj, b"modelID").or_else(|| find_str(obj, b"id"));
        let provider = find_str(obj, b"providerID");
        let name = find_str(obj, b"name").or(model);
        let (Some(m), Some(p), Some(nm)) = (model, provider, name) else { continue };
        let mut e = EMPTY_MODEL;
        e.model_len = m.len().min(40); e.model[..e.model_len].copy_from_slice(&m[..e.model_len]);
        e.provider_len = p.len().min(24); e.provider[..e.provider_len].copy_from_slice(&p[..e.provider_len]);
        e.name_len = nm.len().min(40); e.name[..e.name_len].copy_from_slice(&nm[..e.name_len]);
        out[count] = e;
        count += 1;
    }
    count
}

impl Oc {
    /// Fetch the model list (requires a running server).
    pub fn fetch_models(&mut self) -> usize {
        if self.server_slot.is_none() || !self.ready {
            self.set_status("Start the server first");
            return 0;
        }
        let pw = self.pw_str();
        let mut req = [0u8; 256];
        let rl = build_models_req(pw, &mut req);
        let mut body = [0u8; 24576];
        let bl = http_transport(&req[..rl], &mut body);
        if bl == 0 { self.set_status("Model list failed"); return 0; }
        self.model_count = parse_models(&body[..bl], &mut self.models);
        if self.model_count == 0 {
            self.set_status("No models - add a provider/key");
        } else {
            self.set_status("Pick a model");
        }
        self.model_count
    }

    /// Set the current session's model (creates a session if needed).
    pub fn select_model(&mut self, idx: usize) {
        if idx >= self.model_count { return; }
        self.sel_model = idx;
        self.has_model = true;
        if !self.ensure_session() { return; }
        let e = self.models[idx];
        let pw = self.pw_str();
        let sid = core::str::from_utf8(&self.session[..self.session_len]).unwrap_or("");
        let mut req = [0u8; 512];
        let rl = build_set_model_req(pw, sid, e.provider_str(), e.model_str(), &mut req);
        let mut body = [0u8; 1024];
        let l = http_transport(&req[..rl], &mut body);
        if l > 0 {
            self.set_status("Model set");
        } else {
            self.set_status("Set model failed");
        }
    }
}

#[cfg(not(target_os = "none"))]
pub fn test_models(password: &str, out: &mut [u8]) -> u64 {
    let mut req = [0u8; 256];
    let rl = build_models_req(password, &mut req);
    let mut body = [0u8; 65536];
    let bl = http_transport(&req[..rl], &mut body);
    let mut models = [EMPTY_MODEL; 32];
    let n = parse_models(&body[..bl], &mut models);
    let mut o = 0usize;
    append(out, &mut o, b"[models=");
    append_num(out, &mut o, n as u64);
    append(out, &mut o, b"]\n");
    for i in 0..n.min(10) {
        let e = models[i];
        append(out, &mut o, e.provider_str().as_bytes());
        append(out, &mut o, b"/");
        append(out, &mut o, e.model_str().as_bytes());
        append(out, &mut o, b"  ");
        append(out, &mut o, e.name_str().as_bytes());
        append(out, &mut o, b"\n");
    }
    o as u64
}
