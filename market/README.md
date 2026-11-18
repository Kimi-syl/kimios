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

### `.theme`
11 hex colours (`RRGGBB`), one per line, in palette order:
`black, surface, surface_hi, surface_max, outline, text, muted, accent,
accent_dim, green, panel`. Parsed by `theme::load_theme_file`.

### `.kfont`
Compact bitmap font (little-endian):
- magic `KFONT001` (8 bytes)
- `em: u16`, `glyph_count: u16` (= 95, ASCII 0x20..0x7e)
- 95 glyph records, **16-byte stride**: `advance: u16, x0: i16, y0: i16,
  w: u16, h: u16, offset: u32` (+ 2 padding bytes)
- alpha atlas: `w*h` coverage bytes per glyph at `offset` (0..255).

Parsed by `font_store::load`. The bundled `pixel-bold` / `pixel-wide` are 8×8
variants generated from the public-domain font8x8.

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
