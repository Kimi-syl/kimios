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
    close, create, mkdir, net_resolve, net_status, net_tcp_close, net_tcp_connect, net_tcp_recv,
    net_tcp_send, open, print, print_bytes, print_u64, read_file, spawn, stat, wait, write_file,
};

const DEFAULT_BASE: &str = "http://10.0.2.2:8080";
const REPO_PATH: &str = "/disk/repo.json";
const BIN_DIR: &str = "/disk/bin";

static mut REPO: [u8; 8192] = [0; 8192];

fn repo_buf() -> &'static mut [u8] {
    unsafe { &mut *core::ptr::addr_of_mut!(REPO) }
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
    if kb.len() + 4 > pat.len() {
        return None;
    }
    pat[0] = b'"';
    pat[1..1 + kb.len()].copy_from_slice(kb);
    let mut n = 1 + kb.len();
    pat[n] = b'"';
    n += 1;
    pat[n] = b':';
    n += 1;
    pat[n] = b'"';
    n += 1;
    let pos = find(data, &pat[..n])?;
    let start = pos + n;
    let end = start + data[start..].iter().position(|&b| b == b'"')?;
    Some(&data[start..end])
}

fn json_true(data: &[u8], key: &str) -> bool {
    let kb = key.as_bytes();
    let mut pat = [0u8; 48];
    if kb.len() + 6 > pat.len() {
        return false;
    }
    pat[0] = b'"';
    pat[1..1 + kb.len()].copy_from_slice(kb);
    let mut n = 1 + kb.len();
    pat[n] = b'"';
    n += 1;
    pat[n] = b':';
    n += 1;
    pat[n..n + 4].copy_from_slice(b"true");
    n += 4;
    find(data, &pat[..n]).is_some()
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
    let mut host_buf = [0u8; 64];
    let Some((host, port, prefix)) = parse_url(base, &mut host_buf) else {
        print("kpkg: bad URL\n");
        return;
    };
    let mut path = [0u8; 160];
    let mut plen = 0usize;
    for piece in [prefix.as_bytes(), b"/repo.json"] {
        if plen + piece.len() > path.len() {
            break;
        }
        path[plen..plen + piece.len()].copy_from_slice(piece);
        plen += piece.len();
    }
    let path_str = core::str::from_utf8(&path[..plen]).unwrap_or("/repo.json");

    print("kpkg: fetching ");
    print(base);
    print("/repo.json ...\n");
    let mut got = 0usize;
    let ok = http_body(host, port, path_str, |chunk| {
        let n = chunk.len().min(repo_buf().len() - got);
        repo_buf()[got..got + n].copy_from_slice(&chunk[..n]);
        got += n;
    });
    if ok.is_none() || got == 0 {
        print("kpkg: fetch failed (is the repo server reachable over HTTP?)\n");
        return;
    }
    // Cache to /disk/repo.json.
    if let Some(handle) = create(REPO_PATH) {
        write_file(handle, 0, &repo_buf()[..got]);
        close(handle);
    }
    let mut count = 0usize;
    foreach_package(&repo_buf()[..got], |_| count += 1);
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
    let mut host_buf = [0u8; 64];
    let Some((host, port, prefix)) = parse_url(base, &mut host_buf) else { return false };
    let mut path = [0u8; 160];
    let mut plen = 0usize;
    for piece in [prefix.as_bytes(), b"/repo.json"] {
        if plen + piece.len() > path.len() { break; }
        path[plen..plen + piece.len()].copy_from_slice(piece);
        plen += piece.len();
    }
    let path_str = core::str::from_utf8(&path[..plen]).unwrap_or("/repo.json");
    let mut got = 0usize;
    let ok = http_body(host, port, path_str, |chunk| {
        let n = chunk.len().min(repo_buf().len() - got);
        repo_buf()[got..got + n].copy_from_slice(&chunk[..n]);
        got += n;
    });
    if ok.is_none() || got == 0 { return false; }
    if let Some(handle) = create(REPO_PATH) {
        write_file(handle, 0, &repo_buf()[..got]);
        close(handle);
    }
    true
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

    let mut host_buf = [0u8; 64];
    let Some((host, port, prefix)) = parse_url(base_str, &mut host_buf) else { return false };
    let mut path = [0u8; 160];
    let mut plen = 0usize;
    for piece in [prefix.as_bytes(), b"/", &file[..file_len]] {
        if plen + piece.len() > path.len() { break; }
        path[plen..plen + piece.len()].copy_from_slice(piece);
        plen += piece.len();
    }
    let path_str = core::str::from_utf8(&path[..plen]).unwrap_or("/");

    let Some(handle) = create(dest) else { return false };
    let mut offset = 0u64;
    let ok = http_body(host, port, path_str, |chunk| {
        let w = write_file(handle, offset as usize, chunk);
        offset += w as u64;
    });
    close(handle);
    ok.is_some() && offset > 0
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
