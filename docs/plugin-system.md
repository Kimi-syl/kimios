# KimiOS Plugin System

**Model:** native guest plugins. A plugin is a package (manifest + a Linux
binary or Python/Node script) executed by the existing guest loader
(`SYS_RUN_PATH` / `kimi_elf.c`). Sandboxed by the fork model, distributed via the
Marketplace, no core rebuild needed to add one. Plugins can be launchable apps,
UI extensions, background services, and can call KimiOS system services through a
localhost Plugin API.

## Plugin package (`.kplugin`)
A directory (or zip) installed to `/disk/plugins/<id>/`:
```
plugin.json      # manifest (required)
<main entry>     # binary or script referenced by manifest.entry
icon.png         # optional launcher icon (PNG)
...assets
```
`plugin.json`:
```json
{
  "id": "org.kimios.demo",
  "name": "Demo Plugin",
  "version": "1.0",
  "type": "app",                 // app | service | ui
  "runtime": "python",           // native | python | node
  "entry": "main.py",            // relative to plugin dir
  "args": ["--flag"],            // optional argv tail
  "permissions": ["fs","net","notify","settings"],
  "ui": { "theme": "nord", "font": "iosevka" },   // type=ui only
  "description": "…"
}
```
- `type=app`: appears in the launcher (Plugins page); tap runs it.
- `type=service`: started in the background by the service manager when enabled.
- `type=ui`: applies a bundled theme/font (reuses the existing theme/font system).
- `runtime`: `native` → exec entry directly; `python` → `python3 entry`; `node` → `node entry`.

## Registry
`/disk/plugins.json` — array of `{id, enabled}`. The plugin dir holds the manifest;
the registry holds enable state (survives restart).

## Plugin API (system hooks)
KimiOS runs a localhost HTTP **Plugin API server** (Java, `KimiApiServer.java`,
NanoHTTPD-style). Enabled plugins get env `KIMI_API_PORT=<port>` + `KIMI_PLUGIN_ID`
+ `KIMI_PLUGIN_DIR`. Endpoints (JSON):
- `GET /api/info` → `{os, version, plugin_id, api}`
- `GET /api/fs/list?path=` / `GET /api/fs/read?path=` / `POST /api/fs/write` (sandboxed to `/disk`)
- `POST /api/notify` `{title, body}` → posts a KimiOS notification (shade)
- `GET /api/settings` / `POST /api/settings` (plugin-scoped key/value)
Permissions in the manifest gate which endpoints a plugin may call (enforced by
plugin-id token). v1 keeps it simple: localhost-only, per-plugin token.

## Management
`user/libkimi/src/kplugin.rs` (no_std, fixed buffers) — pure Rust over existing FS
syscalls + `run_path`:
- `list`, `install(src)`, `set_enabled(id,on)`, `uninstall(id)`, `run(id)`,
  `start_services()`, `api_port()`.
New syscall `SYS_PLUGIN_API=53`: start/query the Plugin API server (Java), returns
port. Everything else reuses FS + `SYS_RUN_PATH`.

## GUI
- **Plugin Manager** app (`AppKind::Plugins`): list installed plugins, enable/disable
  toggle, uninstall, install from `/disk/import` (SAF) or Marketplace, run app plugins.
- **Launcher**: a Plugins page listing enabled `type=app` plugins; tap → run.
- **Marketplace**: `.kplugin` import (validate manifest → install) + a Plugins section.
- **Services**: enabled services auto-start on boot / Plugin Manager open.

## SDK / examples
`tools/plugin_sdk/` (Python helper wrapping the API) + `market/plugins/` example
plugins (a Python app, a native binary app, a UI theme plugin) + `docs/plugin-sdk.md`.

## Verification bar
IMPLEMENTED → HOST VERIFIED → DEVICE VERIFIED (install a plugin on the real
device, enable it, launch it from the launcher, call the API). Survives restart
(registry on /disk). Usable without Terminal (Plugin Manager + launcher).
