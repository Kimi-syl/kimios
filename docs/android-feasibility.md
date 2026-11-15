# Can KimiOS run Android APKs? — A feasibility study

Date: 2026-11-15 · Method: primary-source research only (official Android/AOSP docs, AOSP
source trees, DEX spec, FreeBSD/Microsoft docs, real GitHub projects). No code was written
or modified for this study. AOSP line numbers below were measured from `frameworks/base`
branch `main` as fetched on the date above; they drift between releases.

---

## 1. Executive verdict

**KimiOS cannot run real Play-Store APKs, and no realistic amount of hobby effort will
change that.** An APK is not a self-contained program: it is DEX bytecode plus resources
that are *instantiated and driven by the Android framework* (ActivityThread, Context,
Looper, the View/Window system, Resources/AssetManager), which in turn lives on top of
`system_server` (ActivityManagerService, WindowManagerService, PackageManagerService, …),
the Binder IPC driver, ART, libcore, Bionic libc, and a Linux kernel with Android-specific
features. Android apps literally have no `main()` — the framework is the entry point
([App fundamentals](https://developer.android.com/guide/components/fundamentals)).
Every working "Android on non-Android" solution in existence either (a) runs a **full
Android system** in a container or VM on a Linux kernel (Anbox, Waydroid, WSA, the Android
emulator), or (b) is a **massive translation layer on top of an already-rich host kernel**
(FreeBSD Linuxulator, WSL1, and the recent experimental LiquidAPK on iOS). None is a small
runtime, and none runs on a bare-metal hobby kernel.

**What IS achievable, and genuinely worthwhile:** a bounded "mini Dalvik" path —
an APK inspector, a DEX parser/disassembler, and a minimal register-based DEX interpreter
with a hand-written micro core library that runs **purpose-built DEX** (compiled from your
own Java via `d8`, or hand-written smali) on KimiOS, printing to the existing console or
drawing through the existing framebuffer syscalls. This is real, precedented (multiple
public projects exist, including in Rust), and scoped so a solo developer can finish it.
It will **not** run arbitrary real apps, and this report is explicit about where the wall is.

The honest one-liner: **"run simple purpose-built DEX in a mini VM" is bounded and
achievable; "run arbitrary real APKs" requires effectively porting Android (framework +
ART/libcore + Linux ABI), which is open-ended and team-scale.**

---

## 2. APK anatomy (Q1)

An `.apk` is a ZIP archive containing everything an app needs at runtime
([App fundamentals](https://developer.android.com/guide/components/fundamentals)). Parts:

| Part | What it is | What it takes to use it |
|---|---|---|
| `classes.dex` (+ `classes2.dex`…) | Dalvik bytecode, the app's compiled Java/Kotlin. Each DEX file is limited to 64K method references, hence multidex ([d8 docs](https://developer.android.com/studio/command-line/d8)). | A DEX VM (interpreter or compiler) **plus the Java core library** (libcore) the code calls into. See §3. |
| `AndroidManifest.xml` | **Binary XML (AXML)**, not text. Declares components, permissions, min/target SDK. The system must read it before any component can start ([fundamentals](https://developer.android.com/guide/components/fundamentals)). | An AXML parser. Format is defined by AOSP's [`ResourceTypes.h`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/androidfw/include/androidfw/ResourceTypes.h) (chunk headers, string pools). Bounded to parse; produced by [aapt2](https://developer.android.com/tools/aapt2). |
| `resources.arsc` | Binary resource table (strings, layouts refs, configs) compiled by aapt2 ([aapt2 docs](https://developer.android.com/tools/aapt2)). | An ARSC parser + a `Resources`/`AssetManager` implementation to resolve IDs per device configuration. On Android this is native C++ (`androidfw`), driven by Java [`AssetManager`](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/content/res/AssetManager.java) via `ApkAssets`. |
| `res/`, `assets/` | Compiled binary XML layouts, drawables, raw files. | Layout inflation = the entire View system (§5). Raw assets are just ZIP entries. |
| `lib/<abi>/*.so` | Native ELF shared libraries, e.g. `lib/arm64-v8a/libfoo.so`. The package manager extracts the matching ABI at install time ([NDK ABI docs](https://developer.android.com/ndk/guides/abis)). | These are **AArch64 ELF built against Bionic** (Android's own libc/libm/libdl and `linker64`, not glibc — [bionic README](https://android.googlesource.com/platform/bionic/+/refs/heads/main/README.md)). KimiOS's ELF loader could map them, but they expect Bionic symbols, Android's linker namespaces, JNI, and Linux syscalls. |
| `META-INF/` | v1 JAR-signing manifests/signatures. Modern APKs also carry a whole-file v2/v3 signing block; the PackageManager verifies signatures at install ([APK signing](https://source.android.com/docs/security/features/apksigning)). | Verification is optional for a hobby loader (you control what you load), but note real APKs are signed and some apps self-check. |

Practical notes for KimiOS: most APK entries are DEFLATE-compressed, so even an inspector
needs an inflate implementation (bounded, well-understood). KimiOS already has the storage
path (virtio-blk + KimiFS) and an ELF loader; ZIP+AXML+ARSC parsing is incremental work,
not new OS infrastructure.

---

## 3. DEX execution and minimal interpreters (Q2)

### The format and VM model

The DEX format is fully specified and stable:
[DEX format spec](https://source.android.com/docs/core/runtime/dex-format). Key facts:

- Header magic `dex\n039\0` (current mainstream version); versions: 035 (classic),
  037 (default methods, Android 7), 038 (`invoke-polymorphic`/`invoke-custom`, Android 8),
  039 (`const-method-handle`/`const-method-type`, Android 9), 040 (Android 10),
  041 (experimental container format, Android 16). A mini VM should target 035/037.
- Sections: `string_ids`, `type_ids`, `proto_ids`, `field_ids`, `method_ids`, `class_defs`,
  `data` (code items, tries/handlers, debug info). Strings use Modified UTF-8.
- Each method's `code_item` declares `registers_size`, `ins_size`, `outs_size`, and an
  array of 16-bit instruction units.

The [Dalvik bytecode spec](https://source.android.com/docs/core/runtime/dalvik-bytecode)
defines a **register-based** VM: 32-bit registers, adjacent pairs for 64-bit values,
arguments land in the last N registers, fixed-size frames, 16-bit instruction stream.
Of 256 opcode slots, ~224 are defined today (218 in classic DEX 035; the six
`invoke-polymorphic`/`invoke-custom`/method-handle opcodes were added in 038/039).
Many opcodes are just width variants of the same operation (`add-int`, `add-long`,
`add-int/2addr`, `add-int/lit8`, …), so the *semantic* surface is much smaller than the
opcode count suggests.

### How hard is a minimal interpreter?

For **purpose-built bytecode**, a bounded interpreter is very realistic:

- A hello-world needs only: `const-string`, `sget-object` (`System.out`),
  `invoke-virtual` (`PrintStream.println`), `return-void`, plus `move*` plumbing.
- A useful subset (arithmetic, arrays, fields, branches, all five `invoke-*` kinds,
  `new-instance`, exceptions/`try`-handlers, monitors) is on the order of 100–150 opcodes.
  Real data point: [DaliVM](https://github.com/fatalSec/DaliVM) implements **127+ opcodes**
  in ~258 KB of Python and executes targeted methods out of real APKs with **mocked**
  Android APIs — no Android runtime.
- You can skip: the bytecode verifier (trust your own DEX; Dalvik itself supported
  `-Xverify:none`, as used by [dalvik-universal](https://github.com/A2OH/dalvik-universal)),
  JIT/AOT, multidex (initially), `invoke-custom`/method handles (D8 desugars Java 8
  lambdas into ordinary classes — [Java 8 support docs](https://developer.android.com/studio/write/java8-support);
  `MethodHandle.invoke` needs API 26+ and is rare in app code).
- You cannot skip: class loading/resolution, object layout, a heap (even hello-world
  allocates via `StringBuilder`), and *some* core library (below). Monitors can be no-ops
  while single-threaded; GC can start as bump-allocation and grow into mark-sweep later.

### Real minimal/educational DEX interpreters (all public, verified)

| Project | Language / size | Scope / status |
|---|---|---|
| [Sandvik](https://gitlab.com/christophe.duvernois/sandvik) (GitHub mirror: [yag00/sandvik](https://github.com/yag00/sandvik)) | C++ (~1.45 MB C++ + 0.45 MB C of VM code) | The most complete "mini Dalvik": own DEX/APK parser, runs **AOSP 12 core libraries** (`core-oj`, `core-libart`, ICU, Conscrypt, BouncyCastle), JNI incl. loading `.so`, threads, monitors, GC, reflection, virtual Android filesystem/properties, **experimental Zygote boot**. Still "work in progress". Its README's hello-world trace shows exactly what trivial DEX pulls in: `PrintStream.println` → `monitor-enter` → `print` → … |
| [A2OH/dalvik-universal](https://github.com/A2OH/dalvik-universal) | C/C++ (KitKat Dalvik fork) | Port of KitKat's **portable interpreter** (no JIT) to x86_64 Linux and OpenHarmony ARM32/AArch64; 46+ fixes for 64-bit (register width `u4`→`dreg_t`, JNI, GC); boot classpath of ~4,000 `java.*` classes; `libcore_bridge.cpp` = 1,658 lines / 30+ JNI natives; claims 62,153 lines of AOSP framework shims. Self-reported results; demo-app scope. |
| [fatalSec/DaliVM](https://github.com/fatalSec/DaliVM) | Python (~258 KB) | 127+ opcodes, multi-DEX, executes *target methods* (string decryption for static analysis), mocks `Context`/`PackageManager`/reflection, hooks `String`/`StringBuilder`/`Integer`/`Math`/collections. Not a general app runtime. |
| [celymyst/Mihon-Runner](https://github.com/celymyst/Mihon-Runner) | Rust (~158 KB) | DEX parser + interpreter to run **Mihon (Tachiyomi) extension** DEX — a real-world but *fixed, narrow* API surface. Good existence proof for Rust. |
| [maekoos/aar](https://github.com/maekoos/aar) ("Alternative APK Runtime") | Rust (~142 KB) | Explicit proof-of-concept: runs some DEX with a very limited instruction subset; author states it is far from the full instruction set. |
| [notmyst33d/dalvik](https://github.com/notmyst33d/dalvik) | Rust (~27 KB) | Experimental; DEX parsing + instruction decode done, interpretation/stdlib still TODO. Useful as a cautionary scope marker. |
| [cycheng/simple-dvm](https://github.com/cycheng/simple-dvm) | C | University course homework "simple dalvik vm" extended to run Dhrystone-like benchmarks. |
| [jjfiv/dalvik-js](https://github.com/jjfiv/dalvik-js) | JavaScript | CS-course Dalvik VM implementation. |
| [vimalloc/dexterpreter](https://github.com/vimalloc/dexterpreter) | Java | Small educational `.dex` interpreter (2012). |

Tooling that makes the bounded path easy: [smali/baksmali](https://github.com/google/smali)
(assembler/disassembler for DEX, Google-maintained fork) and
[d8](https://developer.android.com/studio/command-line/d8) (Java bytecode → DEX).
Rust DEX *parsers* you can study or reuse: [dexrs](https://github.com/MatrixEditor/dexrs),
[SUPERAndroidAnalyzer/dalvik](https://github.com/SUPERAndroidAnalyzer/dalvik),
[androguard/dex-parser](https://github.com/androguard/dex-parser).

### What core library does even trivial DEX need?

Android's core library is [libcore](https://android.googlesource.com/platform/libcore/)
(`ojluni` = OpenJDK-derived `java.*`, plus `luni`, `libart` extensions). Even
`System.out.println("hi")` transitively needs `Object`, `String`, `StringBuilder`,
`System`, `PrintStream`, `CharSequence`, exception types, and monitor semantics — the
Sandvik trace above demonstrates this concretely. A mini VM has two options:

1. **Hand-written micro-libcore** (a few dozen classes in Rust or interpreted Java):
   bounded, and the right choice for KimiOS. This is what DaliVM/aar/Mihon-Runner do.
2. **Run the real libcore**: thousands of classes, JNI natives into ICU, Bionic, and the
   runtime itself. Sandvik and dalvik-universal show this is possible but is a multi-year
   effort even on a POSIX host — it is *not* the bounded path.

Kotlin raises the bar further: real Kotlin apps bundle `kotlin-stdlib` and AndroidX
(e.g. `androidx.appcompat` — see the default
[build configuration](https://developer.android.com/studio/build)), multiplying the
library surface. Purpose-built Java-only DEX avoids all of that.

---

## 4. ART vs Dalvik (Q3)

[ART overview](https://source.android.com/docs/core/runtime): ART executes the same DEX
format as Dalvik ("apps developed for Dalvik should work when running with ART"). What ART
adds is **performance and tooling**, not new bytecode semantics:

- **AOT compilation** (`dex2oat` at install/build time) and, since Android 7, a **hybrid
  AOT + JIT + interpreter** with profile-guided recompilation
  ([ART configuration](https://source.android.com/docs/core/runtime/configure)).
  Compiler filters: `verify`, `quicken`, `speed`, `speed-profile`. Artifacts: `.vdex`,
  `.odex`, `.art`. ART = compiler (`dex2oat`) + runtime (`libart.so`) loaded at boot.
- **Improved GC** (mostly concurrent, concurrent copying, pause independent of heap size),
  better diagnostics, sampling profiler, debugging features
  ([ART overview](https://source.android.com/docs/core/runtime)).
- Tighter install-time **verification** and class loading.

**Can a plain interpreter skip ART? Yes.** DEX is the contract; interpretation is a
conforming execution mode (ART itself still interprets non-compiled methods —
[ART configuration](https://source.android.com/docs/core/runtime/configure)). Dalvik's
portable interpreter was pure C and has been ported to non-Android systems
([dalvik-universal](https://github.com/A2OH/dalvik-universal)); ART has been ported to
Darwin by [Multi-OS Engine](https://github.com/multi-os-engine/moe-core) (builds `dex2oat`
and MOE.framework for iOS/macOS). But "skip ART" does **not** mean "skip a runtime": you
still must provide class loading, object model, GC, core library, and (for real apps) JNI.

**ART's hard dependencies** (why you can't just drop `libart.so` into KimiOS):

- **Bionic** — Android's libc/libm/libdl and `linker64`
  ([bionic README](https://android.googlesource.com/platform/bionic/+/refs/heads/main/README.md)),
  not glibc/musl; Android `.so` files and ART itself link against it.
- **Linux kernel facilities**: threads, futex, mmap, epoll, eventfd, timerfd, memfd,
  signals, sockets, and userfaultfd (used by ART's GC). This exact list is documented by
  the LiquidAPK project, which had to implement every one of them to run ART on iOS
  ([apk-on-iphone README](https://github.com/heyobi/apk-on-iphone)).
- The **boot classpath** (libcore jars), ICU, and the surrounding framework; on a device
  ART is started via Zygote from `init`, with system properties, `/system`, `/data`, etc.

---

## 5. The Android framework wall (Q4)

### What a "Hello World" Activity actually executes

Android apps have **no `main()`**; the system starts the process and instantiates
components ([fundamentals](https://developer.android.com/guide/components/fundamentals)).
Traced in AOSP `frameworks/base` (branch `main`, line numbers measured 2026-11-15):

1. **`ActivityThread.main()`** ([ActivityThread.java:8907](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/app/ActivityThread.java))
   → `Looper.prepareMainLooper()` (L8927) → `thread.attach()` (L8943), which calls
   **`ActivityManager.getService().attachApplication(mAppThread)`** (L8632) — a **Binder
   IPC into `system_server`'s ActivityManagerService**. The app is now registered with AMS.
2. AMS schedules the launch back into the app: `handleLaunchActivity` (L4385) →
   `performLaunchActivity` (L4062) → `createBaseContextForActivity` (L4318), building a
   **[ContextImpl](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/app/ContextImpl.java)**
   (3,888 lines; `getSystemService` at L2300 is the gateway to every system service).
3. The `Activity` (10,019 lines) is attached to a
   **[PhoneWindow](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/com/android/internal/policy/PhoneWindow.java)**
   (4,191 lines; `setContentView` L522 creates the `DecorView`), and
   `onCreate` runs the app's code.
4. `setContentView` inflates binary-XML layouts through `LayoutInflater` →
   **[Resources](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/content/res/Resources.java)**
   (2,870 lines) / **[AssetManager](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/content/res/AssetManager.java)**
   (1,860 lines) → native `androidfw` C++ that parses `resources.arsc` out of the APK.
5. The view tree is attached to a
   **[ViewRootImpl](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/ViewRootImpl.java)**
   (13,507 lines; `mSurface` L832, `IWindowSession` L558 — Binder to
   **WindowManagerService**), which drives measure/layout/draw of
   **[View](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/View.java)**
   (34,460 lines) and renders via HWUI's
   **[RenderThread/CanvasContext](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/hwui/renderthread/)**
   into a `Surface` composited by SurfaceFlinger.
6. The main **Looper/Handler** loop ([Looper.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/os/Looper.java),
   601 lines) dispatches all of this; its native `MessageQueue` is built on Linux
   **epoll** ([system/core/libutils/Looper.cpp](https://android.googlesource.com/platform/system/core/+/refs/heads/main/libutils/Looper.cpp):
   `epoll_create1` L129, `epoll_wait` L222).

Meanwhile `system_server`
([SystemServer.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/java/com/android/server/SystemServer.java),
3,663 lines) has started AMS (L1220), PackageManagerService (L1313),
WindowManagerService (L1702) and **214 `mSystemServiceManager.startService(...)` calls**
in the file. Apps reach these services through Binder/AIDL
([AIDL docs](https://developer.android.com/develop/background-work/services/aidl)).

### How big is the surface?

Measured samples (single files, branch `main`):

| File | Lines |
|---|---:|
| View.java | 34,460 |
| ViewRootImpl.java | 13,507 |
| Activity.java | 10,019 |
| ActivityThread.java | 9,061 |
| PhoneWindow.java | 4,191 |
| ContextImpl.java | 3,888 |
| SystemServer.java | 3,663 |
| Resources.java | 2,870 |
| AssetManager.java | 1,860 |
| Looper.java | 601 |

~84K lines in just these ten files — before widgets, graphics natives, HWUI,
SurfaceFlinger, the other ~200 system services, and the SDK itself. Scale markers:

- The public SDK package index lists **hundreds of packages** (`android.*`, `java.*`,
  `javax.*`, …): [Package index](https://developer.android.com/reference/packages).
  The Westlake project estimates **~57,000 Android APIs**
  ([westlake README](https://github.com/A2OH/westlake) — project estimate, not official).
- Zygote's default **preloaded-classes list contains 17,808 classes**
  ([config/preloaded-classes](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/config/preloaded-classes),
  counted 2026-11-15) — that's the *warm-start* working set of a typical app.

**Why app code can't run without it:** an Activity's code is callbacks
(`onCreate`, click listeners) executed *by* the framework; `Context`, `Resources`,
`Intent`, view inflation, and lifecycle are all framework-provided. Strip the framework
and there is nothing to call the app's code, nothing to resolve `R.layout.*`, and no
window to draw into. This is why "stub the framework" only works for apps you wrote
against your stubs.

---

## 6. Linux kernel dependencies (Q5)

Android userspace is built on Linux, plus Android-specific kernel features:

- The Android kernel is an upstream **Linux LTS kernel + Android patches** (Android Common
  Kernels / GKI): [Kernel overview](https://source.android.com/docs/core/architecture/kernel).
- The GKI defconfig for arm64 enables, among others:
  **`CONFIG_ANDROID_BINDER_IPC=y`, `CONFIG_ANDROID_BINDERFS=y`, `CONFIG_ASHMEM=y`,
  `CONFIG_SECURITY_SELINUX=y`**
  ([android15-6.6 gki_defconfig](https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/arch/arm64/configs/gki_defconfig)).
  The Binder driver lives in `drivers/android/binder.c` / `binderfs.c` in the same tree.
- **Binder** is the IPC mechanism for essentially every app↔system interaction
  ([AIDL](https://developer.android.com/develop/background-work/services/aidl);
  `attachApplication` trace in §5). Anbox's install docs state plainly: "You need a kernel
  that's compiled with binder and ashmem support", verified via `/dev/binder` and
  `/dev/ashmem` ([anbox docs/install.md](https://github.com/anbox/anbox/blob/master/docs/install.md)).
- **SELinux** is enforcing by default and defines the app sandbox on top of per-app Linux
  UIDs ([SELinux docs](https://source.android.com/docs/security/features/selinux);
  [fundamentals](https://developer.android.com/guide/components/fundamentals)).
- **ashmem/memfd** for shared memory (ashmem still in GKI config; newer userspaces use
  memfd — LiquidAPK lists memfd among required syscalls).
- **futex, epoll, eventfd, timerfd, signals, mmap semantics, /proc, /sys**: the Looper is
  epoll-based (§5); the syscall list ART/Bionic actually exercise is enumerated by
  LiquidAPK's kernel layer: "threads…, futex, mmap…, epoll, eventfd, timerfd, memfd,
  sockets, signals, userfaultfd for ART's GC"
  ([apk-on-iphone README](https://github.com/heyobi/apk-on-iphone)). FreeBSD's
  Linuxulator likewise must mount Linux-flavored `/proc` and `/sys` (linprocfs/linsysfs)
  for Linux apps ([FreeBSD handbook ch.12](https://docs.freebsd.org/en/books/handbook/linuxemu/)).

KimiOS today has none of these: its syscall ABI is ~27 custom calls (write/exit/spawn/
brk/fb_* …, per the repo README). Bridging that gap is not "a few syscalls" — it is a
Linux-compatible kernel ABI *plus* Binder *plus* the SELinux/UID security model.

---

## 7. Translation-layer approaches, and what they imply for KimiOS (Q6)

| System | How it actually works | What had to be implemented |
|---|---|---|
| **FreeBSD Linuxulator** | Optional Linux ABI compatibility in the FreeBSD kernel; unmodified Linux binaries run as native processes against a Linux userland installed under `/compat/linux` ([handbook ch.12](https://docs.freebsd.org/en/books/handbook/linuxemu/)). | Per-architecture syscall translation (x86 32/64 **and AArch64**), Linux ELF branding, linprocfs/linsysfs, futex/epoll/etc. semantics. **Still incomplete after 20+ years**: "Some Linux-specific operating system features are not yet supported… such as cgroups or namespaces"; systemd doesn't work. Note: even this mature layer only recently covers AArch64. |
| **WSL1** | A syscall **translation layer** built by Microsoft inside the NT kernel — no Linux kernel. Microsoft's own comparison: WSL1 lacks "full system call compatibility"; WSL2 replaced it with a **real Linux kernel in a lightweight VM** precisely because translation hit limits (Docker etc.) ([Comparing WSL versions](https://learn.microsoft.com/en-us/windows/wsl/compare-versions), [What is WSL](https://learn.microsoft.com/en-us/windows/wsl/about)). | A team of kernel engineers re-implementing Linux syscall semantics on NT, plus a Linux userland. Even so: not full compatibility. |
| **Anbox** (deprecated 2023) | **Full Android 7.1.1 system in an LXC container** on the host Linux kernel (user/pid/uts/net/mount/ipc namespaces); hardware access proxied to a host daemon reusing the QEMU emulator's OpenGL pipes ([anbox README](https://github.com/anbox/anbox)). | binder+ashmem kernel modules, container integration, emugl/qemud pipes — *not* an Android reimplementation. README also notes **Google Play Store doesn't work because it relies on proprietary Google Play Services, which are not part of the AOSP image**. |
| **Waydroid** | Same idea, modern: LXC container booting a **full Android 13 (LineageOS-based) image**, Wayland integration ([waydroid README](https://github.com/waydroid/waydroid), [docs](https://docs.waydro.id/)). | Requires a Linux kernel with Binder; again, full Android, not a mini runtime. |
| **WSA (Windows Subsystem for Android)** | Microsoft ran Android apps on Windows 11 via a **VM "which provides compatibility with the AOSP framework"** (Android 13, `SDK_INT 33`). **Deprecated March 5, 2025** ([archived Microsoft docs](https://web.archive.org/web/20250525105910/https://learn.microsoft.com/en-us/windows/android/wsa/)). | Even Microsoft — with the WSL1 translation experience — chose full Android + virtualization for APK compatibility. |
| **Android Emulator** | QEMU-based full system images ([emulator docs](https://developer.android.com/studio/run/emulator)). | Google's own desktop answer is also "run all of Android". |

**Implication for KimiOS:** a Linuxulator/WSL1-style layer would have to provide, on top
of a kernel that currently has ~27 custom syscalls: a substantial Linux syscall ABI
(futex, epoll, signals, mmap/clone semantics, /proc, …), the Binder driver, ashmem/memfd,
SELinux-ish labeling, Bionic + `linker64`, ART + libcore, Zygote, and `system_server` —
i.e., layers 4–5 of §9. Every precedent shows this is **open-ended**: FreeBSD (decades,
still partial, and Android needs *more* than generic Linux: Binder/namespaces/SELinux),
Microsoft (gave up on translation for WSL2; gave up on WSA entirely). KimiOS's existing
PE/WinAPI translation (README phases 12/23: 9 `kernel32` thunks to run a generated
`hello.exe`) is a nice analogy for the *shape* of the problem — and for the scale
difference: 9 thunks for a hello-world vs. thousands of framework APIs plus a kernel ABI
for Android.

---

## 8. Existing precedents: Android/DEX on non-Android systems (Q7)

**No hobby/bare-metal kernel is known to run real APKs.** The closest real projects:

- **[Sandvik](https://gitlab.com/christophe.duvernois/sandvik)** — minimal Dalvik VM in
  C++ on Linux x86_64, running real AOSP 12 core libraries, JNI, threads, GC, reflection;
  experimental Zygote boot. The strongest "mini Android runtime" precedent. Limits:
  still WIP; host is Linux; no claim of running arbitrary Play apps.
- **[A2OH/dalvik-universal](https://github.com/A2OH/dalvik-universal)** +
  **[Westlake](https://github.com/A2OH/westlake)** — KitKat Dalvik ported to 64-bit and
  OpenHarmony ARM32/AArch64 (static binaries, "no Android kernel, no bionic, no
  system_server"); Westlake embeds **193K lines of unmodified AOSP framework (166 files)
  + 2,056 shim classes**, bridging to OHOS at ~15 HAL-level boundaries, and reports a
  "MockDonalds" demo APK rendering native Android Views **using the phone's own ART**.
  Limits: self-reported, demo-app scope, and the host OS already provides a JVM/ART and a
  rich kernel. (Treat these claims as unverified but architecturally plausible.)
- **[LiquidAPK / apk-on-iphone](https://github.com/heyobi/apk-on-iphone)** — the most
  ambitious data point: real APKs on a **non-jailbroken iPhone**, no JIT. Architecture:
  an AArch64 **interpreter** (~1 MB of C: CPU + Linux syscalls + **in-process Binder and
  servicemanager + SurfaceFlinger + gralloc**) running **unmodified AOSP 14 GSI binaries**
  (linker64, ART, framework.jar, libhwui), with system services rewritten in Java
  in-process (~314 KB), GPU via ANGLE-on-Metal. Self-reported results: Qalculate!,
  cube.run (libGDX), NewPipe UI, WhatsApp to registration screen; **no Google Play
  services, camera, or notifications**. Interpreted speed ~80–90 M guest instr/s;
  NewPipe cold start 43.5 s (25.5 s after guest-side dex2oat AOT). Key lesson: even on
  iOS — which already provides threads, VM, sockets, a GPU stack — the "kernel + binder +
  SurfaceFlinger" shim alone is ~1 MB of C, and the full AOSP runtime is still required.
  On bare-metal KimiOS, *everything* under that line would also have to be built.
  (Recent, low-visibility project; claims are self-reported, code is public.)
- **[DaliVM](https://github.com/fatalSec/DaliVM)**, **[Mihon-Runner](https://github.com/celymyst/Mihon-Runner)**,
  **[aar](https://github.com/maekoos/aar)** — partial DEX execution with mocked/limited
  libraries (see §3). These prove the *bounded* approach works, including in Rust.
- **[AstoriaUWP](https://github.com/cbialorucki/AstoriaUWP)** — a proof-of-concept "APK
  interpreter for UWP" (2017, unmaintained); educational, never ran real apps at scale.
- **[Multi-OS Engine](https://github.com/multi-os-engine/multi-os-engine)** — Java apps on
  iOS using a ported **ART** ([moe-core](https://github.com/multi-os-engine/moe-core)
  builds dex2oat for iOS/macOS) with iOS-native UI bindings — i.e., ART *can* live on a
  non-Linux kernel, but with substantial porting and **without the Android framework**.
- **[RoboVM](https://github.com/MobiVM/robovm)** (Java bytecode AOT → native for iOS) and
  **[Avian](https://github.com/ReadyTalk/avian)** (lightweight embeddable JVM, now
  unmaintained) — adjacent precedents for "Java on non-Android", both requiring their own
  runtime/class-library work; neither runs APKs.
- **[webos_dalvik](https://github.com/kai4785/webos_dalvik)** — historical attempt to run
  Dalvik on WebOS (itself Linux-based).

Pattern: **bounded DEX execution = many small successes; arbitrary APKs = only full
Android (container/VM) or enormous translation stacks on rich host kernels.**

---

## 9. Layered breakdown with effort estimates

Estimates are **rough, for a solo hobbyist working part-time**, and assume KimiOS's
current base (ELF loader, processes/page tables, KimiFS, framebuffer, ~27 syscalls).
"Bounded" = finishable with a fixed, written-down scope; "open-ended" = scope grows with
every real app you try.

| Layer | Content | Effort (solo, part-time) | Bounded? | Evidence/precedent |
|---|---|---|---|---|
| **L0 — APK inspector** | ZIP+inflate, DEX header/section dump, AXML + resources.arsc decode, manifest listing | weeks–2 months | **Bounded** | Formats fully specified (§2); aapt2 `dump` as oracle |
| **L1 — DEX parser/disassembler** | Full 035/037 parse, smali-style output in no_std Rust | 1–3 months | **Bounded** | dexrs / SUPERAndroidAnalyzer/dalvik / androguard dex-parser (§3) |
| **L2 — Minimal DEX interpreter + micro-libcore** | ~100–150 opcodes, class loading, bump heap → simple GC, single thread, no-op monitors; hand-written `Object/String/StringBuilder/System/PrintStream/Math/…`; runs **purpose-built** DEX from smali or javac+d8; output via KimiOS console/fb syscalls | 6–18 months | **Bounded if scope is frozen** | aar, Mihon-Runner, DaliVM, simple-dvm (§3); Sandvik hello trace shows the exact libcore floor |
| **L3 — Real core library + JNI + threads + GC** | Run actual libcore (core-oj/core-libart), JNI into native libs, real monitors/threads/reflection | 2–5+ years | **Effectively open-ended solo** | Sandvik (multi-year, still WIP, on Linux); dalvik-universal (46+ 64-bit fixes, 30+ JNI bridges) |
| **L4 — Framework stubs for real Activities** | Context/Looper/Resources/Window/View + fake AMS/WMS/PMS so *some* trivial APKs launch | years | **Open-ended** (bounded only for a fixed, self-written app set) | §5 LOC wall; Westlake needed 193K lines of vendored AOSP + 2,056 shims + host ART for one demo app |
| **L5 — Linux-ABI translation + Android userspace** | Linuxulator/WSL1-style syscall layer + Binder + Bionic/linker64 + ART + Zygote + system_server on KimiOS | many years | **Open-ended** | FreeBSD: decades, still no cgroups/namespaces; WSL1 → WSL2 retreat; LiquidAPK: ~1 MB C shim + full AOSP on Darwin, still no GMS (§7–8) |
| **L6 — Full AOSP port** | Port ACK + the whole platform to KimiOS (or replace its kernel interface) | team-years | **Not a solo project** | AOSP build alone: 400 GB disk, ≥64 GB RAM, ~40 min on 72 cores ([build requirements](https://source.android.com/docs/setup/build/requirements)) |

Note on **networking**: a TCP/IP stack (e.g. [smoltcp](https://github.com/smoltcp-rs/smoltcp),
a standalone event-driven bare-metal stack that needs no heap) is a sensible future
KimiOS feature, but it is **orthogonal** to APK compatibility — apps fail at the
framework wall long before sockets matter.

---

## 10. Recommended staged path

1. **Stage 0 — APK inspector (L0).** Read APKs from KimiFS, list entries, dump the DEX
   header, decode AXML manifest (package name, activities, permissions) and the resource
   table. Demo: `apkinfo /disk/app.apk` in the shell. Validates ZIP/inflate/format work
   with zero runtime risk. Cross-check output against `aapt2 dump` on a host.
2. **Stage 1 — DEX disassembler (L1).** Produce smali-style listings; diff against
   `baksmali`. You now have a test oracle and a debugging tool for everything later.
3. **Stage 2 — Mini DEX VM (L2).** Implement the register machine for the frozen opcode
   subset; load DEX directly (no APK needed); implement the micro-libcore in Rust;
   `println` goes to the KimiOS console, later simple drawing via `fb_fill`. Generate
   test DEX with `javac` + `d8` (or hand-write smali) — **purpose-built bytecode only**.
   Add a simple mark-sweep GC, then arrays/exceptions/monitors as needed. Run the same
   interpreter as host-side unit tests (see §11) before booting it on KimiOS.
4. **Stage 3 (optional) — one bounded real-world niche.** Follow the Mihon-Runner model:
   pick a *fixed, documented* DEX API surface (e.g. a specific extension format) and
   implement exactly this. Still bounded, because the contract is fixed — unlike "apps".
5. **Explicit non-goal: real Play-Store apps.** They need Kotlin stdlib/AndroidX, the full
   framework, GMS (proprietary, not in AOSP — [Anbox README](https://github.com/anbox/anbox)),
   and the Linux/Binder substrate. If the owner ever truly wants arbitrary APKs, the only
   proven route is **running full Android as a guest** (the WSA/emulator/Waydroid model):
   i.e., KimiOS becoming a VM host for a Linux+Android image — a different, large project
   (EL2/virtualization, virtio-net, etc.), not a translation layer. State this plainly in
   the roadmap so expectations stay honest.

Success criteria to keep the project honest: Stage 2 is "done" when a fixed corpus of
self-written DEX programs (arithmetic, strings, arrays, exceptions, small algorithms)
runs correctly on-device — **not** when any arbitrary APK launches.

---

## 11. GitHub Actions: where CI helps and where it doesn't (Q8)

Runner facts ([GitHub-hosted runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)):
standard public-repo Linux runners are **4 vCPU / 16 GB RAM / 14 GB SSD**, free and
unlimited for public repos (private repos get 2 vCPU / 8 GB plus included metered
minutes); **nested virtualization is not officially supported**
([runner overview](https://docs.github.com/en/actions/using-github-hosted-runners/using-github-hosted-runners/about-github-hosted-runners)),
though GitHub notes Linux runners support hardware acceleration for Android SDK tools
(i.e., the Android emulator can run in CI).

**CI genuinely helps:**

- Cross-compiling the no_std Rust VM/kernel for `aarch64-unknown-none` on every push;
  caching the toolchain. Small builds fit the 14 GB disk easily.
- **Host-side conformance testing of the interpreter**: run the same VM code on the
  runner against a corpus of DEX compiled by `d8`/smali, with expected outputs — this is
  the single highest-value CI job for Stage 2, and it needs no QEMU at all.
- QEMU smoke tests: boot KimiOS headless (`qemu-system-aarch64`, software emulation is
  fine for boot/syscall tests), assert serial output; `screendump`-based framebuffer
  assertions for GUI regressions (the repo already uses this technique).
- Building test fixtures (d8, baksmali), lint/format, and regression suites (e.g. the
  Dhrystone-style benchmarks simple-dvm used).
- Optionally: running the *real* Android emulator in CI as an oracle for how real DEX
  behaves — useful for comparison, but it tests Android, not KimiOS.

**CI does NOT reduce engineering/porting scope:**

- It cannot make the framework wall smaller (§5) or implement Binder/Linux ABI (§6–7).
  Compute doesn't substitute for design and code.
- **Building ART/AOSP is out of reach of free CI**: AOSP requires ≥400 GB disk and ≥64 GB
  RAM ([requirements](https://source.android.com/docs/setup/build/requirements)) vs.
  14 GB SSD / 16 GB RAM runners. Even a standalone ART build pulls in large chunks of the
  platform; plan host builds or self-hosted runners if you ever go there.
- Heavy app-compatibility test suites (thousands of APKs) presuppose the runtime exists;
  CI multiplies testing throughput, it doesn't create compatibility.

Bottom line: use Actions aggressively for L0–L2 (build + host conformance + QEMU smoke),
and don't budget CI as a lever for L3+.

---

## 12. Source register (primary unless noted)

Android/AOSP docs: [DEX format](https://source.android.com/docs/core/runtime/dex-format) ·
[Dalvik bytecode](https://source.android.com/docs/core/runtime/dalvik-bytecode) ·
[ART overview](https://source.android.com/docs/core/runtime) ·
[ART configuration](https://source.android.com/docs/core/runtime/configure) ·
[App fundamentals](https://developer.android.com/guide/components/fundamentals) ·
[aapt2](https://developer.android.com/tools/aapt2) · [d8](https://developer.android.com/studio/command-line/d8) ·
[Java 8 desugaring](https://developer.android.com/studio/write/java8-support) ·
[NDK ABIs](https://developer.android.com/ndk/guides/abis) ·
[APK signing](https://source.android.com/docs/security/features/apksigning) ·
[AIDL](https://developer.android.com/develop/background-work/services/aidl) ·
[SELinux](https://source.android.com/docs/security/features/selinux) ·
[Kernel overview](https://source.android.com/docs/core/architecture/kernel) ·
[ACK](https://source.android.com/docs/core/architecture/kernel/android-common) ·
[AOSP build requirements](https://source.android.com/docs/setup/build/requirements) ·
[Emulator](https://developer.android.com/studio/run/emulator) ·
[Package index](https://developer.android.com/reference/packages) ·
[Build configuration](https://developer.android.com/studio/build)

AOSP source (branch `main` unless noted):
[ResourceTypes.h](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/androidfw/include/androidfw/ResourceTypes.h) ·
[ActivityThread.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/app/ActivityThread.java) ·
[Activity.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/app/Activity.java) ·
[ContextImpl.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/app/ContextImpl.java) ·
[Looper.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/os/Looper.java) ·
[View.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/View.java) ·
[ViewRootImpl.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/view/ViewRootImpl.java) ·
[PhoneWindow.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/com/android/internal/policy/PhoneWindow.java) ·
[Resources.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/content/res/Resources.java) ·
[AssetManager.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/core/java/android/content/res/AssetManager.java) ·
[SystemServer.java](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/java/com/android/server/SystemServer.java) ·
[hwui/renderthread](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/libs/hwui/renderthread/) ·
[preloaded-classes](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/config/preloaded-classes) ·
[Looper.cpp (epoll)](https://android.googlesource.com/platform/system/core/+/refs/heads/main/libutils/Looper.cpp) ·
[bionic README](https://android.googlesource.com/platform/bionic/+/refs/heads/main/README.md) ·
[libcore](https://android.googlesource.com/platform/libcore/) ·
[gki_defconfig (android15-6.6)](https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/arch/arm64/configs/gki_defconfig) ·
[drivers/android (binder)](https://android.googlesource.com/kernel/common/+/refs/heads/android15-6.6/drivers/android/)

Other primary docs: [FreeBSD handbook ch.12 (Linuxulator)](https://docs.freebsd.org/en/books/handbook/linuxemu/) ·
[WSL comparison](https://learn.microsoft.com/en-us/windows/wsl/compare-versions) ·
[What is WSL](https://learn.microsoft.com/en-us/windows/wsl/about) ·
[WSA overview (archived 2025-05-25; deprecated 2025-03-05)](https://web.archive.org/web/20250525105910/https://learn.microsoft.com/en-us/windows/android/wsa/) ·
[GitHub-hosted runners reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) ·
[GitHub-hosted runners overview](https://docs.github.com/en/actions/using-github-hosted-runners/using-github-hosted-runners/about-github-hosted-runners)

Projects (READMEs/source inspected; claims marked self-reported where applicable):
[Sandvik](https://gitlab.com/christophe.duvernois/sandvik) / [mirror](https://github.com/yag00/sandvik) ·
[dalvik-universal](https://github.com/A2OH/dalvik-universal) ·
[Westlake](https://github.com/A2OH/westlake) ·
[LiquidAPK](https://github.com/heyobi/apk-on-iphone) ·
[DaliVM](https://github.com/fatalSec/DaliVM) ·
[Mihon-Runner](https://github.com/celymyst/Mihon-Runner) ·
[aar](https://github.com/maekoos/aar) ·
[notmyst33d/dalvik](https://github.com/notmyst33d/dalvik) ·
[simple-dvm](https://github.com/cycheng/simple-dvm) ·
[dalvik-js](https://github.com/jjfiv/dalvik-js) ·
[dexterpreter](https://github.com/vimalloc/dexterpreter) ·
[AstoriaUWP](https://github.com/cbialorucki/AstoriaUWP) ·
[Anbox](https://github.com/anbox/anbox) (+ [install docs](https://github.com/anbox/anbox/blob/master/docs/install.md)) ·
[Waydroid](https://github.com/waydroid/waydroid) (+ [docs](https://docs.waydro.id/)) ·
[Multi-OS Engine](https://github.com/multi-os-engine/multi-os-engine) / [moe-core](https://github.com/multi-os-engine/moe-core) ·
[RoboVM](https://github.com/MobiVM/robovm) · [Avian](https://github.com/ReadyTalk/avian) ·
[webos_dalvik](https://github.com/kai4785/webos_dalvik) ·
[smali/baksmali](https://github.com/google/smali) ·
[dexrs](https://github.com/MatrixEditor/dexrs) ·
[SUPERAndroidAnalyzer/dalvik](https://github.com/SUPERAndroidAnalyzer/dalvik) ·
[androguard/dex-parser](https://github.com/androguard/dex-parser) ·
[smoltcp](https://github.com/smoltcp-rs/smoltcp)

### Caveats

- AOSP line numbers/counts are snapshots of branch `main` on 2026-11-15.
- LiquidAPK, Westlake, and dalvik-universal are recent, low-visibility projects; their
  app-compatibility claims are **self-reported** (public code/screenshots, no independent
  verification). Their *architectural* descriptions are consistent with the primary
  sources above and are used here as scale data points, not guarantees.
- Effort estimates are engineering judgments, not measurements; they assume part-time
  solo work and frozen scope where marked "bounded".
- KimiOS capability statements come from the repository README (phases 1–29) as of this
  date.
