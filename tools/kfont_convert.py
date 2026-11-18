#!/usr/bin/env python3
"""Convert an outline font (TTF/OTF) to a KimiOS .kfont v2 bitmap atlas.

Rasterizes a set of Unicode codepoints at a reference pixel size using Pillow
(FreeType), emitting the KFONT002 format consumed by user/kimi-gui/src/font_store.rs:

  header(32) | glyph_count * 16-byte records | cmap_count * 8-byte pairs | alpha atlas

The GUI renderer scales the atlas bilinearly to any on-screen size, so a modest
reference em (e.g. 28px) stays sharp when downscaled for UI / terminal text.

Usage:
  python3 tools/kfont_convert.py Iosevka.ttf iosevka.kfont --size 28 \
      --cp 0x20-0xFF,0x2500-0x257F
"""
import argparse
import struct
import sys
from PIL import ImageFont

MAX_GLYPHS = 512
ALPHA_CAP = 131072


def parse_ranges(spec):
    cps = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a, b = part.split("-", 1)
            lo, hi = int(a, 0), int(b, 0)
            cps.extend(range(lo, hi + 1))
        else:
            cps.append(int(part, 0))
    seen = set()
    out = []
    for c in cps:
        if c not in seen and c >= 0x20:
            seen.add(c)
            out.append(c)
    return sorted(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ttf")
    ap.add_argument("out")
    ap.add_argument("--size", type=int, default=28, help="reference em in pixels")
    ap.add_argument("--cp", default="0x20-0xFF", help="codepoint ranges, e.g. 0x20-0xFF,0x2500-0x257F")
    ap.add_argument("--name", default=None, help="font label (default: file stem)")
    args = ap.parse_args()

    font = ImageFont.truetype(args.ttf, args.size, layout_engine=ImageFont.Layout.BASIC)
    ascent, descent = font.getmetrics()
    cps = parse_ranges(args.cp)
    if len(cps) > MAX_GLYPHS:
        sys.exit(f"too many codepoints ({len(cps)} > {MAX_GLYPHS}); narrow --cp")

    glyphs = []
    cmap = []
    atlas = bytearray()
    for cp in cps:
        ch = chr(cp)
        adv = int(round(font.getlength(ch)))
        try:
            mask, (x0, y0) = font.getmask2(ch, mode="L")
            w, h = mask.size
        except Exception:
            bbox = font.getbbox(ch)
            x0, y0, x1, y1 = bbox
            w, h = max(0, x1 - x0), max(0, y1 - y0)
            mask = font.getmask(ch, mode="L")
        if w * h != len(mask):
            # mask buffer may be stride-padded; repack tightly
            row = (len(mask) + h - 1) // h if h else 0
            data = bytes(mask)
            tight = bytearray()
            for r in range(h):
                tight.extend(data[r * row:r * row + w])
            mask_bytes = bytes(tight)
        else:
            mask_bytes = bytes(mask)
        off = len(atlas)
        atlas.extend(mask_bytes)
        idx = len(glyphs)
        glyphs.append(struct.pack("<HhhHHI2x", adv, x0, y0, w, h, off))
        cmap.append(struct.pack("<IH2x", cp, idx))

    if len(atlas) > ALPHA_CAP:
        sys.exit(f"atlas {len(atlas)} bytes > {ALPHA_CAP}; reduce --size or --cp")

    em = args.size
    flags = 1 if font.getlength("i") == font.getlength("m") else 0
    hdr = struct.pack("<8sHhhhHHHHII", b"KFONT002", em, ascent, descent, 0,
                      len(glyphs), len(cmap), flags, 0, len(atlas), 0)
    with open(args.out, "wb") as f:
        f.write(hdr)
        for g in glyphs:
            f.write(g)
        for c in cmap:
            f.write(c)
        f.write(atlas)
    total = 32 + len(glyphs) * 16 + len(cmap) * 8 + len(atlas)
    label = args.name or args.ttf.split("/")[-1].rsplit(".", 1)[0]
    print(f"{label}: {len(glyphs)} glyphs, em={em} ascent={ascent} descent={descent} "
          f"mono={bool(flags)} atlas={len(atlas)}B file={total}B -> {args.out}")


if __name__ == "__main__":
    main()
