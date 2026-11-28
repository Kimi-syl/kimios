//! 3D pet renderer: owns the offscreen target and the model arena, renders the
//! character into a small buffer and blits it 1:1 into the GUI back buffer.
//!
//! Rendering offscreen at the display size is what makes software 3D affordable
//! here: a 192x256 target is ~49k pixels, versus 2.07M for the full framebuffer.

use crate::draw::Screen;
use crate::k;
use crate::k3d::model::{
    AlphaMode, LoadError, Material, ModelArena, Primitive, ReadAt, Vertex, MAX_NODES, NO_TEXTURE,
    TEX_MAX_DIM,
};
use crate::k3d::raster::{self, DrawParams, DrawStats};
use crate::k3d::skin::{self, Pose, BONE_COUNT};
use crate::k3d::{Camera, Mat4, Quat, Target, Vec3, MAX_JOINTS, MAX_TRIS, MAX_VERTS};

/// Offscreen render target size. Fixed: the blit scales it to the pet's rect.
pub const RT_W: usize = 192;
pub const RT_H: usize = 256;

static mut COLOR: [u32; RT_W * RT_H] = [0; RT_W * RT_H];
static mut DEPTH: [u16; RT_W * RT_H] = [0; RT_W * RT_H];
static mut MODEL: ModelArena = ModelArena::new();
static mut WORLD: [Mat4; MAX_NODES] = [Mat4::IDENTITY; MAX_NODES];
static mut JOINTS: [Mat4; MAX_JOINTS] = [Mat4::IDENTITY; MAX_JOINTS];
static mut TEXTURE_DATA: [[u8; TEX_MAX_DIM * TEX_MAX_DIM * 4]; 1] =
    [[0; TEX_MAX_DIM * TEX_MAX_DIM * 4]; 1];
static mut POSE: Pose = Pose {
    rotation: [Quat::IDENTITY; BONE_COUNT],
    root_y: 0.0,
    lean: 0.0,
};
/// Per-vertex morph deltas for the current frame.
const MAX_VERTS_USIZE: usize = crate::k3d::MAX_VERTS;
static mut MORPH_DELTA: [Vec3; MAX_VERTS_USIZE] = [Vec3::ZERO; MAX_VERTS_USIZE];
static mut MORPH_ACTIVE: bool = false;

/// Set a morph target weight (0..1), e.g. a facial expression.
pub fn set_morph(index: usize, weight: f32) {
    let arena = model();
    if index < arena.morph_weights.len() {
        arena.morph_weights[index] = crate::k3d::clamp(weight, 0.0, 1.0);
    }
}

/// Clear every expression weight.
pub fn clear_morphs() {
    for w in model().morph_weights.iter_mut() {
        *w = 0.0;
    }
}

/// Recompute the per-vertex morph delta buffer from the current weights.
/// Cheap when nothing is active: one flag check.
fn update_morphs() -> Option<&'static [Vec3]> {
    let arena = model();
    if arena.morph_target_count == 0 {
        unsafe { MORPH_ACTIVE = false };
        return None;
    }
    let any = arena.morph_weights[..arena.morph_target_count]
        .iter()
        .any(|w| *w > 0.001);
    if !any {
        unsafe { MORPH_ACTIVE = false };
        return None;
    }
    unsafe {
        let deltas = &mut *core::ptr::addr_of_mut!(MORPH_DELTA);
        for d in deltas.iter_mut() {
            *d = Vec3::ZERO;
        }
        for t in 0..arena.morph_target_count {
            let weight = arena.morph_weights[t];
            if weight <= 0.001 {
                continue;
            }
            let target = arena.morph_targets[t];
            let start = target.first as usize;
            let end = (start + target.count as usize).min(arena.morph_entry_count);
            for e in &arena.morph_entries[start..end] {
                let vi = e.vertex as usize;
                if vi < deltas.len() {
                    deltas[vi] = deltas[vi].add(Vec3::new(
                        e.delta[0] * weight,
                        e.delta[1] * weight,
                        e.delta[2] * weight,
                    ));
                }
            }
        }
        MORPH_ACTIVE = true;
        Some(&(&*core::ptr::addr_of!(MORPH_DELTA))[..arena.vertex_count.min(MAX_VERTS_USIZE)])
    }
}
/// Guards the one-shot model load so a missing file is not retried every frame.
static mut LOAD_ATTEMPTED: bool = false;
/// Timestamp of the last tap, used for the hop animation.
static mut BOUNCE_START: u64 = 0;

/// Trigger a short hop so tapping the 3D pet visibly responds.
pub fn bounce() {
    unsafe {
        BOUNCE_START = k::uptime_ms();
    }
    // Kick the secondary motion so limbs swing from the tap.
    skin::spring_impulse(2.5, -1.5);
}

/// Vertical offset (metres) of the current hop, 0 when idle.
fn bounce_offset() -> f32 {
    const DURATION: f32 = 0.42;
    let start = unsafe { BOUNCE_START };
    if start == 0 {
        return 0.0;
    }
    let age = k::uptime_ms().saturating_sub(start) as f32 / 1000.0;
    if age >= DURATION {
        return 0.0;
    }
    let t = age / DURATION;
    // One decaying arc.
    crate::k3d::sin(t * 3.141_592_7) * (1.0 - t) * 0.16
}

/// Load the model once: prefer `/disk/pet/model.glb`, else the placeholder cube.
fn ensure_model() {
    if model().loaded || unsafe { LOAD_ATTEMPTED } {
        return;
    }
    unsafe {
        LOAD_ATTEMPTED = true;
    }
    if load_from_disk("/disk/pet/model.glb").is_err() {
        build_cube(1.0, Vec3::new(0.0, 1.0, 0.0));
    }
}

/// Last frame's counters, for the FPS overlay.
pub static mut LAST_STATS: DrawStats = DrawStats {
    triangles: 0,
    drawn: 0,
    pixels: 0,
    clipped: 0,
};

fn model() -> &'static mut ModelArena {
    unsafe { &mut *core::ptr::addr_of_mut!(MODEL) }
}

/// Positional reader over a KimiOS file handle, so a GLB is streamed rather than
/// read into RAM (a real VRM buffer is far larger than the user heap).
struct FileReadAt {
    handle: u64,
}

impl ReadAt for FileReadAt {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> usize {
        k::read_file(self.handle, offset as usize, buf)
    }
}

/// Load a `.glb`/`.vrm` from disk into the model arena.
///
/// Returns the loader's error so the UI can report *why* a model was rejected
/// instead of silently falling back.
pub fn load_from_disk(path: &str) -> Result<(), LoadError> {
    let Some((_, size)) = k::stat(path) else {
        return Err(LoadError::Io);
    };
    let Some(handle) = k::open(path) else {
        return Err(LoadError::Io);
    };
    let mut reader = FileReadAt { handle };
    let arena = model();
    let result = crate::k3d::gltf::load(&mut reader, size, arena);
    k::close(handle);
    result
}

/// True when a loaded model carries a skin (i.e. is a character, not a prop).
pub fn has_skin() -> bool {
    let m = model();
    m.joint_count > 0 && m.skin_count > 0
}

/// Build a procedurally textured cube of `size` metres centred at `centre`.
///
/// This exists so the whole pipeline (raster, depth, texturing, blit) can be
/// exercised before a real GLB is available.
pub fn build_cube(size: f32, centre: Vec3) -> bool {
    let m = model();
    m.reset();
    let h = size * 0.5;

    // Texture: 8x8 checker with a coloured border.
    unsafe {
        let tex = &mut TEXTURE_DATA[0];
        let dim = 64usize;
        for y in 0..dim {
            for x in 0..dim {
                let checker = ((x / 8) + (y / 8)) % 2 == 0;
                let border = x < 2 || y < 2 || x >= dim - 2 || y >= dim - 2;
                let (r, g, b) = if border {
                    (240u8, 210u8, 90u8)
                } else if checker {
                    (200, 120, 90)
                } else {
                    (70, 100, 150)
                };
                let o = (y * dim + x) * 4;
                tex[o] = r;
                tex[o + 1] = g;
                tex[o + 2] = b;
                tex[o + 3] = 255;
            }
        }
        m.textures[0].w = dim as u16;
        m.textures[0].h = dim as u16;
        m.textures[0].loaded = true;
        // Copy the generated pixels into the texture arena.
        let src = &*(core::ptr::addr_of!(TEXTURE_DATA[0]));
        for i in 0..(TEX_MAX_DIM * TEX_MAX_DIM * 4) {
            m.textures[0].rgba[i] = src[i];
        }
    }

    // Six faces, four vertices each, so normals and UVs stay per-face.
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        // +X
        ([1.0, 0.0, 0.0], [[h, -h, -h], [h, -h, h], [h, h, h], [h, h, -h]]),
        // -X
        ([-1.0, 0.0, 0.0], [[-h, -h, h], [-h, -h, -h], [-h, h, -h], [-h, h, h]]),
        // +Y
        ([0.0, 1.0, 0.0], [[-h, h, -h], [h, h, -h], [h, h, h], [-h, h, h]]),
        // -Y
        ([0.0, -1.0, 0.0], [[-h, -h, h], [h, -h, h], [h, -h, -h], [-h, -h, -h]]),
        // +Z
        ([0.0, 0.0, 1.0], [[-h, -h, h], [h, -h, h], [h, h, h], [-h, h, h]]),
        // -Z
        ([0.0, 0.0, -1.0], [[h, -h, -h], [-h, -h, -h], [-h, h, -h], [h, h, -h]]),
    ];
    let uvs: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

    let mut v = 0usize;
    let mut i = 0usize;
    for (normal, corners) in faces.iter() {
        for (k, corner) in corners.iter().enumerate() {
            if v >= MAX_VERTS {
                return false;
            }
            m.vertices[v] = Vertex {
                pos: [
                    corner[0] + centre.x,
                    corner[1] + centre.y,
                    corner[2] + centre.z,
                ],
                normal: *normal,
                uv: uvs[k],
                joints: [0; 4],
                weights: [0.0; 4],
            };
            v += 1;
        }
        // Two triangles per face.
        let b = (v - 4) as u32;
        for idx in [b, b + 1, b + 2, b, b + 2, b + 3] {
            if i >= MAX_TRIS {
                return false;
            }
            m.indices[i] = idx;
            i += 1;
        }
    }

    m.vertex_count = v;
    m.index_count = i;
    m.materials[0] = Material {
        base_color: [1.0, 1.0, 1.0, 1.0],
        texture: 0,
        alpha_mode: AlphaMode::Opaque,
        alpha_cutoff: 0.5,
        double_sided: true,
        unlit: false,
    };
    m.material_count = 1;
    m.primitives[0] = Primitive {
        first_index: 0,
        index_count: i as u32,
        material: 0,
    };
    m.primitive_count = 1;
    m.node_count = 0;
    m.joint_count = 0;
    m.loaded = true;
    m.recompute_bounds();
    true
}

/// Read-only view of the loaded model (for UI status text).
pub fn model_stats() -> (usize, usize, bool) {
    let m = model();
    (m.vertex_count, m.triangle_count(), m.loaded)
}

/// Render the model and blit it into the screen rectangle `(x, y, w, h)` in
/// logical coordinates. Pixels the rasteriser did not touch stay transparent.
pub fn draw(screen: &Screen, x: i64, y: i64, w: i64, h: i64, yaw: f32, pitch: f32) -> bool {
    ensure_model();
    let arena = model();
    if !arena.loaded || w <= 0 || h <= 0 {
        return false;
    }

    let color = unsafe { &mut *core::ptr::addr_of_mut!(COLOR) };
    let depth = unsafe { &mut *core::ptr::addr_of_mut!(DEPTH) };

    // Frame the model: fit its bounding sphere into the target's vertical FOV.
    let centre = arena.min.add(arena.max).scale(0.5);
    let extent = arena
        .max
        .sub(arena.min)
        .scale(0.5)
        .length()
        .max(0.5);
    let fov = 0.62f32; // ~35 degrees
    let mut cam = Camera::new(fov, RT_W as f32 / RT_H as f32);
    cam.target = centre;
    cam.target.y -= bounce_offset();
    cam.distance = extent * 2.2;
    cam.yaw = yaw;
    cam.pitch = pitch;
    cam.update(RT_W as f32 / RT_H as f32);

    let mvp = cam.proj.mul(cam.view);

    // Skinned models: joint matrices carry the pose, so the model matrix stays
    // identity and the vertex shader only applies `world * inverse_bind`.
    let joint_count = arena.joint_count.min(MAX_JOINTS);
    let skin_slice: Option<&[Mat4]> = if joint_count > 0 {
        unsafe {
            let world = &mut *core::ptr::addr_of_mut!(WORLD);
            let pose = &*core::ptr::addr_of!(POSE);
            skin::compute_world(arena, pose, world);
            let joints = &mut *core::ptr::addr_of_mut!(JOINTS);
            let n = skin::compute_joint_matrices(arena, world, joints);
            Some(&(&*core::ptr::addr_of!(JOINTS))[..n])
        }
    } else {
        None
    };

    {
        let mut target = match Target::new(color, depth, RT_W, RT_H) {
            Some(t) => t,
            None => return false,
        };
        target.clear(0);
        let stats = raster::draw(DrawParams {
            target: &mut target,
            arena,
            mvp,
            skin: skin_slice,
            light: Vec3::new(0.35, 0.62, 0.70).normalised(),
            ambient: 0.80,
            flat_color: None,
            morph: update_morphs(),
        });
        unsafe {
            LAST_STATS = stats;
        }
    }

    // Blit, treating "depth untouched" as transparent.
    let dst = screen.back_ptr();
    let stride = screen.stride_pixels();
    let (pw, ph) = screen.physical_size();
    let px0 = screen.map_x(x);
    let py0 = screen.map_y(y);
    let px1 = screen.map_x(x + w);
    let py1 = screen.map_y(y + h);
    let dw = (px1 - px0).max(1);
    let dh = (py1 - py0).max(1);
    if dw <= 0 || dh <= 0 {
        return false;
    }

    let sx_scale = RT_W as f32 / dw as f32;
    let sy_scale = RT_H as f32 / dh as f32;

    for oy in 0..dh {
        let ty = (oy as f32 * sy_scale) as usize;
        if ty >= RT_H {
            continue;
        }
        let dy = py0 + oy;
        if dy < 0 || dy >= ph {
            continue;
        }
        for ox in 0..dw {
            let tx = (ox as f32 * sx_scale) as usize;
            if tx >= RT_W {
                continue;
            }
            let dx = px0 + ox;
            if dx < 0 || dx >= pw {
                continue;
            }
            let si = ty * RT_W + tx;
            if unsafe { (*core::ptr::addr_of!(DEPTH))[si] } == crate::k3d::DEPTH_FAR {
                continue;
            }
            let rgb = unsafe { (*core::ptr::addr_of!(COLOR))[si] } & 0x00ff_ffff;
            unsafe {
                let p = dst.offset((dy * stride + dx) as isize);
                *p = rgb;
            }
        }
    }
    true
}

/// Advance the procedural pose for the given pet state and time, then run the
/// secondary-motion spring so limbs and head follow through.
pub fn update_pose(state: usize, t_seconds: f32) {
    static mut LAST_MS: u64 = 0;
    let now = k::uptime_ms();
    let last = unsafe { LAST_MS };
    unsafe { LAST_MS = now };
    let dt = if last == 0 {
        0.016
    } else {
        (now.saturating_sub(last) as f32 / 1000.0).clamp(0.0, 0.1)
    };
    unsafe {
        let pose = &mut *core::ptr::addr_of_mut!(POSE);
        skin::procedural_pose(state, t_seconds, pose);
        let sprung = skin::apply_springs(pose, dt);
        *pose = sprung;
    }
}

/// Stats from the last rendered frame.
pub fn last_stats() -> DrawStats {
    unsafe { *core::ptr::addr_of!(LAST_STATS) }
}

/// Number of draw calls the current model needs.
pub fn primitive_count() -> usize {
    model().primitive_count
}

#[allow(dead_code)]
fn _keep_no_texture_import() -> u16 {
    NO_TEXTURE
}