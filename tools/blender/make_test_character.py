#!/usr/bin/env python3
"""make_test_character.py -- procedurally build a stylised chibi anime character
and export it as a KimiOS device-ready GLB. Runs headless under Blender 5.x:

  "C:\\Program Files\\Blender Foundation\\Blender 5.2\\blender.exe" -b --factory-startup \
      --python-exit-code 1 --python tools/blender/make_test_character.py

Produces (defaults, relative to the repo root):
  assets/models/testchar.glb          skinned GLB: POSITION/NORMAL/TEXCOORD_0/
                                      JOINTS_0/WEIGHTS_0 + indices, skin with
                                      inverseBindMatrices, embedded PNG texture,
                                      standard humanoid bone names
  assets/models/testchar_contact.png  4-pose contact sheet rendered in Blender
  assets/models/testchar_beauty.png   front / three-quarter / face beauty sheet
  assets/models/testchar_app.png      app-faithful preview (runtime shading model,
                                      rendered at the software rasteriser's
                                      384x512 target and downscaled to phone size)

Everything is generated from scratch: no external model, no add-ons, no MCP.
The body is built from lofted rings and tapered sweeps (smooth-shaded, no boxes):
a large rounded head with a tapered chin, big textured anime eyes, layered hair
(a crown shell plus pointed fringe locks, side locks and an ahoge), a flared
skirt, puffy sleeves, knee socks and little shoes. A 256x256 hand-painted atlas
carries soft gradients, blush, iris highlights and fabric shading; eyes, brows
and mouth are small real surface patches so the three shape keys -- `blink`,
`happy`, `surprised` -- deform them (the glTF exporter writes dense POSITION
morph targets plus `meshes[].extras.targetNames`).

Options (after `--`):
  --out PATH            output .glb (default <repo>/assets/models/testchar.glb)
  --contact PATH        output .png (default <repo>/assets/models/testchar_contact.png)
  --beauty PATH         output .png (default <repo>/assets/models/testchar_beauty.png)
  --app-preview PATH    output .png (default <repo>/assets/models/testchar_app.png)
  --expressions PATH    optional 4-panel shape-key proof render
  --tex-size N          atlas size, <= 256 (default 256)
  --no-contact          skip the contact-sheet render
  --no-beauty           skip the beauty-sheet render
  --no-app-preview      skip the app-faithful preview render
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

TAU = 2.0 * math.pi

# --------------------------------------------------------------------------- rig
# (name, parent, head, tail) in Blender space: +Z up, character faces -Y so that
# after the glTF +Y-up conversion it faces +Z (the glTF/Vulkan front convention).
# Bone *directions* match the previous humanoid rig so the runtime's procedural
# posing keeps working; only the proportions moved to chibi.
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
BONE_SEGS = {name: (Vector(head), Vector(tail)) for name, _p, head, tail in BONES}

# Which bones a body part may be weighted to. A single-entry list is rigid (the
# head, hair and facial features must not shear when the neck bends); longer
# lists use inverse distance-to-bone-segment for smooth joint blends.
PART_BONES = {
    "torso": ("hips", "spine", "chest", "neck"),
    "skirt": ("hips", "spine"),
    "head": ("head",),
    "arm_l": ("chest", "leftUpperArm", "leftLowerArm", "leftHand"),
    "arm_r": ("chest", "rightUpperArm", "rightLowerArm", "rightHand"),
    "leg_l": ("hips", "leftUpperLeg", "leftLowerLeg", "leftFoot"),
    "leg_r": ("hips", "rightUpperLeg", "rightLowerLeg", "rightFoot"),
    "shoe_l": ("leftLowerLeg", "leftFoot"),
    "shoe_r": ("rightLowerLeg", "rightFoot"),
}

# --------------------------------------------------------------------------- palette
# sRGB 0..255. A soft mint/coral/navy palette: warm skin and hair that read
# against the light GUI backdrop, one saturated accent (coral) for the ribbon,
# socks trim and shoes, and a deep navy for the skirt so the silhouette reads.
C = {
    "skin":       (255, 224, 199),
    "skin_hi":    (255, 238, 221),
    "skin_lo":    (241, 196, 172),
    "skin_deep":  (219, 162, 140),
    "blush":      (255, 146, 146),
    "nose":       (230, 166, 146),
    "hair":       (116, 196, 194),
    "hair_lo":    (64, 140, 142),
    "hair_deep":  (42, 106, 110),
    "hair_hi":    (174, 228, 227),
    "hair_shine": (226, 246, 248),
    "sclera":     (253, 252, 255),
    "sclera_lo":  (206, 219, 238),
    "lash":       (48, 32, 44),
    "iris_hi":    (255, 202, 92),
    "iris_mid":   (224, 138, 44),
    "iris_deep":  (150, 68, 24),
    "iris_ring":  (84, 38, 14),
    "pupil":      (34, 18, 10),
    "white":      (255, 255, 255),
    "cream":      (247, 240, 228),
    "cream_lo":   (224, 214, 200),
    "navy":       (58, 70, 110),
    "navy_lo":    (38, 46, 80),
    "navy_hi":    (82, 96, 142),
    "coral":      (242, 121, 107),
    "coral_hi":   (255, 158, 144),
    "coral_lo":   (203, 86, 74),
    "sock":       (250, 250, 248),
    "sock_lo":    (211, 218, 231),
    "sole":       (246, 238, 224),
    "mouth_in":   (122, 46, 56),
    "mouth_dark": (84, 26, 36),
    "teeth":      (255, 247, 241),
    "tongue":     (231, 116, 128),
}

# --------------------------------------------------------------------------- atlas
# Named atlas regions, normalised (x0, y0, w, h) with the origin bottom-left to
# match bpy image pixels. Resolution is deliberately spent on the face (136x144)
# and the eyes (96x120): the runtime box-downsamples the PNG to 128x128 before
# sampling it, so chunkier features survive far better than fine line work.
TEX_SIZE = 256
PAINT_SCALE = 3
PAINT_SIZE = TEX_SIZE * PAINT_SCALE
REGIONS = {
    "face":     (0 / 256, 112 / 256, 136 / 256, 144 / 256),
    "eye":      (136 / 256, 136 / 256, 88 / 256, 120 / 256),
    "mouth":    (136 / 256, 96 / 256, 88 / 256, 40 / 256),
    "brow":     (224 / 256, 96 / 256, 32 / 256, 40 / 256),
    "hair_top": (0 / 256, 48 / 256, 136 / 256, 64 / 256),
    "hair_lock": (136 / 256, 8 / 256, 80 / 256, 88 / 256),
    "hair_dark": (216 / 256, 8 / 256, 16 / 256, 88 / 256),
    "trim":     (232 / 256, 40 / 256, 24 / 256, 56 / 256),
    "torso":    (0 / 256, 0 / 256, 96 / 256, 48 / 256),
    "skirt":    (96 / 256, 0 / 256, 64 / 256, 48 / 256),
    "arm":      (160 / 256, 0 / 256, 48 / 256, 48 / 256),
    "leg":      (208 / 256, 0 / 256, 24 / 256, 48 / 256),
    "shoe":     (232 / 256, 0 / 256, 24 / 256, 40 / 256),
}


def _srgb_to_linear(c):
    c = c / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _mix(a, b, t):
    t = 0.0 if t < 0.0 else (1.0 if t > 1.0 else t)
    return (int(a[0] + (b[0] - a[0]) * t),
            int(a[1] + (b[1] - a[1]) * t),
            int(a[2] + (b[2] - a[2]) * t))


def _grad(stops, t):
    if t <= stops[0][0]:
        return stops[0][1]
    for i in range(len(stops) - 1):
        t0, c0 = stops[i]
        t1, c1 = stops[i + 1]
        if t <= t1:
            return _mix(c0, c1, (t - t0) / max(1e-9, t1 - t0))
    return stops[-1][1]


def _sstep(a, b, t):
    if abs(b - a) < 1e-9:
        return 1.0 if t >= b else 0.0
    x = (t - a) / (b - a)
    x = 0.0 if x < 0.0 else (1.0 if x > 1.0 else x)
    return x * x * (3.0 - 2.0 * x)


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


class Region:
    """A named rectangle of the atlas with normalised painting coordinates."""

    def __init__(self, cv, name):
        x0, y0, w, h = REGIONS[name]
        self.cv = cv
        self.x0 = int(round(x0 * cv.w))
        self.y0 = int(round(y0 * cv.h))
        self.w = max(1, int(round(w * cv.w)))
        self.h = max(1, int(round(h * cv.h)))

    def paint(self, fn):
        for y in range(self.h):
            v = (y + 0.5) / self.h
            for x in range(self.w):
                u = (x + 0.5) / self.w
                c = fn(u, v)
                if c is not None:
                    self.cv.set(self.x0 + x, self.y0 + y, c)

    def rect(self, u0, v0, u1, v1, rgb):
        x0 = self.x0 + int(round(u0 * self.w))
        x1 = self.x0 + int(round(u1 * self.w))
        y0 = self.y0 + int(round(v0 * self.h))
        y1 = self.y0 + int(round(v1 * self.h))
        for y in range(max(self.y0, y0), min(self.y0 + self.h, y1)):
            for x in range(max(self.x0, x0), min(self.x0 + self.w, x1)):
                self.cv.set(x, y, rgb)


# ---- per-region painters ---------------------------------------------------
_RNG = random.Random(0xC0FFEE)


def paint_face(u, v):
    c = _grad([(0.0, C["skin_lo"]), (0.18, C["skin"]), (0.52, C["skin_hi"]),
               (0.80, C["skin"]), (1.0, C["skin_lo"])], v)
    # soft shadow under the fringe
    c = _mix(c, C["skin_lo"], 0.40 * _sstep(0.76, 1.0, v))
    # under-chin ambient occlusion
    c = _mix(c, C["skin_deep"], 0.36 * _sstep(0.12, 0.0, v))
    # form shading: light comes from the character's +X side
    c = _mix(c, C["skin_lo"], 0.16 * (max(0.0, 0.5 - u) * 2.0) ** 2)
    # cheeks: two soft blush pools, kept small so they do not dominate
    for cu in (0.205, 0.795):
        d = ((u - cu) / 0.072) ** 2 + ((v - 0.320) / 0.050) ** 2
        c = _mix(c, C["blush"], 0.34 * math.exp(-d * 2.3))
    # nose: soft dot + its little shadow
    d = ((u - 0.5) / 0.026) ** 2 + ((v - 0.350) / 0.017) ** 2
    c = _mix(c, C["nose"], 0.34 * math.exp(-d * 2.2))
    d = ((u - 0.5) / 0.020) ** 2 + ((v - 0.333) / 0.011) ** 2
    c = _mix(c, C["skin_deep"], 0.18 * math.exp(-d * 2.2))
    # a hint of forehead highlight
    d = ((u - 0.56) / 0.14) ** 2 + ((v - 0.82) / 0.10) ** 2
    c = _mix(c, C["skin_hi"], 0.30 * math.exp(-d * 2.0))
    return c


def paint_eye(u, v):
    ex, ey = (u - 0.5) * 2.0, (v - 0.5) * 2.0
    d = math.hypot(ex, ey)
    if d > 1.02:
        return C["skin"]
    # sclera, shaded under the upper lid
    c = _mix(C["sclera"], C["sclera_lo"], _sstep(0.40, 1.0, v) * 0.85)
    c = _mix(c, C["sclera_lo"], _sstep(0.16, 0.0, v) * 0.35)
    # iris: vertical amber gradient with a dark limbal ring
    ix, iy = (u - 0.5) / 0.385, (v - 0.555) / 0.380
    di = math.hypot(ix, iy)
    if di < 1.06:
        t = (v - 0.180) / 0.755
        ic = _grad([(0.0, C["iris_hi"]), (0.34, C["iris_mid"]),
                    (0.74, C["iris_deep"]), (1.0, C["iris_deep"])], t)
        ic = _mix(ic, C["iris_ring"], _sstep(0.84, 1.03, di))
        # a warm crescent of bounced light at the bottom of the iris
        ic = _mix(ic, C["iris_hi"], 0.55 * math.exp(
            -(((u - 0.5) / 0.22) ** 2 + ((v - 0.30) / 0.11) ** 2) * 2.0))
        c = _mix(c, ic, 1.0)
    # pupil
    dp = math.hypot((u - 0.5) / 0.165, (v - 0.555) / 0.190)
    c = _mix(c, C["pupil"], _sstep(1.05, 0.90, dp))
    # two hard white highlights (the classic anime catch-light)
    for (hu, hv, hr) in ((0.392, 0.700, 0.108), (0.618, 0.440, 0.054)):
        dd = math.hypot((u - hu) / hr, (v - hv) / hr)
        c = _mix(c, C["white"], _sstep(1.0, 0.80, dd))
        if dd < 1.0:
            c = C["white"]
    # upper lash line: thick arc that follows the eye top
    th = 0.742 + 0.020 * (abs(ex) ** 2)
    c = _mix(c, C["lash"], _sstep(th, th + 0.040, v))
    # thin dark rim all around so the eye never reads as a sticker
    c = _mix(c, C["lash"], _sstep(0.955, 1.02, d) * 0.72)
    return c


def paint_mouth(u, v):
    c = C["mouth_in"]
    c = _mix(c, C["teeth"], _sstep(0.56, 0.70, v))
    c = _mix(c, C["mouth_dark"], _sstep(0.84, 1.0, v))
    d = ((u - 0.5) / 0.32) ** 2 + ((v - 0.17) / 0.21) ** 2
    c = _mix(c, C["tongue"], 0.88 * math.exp(-d * 1.9))
    c = _mix(c, C["mouth_dark"], _sstep(0.13, 0.0, v) * 0.55)
    return c


def paint_brow(u, v):
    c = _mix(C["hair_lo"], C["hair"], 0.35 + 0.55 * _sstep(0.0, 1.0, v))
    t = abs(u - 0.5) * 2.0
    c = _mix(c, C["hair_lo"], t * t * 0.35)
    return c


def paint_trim(u, v):
    # crisp flat swatches for small decals: coral | coral_hi | navy | cream
    if u < 0.25:
        return _mix(C["coral_lo"], C["coral"], _sstep(0.0, 1.0, v))
    if u < 0.50:
        return _mix(C["coral"], C["coral_hi"], _sstep(0.0, 1.0, v))
    if u < 0.75:
        return _mix(C["navy"], C["navy_hi"], _sstep(0.0, 1.0, v) * 0.7)
    return _mix(C["cream_lo"], C["cream"], _sstep(0.0, 1.0, v))


def paint_hair_top(u, v):
    c = _grad([(0.0, C["hair_lo"]), (0.30, C["hair"]), (0.66, C["hair_hi"]),
               (0.84, C["hair"]), (1.0, C["hair_deep"])], v)
    # the anime shine band: a wavy, periodic (seam-safe) highlight ring
    band = 0.58 + 0.05 * math.sin(u * TAU * 3.0 + 0.6) + 0.025 * math.cos(u * TAU * 5.0)
    c = _mix(c, C["hair_shine"], math.exp(-((v - band) / 0.085) ** 2) * 0.55)
    # strand streaks, also periodic so the UV seam at the back disappears
    c = _mix(c, C["hair_lo"], 0.13 * (0.5 + 0.5 * math.cos(u * TAU * 11.0 + 1.3)))
    c = _mix(c, C["hair_hi"], 0.10 * (0.5 + 0.5 * math.cos(u * TAU * 7.0 + 4.1)))
    # the hairline rim sits in shadow under the hair mass
    c = _mix(c, C["hair_deep"], 0.45 * _sstep(0.14, 0.0, v))
    return c


def paint_hair_lock(u, v):
    # v=1 root (dark), v=0 tip (light)
    c = _grad([(0.0, C["hair"]), (0.30, C["hair"]), (0.72, C["hair"]),
               (0.90, C["hair_lo"]), (1.0, C["hair_deep"])], v)
    c = _mix(c, C["hair_shine"], 0.55 * math.exp(-((u - 0.5) / 0.18) ** 2)
             * _sstep(0.15, 0.50, v) * _sstep(0.99, 0.72, v))
    c = _mix(c, C["hair_lo"], 0.12 * (0.5 + 0.5 * math.cos(u * TAU * 3.0 + 0.4)))
    return c


def paint_hair_dark(u, v):
    return _grad([(0.0, C["hair_lo"]), (0.6, C["hair"]), (1.0, C["hair_deep"])], v)


def paint_torso(u, v):
    # exposed neck above the collar
    if v > 0.885:
        c = _mix(C["skin"], C["skin_lo"], _sstep(0.885, 1.0, v) * 0.55)
        c = _mix(c, C["skin_deep"], 0.28 * _sstep(0.905, 0.885, v))
        return c
    c = _grad([(0.0, C["cream_lo"]), (0.28, C["cream"]), (0.72, C["cream"]),
               (1.0, C["cream"])], v)
    c = _mix(c, C["cream_lo"], 0.18 * (max(0.0, 0.5 - u) * 2.0) ** 2)
    # sailor yoke: a navy band with a cream trim along its lower edge
    if v > 0.765:
        c = _grad([(0.0, C["navy"]), (0.5, C["navy_hi"]), (1.0, C["navy"])],
                  (v - 0.765) / 0.12)
        if v < 0.800:
            c = C["cream"]
    elif v > 0.735:
        c = _mix(c, C["navy_lo"], 0.20)          # soft shadow under the yoke
    # a placket hint down the blouse front
    if abs(u - 0.5) < 0.012 and v > 0.30:
        c = _mix(c, C["cream_lo"], 0.55)
    # waist shadow
    c = _mix(c, C["navy_lo"], 0.42 * _sstep(0.06, 0.0, v))
    return c


def paint_skirt(u, v):
    # 12-period soft square wave == the pleat corrugation (seam-safe)
    k = 0.5 + 0.5 * math.tanh(math.sin(u * TAU * 12.0) * 2.6)
    c = _mix(C["navy"], C["navy_hi"], k)
    c = _mix(c, C["navy_lo"], 0.28 * _sstep(0.35, 0.0, v))
    # cream hem band + a thin stitch line above it
    if v < 0.115:
        c = _mix(C["cream"], C["cream_lo"], 0.35 * _sstep(0.115, 0.0, v))
    elif v < 0.135:
        c = _mix(c, C["navy_lo"], 0.8)
    # waistband shadow under the torso
    c = _mix(c, C["navy_lo"], 0.55 * _sstep(0.88, 1.0, v))
    return c


def paint_arm(u, v):
    # v=1 shoulder, v=0 hand
    if v > 0.598:
        c = _grad([(0.598, C["cream_lo"]), (0.72, C["cream"]), (1.0, C["cream_lo"])], v)
        c = _mix(c, C["cream_lo"], 0.22 * (max(0.0, 0.5 - u) * 2.0) ** 2)
        if v < 0.628:
            c = C["navy"]                     # thin sailor cuff trim
        return c
    c = _mix(C["skin"], C["skin_lo"], 0.15 * (max(0.0, 0.5 - u) * 2.0) ** 2)
    if 0.560 < v < 0.598:
        c = _mix(c, C["skin_deep"], 0.35)
    if v < 0.145:
        c = _mix(c, C["blush"], 0.14)
    return c


def paint_leg(u, v):
    # v=1 hip, v=0 ankle
    if v < 0.455:
        c = _mix(C["sock_lo"], C["sock"], _sstep(0.0, 0.30, v))
        c = _mix(c, C["sock_lo"], 0.18 * (max(0.0, 0.5 - u) * 2.0) ** 2)
        if 0.425 < v < 0.460:
            c = _mix(C["coral"], C["coral_hi"], 0.25)
        if v < 0.040:
            c = _mix(c, C["sock_lo"], 0.55)
        return c
    c = _mix(C["skin"], C["skin_lo"], 0.15 * (max(0.0, 0.5 - u) * 2.0) ** 2)
    if v < 0.490:
        c = _mix(c, C["skin_deep"], 0.32)
    c = _mix(c, C["skin_hi"], 0.30 * _sstep(0.50, 0.74, v))
    return c


def paint_shoe(u, v):
    du = abs(u - 0.5)
    c = _mix(C["coral"], C["coral_hi"], 0.30 * _sstep(0.35, 0.0, v))
    if du < 0.135:
        c = C["sole"]
    elif du < 0.175:
        c = _mix(C["coral_lo"], C["sole"], 0.35)
    if v > 0.88:
        c = _mix(c, C["coral_lo"], 0.45)
    return c


def make_atlas(size, path):
    """Draw the atlas and save it as a PNG; returns the packed bpy image.

    The art is painted at PAINT_SCALE x resolution and box-filtered down, which
    anti-aliases every painted edge (the runtime samples the texture with
    nearest-neighbour, so jagged source pixels would stay jagged on screen).
    """
    global TEX_SIZE, PAINT_SIZE
    TEX_SIZE = size
    PAINT_SIZE = size * PAINT_SCALE
    cv = Canvas(PAINT_SIZE, PAINT_SIZE)
    # neutral ground so mip bleed never shows black
    for i in range(PAINT_SIZE * PAINT_SIZE):
        j = i * 4
        cv.buf[j], cv.buf[j + 1], cv.buf[j + 2] = C["cream"]
    for name in REGIONS:
        r = Region(cv, name)
        fn = {
            "face": paint_face, "eye": paint_eye, "mouth": paint_mouth,
            "brow": paint_brow, "hair_top": paint_hair_top,
            "hair_lock": paint_hair_lock, "hair_dark": paint_hair_dark,
            "trim": paint_trim,
            "torso": paint_torso, "skirt": paint_skirt, "arm": paint_arm,
            "leg": paint_leg, "shoe": paint_shoe,
        }[name]
        r.paint(fn)

    s = PAINT_SCALE
    img = bpy.data.images.new("testchar_basecolor", size, size, alpha=True)
    px = [0.0] * (size * size * 4)
    inv = 1.0 / (s * s)
    for y in range(size):
        for x in range(size):
            r = g = b = 0
            for dy in range(s):
                row = ((y * s + dy) * PAINT_SIZE + x * s) * 4
                for dx in range(s):
                    j = row + dx * 4
                    r += cv.buf[j]
                    g += cv.buf[j + 1]
                    b += cv.buf[j + 2]
            i = (y * size + x) * 4
            px[i] = _srgb_to_linear(r * inv)
            px[i + 1] = _srgb_to_linear(g * inv)
            px[i + 2] = _srgb_to_linear(b * inv)
            px[i + 3] = 1.0
    img.pixels.foreach_set(px)
    img.filepath_raw = path
    img.file_format = 'PNG'
    img.save()
    img.pack()
    print("TESTCHAR|atlas %dx%d painted at %dx then downsampled -> %s (%d bytes)"
          % (size, size, PAINT_SIZE, path, os.path.getsize(path)))
    return img


# --------------------------------------------------------------------------- UV
def region_uv(name, u, v):
    """Map region-normalised (u,v) to atlas UVs with a one-texel inset so
    bilinear/mip sampling never bleeds across the region border."""
    x0, y0, w, h = REGIONS[name]
    tx = 1.0 / TEX_SIZE
    return (x0 + tx + max(0.0, min(1.0, u)) * (w - 2.0 * tx),
            y0 + tx + max(0.0, min(1.0, v)) * (h - 2.0 * tx))


# --------------------------------------------------------------------------- mesh
class Builder:
    def __init__(self):
        self.verts = []
        self.vparts = []
        self.vfeats = []
        self.faces = []
        self.fregions = []
        self.fuvs = []      # list of per-corner (u,v) or None for a solid sample
        self.fsmooth = []
        self.fopen = []     # keep the constructed winding (open shells, decals)
        self.fnormals = []

    def vert(self, co, part, feat=None):
        self.verts.append(Vector(co))
        self.vparts.append(part)
        self.vfeats.append(feat)
        return len(self.verts) - 1

    def face(self, idxs, region, uvs=None, smooth=True, open_shell=False):
        idxs = tuple(idxs)
        p = [self.verts[i] for i in idxs]
        if len(p) >= 3:
            n = (p[1] - p[0]).cross(p[2] - p[0])
        else:
            n = Vector((0.0, 0.0, 0.0))
        self.faces.append(idxs)
        self.fregions.append(region)
        self.fuvs.append(uvs)
        self.fsmooth.append(smooth)
        self.fopen.append(open_shell)
        self.fnormals.append(n)

    def stats(self):
        tris = 0
        for f in self.faces:
            tris += max(0, len(f) - 2)
        return len(self.verts), len(self.faces), tris


def _frame(tangent, ref_dir):
    """Orthonormal ring axes; the ring angle whose direction is `ref_dir` maps to
    u = 0.5 (used by every textured band)."""
    t = Vector(tangent).normalized()
    up = Vector((0.0, 0.0, 1.0))
    if abs(t.dot(up)) > 0.92:
        up = Vector((0.0, 1.0, 0.0))
    ua = up.cross(t).normalized()
    va = t.cross(ua).normalized()
    return ua, va, math.atan2(ref_dir.dot(va), ref_dir.dot(ua))


def sweep(b, stations, part, sides, region_of_band, ref_dir=Vector((0.0, -1.0, 0.0)),
          cap_start=True, cap_end=True, tip_end=None, smooth=True, open_shell=True,
          ang_off=0.0, tip_uv=None, cap_uv=None):
    """Lofted tube through `stations` = [(centre, radius_a, radius_b, v_param)].

    Rings are built with a tangent frame, so winding is outward for any path
    direction. `region_of_band(k)` picks the atlas region of the k-th band.
    `open_shell=False` marks the result as a closed volume (normals may be
    recalculated later); open tubes keep their constructed winding.
    `ang_off` rotates the cross-section (a 45 deg phase turns a 4-gon blade so
    a flat face, not a crease, points forward).
    `tip_uv` / `cap_uv` pin the pole/cap UV instead of using the region centre,
    which otherwise samples the wrong colour for e.g. a shoe toe.
    """
    n = len(stations)
    cols = []
    for k, (pos, ra, rb, vp) in enumerate(stations):
        if k == 0:
            tan = Vector(stations[1][0]) - Vector(pos)
        elif k == n - 1:
            tan = Vector(pos) - Vector(stations[n - 2][0])
        else:
            tan = Vector(stations[k + 1][0]) - Vector(stations[k - 1][0])
        ua, va, a_ref = _frame(tan, ref_dir)
        ring = []
        for i in range(sides):
            a = TAU * i / sides + ang_off
            p = Vector(pos) + ua * (math.cos(a) * ra) + va * (math.sin(a) * rb)
            ring.append((b.vert(tuple(p), part), a, vp))
        cols.append(ring)

    for k in range(n - 1):
        region = region_of_band(k)
        for i in range(sides):
            j = (i + 1) % sides
            i0, a0, v0 = cols[k][i]
            i1, a1, _ = cols[k][j]
            i2, a2, v1 = cols[k + 1][j]
            i3, a3, _ = cols[k + 1][i]
            u0 = (a0 - a_ref + math.pi) % TAU / TAU
            u1 = (a1 - a_ref + math.pi) % TAU / TAU
            if u1 <= u0:
                u1 += 1.0
            u2 = u1
            u3 = u0
            uvs = [region_uv(region, u0, v0), region_uv(region, u1, v0),
                   region_uv(region, u2, v1), region_uv(region, u3, v1)]
            b.face((i0, i1, i2, i3), region, uvs, smooth, open_shell)

    if cap_start:
        region = region_of_band(0)
        ids = [c[0] for c in cols[0]]
        solid = cap_uv if cap_uv is not None else region_uv(region, 0.5, 0.5)
        b.face(list(reversed(ids)), region, [solid] * len(ids),
               smooth=False, open_shell=open_shell)
    if tip_end is not None:
        region = region_of_band(n - 2)
        tip = b.vert(tuple(tip_end), part)
        last = cols[-1]
        # one shared UV keeps the pole merged (no split copies)
        pole_uv = tip_uv if tip_uv is not None else region_uv(region, 0.5, 0.0)
        for i in range(sides):
            j = (i + 1) % sides
            u0 = (last[i][1] - a_ref + math.pi) % TAU / TAU
            u1 = (last[j][1] - a_ref + math.pi) % TAU / TAU
            if u1 <= u0:
                u1 += 1.0
            v = last[i][2]
            b.face((last[i][0], last[j][0], tip), region,
                   [region_uv(region, u0, v), region_uv(region, u1, v), pole_uv],
                   smooth, open_shell)
    elif cap_end:
        region = region_of_band(n - 2)
        ids = [c[0] for c in cols[-1]]
        solid = cap_uv if cap_uv is not None else region_uv(region, 0.5, 0.5)
        b.face(ids, region, [solid] * len(ids), smooth=False, open_shell=open_shell)
    return cols


# ---- head ------------------------------------------------------------------
HEAD_SIDES = 16
HEAD_BOT, HEAD_TOP = 1.080, 1.548          # chin underside .. crown
HEAD_CZ = 1.315
HEAD_HX = 0.206                            # planar UV half-width of the face patch
HEAD_HZ = HEAD_TOP - HEAD_BOT
HEAD_RINGS = [
    (1.092, 0.072, 0.076),
    (1.135, 0.142, 0.150),
    (1.195, 0.181, 0.192),
    (1.262, 0.200, 0.211),
    (1.330, 0.205, 0.215),
    (1.400, 0.195, 0.206),
    (1.468, 0.162, 0.175),
]


def head_rx_ry(z):
    rings = HEAD_RINGS
    if z <= rings[0][0]:
        return rings[0][1], rings[0][2]
    if z >= rings[-1][0]:
        return rings[-1][1], rings[-1][2]
    for i in range(len(rings) - 1):
        z0, rx0, ry0 = rings[i]
        z1, rx1, ry1 = rings[i + 1]
        if z <= z1:
            t = (z - z0) / (z1 - z0)
            return rx0 + (rx1 - rx0) * t, ry0 + (ry1 - ry0) * t
    return rings[-1][1], rings[-1][2]


def head_front_y(x, z, offset=0.0):
    """Exact -Y surface point of the head lathe at (x, z)."""
    rx, ry = head_rx_ry(z)
    t = min(1.0, abs(x) / rx) if rx > 1e-6 else 1.0
    return -ry * math.sqrt(max(0.0, 1.0 - t * t)) - offset


def hair_cap_front_y(x, z, offset=0.0):
    k = math.sqrt(max(0.0, 1.0 - ((z - HC_CX) / HC_RZ) ** 2))
    rx, ry = HC_RX * k, HC_RY * k
    t = min(1.0, abs(x) / rx) if rx > 1e-6 else 1.0
    return -ry * math.sqrt(max(0.0, 1.0 - t * t)) - offset


def torso_rx_ry(z):
    rings = TORSO_RINGS
    if z <= rings[0][0]:
        return rings[0][1], rings[0][2]
    if z >= rings[-1][0]:
        return rings[-1][1], rings[-1][2]
    for i in range(len(rings) - 1):
        z0, rx0, ry0 = rings[i]
        z1, rx1, ry1 = rings[i + 1]
        if z <= z1:
            t = (z - z0) / (z1 - z0)
            return rx0 + (rx1 - rx0) * t, ry0 + (ry1 - ry0) * t
    return rings[-1][1], rings[-1][2]


def torso_front_y(x, z, offset=0.0):
    rx, ry = torso_rx_ry(z)
    t = min(1.0, abs(x) / rx) if rx > 1e-6 else 1.0
    return -ry * math.sqrt(max(0.0, 1.0 - t * t)) - offset


def head_face_uv(x, z):
    return ((x + HEAD_HX) / (2.0 * HEAD_HX), (z - HEAD_BOT) / HEAD_HZ)


def build_head(b):
    cols = []
    for (z, rx, ry) in HEAD_RINGS:
        col = []
        for i in range(HEAD_SIDES):
            a = TAU * i / HEAD_SIDES
            col.append(b.vert((rx * math.cos(a), ry * math.sin(a), z), "head"))
        cols.append(col)
    for k in range(len(cols) - 1):
        for i in range(HEAD_SIDES):
            j = (i + 1) % HEAD_SIDES
            ids = (cols[k][i], cols[k][j], cols[k + 1][j], cols[k + 1][i])
            ps = [b.verts[x] for x in ids]
            yc = sum(p.y for p in ps) / 4.0
            if yc < 0.0:
                uvs = [region_uv("face", *head_face_uv(p.x, p.z)) for p in ps]
                b.face(ids, "face", uvs, True)
            else:
                b.face(ids, "hair_dark", None, True)
    top = b.vert((0.0, 0.0, HEAD_TOP), "head")
    for i in range(HEAD_SIDES):
        j = (i + 1) % HEAD_SIDES
        b.face((cols[-1][i], cols[-1][j], top), "hair_dark", None, True)
    bot = b.vert((0.0, 0.0, HEAD_BOT), "head")
    for i in range(HEAD_SIDES):
        j = (i + 1) % HEAD_SIDES
        ids = (bot, cols[0][j], cols[0][i])
        uvs = [region_uv("face", *head_face_uv(b.verts[x].x, b.verts[x].z)) for x in ids]
        b.face(ids, "face", uvs, True)


# ---- hair ------------------------------------------------------------------
HC_CX = HEAD_CZ
HC_RX, HC_RY, HC_RZ = 0.248, 0.244, 0.290
HC_TOP = 1.605
HC_SIDES = 24
HC_ROWS = (0.10, 0.40, 0.72, 1.0)
HC_HAIRLINE_FRONT, HC_HAIRLINE_BACK = 1.462, 1.118
HC_TOOTH = 0.034


def hair_cap_radii(z):
    k = math.sqrt(max(0.0, 1.0 - ((z - HC_CX) / HC_RZ) ** 2))
    return HC_RX * k, HC_RY * k


def hairline_z(a):
    """High at the brow, low at the nape, with a sawtooth fringe across the
    front so the hair shell's own lower edge reads as pointed bangs."""
    t = (1.0 - math.sin(a)) / 2.0
    base = HC_HAIRLINE_BACK + (HC_HAIRLINE_FRONT - HC_HAIRLINE_BACK) * (t ** 0.80)
    front = _sstep(0.46, 0.80, t)
    if front > 0.0:
        i = int(round((a % TAU) / TAU * HC_SIDES)) % HC_SIDES
        base -= (HC_TOOTH if i % 2 == 0 else 0.0) * front
    return base


def build_hair_cap(b):
    sides = HC_SIDES
    cols = []
    for s in HC_ROWS:
        ring = []
        for i in range(sides):
            a = TAU * i / sides
            z = HC_TOP + (hairline_z(a) - HC_TOP) * s
            ex, ey = hair_cap_radii(z)
            hx, hy = head_rx_ry(z)
            w = _sstep(0.55, 1.0, s)
            rx = ex * (1.0 - w) + (hx + 0.008) * w
            ry = ey * (1.0 - w) + (hy + 0.008) * w
            ring.append((b.vert((rx * math.cos(a), ry * math.sin(a), z), "head"), a, 1.0 - s))
        cols.append(ring)
    for k in range(len(cols) - 1):
        for i in range(sides):
            j = (i + 1) % sides
            ids = (cols[k][i][0], cols[k][j][0], cols[k + 1][j][0], cols[k + 1][i][0])
            u0 = (cols[k][i][1] - math.pi / 2.0) % TAU / TAU
            u1 = (cols[k][j][1] - math.pi / 2.0) % TAU / TAU
            if u1 <= u0:
                u1 += 1.0
            u2 = u1
            u3 = u0
            uvs = [region_uv("hair_top", u0, cols[k][i][2]),
                   region_uv("hair_top", u1, cols[k][j][2]),
                   region_uv("hair_top", u2, cols[k + 1][j][2]),
                   region_uv("hair_top", u3, cols[k + 1][i][2])]
            b.face(ids, "hair_top", uvs, True, open_shell=True)
    # crown pole: one UV for all fan triangles so the pole vertex stays merged
    top = b.vert((0.0, 0.0, HC_TOP + 0.007), "head")
    pole_uv = region_uv("hair_top", 0.5, 1.0)
    for i in range(sides):
        j = (i + 1) % sides
        u0 = (cols[0][i][1] - math.pi / 2.0) % TAU / TAU
        u1 = (cols[0][j][1] - math.pi / 2.0) % TAU / TAU
        if u1 <= u0:
            u1 += 1.0
        b.face((cols[0][i][0], cols[0][j][0], top), "hair_top",
               [region_uv("hair_top", u0, cols[0][i][2]),
                region_uv("hair_top", u1, cols[0][j][2]), pole_uv],
               True, open_shell=True)


def build_hair_locks(b):
    """Pointed fringe locks, two cheek-framing side locks and an ahoge. All are
    tapered flattened sweeps closed with a single tip pole."""
    ref = Vector((0.0, -1.0, 0.0))

    # side locks: thin strands framing the cheeks, hugging the head silhouette
    for sgn in (-1.0, 1.0):
        pts = [(1.446, 0.192, 0.016, 0.014), (1.368, 0.205, 0.021, 0.012),
               (1.280, 0.204, 0.018, 0.010), (1.200, 0.194, 0.010, 0.006)]
        stations = []
        for k, (z, x, ra, rb) in enumerate(pts):
            xx = sgn * x
            y = head_front_y(xx, z, 0.012 if k else 0.018)
            stations.append((Vector((xx, y, z)), ra, rb, 1.0 - k / (len(pts) - 1.0)))
        tip = (sgn * 0.190, head_front_y(sgn * 0.190, 1.170, 0.010), 1.168)
        sweep(b, stations, "head", 4, lambda k: "hair_lock", ref_dir=ref,
              cap_start=False, tip_end=tip, ang_off=math.pi / 4.0)

    # ahoge: one playful strand off the crown, curling forward
    st = [
        (Vector((0.010, 0.008, 1.596)), 0.010, 0.008, 1.0),
        (Vector((0.014, -0.020, 1.620)), 0.0070, 0.0052, 0.55),
        (Vector((0.020, -0.058, 1.626)), 0.0028, 0.0022, 0.12),
    ]
    sweep(b, st, "head", 4, lambda k: "hair_lock", ref_dir=ref,
          cap_start=False, tip_end=(0.024, -0.080, 1.616), ang_off=math.pi / 4.0)


# ---- face ------------------------------------------------------------------
EYE_X, EYE_Z, EYE_HW, EYE_HH = 0.090, 1.312, 0.054, 0.074
EYE_SIDES = 16
BROW_X, BROW_Z = 0.096, 1.398
MOUTH_Z, MOUTH_HALF = 1.190, 0.046


def build_face(b):
    feats = {}

    for side, sx in (("l", 1.0), ("r", -1.0)):
        name = "eye_" + side
        cx = sx * EYE_X
        verts = []
        c = b.vert((cx, head_front_y(cx, EYE_Z, 0.010), EYE_Z), "head", name)
        rim = []
        for i in range(EYE_SIDES):
            a = TAU * i / EYE_SIDES
            x = cx + EYE_HW * math.cos(a)
            z = EYE_Z + EYE_HH * math.sin(a)
            rim.append(b.vert((x, head_front_y(x, z, 0.007), z), "head", name))
        for i in range(EYE_SIDES):
            j = (i + 1) % EYE_SIDES
            b.face((c, rim[i], rim[j]), "eye",
                   [region_uv("eye", 0.5, 0.5),
                    region_uv("eye", (b.verts[rim[i]].x - cx + EYE_HW) / (2 * EYE_HW),
                              (b.verts[rim[i]].z - EYE_Z + EYE_HH) / (2 * EYE_HH)),
                    region_uv("eye", (b.verts[rim[j]].x - cx + EYE_HW) / (2 * EYE_HW),
                              (b.verts[rim[j]].z - EYE_Z + EYE_HH) / (2 * EYE_HH))],
                   True, open_shell=True)
        feats[name] = {"verts": [c] + rim, "cx": cx, "cz": EYE_Z,
                       "hw": EYE_HW, "hh": EYE_HH}

    # eyebrows: thin arched strips
    for side, sx in (("l", 1.0), ("r", -1.0)):
        name = "brow_" + side
        cx = sx * BROW_X
        half = 0.027
        xs = (-half, 0.0, half)
        top, bot = [], []
        for x in xs:
            t = x / half
            zc = BROW_Z + 0.0045 * (1.0 - t * t) - 0.0015
            hh = 0.0035 * (1.0 - 0.30 * t * t) + 0.0008
            xx = cx + x
            top.append(b.vert((xx, head_front_y(xx, zc + hh, 0.006), zc + hh), "head", name))
            bot.append(b.vert((xx, head_front_y(xx, zc - hh, 0.006), zc - hh), "head", name))
        z0, z1 = BROW_Z - 0.007, BROW_Z + 0.009
        for i in range(len(xs) - 1):
            ids = (bot[i], bot[i + 1], top[i + 1], top[i])
            uvs = []
            for v in ids:
                p = b.verts[v]
                uvs.append(region_uv("brow", (p.x - (cx - half)) / (2 * half),
                                     (p.z - z0) / (z1 - z0)))
            b.face(ids, "brow", uvs, False, open_shell=True)
        feats[name] = {"verts": top + bot, "cx": cx, "cz": BROW_Z}

    # mouth: a small smiling lens (top edge up at the corners)
    xs = (-MOUTH_HALF, -MOUTH_HALF * 0.5, 0.0, MOUTH_HALF * 0.5, MOUTH_HALF)
    top, bot = [], []
    for x in xs:
        t = x / MOUTH_HALF
        zc = MOUTH_Z + 0.011 * t * t
        hh = 0.0170 * (1.0 - t * t) + 0.0012
        top.append(b.vert((x, head_front_y(x, zc + hh * 0.62, 0.006), zc + hh * 0.62),
                          "head", "mouth"))
        bot.append(b.vert((x, head_front_y(x, zc - hh * 0.58, 0.006), zc - hh * 0.58),
                          "head", "mouth"))
    z0, z1 = MOUTH_Z - 0.025, MOUTH_Z + 0.025
    for i in range(len(xs) - 1):
        ids = (bot[i], bot[i + 1], top[i + 1], top[i])
        uvs = []
        for v in ids:
            p = b.verts[v]
            uvs.append(region_uv("mouth", (p.x + MOUTH_HALF) / (2 * MOUTH_HALF),
                                 (p.z - z0) / (z1 - z0)))
        b.face(ids, "mouth", uvs, False, open_shell=True)
    feats["mouth"] = {"verts": top + bot, "cx": 0.0, "cz": MOUTH_Z}
    return feats


# ---- body ------------------------------------------------------------------
TORSO_Z0, TORSO_Z1 = 0.650, 1.100
TORSO_RINGS = [
    (0.650, 0.126, 0.095),
    (0.740, 0.136, 0.102),
    (0.840, 0.139, 0.105),
    (0.930, 0.149, 0.111),
    (1.005, 0.146, 0.110),
    (1.045, 0.078, 0.067),
    (1.100, 0.057, 0.054),
]
SKIRT_Z0, SKIRT_Z1 = 0.585, 0.748
SKIRT_RINGS = [
    (0.585, 0.238, 0.196),
    (0.650, 0.200, 0.162),
    (0.700, 0.172, 0.136),
    (0.748, 0.152, 0.118),
]
# The T-pose arm span is deliberately close to human proportions: the KimiOS
# rasteriser frames the model by fitting its *rest* bounding-sphere at 2.2x with
# a 0.62 rad vertical FOV, so a narrow chibi T-pose would make the camera crop
# the head and feet. Arms hang down in every posed frame, so the extra span is
# invisible in the app -- it just widens the rest bbox enough for the whole
# character to fit the phone's render target.
ARM_STATIONS = [
    (0.130, 0.070),
    (0.228, 0.078),
    (0.297, 0.070),
    (0.362, 0.056),
    (0.404, 0.043),
    (0.517, 0.0405),
    (0.607, 0.0435),
    (0.666, 0.052),
    (0.711, 0.050),
]
ARM_TIP = 0.759
LEG_STATIONS = [
    (0.705, 0.082),
    (0.600, 0.080),
    (0.470, 0.072),
    (0.340, 0.063),
    (0.200, 0.057),
    (0.060, 0.048),
]
LEG_X = 0.085
# flatter and longer than a circle, so it reads as a little shoe on the ground
SHOE_STATIONS = [
    (0.052, 0.034, 0.046, 0.034),
    (0.004, 0.037, 0.055, 0.037),
    (-0.076, 0.034, 0.055, 0.034),
    (-0.128, 0.028, 0.045, 0.028),
]
SHOE_TIP = (-0.165, 0.024)
LEG_Z0, LEG_Z1 = 0.060, 0.705


def _lathe_band(b, cols, part, region, v_of_ring, smooth=True, open_shell=False):
    """cols: list of lists of vertex indices, equal length; rings ordered bottom-up."""
    sides = len(cols[0])
    for k in range(len(cols) - 1):
        for i in range(sides):
            j = (i + 1) % sides
            ids = (cols[k][i], cols[k][j], cols[k + 1][j], cols[k + 1][i])
            u0 = i / sides
            u1 = (i + 1) / sides
            uvs = [region_uv(region, u0, v_of_ring[k]),
                   region_uv(region, u1, v_of_ring[k]),
                   region_uv(region, u1, v_of_ring[k + 1]),
                   region_uv(region, u0, v_of_ring[k + 1])]
            b.face(ids, region, uvs, smooth, open_shell)


def build_torso(b):
    sides = 12
    cols = []
    vs = []
    for (z, rx, ry) in TORSO_RINGS:
        a_ref = -math.pi / 2.0
        col = []
        for i in range(sides):
            a = TAU * i / sides
            col.append(b.vert((rx * math.cos(a), ry * math.sin(a), z), "torso"))
        cols.append(col)
        vs.append((z - TORSO_Z0) / (TORSO_Z1 - TORSO_Z0))
    # cylindrical UV with the front at u = 0.5 (region seam at the back)
    for k in range(len(cols) - 1):
        for i in range(sides):
            j = (i + 1) % sides
            ids = (cols[k][i], cols[k][j], cols[k + 1][j], cols[k + 1][i])
            u0 = ((TAU * i / sides) - a_ref + math.pi) % TAU / TAU
            u1 = ((TAU * j / sides) - a_ref + math.pi) % TAU / TAU
            if u1 <= u0:
                u1 += 1.0
            uvs = [region_uv("torso", u0, vs[k]), region_uv("torso", u1, vs[k]),
                   region_uv("torso", u1, vs[k + 1]), region_uv("torso", u0, vs[k + 1])]
            b.face(ids, "torso", uvs, True, True)
    # both ends are buried (neck in the head, waist in the skirt): no caps


def build_skirt(b):
    sides = 12
    a_ref = -math.pi / 2.0
    cols, vs = [], []
    for (z, rx, ry) in SKIRT_RINGS:
        col = []
        for i in range(sides):
            a = TAU * i / sides
            col.append(b.vert((rx * math.cos(a), ry * math.sin(a), z), "skirt"))
        cols.append(col)
        vs.append((z - SKIRT_Z0) / (SKIRT_Z1 - SKIRT_Z0))
    for k in range(len(cols) - 1):
        for i in range(sides):
            j = (i + 1) % sides
            ids = (cols[k][i], cols[k][j], cols[k + 1][j], cols[k + 1][i])
            u0 = ((TAU * i / sides) - a_ref + math.pi) % TAU / TAU
            u1 = ((TAU * j / sides) - a_ref + math.pi) % TAU / TAU
            if u1 <= u0:
                u1 += 1.0
            uvs = [region_uv("skirt", u0, vs[k]), region_uv("skirt", u1, vs[k]),
                   region_uv("skirt", u1, vs[k + 1]), region_uv("skirt", u0, vs[k + 1])]
            b.face(ids, "skirt", uvs, True, True)
    # close only the hem; the waist opening is hidden inside the torso
    b.face(list(reversed(cols[0])), "skirt",
           [region_uv("skirt", 0.25, 0.0)] * sides, False, True)


def build_arms(b):
    for side, sx in (("l", 1.0), ("r", -1.0)):
        part = "arm_" + side
        stations = []
        for k, (x, r) in enumerate(ARM_STATIONS):
            z = 1.005 + 0.004 * math.sin(k * 1.1)   # a whisper of arm curve
            v = 1.0 - (x - ARM_STATIONS[0][0]) / (ARM_TIP - ARM_STATIONS[0][0])
            stations.append((Vector((sx * x, 0.0, z)), r, r * 1.04, v))
        sweep(b, stations, part, 8, lambda k: "arm",
              ref_dir=Vector((0.0, -1.0, 0.0)), cap_start=False,
              tip_end=(sx * ARM_TIP, 0.0, 1.005))


def build_legs(b):
    for side, sx in (("l", 1.0), ("r", -1.0)):
        part = "leg_" + side
        stations = []
        for (z, r) in LEG_STATIONS:
            v = 1.0 - (z - LEG_Z0) / (LEG_Z1 - LEG_Z0)
            stations.append((Vector((sx * LEG_X, 0.0, z)), r, r * 1.02, v))
        sweep(b, stations, part, 8, lambda k: "leg",
              ref_dir=Vector((0.0, -1.0, 0.0)), cap_start=False, cap_end=False)
        # shoe: swept forward along -Y, sole flat on y = 0
        part = "shoe_" + side
        st = []
        for k, (y, cz, rx, rz) in enumerate(SHOE_STATIONS):
            v = (y - SHOE_TIP[0]) / (SHOE_STATIONS[0][0] - SHOE_TIP[0])
            st.append((Vector((sx * LEG_X, y, cz)), rx, rz, v))
        sweep(b, st, part, 8, lambda k: "shoe",
              ref_dir=Vector((0.0, 0.0, -1.0)), cap_start=True,
              tip_end=(sx * LEG_X, SHOE_TIP[0], SHOE_TIP[1]), open_shell=False,
              tip_uv=region_uv("shoe", 0.16, 0.0),
              cap_uv=region_uv("shoe", 0.84, 0.5))


def _front_quad(b, ids, region, uvs):
    """Emit a surface decal quad whose normal we force to face -Y (the front)."""
    p = [b.verts[i] for i in ids]
    n = (p[1] - p[0]).cross(p[2] - p[0])
    if n.y > 0.0:
        ids = list(reversed(ids))
        uvs = list(reversed(uvs))
    b.face(ids, region, uvs, True, open_shell=True)


def _front_tri(b, ids, region, uvs):
    p = [b.verts[i] for i in ids]
    n = (p[1] - p[0]).cross(p[2] - p[0])
    if n.y > 0.0:
        ids = list(reversed(ids))
        uvs = list(reversed(uvs))
    b.face(ids, region, uvs, True, open_shell=True)


def _trim_uv(u0, u1, v_of_z, z):
    """UV into the `trim` swatch strip: u0..u1 selects the colour, v follows z."""
    return (u0 + (u1 - u0) * 0.5, v_of_z(z))


def build_collar(b):
    """Sailor neckerchief V on the blouse plus a coral bow -- real surface
    decals so the crisp V shape does not depend on the torso texture density.
    The strip is split into segments because a single flat quad would cut
    inside the convex chest."""
    lift = 0.008

    def pt(x, z):
        return b.vert((x, torso_front_y(x, z, lift), z), "torso")

    outer = [(-0.106, 1.004), (-0.054, 0.966), (0.0, 0.924)]
    inner = [(-0.062, 1.030), (-0.027, 0.974), (0.0, 0.950)]

    def nuv(z):
        return 0.10 + 0.80 * (z - 0.920) / 0.115

    for sgn in (-1.0, 1.0):
        cols = []
        for k in range(3):
            cols.append((pt(sgn * outer[k][0], outer[k][1]),
                         pt(sgn * inner[k][0], inner[k][1])))
        for k in range(2):
            (a, bb), (c, d) = cols[k], cols[k + 1]
            ids = [a, c, d, bb]
            uvs = [region_uv("trim", 0.60, nuv(b.verts[i].z)) for i in ids]
            _front_quad(b, ids, "trim", uvs)

    # bow: knot + two lifted loop tips
    bz = 0.932
    knot = [pt(-0.013, bz - 0.012), pt(0.013, bz - 0.012),
            pt(0.013, bz + 0.014), pt(-0.013, bz + 0.014)]
    uvs_knot = [region_uv("trim", 0.10, 0.30), region_uv("trim", 0.10, 0.30),
                region_uv("trim", 0.12, 0.72), region_uv("trim", 0.12, 0.72)]
    _front_quad(b, knot, "trim", uvs_knot)
    ltip = b.vert((-0.058, torso_front_y(-0.058, bz - 0.004, 0.016), bz - 0.004), "torso")
    rtip = b.vert((0.058, torso_front_y(0.058, bz - 0.004, 0.016), bz - 0.004), "torso")
    for (tip, ids) in ((ltip, [knot[0], knot[3], ltip]), (rtip, [knot[1], knot[2], rtip])):
        uvs = [region_uv("trim", 0.06, 0.45), region_uv("trim", 0.06, 0.55),
               region_uv("trim", 0.20, 0.50)]
        _front_tri(b, ids, "trim", uvs)


def build_hair_clip(b):
    """A small coral ribbon clip on the crown: a signature accessory."""
    cx, cz = -0.128, 1.492

    def pt(x, z, lift):
        return b.vert((x, hair_cap_front_y(x, z, lift), z), "head")

    knot = [pt(cx - 0.009, cz - 0.009, 0.010), pt(cx + 0.009, cz - 0.009, 0.010),
            pt(cx + 0.009, cz + 0.011, 0.010), pt(cx - 0.009, cz + 0.011, 0.010)]
    uvs_knot = [region_uv("trim", 0.10, 0.30)] * 2 + [region_uv("trim", 0.12, 0.72)] * 2
    _front_quad(b, knot, "trim", uvs_knot)
    for (dx, ids) in ((-1.0, [knot[0], knot[3]]), (1.0, [knot[1], knot[2]])):
        tip = pt(cx + dx * 0.040, cz + 0.004, 0.015)
        uvs = [region_uv("trim", 0.06, 0.42), region_uv("trim", 0.06, 0.52),
               region_uv("trim", 0.19, 0.47)]
        _front_tri(b, ids + [tip], "trim", uvs)


def build_geometry():
    b = Builder()
    build_head(b)
    build_hair_cap(b)
    build_hair_locks(b)
    feats = build_face(b)
    build_torso(b)
    build_skirt(b)
    build_collar(b)
    build_arms(b)
    build_legs(b)
    nv, nf, nt = b.stats()
    print("TESTCHAR|geometry: %d verts, %d faces, %d tris (pre-split, pre-decimate)"
          % (nv, nf, nt))
    return b, feats


def _seg_dist(p, a, b):
    ab = b - a
    l2 = ab.length_squared
    if l2 < 1e-12:
        return (p - a).length
    t = max(0.0, min(1.0, (p - a).dot(ab) / l2))
    return (p - (a + ab * t)).length


def bone_weights(co, allowed, power=3.0):
    if len(allowed) == 1:
        return [(allowed[0], 1.0)]
    scored = [(1.0 / max(_seg_dist(co, *BONE_SEGS[name]), 1e-4) ** power, name)
              for name in allowed]
    scored.sort(reverse=True)
    scored = scored[:4]
    total = sum(w for w, _ in scored)
    out = [(name, w / total) for w, name in scored]
    out = [(n, w) for n, w in out if w >= 2e-3]
    total = sum(w for _, w in out)
    return [(n, w / total) for n, w in out]


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
            ebs[name].use_connect = (
                Vector(ebs[name].head) - Vector(ebs[parent].tail)).length < 1e-6
    bpy.ops.object.mode_set(mode='OBJECT')
    aob.show_in_front = True
    print("TESTCHAR|armature: %d bones" % len(arm.bones))
    return aob


def add_expression_shape_keys(mob, feats):
    """Basis + blink/happy/surprised POSITION-only keys on the face geometry."""
    me = mob.data
    while me.shape_keys is not None:
        mob.shape_key_clear()
        if me.shape_keys is None:
            break
    mob.shape_key_add(name="Basis", from_mix=False)
    keys = {name: mob.shape_key_add(name=name, from_mix=False) for name in EXPRESSIONS}
    # Blender 5.2 creates new shape keys with value=1.0; the rest state must be
    # zero or the glTF mesh.weights would be [1,1,1].
    for kb in keys.values():
        kb.value = 0.0
    base = [v.co.copy() for v in me.vertices]

    def eye_verts():
        return feats["eye_l"]["verts"] + feats["eye_r"]["verts"]

    # blink: squash each eye flat onto its lower lid, brows dip a touch
    for side in ("eye_l", "eye_r"):
        f = feats[side]
        for i in f["verts"]:
            d = base[i] - Vector((f["cx"], base[i].y, f["cz"]))
            keys["blink"].data[i].co = Vector((f["cx"] + d.x * 0.92, base[i].y,
                                               f["cz"] + d.z * 0.10))
    for side in ("brow_l", "brow_r"):
        for i in feats[side]["verts"]:
            keys["blink"].data[i].co = base[i] + Vector((0.0, 0.0, -0.0030))

    # happy: smiling eyes (squint + lift) and a wide open smile
    for side in ("eye_l", "eye_r"):
        f = feats[side]
        for i in f["verts"]:
            d = base[i] - Vector((f["cx"], base[i].y, f["cz"]))
            out = 1.0 + 0.06 * (d.x / max(1e-6, f["hw"])) ** 2
            keys["happy"].data[i].co = Vector((f["cx"] + d.x * out, base[i].y,
                                               f["cz"] + d.z * 0.40 + 0.002))
    for side in ("brow_l", "brow_r"):
        for i in feats[side]["verts"]:
            keys["happy"].data[i].co = base[i] + Vector((0.0, 0.0, 0.0035))
    f = feats["mouth"]
    for i in f["verts"]:
        d = base[i] - Vector((0.0, base[i].y, f["cz"]))
        t = abs(d.x) / MOUTH_HALF
        keys["happy"].data[i].co = Vector((d.x * 1.28, base[i].y,
                                           f["cz"] + d.z * 1.55 + 0.009 * t * t))

    # surprised: wide eyes, raised brows, round open mouth
    for side in ("eye_l", "eye_r"):
        f = feats[side]
        for i in f["verts"]:
            d = base[i] - Vector((f["cx"], base[i].y, f["cz"]))
            keys["surprised"].data[i].co = Vector((f["cx"] + d.x * 1.16, base[i].y,
                                                   f["cz"] + d.z * 1.14))
    for side in ("brow_l", "brow_r"):
        for i in feats[side]["verts"]:
            keys["surprised"].data[i].co = base[i] + Vector((0.0, 0.0, 0.0165))
    f = feats["mouth"]
    for i in f["verts"]:
        d = base[i] - Vector((0.0, base[i].y, f["cz"]))
        keys["surprised"].data[i].co = Vector((d.x * 0.58, base[i].y,
                                               f["cz"] + d.z * 1.85))

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
    builder, feats = build_geometry()

    bm = bmesh.new()
    bmverts = [bm.verts.new(co) for co in builder.verts]
    bm.verts.ensure_lookup_table()
    for idxs in builder.faces:
        bm.faces.new([bmverts[i] for i in idxs])
    bm.faces.ensure_lookup_table()
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces[:])
    bm.normal_update()
    # recalc cannot orient open shells / decal patches: restore the winding the
    # primitive was constructed with (outward for shells, -Y for face decals).
    restored = 0
    for fi in range(len(builder.faces)):
        if builder.fopen[fi]:
            if bm.faces[fi].normal.dot(builder.fnormals[fi]) < 0.0:
                bm.faces[fi].normal_flip()
                restored += 1
    bm.normal_update()
    if restored:
        print("TESTCHAR|restored %d open-shell faces to their constructed winding" % restored)
    me = bpy.data.meshes.new("TestCharBody")
    bm.to_mesh(me)
    bm.free()
    me.validate(verbose=False)
    assert len(me.polygons) == len(builder.faces), "face order/count changed"
    assert len(me.vertices) == len(builder.verts), "vertex order/count changed"

    uvl = me.uv_layers.new(name="UVMap")
    for poly in me.polygons:
        uvs = builder.fuvs[poly.index]
        region = builder.fregions[poly.index]
        for k, li in enumerate(poly.loop_indices):
            uvl.data[li].uv = uvs[k] if uvs is not None else region_uv(region, 0.5, 0.5)
        poly.use_smooth = builder.fsmooth[poly.index]
    me.update()

    mob = bpy.data.objects.new("TestCharBody", me)
    bpy.context.scene.collection.objects.link(mob)

    mat = bpy.data.materials.new("TestCharMaterial")
    mat.use_nodes = True
    nt = mat.node_tree
    bsdf = nt.nodes["Principled BSDF"]
    bsdf.inputs["Roughness"].default_value = 0.72
    bsdf.inputs["Metallic"].default_value = 0.0
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = atlas_image
    tex.location = (-320, 260)
    nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    me.materials.append(mat)

    add_expression_shape_keys(mob, feats)

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
    print("TESTCHAR|skin weights: influences/vertex histogram=%s (max 4)"
          % (dict(sorted(hist.items())),))

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
        export_image_format='AUTO',      # Blender 5.x: PNG has no explicit enum
        export_yup=True,
        export_def_bones=False,
        export_leaf_bone=False,
        export_rest_position_armature=True,
        export_extras=False,
        export_cameras=False,
        export_lights=False,
        export_optimize_animation_size=False,
        # facial morph targets: dense POSITION-only (see README)
        export_morph=True,
        export_morph_normal=False,
        export_morph_tangent=False,
        export_morph_animation=False,
        export_morph_reset_sk_data=False,
        export_try_sparse_sk=False,
    )
    print("TESTCHAR|exported %s (%d bytes)" % (path, os.path.getsize(path)))


EXPRESSIONS = ("blink", "happy", "surprised")

# --------------------------------------------------------------------------- poses
def apply_pose(arm_ob, pose):
    for bone, rots in pose.items():
        pb = arm_ob.pose.bones[bone]
        R = Matrix.Identity(3)
        for axis, ang in rots:            # first entry applied first
            R = Matrix.Rotation(ang, 3, axis) @ R
        M = pb.bone.matrix_local.to_3x3()
        pb.rotation_mode = 'QUATERNION'
        pb.rotation_quaternion = (M.inverted() @ R @ M).to_quaternion()


POSES = [
    ("T-POSE", {}),
    ("WAVE", {
        "leftUpperArm": [("Y", -1.18), ("Z", -0.10)],
        "leftLowerArm": [("Y", -0.42)],
        "leftHand": [("Y", -0.12)],
        "rightUpperArm": [("Y", -1.12)],
        "rightLowerArm": [("X", -0.10)],
        "head": [("Y", 0.10)],
        "spine": [("Y", -0.03)],
    }),
    ("WALK", {
        "leftUpperLeg": [("X", -0.52)],
        "leftLowerLeg": [("X", 0.44)],
        "leftFoot": [("X", -0.16)],
        "rightUpperLeg": [("X", 0.42)],
        "rightLowerLeg": [("X", 0.14)],
        "leftUpperArm": [("Y", 1.12), ("X", -0.40)],
        "leftLowerArm": [("X", -0.28)],
        "rightUpperArm": [("Y", -1.12), ("X", 0.40)],
        "rightLowerArm": [("X", 0.16)],
        "spine": [("X", 0.05)],
        "head": [("X", -0.05)],
    }),
    ("KICK", {
        "rightUpperLeg": [("Y", 0.95), ("X", -0.22)],
        "rightLowerLeg": [("X", 0.32)],
        "leftUpperLeg": [("X", 0.04)],
        "leftUpperArm": [("Y", 0.55)],
        "rightUpperArm": [("Y", -0.55)],
        "leftLowerArm": [("Y", 0.25)],
        "head": [("Y", -0.06)],
        "spine": [("Y", 0.05)],
    }),
]


# --------------------------------------------------------------------------- render
def _clear_studio():
    """Drop lights/cameras/studio meshes but keep the character itself."""
    for ob in list(bpy.context.scene.objects):
        if ob.name in ("TestCharRig", "TestCharBody"):
            continue
        try:
            bpy.data.objects.remove(ob, do_unlink=True)
        except Exception:
            pass


def _studio_world(scene, top=(0.52, 0.56, 0.66), bottom=(0.20, 0.22, 0.28)):
    world = bpy.data.worlds.new("Studio")
    world.use_nodes = True
    bg = world.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = (0.42, 0.45, 0.52, 1.0)
    bg.inputs["Strength"].default_value = 1.0
    scene.world = world


def _sun(name, toward_light, energy):
    li = bpy.data.lights.new(name, type='SUN')
    li.energy = energy
    li.angle = math.radians(14)
    ob = bpy.data.objects.new(name, li)
    ob.rotation_euler = Vector(toward_light).normalized().to_track_quat('Z', 'Y').to_euler()
    bpy.context.scene.collection.objects.link(ob)
    return ob


def _studio_lights(scene, key=3.6, fill=1.15, rim=2.6):
    # key from the character's front-upper-left (the runtime's light direction),
    # cool fill from the opposite side and a warm rim from behind.
    _sun("Key", (0.42, -0.72, 0.75), key)
    _sun("Fill", (-0.65, -0.45, 0.30), fill)
    _sun("Rim", (0.10, 0.85, 0.35), rim)
    return None


def _add_ground(scene):
    me = bpy.data.meshes.new("Floor")
    me.from_pydata([(-14, -14, 0), (14, -14, 0), (14, 4.0, 0), (-14, 4.0, 0)], [],
                   [(0, 1, 2, 3)])
    mat = _flat_mat("FloorMat", (0.63, 0.65, 0.70), 1.0)
    me.materials.append(mat)
    ob = bpy.data.objects.new("Floor", me)
    scene.collection.objects.link(ob)
    return ob


def _add_backdrop(scene, y=4.0, z0=-1.0, z1=13.0, rgb=(0.72, 0.75, 0.82)):
    me = bpy.data.meshes.new("Backdrop")
    me.from_pydata([(-16, y, z0), (16, y, z0), (16, y, z1), (-16, y, z1)], [],
                   [(0, 1, 2, 3)])
    me.materials.append(_flat_mat("BackdropMat", rgb, 1.0))
    ob = bpy.data.objects.new("Backdrop", me)
    scene.collection.objects.link(ob)
    return ob


def _flat_mat(name, rgb, roughness=0.9):
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (rgb[0], rgb[1], rgb[2], 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    mat.diffuse_color = (rgb[0], rgb[1], rgb[2], 1.0)
    return mat


def _camera(scene, name, loc, target, lens=70.0, sensor=24.0):
    cd = bpy.data.cameras.new(name)
    cd.lens = lens
    cd.sensor_fit = 'VERTICAL'
    cd.sensor_height = sensor
    cam = bpy.data.objects.new(name, cd)
    cam.location = loc
    d = Vector(target) - Vector(loc)
    cam.rotation_euler = d.to_track_quat('-Z', 'Y').to_euler()
    scene.collection.objects.link(cam)
    return cam


def _render_setup(scene, rx, ry, samples=48, transparent=False):
    scene.render.engine = 'BLENDER_EEVEE'
    try:
        scene.eevee.taa_render_samples = samples
    except Exception:
        pass
    try:
        scene.view_settings.view_transform = 'Standard'
        scene.view_settings.look = 'None'
    except Exception:
        pass
    scene.render.resolution_x, scene.render.resolution_y = rx, ry
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = transparent
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGBA' if transparent else 'RGB'
    scene.render.image_settings.color_depth = '8'
    scene.render.image_settings.compression = 15


def _render_to(scene, cam, path, rx, ry, samples=48, transparent=False):
    _render_setup(scene, rx, ry, samples, transparent)
    scene.camera = cam
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    scene.render.filepath = os.path.abspath(path)
    bpy.ops.render.render(write_still=True)
    size = os.path.getsize(path) if os.path.isfile(path) else 0
    print("TESTCHAR|render %dx%d -> %s (%d bytes)" % (rx, ry, path, size))
    return size


def _add_label(text, x, z, material, size=0.11):
    cu = bpy.data.curves.new("label_%s" % text, type='FONT')
    cu.body = text
    cu.align_x = 'CENTER'
    cu.size = size
    cu.extrude = 0.002
    ob = bpy.data.objects.new("label_%s" % text, cu)
    ob.location = (x, -0.2, z)
    ob.rotation_euler = (math.radians(90), 0, 0)
    ob.data.materials.append(material)
    bpy.context.scene.collection.objects.link(ob)
    return ob


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


def _load_png_pixels(path):
    img = bpy.data.images.load(path, check_existing=False)
    w, h = img.size
    px = [0.0] * (w * h * 4)
    img.pixels.foreach_get(px)
    bpy.data.images.remove(img)
    return w, h, px


def _save_png_pixels(path, w, h, px):
    img = bpy.data.images.new("composite", w, h, alpha=False)
    img.pixels.foreach_set(px)
    img.filepath_raw = os.path.abspath(path)
    img.file_format = 'PNG'
    img.save()
    bpy.data.images.remove(img)
    print("TESTCHAR|composited %dx%d -> %s (%d bytes)"
          % (w, h, path, os.path.getsize(path)))


def compose_panels(out_path, size, panels, bg_top=(0.90, 0.91, 0.94),
                   bg_bottom=(0.66, 0.69, 0.76), shadows=()):
    """Alpha-composite transparent renders onto a soft vertical gradient.

    `shadows` = [(cx, cy_from_bottom, rx, ry, strength)] soft ellipses painted
    before the panels so the character reads as standing on something.
    """
    W, H = size
    buf = [0.0] * (W * H * 4)
    top = [_srgb_to_linear(int(round(c * 255.0))) for c in bg_top]
    bot = [_srgb_to_linear(int(round(c * 255.0))) for c in bg_bottom]
    for y in range(H):
        t = y / max(1.0, float(H - 1))          # y = 0 is the bottom row
        r = bot[0] + (top[0] - bot[0]) * t
        g = bot[1] + (top[1] - bot[1]) * t
        b = bot[2] + (top[2] - bot[2]) * t
        for x in range(W):
            i = (y * W + x) * 4
            buf[i], buf[i + 1], buf[i + 2], buf[i + 3] = r, g, b, 1.0
    for (cx, cy, rx, ry, strength) in shadows:
        for y in range(max(0, int(cy - ry)), min(H, int(cy + ry)) + 1):
            for x in range(max(0, int(cx - rx)), min(W, int(cx + rx)) + 1):
                dx = (x - cx) / max(1.0, rx)
                dy = (y - cy) / max(1.0, ry)
                d = dx * dx + dy * dy
                if d >= 1.0:
                    continue
                a = strength * (1.0 - d) ** 1.5
                i = (y * W + x) * 4
                for c in range(3):
                    buf[i + c] *= (1.0 - a)
    for (path, px0, py0) in panels:
        pw, ph, pp = _load_png_pixels(path)
        for y in range(ph):
            dy = py0 + y
            if not (0 <= dy < H):
                continue
            for x in range(pw):
                dx = px0 + x
                if not (0 <= dx < W):
                    continue
                si = (y * pw + x) * 4
                di = (dy * W + dx) * 4
                a = pp[si + 3]
                if a <= 0.0:
                    continue
                for c in range(3):
                    buf[di + c] = pp[si + c] + buf[di + c] * (1.0 - a)
    _save_png_pixels(out_path, W, H, buf)


CONTACT_RES = (1920, 700)
CONTACT_ORTHO = 2.38          # vertical world extent (ortho_scale on height)
CONTACT_CAM_Z = 0.86
CONTACT_SPACING = 1.62        # must exceed the T-pose arm span


def render_contact_sheet(arm_ob, mesh_ob, out_path, workdir):
    scene = bpy.context.scene
    _clear_studio()
    arm_ob.location = (0.0, 0.0, 0.0)
    apply_pose(arm_ob, {})
    spacing = CONTACT_SPACING
    xs = [-1.5 * spacing, -0.5 * spacing, 0.5 * spacing, 1.5 * spacing]
    for i, (label, pose) in enumerate(POSES):
        if i == 0:
            a, m = arm_ob, mesh_ob
        else:
            a, m = copy_rig(arm_ob, mesh_ob, 0.0)
        a.location = (xs[i], 0.0, 0.0)
        apply_pose(a, pose)
        _add_label(label, xs[i], 1.80,
                   _flat_mat("LabelMat_%d" % i, (0.09, 0.10, 0.14)), size=0.095)
    bpy.context.view_layer.update()
    _studio_lights(scene, 3.4, 1.25, 2.5)
    _studio_world(scene)
    cam = _camera(scene, "ContactCam", (0.0, -9.0, CONTACT_CAM_Z), (0.0, 0.0, CONTACT_CAM_Z))
    cam.data.type = 'ORTHO'
    cam.data.ortho_scale = CONTACT_ORTHO
    cam.rotation_euler = (math.radians(90), 0.0, 0.0)
    raw = os.path.join(workdir, "contact_raw.png")
    _render_to(scene, cam, raw, CONTACT_RES[0], CONTACT_RES[1], 48, True)
    scale = CONTACT_RES[1] / CONTACT_ORTHO
    # y=0 in the composite buffer is the bottom row: z=0 (the feet) lands low
    cy = (CONTACT_ORTHO * 0.5 - CONTACT_CAM_Z + 0.02) * scale
    shadows = [(CONTACT_RES[0] * 0.5 + x * scale, cy, 0.30 * scale, 0.075 * scale, 0.38)
               for x in xs]
    compose_panels(out_path, CONTACT_RES, [(raw, 0, 0)], shadows=shadows)
    arm_ob.location = (0.0, 0.0, 0.0)
    apply_pose(arm_ob, {})
    print("TESTCHAR|contact sheet written: %s (%d bytes)"
          % (out_path, os.path.getsize(out_path)))
    return os.path.getsize(out_path)


def render_beauty(arm_ob, mesh_ob, out_path, workdir):
    """Front / three-quarter / face close-up, composed into one sheet."""
    scene = bpy.context.scene
    _clear_studio()
    arm_ob.location = (0.0, 0.0, 0.0)
    apply_pose(arm_ob, {})
    _studio_lights(scene, 3.9, 1.30, 2.9)
    _studio_world(scene)
    _add_ground(scene)
    _add_backdrop(scene)
    panels = []
    shots = [
        ("beauty_front.png", (0.0, -5.4, 1.22), (0.0, 0.0, 0.80), 70.0),
        ("beauty_quarter.png", (2.86, -4.57, 1.26), (0.0, 0.0, 0.82), 70.0),
        ("beauty_face.png", (0.62, -1.72, 1.40), (0.0, 0.0, 1.30), 82.0),
    ]
    for (fname, loc, tgt, lens) in shots:
        p = os.path.join(workdir, fname)
        cam = _camera(scene, "BeautyCam", loc, tgt, lens=lens)
        _render_to(scene, cam, p, 720, 960, 64, False)
        panels.append(p)
        bpy.data.objects.remove(cam, do_unlink=True)
    compose_panels(out_path, (2160, 960),
                   [(panels[0], 0, 0), (panels[1], 720, 0), (panels[2], 1440, 0)],
                   bg_top=(0.78, 0.80, 0.85), bg_bottom=(0.60, 0.63, 0.70))
    print("TESTCHAR|beauty sheet written: %s (%d bytes)"
          % (out_path, os.path.getsize(out_path)))
    return os.path.getsize(out_path)


def render_expression_proof(arm_ob, mesh_ob, out_path, res=(1600, 500)):
    scene = bpy.context.scene
    spacing = 0.66
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
        created.append(_add_label(label, xs[i], 1.70,
                                  _flat_mat("ExprLabelMat_%d" % i, (0.07, 0.08, 0.11)),
                                  size=0.070))
    mesh_ob.hide_render = True
    bpy.context.view_layer.update()
    _studio_lights(scene, 3.6, 1.2, 2.2)
    _studio_world(scene)
    cam = _camera(scene, "ExprCam", (0.0, -8.0, 1.30), (0.0, 0.0, 1.30), lens=50.0)
    cam.data.type = 'ORTHO'
    cam.data.ortho_scale = 0.92
    cam.rotation_euler = (math.radians(90), 0.0, 0.0)
    _render_to(scene, cam, out_path, res[0], res[1], 48, False)
    for ob in created:
        try:
            bpy.data.objects.remove(ob, do_unlink=True)
        except Exception:
            pass
    mesh_ob.hide_render = False
    scene.camera = None
    return os.path.getsize(out_path)


# --------------------------------------------------------------------------- app preview
RUNTIME_LIGHT = Vector((0.35, -0.70, 0.62)).normalized()   # glTF (0.35,0.62,0.70)


def _runtime_material(atlas_png):
    """Emission material reproducing the KimiOS rasteriser's shading:
       out = texel * (0.80 + 0.55*max(N.L,0) + 0.38*(1-|N.V|)^2)."""
    mat = bpy.data.materials.new("RuntimeShade")
    mat.use_nodes = True
    nt = mat.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    out.location = (600, 0)
    em = nt.nodes.new("ShaderNodeEmission")
    em.location = (400, 0)
    nt.links.new(em.outputs["Emission"], out.inputs["Surface"])

    img = bpy.data.images.load(atlas_png, check_existing=False)
    img.colorspace_settings.name = 'Non-Color'
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = img
    tex.interpolation = 'Closest'
    tex.extension = 'EXTEND'
    tex.location = (-600, 200)

    geo = nt.nodes.new("ShaderNodeNewGeometry")
    geo.location = (-600, -100)

    def dotnode(a, target, x, y):
        n = nt.nodes.new("ShaderNodeVectorMath")
        n.operation = 'DOT_PRODUCT'
        n.location = (x, y)
        nt.links.new(a, n.inputs[0])
        if isinstance(target, (tuple, list)):
            n.inputs[1].default_value = target
        else:
            nt.links.new(target, n.inputs[1])
        return n.outputs["Value"]

    def mathnode(op, x, y, a, b):
        n = nt.nodes.new("ShaderNodeMath")
        n.operation = op
        n.location = (x, y)
        for idx, v in ((0, a), (1, b)):
            if isinstance(v, (int, float)):
                n.inputs[idx].default_value = v
            else:
                nt.links.new(v, n.inputs[idx])
        return n.outputs[0]

    ndl = dotnode(geo.outputs["Normal"], tuple(RUNTIME_LIGHT), -380, -60)
    ndv = dotnode(geo.outputs["Normal"], geo.outputs["Incoming"], -380, -200)
    ndl_pos = mathnode('MAXIMUM', -200, -60, ndl, 0.0)
    abs_ndv = mathnode('ABSOLUTE', -200, -200, ndv, 0.0)
    rim = mathnode('SUBTRACT', -20, -200, 1.0, abs_ndv)
    rim2 = mathnode('POWER', 140, -200, rim, 2.0)
    t1 = mathnode('MULTIPLY', 300, -60, ndl_pos, 0.55)
    t2 = mathnode('MULTIPLY', 300, -200, rim2, 0.38)
    s1 = mathnode('ADD', 440, -120, t1, t2)
    shade = mathnode('ADD', 560, -120, s1, 0.80)

    comb = nt.nodes.new("ShaderNodeCombineXYZ")
    comb.location = (700, -120)
    for sock in ("X", "Y", "Z"):
        nt.links.new(shade, comb.inputs[sock])
    mul = nt.nodes.new("ShaderNodeVectorMath")
    mul.operation = 'MULTIPLY'
    mul.location = (860, 60)
    nt.links.new(tex.outputs["Color"], mul.inputs[0])
    nt.links.new(comb.outputs["Vector"], mul.inputs[1])
    nt.links.new(mul.outputs["Vector"], em.inputs["Color"])
    return mat


def _app_framing(mesh_ob):
    """Mirror pet3d::render_frame: fit the rest bbox's bounding sphere, 0.62 rad
    vertical FOV at 2.2x the half-diagonal. Returns (centre, distance, fov)."""
    co = [v.co for v in mesh_ob.data.vertices]
    mn = Vector((min(c.x for c in co), min(c.z for c in co), -max(c.y for c in co)))
    mx = Vector((max(c.x for c in co), max(c.z for c in co), -min(c.y for c in co)))
    centre = (mn + mx) * 0.5
    extent = max(0.5, (mx - mn).length * 0.5)
    return centre, extent * 2.2, 0.62, (mx - mn)


def _downscale_box(px, w, h, nw, nh):
    out = [0.0] * (nw * nh * 4)
    for y in range(nh):
        y0 = int(y * h / nh)
        y1 = max(y0 + 1, int((y + 1) * h / nh))
        for x in range(nw):
            x0 = int(x * w / nw)
            x1 = max(x0 + 1, int((x + 1) * w / nw))
            acc = [0.0, 0.0, 0.0, 0.0]
            n = 0
            for yy in range(y0, min(y1, h)):
                for xx in range(x0, min(x1, w)):
                    i = (yy * w + xx) * 4
                    acc[0] += px[i]
                    acc[1] += px[i + 1]
                    acc[2] += px[i + 2]
                    acc[3] += px[i + 3]
                    n += 1
            j = (y * nw + x) * 4
            for c in range(4):
                out[j + c] = acc[c] / max(1, n)
    return out


def render_app_preview(mesh_ob, atlas_png, out_path, workdir, rt=(384, 512)):
    """Render exactly what the KimiOS rasteriser draws (shading model, 384x512
    target), emulate the cel outline, then downscale to phone sizes."""
    scene = bpy.context.scene
    _clear_studio()
    _studio_world(scene)
    mat = _runtime_material(atlas_png)
    mesh_ob.data.materials.clear()
    mesh_ob.data.materials.append(mat)
    # keep the skin visible in this shot: neutral pose
    if mesh_ob.parent is not None:
        apply_pose(mesh_ob.parent, {})
    bpy.context.view_layer.update()

    centre, dist, fov, size = _app_framing(mesh_ob)
    # glTF Y-up -> Blender Z-up: blender = (x, -z, y)
    cam_loc = Vector((0.0, -dist, centre.y + 0.02 * dist))
    cam = _camera(scene, "AppCam", cam_loc, (0.0, 0.0, centre.y), lens=50.0)
    cam.data.type = 'PERSP'
    cam.data.sensor_fit = 'VERTICAL'
    cam.data.angle = fov          # vertical FOV (sensor_fit = VERTICAL)
    try:
        scene.view_settings.view_transform = 'Raw'
    except Exception:
        pass
    rt_path = os.path.join(workdir, "app_rt.png")
    _render_to(scene, cam, rt_path, rt[0], rt[1], 32, True)
    scene.view_settings.view_transform = 'Standard'
    print("TESTCHAR|app framing: bbox %.3f x %.3f x %.3f, extent=%.3f, cam dist=%.3f"
          % (size.x, size.y, size.z, (size.length * 0.5), dist))

    # Raw view transform + Non-Color image => pixel values are texel*shade, i.e.
    # exactly the bytes the software rasteriser would write.
    raw = bpy.data.images.load(rt_path, check_existing=False)
    raw.colorspace_settings.name = 'Non-Color'
    w, h = raw.size
    rpx = [0.0] * (w * h * 4)
    raw.pixels.foreach_get(rpx)
    bpy.data.images.remove(raw)

    # ---- cel outline: the runtime draws an inverted hull in 0xb8c2d8 before the
    # model, so emulate it by dilating the silhouette mask by ~2 px.
    oc = (0xb8 / 255.0, 0xc2 / 255.0, 0xd8 / 255.0)
    outl = [0.0] * (w * h * 4)
    for y in range(h):
        for x in range(w):
            i = (y * w + x) * 4
            a = rpx[i + 3]
            if a > 0.02:
                outl[i] = rpx[i]
                outl[i + 1] = rpx[i + 1]
                outl[i + 2] = rpx[i + 2]
                outl[i + 3] = a
                continue
            hit = False
            for dy in (-2, -1, 1, 2):
                yy = y + dy
                if yy < 0 or yy >= h:
                    continue
                for dx in (-2, -1, 1, 2):
                    xx = x + dx
                    if xx < 0 or xx >= w:
                        continue
                    if rpx[(yy * w + xx) * 4 + 3] > 0.02:
                        hit = True
                        break
                if hit:
                    break
            if hit:
                outl[i], outl[i + 1], outl[i + 2], outl[i + 3] = oc[0], oc[1], oc[2], 1.0

    def build(scale_h, bg):
        nw = max(40, int(round(w * scale_h / float(h))))
        nh = scale_h
        small = _downscale_box(outl, w, h, nw, nh)
        out = [0.0] * (nw * nh * 4)
        for i in range(nw * nh):
            a = small[i * 4 + 3]
            for c in range(3):
                out[i * 4 + c] = small[i * 4 + c] * a + bg[c] * (1.0 - a)
            out[i * 4 + 3] = 1.0
        return nw, nh, out

    # full-size panel on a light backdrop
    full_w, full_h, full = build(h, (0.86, 0.88, 0.92))
    # phone-scale strip on a light and a dark backdrop
    smalls = []
    for sh in (190, 150, 120):
        smalls.append(build(sh, (0.86, 0.88, 0.92)))
    smalls_dark = [build(sh, (0.16, 0.18, 0.24)) for sh in (190, 150, 120)]

    gap = 14
    strip_w = sum(s[0] for s in smalls) + gap * (len(smalls) + 1)
    strip_h = max(s[1] for s in smalls) + gap * 2
    W = full_w + gap + strip_w
    H = max(full_h, strip_h * 2 + gap)
    buf = [0.0] * (W * H * 4)
    for i in range(W * H):
        buf[i * 4 + 0] = 0.20
        buf[i * 4 + 1] = 0.22
        buf[i * 4 + 2] = 0.27
        buf[i * 4 + 3] = 1.0

    def blit(px_, pw, ph, x0, y0):
        for y in range(ph):
            for x in range(pw):
                si = (y * pw + x) * 4
                di = ((y0 + y) * W + (x0 + x)) * 4
                buf[di] = px_[si]
                buf[di + 1] = px_[si + 1]
                buf[di + 2] = px_[si + 2]
                buf[di + 3] = 1.0

    blit(full, full_w, full_h, 0, 0)
    x = full_w + gap
    for s in smalls:
        blit(s[2], s[0], s[1], x, gap)
        x += s[0] + gap
    x = full_w + gap
    for s in smalls_dark:
        blit(s[2], s[0], s[1], x, strip_h + gap)
        x += s[0] + gap
    _save_png_pixels(out_path, W, H, buf)
    # restore the texture material for any later render
    mesh_ob.data.materials.clear()
    atlas_img = bpy.data.images.get("testchar_basecolor")
    if atlas_img is not None:
        m2 = bpy.data.materials.get("TestCharMaterial")
        if m2 is not None:
            mesh_ob.data.materials.append(m2)
    return os.path.getsize(out_path)


# --------------------------------------------------------------------------- main
def main(argv):
    ap = argparse.ArgumentParser(prog="make_test_character")
    ap.add_argument("--out", default=os.path.join(REPO, "assets", "models", "testchar.glb"))
    ap.add_argument("--contact",
                    default=os.path.join(REPO, "assets", "models", "testchar_contact.png"))
    ap.add_argument("--beauty",
                    default=os.path.join(REPO, "assets", "models", "testchar_beauty.png"))
    ap.add_argument("--app-preview",
                    default=os.path.join(REPO, "assets", "models", "testchar_app.png"))
    ap.add_argument("--expressions", default=None,
                    help="optional 4-panel shape-key proof render (PNG path)")
    ap.add_argument("--tex-size", type=int, default=256)
    ap.add_argument("--no-contact", action="store_true")
    ap.add_argument("--no-beauty", action="store_true")
    ap.add_argument("--no-app-preview", action="store_true")
    ap.add_argument("--save-blend", default=None)
    args = ap.parse_args(argv)
    if args.tex_size > 256 or args.tex_size % 8:
        raise SystemExit("--tex-size must be a multiple of 8 and <= 256")

    reset_scene()
    work = os.path.join(HERE, "_work")
    os.makedirs(work, exist_ok=True)
    atlas_png = os.path.join(work, "testchar_basecolor.png")
    atlas = make_atlas(args.tex_size, atlas_png)
    arm_ob, mesh_ob = build_character(atlas)
    export_glb(args.out)

    rep = verify_glb.audit(args.out, expect_morphs=EXPRESSIONS)   # prints the audit

    exp_size = None
    if args.expressions:
        exp_size = render_expression_proof(arm_ob, mesh_ob, args.expressions)
    contact_size = None
    if not args.no_contact:
        contact_size = render_contact_sheet(arm_ob, mesh_ob, args.contact, work)
    beauty_size = None
    if not args.no_beauty:
        beauty_size = render_beauty(arm_ob, mesh_ob, args.beauty, work)
    app_size = None
    if not args.no_app_preview:
        try:
            app_size = render_app_preview(mesh_ob, atlas_png, args.app_preview, work)
        except Exception as exc:      # preview is diagnostic: never fail the build
            import traceback
            traceback.print_exc()
            print("TESTCHAR|WARN app preview skipped: %r" % (exc,))
    if args.save_blend:
        bpy.ops.wm.save_as_mainfile(filepath=os.path.abspath(args.save_blend))
        print("TESTCHAR|blend saved %s" % args.save_blend)

    summary = {
        "glb": os.path.abspath(args.out),
        "glb_bytes": os.path.getsize(args.out),
        "triangles": rep["triangles"],
        "vertices": rep["vertices"],
        "texture": rep["images"][0]["png"] if rep["images"] else None,
        "humanoid_bones": "%d/%d" % (len(rep["humanoid_bones_found"]),
                                     len(verify_glb.HUMANOID_BONES)),
        "max_influences": rep["max_influences"],
        "joints": rep["skins"][0]["joint_count"] if rep["skins"] else 0,
        "morph_targets": rep["morph"]["count"],
        "morph_names": rep["morph"]["names"],
        "morph_deltas": [t.get("delta_count") for t in rep["morph"]["targets"]],
        "expressions": None if not args.expressions else os.path.abspath(args.expressions),
        "expressions_bytes": exp_size,
        "contact": None if args.no_contact else os.path.abspath(args.contact),
        "contact_bytes": contact_size,
        "beauty": None if args.no_beauty else os.path.abspath(args.beauty),
        "beauty_bytes": beauty_size,
        "app_preview": None if args.no_app_preview else os.path.abspath(args.app_preview),
        "app_preview_bytes": app_size,
        "audit_ok": rep["ok"],
    }
    print("TESTCHAR|SUMMARY " + json.dumps(summary, sort_keys=True))
    if not rep["ok"]:
        return 1
    return 0


if __name__ == "__main__":
    av = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    sys.exit(main(av))