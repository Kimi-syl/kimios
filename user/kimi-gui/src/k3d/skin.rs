//! Skeletal animation: humanoid bone map, procedural posing and linear-blend
//! skinning matrix computation.
//!
//! A VRM carries no animation clips; animation means rotating the humanoid bones
//! procedurally (idle sway, breathing, blink, look-at) and letting the skinning
//! matrices fall out of the node hierarchy.

use super::model::{ModelArena, MAX_NODES, MAX_JOINTS, BONE_NONE};
use super::{cos, sin, Mat4, Quat, Vec3};

/// Humanoid bone ids we map from the VRM bone names. Index 0 is unused so that
/// `0` can mean "no bone" in serialised data; `BONE_NONE` is 255.
pub const BONE_HIPS: u8 = 1;
pub const BONE_SPINE: u8 = 2;
pub const BONE_CHEST: u8 = 3;
pub const BONE_NECK: u8 = 4;
pub const BONE_HEAD: u8 = 5;
pub const BONE_LEFT_SHOULDER: u8 = 6;
pub const BONE_LEFT_UPPER_ARM: u8 = 7;
pub const BONE_LEFT_LOWER_ARM: u8 = 8;
pub const BONE_LEFT_HAND: u8 = 9;
pub const BONE_RIGHT_SHOULDER: u8 = 10;
pub const BONE_RIGHT_UPPER_ARM: u8 = 11;
pub const BONE_RIGHT_LOWER_ARM: u8 = 12;
pub const BONE_RIGHT_HAND: u8 = 13;
pub const BONE_LEFT_UPPER_LEG: u8 = 14;
pub const BONE_LEFT_LOWER_LEG: u8 = 15;
pub const BONE_LEFT_FOOT: u8 = 16;
pub const BONE_RIGHT_UPPER_LEG: u8 = 17;
pub const BONE_RIGHT_LOWER_LEG: u8 = 18;
pub const BONE_RIGHT_FOOT: u8 = 19;
pub const BONE_COUNT: usize = 32;

/// Per-bone local rotation offsets applied on top of the bind pose.
#[derive(Clone, Copy)]
pub struct Pose {
    pub rotation: [Quat; BONE_COUNT],
    /// Vertical bob applied to the root, in metres.
    pub root_y: f32,
    /// Whole-body lean, radians around Z.
    pub lean: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Pose {
            rotation: [Quat::IDENTITY; BONE_COUNT],
            root_y: 0.0,
            lean: 0.0,
        }
    }
}

impl Pose {
    pub fn set(&mut self, bone: u8, x: f32, y: f32, z: f32) {
        let i = bone as usize;
        if i < BONE_COUNT {
            self.rotation[i] = Quat::from_euler(x, y, z);
        }
    }
}

/// Procedural idle/working/thinking pose at time `t` (seconds).
pub fn procedural_pose(state: usize, t: f32, out: &mut Pose) {
    *out = Pose::default();
    // Rest-pose correction: the bind pose is a T-pose, which looks wrong for a
    // desktop pet. Drop the arms to a relaxed A-pose and add a slight elbow bend.
    out.set(BONE_LEFT_UPPER_ARM, 0.0, 0.0, 0.55);
    out.set(BONE_RIGHT_UPPER_ARM, 0.0, 0.0, -0.55);
    out.set(BONE_LEFT_LOWER_ARM, 0.0, 0.0, 0.05);
    out.set(BONE_RIGHT_LOWER_ARM, 0.0, 0.0, -0.05);

    // Breathing: gentle vertical bob plus a small spine pitch.
    let breath = sin(t * 1.6);
    out.root_y = breath * 0.006;
    out.set(BONE_SPINE, breath * 0.02, 0.0, 0.0);
    out.set(BONE_CHEST, breath * 0.015, 0.0, 0.0);
    // Head: slow sway, and a look-around pattern while thinking.
    let (hx, hy) = match state {
        2 => (sin(t * 0.5) * 0.10, sin(t * 0.35) * 0.28), // thinking: look around
        1 => (0.10 + sin(t * 1.0) * 0.03, sin(t * 2.1) * 0.05), // working: look down
        _ => (sin(t * 0.7) * 0.04, sin(t * 0.5) * 0.08),
    };
    out.set(BONE_NECK, hx * 0.5, hy * 0.5, 0.0);
    out.set(BONE_HEAD, hx * 0.5, hy * 0.5, 0.0);
    // Arms: small counter-sway.
    let sway = sin(t * 1.1) * 0.06;
    out.set(BONE_LEFT_UPPER_ARM, 0.0, sway * 0.5, 0.55 - sway * 0.3);
    out.set(BONE_RIGHT_UPPER_ARM, 0.0, -sway * 0.5, -0.55 + sway * 0.3);
}

/// Compute world matrices for every node: `world[i] = parent_world * local(i)`,
/// with the pose offsets applied to mapped bones.
///
/// glTF does not guarantee parents precede children, so this resolves in passes
/// (cheap: a character has a few hundred nodes at most).
pub fn compute_world(arena: &ModelArena, pose: &Pose, world: &mut [Mat4; MAX_NODES]) -> usize {
    let mut done = [false; MAX_NODES];
    let mut count = 0usize;
    let limit = arena.node_count.min(MAX_NODES);

    for _ in 0..MAX_NODES {
        let mut progress = false;
        for i in 0..limit {
            if done[i] {
                continue;
            }
            let node = arena.nodes[i];
            let parent = node.parent;
            if parent >= 0 {
                let pi = parent as usize;
                if pi >= limit || !done[pi] {
                    continue;
                }
            }

            let mut q = Quat::new(
                node.rotation[0],
                node.rotation[1],
                node.rotation[2],
                node.rotation[3],
            );
            if node.bone != BONE_NONE {
                let b = node.bone as usize;
                if b < BONE_COUNT {
                    // Pose is applied as a local rotation on top of the bind pose.
                    q = q.mul(pose.rotation[b]).normalised();
                }
            }

            let mut t = Vec3::new(
                node.translation[0],
                node.translation[1],
                node.translation[2],
            );
            if parent < 0 {
                t = t.add(Vec3::new(0.0, pose.root_y, 0.0));
                if pose.lean != 0.0 {
                    q = Quat::from_euler(0.0, 0.0, pose.lean).mul(q).normalised();
                }
            }
            let s = Vec3::new(node.scale[0], node.scale[1], node.scale[2]);
            let local = Mat4::from_trs(t, q, s);

            world[i] = if parent >= 0 {
                world[parent as usize].mul(local)
            } else {
                local
            };
            done[i] = true;
            count += 1;
            progress = true;
        }
        if !progress {
            break;
        }
    }
    count
}

/// Compute skinning matrices `world(joint) * inverse_bind(joint)` for every joint.
pub fn compute_joint_matrices(
    arena: &ModelArena,
    world: &[Mat4; MAX_NODES],
    out: &mut [Mat4; MAX_JOINTS],
) -> usize {
    let n = arena.joint_count.min(MAX_JOINTS);
    for j in 0..n {
        let node = arena.joint_nodes[j] as usize;
        let w = if node < MAX_NODES {
            world[node]
        } else {
            Mat4::IDENTITY
        };
        let ibm = Mat4 {
            m: arena.joint_ibm[j],
        };
        out[j] = w.mul(ibm);
    }
    n
}

/// Distance helper reused by spring bones.
pub fn damped_spring(
    current: Vec3,
    target: Vec3,
    velocity: &mut Vec3,
    stiffness: f32,
    damping: f32,
    dt: f32,
) -> Vec3 {
    let delta = target.sub(current);
    *velocity = velocity.add(delta.scale(stiffness * dt)).scale(1.0 - damping * dt);
    current.add(velocity.scale(dt))
}

// ---------------------------------------------------------------------------
// Secondary motion ("spring bones")
// ---------------------------------------------------------------------------
//
// A VRM carries explicit spring-bone chains, but the same visual result — parts
// of the body lagging behind the rest — can be produced generically: treat each
// secondary bone's rotation as a damped spring chasing its target rotation from
// the pose. That gives follow-through on any rig, including plain glTF exports,
// without needing authored spring data.

/// Per-bone angular lag state (xyz = euler lag, xyz velocity).
static mut LAG: [[f32; 3]; BONE_COUNT] = [[0.0; 3]; BONE_COUNT];
static mut LAG_VEL: [[f32; 3]; BONE_COUNT] = [[0.0; 3]; BONE_COUNT];

/// Bones that should carry follow-through: limbs, head, neck. Core bones (hips,
/// spine, chest) stay rigid so the body does not feel rubbery.
fn is_secondary(bone: usize) -> bool {
    matches!(
        bone as u8,
        BONE_HEAD
            | BONE_NECK
            | BONE_LEFT_UPPER_ARM
            | BONE_LEFT_LOWER_ARM
            | BONE_LEFT_HAND
            | BONE_RIGHT_UPPER_ARM
            | BONE_RIGHT_LOWER_ARM
            | BONE_RIGHT_HAND
            | BONE_LEFT_LOWER_LEG
            | BONE_RIGHT_LOWER_LEG
    )
}

/// Advance the spring state for `dt` seconds and return a pose with lag applied.
///
/// The lag is additive on top of the pose rotation, so the primary animation is
/// unchanged and only the follow-through differs.
pub fn apply_springs(pose: &Pose, dt: f32) -> Pose {
    let dt = dt.clamp(0.0, 0.1);
    let mut out = *pose;
    if dt <= 0.0 {
        return out;
    }
    // Stiffness/damping tuned for a small character: visible but not floaty.
    const STIFFNESS: f32 = 90.0;
    const DAMPING: f32 = 12.0;

    unsafe {
        let lag = &mut *core::ptr::addr_of_mut!(LAG);
        let vel = &mut *core::ptr::addr_of_mut!(LAG_VEL);
        for b in 0..BONE_COUNT {
            if !is_secondary(b) {
                continue;
            }
            for axis in 0..3 {
                // Target lag is zero: the spring returns to the pose rotation.
                let x = lag[b][axis];
                let v = vel[b][axis];
                let accel = -STIFFNESS * x - DAMPING * v;
                let nv = v + accel * dt;
                lag[b][axis] = x + nv * dt;
                vel[b][axis] = nv;
            }
            let base = pose.rotation[b];
            let extra = Quat::from_euler(lag[b][0], lag[b][1], lag[b][2]);
            out.rotation[b] = base.mul(extra).normalised();
        }
    }
    out
}

/// Feed an impulse into the spring state, e.g. when the pet is tapped.
pub fn spring_impulse(x: f32, y: f32) {
    unsafe {
        let vel = &mut *core::ptr::addr_of_mut!(LAG_VEL);
        for b in 0..BONE_COUNT {
            if !is_secondary(b) {
                continue;
            }
            vel[b][0] += x;
            vel[b][1] += y;
        }
    }
}

#[allow(dead_code)]
fn _unused(_: u8) -> u8 {
    BONE_NONE
}