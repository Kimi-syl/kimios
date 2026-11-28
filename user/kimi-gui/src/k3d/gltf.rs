//! glTF 2.0 / VRM loader: streaming, fixed arenas, no allocator.
//!
//! A `.glb`/`.vrm` is read through [`ReadAt`] only. The container header is
//! walked, then the JSON chunk is tokenised *in place* through a small sliding
//! window, and binary data is fetched by absolute offset exactly for the
//! accessors the model uses. Nothing ever assumes the file (or the JSON chunk)
//! is resident in RAM, so a 10 MB VRM costs only the windows below.
//!
//! Two JSON passes are made. The first fills the index tables (buffer views,
//! accessors, nodes, skins, materials, textures, images) plus the VRM humanoid
//! map; glTF does not require those to be declared before `meshes`, so the
//! geometry pass runs afterwards with every index table already validated.
//!
//! Memory: everything lives in fixed arrays. The one ambient cache is the PNG
//! scratch (two `static` buffers, ~1.2 MiB, `SharedBuf`) because the inflate
//! routine in `libkimi` takes a whole output buffer; it is guarded by
//! `PNG_MAX_DIM` and reused, never reallocated. The loader is therefore not
//! re-entrant, which is fine for the single-threaded GUI.
//!
//! Limits / deliberately unsupported (each fails soft or with a `LoadError`):
//! - interlaced PNG, colour types other than RGB(2)/RGBA(6), 16-bit channels,
//!   source images larger than `PNG_MAX_DIM` (texture is skipped, the material
//!   keeps its base colour);
//! - JPEG / external-URI images (texture skipped);
//! - sparse accessors, morph targets, animations, MToon shading (materials get
//!   a flat "unlit" flag instead), non-triangle primitive modes (skipped);
//! - more than `MAX_VERTS`/`MAX_TRIS`/`MAX_NODES`/... -> `LoadError::ArenaFull`.
//!
//! Every failure path returns a [`LoadError`]; there is no `unwrap`, no
//! indexing that a bounds check cannot prove safe, and no arithmetic that can
//! overflow silently (all offset maths is `saturating_*`).

use core::cell::UnsafeCell;

use super::model::{
    AlphaMode, LoadError, Material, ModelArena, Node, Primitive, ReadAt, Skin, Texture, BONE_NONE,
    MAX_JOINTS, MAX_MATERIALS, MAX_NODES, MAX_PRIMS, MAX_TEXTURES, MAX_TRIS, MAX_VERTS,
    NO_TEXTURE, TEX_MAX_DIM,
};
use super::skin::{
    BONE_CHEST, BONE_HEAD, BONE_HIPS, BONE_LEFT_FOOT, BONE_LEFT_HAND, BONE_LEFT_LOWER_ARM,
    BONE_LEFT_LOWER_LEG, BONE_LEFT_SHOULDER, BONE_LEFT_UPPER_ARM, BONE_LEFT_UPPER_LEG, BONE_NECK,
    BONE_RIGHT_FOOT, BONE_RIGHT_HAND, BONE_RIGHT_LOWER_ARM, BONE_RIGHT_LOWER_LEG,
    BONE_RIGHT_SHOULDER, BONE_RIGHT_UPPER_ARM, BONE_RIGHT_UPPER_LEG, BONE_SPINE,
};
use super::{sqrt, Vec3};
use libkimi::apk::inflate;

// ---------------------------------------------------------------------------
// Loader-internal fixed capacities
// ---------------------------------------------------------------------------

/// bufferViews kept. Accessors may refer to views beyond this; such accessors
/// are then reported as `Unsupported` instead of failing the whole load.
const MAX_VIEWS: usize = 256;
/// accessors kept. Morph-target accessors typically sit after the geometry
/// ones, so dropping the tail loses nothing the loader needs.
const MAX_ACC: usize = 384;
/// images kept (each is one PNG blob).
const MAX_IMAGES: usize = 8;
/// Flat node->children storage (each parent's list is contiguous).
const MAX_CHILD: usize = 512;
/// Humanoid bone assignments collected before nodes are indexed.
const MAX_BONE_PAIRS: usize = 48;
/// Longest JSON key we compare against (longer keys are truncated -> skipped).
const KEY_MAX: usize = 40;
/// JSON nesting budget; deeper documents are rejected rather than recursing.
const MAX_DEPTH: u32 = 32;
/// Bytes of JSON kept in the sliding window.
const JSON_WIN: usize = 1024;
/// Source PNG edge limit (the decoded texture is box-downsampled to <=128).
/// 384 matches the sprite-atlas ceiling `pet.rs` already spends memory on; a
/// larger source would need a much bigger inflate buffer and is skipped.
const PNG_MAX_DIM: usize = 384;
/// Compressed IDAT bytes buffered per image.
const PNG_IDAT_CAP: usize = 640_000;
/// Inflated PNG bytes buffered per image (384 * (1 + 384*4) = 590_208).
const PNG_RAW_CAP: usize = PNG_MAX_DIM * (1 + PNG_MAX_DIM * 4);

/// One `static` buffer that is only touched from the single GUI thread.
struct SharedBuf<const N: usize>(UnsafeCell<[u8; N]>);

// SAFETY: the loader is called from the one GUI thread; the buffers are pure
// scratch whose contents never outlive one `decode_png` call.
unsafe impl<const N: usize> Sync for SharedBuf<N> {}

impl<const N: usize> SharedBuf<N> {
    fn get(&self) -> &mut [u8] {
        unsafe { &mut *self.0.get() }
    }
}

static PNG_IDAT: SharedBuf<PNG_IDAT_CAP> = SharedBuf(UnsafeCell::new([0; PNG_IDAT_CAP]));
static PNG_RAW: SharedBuf<PNG_RAW_CAP> = SharedBuf(UnsafeCell::new([0; PNG_RAW_CAP]));

// ---------------------------------------------------------------------------
// GLB container
// ---------------------------------------------------------------------------

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

fn read_exact(r: &mut dyn ReadAt, off: u64, out: &mut [u8]) -> Result<(), LoadError> {
    if r.read_at(off, out) == out.len() {
        Ok(())
    } else {
        Err(LoadError::Io)
    }
}

fn read_u32_le(r: &mut dyn ReadAt, off: u64) -> Result<u32, LoadError> {
    let mut b = [0u8; 4];
    read_exact(r, off, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// Where the JSON and BIN chunks live inside the file.
struct Glb {
    json_off: u64,
    json_len: u64,
    bin_off: u64,
    bin_len: u64,
}

fn parse_glb(r: &mut dyn ReadAt, size: u64) -> Result<Glb, LoadError> {
    // 12-byte header + at least one 8-byte chunk header.
    if size < 20 {
        return Err(LoadError::BadContainer);
    }
    let mut hdr = [0u8; 12];
    read_exact(r, 0, &mut hdr)?;
    let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let version = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    let length = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as u64;
    if magic != GLB_MAGIC || version != GLB_VERSION {
        return Err(LoadError::BadContainer);
    }
    if length < 12 || length > size {
        return Err(LoadError::BadContainer);
    }

    let mut glb = Glb {
        json_off: 0,
        json_len: 0,
        bin_off: 0,
        bin_len: 0,
    };
    let mut off = 12u64;
    while off + 8 <= length {
        let clen = read_u32_le(r, off)? as u64;
        let ctype = read_u32_le(r, off + 4)?;
        let data = off + 8;
        if data.saturating_add(clen) > length {
            return Err(LoadError::BadContainer);
        }
        if ctype == CHUNK_JSON {
            if glb.json_len == 0 && clen > 0 {
                glb.json_off = data;
                glb.json_len = clen;
            }
        } else if ctype == CHUNK_BIN {
            if glb.bin_len == 0 {
                glb.bin_off = data;
                glb.bin_len = clen;
            }
        }
        off = data.saturating_add(clen);
    }
    if glb.json_len < 2 {
        return Err(LoadError::BadContainer);
    }
    Ok(glb)
}

// ---------------------------------------------------------------------------
// Streaming JSON tokeniser
// ---------------------------------------------------------------------------

/// Pull parser over the JSON chunk. Only one window of bytes is resident; every
/// byte is consumed exactly once and the cursor only moves forward. Because the
/// underlying [`ReadAt`] is positional, the tokeniser can also lend its reader
/// out (`s.r`) to fetch binary data between tokens without disturbing the
/// window.
struct Src<'r> {
    r: &'r mut dyn ReadAt,
    buf: [u8; JSON_WIN],
    /// Absolute file offset of `buf[0]`.
    buf_abs: u64,
    /// Next unread index in `buf`.
    pos: usize,
    /// Valid bytes in `buf`.
    have: usize,
    /// Absolute end of the JSON chunk.
    end: u64,
}

impl<'r> Src<'r> {
    fn new(r: &'r mut dyn ReadAt, base: u64, len: u64) -> Self {
        let mut s = Src {
            r,
            buf: [0; JSON_WIN],
            buf_abs: base,
            pos: 0,
            have: 0,
            end: base.saturating_add(len),
        };
        s.fill();
        s
    }

    /// Slide/refill the window. Only meaningful when `pos == have`.
    fn fill(&mut self) {
        if self.pos < self.have {
            return;
        }
        self.buf_abs = self.buf_abs.saturating_add(self.have as u64);
        self.pos = 0;
        self.have = 0;
        if self.buf_abs >= self.end {
            return;
        }
        let mut want = JSON_WIN as u64;
        let left = self.end - self.buf_abs;
        if want > left {
            want = left;
        }
        let n = self.r.read_at(self.buf_abs, &mut self.buf[..want as usize]);
        self.have = if n > want as usize { want as usize } else { n };
    }

    fn peek(&mut self) -> Option<u8> {
        if self.pos >= self.have {
            self.fill();
            if self.pos >= self.have {
                return None;
            }
        }
        Some(self.buf[self.pos])
    }

    fn adv(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn ws(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') => self.pos += 1,
                _ => return,
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), LoadError> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(LoadError::BadJson)
        }
    }

    fn lit(&mut self, s: &[u8]) -> Result<(), LoadError> {
        for &want in s {
            if self.adv() != Some(want) {
                return Err(LoadError::BadJson);
            }
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, LoadError> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.adv().ok_or(LoadError::BadJson)?;
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a') as u32 + 10,
                b'A'..=b'F' => (c - b'A') as u32 + 10,
                _ => return Err(LoadError::BadJson),
            };
            v = (v << 4) | d;
        }
        Ok(v)
    }

    /// Consume one JSON string, copying at most `out.len()` decoded bytes.
    /// Longer strings are consumed but truncated; callers only need short
    /// semantic names.
    fn string_into(&mut self, out: &mut [u8]) -> Result<usize, LoadError> {
        self.expect(b'"')?;
        let mut n = 0usize;
        loop {
            let c = self.adv().ok_or(LoadError::BadJson)?;
            match c {
                b'"' => return Ok(n),
                b'\\' => {
                    let e = self.adv().ok_or(LoadError::BadJson)?;
                    match e {
                        b'"' | b'\\' | b'/' => {
                            if n < out.len() {
                                out[n] = e;
                                n += 1;
                            }
                        }
                        b'b' | b'f' | b'n' | b'r' | b't' => {
                            let d = match e {
                                b'b' => 0x08,
                                b'f' => 0x0c,
                                b'n' => b'\n',
                                b'r' => b'\r',
                                _ => b'\t',
                            };
                            if n < out.len() {
                                out[n] = d;
                                n += 1;
                            }
                        }
                        b'u' => {
                            let cp = self.hex4()?;
                            let mut tmp = [0u8; 3];
                            let m = if cp < 0x80 {
                                tmp[0] = cp as u8;
                                1
                            } else if cp < 0x800 {
                                tmp[0] = 0xC0 | (cp >> 6) as u8;
                                tmp[1] = 0x80 | (cp & 0x3F) as u8;
                                2
                            } else {
                                tmp[0] = 0xE0 | (cp >> 12) as u8;
                                tmp[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
                                tmp[2] = 0x80 | (cp & 0x3F) as u8;
                                3
                            };
                            for &b in tmp.iter().take(m) {
                                if n < out.len() {
                                    out[n] = b;
                                    n += 1;
                                }
                            }
                        }
                        _ => return Err(LoadError::BadJson),
                    }
                }
                0x00..=0x1f => return Err(LoadError::BadJson),
                _ => {
                    if n < out.len() {
                        out[n] = c;
                        n += 1;
                    }
                }
            }
        }
    }

    /// Parse a JSON number as `f32` (exponents included; this is the reason
    /// pet.rs's integer-only reader cannot be reused).
    fn number_f32(&mut self) -> Result<f32, LoadError> {
        self.ws();
        let mut neg = false;
        match self.peek() {
            Some(b'-') => {
                neg = true;
                self.pos += 1;
            }
            Some(b'+') => self.pos += 1,
            _ => {}
        }
        let mut mant = 0.0f64;
        let mut any = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                mant = mant * 10.0 + (c - b'0') as f64;
                any = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        let mut frac = 0.0f64;
        let mut scale = 1.0f64;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    frac = frac * 10.0 + (c - b'0') as f64;
                    scale *= 10.0;
                    any = true;
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if !any {
            return Err(LoadError::BadJson);
        }
        let mut exp: i32 = 0;
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            let mut eneg = false;
            match self.peek() {
                Some(b'-') => {
                    eneg = true;
                    self.pos += 1;
                }
                Some(b'+') => self.pos += 1,
                _ => {}
            }
            let mut ev: i32 = 0;
            let mut any_e = false;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    if ev < 100_000 {
                        ev = ev * 10 + (c - b'0') as i32;
                    }
                    any_e = true;
                    self.pos += 1;
                } else {
                    break;
                }
            }
            if !any_e {
                return Err(LoadError::BadJson);
            }
            exp = if eneg { -ev } else { ev };
        }
        let mut v = mant + frac / scale;
        let mut e = exp.clamp(-400, 400);
        while e > 0 {
            v *= 10.0;
            e -= 1;
        }
        while e < 0 {
            v /= 10.0;
            e += 1;
        }
        if neg {
            v = -v;
        }
        Ok(v as f32)
    }

    /// Parse a JSON number as `i64`, truncating any fractional part.
    fn number_i64(&mut self) -> Result<i64, LoadError> {
        self.ws();
        let mut neg = false;
        match self.peek() {
            Some(b'-') => {
                neg = true;
                self.pos += 1;
            }
            Some(b'+') => self.pos += 1,
            _ => {}
        }
        let mut v: i64 = 0;
        let mut any = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                v = v
                    .saturating_mul(10)
                    .saturating_add((c - b'0') as i64);
                any = true;
                self.pos += 1;
            } else {
                break;
            }
        }
        if !any {
            return Err(LoadError::BadJson);
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'-') | Some(b'+')) {
                self.pos += 1;
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        Ok(if neg { -v } else { v })
    }

    fn bool_val(&mut self) -> Result<bool, LoadError> {
        self.ws();
        match self.peek() {
            Some(b't') => {
                self.lit(b"true")?;
                Ok(true)
            }
            Some(b'f') => {
                self.lit(b"false")?;
                Ok(false)
            }
            _ => Err(LoadError::BadJson),
        }
    }

    /// Read one element of an object, returning its key length. `None` means
    /// the closing brace was consumed.
    fn obj_key(&mut self, key: &mut [u8]) -> Result<Option<usize>, LoadError> {
        self.ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(None);
        }
        if self.eat(b',') {
            self.ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(None);
            }
        }
        if self.peek() != Some(b'"') {
            return Err(LoadError::BadJson);
        }
        let n = self.string_into(key)?;
        self.expect(b':')?;
        Ok(Some(n))
    }

    /// Advance to the next array element, consuming `]` when done.
    fn arr_next(&mut self) -> Result<bool, LoadError> {
        self.ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(false);
        }
        if self.eat(b',') {
            self.ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(false);
            }
        }
        if self.peek().is_none() {
            return Err(LoadError::BadJson);
        }
        Ok(true)
    }

    fn skip_value(&mut self, depth: u32) -> Result<(), LoadError> {
        if depth == 0 {
            return Err(LoadError::BadJson);
        }
        self.ws();
        match self.peek() {
            Some(b'"') => {
                let mut sink = [0u8; 8];
                self.string_into(&mut sink)?;
                Ok(())
            }
            Some(b'{') => {
                self.pos += 1;
                let mut key = [0u8; KEY_MAX];
                loop {
                    if self.obj_key(&mut key)?.is_none() {
                        break;
                    }
                    self.skip_value(depth - 1)?;
                }
                Ok(())
            }
            Some(b'[') => {
                self.pos += 1;
                while self.arr_next()? {
                    self.skip_value(depth - 1)?;
                }
                Ok(())
            }
            Some(b't') => self.lit(b"true"),
            Some(b'f') => self.lit(b"false"),
            Some(b'n') => self.lit(b"null"),
            Some(_) => {
                self.number_f32()?;
                Ok(())
            }
            None => Err(LoadError::BadJson),
        }
    }
}

// Small typed readers shared by the parse routines.

fn num_u64(s: &mut Src) -> Result<u64, LoadError> {
    let v = s.number_i64()?;
    Ok(if v < 0 { 0 } else { v as u64 })
}

fn num_u32(s: &mut Src) -> Result<u32, LoadError> {
    let v = s.number_i64()?;
    Ok(if v < 0 {
        0
    } else if v > u32::MAX as i64 {
        u32::MAX
    } else {
        v as u32
    })
}

/// Parse exactly-fits array of floats, tolerating a different length.
fn f32_array(s: &mut Src, out: &mut [f32]) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut i = 0usize;
    while s.arr_next()? {
        let v = s.number_f32()?;
        if i < out.len() {
            out[i] = v;
        }
        i += 1;
    }
    Ok(())
}

fn fabs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// JSON index tables
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct View {
    off: u64,
    len: u64,
    /// `byteStride`, or 0 for a tightly packed view.
    stride: u32,
}

#[derive(Clone, Copy)]
struct Acc {
    count: u32,
    off: u64,
    view: i16,
    comp: u16,
    /// 0 = invalid/unmapped type; otherwise component count.
    comps: u8,
    normalized: bool,
    sparse: bool,
}

#[derive(Clone, Copy)]
struct Img {
    view: i16,
    valid: bool,
}

#[derive(Clone, Copy)]
struct BonePair {
    node: u16,
    bone: u8,
    /// True for names that are a *fallback* for a slot already taken (e.g.
    /// `upperChest` vs `chest`); applied only where the slot is still free.
    fallback: bool,
}

/// Everything the loader remembers between the two JSON passes.
struct State {
    views: [View; MAX_VIEWS],
    nviews: usize,
    views_total: usize,
    accs: [Acc; MAX_ACC],
    naccs: usize,
    accs_total: usize,
    images: [Img; MAX_IMAGES],
    nimages: usize,
    /// texture slot -> image index.
    tex_src: [i16; MAX_TEXTURES],
    ntex: usize,
    child_off: [u16; MAX_NODES],
    child_num: [u16; MAX_NODES],
    children: [u16; MAX_CHILD],
    nchildren: usize,
    bone_pairs: [BonePair; MAX_BONE_PAIRS],
    nbone: usize,
    /// Accessor index holding the skin's inverse bind matrices, or -1.
    skin_ibm: i32,
}

const EMPTY_VIEW: View = View {
    off: 0,
    len: 0,
    stride: 0,
};
const EMPTY_ACC: Acc = Acc {
    count: 0,
    off: 0,
    view: -1,
    comp: 0,
    comps: 0,
    normalized: false,
    sparse: false,
};
const EMPTY_IMG: Img = Img {
    view: -1,
    valid: false,
};
const EMPTY_BONE: BonePair = BonePair {
    node: 0,
    bone: 0,
    fallback: false,
};

impl State {
    fn new() -> State {
        State {
            views: [EMPTY_VIEW; MAX_VIEWS],
            nviews: 0,
            views_total: 0,
            accs: [EMPTY_ACC; MAX_ACC],
            naccs: 0,
            accs_total: 0,
            images: [EMPTY_IMG; MAX_IMAGES],
            nimages: 0,
            tex_src: [-1; MAX_TEXTURES],
            ntex: 0,
            child_off: [0; MAX_NODES],
            child_num: [0; MAX_NODES],
            children: [0; MAX_CHILD],
            nchildren: 0,
            bone_pairs: [EMPTY_BONE; MAX_BONE_PAIRS],
            nbone: 0,
            skin_ibm: -1,
        }
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a GLB from `read` into `arena`.
///
/// `size` is the byte length of the file. On error the arena is left with
/// `loaded == false` and must not be drawn from.
pub fn load(read: &mut dyn ReadAt, size: u64, arena: &mut ModelArena) -> Result<(), LoadError> {
    arena.reset();
    if size < 20 {
        return Err(LoadError::BadContainer);
    }
    let glb = parse_glb(read, size)?;
    if glb.json_off.saturating_add(glb.json_len) > size {
        return Err(LoadError::BadContainer);
    }

    let mut st = State::new();

    // Pass 1: index tables (and the humanoid map, which does not need nodes to
    // exist yet because node indices are just integers).
    {
        let mut s = Src::new(&mut *read, glb.json_off, glb.json_len);
        parse_pass1(&mut s, &mut st, arena)?;
    }

    resolve_parents(&st, arena)?;
    apply_bones(&st, arena);
    validate_joints(&st, arena)?;
    read_ibm(read, &st, arena, &glb)?;
    decode_textures(read, &st, arena, &glb);

    // The rasteriser needs at least one material slot to draw anything.
    if arena.material_count == 0 {
        arena.materials[0] = Material::default();
        arena.material_count = 1;
    }
    finalize_materials(arena);

    // Pass 2: geometry. Accessor nodes point into the validated tables.
    {
        let mut s = Src::new(&mut *read, glb.json_off, glb.json_len);
        parse_pass2(&mut s, &st, arena, &glb)?;
    }

    if arena.primitive_count == 0 || arena.vertex_count == 0 {
        return Err(LoadError::Unsupported);
    }
    arena.recompute_bounds();
    arena.loaded = true;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pass 1: tables
// ---------------------------------------------------------------------------

fn parse_pass1(s: &mut Src, st: &mut State, arena: &mut ModelArena) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"bufferViews" => parse_buffer_views(s, st)?,
            b"accessors" => parse_accessors(s, st)?,
            b"nodes" => parse_nodes(s, st, arena)?,
            b"skins" => parse_skins(s, st, arena)?,
            b"materials" => parse_materials(s, arena)?,
            b"textures" => parse_textures(s, st)?,
            b"images" => parse_images(s, st)?,
            b"extensions" => parse_extensions(s, st)?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

fn parse_buffer_views(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        let mut v = EMPTY_VIEW;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"byteOffset" => v.off = num_u64(s)?,
                b"byteLength" => v.len = num_u64(s)?,
                b"byteStride" => v.stride = num_u32(s)?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        let idx = st.views_total;
        st.views_total = st.views_total.saturating_add(1);
        if idx < MAX_VIEWS {
            st.views[idx] = v;
            if idx + 1 > st.nviews {
                st.nviews = idx + 1;
            }
        }
    }
    Ok(())
}

fn parse_accessors(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        let mut a = EMPTY_ACC;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"bufferView" => {
                    let v = s.number_i64()?;
                    a.view = if v >= 0 && v <= i16::MAX as i64 {
                        v as i16
                    } else {
                        -1
                    };
                }
                b"byteOffset" => a.off = num_u64(s)?,
                b"componentType" => a.comp = num_u32(s)?.min(u16::MAX as u32) as u16,
                b"count" => a.count = num_u32(s)?,
                b"type" => {
                    let mut t = [0u8; 8];
                    let m = s.string_into(&mut t)?;
                    a.comps = match &t[..m] {
                        b"SCALAR" => 1,
                        b"VEC2" => 2,
                        b"VEC3" => 3,
                        b"VEC4" => 4,
                        b"MAT2" => 4,
                        b"MAT3" => 9,
                        b"MAT4" => 16,
                        _ => 0,
                    };
                }
                b"normalized" => a.normalized = s.bool_val()?,
                b"sparse" => {
                    a.sparse = true;
                    s.skip_value(MAX_DEPTH)?;
                }
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        let idx = st.accs_total;
        st.accs_total = st.accs_total.saturating_add(1);
        if idx < MAX_ACC {
            st.accs[idx] = a;
            if idx + 1 > st.naccs {
                st.naccs = idx + 1;
            }
        }
    }
    Ok(())
}

fn parse_nodes(s: &mut Src, st: &mut State, arena: &mut ModelArena) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        let idx = arena.node_count;
        if idx >= MAX_NODES {
            return Err(LoadError::ArenaFull);
        }
        let mut node = Node::default();
        let mut child_off = 0usize;
        let mut child_num = 0usize;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"translation" => f32_array(s, &mut node.translation)?,
                b"rotation" => f32_array(s, &mut node.rotation)?,
                b"scale" => f32_array(s, &mut node.scale)?,
                // Plain glTF exports (e.g. Blender) carry no VRM humanoid map,
                // so fall back to matching the node name against the standard
                // bone names. This is what lets a non-VRM rig be posed.
                b"name" => {
                    let mut name = [0u8; 64];
                    let nn = s.string_into(&mut name)?;
                    if let Some((b, _fallback)) = bone_slot(&name[..nn]) {
                        node.bone = b;
                    }
                }
                b"matrix" => {
                    let mut m = [0.0f32; 16];
                    f32_array(s, &mut m)?;
                    decompose_matrix(&mut node, &m);
                }
                b"mesh" => {
                    let v = s.number_i64()?;
                    node.mesh = if v >= 0 && v <= i16::MAX as i64 {
                        v as i16
                    } else {
                        -1
                    };
                }
                b"skin" => {
                    let v = s.number_i64()?;
                    node.skin = if v >= 0 && v <= i16::MAX as i64 {
                        v as i16
                    } else {
                        -1
                    };
                }
                b"children" => {
                    child_off = st.nchildren;
                    s.expect(b'[')?;
                    child_num = 0;
                    while s.arr_next()? {
                        let v = s.number_i64()?;
                        if v < 0 || v > u16::MAX as i64 {
                            return Err(LoadError::BadJson);
                        }
                        if st.nchildren >= MAX_CHILD {
                            return Err(LoadError::ArenaFull);
                        }
                        st.children[st.nchildren] = v as u16;
                        st.nchildren += 1;
                        child_num += 1;
                    }
                }
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        st.child_off[idx] = child_off as u16;
        st.child_num[idx] = child_num as u16;
        arena.nodes[idx] = node;
        arena.node_count = idx + 1;
    }
    Ok(())
}

/// Decompose a column-major glTF node matrix into T/R/S (the arena stores TRS).
fn decompose_matrix(node: &mut Node, m: &[f32; 16]) {
    let c0 = Vec3::new(m[0], m[1], m[2]);
    let c1 = Vec3::new(m[4], m[5], m[6]);
    let c2 = Vec3::new(m[8], m[9], m[10]);
    let mut sx = c0.length();
    let sy = c1.length();
    let sz = c2.length();
    // A mirrored basis is folded into the X scale so the rotation stays proper.
    if c0.cross(c1).dot(c2) < 0.0 {
        sx = -sx;
    }
    let isx = if fabs(sx) > 1e-20 { 1.0 / sx } else { 1.0 };
    let isy = if sy > 1e-20 { 1.0 / sy } else { 1.0 };
    let isz = if sz > 1e-20 { 1.0 / sz } else { 1.0 };
    node.translation = [m[12], m[13], m[14]];
    node.scale = [sx, sy, sz];
    node.rotation = quat_from_basis(
        m[0] * isx,
        m[1] * isx,
        m[2] * isx,
        m[4] * isy,
        m[5] * isy,
        m[6] * isy,
        m[8] * isz,
        m[9] * isz,
        m[10] * isz,
    );
}

/// Rotation part (column vectors) -> quaternion xyzw, via the trace method.
#[allow(clippy::too_many_arguments)]
fn quat_from_basis(
    r00: f32,
    r10: f32,
    r20: f32,
    r01: f32,
    r11: f32,
    r21: f32,
    r02: f32,
    r12: f32,
    r22: f32,
) -> [f32; 4] {
    let trace = r00 + r11 + r22;
    let (x, y, z, w, s);
    if trace > 0.0 {
        s = sqrt(trace + 1.0) * 2.0;
        if s <= 0.0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        w = 0.25 * s;
        x = (r21 - r12) / s;
        y = (r02 - r20) / s;
        z = (r10 - r01) / s;
    } else if r00 > r11 && r00 > r22 {
        s = sqrt(1.0 + r00 - r11 - r22) * 2.0;
        if s <= 0.0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        w = (r21 - r12) / s;
        x = 0.25 * s;
        y = (r01 + r10) / s;
        z = (r02 + r20) / s;
    } else if r11 > r22 {
        s = sqrt(1.0 + r11 - r00 - r22) * 2.0;
        if s <= 0.0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        w = (r02 - r20) / s;
        x = (r01 + r10) / s;
        y = 0.25 * s;
        z = (r12 + r21) / s;
    } else {
        s = sqrt(1.0 + r22 - r00 - r11) * 2.0;
        if s <= 0.0 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        w = (r10 - r01) / s;
        x = (r02 + r20) / s;
        y = (r12 + r21) / s;
        z = 0.25 * s;
    }
    let n = sqrt(x * x + y * y + z * z + w * w);
    if n > 1e-20 {
        [x / n, y / n, z / n, w / n]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn parse_skins(s: &mut Src, st: &mut State, arena: &mut ModelArena) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut index = 0usize;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        if index > 0 || arena.skin_count > 0 {
            // The arena holds one skin; extra skins are skipped.
            s.skip_value(MAX_DEPTH)?;
            index += 1;
            continue;
        }
        let first_joint = arena.joint_count;
        let mut joint_num = 0usize;
        let mut ibm: i64 = -1;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"joints" => {
                    s.expect(b'[')?;
                    while s.arr_next()? {
                        let v = s.number_i64()?;
                        if v < 0 || v > u16::MAX as i64 {
                            return Err(LoadError::BadJson);
                        }
                        if arena.joint_count >= MAX_JOINTS {
                            return Err(LoadError::ArenaFull);
                        }
                        arena.joint_nodes[arena.joint_count] = v as u16;
                        arena.joint_count += 1;
                        joint_num += 1;
                    }
                }
                b"inverseBindMatrices" => ibm = s.number_i64()?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        if joint_num > 0 {
            arena.skins[0] = Skin {
                first_joint: first_joint as u16,
                joint_count: joint_num as u16,
                first_ibm: first_joint as u16,
            };
            arena.skin_count = 1;
        }
        st.skin_ibm = if ibm >= 0 && ibm <= i32::MAX as i64 {
            ibm as i32
        } else {
            -1
        };
        index += 1;
    }
    Ok(())
}

fn parse_materials(s: &mut Src, arena: &mut ModelArena) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut index = 0usize;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        if index >= MAX_MATERIALS {
            s.skip_value(MAX_DEPTH)?;
            index += 1;
            continue;
        }
        let mut m = Material::default();
        let mut tex: i64 = -1;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"pbrMetallicRoughness" => {
                    // The spec location: base colour lives in here.
                    s.expect(b'{')?;
                    let mut k2 = [0u8; KEY_MAX];
                    loop {
                        let m2 = match s.obj_key(&mut k2)? {
                            Some(m2) => m2,
                            None => break,
                        };
                        match &k2[..m2] {
                            b"baseColorFactor" => f32_array(s, &mut m.base_color)?,
                            b"baseColorTexture" => tex = tex_ref(s)?,
                            _ => s.skip_value(MAX_DEPTH)?,
                        }
                    }
                }
                b"baseColorFactor" => f32_array(s, &mut m.base_color)?,
                b"baseColorTexture" => tex = tex_ref(s)?,
                b"alphaMode" => {
                    let mut t = [0u8; 8];
                    let tn = s.string_into(&mut t)?;
                    m.alpha_mode = match &t[..tn] {
                        b"MASK" => AlphaMode::Mask,
                        b"BLEND" => AlphaMode::Blend,
                        _ => AlphaMode::Opaque,
                    };
                }
                b"alphaCutoff" => m.alpha_cutoff = s.number_f32()?,
                b"doubleSided" => m.double_sided = s.bool_val()?,
                b"extensions" => parse_material_extensions(s, &mut m)?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        m.texture = if tex >= 0 && tex <= u16::MAX as i64 {
            tex as u16
        } else {
            NO_TEXTURE
        };
        arena.materials[index] = m;
        arena.material_count = index + 1;
        index += 1;
    }
    Ok(())
}

/// Parse a `{ "index": N, "texCoord": 0 }` texture reference.
fn tex_ref(s: &mut Src) -> Result<i64, LoadError> {
    let mut tex: i64 = -1;
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"index" => tex = s.number_i64()?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(tex)
}

fn parse_material_extensions(s: &mut Src, m: &mut Material) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            // Flat-shaded fallbacks: the rasteriser's `unlit` path.
            b"KHR_materials_unlit" | b"VRMC_materials_mtoon" => {
                m.unlit = true;
                s.skip_value(MAX_DEPTH)?;
            }
            b"VRM" => {
                m.unlit = true;
                s.skip_value(MAX_DEPTH)?;
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

fn parse_textures(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut index = 0usize;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        if index >= MAX_TEXTURES {
            s.skip_value(MAX_DEPTH)?;
            index += 1;
            continue;
        }
        let mut source: i64 = -1;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"source" => source = s.number_i64()?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        st.tex_src[index] = if source >= 0 && source <= i16::MAX as i64 {
            source as i16
        } else {
            -1
        };
        st.ntex = index + 1;
        index += 1;
    }
    Ok(())
}

fn parse_images(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut index = 0usize;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        if index >= MAX_IMAGES {
            s.skip_value(MAX_DEPTH)?;
            index += 1;
            continue;
        }
        let mut img = Img {
            view: -1,
            valid: true,
        };
        let mut mime_png = true;
        let mut has_mime = false;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"bufferView" => {
                    let v = s.number_i64()?;
                    img.view = if v >= 0 && v <= i16::MAX as i64 {
                        v as i16
                    } else {
                        -1
                    };
                }
                b"mimeType" => {
                    let mut t = [0u8; 16];
                    let tn = s.string_into(&mut t)?;
                    has_mime = true;
                    mime_png = &t[..tn] == b"image/png";
                }
                // External and data-URI images are not supported.
                b"uri" => {
                    img.valid = false;
                    s.skip_value(MAX_DEPTH)?;
                }
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        if has_mime && !mime_png {
            img.valid = false;
        }
        if img.view < 0 {
            img.valid = false;
        }
        st.images[index] = img;
        st.nimages = index + 1;
        index += 1;
    }
    Ok(())
}

// --- VRM humanoid map -------------------------------------------------------

fn bone_slot(name: &[u8]) -> Option<(u8, bool)> {
    Some(match name {
        b"hips" => (BONE_HIPS, false),
        b"spine" => (BONE_SPINE, false),
        b"chest" => (BONE_CHEST, false),
        b"upperChest" => (BONE_CHEST, true),
        b"neck" => (BONE_NECK, false),
        b"head" => (BONE_HEAD, false),
        b"leftShoulder" => (BONE_LEFT_SHOULDER, false),
        b"leftUpperArm" => (BONE_LEFT_UPPER_ARM, false),
        b"leftLowerArm" => (BONE_LEFT_LOWER_ARM, false),
        b"leftHand" => (BONE_LEFT_HAND, false),
        b"rightShoulder" => (BONE_RIGHT_SHOULDER, false),
        b"rightUpperArm" => (BONE_RIGHT_UPPER_ARM, false),
        b"rightLowerArm" => (BONE_RIGHT_LOWER_ARM, false),
        b"rightHand" => (BONE_RIGHT_HAND, false),
        b"leftUpperLeg" => (BONE_LEFT_UPPER_LEG, false),
        b"leftLowerLeg" => (BONE_LEFT_LOWER_LEG, false),
        b"leftFoot" => (BONE_LEFT_FOOT, false),
        b"rightUpperLeg" => (BONE_RIGHT_UPPER_LEG, false),
        b"rightLowerLeg" => (BONE_RIGHT_LOWER_LEG, false),
        b"rightFoot" => (BONE_RIGHT_FOOT, false),
        _ => return None,
    })
}

fn add_bone(st: &mut State, node: i64, bone: u8, fallback: bool) {
    if st.nbone >= MAX_BONE_PAIRS || node < 0 || node > u16::MAX as i64 {
        return;
    }
    st.bone_pairs[st.nbone] = BonePair {
        node: node as u16,
        bone,
        fallback,
    };
    st.nbone += 1;
}

fn parse_extensions(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"VRMC_vrm" => parse_vrmc_vrm(s, st)?,
            b"VRM" => parse_vrm0(s, st)?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// VRM 1.0: `extensions.VRMC_vrm.humanoid.humanBones.<name>.node`.
fn parse_vrmc_vrm(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"humanoid" => {
                s.expect(b'{')?;
                let mut k2 = [0u8; KEY_MAX];
                loop {
                    let m = match s.obj_key(&mut k2)? {
                        Some(m) => m,
                        None => break,
                    };
                    match &k2[..m] {
                        b"humanBones" => parse_human_bones_map(s, st)?,
                        _ => s.skip_value(MAX_DEPTH)?,
                    }
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// Object map form: `{ "hips": { "node": 3 }, ... }`.
fn parse_human_bones_map(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match bone_slot(&key[..n]) {
            Some((bone, fallback)) => {
                s.expect(b'{')?;
                let mut node: i64 = -1;
                let mut k2 = [0u8; 16];
                loop {
                    let m = match s.obj_key(&mut k2)? {
                        Some(m) => m,
                        None => break,
                    };
                    match &k2[..m] {
                        b"node" => node = s.number_i64()?,
                        _ => s.skip_value(MAX_DEPTH)?,
                    }
                }
                add_bone(st, node, bone, fallback);
            }
            None => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// VRM 0.x: `extensions.VRM.humanoid.humanBones[] = { bone, node }`.
fn parse_vrm0(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"humanoid" => {
                s.expect(b'{')?;
                let mut k2 = [0u8; KEY_MAX];
                loop {
                    let m = match s.obj_key(&mut k2)? {
                        Some(m) => m,
                        None => break,
                    };
                    match &k2[..m] {
                        b"humanBones" => {
                            s.expect(b'[')?;
                            while s.arr_next()? {
                                parse_vrm0_bone(s, st)?;
                            }
                        }
                        _ => s.skip_value(MAX_DEPTH)?,
                    }
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

fn parse_vrm0_bone(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut bone = 0u8;
    let mut fallback = false;
    let mut node: i64 = -1;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"bone" => {
                let mut name = [0u8; 32];
                let nn = s.string_into(&mut name)?;
                if let Some((b, f)) = bone_slot(&name[..nn]) {
                    bone = b;
                    fallback = f;
                }
            }
            b"node" => node = s.number_i64()?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    if bone != 0 {
        add_bone(st, node, bone, fallback);
    }
    Ok(())
}

// --- post-pass-1 fixups -----------------------------------------------------

fn resolve_parents(st: &State, arena: &mut ModelArena) -> Result<(), LoadError> {
    for i in 0..arena.node_count {
        let off = st.child_off[i] as usize;
        let num = st.child_num[i] as usize;
        for k in 0..num {
            let slot = off + k;
            if slot >= MAX_CHILD {
                return Err(LoadError::BadJson);
            }
            let child = st.children[slot] as usize;
            if child >= arena.node_count || child == i {
                return Err(LoadError::BadJson);
            }
            arena.nodes[child].parent = i as i16;
        }
    }
    // Reject parent cycles, which would make any hierarchy walk diverge.
    for i in 0..arena.node_count {
        let mut p = arena.nodes[i].parent;
        let mut steps = 0usize;
        while p >= 0 {
            steps += 1;
            if steps > arena.node_count {
                return Err(LoadError::BadJson);
            }
            let pi = p as usize;
            if pi >= arena.node_count {
                return Err(LoadError::BadJson);
            }
            p = arena.nodes[pi].parent;
        }
    }
    Ok(())
}

fn apply_bones(st: &State, arena: &mut ModelArena) {
    // Two rounds so `upperChest` only fills a slot `chest` left empty.
    for want_fallback in [false, true] {
        for pair in st.bone_pairs[..st.nbone].iter() {
            if pair.fallback != want_fallback {
                continue;
            }
            let n = pair.node as usize;
            if n < arena.node_count && arena.nodes[n].bone == BONE_NONE {
                arena.nodes[n].bone = pair.bone;
            }
        }
    }
}

fn validate_joints(st: &State, arena: &mut ModelArena) -> Result<(), LoadError> {
    let _ = st;
    for j in 0..arena.joint_count {
        if arena.joint_nodes[j] as usize >= arena.node_count {
            return Err(LoadError::BadJson);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Accessor streaming
// ---------------------------------------------------------------------------

/// A validated accessor ready to be read element by element.
#[derive(Clone, Copy)]
struct AccInfo {
    /// Absolute file offset of element 0.
    off: u64,
    stride: u64,
    elem: usize,
    count: u32,
    comp: u16,
    comps: usize,
    normalized: bool,
}

fn comp_size(comp: u16) -> usize {
    match comp {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        _ => 0,
    }
}

fn acc_info(st: &State, idx: usize, glb: &Glb) -> Result<AccInfo, LoadError> {
    if idx >= st.accs_total {
        return Err(LoadError::BadJson);
    }
    if idx >= st.naccs {
        // We capped the table; a *referenced* dropped accessor cannot be served.
        return Err(LoadError::Unsupported);
    }
    let a = st.accs[idx];
    if a.sparse {
        return Err(LoadError::Unsupported);
    }
    let csz = comp_size(a.comp);
    if csz == 0 || a.comps == 0 || a.off == 0 && a.count == 0 {
        return Err(LoadError::BadAccessor);
    }
    if a.view < 0 {
        // No bufferView and no sparse means no data at all.
        return Err(LoadError::BadAccessor);
    }
    let vi = a.view as usize;
    if vi >= st.views_total {
        return Err(LoadError::BadJson);
    }
    if vi >= st.nviews {
        return Err(LoadError::Unsupported);
    }
    if glb.bin_len == 0 {
        return Err(LoadError::Unsupported);
    }
    let v = st.views[vi];
    let elem = csz.saturating_mul(a.comps as usize);
    if elem == 0 {
        return Err(LoadError::BadAccessor);
    }
    let stride = if v.stride != 0 {
        v.stride as u64
    } else {
        elem as u64
    };
    if stride < elem as u64 {
        return Err(LoadError::BadAccessor);
    }
    let n = a.count as u64;
    let last = if n == 0 {
        a.off
    } else {
        a.off
            .saturating_add((n - 1).saturating_mul(stride))
            .saturating_add(elem as u64)
    };
    if last > v.len {
        return Err(LoadError::BadAccessor);
    }
    if v.off.saturating_add(v.len) > glb.bin_len {
        return Err(LoadError::BadAccessor);
    }
    Ok(AccInfo {
        off: glb.bin_off.saturating_add(v.off).saturating_add(a.off),
        stride,
        elem,
        count: a.count,
        comp: a.comp,
        comps: a.comps as usize,
        normalized: a.normalized,
    })
}

/// Sequential element reader. Tightly packed accessors are read through a
/// block buffer (one `read_at` per ~1 KiB); strided ones fall back to one
/// positional read per element.
struct ElemStream<'r> {
    r: &'r mut dyn ReadAt,
    off: u64,
    stride: u64,
    elem: usize,
    packed: bool,
    idx: u32,
    count: u32,
    buf: [u8; 1024],
    buf_abs: u64,
    pos: usize,
    have: usize,
}

impl<'r> ElemStream<'r> {
    fn new(r: &'r mut dyn ReadAt, ai: &AccInfo) -> Self {
        ElemStream {
            r,
            off: ai.off,
            stride: ai.stride,
            elem: ai.elem,
            packed: ai.stride == ai.elem as u64 && ai.elem <= 1024,
            idx: 0,
            count: ai.count,
            buf: [0; 1024],
            buf_abs: ai.off,
            pos: 0,
            have: 0,
        }
    }

    fn fill(&mut self) -> bool {
        if self.pos > 0 {
            let rem = self.have - self.pos;
            self.buf.copy_within(self.pos..self.have, 0);
            self.buf_abs = self.buf_abs.saturating_add(self.pos as u64);
            self.pos = 0;
            self.have = rem;
        }
        if self.have >= self.buf.len() {
            return false;
        }
        let at = self.buf_abs.saturating_add(self.have as u64);
        let n = self.r.read_at(at, &mut self.buf[self.have..]);
        if n == 0 {
            return false;
        }
        self.have += n.min(self.buf.len() - self.have);
        true
    }

    fn next(&mut self) -> Result<&[u8], LoadError> {
        if self.idx >= self.count {
            return Err(LoadError::BadAccessor);
        }
        if self.packed {
            while self.pos + self.elem > self.have {
                if !self.fill() {
                    return Err(LoadError::Io);
                }
            }
            let a = self.pos;
            self.pos += self.elem;
            self.idx += 1;
            Ok(&self.buf[a..a + self.elem])
        } else {
            let at = self
                .off
                .saturating_add((self.idx as u64).saturating_mul(self.stride));
            self.idx += 1;
            if self.r.read_at(at, &mut self.buf[..self.elem]) != self.elem {
                return Err(LoadError::Io);
            }
            Ok(&self.buf[..self.elem])
        }
    }
}

fn decode_f32(b: &[u8], comp: u16, comps: usize, normalized: bool, out: &mut [f32; 16]) {
    let n = if comps < 16 { comps } else { 16 };
    for k in 0..n {
        out[k] = match comp {
            5126 => {
                let o = k * 4;
                f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
            }
            5120 => {
                let v = b[k] as i8 as f32;
                if normalized {
                    (v / 127.0).max(-1.0)
                } else {
                    v
                }
            }
            5121 => {
                let v = b[k] as f32;
                if normalized {
                    v / 255.0
                } else {
                    v
                }
            }
            5122 => {
                let o = k * 2;
                let v = i16::from_le_bytes([b[o], b[o + 1]]) as f32;
                if normalized {
                    (v / 32767.0).max(-1.0)
                } else {
                    v
                }
            }
            5123 => {
                let o = k * 2;
                let v = u16::from_le_bytes([b[o], b[o + 1]]) as f32;
                if normalized {
                    v / 65535.0
                } else {
                    v
                }
            }
            5125 => {
                let o = k * 4;
                u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as f32
            }
            _ => 0.0,
        };
    }
}

fn decode_index(b: &[u8], comp: u16) -> Result<u32, LoadError> {
    Ok(match comp {
        5121 => b[0] as u32,
        5123 => u16::from_le_bytes([b[0], b[1]]) as u32,
        5125 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        _ => return Err(LoadError::BadAccessor),
    })
}

fn decode_joint(b: &[u8], comp: u16) -> Result<u16, LoadError> {
    Ok(match comp {
        5121 => b[0] as u16,
        5123 => u16::from_le_bytes([b[0], b[1]]),
        _ => return Err(LoadError::BadAccessor),
    })
}

/// Fill the inverse bind matrices (identity where the file omits them).
fn read_ibm(
    read: &mut dyn ReadAt,
    st: &State,
    arena: &mut ModelArena,
    glb: &Glb,
) -> Result<(), LoadError> {
    for j in 0..MAX_JOINTS {
        arena.joint_ibm[j] = [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ];
    }
    if arena.skin_count == 0 || st.skin_ibm < 0 {
        return Ok(());
    }
    let ai = acc_info(st, st.skin_ibm as usize, glb)?;
    if ai.comp != 5126 || ai.comps != 16 {
        return Err(LoadError::BadAccessor);
    }
    let n = arena.joint_count.min(ai.count as usize);
    let mut es = ElemStream::new(&mut *read, &ai);
    let mut out = [0.0f32; 16];
    for j in 0..n {
        let b = es.next()?;
        decode_f32(b, ai.comp, ai.comps, ai.normalized, &mut out);
        arena.joint_ibm[j] = out;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pass 2: geometry
// ---------------------------------------------------------------------------

fn parse_pass2(
    s: &mut Src,
    st: &State,
    arena: &mut ModelArena,
    glb: &Glb,
) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"meshes" => parse_meshes(s, st, arena, glb)?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

fn parse_meshes(
    s: &mut Src,
    st: &State,
    arena: &mut ModelArena,
    glb: &Glb,
) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"primitives" => parse_primitives(s, st, arena, glb)?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
    }
    Ok(())
}

fn parse_primitives(
    s: &mut Src,
    st: &State,
    arena: &mut ModelArena,
    glb: &Glb,
) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        let mut pos: i64 = -1;
        let mut normal: i64 = -1;
        let mut uv: i64 = -1;
        let mut joints: i64 = -1;
        let mut weights: i64 = -1;
        let mut indices: i64 = -1;
        let mut material: i64 = -1;
        let mut mode: i64 = 4;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"attributes" => {
                    s.expect(b'{')?;
                    let mut k2 = [0u8; KEY_MAX];
                    loop {
                        let m = match s.obj_key(&mut k2)? {
                            Some(m) => m,
                            None => break,
                        };
                        match &k2[..m] {
                            b"POSITION" => pos = s.number_i64()?,
                            b"NORMAL" => normal = s.number_i64()?,
                            b"TEXCOORD_0" => uv = s.number_i64()?,
                            b"JOINTS_0" => joints = s.number_i64()?,
                            b"WEIGHTS_0" => weights = s.number_i64()?,
                            _ => s.skip_value(MAX_DEPTH)?,
                        }
                    }
                }
                b"indices" => indices = s.number_i64()?,
                b"material" => material = s.number_i64()?,
                b"mode" => mode = s.number_i64()?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        // Only TRIANGLES (mode 4) is drawable; other primitive kinds (points,
        // lines, strips...) are skipped rather than failing the model.
        if mode != 4 {
            continue;
        }
        build_primitive(
            s, st, arena, glb, pos, normal, uv, joints, weights, indices, material,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_primitive(
    s: &mut Src,
    st: &State,
    arena: &mut ModelArena,
    glb: &Glb,
    acc_pos: i64,
    acc_norm: i64,
    acc_uv: i64,
    acc_joints: i64,
    acc_weights: i64,
    acc_indices: i64,
    material: i64,
) -> Result<(), LoadError> {
    if acc_pos < 0 {
        return Ok(());
    }
    let ai = acc_info(st, acc_pos as usize, glb)?;
    if ai.comps < 3 || ai.comp != 5126 {
        return Err(LoadError::BadAccessor);
    }
    let n = ai.count as usize;
    if n == 0 {
        return Ok(());
    }
    if arena.vertex_count.saturating_add(n) > MAX_VERTS {
        return Err(LoadError::ArenaFull);
    }
    if arena.primitive_count >= MAX_PRIMS {
        return Err(LoadError::ArenaFull);
    }
    let v0 = arena.vertex_count;
    let first_index = arena.index_count;

    // POSITION
    {
        let mut es = ElemStream::new(&mut *s.r, &ai);
        let mut out = [0.0f32; 16];
        for i in 0..n {
            let b = es.next()?;
            decode_f32(b, ai.comp, ai.comps, ai.normalized, &mut out);
            arena.vertices[v0 + i].pos = [out[0], out[1], out[2]];
        }
    }

    // NORMAL
    if acc_norm >= 0 {
        let an = acc_info(st, acc_norm as usize, glb)?;
        if an.comps < 3 || an.count as usize != n {
            return Err(LoadError::BadAccessor);
        }
        let mut es = ElemStream::new(&mut *s.r, &an);
        let mut out = [0.0f32; 16];
        for i in 0..n {
            let b = es.next()?;
            decode_f32(b, an.comp, an.comps, an.normalized, &mut out);
            let nv = Vec3::new(out[0], out[1], out[2]).normalised();
            arena.vertices[v0 + i].normal = [nv.x, nv.y, nv.z];
        }
    }

    // TEXCOORD_0
    if acc_uv >= 0 {
        let au = acc_info(st, acc_uv as usize, glb)?;
        if au.comps < 2 || au.count as usize != n {
            return Err(LoadError::BadAccessor);
        }
        let mut es = ElemStream::new(&mut *s.r, &au);
        let mut out = [0.0f32; 16];
        for i in 0..n {
            let b = es.next()?;
            decode_f32(b, au.comp, au.comps, au.normalized, &mut out);
            arena.vertices[v0 + i].uv = [out[0], out[1]];
        }
    }

    // JOINTS_0
    if acc_joints >= 0 {
        let aj = acc_info(st, acc_joints as usize, glb)?;
        if aj.comps < 4 || aj.count as usize != n {
            return Err(LoadError::BadAccessor);
        }
        if !matches!(aj.comp, 5121 | 5123) {
            return Err(LoadError::BadAccessor);
        }
        let mut es = ElemStream::new(&mut *s.r, &aj);
        let csz = comp_size(aj.comp);
        for i in 0..n {
            let b = es.next()?;
            let j0 = decode_joint(b, aj.comp)?;
            let j1 = decode_joint(&b[csz..], aj.comp)?;
            let j2 = decode_joint(&b[csz * 2..], aj.comp)?;
            let j3 = decode_joint(&b[csz * 3..], aj.comp)?;
            arena.vertices[v0 + i].joints = [j0, j1, j2, j3];
            // A jointed vertex with no weight array still needs one influence,
            // otherwise every skinning matrix would cancel out.
            arena.vertices[v0 + i].weights = [1.0, 0.0, 0.0, 0.0];
        }
    }

    // WEIGHTS_0
    if acc_weights >= 0 {
        let aw = acc_info(st, acc_weights as usize, glb)?;
        if aw.comps < 4 || aw.count as usize != n {
            return Err(LoadError::BadAccessor);
        }
        let mut es = ElemStream::new(&mut *s.r, &aw);
        let mut out = [0.0f32; 16];
        for i in 0..n {
            let b = es.next()?;
            decode_f32(b, aw.comp, aw.comps, aw.normalized, &mut out);
            let mut w = [out[0], out[1], out[2], out[3]];
            for x in w.iter_mut() {
                if !(*x >= 0.0) {
                    *x = 0.0;
                }
            }
            let sum = w[0] + w[1] + w[2] + w[3];
            if sum > 1e-20 {
                w[0] /= sum;
                w[1] /= sum;
                w[2] /= sum;
                w[3] /= sum;
            } else {
                w = [0.0; 4];
            }
            arena.vertices[v0 + i].weights = w;
        }
    }

    // Indices (or generated 0..n).
    let idx_count;
    if acc_indices >= 0 {
        let ax = acc_info(st, acc_indices as usize, glb)?;
        if ax.comps != 1 || !matches!(ax.comp, 5121 | 5123 | 5125) {
            return Err(LoadError::BadAccessor);
        }
        let m = ax.count as usize;
        if m % 3 != 0 {
            return Err(LoadError::BadAccessor);
        }
        if arena.index_count.saturating_add(m) > MAX_TRIS {
            return Err(LoadError::ArenaFull);
        }
        let mut es = ElemStream::new(&mut *s.r, &ax);
        for i in 0..m {
            let b = es.next()?;
            let idx = decode_index(b, ax.comp)?;
            if idx as usize >= n {
                return Err(LoadError::BadAccessor);
            }
            arena.indices[arena.index_count + i] = (v0 as u32) + idx;
        }
        idx_count = m;
    } else {
        let m = n - (n % 3);
        if m == 0 {
            // Fewer than three vertices: nothing drawable, leave the arena as
            // it was (the vertex slots written above are simply not counted).
            return Ok(());
        }
        if arena.index_count.saturating_add(m) > MAX_TRIS {
            return Err(LoadError::ArenaFull);
        }
        for i in 0..m {
            arena.indices[arena.index_count + i] = (v0 as u32) + i as u32;
        }
        idx_count = m;
    }

    let mat = if material >= 0 && (material as usize) < arena.material_count {
        material as u16
    } else {
        0
    };
    arena.primitives[arena.primitive_count] = Primitive {
        first_index: first_index as u32,
        index_count: idx_count as u32,
        material: mat,
    };
    arena.primitive_count += 1;
    arena.index_count += idx_count;
    arena.vertex_count = v0 + n;
    Ok(())
}

// ---------------------------------------------------------------------------
// Textures
// ---------------------------------------------------------------------------

fn decode_textures(read: &mut dyn ReadAt, st: &State, arena: &mut ModelArena, glb: &Glb) {
    let n = if st.ntex < MAX_TEXTURES {
        st.ntex
    } else {
        MAX_TEXTURES
    };
    for t in 0..n {
        let src = st.tex_src[t];
        if src < 0 {
            continue;
        }
        let ii = src as usize;
        if ii >= st.nimages {
            continue;
        }
        let im = st.images[ii];
        if !im.valid || im.view < 0 {
            continue;
        }
        let vi = im.view as usize;
        if vi >= st.nviews {
            continue;
        }
        let v = st.views[vi];
        if v.len < 8 || v.off.saturating_add(v.len) > glb.bin_len {
            continue;
        }
        let base = glb.bin_off.saturating_add(v.off);
        // A failed texture decode keeps the material's base colour; it is not
        // worth rejecting an otherwise good model over.
        let _ = decode_png(&mut *read, base, v.len, &mut arena.textures[t]);
    }
}

fn finalize_materials(arena: &mut ModelArena) {
    for i in 0..arena.material_count {
        let t = arena.materials[i].texture;
        if t == NO_TEXTURE {
            continue;
        }
        let ti = t as usize;
        if ti >= MAX_TEXTURES || !arena.textures[ti].loaded {
            arena.materials[i].texture = NO_TEXTURE;
        }
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = fabs_i32(p - a as i32);
    let pb = fabs_i32(p - b as i32);
    let pc = fabs_i32(p - c as i32);
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn fabs_i32(x: i32) -> i32 {
    if x < 0 {
        -x
    } else {
        x
    }
}

/// Read a PNG blob through `read` (never materialised) and box-downsample it
/// into `out`. Returns false for anything outside the supported subset.
fn decode_png(read: &mut dyn ReadAt, base: u64, len: u64, out: &mut Texture) -> bool {
    const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let end = base.saturating_add(len);
    if len < 8 + 25 {
        return false;
    }
    let mut sig = [0u8; 8];
    if read.read_at(base, &mut sig) != 8 || sig != SIG {
        return false;
    }

    // --- chunk walk: keep the IHDR fields and concatenate the IDAT payloads ---
    let idat = PNG_IDAT.get();
    let mut idat_len = 0usize;
    let mut w = 0usize;
    let mut h = 0usize;
    let mut depth = 0u8;
    let mut colour = 0u8;
    let mut interlace = 0u8;
    let mut off = base.saturating_add(8);
    let mut hdr = [0u8; 8];
    while off.saturating_add(8) <= end {
        if read.read_at(off, &mut hdr) != 8 {
            return false;
        }
        let clen = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let data = off.saturating_add(8);
        if data.saturating_add(clen) > end {
            return false;
        }
        let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
        if kind == *b"IHDR" {
            if clen < 13 {
                return false;
            }
            let mut b = [0u8; 13];
            if read.read_at(data, &mut b) != 13 {
                return false;
            }
            w = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
            h = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as usize;
            depth = b[8];
            colour = b[9];
            interlace = b[12];
        } else if kind == *b"IDAT" {
            let n = clen as usize;
            if n == 0 || idat_len.saturating_add(n) > idat.len() {
                return false;
            }
            if read.read_at(data, &mut idat[idat_len..idat_len + n]) != n {
                return false;
            }
            idat_len += n;
        } else if kind == *b"IEND" {
            break;
        }
        off = data.saturating_add(clen).saturating_add(4);
    }

    if w == 0 || h == 0 || w > PNG_MAX_DIM || h > PNG_MAX_DIM {
        return false;
    }
    if depth != 8 || interlace != 0 {
        return false;
    }
    let bpp = match colour {
        2 => 3usize, // RGB
        6 => 4usize, // RGBA
        _ => return false,
    };
    let stride = w.saturating_mul(bpp);
    let raw_need = h.saturating_mul(1 + stride);
    if idat_len < 2 || raw_need == 0 || raw_need > PNG_RAW_CAP {
        return false;
    }

    // --- inflate the zlib stream (skip the 2-byte zlib header) ---
    let raw = PNG_RAW.get();
    let inflated = inflate(&idat[2..idat_len], &mut raw[..raw_need]);
    if inflated != raw_need {
        return false;
    }

    // --- unfilter in place, then box-downsample into RGBA8 ---
    let dw = if w < TEX_MAX_DIM { w } else { TEX_MAX_DIM };
    let dh = if h < TEX_MAX_DIM { h } else { TEX_MAX_DIM };
    if dw == 0 || dh == 0 {
        return false;
    }
    let mut acc = [[0u32; 4]; TEX_MAX_DIM];
    let mut cnt = [0u32; TEX_MAX_DIM];
    let mut cur_dy = usize::MAX;
    for y in 0..h {
        let row = y * (1 + stride);
        let prev = if y > 0 { (y - 1) * (1 + stride) } else { row };
        let ft = raw[row];
        if ft > 4 {
            return false;
        }
        let mut x = 0usize;
        while x < stride {
            let a = if x >= bpp { raw[row + 1 + x - bpp] } else { 0 };
            let b = if y > 0 { raw[prev + 1 + x] } else { 0 };
            let c = if y > 0 && x >= bpp {
                raw[prev + 1 + x - bpp]
            } else {
                0
            };
            let v = raw[row + 1 + x];
            let o = match ft {
                0 => v,
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(b),
                3 => v.wrapping_add(((a as u16 + b as u16) / 2) as u8),
                _ => v.wrapping_add(paeth(a, b, c)),
            };
            raw[row + 1 + x] = o;
            x += 1;
        }

        let dy = y * dh / h;
        if dy != cur_dy {
            if cur_dy < dh {
                flush_row(out, dw, cur_dy, &acc, &cnt);
            }
            cur_dy = dy;
            for slot in acc.iter_mut() {
                *slot = [0; 4];
            }
            for slot in cnt.iter_mut() {
                *slot = 0;
            }
        }
        let mut x = 0usize;
        while x < w {
            let dx = x * dw / w;
            let s = row + 1 + x * bpp;
            let p = &mut acc[dx];
            p[0] = p[0].saturating_add(raw[s] as u32);
            p[1] = p[1].saturating_add(raw[s + 1] as u32);
            p[2] = p[2].saturating_add(raw[s + 2] as u32);
            p[3] = p[3].saturating_add(if bpp == 4 { raw[s + 3] as u32 } else { 255 });
            cnt[dx] = cnt[dx].saturating_add(1);
            x += 1;
        }
    }
    if cur_dy < dh {
        flush_row(out, dw, cur_dy, &acc, &cnt);
    }
    out.w = dw as u16;
    out.h = dh as u16;
    out.loaded = true;
    true
}

fn flush_row(
    out: &mut Texture,
    dw: usize,
    dy: usize,
    acc: &[[u32; 4]; TEX_MAX_DIM],
    cnt: &[u32; TEX_MAX_DIM],
) {
    for dx in 0..dw {
        let n = if cnt[dx] == 0 { 1 } else { cnt[dx] };
        let o = (dy * dw + dx) * 4;
        if o + 3 >= out.rgba.len() {
            return;
        }
        out.rgba[o] = (acc[dx][0] / n).min(255) as u8;
        out.rgba[o + 1] = (acc[dx][1] / n).min(255) as u8;
        out.rgba[o + 2] = (acc[dx][2] / n).min(255) as u8;
        out.rgba[o + 3] = (acc[dx][3] / n).min(255) as u8;
    }
}