# Device test hooks (adb intents)

`MainActivity.maybeRunGuest` turns intent extras into scripted runs of the
libOS, so a device can be driven entirely from `adb` with no UI taps. Every hook
runs on its own thread and reports its result on **logcat tag `KimiOS-guest`**
(some also log on `KimiOS` / `KimiOS-plugin`).

```powershell
adb connect <device>:5555
adb logcat -c
adb shell am start -n com.kimios.libos/.MainActivity --es <key> <value>
adb logcat -d | Select-String 'KimiOS'
```

The hooks are checked **in a fixed order and the first match wins**, so supply
only one hook key per launch.

## Hooks

| Extra(s) | Value | What it does | Log line to grep |
|---|---|---|---|
| `openapp` | app kind (see table) | Opens that app screen | — |
| `sys` | any | `nativeSysTest` — sysinfo/proclist/copy probe | — |
| `kpkg` | any | `kpkg update` (market index refresh) | `kpkg update result=` |
| `install` (+`dest`) | asset name, `/disk/...` path | kpkg update + install asset; `dest` defaults to `/disk/fonts/<name>.kfont` | `install <name> result=` |
| `linux` (+`arg`) | binary path in the sysroot, argument | Runs an ELF from the Alpine sysroot | `linux_run <bin> exit=` |
| `api` | any | Starts the localhost plugin API, logs its port | `plugin api port=` |
| `imalpine` | Alpine package name | Installs it and wraps it as a `sysroot:` plugin | `import-alpine <pkg> result=` |
| `mstore` (+`q`) | source filter, search text | Store listing count + first 8 names | `mstore filter=<f> count=` |
| `mrun` (+`name`) | source filter, item name | Activates that Store row (install / run) | `mstore run name=... src=... result=` |
| `term` | any | PTY terminal smoke test, returns byte count | `term-test result=` |
| `plugin` (+`bg`) | plugin id, `1` for background | Runs an installed plugin | `plugin <id> result=` |
| `guest` (+`arg`) | `node` / `python3`, script text | Runs a native guest binary | `guest <g> exit=` |

> Note: an early commit message refers to `--es imalpines`. The implemented key
> is **`imalpine`** (no trailing `s`); `imalpines` is silently ignored.

### Store source filters (`mstore` / `mrun`)

| Value | Source |
|---|---|
| `0` | All (installed + Alpine catalog + npm catalog) |
| `1` | Installed plugins |
| `2` | Alpine catalog |
| `3` | npm catalog |

`mrun` results: the Rust hook returns `output_length + 1`, `0` means "row not
found". Output is truncated to the hook's buffer.

### `openapp` kinds

| | | | |
|---|---|---|---|
| 0 Music | 1 Clock | 2 Settings | 3 Calculator |
| 4 Notes | 5 Maps | 6 Photos | 7 Files |
| 8 Weather | 9 Terminal | 10 Browser | 11 APK |
| 12 DEX | 13 Market | 14 Monitor | 15 Plugins |
| 16 AI Studio | 17 OpenCode | | |

## Native harness equivalents

The C entry points behind these hooks are also called directly by
`ios/host-harness/*.c` on a host (no device), e.g.:

| C symbol | Purpose |
|---|---|
| `kimi_request_app(kind)` | Open an app (same numbering as `openapp`) |
| `kimi_test_store_count` / `_names` / `_run` | Store listing / row activation |
| `kimi_test_import_alpine` | Import an Alpine package as a plugin |
| `kimi_test_plugin_run` / `_out` / `_list` | Plugin run + captured output |
| `kimi_test_term` | PTY terminal smoke |
| `kimi_test_weather`, `kimi_test_vt`, `kimi_test_blit` | App/emulator probes |