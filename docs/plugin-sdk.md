# KimiOS Plugin SDK & Developer Guide

How to build, package, and install plugins for KimiOS. This guide covers
the plugin manifest, the localhost Plugin API, the Python SDK
(`tools/plugin_sdk/kimi.py`), and the example plugins in `market/plugins/`.

**Design reference:** [`docs/plugin-system.md`](plugin-system.md) (plugin
model, `.kplugin` package, registry, API, management). Implementation:
`user/libkimi/src/kplugin.rs` (manifest parsing, install, registry, run)
and the host guest loader (`SYS_PLUGIN_RUN` / `SYS_PLUGIN_API`).

---

## 1. Plugin model

A plugin is a **directory** installed to `/disk/plugins/<id>/` containing a
`plugin.json` manifest plus an entry (script or binary) and optional assets.
Plugins are executed by the existing guest loader — no core rebuild needed.
Three kinds:

| `type`    | Behavior                                                        |
|-----------|-----------------------------------------------------------------|
| `app`     | Appears in the launcher (Plugins page); tap runs it foreground. |
| `service` | Started in the background by the service manager when enabled.  |
| `ui`      | Applies a bundled theme/font via the existing theme/font system.|

Enable state lives in `/disk/plugins.cfg` (lines `<id> <0|1>`, default
enabled) and survives restart. The plugin directory holds the manifest;
the registry holds only enable state.

## 2. Manifest reference (`plugin.json`)

```json
{
  "id": "org.kimios.demo",
  "name": "Demo Plugin",
  "version": "1.0",
  "type": "app",
  "runtime": "python",
  "entry": "main.py",
  "args": ["--flag"],
  "permissions": ["fs", "net", "notify", "settings"],
  "description": "What it does",
  "ui": { "theme": "nord.ktheme", "font": "iosevka" }
}
```

| Field         | Required | Notes                                                        |
|---------------|----------|--------------------------------------------------------------|
| `id`          | yes      | Unique. No `/` or `..` (install rejects them). ≤ 48 bytes.   |
| `name`        | yes      | Display name. ≤ 48 bytes.                                    |
| `version`     | yes      | ≤ 16 bytes.                                                  |
| `type`        | yes      | `app` \| `service` \| `ui` (unknown values default to `app`).|
| `runtime`     | yes      | `native` \| `python` \| `node` (unknown defaults to native). |
| `entry`       | yes      | Path **relative to the plugin dir**.                         |
| `args`        | no       | Optional argv tail appended after the entry.                 |
| `permissions` | no       | Gates API endpoints (see §4).                                |
| `description` | no       | Short text shown in the Plugin Manager / Marketplace.        |
| `ui`          | type=ui  | `{theme, font}`; `theme`/`font` name bundled files.           |

Parsing notes (from `kplugin.rs`): the manifest is read into a 2048-byte
buffer and parsed with a compact JSON string extractor — keep manifests
small (< 2 KB), use plain string values, and avoid nested quotes. For
`type=ui`, `apply_ui` reads the `theme` value and loads
`/disk/plugins/<id>/<theme>` through the theme system, so `ui.theme`
should point at a **bundled file name** (e.g. `nord.ktheme`), not a
installed theme name.

### Runtime types

| `runtime` | How the host launches the entry        |
|-----------|----------------------------------------|
| `native`  | exec the entry directly (binary/script)|
| `python`  | `python3 <entry>` (guest stdlib only)  |
| `node`    | `node <entry>`                         |

Every launched plugin gets these env vars injected:

- `KIMI_API_PORT` — localhost Plugin API port (`0`/absent = unavailable)
- `KIMI_PLUGIN_ID` — the manifest `id`
- `KIMI_PLUGIN_DIR` — `/disk/plugins/<id>`

## 3. Plugin API (localhost HTTP, JSON)

Enabled plugins call a localhost-only HTTP server (per-plugin token).
Base URL: `http://127.0.0.1:$KIMI_API_PORT`.

| Method & path                  | Purpose                          | Permission |
|--------------------------------|----------------------------------|------------|
| `GET /api/info`                | `{os, version, plugin_id, api}`  | —          |
| `GET /api/fs/list?path=`       | List a directory (sandboxed `/disk`) | `fs`   |
| `GET /api/fs/read?path=`       | Read a file (sandboxed `/disk`)  | `fs`       |
| `POST /api/fs/write?path=`     | Write a file; content = request body | `fs`   |
| `POST /api/notify`             | `{title, body}` → shade notification | `notify`|
| `GET /api/settings?plugin=<id>`| Plugin-scoped key/value store (read) | `settings`|
| `POST /api/settings?plugin=<id>`| `{key, value}` (write)          | `settings`|

Permissions declared in the manifest gate which endpoints a plugin may
call (enforced by plugin-id token). FS access is sandboxed to `/disk`.
v1 is localhost-only with a per-plugin token.

## 4. Python SDK (`tools/plugin_sdk/kimi.py`)

Stdlib-only (`urllib` / `json` / `os`). Bundle `kimi.py` next to your entry
so the plugin is self-contained. See
[`tools/plugin_sdk/README.md`](../tools/plugin_sdk/README.md).

```python
from kimi import KimiPlugin, KimiApiError

p = KimiPlugin()                 # reads KIMI_API_PORT / _ID / _DIR
try:
    print(p.info())
    p.notify("Hello", "from my plugin")
    p.fs_write(p.path("out.txt"), "data")
    p.settings_set("interval", 30)
except KimiApiError as e:
    ...                          # fall back to local behavior
```

Methods: `info()`, `fs_list(path)`, `fs_read(path)`, `fs_write(path, data)`,
`notify(title, body)`, `settings_get(key=None)`, `settings_set(key, value)`.
Helpers: `available`, `path(name)`, `plugin_id`, `plugin_dir`, `port`.
All raise `KimiApiError` (`.status` set for HTTP errors) on failure.

CLI demo: `python3 kimi.py demo` (also `info`, `list`, `read`, `write`,
`notify`, `settings`, `set`).

**Graceful degradation:** the API server may be down. Always wrap calls in
`try/except KimiApiError` and fall back to local file I/O / `os.uname()` so
the plugin still does something useful offline. All examples do this.

## 5. Packaging & installation

A plugin ships as a **directory** (the `.kplugin` package). Two install
paths, both handled by the Plugin Manager / Marketplace / Files:

1. **Marketplace:** `market/repo.json` lists plugins with `"kind": "plugin"`
   and `"file": "plugins/<dir>"`. The entry is a directory tree served from
   the GitHub base; importing it installs the whole tree.
2. **Files / Plugin Manager import:** place the **extracted plugin
   directory** under `/disk/import` (e.g. via SAF), then install. The
   installer copies the tree to `/disk/plugins/<id>` and enables it.

> **Important:** `.kplugin` import expects an **extracted directory** under
> `/disk/import` (containing `plugin.json`), not a single file. The
> Marketplace / Files import handles directories. `install(src)` reads
> `<src>/plugin.json`, validates the `id`, and copies the tree.

Package layout:

```
my-plugin/
  plugin.json      # required manifest
  main.py          # entry referenced by manifest.entry
  kimi.py          # bundled SDK (for Python plugins)
  icon.png         # optional launcher icon
  ...assets
```

Scaffold a new plugin quickly:

```
python3 tools/make_plugin.py org.kimios.demo "Demo Plugin" \
    --type app --runtime python -o market/plugins/demo
```

## 6. Example plugins walkthrough (`market/plugins/`)

| Plugin          | type    | runtime | entry       | Demonstrates                        |
|-----------------|---------|---------|-------------|-------------------------------------|
| `hello-python`  | app     | python  | `main.py`   | SDK notify + fs_write + info        |
| `sysinfo-app`   | app     | python  | `main.py`   | info + fs_list report               |
| `notes-service` | service | python  | `service.py`| background loop + notify + settings |
| `nord-ui`       | ui      | native  | `apply.sh`  | bundled `.ktheme` via `ui.theme`    |

- **hello-python** — notifies "Hello from <name>", writes `out.txt` into the
  plugin dir, prints `/api/info`. Offline: skips notify, writes `out.txt`
  with local I/O.
- **sysinfo-app** — prints `/api/info` plus a `/disk` listing from
  `/api/fs/list`. Offline: falls back to `os.uname()` / `os.listdir()`.
- **notes-service** — every 60s (configurable via settings key `interval`)
  appends to `heartbeat.txt` and posts a throttled notification. Handles
  `SIGTERM`/`SIGINT` for clean shutdown; works offline via local writes.
- **nord-ui** — bundles `nord.ktheme` (11 `RRGGBB` lines, copied from
  `market/themes/nord.theme`); `ui.theme` points at it. `apply.sh` is a
  native hook that reports the palette; the host applies the theme via
  `apply_ui`. Theme slot order is documented in `market/THEMES.md`.

## 7. Verification

```
python3 -m json.tool market/plugins/<dir>/plugin.json   # manifest valid
python3 -m json.tool market/repo.json                   # repo valid
python3 -m py_compile market/plugins/<dir>/*.py         # examples compile
python3 tools/make_ktheme.py --check market/plugins/nord-ui/nord.ktheme
```

Run an example offline (API down) to confirm graceful degradation:

```
python3 market/plugins/hello-python/main.py
python3 market/plugins/sysinfo-app/main.py
```

Device bar (from the design doc): IMPLEMENTED → HOST VERIFIED → DEVICE
VERIFIED (install on device, enable, launch from launcher, call the API;
survives restart via the `/disk` registry).
