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
The mesh is a single skinned object (~540 tris) wearing a 256x256 hand-drawn
palette atlas (face on the front of the head, emblem on the chest). Eyes and
mouth are real (small) geometry so the three facial shape keys -- `blink`,
`happy`, `surprised` -- deform them visibly; the glTF exporter writes them as
dense POSITION morph targets plus `meshes[].extras.targetNames`.

Options (after `--`):
  --out PATH            output .glb (default <repo>/assets/models/testchar.glb)
  --contact PATH        output .png (default <repo>/assets/models/testchar_contact.png)
  --expressions PATH    optional 4-panel shape-key proof render
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
    "eye":    ((2, 1), (250, 250, 252)),
    "mouth":  ((3, 1), (118, 50, 46)),
}
TORSO_Z0, TORSO_Z1 = 0.95, 1.23      # shirt band used by the planar UV projection
TORSO_X = 0.20
HEAD_X, HEAD_Z0, HEAD_Z1 = 0.135, 1.42, 1.68

# Facial features are separate quads a few mm in front of the head box, so the
# shape keys have real geometry to move (a painted-on face cannot blink).
FACE_Y = -0.1375
EYE_X, EYE_Z, EYE_HW, EYE_HH = 0.058, 1.552, 0.021, 0.018
MOUTH_XS = (-0.048, -0.016, 0.016, 0.048)
MOUTH_Z0, MOUTH_Z1 = 1.466, 1.482
EXPRESSIONS = ("blink", "happy", "surprised")


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
    # hair fringe across the top, with sideburns
    nrect_in(rect, 0.0, 0.78, 1.0, 1.0, hair)
    nrect_in(rect, 0.0, 0.66, 0.12, 0.78, hair)
    nrect_in(rect, 0.88, 0.66, 1.0, 0.78, hair)
    # eyebrows (the eyes themselves are geometry, see build_geometry)
    nrect_in(rect, 0.156, 0.594, 0.344, 0.656, hair)
    nrect_in(rect, 0.656, 0.594, 0.844, 0.656, hair)
    # nose
    nx, ny = fx0 + cell // 2, fy0 + int(round(0.34 * cell))
    cv.blend(nx, ny, (150, 104, 82), 0.55)
    cv.blend(nx, ny - 1, (150, 104, 82), 0.35)
    # blush
    for bx in (0.06, 0.81):
        nrect_in(rect, bx, 0.28, bx + 0.13, 0.375, (232, 118, 112), blend=0.35)

    # ---- eye patch, mapped onto each eye quad --------------------------------
    ex0, ey0 = origin("eye")
    cv.rect(ex0, ey0, ex0 + cell, ey0 + cell, (250, 250, 252))
    c2 = cell / 2.0
    iris_r, pupil_r = cell * 0.36, cell * 0.17
    for y in range(cell):
        for x in range(cell):
            d = math.hypot(x - c2 + 0.5, y - c2 + 0.5)
            if d <= pupil_r:
                cv.set(ex0 + x, ey0 + y, (24, 19, 23))
            elif d <= iris_r:
                t = y / max(1, cell - 1)
                cv.set(ex0 + x, ey0 + y, (int(64 + 46 * t), int(42 + 34 * t), int(30 + 26 * t)))
    hr = max(1, int(cell * 0.10))
    cv.rect(ex0 + int(cell * 0.32), ey0 + int(cell * 0.56),
            ex0 + int(cell * 0.32) + hr, ey0 + int(cell * 0.56) + hr, (255, 255, 255))
    cv.rect(ex0, ey0 + cell - max(1, cell // 16), ex0 + cell, ey0 + cell, (74, 50, 40))

    # ---- mouth patch, mapped onto the mouth grid ----------------------------
    mx0, my0 = origin("mouth")
    cv.rect(mx0, my0, mx0 + cell, my0 + cell, (118, 50, 46))
    cv.rect(mx0 + int(cell * 0.06), my0 + int(cell * 0.60), mx0 + int(cell * 0.94),
            my0 + int(cell * 0.78), (246, 246, 242))          # teeth (show when open)
    cv.rect(mx0, my0 + int(cell * 0.82), mx0 + cell, my0 + cell, (66, 24, 24))
    cv.rect(mx0 + int(cell * 0.16), my0, mx0 + int(cell * 0.84),
            my0 + int(cell * 0.20), (158, 72, 66))            # lower lip

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

    # ---- facial features: real quads so shape keys have something to move ----
    feats = {"eye_l": [], "eye_r": [], "mouth": []}
    feat_faces = []
    for side, cx in (("l", EYE_X), ("r", -EYE_X)):
        x0, x1 = cx - EYE_HW, cx + EYE_HW
        z0, z1 = EYE_Z - EYE_HH, EYE_Z + EYE_HH
        ids = [b.vert((x0, FACE_Y, z0), "head"), b.vert((x1, FACE_Y, z0), "head"),
               b.vert((x1, FACE_Y, z1), "head"), b.vert((x0, FACE_Y, z1), "head")]
        b.face(ids, "eye", ("rect", x0, x1, z0, z1), False, "head")
        feats["eye_" + side] = ids
        feat_faces.append(len(b.faces) - 1)
    grid = {}
    for ci, x in enumerate(MOUTH_XS):
        for ri, z in enumerate((MOUTH_Z0, MOUTH_Z1)):
            grid[(ci, ri)] = b.vert((x, FACE_Y, z), "head")
            feats["mouth"].append(grid[(ci, ri)])
    for ci in range(len(MOUTH_XS) - 1):
        ids = (grid[(ci, 0)], grid[(ci + 1, 0)], grid[(ci + 1, 1)], grid[(ci, 1)])
        b.face(ids, "mouth", ("rect", MOUTH_XS[0], MOUTH_XS[-1], MOUTH_Z0, MOUTH_Z1), False, "head")
        feat_faces.append(len(b.faces) - 1)

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
    return b, feats, feat_faces


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
    px0, py0 = col * cell, row * cell
    inset = 0.02 * cell
    if mode == "face":
        u = min(1.0, max(0.0, (co.x + HEAD_X) / (2 * HEAD_X)))
        v = min(1.0, max(0.0, (co.z - HEAD_Z0) / (HEAD_Z1 - HEAD_Z0)))
    elif mode == "planar":
        u = min(1.0, max(0.0, (co.x + TORSO_X) / (2 * TORSO_X)))
        v = min(1.0, max(0.0, (co.z - TORSO_Z0) / (TORSO_Z1 - TORSO_Z0)))
    elif isinstance(mode, tuple) and mode and mode[0] == "rect":
        _tag, rx0, rx1, rz0, rz1 = mode
        u = min(1.0, max(0.0, (co.x - rx0) / (rx1 - rx0))) if rx1 > rx0 else 0.5
        v = min(1.0, max(0.0, (co.z - rz0) / (rz1 - rz0))) if rz1 > rz0 else 0.5
    else:
        return (px0 + cell * 0.5, py0 + cell * 0.5)
    return (px0 + inset + u * (cell - 2 * inset), py0 + inset + v * (cell - 2 * inset))


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


def add_expression_shape_keys(mob, feats):
    """Create Basis + blink/happy/surprised shape keys on the face geometry.

    The keys are POSITION deltas only (the exporter is told not to emit NORMAL
    targets), and the names/order are exactly what the runtime looks up in
    glTF `meshes[].extras.targetNames`.
    """
    me = mob.data
    while me.shape_keys is not None:            # idempotent if called twice
        mob.shape_key_clear()
        if me.shape_keys is None:
            break
    mob.shape_key_add(name="Basis", from_mix=False)
    keys = {name: mob.shape_key_add(name=name, from_mix=False) for name in EXPRESSIONS}
    # Blender 5.2 creates new shape keys with value=1.0 (older releases used 0.0).
    # They must rest at 0, otherwise the exported glTF mesh.weights is [1,1,1]
    # and the runtime would apply every expression at once.
    for kb in keys.values():
        kb.value = 0.0
    base = [v.co.copy() for v in me.vertices]

    def move(key, idx, dx=0.0, dy=0.0, dz=0.0):
        c = base[idx]
        key.data[idx].co = (c.x + dx, c.y + dy, c.z + dz)

    # blink: squash each eye quad flat onto its lower edge -> eyes visibly close
    for name in ("eye_l", "eye_r"):
        zmin = min(base[i].z for i in feats[name])
        for i in feats[name]:
            if base[i].z > zmin + 1e-6:
                keys["blink"].data[i].co.z = zmin + 0.0012

    # happy: mouth corners lift into a smile + a slight eye squint
    half = MOUTH_XS[-1]
    for i in feats["mouth"]:
        t = (abs(base[i].x) / half) ** 1.7
        move(keys["happy"], i, dx=base[i].x * 0.06, dz=0.014 * t)
    for name in ("eye_l", "eye_r"):
        zs = [base[i].z for i in feats[name]]
        zmid = 0.5 * (min(zs) + max(zs))
        for i in feats[name]:
            if base[i].z > zmid:
                move(keys["happy"], i, dz=-0.0045)

    # surprised: mouth opens (top up, bottom down, narrower) + eyes go wide
    zs = [base[i].z for i in feats["mouth"]]
    zmid = 0.5 * (min(zs) + max(zs))
    for i in feats["mouth"]:
        if base[i].z > zmid:
            move(keys["surprised"], i, dx=-base[i].x * 0.08, dz=0.013)
        else:
            move(keys["surprised"], i, dx=-base[i].x * 0.08, dz=-0.006)
    for name in ("eye_l", "eye_r"):
        cx = sum(base[i].x for i in feats[name]) / len(feats[name])
        cz = sum(base[i].z for i in feats[name]) / len(feats[name])
        for i in feats[name]:
            keys["surprised"].data[i].co.x = cx + (base[i].x - cx) * 1.22
            keys["surprised"].data[i].co.z = cz + (base[i].z - cz) * 1.16

    stats = []
    for name in EXPRESSIONS:
        kb = keys[name]
        moved = 0
        biggest = 0.0
        for i in range(len(me.vertices)):
            d = (kb.data[i].co - base[i]).length
            if d > 1e-6:
                moved += 1
                biggest = max(biggest, d)
        if moved == 0:
            raise RuntimeError("shape key %r has no deltas" % name)
        stats.append("%s(%d verts, max %.1f mm)" % (name, moved, biggest * 1000.0))
    print("TESTCHAR|shape keys: %d morph targets -> %s" % (len(EXPRESSIONS), ", ".join(stats)))
    return keys


def build_character(atlas_image):
    builder, feats, feat_faces = build_geometry()
    print("TESTCHAR|geometry: %d verts, %d faces" % (len(builder.verts), len(builder.faces)))

    bm = bmesh.new()
    bmverts = [bm.verts.new(co) for co in builder.verts]
    bm.verts.ensure_lookup_table()
    for idxs in builder.faces:
        bm.faces.new([bmverts[i] for i in idxs])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    bm.faces.ensure_lookup_table()
    bm.normal_update()
    # loose face quads have no volume, so recalc cannot orient them; point the
    # eye/mouth quads outward (-Y, the front of the head) for correct lighting
    flipped = 0
    for fidx in feat_faces:
        if bm.faces[fidx].normal.y > 0.0:
            bm.faces[fidx].normal_flip()
            flipped += 1
    if flipped:
        print("TESTCHAR|flipped %d face-feature quads to +front" % flipped)
    me = bpy.data.meshes.new("TestCharBody")
    bm.to_mesh(me)
    bm.free()
    me.validate()
    assert len(me.polygons) == len(builder.faces), "face order/count changed"
    assert len(me.vertices) == len(builder.verts), "vertex order/count changed"

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

    add_expression_shape_keys(mob, feats)

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
        # ---- facial morph targets -------------------------------------------
        # Blender 5.2 defaults are export_morph_normal=True (adds NORMAL
        # targets) and export_try_sparse_sk=True (emits sparse accessors when
        # few vertices move -- which a simple loader cannot decode), so both
        # are pinned off here: dense POSITION-only targets.
        export_morph=True,
        export_morph_normal=False,
        export_morph_tangent=False,
        export_morph_animation=False,
        export_morph_reset_sk_data=False,
        export_try_sparse_sk=False,
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


def add_label(text, x, z, material, size=0.15):
    cu = bpy.data.curves.new("label_%s" % text, type='FONT')
    cu.body = text
    cu.align_x = 'CENTER'
    cu.size = size
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
def render_expression_proof(arm_ob, mesh_ob, out_path, res=(1600, 500)):
    """Optional 4-panel portrait: NEUTRAL + the three shape keys at value 1.0.

    Each panel needs its own mesh datablock (shape keys live on mesh data), so
    the mesh is copied per panel. Everything created here is removed afterwards
    so the contact-sheet render starts from a clean studio.
    """
    scene = bpy.context.scene
    spacing = 0.425
    xs = [(-1.5 + i) * spacing for i in range(4)]
    panels = [("NEUTRAL", None)] + [(name.upper(), name) for name in EXPRESSIONS]
    created = []

    for i, (label, key) in enumerate(panels):
        a = arm_ob.copy()
        a.data = arm_ob.data
        a.animation_data_clear()
        a.name = "ExprRig_%d" % i
        scene.collection.objects.link(a)
        created.append(a)
        a.location = (xs[i], 0.0, 0.0)
        m = mesh_ob.copy()
        m.data = mesh_ob.data.copy()
        m.name = "ExprBody_%d" % i
        m.hide_render = False
        scene.collection.objects.link(m)
        created.append(m)
        m.parent = a
        m.matrix_parent_inverse = Matrix.Identity(4)
        for mod in m.modifiers:
            if mod.type == 'ARMATURE':
                mod.object = a
        if key:
            m.data.shape_keys.key_blocks[key].value = 1.0
        created.append(add_label(label, xs[i], 1.76,
                                 simple_material("ExprLabelMat_%d" % i, (0.06, 0.07, 0.09), 1.0),
                                 size=0.075))

    mesh_ob.hide_render = True
    bpy.context.view_layer.update()

    created.append(add_plane("ExprBackdrop", 20, 12, (0, 2.6, 0.0),
                             (math.radians(90), 0, 0),
                             simple_material("ExprBackdropMat", (0.62, 0.65, 0.72), 1.0)))
    for name, rot, energy in (("ExprKey", (math.radians(58), 0, math.radians(-32)), 3.4),
                              ("ExprFill", (math.radians(74), 0, math.radians(138)), 1.2),
                              ("ExprRim", (math.radians(-52), 0, math.radians(6)), 2.0)):
        li = bpy.data.lights.new(name, type='SUN')
        li.energy = energy
        li.angle = math.radians(12)
        ob = bpy.data.objects.new(name, li)
        ob.rotation_euler = rot
        scene.collection.objects.link(ob)
        created.append(ob)

    world = bpy.data.worlds.new("ExprWorld")
    world.use_nodes = True
    bg = world.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = (0.45, 0.48, 0.55, 1.0)
    bg.inputs["Strength"].default_value = 1.0
    scene.world = world

    cam_data = bpy.data.cameras.new("ExprCam")
    cam_data.type = 'ORTHO'
    cam_data.ortho_scale = 1.9
    cam = bpy.data.objects.new("ExprCam", cam_data)
    cam.location = (0.0, -8.0, 1.56)
    cam.rotation_euler = (math.radians(90), 0, 0)
    scene.collection.objects.link(cam)
    created.append(cam)
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
    scene.render.resolution_x, scene.render.resolution_y = res
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = False
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGB'
    scene.render.image_settings.color_depth = '8'
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    scene.render.filepath = os.path.abspath(out_path)
    print("TESTCHAR|rendering expression proof %dx%d -> %s" % (res[0], res[1], out_path))
    bpy.ops.render.render(write_still=True)

    for ob in created:
        try:
            bpy.data.objects.remove(ob, do_unlink=True)
        except Exception:
            pass
    mesh_ob.hide_render = False
    scene.camera = None
    scene.render.resolution_x, scene.render.resolution_y = 1920, 600
    size = os.path.getsize(out_path) if os.path.isfile(out_path) else 0
    print("TESTCHAR|expression proof written: %s (%d bytes)" % (out_path, size))
    return size


def main(argv):
    ap = argparse.ArgumentParser(prog="make_test_character")
    ap.add_argument("--out", default=os.path.join(REPO, "assets", "models", "testchar.glb"))
    ap.add_argument("--contact", default=os.path.join(REPO, "assets", "models", "testchar_contact.png"))
    ap.add_argument("--expressions", default=None,
                    help="optional 4-panel shape-key proof render (PNG path)")
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

    rep = verify_glb.audit(args.out, expect_morphs=EXPRESSIONS)   # prints the audit

    exp_size = None
    if args.expressions:
        exp_size = render_expression_proof(arm_ob, mesh_ob, args.expressions)
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
        "morph_targets": rep["morph"]["count"],
        "morph_names": rep["morph"]["names"],
        "morph_deltas": [t.get("delta_count") for t in rep["morph"]["targets"]],
        "expressions": None if not args.expressions else os.path.abspath(args.expressions),
        "expressions_bytes": exp_size,
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