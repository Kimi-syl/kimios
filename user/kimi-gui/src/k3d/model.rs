//! Fixed-capacity model arenas and the streaming reader contract.
//!
//! There is no allocator in the GUI, so a loaded model lives in arrays sized at
//! compile time. The loader fills these in place; nothing is ever reallocated.

use super::Vec3;

/// Vertices retained at once. A 300 px character needs far fewer than this; the
/// ceiling is what keeps a hostile file from running the loader out of arena.
pub const MAX_VERTS: usize = 8192;
/// Indices retained at once (triangles = indices / 3).
pub const MAX_TRIS: usize = 16384;
/// Joints across all skins.
pub const MAX_JOINTS: usize = 128;
/// Primitives (draw calls).
pub const MAX_PRIMS: usize = 16;
/// Material slots.
pub const MAX_MATERIALS: usize = 16;
/// Node records (scene graph).
pub const MAX_NODES: usize = 256;
/// Texture slots.
pub const MAX_TEXTURES: usize = 4;
/// Longest edge of a decoded texture. Textures are box-downsampled on load.
pub const TEX_MAX_DIM: usize = 128;

/// Morph targets (facial expressions) retained at once.
pub const MAX_MORPH_TARGETS: usize = 16;
/// Total sparse (vertex, delta) entries across all morph targets.
pub const MAX_MORPH_ENTRIES: usize = 16384;

/// One morph target: a contiguous run of [`MorphEntry`] records.
#[derive(Clone, Copy, Default)]
pub struct MorphTarget {
    pub first: u32,
    pub count: u32,
}

/// A sparse position delta for one vertex, as glTF morph targets store them.
#[derive(Clone, Copy, Default)]
pub struct MorphEntry {
    pub vertex: u16,
    pub delta: [f32; 3],
}

/// One skinned vertex. 56 bytes, so the vertex arena is ~448 KB.
#[derive(Clone, Copy)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

impl Default for Vertex {
    fn default() -> Self {
        Vertex {
            pos: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            uv: [0.0; 2],
            joints: [0; 4],
            weights: [0.0; 4],
        }
    }
}

impl Vertex {
    pub fn position(&self) -> Vec3 {
        Vec3::new(self.pos[0], self.pos[1], self.pos[2])
    }

    pub fn normal(&self) -> Vec3 {
        Vec3::new(self.normal[0], self.normal[1], self.normal[2])
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}

#[derive(Clone, Copy)]
pub struct Material {
    /// Linear-ish base colour times alpha.
    pub base_color: [f32; 4],
    /// Index into the texture table, or `NO_TEXTURE`.
    pub texture: u16,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    /// Emissive/shadeless factor, used by the MToon fallback.
    pub unlit: bool,
}

pub const NO_TEXTURE: u16 = u16::MAX;

impl Default for Material {
    fn default() -> Self {
        Material {
            base_color: [1.0, 1.0, 1.0, 1.0],
            texture: NO_TEXTURE,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            unlit: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Primitive {
    /// Offset into the shared index list.
    pub first_index: u32,
    pub index_count: u32,
    pub material: u16,
}

#[derive(Clone, Copy)]
pub struct Node {
    pub translation: [f32; 3],
    /// Quaternion xyzw.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Parent node index, or -1 for a root.
    pub parent: i16,
    /// Mesh index, or -1 when the node has no mesh.
    pub mesh: i16,
    pub skin: i16,
    /// Humanoid bone id when this node is a mapped bone, else `BONE_NONE`.
    pub bone: u8,
}

pub const BONE_NONE: u8 = 255;

impl Default for Node {
    fn default() -> Self {
        Node {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            parent: -1,
            mesh: -1,
            skin: -1,
            bone: BONE_NONE,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Skin {
    /// Offset into the joint->node table.
    pub first_joint: u16,
    pub joint_count: u16,
    /// Offset into the inverse bind matrix table (16 floats each).
    pub first_ibm: u16,
}

/// A decoded texture: RGBA8, `w * h` pixels.
pub struct Texture {
    pub w: u16,
    pub h: u16,
    pub rgba: [u8; TEX_MAX_DIM * TEX_MAX_DIM * 4],
    pub loaded: bool,
}

impl Texture {
    pub const fn empty() -> Self {
        Texture {
            w: 0,
            h: 0,
            rgba: [0; TEX_MAX_DIM * TEX_MAX_DIM * 4],
            loaded: false,
        }
    }

    pub fn sample(&self, u: f32, v: f32) -> [u8; 4] {
        if !self.loaded || self.w == 0 || self.h == 0 {
            return [255, 255, 255, 255];
        }
        // Nearest neighbour, wrapped; the GUI is nearest-neighbour throughout.
        let w = self.w as i32;
        let h = self.h as i32;
        let mut x = (u * w as f32) as i32;
        // glTF stores UVs with the origin at the top-left, which matches the PNG
        // row order the decoder produces, so `v` maps straight to a row.
        let mut y = (v.clamp(0.0, 1.0) * h as f32) as i32;
        x = ((x % w) + w) % w;
        y = ((y % h) + h) % h;
        let o = ((y * w + x) * 4) as usize;
        [
            self.rgba[o],
            self.rgba[o + 1],
            self.rgba[o + 2],
            self.rgba[o + 3],
        ]
    }
}

/// Everything a loaded model occupies.
pub struct ModelArena {
    pub vertices: [Vertex; MAX_VERTS],
    pub vertex_count: usize,
    pub indices: [u32; MAX_TRIS],
    pub index_count: usize,
    pub primitives: [Primitive; MAX_PRIMS],
    pub primitive_count: usize,
    pub materials: [Material; MAX_MATERIALS],
    pub material_count: usize,
    pub nodes: [Node; MAX_NODES],
    pub node_count: usize,
    pub skins: [Skin; 1],
    pub skin_count: usize,
    /// Joint index -> node index.
    pub joint_nodes: [u16; MAX_JOINTS],
    /// Joint index -> 4x4 inverse bind matrix (column-major).
    pub joint_ibm: [[f32; 16]; MAX_JOINTS],
    pub joint_count: usize,
    pub textures: [Texture; MAX_TEXTURES],
    pub morph_targets: [MorphTarget; MAX_MORPH_TARGETS],
    pub morph_target_count: usize,
    pub morph_entries: [MorphEntry; MAX_MORPH_ENTRIES],
    pub morph_entry_count: usize,
    /// Per-target weights, driven by the expression system each frame.
    pub morph_weights: [f32; MAX_MORPH_TARGETS],
    /// Model-space bounds, used to frame the camera.
    pub min: Vec3,
    pub max: Vec3,
    pub loaded: bool,
}

impl ModelArena {
    pub const fn new() -> Self {
        ModelArena {
            vertices: [Vertex {
                pos: [0.0; 3],
                normal: [0.0, 1.0, 0.0],
                uv: [0.0; 2],
                joints: [0; 4],
                weights: [0.0; 4],
            }; MAX_VERTS],
            vertex_count: 0,
            indices: [0; MAX_TRIS],
            index_count: 0,
            primitives: [Primitive {
                first_index: 0,
                index_count: 0,
                material: 0,
            }; MAX_PRIMS],
            primitive_count: 0,
            materials: [Material {
                base_color: [1.0, 1.0, 1.0, 1.0],
                texture: NO_TEXTURE,
                alpha_mode: AlphaMode::Opaque,
                alpha_cutoff: 0.5,
                double_sided: false,
                unlit: false,
            }; MAX_MATERIALS],
            material_count: 0,
            nodes: [Node {
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                parent: -1,
                mesh: -1,
                skin: -1,
                bone: BONE_NONE,
            }; MAX_NODES],
            node_count: 0,
            skins: [Skin {
                first_joint: 0,
                joint_count: 0,
                first_ibm: 0,
            }; 1],
            skin_count: 0,
            joint_nodes: [0; MAX_JOINTS],
            joint_ibm: [[0.0; 16]; MAX_JOINTS],
            joint_count: 0,
            textures: [
                Texture::empty(),
                Texture::empty(),
                Texture::empty(),
                Texture::empty(),
            ],
            morph_targets: [MorphTarget { first: 0, count: 0 }; MAX_MORPH_TARGETS],
            morph_target_count: 0,
            morph_entries: [MorphEntry { vertex: 0, delta: [0.0; 3] }; MAX_MORPH_ENTRIES],
            morph_entry_count: 0,
            morph_weights: [0.0; MAX_MORPH_TARGETS],
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(0.0, 1.0, 0.0),
            loaded: false,
        }
    }

    pub fn reset(&mut self) {
        self.vertex_count = 0;
        self.index_count = 0;
        self.primitive_count = 0;
        self.material_count = 0;
        self.node_count = 0;
        self.skin_count = 0;
        self.joint_count = 0;
        self.morph_target_count = 0;
        self.morph_entry_count = 0;
        for w in self.morph_weights.iter_mut() {
            *w = 0.0;
        }
        self.loaded = false;
        for t in self.textures.iter_mut() {
            t.loaded = false;
            t.w = 0;
            t.h = 0;
        }
    }

    /// Recompute model-space bounds from the vertex positions.
    pub fn recompute_bounds(&mut self) {
        if self.vertex_count == 0 {
            return;
        }
        let mut min = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max = Vec3::new(-f32::MAX, -f32::MAX, -f32::MAX);
        for v in self.vertices[..self.vertex_count].iter() {
            let p = v.position();
            min = min.min(p);
            max = max.max(p);
        }
        self.min = min;
        self.max = max;
    }

    pub fn triangle_count(&self) -> usize {
        self.index_count / 3
    }
}

/// Positional reader, so a GLB can be streamed instead of held in RAM.
pub trait ReadAt {
    /// Fill up to `buf.len()` bytes starting at `offset`; returns bytes read.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> usize;
}

/// A reader over an in-memory slice (host tests, small assets).
pub struct SliceReader<'a>(pub &'a [u8]);

impl<'a> ReadAt for SliceReader<'a> {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> usize {
        let start = offset as usize;
        if start >= self.0.len() {
            return 0;
        }
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        n
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not a GLB: bad magic, version or truncated header.
    BadContainer,
    /// JSON chunk failed to parse or a required field was missing.
    BadJson,
    /// An accessor pointed outside the binary chunk.
    BadAccessor,
    /// Ran out of a fixed arena.
    ArenaFull,
    /// The file uses a feature this loader deliberately does not support.
    Unsupported,
    /// Short read from the underlying reader.
    Io,
}

impl ModelArena {
    /// Convenience: load from a byte slice (host tests).
    pub fn load_from_slice(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        let mut r = SliceReader(bytes);
        super::gltf::load(&mut r, bytes.len() as u64, self)
    }
}