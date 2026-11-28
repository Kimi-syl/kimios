#!/usr/bin/env python3
"""vrm_to_kimios.py -- normalise ANY VRM / glTF / GLB avatar into a GLB the
KimiOS software rasteriser can actually spawn, then audit it.

  "C:\\Program Files\\Blender Foundation\\Blender 5.2\\blender.exe" -b --factory-startup \
      --python-exit-code 1 --python tools/blender/vrm_to_kimios.py -- \
      --input avatar.vrm --out assets/models/avatar.glb --report avatar.json

Why this exists: the device loader (user/kimi-gui/src/k3d) has hard, fixed-size
arenas and a narrow format contract. A stock VRM usually violates several of
them at once -- arbitrary bone names, 40k+ triangles, 2048 px textures, JPEG
images, MToon-only materials, no matching rest pose. This tool rebuilds the
skeleton, re-targets the skin weights and rewrites the asset so it fits.

What is guaranteed for every input:
  * 17-joint humanoid armature with EXACTLY the KimiOS bone names:
      hips, spine, chest, neck, head, leftUpperArm, leftLowerArm, leftHand,
      rightUpperArm, rightLowerArm, rightHand, leftUpperLeg, leftLowerLeg,
      leftFoot, rightUpperLeg, rightLowerLeg, rightFoot
    Source bones are mapped from the VRM humanoid extension when present, from
    bone-name heuristics otherwise; every other bone (fingers, toes, hair,
    accessories) is folded into its nearest canonical ancestor and the vertex
    weights are summed, re-normalised and capped at 4 influences.
  * all skinned meshes joined into ONE skinned mesh with one UV layer.
  * transforms baked: glTF Y-up, character faces +Z, feet at Y=0, identity root.
  * every embedded image converted to PNG and downscaled to <= --tex-size (256),
    because the loader skips images larger than 384 px and rejects JPEG.
  * facial morphs named exactly blink / happy / surprised (combined from the
    source expression binds), POSITION-only, all shape-key values zeroed.
  * the loader's arena caps enforced by decimation when needed; the mesh is
    simplified with a KD-tree transfer of the morph deltas so expressions keep
    working on the reduced topology.
  * the result is parsed back with verify_glb and audited against both the
    loader caps and the KimiOS asset contract.

Options (after `--`):
  --input PATH        source .vrm / .glb / .gltf / .fbx               (required)
  --out PATH          destination .glb (default <repo>/assets/models/<stem>.glb)
  --report PATH       optional JSON report destination
  --check-only        only audit the source against the loader caps, no export
  --tex-size N        maximum texture edge, <= 256 (default 256)
  --max-tris N        triangle budget (default 4000, loader hard cap 5461)
  --max-verts N       vertex budget after UV/normal splitting (default 3000,
                      loader hard cap 8192)
  --keep-morphs       keep the three canonical morphs even if it costs verts
  --no-decimate       never decimate; fail if the model exceeds the caps
  --strict            also require the full KimiOS asset contract (one material,
                      one UV layer, <=4000 tris, <=1000 verts, 3 morphs)
  --save-blend PATH   keep the intermediate .blend

Exit code 0 = exported and audited clean; 1 = could not normalise.
"""
import argparse
import json
import math
import os
import shutil
import sys
import tempfile

import bpy
import bmesh
from mathutils import Matrix, Vector, kdtree

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import verify_glb  # noqa: E402

# --------------------------------------------------------------------------- caps
# Mirrors user/kimi-gui/src/k3d/model.rs and gltf.rs. Anything above these simply
# does not fit the device's fixed arenas.
CAP_VERTS = 8192          # MAX_VERTS (after UV/normal splitting)
CAP_INDICES = 16384       # MAX_TRIS (index entries, so 5461 triangles)
CAP_JOINTS = 128          # MAX_JOINTS
CAP_PRIMS = 16            # MAX_PRIMS
CAP_MATERIALS = 16        # MAX_MATERIALS
CAP_NODES = 256           # MAX_NODES
CAP_TEXTURES = 4          # MAX_TEXTURES (Texture arena slots)
CAP_MORPH_TARGETS = 16    # MAX_MORPH_TARGETS
CAP_MORPH_ENTRIES = 16384  # MAX_MORPH_ENTRIES (deltas stored across targets)
PNG_MAX_DIM = 384         # larger source PNGs are skipped entirely
TEX_MAX_DIM = 128         # decoded texture size on device

BONES = [
    ("hips", None, (0.0, 0.0, 0.605), (0.0, 0.0, 0.725)),
    ("spine", "hips", (0.0, 0.0, 0.725), (0.0, 0.0, 0.865)),
    ("chest", "spine", (0.0, 0.0, 0.865), (0.0, 0.0, 1.005)),
    ("neck", "chest", (0.0, 0.0, 1.005), (0.0, 0.0, 1.090)),
    ("head", "neck", (0.0, 0.0, 1.090), (0.0, 0.0, 1.455)),
    ("leftUpperArm", "chest", (0.140, 0.0, 1.005), (0.385, 0.0, 1.005)),
    ("leftLowerArm", "leftUpperArm", (0.385, 0.0, 1.005), (0.585, 0.0, 1.005)),
    ("leftHand", "leftLowerArm", (0.585, 0.0, 1.005), (0.735, 0.0, 1.005)),
    ("rightUpperArm", "chest", (-0.140, 0.0, 1.005), (-0.385, 0.0, 1.005)),
    ("rightLowerArm", "rightUpperArm", (-0.385, 0.0, 1.005), (-0.585, 0.0, 1.005)),
    ("rightHand", "rightLowerArm", (-0.585, 0.0, 1.005), (-0.735, 0.0, 1.005)),
    ("leftUpperLeg", "hips", (0.085, 0.0, 0.620), (0.085, 0.0, 0.350)),
    ("leftLowerLeg", "leftUpperLeg", (0.085, 0.0, 0.350), (0.085, 0.0, 0.100)),
    ("leftFoot", "leftLowerLeg", (0.085, 0.0, 0.100), (0.085, -0.135, 0.045)),
    ("rightUpperLeg", "hips", (-0.085, 0.0, 0.620), (-0.085, 0.0, 0.350)),
    ("rightLowerLeg", "rightUpperLeg", (-0.085, 0.0, 0.350), (-0.085, 0.0, 0.100)),
    ("rightFoot", "rightLowerLeg", (-0.085, 0.0, 0.100), (-0.085, -0.135, 0.045)),
]
BONE_NAMES = [b[0] for b in BONES]
BONE_PARENT = {b[0]: b[1] for b in BONES}
EXPRESSIONS = ("blink", "happy", "surprised")

# VRM humanoid slot -> KimiOS bone. VRM 0.x and 1.0 use the same slot names for
# the 17 bones this runtime needs (1.0 adds optional ones we ignore).
VRM_SLOTS = {
    "hips": "hips", "spine": "spine", "chest": "chest", "upperChest": "chest",
    "neck": "neck", "head": "head",
    "leftUpperArm": "leftUpperArm", "leftLowerArm": "leftLowerArm",
    "leftHand": "leftHand",
    "rightUpperArm": "rightUpperArm", "rightLowerArm": "rightLowerArm",
    "rightHand": "rightHand",
    "leftUpperLeg": "leftUpperLeg", "leftLowerLeg": "leftLowerLeg",
    "leftFoot": "leftFoot",
    "rightUpperLeg": "rightUpperLeg", "rightLowerLeg": "rightLowerLeg",
    "rightFoot": "rightFoot",
}

# Fallback name heuristics for plain glTF/FBX rigs (no VRM humanoid extension).
NAME_HINTS = [
    ("hips", ("hips", "pelvis", "hip", "root", "bip01 pelvis", "j_bip_c_hips")),
    ("spine", ("spine", "waist", "abdomen", "spine1", "j_bip_c_spine")),
    ("chest", ("chest", "upperchest", "ribcage", "spine2", "spine3", "j_bip_c_chest")),
    ("neck", ("neck", "j_bip_c_neck")),
    ("head", ("head", "j_bip_c_head")),
    ("leftUpperArm", ("leftupperarm", "upper_arm.l", "shoulder.l", "lupperarm",
                      "l_upperarm", "arm.l", "j_bip_l_upperarm")),
    ("leftLowerArm", ("leftlowerarm", "forearm.l", "lower_arm.l", "lforearm",
                      "l_lowerarm", "j_bip_l_lowerarm")),
    ("leftHand", ("lefthand", "hand.l", "lhand", "j_bip_l_hand")),
    ("rightUpperArm", ("rightupperarm", "upper_arm.r", "shoulder.r", "rupperarm",
                       "r_upperarm", "arm.r", "j_bip_r_upperarm")),
    ("rightLowerArm", ("rightlowerarm", "forearm.r", "lower_arm.r", "rforearm",
                       "r_lowerarm", "j_bip_r_lowerarm")),
    ("rightHand", ("righthand", "hand.r", "rhand", "j_bip_r_hand")),
    ("leftUpperLeg", ("leftupperleg", "thigh.l", "upper_leg.l", "lthigh",
                      "l_upperleg", "leg.l", "j_bip_l_upperleg")),
    ("leftLowerLeg", ("leftlowerleg", "shin.l", "calf.l", "lower_leg.l",
                      "lshin", "l_lowerleg", "j_bip_l_lowerleg")),
    ("leftFoot", ("leftfoot", "foot.l", "lfoot", "j_bip_l_foot")),
    ("rightUpperLeg", ("rightupperleg", "thigh.r", "upper_leg.r", "rthigh",
                       "r_upperleg", "leg.r", "j_bip_r_upperleg")),
    ("rightLowerLeg", ("rightlowerleg", "shin.r", "calf.r", "lower_leg.r",
                       "rshin", "r_lowerleg", "j_bip_r_lowerleg")),
    ("rightFoot", ("rightfoot", "foot.r", "rfoot", "j_bip_r_foot")),
]

MORPH_HINTS = {
    "blink": ("blink", "eyeclose", "eye_close", "close", "mabataki", "wink"),
    "happy": ("happy", "joy", "smile", "fun", "warai", "egao"),
    "surprised": ("surprised", "surprise", "odoroki", "bikkuri"),
}

# VRM 0.x / 1.0 preset names that should drive the three KimiOS expressions,
# in priority order (a full blink wins over per-eye blinks, etc.).
PRESET_ALIASES = {
    "blink": ("blink", "blink_l", "blink_r", "blinkleft", "blinkright"),
    "happy": ("happy", "joy", "fun", "relaxed"),
    "surprised": ("surprised", "surprise", "unknown"),
}


# --------------------------------------------------------------------------- utils
def log(msg):
    print("VRM2KIMI|%s" % msg)


def warn(msg, warns):
    warns.append(msg)
    print("VRM2KIMI|WARN %s" % msg)


def reset_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    for coll in (bpy.data.meshes, bpy.data.materials, bpy.data.images,
                 bpy.data.armatures, bpy.data.objects):
        for item in list(coll):
            try:
                coll.remove(item)
            except Exception:
                pass


def read_gltf_json(path):
    """Return (gltf_json, error). Works for .glb, .vrm (GLB container) and .gltf."""
    try:
        if path.lower().endswith(".gltf"):
            with open(path, "r", encoding="utf-8") as f:
                return json.load(f), None
        g = verify_glb.parse_glb(path)
        return g["json"], None
    except Exception as exc:
        return None, repr(exc)


def parse_source_meta(path):
    """Pull the humanoid map, expression binds and morph names out of the file."""
    meta = {"vrm_version": None, "facing": "+Z", "humanoid": {}, "expressions": {},
            "morph_names": {}, "mesh_names": [], "error": None}
    gltf, err = read_gltf_json(path)
    if gltf is None:
        meta["error"] = err
        return meta
    nodes = gltf.get("nodes", [])
    meta["mesh_names"] = [m.get("name", "mesh%d" % i)
                          for i, m in enumerate(gltf.get("meshes", []))]
    for mi, mesh in enumerate(gltf.get("meshes", [])):
        names = (mesh.get("extras") or {}).get("targetNames")
        if names:
            meta["morph_names"][mi] = list(names)
    ext = gltf.get("extensions", {}) or {}
    vrm0 = ext.get("VRM")
    vrm1 = ext.get("VRMC_vrm")
    if vrm0:
        meta["vrm_version"] = 0
        meta["facing"] = "-Z"                      # VRM 0.x avatars face -Z
        for hb in (vrm0.get("humanoid", {}) or {}).get("humanBones", []) or []:
            slot = hb.get("bone")
            idx = hb.get("node")
            if slot and isinstance(idx, int) and 0 <= idx < len(nodes):
                meta["humanoid"][nodes[idx].get("name", "")] = slot
        groups = (vrm0.get("blendShapeMaster", {}) or {}).get("blendShapeGroups", [])
        for grp in groups or []:
            preset = (grp.get("presetName") or "").strip().lower()
            binds = []
            for bind in grp.get("binds", []) or []:
                try:
                    binds.append((int(bind.get("mesh", 0)), int(bind.get("index", 0)),
                                  float(bind.get("weight", 100.0)) / 100.0))
                except Exception:
                    pass
            if preset:
                meta["expressions"].setdefault(preset, []).extend(binds)
    elif vrm1:
        meta["vrm_version"] = 1
        meta["facing"] = "+Z"                      # VRM 1.0 faces +Z
        hb = (vrm1.get("humanoid", {}) or {}).get("humanBones", {}) or {}
        for slot, entry in hb.items():
            idx = (entry or {}).get("node")
            if isinstance(idx, int) and 0 <= idx < len(nodes):
                meta["humanoid"][nodes[idx].get("name", "")] = slot
        ex = (vrm1.get("expressions", {}) or {}).get("preset", {}) or {}
        for slot, entry in ex.items():
            binds = []
            for bind in (entry or {}).get("morphTargetBinds", []) or []:
                node_idx = (bind or {}).get("node")
                ti = (bind or {}).get("index")
                weight = (bind or {}).get("weight", 1.0)
                mesh_idx = None
                if isinstance(node_idx, int) and 0 <= node_idx < len(nodes):
                    mesh_idx = nodes[node_idx].get("mesh")
                if isinstance(ti, int):
                    binds.append((mesh_idx if mesh_idx is not None else 0, ti,
                                  float(weight)))
            if binds:
                meta["expressions"].setdefault(slot.lower(), []).extend(binds)
    return meta


def norm(name):
    return name.lower().replace("_", "").replace("-", "").replace(".", "").replace(" ", "")


def map_by_name(bone_names):
    """Best-effort canonical mapping from bone names (normalised)."""
    out = {}
    for bone in bone_names:
        nb = norm(bone)
        best = None
        for canon, hints in NAME_HINTS:
            for hint in hints:
                if nb == norm(hint):
                    best = (canon, 3)
                    break
                if norm(hint) in nb:
                    cand = (canon, 2 - (0 if nb.startswith(norm(hint)) else 1))
                    if best is None or cand[1] > best[1]:
                        best = cand
            if best and best[1] >= 3:
                break
        if best:
            out[bone] = best[0]
    return out


def map_from_vrm(bone_names, humanoid):
    out = {}
    for bone in bone_names:
        slot = humanoid.get(bone)
        if slot and slot in VRM_SLOTS:
            out[bone] = VRM_SLOTS[slot]
    return out


# --------------------------------------------------------------------------- rig
def find_armature():
    arms = [o for o in bpy.context.scene.objects if o.type == 'ARMATURE']
    if not arms:
        return None
    arms.sort(key=lambda o: len(o.data.bones), reverse=True)
    return arms[0]


def source_skeleton(arm_obj):
    """name -> dict(head, tail, parent) in world space."""
    mw = arm_obj.matrix_world
    out = {}
    for b in arm_obj.data.bones:
        out[b.name] = {
            "head": (mw @ b.head_local).copy(),
            "tail": (mw @ b.tail_local).copy(),
            "parent": b.parent.name if b.parent else None,
        }
    return out


def canonical_positions(src, mapping, warns):
    """Resolve the 17 canonical bone head/tail positions from the mapped source."""
    def pos(canon, which):
        for bone, c in mapping.items():
            if c == canon and bone in src:
                return src[bone][which].copy()
        return None

    head = {c: pos(c, "head") for c in BONE_NAMES}
    tail = {c: pos(c, "tail") for c in BONE_NAMES}

    # spine chain defaults out of whatever the source has
    if head["hips"] is None:
        for cand in ("spine", "chest", "leftUpperLeg", "rightUpperLeg", "neck"):
            if head[cand] is not None:
                head["hips"] = head[cand].copy() + Vector((0.0, 0.0, 0.07))
                break
    if head["hips"] is None:
        head["hips"] = Vector((0.0, 0.0, 0.60))
    for c in ("spine", "chest", "neck", "head"):
        if head[c] is None:
            parent = BONE_PARENT[c]
            base = head[parent] if head[parent] is not None else head["hips"]
            d = {"spine": 0.12, "chest": 0.12, "neck": 0.09, "head": 0.42}[c]
            head[c] = base.copy() + Vector((0.0, 0.0, d))
    # tails: use the mapped tail when sane, else the next bone's head
    order = ["hips", "spine", "chest", "neck"]
    for i, c in enumerate(order):
        nxt = order[i + 1] if i + 1 < len(order) else "head"
        if tail[c] is None or (head[nxt] - tail[c]).length < 1e-4:
            tail[c] = head[nxt].copy()
    if tail["head"] is None:
        tail["head"] = head["head"].copy() + Vector((0.0, 0.0, 0.30))

    # arms / legs: fall back to symmetric offsets from the chain
    for side, sgn in (("left", 1.0), ("right", -1.0)):
        ua, la, ha = side + "UpperArm", side + "LowerArm", side + "Hand"
        if head[ua] is None:
            head[ua] = tail["chest"] + Vector((sgn * 0.13, 0.0, 0.0))
        if tail[ua] is None:
            tail[ua] = head[ua] + Vector((sgn * 0.20, 0.0, 0.0))
        if head[la] is None:
            head[la] = tail[ua].copy()
        if tail[la] is None:
            tail[la] = head[la] + Vector((sgn * 0.18, 0.0, 0.0))
        if head[ha] is None:
            head[ha] = tail[la].copy()
        if tail[ha] is None:
            tail[ha] = head[ha] + Vector((sgn * 0.09, 0.0, 0.0))
        ul, ll, ft = side + "UpperLeg", side + "LowerLeg", side + "Foot"
        if head[ul] is None:
            head[ul] = head["hips"] + Vector((sgn * 0.085, 0.0, -0.02))
        if tail[ul] is None:
            tail[ul] = head[ul] + Vector((0.0, 0.0, -0.26))
        if head[ll] is None:
            head[ll] = tail[ul].copy()
        if tail[ll] is None:
            tail[ll] = head[ll] + Vector((0.0, 0.0, -0.25))
        if head[ft] is None:
            head[ft] = tail[ll].copy()
        if tail[ft] is None:
            tail[ft] = head[ft] + Vector((0.0, -0.11, -0.05))
    # collapse any zero-length bone
    for c in BONE_NAMES:
        if (tail[c] - head[c]).length < 1e-5:
            tail[c] = head[c] + Vector((0.0, 0.0, 0.03))
    return {c: (head[c], tail[c]) for c in BONE_NAMES}


def build_canonical_armature(positions, name="KimiRig"):
    arm = bpy.data.armatures.new(name + "Armature")
    obj = bpy.data.objects.new(name, arm)
    bpy.context.scene.collection.objects.link(obj)
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    bpy.ops.object.mode_set(mode='EDIT')
    ebs = {}
    for canon, parent, _h, _t in BONES:
        head, tail = positions[canon]
        eb = arm.edit_bones.new(canon)
        eb.head = head
        eb.tail = tail
        eb.use_deform = True
        ebs[canon] = eb
    for canon, parent, _h, _t in BONES:
        if parent:
            ebs[canon].parent = ebs[parent]
    bpy.ops.object.mode_set(mode='OBJECT')
    obj.show_in_front = True
    log("armature: rebuilt %d canonical bones" % len(arm.bones))
    return obj


def canonical_map_for_skeleton(src, meta, warns):
    """Map every source bone name to a canonical bone, folding children upward."""
    mapping = map_from_vrm(list(src.keys()), meta.get("humanoid") or {})
    if not mapping:
        warn("no VRM humanoid map found; falling back to bone-name heuristics", warns)
        mapping = map_by_name(list(src.keys()))
    if not mapping:
        warn("could not map any bone by name either", warns)
    # fold unmapped bones into the nearest mapped ancestor
    folded = 0
    for bone in src:
        if bone in mapping:
            continue
        cur = src[bone].get("parent")
        guard = 0
        while cur is not None and guard < 64:
            if cur in mapping:
                mapping[bone] = mapping[cur]
                folded += 1
                break
            cur = src[cur].get("parent") if cur in src else None
            guard += 1
    log("bone mapping: %d mapped, %d folded into an ancestor"
        % (len(mapping) - folded, folded))
    return mapping


# --------------------------------------------------------------------------- mesh
def bake_world(obj):
    """Apply the object's world matrix to its vertices (rest positions)."""
    mw = obj.matrix_world.copy()
    me = obj.data
    for v in me.vertices:
        v.co = mw @ v.co
    me.update()
    obj.parent = None
    obj.matrix_world = Matrix.Identity(4)


def merge_split_vertices(obj):
    """glTF meshes are usually fully vertex-split. Merging by distance (and
    smoothing the normals) collapses those duplicates, which is what makes a
    42k-vertex VRoid avatar fit the device's vertex arena at all."""
    before = len(obj.data.vertices)
    bpy.ops.object.select_all(action='DESELECT')
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    try:
        bpy.ops.object.mode_set(mode='EDIT')
        bpy.ops.mesh.select_all(action='SELECT')
        bpy.ops.mesh.remove_doubles(threshold=1e-5)
        bpy.ops.mesh.normals_make_consistent(inside=False)
        bpy.ops.object.mode_set(mode='OBJECT')
        for poly in obj.data.polygons:
            poly.use_smooth = True
    except Exception as exc:
        try:
            bpy.ops.object.mode_set(mode='OBJECT')
        except Exception:
            pass
        warn("could not merge split vertices on %r: %r" % (obj.name, exc), [])
        return before
    # shape keys survive an edit-mode merge in Blender, but re-verify
    if obj.data.shape_keys:
        for kb in obj.data.shape_keys.key_blocks:
            if len(kb.data) != len(obj.data.vertices):
                warn("shape key %r lost alignment after merge" % kb.name, [])
    after = len(obj.data.vertices)
    log("merged split vertices on %r: %d -> %d" % (obj.name, before, after))
    return after


def retarget_weights(obj, mapping, positions, root_parent):
    """Replace the source vertex groups with the 17 canonical ones."""
    src_names = [g.name for g in obj.vertex_groups]
    me = obj.data
    per_vert = []
    orphans = 0
    for v in me.vertices:
        acc = {}
        for g in v.groups:
            if g.group >= len(src_names):
                continue
            canon = mapping.get(src_names[g.group])
            if canon is None or g.weight <= 0.0:
                continue
            acc[canon] = acc.get(canon, 0.0) + g.weight
        if not acc:
            orphans += 1
            # nearest canonical bone segment
            best, bestd = None, 1e9
            for canon, (h, t) in positions.items():
                ab = t - h
                l2 = ab.length_squared
                p = v.co
                if l2 < 1e-12:
                    d = (p - h).length
                else:
                    k = max(0.0, min(1.0, (p - h).dot(ab) / l2))
                    d = (p - (h + ab * k)).length
                if d < bestd:
                    best, bestd = canon, d
            if best:
                acc[best] = 1.0
        items = sorted(acc.items(), key=lambda kv: -kv[1])[:4]
        total = sum(w for _, w in items)
        per_vert.append([(n, w / total) for n, w in items] if total > 0 else [])
    for g in list(obj.vertex_groups):
        obj.vertex_groups.remove(g)
    groups = {n: obj.vertex_groups.new(name=n) for n in BONE_NAMES}
    for vi, ws in enumerate(per_vert):
        for n, w in ws:
            groups[n].add([vi], w, 'REPLACE')
    if orphans:
        log("weights: %d vertices had no mapped influence -> nearest bone" % orphans)
    return per_vert


def combine_morphs(obj, gltf_mesh_index, meta, warns):
    """Build blink/happy/surprised keys from the source expression binds."""
    me = obj.data
    if me.shape_keys is None:
        return
    kb = list(me.shape_keys.key_blocks)
    if len(kb) <= 1:
        return
    basis = kb[0]
    names = meta.get("morph_names", {}).get(gltf_mesh_index, [])
    # target index -> shape key
    by_target = {}
    for ti in range(len(kb) - 1):
        key = kb[ti + 1]
        by_target[ti] = key

    def add_binds(canon, binds):
        weight_sum = {}
        for (mesh_i, ti, w) in binds:
            if mesh_i is not None and mesh_i != gltf_mesh_index:
                continue
            if ti not in by_target or w == 0.0:
                continue
            weight_sum[ti] = weight_sum.get(ti, 0.0) + w
        if not weight_sum:
            return False
        new = obj.shape_key_add(name=canon, from_mix=False)
        new.value = 0.0
        nverts = len(me.vertices)
        for ti, w in weight_sum.items():
            src = by_target[ti]
            for vi in range(nverts):
                d = src.data[vi].co - basis.data[vi].co
                if w != 1.0:
                    d = d * w
                new.data[vi].co = Vector(new.data[vi].co) + d
        return True

    made = []
    for canon in EXPRESSIONS:
        binds = []
        # combine every preset alias for this expression, highest priority first
        for alias in PRESET_ALIASES[canon]:
            if alias in meta.get("expressions", {}):
                binds.extend(meta["expressions"][alias])
                if alias == "blink" and canon == "blink":
                    break            # a full blink supersedes blink_l/blink_r
        ok = add_binds(canon, binds) if binds else False
        if not ok:
            # name-based fallback
            for ti, key in by_target.items():
                nm = norm(key.name)
                if any(h in nm for h in MORPH_HINTS[canon]):
                    ok = add_binds(canon, [(gltf_mesh_index, ti, 1.0)])
                    break
        if ok:
            made.append(canon)
    if made:
        log("morphs: combined %s from mesh %d" % (",".join(made), gltf_mesh_index))
    # keep only Basis + the three canonical keys
    keep = set(EXPRESSIONS)
    for key in list(me.shape_keys.key_blocks):
        if key.name == "Basis" or key.name in keep:
            continue
        try:
            obj.shape_key_remove(key)
        except Exception:
            pass
    for key in me.shape_keys.key_blocks:
        key.value = 0.0


def transfer_shape_keys(obj, snapshot):
    """After decimation, rebuild the shape keys by nearest-vertex transfer."""
    me = obj.data
    for key in list(me.shape_keys.key_blocks if me.shape_keys else []):
        obj.shape_key_remove(key)
    if not snapshot:
        return
    old_pos, keys = snapshot
    kd = kdtree.KDTree(len(old_pos))
    for i, co in enumerate(old_pos):
        kd.insert(co, i)
    kd.balance()
    basis = obj.shape_key_add(name="Basis", from_mix=False)
    basis.value = 0.0
    for name, deltas in keys:
        kb = obj.shape_key_add(name=name, from_mix=False)
        kb.value = 0.0
        for vi, v in enumerate(me.vertices):
            _, idx, _ = kd.find(v.co)
            kb.data[vi].co = Vector(v.co) + deltas[idx]


def snapshot_shape_keys(obj):
    me = obj.data
    if me.shape_keys is None or len(me.shape_keys.key_blocks) <= 1:
        return None
    basis = me.shape_keys.key_blocks[0]
    old_pos = [v.co.copy() for v in me.vertices]
    keys = []
    for kb in list(me.shape_keys.key_blocks)[1:]:
        deltas = []
        for vi in range(len(me.vertices)):
            deltas.append(kb.data[vi].co - basis.data[vi].co)
        keys.append((kb.name, deltas))
    return (old_pos, keys)


# --------------------------------------------------------------------------- textures
def detect_facing(src, mapping, meta, warns):
    """Decide whether the imported model already faces -Y (the KimiOS front).

    The VRM spec's front axis differs between 0.x and 1.0 and real exports get it
    wrong, so geometry decides: the eye bones sit in front of the head bone, and
    the toe bones in front of the foot bones. Both are real node translations in
    the imported armature (unlike bone *tails*, which the importer synthesises).
    """
    humanoid = meta.get("humanoid") or {}

    def head_of(slot):
        for node_name, s in humanoid.items():
            if s == slot and node_name in src:
                return src[node_name]["head"]
        return None

    head = head_of("head")
    eyes = [p for p in (head_of("leftEye"), head_of("rightEye")) if p is not None]
    if head is not None and eyes:
        eye = sum(eyes, Vector((0, 0, 0))) / float(len(eyes))
        d = eye - head
        if abs(d.y) > 1e-4:
            log("facing: eye bones are at y=%+.3f from the head -> %s"
                % (d.y, "rotating 180" if d.y > 0 else "already faces -Y"))
            return 1.0 if d.y > 0 else 0.0
    for foot_slot, toe_slot in (("leftFoot", "leftToes"), ("rightFoot", "rightToes")):
        f, t = head_of(foot_slot), head_of(toe_slot)
        if f is not None and t is not None:
            d = t - f
            if abs(d.y) > 1e-4:
                log("facing: toe bones are at y=%+.3f from the foot -> %s"
                    % (d.y, "rotating 180" if d.y > 0 else "already faces -Y"))
                return 1.0 if d.y > 0 else 0.0
    spec_rot = 1.0 if meta.get("facing") == "-Z" else 0.0
    warn("facing: no eye/toe bones, falling back to the VRM spec (%s)"
         % ("rotate 180" if spec_rot else "none"), warns)
    return spec_rot


def set_flat_material(mat, rgb):
    """Rebuild a material as one Principled BSDF of a flat colour. VRM MToon
    materials import with exotic node trees (often unlit), so poking at a
    'Principled BSDF' node is not reliable -- replacing the tree always works."""
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    out.location = (300, 0)
    bsdf = nt.nodes.new("ShaderNodeBsdfPrincipled")
    bsdf.location = (0, 0)
    bsdf.inputs["Base Color"].default_value = (rgb[0], rgb[1], rgb[2], 1.0)
    bsdf.inputs["Metallic"].default_value = 0.0
    bsdf.inputs["Roughness"].default_value = 0.8
    nt.links.new(bsdf.outputs["BSDF"], out.inputs["Surface"])
    mat.diffuse_color = (rgb[0], rgb[1], rgb[2], 1.0)


def fix_images(max_size, warns):
    """Convert every image to a packed PNG <= max_size.

    Blender keeps the *source* bytes of a packed glTF image, so simply calling
    scale()+pack() does not change what the exporter embeds. The image is saved
    to a real PNG and swapped in as a fresh datablock instead.
    """
    tmpdir = os.path.join(tempfile.gettempdir(), "kimi_tex")
    os.makedirs(tmpdir, exist_ok=True)
    images = [i for i in bpy.data.images if i.type == 'IMAGE' and i.size[0] > 0]
    for idx, img in enumerate(images):
        w, h = img.size
        try:
            if max(w, h) > max_size:
                scale = max_size / float(max(w, h))
                img.scale(max(1, int(round(w * scale))), max(1, int(round(h * scale))))
            tmp = os.path.join(tmpdir, "tex_%03d.png" % idx)
            img.file_format = 'PNG'
            img.filepath_raw = tmp
            img.save()
            new = bpy.data.images.load(tmp, check_existing=False)
            new.name = img.name + "_kimi"
            new.colorspace_settings.name = img.colorspace_settings.name
            new.pack()
            # repoint every texture node that used the original
            swapped = 0
            for mat in bpy.data.materials:
                if not mat.node_tree:
                    continue
                for node in mat.node_tree.nodes:
                    if node.type == 'TEX_IMAGE' and node.image is img:
                        node.image = new
                        swapped += 1
            log("texture %r: %dx%d -> %dx%d PNG, %d node(s) repointed"
                % (img.name, w, h, new.size[0], new.size[1], swapped))
            bpy.data.images.remove(img)
        except Exception as exc:
            warn("could not re-encode image %r: %r" % (img.name, exc), warns)
    imgs = [i for i in bpy.data.images if i.type == 'IMAGE' and i.size[0] > 0]
    if len(imgs) > CAP_TEXTURES:
        warn("%d textures but the device keeps only %d" % (len(imgs), CAP_TEXTURES),
             warns)
    for img in imgs:
        if max(img.size) > PNG_MAX_DIM:
            warn("texture %r still larger than %d px (device would skip it)"
                 % (img.name, PNG_MAX_DIM), warns)


# --------------------------------------------------------------------------- main flow
def import_source(path, workdir, warns):
    low = path.lower()
    if low.endswith(".vrm"):
        # the bundled glTF importer keys off the extension
        tmp = os.path.join(workdir, "source.glb")
        shutil.copyfile(path, tmp)
        path = tmp
        low = tmp.lower()
    if low.endswith((".glb", ".gltf", ".vrm")):
        kw = dict(import_pack_images=True, disable_bone_shape=True)
        try:
            bpy.ops.import_scene.gltf(filepath=path, **kw)
        except TypeError:
            bpy.ops.import_scene.gltf(filepath=path, import_pack_images=True)
    elif low.endswith(".fbx"):
        bpy.ops.import_scene.fbx(filepath=path)
    else:
        raise SystemExit("unsupported input (use .vrm/.glb/.gltf/.fbx): %s" % path)
    for ob in list(bpy.context.scene.objects):
        if ob.type in ('CAMERA', 'LIGHT'):
            try:
                bpy.data.objects.remove(ob, do_unlink=True)
            except Exception:
                pass
    # The glTF importer adds a bone-display icosphere in a "glTF_not_exported"
    # collection; it is not hidden from render and would poison the bbox/skin.
    meshes = []
    for ob in list(bpy.context.scene.objects):
        if ob.type != 'MESH':
            continue
        if any(c.name == "glTF_not_exported" for c in ob.users_collection):
            continue
        if "icosphere" in ob.name.lower() or "bone_shape" in ob.name.lower():
            continue
        # stray props (the factory cube, placeholder boxes) are not part of the avatar
        if len(ob.data.vertices) < 12 and not ob.vertex_groups:
            log("dropping stray mesh %r (%d verts, unskinned)"
                % (ob.name, len(ob.data.vertices)))
            continue
        meshes.append(ob)
    for ob in list(bpy.context.scene.objects):
        if ob.type == 'MESH' and ob not in meshes:
            try:
                bpy.data.objects.remove(ob, do_unlink=True)
            except Exception:
                pass
    if not meshes:
        raise SystemExit("no mesh found in %s" % path)
    log("imported %d mesh object(s)" % len(meshes))
    return meshes


def render_preview(out_path, arm_obj, body_obj):
    """Plain studio render of the converted avatar so the result can be judged."""
    scene = bpy.context.scene
    world = bpy.data.worlds.new("PreviewWorld")
    world.use_nodes = True
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.45, 0.48, 0.56, 1.0)
    scene.world = world
    for name, toward, energy in (
            ("Key", (0.42, -0.72, 0.75), 3.6),
            ("Fill", (-0.65, -0.45, 0.30), 1.2),
            ("Rim", (0.10, 0.85, 0.35), 2.4)):
        li = bpy.data.lights.new(name, type='SUN')
        li.energy = energy
        li.angle = math.radians(14)
        ob = bpy.data.objects.new(name, li)
        ob.rotation_euler = Vector(toward).normalized().to_track_quat('Z', 'Y').to_euler()
        scene.collection.objects.link(ob)
    cd = bpy.data.cameras.new("PreviewCam")
    cd.lens = 70.0
    cd.sensor_fit = 'VERTICAL'
    cd.sensor_height = 24.0
    cam = bpy.data.objects.new("PreviewCam", cd)
    scene.collection.objects.link(cam)
    # frame the converted mesh
    co = [arm_obj.matrix_world @ v.co for v in body_obj.data.vertices]
    mn = Vector((min(c.x for c in co), min(c.y for c in co), min(c.z for c in co)))
    mx = Vector((max(c.x for c in co), max(c.y for c in co), max(c.z for c in co)))
    centre = (mn + mx) * 0.5
    height = max(0.5, mx.z - mn.z)
    dist = (height * 1.18) / (2.0 * math.tan(math.radians(19.5) * 0.5))
    yaw = math.radians(28.0)
    cam.location = (centre.x + dist * math.sin(yaw), centre.y - dist * math.cos(yaw),
                    centre.z + height * 0.06)
    d = centre - Vector(cam.location)
    cam.rotation_euler = d.to_track_quat('-Z', 'Y').to_euler()
    scene.render.engine = 'BLENDER_EEVEE'
    try:
        scene.eevee.taa_render_samples = 32
        scene.view_settings.view_transform = 'Standard'
    except Exception:
        pass
    scene.render.resolution_x, scene.render.resolution_y = 900, 900
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = False
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGB'
    scene.camera = cam
    scene.render.filepath = os.path.abspath(out_path)
    bpy.ops.render.render(write_still=True)
    log("preview -> %s (%d bytes)" % (out_path, os.path.getsize(out_path)))


def main(argv):
    ap = argparse.ArgumentParser(prog="vrm_to_kimios")
    ap.add_argument("--input", required=True)
    ap.add_argument("--out", default=None)
    ap.add_argument("--report", default=None)
    ap.add_argument("--check-only", action="store_true")
    ap.add_argument("--tex-size", type=int, default=256)
    ap.add_argument("--max-tris", type=int, default=4000)
    ap.add_argument("--max-verts", type=int, default=5000)
    ap.add_argument("--keep-morphs", action="store_true",
                    help="(default) keep the three canonical morphs after decimation")
    ap.add_argument("--drop-morphs", action="store_true",
                    help="drop facial morphs to spend the whole vertex budget on shape")
    ap.add_argument("--no-decimate", action="store_true")
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--preview", default=None,
                    help="also render a studio PNG of the converted avatar")
    ap.add_argument("--save-blend", default=None)
    args = ap.parse_args(argv)
    if args.tex_size > 256:
        raise SystemExit("--tex-size must be <= 256")

    warns = []
    src_path = os.path.abspath(args.input)
    if not os.path.isfile(src_path):
        raise SystemExit("no such input: %s" % src_path)
    reset_scene()
    stem = os.path.splitext(os.path.basename(src_path))[0]
    out_path = args.out or os.path.join(REPO, "assets", "models", stem + ".glb")
    workdir = os.path.join(HERE, "_work")
    os.makedirs(workdir, exist_ok=True)
    report = {"input": src_path, "out": os.path.abspath(out_path), "warnings": warns}

    meta = parse_source_meta(src_path)
    report["source"] = {
        "vrm": meta["vrm_version"], "facing": meta["facing"],
        "humanoid_bones": len(meta["humanoid"]),
        "expressions": {k: len(v) for k, v in meta["expressions"].items()},
        "error": meta["error"],
    }
    if meta["vrm_version"] is not None:
        log("source: VRM %s (faces %s), %d humanoid bones mapped, expressions: %s"
            % (meta["vrm_version"], meta["facing"], len(meta["humanoid"]),
               ", ".join(sorted(meta["expressions"])) or "none"))
    else:
        log("source: plain glTF/FBX (no VRM extension): %s" % (meta["error"] or "ok"))

    if args.check_only:
        gltf, err = read_gltf_json(src_path)
        if gltf is None:
            raise SystemExit("cannot parse %s: %s" % (src_path, err))
        prims = [(mi, pi) for mi, m in enumerate(gltf.get("meshes", []))
                 for pi, _p in enumerate(m.get("primitives", []))]
        n_tris = 0
        n_verts = 0
        for mi, mesh in enumerate(gltf.get("meshes", [])):
            for prim in mesh.get("primitives", []):
                idx = prim.get("indices")
                if idx is not None:
                    n_tris += gltf["accessors"][idx]["count"] // 3
                pos = prim["attributes"].get("POSITION")
                if pos is not None:
                    n_verts += gltf["accessors"][pos]["count"]
        checks = [
            ("triangles", n_tris, args.max_tris),
            ("vertices", n_verts, args.max_verts),
            ("joints", len(gltf.get("skins", [{}])[0].get("joints", [])) if gltf.get("skins") else 0, CAP_JOINTS),
            ("nodes", len(gltf.get("nodes", [])), CAP_NODES),
            ("primitives", len(prims), CAP_PRIMS),
            ("materials", len(gltf.get("materials", [])), CAP_MATERIALS),
            ("images", len(gltf.get("images", [])), CAP_TEXTURES),
        ]
        report["check"] = {name: {"value": v, "limit": lim, "ok": v <= lim}
                           for name, v, lim in checks}
        for name, v, lim in checks:
            log("check %-11s %6d / %6d  %s" % (name, v, lim, "ok" if v <= lim else "OVER"))
        print("VRM2KIMI|CHECK " + json.dumps(report, sort_keys=True))
        bad = [n for n, v, lim in checks if v > lim]
        return 0 if not bad else 1

    import_source(src_path, workdir, warns)
    arm = find_armature()
    meshes = [o for o in bpy.context.scene.objects if o.type == 'MESH']
    if arm is None:
        raise SystemExit("no armature in the source: KimiOS needs a 17-joint skin")
    src = source_skeleton(arm)
    mapping = canonical_map_for_skeleton(src, meta, warns)
    if not mapping:
        raise SystemExit("could not map a single source bone to the humanoid rig")
    positions = canonical_positions(src, mapping, warns)

    # ---- orientation + feet at Y=0 -------------------------------------
    # Everything is brought into world space once, then rotated once, then
    # grounded once -- mesh vertices and skeleton stay in lock-step.
    rot = (Matrix.Rotation(math.pi, 4, 'Z')
           if detect_facing(src, mapping, meta, warns) else Matrix.Identity(4))
    for ob in meshes:
        bake_world(ob)
        if rot != Matrix.Identity(4):
            for v in ob.data.vertices:
                v.co = rot @ v.co
            ob.data.update()
    positions = canonical_positions(src, mapping, warns)
    positions = {k: (rot @ h, rot @ t) for k, (h, t) in positions.items()}
    min_z = min((v.co.z for ob in meshes for v in ob.data.vertices), default=0.0)
    shift = Vector((-positions["hips"][0].x, 0.0, -min_z))
    for ob in meshes:
        for v in ob.data.vertices:
            v.co = v.co + shift
        ob.data.update()
    positions = {k: (h + shift, t + shift) for k, (h, t) in positions.items()}
    log("baked transforms: rotate180=%s, feet lifted by %.3f, hips centred on x"
        % (rot != Matrix.Identity(4), -min_z))

    # ---- per-mesh: merge split verts, morphs, weights --------------------
    objs_sorted = meshes
    for ob in objs_sorted:
        merge_split_vertices(ob)
    for mi, ob in enumerate(objs_sorted):
        gmesh = None
        for cand_i, cand_name in enumerate(meta.get("mesh_names", [])):
            if ob.data.name == cand_name:
                gmesh = cand_i
                break
        if gmesh is None:
            gmesh = mi
        combine_morphs(ob, gmesh, meta, warns)
        retarget_weights(ob, mapping, positions, None)

    # ---- join ----------------------------------------------------------
    arm_old = arm
    bpy.ops.object.select_all(action='DESELECT')
    for ob in objs_sorted:
        ob.select_set(True)
    bpy.context.view_layer.objects.active = objs_sorted[0]
    if len(objs_sorted) > 1:
        bpy.ops.object.join()
    body = bpy.context.view_layer.objects.active
    body.name = "KimiBody"
    body.data.name = "KimiBodyMesh"
    for ob in list(bpy.context.scene.objects):
        if ob.type == 'ARMATURE':
            bpy.data.objects.remove(ob, do_unlink=True)
    if body.parent is not None:
        body.parent = None
    body.matrix_world = Matrix.Identity(4)
    # one UV layer
    if not body.data.uv_layers:
        warn("mesh has no UV layer; adding a flat one", warns)
        body.data.uv_layers.new(name="UVMap")
    uvl = body.data.uv_layers[0]
    uvl.name = "UVMap"
    body.data.uv_layers.active = uvl

    # ---- simplify to the loader caps ----------------------------------
    body.data.calc_loop_triangles()
    tris = len(body.data.loop_triangles)
    verts = len(body.data.vertices)
    log("joined mesh: %d verts, %d tris, %d material slots, %d shape keys"
        % (verts, tris, len(body.data.materials),
           len(body.data.shape_keys.key_blocks) if body.data.shape_keys else 0))

    # estimate the post-split vertex count (UV/normal seams add a little)
    nmorphs = len(body.data.shape_keys.key_blocks) - 1 if body.data.shape_keys else 0
    vert_budget = args.max_verts
    if nmorphs > 0:
        # the device stores one delta entry per (vertex, target)
        morph_budget = CAP_MORPH_ENTRIES // nmorphs
        vert_budget = min(vert_budget, morph_budget)
    est_verts = int(verts * 1.05) + 16
    need_ratio = 1.0
    if tris > args.max_tris:
        need_ratio = min(need_ratio, args.max_tris / float(tris))
    if est_verts > vert_budget:
        need_ratio = min(need_ratio, vert_budget / float(est_verts))
    morph_snapshot = snapshot_shape_keys(body)
    # stale modifiers from the join (the old armature) must not precede Decimate
    for m in list(body.modifiers):
        try:
            body.modifiers.remove(m)
        except Exception:
            pass
    if need_ratio < 1.0:
        if args.no_decimate:
            warn("model needs decimation to fit but --no-decimate was given", warns)
        else:
            log("decimating to %.2f of the original triangles (morph deltas will "
                "be transferred by nearest vertex)" % need_ratio)
            # Blender refuses to apply a modifier while shape keys exist, so the
            # keys are snapshotted, removed, and rebuilt on the new topology.
            if body.data.shape_keys is not None:
                for key in list(body.data.shape_keys.key_blocks):
                    body.shape_key_remove(key)
            mod = body.modifiers.new("Fit", 'DECIMATE')
            mod.decimate_type = 'COLLAPSE'
            mod.ratio = max(0.02, need_ratio)
            bpy.context.view_layer.objects.active = body
            bpy.ops.object.modifier_apply(modifier=mod.name)
            if args.drop_morphs:
                morph_snapshot = None
            transfer_shape_keys(body, morph_snapshot)
            body.data.calc_loop_triangles()
            tris = len(body.data.loop_triangles)
            verts = len(body.data.vertices)
            log("after decimation: %d verts, %d tris, %d shape keys"
                % (verts, tris,
                   len(body.data.shape_keys.key_blocks) if body.data.shape_keys else 0))

    # ---- materials + textures -----------------------------------------
    fix_images(args.tex_size, warns)
    slots = [m for m in body.data.materials if m is not None]
    if len(slots) > CAP_MATERIALS:
        warn("%d materials (device keeps %d); extra slots dropped"
             % (len(slots), CAP_MATERIALS), warns)
    # The device has only CAP_TEXTURES texture slots. Rank materials by how many
    # faces they cover, keep the top ones textured and flatten the rest to their
    # average texel colour so nothing renders white/black.
    def base_tex(mat):
        try:
            for node in mat.node_tree.nodes:
                if node.type == 'TEX_IMAGE' and node.image is not None:
                    return node.image
        except Exception:
            pass
        return None

    def face_count(mat):
        n = 0
        for poly in body.data.polygons:
            if poly.material_index < len(body.data.materials) and \
                    body.data.materials[poly.material_index] == mat:
                n += 1
        return n

    textured = [m for m in slots if base_tex(m) is not None]

    def image_stats(img):
        try:
            w, h = img.size
            px = [0.0] * (w * h * 4)
            img.pixels.foreach_get(px)
            n = w * h
            step = max(1, n // 2048)
            vals = []
            for i in range(0, n, step):
                vals.append((px[i * 4], px[i * 4 + 1], px[i * 4 + 2]))
            if not vals:
                return (0.5, 0.5, 0.5), 1.0
            mean = tuple(sum(v[c] for v in vals) / len(vals) for c in range(3))
            var = sum((v[c] - mean[c]) ** 2 for v in vals for c in range(3))
            var = (var / (len(vals) * 3.0)) ** 0.5
            return mean, var
        except Exception:
            return (0.5, 0.5, 0.5), 1.0

    # a near-uniform texture (eye white, highlight, plain skin) is safely
    # replaced by its average colour -- that saves a slot with no visible loss
    for mat in list(textured):
        mean, var = image_stats(base_tex(mat))
        if var < 0.055:
            set_flat_material(mat, mean)
            textured.remove(mat)
            log("material %r is near-uniform (std %.3f) -> flat colour, slot freed"
                % (mat.name, var))

    if len(textured) > CAP_TEXTURES:
        # The device has four texture slots. At ~150 px the eyes carry the
        # character, so pick one texture per visual area (eyes, face, hair,
        # body/cloth) instead of four eye layers; the rest become flat colours.
        def category(mat):
            name = mat.name.lower()
            if any(k in name for k in ("iris", "pupil", "eyewhite", "eye_white",
                                        "eyeline", "highlight", "eye")):
                return "eye"
            if any(k in name for k in ("face", "mouth", "brow", "eyebrow")):
                return "face"
            if "hair" in name:
                return "hair"
            if any(k in name for k in ("cloth", "tops", "bottoms", "shoes",
                                        "skirt", "dress", "outfit", "uniform")):
                return "cloth"
            return "skin"

        def sub_rank(mat):
            name = mat.name.lower()
            if "iris" in name or "pupil" in name:
                return 3
            if "eyeline" in name or "highlight" in name:
                return 2
            return 1

        by_cat = {}
        for mat in textured:
            by_cat.setdefault(category(mat), []).append(mat)
        order = ["eye", "face", "cloth", "hair", "skin"]
        keep = []
        # round 1: best of each category in priority order
        for cat in order:
            if len(keep) >= CAP_TEXTURES:
                break
            group = by_cat.get(cat)
            if group:
                group.sort(key=lambda m: (sub_rank(m), face_count(m)), reverse=True)
                keep.append(group.pop(0))
        # round 2: fill any remaining slots with the largest leftover surfaces
        leftover = [m for group in by_cat.values() for m in group]
        leftover.sort(key=face_count, reverse=True)
        while len(keep) < CAP_TEXTURES and leftover:
            keep.append(leftover.pop(0))
        ranked = keep + leftover
        log("texture slots: keeping %s" % ", ".join(repr(m.name) for m in keep))
        for mat in ranked[CAP_TEXTURES:]:
            mean, _var = image_stats(base_tex(mat))
            set_flat_material(mat, mean)
            warn("material %r flattened to its average colour (texture slot budget)"
                 % mat.name, warns)
        warn("%d textured materials, device keeps %d -- extra ones flattened"
             % (len(textured), CAP_TEXTURES), warns)
    for mat in slots:
        mat.use_backface_culling = False
        try:
            if mat.node_tree:
                for node in mat.node_tree.nodes:
                    if node.type == 'BSDF_PRINCIPLED':
                        node.inputs["Metallic"].default_value = 0.0
                        node.inputs["Roughness"].default_value = 0.8
        except Exception:
            pass
    if body.data.shape_keys:
        for kb in body.data.shape_keys.key_blocks:
            kb.value = 0.0

    # ---- new armature + skinning --------------------------------------
    new_arm = build_canonical_armature(positions)
    mod = body.modifiers.new("Armature", 'ARMATURE')
    mod.object = new_arm
    mod.use_vertex_groups = True
    body.parent = new_arm
    body.matrix_parent_inverse = Matrix.Identity(4)

    # ---- export --------------------------------------------------------
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    flags = dict(
        filepath=out_path, export_format='GLB', use_selection=False,
        export_apply=False, export_animations=False, export_skins=True,
        export_influence_nb=4, export_all_influences=False,
        export_texcoords=True, export_normals=True, export_tangents=False,
        export_materials='EXPORT', export_image_format='AUTO', export_yup=True,
        export_def_bones=False, export_leaf_bone=False,
        export_rest_position_armature=True, export_extras=False,
        export_cameras=False, export_lights=False,
        export_optimize_animation_size=False,
        export_morph=True, export_morph_normal=False, export_morph_tangent=False,
        export_morph_animation=False, export_morph_reset_sk_data=False,
        export_try_sparse_sk=False,
    )
    bpy.ops.export_scene.gltf(**flags)
    log("exported %s (%d bytes)" % (out_path, os.path.getsize(out_path)))

    # ---- audit ---------------------------------------------------------
    rep = verify_glb.audit(out_path, expect_morphs=None)
    name_map = {}
    for s in rep.get("skins", []):
        name_map.update({n: i for i, n in enumerate(s.get("joint_names", []))})
    report.update({
        "export": {
            "triangles": rep["triangles"], "vertices": rep["vertices"],
            "joints": rep["skins"][0]["joint_count"] if rep["skins"] else 0,
            "max_influences": rep["max_influences"],
            "materials": len(rep["gltf"].get("materials", [])),
            "images": [im.get("png") for im in rep.get("images", [])],
            "morph_targets": rep["morph"]["count"],
            "morph_names": rep["morph"]["names"],
            "bone_coverage": "%d/%d" % (len(rep["humanoid_bones_found"]),
                                        len(verify_glb.HUMANOID_BONES)),
            "bytes": os.path.getsize(out_path),
        },
        "source_bones": len(src),
        "mapped_bones": len(mapping),
        "audit_ok": rep["ok"],
    })
    checks = [
        ("triangles", rep["triangles"], args.max_tris),
        ("vertices", rep["vertices"], args.max_verts),
        ("indices", rep["triangles"] * 3, CAP_INDICES),
        ("joints", report["export"]["joints"], CAP_JOINTS),
        ("nodes", len(rep["node_names"]), CAP_NODES),
        ("images", len(rep.get("images", [])), CAP_TEXTURES),
        ("morph_targets", rep["morph"]["count"], CAP_MORPH_TARGETS),
        ("morph_entries", rep["vertices"] * max(1, rep["morph"]["count"]),
         CAP_MORPH_ENTRIES),
    ]
    report["caps"] = {}
    bad = []
    for name, value, limit in checks:
        ok = value <= limit
        report["caps"][name] = {"value": value, "limit": limit, "ok": ok}
        if not ok:
            bad.append(name)
        log("cap %-13s %6d / %6d  %s" % (name, value, limit, "ok" if ok else "OVER"))
    if args.strict:
        strict_checks = [
            ("materials==1", len(rep["gltf"].get("materials", [])) == 1),
            ("uv_layers==1", len(rep["gltf"].get("meshes", [{}])[0]
                                .get("primitives", [{}])[0].get("attributes", {})) > 0),
            ("triangles<=4000", rep["triangles"] <= 4000),
            ("vertices<=1000", rep["vertices"] <= 1000),
            ("morphs==3", rep["morph"]["names"] == list(EXPRESSIONS)),
            ("weights==0", rep["morph"]["weights"] == [0, 0, 0]),
        ]
        for name, ok in strict_checks:
            report.setdefault("strict", {})[name] = ok
            if not ok:
                bad.append(name)
                warn("strict check failed: %s" % name, warns)
    if not rep["ok"]:
        bad.append("verify_glb")
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            json.dump(report, f, indent=2)
        log("report -> %s" % args.report)
    print("VRM2KIMI|SUMMARY " + json.dumps({
        "out": os.path.abspath(out_path),
        "bytes": os.path.getsize(out_path),
        "triangles": rep["triangles"], "vertices": rep["vertices"],
        "joints": report["export"]["joints"],
        "morphs": rep["morph"]["names"],
        "textures": report["export"]["images"],
        "caps_ok": not bad,
        "problems": bad,
    }, sort_keys=True))
    if args.preview:
        try:
            render_preview(args.preview, new_arm, body)
        except Exception as exc:
            warn("preview render failed: %r" % (exc,), warns)
    if args.save_blend:
        bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(args.save_blend))
        log("blend saved %s" % args.save_blend)
    return 0 if not bad else 1


if __name__ == "__main__":
    av = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    sys.exit(main(av))