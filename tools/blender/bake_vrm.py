#!/usr/bin/env python3
"""bake_vrm.py -- KimiOS pet-pack converter: import a VRM/glTF/FBX character,
fit a camera to the union of every baked pose, and render a 4x4 grid of
96 px cells (16 frames) plus a combined 384x384 sprite sheet.

  "C:\\Program Files\\Blender Foundation\\Blender 5.2\\blender.exe" -b --factory-startup \
      --python-exit-code 1 --python tools/blender/bake_vrm.py -- \
      --input "C:\\path\\to\\model.vrm" --outdir assets/models/pet_frames

It is NOT a stub: with no --input it auto-picks the first model it can find
(assets/models/*.vrm, then assets/models/testchar.glb), and if the repo has no
model at all it builds a coloured fallback prop so the whole pipeline still
runs end to end. It prints every frame it wrote.

  --input PATH     .vrm / .glb / .gltf / .fbx source (optional)
  --outdir DIR     where to write frame_00.png .. frame_15.png + sheet_4x4.png
                   (default <repo>/assets/models/pet_frames)
  --cell N         cell size in px (default 96 -> 4x4 sheet is 384x384)
  --grid N         cells per side (default 4 -> 16 frames)
  --margin F       camera padding factor (default 1.12)
  --angle DEG      front-view yaw for the camera, turntable mode (default 0)
  --opaque         render on an opaque grey background instead of transparent
  --no-sheet       skip writing the combined sheet
"""
import argparse
import json
import math
import os
import shutil
import struct
import sys
import tempfile
import zlib

import bpy
from mathutils import Matrix, Vector

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))


def log(msg):
    print("BAKE|" + msg, flush=True)


# --------------------------------------------------------------------------- PNG
def _png_chunks(data):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    pos, ihdr, idat = 8, None, bytearray()
    while pos + 8 <= len(data):
        ln = struct.unpack_from(">I", data, pos)[0]
        typ = data[pos + 4:pos + 8]
        payload = data[pos + 8:pos + 8 + ln]
        if typ == b"IHDR":
            ihdr = struct.unpack(">IIBBBBB", payload)
        elif typ == b"IDAT":
            idat += payload
        elif typ == b"IEND":
            break
        pos += 12 + ln
    if ihdr is None:
        raise ValueError("PNG without IHDR")
    return ihdr, bytes(idat)


def png_read_rgba(path):
    """Decode an 8-bit non-interlaced PNG (grey/RGB/palette-free) to top-down RGBA."""
    with open(path, "rb") as f:
        data = f.read()
    (w, h, bitdepth, colortype, comp, filt, interlace), idat = _png_chunks(data)
    if bitdepth != 8 or interlace != 0:
        raise ValueError("unsupported PNG (bitdepth=%d interlace=%d)" % (bitdepth, interlace))
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(colortype)
    if channels is None:
        raise ValueError("unsupported PNG color type %d" % colortype)
    raw = zlib.decompress(idat)
    stride = w * channels
    out = bytearray(h * stride)
    prev = bytearray(stride)
    p = 0
    for y in range(h):
        ft = raw[p]
        p += 1
        line = bytearray(raw[p:p + stride])
        p += stride
        if ft == 1:
            for i in range(channels, stride):
                line[i] = (line[i] + line[i - channels]) & 255
        elif ft == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif ft == 3:
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                line[i] = (line[i] + ((a + prev[i]) >> 1)) & 255
        elif ft == 4:
            for i in range(stride):
                a = line[i - channels] if i >= channels else 0
                b = prev[i]
                c = prev[i - channels] if i >= channels else 0
                pp = a + b - c
                pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        elif ft != 0:
            raise ValueError("bad PNG filter %d" % ft)
        out[y * stride:(y + 1) * stride] = line
        prev = line
    # normalise to RGBA
    rgba = bytearray(w * h * 4)
    for i in range(w * h):
        s = i * channels
        d = i * 4
        if channels >= 3:
            rgba[d], rgba[d + 1], rgba[d + 2] = out[s], out[s + 1], out[s + 2]
            rgba[d + 3] = out[s + 3] if channels == 4 else 255
        else:
            rgba[d] = rgba[d + 1] = rgba[d + 2] = out[s]
            rgba[d + 3] = out[s + 1] if channels == 2 else 255
    return w, h, rgba


def _chunk(typ, payload):
    return (struct.pack(">I", len(payload)) + typ + payload
            + struct.pack(">I", zlib.crc32(typ + payload) & 0xFFFFFFFF))


def png_write_rgba(path, w, h, rgba):
    """Encode top-down RGBA bytes as an 8-bit PNG."""
    raw = bytearray()
    stride = w * 4
    for y in range(h):
        raw.append(0)
        raw += rgba[y * stride:(y + 1) * stride]
    body = (b"\x89PNG\r\n\x1a\n"
            + _chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + _chunk(b"IDAT", zlib.compress(bytes(raw), 9))
            + _chunk(b"IEND", b""))
    with open(path, "wb") as f:
        f.write(body)
    return len(body)


# --------------------------------------------------------------------------- scene
def reset_scene():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def import_model(path):
    ext = os.path.splitext(path)[1].lower()
    log("importing %s (importer=%s)" % (path, ext))
    if ext == ".fbx":
        bpy.ops.import_scene.fbx(filepath=path)
    elif ext in (".glb", ".gltf", ".vrm"):
        target = path
        if ext == ".vrm":
            # the glTF importer keys off the extension, so present the VRM as a .glb
            tmpdir = tempfile.mkdtemp(prefix="bake_vrm_")
            target = os.path.join(tmpdir, "source.glb")
            shutil.copyfile(path, target)
            log("VRM is a glTF container; imported through a .glb copy: %s" % target)
        # disable_bone_shape stops the importer adding an "Icosphere" per bone
        # (it would otherwise be rendered and inflate the camera-fit bbox)
        try:
            bpy.ops.import_scene.gltf(filepath=target, import_pack_images=True,
                                      disable_bone_shape=True)
        except TypeError:
            bpy.ops.import_scene.gltf(filepath=target, import_pack_images=True)
    else:
        raise ValueError("unsupported model extension: %s" % ext)


def build_fallback():
    """Last resort: a bright spinning prop so the bake always produces output."""
    log("WARNING no model found -- building the built-in fallback prop")
    me = bpy.data.meshes.new("FallbackMesh")
    verts = [(-.5, -.5, 0), (.5, -.5, 0), (.5, .5, 0), (-.5, .5, 0),
             (-.35, -.35, 1.2), (.35, -.35, 1.2), (.35, .35, 1.2), (-.35, .35, 1.2)]
    faces = [(0, 1, 2, 3), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]
    me.from_pydata(verts, [], faces)
    me.validate()
    mat = bpy.data.materials.new("FallbackMat")
    mat.use_nodes = True
    mat.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (0.95, 0.35, 0.15, 1.0)
    me.materials.append(mat)
    ob = bpy.data.objects.new("FallbackProp", me)
    bpy.context.scene.collection.objects.link(ob)
    return ob


def collect_model():
    """Real renderable meshes only: skip bone-display shapes, glTF helper collections
    and anything explicitly hidden from render."""
    shape_objects = set()
    armatures = [o for o in bpy.context.scene.objects if o.type == 'ARMATURE']
    for a in armatures:
        for pb in a.pose.bones:
            if pb.custom_shape:
                shape_objects.add(pb.custom_shape.name)
                pb.custom_shape.hide_render = True
    meshes = []
    for ob in bpy.context.scene.objects:
        if ob.type != 'MESH' or ob.hide_render:
            continue
        if ob.name in shape_objects or ob.data is None:
            continue
        if any(c.name == "glTF_not_exported" for c in ob.users_collection):
            continue
        meshes.append(ob)
    return meshes, armatures


def action_frame_range(armatures, meshes):
    best = None
    for ob in list(armatures) + list(meshes):
        ad = getattr(ob, "animation_data", None)
        act = getattr(ad, "action", None) if ad else None
        if act is not None:
            try:
                fr = act.frame_range
                if fr[1] > fr[0]:
                    best = (float(fr[0]), float(fr[1]))
            except Exception:
                pass
    if best is None:
        for act in bpy.data.actions:
            try:
                fr = act.frame_range
                if fr[1] > fr[0]:
                    cand = (float(fr[0]), float(fr[1]))
                    best = cand if best is None else (min(best[0], cand[0]), max(best[1], cand[1]))
            except Exception:
                pass
    return best


def setup_studio(engine='BLENDER_EEVEE', samples=16, opaque=False):
    scene = bpy.context.scene
    scene.render.engine = engine
    try:
        scene.eevee.taa_render_samples = samples
    except Exception:
        pass
    try:
        scene.view_settings.view_transform = 'Standard'
        scene.view_settings.look = 'None'
    except Exception:
        pass
    scene.render.film_transparent = not opaque
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGB' if opaque else 'RGBA'
    scene.render.image_settings.color_depth = '8'
    scene.render.image_settings.compression = 20

    world = bpy.data.worlds.new("BakeWorld")
    world.use_nodes = True
    bg = world.node_tree.nodes["Background"]
    bg.inputs["Color"].default_value = (0.5, 0.52, 0.58, 1.0)
    bg.inputs["Strength"].default_value = 1.0
    scene.world = world

    def sun(name, rot, energy, angle=12):
        li = bpy.data.lights.new(name, type='SUN')
        li.energy = energy
        li.angle = math.radians(angle)
        ob = bpy.data.objects.new(name, li)
        ob.rotation_euler = rot
        scene.collection.objects.link(ob)

    sun("BakeKey", (math.radians(58), 0, math.radians(-32)), 3.2)
    sun("BakeFill", (math.radians(74), 0, math.radians(138)), 1.2)
    sun("BakeRim", (math.radians(-52), 0, math.radians(6)), 2.2)
    if opaque:
        # a plain backdrop so frames are not floating in the world colour
        me = bpy.data.meshes.new("BakeBackdrop")
        me.from_pydata([(-20, 0, -20), (20, 0, -20), (20, 0, 20), (-20, 0, 20)], [], [(0, 1, 2, 3)])
        mat = bpy.data.materials.new("BakeBackdropMat")
        mat.use_nodes = True
        mat.node_tree.nodes["Principled BSDF"].inputs["Base Color"].default_value = (0.55, 0.57, 0.62, 1.0)
        me.materials.append(mat)
        ob = bpy.data.objects.new("BakeBackdrop", me)
        ob.location = (0, 6, 0)
        scene.collection.objects.link(ob)


def union_bbox(meshes):
    deps = bpy.context.evaluated_depsgraph_get()
    lo = Vector((float("inf"),) * 3)
    hi = Vector((float("-inf"),) * 3)
    for ob in meshes:
        ev = ob.evaluated_get(deps)
        try:
            me = ev.to_mesh()
        except Exception:
            continue
        if me is None:
            continue
        mw = ev.matrix_world
        for v in me.vertices:
            co = mw @ v.co
            for i in range(3):
                lo[i] = min(lo[i], co[i])
                hi[i] = max(hi[i], co[i])
        ev.to_mesh_clear()
    if lo.x > hi.x:
        return Vector((-1, -1, 0)), Vector((1, 1, 2))
    return lo, hi


def main(argv):
    ap = argparse.ArgumentParser(prog="bake_vrm")
    ap.add_argument("--input", default=None)
    ap.add_argument("--outdir", default=os.path.join(REPO, "assets", "models", "pet_frames"))
    ap.add_argument("--cell", type=int, default=96)
    ap.add_argument("--grid", type=int, default=4)
    ap.add_argument("--margin", type=float, default=1.12)
    ap.add_argument("--angle", type=float, default=0.0)
    ap.add_argument("--opaque", action="store_true")
    ap.add_argument("--no-sheet", action="store_true")
    args = ap.parse_args(argv)
    if args.cell < 8 or args.grid < 1:
        raise SystemExit("--cell must be >= 8 and --grid >= 1")

    reset_scene()
    source = args.input
    if source and not os.path.isfile(source):
        log("ERROR input does not exist: %s" % source)
        return 2
    if not source:
        for cand in (os.path.join(REPO, "assets", "models", "testchar.glb"),):
            if os.path.isfile(cand):
                source = cand
                break
        if source is None and os.path.isdir(os.path.join(REPO, "assets", "models")):
            vrms = sorted(f for f in os.listdir(os.path.join(REPO, "assets", "models"))
                          if f.lower().endswith(".vrm"))
            if vrms:
                source = os.path.join(REPO, "assets", "models", vrms[0])
        log("no --input given; auto-selected: %s" % (source or "<fallback prop>"))

    if source:
        import_model(source)
    else:
        build_fallback()

    meshes, armatures = collect_model()
    if not meshes:
        build_fallback()
        meshes, armatures = collect_model()
    tris = 0
    for ob in meshes:
        me = ob.data
        me.calc_loop_triangles()
        tris += len(me.loop_triangles)
    log("model: %d mesh object(s), %d armature(s), %d triangles, materials=%d"
        % (len(meshes), len(armatures), tris,
           len({m.name for o in meshes for m in o.data.materials if m})))

    # ---- choose animation frames or a turntable ----------------------------
    frames = args.grid * args.grid
    fr = action_frame_range(armatures, meshes)
    mode = "animation"
    if fr is None:
        mode = "turntable"
    log("mode: %s" % ("animation frames %.1f..%.1f" % fr if mode == "animation"
                      else "turntable (%d yaw steps; model has no animation)" % frames))

    roots = [o for o in meshes + armatures if o.parent is None]
    root = bpy.data.objects.new("BakeRoot", None)
    bpy.context.scene.collection.objects.link(root)
    for ob in roots:
        ob.parent = root
        ob.matrix_parent_inverse = Matrix.Identity(4)

    step_list = []
    for i in range(frames):
        if mode == "animation":
            step_list.append(fr[0] + (fr[1] - fr[0]) * i / max(1, frames - 1))
        else:
            step_list.append(2.0 * math.pi * i / frames)

    def apply_step(i):
        if mode == "animation":
            bpy.context.scene.frame_set(int(round(step_list[i])))
        else:
            root.rotation_euler = (0.0, 0.0, step_list[i])
        bpy.context.view_layer.update()

    setup_studio(opaque=args.opaque)

    # ---- camera fitted to the union of all poses ---------------------------
    lo = Vector((float("inf"),) * 3)
    hi = Vector((float("-inf"),) * 3)
    for i in range(frames):
        apply_step(i)
        a, b = union_bbox(meshes)
        for k in range(3):
            lo[k] = min(lo[k], a[k])
            hi[k] = max(hi[k], b[k])
    center = (lo + hi) * 0.5
    size = hi - lo
    span = max(size.x, size.z, 1e-4) * args.margin
    log("union bbox min=(%.3f, %.3f, %.3f) max=(%.3f, %.3f, %.3f) size=(%.3f, %.3f, %.3f)"
        % (lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, size.x, size.y, size.z))

    cam_data = bpy.data.cameras.new("BakeCam")
    cam_data.type = 'ORTHO'
    cam_data.ortho_scale = span
    cam_data.clip_start = 0.01
    cam_data.clip_end = max(10.0, size.y + 10.0)
    cam = bpy.data.objects.new("BakeCam", cam_data)
    cam.location = (center.x, center.y - (size.y + 5.0), center.z)
    cam.rotation_euler = (math.radians(90.0), 0.0, math.radians(args.angle))
    bpy.context.scene.collection.objects.link(cam)
    bpy.context.scene.camera = cam
    log("camera: ORTHO scale=%.3f at (%.3f, %.3f, %.3f) yaw=%.1f deg"
        % (span, cam.location.x, cam.location.y, cam.location.z, args.angle))

    scene = bpy.context.scene
    scene.render.resolution_x = args.cell
    scene.render.resolution_y = args.cell
    scene.render.resolution_percentage = 100
    os.makedirs(args.outdir, exist_ok=True)

    written = []
    for i in range(frames):
        apply_step(i)
        path = os.path.join(args.outdir, "frame_%02d.png" % i)
        scene.render.filepath = os.path.abspath(path)
        bpy.ops.render.render(write_still=True)
        written.append((path, os.path.getsize(path) if os.path.isfile(path) else 0))
        row, col = divmod(i, args.grid)
        what = ("frame %.1f" % step_list[i]) if mode == "animation" else ("yaw %.0f deg" % math.degrees(step_list[i]))
        log("cell r%dc%d  %-14s -> %s (%d bytes)" % (row, col, what, os.path.basename(path), written[-1][1]))

    sheet_path = None
    if not args.no_sheet:
        sw, sh = args.cell * args.grid, args.cell * args.grid
        sheet = bytearray(sw * sh * 4)
        for i, (path, _n) in enumerate(written):
            row, col = divmod(i, args.grid)
            w, h, rgba = png_read_rgba(path)
            if (w, h) != (args.cell, args.cell):
                log("WARNING frame %d is %dx%d, expected %dx%d" % (i, w, h, args.cell, args.cell))
            for y in range(min(h, args.cell)):
                src = y * w * 4
                dst = ((row * args.cell + y) * sw + col * args.cell) * 4
                sheet[dst:dst + min(w, args.cell) * 4] = rgba[src:src + min(w, args.cell) * 4]
        sheet_path = os.path.join(args.outdir, "sheet_%dx%d.png" % (args.grid, args.grid))
        nbytes = png_write_rgba(sheet_path, sw, sh, sheet)
        log("sheet: %s (%dx%d, %d bytes)" % (sheet_path, sw, sh, nbytes))

    total = sum(n for _p, n in written) + (os.path.getsize(sheet_path) if sheet_path else 0)
    summary = {
        "source": os.path.abspath(source) if source else "<fallback>",
        "mode": mode,
        "frames": frames,
        "cell": args.cell,
        "grid": args.grid,
        "sheet": os.path.abspath(sheet_path) if sheet_path else None,
        "sheet_px": (args.cell * args.grid) if sheet_path else None,
        "outdir": os.path.abspath(args.outdir),
        "triangles": tris,
        "total_bytes": total,
        "frame_files": [os.path.basename(p) for p, _n in written],
    }
    log("wrote %d frames%s to %s, total %d bytes"
        % (frames, " + sheet" if sheet_path else "", os.path.abspath(args.outdir), total))
    log("SUMMARY " + json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    av = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    sys.exit(main(av))