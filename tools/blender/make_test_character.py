#!/usr/bin/env python3
"""make_test_character.py -- procedurally build a rigged, textured humanoid and
export it as a KimiOS device-ready GLB. Runs headless under Blender 5.x:

  "C:\\Program Files\\Blender Foundation\\Blender 5.2\\blender.exe" -b --factory-startup \
      --python-exit-code 1 --python tools/blender/make_test_character.py

Produces (defaults, relative to the repo root):
  assets/models/testchar.glb          skinned GLB: POSITION/NORMAL/TEXCOORD_0/
                                      JOINTS_0/WEIGHTS_0 + indices, skin with
                                      inverseBindMatrices, embedded PNG texture,
                                      standard humanoid bone names
  assets/models/testchar_contact.png  4-pose contact sheet rendered in Blender

Everything is generated from scratch: no external model, no add-ons, no MCP.
The mesh is a single skinned object (~500 tris) wearing a 256x256 hand-drawn
palette atlas (face on the front of the head, emblem on the chest).

Options (after `--`):
  --out PATH            output .glb (default <repo>/assets/models/testchar.glb)
  --contact PATH        output .png (default <repo>/assets/models/testchar_contact.png)
  --tex-size N          atlas size, <= 256 (default 256)
  --no-contact          skip the contact-sheet render
  --save-blend PATH     also save the generated .blend (optional)
"""
import argparse
import json
import math
import os
import random
import sys

import bpy
import bmesh
from mathutils import Matrix, Vector

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import verify_glb  # noqa: E402  (local module, pure Python)

# --------------------------------------------------------------------------- rig
# (name, parent, head, tail) in Blender space: +Z up, character faces -Y so that
# after the glTF +Y-up conversion it faces +Z (the glTF/Vulkan front convention).
BONES = [
    ("hips", None, (0.0, 0.0, 0.88), (0.0, 0.0, 1.00)),
    ("spine", "hips", (0.0, 0.0, 1.00), (0.0, 0.0, 1.14)),
    ("chest", "spine", (0.0, 0.0, 1.14), (0.0, 0.0, 1.30)),
    ("neck", "chest", (0.0, 0.0, 1.30), (0.0, 0.0, 1.40)),
    ("head", "neck", (0.0, 0.0, 1.40), (0.0, 0.0, 1.66)),
    ("leftUpperArm", "chest", (0.18, 0.0, 1.29), (0.42, 0.0, 1.29)),
    ("leftLowerArm", "leftUpperArm", (0.42, 0.0, 1.29), (0.62, 0.0, 1.29)),
    ("leftHand", "leftLowerArm", (0.62, 0.0, 1.29), (0.74, 0.0, 1.29)),
    ("rightUpperArm", "chest", (-0.18, 0.0, 1.29), (-0.42, 0.0, 1.29)),
    ("rightLowerArm", "rightUpperArm", (-0.42, 0.0, 1.29), (-0.62, 0.0, 1.29)),
    ("rightHand", "rightLowerArm", (-0.62, 0.0, 1.29), (-0.74, 0.0, 1.29)),
    ("leftUpperLeg", "hips", (0.10, 0.0, 0.88), (0.10, 0.0, 0.50)),
    ("leftLowerLeg", "leftUpperLeg", (0.10, 0.0, 0.50), (0.10, 0.0, 0.09)),
    ("leftFoot", "leftLowerLeg", (0.10, 0.0, 0.09), (0.10, -0.17, 0.02)),
    ("rightUpperLeg", "hips", (-0.10, 0.0, 0.88), (-0.10, 0.0, 0.50)),
    ("rightLowerLeg", "rightUpperLeg", (-0.10, 0.0, 0.50), (-0.10, 0.0, 0.09)),
    ("rightFoot", "rightLowerLeg", (-0.10, 0.0, 0.09), (-0.10, -0.17, 0.02)),
]
BONE_SEGS = {name: (Vector(head), Vector(tail)) for name, _p, head, tail in BONES}

# Which bones a body part is allowed to be weighted to. Distance-from-bone-segment
# inside this allow-list gives smooth joint blends without cross-body bleeding.
PART_BONES = {
    "torso": ("hips", "spine", "chest", "neck"),
    "neck": ("chest", "neck", "head"),
    "head": ("head", "neck"),
    "upperarm_l": ("chest", "leftUpperArm", "leftLowerArm"),
    "lowerarm_l": ("leftUpperArm", "leftLowerArm", "leftHand"),
    "hand_l": ("leftLowerArm", "leftHand"),
    "upperarm_r": ("chest", "rightUpperArm", "rightLowerArm"),
    "lowerarm_r": ("rightUpperArm", "rightLowerArm", "rightHand"),
    "hand_r": ("rightLowerArm", "rightHand"),
    "upperleg_l": ("hips", "leftUpperLeg", "leftLowerLeg"),
    "lowerleg_l": ("leftUpperLeg", "leftLowerLeg", "leftFoot"),
    "foot_l": ("leftLowerLeg", "leftFoot"),
    "upperleg_r": ("hips", "rightUpperLeg", "rightLowerLeg"),
    "lowerleg_r": ("rightUpperLeg", "rightLowerLeg", "rightFoot"),
    "foot_r": ("rightLowerLeg", "rightFoot"),
}

# --------------------------------------------------------------------------- texture
GRID = 8                       # 8x8 cells of 32 px in the default 256 px atlas
PATCHES = {                    # name: ((col, row), sRGB 0..255)
    "skin":   ((0, 0), (220, 176, 144)),
    "glove":  ((1, 0), (96, 66, 54)),
    "shirt":  ((2, 0), (40, 92, 184)),
    "sleeve": ((3, 0), (56, 112, 208)),
    "pants":  ((4, 0), (42, 46, 62)),
    "shoe":   ((5, 0), (20, 20, 26)),
    "hair":   ((6, 0), (38, 22, 14)),
    "face":   ((7, 0), (228, 186, 152)),
    "belt":   ((0, 1), (74, 48, 26)),
    "collar": ((1, 1), (236, 238, 244)),
}
TORSO_Z0, TORSO_Z1 = 0.95, 1.23      # shirt band used by the planar UV projection
TORSO_X = 0.20
HEAD_X, HEAD_Z0, HEAD_Z1 = 0.135, 1.42, 1.68


def _srgb_to_linear(c):
    c = c / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


class Canvas:
    """Tiny sRGB RGBA8 canvas; rows are bottom-up to match bpy image pixels."""

    def __init__(self, w, h):
        self.w, self.h = w, h
        self.buf = bytearray([0, 0, 0, 255] * (w * h))

    def set(self, x, y, rgb):
        if 0 <= x < self.w and 0 <= y < self.h:
            i = (y * self.w + x) * 4
            self.buf[i], self.buf[i + 1], self.buf[i + 2] = rgb[0], rgb[1], rgb[2]

    def get(self, x, y):
        i = (y * self.w + x) * 4
        return self.buf[i], self.buf[i + 1], self.buf[i + 2]

    def blend(self, x, y, rgb, a):
        if not (0 <= x < self.w and 0 <= y < self.h):
            return
        r, g, b = self.get(x, y)
        self.set(x, y, (int(r + (rgb[0] - r) * a), int(g + (rgb[1] - g) * a), int(b + (rgb[2] - b) * a)))

    def rect(self, x0, y0, x1, y1, rgb):
        for y in range(max(0, y0), min(self.h, y1)):
            for x in range(max(0, x0), min(self.w, x1)):
                self.set(x, y, rgb)


def _patch_origin(name):
    (col, row), _rgb = PATCHES[name]
    return col * (256 // GRID), row * (256 // GRID)


def make_atlas(size, path):
    """Draw the palette atlas + face + chest emblem and save it as PNG.

    All hand-drawn details are expressed in normalised patch coordinates, so the
    atlas works at any size that is a multiple of GRID (tested 128 and 256).
    """
    assert size % GRID == 0
    cell = size // GRID
    cv = Canvas(size, size)
    rng = random.Random(0xC0FFEE)

    def origin(name):
        (col, row), _rgb = PATCHES[name]
        return col * cell, row * cell

    for name, ((col, row), rgb) in PATCHES.items():
        x0, y0 = col * cell, row * cell
        for y in range(cell):
            for x in range(cell):
                n = rng.randint(-3, 3)
                cv.set(x0 + x, y0 + y, tuple(max(0, min(255, c + n)) for c in rgb))

    # ---- chest: subtle vertical gradient + emblem (planar UV projection) -----
    x0, y0 = origin("shirt")

    def nrect_in(rect_origin, fx0, fy0, fx1, fy1, rgb, blend=None):
        ox, oy = rect_origin
        px0, py0 = ox + int(round(fx0 * cell)), oy + int(round(fy0 * cell))
        px1, py1 = ox + int(round(fx1 * cell)), oy + int(round(fy1 * cell))
        if blend is None:
            cv.rect(px0, py0, max(px0 + 1, px1), max(py0 + 1, py1), rgb)
        else:
            for yy in range(py0, max(py0 + 1, py1)):
                for xx in range(px0, max(px0 + 1, px1)):
                    cv.blend(xx, yy, rgb, blend)

    for y in range(cell):
        shade = int(18 * (y / (cell - 1.0)))
        for x in range(cell):
            r, g, b = PATCHES["shirt"][1]
            cv.set(x0 + x, y0 + y, (min(255, r + shade), min(255, g + shade), min(255, b + shade)))
    cx, cy = x0 + cell // 2, y0 + int(round(cell * 0.56))
    R = max(2, cell // 4)
    for dy in range(-R - 1, R + 2):
        for dx in range(-R - 1, R + 2):
            d = abs(dx) + abs(dy)
            if d <= R:
                if d >= R - 1:
                    cv.blend(cx + dx, cy + dy, (250, 252, 255), 0.95)   # diamond rim
                else:
                    cv.blend(cx + dx, cy + dy, (24, 196, 232), 0.95)    # cyan core
    # a thin belt stripe near the bottom of the shirt band
    nrect_in((x0, y0), 0.0, 0.03, 1.0, 0.10, (24, 30, 60), blend=0.5)

    # ---- face ---------------------------------------------------------------
    fx0, fy0 = origin("face")
    rect = (fx0, fy0)
    hair = PATCHES["hair"][1]
    blink = (30, 24, 28)
    white = (248, 248, 250)
    # hair fringe across the top, with sideburns
    nrect_in(rect, 0.0, 0.78, 1.0, 1.0, hair)
    nrect_in(rect, 0.0, 0.66, 0.12, 0.78, hair)
    nrect_in(rect, 0.88, 0.66, 1.0, 0.78, hair)
    # eyebrows
    nrect_in(rect, 0.156, 0.594, 0.344, 0.656, hair)
    nrect_in(rect, 0.656, 0.594, 0.844, 0.656, hair)
    # eyes + one-pixel highlights
    for ex in (0.1875, 0.656):
        nrect_in(rect, ex, 0.406, ex + 0.156, 0.5625, blink)
        cv.set(fx0 + int(round((ex + 0.03) * cell)), fy0 + int(round(0.515 * cell)), white)
    # nose
    nx, ny = fx0 + cell // 2, fy0 + int(round(0.34 * cell))
    cv.blend(nx, ny, (150, 104, 82), 0.55)
    cv.blend(nx, ny - 1, (150, 104, 82), 0.35)
    # smile
    nrect_in(rect, 0.375, 0.219, 0.625, 0.25, (92, 46, 42))
    cv.set(fx0 + int(round(0.344 * cell)), fy0 + int(round(0.25 * cell)), (92, 46, 42))
    cv.set(fx0 + int(round(0.656 * cell)), fy0 + int(round(0.25 * cell)), (92, 46, 42))
    # blush
    for bx in (0.06, 0.81):
        nrect_in(rect, bx, 0.28, bx + 0.13, 0.375, (232, 118, 112), blend=0.35)

    # ---- upload to a bpy image and save as PNG ------------------------------
    img = bpy.data.images.new("testchar_basecolor", size, size, alpha=True)
    px = [0.0] * (size * size * 4)
    for i in range(size * size):
        j = i * 4
        px[j] = _srgb_to_linear(cv.buf[j])
        px[j + 1] = _srgb_to_linear(cv.buf[j + 1])
        px[j + 2] = _srgb_to_linear(cv.buf[j + 2])
        px[j + 3] = 1.0
    img.pixels.foreach_set(px)
    img.filepath_raw = path
    img.file_format = 'PNG'
    img.save()
    img.pack()
    print("TESTCHAR|atlas %dx%d -> %s (%d bytes)" % (size, size, path, os.path.getsize(path)))
    return img


# --------------------------------------------------------------------------- mesh
class Builder:
    def __init__(self):
        self.verts, self.vparts = [], []
        self.faces, self.fpatches, self.fuvmodes, self.fsmooth = [], [], [], []

    def vert(self, co, part):
        self.verts.append(Vector(co))
        self.vparts.append(part)
        return len(self.verts) - 1

    def face(self, idxs, patch, uv, smooth, part):
        self.faces.append(tuple(idxs))
        self.fpatches.append(patch)
        self.fuvmodes.append(uv)
        self.fsmooth.append(smooth)


def add_box(b, center, size, part, patches, uv="flat", smooth=False):
    """Axis-aligned box. patches maps face name -> atlas patch."""
    cx, cy, cz = center
    sx, sy, sz = size
    x0, x1, y0, y1, z0, z1 = cx - sx / 2, cx + sx / 2, cy - sy / 2, cy + sy / 2, cz - sz / 2, cz + sz / 2
    v = [b.vert((x0, y0, z0), part), b.vert((x1, y0, z0), part),
         b.vert((x1, y1, z0), part), b.vert((x0, y1, z0), part),
         b.vert((x0, y0, z1), part), b.vert((x1, y0, z1), part),
         b.vert((x1, y1, z1), part), b.vert((x0, y1, z1), part)]
    spec = [((0, 1, 2, 3), "bottom"), ((4, 5, 6, 7), "top"),
            ((0, 1, 5, 4), "front"), ((3, 2, 6, 7), "back"),
            ((0, 3, 7, 4), "left"), ((1, 2, 6, 5), "right")]
    for idxs, key in spec:
        b.face([v[i] for i in idxs], patches.get(key, patches["all"]), uv, smooth, part)


def add_tube(b, p0, p1, r0, r1, part, patch, sides=10, smooth=True, cap0=True, cap1=True):
    p0, p1 = Vector(p0), Vector(p1)
    axis = (p1 - p0).normalized()
    up = Vector((0, 0, 1)) if abs(axis.z) < 0.95 else Vector((1, 0, 0))
    u = axis.cross(up).normalized()
    v = axis.cross(u).normalized()
    ring0, ring1 = [], []
    for i in range(sides):
        a = 2.0 * math.pi * i / sides
        d = math.cos(a) * u + math.sin(a) * v
        ring0.append(b.vert(p0 + d * r0, part))
        ring1.append(b.vert(p1 + d * r1, part))
    for i in range(sides):
        j = (i + 1) % sides
        b.face((ring0[i], ring0[j], ring1[j], ring1[i]), patch, "flat", smooth, part)
    if cap0:
        b.face(list(reversed(ring0)), patch, "flat", False, part)
    if cap1:
        b.face(ring1, patch, "flat", False, part)


def add_loft(b, rings, part, sides, patch_fn, smooth=True):
    """Lofted ellipse rings (z, rx, ry). patch_fn(mid_z) picks the atlas patch."""
    ring_ids = []
    for (z, rx, ry) in rings:
        ids = []
        for i in range(sides):
            a = 2.0 * math.pi * i / sides
            ids.append(b.vert((rx * math.cos(a), ry * math.sin(a), z), part))
        ring_ids.append(ids)
    for k in range(len(ring_ids) - 1):
        zmid = 0.5 * (rings[k][0] + rings[k + 1][0])
        patch = patch_fn(zmid)
        uv = "planar" if patch == "shirt" else "flat"
        for i in range(sides):
            j = (i + 1) % sides
            b.face((ring_ids[k][i], ring_ids[k][j], ring_ids[k + 1][j], ring_ids[k + 1][i]),
                   patch, uv, smooth, part)
    b.face(list(reversed(ring_ids[0])), patch_fn(rings[0][0]), "flat", False, part)
    b.face(ring_ids[-1], patch_fn(rings[-1][0]), "flat", False, part)


def _torso_patch(z):
    if z < TORSO_Z0:
        return "belt"
    if z < TORSO_Z1:
        return "shirt"
    return "collar"


def build_geometry():
    b = Builder()
    # torso: wide hips (covers the thigh sockets), narrow waist, broad chest
    add_loft(b, [(0.86, 0.190, 0.122), (0.95, 0.188, 0.120), (1.03, 0.168, 0.112),
                 (1.13, 0.170, 0.116), (1.23, 0.186, 0.123), (1.30, 0.196, 0.126)],
             "torso", 12, _torso_patch, smooth=True)
    add_tube(b, (0, 0, 1.29), (0, 0, 1.43), 0.056, 0.060, "neck", "skin", sides=10)
    # head: face on -Y, hair elsewhere, skin underneath
    add_box(b, (0.0, -0.005, 1.55), (0.27, 0.26, 0.26), "head",
            {"all": "hair", "front": "face", "bottom": "skin"}, uv="face", smooth=False)
    for side, sx in (("l", 1.0), ("r", -1.0)):
        # start the sleeve inside the torso so the shoulder stays covered when posed
        add_tube(b, (sx * 0.15, 0, 1.29), (sx * 0.42, 0, 1.29), 0.062, 0.052,
                 "upperarm_%s" % side, "sleeve", sides=10)
        add_tube(b, (sx * 0.42, 0, 1.29), (sx * 0.62, 0, 1.29), 0.052, 0.044,
                 "lowerarm_%s" % side, "skin", sides=10)
        add_box(b, (sx * 0.68, 0, 1.29), (0.13, 0.076, 0.096), "hand_%s" % side,
                {"all": "glove"}, smooth=False)
        # thigh starts inside the pelvis so the hip socket stays covered when posed
        add_tube(b, (sx * 0.10, 0, 0.92), (sx * 0.10, 0, 0.50), 0.086, 0.068,
                 "upperleg_%s" % side, "pants", sides=10)
        add_tube(b, (sx * 0.10, 0, 0.50), (sx * 0.10, 0, 0.10), 0.064, 0.048,
                 "lowerleg_%s" % side, "skin", sides=10)
        add_box(b, (sx * 0.10, -0.055, 0.045), (0.115, 0.26, 0.09), "foot_%s" % side,
                {"all": "shoe"}, smooth=False)
    return b


def _seg_dist(p, a, b):
    ab = b - a
    l2 = ab.length_squared
    if l2 < 1e-12:
        return (p - a).length
    t = max(0.0, min(1.0, (p - a).dot(ab) / l2))
    return (p - (a + ab * t)).length


def bone_weights(co, allowed, power=3.0):
    scored = [(1.0 / max(_seg_dist(co, *BONE_SEGS[name]), 1e-4) ** power, name) for name in allowed]
    scored.sort(reverse=True)
    scored = scored[:4]
    total = sum(w for w, _ in scored)
    out = [(name, w / total) for w, name in scored]
    out = [(n, w) for n, w in out if w >= 2e-3]
    total = sum(w for _, w in out)
    return [(n, w / total) for n, w in out]


def uv_for(patch, mode, co):
    cell = 1.0 / GRID
    (col, row), _rgb = PATCHES[patch]
    x0, y0 = col * cell, row * cell
    inset = 0.02 * cell
    if mode == "face":
        u = min(1.0, max(0.0, (co.x + HEAD_X) / (2 * HEAD_X)))
        v = min(1.0, max(0.0, (co.z - HEAD_Z0) / (HEAD_Z1 - HEAD_Z0)))
    elif mode == "planar":
        u = min(1.0, max(0.0, (co.x + TORSO_X) / (2 * TORSO_X)))
        v = min(1.0, max(0.0, (co.z - TORSO_Z0) / (TORSO_Z1 - TORSO_Z0)))
    else:
        return (x0 + cell * 0.5, y0 + cell * 0.5)
    return (x0 + inset + u * (cell - 2 * inset), y0 + inset + v * (cell - 2 * inset))


# --------------------------------------------------------------------------- build
def reset_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    for coll in (bpy.data.meshes, bpy.data.materials, bpy.data.images,
                 bpy.data.armatures, bpy.data.objects):
        for item in list(coll):
            try:
                coll.remove(item)
            except Exception:
                pass


def build_armature():
    arm = bpy.data.armatures.new("TestCharArmature")
    aob = bpy.data.objects.new("TestCharRig", arm)
    bpy.context.scene.collection.objects.link(aob)
    bpy.context.view_layer.objects.active = aob
    aob.select_set(True)
    bpy.ops.object.mode_set(mode='EDIT')
    ebs = {}
    for name, _parent, head, tail in BONES:
        eb = arm.edit_bones.new(name)
        eb.head, eb.tail = head, tail
        eb.use_deform = True
        ebs[name] = eb
    for name, parent, _head, _tail in BONES:
        if parent:
            ebs[name].parent = ebs[parent]
            ebs[name].use_connect = (Vector(ebs[name].head) - Vector(ebs[parent].tail)).length < 1e-6
    bpy.ops.object.mode_set(mode='OBJECT')
    aob.show_in_front = True
    print("TESTCHAR|armature: %d bones, hierarchy: %s"
          % (len(arm.bones), ", ".join("%s<%s" % (b.name, b.parent.name if b.parent else "-") for b in arm.bones)))
    return aob


def build_character(atlas_image):
    builder = build_geometry()
    print("TESTCHAR|geometry: %d verts, %d faces" % (len(builder.verts), len(builder.faces)))

    bm = bmesh.new()
    bmverts = [bm.verts.new(co) for co in builder.verts]
    bm.verts.ensure_lookup_table()
    for idxs in builder.faces:
        bm.faces.new([bmverts[i] for i in idxs])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    me = bpy.data.meshes.new("TestCharBody")
    bm.to_mesh(me)
    bm.free()
    me.validate()
    assert len(me.polygons) == len(builder.faces), "face order/count changed"

    # UVs computed from world position after normals are final (winding-proof)
    uvl = me.uv_layers.new(name="UVMap")
    for poly in me.polygons:
        patch = builder.fpatches[poly.index]
        mode = builder.fuvmodes[poly.index]
        for li in poly.loop_indices:
            co = me.vertices[me.loops[li].vertex_index].co
            uvl.data[li].uv = uv_for(patch, mode, co)
        poly.use_smooth = builder.fsmooth[poly.index]
    me.update()

    mob = bpy.data.objects.new("TestCharBody", me)
    bpy.context.scene.collection.objects.link(mob)

    mat = bpy.data.materials.new("TestCharMaterial")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes["Principled BSDF"]
    bsdf.inputs["Roughness"].default_value = 0.75
    bsdf.inputs["Metallic"].default_value = 0.0
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = atlas_image
    tex.location = (-320, 260)
    nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    me.materials.append(mat)

    # ---- skinning -----------------------------------------------------------
    aob = build_armature()
    groups = {}
    for name in BONE_SEGS:
        groups[name] = mob.vertex_groups.new(name=name)
    hist = {}
    for i, co in enumerate(builder.verts):
        ws = bone_weights(co, PART_BONES[builder.vparts[i]])
        hist[len(ws)] = hist.get(len(ws), 0) + 1
        for name, w in ws:
            groups[name].add([i], w, 'REPLACE')
    print("TESTCHAR|skin weights: influences/vertex histogram=%s (max 4)" % (dict(sorted(hist.items())),))

    mod = mob.modifiers.new("Armature", 'ARMATURE')
    mod.object = aob
    mod.use_vertex_groups = True
    mob.parent = aob
    mob.matrix_parent_inverse = Matrix.Identity(4)

    me.calc_loop_triangles()
    print("TESTCHAR|mesh: %d tris (budget 4000), %d vertices, 1 material, 1 uv layer"
          % (len(me.loop_triangles), len(me.vertices)))
    return aob, mob


def export_glb(path):
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format='GLB',
        use_selection=False,
        export_apply=False,
        export_animations=False,
        export_skins=True,
        export_influence_nb=4,
        export_all_influences=False,
        export_texcoords=True,
        export_normals=True,
        export_tangents=False,
        export_materials='EXPORT',
        export_image_format='AUTO',      # Blender 5.x: PNG has no explicit enum, AUTO keeps PNG
        export_yup=True,
        export_def_bones=False,
        export_leaf_bone=False,
        export_rest_position_armature=True,
        export_extras=False,
        export_cameras=False,
        export_lights=False,
        export_optimize_animation_size=False,
    )
    print("TESTCHAR|exported %s (%d bytes)" % (path, os.path.getsize(path)))


# --------------------------------------------------------------------------- poses
def rotate_world(pb, rx=0.0, ry=0.0, rz=0.0):
    """Rotate a pose bone by a world-axis XYZ rotation expressed in bone space."""
    R = Matrix.Rotation(rz, 3, 'Z') @ Matrix.Rotation(ry, 3, 'Y') @ Matrix.Rotation(rx, 3, 'X')
    M = pb.bone.matrix_local.to_3x3()
    pb.rotation_mode = 'QUATERNION'
    pb.rotation_quaternion = (M.inverted() @ R @ M).to_quaternion()


POSES = [
    ("T-POSE", {}),
    ("WAVE", {
        "leftUpperArm": (0.0, -1.35, 0.10), "leftLowerArm": (0.0, 0.75, 0.25),
        "leftHand": (0.0, 0.25, 0.0),
        "rightUpperArm": (0.0, -0.45, 0.0), "rightLowerArm": (0.0, 0.20, 0.0),
        "head": (0.0, 0.16, 0.20), "spine": (0.0, 0.0, -0.08),
    }),
    ("WALK", {
        "leftUpperLeg": (-0.62, 0.0, 0.0), "leftLowerLeg": (0.36, 0.0, 0.0),
        "rightUpperLeg": (0.46, 0.0, 0.0), "rightLowerLeg": (0.60, 0.0, 0.0),
        "leftUpperArm": (0.0, 0.28, -0.55), "rightUpperArm": (0.0, -0.28, -0.55),
        "leftLowerArm": (0.0, 0.0, -0.30), "rightLowerArm": (0.0, 0.0, -0.30),
        "spine": (0.0, 0.0, 0.14), "head": (0.0, 0.0, -0.18),
    }),
    ("KICK", {
        # side kick: the raised leg swings outward (+X) so it reads in a front view
        "leftUpperLeg": (-0.32, -1.05, 0.0), "leftLowerLeg": (0.36, 0.0, 0.0),
        "rightUpperLeg": (0.10, 0.06, 0.0), "rightLowerLeg": (0.05, 0.0, 0.0),
        "leftUpperArm": (0.0, 0.18, 0.50), "rightUpperArm": (0.0, -0.18, 0.50),
        "rightLowerArm": (0.0, 0.0, 0.45), "leftLowerArm": (0.0, 0.0, 0.20),
        "head": (0.0, 0.0, 0.14), "spine": (0.10, 0.0, -0.12),
    }),
]


def copy_rig(arm_ob, mesh_ob, dx):
    new_arm = arm_ob.copy()
    new_arm.data = arm_ob.data
    new_arm.animation_data_clear()
    new_arm.name = "RigCopy"
    bpy.context.scene.collection.objects.link(new_arm)
    new_mesh = mesh_ob.copy()
    new_mesh.data = mesh_ob.data
    new_mesh.name = "BodyCopy"
    bpy.context.scene.collection.objects.link(new_mesh)
    new_mesh.parent = new_arm
    new_mesh.matrix_parent_inverse = mesh_ob.matrix_parent_inverse.copy()
    for m in new_mesh.modifiers:
        if m.type == 'ARMATURE':
            m.object = new_arm
    new_arm.location = (dx, 0.0, 0.0)
    return new_arm, new_mesh


def simple_material(name, rgb, roughness=0.9):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (rgb[0], rgb[1], rgb[2], 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    mat.diffuse_color = (rgb[0], rgb[1], rgb[2], 1.0)
    return mat


def add_plane(name, sx, sy, location, rotation, material):
    me = bpy.data.meshes.new(name)
    hx, hy = sx / 2.0, sy / 2.0
    me.from_pydata([(-hx, -hy, 0), (hx, -hy, 0), (hx, hy, 0), (-hx, hy, 0)], [], [(0, 1, 2, 3)])
    me.materials.append(material)
    ob = bpy.data.objects.new(name, me)
    ob.location = location
    ob.rotation_euler = rotation
    bpy.context.scene.collection.objects.link(ob)
    return ob


def add_label(text, x, z, material):
    cu = bpy.data.curves.new("label_%s" % text, type='FONT')
    cu.body = text
    cu.align_x = 'CENTER'
    cu.size = 0.15
    cu.extrude = 0.004
    ob = bpy.data.objects.new("label_%s" % text, cu)
    ob.location = (x, 0.2, z)
    ob.rotation_euler = (math.radians(90), 0, 0)
    ob.data.materials.append(material)
    bpy.context.scene.collection.objects.link(ob)
    return ob


def render_contact_sheet(arm_ob, mesh_ob, out_path):
    scene = bpy.context.scene
    spacing = 1.75
    xs = [-1.5 * spacing, -0.5 * spacing, 0.5 * spacing, 1.5 * spacing]

    rigid = []
    for i, (label, pose) in enumerate(POSES):
        if i == 0:
            a, m = arm_ob, mesh_ob
        else:
            a, m = copy_rig(arm_ob, mesh_ob, 0.0)
        a.location = (xs[i], 0.0, 0.0)
        for bone, (rx, ry, rz) in pose.items():
            if bone in a.pose.bones:
                rotate_world(a.pose.bones[bone], rx, ry, rz)
            else:
                print("TESTCHAR|WARN pose bone missing: %s" % bone)
        rigid.append((a, m))

    bpy.context.view_layer.update()

    # studio
    floor = add_plane("Floor", 40, 40, (0, 0, 0.0), (0, 0, 0), simple_material("FloorMat", (0.35, 0.36, 0.40), 0.95))
    backdrop = add_plane("Backdrop", 40, 20, (0, 3.0, 0.0), (math.radians(90), 0, 0),
                         simple_material("BackdropMat", (0.62, 0.65, 0.72), 1.0))
    del floor, backdrop

    for i, (label, _pose) in enumerate(POSES):
        add_label(label, xs[i], 1.86, simple_material("LabelMat_%d" % i, (0.06, 0.07, 0.09), 1.0))

    # three suns: key / fill / rim (orientation-only, scale independent)
    def add_sun(name, rot, energy):
        li = bpy.data.lights.new(name, type='SUN')
        li.energy = energy
        li.angle = math.radians(12)
        ob = bpy.data.objects.new(name, li)
        ob.rotation_euler = rot
        scene.collection.objects.link(ob)

    add_sun("Key", (math.radians(58), 0, math.radians(-32)), 3.2)
    add_sun("Fill", (math.radians(74), 0, math.radians(138)), 1.1)
    add_sun("Rim", (math.radians(-52), 0, math.radians(6)), 2.2)

    world = bpy.data.worlds.new("Studio")
    world.use_nodes = True
    bg = world.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = (0.45, 0.48, 0.55, 1.0)
    bg.inputs["Strength"].default_value = 1.0
    scene.world = world

    cam_data = bpy.data.cameras.new("ContactCam")
    cam_data.type = 'ORTHO'
    cam_data.ortho_scale = 7.6
    cam = bpy.data.objects.new("ContactCam", cam_data)
    cam.location = (0.0, -8.0, 1.05)
    cam.rotation_euler = (math.radians(90), 0, 0)
    scene.collection.objects.link(cam)
    scene.camera = cam

    scene.render.engine = 'BLENDER_EEVEE'
    try:
        scene.eevee.taa_render_samples = 48
    except Exception:
        pass
    try:
        scene.view_settings.view_transform = 'Standard'
        scene.view_settings.look = 'None'
    except Exception:
        pass
    scene.render.resolution_x, scene.render.resolution_y = 1920, 600
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = False
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGB'
    scene.render.image_settings.color_depth = '8'
    scene.render.image_settings.compression = 20
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    scene.render.filepath = os.path.abspath(out_path)
    print("TESTCHAR|rendering contact sheet %dx%d -> %s"
          % (scene.render.resolution_x, scene.render.resolution_y, out_path))
    bpy.ops.render.render(write_still=True)
    size = os.path.getsize(out_path)
    print("TESTCHAR|contact sheet written: %s (%d bytes)" % (out_path, size))
    return size


# --------------------------------------------------------------------------- main
def main(argv):
    ap = argparse.ArgumentParser(prog="make_test_character")
    ap.add_argument("--out", default=os.path.join(REPO, "assets", "models", "testchar.glb"))
    ap.add_argument("--contact", default=os.path.join(REPO, "assets", "models", "testchar_contact.png"))
    ap.add_argument("--tex-size", type=int, default=256)
    ap.add_argument("--no-contact", action="store_true")
    ap.add_argument("--save-blend", default=None)
    args = ap.parse_args(argv)
    if args.tex_size > 256 or args.tex_size % GRID:
        raise SystemExit("--tex-size must be a multiple of 8 and <= 256")

    reset_scene()
    work = os.path.join(HERE, "_work")
    os.makedirs(work, exist_ok=True)
    atlas = make_atlas(args.tex_size, os.path.join(work, "testchar_basecolor.png"))
    arm_ob, mesh_ob = build_character(atlas)
    export_glb(args.out)

    rep = verify_glb.audit(args.out)           # prints the container/accessor audit
    if not args.no_contact:
        render_contact_sheet(arm_ob, mesh_ob, args.contact)
    if args.save_blend:
        bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(args.save_blend))
        print("TESTCHAR|blend saved %s" % args.save_blend)

    summary = {
        "glb": os.path.abspath(args.out),
        "glb_bytes": os.path.getsize(args.out),
        "triangles": rep["triangles"],
        "vertices": rep["vertices"],
        "texture": rep["images"][0]["png"] if rep["images"] else None,
        "humanoid_bones": "%d/%d" % (len(rep["humanoid_bones_found"]), len(verify_glb.HUMANOID_BONES)),
        "max_influences": rep["max_influences"],
        "joints": rep["skins"][0]["joint_count"] if rep["skins"] else 0,
        "contact": None if args.no_contact else os.path.abspath(args.contact),
        "audit_ok": rep["ok"],
    }
    print("TESTCHAR|SUMMARY " + json.dumps(summary, sort_keys=True))
    if not rep["ok"]:
        return 1
    return 0


if __name__ == "__main__":
    av = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    sys.exit(main(av))