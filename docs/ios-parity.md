# iOS / Android feature parity

This matrix tracks the shared libOS userland against the two host dispatchers.
"Shared" means the code lives in `user/libkimi` + `user/kimi-gui` and is
compiled into both the APK and the IPA. "Android host" means the feature needs
the Android C dispatcher (JNI/guest loader); "iOS host" means the Swift
dispatcher implements it with Apple frameworks.


> **Update — iOS executes a real Linux userland locally.** The IPA embeds
> **iSH-ARM64** (`OpenMinis/ish-arm64`), a threaded-code ARM64 Linux userspace
> interpreter with no JIT. Alpine is bundled as a fakefs rootfs, and
> `SYS_LINUX_RUN` / `SYS_RUN_PATH` plus streaming PTY-backed `SYS_PROC`
> sessions run real Linux processes. The actual OpenCode ARM64 musl binary has
> been executed inside iSH in CI (`opencode --version` → `1.19.6`). See
> `ios/deps/build_ish.sh`, `ios/deps/prepare_rootfs.sh`,
> `ios/deps/install_guest_runtime.sh`, and `ios/app/Sources/KimiISH.{h,m}`.
>
> iSH is **GPLv3**; the distributed IPA is a GPLv3 combined work. See `NOTICE`.
> Device runtime verification of the full OpenCode TUI is still required.

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
| `fork`/`exec` of arbitrary host binaries | iSH executes Linux ELF inside its emulated kernel; it does not grant Darwin `fork`/`exec`. |
| GPU acceleration / Metal inside the guest | The guest is a software ARM64 interpreter; Metal is available only to the host UI. |

## Practical OpenCode path on iPad

The OpenCode app's **Cloud** model mode uses the shared HTTP client and works on
the IPA. The terminal's VT emulator is available, but it intentionally reports
the guest-execution limitation instead of attempting a broken `SYS_PROC` call.

To get the OpenCode TUI on iPad, use one of:

1. a remote `opencode serve`/PTY endpoint reachable from Safari/WKWebView, or
2. a future in-app emulator (QEMU-TCG or equivalent) that hosts a Linux guest
   without relying on iOS `fork`/`exec`.

Neither is enabled by default in this build.