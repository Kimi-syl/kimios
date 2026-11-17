# KimiOS milestones

Terse checklist; detailed per-phase write-ups (Ph1-Ph52) live in
[docs/phases.md](docs/phases.md).

Two tracks share one kernel:

- **QEMU `virt`**: the executable development/test target on this phone
  (aarch64 proot, TCG, no KVM).
- **Redmi Note 11 (spes)**: bare-metal bring-up, blocked until the bootloader
  is unlocked and the stock boot chain is dumped and verified. See
  `docs/hardware.md`.

## Status

| # | Milestone | QEMU | spes |
|---|---|---|---|
| 1 | Boot chain accepts a KimiOS development image | done (raw ELF `-kernel`) | blocked: locked bootloader |
| 2 | ARM64 entry point executes | done | - |
| 3 | Early debug output works somehow | done (PL011 UART) | needs framebuffer/UART path |
| 4 | Correctly discover/map RAM | done (DTB + PFA) | needs khaje DTB parse |
| 5 | Interrupt controller + timer | done (GICv3, CNTP) | needs khaje addresses |
| 6 | Bring up secondary CPUs | done (PSCI over HVC) | - |
| 7 | Basic display output | done on QEMU (ramfb) | needs panel/DSI |
| 8 | Input | keyboard done (virtio-input); touch planned | needs touch controller |
| 9 | Persistent storage | done (virtio-blk + KimiFS) | needs UFS + partition access |
| 10 | USB | planned (virtio/keyboard or DWC3) | needs DWC3 + PMIC |
| 11 | Battery/charging | not meaningful in QEMU | needs PMIC (PMI632?) |
| 12 | GPU | planned (ramfb only; no 3D) | Adreno 610 + firmware |
| 13 | Wi-Fi/Bluetooth | not meaningful in QEMU | vendor firmware/blobs |
| 14 | Audio | planned (virtio-sound, optional) | needs codec |
| 15 | Sensors | not meaningful in QEMU | needs sensor hub |
| 16 | Camera | not meaningful in QEMU | needs camera stack |
| 17 | Cellular | not meaningful in QEMU | needs modem + firmware |

`spes` milestones 10-17 require vendor firmware and Linux drivers; a
from-scratch OS realistically cannot implement them without reusing vendor
binaries. This is documented up front rather than promised.

## Current work: process and storage foundations

Target-independent, implemented before display/device work:

1. ~~Per-process address spaces with independent TTBR0 translation tables.~~
   Done on QEMU (private L3 tables, PFA-backed).
2. ~~PIDs, parent/child links, process states, exit status and reaping.~~
   Done on QEMU; PID 1 adopts orphans.
3. ~~`spawn` / `exec` / `wait`~~ (no `fork`; spawn is the primitive). Done on
   QEMU, plus `getpid` and child-status reporting.
4. ~~Per-process file descriptor table and resource ownership; safe teardown
   of address space frames on exec/exit.~~ Implemented; free-frame count is
   stable across repeated spawn/exit cycles.
5. ~~VFS handles backed by RamFS now, then a disk filesystem on top of a
   block-device trait. `virtio-blk` is the first QEMU driver.~~ Done: legacy
   and modern virtio-mmio transports, KimiFS, and `/disk/` through the same
   descriptor table; persistence verified across QEMU reboots.
6. ~~Blocking console `read`.~~ Done: the PL011 RX interrupt (SPI 33) queues
   completed lines; `read` parks in a `Blocked` state with a rewound `SVC` and
   the console IRQ wakes it, so background processes run at the prompt.
7. ~~Basic display output.~~ Done on QEMU: a fw_cfg DMA driver writes
   `etc/ramfb` and the kernel draws an XRGB8888 test pattern into an
   identity-mapped buffer (640x480 at first bring-up; 1080x1920 default
   today, boot-configurable via `kimios.fb=`). Pixel output verified with
   monitor `screendump`. Real hardware still needs a panel/DSI driver.
8. ~~Framebuffer text console.~~ Done: an embedded 8x8 font mirrors kernel and
   user console output at 80x60 with wrapping, scrolling and input echo;
   verified pixel-for-pixel and with a 60-line scroll test.
9. ~~Graphics syscalls.~~ Done: `fb_info` (16) and `fb_fill` (17) let EL0 query
   the display and draw clipped `0x00RRGGBB` rectangles; `kimi-fbtest`
   verified exact pixel colours.
10. ~~Secondary CPU bring-up.~~ Done on QEMU with `-smp 4`: PSCI 1.1 over HVC,
    per-core stacks, MMU/caches enabled on each core, atomics-based checksum
    test, then parked in `wfe`. SMP scheduling is still future work.
11. ~~KimiFS free-space reuse.~~ Done: a bitmap rebuilt at mount drives
    first-fit allocation; `unlink` and truncation recycle runs; `rm`,
    `freeblocks` (19) and a richer `df` exercise it across reboots.
12. ~~Sleep and uptime.~~ Done: `uptime` (20) from `CNTPCT_EL0`, `sleep` (21)
    with timer-IRQ wakeups, a rewind/`sleep_done` retry path and a kernel
    `wfi` idle when nothing else is runnable; shell `time`/`sleep`.
13. ~~WinAPI surface.~~ Done: 9 translated imports including `Sleep`,
    `GetTickCount`/`64` and `WriteConsoleA`; the generated PE calls all of
    them and survives sleeping.
14. ~~User heap.~~ Done: `brk` (22) with page-rounded eager mapping, a
    `spare_frames` guard against page-table exhaustion and a `/heaptest`
    that verifies 16 KiB of fresh zeroed pages.
15. ~~Process inspection and termination.~~ Done: `ps` (23) returns
    PID/parent/state/exit records; `kill` (24) zombies a target with status
    137, reparents its children and wakes a blocked parent; shell `ps`/`kill`
    and the `/sleeper` test program.
16. ~~Rename.~~ Done: `rename` (25) updates the inode name and flushes,
    rejecting collisions; shell `mv`.
17. ~~Wall clock.~~ Done: `rtc` (26) reads the PL031 at `0x0901_0000`, with
    UTC civil-time conversion in the kernel; shell `date`.
18. ~~File metadata.~~ Done: `stat` (27) returns kind/size/blocks, `ls`
    shows aligned sizes, shell `stat`.
19. ~~User-space allocator.~~ Done: `libkimi::Heap` (`GlobalAlloc` over
    `brk`, free list + bump), `alloc` collections usable in user programs;
    `/alloctest` verifies reuse and large vectors.
20. ~~Keyboard input.~~ Done: virtio-input (id 18) driver with legacy and
    modern transports, shift-aware keycode translation into the shared UART
    line editor; verified with QMP key events.
21. ~~Directories.~~ Done: KimiFS parent/kind fields, component-wise path
    resolution, `mkdir`/`rmdir`/`listdir`, nested `mv`/`rm`/`stat`; inode 0
    reserved and legacy disks migrated at mount.
22. ~~User-space framebuffer.~~ Done: `fb_map` (31) maps the shared pixel
    buffer read/write and suppresses console mirroring; `/gfxtest` draws at
    compositor speed and is pixel-verified via screendump.
23. ~~Pointer input and events.~~ Done: two virtio-input devices (keyboard +
    tablet), type detection via EV_BITS, lock-free event ring, blocking
    `input_wait` (33) with its own wakeup path; `/inputtest` verified.
24. ~~GUI demo.~~ Done: `/gui` draws windows and a cursor with a clickable
    button over the mapped framebuffer; fixes `wait` on blocked children and
    eager reaping before `switch_to`.
25. ~~Stability hardening.~~ Done: coalescing kernel heap with ELF buffers
    freed, lower-EL fault statuses (139/135) that kill only the faulting
    process, and page-walked syscall buffer validation; `/stress` returns all
    frames across repeated runs.
26. ~~Phone UI.~~ Done: 480x800 portrait display, lock screen, status bar,
    home grid/dock, Calculator, live Clock and Settings apps with working
    toggles and slide transitions; user-space 8x8 text renderer.
27. ~~Material 3 / Nothing redesign.~~ Done: monochrome/red palette, dot-grid
    wallpaper, dot-matrix clocks, tonal cards, alpha-blended dock and shade,
    Quick Settings with brightness and toggles, 12-app launcher.
28. ~~1080p rendering.~~ Done: 1080x1920 ramfb, logical 480x800 layout mapped
    to native pixels, anti-aliased text, uniform circles, dirty-background
    repaints and a 3x3 kernel console font.
29. ~~Real apps and boot-to-GUI.~~ Done: Phone, Messages, Camera, Music,
    Notes, Maps, Photos, Files, Weather and Terminal are interactive; the GUI
    starts at boot and ESC returns to the shell. UI animations are now an
    experimental, default-off Settings toggle.
30. ~~Real networking.~~ Done: virtio-net driver + smoltcp (Ethernet/ARP/IPv4/
    ICMP/UDP/TCP), DHCP, DNS, and a TCP socket syscall API. Verified with a
    live DNS lookup and HTTP fetch over QEMU user-mode NAT.
31. ~~Mini web browser.~~ Done: a GUI browser (URL bar, DNS+TCP+HTTP/1.0,
    HTML-to-text renderer with title/word-wrap/scroll). Renders live pages.
32. ~~APK inspector.~~ Done: ZIP + DEFLATE + binary-AXML manifest + DEX header
    parser in libkimi, exposed as a shell command and a GUI app. Inspects
    package/version/permissions/activities/DEX. Execution of real APKs is out
    of scope (needs the Android runtime) — see docs/android-feasibility.md.
33. ~~Mini-Dalvik (DEX execution).~~ Done: a bounded register-based DEX
    interpreter (`libkimi/src/dvm.rs`) with a hand-written micro core library
    (Object/String/StringBuilder/System.out/PrintStream). Executes
    purpose-built DEX (javac + d8) — arithmetic, branches/loops, arrays,
    strings, instance fields and virtual/static user methods. Exposed as the
    shell `dvm <file.dex>` command and a GUI "DEX" runner (home page 2).
    Verified host-side (8/8 conformance) and on the guest (shell + GUI). This
    is Stage 2 / layer L2 of docs/android-feasibility.md: it runs *your own*
    bytecode, NOT Play-Store APKs (those still need the full Android runtime).

> **The Pivot (Phase 47+).** Android APK compatibility is officially abandoned
> — the Bionic/ART/framework wall is team-scale. KimiOS is now a "Desktop Lite"
> environment (the WSL1 model): a Linux syscall translation layer runs standard
> statically-linked Linux binaries (musl/glibc) and native Rust apps, with a
> sub-5-second boot.

34. ~~Linux ABI (syscall translation layer).~~ Done: the SVC handler detects a
    static Linux ELF (linked below the KimiOS user base) and routes its Linux
    syscall numbers (x8) to a translation layer (`src/linux.rs`): write→UART,
    exit→kill, brk/mmap→frame allocator, openat/read/close→KimiFS, plus
    getpid/uname/clock_gettime/getrandom/fstat and signal no-ops. The MMU maps
    low link addresses (0x40_0000) by splitting device blocks into tables, and
    the loader builds a real Linux initial stack (argc/argv/envp/auxv).
    Verified on the guest: asm, musl C, musl Rust and glibc static hello all
    print and exit 0; malloc/brk (fib) and uname work. `run /disk/<binary>`.
35. ~~Kimi package manager (kpkg).~~ Done: apt-like shell command. `kpkg
    update [url]` fetches repo.json over HTTP (smoltcp) and caches it; `kpkg
    list` shows packages; `kpkg install <name>` downloads a static binary to
    /disk/bin; `kpkg run <name>` executes it via the Linux ABI. Verified
    end-to-end on the guest (update→install hello/fib→run). Honest scope:
    npm/Node, pip/CPython and TLS curl need a far larger ABI (threads/futex/
    epoll/dlopen/sockets/TLS) and are marked not-runnable in the repo.
36. ~~Fast boot.~~ Done: trimmed the legacy timer/scheduler boot demos; kernel
    reaches init in ~0.4 s (was ~7-8 s), well under the 5 s Desktop Lite goal.
37. ~~BusyBox + expanded Linux ABI.~~ Done: real argv passing (spawn_args
    syscall + a proper argc/argv/envp/auxv stack), cwd + directory descriptors,
    and many more syscalls (getdents64, newfstatat/fstat, lseek, fcntl, ioctl/
    TIOCGWINSZ, getcwd/chdir, mkdirat/unlinkat/renameat, dup/dup3, sendfile,
    getuid/gettimeofday/sysinfo/sched_getaffinity, reserved stdin/out/err fds).
    A statically-built BusyBox (402 applets) runs on the guest: `ls -l`, `cat`,
    `echo`, `pwd`, `uname -a`, `wc`, `mkdir`, `cp`, `date` all verified. Run as
    `run /disk/busybox <applet> [args]` or via `kpkg run busybox <applet>`.
    Honest limit: interactive `busybox sh` needs fork/execve (not implemented);
    individual applets work. Kernel heap grown to 8 MiB for large binaries.
38. ~~GUI flicker fix (double buffering).~~ Done: the GUI now renders to an
    offscreen back buffer and publishes each finished frame with a single
    `fb_present` copy (new SYS_FB_BACK/SYS_FB_PRESENT), so the display never
    shows a half-drawn frame (the black flicker on scroll). The Android RFB
    client forces opaque alpha (VNC's 32bpp padding byte was 0 → transparent
    pixels) and handles DesktopSize/Cursor pseudo-encodings without dropping
    the connection (which had caused constant reconnect/reload).
39. Hardware-independent graphics interface. Done: the GUI is decoupled from
    QEMU graphics — it renders to a back buffer and publishes via
    fb_info/fb_back/fb_present; `src/fb.rs` dispatches to a backend selected by
    `kimios.gpu=` (auto/ramfb/virtio). ramfb is the stable boot/recovery
    fallback. A VirtIO-GPU 2D driver (`src/virtio_gpu.rs`: create_2d,
    attach_backing, set_scanout, transfer_to_host_2d, resource_flush) is
    implemented; on the tested QEMU builds attach_backing returns ERR_UNSPEC
    (shareable-image/udmabuf path), so it auto-falls back to ramfb — see
    docs/graphics-architecture.md for the honest status + next debugging step.
    Added `gfxbench` (SYS_GFX_BENCH): ramfb full-frame present ~15.6 ms /
    256px dirty rect ~0.44 ms under TCG (dirty rects ~35x cheaper). Accelerated
    backends (VirGL/Venus/rutabaga-gfxstream) are designed but staged until 2D
    is stable; the real latency lever remains the ashmem transport.

EL0 processes are preempted by the timer (50 ms quantum, round-robin among
`Ready` processes), and a process blocked in `read` yields the CPU. Syscalls
still execute with interrupts masked, which is acceptable until blocking I/O
beyond the console appears.

## Next

1. ashmem/shared-memory display transport in the Android frontend (removes
   the VNC latency floor; see kimios-android/docs/display-pipeline.md).
2. VirtIO-GPU `attach_backing` debugging (non-`none` display backend /
   upstream QEMU), then staged accelerated backends.
3. Linux ABI growth: fork/execve for interactive shells, then threads/
   futex/epoll and sockets as real binaries demand them.
4. SMP scheduling: per-CPU runqueues, GIC redistributor init and timer/IPIs on
   secondary cores (needs khaje firmware on spes for hardware).
5. Block storage beyond QEMU: UFS and the spes partition table (blocked on
   bootloader unlock for bare-metal testing).
