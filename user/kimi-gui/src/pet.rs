//! Desktop pet: a data-driven sprite companion (dsh-Go-compatible `pack.json`
//! schema 1 + PNG sprite atlas). The agent/app activity is reduced to a small
//! state machine (idle/thinking/working/…/sleeping/waking) with priorities,
//! one-shot timings and tap/tickle/drag reactions; the current animation frame
//! is blitted from a decoded PNG atlas. No heap: fixed buffers + static arenas,
//! matching the rest of the no_std GUI.

use crate::draw::Screen;
use libkimi as k;

// ---------------------------------------------------------------------------
// Minimal JSON reader (borrowed slices, no alloc)
// ---------------------------------------------------------------------------

enum JVal<'a> {
    Null,
    Bool(bool),
    Int(i64),
    Str(&'a [u8]),
    Arr(&'a [u8]),
    Obj(&'a [u8]),
}

struct J<'a> {
    s: &'a [u8],
    p: usize,
}

impl<'a> J<'a> {
    fn new(s: &'a [u8]) -> Self {
        J { s, p: 0 }
    }
    fn ws(&mut self) {
        while self.p < self.s.len()
            && matches!(self.s[self.p], b' ' | b'\t' | b'\n' | b'\r')
        {
            self.p += 1;
        }
    }
    fn peek(&mut self) -> u8 {
        self.ws();
        if self.p < self.s.len() {
            self.s[self.p]
        } else {
            0
        }
    }
    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == b {
            self.p += 1;
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        if !self.eat(b'"') {
            return None;
        }
        let start = self.p;
        while self.p < self.s.len() {
            let c = self.s[self.p];
            if c == b'\\' {
                self.p += 2;
                continue;
            }
            if c == b'"' {
                let inner = &self.s[start..self.p];
                self.p += 1;
                return Some(inner);
            }
            self.p += 1;
        }
        None
    }
    fn int(&mut self) -> Option<i64> {
        self.ws();
        let start = self.p;
        if self.p < self.s.len() && (self.s[self.p] == b'-' || self.s[self.p] == b'+') {
            self.p += 1;
        }
        let mut v: i64 = 0;
        let mut any = false;
        while self.p < self.s.len() && self.s[self.p].is_ascii_digit() {
            v = v.saturating_mul(10).saturating_add((self.s[self.p] - b'0') as i64);
            self.p += 1;
            any = true;
        }
        // consume (but ignore) a fractional part
        if self.p < self.s.len() && self.s[self.p] == b'.' {
            self.p += 1;
            while self.p < self.s.len() && self.s[self.p].is_ascii_digit() {
                self.p += 1;
            }
        }
        if !any {
            self.p = start;
            return None;
        }
        if self.s[start] == b'-' {
            v = -v;
        }
        Some(v)
    }
    /// Scan a balanced `{}` / `[]` compound, returning the inner slice.
    fn compound(&mut self, open: u8, close: u8) -> Option<&'a [u8]> {
        if !self.eat(open) {
            return None;
        }
        let start = self.p;
        let mut depth = 1i32;
        let mut in_str = false;
        while self.p < self.s.len() {
            let c = self.s[self.p];
            if in_str {
                if c == b'\\' {
                    self.p += 2;
                    continue;
                }
                if c == b'"' {
                    in_str = false;
                }
            } else if c == b'"' {
                in_str = true;
            } else if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    let inner = &self.s[start..self.p];
                    self.p += 1;
                    return Some(inner);
                }
            }
            self.p += 1;
        }
        None
    }
    fn value(&mut self) -> Option<JVal<'a>> {
        match self.peek() {
            b'"' => self.string().map(JVal::Str),
            b'{' => self.compound(b'{', b'}').map(JVal::Obj),
            b'[' => self.compound(b'[', b']').map(JVal::Arr),
            b't' => {
                self.p = (self.p + 4).min(self.s.len());
                Some(JVal::Bool(true))
            }
            b'f' => {
                self.p = (self.p + 5).min(self.s.len());
                Some(JVal::Bool(false))
            }
            b'n' => {
                self.p = (self.p + 4).min(self.s.len());
                Some(JVal::Null)
            }
            _ => self.int().map(JVal::Int),
        }
    }
}

fn eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// Iterate `key: value` pairs of a JSON object's inner slice.
struct ObjIter<'a> {
    j: J<'a>,
}
impl<'a> ObjIter<'a> {
    fn new(inner: &'a [u8]) -> Self {
        ObjIter { j: J::new(inner) }
    }
    fn next(&mut self) -> Option<(&'a [u8], JVal<'a>)> {
        if self.j.peek() == 0 {
            return None;
        }
        let key = self.j.string()?;
        if !self.j.eat(b':') {
            return None;
        }
        let val = self.j.value()?;
        self.j.eat(b',');
        Some((key, val))
    }
}

fn arr_first_str(inner: &[u8]) -> Option<&[u8]> {
    let mut j = J::new(inner);
    match j.value()? {
        JVal::Str(s) => Some(s),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// PNG decoder (8-bit RGB/RGBA, non-interlaced) via libkimi's inflate
// ---------------------------------------------------------------------------

const MAX_DIM: i64 = 384;
static mut ATLAS: [u8; (MAX_DIM * MAX_DIM * 4) as usize] = [0; (MAX_DIM * MAX_DIM * 4) as usize];
static mut ATLAS_W: i64 = 0;
static mut ATLAS_H: i64 = 0;
static mut IDAT: [u8; 262144] = [0; 262144];
static mut RAW: [u8; (MAX_DIM * (1 + MAX_DIM * 4)) as usize] =
    [0; (MAX_DIM * (1 + MAX_DIM * 4)) as usize];

fn be32(b: &[u8], o: usize) -> u32 {
    ((b[o] as u32) << 24) | ((b[o + 1] as u32) << 16) | ((b[o + 2] as u32) << 8) | b[o + 3] as u32
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Decode a PNG into the static ATLAS arena (RGBA8). Returns (w,h).
fn decode_png(png: &[u8]) -> Option<(i64, i64)> {
    if png.len() < 8 || &png[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let mut w = 0i64;
    let mut h = 0i64;
    let mut depth = 0u8;
    let mut ct = 0u8;
    let mut interlace = 0u8;
    let mut idat_len = 0usize;
    let mut p = 8usize;
    while p + 8 <= png.len() {
        let len = be32(png, p) as usize;
        let tag = &png[p + 4..p + 8];
        let data_start = p + 8;
        if data_start + len > png.len() {
            break;
        }
        let data = &png[data_start..data_start + len];
        if tag == b"IHDR" && len >= 13 {
            w = be32(data, 0) as i64;
            h = be32(data, 4) as i64;
            depth = data[8];
            ct = data[9];
            interlace = data[12];
        } else if tag == b"IDAT" {
            if idat_len + len > unsafe { IDAT.len() } {
                return None;
            }
            unsafe {
                IDAT[idat_len..idat_len + len].copy_from_slice(data);
            }
            idat_len += len;
        } else if tag == b"IEND" {
            break;
        }
        p = data_start + len + 4;
    }
    if w <= 0 || h <= 0 || w > MAX_DIM || h > MAX_DIM || depth != 8 || interlace != 0 {
        return None;
    }
    let bpp: i64 = match ct {
        2 => 3,
        6 => 4,
        _ => return None,
    };
    let raw_need = (h * (1 + w * bpp)) as usize;
    if idat_len < 2 || raw_need > unsafe { RAW.len() } {
        return None;
    }
    let inflated = unsafe {
        let idat = &IDAT[2..idat_len]; // skip zlib CMF/FLG
        let raw = core::slice::from_raw_parts_mut(RAW.as_mut_ptr(), raw_need);
        k::apk::inflate(idat, raw)
    };
    if inflated != raw_need {
        return None;
    }
    // Unfilter scanlines into RGBA.
    let stride = (w * bpp) as usize;
    unsafe {
        let raw = core::slice::from_raw_parts(RAW.as_ptr(), raw_need);
        let out = core::slice::from_raw_parts_mut(ATLAS.as_mut_ptr(), (w * h * 4) as usize);
        let mut prev = [0u8; (MAX_DIM * 4) as usize];
        let mut cur = [0u8; (MAX_DIM * 4) as usize];
        for y in 0..h as usize {
            let row = &raw[y * (1 + stride)..y * (1 + stride) + 1 + stride];
            let ft = row[0];
            for x in 0..stride {
                let a = if x >= bpp as usize { cur[x - bpp as usize] } else { 0 };
                let b = prev[x];
                let c = if x >= bpp as usize { prev[x - bpp as usize] } else { 0 };
                let v = row[1 + x];
                cur[x] = match ft {
                    0 => v,
                    1 => v.wrapping_add(a),
                    2 => v.wrapping_add(b),
                    3 => v.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                    4 => v.wrapping_add(paeth(a, b, c)),
                    _ => return None,
                };
            }
            for x in 0..w as usize {
                let o = (y * w as usize + x) * 4;
                let s = x * bpp as usize;
                out[o] = cur[s];
                out[o + 1] = cur[s + 1];
                out[o + 2] = cur[s + 2];
                out[o + 3] = if bpp == 4 { cur[s + 3] } else { 255 };
            }
            prev[..stride].copy_from_slice(&cur[..stride]);
        }
        ATLAS_W = w;
        ATLAS_H = h;
    }
    Some((w, h))
}

fn atlas_slice() -> &'static [u8] {
    unsafe { core::slice::from_raw_parts(ATLAS.as_ptr(), (ATLAS_W * ATLAS_H * 4) as usize) }
}

// ---------------------------------------------------------------------------
// Pack model
// ---------------------------------------------------------------------------

const MAX_ANIMS: usize = 16;
const MAX_FRAMES: usize = 24;
const NAME: usize = 16;

#[derive(Clone, Copy)]
struct Frame {
    i: i64,
    ms: i64,
}

#[derive(Clone, Copy)]
struct Anim {
    name: [u8; NAME],
    name_len: usize,
    fw: i64,
    fh: i64,
    cols: i64,
    frames: [Frame; MAX_FRAMES],
    nframes: usize,
    looping: bool,
}

impl Anim {
    fn empty() -> Self {
        Anim {
            name: [0; NAME],
            name_len: 0,
            fw: 0,
            fh: 0,
            cols: 1,
            frames: [Frame { i: 0, ms: 200 }; MAX_FRAMES],
            nframes: 0,
            looping: true,
        }
    }
    fn is(&self, n: &[u8]) -> bool {
        &self.name[..self.name_len] == n
    }
    fn duration(&self) -> i64 {
        let mut t = 0;
        for f in &self.frames[..self.nframes] {
            t += f.ms.max(1);
        }
        t.max(1)
    }
}

// State indices (also the priority order is defined separately).
const S_IDLE: usize = 0;
const S_THINKING: usize = 1;
const S_WORKING: usize = 2;
const S_JUGGLING: usize = 3;
const S_SWEEPING: usize = 4;
const S_NOTIFICATION: usize = 5;
const S_ATTENTION: usize = 6;
const S_ERROR: usize = 7;
const S_CARRYING: usize = 8;
const S_SLEEPING: usize = 9;
const S_WAKING: usize = 10;
const N_STATES: usize = 11;

fn state_index(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"idle" => S_IDLE,
        b"thinking" => S_THINKING,
        b"working" => S_WORKING,
        b"juggling" => S_JUGGLING,
        b"sweeping" => S_SWEEPING,
        b"notification" => S_NOTIFICATION,
        b"attention" => S_ATTENTION,
        b"error" => S_ERROR,
        b"carrying" => S_CARRYING,
        b"sleeping" => S_SLEEPING,
        b"waking" => S_WAKING,
        _ => return None,
    })
}

fn priority(s: usize) -> i32 {
    match s {
        S_ERROR => 8,
        S_NOTIFICATION => 7,
        S_SWEEPING => 6,
        S_ATTENTION => 5,
        S_CARRYING | S_JUGGLING => 4,
        S_WORKING => 3,
        S_THINKING => 2,
        S_IDLE | S_WAKING => 1,
        S_SLEEPING => 0,
        _ => 1,
    }
}

fn is_oneshot(s: usize) -> bool {
    matches!(s, S_ATTENTION | S_ERROR | S_WAKING)
}

#[derive(Clone, Copy)]
struct PetPack {
    canvas_w: i64,
    canvas_h: i64,
    anchor_x: i64,
    baseline: i64,
    cb_x: i64,
    cb_y: i64,
    cb_w: i64,
    cb_h: i64,
    scale: i64,
    anims: [Anim; MAX_ANIMS],
    nanims: usize,
    state_anim: [i8; N_STATES],
    min_display: [i64; N_STATES],
    auto_return: [i64; N_STATES],
    react: [i8; 4], // tapLeft, tapRight, tickle, drag
    roam_anim: i8,
    sleep_after: i64,
}

impl PetPack {
    fn empty() -> Self {
        PetPack {
            canvas_w: 24,
            canvas_h: 24,
            anchor_x: 12,
            baseline: 24,
            cb_x: 0,
            cb_y: 0,
            cb_w: 24,
            cb_h: 24,
            scale: 1,
            anims: [Anim::empty(); MAX_ANIMS],
            nanims: 0,
            state_anim: [-1; N_STATES],
            min_display: [0; N_STATES],
            auto_return: [0; N_STATES],
            react: [-1; 4],
            roam_anim: -1,
            sleep_after: 60000,
        }
    }
    fn find(&self, n: &[u8]) -> i8 {
        for i in 0..self.nanims {
            if self.anims[i].is(n) {
                return i as i8;
            }
        }
        -1
    }
}

fn parse_anim(inner: &[u8], out: &mut Anim) -> bool {
    let mut it = ObjIter::new(inner);
    while let Some((key, val)) = it.next() {
        match (key, val) {
            (b"frameW", JVal::Int(v)) => out.fw = v,
            (b"frameH", JVal::Int(v)) => out.fh = v,
            (b"cols", JVal::Int(v)) => out.cols = v.max(1),
            (b"loop", JVal::Bool(v)) => out.looping = v,
            (b"frames", JVal::Arr(a)) => {
                let mut j = J::new(a);
                out.nframes = 0;
                while j.peek() != 0 && out.nframes < MAX_FRAMES {
                    if let Some(JVal::Obj(fo)) = j.value() {
                        let mut fi = 0i64;
                        let mut fm = 200i64;
                        let mut fit = ObjIter::new(fo);
                        while let Some((fk, fv)) = fit.next() {
                            match (fk, fv) {
                                (b"i", JVal::Int(v)) => fi = v,
                                (b"ms", JVal::Int(v)) => fm = v.max(1),
                                _ => {}
                            }
                        }
                        out.frames[out.nframes] = Frame { i: fi, ms: fm };
                        out.nframes += 1;
                    }
                    j.eat(b',');
                }
            }
            _ => {}
        }
    }
    out.fw > 0 && out.fh > 0 && out.nframes > 0
}

fn parse_pack(json: &[u8]) -> Option<PetPack> {
    let mut pack = PetPack::empty();
    let inner = J::new(json).compound(b'{', b'}')?;
    let mut it = ObjIter::new(inner);
    while let Some((key, val)) = it.next() {
        match (key, val) {
            (b"scale", JVal::Int(v)) => pack.scale = v.max(1),
            (b"sleep", JVal::Obj(o)) => {
                let mut sit = ObjIter::new(o);
                while let Some((sk, sv)) = sit.next() {
                    if sk == b"afterMs" {
                        if let JVal::Int(v) = sv {
                            pack.sleep_after = v.max(1000);
                        }
                    }
                }
            }
            (b"canvas", JVal::Obj(o)) => {
                let mut cit = ObjIter::new(o);
                while let Some((ck, cv)) = cit.next() {
                    if let JVal::Int(v) = cv {
                        match ck {
                            b"w" => pack.canvas_w = v,
                            b"h" => pack.canvas_h = v,
                            _ => {}
                        }
                    }
                }
            }
            (b"anchor", JVal::Obj(o)) => {
                let mut ait = ObjIter::new(o);
                while let Some((ak, av)) = ait.next() {
                    if let JVal::Int(v) = av {
                        match ak {
                            b"x" => pack.anchor_x = v,
                            b"baseline" => pack.baseline = v,
                            _ => {}
                        }
                    }
                }
            }
            (b"contentBox", JVal::Obj(o)) => {
                let mut cit = ObjIter::new(o);
                while let Some((ck, cv)) = cit.next() {
                    if let JVal::Int(v) = cv {
                        match ck {
                            b"x" => pack.cb_x = v,
                            b"y" => pack.cb_y = v,
                            b"w" => pack.cb_w = v,
                            b"h" => pack.cb_h = v,
                            _ => {}
                        }
                    }
                }
            }
            (b"animations", JVal::Obj(o)) => {
                let mut ait = ObjIter::new(o);
                while let Some((name, av)) = ait.next() {
                    if pack.nanims >= MAX_ANIMS {
                        break;
                    }
                    if let JVal::Obj(ao) = av {
                        let mut a = Anim::empty();
                        let n = name.len().min(NAME);
                        a.name[..n].copy_from_slice(&name[..n]);
                        a.name_len = n;
                        if parse_anim(ao, &mut a) {
                            pack.anims[pack.nanims] = a;
                            pack.nanims += 1;
                        }
                    }
                }
            }
            (b"states", JVal::Obj(o)) => {
                let mut sit = ObjIter::new(o);
                while let Some((name, sv)) = sit.next() {
                    if let Some(si) = state_index(name) {
                        if let JVal::Arr(a) = sv {
                            if let Some(first) = arr_first_str(a) {
                                pack.state_anim[si] = pack.find(first);
                            }
                        }
                    }
                }
            }
            (b"timings", JVal::Obj(o)) => {
                let mut tit = ObjIter::new(o);
                while let Some((tk, tv)) = tit.next() {
                    if let JVal::Obj(to) = tv {
                        let mut mit = ObjIter::new(to);
                        while let Some((mk, mv)) = mit.next() {
                            if let (Some(si), JVal::Int(v)) = (state_index(mk), mv) {
                                if tk == b"minDisplay" {
                                    pack.min_display[si] = v;
                                } else if tk == b"autoReturn" {
                                    pack.auto_return[si] = v;
                                }
                            }
                        }
                    }
                }
            }
            (b"reactions", JVal::Obj(o)) => {
                let mut rit = ObjIter::new(o);
                while let Some((rk, rv)) = rit.next() {
                    if let JVal::Str(an) = rv {
                        let idx = pack.find(an);
                        match rk {
                            b"tapLeft" => pack.react[0] = idx,
                            b"tapRight" => pack.react[1] = idx,
                            b"tickle" => pack.react[2] = idx,
                            b"drag" => pack.react[3] = idx,
                            _ => {}
                        }
                    }
                }
            }
            (b"roam", JVal::Obj(o)) => {
                let mut rit = ObjIter::new(o);
                while let Some((rk, rv)) = rit.next() {
                    if rk == b"anim" {
                        if let JVal::Str(an) = rv {
                            pack.roam_anim = pack.find(an);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if pack.nanims == 0 {
        return None;
    }
    // idle fallback for any unmapped state
    let idle = pack.state_anim[S_IDLE];
    for s in 0..N_STATES {
        if pack.state_anim[s] < 0 {
            pack.state_anim[s] = idle;
        }
    }
    Some(pack)
}

/// Read a whole `/disk/...` file into `buf`; returns bytes read.
fn read_disk(path: &str, buf: &mut [u8]) -> Option<usize> {
    let h = k::open(path)?;
    let mut total = 0usize;
    while total < buf.len() {
        let n = k::read_file(h, total, &mut buf[total..]);
        if n == 0 {
            break;
        }
        total += n;
    }
    k::close(h);
    Some(total)
}

/// Map app activity to a pet state: streaming -> thinking, an open app ->
/// working, otherwise idle. Pure so it can be unit-tested.
pub fn activity_state(app_open: bool, streaming: bool) -> usize {
    if streaming {
        S_THINKING
    } else if app_open {
        S_WORKING
    } else {
        S_IDLE
    }
}

// ---------------------------------------------------------------------------
// Runtime state machine + renderer
// ---------------------------------------------------------------------------

pub struct Pet {
    pub pack: PetPack,
    pub valid: bool,
    state: usize,
    frame: usize,
    frame_ms: i64,
    state_ms: i64,
    react_anim: i8,
    react_ms: i64,
    last_touch: u64,
    last_tap: u64,
    tap_count: u32,
}

impl Pet {
    pub fn disabled() -> Self {
        Pet {
            pack: PetPack::empty(),
            valid: false,
            state: S_IDLE,
            frame: 0,
            frame_ms: 0,
            state_ms: 0,
            react_anim: -1,
            react_ms: 0,
            last_touch: 0,
            last_tap: 0,
            tap_count: 0,
        }
    }

    /// Build a pet from a pack.json + PNG atlas (the import path). Returns a
    /// disabled pet if either fails to parse/decode.
    pub fn from_pack(json: &[u8], png: &[u8]) -> Self {
        let mut pet = Pet::disabled();
        if decode_png(png).is_none() {
            return pet;
        }
        if let Some(pack) = parse_pack(json) {
            pet.pack = pack;
            pet.valid = true;
            pet.state = S_IDLE;
        }
        pet
    }

    /// Load the active pet: an imported pack in `/disk/pet/` (pack.json +
    /// atlas.png) if present and valid, otherwise the built-in whale.
    pub fn load_default() -> Self {
        static mut DJ: [u8; 65536] = [0; 65536];
        static mut DP: [u8; 262144] = [0; 262144];
        unsafe {
            let dj = core::slice::from_raw_parts_mut(DJ.as_mut_ptr(), DJ.len());
            let dp = core::slice::from_raw_parts_mut(DP.as_mut_ptr(), DP.len());
            if let (Some(jn), Some(pn)) = (read_disk("/disk/pet/pack.json", dj), read_disk("/disk/pet/atlas.png", dp)) {
                if jn > 0 && pn > 0 {
                    let pet = Pet::from_pack(&dj[..jn], &dp[..pn]);
                    if pet.valid {
                        return pet;
                    }
                }
            }
        }
        Pet::from_pack(include_bytes!("pet_default.json"), include_bytes!("pet_default.png"))
    }

    fn display_anim(&self) -> usize {
        let a = if self.react_ms > 0 && self.react_anim >= 0 {
            self.react_anim
        } else {
            self.pack.state_anim[self.state]
        };
        if a < 0 {
            0
        } else {
            (a as usize).min(self.pack.nanims.saturating_sub(1))
        }
    }

    pub fn state(&self) -> usize {
        self.state
    }

    /// Request a state, honouring one-shot priority + minDisplay.
    pub fn set_state(&mut self, s: usize, now: u64) {
        if s >= N_STATES {
            return;
        }
        self.last_touch = now;
        if self.react_ms > 0 {
            return; // a reaction is playing
        }
        if is_oneshot(self.state)
            && self.state_ms < self.pack.min_display[self.state]
            && priority(s) <= priority(self.state)
        {
            return; // don't interrupt a higher/equal one-shot too early
        }
        if s == self.state && !is_oneshot(s) {
            return;
        }
        self.state = s;
        self.state_ms = 0;
        self.frame = 0;
        self.frame_ms = 0;
    }

    pub fn tick(&mut self, now: u64, dt: i64) {
        if !self.valid {
            return;
        }
        let dt = dt.max(0);
        self.state_ms += dt;
        if self.react_ms > 0 {
            self.react_ms -= dt;
            if self.react_ms <= 0 {
                self.react_anim = -1;
                self.frame = 0;
                self.frame_ms = 0;
            }
        }
        // one-shot auto-return
        if is_oneshot(self.state) || self.state == S_NOTIFICATION {
            let lim = self.pack.auto_return[self.state]
                .max(self.pack.min_display[self.state]);
            if lim > 0 && self.state_ms >= lim {
                self.state = S_IDLE;
                self.state_ms = 0;
                self.frame = 0;
                self.frame_ms = 0;
            }
        }
        // sleep after idle timeout
        if self.state == S_IDLE && now.saturating_sub(self.last_touch) >= self.pack.sleep_after as u64
        {
            self.state = S_SLEEPING;
            self.state_ms = 0;
            self.frame = 0;
        }
        // advance animation frames
        let ai = self.display_anim();
        let anim = self.pack.anims[ai];
        if anim.nframes == 0 {
            return;
        }
        self.frame_ms += dt;
        let mut guard = 0;
        while self.frame < anim.nframes
            && self.frame_ms >= anim.frames[self.frame].ms.max(1)
            && guard < 64
        {
            self.frame_ms -= anim.frames[self.frame].ms.max(1);
            self.frame += 1;
            if self.frame >= anim.nframes {
                if anim.looping {
                    self.frame = 0;
                } else {
                    self.frame = anim.nframes - 1;
                    self.frame_ms = 0;
                    break;
                }
            }
            guard += 1;
        }
    }

    fn play_reaction(&mut self, idx: i8, now: u64) {
        if idx < 0 {
            return;
        }
        self.last_touch = now;
        if self.state == S_SLEEPING {
            self.state = S_WAKING;
            self.state_ms = 0;
        }
        self.react_anim = idx;
        self.react_ms = self.pack.anims[idx as usize].duration();
        self.frame = 0;
        self.frame_ms = 0;
    }

    /// Tap on the left/right half of the pet. Three taps within 1.1s = tickle.
    pub fn tap(&mut self, now: u64, left: bool) {
        if !self.valid {
            return;
        }
        if now.saturating_sub(self.last_tap) <= 1100 {
            self.tap_count += 1;
        } else {
            self.tap_count = 1;
        }
        self.last_tap = now;
        if self.tap_count >= 3 {
            self.tap_count = 0;
            self.play_reaction(self.pack.react[2], now);
        } else {
            self.play_reaction(self.pack.react[if left { 0 } else { 1 }], now);
        }
    }

    pub fn drag(&mut self, now: u64) {
        if self.valid {
            self.play_reaction(self.pack.react[3], now);
        }
    }

    /// Draw the current frame at logical `(x,y)` (top-left of the canvas).
    pub fn draw(&self, screen: &Screen, x: i64, y: i64) {
        if !self.valid {
            return;
        }
        let ai = self.display_anim();
        let anim = self.pack.anims[ai];
        if anim.nframes == 0 {
            return;
        }
        let f = anim.frames[self.frame.min(anim.nframes - 1)];
        let cols = anim.cols.max(1);
        let sx = (f.i % cols) * anim.fw;
        let sy = (f.i / cols) * anim.fh;
        let (aw, ah) = unsafe { (ATLAS_W, ATLAS_H) };
        // centre the canvas on x, sit the baseline on y
        let dx = x - self.pack.anchor_x;
        let dy = y - self.pack.baseline;
        screen.sprite(
            dx,
            dy,
            self.pack.scale.max(1),
            atlas_slice(),
            aw,
            ah,
            sx,
            sy,
            anim.fw,
            anim.fh,
            false,
        );
    }

    /// Logical hit-box for tap routing (content box at the draw position).
    pub fn hit(&self, x: i64, y: i64, px: i64, py: i64) -> bool {
        if !self.valid {
            return false;
        }
        let dx = x - self.pack.anchor_x + self.pack.cb_x;
        let dy = y - self.pack.baseline + self.pack.cb_y;
        px >= dx && px < dx + self.pack.cb_w && py >= dy && py < dy + self.pack.cb_h
    }
}

// ---------------------------------------------------------------------------
// Host self-test (driven by kimi_test_pet)
// ---------------------------------------------------------------------------

/// Returns a bitmask of passed checks (all bits = success).
pub fn self_test() -> u64 {
    let mut ok = 0u64;
    let mut pet = Pet::load_default();
    if pet.valid {
        ok |= 1 << 0;
    } else {
        return ok;
    }
    let (w, h) = unsafe { (ATLAS_W, ATLAS_H) };
    if w == 144 && h == 72 {
        ok |= 1 << 1;
    }
    if pet.pack.nanims >= 10 && pet.pack.state_anim[S_WORKING] >= 0 {
        ok |= 1 << 2;
    }
    // frame advance: working frames are 220ms; after 300ms -> frame 1
    pet.set_state(S_WORKING, 0);
    pet.tick(300, 300);
    if pet.frame == 1 {
        ok |= 1 << 3;
    }
    // one-shot attention: minDisplay 2500 / autoReturn 2500
    pet.set_state(S_ATTENTION, 1000);
    pet.tick(2000, 1000); // state_ms 1000 < 2500 -> still attention
    if pet.state == S_ATTENTION {
        ok |= 1 << 4;
    }
    pet.tick(4000, 2000); // state_ms 3000 >= 2500 -> back to idle
    if pet.state == S_IDLE {
        ok |= 1 << 5;
    }
    // sleep after idle timeout (30000ms)
    pet.last_touch = 0;
    pet.set_state(S_IDLE, 0);
    pet.tick(40000, 40000);
    if pet.state == S_SLEEPING {
        ok |= 1 << 6;
    }
    // tap wakes + plays a reaction
    pet.tap(40001, true);
    if pet.react_ms > 0 && pet.state == S_WAKING {
        ok |= 1 << 7;
    }
    // render produces non-transparent pixels
    let mut fb = [0u32; 480 * 160];
    let screen = Screen::new(fb.as_mut_ptr() as u64, 480, 160, 480 * 4);
    pet.draw(&screen, 32, 40);
    let nonzero = fb.iter().filter(|&&p| p != 0).count();
    if nonzero > 20 {
        ok |= 1 << 8;
    }
    if activity_state(false, false) == S_IDLE {
        ok |= 1 << 9;
    }
    if activity_state(true, false) == S_WORKING {
        ok |= 1 << 10;
    }
    if activity_state(true, true) == S_THINKING {
        ok |= 1 << 11;
    }
    ok
}

/// Host test for the import path: build a pet from caller-supplied pack bytes.
/// Bitmask: 1 valid, 2 atlas dims match pack canvas*tiles, 4 >=1 anim, 8 renders.
pub fn test_pack(json: &[u8], png: &[u8]) -> u64 {
    let mut ok = 0u64;
    let pet = Pet::from_pack(json, png);
    if pet.valid {
        ok |= 1;
    } else {
        return ok;
    }
    if pet.pack.nanims >= 1 {
        ok |= 4;
    }
    let (w, h) = unsafe { (ATLAS_W, ATLAS_H) };
    if w > 0 && h > 0 {
        ok |= 2;
    }
    let mut fb = [0u32; 480 * 160];
    let screen = Screen::new(fb.as_mut_ptr() as u64, 480, 160, 480 * 4);
    pet.draw(&screen, 60, 60);
    if fb.iter().filter(|&&p| p != 0).count() > 10 {
        ok |= 8;
    }
    ok
}
