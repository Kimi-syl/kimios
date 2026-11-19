//! kpkg: the Kimi package manager (apt-like).
//!
//! `kpkg update [url]` fetches a repo.json over HTTP (smoltcp) and caches it to
//! /disk/repo.json. `kpkg list` shows available packages. `kpkg install <name>`
//! downloads a static Linux binary to /disk/bin/<name>. `kpkg run <name>`
//! executes it through the Linux ABI (Phase 47).
//!
//! Scope honesty: packages are statically-linked console binaries that the
//! Linux ABI can actually execute. Heavyweight runtimes (npm/Node, pip/CPython)
//! and TLS network tools (curl) need a far larger syscall surface (threads,
//! futex, epoll, dlopen, sockets, TLS) and are marked not-runnable in the repo.

use crate::{
    close, copy_file, create, listdir, mkdir, net_resolve, net_status, net_tcp_close,
    net_tcp_connect, net_tcp_recv, net_tcp_send, open, print, print_bytes, print_u64, read_file,
    spawn, stat, wait, write_file,
};

const DEFAULT_BASE: &str = "https://raw.githubusercontent.com/Kimi-syl/kimios/main/market";
const REPO_PATH: &str = "/disk/repo.json";
const BIN_DIR: &str = "/disk/bin";

static mut REPO: [u8; 8192] = [0; 8192];

fn repo_buf() -> &'static mut [u8] {
    unsafe { &mut *core::ptr::addr_of_mut!(REPO) }
}

/// Join base + suffix into `buf` as a full URL string.
fn join_url<'a>(base: &'a str, suffix: &str, buf: &'a mut [u8; 256]) -> &'a str {
    let mut n = 0usize;
    for piece in [base.as_bytes(), suffix.as_bytes()] {
        let take = piece.len().min(buf.len() - n);
        buf[n..n + take].copy_from_slice(&piece[..take]);
        n += take;
    }
    core::str::from_utf8(&buf[..n]).unwrap_or(base)
}

/// Fetch `url` into the `/disk/...` file `dest`. Prefers the host high-level
/// fetch (HTTP/HTTPS via SYS_FETCH_URL); falls back to raw-socket HTTP streaming
/// on bare metal. Returns true on success.
fn fetch_to(url: &str, dest: &str) -> bool {
    if let Some(n) = crate::fetch_url(url, dest) {
        return n > 0;
    }
    let mut hb = [0u8; 64];
    let Some((host, port, path)) = parse_url(url, &mut hb) else { return false };
    let Some(handle) = create(dest) else { return false };
    let mut offset = 0u64;
    let ok = http_body(host, port, path, |chunk| {
        let w = write_file(handle, offset as usize, chunk);
        offset += w as u64;
    });
    close(handle);
    ok.is_some() && offset > 0
}

// -- tiny JSON helpers (compact repo.json: {"key":"value",...}) -------------

fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || data.len() < needle.len() {
        return None;
    }
    (0..=data.len() - needle.len()).find(|&i| &data[i..i + needle.len()] == needle)
}

fn json_str<'a>(data: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let kb = key.as_bytes();
    let mut pat = [0u8; 48];
    if kb.len() + 3 > pat.len() {
        return None;
    }
    pat[0] = b'"';
    pat[1..1 + kb.len()].copy_from_slice(kb);
    let mut n = 1 + kb.len();
    pat[n] = b'"';
    n += 1;
    pat[n] = b':';
    n += 1;
    let pos = find(data, &pat[..n])?;
    let mut start = pos + n;
    while start < data.len() && matches!(data[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    if start >= data.len() || data[start] != b'"' {
        return None;
    }
    start += 1;
    let end = start + data[start..].iter().position(|&b| b == b'"')?;
    Some(&data[start..end])
}

fn json_true(data: &[u8], key: &str) -> bool {
    let kb = key.as_bytes();
    let mut pat = [0u8; 48];
    if kb.len() + 3 > pat.len() {
        return false;
    }
    pat[0] = b'"';
    pat[1..1 + kb.len()].copy_from_slice(kb);
    let mut n = 1 + kb.len();
    pat[n] = b'"';
    n += 1;
    pat[n] = b':';
    n += 1;
    let Some(pos) = find(data, &pat[..n]) else { return false };
    let mut start = pos + n;
    while start < data.len() && matches!(data[start], b' ' | b'\t' | b'\n' | b'\r') {
        start += 1;
    }
    data[start..].starts_with(b"true")
}

/// Invoke `f` on each `{...}` package object (delimited by a "name" field).
fn foreach_package(data: &[u8], mut f: impl FnMut(&[u8])) {
    let mut i = 0usize;
    while let Some(pos) = find(&data[i..], b"\"name\"") {
        let abs = i + pos;
        let start = data[..abs].iter().rposition(|&b| b == b'{').unwrap_or(0);
        let end = data[abs..]
            .iter()
            .position(|&b| b == b'}')
            .map(|p| abs + p + 1)
            .unwrap_or(data.len());
        f(&data[start..end]);
        i = end;
    }
}

// -- HTTP -------------------------------------------------------------------

fn parse_u16(s: &[u8]) -> u16 {
    let mut n: u16 = 0;
    for &b in s {
        if b < b'0' || b > b'9' {
            break;
        }
        n = n.saturating_mul(10).saturating_add((b - b'0') as u16);
    }
    if n == 0 {
        80
    } else {
        n
    }
}

/// Split "http://host:port/prefix" into (host, port, prefix).
fn parse_url<'a>(url: &'a str, host_out: &'a mut [u8]) -> Option<(&'a str, u16, &'a str)> {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    let (authority, prefix) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rfind(':') {
        Some(i) => (&authority[..i], parse_u16(authority[i + 1..].as_bytes())),
        None => (authority, 80),
    };
    let n = host.len().min(host_out.len());
    host_out[..n].copy_from_slice(host.as_bytes());
    let host_str = core::str::from_utf8(&host_out[..n]).ok()?;
    Some((host_str, port, prefix))
}

/// Parse a dotted-quad IPv4 literal into the big-endian u32 the TCP API uses.
fn parse_ipv4(host: &str) -> Option<u32> {
    let mut octets = [0u32; 4];
    let mut i = 0;
    for part in host.split('.') {
        if i >= 4 || part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut v = 0u32;
        for b in part.bytes() {
            if b < b'0' || b > b'9' {
                return None;
            }
            v = v * 10 + (b - b'0') as u32;
        }
        if v > 255 {
            return None;
        }
        octets[i] = v;
        i += 1;
    }
    if i != 4 {
        return None;
    }
    Some((octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) | octets[3])
}

/// HTTP GET, streaming the response body (headers stripped) into `sink`.
fn http_body(host: &str, port: u16, path: &str, mut sink: impl FnMut(&[u8])) -> Option<u64> {
    net_status(5000);
    let ip = parse_ipv4(host).or_else(|| net_resolve(host))?;
    let slot = net_tcp_connect(ip, port, 5000)?;

    let mut request = [0u8; 320];
    let mut used = 0usize;
    for piece in [
        &b"GET "[..],
        path.as_bytes(),
        b" HTTP/1.0\r\nHost: ",
        host.as_bytes(),
        b"\r\nConnection: close\r\n\r\n",
    ] {
        if used + piece.len() > request.len() {
            break;
        }
        request[used..used + piece.len()].copy_from_slice(piece);
        used += piece.len();
    }
    net_tcp_send(slot, &request[..used]);

    let mut buf = [0u8; 2048];
    let mut pending = [0u8; 2048];
    let mut pending_len = 0usize;
    let mut header_done = false;
    let mut total = 0u64;
    loop {
        let n = net_tcp_recv(slot, &mut buf, 5000);
        if n == 0 {
            break;
        }
        if !header_done {
            let copy = n.min(pending.len() - pending_len);
            pending[pending_len..pending_len + copy].copy_from_slice(&buf[..copy]);
            pending_len += copy;
            if let Some(pos) = find(&pending[..pending_len], b"\r\n\r\n") {
                header_done = true;
                let body_start = pos + 4;
                if body_start < pending_len {
                    sink(&pending[body_start..pending_len]);
                    total += (pending_len - body_start) as u64;
                }
                pending_len = 0;
            }
        } else {
            sink(&buf[..n]);
            total += n as u64;
        }
    }
    net_tcp_close(slot);
    if header_done {
        Some(total)
    } else {
        None
    }
}

fn read_repo() -> Option<usize> {
    let (_, size) = stat(REPO_PATH)?;
    let handle = open(REPO_PATH)?;
    let want = (size as usize).min(repo_buf().len());
    let n = read_file(handle, 0, &mut repo_buf()[..want]);
    close(handle);
    Some(n)
}

// -- commands ---------------------------------------------------------------

pub fn update(base_url: &str) {
    let base = if base_url.is_empty() { DEFAULT_BASE } else { base_url };
    let mut ub = [0u8; 256];
    let url = join_url(base, "/repo.json", &mut ub);
    print("kpkg: fetching ");
    print(url);
    print(" ...\n");
    if !fetch_to(url, REPO_PATH) {
        print("kpkg: fetch failed (repo reachable? HTTPS needs the libOS host)\n");
        return;
    }
    let mut count = 0usize;
    if let Some(n) = read_repo() {
        foreach_package(&repo_buf()[..n], |_| count += 1);
    }
    print("kpkg: repo updated, ");
    print_u64(count as u64);
    print(" packages available\n");
}

pub fn list() {
    let Some(n) = read_repo() else {
        print("kpkg: no repo cached — run `kpkg update` first\n");
        return;
    };
    let data = &repo_buf()[..n];
    print("Available packages (run = executable on KimiOS):\n");
    let mut count = 0usize;
    foreach_package(data, |obj| {
        let name = json_str(obj, "name").unwrap_or(b"?");
        let desc = json_str(obj, "desc").unwrap_or(b"");
        let run = json_true(obj, "run");
        print("  ");
        print_bytes(name);
        print("  —  ");
        print_bytes(desc);
        print(if run { "  [run]\n" } else { "  [needs full Linux]\n" });
        count += 1;
    });
    if count == 0 {
        print("  (empty repo)\n");
    }
}

pub fn install(name: &str) {
    let Some(n) = read_repo() else {
        print("kpkg: no repo cached — run `kpkg update` first\n");
        return;
    };
    let data = &repo_buf()[..n];
    let Some(base) = json_str(data, "base") else {
        print("kpkg: repo has no base URL\n");
        return;
    };
    let base_str = core::str::from_utf8(base).unwrap_or(DEFAULT_BASE);

    let mut found = false;
    let mut file = [0u8; 64];
    let mut file_len = 0usize;
    let mut runnable = false;
    let mut note = [0u8; 160];
    let mut note_len = 0usize;
    foreach_package(data, |obj| {
        if let Some(nm) = json_str(obj, "name") {
            if nm == name.as_bytes() {
                found = true;
                runnable = json_true(obj, "run");
                if let Some(f) = json_str(obj, "file") {
                    file_len = f.len().min(file.len());
                    file[..file_len].copy_from_slice(&f[..file_len]);
                }
                if let Some(nt) = json_str(obj, "note") {
                    note_len = nt.len().min(note.len());
                    note[..note_len].copy_from_slice(&nt[..note_len]);
                }
            }
        }
    });

    if !found {
        print("kpkg: package not found: ");
        print(name);
        print("\n");
        return;
    }
    if !runnable {
        print("kpkg: ");
        print(name);
        print(" is not runnable on KimiOS yet.\n  ");
        if note_len > 0 {
            print_bytes(&note[..note_len]);
            print("\n  ");
        }
        print("It needs a fuller Linux ABI (threads/futex/epoll/dlopen/sockets/TLS).\n");
        return;
    }
    if file_len == 0 {
        print("kpkg: package has no download file\n");
        return;
    }

    let mut host_buf = [0u8; 64];
    let Some((host, port, prefix)) = parse_url(base_str, &mut host_buf) else {
        print("kpkg: bad base URL\n");
        return;
    };
    let mut path = [0u8; 160];
    let mut plen = 0usize;
    for piece in [prefix.as_bytes(), b"/", &file[..file_len]] {
        if plen + piece.len() > path.len() {
            break;
        }
        path[plen..plen + piece.len()].copy_from_slice(piece);
        plen += piece.len();
    }
    let path_str = core::str::from_utf8(&path[..plen]).unwrap_or("/");

    let _ = mkdir(BIN_DIR);
    let mut dest = [0u8; 96];
    let mut dlen = 0usize;
    for piece in [b"/disk/bin/", name.as_bytes()] {
        if dlen + piece.len() > dest.len() {
            break;
        }
        dest[dlen..dlen + piece.len()].copy_from_slice(piece);
        dlen += piece.len();
    }
    let Ok(dest_str) = core::str::from_utf8(&dest[..dlen]) else {
        print("kpkg: bad package name\n");
        return;
    };

    print("kpkg: downloading ");
    print(name);
    print(" ...\n");
    let Some(handle) = create(dest_str) else {
        print("kpkg: cannot create ");
        print(dest_str);
        print("\n");
        return;
    };
    let mut offset = 0u64;
    let ok = http_body(host, port, path_str, |chunk| {
        let w = write_file(handle, offset as usize, chunk);
        offset += w as u64;
    });
    close(handle);
    match ok {
        Some(_) if offset > 0 => {
            print("kpkg: installed ");
            print(name);
            print(" (");
            print_u64(offset);
            print(" bytes) -> ");
            print(dest_str);
            print("\n  run it with: kpkg run ");
            print(name);
            print("\n");
        }
        _ => {
            print("kpkg: download failed\n");
        }
    }
}

pub fn run(name: &str, args: &[&str]) {
    let mut path = [0u8; 96];
    let mut plen = 0usize;
    for piece in [b"/disk/bin/", name.as_bytes()] {
        if plen + piece.len() > path.len() {
            break;
        }
        path[plen..plen + piece.len()].copy_from_slice(piece);
        plen += piece.len();
    }
    let Ok(path_str) = core::str::from_utf8(&path[..plen]) else {
        print("kpkg: bad name\n");
        return;
    };
    if stat(path_str).is_none() {
        print("kpkg: not installed — run `kpkg install ");
        print(name);
        print("` first\n");
        return;
    }
    let spawned = if args.is_empty() {
        spawn(path_str)
    } else {
        crate::spawn_args(path_str, args)
    };
    match spawned {
        Some(pid) => {
            let _ = wait(pid);
        }
        None => {
            print("kpkg: spawn failed\n");
        }
    }
}

pub fn help() {
    print("kpkg — Kimi package manager (static Linux binaries)\n");
    print("  kpkg update [url]   fetch repo.json (default ");
    print(DEFAULT_BASE);
    print(")\n");
    print("  kpkg list           show available packages\n");
    print("  kpkg install <name> download a package to /disk/bin\n");
    print("  kpkg run <name>     execute an installed package\n");
    print("Packages run via the Linux ABI (Phase 47). npm/pip/curl need a\n");
    print("fuller Linux runtime and are marked not-runnable.\n");
}
// -- programmatic API (used by the GUI Marketplace) -------------------------

/// A repo package entry, parsed into fixed buffers (no allocator).
#[derive(Clone, Copy)]
pub struct Package {
    pub name: [u8; 32],
    pub name_len: usize,
    pub desc: [u8; 96],
    pub desc_len: usize,
    pub kind: [u8; 16],
    pub kind_len: usize,
    pub file: [u8; 64],
    pub file_len: usize,
    pub run: bool,
}

impl Package {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
    pub fn desc_str(&self) -> &str {
        core::str::from_utf8(&self.desc[..self.desc_len]).unwrap_or("")
    }
    /// "app" (default), "theme" or "font".
    pub fn kind_str(&self) -> &str {
        if self.kind_len == 0 {
            "app"
        } else {
            core::str::from_utf8(&self.kind[..self.kind_len]).unwrap_or("app")
        }
    }
}

/// True if a repo.json is cached on disk.
pub fn repo_cached() -> bool {
    read_repo().is_some()
}

/// Invoke `f` for every package in the cached repo.
pub fn each_package(mut f: impl FnMut(&Package)) {
    let Some(n) = read_repo() else { return };
    let data = &repo_buf()[..n];
    foreach_package(data, |obj| {
        let mut p = Package {
            name: [0; 32], name_len: 0,
            desc: [0; 96], desc_len: 0,
            kind: [0; 16], kind_len: 0,
            file: [0; 64], file_len: 0,
            run: false,
        };
        if let Some(v) = json_str(obj, "name") {
            p.name_len = v.len().min(32);
            p.name[..p.name_len].copy_from_slice(&v[..p.name_len]);
        }
        if let Some(v) = json_str(obj, "desc") {
            p.desc_len = v.len().min(96);
            p.desc[..p.desc_len].copy_from_slice(&v[..p.desc_len]);
        }
        if let Some(v) = json_str(obj, "kind") {
            p.kind_len = v.len().min(16);
            p.kind[..p.kind_len].copy_from_slice(&v[..p.kind_len]);
        }
        if let Some(v) = json_str(obj, "file") {
            p.file_len = v.len().min(64);
            p.file[..p.file_len].copy_from_slice(&v[..p.file_len]);
        }
        p.run = json_true(obj, "run");
        f(&p);
    });
}

/// Fetch + cache repo.json; returns true on success (quiet, for the GUI).
pub fn update_quiet(base_url: &str) -> bool {
    let base = if base_url.is_empty() { DEFAULT_BASE } else { base_url };
    let mut ub = [0u8; 256];
    let url = join_url(base, "/repo.json", &mut ub);
    fetch_to(url, REPO_PATH)
}

/// Download a package's asset file to an arbitrary destination path. Unlike
/// `install`, this does not require the package to be a runnable binary — it is
/// used for theme/font assets as well as apps. Returns true on success.
pub fn install_to(name: &str, dest: &str) -> bool {
    let Some(n) = read_repo() else { return false };
    let data = &repo_buf()[..n];
    let Some(base) = json_str(data, "base") else { return false };
    let base_str = core::str::from_utf8(base).unwrap_or(DEFAULT_BASE);

    let mut found = false;
    let mut file = [0u8; 64];
    let mut file_len = 0usize;
    foreach_package(data, |obj| {
        if let Some(nm) = json_str(obj, "name") {
            if nm == name.as_bytes() {
                found = true;
                if let Some(f) = json_str(obj, "file") {
                    file_len = f.len().min(file.len());
                    file[..file_len].copy_from_slice(&f[..file_len]);
                }
            }
        }
    });
    if !found || file_len == 0 { return false; }

    let mut sb = [0u8; 96];
    sb[0] = b'/';
    let fl = file_len.min(sb.len() - 1);
    sb[1..1 + fl].copy_from_slice(&file[..fl]);
    let suffix = core::str::from_utf8(&sb[..1 + fl]).unwrap_or("/");
    let mut ub = [0u8; 256];
    let url = join_url(base_str, suffix, &mut ub);
    fetch_to(url, dest)
}

// -- helpers exposed for the Alpine apk backend -----------------------------

/// Public wrapper around the URL splitter (host buffer borrowed from caller).
pub fn parse_url_pub<'a>(url: &'a str, host_out: &'a mut [u8]) -> Option<(&'a str, u16, &'a str)> {
    parse_url(url, host_out)
}

/// Resolve a host to the big-endian IPv4 u32 the TCP API uses (literal or DNS).
pub fn resolve_host(host: &str) -> Option<u32> {
    parse_ipv4(host).or_else(|| net_resolve(host))
}

// -- local import (.kpkg / .kfont / .ktheme) ---------------------------------

/// Outcome of validating + installing a local package file. `kind` uses the
/// Marketplace encoding: 0 theme, 1 font, 2 app, 3 plugin.
pub struct LocalImport {
    pub ok: bool,
    pub kind: u8,
    pub name: [u8; 48],
    pub name_len: usize,
    pub dest: [u8; 96],
    pub dest_len: usize,
    pub msg: [u8; 64],
    pub msg_len: usize,
}

impl LocalImport {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
    pub fn dest_str(&self) -> &str {
        core::str::from_utf8(&self.dest[..self.dest_len]).unwrap_or("")
    }
    pub fn msg_str(&self) -> &str {
        core::str::from_utf8(&self.msg[..self.msg_len]).unwrap_or("")
    }

    fn fail(msg: &str) -> Self {
        let mut r = Self {
            ok: false, kind: 255,
            name: [0; 48], name_len: 0,
            dest: [0; 96], dest_len: 0,
            msg: [0; 64], msg_len: 0,
        };
        r.set_msg(msg);
        r
    }

    fn set_msg(&mut self, msg: &str) {
        let b = msg.as_bytes();
        self.msg_len = b.len().min(64);
        self.msg[..self.msg_len].copy_from_slice(&b[..self.msg_len]);
    }

    fn set_name(&mut self, name: &[u8]) {
        self.name_len = name.len().min(48);
        self.name[..self.name_len].copy_from_slice(&name[..self.name_len]);
    }

    fn set_dest(&mut self, dir: &str, name: &[u8], ext: &str) -> bool {
        let mut n = 0usize;
        for piece in [dir.as_bytes(), name, ext.as_bytes()] {
            if n + piece.len() > self.dest.len() {
                self.set_msg("Name too long");
                return false;
            }
            self.dest[n..n + piece.len()].copy_from_slice(piece);
            n += piece.len();
        }
        self.dest_len = n;
        true
    }
}

fn ends_ci(path: &str, suffix: &str) -> bool {
    let p = path.as_bytes();
    let s = suffix.as_bytes();
    p.len() >= s.len()
        && p[p.len() - s.len()..]
            .iter()
            .zip(s)
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
}

/// Basename of `path` with the final extension stripped.
fn stem(path: &str) -> &[u8] {
    let p = path.as_bytes();
    let base = match p.iter().rposition(|&b| b == b'/') {
        Some(i) => &p[i + 1..],
        None => p,
    };
    match base.iter().rposition(|&b| b == b'.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    }
}

fn dirname(path: &str) -> &str {
    let p = path.as_bytes();
    match p.iter().rposition(|&b| b == b'/') {
        Some(i) if i > 0 => core::str::from_utf8(&p[..i]).unwrap_or("/disk"),
        _ => "/disk",
    }
}

fn join3<'a>(a: &str, b: &[u8], c: &str, buf: &'a mut [u8; 160]) -> Option<&'a str> {
    let mut n = 0usize;
    for piece in [a.as_bytes(), b, c.as_bytes()] {
        if n + piece.len() > buf.len() {
            return None;
        }
        buf[n..n + piece.len()].copy_from_slice(piece);
        n += piece.len();
    }
    core::str::from_utf8(&buf[..n]).ok()
}

/// Count 6-digit hex colour tokens (a `.ktheme` needs 11 palette entries).
fn count_hex6(data: &[u8]) -> usize {
    let mut count = 0usize;
    let mut run = 0usize;
    let mut prev_hex = false;
    for &b in data {
        let hex = matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F');
        if hex {
            run += 1;
        } else {
            if prev_hex && run == 6 {
                count += 1;
            }
            run = 0;
        }
        prev_hex = hex;
    }
    if prev_hex && run == 6 {
        count += 1;
    }
    count
}

/// Validate a local `.kpkg` / `.kfont` / `.ktheme` / `.kplugin` and install it:
/// fonts -> /disk/fonts/<name>.kfont, themes -> /disk/themes/<name>.theme,
/// apps -> /disk/bin/<name>, plugins -> /disk/plugins/<id>. A `.kpkg` is a
/// repo-style manifest ({"name","kind","file"}) whose asset is a sibling path
/// or an HTTP(S) URL. A `.kplugin` is a directory holding plugin.json, or a
/// single manifest file staged next to its entry asset.
pub fn import_local(path: &str) -> LocalImport {
    if is_plugin_dir(path) {
        return import_plugin(path);
    }
    let Some((_, size)) = stat(path) else {
        return LocalImport::fail("File not found");
    };
    if size < 8 {
        return LocalImport::fail("File too small");
    }
    let Some(h) = open(path) else {
        return LocalImport::fail("Cannot open file");
    };
    let mut head = [0u8; 1024];
    let want = (size as usize).min(head.len());
    let n = read_file(h, 0, &mut head[..want]);
    close(h);
    if n < 8 {
        return LocalImport::fail("Cannot read file");
    }
    let data = &head[..n];

    if ends_ci(path, ".kplugin") {
        return import_plugin_file(path, data);
    }

    if &data[..4] == b"PK\x03\x04" {
        return LocalImport::fail("Zip archives not supported yet");
    }

    let font_magic = &data[..8] == b"KFONT001" || &data[..8] == b"KFONT002";
    let mut r = LocalImport {
        ok: false, kind: 2,
        name: [0; 48], name_len: 0,
        dest: [0; 96], dest_len: 0,
        msg: [0; 64], msg_len: 0,
    };

    if font_magic || ends_ci(path, ".kfont") {
        if !font_magic {
            return LocalImport::fail("Bad magic: expected KFONT001/002");
        }
        if size < 32 {
            return LocalImport::fail("Truncated kfont header");
        }
        let em = u16::from_le_bytes([data[8], data[9]]);
        if em == 0 {
            return LocalImport::fail("Invalid kfont (em=0)");
        }
        r.kind = 1;
        let nm = stem(path);
        r.set_name(nm);
        if !r.set_dest("/disk/fonts/", nm, ".kfont") {
            return r;
        }
        let _ = mkdir("/disk/fonts");
        if !copy_file(path, r.dest_str()) {
            r.set_msg("Copy to /disk/fonts failed");
            return r;
        }
        r.ok = true;
        r.set_msg("Font installed");
        return r;
    }

    if ends_ci(path, ".ktheme") {
        if count_hex6(data) < 11 {
            return LocalImport::fail("Invalid theme: need 11 RRGGBB colours");
        }
        r.kind = 0;
        let nm = stem(path);
        r.set_name(nm);
        if !r.set_dest("/disk/themes/", nm, ".theme") {
            return r;
        }
        let _ = mkdir("/disk/themes");
        if !copy_file(path, r.dest_str()) {
            r.set_msg("Copy to /disk/themes failed");
            return r;
        }
        r.ok = true;
        r.set_msg("Theme installed");
        return r;
    }

    if ends_ci(path, ".kpkg") || data[0] == b'{' {
        let Some(name) = json_str(data, "name") else {
            return LocalImport::fail("Manifest missing \"name\"");
        };
        if name.is_empty() || name.contains(&b'/') || name.len() > 40 {
            return LocalImport::fail("Bad package name");
        }
        let kind = match json_str(data, "kind") {
            Some(k) if k == b"theme" => 0u8,
            Some(k) if k == b"font" => 1u8,
            _ => 2u8,
        };
        let asset = json_str(data, "file").or_else(|| json_str(data, "url"));
        let Some(asset) = asset else {
            return LocalImport::fail("Manifest missing \"file\"");
        };
        r.kind = kind;
        r.set_name(name);
        let (dir, ext) = match kind {
            0 => ("/disk/themes/", ".theme"),
            1 => ("/disk/fonts/", ".kfont"),
            _ => ("/disk/bin/", ""),
        };
        if !r.set_dest(dir, name, ext) {
            return r;
        }
        let _ = mkdir(dir);
        let asset_str = core::str::from_utf8(asset).unwrap_or("");
        let installed = if asset_str.starts_with("http://") || asset_str.starts_with("https://") {
            fetch_to(asset_str, r.dest_str())
        } else {
            let mut src = [0u8; 160];
            let mut src_len = 0usize;
            if !asset_str.starts_with("/disk") {
                if let Some(p) = join3(dirname(path), asset, "", &mut src) {
                    if stat(p).is_some() {
                        src_len = p.len();
                    }
                }
                if src_len == 0 {
                    if let Some(p) = join3("/disk/import/", asset, "", &mut src) {
                        src_len = p.len();
                    }
                }
            }
            let candidate = if asset_str.starts_with("/disk") {
                Some(asset_str)
            } else if src_len > 0 {
                core::str::from_utf8(&src[..src_len]).ok()
            } else {
                None
            };
            match candidate {
                Some(p) if stat(p).is_some() => copy_file(p, r.dest_str()),
                _ => {
                    r.set_msg("Asset file not found next to manifest");
                    return r;
                }
            }
        };
        if !installed {
            r.set_msg("Asset fetch/copy failed");
            return r;
        }
        r.ok = true;
        r.set_msg(match kind {
            0 => "Theme installed",
            1 => "Font installed",
            _ => "App installed to /disk/bin",
        });
        return r;
    }

    LocalImport::fail("Unknown format (.kpkg/.kfont/.ktheme/.kplugin)")
}

// -- plugin import (.kplugin) -----------------------------------------------

/// True if `path` is a directory holding a `plugin.json` manifest.
pub fn is_plugin_dir(path: &str) -> bool {
    let mut pb = [0u8; 160];
    let mut n = 0usize;
    for piece in [path.as_bytes(), b"/plugin.json"] {
        if n + piece.len() > pb.len() {
            return false;
        }
        pb[n..n + piece.len()].copy_from_slice(piece);
        n += piece.len();
    }
    let Ok(p) = core::str::from_utf8(&pb[..n]) else { return false };
    stat(p).is_some()
}

/// Validate a `.kplugin` source directory's manifest and install it through
/// the plugin manager. `kind` is 3 (plugin). The manifest must carry an `id`.
pub fn import_plugin(src_dir: &str) -> LocalImport {
    let mut r = LocalImport {
        ok: false, kind: 3,
        name: [0; 48], name_len: 0,
        dest: [0; 96], dest_len: 0,
        msg: [0; 64], msg_len: 0,
    };
    if !is_plugin_dir(src_dir) {
        r.set_msg("No plugin.json in directory");
        return r;
    }
    let mut pb = [0u8; 160];
    let mut n = 0usize;
    for piece in [src_dir.as_bytes(), b"/plugin.json"] {
        if n + piece.len() > pb.len() {
            r.set_msg("Path too long");
            return r;
        }
        pb[n..n + piece.len()].copy_from_slice(piece);
        n += piece.len();
    }
    let Ok(mpath) = core::str::from_utf8(&pb[..n]) else {
        r.set_msg("Bad path");
        return r;
    };
    let Some(h) = open(mpath) else {
        r.set_msg("Cannot read plugin.json");
        return r;
    };
    let mut buf = [0u8; 2048];
    let got = read_file(h, 0, &mut buf);
    close(h);
    let data = &buf[..got];
    let Some(id) = json_str(data, "id") else {
        r.set_msg("Manifest missing \"id\"");
        return r;
    };
    if id.is_empty() || id.contains(&b'/') || id.len() > 40 {
        r.set_msg("Bad plugin id");
        return r;
    }
    r.set_name(id);
    if !r.set_dest("/disk/plugins/", id, "") {
        return r;
    }
    if !crate::kplugin::install(src_dir) {
        r.set_msg("Plugin install failed");
        return r;
    }
    r.ok = true;
    r.set_msg("Plugin installed");
    r
}

/// Stage a single-file `.kplugin` manifest (plus its sibling entry asset) into
/// a directory and install it. The manifest is JSON with an `id` and `entry`.
fn import_plugin_file(path: &str, data: &[u8]) -> LocalImport {
    let mut r = LocalImport {
        ok: false, kind: 3,
        name: [0; 48], name_len: 0,
        dest: [0; 96], dest_len: 0,
        msg: [0; 64], msg_len: 0,
    };
    if data.first() != Some(&b'{') {
        r.set_msg("Plugin manifest must be JSON");
        return r;
    }
    let Some(id) = json_str(data, "id") else {
        r.set_msg("Manifest missing \"id\"");
        return r;
    };
    if id.is_empty() || id.contains(&b'/') || id.len() > 40 {
        r.set_msg("Bad plugin id");
        return r;
    }
    let mut stage = [0u8; 96];
    let mut sn = 0usize;
    for piece in [b"/disk/import/.stage-", id] {
        if sn + piece.len() > stage.len() {
            r.set_msg("Plugin id too long");
            return r;
        }
        stage[sn..sn + piece.len()].copy_from_slice(piece);
        sn += piece.len();
    }
    let Ok(stage_dir) = core::str::from_utf8(&stage[..sn]) else {
        r.set_msg("Bad plugin id");
        return r;
    };
    let _ = mkdir("/disk/import");
    if !mkdir(stage_dir) {
        r.set_msg("Cannot stage plugin");
        return r;
    }
    let mut mp = [0u8; 128];
    let mn = {
        let mut n = 0usize;
        for piece in [stage_dir.as_bytes(), b"/plugin.json"] {
            if n + piece.len() > mp.len() {
                r.set_msg("Path too long");
                return r;
            }
            mp[n..n + piece.len()].copy_from_slice(piece);
            n += piece.len();
        }
        n
    };
    let Ok(manifest_dest) = core::str::from_utf8(&mp[..mn]) else {
        r.set_msg("Bad path");
        return r;
    };
    let Some(h) = create(manifest_dest) else {
        r.set_msg("Cannot write manifest");
        return r;
    };
    write_file(h, 0, data);
    close(h);
    if let Some(entry) = json_str(data, "entry") {
        if !entry.is_empty() && !entry.contains(&b'/') && entry.len() <= 48 {
            let mut src = [0u8; 160];
            if let Some(sp) = join3(dirname(path), entry, "", &mut src) {
                if stat(sp).is_some() {
                    let mut dst = [0u8; 160];
                    let mut dn = 0usize;
                    let mut ok = true;
                    for piece in [stage_dir.as_bytes(), b"/", entry] {
                        if dn + piece.len() > dst.len() {
                            ok = false;
                            break;
                        }
                        dst[dn..dn + piece.len()].copy_from_slice(piece);
                        dn += piece.len();
                    }
                    if ok {
                        if let Ok(dp) = core::str::from_utf8(&dst[..dn]) {
                            let _ = copy_file(sp, dp);
                        }
                    }
                }
            }
        }
    }
    import_plugin(stage_dir)
}

/// Scan `/disk/import` for installable plugin sources: directories containing
/// `plugin.json`, or single `.kplugin` manifest files. Invokes `f` with each
/// source path (directory paths keep no trailing slash). Returns the count.
pub fn scan_plugin_imports(mut f: impl FnMut(&str)) -> usize {
    let _ = mkdir("/disk/import");
    let mut buffer = [0u8; 1024];
    let Some(n) = listdir("/disk/import", &mut buffer) else { return 0 };
    let mut count = 0usize;
    for name in buffer[..n].split(|&b| b == 0) {
        if name.is_empty() || name.first() == Some(&b'.') {
            continue;
        }
        let is_dir = name.last() == Some(&b'/');
        let base = if is_dir { &name[..name.len() - 1] } else { name };
        if base.is_empty() || base.len() > 64 {
            continue;
        }
        let mut pb = [0u8; 96];
        let mut pn = 0usize;
        let mut too_long = false;
        for piece in [b"/disk/import/", base] {
            if pn + piece.len() > pb.len() {
                too_long = true;
                break;
            }
            pb[pn..pn + piece.len()].copy_from_slice(piece);
            pn += piece.len();
        }
        if too_long {
            continue;
        }
        let Ok(path) = core::str::from_utf8(&pb[..pn]) else { continue };
        let hit = if is_dir {
            is_plugin_dir(path)
        } else {
            ends_ci(path, ".kplugin")
        };
        if hit {
            f(path);
            count += 1;
        }
    }
    count
}
