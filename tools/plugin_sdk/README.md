# KimiOS Plugin SDK (Python)

`kimi.py` is a tiny, **stdlib-only** client for the KimiOS localhost
Plugin API. It uses nothing beyond `urllib`, `json`, and `os`, so it runs
on the guest `python3` with no dependencies. Design reference:
[`docs/plugin-system.md`](../../docs/plugin-system.md); full developer
guide: [`docs/plugin-sdk.md`](../../docs/plugin-sdk.md).

## Environment

The host injects these into every launched plugin:

| Variable          | Meaning                                   |
|-------------------|-------------------------------------------|
| `KIMI_API_PORT`   | Plugin API port (`0`/absent = server down)|
| `KIMI_PLUGIN_ID`  | The plugin's manifest `id`                |
| `KIMI_PLUGIN_DIR` | Install dir, `/disk/plugins/<id>`         |

`KimiPlugin()` reads all three automatically.

## Bundle it

Copy `kimi.py` next to your entry script so the plugin is self-contained:

```
my-plugin/
  plugin.json
  main.py        # from kimi import KimiPlugin
  kimi.py        # copied from tools/plugin_sdk/kimi.py
```

## API

```python
from kimi import KimiPlugin, KimiApiError

p = KimiPlugin()
try:
    info = p.info()                     # GET  /api/info
    entries = p.fs_list("/disk")        # GET  /api/fs/list?path=
    text = p.fs_read("/disk/a.txt")     # GET  /api/fs/read?path=
    p.fs_write("/disk/a.txt", "hi")     # POST /api/fs/write?path=  (body=data)
    p.notify("Title", "Body")           # POST /api/notify {title,body}
    all_settings = p.settings_get()     # GET  /api/settings?plugin=<id>
    one = p.settings_get("interval")    # just one key (None if unset)
    p.settings_set("interval", 30)      # POST /api/settings?plugin=<id>
except KimiApiError as e:
    print("API unavailable:", e)        # degrade gracefully
```

Helpers:

- `p.available` — `True` when `KIMI_API_PORT` is set to a nonzero port.
- `p.path("out.txt")` — absolute path inside `KIMI_PLUGIN_DIR` (cwd fallback).
- `p.plugin_id`, `p.plugin_dir`, `p.port`, `p.base_url`.

Every method raises `KimiApiError` (with `.status` for HTTP errors) when
the server is unreachable or returns an error, so one `except` block lets
you fall back to local behavior. `fs_read` returns raw text, or the
`data`/`content` field if the server wraps it in JSON.

## CLI demo

```
python3 kimi.py demo                       # info + notify + settings round-trip
python3 kimi.py info
python3 kimi.py list /disk
python3 kimi.py read /disk/notes.txt
python3 kimi.py write /disk/notes.txt "hello"
python3 kimi.py notify "Title" "Body"
python3 kimi.py settings [KEY]
python3 kimi.py set KEY VALUE
```

Without `KIMI_API_PORT` the CLI reports the API as unavailable and exits
nonzero — the same graceful path plugins should take.

## Examples

Working plugins live in [`market/plugins/`](../../market/plugins/):
`hello-python` (app), `sysinfo-app` (app), `notes-service` (service),
`nord-ui` (ui). Each Python example bundles its own copy of `kimi.py`.
