//! APK inspector: reads an Android package (a ZIP archive) and extracts the
//! binary manifest, permissions, components and DEX header.
//!
//! This does NOT execute the app — running real APKs requires the Android
//! runtime (see docs/android-feasibility.md). It parses the package so its
//! contents can be inspected on KimiOS.

use crate::{close, open, read_file, stat};

pub struct ApkInfo {
    pub valid: bool,
    pub package: [u8; 96],
    pub package_len: usize,
    pub version_name: [u8; 32],
    pub version_name_len: usize,
    pub version_code: u32,
    pub min_sdk: u32,
    pub target_sdk: u32,
    pub label: [u8; 64],
    pub label_len: usize,
    pub permissions: [[u8; 64]; 24],
    pub permission_lens: [usize; 24],
    pub permission_count: usize,
    pub activities: [[u8; 64]; 12],
    pub activity_lens: [usize; 12],
    pub activity_count: usize,
    pub file_count: usize,
    pub has_dex: bool,
    pub dex_version: [u8; 4],
    pub dex_classes: u32,
    pub dex_methods: u32,
    pub dex_size: u32,
}

impl ApkInfo {
    pub fn new() -> Self {
        ApkInfo {
            valid: false,
            package: [0; 96],
            package_len: 0,
            version_name: [0; 32],
            version_name_len: 0,
            version_code: 0,
            min_sdk: 0,
            target_sdk: 0,
            label: [0; 64],
            label_len: 0,
            permissions: [[0; 64]; 24],
            permission_lens: [0; 24],
            permission_count: 0,
            activities: [[0; 64]; 12],
            activity_lens: [0; 12],
            activity_count: 0,
            file_count: 0,
            has_dex: false,
            dex_version: [0; 4],
            dex_classes: 0,
            dex_methods: 0,
            dex_size: 0,
        }
    }
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

// Large scratch buffers live in .bss, not on the (small) user stack. The
// inspector is single-threaded and non-reentrant within a process.
static mut TAIL: [u8; 65536] = [0; 65536];
static mut CD: [u8; 16384] = [0; 16384];
static mut COMPRESSED: [u8; 65536] = [0; 65536];
static mut MANIFEST: [u8; 32768] = [0; 32768];
static mut DEXBUF: [u8; 4096] = [0; 4096];

// ---------------------------------------------------------------------------
// DEFLATE (RFC 1951) decompression.
// ---------------------------------------------------------------------------

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, bitbuf: 0, bitcnt: 0 }
    }

    fn bits(&mut self, need: u32) -> u32 {
        while self.bitcnt < need {
            if self.pos >= self.data.len() {
                return 0;
            }
            self.bitbuf |= (self.data[self.pos] as u32) << self.bitcnt;
            self.pos += 1;
            self.bitcnt += 8;
        }
        let value = self.bitbuf & ((1u32 << need) - 1);
        self.bitbuf >>= need;
        self.bitcnt -= need;
        value
    }

    fn align(&mut self) {
        self.bitbuf = 0;
        self.bitcnt = 0;
    }
}

pub(crate) struct Huffman {
    pub(crate) counts: [u16; 16],
    pub(crate) symbols: [u16; 288],
}

pub(crate) fn build_huffman(lengths: &[u8]) -> Huffman {
    let mut h = Huffman { counts: [0; 16], symbols: [0; 288] };
    for &len in lengths {
        h.counts[len as usize] += 1;
    }
    h.counts[0] = 0;
    let mut offsets = [0u16; 16];
    for len in 1..16 {
        offsets[len] = offsets[len - 1] + h.counts[len - 1];
    }
    for (symbol, &len) in lengths.iter().enumerate() {
        if len != 0 {
            h.symbols[offsets[len as usize] as usize] = symbol as u16;
            offsets[len as usize] += 1;
        }
    }
    h
}

fn decode_symbol(br: &mut BitReader, h: &Huffman) -> u16 {
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

pub(crate) const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
    131, 163, 195, 227, 258,
];
pub(crate) const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub(crate) const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub(crate) const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
    13, 13,
];

fn inflate_block(
    br: &mut BitReader,
    out: &mut [u8],
    out_pos: &mut usize,
    lit: &Huffman,
    dist: &Huffman,
) -> bool {
    loop {
        let symbol = decode_symbol(br, lit);
        if symbol < 256 {
            if *out_pos >= out.len() {
                return false;
            }
            out[*out_pos] = symbol as u8;
            *out_pos += 1;
        } else if symbol == 256 {
            return true;
        } else {
            let idx = symbol as usize - 257;
            if idx >= 29 {
                return false;
            }
            let length = LEN_BASE[idx] as usize + br.bits(LEN_EXTRA[idx] as u32) as usize;
            let dsym = decode_symbol(br, dist) as usize;
            if dsym >= 30 {
                return false;
            }
            let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32) as usize;
            if distance > *out_pos {
                return false;
            }
            for _ in 0..length {
                if *out_pos >= out.len() {
                    return false;
                }
                let byte = out[*out_pos - distance];
                out[*out_pos] = byte;
                *out_pos += 1;
            }
        }
    }
}

/// Inflate a DEFLATE stream into `out`, returning the number of bytes written.
pub fn inflate(input: &[u8], out: &mut [u8]) -> usize {
    inflate_ex(input, out).0
}

/// Like [`inflate`], but also returns the number of input bytes consumed (the
/// DEFLATE stream length). Needed to locate the next member in a multi-stream
/// gzip file (Alpine `.apk` packages concatenate control + data gzip members).
pub fn inflate_ex(input: &[u8], out: &mut [u8]) -> (usize, usize) {
    let mut br = BitReader::new(input);
    let mut out_pos = 0usize;
    loop {
        let final_block = br.bits(1);
        let block_type = br.bits(2);
        match block_type {
            0 => {
                br.align();
                if br.pos + 4 > br.data.len() {
                    break;
                }
                let len = le16(br.data, br.pos) as usize;
                br.pos += 4;
                for _ in 0..len {
                    if br.pos >= br.data.len() || out_pos >= out.len() {
                        return (out_pos, br.pos);
                    }
                    out[out_pos] = br.data[br.pos];
                    br.pos += 1;
                    out_pos += 1;
                }
            }
            1 => {
                let mut lengths = [0u8; 288];
                for (i, item) in lengths.iter_mut().enumerate() {
                    *item = if i < 144 {
                        8
                    } else if i < 256 {
                        9
                    } else if i < 280 {
                        7
                    } else {
                        8
                    };
                }
                let lit = build_huffman(&lengths);
                let dist = build_huffman(&[5u8; 30]);
                if !inflate_block(&mut br, out, &mut out_pos, &lit, &dist) {
                    break;
                }
            }
            2 => {
                let hlit = br.bits(5) as usize + 257;
                let hdist = br.bits(5) as usize + 1;
                let hclen = br.bits(4) as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut code_lengths = [0u8; 19];
                for i in 0..hclen {
                    code_lengths[ORDER[i]] = br.bits(3) as u8;
                }
                let code_huff = build_huffman(&code_lengths);
                let mut lengths = [0u8; 320];
                let total = hlit + hdist;
                let mut i = 0usize;
                while i < total {
                    let symbol = decode_symbol(&mut br, &code_huff);
                    if symbol < 16 {
                        lengths[i] = symbol as u8;
                        i += 1;
                    } else if symbol == 16 {
                        if i == 0 {
                            return (out_pos, br.pos);
                        }
                        let prev = lengths[i - 1];
                        let repeat = 3 + br.bits(2) as usize;
                        for _ in 0..repeat {
                            if i >= total {
                                break;
                            }
                            lengths[i] = prev;
                            i += 1;
                        }
                    } else if symbol == 17 {
                        let repeat = 3 + br.bits(3) as usize;
                        i += repeat;
                    } else if symbol == 18 {
                        let repeat = 11 + br.bits(7) as usize;
                        i += repeat;
                    } else {
                        return (out_pos, br.pos);
                    }
                }
                let lit = build_huffman(&lengths[..hlit]);
                let dist = build_huffman(&lengths[hlit..total]);
                if !inflate_block(&mut br, out, &mut out_pos, &lit, &dist) {
                    break;
                }
            }
            _ => break,
        }
        if final_block == 1 {
            break;
        }
    }
    (out_pos, br.pos)
}

// ---------------------------------------------------------------------------
// ZIP reading + APK inspection.
// ---------------------------------------------------------------------------

struct Entry {
    name: [u8; 64],
    name_len: usize,
    method: u16,
    csize: u32,
    usize: u32,
    local_offset: u32,
}

fn read_entry(cd: &[u8], off: usize) -> Option<(Entry, usize)> {
    if off + 46 > cd.len() || le32(cd, off) != 0x0201_4b50 {
        return None;
    }
    let method = le16(cd, off + 10);
    let csize = le32(cd, off + 20);
    let usize = le32(cd, off + 24);
    let name_len = le16(cd, off + 28) as usize;
    let extra_len = le16(cd, off + 30) as usize;
    let comment_len = le16(cd, off + 32) as usize;
    let local_offset = le32(cd, off + 42);
    if off + 46 + name_len > cd.len() {
        return None;
    }
    let mut name = [0u8; 64];
    let copy = name_len.min(64);
    name[..copy].copy_from_slice(&cd[off + 46..off + 46 + copy]);
    Some((
        Entry { name, name_len: copy, method, csize, usize, local_offset },
        off + 46 + name_len + extra_len + comment_len,
    ))
}

fn entry_name_eq(entry: &Entry, name: &str) -> bool {
    entry.name_len == name.len() && &entry.name[..entry.name_len] == name.as_bytes()
}

/// Read a ZIP entry's bytes (inflating if needed) into `out`.
#[inline(never)]
fn read_entry_data(
    handle: u64,
    entry: &Entry,
    out: &mut [u8],
) -> usize {
    let mut header = [0u8; 30];
    if read_file(handle, entry.local_offset as usize, &mut header) < 30 {
        return 0;
    }
    if le32(&header, 0) != 0x0403_4b50 {
        return 0;
    }
    let name_len = le16(&header, 26) as usize;
    let extra_len = le16(&header, 28) as usize;
    let data_offset = entry.local_offset as usize + 30 + name_len + extra_len;

    let csize = (entry.csize as usize).min(65536);
    let compressed = unsafe { &mut COMPRESSED[..] };
    let got = read_file(handle, data_offset, &mut compressed[..csize]);
    if entry.method == 0 {
        let n = got.min(out.len());
        out[..n].copy_from_slice(&compressed[..n]);
        n
    } else if entry.method == 8 {
        inflate(&compressed[..got], out)
    } else {
        0
    }
}

#[inline(never)]
pub fn inspect_apk(path: &str, info: &mut ApkInfo) -> bool {
    let Some((_, size)) = stat(path) else {
        return false;
    };
    let Some(handle) = open(path) else {
        return false;
    };

    let tail_len = (size as usize).min(65536);
    let tail = unsafe { &mut TAIL[..] };
    let tail_start = size as usize - tail_len;
    if read_file(handle, tail_start, &mut tail[..tail_len]) < tail_len {
        close(handle);
        return false;
    }

    // Find End Of Central Directory.
    let mut eocd = None;
    if tail_len >= 22 {
        let mut i = tail_len - 22;
        loop {
            if le32(&tail, i) == 0x0605_4b50 {
                eocd = Some(i);
                break;
            }
            if i == 0 {
                break;
            }
            i -= 1;
        }
    }
    let Some(eocd) = eocd else {
        close(handle);
        return false;
    };
    let entry_count = le16(&tail, eocd + 10) as usize;
    let cd_size = le32(&tail, eocd + 12) as usize;
    let cd_offset = le32(&tail, eocd + 16) as usize;

    let cd = unsafe { &mut CD[..] };
    let cd_read = cd_size.min(cd.len());
    if read_file(handle, cd_offset, &mut cd[..cd_read]) < cd_read {
        close(handle);
        return false;
    }

    let mut manifest_entry: Option<Entry> = None;
    let mut dex_entry: Option<Entry> = None;
    let mut off = 0usize;
    let mut count = 0usize;
    while count < entry_count {
        let Some((entry, next)) = read_entry(&cd[..cd_read], off) else {
            break;
        };
        count += 1;
        if entry_name_eq(&entry, "AndroidManifest.xml") {
            manifest_entry = Some(entry);
        } else if entry_name_eq(&entry, "classes.dex") {
            dex_entry = Some(entry);
        }
        off = next;
    }
    info.file_count = count;

    if let Some(entry) = &manifest_entry {
        let manifest = unsafe { &mut MANIFEST[..] };
        let n = read_entry_data(handle, entry, manifest);
        if n > 0 {
            parse_manifest(&manifest[..n], info);
            info.valid = true;
        }
    }

    if let Some(entry) = &dex_entry {
        let dex = unsafe { &mut DEXBUF[..] };
        let n = read_entry_data(handle, entry, dex);
        if n >= 112 && &dex[..4] == b"dex\n" {
            info.has_dex = true;
            info.dex_version.copy_from_slice(&dex[4..8]);
            info.dex_size = le32(&dex, 32);
            info.dex_classes = le32(&dex, 96);
            info.dex_methods = le32(&dex, 88);
        }
    }

    close(handle);
    info.valid
}

// ---------------------------------------------------------------------------
// Binary AndroidManifest.xml (AXML).
// ---------------------------------------------------------------------------

struct StringPool {
    offsets: usize,
    data: usize,
    count: usize,
    utf8: bool,
}

fn ax_string<'a>(data: &'a [u8], pool: &StringPool, index: u32) -> &'a [u8] {
    if index as usize >= pool.count || index == u32::MAX {
        return &[];
    }
    let off_pos = pool.offsets + index as usize * 4;
    if off_pos + 4 > data.len() {
        return &[];
    }
    let str_off = pool.data + le32(data, off_pos) as usize;
    if str_off >= data.len() {
        return &[];
    }
    if pool.utf8 {
        let mut p = str_off;
        // character length (1 or 2 bytes)
        if p >= data.len() {
            return &[];
        }
        let mut clen = data[p] as usize;
        p += 1;
        if clen & 0x80 != 0 {
            if p >= data.len() {
                return &[];
            }
            clen = ((clen & 0x7f) << 8) | data[p] as usize;
            p += 1;
        }
        let _ = clen;
        if p >= data.len() {
            return &[];
        }
        let mut blen = data[p] as usize;
        p += 1;
        if blen & 0x80 != 0 {
            if p >= data.len() {
                return &[];
            }
            blen = ((blen & 0x7f) << 8) | data[p] as usize;
            p += 1;
        }
        let end = (p + blen).min(data.len());
        &data[p..end]
    } else {
        let mut p = str_off;
        if p + 2 > data.len() {
            return &[];
        }
        let mut clen = le16(data, p) as usize;
        p += 2;
        if clen & 0x8000 != 0 {
            if p + 2 > data.len() {
                return &[];
            }
            clen = ((clen & 0x7fff) << 16) | le16(data, p) as usize;
            p += 2;
        }
        // UTF-16 -> copy ASCII-ish bytes into a static-free slice is not possible;
        // decode into the caller via a small transient. We return the raw UTF-16
        // region and let the caller narrow; instead decode here to a best-effort.
        let end = (p + clen * 2).min(data.len());
        &data[p..end]
    }
}

fn copy_str(dst: &mut [u8], dst_len: &mut usize, src: &[u8]) {
    // If src looks like UTF-16 (every other byte zero), narrow it.
    let mut n = 0usize;
    if src.len() >= 2 && src.len() % 2 == 0 && src[1] == 0 {
        let mut i = 0;
        while i + 1 < src.len() && n < dst.len() {
            let c = src[i];
            if src[i + 1] == 0 && c >= 0x20 {
                dst[n] = c;
                n += 1;
            }
            i += 2;
        }
    } else {
        for &c in src {
            if n >= dst.len() {
                break;
            }
            if c >= 0x20 {
                dst[n] = c;
                n += 1;
            }
        }
    }
    *dst_len = n;
}

fn attr_string<'a>(data: &'a [u8], pool: &StringPool, attr: usize) -> &'a [u8] {
    let raw = le32(data, attr + 8);
    if raw != u32::MAX {
        return ax_string(data, pool, raw);
    }
    let data_type = data[attr + 15];
    let value = le32(data, attr + 16);
    if data_type == 0x03 {
        return ax_string(data, pool, value);
    }
    &[]
}

fn attr_int(data: &[u8], attr: usize) -> u32 {
    le32(data, attr + 16)
}

/// Compare an AXML string (possibly UTF-16) against an ASCII literal.
fn str_is(raw: &[u8], utf8: bool, ascii: &[u8]) -> bool {
    if utf8 {
        raw == ascii
    } else {
        if raw.len() != ascii.len() * 2 {
            return false;
        }
        for i in 0..ascii.len() {
            if raw[i * 2] != ascii[i] || raw[i * 2 + 1] != 0 {
                return false;
            }
        }
        true
    }
}

fn parse_manifest(data: &[u8], info: &mut ApkInfo) {
    if data.len() < 8 || le16(data, 0) != 0x0003 {
        return;
    }
    // Locate the string pool chunk (first chunk after the 8-byte header).
    let mut pool = StringPool { offsets: 0, data: 0, count: 0, utf8: false };
    let mut pos = 8usize;
    if pos + 8 <= data.len() && le16(data, pos) == 0x0001 {
        let chunk_size = le32(data, pos + 4) as usize;
        let string_count = le32(data, pos + 8) as usize;
        let flags = le32(data, pos + 16);
        let strings_start = le32(data, pos + 20) as usize;
        pool = StringPool {
            offsets: pos + 28,
            data: pos + strings_start,
            count: string_count,
            utf8: flags & 0x100 != 0,
        };
        pos += chunk_size;
    }

    // Walk XML chunks.
    while pos + 8 <= data.len() {
        let chunk_type = le16(data, pos);
        let chunk_size = le32(data, pos + 4) as usize;
        if chunk_size < 8 || pos + chunk_size > data.len() {
            break;
        }
        if chunk_type == 0x0102 {
            // Start element: header(16) then ns(4) name(4) attr_start(2)
            // attr_size(2) attr_count(2) ...
            let name_idx = le32(data, pos + 20);
            let attr_count = le16(data, pos + 28) as usize;
            let tag = ax_string(data, &pool, name_idx);
            let attrs = pos + 36;
            if str_is(tag, pool.utf8, b"manifest") {
                for a in 0..attr_count {
                    let attr = attrs + a * 20;
                    if attr + 20 > data.len() {
                        break;
                    }
                    let aname = ax_string(data, &pool, le32(data, attr + 4));
                    if str_is(aname, pool.utf8, b"package") {
                        let v = attr_string(data, &pool, attr);
                        copy_str(&mut info.package, &mut info.package_len, v);
                    } else if str_is(aname, pool.utf8, b"versionCode") {
                        info.version_code = attr_int(data, attr);
                    } else if str_is(aname, pool.utf8, b"versionName") {
                        let v = attr_string(data, &pool, attr);
                        copy_str(&mut info.version_name, &mut info.version_name_len, v);
                    }
                }
            } else if str_is(tag, pool.utf8, b"uses-permission") {
                for a in 0..attr_count {
                    let attr = attrs + a * 20;
                    if attr + 20 > data.len() {
                        break;
                    }
                    let aname = ax_string(data, &pool, le32(data, attr + 4));
                    if str_is(aname, pool.utf8, b"name") && info.permission_count < 24 {
                        let v = attr_string(data, &pool, attr);
                        let i = info.permission_count;
                        copy_str(&mut info.permissions[i], &mut info.permission_lens[i], v);
                        info.permission_count += 1;
                    }
                }
            } else if str_is(tag, pool.utf8, b"activity") || str_is(tag, pool.utf8, b"activity-alias") {
                for a in 0..attr_count {
                    let attr = attrs + a * 20;
                    if attr + 20 > data.len() {
                        break;
                    }
                    let aname = ax_string(data, &pool, le32(data, attr + 4));
                    if str_is(aname, pool.utf8, b"name") && info.activity_count < 12 {
                        let v = attr_string(data, &pool, attr);
                        let i = info.activity_count;
                        copy_str(&mut info.activities[i], &mut info.activity_lens[i], v);
                        info.activity_count += 1;
                    }
                }
            } else if str_is(tag, pool.utf8, b"uses-sdk") {
                for a in 0..attr_count {
                    let attr = attrs + a * 20;
                    if attr + 20 > data.len() {
                        break;
                    }
                    let aname = ax_string(data, &pool, le32(data, attr + 4));
                    if str_is(aname, pool.utf8, b"minSdkVersion") {
                        info.min_sdk = attr_int(data, attr);
                    } else if str_is(aname, pool.utf8, b"targetSdkVersion") {
                        info.target_sdk = attr_int(data, attr);
                    }
                }
            } else if str_is(tag, pool.utf8, b"application") {
                for a in 0..attr_count {
                    let attr = attrs + a * 20;
                    if attr + 20 > data.len() {
                        break;
                    }
                    let aname = ax_string(data, &pool, le32(data, attr + 4));
                    if str_is(aname, pool.utf8, b"label") {
                        let v = attr_string(data, &pool, attr);
                        copy_str(&mut info.label, &mut info.label_len, v);
                    }
                }
            }
        }
        pos += chunk_size;
    }
}
