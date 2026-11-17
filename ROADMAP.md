# KimiOS roadmap

**Goal: Desktop Lite** — run standard static Linux binaries and native apps on a
phone (the WSL1 syscall-translation model), efficiently. The **library-OS** is
the native, no-QEMU realization of that goal; the bare-metal kernel + QEMU was
the first implementation and remains the foundation. One pure-Rust userland,
three ways to run it. Detailed per-phase write-ups (Ph1–Ph52) live in
[docs/phases.md](docs/phases.md).

- **Library-OS (the product):** the userland compiled as a native ARM64 library,
  hosted directly by iOS / Android. No QEMU, native speed. Desktop Lite's binary
  hosting comes natively via the JIT ELF loader + host Linux-ABI translation.
- **Bare-metal kernel:** the full EL1 OS the userland was built on; runs on QEMU
  `virt` (where the Linux ABI was first proven). Real silicon (Redmi Note 11,
  `spes`) blocked on bootloader unlock.
- **QEMU frontend:** [kimios-android](https://github.com/Kimi-syl/kimios-android)
  emulates the whole kernel on a phone (the pre-library-OS Desktop Lite path).

## Library-OS status

The pivot: the userland is `no_std`, no allocator, and reaches the OS through
three `cfg`-gated primitives in libkimi (`svc`/`cntpct`/`wfe`). On a host they
route to a `kimi_host_syscall` dispatcher; everything above is unchanged.

| Capability | iOS | Android |
|---|---|---|
| Rust userland → native staticlib (`aarch64-apple-ios` / `-linux-android`) | done | done |
| Host syscall dispatcher (Swift `@_cdecl` / C JNI) | done | done |
| Display (Metal double-fb, on-demand / `ANativeWindow` dirty-rect) | done | done |
| Touch input (UIKit / JNI → virtio-style events) | done | done |
| Sandboxed `/disk/` filesystem + RamFS (full CRUD/dirs/stat/df) | done | done |
| Networking (DNS + TCP; Browser/dns/fetch/kpkg) | done | done |
| Time / rtc / ps / mem | done | done |
| Thin platform shell (UIKit Activity / SurfaceView Activity) | done | done |
| CI build → artifact (`.ipa` / signed APK) | done (`macos-latest`) | done (`ubuntu`) |
| Signing | unsigned (sideload re-sign) | phone debug key (`b1339cd3…`) |
| Native contract verification (shell, DNS, HTTP fetch; GUI render+touch) | done | done (shared harness) |
| On-device runtime verification | pending hardware | pending hardware |
| Desktop Lite binary hosting (JIT ELF loader + host Linux-ABI translation) | not yet | not yet |

Verified natively without a device: a `no_std` shell-test staticlib + C
dispatcher runs the real libkimi shell (file CRUD, nested dirs, RamFS, df,
mv/rm/rmdir, ps, real DNS, HTTP 200 fetch); a GUI harness renders lock/home and
responds to a scripted unlock tap. CI compiles the identical staticlib for each
device target and links the platform shell.

## Bare-metal kernel status

The foundation. QEMU `virt` is the executable target (aarch64 proot, TCG, no
KVM); `spes` needs vendor firmware/Linux drivers for most peripherals.

| # | Milestone | QEMU | spes |
|---|---|---|---|
| 1 | Boot chain accepts a dev image | done (raw ELF `-kernel`) | blocked: locked bootloader |
| 2 | ARM64 entry executes (EL2→EL1 drop for UTM) | done | - |
| 3 | Early debug output | done (PL011 UART) | needs UART/fb path |
| 4 | Discover/map RAM | done (DTB + PFA) | needs khaje DTB |
| 5 | Interrupt controller + timer | done (GICv3, CNTP) | needs khaje addresses |
| 6 | Secondary CPUs | done (PSCI/HVC) | - |
| 7 | Display | done (ramfb) | needs panel/DSI |
| 8 | Input | keyboard + tablet (virtio-input) | needs touch controller |
| 9 | Persistent storage | done (virtio-blk + KimiFS) | needs UFS + partitions |
| 10–17 | USB / battery / GPU / Wi-Fi / audio / sensors / camera / cellular | n/a in QEMU | need vendor firmware/blobs |

`spes` milestones 10–17 realistically require reusing vendor binaries; documented
up front rather than promised. See [docs/hardware.md](docs/hardware.md).

### Kernel foundations (Ph1–40, all done on QEMU)

Per-process TTBR0 address spaces; PIDs/states/reaping; `spawn`/`exec`/`wait`
(no `fork`); fd tables + safe teardown; VFS (RamFS + KimiFS on virtio-blk,
legacy + modern mmio); blocking console read (PL011 RX IRQ); ramfb display +
fw_cfg DMA; framebuffer text console; `fb_info`/`fb_fill`/`fb_map`/`fb_back`/
`fb_present`; PSCI SMP bring-up; KimiFS free-space reuse + directories; sleep/
uptime; PE32+ loader (9 `kernel32` imports); `brk` heaps + user `GlobalAlloc`;
`ps`/`kill`; rename; PL031 rtc; stat; virtio-keyboard + tablet input ring;
double-buffered 1080p Nothing/Material-3 phone GUI with 12 interactive apps;
virtio-net + smoltcp (DHCP/DNS/TCP) + mini browser; APK inspector; mini-Dalvik
DEX interpreter; shared CLI/Terminal shell; touch→display latency instrumentation
+ dirty-rect optimisation. Full detail: [docs/phases.md](docs/phases.md).

## History: the pivots

1. **Ph1–44 — bare-metal kernel + phone GUI.** A working ARM64 OS on QEMU.
2. **Android APK compatibility — abandoned.** APK inspector (Ph45) + mini-Dalvik
   (Ph46) shipped and verified, but Play-Store apps need the full Android runtime
   (Bionic/ART/framework/Binder) — team-scale.
   [docs/android-feasibility.md](docs/android-feasibility.md).
3. **Ph47+ — Desktop Lite adopted as the goal (WSL1 model).** A Linux
   syscall-translation layer (`src/linux.rs`) ran static Linux binaries
   (musl/glibc, BusyBox 402 applets) and native Rust apps on the kernel; kpkg
   package manager; ~0.4 s boot. This proved the model — the first implementation,
   inside QEMU, with its emulation and display-latency costs.
4. **Library-OS — Desktop Lite done efficiently (current).** The same userland as
   a native library on iOS/Android: no QEMU, native speed, host FS/net. The
   three-primitive `cfg` gate made the port a dispatcher implementation, not an
   OS rewrite. The JIT ELF loader + host-side Linux syscall translation bring the
   Desktop Lite binary-hosting capability natively to the device — the goal,
   achieved the efficient way.

## Next

Priority is the library-OS product; kernel items continue as the foundation.

1. **On-device verification** of the iOS/Android ports (render + touch + FS/net
   on real hardware) once a test device is available; fix any platform-shell
   runtime issues.
2. **Desktop Lite natively — JIT ELF loader + host Linux-ABI translation** (the
   core payoff): map real aarch64 Linux ELF/PE images, rewrite `svc` to call the
   host dispatcher (no EL0 vectors on host), and translate Linux syscall numbers
   onto host services (sandbox FS, BSD sockets, host memory) — the same WSL1 model
   as `src/linux.rs`, but native, no QEMU. StikDebug enables JIT on iPad; Android
   needs a similar path. Then `run /disk/<binary>` / `kpkg run busybox …` work on
   the device at native speed.
3. **Library-OS polish:** richer FS (file picker / import), TLS networking
   (HTTPS browser/kpkg), background audio, performance/battery tuning of the
   render+input loop.
4. **Publish releases** (iOS `.ipa`, Android APK) out of draft once verified.
5. Kernel: VirtIO-GPU `attach_backing` debug → staged accelerated backends; SMP
   scheduling (per-CPU runqueues, GIC redistributor, timer/IPIs).
6. Kernel: Linux ABI growth (fork/execve, threads/futex/epoll, sockets) for the
   QEMU/Desktop-Lite path.
7. Bare metal: UFS + spes partition table (blocked on bootloader unlock).
