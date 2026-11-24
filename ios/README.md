# KimiOS for iPad — library-OS (native, no QEMU)

This is the **library-OS** build of KimiOS: the exact same pure-Rust userland
(`user/libkimi` + `user/kimi-gui` + apps + shell) that runs bare-metal, compiled
as a native ARM64 library and linked into a thin Swift/UIKit/Metal iOS shell.

No QEMU. No EL1. No MMU. No exception vectors. The GUI runs at **native speed**
on the iPad's CPU and renders through Metal.

## How it works

On bare metal, every OS service in libkimi is a `svc #0` trap to the KimiOS
kernel. The whole userland is `no_std` with **no allocator** — pure computation
over a framebuffer plus a small syscall surface. That makes it trivially
portable:

```
                 bare metal                         iPad (library-OS)
  kimi-gui  ──▶  libkimi syscall3/4/5         kimi-gui  ──▶  libkimi syscall3/4/5
                 │  svc #0  (cfg none)                         │  kimi_host_syscall()  (cfg host)
                 ▼                                             ▼
              KimiOS kernel (EL1)                     Swift host dispatcher
              MMU / virtio / ramfb                    Metal fb / UIKit touches /
                                                      mach time / sandbox
```

The only code that differs is three primitives in `user/libkimi/src/lib.rs`,
gated by `cfg(target_os = "none")`:

- `syscall3/4/5` → `svc #0` (bare metal) **or** `kimi_host_syscall(n, a0..a4)` (host)
- `cntpct()` → `mrs cntpct_el0` **or** `kimi_host_cntpct()`
- `exit()` → `wfe` park **or** host `SYS_EXIT`

Everything above libkimi is **byte-for-byte identical** across both builds. The
GUI entry is `_start` on bare metal and `kimi_gui_start` on host (renamed to avoid
colliding with the host C runtime's ELF entry symbol).

The iOS host (`ios/app/Sources/KimiHost.swift`) implements `kimi_host_syscall`
with `@_cdecl`, providing:

- **Display**: double Metal-shared framebuffer (1080×1920 XRGB8888). The GUI
  renders to `back`, publishes dirty rects to `front` on `fb_present`; the Metal
  renderer blits `front` → texture → screen. Rendering is **on-demand** (MTKView
  paused, redraw only on present) for low battery.
- **Input**: UIKit touches → virtio-style `EV_ABS`/`BTN_TOUCH` events (ABS range
  0..32767), same contract the GUI already expects.
- **Time**: `clock_gettime_nsec_np` for `uptime_ms`/`cntpct`, `gmtime_r` for `rtc`.
- **Filesystem** (`KimiFS.swift`): persistent `/disk/` backed by the app sandbox
  `Documents/disk` (full CRUD, directories, rename, stat, df) plus a read-only
  RamFS — matching the kernel's exact syscall semantics.
- **Networking** (`KimiNet.swift`): DNS via `getaddrinfo`, TCP via BSD sockets
  with connect/recv timeouts (Browser, `dns`, `fetch`, kpkg).
- **Process**: `ps`/`mem` report the native GUI process; `brk` over a malloc
  arena (the GUI itself uses no allocator).

All built-in apps and shell commands work natively. `spawn`/`run`/`exec` of
external ELF/PE images needs the JIT ELF loader (mode B, future work) and fails
gracefully here; APKs are inspected and DEX runs on the built-in mini-Dalvik VM,
exactly as on bare metal.

The Terminal app's VT/ANSI emulator, 256-colour style, cursor handling, key
strip and rendering are the shared Rust userland and are identical in the
`.ipa`. PTY-backed full-screen guests — **opencode**, vim, htop, etc. — are
Android-only: iOS app sandboxes do not permit `fork`/`exec` of downloaded
binaries. On iPad the terminal therefore shows the built-in shell and reports
the guest-execution limitation instead of attempting a broken PTY spawn. Use
the OpenCode app's remote/cloud model or a remote `opencode serve` endpoint for
OpenCode on iOS.

## Host service parity

The Swift dispatcher now implements the same non-guest host services as Android:
synchronous/async HTTPS fetch with progress and Bearer auth, CoreLocation,
AVAudioPlayer, UIDocumentPicker import, symlinks, file copy, system/process
info, local time and an offscreen WKWebView pixel bridge. See
[docs/ios-parity.md](../docs/ios-parity.md) for the full matrix.

## Verified natively (no Mac required)

`ios/host-harness/` implements the same dispatcher in C and runs the real GUI on
Linux aarch64, dumping PPM frames:

```bash
bash ios/host-harness/build.sh
# lock.ppm: 1 present, 96.8% non-black, ~970 colours
# home.ppm: 5 presents after a scripted unlock tap, ~53% pixels changed
```

This proves the userland renders and responds to touch natively, de-risking the
iOS build (identical staticlib + dispatcher contract).

The FS/net contract is verified end-to-end too: `ios/host-harness/shelltest` (a
`no_std` staticlib) runs the real libkimi shell against `harness_fs.c` (temp-dir
FS + BSD sockets):

```bash
cargo build --release --manifest-path ios/host-harness/shelltest/Cargo.toml \
  --target aarch64-unknown-linux-gnu
cc -O2 -o shelltest ios/host-harness/harness_fs.c \
  ios/host-harness/shelltest/target/aarch64-unknown-linux-gnu/release/libshelltest.a -lpthread -lm
./shelltest
# write/cat/stat/mkdir/ls/mv/rm/rmdir/df/ps all correct;
# dns example.com -> real IP; fetch example.com / -> HTTP 200 OK + body
```

## Build the .ipa

### GitHub Actions (recommended)

`.github/workflows/ios.yml` runs on `macos-latest`:

1. builds `libkimi_gui.a` for `aarch64-apple-ios`,
2. generates the Xcode project with `xcodegen`,
3. `xcodebuild` (unsigned, `iphoneos`),
4. packages `Payload/KimiOS.app` → `KimiOS.ipa`,
5. uploads the **KimiOS-iOS-ipa** artifact.

Trigger it manually:

```bash
GH_TOKEN=$(cat /opt/gh_token) gh workflow run ios.yml --repo Kimi-syl/kimios
```

### Locally on a Mac

```bash
bash ios/build-ipa.sh   # needs Xcode + Rust + xcodegen
```

## Install on the iPad

The CI `.ipa` is **unsigned** (no Apple cert in CI). Install with any of:

- **Sideloadly / AltStore / SideStore** — re-signs with your Apple ID (free
  developer account works; 7-day refresh).
- **StikDebug** — sideload + JIT. This build is pure ahead-of-time native code
  (App-Store-legal, no JIT required), but StikDebug is a convenient installer.
- **Your own cert** — sign the `.app` with `codesign` + a provisioning profile,
  or set `CODE_SIGN_IDENTITY` / team in `ios/app/project.yml` and rebuild.

Because the GUI is compiled-in native ARM64 (no interpreter, no JIT), it runs
with full performance and low overhead — the goal of the library-OS pivot.

## Layout

```
ios/
  app/
    Sources/KimiHost.swift        host syscall dispatcher (@_cdecl)
    Sources/Renderer.swift        Metal framebuffer presenter
    Sources/ViewController.swift  MTKView + touches + GUI thread
    Sources/AppDelegate.swift     UIKit app lifecycle
    Sources/main.swift            UIApplicationMain
    KimiosBridge.h                declares kimi_gui_start to Swift
    Info.plist
    project.yml                   xcodegen project definition
  host-harness/                   native Linux verification (C dispatcher + PPM)
  build-ipa.sh                    local/CI build script
```
