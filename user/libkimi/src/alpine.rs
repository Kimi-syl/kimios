//! Alpine Linux `apk` package compatibility for kpkg.
//!
//! Understands Alpine repositories (APKINDEX.tar.gz) and packages (`.apk` =
//! concatenated gzip members: a control tar with `.PKGINFO`, then a data tar).
//! Resolves dependencies (including `so:` shared-library providers) and streams
//! packages into a sysroot at `/disk/alpine/root` — large packages (nodejs,
//! python3) are extracted with a streaming inflater so they need no big buffers.
//!
//! Scope honesty: this downloads and extracts real Alpine packages (musl-based,
//! dynamically linked). *Executing* them additionally needs a dynamic ELF loader
//! + musl + a fuller Linux ABI (threads/futex/epoll/sockets/dlopen) — the Desktop
//! Lite runtime milestone. See market/README.md and docs/android-feasibility.md.

use crate as k;
use crate::apk::{build_huffman, Huffman, DIST_BASE, DIST_EXTRA, LEN_BASE, LEN_EXTRA};

const DIR: &str = "/disk/alpine";
const INDEX: &str = "/disk/alpine/APKINDEX";
const REPO: &str = "/disk/alpine/repo.cfg";
const SYSROOT: &str = "/disk/alpine/root";
const TMP: &str = "/disk/alpine/pkg.apk";
const DEFAULT_ROOT: &str = "http://dl-cdn.alpinelinux.org/alpine/latest-stable";
const ARCH: &str = "aarch64";

// ---------------------------------------------------------------------------
// Streaming input
// ---------------------------------------------------------------------------

trait Reader {
    fn fill(&mut self, buf: &mut [u8]) -> usize;
}

struct FileReader {
    handle: u64,
    offset: usize,
}
impl Reader for FileReader {
    fn fill(&mut self, buf: &mut [u8]) -> usize {
        let n = k::read_file(self.handle, self.offset, buf);
        self.offset += n;
        n
    }
}

struct SliceReader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl Reader for SliceReader<'_> {
    fn fill(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        n
    }
}

/// Bit reader pulling from a chunked [`Reader`].
struct SBits<R: Reader> {
    r: R,
    buf: [u8; 4096],
    pos: usize,
    len: usize,
    bitbuf: u32,
    bitcnt: u32,
    eof: bool,
}
impl<R: Reader> SBits<R> {
    fn new(r: R) -> Self {
        SBits { r, buf: [0; 4096], pos: 0, len: 0, bitbuf: 0, bitcnt: 0, eof: false }
    }
    fn byte(&mut self) -> u32 {
        if self.pos >= self.len {
            self.len = self.r.fill(&mut self.buf);
            self.pos = 0;
            if self.len == 0 {
                self.eof = true;
                return 0;
            }
        }
        let b = self.buf[self.pos] as u32;
        self.pos += 1;
        b
    }
    fn bits(&mut self, need: u32) -> u32 {
        while self.bitcnt < need {
            let b = self.byte();
            self.bitbuf |= b << self.bitcnt;
            self.bitcnt += 8;
        }
        let v = self.bitbuf & ((1u32 << need) - 1);
        self.bitbuf >>= need;
        self.bitcnt -= need;
        v
    }
    fn align(&mut self) {
        self.bitbuf = 0;
        self.bitcnt = 0;
    }
}

fn decode<R: Reader>(br: &mut SBits<R>, h: &Huffman) -> u16 {
    let mut code = 0i32;
    let mut first = 0i32;
    let mut index = 0i32;
    for len in 1..16 {
        code |= br.bits(1) as i32;
        let count = h.counts[len] as i32;
        if code - first < count {
            return h.symbols[(index + (code - first)) as usize];
        }
        index += count;
        first += count;
        first <<= 1;
        code <<= 1;
    }
    0
}

// ---------------------------------------------------------------------------
// Streaming inflate (32 KiB window) writing to a sink
// ---------------------------------------------------------------------------

trait Sink {
    fn emit(&mut self, data: &[u8]);
}

// 32 KiB DEFLATE history lives in BSS, not the (small) user stack.
static mut WIN_BUF: [u8; 32768] = [0; 32768];
struct Window {
    pos: usize,
    written: usize,
}
impl Window {
    fn push<S: Sink>(&mut self, sink: &mut S, b: u8) {
        unsafe { WIN_BUF[self.pos] = b; }
        self.pos = (self.pos + 1) & 0x7fff;
        self.written += 1;
        sink.emit(&[b]);
    }
    fn back(&self, distance: usize) -> u8 {
        unsafe { WIN_BUF[(self.pos.wrapping_sub(distance)) & 0x7fff] }
    }
}

fn block<R: Reader, S: Sink>(
    br: &mut SBits<R>,
    sink: &mut S,
    win: &mut Window,
    lit: &Huffman,
    dist: &Huffman,
) -> bool {
    loop {
        if br.eof {
            return false;
        }
        let sym = decode(br, lit);
        if br.eof {
            return false;
        }
        if sym < 256 {
            win.push(sink, sym as u8);
        } else if sym == 256 {
            return true;
        } else {
            let idx = sym as usize - 257;
            if idx >= 29 {
                return false;
            }
            let length = LEN_BASE[idx] as usize + br.bits(LEN_EXTRA[idx] as u32) as usize;
            let dsym = decode(br, dist) as usize;
            if dsym >= 30 || br.eof {
                return false;
            }
            let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32) as usize;
            if distance > win.written || br.eof {
                return false;
            }
            for _ in 0..length {
                let b = win.back(distance);
                win.push(sink, b);
            }
        }
    }
}

fn inflate_stream<R: Reader, S: Sink>(br: &mut SBits<R>, sink: &mut S) -> bool {
    let mut win = Window { pos: 0, written: 0 };
    loop {
        let final_block = br.bits(1);
        let typ = br.bits(2);
        if br.eof {
            return false;
        }
        match typ {
            0 => {
                br.align();
                let len = br.bits(16) as usize;
                let _nlen = br.bits(16);
                if br.eof {
                    return false;
                }
                for _ in 0..len {
                    let b = br.bits(8) as u8;
                    if br.eof {
                        return false;
                    }
                    win.push(sink, b);
                }
            }
            1 => {
                let mut lengths = [0u8; 288];
                for (i, l) in lengths.iter_mut().enumerate() {
                    *l = if i < 144 { 8 } else if i < 256 { 9 } else if i < 280 { 7 } else { 8 };
                }
                let lit = build_huffman(&lengths);
                let dist = build_huffman(&[5u8; 30]);
                if !block(br, sink, &mut win, &lit, &dist) {
                    return false;
                }
            }
            2 => {
                let hlit = br.bits(5) as usize + 257;
                let hdist = br.bits(5) as usize + 1;
                let hclen = br.bits(4) as usize + 4;
                const ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];
                let mut cl = [0u8; 19];
                for i in 0..hclen {
                    cl[ORDER[i]] = br.bits(3) as u8;
                }
                let code_huff = build_huffman(&cl);
                let mut lengths = [0u8; 320];
                let total = hlit + hdist;
                let mut i = 0;
                while i < total {
                    let s = decode(br, &code_huff);
                    if br.eof {
                        return false;
                    }
                    if s < 16 {
                        lengths[i] = s as u8;
                        i += 1;
                    } else if s == 16 {
                        if i == 0 {
                            return false;
                        }
                        let prev = lengths[i - 1];
                        let rep = 3 + br.bits(2) as usize;
                        for _ in 0..rep {
                            if i >= total {
                                break;
                            }
                            lengths[i] = prev;
                            i += 1;
                        }
                    } else if s == 17 {
                        i += 3 + br.bits(3) as usize;
                    } else if s == 18 {
                        i += 11 + br.bits(7) as usize;
                    } else {
                        return false;
                    }
                }
                let lit = build_huffman(&lengths[..hlit]);
                let dist = build_huffman(&lengths[hlit..total]);
                if !block(br, sink, &mut win, &lit, &dist) {
                    return false;
                }
            }
            _ => return false,
        }
        if final_block == 1 {
            return true;
        }
    }
}

/// Skip a gzip member header (RFC 1952), leaving the reader at the DEFLATE data.
fn gzip_header<R: Reader>(br: &mut SBits<R>) -> bool {
    if br.bits(8) != 0x1f || br.bits(8) != 0x8b {
        return false;
    }
    gzip_header_rest(br)
}

/// Header fields after the 2 magic bytes (CM, FLG, MTIME/XFL/OS, optional).
fn gzip_header_rest<R: Reader>(br: &mut SBits<R>) -> bool {
    let cm = br.bits(8);
    let flg = br.bits(8);
    for _ in 0..6 {
        br.bits(8); // MTIME(4) XFL OS
    }
    if cm != 8 {
        return false;
    }
    if flg & 4 != 0 {
        let xlen = br.bits(8) | (br.bits(8) << 8);
        for _ in 0..xlen {
            br.bits(8);
        }
    }
    if flg & 8 != 0 {
        while br.bits(8) != 0 {}
    }
    if flg & 16 != 0 {
        while br.bits(8) != 0 {}
    }
    if flg & 2 != 0 {
        br.bits(8);
        br.bits(8);
    }
    true
}

/// Buffer sink output into 8 KiB chunks so the underlying sink (tar extractor /
/// index appender) does large sequential writes instead of per-byte calls.
struct BufSink<S: Sink> {
    inner: S,
    buf: [u8; 4096],
    len: usize,
}
impl<S: Sink> Sink for BufSink<S> {
    fn emit(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            let take = (self.buf.len() - self.len).min(data.len() - i);
            self.buf[self.len..self.len + take].copy_from_slice(&data[i..i + take]);
            self.len += take;
            i += take;
            if self.len == self.buf.len() {
                self.inner.emit(&self.buf[..self.len]);
                self.len = 0;
            }
        }
    }
}
impl<S: Sink> BufSink<S> {
    fn flush(&mut self) {
        if self.len > 0 {
            self.inner.emit(&self.buf[..self.len]);
            self.len = 0;
        }
    }
}

/// Decompress a (possibly multi-member) gzip stream into a sink. Alpine splits
/// the tar across gzip members (e.g. APKINDEX.tar.gz: .SIGN in member 1, the
/// index in member 2), so the tar sink must be fed across all members. Returns
/// the sink on success (so callers can read counters / close handles).
fn gunzip_all<R: Reader, S: Sink>(br: &mut SBits<R>, sink: S) -> Option<S> {
    let mut bs = BufSink { inner: sink, buf: [0; 4096], len: 0 };
    if !gzip_header(br) {
        return None;
    }
    loop {
        if !inflate_stream(br, &mut bs) {
            bs.flush();
            return None;
        }
        bs.flush();
        br.align(); // drop deflate padding bits; byte-align at the trailer
        for _ in 0..8 {
            br.bits(8); // CRC32 + ISIZE
        }
        let m1 = br.bits(8);
        let m2 = br.bits(8);
        if m1 != 0x1f || m2 != 0x8b {
            break; // end of stream (EOF or zero padding)
        }
        if !gzip_header_rest(br) {
            return None;
        }
    }
    bs.flush();
    Some(bs.inner)
}

// ---------------------------------------------------------------------------
// tar extraction sink
// ---------------------------------------------------------------------------

struct TarSink<'a> {
    dest: &'a str,
    hdr: [u8; 512],
    hdr_len: usize,
    file_remaining: usize,
    pad_remaining: usize,
    handle: Option<u64>,
    offset: usize,
    longname: [u8; 256],
    longname_len: usize,
    pub files: usize,
    pub links: usize,
}

fn octal(b: &[u8]) -> usize {
    let mut v = 0usize;
    for &c in b {
        if c < b'0' || c > b'7' {
            break;
        }
        v = v * 8 + (c - b'0') as usize;
    }
    v
}

fn mkdir_p(path: &str) {
    let mut acc = [0u8; 256];
    let mut n = 0;
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if n == 0 && path.starts_with('/') {
            acc[0] = b'/';
            n = 1;
        }
        let pb = part.as_bytes();
        if n + pb.len() + 1 >= acc.len() {
            return;
        }
        acc[n..n + pb.len()].copy_from_slice(pb);
        n += pb.len();
        let _ = k::mkdir(core::str::from_utf8(&acc[..n]).unwrap_or(""));
        acc[n] = b'/';
        n += 1;
    }
}

impl<'a> TarSink<'a> {
    fn new(dest: &'a str) -> Self {
        TarSink {
            dest, hdr: [0; 512], hdr_len: 0, file_remaining: 0, pad_remaining: 0,
            handle: None, offset: 0, longname: [0; 256], longname_len: 0, files: 0, links: 0,
        }
    }

    fn start_file(&mut self, name: &[u8], size: usize) {
        let mut path = [0u8; 256];
        let mut n = 0;
        let db = self.dest.as_bytes();
        path[..db.len()].copy_from_slice(db);
        n += db.len();
        path[n] = b'/';
        n += 1;
        // strip leading "./"
        let name = if name.starts_with(b"./") { &name[2..] } else { name };
        let take = name.len().min(path.len() - n - 1);
        path[n..n + take].copy_from_slice(&name[..take]);
        n += take;
        let p = core::str::from_utf8(&path[..n]).unwrap_or("");
        if let Some(slash) = p.rfind('/') {
            mkdir_p(&p[..slash]);
        }
        self.handle = k::create(p);
        self.offset = 0;
        self.file_remaining = size;
        self.pad_remaining = (512 - (size % 512)) % 512;
        self.files += 1;
    }

    fn process_header(&mut self) {
        let h = &self.hdr;
        if h.iter().all(|&b| b == 0) {
            return;
        }
        let mut name = [0u8; 256];
        let mut nl = 0;
        if self.longname_len > 0 {
            let take = self.longname_len.min(255);
            name[..take].copy_from_slice(&self.longname[..take]);
            nl = take;
            self.longname_len = 0;
        } else {
            for &b in &h[..100] {
                if b == 0 {
                    break;
                }
                if nl < 255 {
                    name[nl] = b;
                    nl += 1;
                }
            }
            // POSIX ustar prefix (magic "ustar\0"); GNU tar ("ustar  ") stores
            // atime at 345, so only honour the prefix for the POSIX variant.
            if &h[257..263] == b"ustar\0" && h[345] != 0 {
                let mut full = [0u8; 256];
                let mut fl = 0;
                for &b in &h[345..500] {
                    if b == 0 {
                        break;
                    }
                    if fl < 254 {
                        full[fl] = b;
                        fl += 1;
                    }
                }
                full[fl] = b'/';
                fl += 1;
                let take = nl.min(full.len() - fl - 1);
                full[fl..fl + take].copy_from_slice(&name[..take]);
                fl += take;
                name = full;
                nl = fl;
            }
        }
        let size = octal(&h[124..136]);
        let typeflag = h[156];
        match typeflag {
            b'5' => {
                let mut path = [0u8; 256];
                let mut n = 0;
                let db = self.dest.as_bytes();
                path[..db.len()].copy_from_slice(db);
                n += db.len();
                path[n] = b'/';
                n += 1;
                let nm = if name[..nl].starts_with(b"./") { &name[2..nl] } else { &name[..nl] };
                let take = nm.len().min(path.len() - n - 1);
                path[n..n + take].copy_from_slice(&nm[..take]);
                n += take;
                mkdir_p(core::str::from_utf8(&path[..n]).unwrap_or(""));
                self.file_remaining = 0;
                self.pad_remaining = 0;
            }
            b'L' => {
                // GNU long name: data is the real name of the next entry
                self.file_remaining = size;
                self.pad_remaining = (512 - (size % 512)) % 512;
                self.longname_len = 0;
                self.handle = None;
                // mark: collect into longname via a flag using offset==usize::MAX
                self.offset = usize::MAX;
            }
            b'0' | 0 => self.start_file(&name[..nl], size),
            _ => {
                // symlinks/hardlinks/pax headers/etc: skip data. KimiFS has no
                // symlink inode type, so links are counted but not created.
                if typeflag == b'2' {
                    self.links += 1;
                }
                self.file_remaining = size;
                self.pad_remaining = (512 - (size % 512)) % 512;
                self.handle = None;
                self.offset = 0;
            }
        }
    }
}

impl Sink for TarSink<'_> {
    fn emit(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            if self.file_remaining > 0 {
                let take = self.file_remaining.min(data.len() - i);
                if self.offset == usize::MAX {
                    // collecting GNU long name
                    let t = take.min(self.longname.len() - self.longname_len);
                    self.longname[self.longname_len..self.longname_len + t].copy_from_slice(&data[i..i + t]);
                    self.longname_len += t;
                } else if let Some(h) = self.handle {
                    k::write_file(h, self.offset, &data[i..i + take]);
                    self.offset += take;
                }
                self.file_remaining -= take;
                i += take;
                if self.file_remaining == 0 {
                    if let Some(h) = self.handle {
                        k::close(h);
                        self.handle = None;
                    }
                    if self.offset == usize::MAX {
                        // trim trailing NUL from long name
                        while self.longname_len > 0 && self.longname[self.longname_len - 1] == 0 {
                            self.longname_len -= 1;
                        }
                        self.offset = 0;
                    }
                }
            } else if self.pad_remaining > 0 {
                let take = self.pad_remaining.min(data.len() - i);
                self.pad_remaining -= take;
                i += take;
            } else {
                // header
                let take = (512 - self.hdr_len).min(data.len() - i);
                self.hdr[self.hdr_len..self.hdr_len + take].copy_from_slice(&data[i..i + take]);
                self.hdr_len += take;
                i += take;
                if self.hdr_len == 512 {
                    self.hdr_len = 0;
                    self.process_header();
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// control member (buffer-based, small) for .PKGINFO
// ---------------------------------------------------------------------------

fn gunzip_member_buf(input: &[u8], out: &mut [u8]) -> Option<(usize, usize)> {
    if input.len() < 18 || input[0] != 0x1f || input[1] != 0x8b || input[2] != 8 {
        return None;
    }
    let flg = input[3];
    let mut p = 10;
    if flg & 4 != 0 {
        let xl = input[p] as usize | ((input[p + 1] as usize) << 8);
        p += 2 + xl;
    }
    if flg & 8 != 0 {
        while p < input.len() && input[p] != 0 {
            p += 1;
        }
        p += 1;
    }
    if flg & 16 != 0 {
        while p < input.len() && input[p] != 0 {
            p += 1;
        }
        p += 1;
    }
    if flg & 2 != 0 {
        p += 2;
    }
    if p >= input.len() {
        return None;
    }
    let (written, consumed) = crate::apk::inflate_ex(&input[p..], out);
    Some((written, p + consumed + 8))
}

// ---------------------------------------------------------------------------
// APKINDEX parsing + dependency resolution
// ---------------------------------------------------------------------------

fn with_index_stanzas(mut f: impl FnMut(&[u8]) -> bool) {
    let Some(h) = k::open(INDEX) else { return };
    let mut chunk = [0u8; 4096];
    let mut off = 0usize;
    let mut stanza = [0u8; 4096];
    let mut slen = 0usize;
    let mut line = [0u8; 512];
    let mut llen = 0usize;
    let mut in_stanza = false;
    let mut stopped_early = false;
    'outer: loop {
        let n = k::read_file(h, off, &mut chunk);
        if n == 0 {
            break;
        }
        off += n;
        for &b in &chunk[..n] {
            if b == b'\n' {
                if llen == 0 {
                    if in_stanza {
                        if !f(&stanza[..slen]) {
                            stopped_early = true;
                            break 'outer;
                        }
                        slen = 0;
                        in_stanza = false;
                    }
                } else {
                    if slen + llen + 1 < stanza.len() {
                        stanza[slen..slen + llen].copy_from_slice(&line[..llen]);
                        slen += llen;
                        stanza[slen] = b'\n';
                        slen += 1;
                    }
                    in_stanza = true;
                    llen = 0;
                }
            } else if llen < line.len() {
                line[llen] = b;
                llen += 1;
            }
        }
    }
    // Flush a trailing unterminated stanza only when the input was exhausted;
    // after an early stop the callback already saw (and rejected) this stanza.
    if !stopped_early && in_stanza && slen > 0 {
        f(&stanza[..slen]);
    }
    k::close(h);
}

fn field<'a>(stanza: &'a [u8], tag: &[u8]) -> Option<&'a [u8]> {
    // tag like b"P:" at line start
    let mut start = 0;
    loop {
        if stanza[start..].starts_with(tag) {
            let vs = start + tag.len();
            let ve = vs + stanza[vs..].iter().position(|&b| b == b'\n').unwrap_or(stanza.len() - vs);
            return Some(&stanza[vs..ve]);
        }
        match stanza[start..].iter().position(|&b| b == b'\n') {
            Some(p) => start += p + 1,
            None => return None,
        }
        if start >= stanza.len() {
            return None;
        }
    }
}

#[derive(Clone, Copy)]
struct Pkg {
    name: [u8; 48],
    name_len: usize,
    ver: [u8; 40],
    ver_len: usize,
    size: u64,
}

fn parse_pkg(stanza: &[u8]) -> Option<Pkg> {
    let name = field(stanza, b"P:")?;
    let ver = field(stanza, b"V:").unwrap_or(b"");
    let size = field(stanza, b"S:").and_then(|s| core::str::from_utf8(s).ok()).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut p = Pkg { name: [0; 48], name_len: 0, ver: [0; 40], ver_len: 0, size };
    p.name_len = name.len().min(48);
    p.name[..p.name_len].copy_from_slice(&name[..p.name_len]);
    p.ver_len = ver.len().min(40);
    p.ver[..p.ver_len].copy_from_slice(&ver[..p.ver_len]);
    Some(p)
}

fn find_by_name(name: &str) -> Option<Pkg> {
    let mut found = None;
    let nb = name.as_bytes();
    with_index_stanzas(|s| {
        if let Some(n) = field(s, b"P:") {
            if n == nb {
                found = parse_pkg(s);
                return false;
            }
        }
        true
    });
    found
}

/// Find a package providing a dependency token (e.g. "so:libc.musl-aarch64.so.1"
/// or "/bin/sh"). Matches the `p:` provides line (token may carry "=version").
fn find_provider(token: &str) -> Option<Pkg> {
    let mut found = None;
    let tb = token.as_bytes();
    with_index_stanzas(|s| {
        if let Some(prov) = field(s, b"p:") {
            for t in prov.split(|&b| b == b' ') {
                let base = t.split(|&b| b == b'=').next().unwrap_or(t);
                if base == tb {
                    found = parse_pkg(s);
                    return false;
                }
            }
        }
        true
    });
    found
}

fn resolve_dep(token: &str) -> Option<Pkg> {
    // strip version constraints (>=, =, ~, <)
    let base = token.split(|c| c == '>' || c == '=' || c == '<' || c == '~').next().unwrap_or(token);
    if base.starts_with("so:") || base.starts_with('/') {
        find_provider(base).or_else(|| find_by_name(base))
    } else {
        find_by_name(base).or_else(|| find_provider(base))
    }
}

// ---------------------------------------------------------------------------
// HTTP download (reuse kpkg's plain-HTTP body fetch via a small inline client)
// ---------------------------------------------------------------------------

/// Case-insensitive lookup of an HTTP header value within a header block.
fn header_value<'a>(hdr: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut start = 0;
    while start < hdr.len() {
        let end = hdr[start..].windows(2).position(|w| w == b"\r\n").map(|p| start + p).unwrap_or(hdr.len());
        let line = &hdr[start..end];
        if line.len() > name.len() && line[..name.len()].eq_ignore_ascii_case(name) && line[name.len()] == b':' {
            let mut v = &line[name.len() + 1..];
            while let Some((&b' ', rest)) = v.split_first() {
                v = rest;
            }
            return Some(v);
        }
        start = end + 2;
    }
    None
}

fn http_to_file(url: &str, dest: &str) -> bool {
    let mut hb = [0u8; 128];
    let Some((host, port, path)) = crate::kpkg::parse_url_pub(url, &mut hb) else { return false };
    let ip = crate::kpkg::resolve_host(host);
    let Some(ip) = ip else { return false };
    let Some(slot) = k::net_tcp_connect(ip, port, 8000) else { return false };
    let mut req = [0u8; 512];
    let mut n = 0;
    for piece in [b"GET ", path.as_bytes(), b" HTTP/1.0\r\nHost: ", host.as_bytes(), b"\r\nConnection: close\r\n\r\n"] {
        let take = piece.len().min(req.len() - n);
        req[n..n + take].copy_from_slice(&piece[..take]);
        n += take;
    }
    k::net_tcp_send(slot, &req[..n]);
    let Some(h) = k::create(dest) else { k::net_tcp_close(slot); return false };
    let mut buf = [0u8; 4096];
    let mut pending = [0u8; 4096];
    let mut plen = 0;
    let mut header_done = false;
    let mut expect: Option<u64> = None;
    let mut off = 0u64;
    let mut ok = false;
    loop {
        let got = k::net_tcp_recv(slot, &mut buf, 15000);
        if got == 0 {
            break;
        }
        if !header_done {
            let copy = got.min(pending.len() - plen);
            pending[plen..plen + copy].copy_from_slice(&buf[..copy]);
            plen += copy;
            if let Some(pos) = pending[..plen].windows(4).position(|w| w == b"\r\n\r\n") {
                header_done = true;
                let hdr = &pending[..pos];
                // check status line for 200
                let status_ok = hdr.starts_with(b"HTTP/1.1 200") || hdr.starts_with(b"HTTP/1.0 200");
                // chunked bodies carry framing we do not decode; refuse them
                let chunked = header_value(hdr, b"transfer-encoding")
                    .map(|v| v.windows(7).any(|w| w.eq_ignore_ascii_case(b"chunked")))
                    .unwrap_or(false);
                expect = header_value(hdr, b"content-length")
                    .and_then(|v| core::str::from_utf8(v).ok())
                    .and_then(|s| s.trim().parse().ok());
                if !status_ok || chunked {
                    break;
                }
                let bs = pos + 4;
                if bs < plen {
                    k::write_file(h, off as usize, &pending[bs..plen]);
                    off += (plen - bs) as u64;
                }
                plen = 0;
            } else if plen == pending.len() {
                break; // header larger than the buffer; cannot parse
            }
        } else {
            k::write_file(h, off as usize, &buf[..got]);
            off += got as u64;
        }
    }
    // A dropped connection mid-body must not pass as a complete download:
    // verify against Content-Length when the server sent one.
    if header_done && off > 0 {
        ok = match expect {
            Some(len) => off == len,
            None => true,
        };
    }
    k::close(h);
    k::net_tcp_close(slot);
    if !ok {
        let _ = k::unlink(dest);
    }
    ok
}

// ---------------------------------------------------------------------------
// public commands
// ---------------------------------------------------------------------------

fn repo_root() -> [u8; 128] {
    let mut buf = [0u8; 128];
    if let Some((_, size)) = k::stat(REPO) {
        if let Some(h) = k::open(REPO) {
            let n = k::read_file(h, 0, &mut buf[..(size as usize).min(128)]);
            k::close(h);
            // trim newline
            let mut len = n;
            while len > 0 && (buf[len - 1] == b'\n' || buf[len - 1] == b'\r') {
                len -= 1;
            }
            if len > 0 {
                return buf;
            }
        }
    }
    let d = DEFAULT_ROOT.as_bytes();
    buf[..d.len()].copy_from_slice(d);
    buf
}

fn url_join(root: &[u8], repo: &str, file: &str, out: &mut [u8; 256]) -> usize {
    let mut n = 0;
    let rs = core::str::from_utf8(root).unwrap_or(DEFAULT_ROOT);
    let rs = rs.trim_end_matches('\0').trim_end_matches('/');
    for piece in [rs.as_bytes(), b"/", repo.as_bytes(), b"/", ARCH.as_bytes(), b"/", file.as_bytes()] {
        let take = piece.len().min(out.len() - n);
        out[n..n + take].copy_from_slice(&piece[..take]);
        n += take;
    }
    n
}

/// Fetch APKINDEX for main + community into the combined index file.
pub fn update(root_url: &str) -> bool {
    let _ = k::mkdir(DIR);
    let root = if root_url.is_empty() { DEFAULT_ROOT } else { root_url };
    if let Some(h) = k::create(REPO) {
        k::write_file(h, 0, root.as_bytes());
        k::close(h);
    }
    // truncate combined index
    if let Some(h) = k::create(INDEX) {
        k::close(h);
    }
    let mut got_any = false;
    for repo in ["main", "community"] {
        let mut url = [0u8; 256];
        let n = url_join(root.as_bytes(), repo, "APKINDEX.tar.gz", &mut url);
        let url_str = core::str::from_utf8(&url[..n]).unwrap_or("");
        let tmpgz = "/disk/alpine/idx.tar.gz";
        let mut fetched = false;
        for attempt in 0..4 {
            if attempt > 0 {
                k::sleep_ms(200);
            }
            if http_to_file(url_str, tmpgz) {
                fetched = true;
                break;
            }
        }
        if !fetched {
            continue;
        }
        let Some(h) = k::open(tmpgz) else { continue };
        // stream-decompress the index tar, appending the APKINDEX entry to INDEX
        let sink = IndexAppendSink { out: None, off: 0, in_index: false, hdr: [0; 512], hl: 0, rem: 0, pad: 0 };
        let fr = FileReader { handle: h, offset: 0 };
        let mut br = SBits::new(fr);
        if let Some(s) = gunzip_all(&mut br, sink) {
            got_any = true;
            if let Some(o) = s.out {
                k::close(o);
            }
        }
        k::close(h);
        let _ = k::unlink(tmpgz);
    }
    got_any
}

/// Tar sink that extracts only the APKINDEX member, appending to INDEX.
struct IndexAppendSink {
    out: Option<u64>,
    off: usize,
    in_index: bool,
    hdr: [u8; 512],
    hl: usize,
    rem: usize,
    pad: usize,
}
impl Sink for IndexAppendSink {
    fn emit(&mut self, data: &[u8]) {
        let mut i = 0;
        while i < data.len() {
            if self.rem > 0 {
                let take = self.rem.min(data.len() - i);
                if self.in_index {
                    if self.out.is_none() {
                        self.out = k::open(INDEX).or_else(|| k::create(INDEX));
                        // append at end: find size
                        if let Some((_, sz)) = k::stat(INDEX) {
                            self.off = sz as usize;
                        }
                    }
                    if let Some(o) = self.out {
                        k::write_file(o, self.off, &data[i..i + take]);
                        self.off += take;
                    }
                }
                self.rem -= take;
                i += take;
                if self.rem == 0 {
                    self.in_index = false;
                }
            } else if self.pad > 0 {
                let take = self.pad.min(data.len() - i);
                self.pad -= take;
                i += take;
            } else {
                let take = (512 - self.hl).min(data.len() - i);
                self.hdr[self.hl..self.hl + take].copy_from_slice(&data[i..i + take]);
                self.hl += take;
                i += take;
                if self.hl == 512 {
                    self.hl = 0;
                    let name = &self.hdr[..100];
                    let is_idx = name.starts_with(b"APKINDEX");
                    let size = octal(&self.hdr[124..136]);
                    self.in_index = is_idx;
                    self.rem = size;
                    self.pad = (512 - (size % 512)) % 512;
                }
            }
        }
    }
}

/// List available packages (name version).
pub fn list() {
    if k::stat(INDEX).is_none() {
        k::print("apk: no index — run `kpkg apk-update` first\n");
        return;
    }
    let mut count = 0u64;
    with_index_stanzas(|s| {
        if let Some(p) = parse_pkg(s) {
            k::print("  ");
            k::print_bytes(&p.name[..p.name_len]);
            k::print(" ");
            k::print_bytes(&p.ver[..p.ver_len]);
            k::print("\n");
            count += 1;
        }
        true
    });
    k::print("apk: ");
    k::print_u64(count);
    k::print(" packages in index\n");
}

fn pkg_filename(p: &Pkg, out: &mut [u8; 128]) -> usize {
    let mut n = 0;
    for piece in [&p.name[..p.name_len], b"-", &p.ver[..p.ver_len], b".apk"] {
        let take = piece.len().min(out.len() - n);
        out[n..n + take].copy_from_slice(&piece[..take]);
        n += take;
    }
    n
}

/// Install a package and its dependency closure into the sysroot.
pub fn install(name: &str) -> bool {
    // Convenience aliases; `nodejs` in Alpine is the LTS line (nodejs-current is
    // the latest non-LTS). latest-stable repo keeps these at the current LTS.
    let name = match name {
        "node" | "nodejs-lts" | "node-lts" => "nodejs",
        "python" | "python3-lts" => "python3",
        _ => name,
    };
    if k::stat(INDEX).is_none() {
        k::print("apk: no index — run `kpkg apk-update` first\n");
        return false;
    }
    let _ = k::mkdir(SYSROOT);
    let root = repo_root();

    // resolve closure (BFS, bounded)
    let mut queue: [Pkg; 256] = [Pkg { name: [0; 48], name_len: 0, ver: [0; 40], ver_len: 0, size: 0 }; 256];
    let mut qn = 0usize;
    let mut seen: [[u8; 48]; 256] = [[0; 48]; 256];
    let mut seen_len = [0usize; 256];
    let mut sn = 0usize;

    let Some(start) = find_by_name(name).or_else(|| find_provider(name)) else {
        k::print("apk: package not in index: ");
        k::print(name);
        k::print("\n");
        return false;
    };
    queue[qn] = start;
    qn += 1;

    let mut head = 0;
    while head < qn && qn < 256 {
        let cur = queue[head];
        head += 1;
        let cname = core::str::from_utf8(&cur.name[..cur.name_len]).unwrap_or("");
        // skip if already seen
        let mut dup = false;
        for i in 0..sn {
            if seen_len[i] == cur.name_len && seen[i][..cur.name_len] == cur.name[..cur.name_len] {
                dup = true;
                break;
            }
        }
        if dup {
            continue;
        }
        if sn < 256 {
            seen[sn][..cur.name_len].copy_from_slice(&cur.name[..cur.name_len]);
            seen_len[sn] = cur.name_len;
            sn += 1;
        }
        // enqueue deps from this package's stanza
        with_index_stanzas(|s| {
            if let Some(n) = field(s, b"P:") {
                if n == &cur.name[..cur.name_len] {
                    if let Some(deps) = field(s, b"D:") {
                        if let Ok(ds) = core::str::from_utf8(deps) {
                            for tok in ds.split(' ') {
                                if tok.is_empty() || tok.starts_with('!') {
                                    continue;
                                }
                                if let Some(dep) = resolve_dep(tok) {
                                    if qn < 256 {
                                        queue[qn] = dep;
                                        qn += 1;
                                    }
                                }
                            }
                        }
                    }
                    return false;
                }
            }
            true
        });
        let _ = cname;
    }

    k::print("apk: installing ");
    k::print(name);
    k::print(" + ");
    k::print_u64(sn as u64);
    k::print(" packages into ");
    k::print(SYSROOT);
    k::print("\n");

    let mut ok = 0;
    let mut total_files = 0usize;
    let mut total_links = 0usize;
    for i in 0..sn {
        let pname = core::str::from_utf8(&seen[i][..seen_len[i]]).unwrap_or("");
        let Some(p) = find_by_name(pname) else { continue };
        let mut fname = [0u8; 128];
        let fl = pkg_filename(&p, &mut fname);
        let fname_str = core::str::from_utf8(&fname[..fl]).unwrap_or("");
        let mut downloaded = false;
        for repo in ["main", "community"] {
            let mut url = [0u8; 256];
            let n = url_join(&root, repo, fname_str, &mut url);
            let url_str = core::str::from_utf8(&url[..n]).unwrap_or("");
            // retry transient connection drops (CDN closes keep-alive sockets)
            for attempt in 0..4 {
                if attempt > 0 { k::sleep_ms(200); }
                if http_to_file(url_str, TMP) {
                    downloaded = true;
                    break;
                }
            }
            if downloaded {
                break;
            }
        }
        if !downloaded {
            k::print("  ! download failed: ");
            k::print(pname);
            k::print("\n");
            continue;
        }
        if let Some((files, links)) = extract_apk(TMP, SYSROOT) {
            ok += 1;
            total_files += files;
            total_links += links;
            k::print("  + ");
            k::print(pname);
            k::print(" ");
            k::print_bytes(&p.ver[..p.ver_len]);
            k::print(" (");
            k::print_u64(files as u64);
            k::print(" files");
            if links > 0 {
                k::print(", ");
                k::print_u64(links as u64);
                k::print(" symlinks skipped");
            }
            k::print(")\n");
        } else {
            k::print("  ! extract failed: ");
            k::print(pname);
            k::print("\n");
        }
        let _ = k::unlink(TMP);
    }
    let _ = k::unlink(TMP);
    k::print("apk: extracted ");
    k::print_u64(ok);
    k::print("/");
    k::print_u64(sn as u64);
    k::print(" packages, ");
    k::print_u64(total_files as u64);
    k::print(" files (");
    k::print_u64(total_links as u64);
    k::print(" symlinks skipped)\n");
    k::print("note: binaries are dynamic musl; running them needs the dynamic\n");
    k::print("loader + musl + fuller Linux ABI (Desktop Lite runtime milestone).\n");
    ok > 0
}

// Scratch for member probing lives in BSS: 2 x 256 KiB on the stack would
// overflow the 64 KiB bare-metal user stack. Single-threaded, non-reentrant.
static mut APK_HEAD: [u8; 262144] = [0; 262144];
static mut APK_CONTROL: [u8; 262144] = [0; 262144];

/// True if a decompressed tar member is the signature or control member
/// (contains `.SIGN*` / `.PKGINFO` entries) rather than payload data.
fn is_control_tar(tar: &[u8]) -> bool {
    let mut pos = 0;
    while pos + 512 <= tar.len() {
        let h = &tar[pos..pos + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let name = &h[..100];
        if name.windows(5).any(|w| w == b".SIGN") || name.windows(8).any(|w| w == b".PKGINFO") {
            return true;
        }
        let size = octal(&h[124..136]);
        pos += 512 + ((size + 511) / 512) * 512;
    }
    false
}

/// Extract a downloaded `.apk` into dest. Alpine packages are concatenated
/// gzip members: a `.SIGN` tar, a control tar (`.PKGINFO` + scripts), then the
/// data tar (older packages may omit the signature member). Skip the metadata
/// members and stream only the data tar into the sysroot. Returns
/// `(regular files extracted, symlinks skipped)`.
fn extract_apk(path: &str, dest: &str) -> Option<(usize, usize)> {
    let (_, size) = k::stat(path)?;
    let h = k::open(path)?;
    let head = unsafe { &mut *core::ptr::addr_of_mut!(APK_HEAD) };
    let control = unsafe { &mut *core::ptr::addr_of_mut!(APK_CONTROL) };
    let mut data_start = 0usize;
    let mut found_data = false;
    while data_start < size as usize {
        let want = ((size as usize) - data_start).min(head.len());
        let mut got = 0;
        while got < want {
            let n = k::read_file(h, data_start + got, &mut head[got..want]);
            if n == 0 {
                break;
            }
            got += n;
        }
        if got < 18 {
            break;
        }
        let Some((written, consumed)) = gunzip_member_buf(&head[..got], control) else { break };
        if written == control.len() {
            // Too large to be signature/control: this is the data member.
            found_data = true;
            break;
        }
        if is_control_tar(&control[..written]) {
            data_start += consumed;
            continue;
        }
        found_data = true;
        break;
    }
    if !found_data {
        k::close(h);
        return None;
    }
    let fr = FileReader { handle: h, offset: data_start };
    let res = gunzip_all(&mut SBits::new(fr), TarSink::new(dest));
    k::close(h);
    res.map(|s| (s.files, s.links))
}

/// Print metadata for an installed/available package.
pub fn info(name: &str) {
    let Some(p) = find_by_name(name) else {
        k::print("apk: not in index\n");
        return;
    };
    k::print("package: ");
    k::print_bytes(&p.name[..p.name_len]);
    k::print("\nversion: ");
    k::print_bytes(&p.ver[..p.ver_len]);
    k::print("\nsize:    ");
    k::print_u64(p.size);
    k::print(" bytes (compressed)\n");
    with_index_stanzas(|s| {
        if let Some(n) = field(s, b"P:") {
            if n == &p.name[..p.name_len] {
                if let Some(d) = field(s, b"D:") {
                    k::print("depends: ");
                    k::print_bytes(d);
                    k::print("\n");
                }
                if let Some(t) = field(s, b"T:") {
                    k::print("desc:    ");
                    k::print_bytes(t);
                    k::print("\n");
                }
                return false;
            }
        }
        true
    });
}

/// Debug: stream-extract a gzip tar file to dest, returning the file count.
pub fn debug_extract(src: &str, dest: &str) -> u64 {
    let Some(h) = k::open(src) else { return 0 };
    let fr = FileReader { handle: h, offset: 0 };
    let mut br = SBits::new(fr);
    let res = gunzip_all(&mut br, TarSink::new(dest));
    k::close(h);
    match res { Some(s) => s.files as u64, None => 0 }
}
