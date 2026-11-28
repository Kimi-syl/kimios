//! Software triangle rasteriser: transform, near-plane clip, setup, fill, depth
//! test, texture sample, alpha test/blend.
//!
//! Draws into an offscreen [`Target`] sized to the pet's display rect, so cost
//! scales with the character's on-screen size rather than the framebuffer.
//!
//! Pipeline per triangle:
//!   skinned vertex -> clip space -> near clip -> project -> cull -> fill
//! Depth is u16 fixed point. Textures are nearest-neighbour, matching the GUI.
//! Interpolated attributes are perspective-correct (they carry `1/w`).

use super::model::{AlphaMode, Material, ModelArena, Texture, NO_TEXTURE};
use super::{Mat4, Target, Vec3, Vec4};

/// Near plane epsilon in clip space.
const NEAR_EPS: f32 = 1e-4;

/// Post-transform vertex in screen space.
#[derive(Clone, Copy, Default)]
pub struct ScreenVert {
    pub x: f32,
    pub y: f32,
    /// Normalised depth in [0, 1] (0 = near).
    pub z: f32,
    /// Reciprocal clip-space w.
    pub inv_w: f32,
    /// `uv * inv_w`.
    pub uv: [f32; 2],
    /// `normal * inv_w` (world space).
    pub normal: Vec3,
}

#[derive(Clone, Copy)]
struct ClipVert {
    pos: Vec4,
    uv: [f32; 2],
    normal: Vec3,
}

/// Per-frame draw parameters.
pub struct DrawParams<'a> {
    pub target: &'a mut Target<'a>,
    pub arena: &'a ModelArena,
    /// model -> clip.
    pub mvp: Mat4,
    /// Skinning matrices per joint, or `None` for a rigid mesh.
    pub skin: Option<&'a [Mat4]>,
    /// Normalised direction towards the key light.
    pub light: Vec3,
    /// Ambient term (floor for lit materials, base for unlit ones).
    pub ambient: f32,
    /// Overrides material colour, for flat debug shading.
    pub flat_color: Option<u32>,
    /// Per-vertex morph deltas for this frame (expressions), indexed by vertex.
    pub morph: Option<&'a [Vec3]>,
}

#[derive(Clone, Copy, Default)]
pub struct DrawStats {
    pub triangles: u32,
    pub drawn: u32,
    pub pixels: u32,
    pub clipped: u32,
}

/// Rasterise every primitive in the model.
pub fn draw(params: DrawParams<'_>) -> DrawStats {
    let DrawParams {
        target,
        arena,
        mvp,
        skin,
        light,
        ambient,
        flat_color,
        morph,
    } = params;

    let mut stats = DrawStats::default();

    for p in 0..arena.primitive_count {
        let prim = arena.primitives[p];
        if arena.material_count == 0 {
            break;
        }
        let mat = arena.materials[(prim.material as usize).min(arena.material_count - 1)];
        let tex = if mat.texture != NO_TEXTURE {
            arena.textures.get(mat.texture as usize).filter(|t| t.loaded)
        } else {
            None
        };

        let first = prim.first_index as usize;
        let count = prim.index_count as usize;
        let mut i = 0usize;
        while i + 2 < count {
            let ia = arena.indices[first + i] as usize;
            let ib = arena.indices[first + i + 1] as usize;
            let ic = arena.indices[first + i + 2] as usize;
            i += 3;
            stats.triangles += 1;

            let (Some(va), Some(vb), Some(vc)) = (
                arena.vertices.get(ia),
                arena.vertices.get(ib),
                arena.vertices.get(ic),
            ) else {
                continue;
            };

            let (pa, na) = skinned(morphed(va.pos, ia, morph), va.normal, va.joints, va.weights, skin);
            let (pb, nb) = skinned(morphed(vb.pos, ib, morph), vb.normal, vb.joints, vb.weights, skin);
            let (pc, nc) = skinned(morphed(vc.pos, ic, morph), vc.normal, vc.joints, vc.weights, skin);

            let tri = [
                clip(mvp, pa, na, va.uv),
                clip(mvp, pb, nb, vb.uv),
                clip(mvp, pc, nc, vc.uv),
            ];

            // Near-plane clip, then project.
            let mut poly = [ClipVert {
                pos: Vec4::new(0.0, 0.0, 0.0, 0.0),
                uv: [0.0; 2],
                normal: Vec3::ZERO,
            }; 4];
            let n = clip_near(&tri, &mut poly);
            if n < 3 {
                stats.clipped += 1;
                continue;
            }
            let mut sv = [ScreenVert::default(); 4];
            let mut visible = true;
            for k in 0..n {
                match project(&poly[k], target.w, target.h) {
                    Some(s) => sv[k] = s,
                    None => {
                        visible = false;
                        break;
                    }
                }
            }
            if !visible {
                stats.clipped += 1;
                continue;
            }

            for k in 1..n - 1 {
                let a = sv[0];
                let b = sv[k];
                let c = sv[k + 1];
                // Screen-space area. Clip-space Y-up is flipped into Y-down screen
                // space, so front faces (CCW in model space) come out negative.
                let area = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
                if area == 0.0 || area.is_nan() {
                    continue;
                }
                if !mat.double_sided && area > 0.0 {
                    continue;
                }
                if fill(target, &a, &b, &c, area, light, ambient, &mat, tex, flat_color, &mut stats) {
                    stats.drawn += 1;
                }
            }
        }
    }
    stats
}


/// Apply this frame's morph deltas (facial expressions) to a base position.
#[inline]
fn morphed(pos: [f32; 3], vertex: usize, morph: Option<&[Vec3]>) -> [f32; 3] {
    match morph {
        Some(deltas) => match deltas.get(vertex) {
            Some(d) => [pos[0] + d.x, pos[1] + d.y, pos[2] + d.z],
            None => pos,
        },
        None => pos,
    }
}

#[inline]
fn skinned(
    pos: [f32; 3],
    normal: [f32; 3],
    joints: [u16; 4],
    weights: [f32; 4],
    skin: Option<&[Mat4]>,
) -> (Vec3, Vec3) {
    let p = Vec3::new(pos[0], pos[1], pos[2]);
    let n = Vec3::new(normal[0], normal[1], normal[2]);
    let Some(mats) = skin else {
        return (p, n);
    };
    let mut out_p = Vec3::ZERO;
    let mut out_n = Vec3::ZERO;
    let mut total = 0.0f32;
    for k in 0..4 {
        let w = weights[k];
        if w <= 0.0 {
            continue;
        }
        let Some(m) = mats.get(joints[k] as usize) else {
            continue;
        };
        out_p = out_p.add(m.transform_point(p).scale(w));
        out_n = out_n.add(m.transform_vec(n).scale(w));
        total += w;
    }
    if total <= 0.0 {
        (p, n)
    } else {
        (out_p, out_n)
    }
}

#[inline]
fn clip(mvp: Mat4, p: Vec3, n: Vec3, uv: [f32; 2]) -> ClipVert {
    ClipVert {
        pos: mvp.transform_vec4(Vec4::new(p.x, p.y, p.z, 1.0)),
        uv,
        normal: n,
    }
}

/// Sutherland-Hodgman against `w >= NEAR_EPS`, writing at most 4 vertices.
fn clip_near(input: &[ClipVert; 3], out: &mut [ClipVert; 4]) -> usize {
    let mut count = 0usize;
    for i in 0..3 {
        let a = input[i];
        let b = input[(i + 1) % 3];
        let da = a.pos.w - NEAR_EPS;
        let db = b.pos.w - NEAR_EPS;
        if da >= 0.0 && count < 4 {
            out[count] = a;
            count += 1;
        }
        if (da >= 0.0) != (db >= 0.0) && count < 4 {
            let t = da / (da - db);
            out[count] = ClipVert {
                pos: lerp4(a.pos, b.pos, t),
                uv: [
                    a.uv[0] + (b.uv[0] - a.uv[0]) * t,
                    a.uv[1] + (b.uv[1] - a.uv[1]) * t,
                ],
                normal: lerp3(a.normal, b.normal, t),
            };
            count += 1;
        }
    }
    count
}

#[inline]
fn lerp4(a: Vec4, b: Vec4, t: f32) -> Vec4 {
    a.add(b.sub(a).scale(t))
}

#[inline]
fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.add(b.sub(a).scale(t))
}

#[inline]
fn project(c: &ClipVert, w: usize, h: usize) -> Option<ScreenVert> {
    if c.pos.w <= NEAR_EPS {
        return None;
    }
    let inv_w = 1.0 / c.pos.w;
    Some(ScreenVert {
        x: (c.pos.x * inv_w * 0.5 + 0.5) * w as f32,
        // Clip space is Y-up; the GUI's screen space is Y-down.
        y: (0.5 - c.pos.y * inv_w * 0.5) * h as f32,
        z: c.pos.z * inv_w,
        inv_w,
        // Pre-multiply by 1/w so plain screen-space barycentrics give the
        // perspective-correct attribute after dividing by the interpolated 1/w.
        uv: [c.uv[0] * inv_w, c.uv[1] * inv_w],
        normal: c.normal.scale(inv_w),
    })
}

#[allow(clippy::too_many_arguments)]
fn fill(
    target: &mut Target<'_>,
    a: &ScreenVert,
    b: &ScreenVert,
    c: &ScreenVert,
    area: f32,
    light: Vec3,
    ambient: f32,
    mat: &Material,
    tex: Option<&Texture>,
    flat_color: Option<u32>,
    stats: &mut DrawStats,
) -> bool {
    let inv_area = 1.0 / area;

    let min_x = a.x.min(b.x).min(c.x).max(0.0) as i32;
    let max_x = a.x.max(b.x).max(c.x).min(target.w as f32 - 1.0) as i32;
    let min_y = a.y.min(b.y).min(c.y).max(0.0) as i32;
    let max_y = a.y.max(b.y).max(c.y).min(target.h as f32 - 1.0) as i32;
    if min_x > max_x || min_y > max_y {
        return false;
    }

    let w = target.w;
    let mut pixels = 0u32;
    for py in min_y..=max_y {
        let fy = py as f32 + 0.5;
        for px in min_x..=max_x {
            let fx = px as f32 + 0.5;

            // Edge functions. For triangle (a,b,c) the barycentric weight of `a`
            // is E(b,c,p)/area and of `c` is E(a,b,p)/area.
            let wc = ((b.x - a.x) * (fy - a.y) - (fx - a.x) * (b.y - a.y)) * inv_area;
            let wa = ((c.x - b.x) * (fy - b.y) - (fx - b.x) * (c.y - b.y)) * inv_area;
            let wb = 1.0 - wa - wc;
            if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                continue;
            }

            let z = a.z * wa + b.z * wb + c.z * wc;
            if !(0.0..=1.0).contains(&z) {
                continue;
            }
            let di = py as usize * w + px as usize;
            let depth = (z * 65535.0) as u16;
            if depth >= target.depth[di] {
                continue;
            }

            let inv_w = a.inv_w * wa + b.inv_w * wb + c.inv_w * wc;
            if inv_w <= 0.0 {
                continue;
            }
            let iw = 1.0 / inv_w;
            let u = (a.uv[0] * wa + b.uv[0] * wb + c.uv[0] * wc) * iw;
            let v = (a.uv[1] * wa + b.uv[1] * wb + c.uv[1] * wc) * iw;

            let texel = match tex {
                Some(t) => t.sample(u, v),
                None => [255, 255, 255, 255],
            };
            let alpha = (texel[3] as f32 / 255.0) * mat.base_color[3];
            match mat.alpha_mode {
                AlphaMode::Opaque | AlphaMode::Mask => {
                    if alpha < mat.alpha_cutoff {
                        continue;
                    }
                }
                AlphaMode::Blend => {
                    if alpha <= 0.004 {
                        continue;
                    }
                }
            }

            let (r, g, bl) = if let Some(flat) = flat_color {
                (
                    ((flat >> 16) & 0xff) as f32,
                    ((flat >> 8) & 0xff) as f32,
                    (flat & 0xff) as f32,
                )
            } else {
                let shade = if mat.unlit {
                    ambient + 0.35
                } else {
                    let nx = (a.normal.x * wa + b.normal.x * wb + c.normal.x * wc) * iw;
                    let ny = (a.normal.y * wa + b.normal.y * wb + c.normal.y * wc) * iw;
                    let nz = (a.normal.z * wa + b.normal.z * wb + c.normal.z * wc) * iw;
                    let ndl = Vec3::new(nx, ny, nz).normalised().dot(light).max(0.0);
                    ambient + ndl * 0.45
                };
                (
                    texel[0] as f32 * mat.base_color[0] * shade,
                    texel[1] as f32 * mat.base_color[1] * shade,
                    texel[2] as f32 * mat.base_color[2] * shade,
                )
            };

            let src = (clamp_u8(r) << 16) | (clamp_u8(g) << 8) | clamp_u8(bl);
            if mat.alpha_mode == AlphaMode::Blend && alpha < 0.996 {
                let dst = target.color[di];
                let dr = ((dst >> 16) & 0xff) as f32;
                let dg = ((dst >> 8) & 0xff) as f32;
                let db = (dst & 0xff) as f32;
                let sr = ((src >> 16) & 0xff) as f32;
                let sg = ((src >> 8) & 0xff) as f32;
                let sb = (src & 0xff) as f32;
                target.color[di] = (clamp_u8(dr + (sr - dr) * alpha) << 16)
                    | (clamp_u8(dg + (sg - dg) * alpha) << 8)
                    | clamp_u8(db + (sb - db) * alpha);
            } else {
                target.color[di] = src;
            }
            target.depth[di] = depth;
            pixels += 1;
        }
    }
    stats.pixels += pixels;
    pixels > 0
}

#[inline]
fn clamp_u8(x: f32) -> u32 {
    if x <= 0.0 {
        0
    } else if x >= 255.0 {
        255
    } else {
        x as u32
    }
}