# KimiOS Marketplace repo

Package repo for the in-GUI **Marketplace** app (powered by [`kpkg`](../user/libkimi/src/kpkg.rs)).
Ships downloadable **themes** and **fonts**, plus example app entries.

The Marketplace works fully offline with the built-in themes (Nothing Dark,
Graphite Light, Ocean, Forest, Sunset, Lavender, Mono) and fonts (NType82,
Ndot-55, NType82 Mono, Pixel 8x8). Point it at this repo to install more.

## Serve it

kpkg speaks plain HTTP (no TLS). Serve this directory over HTTP and open the
Marketplace → **Refresh**:

```bash
cd market
python3 -m http.server 8080
```

The default repo base is `http://10.0.2.2:8080` (the QEMU/Android-frontend host
loopback). On a real device, serve from a host the device can reach and update
the `base` field in `repo.json` (or pass a URL to `kpkg update <url>`).

## repo.json

```json
{ "base": "http://10.0.2.2:8080",
  "packages": [
    { "name": "nord", "desc": "...", "kind": "theme", "file": "themes/nord.theme", "run": false },
    { "name": "pixel-bold", "desc": "...", "kind": "font", "file": "fonts/pixel-bold.kfont", "run": false },
    { "name": "hello", "desc": "...", "kind": "app", "file": "bin/hello", "run": true }
  ] }
```

- `kind`: `theme` | `font` | `app` (default `app`).
- `file`: path relative to `base`.
- `run`: for apps, whether the Linux ABI can execute it (static binary).

Install destinations: themes → `/disk/themes/<name>.theme`, fonts →
`/disk/fonts/<name>.kfont`, apps → `/disk/bin/<name>`.

## Asset formats

### `.theme` / `.ktheme`
11 hex colours (`RRGGBB`), one per line, in palette order:
`black, surface, surface_hi, surface_max, outline, text, muted, accent,
accent_dim, green, panel`. Parsed by `theme::load_theme_file` (same format
for both extensions; the parser reads only the first 256 bytes and accepts
nothing but the 11 hex lines — no comments).

## Theme packs (open-source schemes)

Redistributable packs of popular MIT-licensed color schemes, mapped onto the
11 UI slots. Full palette values, slot mappings, sources and the required
MIT copyright/permission notices: **[THEMES.md](THEMES.md)**.

| Pack | Scheme | License |
|------|--------|---------|
| `nord.ktheme` | [Nord](https://github.com/nordtheme/nord) — Sven Greb | MIT |
| `dracula.ktheme` | [Dracula](https://github.com/dracula/dracula-theme) — Zeno Rocha / Dracula Theme | MIT |
| `catppuccin-mocha.ktheme` | [Catppuccin](https://github.com/catppuccin/catppuccin) Mocha (dark) | MIT |
| `catppuccin-latte.ktheme` | Catppuccin Latte (light) | MIT |
| `gruvbox-dark.ktheme` | [Gruvbox](https://github.com/morhetz/gruvbox) dark, medium — Pavel Pertsev | MIT/X11 |
| `gruvbox-dark-hard.ktheme` | Gruvbox dark, hard contrast | MIT/X11 |
| `gruvbox-dark-soft.ktheme` | Gruvbox dark, soft contrast | MIT/X11 |
| `tokyo-night.ktheme` | [Tokyo Night](https://github.com/tokyo-night/tokyo-night-vscode-theme) — Enkia | MIT |
| `tokyo-storm.ktheme` | Tokyo Night Storm | MIT |
| `one-dark.ktheme` | Atom One Dark via [One Dark Pro](https://github.com/Binaryify/OneDark-Pro) | MIT |

`nord` / `dracula` keep their existing package names; the original
`nord.theme` / `dracula.theme` files remain for compatibility. `cyberpunk`
and `sakura` are original KimiOS palettes.

### Make your own theme

`tools/make_ktheme.py` converts a palette (JSON with the 11 slot keys) to a
`.ktheme`, documents the slot mapping, and can validate any theme file
exactly the way `theme.rs` parses it:

```bash
python3 tools/make_ktheme.py my-palette.json -o market/themes/my-theme.ktheme
python3 tools/make_ktheme.py --check market/themes/*.ktheme
python3 tools/make_ktheme.py --pack nord        # regenerate a bundled pack
```

### `.kfont`
Compact bitmap font (little-endian), two versions:

**v1 — `KFONT001`** (ASCII bitmap):
- magic `KFONT001` (8 bytes)
- `em: u16`, `glyph_count: u16` (= 95, ASCII 0x20..0x7e)
- 95 glyph records, **16-byte stride**: `advance: u16, x0: i16, y0: i16,
  w: u16, h: u16, offset: u32` (+ 2 padding bytes)
- alpha atlas: `w*h` coverage bytes per glyph at `offset` (0..255).

**v2 — `KFONT002`** (real outline fonts rasterized to an atlas, arbitrary Unicode):
- header (32 bytes): magic `KFONT002`, `em: u16`, `ascent: i16`, `descent: i16`,
  `line_gap: u16`, `glyph_count: u16`, `cmap_count: u16`, `flags: u16`
  (bit0 = monospace), reserved, `atlas_len: u32`, reserved.
- `glyph_count` records, same 16-byte stride as v1.
- `cmap_count` pairs, **8-byte stride**: `codepoint: u32, glyph_index: u16` (+2 pad),
  sorted by codepoint (the renderer binary-searches it).
- alpha atlas (`atlas_len` bytes).

Both parsed by `font_store::load` (v1 stays backward-compatible). The renderer
(`draw.rs`) iterates Unicode `chars` and resolves each via `Font::glyph_for`
(cmap, or ASCII fallback for the built-in atlases), scaling bilinearly to any
on-screen size. Caps: 512 glyphs, 128 KiB atlas (fixed BSS buffers, no allocator).

The bundled `pixel-bold` / `pixel-wide` are v1 8×8 variants from the public-domain
font8x8. **`iosevka`** is v2: the [Iosevka](https://github.com/be5invis/Iosevka)
coding monospace face (OFL-1.1), 352 glyphs (ASCII + Latin-1 + box-drawing),
rasterized at 28 px.

### Convert a TTF/OTF to `.kfont` v2

`tools/kfont_convert.py` (needs Pillow/FreeType) rasterizes any outline font:

```bash
python3 tools/kfont_convert.py Iosevka-Regular.ttc iosevka.kfont \
    --size 28 --cp 0x20-0xFF,0x2500-0x257F --name Iosevka
```

`--size` is the reference em (the GUI downscales bilinearly, so a modest size
stays sharp); `--cp` selects codepoint ranges. It emits metrics, a sorted cmap
and the alpha atlas. Reduce `--size`/`--cp` if the atlas exceeds 128 KiB.

## Regenerate the bitmap fonts

`pixel-bold` (dilated) and `pixel-wide` (wider advance) are derived from
`user/kimi-gui/src/font.rs` (font8x8). The built-in `Pixel 8x8` atlas lives in
`user/kimi-gui/src/font_data.rs` (`PIXEL`) and can be exported with
`font_store::export_pixel`.

## Alpine Linux `apk` compatibility (`apkg`)

kpkg also speaks **Alpine's `apk` format** via the `apkg` command (in the Terminal
or CLI), backed by [`user/libkimi/src/alpine.rs`](../user/libkimi/src/alpine.rs):

```text
apkg update [root-url]   fetch APKINDEX.tar.gz (main + community), default Alpine v3.20
apkg list                list indexed packages
apkg info <name>         version / size / depends / description
apkg install <name>      resolve deps, download .apk files, extract to /disk/alpine/root
```

Implemented:
- **APKINDEX.tar.gz** parsing (multi-member gzip + tar; the index spans gzip
  members — `.SIGN` in member 1, `APKINDEX` in member 2).
- **`.apk` packages** (control tar with `.PKGINFO` + data tar), streamed into the
  sysroot `/disk/alpine/root` with a 32 KiB-window streaming inflater — large
  packages need no big buffers.
- **Dependency resolution** including `so:` shared-library providers and `/bin/sh`
  virtual provides, with version-constraint stripping.
- Plain-HTTP downloads from the Alpine CDN (kpkg has no TLS), with retry/backoff.

Verified against the live Alpine v3.20 aarch64 repo: `musl`, `python3` (→
`usr/bin/python3.12`) and `nodejs` (→ `usr/bin/node`, a real aarch64 PIE with
interpreter `/lib/ld-musl-aarch64.so.1`) download and extract with their full
dependency closures.

**Execution caveat (honest):** Alpine binaries are *dynamically linked against
musl*. Downloading/extracting them works; **running** them additionally requires a
dynamic ELF loader (`PT_INTERP`/`DT_NEEDED`/relocations), musl libc on KimiOS
syscalls, and a fuller Linux ABI (real threads/futex/epoll/sockets/dlopen — the
current kernel ABI has futex as a no-op). That is the Desktop Lite runtime
milestone (see ROADMAP), not yet implemented. `apkg` prints this note on install.
