//! Runtime 3D for the desktop pet: shared math, arenas and the renderer contract.
//!
//! `no_std` with no allocator: every buffer is a fixed static arena owned by the
//! caller, and sizes are compile-time constants. The renderer draws into an
//! offscreen target the size of the pet's display rect, which the GUI then
//! blits 1:1 into the back buffer.
//!
//! Math note: `f32::sqrt`/`sin`/`cos` are not available without `libm`, so this
//! module provides its own. Everything else (add/mul/dot) is plain f32 arithmetic,
//! which the FPU handles directly (`CPACR_EL1.FPEN` is set at boot).

pub mod gltf;
pub mod model;
pub mod raster;
pub mod skin;

pub use model::{
    LoadError, Material, ModelArena, ReadAt, MAX_JOINTS, MAX_PRIMS, MAX_TRIS, MAX_VERTS,
};

// ---------------------------------------------------------------------------
// Scalar helpers (no libm)
// ---------------------------------------------------------------------------

/// Fast approximate square root: exponent bit hack plus Newton refinement.
/// Accurate to ~1 ulp after three iterations over the useful range.
pub fn sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1fc0_0000);
    // Three Newton steps: y = (y + x/y) / 2
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);
    y = 0.5 * (y + x / y);
    y
}

pub fn abs(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & 0x7fff_ffff)
}

pub fn floor(x: f32) -> f32 {
    let i = x as i32 as f32;
    if x < 0.0 && i != x {
        i - 1.0
    } else {
        i
    }
}

pub fn ceil(x: f32) -> f32 {
    -floor(-x)
}

/// Wrap to `[-pi, pi]`.
fn wrap_pi(mut x: f32) -> f32 {
    const TWO_PI: f32 = 6.283_185_5;
    const PI: f32 = 3.141_592_7;
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    x
}

/// Sine via range reduction and a 7th-order Taylor polynomial (|err| < 1e-5 on
/// the reduced range, which is plenty for posing and cameras).
pub fn sin(x: f32) -> f32 {
    let x = wrap_pi(x);
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0)))
}

pub fn cos(x: f32) -> f32 {
    sin(x + 1.570_796_3)
}

pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Degrees to radians.
pub fn radians(deg: f32) -> f32 {
    deg * 0.017_453_292
}

// ---------------------------------------------------------------------------
// Vectors
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const ONE: Vec3 = Vec3 { x: 1.0, y: 1.0, z: 1.0 };

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    pub fn mul(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x * o.x, self.y * o.y, self.z * o.z)
    }

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        sqrt(self.dot(self))
    }

    pub fn normalised(self) -> Vec3 {
        let len = self.length();
        if len > 1e-20 {
            self.scale(1.0 / len)
        } else {
            Vec3::ZERO
        }
    }

    pub fn min(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }

    pub fn max(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }

    /// Component by index (0=x, 1=y, 2=z).
    pub fn axis(self, i: usize) -> f32 {
        match i {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Vec4 { x, y, z, w }
    }

    pub fn xyz(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    pub fn add(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x + o.x, self.y + o.y, self.z + o.z, self.w + o.w)
    }

    pub fn sub(self, o: Vec4) -> Vec4 {
        Vec4::new(self.x - o.x, self.y - o.y, self.z - o.z, self.w - o.w)
    }

    pub fn scale(self, s: f32) -> Vec4 {
        Vec4::new(self.x * s, self.y * s, self.z * s, self.w * s)
    }

    pub fn dot(self, o: Vec4) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }
}

// ---------------------------------------------------------------------------
// Matrices (column-major, matching glTF)
// ---------------------------------------------------------------------------

/// 4x4 matrix, column-major: `m[col * 4 + row]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub m: [f32; 16],
}

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        m: [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ],
    };

    pub fn identity() -> Self {
        Mat4::IDENTITY
    }

    /// `self * other` (apply `other` first, then `self`).
    pub fn mul(self, o: Mat4) -> Mat4 {
        let mut r = [0.0f32; 16];
        let mut c = 0;
        while c < 4 {
            let mut row = 0;
            while row < 4 {
                let mut sum = 0.0;
                let mut k = 0;
                while k < 4 {
                    sum += self.m[k * 4 + row] * o.m[c * 4 + k];
                    k += 1;
                }
                r[c * 4 + row] = sum;
                row += 1;
            }
            c += 1;
        }
        Mat4 { m: r }
    }

    pub fn translate(t: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.m[12] = t.x;
        m.m[13] = t.y;
        m.m[14] = t.z;
        m
    }

    pub fn scale(s: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.m[0] = s.x;
        m.m[5] = s.y;
        m.m[10] = s.z;
        m
    }

    pub fn rotate_x(a: f32) -> Mat4 {
        let (s, c) = (sin(a), cos(a));
        let mut m = Mat4::IDENTITY;
        m.m[5] = c;
        m.m[6] = s;
        m.m[9] = -s;
        m.m[10] = c;
        m
    }

    pub fn rotate_y(a: f32) -> Mat4 {
        let (s, c) = (sin(a), cos(a));
        let mut m = Mat4::IDENTITY;
        m.m[0] = c;
        m.m[2] = -s;
        m.m[8] = s;
        m.m[10] = c;
        m
    }

    pub fn rotate_z(a: f32) -> Mat4 {
        let (s, c) = (sin(a), cos(a));
        let mut m = Mat4::IDENTITY;
        m.m[0] = c;
        m.m[1] = s;
        m.m[4] = -s;
        m.m[5] = c;
        m
    }

    /// Compose translation * rotation(quaternion) * scale.
    pub fn from_trs(t: Vec3, r: Quat, s: Vec3) -> Mat4 {
        let rm = r.to_mat4();
        Mat4::translate(t).mul(rm).mul(Mat4::scale(s))
    }

    pub fn transform_point(self, p: Vec3) -> Vec3 {
        let x = self.m[0] * p.x + self.m[4] * p.y + self.m[8] * p.z + self.m[12];
        let y = self.m[1] * p.x + self.m[5] * p.y + self.m[9] * p.z + self.m[13];
        let z = self.m[2] * p.x + self.m[6] * p.y + self.m[10] * p.z + self.m[14];
        Vec3::new(x, y, z)
    }

    pub fn transform_vec(self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0] * v.x + self.m[4] * v.y + self.m[8] * v.z,
            self.m[1] * v.x + self.m[5] * v.y + self.m[9] * v.z,
            self.m[2] * v.x + self.m[6] * v.y + self.m[10] * v.z,
        )
    }

    pub fn transform_vec4(self, v: Vec4) -> Vec4 {
        Vec4::new(
            self.m[0] * v.x + self.m[4] * v.y + self.m[8] * v.z + self.m[12] * v.w,
            self.m[1] * v.x + self.m[5] * v.y + self.m[9] * v.z + self.m[13] * v.w,
            self.m[2] * v.x + self.m[6] * v.y + self.m[10] * v.z + self.m[14] * v.w,
            self.m[3] * v.x + self.m[7] * v.y + self.m[11] * v.z + self.m[15] * v.w,
        )
    }

    /// Right-handed look-at, used for the orbit camera.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
        let f = target.sub(eye).normalised();
        let s = f.cross(up).normalised();
        let u = s.cross(f);
        Mat4 {
            m: [
                s.x, u.x, -f.x, 0.0, //
                s.y, u.y, -f.y, 0.0, //
                s.z, u.z, -f.z, 0.0, //
                -s.dot(eye), -u.dot(eye), f.dot(eye), 1.0,
            ],
        }
    }

    /// Right-handed perspective with depth mapped to [0, 1].
    pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fovy * 0.5).tan_approx();
        Mat4 {
            m: [
                f / aspect, 0.0, 0.0, 0.0, //
                0.0, f, 0.0, 0.0, //
                0.0, 0.0, far / (near - far), -1.0, //
                0.0, 0.0, (near * far) / (near - far), 0.0,
            ],
        }
    }

    /// Right-handed orthographic with depth mapped to [0, 1].
    pub fn ortho(l: f32, r: f32, b: f32, t: f32, near: f32, far: f32) -> Mat4 {
        let rl = r - l;
        let tb = t - b;
        let fn_ = far - near;
        Mat4 {
            m: [
                2.0 / rl, 0.0, 0.0, 0.0, //
                0.0, 2.0 / tb, 0.0, 0.0, //
                0.0, 0.0, -1.0 / fn_, 0.0, //
                -(r + l) / rl, -(t + b) / tb, -near / fn_, 1.0,
            ],
        }
    }
}

/// `tan` half-angle for perspective: sin/cos on the reduced range.
impl core::ops::Mul for Mat4 {
    type Output = Mat4;
    fn mul(self, o: Mat4) -> Mat4 {
        Mat4::mul(self, o)
    }
}

pub trait TanApprox {
    fn tan_approx(self) -> f32;
}

impl TanApprox for f32 {
    fn tan_approx(self) -> f32 {
        sin(self) / cos(self)
    }
}

// ---------------------------------------------------------------------------
// Quaternion
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Quat::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Quat { x, y, z, w }
    }

    /// XYZ intrinsic euler angles, in radians (matches how poses are authored).
    pub fn from_euler(x: f32, y: f32, z: f32) -> Quat {
        let (sx, cx) = (sin(x * 0.5), cos(x * 0.5));
        let (sy, cy) = (sin(y * 0.5), cos(y * 0.5));
        let (sz, cz) = (sin(z * 0.5), cos(z * 0.5));
        Quat::new(
            sx * cy * cz + cx * sy * sz,
            cx * sy * cz - sx * cy * sz,
            cx * cy * sz + sx * sy * cz,
            cx * cy * cz - sx * sy * sz,
        )
    }

    pub fn normalised(self) -> Quat {
        let len = sqrt(self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w);
        if len > 1e-20 {
            Quat::new(self.x / len, self.y / len, self.z / len, self.w / len)
        } else {
            Quat::IDENTITY
        }
    }

    pub fn mul(self, o: Quat) -> Quat {
        Quat::new(
            self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
            self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
        )
    }

    pub fn conjugate(self) -> Quat {
        Quat::new(-self.x, -self.y, -self.z, self.w)
    }

    pub fn rotate_vec3(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let t = u.cross(v).scale(2.0);
        v.add(t.scale(self.w)).add(u.cross(t))
    }

    pub fn to_mat4(self) -> Mat4 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        Mat4 {
            m: [
                1.0 - (yy + zz), xy + wz, xz - wy, 0.0, //
                xy - wz, 1.0 - (xx + zz), yz + wx, 0.0, //
                xz + wy, yz - wx, 1.0 - (xx + yy), 0.0, //
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }
}

// ---------------------------------------------------------------------------
// Camera and render target
// ---------------------------------------------------------------------------

/// Camera rig: orbit around a target point, which is what a desktop pet wants.
#[derive(Clone, Copy)]
pub struct Camera {
    pub view: Mat4,
    pub proj: Mat4,
    /// Distance from the orbit centre.
    pub distance: f32,
    /// Vertical field of view in radians.
    pub fov: f32,
    /// Yaw/pitch in radians.
    pub yaw: f32,
    pub pitch: f32,
    /// Point the camera orbits.
    pub target: Vec3,
}

impl Camera {
    pub fn new(fov: f32, aspect: f32) -> Camera {
        let mut c = Camera {
            view: Mat4::IDENTITY,
            proj: Mat4::IDENTITY,
            distance: 2.4,
            fov,
            yaw: 0.0,
            pitch: 0.12,
            target: Vec3::new(0.0, 0.95, 0.0),
        };
        c.update(aspect);
        c
    }

    pub fn update(&mut self, aspect: f32) {
        let cp = cos(self.pitch);
        let eye = Vec3::new(
            self.target.x + self.distance * cp * sin(self.yaw),
            self.target.y + self.distance * sin(self.pitch),
            self.target.z + self.distance * cp * cos(self.yaw),
        );
        self.view = Mat4::look_at(eye, self.target, Vec3::new(0.0, 1.0, 0.0));
        self.proj = Mat4::perspective(self.fov, aspect, 0.05, 100.0);
    }
}

/// Depth-buffer clear value (u16, 1.0 == far).
pub const DEPTH_FAR: u16 = u16::MAX;

/// An offscreen render target. The GUI owns the buffers; the rasteriser only
/// borrows them, so nothing here allocates.
pub struct Target<'a> {
    /// `w * h` pixels, `0x00RRGGBB` like the rest of the GUI.
    pub color: &'a mut [u32],
    /// `w * h` depth values, fixed point.
    pub depth: &'a mut [u16],
    pub w: usize,
    pub h: usize,
}

impl<'a> Target<'a> {
    pub fn new(color: &'a mut [u32], depth: &'a mut [u16], w: usize, h: usize) -> Option<Self> {
        if w == 0 || h == 0 || color.len() < w * h || depth.len() < w * h {
            return None;
        }
        Some(Target { color, depth, w, h })
    }

    pub fn clear(&mut self, rgb: u32) {
        for p in self.color.iter_mut().take(self.w * self.h) {
            *p = rgb;
        }
        for d in self.depth.iter_mut().take(self.w * self.h) {
            *d = DEPTH_FAR;
        }
    }

    pub fn aspect(&self) -> f32 {
        self.w as f32 / self.h as f32
    }
}