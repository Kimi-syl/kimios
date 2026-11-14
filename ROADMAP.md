# KimiOS milestones

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
   `etc/ramfb` and the kernel draws a 640x480 XRGB8888 test pattern into an
   identity-mapped buffer. Pixel output verified with monitor `screendump`.
   Real hardware still needs a panel/DSI driver.
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

EL0 processes are preempted by the timer (50 ms quantum, round-robin among
`Ready` processes), and a process blocked in `read` yields the CPU. Syscalls
still execute with interrupts masked, which is acceptable until blocking I/O
beyond the console appears.

## Next

1. Block storage beyond QEMU: UFS and the spes partition table (blocked on
   bootloader unlock for bare-metal testing).
2. Filesystem surface: directories, rename, and a real free-inode list.
3. SMP scheduling: per-CPU runqueues, GIC redistributor init and timer/IPIs on
   secondary cores (needs khaje firmware on spes for hardware).
4. More drawing: user-space font/glyph blitting or a shared framebuffer
   mapping, plus a proper compositor/invalid-rect refresh model.
