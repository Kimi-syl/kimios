#!/usr/bin/env python3
"""Convert a color palette into a KimiOS `.ktheme` theme pack.

Format (parsed by `user/kimi-gui/src/theme.rs::load_theme_file`):
exactly 11 lines of `RRGGBB` hex, one color per line, in this slot order:

    idx  slot         meaning in the KimiOS GUI
    ---  -----------  --------------------------------------------------
     0   black        deepest background
     1   surface      main background
     2   surface_hi   elevated surface
     3   surface_max  highest surface
     4   outline      borders / separators
     5   text         foreground text
     6   muted        comment / subtext
     7   red          primary accent   (theme.rs `accent`, RED())
     8   red_dim      dimmed accent    (theme.rs `accent_dim`, RED_DIM())
     9   green        green / success accent
    10   panel        panel background (usually darker than surface)

Parser constraints (mirrored from theme.rs):
  * only the first 256 bytes of the file are read;
  * the parser scans for runs of hex digits anywhere in those bytes and
    takes every complete 6-digit run as one color, stopping after 11.
    Therefore a .ktheme must contain NOTHING but the 11 hex lines --
    no comments, names or headers (stray hex-looking text corrupts it).

Light themes invert the surface ladder (see the built-in "Graphite
Light"): black = lightest page background, surfaces get progressively
darker, red_dim becomes a light tint of the accent.

Usage:
  make_ktheme.py --list                     list bundled scheme packs
  make_ktheme.py --pack nord [-o OUT]       write a bundled pack
                                            (default: market/themes/<name>.ktheme)
  make_ktheme.py --all                      regenerate every bundled pack
  make_ktheme.py palette.json [-o OUT]      convert your own palette JSON
                                            (keys = the 11 slot names above;
                                            "accent"/"accent_dim" accepted as
                                            aliases for red/red_dim)
  make_ktheme.py --check FILE [FILE...]     validate files exactly the way
                                            theme.rs parses them

Palette JSON values may be "RRGGBB", "#RRGGBB" or "#RGB". A value may also
be a blend expression ["mix", "aabbcc", "ddeeff", 0.3] meaning 30% of the
first color over the second -- used for derived slots (dim accents, extra
background steps) that a source scheme does not define itself.
"""

import argparse
import json
import re
import sys
from pathlib import Path

SLOTS = (
    "black", "surface", "surface_hi", "surface_max", "outline",
    "text", "muted", "red", "red_dim", "green", "panel",
)

ALIASES = {"accent": "red", "accent_dim": "red_dim"}

REPO_ROOT = Path(__file__).resolve().parent.parent
THEMES_DIR = REPO_ROOT / "market" / "themes"


def norm_hex(value):
    if not isinstance(value, str):
        raise ValueError(f"expected hex string, got {value!r}")
    s = value.strip().lstrip("#")
    if re.fullmatch(r"[0-9a-fA-F]{3}", s):
        s = "".join(c * 2 for c in s)
    if not re.fullmatch(r"[0-9a-fA-F]{6}", s):
        raise ValueError(f"invalid hex color: {value!r}")
    return s.lower()


def mix(a, b, t):
    """Blend color a over color b: t=0 -> b, t=1 -> a (round half up)."""
    a, b = norm_hex(a), norm_hex(b)
    if not 0.0 <= t <= 1.0:
        raise ValueError(f"mix fraction out of range: {t}")
    out = []
    for i in (0, 2, 4):
        x, y = int(a[i:i + 2], 16), int(b[i:i + 2], 16)
        out.append(min(255, max(0, int(y + (x - y) * t + 0.5))))
    return "".join(f"{c:02x}" for c in out)


def resolve(value):
    if isinstance(value, (list, tuple)):
        if len(value) == 4 and value[0] == "mix":
            return mix(value[1], value[2], value[3])
        raise ValueError(f"unknown blend expression: {value!r}")
    return norm_hex(value)


def palette_to_lines(palette):
    pal = {}
    for key, value in palette.items():
        key = ALIASES.get(key, key)
        if key not in SLOTS:
            raise ValueError(f"unknown slot: {key!r}")
        pal[key] = resolve(value)
    missing = [s for s in SLOTS if s not in pal]
    if missing:
        raise ValueError(f"missing slots: {', '.join(missing)}")
    return [pal[s] for s in SLOTS]


def ktheme_text(palette):
    return "\n".join(palette_to_lines(palette)) + "\n"


def write_ktheme(palette, path):
    text = ktheme_text(palette)
    if len(text.encode()) > 256:
        raise ValueError("theme exceeds the 256-byte parser window")
    Path(path).write_text(text)
    return text


def parse_like_theme_rs(data):
    """Byte-for-byte port of theme.rs::load_theme_file's color scanner.

    Returns the 11 parsed 0xRRGGBB values, or None if fewer than 11
    complete hex runs are found in the first 256 bytes.
    """
    v = []
    cur = bytearray()
    started = False
    for b in data[:256]:
        is_hex = 0x30 <= b <= 0x39 or 0x61 <= b <= 0x66 or 0x41 <= b <= 0x46
        if is_hex:
            if len(cur) < 6:
                cur.append(b)
            started = True
        elif started:
            if len(cur) == 6 and len(v) < 11:
                v.append(int(cur, 16))
            cur = bytearray()
            started = False
            if len(v) >= 11:
                break
    if started and len(cur) == 6 and len(v) < 11:
        v.append(int(cur, 16))
    return v if len(v) == 11 else None


def check_ktheme(path):
    data = Path(path).read_bytes()
    parsed = parse_like_theme_rs(data)
    if parsed is None:
        return False, "theme.rs parser found fewer than 11 hex colors"
    strict = re.fullmatch(r"(?:[0-9a-fA-F]{6}\n){11}", data.decode("ascii", "replace"))
    if not strict:
        return False, "parses, but file is not exactly 11 RRGGBB lines (stray text?)"
    lines = [int(l, 16) for l in data.decode().splitlines()]
    if lines != parsed:
        return False, "strict lines disagree with theme.rs scanner"
    if len(data) > 256:
        return False, "file exceeds the 256-byte parser window"
    return True, "ok (11 slots: " + " ".join(f"{c:06x}" for c in parsed) + ")"


# --- Bundled open-source scheme packs --------------------------------------
# Every canonical hex below was fetched from the official source listed in
# "source". Slots marked DERIVED in market/THEMES.md are ["mix", ...] blends.
# Licenses verified from the upstream license files / package.json.

PACKS = {
    "catppuccin-mocha": {
        "scheme": "Catppuccin Mocha",
        "source": "https://github.com/catppuccin/catppuccin (README.md palette table)",
        "license": "MIT",
        "copyright": "Copyright (c) 2021 Catppuccin",
        "palette": {
            "black": "11111b",        # crust
            "surface": "1e1e2e",      # base
            "surface_hi": "313244",   # surface0
            "surface_max": "45475a",  # surface1
            "outline": "585b70",      # surface2
            "text": "cdd6f4",         # text
            "muted": "7f849c",        # overlay1
            "red": "f38ba8",          # red
            "red_dim": ["mix", "f38ba8", "1e1e2e", 0.30],  # DERIVED
            "green": "a6e3a1",        # green
            "panel": "181825",        # mantle
        },
    },
    "catppuccin-latte": {
        "scheme": "Catppuccin Latte",
        "source": "https://github.com/catppuccin/catppuccin (README.md palette table)",
        "license": "MIT",
        "copyright": "Copyright (c) 2021 Catppuccin",
        "palette": {
            "black": "eff1f5",        # base (light theme: lightest)
            "surface": "e6e9ef",      # mantle
            "surface_hi": "ccd0da",   # surface0
            "surface_max": "bcc0cc",  # surface1
            "outline": "acb0be",      # surface2
            "text": "4c4f69",         # text
            "muted": "6c6f85",        # subtext0
            "red": "d20f39",          # red
            "red_dim": ["mix", "d20f39", "eff1f5", 0.20],  # DERIVED light tint
            "green": "40a02b",        # green
            "panel": "dce0e8",        # crust
        },
    },
    "gruvbox-dark": {
        "scheme": "Gruvbox Dark (medium)",
        "source": "https://github.com/morhetz/gruvbox (colors/gruvbox.vim)",
        "license": "MIT",
        "copyright": "Copyright (c) Pavel Pertsev <morhetz@gmail.com> (morhetz)",
        "palette": {
            "black": "1d2021",        # dark0_hard
            "surface": "282828",      # dark0 (medium bg)
            "surface_hi": "3c3836",   # dark1
            "surface_max": "504945",  # dark2
            "outline": "665c54",      # dark3
            "text": "fbf1c7",         # light0 / fg0
            "muted": "928374",        # gray_245 (comments)
            "red": "fb4934",          # bright_red
            "red_dim": "cc241d",      # neutral_red
            "green": "b8bb26",        # bright_green
            "panel": ["mix", "1d2021", "282828", 0.5],  # DERIVED
        },
    },
    "gruvbox-dark-hard": {
        "scheme": "Gruvbox Dark (hard contrast)",
        "source": "https://github.com/morhetz/gruvbox (colors/gruvbox.vim)",
        "license": "MIT",
        "copyright": "Copyright (c) Pavel Pertsev <morhetz@gmail.com> (morhetz)",
        "palette": {
            "black": ["mix", "1d2021", "000000", 0.5],  # DERIVED
            "surface": "1d2021",      # dark0_hard (hard bg)
            "surface_hi": "282828",   # dark0
            "surface_max": "3c3836",  # dark1
            "outline": "504945",      # dark2
            "text": "fbf1c7",         # light0 / fg0
            "muted": "928374",        # gray_245
            "red": "fb4934",          # bright_red
            "red_dim": "cc241d",      # neutral_red
            "green": "b8bb26",        # bright_green
            "panel": ["mix", "1d2021", "000000", 0.75],  # DERIVED
        },
    },
    "gruvbox-dark-soft": {
        "scheme": "Gruvbox Dark (soft contrast)",
        "source": "https://github.com/morhetz/gruvbox (colors/gruvbox.vim)",
        "license": "MIT",
        "copyright": "Copyright (c) Pavel Pertsev <morhetz@gmail.com> (morhetz)",
        "palette": {
            "black": "1d2021",        # dark0_hard
            "surface": "32302f",      # dark0_soft (soft bg)
            "surface_hi": "3c3836",   # dark1
            "surface_max": "504945",  # dark2
            "outline": "665c54",      # dark3
            "text": "fbf1c7",         # light0 / fg0
            "muted": "928374",        # gray_245
            "red": "fb4934",          # bright_red
            "red_dim": "cc241d",      # neutral_red
            "green": "b8bb26",        # bright_green
            "panel": ["mix", "1d2021", "32302f", 0.5],  # DERIVED (= dark0)
        },
    },
    "tokyo-night": {
        "scheme": "Tokyo Night",
        "source": "https://github.com/tokyo-night/tokyo-night-vscode-theme "
                  "(themes/tokyo-night-color-theme.json)",
        "license": "MIT",
        "copyright": "Copyright (c) 2018-present Enkia",
        "palette": {
            "black": "101014",        # editorWidget.border (deepest)
            "surface": "1a1b26",      # editor.background
            "surface_hi": "202330",   # list.activeSelectionBackground
            "surface_max": "232433",  # editorIndentGuide.background1
            "outline": "363b54",      # editorLineNumber.foreground / ansiBlack
            "text": "a9b1d6",         # editor.foreground
            "muted": "51597d",        # comment
            "red": "f7768e",          # terminal.ansiRed
            "red_dim": "914c54",      # markup.deleted
            "green": "9ece6a",        # string
            "panel": "16161e",        # sideBar/panel.background
        },
    },
    "tokyo-storm": {
        "scheme": "Tokyo Night Storm",
        "source": "https://github.com/tokyo-night/tokyo-night-vscode-theme "
                  "(themes/tokyo-night-storm-color-theme.json)",
        "license": "MIT",
        "copyright": "Copyright (c) 2018-present Enkia",
        "palette": {
            "black": "1b1e2e",        # editorWidget.border (deepest)
            "surface": "24283b",      # editor.background
            "surface_hi": "2c324a",   # list.activeSelectionBackground
            "surface_max": "3b4261",  # editorLineNumber.foreground
            "outline": "414868",      # terminal.ansiBlack
            "text": "a9b1d6",         # editor.foreground
            "muted": "5f6996",        # comment
            "red": "f7768e",          # terminal.ansiRed
            "red_dim": "914c54",      # markup.deleted
            "green": "9ece6a",        # string
            "panel": "1f2335",        # sideBar/panel.background
        },
    },
    "one-dark": {
        "scheme": "One Dark (Atom One Dark via One Dark Pro)",
        "source": "https://github.com/Binaryify/OneDark-Pro (themes/OneDark-Pro.json); "
                  "original: https://github.com/atom/one-dark-syntax",
        "license": "MIT",
        "copyright": "Copyright (c) 2013-2022 Binaryify; "
                     "Atom One Dark: Copyright (c) 2016 GitHub Inc.",
        "palette": {
            "black": "181a1f",        # tab.border / editorGroup.border (deepest)
            "surface": "282c34",      # editor.background
            "surface_hi": "2c313a",   # list.activeSelectionBackground
            "surface_max": "3b4048",  # editorIndentGuide.background1
            "outline": "3e4452",      # focusBorder
            "text": "abb2bf",         # editor.foreground
            "muted": "5c6370",        # comment
            "red": "e06c75",          # entity.name.tag / markup.deleted
            "red_dim": ["mix", "e06c75", "282c34", 0.30],  # DERIVED
            "green": "98c379",        # string
            "panel": "21252b",        # sideBar.background
        },
    },
    "nord": {
        "scheme": "Nord",
        "source": "https://github.com/nordtheme/nord (src/nord.scss)",
        "license": "MIT",
        "copyright": "Copyright (c) 2016-present Sven Greb <development@svengreb.de>",
        "palette": {
            "black": "2e3440",        # nord0
            "surface": "3b4252",      # nord1
            "surface_hi": "434c5e",   # nord2
            "surface_max": "4c566a",  # nord3
            "outline": ["mix", "4c566a", "d8dee9", 0.75],  # DERIVED nord3->nord4
            "text": "eceff4",         # nord6
            "muted": ["mix", "4c566a", "d8dee9", 0.40],    # DERIVED nord3->nord4
            "red": "88c0d0",          # nord8 (Nord's official accent)
            "red_dim": ["mix", "88c0d0", "2e3440", 0.30],  # DERIVED
            "green": "a3be8c",        # nord14
            "panel": ["mix", "2e3440", "000000", 0.85],    # DERIVED
        },
    },
    "dracula": {
        "scheme": "Dracula",
        "source": "https://github.com/dracula/dracula-theme (README.md palette table)",
        "license": "MIT",
        "copyright": "Copyright (c) 2023 Dracula Theme",
        "palette": {
            "black": ["mix", "282a36", "000000", 0.75],    # DERIVED
            "surface": "282a36",      # Background
            "surface_hi": "44475a",   # Current Line / Selection
            "surface_max": ["mix", "44475a", "f8f8f2", 0.90],  # DERIVED
            "outline": "6272a4",      # Comment
            "text": "f8f8f2",         # Foreground
            "muted": ["mix", "6272a4", "f8f8f2", 0.65],    # DERIVED
            "red": "bd93f9",          # Purple (Dracula's signature accent)
            "red_dim": ["mix", "bd93f9", "282a36", 0.30],  # DERIVED
            "green": "50fa7b",        # Green
            "panel": ["mix", "282a36", "000000", 0.82],    # DERIVED
        },
    },
}


def main():
    ap = argparse.ArgumentParser(description="Convert palettes to KimiOS .ktheme packs")
    ap.add_argument("palette", nargs="?", help="palette JSON file (11 slot keys)")
    ap.add_argument("-o", "--out", help="output .ktheme path")
    ap.add_argument("--pack", choices=sorted(PACKS), help="write a bundled scheme pack")
    ap.add_argument("--all", action="store_true", help="regenerate all bundled packs")
    ap.add_argument("--list", action="store_true", help="list bundled packs")
    ap.add_argument("--check", nargs="+", metavar="FILE", help="validate .ktheme/.theme files")
    args = ap.parse_args()

    if args.list:
        for name, p in sorted(PACKS.items()):
            print(f"{name:20} {p['scheme']}  [{p['license']}]")
        return 0

    if args.check:
        failed = False
        for f in args.check:
            ok, msg = check_ktheme(f)
            print(f"{'OK  ' if ok else 'FAIL'} {f}: {msg}")
            failed |= not ok
        return 1 if failed else 0

    if args.all:
        for name, p in sorted(PACKS.items()):
            out = THEMES_DIR / f"{name}.ktheme"
            write_ktheme(p["palette"], out)
            print(f"wrote {out}")
        return 0

    if args.pack:
        p = PACKS[args.pack]
        out = Path(args.out) if args.out else THEMES_DIR / f"{args.pack}.ktheme"
        write_ktheme(p["palette"], out)
        print(f"wrote {out}  ({p['scheme']}, {p['license']})")
        return 0

    if args.palette:
        palette = json.loads(Path(args.palette).read_text())
        out = Path(args.out) if args.out else Path(args.palette).with_suffix(".ktheme")
        write_ktheme(palette, out)
        print(f"wrote {out}")
        return 0

    ap.print_help()
    return 2


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, OSError) as e:
        print(f"error: {e}", file=sys.stderr)
        sys.exit(1)
