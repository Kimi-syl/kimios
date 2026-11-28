#!/usr/bin/env python3
"""Pack rendered RGBA frames into a KimiOS desktop-pet pack, and validate any
pack exactly the way `user/kimi-gui/src/pet.rs` parses it.

Drop-in destination in the repo: `tools/make_pet_pack.py` (same style as
`tools/make_ktheme.py`: convert, plus `--check` that mirrors the Rust parser).

  make_pet_pack.py pack FRAMES_DIR -o OUT_DIR [--cell 96] [--cols 4] [--src-cell 192]
  make_pet_pack.py check PACK_DIR

`pack` does the things that decide whether the sprite looks right on device:
  * one GLOBAL sub-pixel-free offset for every frame (feet on `baseline`,
    content centred on `anchor_x`) -> the character cannot jitter between frames;
  * optional premultiplied LANCZOS downscale of supersampled renders (avoids
    dark hair/edge fringes);
  * RGB zeroed under alpha==0 (pet.rs skips a==0; this is ~30% smaller PNG);
  * pack.json key order animations -> states -> timings -> reactions -> roam
    (pet.rs resolves animation names while walking the object, so order matters).

`check` mirrors parse_pack (pet.rs:517), parse_anim (pet.rs:482),
decode_png (pet.rs:226) and the import caps (pet.rs:790-807, 829-888).
"""
import argparse
import json
import os
import re
import struct
import sys
import zlib

from PIL import Image

MAX_ANIM = 16          # pet.rs:332  MAX_ANIMS
MAX_FRAMES = 24        # pet.rs:333
NAME_LEN = 16          # pet.rs:334
MAX_DIM = 384          # pet.rs:199
JSON_CAP = 65536       # pet.rs:791 read buffer
PNG_CAP = 262144       # pet.rs:792 read buffer
BYTE_BUDGET = 180 << 10  # house rule: stay well under PNG_CAP for marketplace
STATES = ["idle", "thinking", "working", "juggling", "sweeping", "notification",
          "attention", "error", "carrying", "sleeping", "waking"]      # pet.rs:380-391
REACTIONS = ["tapLeft", "tapRight", "tickle", "drag"]                 # pet.rs:445

DEFAULT_MATRIX = [
    {"name": "idle",     "frames": 4, "ms": 250, "loop": True},
    {"name": "thinking", "frames": 4, "ms": 220, "loop": True},
    {"name": "working",  "frames": 4, "ms": 160, "loop": True},
    {"name": "sleeping", "frames": 2, "ms": 900, "loop": True},
    {"name": "tap",      "frames": 2, "ms": 140, "loop": False},
]
DEFAULT_STATES = {
    "idle": "idle", "thinking": "thinking", "working": "working",
    "juggling": "working", "sweeping": "working", "notification": "tap",
    "attention": "tap", "error": "tap", "carrying": "working",
    "sleeping": "sleeping", "waking": "idle",
}
DEFAULT_TIMINGS = {
    "minDisplay": {"attention": 2500, "error": 2500, "notification": 2000,
                   "waking": 600},
    "autoReturn": {"attention": 2500, "error": 3000, "notification": 4000},
}


# ------------------------------------------------------------------ helpers --
def premultiplied_resize(img, size):
    """RGBA LANCZOS resize in premultiplied space (no dark hair/edge fringes)."""
    if img.size == (size, size):
        return img
    import numpy as np
    a = np.asarray(img, dtype=np.float32)
    alpha = a[:, :, 3:4] / 255.0
    pm = np.concatenate([a[:, :, :3] * alpha, a[:, :, 3:4]], axis=2).astype(np.uint8)
    out = np.asarray(Image.fromarray(pm, "RGBA").resize((size, size), Image.LANCZOS),
                     dtype=np.float32)
    oa = out[:, :, 3:4]
    rgb = np.where(oa > 0, out[:, :, :3] * 255.0 / np.maximum(oa, 1.0), 0.0)
    return Image.fromarray(
        np.concatenate([np.clip(rgb, 0, 255), oa], axis=2).astype(np.uint8), "RGBA")


def load_frames(frames_dir, matrix, cell, src_cell):
    frames = {}
    for a in matrix:
        got = []
        for i in range(a["frames"]):
            p = os.path.join(frames_dir, f"{a['name']}_{i:02d}.png")
            if not os.path.isfile(p):
                raise SystemExit(f"missing frame: {p}")
            img = Image.open(p)
            if img.mode != "RGBA":
                img = img.convert("RGBA")
            if img.size != (src_cell, src_cell):
                raise SystemExit(f"{p}: {img.size[0]}x{img.size[1]} != "
                                 f"--src-cell {src_cell}")
            got.append(premultiplied_resize(img, cell))
        frames[a["name"]] = got
    return frames


def alpha_bbox(img):
    return img.getchannel("A").getbbox()      # (l, t, r, b), exclusive r/b


def zero_under_transparent(img):
    px = img.load()
    for y in range(img.height):
        for x in range(img.width):
            r, g, b, a = px[x, y]
            if a == 0 and (r or g or b):
                px[x, y] = (0, 0, 0, 0)
    return img


# ------------------------------------------------------------- vrm metadata --
def glb_json(path):
    """Return the glTF JSON chunk of a .glb/.vrm container, or None."""
    if not os.path.isfile(path):
        return None
    with open(path, "rb") as f:
        head = f.read(12)                          # magic, version, total length
        if len(head) < 12 or head[:4] != b"glTF":
            return None
        (clen,) = struct.unpack("<I", f.read(4))   # chunk 0 length
        (ctype,) = struct.unpack("<I", f.read(4))  # chunk 0 type
        if ctype != 0x4E4F534A:                    # 'JSON'
            return None
        return json.loads(f.read(clen).decode("utf-8"))


def vrm_meta(path):
    """Spec version + avatar licence block, without any 3D tool."""
    doc = glb_json(path) or {}
    ext = doc.get("extensions", {})
    facts = {"file": os.path.basename(path), "spec": "1.0" if "VRMC_vrm" in ext
             else "0.x" if "VRM" in ext else "plain glTF (no VRM extension)"}
    meta = (ext.get("VRM") or {}).get("meta") or \
           (ext.get("VRMC_vrm") or {}).get("meta") or {}
    for k in ("name", "authors", "licenseName", "avatarPermission",
              "commercialUsage", "allowRedistribution", "modification",
              "creditNotation", "otherLicenseUrl"):
        if k in meta:
            facts[k] = meta[k]
    return facts


# ------------------------------------------------------------------ packing --
def build(frames_dir, out_dir, matrix, cell, cols, src_cell, meta):
    if len(matrix) > MAX_ANIM:
        raise SystemExit(f"{len(matrix)} animations > MAX_ANIMS={MAX_ANIM}")
    for a in matrix:
        if a["frames"] > MAX_FRAMES:
            raise SystemExit(f"{a['name']}: {a['frames']} > MAX_FRAMES={MAX_FRAMES}")
        if len(a["name"]) > NAME_LEN:
            raise SystemExit(f"animation name {a['name']!r} > {NAME_LEN} bytes")
    total = sum(a["frames"] for a in matrix)
    rows = (total + cols - 1) // cols
    aw, ah = cols * cell, rows * cell
    if aw > MAX_DIM or ah > MAX_DIM:
        raise SystemExit(f"atlas {aw}x{ah} exceeds MAX_DIM={MAX_DIM} "
                         f"({total} frames at {cell}px in {cols} cols); "
                         f"use <= {MAX_DIM // cell} cols x rows")

    frames = load_frames(frames_dir, matrix, cell, src_cell)

    # ---- ONE global offset for every frame: feet on baseline, centred on anchor
    anchor_x = meta["anchor_x"] if meta["anchor_x"] is not None else cell // 2
    baseline = meta["baseline"] if meta["baseline"] is not None else cell - max(2, cell // 24)
    l, t, r, b = cell, cell, 0, 0
    for imgs in frames.values():
        for img in imgs:
            bb = alpha_bbox(img)
            if not bb:
                continue
            l, t = min(l, bb[0]), min(t, bb[1])
            r, b = max(r, bb[2]), max(b, bb[3])
    if r <= l:
        raise SystemExit("every frame is fully transparent")
    dx = anchor_x - (l + r) // 2
    dy = baseline - (b - 1)     # last ink row lands exactly on `baseline`
    print(f"union ink bbox in frame space: x={l}..{r - 1} y={t}..{b - 1}")
    print(f"global offset dx={dx} dy={dy} -> feet on baseline={baseline}, "
          f"centre on anchor_x={anchor_x}")

    atlas = Image.new("RGBA", (aw, ah), (0, 0, 0, 0))
    placed, idx, clipped = {}, 0, 0
    for a in matrix:
        placed[a["name"]] = []
        for img in frames[a["name"]]:
            ox, oy = (idx % cols) * cell, (idx // cols) * cell
            cell_img = Image.new("RGBA", (cell, cell), (0, 0, 0, 0))
            cell_img.paste(img, (dx, dy))
            bb = alpha_bbox(cell_img)
            if bb and (bb[0] < 0 or bb[1] < 0 or bb[2] > cell or bb[3] > cell):
                clipped += 1
            atlas.paste(cell_img, (ox, oy))
            placed[a["name"]].append(idx)
            idx += 1
    if clipped:
        print(f"warn: {clipped} frame(s) spill outside the {cell}px cell — "
              f"increase --cell or reduce the motion amplitude", file=sys.stderr)

    zero_under_transparent(atlas)

    # content box: union ink in CELL-LOCAL coords (pet.rs:1109 adds cb_* to the
    # same anchor-relative origin the sprite is drawn at)
    l2, t2, r2, b2 = cell, cell, 0, 0
    for y in range(ah):
        for x in range(aw):
            if atlas.getpixel((x, y))[3]:
                cx, cy = x % cell, y % cell
                l2, t2, r2, b2 = min(l2, cx), min(t2, cy), max(r2, cx), max(b2, cy)
    box = {"x": l2, "y": t2, "w": r2 - l2 + 1, "h": b2 - t2 + 1}

    pack = {
        "schema": 1,                      # cosmetic: parse_pack ignores it
        "id": meta["id"], "name": meta["name"],
        "author": meta["author"], "license": meta["license"],
        "pixelArt": False,                # cosmetic
        "scale": 1,                       # integer >= 1 (pet.rs:523)
        "canvas": {"w": cell, "h": cell},
        "anchor": {"x": anchor_x, "baseline": baseline},
        "contentBox": box,
        "animations": {},
        "states": {},
        "timings": DEFAULT_TIMINGS,
        "reactions": {"tapLeft": "tap", "tapRight": "tap",
                      "tickle": "tap", "drag": "tap"},
        "roam": {"anim": "working", "faces": "right"},   # parsed, unused at runtime
        "sleep": {"afterMs": 60000},
    }
    for a in matrix:
        pack["animations"][a["name"]] = {
            "sheet": "atlas.png",         # cosmetic: the file must be atlas.png
            "frameW": cell, "frameH": cell, "cols": cols, "loop": a["loop"],
            "box": {"x": 0, "y": 0, "w": cell, "h": cell},   # parsed? no.
            "frames": [{"i": i, "ms": a["ms"]} for i in placed[a["name"]]],
        }
    for s in STATES:                      # every state, so no idle fallback surprises
        pack["states"][s] = [DEFAULT_STATES.get(s, "idle")]

    os.makedirs(out_dir, exist_ok=True)
    atlas.save(os.path.join(out_dir, "atlas.png"), optimize=False, compress_level=9)
    with open(os.path.join(out_dir, "pack.json"), "w", encoding="utf-8",
              newline="\n") as f:
        json.dump(pack, f, indent=1)
        f.write("\n")
    return pack


# --------------------------------------------------------------- validation --
def read_png(path):
    """Strict reader mirroring decode_png (pet.rs:226-322)."""
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos, idat = 8, b""
    w = h = None
    while pos + 8 <= len(data):
        (ln,) = struct.unpack(">I", data[pos:pos + 4])
        tag = data[pos + 4:pos + 8]
        chunk = data[pos + 8:pos + 8 + ln]
        if tag == b"IHDR":
            w, h, depth, ct, _, _, interlace = struct.unpack(">IIBBBBB", chunk[:13])
        elif tag == b"IDAT":
            idat += chunk
        elif tag == b"IEND":
            break
        pos += 12 + ln
    if w is None:
        raise SystemExit(f"{path}: no IHDR")
    if depth != 8 or interlace != 0 or ct not in (2, 6):
        raise SystemExit(f"{path}: pet.rs needs 8-bit, non-interlaced RGB/RGBA "
                         f"(got depth={depth} ct={ct} interlace={interlace})")
    bpp = 3 if ct == 2 else 4
    if len(idat) > 262144:
        raise SystemExit(f"{path}: IDAT {len(idat)} > IDAT buffer 262144 (pet.rs:203)")
    raw = zlib.decompressobj(-15).decompress(idat[2:])   # pet.rs skips CMF/FLG
    need = h * (1 + w * bpp)
    if len(raw) != need:
        raise SystemExit(f"{path}: inflate {len(raw)} != {need} (pet.rs rejects)")
    rows, prev = [], bytearray(w * bpp)
    for y in range(h):
        ft = raw[y * (1 + w * bpp)]
        line = bytearray(raw[y * (1 + w * bpp) + 1:(y + 1) * (1 + w * bpp)])
        for x in range(w * bpp):
            a = line[x - bpp] if x >= bpp else 0
            b_ = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if ft == 1:
                line[x] = (line[x] + a) & 0xFF
            elif ft == 2:
                line[x] = (line[x] + b_) & 0xFF
            elif ft == 3:
                line[x] = (line[x] + ((a + b_) >> 1)) & 0xFF
            elif ft == 4:
                p = a + b_ - c
                pa, pb, pc = abs(p - a), abs(p - b_), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b_ if pb <= pc else c)
                line[x] = (line[x] + pr) & 0xFF
            elif ft != 0:
                raise SystemExit(f"{path}: bad filter {ft}")
        prev = line
        rows.append([(line[x * bpp], line[x * bpp + 1], line[x * bpp + 2],
                      255 if bpp == 3 else line[x * bpp + 3]) for x in range(w)])
    return w, h, ct, depth, interlace, rows


def parse_like_pet_rs(text):
    if text.startswith("\ufeff"):
        raise SystemExit("pack.json starts with a UTF-8 BOM; pet.rs's scanner "
                         "expects '{' after whitespace")
    try:
        doc = json.loads(text)
    except json.JSONDecodeError as e:
        raise SystemExit(f"pack.json is not valid JSON: {e}")

    def walk(v):
        if isinstance(v, float):
            print("warn: a float is in pack.json; pet.rs keeps the integer part "
                  "and an exponent form truncates the rest of the object",
                  file=sys.stderr)
        elif isinstance(v, dict):
            for x in v.values():
                walk(x)
        elif isinstance(v, list):
            for x in v:
                walk(x)
    walk(doc)

    order = re.findall(r'"([^"]+)"\s*:', text)

    def pos(key):
        return order.index(key) if key in order else 10 ** 6
    if "states" in doc and pos("states") < pos("animations"):
        raise SystemExit("`states` must come AFTER `animations`: parse_pack "
                         "resolves names as it walks the object (pet.rs:572-601)")

    anims = doc.get("animations") or {}
    parsed = {}
    for name, a in anims.items():
        if len(name.encode()) > NAME_LEN:
            raise SystemExit(f"animation {name!r} > {NAME_LEN} bytes (truncated)")
        fw = int(a.get("frameW", 0))
        fh = int(a.get("frameH", 0))
        cols = max(1, int(a.get("cols", 1)))
        fr = [{"i": int(f.get("i", 0)), "ms": max(1, int(f.get("ms", 200)))}
              for f in (a.get("frames") or [])]
        if fw > 0 and fh > 0 and fr:
            parsed[name] = {"fw": fw, "fh": fh, "cols": cols,
                            "loop": bool(a.get("loop", True)), "frames": fr}
        else:
            print(f"warn: animation {name!r} silently dropped by parse_anim "
                  f"(needs frameW>0, frameH>0, >=1 frame)", file=sys.stderr)
    if not parsed:
        raise SystemExit("no usable animations -> pet.rs returns None")
    states = {}
    for s, v in (doc.get("states") or {}).items():
        if s not in STATES:
            print(f"warn: unknown state {s!r} ignored by state_index", file=sys.stderr)
            continue
        first = v[0] if isinstance(v, list) and v else None
        if first not in parsed:
            print(f"warn: state {s!r} names unknown animation {first!r} -> idle",
                  file=sys.stderr)
            first = None
        states[s] = first
    idle = states.get("idle")
    for s in STATES:
        if not states.get(s):
            states[s] = idle
    return {"anims": parsed, "states": states, "raw": doc}


def check(pack_dir):
    ok = True
    jp = os.path.join(pack_dir, "pack.json")
    pp = os.path.join(pack_dir, "atlas.png")
    for p in (jp, pp):
        if not os.path.isfile(p):
            raise SystemExit(f"missing {p}")
    jsize, psize = os.path.getsize(jp), os.path.getsize(pp)
    print(f"pack.json {jsize} B (cap {JSON_CAP})   atlas.png {psize} B "
          f"(cap {PNG_CAP}, budget {BYTE_BUDGET})")
    if jsize > JSON_CAP:
        print("FAIL pack.json exceeds the 64 KiB read buffer -> truncated")
        ok = False
    if psize > PNG_CAP:
        print("FAIL atlas.png exceeds the 256 KiB read buffer -> truncated")
        ok = False
    elif psize > BYTE_BUDGET:
        print(f"warn: {psize - BYTE_BUDGET} B over the {BYTE_BUDGET} B house budget")
    parsed = parse_like_pet_rs(open(jp, encoding="utf-8").read())
    w, h, ct, depth, interlace, px = read_png(pp)
    print(f"atlas {w}x{h} colortype={ct} depth={depth} interlace={interlace} "
          f"(MAX_DIM={MAX_DIM})")
    if w > MAX_DIM or h > MAX_DIM:
        print("FAIL atlas larger than MAX_DIM")
        ok = False
    for name, a in parsed["anims"].items():
        for f in a["frames"]:
            sx = (f["i"] % a["cols"]) * a["fw"]
            sy = (f["i"] // a["cols"]) * a["fh"]
            if sx + a["fw"] > w or sy + a["fh"] > h:
                print(f"FAIL {name} frame i={f['i']} cell ({sx},{sy}) "
                      f"{a['fw']}x{a['fh']} outside the {w}x{h} atlas")
                ok = False
        print(f"  {name:10} {len(a['frames'])} frames {a['fw']}x{a['fh']} "
              f"cols={a['cols']} loop={a['loop']} "
              f"cycle={sum(f['ms'] for f in a['frames'])}ms")
    ink = [(x, y) for y in range(h) for x in range(w) if px[y][x][3]]
    if not ink:
        print("FAIL atlas fully transparent -> nothing renders")
        return False
    xs, ys = [p[0] for p in ink], [p[1] for p in ink]
    print(f"ink bbox x={min(xs)}..{max(xs)} y={min(ys)}..{max(ys)} "
          f"({100 * len(ink) / (w * h):.1f}% coverage)")
    lx0, ly0, lx1, ly1 = 10 ** 6, 10 ** 6, -1, -1
    for a in parsed["anims"].values():
        for f in a["frames"]:
            sx = (f["i"] % a["cols"]) * a["fw"]
            sy = (f["i"] // a["cols"]) * a["fh"]
            for y in range(sy, min(sy + a["fh"], h)):
                for x in range(sx, min(sx + a["fw"], w)):
                    if px[y][x][3]:
                        lx0, ly0 = min(lx0, x - sx), min(ly0, y - sy)
                        lx1, ly1 = max(lx1, x - sx), max(ly1, y - sy)
    print(f"cell-local ink bbox x={lx0}..{lx1} y={ly0}..{ly1}")
    cb = parsed["raw"].get("contentBox") or {}
    if cb and not (cb.get("x", 0) <= lx0 and cb.get("y", 0) <= ly0 and
                   cb.get("x", 0) + cb.get("w", 0) >= lx1 + 1 and
                   cb.get("y", 0) + cb.get("h", 0) >= ly1 + 1):
        print("warn: contentBox does not cover the cell-local ink bbox "
              "(taps outside it are ignored)")
    anchor = parsed["raw"].get("anchor") or {}
    if "x" not in anchor or "baseline" not in anchor:
        print("warn: anchor/baseline missing -> defaults (12,24) from "
              "PetPack::empty (pet.rs:451-471) will misplace the sprite")
    for name, a in parsed["anims"].items():
        feet = []
        for f in a["frames"]:
            sx = (f["i"] % a["cols"]) * a["fw"]
            sy = (f["i"] // a["cols"]) * a["fh"]
            rows = [y for y in range(sy, min(sy + a["fh"], h))
                    if any(px[y][x][3] for x in range(sx, min(sx + a["fw"], w)))]
            feet.append(rows[-1] - sy if rows else None)
        if None in feet:
            print(f"warn: {name} has an empty frame")
        elif max(feet) - min(feet) > 1:
            print(f"warn: {name} foot line moves {min(feet)}..{max(feet)} "
                  f"({max(feet) - min(feet)} px jitter)")
        else:
            print(f"  {name:10} foot line stable at y={feet[0]}")
    print("RESULT:", "PASS" if ok else "FAIL")
    return ok


# --------------------------------------------------------------------- main --
def main():
    ap = argparse.ArgumentParser(description="Pack frames into a KimiOS pet pack")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pack")
    p.add_argument("frames")
    p.add_argument("-o", "--out", required=True)
    p.add_argument("--cell", type=int, default=96)
    p.add_argument("--src-cell", type=int, default=None,
                   help="rendered frame size (e.g. 192 for 2x supersampling)")
    p.add_argument("--cols", type=int, default=4)
    p.add_argument("--anchor-x", type=int, default=None)
    p.add_argument("--baseline", type=int, default=None)
    p.add_argument("--id", default="custom")
    p.add_argument("--name", default="Custom")
    p.add_argument("--author", default="unknown")
    p.add_argument("--license", default="unknown")
    c = sub.add_parser("check")
    c.add_argument("pack_dir")
    v = sub.add_parser("vrm-meta", help="print a .vrm/.glb licence + spec block")
    v.add_argument("vrm")
    a = ap.parse_args()
    if a.cmd == "vrm-meta":
        print(json.dumps(vrm_meta(a.vrm), indent=1, ensure_ascii=False))
        return 0
    if a.cmd == "pack":
        meta = {"id": a.id, "name": a.name, "author": a.author,
                "license": a.license, "anchor_x": a.anchor_x,
                "baseline": a.baseline}
        build(a.frames, a.out, DEFAULT_MATRIX, a.cell, a.cols,
              a.src_cell or a.cell, meta)
        print(f"wrote {a.out}/pack.json and {a.out}/atlas.png")
        return 0 if check(a.out) else 1
    return 0 if check(a.pack_dir) else 1


if __name__ == "__main__":
    sys.exit(main())