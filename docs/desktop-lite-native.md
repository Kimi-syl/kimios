# Desktop Lite, natively — running Linux binaries on the libOS

**Status: working on Android.** CPython 3.14 and Node.js 24 (Alpine, dynamically
linked against musl) execute at native speed inside the KimiOS Android app, on
the real device kernel. This is the Desktop Lite goal — run standard Linux
binaries on a phone — achieved the efficient way: no QEMU, no syscall emulation.

This document explains the design, the three platform mitigations that make it
work, the subtle bionic signal-delivery bug that blocked Node, and how to build,
deploy and verify it.

---

## 1. The key insight: Android already is a Linux kernel

The bare-metal/QEMU path (`src/linux.rs`, `src/elf.rs`) had to *emulate* the
Linux ABI: translate every `svc #0` into kernel services, and reimplement
threads, futex, epoll, sockets, `mprotect`, `dlopen`, `/proc` from scratch. That
is a large surface and the reason Node was far from running under QEMU.

On the **library-OS** the host is different. An Android app runs on a real
**aarch64 Linux kernel**. A musl-linked aarch64 ELF issues ordinary Linux
`svc #0` syscalls — and the Android kernel services them **natively**. So instead
of emulating the ABI we only need to *load* the binary and *jump* to it; the
kernel provides everything else:

| Need | QEMU path | Android libOS path |
|---|---|---|
| threads / `clone` / `futex` | implement in kernel | native |
| `epoll` / `eventfd` / `pipe2` | implement | native |
| sockets | implement | native (app `INTERNET`) |
| `mmap` / `mprotect` (JIT) | partial | native |
| `dlopen` (C extensions) | musl ldso + file mmap | native |
| `/proc/self/exe`, `/proc/self/maps` | absent | native |
| filesystem inode limit | KimiFS 64 inodes | host sandbox FS (no limit) |

The remaining work is **plumbing, not OS subsystems**: an ELF loader, a startup
stack/auxv, and three Android-specific mitigations (§3).

## 2. Architecture

```
Java Activity (MainActivity)
  └─ KimiGuest.run(which, arg)            builds argv/envp for python/node/hello
      └─ KimiBridge.nativeRunElf(...)     JNI
          └─ kimi_guest_spawn()           C, android/jni/kimi_elf.c
              ├─ pipe2 x2 (stdin, stdout+stderr)
              ├─ fork()
              │   └─ CHILD (raw syscalls only — fork-safe, no malloc/stdio):
              │       ├─ dup2 pipes onto 0/1/2; close inherited fds 3..1024
              │       ├─ install_sigsys_shim()      (§3.1, raw rt_sigaction)
              │       ├─ kimi_elf_load()            map exe + ld-musl (§4)
              │       └─ kimi_elf_enter()           mov sp, …; br interp_entry
              │           └─ guest runs: real svc #0 → Android kernel
              │               exit_group → kills ONLY the child
              └─ PARENT:
                  ├─ stdout reader pthread → logcat (tag KimiOS-guest)
                  └─ wait4(child) → exit code
```

### Why a forked child (not an in-process thread)

Running the guest on a sacrificial pthread inside the app process would also
work for loading, but three problems disappear with `fork()`:

1. **`exit_group`.** musl `exit()` ends in `SYS_exit_group`, which kills the
   *whole process*. In a child it kills only the guest; the app survives and
   reaps with `wait4`. In-process would need a seccomp `ERRNO` shim for
   `exit_group` plus sibling-thread teardown.
2. **TLS / `TPIDR_EL0`.** musl hijacks the thread pointer early
   (`__set_thread_area` → `msr tpidr_el0`). In a child there is no live bionic
   TLS to corrupt. (This same register is at the heart of the Node bug in §5.)
3. **V8 signal handlers.** Node/V8 installs a `SIGSEGV` handler for wasm traps
   via `rt_sigaction`, which would clobber ART's fault handler in a shared
   process. A child has no live ART to break.

`fork()` is allowed for Android apps: bionic implements it as
`clone(CLONE_CHILD_SETTID|CLONE_CHILD_CLEARTID|SIGCHLD)`, and `clone` is in the
app seccomp allowlist. Between `fork()` and guest entry the child uses **raw
syscalls only** (no bionic `malloc`/`stdio`), which is the multithreaded-fork
safety rule.

## 3. The three mandatory platform mitigations

### 3.1 SIGSYS → ENOSYS shim (Node dies without it)

Android's app zygote installs a seccomp-bpf **allowlist**. Blocked syscalls are
answered with `SECCOMP_RET_TRAP` → **SIGSYS → process death**, *not* `-ENOSYS`.

musl and libuv *probe* newer syscalls expecting `-ENOSYS` so they can fall back:

- `faccessat2` (439), `fchmodat2` (452) — musl, when flags ≠ 0
- `io_uring_setup` (425) — **libuv at loop init**. Alpine Node is musl-built, so
  libuv's `#if defined(__ANDROID_API__)` io_uring-disable guard is *off*; on a
  ≥ 6.1 kernel it calls `io_uring_setup` and would be killed instantly.
- `clone3` (435), `openat2` (437), `epoll_pwait2` (441)

Fix: a `SIGSYS` handler that, for `si_code == SYS_SECCOMP` and a blocked number,
sets `uc_mcontext.regs[0] = -ENOSYS` and returns. The syscall then "returns"
ENOSYS and musl/libuv take their fallbacks. (An extra seccomp filter cannot fix
this: filters chain and the highest-precedence action wins — the zygote's TRAP
outranks an added ERRNO.) **How the handler is installed matters critically —
see §5.**

### 3.2 targetSdkVersion 28 (`untrusted_app_27`)

Android 10 (targetSdk ≥ 29) removed two permissions from apps:

- `execve()` of files in the app home directory (`execute_no_trans`)
- executing modified file pages / text relocations (`execmod`)

Setting **targetSdk 28** places the app in the `untrusted_app_27` SELinux domain,
which retains both (this is exactly why Termux targets SDK 28). Verified on
device: `ps -Z` shows `u:r:untrusted_app_27:s0:c133,c256,c512,c768`.

Note: anonymous `execmem` (RW→RX JIT, needed by V8) is allowed for **all**
targetSdk on AOSP (`allow appdomain self:process execmem` — WebView/Chrome JIT
prove it). targetSdk 28 is the Termux-proven configuration and buffers against
OEM policy deviations; it means sideload-only distribution (already our model).

### 3.3 4 KiB pages

musl's ldso file-mmaps Alpine libraries at 4 KiB alignment. On a 16 KiB-page
kernel (an Android 15/16 option) those mappings fail `EINVAL`; Android's
`pageSizeCompat` backcompat covers the app's own linker, not musl. The loader
asserts `getpagesize() == 4096` (the test device reports 4096). A 16 KiB device
would need a 16 KiB-aligned sysroot.

## 4. The ELF loader (`kimi_elf.c`)

A C port of the QEMU-path loader (`src/elf.rs:407–560`), adapted for a host
address space:

- **`mmap(NULL)` reservations, not fixed bases.** The QEMU loader maps the exe
  at `0x400000` and interp at `0x60000000`; on Android those collide with
  ART/bionic. Instead we reserve each image's full `PT_LOAD` span with
  `mmap(NULL, span, PROT_NONE, MAP_PRIVATE|MAP_ANONYMOUS)` and let the kernel
  pick a free hole; `base = reservation - min_vaddr_page`. PIE (`ET_DYN`) loads
  at any base.
- **File-backed segments.** Per `PT_LOAD`: text mapped `RX` directly from the
  read-only file fd (no COW → no `execmod` needed), data `RW` private, the bss
  tail zeroed and pure-bss pages mapped anonymous. `MAP_FIXED` only inside our
  own reservation. The header page stays readable (musl `__dls2` reads its own
  `Ehdr` at base).
- **`PT_INTERP`.** Read the interpreter path (`/lib/ld-musl-aarch64.so.1`),
  prepend the sysroot, load it the same way. musl's ldso then self-relocates and
  loads `DT_NEEDED` libs via real `openat`/`mmap`/`mprotect` — we do no
  relocations ourselves.
- **SysV aarch64 startup stack + auxv.** A fresh 8 MiB anonymous RW stack with a
  `PROT_NONE` guard page (never the bionic pthread stack, never brk —
  `execstack`/`execheap` are neverallow'd). At the top, low→high:
  `argc | argv[] | NULL | envp[] | NULL | auxv pairs | AT_NULL | strings |
  16 random bytes | "aarch64"`. auxv carries real `AT_HWCAP`/`AT_HWCAP2`
  (`getauxval`), `AT_RANDOM` (`getrandom`), `AT_UID/GID`, `AT_PAGESZ`,
  `AT_PHDR/PHENT/PHNUM`, `AT_BASE` (interp base), `AT_ENTRY` (exe entry),
  `AT_EXECFN`.
- **Entry.** `mov sp, <stack>; br <interp_entry>`. musl `_dlstart` reads `sp`,
  self-relocates, loads libc + the exe, and calls `main`.

`LD_LIBRARY_PATH` is set explicitly to `<sysroot>/usr/lib:<sysroot>/lib` because
`/proc/self/exe` points at `app_process64` (we never `execve`), so musl's
`$ORIGIN` expansion would resolve wrong. `PYTHONHOME`/`PYTHONPATH` are set for
the same reason (CPython `getpath`).

## 5. The Node bug: bionic sigchain vs musl TLS (deep dive)

**Symptom.** Python ran on device immediately. Node segfaulted ~90 ms after
spawn, exit 139 (SIGSEGV). It ran fine on the host (aarch64 Linux/proot).

**Diagnosis.** A guest fatal-signal handler installed via bionic `sigaction()`
never logged — musl/V8 replaced it. So we used **ptrace from the parent**
(`PTRACE_TRACEME` in the child; parent `PTRACE_GETREGSET`/`GETSIGINFO` on the
signal stop), which a guest cannot clobber. It showed:

```
GUEST-PTRACE sig=31 … syscall=425          ← io_uring_setup probe (expected)
GUEST-PTRACE sig=11 pc=0x7fbaa210fe94 faultaddr=0x308
GUEST-MAP  7fbaa20d3000-… r-xp … /apex/com.android.runtime/lib64/bionic/libc.so
```

The fault was a **null dereference at offset `0x308` inside bionic `libc.so`** —
not musl, not Node.

**Root cause.** bionic `sigaction()` does not install the kernel handler
directly; it wraps it in bionic's **sigchain** dispatcher (so ART/debuggerd can
chain). That dispatcher reads bionic thread-local state via `TPIDR_EL0`. But the
guest thread's `TPIDR_EL0` belongs to **musl** (hijacked at ldso startup). The
moment the `io_uring_setup` SIGSYS probe was delivered, the sigchain dispatcher
dereferenced a bionic TLS structure that wasn't there → fault `0x308`. On the
host, glibc has no such sigchain, which is why it worked there.

**Fix.** Install guest signal handlers with **raw `rt_sigaction` +
`SA_RESTORER`**, bypassing bionic sigchain entirely:

- a naked `rt_sigreturn` trampoline (`mov x8, #139; svc #0`) as `sa_restorer`;
- a kernel-layout `struct k_sigaction { handler; flags; restorer; mask; }`
  passed to `SYS_rt_sigaction` directly;
- handlers compiled `__attribute__((no_stack_protector))` so they never read the
  TPIDR-relative stack canary, and touch no bionic TLS.

The `SIGSYS` handler now runs as a plain kernel handler with musl's TPIDR,
writes `-ENOSYS` to `regs[0]`, and returns through our trampoline. Node started
immediately after this change. (The fatal-signal diagnostic handlers are
installed the same way.)

## 6. Alpine sysroot + symlinks (`apkg`, `SYS_SYMLINK`)

`apkg` (`user/libkimi/src/alpine.rs`) tracks Alpine **latest-stable** (LTS):
`apkg update|list|info|install <pkg>`. It parses the multi-member gzip
`APKINDEX.tar.gz`, resolves `so:`/dependency providers, downloads `.apk` files
(3 gzip members: `.SIGN` tar, control tar, data tar) and streams the data tar
into `/disk/alpine/root`. On the libOS host that maps to the app sandbox
(`getFilesDir()/disk/alpine/root`) — a real filesystem with no inode cap.

A full `python3 + nodejs` sysroot is **899 files + 107 dirs (~99 MB)**.

**Symlinks.** Node's `DT_NEEDED` libraries use soname symlinks (`libz.so.1`,
`libicuuc.so.78`, …) and Python needs `usr/bin/python3`, `/bin/sh`. The original
extractor skipped all symlinks because bare-metal KimiFS has no symlink inode
type. Added **`SYS_SYMLINK` (40)** to the KimiOS ABI, implemented over host
`symlink()` in the Android dispatcher (`fs_symlink`) and host harness; bare metal
falls through to `ERROR` (unchanged). `TarSink` now creates typeflag-`'2'` links
on the host. After the fix a fresh install reports **0 symlinks skipped** and
both runtimes execute with no manual steps.

## 7. Build, deploy, verify

### Build the APK
```bash
export PATH=$HOME/.cargo/bin:/opt/jdk17/bin:$PATH
bash android/build-apk.sh        # → android/build/KimiOS-libos.apk
```
Compiles the Rust userland staticlib (`aarch64-linux-android`), links
`jni/kimios_jni.c` + `jni/kimi_elf.c` into `libkimios.so`, dexes the Java shell,
packages, zipaligns, signs (debug key). Manifest: `targetSdkVersion 28`,
`debuggable true` (for `run-as` deployment), `extractNativeLibs true`.

### Deploy the sysroot to the device
```bash
ADB=/opt/aapt2arm/platform-tools/adb
$ADB install -r -t android/build/KimiOS-libos.apk
$ADB shell settings put global settings_enable_monitor_phantom_procs false  # §8
$ADB shell am start -n com.kimios.libos/.MainActivity                        # creates files/disk
# build a sysroot tar on the host (apkg), then:
$ADB push sysroot.tar.gz /data/local/tmp/
$ADB shell "cat /data/local/tmp/sysroot.tar.gz | run-as com.kimios.libos sh -c 'cat > files/sysroot.tar.gz'"
$ADB shell "run-as com.kimios.libos sh -c 'mkdir -p files/disk/alpine/root && tar -xzf files/sysroot.tar.gz -C files/disk/alpine/root'"
```
The sysroot must live in the **app sandbox** (`app_data_file`) — that is the
context SELinux grants `execute` to. `/data/local/tmp` (`shell_data_file`) is
readable but not executable by apps.

### Run a guest
```bash
$ADB shell "am start -n com.kimios.libos/.MainActivity --es guest python --es arg 'print(12345)'"
$ADB shell "am start -n com.kimios.libos/.MainActivity --es guest node   --es arg 'console.log(6*7)'"
$ADB logcat -d -s KimiOS-elf:* KimiOS-guest:*
```
`MainActivity` is `singleTop`; the intent extra `guest` (`hello`/`python`/`node`)
+ `arg` triggers `KimiGuest.run` on a background thread via `onNewIntent`.

### Verified results

Host (aarch64 Linux, `gcc -DKIMI_ELF_NO_JNI` harness against the same C loader):
```
hello_dyn                 → hello from dynamic musl! argc=1     exit 0
python3.14 -c print(…)    → hello from python / 3.14.10         exit 0
node -e console.log(…)    → hello from node / v24.22.3          exit 0
```

Device (Andarium Virtual Device, SDK 37, 4096-byte pages, `untrusted_app_27`):
```
KimiOS-elf: guest spawn pid=3819 exe=…/usr/bin/python3.14
KimiOS-guest: PY-OK  3.14.10 5050          ← import sys; print(sys.version, sum(range(101)))
KimiOS-elf: guest pid=3819 exited code=0
KimiOS-elf: guest spawn pid=3838 exe=…/usr/bin/node
KimiOS-guest: NODE-OK v24.22.3 42          ← console.log(process.version, 6*7)
KimiOS-elf: guest pid=3838 exited code=0
```

## 8. Limitations & future work

- **Phantom process killer (Android 12+).** Forked children of apps are tracked
  and culled (>32 system-wide, or "excessive CPU" when backgrounded) — Termux's
  notorious `signal 9`. Disabled on the test device via the `settings` command
  above (Developer Options → "Disable child process restrictions"). Production
  hardening: run the guest in a dedicated registered app process
  (`android:process=":guest"` Service) — not a phantom process, and `exit_group`
  killing that one process is acceptable.
- **iOS.** The same loader design ports to Swift/Darwin, but iOS forbids `fork`
  for apps and has stricter JIT/W^X (StikDebug-style enablement needed). The
  iOS libOS currently runs the GUI/shell; native guest execution is future work.
- **`/proc/self/exe` = `app_process64`.** Cosmetic for running scripts
  (`process.execPath` reports the wrong path); `PYTHONHOME`/`LD_LIBRARY_PATH`
  compensate. npm resolution may need absolute `--prefix`.
- **stdin / REPL.** v1 captures stdout/stderr only (stdin closed). Interactive
  `python3 -i` needs an stdin pipe wired to the GUI terminal.
- **GUI integration.** Guests are triggered by intent extra today; wiring
  `run /disk/<binary>` in the KimiOS shell / a Terminal app to `nativeRunElf`
  is the next UX step.
- **16 KiB-page devices** need a 16 KiB-aligned sysroot (§3.3).
- **QEMU path unchanged.** `src/linux.rs` still has the `set_robust_list`/
  `sysinfo` aliasing bug and stub `prlimit64`/`readlinkat`; Python hello there is
  close, Node far off. The native libOS path supersedes it for the product.

## 9. File map

| File | Role |
|---|---|
| `android/jni/kimi_elf.c` | ELF loader, SIGSYS shim, raw-signal install, forked-child spawn, JNI `nativeRunElf` |
| `android/jni/kimios_jni.c` | GUI host dispatcher; `fs_symlink` (`SYS_SYMLINK`) |
| `android/java/…/KimiGuest.java` | argv/envp builders for python/node/hello |
| `android/java/…/KimiBridge.java` | `nativeRunElf` declaration |
| `android/java/…/MainActivity.java` | intent-extra guest trigger (`singleTop`/`onNewIntent`) |
| `android/AndroidManifest.xml` | targetSdk 28, debuggable, extractNativeLibs |
| `android/build-apk.sh` | adds `kimi_elf.c` to the native link |
| `user/libkimi/src/alpine.rs` | `apkg`; 3-member `.apk` extraction; symlink creation |
| `user/libkimi/src/lib.rs` | `SYS_SYMLINK` (40) + `symlink()` wrapper |
| `ios/host-harness/harness_apkg.c` | host `apkg` harness; `SYS_SYMLINK`; `build_sysroot.sh` |
| `src/elf.rs`, `src/linux.rs` | QEMU-path loader/ABI (the port source; unchanged here) |
