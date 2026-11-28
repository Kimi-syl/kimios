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
//! Memory: everything lives in fixed arrays. The index table set (`State`,
//! ~60 KiB) and the PNG scratch (an IDAT read window, a 32 KiB LZ77 window and
//! two scanline buffers, ~72 KiB) are `static`s, not stack locals: the user
//! stack is only 64 KiB, so `State` on the stack would overflow it. The loader
//! is therefore not re-entrant, which is fine for the single-threaded GUI, and
//! each `load` clears the tables before use.
//!
//! Supported beyond the base format:
//! - sparse accessors (`sparse.indices` + `sparse.values`), including the common
//!   morph-target case of a sparse accessor with *no* base `bufferView`, whose
//!   base values are all zero. Substitution happens per element inside
//!   [`ElemStream`], so every accessor user (positions, normals, UVs, joints,
//!   weights, indices, IBMs, morph deltas) gets it for free;
//! - morph targets: `meshes[].primitives[].targets[].POSITION` deltas are
//!   flattened into `ModelArena::morph_entries`/`morph_targets`, the mesh's
//!   `extras.targetNames` are kept loader-side for [`morph_index_by_name`], and
//!   VRM 1.0 `expressions.preset` / VRM 0.x `blendShapeMaster` preset binds are
//!   resolved for [`morph_preset_index`]. Arena overflow truncates the target
//!   list instead of failing the load.
//!
//! Limits / deliberately unsupported (each fails soft or with a `LoadError`):
//! - interlaced PNG, colour types other than RGB(2)/RGBA(6), 16-bit channels,
//!   source images wider than `PNG_MAX_W`, taller than `PNG_MAX_H` or above
//!   `PNG_MAX_PIXELS` (the texture is skipped and the material keeps its base
//!   colour). Within those bounds - which include every real VRM texture,
//!   1024/2048/4096 px wide - the PNG is inflated scanline by scanline and
//!   box-downsampled to `TEX_MAX_DIM` *during* the decode, so loader memory is
//!   constant instead of scaling with the source image;
//! - JPEG / external-URI images (texture skipped);
//! - animations, MToon shading (materials get a flat "unlit" flag instead),
//!   non-triangle primitive modes (skipped);
//! - more than `MAX_VERTS`/`MAX_TRIS`/`MAX_NODES`/... -> `LoadError::ArenaFull`.
//! - only the first morph target bind of a VRM preset expression is recorded,
//!   and only six presets (blink/happy/angry/sad/relaxed/surprised) are mapped.
//!
//! Every failure path returns a [`LoadError`]; there is no `unwrap`, no
//! indexing that a bounds check cannot prove safe, and no arithmetic that can
//! overflow silently (all offset maths is `saturating_*`).

use core::cell::UnsafeCell;

use super::model::{
    AlphaMode, LoadError, Material, ModelArena, MorphEntry, MorphTarget, Node, Primitive, ReadAt,
    Skin, Texture, BONE_NONE, MAX_JOINTS, MAX_MATERIALS, MAX_MORPH_ENTRIES, MAX_MORPH_TARGETS,
    MAX_NODES, MAX_PRIMS, MAX_TEXTURES, MAX_TRIS, MAX_VERTS, NO_TEXTURE, TEX_MAX_DIM,
};
use super::skin::{
    BONE_CHEST, BONE_HEAD, BONE_HIPS, BONE_LEFT_FOOT, BONE_LEFT_HAND, BONE_LEFT_LOWER_ARM,
    BONE_LEFT_LOWER_LEG, BONE_LEFT_SHOULDER, BONE_LEFT_UPPER_ARM, BONE_LEFT_UPPER_LEG, BONE_NECK,
    BONE_RIGHT_FOOT, BONE_RIGHT_HAND, BONE_RIGHT_LOWER_ARM, BONE_RIGHT_LOWER_LEG,
    BONE_RIGHT_SHOULDER, BONE_RIGHT_UPPER_ARM, BONE_RIGHT_UPPER_LEG, BONE_SPINE,
};
use super::{sqrt, Vec3};

// ---------------------------------------------------------------------------
// Loader-internal fixed capacities
// ---------------------------------------------------------------------------

/// bufferViews kept. Real production VRMs routinely carry hundreds (a 10 MB
/// sample in `tools/blender/_work` has 898), so the table is sized for the
/// largest plausible asset. Accessors may refer to views beyond this; such
/// accessors are then reported as `Unsupported` instead of failing the load.
const MAX_VIEWS: usize = 1024;
/// accessors kept. Morph-target accessors typically sit after the geometry
/// ones; an accessor beyond the cap is reported as `Unsupported`, which for a
/// morph target means that target is recorded with zero deltas rather than
/// failing the model. The same 10 MB sample has 717 accessors.
const MAX_ACC: usize = 1024;
/// sparse accessor descriptors kept. VRM 1.0 expression samples can carry one
/// sparse accessor per morph target (the sample above has 162), so keep room
/// for a full facial set; past the cap the accessor is treated as unsupported,
/// which again only costs a morph target.
const MAX_SPARSE: usize = 256;
/// images kept (each is one PNG blob). Texture slots are the real limit
/// (`MAX_TEXTURES`); this only has to be big enough that every image a kept
/// texture can point at survives, including files that order images oddly.
const MAX_IMAGES: usize = 32;
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
/// Largest source PNG width accepted. The unfiltered scanline has to be kept
/// (the Up/Average/Paeth filters reference it), so this bounds two static row
/// buffers; 4096 covers every real VRM texture (1024-2048 is the common size).
const PNG_MAX_W: usize = 4096;
/// Largest source PNG height accepted. Height costs no memory (rows are
/// streamed straight into the accumulator) but bounds the work one file can
/// make the GUI do.
const PNG_MAX_H: usize = 65535;
/// Largest source pixel count accepted, for the same reason as `PNG_MAX_H`:
/// bounds decode time, not memory. 4096x4096 is ~16.8 M pixels.
const PNG_MAX_PIXELS: usize = 32 * 1024 * 1024;
/// Compressed IDAT bytes buffered at once while inflating. The compressed
/// stream is pulled from the file on demand, so this is the only copy of it.
const PNG_IN_CAP: usize = 8192;
/// DEFLATE LZ77 history window (the RFC 1951 maximum distance).
const PNG_WIN: usize = 32768;
/// One unfiltered scanline at the widest accepted source (RGBA8).
const PNG_ROW_CAP: usize = PNG_MAX_W * 4;

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

/// Compressed IDAT input window (walked chunk by chunk from the file).
static PNG_IN: SharedBuf<PNG_IN_CAP> = SharedBuf(UnsafeCell::new([0; PNG_IN_CAP]));
/// DEFLATE 32 KiB history, kept as a ring so back-references work while the
/// decompressed image is streamed past the accumulator.
static PNG_WIN_BUF: SharedBuf<PNG_WIN> = SharedBuf(UnsafeCell::new([0; PNG_WIN]));
/// Current unfiltered scanline.
static PNG_ROW: SharedBuf<PNG_ROW_CAP> = SharedBuf(UnsafeCell::new([0; PNG_ROW_CAP]));
/// Previous unfiltered scanline (filter types 2/3/4 reference it).
static PNG_PREV: SharedBuf<PNG_ROW_CAP> = SharedBuf(UnsafeCell::new([0; PNG_ROW_CAP]));

// ---------------------------------------------------------------------------
// Morph-target bookkeeping (names and VRM expression bindings)
// ---------------------------------------------------------------------------
//
// The arena has no string storage, so `meshes[].extras.targetNames` and the VRM
// preset -> target bindings live in this one loader-side set, just like the PNG
// scratch above. It is filled while `load` runs and read back afterwards through
// [`morph_index_by_name`] / [`morph_preset_index`].

/// Longest morph-target name retained; longer names are truncated.
const MORPH_NAME_LEN: usize = 32;

/// Number of VRM preset expressions this loader maps.
pub const MORPH_PRESET_COUNT: usize = 6;
/// Preset slot for VRM `blink` (see [`morph_preset_index`]).
pub const MORPH_PRESET_BLINK: usize = 0;
/// Preset slot for VRM `happy`.
pub const MORPH_PRESET_HAPPY: usize = 1;
/// Preset slot for VRM `angry`.
pub const MORPH_PRESET_ANGRY: usize = 2;
/// Preset slot for VRM `sad`.
pub const MORPH_PRESET_SAD: usize = 3;
/// Preset slot for VRM `relaxed`.
pub const MORPH_PRESET_RELAXED: usize = 4;
/// Preset slot for VRM `surprised`.
pub const MORPH_PRESET_SURPRISED: usize = 5;

/// Lower-case VRM preset name for a slot; also used as the name fallback when a
/// file has no VRM expression bindings but does name its targets.
fn preset_name(slot: usize) -> &'static [u8] {
    match slot {
        MORPH_PRESET_BLINK => b"blink",
        MORPH_PRESET_HAPPY => b"happy",
        MORPH_PRESET_ANGRY => b"angry",
        MORPH_PRESET_SAD => b"sad",
        MORPH_PRESET_RELAXED => b"relaxed",
        MORPH_PRESET_SURPRISED => b"surprised",
        _ => b"",
    }
}

/// Fixed-capacity morph metadata kept outside the arena.
#[derive(Clone, Copy)]
struct MorphBook {
    /// Named targets, at most one name per flat target.
    nnames: usize,
    name: [[u8; MORPH_NAME_LEN]; MAX_MORPH_TARGETS],
    name_len: [u8; MAX_MORPH_TARGETS],
    /// Flat arena target index each name belongs to (-1 when unset).
    name_target: [i16; MAX_MORPH_TARGETS],
    /// Flat target -> mesh index and target index within that mesh. Needed to
    /// resolve VRM binds, which address (node/mesh, target index), to the flat
    /// arena list.
    flat_mesh: [u16; MAX_MORPH_TARGETS],
    flat_k: [u8; MAX_MORPH_TARGETS],
    nflat: usize,
    /// VRM 1.0 bind: node whose mesh carries the target (-1 unset).
    preset_node: [i16; MORPH_PRESET_COUNT],
    /// VRM 0.x bind: mesh index directly (-1 unset).
    preset_mesh: [i16; MORPH_PRESET_COUNT],
    /// Target index inside that mesh (-1 unset).
    preset_k: [i16; MORPH_PRESET_COUNT],
    /// Resolved flat arena target per preset (-1 unset).
    preset_flat: [i16; MORPH_PRESET_COUNT],
}

impl MorphBook {
    const fn empty() -> Self {
        MorphBook {
            nnames: 0,
            name: [[0u8; MORPH_NAME_LEN]; MAX_MORPH_TARGETS],
            name_len: [0; MAX_MORPH_TARGETS],
            name_target: [-1; MAX_MORPH_TARGETS],
            flat_mesh: [0; MAX_MORPH_TARGETS],
            flat_k: [0; MAX_MORPH_TARGETS],
            nflat: 0,
            preset_node: [-1; MORPH_PRESET_COUNT],
            preset_mesh: [-1; MORPH_PRESET_COUNT],
            preset_k: [-1; MORPH_PRESET_COUNT],
            preset_flat: [-1; MORPH_PRESET_COUNT],
        }
    }
}

struct BookStore(UnsafeCell<MorphBook>);

// SAFETY: same single-GUI-thread contract as `SharedBuf`; the table is written
// only from `load` and read only from the query helpers below.
unsafe impl Sync for BookStore {}

static MORPH_BOOK: BookStore = BookStore(UnsafeCell::new(MorphBook::empty()));

fn morph_book() -> &'static mut MorphBook {
    // SAFETY: see `BookStore`.
    unsafe { &mut *MORPH_BOOK.0.get() }
}

#[allow(dead_code)]
fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn name_eq_ci(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i].to_ascii_lowercase() != b[i].to_ascii_lowercase() {
            return false;
        }
        i += 1;
    }
    true
}

/// Look up a flat arena morph-target index by the mesh's `extras.targetNames`
/// entry, e.g. `gltf::morph_index_by_name(b"blink")`.
///
/// The comparison is exact (case-sensitive) on the bytes retained; names longer
/// than 32 bytes are truncated. Returns `None` when the model has no such
/// target. Only meaningful after a successful [`load`].
#[allow(dead_code)] // Public API for the expression driver.
pub fn morph_index_by_name(name: &[u8]) -> Option<usize> {
    let book = morph_book();
    let mut i = 0;
    while i < book.nnames {
        let n = book.name_len[i] as usize;
        let target = book.name_target[i];
        if target >= 0 && n == name.len() && bytes_eq(&book.name[i][..n], name) {
            return Some(target as usize);
        }
        i += 1;
    }
    None
}

/// Flat arena morph-target index for a VRM preset expression slot (one of the
/// `MORPH_PRESET_*` constants), or `None` when the model does not carry it.
///
/// Resolution order: explicit VRM 1.0 `expressions.preset.<name>` bind, then
/// VRM 0.x `blendShapeMaster` group bind, then a case-insensitive match against
/// `extras.targetNames`. Only meaningful after a successful [`load`].
#[allow(dead_code)] // Public API for the expression driver.
pub fn morph_preset_index(preset: usize) -> Option<usize> {
    if preset >= MORPH_PRESET_COUNT {
        return None;
    }
    let target = morph_book().preset_flat[preset];
    if target >= 0 {
        Some(target as usize)
    } else {
        None
    }
}

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

/// One glTF `sparse` descriptor, as parsed from `accessors[].sparse`.
#[derive(Clone, Copy)]
struct Sparse {
    /// Number of substituted elements.
    count: u32,
    /// View holding the indices, or -1.
    idx_view: i16,
    /// Indices component type (5121/5123/5125).
    idx_comp: u16,
    /// `sparse.indices.byteOffset`.
    idx_off: u64,
    /// View holding the values, or -1.
    val_view: i16,
    /// `sparse.values.byteOffset`.
    val_off: u64,
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
    /// Index into [`State::sparses`], -1 for none, -2 when the table was full.
    sparse: i16,
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
    sparses: [Sparse; MAX_SPARSE],
    nsparse: usize,
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
    /// Morph-target names and VRM preset bindings gathered across both passes.
    morph: MorphBook,
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
    sparse: -1,
};
const EMPTY_SPARSE: Sparse = Sparse {
    count: 0,
    idx_view: -1,
    idx_comp: 0,
    idx_off: 0,
    val_view: -1,
    val_off: 0,
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

/// A freshly reset [`State`]; every load starts from this.
const EMPTY_STATE: State = State {
    views: [EMPTY_VIEW; MAX_VIEWS],
    nviews: 0,
    views_total: 0,
    accs: [EMPTY_ACC; MAX_ACC],
    naccs: 0,
    accs_total: 0,
    sparses: [EMPTY_SPARSE; MAX_SPARSE],
    nsparse: 0,
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
    morph: MorphBook::empty(),
};

/// The one `State` record, kept in `.bss` instead of on the stack: at
/// real-world table sizes it is ~60 KiB and the user stack is only 64 KiB.
struct StateStore(UnsafeCell<State>);

// SAFETY: the loader runs on the single GUI thread and never re-enters, the
// same contract as the `SharedBuf` scratch above.
unsafe impl Sync for StateStore {}

static STATE: StateStore = StateStore(UnsafeCell::new(EMPTY_STATE));

fn state_store() -> &'static mut State {
    unsafe { &mut *STATE.0.get() }
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

    // The index tables live in `.bss` (see `StateStore`), so every load must
    // start from a clean slate rather than the previous model's tables.
    let st = state_store();
    *st = EMPTY_STATE;
    // The morph metadata table outlives `load` (the Lead queries it), so start
    // every load from an empty one rather than leaving a previous model's names.
    *morph_book() = MorphBook::empty();

    // Pass 1: index tables (and the humanoid map, which does not need nodes to
    // exist yet because node indices are just integers).
    {
        let mut s = Src::new(&mut *read, glb.json_off, glb.json_len);
        parse_pass1(&mut s, st, arena)?;
    }

    resolve_parents(&*st, arena)?;
    apply_bones(&*st, arena);
    validate_joints(&*st, arena)?;
    read_ibm(read, &*st, arena, &glb)?;
    decode_textures(read, &*st, arena, &glb);

    // The rasteriser needs at least one material slot to draw anything.
    if arena.material_count == 0 {
        arena.materials[0] = Material::default();
        arena.material_count = 1;
    }
    finalize_materials(arena);

    // Pass 2: geometry. Accessor nodes point into the validated tables.
    {
        let mut s = Src::new(&mut *read, glb.json_off, glb.json_len);
        parse_pass2(&mut s, st, arena, &glb)?;
    }

    // Morph targets parsed in pass 2 are now addressable; resolve the VRM
    // preset binds collected in pass 1 against them, then publish the table.
    {
        let mut book = st.morph;
        resolve_presets(&mut book, arena);
        *morph_book() = book;
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
                    // Keep the descriptor in the shared sparse table rather than
                    // in `Acc`, so the (mostly sparse-free) accessor table stays
                    // small enough to live on the loader's stack.
                    let sp = parse_sparse(s)?;
                    if st.nsparse < MAX_SPARSE {
                        st.sparses[st.nsparse] = sp;
                        a.sparse = st.nsparse as i16;
                        st.nsparse += 1;
                    } else {
                        a.sparse = -2;
                    }
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

/// Clamp an optional glTF index to `i16`, with -1 meaning "absent".
fn i16_index(v: i64) -> i16 {
    if v >= 0 && v <= i16::MAX as i64 {
        v as i16
    } else {
        -1
    }
}

/// Parse an accessor `sparse` object:
/// `{ "count": N, "indices": { "bufferView", "byteOffset", "componentType" },
///    "values": { "bufferView", "byteOffset" } }`.
///
/// `values` deliberately has no `componentType` of its own: the values use the
/// enclosing accessor's component type, which is resolved in [`acc_info`].
fn parse_sparse(s: &mut Src) -> Result<Sparse, LoadError> {
    s.expect(b'{')?;
    let mut sp = EMPTY_SPARSE;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"count" => sp.count = num_u32(s)?,
            b"indices" => {
                s.expect(b'{')?;
                let mut k2 = [0u8; KEY_MAX];
                loop {
                    let m = match s.obj_key(&mut k2)? {
                        Some(m) => m,
                        None => break,
                    };
                    match &k2[..m] {
                        b"bufferView" => sp.idx_view = i16_index(s.number_i64()?),
                        b"byteOffset" => sp.idx_off = num_u64(s)?,
                        b"componentType" => {
                            sp.idx_comp = num_u32(s)?.min(u16::MAX as u32) as u16
                        }
                        _ => s.skip_value(MAX_DEPTH)?,
                    }
                }
            }
            b"values" => {
                s.expect(b'{')?;
                let mut k2 = [0u8; KEY_MAX];
                loop {
                    let m = match s.obj_key(&mut k2)? {
                        Some(m) => m,
                        None => break,
                    };
                    match &k2[..m] {
                        b"bufferView" => sp.val_view = i16_index(s.number_i64()?),
                        b"byteOffset" => sp.val_off = num_u64(s)?,
                        _ => s.skip_value(MAX_DEPTH)?,
                    }
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(sp)
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
            b"expressions" => parse_vrm1_expressions(s, st)?,
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
            b"blendShapeMaster" => parse_vrm0_blendshapes(s, st)?,
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

// --- VRM expressions -> morph target bindings -------------------------------

/// Parse `extensions.VRMC_vrm.expressions` (VRM 1.0).
fn parse_vrm1_expressions(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"preset" => parse_vrm1_presets(s, st)?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// `expressions.preset` is an object keyed by expression name.
fn parse_vrm1_presets(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match vrm1_preset_slot(&key[..n]) {
            Some(slot) => parse_vrm1_expression(s, st, slot)?,
            None => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

fn vrm1_preset_slot(name: &[u8]) -> Option<usize> {
    match name {
        b"blink" => Some(MORPH_PRESET_BLINK),
        b"happy" => Some(MORPH_PRESET_HAPPY),
        b"angry" => Some(MORPH_PRESET_ANGRY),
        b"sad" => Some(MORPH_PRESET_SAD),
        b"relaxed" => Some(MORPH_PRESET_RELAXED),
        b"surprised" => Some(MORPH_PRESET_SURPRISED),
        _ => None,
    }
}

/// One preset expression object; only its `morphTargetBinds` list matters. The
/// first bind that names a node and a target index wins; additional binds (one
/// expression driving several targets) are parsed and dropped.
fn parse_vrm1_expression(s: &mut Src, st: &mut State, slot: usize) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"morphTargetBinds" => {
                s.expect(b'[')?;
                while s.arr_next()? {
                    if st.morph.preset_node[slot] < 0 {
                        let (node, index) = parse_vrm_bind(s)?;
                        if node >= 0 && index >= 0 {
                            st.morph.preset_node[slot] = node;
                            st.morph.preset_k[slot] = index;
                        }
                    } else {
                        s.skip_value(MAX_DEPTH)?;
                    }
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// A VRM expression bind: `{ "node"|"mesh": N, "index": K, "weight": W }`.
///
/// VRM 1.0 names the node; VRM 0.x names the mesh directly. The same record
/// shape is used for both, the caller knows which field the first one is.
fn parse_vrm_bind(s: &mut Src) -> Result<(i16, i16), LoadError> {
    let mut holder: i64 = -1;
    let mut index: i64 = -1;
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"node" | b"mesh" => holder = s.number_i64()?,
            b"index" => index = s.number_i64()?,
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok((i16_index(holder), i16_index(index)))
}

/// Parse `extensions.VRM.blendShapeMaster` (VRM 0.x).
fn parse_vrm0_blendshapes(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"blendShapeGroups" => {
                s.expect(b'[')?;
                while s.arr_next()? {
                    parse_vrm0_blendshape_group(s, st)?;
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// One VRM 0.x blend-shape group: `{ presetName, binds: [{ mesh, index, weight }] }`.
fn parse_vrm0_blendshape_group(s: &mut Src, st: &mut State) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut slot: Option<usize> = None;
    let mut mesh: i64 = -1;
    let mut index: i64 = -1;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"presetName" => {
                let mut name = [0u8; 32];
                let nn = s.string_into(&mut name)?;
                slot = vrm0_preset_slot(&name[..nn]);
            }
            b"binds" => {
                s.expect(b'[')?;
                while s.arr_next()? {
                    if mesh < 0 || index < 0 {
                        let (m, i) = parse_vrm_bind(s)?;
                        if m >= 0 && i >= 0 {
                            mesh = m as i64;
                            index = i as i64;
                        }
                    } else {
                        s.skip_value(MAX_DEPTH)?;
                    }
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    if let Some(slot) = slot {
        if st.morph.preset_mesh[slot] < 0
            && st.morph.preset_node[slot] < 0
            && mesh >= 0
            && index >= 0
        {
            st.morph.preset_mesh[slot] = mesh as i16;
            st.morph.preset_k[slot] = index as i16;
        }
    }
    Ok(())
}

/// VRM 0.x preset names are upper-case (`"BLINK"`), so compare case-insensitively.
fn vrm0_preset_slot(name: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < MORPH_PRESET_COUNT {
        if name_eq_ci(name, preset_name(i)) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Turn the VRM bindings collected in pass 1 into flat arena target indices.
///
/// Runs after pass 2, when every morph target has been appended and its
/// (mesh, target index) pair recorded in `book`.
fn resolve_presets(book: &mut MorphBook, arena: &ModelArena) {
    for slot in 0..MORPH_PRESET_COUNT {
        let mut mesh: i32 = -1;
        if book.preset_node[slot] >= 0 {
            let node = book.preset_node[slot] as usize;
            if node < arena.node_count {
                mesh = arena.nodes[node].mesh as i32;
            }
        } else if book.preset_mesh[slot] >= 0 {
            mesh = book.preset_mesh[slot] as i32;
        }
        let k = book.preset_k[slot];
        let mut flat: i16 = -1;
        if mesh >= 0 && k >= 0 {
            let mut f = 0;
            while f < book.nflat && f < arena.morph_target_count {
                if book.flat_mesh[f] as i32 == mesh && book.flat_k[f] as i16 == k {
                    flat = f as i16;
                    break;
                }
                f += 1;
            }
        }
        if flat < 0 {
            // No explicit binding: fall back to a target named after the preset.
            let want = preset_name(slot);
            let mut i = 0;
            while i < book.nnames {
                let n = book.name_len[i] as usize;
                if book.name_target[i] >= 0 && name_eq_ci(&book.name[i][..n], want) {
                    flat = book.name_target[i];
                    break;
                }
                i += 1;
            }
        }
        book.preset_flat[slot] = flat;
    }
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

/// Largest accessor element accepted: 16 components * 4 bytes (MAT4 float).
const ELEM_MAX: usize = 64;

/// A validated accessor ready to be read element by element.
#[derive(Clone, Copy)]
struct AccInfo {
    /// Absolute file offset of element 0, or 0 when `has_base` is false.
    off: u64,
    stride: u64,
    elem: usize,
    count: u32,
    comp: u16,
    comps: usize,
    normalized: bool,
    /// False for a sparse-only accessor (no base `bufferView`): base is zero.
    has_base: bool,
    /// Resolved sparse substitution table, if the accessor is sparse.
    sparse: Option<SparseRun>,
}

/// A validated sparse substitution table, with absolute file offsets.
#[derive(Clone, Copy)]
struct SparseRun {
    /// Number of substituted elements.
    count: u32,
    /// Absolute file offset of sparse index 0.
    idx_off: u64,
    /// Size of one sparse index (1, 2 or 4).
    idx_size: usize,
    /// Sparse index component type (5121/5123/5125).
    idx_comp: u16,
    /// Absolute file offset of sparse value 0.
    val_off: u64,
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
    let csz = comp_size(a.comp);
    if csz == 0 || a.comps == 0 || (a.off == 0 && a.count == 0 && a.sparse < 0) {
        return Err(LoadError::BadAccessor);
    }
    if a.sparse == -2 {
        // The sparse descriptor table was full; this accessor cannot be served.
        return Err(LoadError::Unsupported);
    }
    let elem = csz.saturating_mul(a.comps as usize);
    if elem == 0 || elem > ELEM_MAX {
        return Err(LoadError::BadAccessor);
    }
    // A sparse accessor may have no base bufferView at all; its base values are
    // then all zero and only the substituted elements carry data.
    let has_base = a.view >= 0;
    if !has_base && a.sparse < 0 {
        // No bufferView and no sparse means no data at all.
        return Err(LoadError::BadAccessor);
    }
    if glb.bin_len == 0 {
        return Err(LoadError::Unsupported);
    }

    // --- base element range ---
    let mut base_off = 0u64;
    let mut stride = elem as u64;
    if has_base {
        let vi = a.view as usize;
        if vi >= st.views_total {
            return Err(LoadError::BadJson);
        }
        if vi >= st.nviews {
            return Err(LoadError::Unsupported);
        }
        let v = st.views[vi];
        stride = if v.stride != 0 {
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
        base_off = glb.bin_off.saturating_add(v.off).saturating_add(a.off);
    }

    // --- sparse substitution table ---
    let sparse = if a.sparse >= 0 {
        let si = a.sparse as usize;
        if si >= st.nsparse {
            return Err(LoadError::BadAccessor);
        }
        let sp = st.sparses[si];
        let isz = comp_size(sp.idx_comp);
        if isz == 0 || !matches!(sp.idx_comp, 5121 | 5123 | 5125) {
            return Err(LoadError::BadAccessor);
        }
        if sp.idx_view < 0 || sp.val_view < 0 {
            return Err(LoadError::BadAccessor);
        }
        let iv = sp.idx_view as usize;
        let vv = sp.val_view as usize;
        if iv >= st.views_total || vv >= st.views_total {
            return Err(LoadError::BadJson);
        }
        if iv >= st.nviews || vv >= st.nviews {
            return Err(LoadError::Unsupported);
        }
        let iview = st.views[iv];
        let vview = st.views[vv];
        let cnt = sp.count as u64;
        if cnt.saturating_mul(isz as u64) > iview.len
            || cnt.saturating_mul(elem as u64) > vview.len
        {
            return Err(LoadError::BadAccessor);
        }
        if iview.off.saturating_add(iview.len) > glb.bin_len
            || vview.off.saturating_add(vview.len) > glb.bin_len
        {
            return Err(LoadError::BadAccessor);
        }
        Some(SparseRun {
            count: sp.count,
            idx_off: glb.bin_off
                .saturating_add(iview.off)
                .saturating_add(sp.idx_off),
            idx_size: isz,
            idx_comp: sp.idx_comp,
            val_off: glb.bin_off
                .saturating_add(vview.off)
                .saturating_add(sp.val_off),
        })
    } else {
        None
    };

    Ok(AccInfo {
        off: base_off,
        stride,
        elem,
        count: a.count,
        comp: a.comp,
        comps: a.comps as usize,
        normalized: a.normalized,
        has_base,
        sparse,
    })
}

/// Sequential element reader. Tightly packed accessors are read through a
/// block buffer (one `read_at` per ~1 KiB); strided ones fall back to one
/// positional read per element.
///
/// Sparse substitution is applied per element. glTF requires sparse indices to
/// be strictly increasing, so the index and value lists are merged in one
/// forward pass instead of being held in RAM. Out-of-order indices are dropped
/// (the file is malformed; the request is simply ignored).
struct ElemStream<'r> {
    r: &'r mut dyn ReadAt,
    off: u64,
    stride: u64,
    elem: usize,
    packed: bool,
    has_base: bool,
    idx: u32,
    count: u32,
    /// Sparse substitution table, when the accessor is sparse.
    sparse: Option<SparseRun>,
    /// Ordinal of the next sparse entry to consider.
    sp_k: u32,
    /// Whether `sp_idx` holds a loaded sparse index.
    sp_loaded: bool,
    sp_idx: u32,
    /// Current element bytes, handed back to the caller.
    out: [u8; ELEM_MAX],
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
            packed: ai.has_base && ai.stride == ai.elem as u64 && ai.elem <= 1024,
            has_base: ai.has_base,
            idx: 0,
            count: ai.count,
            sparse: ai.sparse,
            sp_k: 0,
            sp_loaded: false,
            sp_idx: 0,
            out: [0; ELEM_MAX],
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

    /// Read element `i`'s base bytes into `out` (zeros when there is no base
    /// `bufferView`).
    fn read_elem(&mut self, i: u32) -> Result<(), LoadError> {
        let elem = self.elem;
        if !self.has_base {
            let mut k = 0;
            while k < elem {
                self.out[k] = 0;
                k += 1;
            }
            return Ok(());
        }
        if self.packed {
            while self.pos + elem > self.have {
                if !self.fill() {
                    return Err(LoadError::Io);
                }
            }
            let a = self.pos;
            self.pos += elem;
            self.out[..elem].copy_from_slice(&self.buf[a..a + elem]);
        } else {
            let at = self
                .off
                .saturating_add((i as u64).saturating_mul(self.stride));
            if self.r.read_at(at, &mut self.out[..elem]) != elem {
                return Err(LoadError::Io);
            }
        }
        Ok(())
    }

    /// Overwrite `out` with the sparse value for element `i`, if the accessor
    /// substitutes one.
    fn apply_sparse(&mut self, i: u32) -> Result<(), LoadError> {
        let sp = match self.sparse {
            Some(sp) => sp,
            None => return Ok(()),
        };
        loop {
            if !self.sp_loaded {
                if self.sp_k >= sp.count {
                    return Ok(());
                }
                let at = sp
                    .idx_off
                    .saturating_add((self.sp_k as u64).saturating_mul(sp.idx_size as u64));
                let mut b = [0u8; 4];
                if self.r.read_at(at, &mut b[..sp.idx_size]) != sp.idx_size {
                    return Err(LoadError::Io);
                }
                self.sp_idx = match sp.idx_comp {
                    5121 => b[0] as u32,
                    5123 => u16::from_le_bytes([b[0], b[1]]) as u32,
                    _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                };
                self.sp_loaded = true;
            }
            if self.sp_idx < i {
                // A substitution belonging to an element already passed; drop it.
                self.sp_k += 1;
                self.sp_loaded = false;
                continue;
            }
            if self.sp_idx > i {
                return Ok(());
            }
            let at = sp
                .val_off
                .saturating_add((self.sp_k as u64).saturating_mul(self.elem as u64));
            if self.r.read_at(at, &mut self.out[..self.elem]) != self.elem {
                return Err(LoadError::Io);
            }
            self.sp_k += 1;
            self.sp_loaded = false;
            return Ok(());
        }
    }

    fn next(&mut self) -> Result<&[u8], LoadError> {
        if self.idx >= self.count {
            return Err(LoadError::BadAccessor);
        }
        let i = self.idx;
        self.idx += 1;
        self.read_elem(i)?;
        if self.sparse.is_some() {
            self.apply_sparse(i)?;
        }
        Ok(&self.out[..self.elem])
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
    st: &mut State,
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
    st: &mut State,
    arena: &mut ModelArena,
    glb: &Glb,
) -> Result<(), LoadError> {
    s.expect(b'[')?;
    let mut key = [0u8; KEY_MAX];
    let mut mesh_index = 0usize;
    while s.arr_next()? {
        // Targets appended while parsing this mesh's primitives are contiguous.
        let flat_start = arena.morph_target_count;
        // `extras.targetNames` may appear before or after `primitives`, so hold
        // the names locally and attach them once the mesh is fully read.
        let mut names = [[0u8; MORPH_NAME_LEN]; MAX_MORPH_TARGETS];
        let mut name_lens = [0u8; MAX_MORPH_TARGETS];
        let mut name_count = 0usize;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"primitives" => parse_primitives(s, st, arena, glb, mesh_index)?,
                b"extras" => {
                    parse_mesh_extras(s, &mut names, &mut name_lens, &mut name_count)?
                }
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        apply_mesh_names(
            &mut st.morph,
            arena,
            mesh_index,
            flat_start,
            &names,
            &name_lens,
            name_count,
        );
        mesh_index = mesh_index.saturating_add(1);
    }
    Ok(())
}

/// Pull `targetNames` out of a mesh's `extras` object; every other key is
/// skipped. Names are indexed by morph target index.
fn parse_mesh_extras(
    s: &mut Src,
    names: &mut [[u8; MORPH_NAME_LEN]; MAX_MORPH_TARGETS],
    lens: &mut [u8; MAX_MORPH_TARGETS],
    count: &mut usize,
) -> Result<(), LoadError> {
    s.expect(b'{')?;
    let mut key = [0u8; KEY_MAX];
    loop {
        let n = match s.obj_key(&mut key)? {
            Some(n) => n,
            None => break,
        };
        match &key[..n] {
            b"targetNames" => {
                s.expect(b'[')?;
                let mut i = 0usize;
                while s.arr_next()? {
                    let mut name = [0u8; MORPH_NAME_LEN];
                    let len = s.string_into(&mut name)?;
                    if i < MAX_MORPH_TARGETS && *count < MAX_MORPH_TARGETS {
                        names[*count] = name;
                        lens[*count] = len as u8;
                        *count += 1;
                    }
                    i += 1;
                }
            }
            _ => s.skip_value(MAX_DEPTH)?,
        }
    }
    Ok(())
}

/// Attach a mesh's `targetNames` entries to the flat arena targets that mesh
/// contributed. With several morph-target-carrying primitives per mesh the
/// first flat target for each index wins, which is the common single-primitive
/// case; the name still resolves to a usable target.
fn apply_mesh_names(
    book: &mut MorphBook,
    arena: &ModelArena,
    mesh_index: usize,
    flat_start: usize,
    names: &[[u8; MORPH_NAME_LEN]; MAX_MORPH_TARGETS],
    lens: &[u8; MAX_MORPH_TARGETS],
    count: usize,
) {
    let mut j = 0usize;
    while j < count {
        let mut target: i16 = -1;
        let mut f = flat_start;
        while f < book.nflat && f < arena.morph_target_count {
            if book.flat_mesh[f] as usize == mesh_index && book.flat_k[f] as usize == j {
                target = f as i16;
                break;
            }
            f += 1;
        }
        if target >= 0 && book.nnames < MAX_MORPH_TARGETS {
            let at = book.nnames;
            book.name[at] = names[j];
            book.name_len[at] = lens[j];
            book.name_target[at] = target;
            book.nnames = at + 1;
        }
        j += 1;
    }
}

fn parse_primitives(
    s: &mut Src,
    st: &mut State,
    arena: &mut ModelArena,
    glb: &Glb,
    mesh_index: usize,
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
        // POSITION accessors of `targets[k]`, collected while the object is
        // streamed, then applied after the primitive's vertices exist.
        let mut target_accs = [-1i64; MAX_MORPH_TARGETS];
        let mut target_count = 0usize;
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
                b"targets" => target_count = parse_targets(s, &mut target_accs)?,
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
        if let Some((v0, n)) = build_primitive(
            s, st, arena, glb, pos, normal, uv, joints, weights, indices, material,
        )? {
            append_morph_targets(
                s,
                st,
                arena,
                glb,
                &target_accs,
                target_count,
                v0,
                n,
                mesh_index,
            )?;
        }
    }
    Ok(())
}

/// Parse `primitives[].targets`: an array of attribute maps, one per morph
/// target. Only `POSITION` deltas are kept; the other semantics (NORMAL,
/// TANGENT) are parsed and dropped.
fn parse_targets(
    s: &mut Src,
    out: &mut [i64; MAX_MORPH_TARGETS],
) -> Result<usize, LoadError> {
    s.expect(b'[')?;
    let mut total = 0usize;
    let mut key = [0u8; KEY_MAX];
    while s.arr_next()? {
        let mut position: i64 = -1;
        s.expect(b'{')?;
        loop {
            let n = match s.obj_key(&mut key)? {
                Some(n) => n,
                None => break,
            };
            match &key[..n] {
                b"POSITION" => position = s.number_i64()?,
                _ => s.skip_value(MAX_DEPTH)?,
            }
        }
        if total < MAX_MORPH_TARGETS {
            out[total] = position;
        }
        total += 1;
    }
    Ok(if total < MAX_MORPH_TARGETS {
        total
    } else {
        MAX_MORPH_TARGETS
    })
}

/// Flatten one primitive's morph targets into the arena.
///
/// Each target becomes a contiguous run of [`MorphEntry`] records holding only
/// the nonzero deltas, with `vertex` as an absolute arena vertex index. Targets
/// are recorded even when they are empty or unusable so that target indices
/// stay aligned with `extras.targetNames` and VRM bindings. Hitting
/// `MAX_MORPH_TARGETS` or `MAX_MORPH_ENTRIES` truncates the tail rather than
/// failing the load; a malformed target accessor is also skipped softly, since
/// expressions are decorative.
#[allow(clippy::too_many_arguments)]
fn append_morph_targets(
    s: &mut Src,
    st: &mut State,
    arena: &mut ModelArena,
    glb: &Glb,
    accs: &[i64; MAX_MORPH_TARGETS],
    ntargets: usize,
    v0: usize,
    nverts: usize,
    mesh_index: usize,
) -> Result<(), LoadError> {
    let mut k = 0usize;
    while k < ntargets {
        if arena.morph_target_count >= MAX_MORPH_TARGETS {
            break;
        }
        let first = arena.morph_entry_count as u32;
        if accs[k] >= 0 {
            if let Ok(ai) = acc_info(st, accs[k] as usize, glb) {
                if ai.comp == 5126 && ai.comps >= 3 {
                    let limit = (ai.count as usize).min(nverts);
                    let mut es = ElemStream::new(&mut *s.r, &ai);
                    let mut out = [0.0f32; 16];
                    let mut i = 0usize;
                    while i < limit {
                        if arena.morph_entry_count >= MAX_MORPH_ENTRIES {
                            break;
                        }
                        let b = match es.next() {
                            Ok(b) => b,
                            Err(_) => break,
                        };
                        decode_f32(b, ai.comp, ai.comps, ai.normalized, &mut out);
                        let delta = [out[0], out[1], out[2]];
                        if delta[0] != 0.0 || delta[1] != 0.0 || delta[2] != 0.0 {
                            arena.morph_entries[arena.morph_entry_count] = MorphEntry {
                                vertex: (v0 + i) as u16,
                                delta,
                            };
                            arena.morph_entry_count += 1;
                        }
                        i += 1;
                    }
                }
            }
        }
        let t = arena.morph_target_count;
        arena.morph_targets[t] = MorphTarget {
            first,
            count: arena.morph_entry_count as u32 - first,
        };
        let book = &mut st.morph;
        if t < MAX_MORPH_TARGETS {
            book.flat_mesh[t] = mesh_index.min(u16::MAX as usize) as u16;
            book.flat_k[t] = k as u8;
            book.nflat = t + 1;
        }
        arena.morph_target_count = t + 1;
        k += 1;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_primitive(
    s: &mut Src,
    st: &mut State,
    arena: &mut ModelArena,
    glb: &Glb,
    acc_pos: i64,
    acc_norm: i64,
    acc_uv: i64,
    acc_joints: i64,
    acc_weights: i64,
    acc_indices: i64,
    material: i64,
) -> Result<Option<(usize, usize)>, LoadError> {
    if acc_pos < 0 {
        return Ok(None);
    }
    let ai = acc_info(st, acc_pos as usize, glb)?;
    if ai.comps < 3 || ai.comp != 5126 {
        return Err(LoadError::BadAccessor);
    }
    let n = ai.count as usize;
    if n == 0 {
        return Ok(None);
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
            return Ok(None);
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
    Ok(Some((v0, n)))
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
/// into `out`, which is at most `TEX_MAX_DIM` square.
///
/// Returns false for anything outside the supported subset; the caller then
/// keeps the material's base colour. The zlib stream is inflated one scanline
/// at a time: compressed bytes are pulled from the IDAT chunks on demand, a
/// 32 KiB LZ77 ring supplies back-references, each scanline is unfiltered into
/// a static row buffer, and every pixel is accumulated straight into the
/// 128x128 destination. Memory is constant (the four `PNG_*` statics plus one
/// row accumulator) whether the source is 256 px or 4096 px wide.
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

    // --- chunk walk: keep the IHDR fields and remember where IDAT starts ---
    let mut w = 0usize;
    let mut h = 0usize;
    let mut depth = 0u8;
    let mut colour = 0u8;
    let mut interlace = 0u8;
    let mut idat_hdr = 0u64;
    let mut off = base.saturating_add(8);
    let mut hdr = [0u8; 8];
    while off.saturating_add(12) <= end {
        if read.read_at(off, &mut hdr) != 8 {
            return false;
        }
        let clen = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let data = off.saturating_add(8);
        if data.saturating_add(clen).saturating_add(4) > end {
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
            if idat_hdr == 0 {
                idat_hdr = off;
            }
        } else if kind == *b"IEND" {
            break;
        }
        off = data.saturating_add(clen).saturating_add(4);
    }

    if w == 0 || h == 0 || depth != 8 || interlace != 0 {
        return false;
    }
    let bpp = match colour {
        2 => 3usize, // RGB
        6 => 4usize, // RGBA
        _ => return false,
    };
    if idat_hdr == 0 || w > PNG_MAX_W || h > PNG_MAX_H {
        return false;
    }
    if w.saturating_mul(h) > PNG_MAX_PIXELS {
        return false;
    }
    let stride = w * bpp;
    let dw = if w < TEX_MAX_DIM { w } else { TEX_MAX_DIM };
    let dh = if h < TEX_MAX_DIM { h } else { TEX_MAX_DIM };
    if dw == 0 || dh == 0 || stride == 0 || stride > PNG_ROW_CAP {
        return false;
    }

    let mut dec = PngDec {
        read,
        next_hdr: idat_hdr,
        end,
        payload_off: 0,
        chunk_left: 0,
        inbuf: PNG_IN.get(),
        in_pos: 0,
        in_len: 0,
        inp_eof: false,
        bitbuf: 0,
        bitcnt: 0,
        win: PNG_WIN_BUF.get(),
        win_pos: 0,
        win_written: 0,
        row: PNG_ROW.get(),
        prev: PNG_PREV.get(),
        w,
        h,
        dw,
        dh,
        stride,
        bpp,
        y: 0,
        xi: 0,
        filt: 0,
        in_row: false,
        cur_dy: usize::MAX,
        acc: [[0u32; 4]; TEX_MAX_DIM],
        cnt: [0u32; TEX_MAX_DIM],
        out,
    };
    // Skip the 2-byte zlib header; the CMF/FLG check rejects non-deflate data.
    let z = dec.bits(16);
    if dec.inp_eof {
        return false;
    }
    let cmf = (z & 0xff) as u32;
    let flg = (z >> 8) as u32;
    if (cmf & 0x0f) != 8 || (cmf * 256 + flg) % 31 != 0 {
        return false;
    }
    if !dec.inflate() {
        return false;
    }
    out.w = dw as u16;
    out.h = dh as u16;
    out.loaded = true;
    true
}

/// `emit` result: the byte was consumed and more are expected.
const PNG_OK: u8 = 1;
/// `emit` result: the last source scanline is complete, stop inflating.
const PNG_DONE: u8 = 2;

/// Canonical DEFLATE Huffman table.
struct Huff {
    counts: [u16; 16],
    symbols: [u16; 288],
    /// False for an empty code (a malformed block, not a panic).
    valid: bool,
}

/// Build the canonical Huffman table for `lengths` (code length per symbol).
fn build_huff(lengths: &[u8]) -> Huff {
    let mut h = Huff {
        counts: [0; 16],
        symbols: [0; 288],
        valid: false,
    };
    for &l in lengths {
        let l = l as usize;
        if l != 0 && l < 16 {
            h.counts[l] += 1;
            h.valid = true;
        }
    }
    let mut offsets = [0u16; 16];
    let mut len = 1usize;
    while len < 16 {
        offsets[len] = offsets[len - 1].saturating_add(h.counts[len - 1]);
        len += 1;
    }
    let mut sym = 0usize;
    while sym < lengths.len() && sym < 288 {
        let l = lengths[sym] as usize;
        if l != 0 && l < 16 {
            let o = offsets[l] as usize;
            if o < 288 {
                h.symbols[o] = sym as u16;
            }
            offsets[l] = offsets[l].saturating_add(1);
        }
        sym += 1;
    }
    h
}

/// Streaming PNG decoder: pulls the IDAT zlib stream from the file on demand
/// and inflates it one byte at a time into a 32 KiB history ring, a scanline
/// buffer and the 128x128 destination accumulator.
struct PngDec<'r> {
    // --- compressed input (chunk walk, on demand) ---
    read: &'r mut dyn ReadAt,
    /// File offset of the next chunk header to inspect.
    next_hdr: u64,
    /// End of this image's blob.
    end: u64,
    /// Offset of the next byte of the current IDAT payload.
    payload_off: u64,
    /// Bytes left in the current IDAT payload.
    chunk_left: u64,
    inbuf: &'r mut [u8],
    in_pos: usize,
    in_len: usize,
    inp_eof: bool,
    // --- bit reader ---
    bitbuf: u32,
    bitcnt: u32,
    // --- DEFLATE history ring ---
    win: &'r mut [u8],
    win_pos: usize,
    win_written: usize,
    // --- scanline state ---
    row: &'r mut [u8],
    prev: &'r mut [u8],
    w: usize,
    h: usize,
    dw: usize,
    dh: usize,
    stride: usize,
    bpp: usize,
    /// Source row being assembled.
    y: usize,
    /// Next byte position inside `row`.
    xi: usize,
    /// Filter type of the row being assembled.
    filt: u8,
    /// False while the next decompressed byte is a filter type.
    in_row: bool,
    /// Destination row currently accumulating, or `usize::MAX` for none.
    cur_dy: usize,
    acc: [[u32; 4]; TEX_MAX_DIM],
    cnt: [u32; TEX_MAX_DIM],
    out: &'r mut Texture,
}

impl<'r> PngDec<'r> {
    /// Make sure at least one compressed byte is buffered, walking IDAT chunks
    /// as needed. Returns false at end of stream or on a malformed chunk.
    fn fill(&mut self) -> bool {
        while self.in_pos >= self.in_len {
            if self.chunk_left > 0 {
                let want = if (self.chunk_left as usize) < self.inbuf.len() {
                    self.chunk_left as usize
                } else {
                    self.inbuf.len()
                };
                let got = self.read.read_at(self.payload_off, &mut self.inbuf[..want]);
                if got == 0 {
                    self.inp_eof = true;
                    return false;
                }
                self.payload_off = self.payload_off.saturating_add(got as u64);
                self.chunk_left -= got as u64;
                self.in_pos = 0;
                self.in_len = got;
                return true;
            }
            // No payload left: read the next chunk header.
            if self.next_hdr.saturating_add(12) > self.end {
                self.inp_eof = true;
                return false;
            }
            let mut hdr = [0u8; 8];
            if self.read.read_at(self.next_hdr, &mut hdr) != 8 {
                self.inp_eof = true;
                return false;
            }
            let clen = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
            let data = self.next_hdr.saturating_add(8);
            let next = data.saturating_add(clen).saturating_add(4);
            if next > self.end {
                self.inp_eof = true;
                return false;
            }
            let kind = [hdr[4], hdr[5], hdr[6], hdr[7]];
            self.next_hdr = next;
            if kind == *b"IEND" {
                self.inp_eof = true;
                return false;
            }
            if kind == *b"IDAT" && clen > 0 {
                self.payload_off = data;
                self.chunk_left = clen;
            }
        }
        true
    }

    /// Read `need` (1..=16) bits LSB-first from the DEFLATE stream.
    fn bits(&mut self, need: u32) -> u32 {
        while self.bitcnt < need {
            if !self.fill() {
                self.inp_eof = true;
                return 0;
            }
            self.bitbuf |= (self.inbuf[self.in_pos] as u32) << self.bitcnt;
            self.in_pos += 1;
            self.bitcnt += 8;
        }
        let v = self.bitbuf & ((1u32 << need) - 1);
        self.bitbuf >>= need;
        self.bitcnt -= need;
        v
    }

    /// Decode one Huffman symbol; the caller must check `inp_eof`.
    fn symbol(&mut self, h: &Huff) -> u16 {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        let mut len = 1usize;
        while len < 16 {
            code |= self.bits(1) as i32;
            if self.inp_eof {
                return 0;
            }
            let count = h.counts[len] as i32;
            if code - first < count {
                let o = index + (code - first);
                if o < 0 || o >= 288 {
                    return 0;
                }
                return h.symbols[o as usize];
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
            len += 1;
        }
        0
    }

    /// Push one decompressed byte through the filter and downsampler.
    fn emit(&mut self, byte: u8) -> u8 {
        self.win[self.win_pos] = byte;
        self.win_pos = (self.win_pos + 1) & (PNG_WIN - 1);
        self.win_written = self.win_written.saturating_add(1);
        if !self.in_row {
            // Scanline start: the first byte is the filter type.
            if byte > 4 {
                return 0;
            }
            self.filt = byte;
            self.xi = 0;
            self.in_row = true;
            return PNG_OK;
        }
        let x = self.xi;
        if x >= self.stride {
            return 0;
        }
        let bpp = self.bpp;
        let left = if x >= bpp { self.row[x - bpp] } else { 0 };
        let up = if self.y > 0 { self.prev[x] } else { 0 };
        let upleft = if self.y > 0 && x >= bpp {
            self.prev[x - bpp]
        } else {
            0
        };
        let v = match self.filt {
            0 => byte,
            1 => byte.wrapping_add(left),
            2 => byte.wrapping_add(up),
            3 => byte.wrapping_add(((left as u16 + up as u16) >> 1) as u8),
            _ => byte.wrapping_add(paeth(left, up, upleft)),
        };
        self.row[x] = v;
        self.xi = x + 1;
        if self.xi >= self.stride {
            return self.end_row();
        }
        PNG_OK
    }

    /// Accumulate the finished scanline, then swap it into `prev`.
    fn end_row(&mut self) -> u8 {
        let dy = self.y.saturating_mul(self.dh) / self.h;
        if dy != self.cur_dy {
            if self.cur_dy < self.dh {
                self.flush(self.cur_dy);
            }
            let mut i = 0usize;
            while i < TEX_MAX_DIM {
                self.acc[i] = [0; 4];
                self.cnt[i] = 0;
                i += 1;
            }
            self.cur_dy = dy;
        }
        let mut x = 0usize;
        while x < self.w {
            let dx = x * self.dw / self.w;
            let s = x * self.bpp;
            let a = &mut self.acc[dx];
            a[0] = a[0].saturating_add(self.row[s] as u32);
            a[1] = a[1].saturating_add(self.row[s + 1] as u32);
            a[2] = a[2].saturating_add(self.row[s + 2] as u32);
            a[3] = a[3].saturating_add(if self.bpp == 4 {
                self.row[s + 3] as u32
            } else {
                255
            });
            self.cnt[dx] = self.cnt[dx].saturating_add(1);
            x += 1;
        }
        core::mem::swap(&mut self.row, &mut self.prev);
        self.y += 1;
        self.in_row = false;
        if self.y >= self.h {
            if self.cur_dy < self.dh {
                self.flush(self.cur_dy);
            }
            return PNG_DONE;
        }
        PNG_OK
    }

    /// Write one accumulator row into the texture, averaging exactly with an
    /// integer divide over every source pixel mapped to that cell.
    fn flush(&mut self, dy: usize) {
        let dw = self.dw;
        let mut dx = 0usize;
        while dx < dw {
            let n = if self.cnt[dx] == 0 { 1 } else { self.cnt[dx] };
            let o = (dy * dw + dx) * 4;
            if o + 3 >= self.out.rgba.len() {
                return;
            }
            self.out.rgba[o] = (self.acc[dx][0] / n).min(255) as u8;
            self.out.rgba[o + 1] = (self.acc[dx][1] / n).min(255) as u8;
            self.out.rgba[o + 2] = (self.acc[dx][2] / n).min(255) as u8;
            self.out.rgba[o + 3] = (self.acc[dx][3] / n).min(255) as u8;
            dx += 1;
        }
    }

    /// One stored (uncompressed) DEFLATE block.
    fn stored_block(&mut self) -> u8 {
        self.bitbuf = 0;
        self.bitcnt = 0;
        let len = self.bits(16) as usize;
        let nlen = self.bits(16);
        if self.inp_eof {
            return 0;
        }
        if (len as u32) ^ nlen != 0xffff {
            return 0;
        }
        let mut i = 0usize;
        while i < len {
            let b = self.bits(8) as u8;
            if self.inp_eof {
                return 0;
            }
            let r = self.emit(b);
            if r != PNG_OK {
                return r;
            }
            i += 1;
        }
        PNG_OK
    }

    /// One Huffman-coded block (fixed or dynamic tables), up to end-of-block.
    fn coded_block(&mut self, lit: &Huff, dist: &Huff) -> u8 {
        if !lit.valid {
            return 0;
        }
        loop {
            let sym = self.symbol(lit);
            if self.inp_eof {
                return 0;
            }
            if sym < 256 {
                let r = self.emit(sym as u8);
                if r != PNG_OK {
                    return r;
                }
            } else if sym == 256 {
                return PNG_OK;
            } else {
                let idx = sym as usize - 257;
                if idx >= 29 || !dist.valid {
                    return 0;
                }
                let length = LEN_BASE[idx] as usize + self.bits(LEN_EXTRA[idx] as u32) as usize;
                let dsym = self.symbol(dist) as usize;
                if self.inp_eof || dsym >= 30 {
                    return 0;
                }
                let distance =
                    DIST_BASE[dsym] as usize + self.bits(DIST_EXTRA[dsym] as u32) as usize;
                if self.inp_eof || distance == 0 || distance > self.win_written {
                    return 0;
                }
                let mut i = 0usize;
                while i < length {
                    let b = self.win[(self.win_pos.wrapping_sub(distance)) & (PNG_WIN - 1)];
                    let r = self.emit(b);
                    if r != PNG_OK {
                        return r;
                    }
                    i += 1;
                }
            }
        }
    }

    fn fixed_block(&mut self) -> u8 {
        let mut lengths = [0u8; 288];
        let mut i = 0usize;
        while i < 288 {
            lengths[i] = if i < 144 {
                8
            } else if i < 256 {
                9
            } else if i < 280 {
                7
            } else {
                8
            };
            i += 1;
        }
        let lit = build_huff(&lengths);
        let dist = build_huff(&[5u8; 30]);
        self.coded_block(&lit, &dist)
    }

    fn dynamic_block(&mut self) -> u8 {
        let hlit = self.bits(5) as usize + 257;
        let hdist = self.bits(5) as usize + 1;
        let hclen = self.bits(4) as usize + 4;
        if self.inp_eof || hlit > 286 || hdist > 30 {
            return 0;
        }
        const ORDER: [usize; 19] = [
            16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
        ];
        let mut code_lengths = [0u8; 19];
        let mut i = 0usize;
        while i < hclen {
            let v = self.bits(3) as u8;
            if self.inp_eof {
                return 0;
            }
            code_lengths[ORDER[i]] = v;
            i += 1;
        }
        let code_huff = build_huff(&code_lengths);
        let mut lengths = [0u8; 320];
        let total = hlit + hdist;
        let mut i = 0usize;
        while i < total {
            let sym = self.symbol(&code_huff);
            if self.inp_eof {
                return 0;
            }
            if sym < 16 {
                lengths[i] = sym as u8;
                i += 1;
            } else if sym == 16 {
                if i == 0 {
                    return 0;
                }
                let prev = lengths[i - 1];
                let repeat = 3 + self.bits(2) as usize;
                if self.inp_eof {
                    return 0;
                }
                let mut k = 0usize;
                while k < repeat && i < total {
                    lengths[i] = prev;
                    i += 1;
                    k += 1;
                }
            } else if sym == 17 || sym == 18 {
                let repeat = if sym == 17 {
                    3 + self.bits(3) as usize
                } else {
                    11 + self.bits(7) as usize
                };
                if self.inp_eof {
                    return 0;
                }
                let mut k = 0usize;
                while k < repeat && i < total {
                    lengths[i] = 0;
                    i += 1;
                    k += 1;
                }
            } else {
                return 0;
            }
        }
        let lit = build_huff(&lengths[..hlit]);
        let dist = build_huff(&lengths[hlit..total]);
        self.coded_block(&lit, &dist)
    }

    /// Inflate until the last scanline has been consumed. True only when every
    /// source row arrived: a truncated stream leaves the texture unloaded.
    fn inflate(&mut self) -> bool {
        loop {
            let final_block = self.bits(1);
            let typ = self.bits(2);
            if self.inp_eof {
                return false;
            }
            let r = match typ {
                0 => self.stored_block(),
                1 => self.fixed_block(),
                2 => self.dynamic_block(),
                _ => return false,
            };
            if r == PNG_DONE {
                return true;
            }
            if r != PNG_OK {
                return false;
            }
            if final_block == 1 {
                return false;
            }
        }
    }
}

/// DEFLATE length bases and extra-bit counts (RFC 1951 section 3.2.5).
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// DEFLATE distance bases and extra-bit counts (RFC 1951 section 3.2.5).
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
