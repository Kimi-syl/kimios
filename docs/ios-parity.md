# iOS / Android feature parity

This matrix tracks the shared libOS userland against the two host dispatchers.
"Shared" means the code lives in `user/libkimi` + `user/kimi-gui` and is
compiled into both the APK and the IPA. "Android host" means the feature needs
the Android C dispatcher (JNI/guest loader); "iOS host" means the Swift
dispatcher implements it with Apple frameworks.

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
| PTY guest sessions (`SYS_PROC`) and full-screen TUIs (OpenCode, vim, htop) | They require the Linux guest runner above. |
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