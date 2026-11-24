# iOS / Android feature parity

This matrix tracks the shared libOS userland against the two host dispatchers.
"Shared" means the code lives in `user/libkimi` + `user/kimi-gui` and is
compiled into both the APK and the IPA. "Android host" means the feature needs
the Android C dispatcher (JNI/guest loader); "iOS host" means the Swift
dispatcher implements it with Apple frameworks.


> **Update — iOS can now execute Linux binaries.** The IPA embeds **iSH**
> (`OpenMinis/ish-arm64`), a userspace Linux ARM64 emulator (threaded-code
> interpreter, no JIT), with an Alpine fakefs rootfs. `SYS_LINUX_RUN` /
> `SYS_RUN_PATH` boot that guest headlessly and run `/bin/sh -c <cmd>`, so
> terminal commands (`git`, `curl`, `apk`, `node`, …) execute on device without
> `fork`/`exec` of guest ELF. See `ios/deps/build_ish.sh`,
> `ios/deps/prepare_rootfs.sh`, `ios/app/Sources/KimiISH.{h,m}`.
>
> Consequence: iSH is **GPLv3**, so the distributed IPA is a GPLv3 combined
> work. Build/packaging is CI-verified; runtime guest boot is not yet
> device-verified. Interactive PTY TUIs still use the remote-PTY path.

## Implemented on both

| Feature | Shared userland | Android host | iOS host |
|---|---|---|---|
| Framebuffer, input, Metal/ANativeWindow display | yes | yes | yes |
| Filesystem `/disk/` + RamFS CRUD | yes | yes | yes |
| TCP/DNS sockets | yes | yes | yes |
| Local time / RTC / uptime / sleep | yes | yes | yes |
| `SYS_SYMLINK` | yes | yes | yes |
| Synchronous URL fetch | yes | yes | yes |
| Async download + progress + Bearer auth | yes | yes | yes |
| Location fix | yes | yes | yes (CoreLocation) |
| Open URL (Safari / WebView) | yes | yes | yes |
| Audio play/pause/stop/query | yes | yes | yes (AVAudioPlayer) |
| File import picker | yes | yes | yes (UIDocumentPicker) |
| System info / process list | yes | yes | yes |
| File copy | yes | yes | yes |
| Embedded browser/WKWebView pixel bridge | yes | Android WebView | WKWebView snapshots |
| VT/ANSI terminal renderer, colours, cursor, key strip | yes | yes | yes |

## Android-only / not currently possible on a sandboxed IPA

| Feature | Why |
|---|---|
| Local Linux ELF execution (`SYS_LINUX_RUN`, `SYS_RUN_PATH`) | iOS app sandboxes do not permit `fork`/`exec` of downloaded binaries; the Darwin kernel cannot service a Linux AArch64 guest's `svc` ABI. |
| Local PTY guest sessions (`SYS_PROC`) and full-screen TUIs (OpenCode, vim, htop) | They require the Linux guest runner above. A **remote PTY backend** is implemented instead: see [ios-remote-terminal.md](ios-remote-terminal.md). |
| Plugins and AI Studio local `llama-server` | Same guest runner dependency. |
| `spawn`/`exec`/`wait`/`kill` process model | iOS apps are single-process under the sandbox. |
| Native local OpenCode `serve` / OpenCode TUI | OpenCode is a Linux ARM64 Bun/JSC executable; iOS has no compatible local loader/kernel ABI. |

## Practical OpenCode path on iPad

The OpenCode app's **Cloud** model mode uses the shared HTTP client and works on
the IPA. The terminal's VT emulator is available, but it intentionally reports
the guest-execution limitation instead of attempting a broken `SYS_PROC` call.

To get the OpenCode TUI on iPad, use one of:

1. a remote `opencode serve`/PTY endpoint reachable from Safari/WKWebView, or
2. a future in-app emulator (QEMU-TCG or equivalent) that hosts a Linux guest
   without relying on iOS `fork`/`exec`.

Neither is enabled by default in this build.