# KimiOS for Android — library-OS (native, no QEMU)

The **same pure-Rust KimiOS userland** (`user/libkimi` + `user/kimi-gui` + apps +
shell) that runs bare-metal, compiled as a native ARM64 library and linked into a
thin Java/SurfaceView Android shell. No QEMU, no EL1, no MMU, no exception
vectors — the GUI runs at **native speed** on the device CPU.

This is the Android sibling of [`../ios`](../ios/README.md). It is **distinct from
the QEMU frontend** in the separate `kimios-android` repo: that one emulates the
whole bare-metal kernel; this one runs the userland directly as a library.

## How it works

On bare metal every OS service in libkimi is a `svc #0` trap. The userland is
`no_std` with **no allocator** — pure computation over a framebuffer plus a small
syscall surface. The only bare-metal-specific code is three primitives in
`user/libkimi/src/lib.rs`, gated by `cfg(target_os = "none")`:

- `syscall3/4/5` → `svc #0` (bare metal) **or** `kimi_host_syscall(n, a0..a4)` (host)
- `cntpct()` → `mrs cntpct_el0` **or** `kimi_host_cntpct()`
- `exit()` → `wfe` park **or** host `SYS_EXIT`

Everything above libkimi is **byte-for-byte identical** across bare metal, iOS and
Android. The GUI entry is `_start` on bare metal and `kimi_gui_start` on host.

For Android the GUI is built as a `no_std` **staticlib** for
`aarch64-linux-android` and linked into `libkimios.so` together with a C dispatcher
(`jni/kimios_jni.c`) that implements `kimi_host_syscall` using NDK APIs:

- **Display**: `ANativeWindow` from the SurfaceView's `Surface` — a direct CPU
  framebuffer. The GUI renders to a back buffer, publishes dirty rects to a front
  buffer on `fb_present`; the host blits front → window (XRGB→RGBX swizzle) and
  posts. Rendering happens only when the GUI presents (low battery).
- **Input**: Activity `onTouchEvent` → JNI → virtio-style `EV_ABS`/`BTN_TOUCH`
  events (ABS range 0..32767), the contract the GUI already expects.
- **Filesystem**: persistent `/disk/` in `getFilesDir()/disk` (full CRUD, dirs,
  rename, stat, df) plus a read-only RamFS — same semantics as the kernel.
- **Networking**: DNS (`getaddrinfo`) + TCP (BSD sockets with timeouts) — Browser,
  `dns`, `fetch`, kpkg.
- **Time**: `clock_gettime` for `uptime_ms`/`cntpct`, `gmtime_r` for `rtc`.
- **Process**: `ps`/`mem` report the native GUI process. **Linux ELF guests run
  natively** — see [Native Linux execution](#native-linux-execution-desktop-lite)
  below and [`../docs/desktop-lite-native.md`](../docs/desktop-lite-native.md).

The Java side (`java/com/kimios/libos/`) is a minimal `SurfaceView` Activity that
aspect-fits the 9:16 framebuffer (letterbox), forwards touches, and starts the
native GUI thread. All app logic stays in Rust.

## Native Linux execution (Desktop Lite)

The Android host **is a Linux kernel**, so the libOS runs standard Linux
binaries directly — no QEMU, no syscall emulation. `jni/kimi_elf.c` forks a
child, maps a dynamically-linked musl aarch64 ELF + its `ld-musl` interpreter
with `mmap(NULL)` reservations, builds a SysV auxv/stack, and jumps to the
interpreter entry. The guest's real `svc #0` syscalls are serviced by the device
kernel: threads/futex/epoll/sockets/`mprotect`/`dlopen`/`/proc` all work natively.

Three platform mitigations (full detail in
[`../docs/desktop-lite-native.md`](../docs/desktop-lite-native.md)):

1. **SIGSYS→ENOSYS shim** — app seccomp answers blocked probes
   (`io_uring_setup`, `faccessat2`, …) with SIGSYS (death), not ENOSYS; Node dies
   at startup without it. Handlers are installed via **raw `rt_sigaction` +
   `SA_RESTORER`** to bypass bionic's sigchain, which would otherwise null-deref
   on the musl-hijacked `TPIDR_EL0` (the bug that initially blocked Node).
2. **targetSdk 28** (`untrusted_app_27`) — W^X `execmod` + sandbox `execve`.
3. **4 KiB pages** — musl maps Alpine libs 4 KiB-aligned.

`apkg install python3 nodejs` populates an Alpine sysroot in the sandbox
(`files/disk/alpine/root`, ~899 files); `SYS_SYMLINK` preserves the soname links
Node needs. `KimiGuest.java` builds argv/envp; `MainActivity` runs a guest from
an intent extra (`--es guest python|node|hello --es arg …`).

**Verified on the Andarium device (SDK 37, `untrusted_app_27`):**
`python3.14 -c 'import sys;print(sys.version,sum(range(101)))'` → `3.14.10 5050`,
exit 0; `node -e 'console.log(process.version,6*7)'` → `v24.22.3 42`, exit 0.

## Verified

- The FS/net contract is verified end-to-end natively (shared with iOS): a `no_std`
  shell-test staticlib + C dispatcher runs the real libkimi shell — file CRUD,
  nested dirs, RamFS, df, mv/rm/rmdir, ps, real DNS and HTTP fetch. See
  `../ios/host-harness/`.
- The GUI render path is verified natively on Linux aarch64 (lock + home frames,
  touch unlock). See `../ios/host-harness/build.sh`.
- The APK builds locally and in CI: signed, `lib/arm64-v8a/libkimios.so` exports
  all JNI methods + `kimi_gui_start`, full GUI embedded.

## Build the APK

### GitHub Actions (recommended)

`.github/workflows/android-libos.yml` runs on `ubuntu-latest`:

1. installs JDK 17 + Android SDK (build-tools 34, platform 34, NDK 26.1),
2. builds `libkimi_gui.a` for `aarch64-linux-android`,
3. links `libkimios.so` (NDK clang + C dispatcher),
4. `javac` + `d8` → `classes.dex`, `aapt2` → resources + manifest,
5. packages dex + lib, `zipalign`, `apksigner` (auto-generated debug key),
6. uploads the **KimiOS-libOS-APK** artifact.

Trigger manually:

```bash
GH_TOKEN=$(cat /opt/gh_token) gh workflow run android-libos.yml --repo Kimi-syl/kimios
```

### Locally

```bash
bash android/build-apk.sh   # needs JDK 17 + Android SDK + NDK + Rust
```

On the phone build host the script uses `/opt/tusr/bin/clang` (with
`TERMUX_EXEC__PROC_SELF_EXE` unset) and the arm-native `aapt2`/`zipalign`; on CI it
uses the NDK toolchain. Output: `android/build/KimiOS-libos.apk`.

## Install

The APK is signed with the **same debug key as the phone / frontend releases**
(`~/.android/debug.keystore`, cert SHA-256 `B1:33:9C:D3…`). Locally the build uses
that keystore directly; in CI it is provided via the `ANDROID_DEBUG_KEYSTORE`
secret (base64), so CI and local builds share one signature. Install with
`adb install -r KimiOS-libos.apk`, or sideload directly. Requires Android 8.0+
(API 26), arm64. Portrait.

> The libOS app (`com.kimios.libos`) is a separate package from the QEMU frontend
> (`com.kimios.frontend`), so it installs alongside it — but both are signed by the
> same key. To use your own release key, set `KIMIOS_KEYSTORE` (and adjust the
> `apksigner` aliases/passwords in `build-apk.sh`).

## Layout

```
android/
  jni/kimios_jni.c              C syscall dispatcher + JNI bridge (ANativeWindow)
  java/com/kimios/libos/        SurfaceView Activity + JNI declarations
  res/values/strings.xml
  AndroidManifest.xml
  build-apk.sh                  local/CI build script
```
