# KimiOS phases — detailed history and design notes

This document explains every development phase (Ph1-Ph52): what was built,
why, how it works (with the key files and symbols), what was verified, and
the honest limitations that remain. [ROADMAP.md](../ROADMAP.md) is the terse
checklist; this is the prose behind it.

Phase numbers follow the git history. Note that the ROADMAP's "current work"
checklist uses its own numbering (items 1-39), which diverges from phase
numbers after item ~30 (e.g. ROADMAP item 34 = Ph47, item 38 = Ph50,
item 39 = Ph52; Ph51 has no checklist entry).

**Verification toolkit used throughout:** QEMU monitor `screendump` + PPM
decode for exact pixel checks; QMP `input-send-event` for scripted
keyboard/pointer input; serial-log assertions; host-side conformance tests
(the same `no_std` code compiled for the host); and on-device runs through
the [Android frontend](https://github.com/Kimi-syl/kimios-android). All
performance numbers were measured under TCG (software emulation) on the
test phone — bare-metal numbers would be very different.

---

## Part I — Kernel foundations (Ph1-Ph16)

### Ph1 — Boot

**Built.** A kernel that QEMU loads and executes: `linker.ld` places the
image at `0x4008_0000` with `.text.boot` first; `src/boot.s` provides
`_start` (EL1 entry), enables FP/SIMD, sets up the boot stack, zeroes
`.bss`, parks secondary cores, and calls `kmain` (`src/main.rs`). A panic
handler and ARM semihosting exit complete the scaffolding.

**Why.** Everything else needs a reliable entry point. No bootloader is
used: QEMU's `-kernel` loads the raw ELF directly, which keeps the boot
chain trivial while the project is QEMU-only.

**How.** QEMU enters at EL1. `CPACR_EL1.FPEN` is 0 after reset, so
compiler-generated NEON would trap until `_start` sets bits 20-21. The
kernel runs on `SP_EL1`; the vector table's "current EL with SPx" entries
are the ones actually used.

**Verified.** QEMU boots the ELF and reaches `kmain`; semihosting
(`SYS_EXIT_EXTENDED`, `HLT #0xF000`) exits QEMU cleanly with status 0.

**Limits.** A raw ELF `-kernel` does not use the Linux boot protocol, so
some QEMU versions pass `x0 = 0` instead of a DTB pointer (handled in Ph5).
Bare-metal boot is blocked on the locked bootloader of the target phone
(see [hardware.md](hardware.md)).

### Ph2 — UART console

**Built.** PL011 driver (`src/uart.rs`) at `0x0900_0000`, 115200 8N1,
`core::fmt::Write` implementation, `print!`/`println!` macros, and a
`spin::Mutex` guarding the console.

**Why.** Early debug output is prerequisite to debugging everything else;
the UART is the simplest reliable output path on QEMU `virt`.

**How.** Polling TX; the mutex makes console access safe from multiple
contexts. Later phases add RX interrupts (Ph16) and framebuffer mirroring
(Ph18) on top of the same driver.

**Verified.** Kernel messages appear on `-serial stdio`.

**Limits.** Polling output; no hardware flow control. On real hardware an
early console needs a verified UART path or the bootloader framebuffer.

### Ph3 — Exceptions

**Built.** A 2 KiB-aligned AArch64 vector table (`src/vectors.s`, 16
entries of `0x80` bytes) installed in `VBAR_EL1`, full register
save/restore into a 272-byte `ExceptionFrame`, and a Rust handler
(`src/exceptions.rs`) that decodes `ESR_EL1`/`FAR_EL1`.

**Why.** Syscalls, faults and interrupts all arrive as exceptions; a
correct frame layout is the foundation for EL0 (Ph9) and preemption (Ph15).

**How.** Synchronous exceptions are classified by `ESR_EL1.EC`. `BRK`
resumes by advancing `ELR_EL1` manually; for `SVC`, `ELR_EL1` already
points at the next instruction. Same-EL vs lower-EL origins are
distinguished (`SPSR_EL1 & 0xf == 0` means EL0), which later drives fault
isolation (Ph35) and user preemption (Ph15).

**Verified.** `BRK` exceptions decode and resume; the frame survives
save/restore round-trips.

**Limits.** Only the paths needed so far are implemented; lower-EL abort
reporting was improved later (Ph35).

### Ph4 — Memory management

**Built.** 4 KiB-granule translation tables (L0 -> L1 -> L2 with 2 MiB
blocks) in `src/mmu.rs`, an identity map (virtual == physical) with Device
memory for peripherals and Normal memory for RAM, MMU + caches enabled via
`SCTLR_EL1`, and a 1 MiB kernel heap (`src/allocator.rs`, initially a bump
allocator).

**Why.** EL0 isolation, per-process address spaces and a kernel heap all
need controlled page tables; identity mapping keeps early boot simple.

**How.** `MAIR_EL1` Attr0 = Normal write-back, Attr1 = Device-nGnRnE. The
low 1 GiB (UART, GIC, fw_cfg, ...) is mapped Device and never executable;
RAM above `0x4000_0000` is mapped Normal. `MAIR`/`TCR`/`TTBR0`/`SCTLR` are
programmed explicitly. The heap grew later: coalescing free list (Ph35),
4 MiB (Ph41), 8 MiB (Ph49).

**Verified.** `SCTLR_EL1.M = 1` readback; heap alloc/readback test passes.

**Limits.** Originally only 1-2 GiB of RAM was mapped; extended to 4 GiB
in Ph40. No unmapping primitive exists yet (affects `brk` shrink, Ph24).

### Ph5 — Device tree and physical frames

**Built.** DTB discovery and parsing (`src/dtb.rs`) plus a bitmap physical
frame allocator (`src/pfa.rs`): 4 KiB frames with alloc/free/stats.

**Why.** The kernel must know how much RAM exists and where the DTB is
before it can hand memory to processes.

**How.** `x0` is captured at boot and validated; if it is not a DTB, RAM is
scanned for the `0xd00dfeed` magic (raw-ELF boots may pass 0). `/memory` is
parsed with `hermit-dtb`. The PFA reserves every frame below
`__kernel_end` — DTB, boot area, kernel image, heap, boot stack and the
bitmap itself. Passing `-dtb virt.dtb` guarantees a blob is present.

**Verified.** With `-m 128M`: 32768 frames discovered; alloc/free/realloc
reuses the same frame; free counts are exact.

**Limits.** `virt.dtb` is trimmed for a specific `-m`/`-smp`; regenerate it
when those change. Later phases also parse `/chosen/bootargs` here
(`kimios.fb=` in Ph40, `kimios.gpu=` in Ph52).

### Ph6 — Interrupts and timer

**Built.** GICv3 support (`src/gic.rs`: distributor + redistributor,
system-register CPU interface via `ICC_SRE_EL1`, `ICC_PMR_EL1`,
`ICC_IGRPEN1_EL1`), the EL1 physical timer on PPI 30 (`src/timer.rs`), and
IRQ acknowledge/EOI handling (`src/irq.rs`) with separate synchronous/IRQ
vector paths and IRQ-safe console output.

**Why.** Preemptive scheduling, blocking input and sleep all need a
reliable periodic interrupt.

**How.** QEMU `virt`'s generic timer runs at 62.5 MHz (`CNTFRQ_EL0`); the
boot demo used 500 ms ticks. `ICC_IAR1_EL1` acknowledges Group 1
interrupts, `ICC_EOIR1_EL1` ends them; the timer must be re-armed *before*
EOI because PPI 30 is level-sensitive. Requires
`-machine virt,gic-version=3`.

**Verified.** Five timer interrupts received and counted at boot.

**Limits.** Timer/GIC init runs on CPU 0 only; secondary-core interrupt
setup is still future work (SMP scheduling).

### Ph7-Ph8 — Kernel threads and scheduler

**Built.** Callee-saved context switch (`src/switch.s`: `x19-x30`, SP,
DAIF), thread structs and spawn (`src/sched.rs`), 64 KiB kernel thread
stacks from the kernel heap, a thread trampoline, `thread_exit`, and a
preemptive round-robin scheduler driven by the timer IRQ.

**Why.** Validate the switch mechanics in-kernel before adding EL0
processes; the same timer path later preempts user processes.

**How.** `context_switch` saves the outgoing callee-saved state and loads
the incoming one; new threads enter through `thread_entry`. The boot demo
ran threads A/B plus a one-shot C at a 100 ms quantum.

**Verified.** Interleaved `ABAB...` output, one-shot C runs and returns to
the scheduler, tick stats match (A=10 B=10 over 35 ticks).

**Limits.** This kernel-thread scheduler was later superseded by the
process scheduler for EL0 work; the boot demos were trimmed in Ph48 for
fast boot.

### Ph9 — EL0 user mode and syscalls

**Built.** Entry into EL0 via `eret`, L3 4 KiB user pages (AP = EL0 RW,
UXN controlled by the ELF `PF_X` flag, PXN = 1), SVC decoding from
`ESR_EL1.EC`, and the first syscalls: `write` (1) and `exit` (2)
(`src/usermode.rs`).

**Why.** The whole point of the OS is running untrusted user code safely;
EL0 + page permissions + a syscall gate is the isolation boundary.

**How.** The user ELF region at `0x6000_0000` gets EL0-accessible pages;
code pages are EL0-executable, the stack is UXN=1. Kernel RAM and device
MMIO stay EL1-only. The SVC ABI: `x8` = number, `x0..x4` = args, result in
`x0`.

**Verified.** A user program prints through the kernel console and exits;
EL1 state is untouched.

**Limits.** Two syscalls only at this stage; the ABI grew to 38 native
calls plus the Linux translation layer (Ph47+).

### Ph10 — VFS, RamFS and ELF loading

**Built.** A `Vfs` trait (`open`/`read`/`write`/`close`) with a read-only
RamFS embedding `kimi-init` via `include_bytes!` (`src/ramfs.rs`), and an
ELF loader (`src/elf.rs`) using `xmas-elf`: allocate PFA frames, copy
`PT_LOAD` segments, zero BSS, map with user permissions. `build.rs` builds
the user program with a separate target directory.

**Why.** User programs must come from somewhere; embedding them in the
kernel image keeps the boot chain dependency-free until disk storage
exists (Ph14).

**How.** `xmas-elf` requires natural alignment, but `include_bytes!` only
guarantees byte alignment, so the loader parses an aligned copy (freed
afterwards — important once the heap became a free list, Ph35).
`user/linker.ld` page-aligns sections so no two `PT_LOAD` segments share a
page.

**Verified.** `/init` loads (magic `[7f, 45, 4c, 46]`), maps 18 pages, and
runs at EL0.

**Limits.** RamFS is read-only; executables still load from RamFS even
after KimiFS exists (disk files are data, plus Linux binaries via Ph47).

### Ph11 — User shell (libkimi + kimi-init)

**Built.** The `libkimi` user crate with syscall wrappers (`write`, `exit`,
`read`, `open`, `read_file`, `close`, `exec`, `list`), the `kimi-init`
shell (`ls`, `cat`, `exec`, `help`, `exit`), and a second ELF `kimi-hello`
that `exec` replaces the shell with. `build.rs` now builds the whole
`user/` workspace and embeds both binaries.

**Why.** An interactive shell is the test harness for every later syscall;
`exec` validates address-space reuse.

**How.** `exec` reuses the ELF loader, remaps and zeroes the user stack,
then rewrites the exception-return frame (`ELR_EL1`, `SP_EL0`) so the
syscall returns directly into the new program. The kernel copies the path
out of user memory first, because the path buffer usually lives on the
stack being replaced.

**Verified.** `ls`, `cat /motd`, `exec /hello` all work end-to-end at EL0.

**Limits.** Single address space at this point; per-process spaces arrive
in Ph13.

### Ph12 — PE32+ loader and WinAPI translation

**Built.** A PE32+ (AArch64) loader (`src/pe.rs`) that maps sections,
parses the import table, and patches the IAT with generated EL0 thunks.
Five `kernel32.dll` imports (`GetStdHandle`, `WriteFile`, `ExitProcess`,
`GetLastError`, `SetLastError`) translate to KimiOS syscalls 100-104, so a
generated `hello.exe` runs at EL0 and prints through the kernel console.

**Why.** An early experiment in binary-format translation — the same
"shape" of problem later solved for Linux (Ph47), and a useful precedent
when evaluating Android compatibility (9 thunks for hello-world vs
thousands of framework APIs for an APK).

**How.** Each import points at a 16-byte thunk (`mov x8, #id; svc #0; ret;
nop`) at `0x6300_0000`, indexed by `id - 100`. The PE image is fixed at
`0x6200_0000`, its stack at `0x6400_0000`. `build.rs` generates the test
image entirely by hand (no Windows ARM64 toolchain in the project): real
PE32+ header, section table, import descriptor, ILT/IAT, hint/name table,
and a hand-assembled `_start`. PE calls use the Windows ARM64 convention
(`x0-x7` args, result `x0`).

**Verified.** `exec /hello.exe` detects the `MZ` signature, resolves 3
imports, prints "Hello from an ARM64 PE on KimiOS!" and exits via
`ExitProcess(0)`.

**Limits.** Fixed load addresses; a hand-built test image, not arbitrary
PE binaries. Extended to 9 imports in Ph23; superseded in importance by
the Linux ABI (Ph47).

### Ph13 — Processes

**Built.** Per-process address spaces (private TTBR0 L3 tables),
PID/parent/state tracking, exit statuses and reaping, `spawn`/`exec`/
`wait`/`getpid` (no `fork`), per-process file descriptor tables with
ownership, and eager teardown of address-space frames
(`src/process.rs`). `init` is PID 1 and adopts orphans. Shell gains
`spawn`, `run`, `wait`, `mem`.

**Why.** Real multitasking needs isolation between programs and a
life-cycle model; spawn-based (not fork-based) keeps the kernel simple —
no copy-on-write, no duplicated page tables.

**How.** Each process gets a private copy of the kernel's L2 table whose
L1 user entries point at fresh L3 tables. `spawn`/`wait`/`exit` save the
current `ExceptionFrame`, set `TTBR0_EL1` to the next process's table,
`TLBI VMALLE1`, and return the next frame to the exception-return path.
Frames are released on exit and after `exec`. When PID 1 exits and nothing
is runnable, the kernel shuts QEMU down via semihosting.

**Verified.** The free-frame count is identical before and after repeated
spawn/exit cycles — address spaces, L3 tables and user frames all return
to the PFA. `wait 0` reaps any child; exit statuses propagate.

**Limits.** No `fork` (deliberate). This limitation resurfaces in Ph49:
interactive `busybox sh` cannot work without fork/execve.

### Ph14 — Persistent storage (virtio-blk + KimiFS)

**Built.** A block-device trait (`src/block.rs`: read/write one 512-byte
sector, flush), a virtio-mmio block driver (`src/virtio_blk.rs`, legacy v1
and modern v2 transports, polling request queue), and **KimiFS**
(`src/diskfs.rs`): superblock, fixed inode table, contiguous file data.
Files under `/disk/` survive reboots; system binaries stay in RamFS.

**Why.** Persistence is needed for notes, camera captures, downloaded
packages — everything the phone UI and kpkg later store.

**How.** The driver scans the 32 virtio-mmio slots at `0x0a00_0000`,
initializes queue 0 (256 descriptors legacy, 8 modern) and submits
synchronous three-descriptor chains (header, sector buffer, status):
publish, `DSB`, notify, poll `used.idx` — no virtio interrupts. Legacy
publishes queues by page frame number and needs contiguous rings; a
256-entry queue makes both rings land on separate 4 KiB pages, which also
works for modern. KimiFS: 64-byte superblock at LBA 0, 64 fixed 128-byte
inodes at LBA 1-16, data from LBA 17. Each file is one contiguous run;
growing allocates a new run and copies. `/disk/<name>` is served through
the same VFS descriptor table as RamFS.

**Verified.** `fstest` writes 800 bytes across 2 sectors; `write
/disk/hello` + reboot + `cat /disk/hello` returns the saved text; mounting
an existing image preserves the namespace.

**Limits.** Contiguous-run files fragment; polling I/O blocks the CPU.
Space reuse arrives in Ph21, directories in Ph31, 64 GiB capacity in Ph40.

### Ph15 — Preemptive EL0 scheduling

**Built.** Timer-driven round-robin preemption of user processes: 50 ms
quantum (`CNTFRQ_EL0 / 20`), saving/restoring the EL0 `ExceptionFrame` and
switching `TTBR0_EL1` from the IRQ path (`process::preempt`).

**Why.** Cooperative switching alone would let one program hog the CPU;
the GUI must stay responsive while background processes run.

**How.** The timer is restarted before dropping to `/init` and init's
`SPSR_EL1` has IRQs unmasked, so ticks arrive from EL0. The lower-EL IRQ
vector (offset `0x480`) saves the same frame layout; `irq_handler` re-arms
the timer, EOIs the GIC, and scans forward from the current slot for a
`Ready` process, installing its frame and TTBR0. A tick with no other
runnable process keeps the current one.

**Verified.** Two `spawn /spin` children produce interleaved `2222333...`
output; each exits cleanly and is reaped.

**Limits.** Syscalls still execute with IRQs masked (exception entry), so
syscalls themselves are not preempted — acceptable until blocking I/O
beyond the console appears. Uniprocessor scheduling only (see Ph20).

### Ph16 — Blocking console input

**Built.** PL011 receive interrupt (SPI 33) draining the RX FIFO with
echo, backspace editing and a four-line completed-line queue; `read`
parks the process in `Blocked` state with a rewound SVC; the console IRQ
wakes the reader.

**Why.** The shell must wait for input without burning the CPU, while
background processes keep running.

**How.** When `read` finds no completed line it marks the process
`Blocked`, rewinds `ELR_EL1` by one instruction and switches away; EL0
re-executes the SVC when resumed, which now finds the queued line. If
nothing else is runnable the kernel idles with `wfi`. This
rewind-and-retry pattern is reused by `sleep` (Ph22) and `input_wait`
(Ph33). Clean QEMU shutdown via semihosting was also finalized here.

**Verified.** With a `/spin` child running, typed `help`/`mem` lines are
echoed and executed while the child's `2`s interleave — input arrives
while the shell is parked, and the child keeps the CPU.

**Limits.** Four-line input queue (fast pastes beyond that can drop);
line editing is basic. The serial log remains the authoritative console
record on the Android frontend.

---

## Part II — Display, SMP and system services (Ph17-Ph35)

### Ph17 — Framebuffer via ramfb

**Built.** A minimal fw_cfg driver (`src/fwcfg.rs`: MMIO selector/data plus
the DMA interface) that programs QEMU's `etc/ramfb` file, and a linear
XRGB8888 pixel buffer in kernel RAM (`src/fb.rs`). First bring-up drew a
colour ramp, SMPTE-style bars and corner markers.

**Why.** A display is the prerequisite for the phone GUI; ramfb is the
simplest QEMU display device — no GPU, no 3D, just a RAM buffer QEMU scans
out.

**How.** QEMU removed writes to the fw_cfg data port, so the kernel uses
the DMA interface: a 28-byte big-endian `RAMFBCfg` (address, fourcc, flags,
width, height, stride) plus an aligned DMA descriptor in RAM; QEMU copies
it synchronously when the DMA register is kicked. fw_cfg lives at
`0x0902_0000`. The buffer is 4 KiB-aligned inside the identity-mapped RAM
window, so drawing is ordinary memory stores — zero-copy guest-to-QEMU.
ramfb requires `stride == width * 4` for XRGB8888.

**Verified.** Monitor `screendump` + PPM decode confirmed the test pattern
pixel-for-pixel. Without `-device ramfb` the kernel prints
`ramfb: not configured` and continues headless.

**Limits.** ramfb is a dumb framebuffer: no acceleration, no vsync signal.
The buffer is statically sized (`MAX_WIDTH` 1440 x `MAX_HEIGHT` 2560);
actual panel size became boot-configurable in Ph40. Backend abstraction
arrives in Ph52.

### Ph18 — Framebuffer text console

**Built.** An embedded public-domain 8x8 bitmap font (`src/font.rs`,
`font8x8_basic`, ASCII `0x20..=0x7e`) rendering all console output on the
framebuffer: kernel `println!`, the user `write` syscall, and RX-echo all
mirror to the display. Wrapping, scrolling and backspace work.

**Why.** The GUI-less boot/shell experience should be visible on the same
screen the GUI will later use; it also doubles as the recovery console.

**How.** `fb::putc` handles `\n`, `\r`, backspace, wraps at the last
column and scrolls by copying the pixel buffer up one text row. Geometry:
originally 80x60 cells at 640x480; at today's 1080x1920 default the glyphs
are scaled 3x3 into 24x24 cells (45x80). font8x8 rows are bitmaps with bit
0 as the leftmost pixel; colours are `0x00RRGGBB` stored little-endian
B,G,R,X (pixman `x8r8g8b8`). The framebuffer has its own spin mutex and
all writers run with IRQs masked, so glyphs never interleave.

**Verified.** Exact pixel comparison against glyphs regenerated from the
same font table; a 60-line scroll test (seven `help` commands) left the
expected line at the top row.

**Limits.** Bitmap font only (the GUI later got real typefaces, Ph41).
Console mirroring is suppressed while a GUI owns the display (Ph32/Ph34).

### Ph19 — Graphics syscalls

**Built.** `fb_info` (16): writes `[width, height, stride, format]` to
user memory; `fb_fill` (17): fills a clipped rectangle with an
`0x00RRGGBB` colour. Test program `kimi-fbtest`.

**Why.** EL0 needs a safe way to query and draw on the display before a
full framebuffer mapping exists (Ph32).

**How.** `format` is the DRM fourcc `XR24` (`0x3432_5258`). Rectangle
arguments use saturating arithmetic so huge/off-screen values clip instead
of wrapping; the info pointer is validated against the coarse user window
and must be 8-byte aligned. The console cursor is untouched by fills.

**Verified.** `kimi-fbtest` paints a background plus red/green/blue/yellow
rectangles and blocks on Enter; screendump pixel values matched exactly at
rectangle centres and background elsewhere, both while blocked and after
exit.

**Limits.** One syscall per rectangle is too slow for a compositor —
solved by the shared mapping in Ph32.

### Ph20 — Secondary CPUs (PSCI)

**Built.** SMP bring-up (`src/smp.rs`): PSCI 1.1 over HVC powers on
secondary CPUs; each gets a 16 KiB stack, enables FP/SIMD, installs its
vector table, turns on the MMU with the shared kernel tables, runs a
bounded checksum workload with atomics, then parks in `wfe`.

**Why.** Establish that multiple cores can safely enter Rust shared state
— groundwork for future SMP scheduling.

**How.** The DTB advertises `/psci/method = "hvc"`, so the kernel uses
`hvc #0` (not `smc`). `PSCI_CPU_ON` (`0xC400_0003`) targets MPIDR affinity
values 1-3; absent CPUs return `-2` (`INVALID_PARAMETERS`), keeping
`-smp 1` boots working. Secondaries start with MMU/caches off, so the
`_secondary_start` trampoline runs identity-mapped until
`mmu::enable_secondary` loads the same MAIR/TCR/TTBR0. Each core computes
`sum(value ^ cpu)` over 300,000 iterations and publishes it with atomics;
the boot CPU re-computes and compares.

**Verified.** `-smp 4`: 3/3 checksums ok, "4 CPU(s) online".

**Limits.** Bring-up, not SMP scheduling: GIC redistributor setup, timer
and the EL0 scheduler still run only on CPU 0; secondaries stay parked.
Per-CPU runqueues and IPIs remain future work.

### Ph21 — KimiFS free-space reuse

**Built.** A block bitmap rebuilt from the inode table at mount drives
first-fit allocation; `unlink` and truncation return runs to the bitmap.
New syscalls `unlink` (18) and `freeblocks` (19); shell `rm`; `df` reports
free data blocks.

**Why.** Without reuse, a 4 MiB disk fills permanently after a few
write/delete cycles — unusable for Notes/Camera/kpkg.

**How.** `mount` reconstructs the in-RAM bitmap (reserving
superblock/inode area); `write` grows files via first-fit search;
`unlink`/create-truncation clear the old run. The superblock's
`next_free` became an informational high-water mark. Because the bitmap
is rebuilt from inodes, free counts are correct after reboot without
persisting the bitmap.

**Verified.** An 800-byte file occupied 2 blocks; `rm` returned both;
creating a 1-block file consumed exactly one; counts match across reboots.

**Limits.** At this stage disks over 65,536 sectors (32 MiB) were refused
(fixed 8 KiB bitmap); capacity grew to 64 GiB in Ph40. Files are still
contiguous runs.

### Ph22 — Sleep and uptime

**Built.** `uptime` (20) reads `CNTPCT_EL0` in milliseconds; `sleep` (21)
parks the process until a timer deadline. Shell `time` and `sleep <ms>`.

**Why.** GUI animations, call timers and any timed behaviour need a
clock and a way to yield without busy-waiting.

**How.** `uptime_ms` divides `CNTPCT_EL0` by `CNTFRQ_EL0` using a 128-bit
product (no overflow over long uptimes), independent of the timer's
current interval. `sleep` stores an absolute deadline, marks `Blocked`,
rewinds the SVC and switches; every timer IRQ calls `wake_sleepers(now)`
before preemption so an expired sleeper can run on the same tick. A
per-process `sleep_done` flag makes the retried SVC return 0 instead of
sleeping again. Alone, the kernel idles with IRQs enabled around `wfi`.
The child-exit wake path checks `sleep_at == 0` so exiting children never
wake a sleeping parent (only a parent blocked in `wait`).

**Verified.** `sleep 1000` reports ~1020 ms (includes one 50 ms quantum of
scheduling latency); a `/spin` child interleaves while the shell sleeps.

**Limits.** Granularity is the timer tick; no high-resolution timers.

### Ph23 — Extended WinAPI translation

**Built.** The PE thunk page grew to nine imports: `Sleep` (105),
`GetTickCount64` (106), `GetTickCount` (107) and `WriteConsoleA` (108)
join the original five (100-104).

**Why.** Prove the translation layer handles stateful calls (sleep,
per-process error slots, tick counts), not just console I/O.

**How.** Nine 16-byte thunks; `Sleep` reuses the kernel sleep path (may
switch processes); `GetLastError`/`SetLastError` use a per-process error
slot; `WriteConsoleA` aliases console write and sets the `written`
out-param. `build.rs`'s hand-generated image now calls all nine, sleeping
twice and printing through both `WriteFile` and `WriteConsoleA` (the
larger IAT/ILT moved section offsets but still fits the 1 KiB section).

**Verified.** Messages printed after each sleep prove `Sleep` returns to
the PE image alive; both tick-count calls complete without an
unknown-syscall fault; `ExitProcess(0)` reaps cleanly.

**Limits.** Still a fixed set of 9 imports for one generated binary. The
translation *approach* was later redirected at Linux (Ph47).

### Ph24 — User heap (brk)

**Built.** `brk` (22): queries or grows a per-process heap at
`0x6080_0000` (limit `0x60C0_0000`). New pages are zeroed, mapped EL0
RW/UXN and tracked in the address space. Test program `/heaptest`.

**Why.** User programs need dynamic memory before `alloc` collections can
work (Ph29).

**How.** `brk(0)` returns the current break; larger requests map fresh
pages eagerly and the break moves only after success — a caller ignoring
a short return never sees a half-grown heap. Requests round up to whole
pages; the syscall refuses to start unless `spare_frames()` covers the
pages plus one possible L3 table, so a nearly full address space returns
the old break instead of panicking in `map_page`. `MAX_FRAMES` rose from
64 to 256 per address space (later 1024, Ph41). `exec` resets the break.

**Verified.** `/heaptest` grows 4 pages (16 KiB), writes and reads back
every byte, checks page-rounding of a one-byte over-page request and that
`sbrk(start)` does not move the break.

**Limits.** Shrinking is a deliberate no-op — there is no unmap primitive
yet, so `brk(smaller)` leaves the break alone rather than leaking pages
invisibly.

### Ph25 — Process inspection and termination

**Built.** `ps` (23) copies `[pid, parent, state, exit]` records into user
memory; `kill` (24) turns another process into a zombie with status 137.
Shell `ps`/`kill`; test target `/sleeper`.

**Why.** The GUI and shell need visibility into running processes and a
way to stop them; kill semantics must not corrupt the wait/reap model.

**How.** States: 0 free, 1 ready, 2 running, 3 blocked, 4 zombie. Zombie
slots stay until reaped, so a finished child is visible before `wait`.
`kill` reuses the exit path's bookkeeping: clears the console-reader slot,
reparents children to init, and delivers the status through the saved
`x0` of a parent blocked on exactly that child. It refuses PID 0, itself,
missing processes and existing zombies. On this uniprocessor kernel only
the current process executes, so no stop/switch of the target is needed.

**Verified.** `ps` shows block -> zombie(137) -> reaped transitions;
`wait 2` returns 137; the slot frees afterwards.

**Limits.** No signals beyond kill; no process groups.

### Ph26 — Rename

**Built.** `rename` (25) rewrites a KimiFS inode's 32-byte name field and
flushes it; shell `mv`.

**Why.** File management without copy-delete cycles; a prerequisite for
directory moves later.

**How.** Both paths are resolved under the filesystem lock — checking the
destination and updating the source in one critical section prevents
racing renames from claiming the same name. Missing sources and taken
destinations (including rename-onto-self) are refused. No data blocks
move.

**Verified.** `mv disk/a.txt disk/b.txt` preserves content across reboot;
collisions fail with a clear error.

**Limits.** Disk files only at this stage; names cannot contain `/`;
directories arrive in Ph31 (which also lets rename move subtrees).

### Ph27 — Wall clock (RTC)

**Built.** `rtc` (26) reads the PL031 data register at `0x0901_0000` and
converts Unix seconds to UTC civil time in-kernel (Howard Hinnant's
`civil_from_days`); shell `date` prints `YYYY-MM-DD HH:MM:SS UTC`.
`src/rtc.rs`.

**Why.** The lock screen, status bar and Clock app need real wall time,
not just uptime.

**How.** QEMU defaults to `-rtc base=utc,clock=host`, so the PL031 tracks
the host clock. `rtc(out)` returns `[epoch, year, month, day, hour,
minute, second]` — the kernel does the calendar math so user programs
don't carry the algorithm. A zero data register reads as "no RTC" and
returns an error. (The Android frontend passes `-rtc
base=localtime,clock=host` for host time sync.)

**Verified.** `date` matches host UTC time.

**Limits.** No timezone support in-kernel; seconds resolution.

### Ph28 — File metadata (stat)

**Built.** `stat` (27) reports `[kind, size, blocks]` for RamFS and KimiFS
paths; `ls` gained aligned size columns; shell `stat <file>`.

**Why.** The Files app and shell listings need sizes and kinds; blocks
are reported for callers even though the shell doesn't print them.

**How.** Kind 0 = RamFS entry (no blocks), kind 1 = KimiFS inode (kind 2 =
directory after Ph31). The disk side re-reads the inode under the
filesystem lock, so the size is the on-disk value, not a cached copy.

**Verified.** `ls` shows correct sizes for embedded and disk files;
`stat disk/b.txt` reports kind disk / 6 bytes; `stat motd` reports ramfs /
45 bytes.

**Limits.** No timestamps, permissions or ownership in KimiFS inodes.

### Ph29 — User-space allocator

**Built.** `libkimi::Heap` implements `GlobalAlloc` over `brk`, so `Vec`,
`Box`, `String` and friends work in user binaries. Test `/alloctest`.

**Why.** The GUI and all real apps need collections; writing everything
against fixed buffers does not scale.

**How.** A singly linked free list of 16-byte blocks plus a bump pointer
that calls `sbrk` one page at a time. Freed blocks are reused first-fit
and split when the remainder can hold another block. No coalescing and no
realloc-specific path (default `GlobalAlloc::realloc` = alloc/copy/free).
Alignment above 16 bytes bypasses the free list and is served from the
bump pointer. No lock is needed: allocator state is private to each
process address space and the kernel never preempts inside its
bookkeeping.

**Verified.** `/alloctest`: 64 rounds of 200-element vectors (growth and
drops) produce checksum 21216; a 5000-`u64` vector pushes the break past
sixty pages; sum verified.

**Limits.** Fragmentation possible (no coalescing); per-process heap cap
still applies (`0x60C0_0000`).

### Ph30 — Virtio keyboard

**Built.** A virtio-input keyboard driver (`src/virtio_input.rs`, virtio
id 18): both queues (event/status), eight 8-byte `VIRTQ_DESC_F_WRITE`
event buffers, slot SPI enabled. EV_KEY presses translate from Linux
keycodes to ASCII (shift-aware, symbols/uppercase) and inject through
`uart::inject` — the shell cannot tell keyboard bytes from serial bytes.

**Why.** The phone GUI and shell need real key input, not just QMP
scripting; this is also the first virtio *interrupt-driven* device.

**How.** Two transport bugs surfaced and were fixed: (1) virtio status
bits `DRIVER_OK = 4` / `FEATURES_OK = 8` had been swapped in the block and
input drivers — legacy QEMU didn't enforce `DRIVER_OK`, so disk worked
while input queues never started; (2) the DTB lists virtio-mmio interrupts
as SPI numbers (`16 + slot`), so the GIC INTID is `48 + slot` — using
`16 + slot` enabled the wrong SPI and left events to polling. Both
transports now negotiate correctly.

**Verified.** QMP `input-send-event` typing (including backspace and
shifted `A`) reaches the shell line editor.

**Limits.** ASCII translation only (no compose/Unicode keymaps).

### Ph31 — Directories on KimiFS

**Built.** Inodes gained a 64-bit parent index and a kind byte (1 file,
2 directory); inode 0 reserved so parent 0 unambiguously means root.
Component-wise path resolution shared by `mkdir`/`lookup`/`create`/`stat`/
`unlink`/`rename`; `listdir` (28), `mkdir` (29), `rmdir` (30); legacy
disks migrated once at mount.

**Why.** A flat namespace doesn't scale for apps, packages (`/disk/bin`)
or user files; the Files app needs a tree.

**How.** Resolution walks one component at a time, requiring every
intermediate entry to be a directory; each component must fit the 32-byte
name field. `rename` also rewrites the parent field, so files and whole
subtrees move between directories; moving a directory into its own
subtree is refused. `rmdir` only removes empty directories. `listdir`
returns direct children with trailing `/` for directories, so `ls /disk`
is no longer flat. Old disks are migrated by moving whatever lived in
inode 0 to the first free slot.

**Verified.** Nested `mkdir disk/docs/sub`, writes, `mv` within and
across directories, `ls disk/docs`, non-empty `rmdir` refusal, then
successful teardown — all persisted.

**Limits.** 64 inodes total; 31-char name components; no symlinks or
hardlinks.

### Ph32 — User-space framebuffer mapping

**Built.** `fb_map` (31) walks the framebuffer's physical pages and
installs shared 4 KiB mappings at `0x6600_0000`. Mapping clears the
console mirror flag; it is restored when the mapping process exits or
execs. Test `/gfxtest`.

**Why.** A compositor cannot afford a syscall per rectangle; direct
stores into the pixel buffer are the only way to hit frame budgets under
TCG.

**How.** Unlike `brk` pages, the framebuffer pages are *not* owned by the
process — teardown leaves the buffer alone. Suppressing the console
mirror means kernel/shell text no longer paints over a GUI.

**Verified.** `/gfxtest` writes a 1080x1920 animated gradient eight times
(16.6 million pixels) in ~40 ms under TCG, then paints known rectangles;
screendump confirms exact colours (background `0x102040`, red/green/blue/
yellow rects, one-pixel white rules, corners untouched).

**Limits.** Shared mapping means a misbehaving GUI can scribble anywhere
on screen (fault isolation still protects the kernel, Ph35). No
double-buffering yet — that is Ph50.

### Ph33 — Pointer events

**Built.** A second virtio-input device (`-device virtio-tablet-device`),
device-type detection via config-space `VIRTIO_INPUT_CFG_EV_BITS`
(EV_ABS/EV_REL present -> pointer, else keyboard), and a 128-entry
single-producer/single-consumer event ring (`src/input.rs`). Syscalls
`input_poll` (32) and `input_wait` (33). Test `/inputtest`.

**Why.** A touch-first phone UI needs absolute pointer coordinates and
press/release events, delivered to an event loop without disturbing the
shell's line editor.

**How.** Everything (key press/release, ABS_X/Y, BTN_LEFT/BTN_TOUCH,
relative motion) goes into the ring; keyboards *also* feed the UART line
editor. Events are packed `[type:16][code:16][value:32]`. `input_wait`
rewinds the SVC and marks the process `Blocked` with a dedicated
`INPUT_PID`, so pointer motion wakes the GUI event loop without waking a
shell blocked in `read`; alone, the kernel idles in `wfi`.

**Verified.** QMP-injected absolute axes, BTN_LEFT (272) press/release and
an `a` key (code 30) all arrive in order; `/inputtest` PASS.

**Limits.** 128-entry ring (overflow drops under extreme event rates);
single consumer.

### Ph34 — First user-space GUI

**Built.** `/gui` as a classic event loop composing `fb_map` +
`input_wait`: a desktop, back/front windows with title bars, a raised
button, and a 12x18 arrow cursor that saves/restores the pixels under it.

**Why.** Proof that the pieces (pixel access, events, scheduling) compose
into an interactive application — the seed of the phone shell.

**How.** Two scheduling bugs surfaced and were fixed: (1) `wait` failed
when the child was already blocked (GUI waiting for input) instead of
waiting — it now idles in the kernel with interrupts on until any process
becomes runnable; (2) `sys_exit` reaped the child *after* `switch_to`,
which never returns (dead code) — reaping now happens before the switch,
so a `run` child frees its slot and restores the text console immediately.
While a GUI owns the display, keyboard presses are not fed to the shell
line editor, so quitting leaves no stray escape bytes.

**Verified.** Screendumps: button colour `0x909090` -> `0x30c060` on
click; kernel text restored after ESC/exit.

**Limits.** A drawing demo, not a phone UI — that is Ph36.

### Ph35 — Stability hardening

**Built.** Three changes making the kernel safe for untrusted graphics
programs: a coalescing kernel heap, proper lower-EL fault isolation, and
page-walked syscall pointer validation. Tests `/stress`, `/faulttest`,
`/badptr`.

**Why.** The original bump heap leaked an ELF copy per spawn (ten
children exhausted 1 MiB); faults mislabelled as same-EL killed the wrong
context; raw user pointers could fault in EL1.

**How.** (1) `src/allocator.rs` became a sorted, coalescing free list with
16-byte headers; `elf::load` frees its aligned copy on both success and
error paths. (2) Lower-EL instruction/data aborts now terminate only the
faulting process with `128 + SIGSEGV` (139); alignment faults use SIGBUS
(135); the parent receives the status through the normal wait path.
(3) Every syscall buffer is checked against the coarse user window *and*
walked page-by-page through the current address space (`user_range_ok`);
an unmapped pointer returns `u64::MAX` instead of faulting in EL1.

**Verified.** `/stress` (children, file cycles, allocator rounds) returns
the exact starting free-frame count; `/faulttest` is killed with 139 and
the shell survives; `/badptr` confirms syscalls reject unmapped buffers.

**Limits.** Validation is per-syscall, not a full copy-in/copy-out model;
the kernel heap is still a fixed static region.

---

## Part III — The phone GUI (Ph36-Ph42)

### Ph36 — Portrait phone UI

**Built.** `/gui` became a 480x800 portrait phone shell: lock screen
(live clock, date, padlock, swipe-up unlock), status bar (HH:MM from the
PL031, signal bars, battery), dot-grid wallpaper, 4x3 launcher grid with
hand-drawn glyphs, dock, and first apps — Calculator (AC/sign/backspace/
four ops), analog+digital Clock, Settings (Wi-Fi/Bluetooth/Dark-mode
switches, About).

**Why.** The project's goal is a phone OS experience, not a desktop demo;
this phase established the UI structure everything else builds on.

**How.** A small user-space renderer (`user/kimi-gui/src/draw.rs`):
clipped fills, rounded rectangles, circles, rings, Bresenham lines,
vertical gradients, alpha-blended surfaces. Text at this stage used an
8x8 renderer. Taps are distinguished from drags by a 24 px threshold;
transitions were opt-in/experimental from the start.

**Verified.** All interactions exercised over QMP (unlock, toggles,
calculator arithmetic, clock redraw).

**Limits.** 480x800 logical only; placeholder apps were filled in Ph39;
bitmap text replaced by real typefaces in Ph41.

### Ph37 — Nothing OS x Material 3 redesign

**Built.** A full restyle around the Nothing OS / Material 3 language:
near-black surfaces, dot-grid wallpaper, white type, single red accent;
dot-matrix clock renderer; tonal cards (26-30 px radii), alpha-blended
dock and scrim; M3 switches and a pill brightness slider; lock screen
with notification cards and a fingerprint target drawn from three broken
red ridges; At-a-Glance card, search pill, interactive music widget, 12
monochrome icons, active-pill page indicator; Quick Settings shade
(Wi-Fi/Bluetooth/DND/Torch tiles, working brightness slider, notification
cards, Settings shortcut).

**Why.** Give the shell a coherent, distinctive design language instead
of generic widgets.

**How.** The renderer's alpha blending lets the shade, dock and cards sit
over the wallpaper without a compositor. Motion uses quadratic ease-out
for lock/home and app slides; icons shrink on press; the shade slides
with the scrim.

**Verified.** QMP-driven: unlock, Wi-Fi toggle, brightness set,
shade-to-Settings navigation, calculator `7 * 8 = 56`, live Clock, back
navigation. Screenshots in `docs/screenshot-*.png`.

**Limits.** Still 480x800 logical; software rendering costs under TCG
(addressed in Ph38).

### Ph38 — Native 1080p rendering

**Built.** The panel became 1080x1920 (8.1 MiB XRGB8888) while all layout
stayed in the original 480x800 coordinate space: `Screen` maps
rectangles, circles, lines and text to physical pixels with integer math.
Anti-aliased text (4x supersampling at larger scales, nearest blocks at
scale 1 to match the dot-matrix look), dirty-background repaints, uniform
circle radius, and a 3x3 kernel console font (45x80 cells).

**Why.** Match a real phone panel without rewriting every app's layout
code; keep repaint costs viable under TCG.

**How.** Taps redraw only content; the wallpaper fill happens on scene
changes. A full home repaint measured ~35 ms on the emulated CPU; partial
repaints about half that. Circles use a uniform radius (not the 9:16
vertical scale) so icons and dots stay perfectly round.

**Verified.** Native-resolution screenshots; `/stress` twice returns
exactly 29929 free frames after each exit (ten spawn/wait cycles, twenty
file cycles, a hundred allocator rounds — no frame or heap leaks).

**Limits.** Full-frame copies remain expensive under TCG (~15.6 ms later
measured in Ph52); dirty-rect *present* arrives in Ph51.

### Ph39 — Real apps, boot-to-GUI, experimental motion

**Built.** Every launcher app got real state and behaviour: Phone
(keypad, recent number, call screen, mute/speaker, live call timer),
Messages (three threads, typed text, sent bubbles with word wrap), Camera
(viewfinder; captures the framebuffer to `/disk/camera-N.ppm` in 8 KiB
chunks — the syscall transfer limit), Music (elapsed time, track cycling,
drives the home widget), Notes (512-byte buffer persisted to
`/disk/notes.txt`), Maps (pins with coordinates), Photos (generated
gallery), Files (browses RamFS + KimiFS with sizes and text previews),
Weather (C/F conversion, forecast days), Terminal (in-GUI shell with line
editor, scrollback, real filesystem/RTC/process commands). `kimi-init`
now spawns `/gui` at boot, waits for it, and falls back to the text shell
on exit. Motion became an opt-in Settings toggle (default off).

**Why.** "Coming soon" screens are not a phone; boot-to-GUI makes the
phone UI the default experience with the shell as recovery terminal.

**How.** The process loader's ELF stack grew from 4 KiB to 64 KiB because
UI state and app buffers no longer fit one page. App logic lives in
`user/kimi-gui/src/apps.rs`; persistence goes through the existing VFS
syscalls.

**Verified.** All apps interactive over QMP/on-device; ESC returns to a
clean shell; camera PPMs readable after reboot.

**Limits.** Apps are self-contained simulations (no telephony, real
camera sensor, or media decoding — the hardware doesn't exist in QEMU).

### Ph40 — Bootargs resolution, 4 GiB map, 64 GiB KimiFS

**Built.** Three scaling changes: (1) framebuffer resolution from the
kernel command line — `kimios.fb=WIDTHxHEIGHT` parsed from
`/chosen/bootargs` (`src/dtb.rs`) and applied by `fb::configure`;
(2) the identity map extended to up to 4 GiB of RAM (four L2 tables,
`0x4000_0000..0x1_4000_0000`, `src/mmu.rs`); (3) KimiFS bitmap capacity
grown to 134,217,728 blocks = 64 GiB (`MAX_BLOCKS` in `src/diskfs.rs`).

**Why.** The Android frontend needs per-device resolutions (fit-screen
presets) without rebuilding the kernel; bigger guest RAM and disks are
needed for real packages and files.

**How.** QEMU's `-append` lands in `/chosen/bootargs`; the frontend
passes `-append kimios.fb=<width>x<height>` (see `QemuService.kt`).
`fb::configure` clamps to the static buffer bounds (max 1440x2560).
The larger KimiFS bitmap is rebuilt from inodes at mount as before, so
existing small disks keep working.

**Verified.** Frontend resolution presets produce correctly-sized guest
displays (v0.3.0+); large disks mount and pass file tests.

**Limits.** Resolution is bounded by the static framebuffer allocation;
no dynamic runtime resolution changes.

### Ph41 — Nothing typefaces, bigger loader heap, 1024-frame pool

**Built.** The official Nothing OS typefaces — NType82 (UI), Ndot-55
(clocks/dot indicators), NType82Mono (terminal) — rasterized to alpha
atlases (`user/kimi-gui/src/sans.bin`, `dot.bin`, `mono.bin`,
`font_data.rs`) and rendered with bilinear sampling (`draw.rs`). Kernel
loader heap grew to 4 MiB (`src/allocator.rs`); per-address-space frame
pool grew to 1024 (`MAX_FRAMES` in `src/mmu.rs`).

**Why.** Bitmap 8x8 text undercut the design language; real typefaces are
the single biggest visual upgrade. Larger binaries (the GUI with fonts)
need more loader heap and more frames per process.

**How.** Glyphs are pre-rasterized alpha atlases embedded in the GUI
binary; the renderer samples them bilinearly at arbitrary scales (4x
supersampling at large scales, nearest at scale 1 for the dot-matrix
look). The dot-matrix clock renderer uses Ndot-55 glyphs.

**Verified.** Updated screenshots (`docs/screenshot-lock/home/clock.png`)
show the new type; the GUI loads and runs within the enlarged pools.

**Limits.** Atlases are static (fixed glyph set/sizes baked at build
time); font binaries add ~1.1 MiB to the GUI image.

### Ph42 — Aspect-adaptive UI

**Built.** The GUI adapts to arbitrary panel aspect ratios: dynamic
logical height derived from `fb_info`, compact home/lock layouts for
short screens, and app-space coordinate mapping
(`user/kimi-gui/src/main.rs`: app points map `y * 800 / logical_height`).

**Why.** Frontend fit-screen presets produce many resolutions (e.g.
1080x2400-class tall panels, squarer custom sizes); a fixed 480x800
logical space would letterbox or stretch.

**How.** `Screen` still maps logical to physical pixels, but the logical
height now follows the panel aspect; launcher/lock layouts switch to
compact variants when vertical space is tight; app hit-testing and
drawing share one mapping so touch stays accurate.

**Verified.** On-device across frontend presets; v0.3.1 fixed preset
migration so "Auto" applies correctly.

**Limits.** Layouts are tuned for portrait phone aspects; landscape or
extreme aspects are not a design target.

---

## Part IV — Networking and the Android experiment (Ph43-Ph46)

### Ph43 — Real networking

**Built.** A virtio-net driver (`src/virtio_net.rs`) and a smoltcp-based
network stack (`src/net.rs`): Ethernet/ARP/IPv4/ICMP/UDP/TCP, DHCPv4
leases, DNS resolution, and a small pool of TCP sockets exposed through
`SYS_NET` (34). Shell commands `net`, `dns`, `fetch`. Also shipped: the
Android-feasibility study ([android-feasibility.md](android-feasibility.md)).

**Why.** A phone needs networking; kpkg (Ph48) and the browser (Ph44)
both depend on it. smoltcp is a standalone event-driven bare-metal stack
that fits `no_std` without a large heap.

**How.** `NetDevice`/`NetRxToken`/`NetTxToken` bridge virtio-net to
smoltcp; `net::poll()` drives the interface; `wait_for_ip` blocks on
DHCP; `resolve`, `tcp_connect`, `tcp_send`, `tcp_recv`, `tcp_close` are
the socket API. QEMU user-mode networking (`-netdev user,id=n0 -device
virtio-net-device,netdev=n0`) provides SLIRP NAT: the guest gets
10.0.2.15, the host loopback is 10.0.2.2, DNS forwarder 10.0.2.3.

**Verified.** Live DNS lookup and HTTP fetch from the guest over SLIRP;
on-device networking in frontend v0.4.0.

**Limits.** IPv4 only; a small fixed socket pool; no TLS; the Linux ABI
does not yet expose Linux socket syscalls (guest apps use the native
`SYS_NET` path or the browser/kpkg built on it).

### Ph44 — Mini web browser + home paging

**Built.** A GUI browser app (`user/kimi-gui/src/apps.rs`, `Browser`):
URL bar with line editor, DNS + TCP + HTTP/1.0 GET, and an HTML-to-text
renderer with title extraction, word wrap and scrolling. The launcher
gained paging (`page_apps(page)`, `home_page` state, page dots).

**Why.** Prove the network stack end-to-end with a user-visible app;
paging makes room for more apps (the DEX runner arrived on page 2).

**How.** The browser resolves the host, opens a TCP socket, sends a
minimal HTTP/1.0 request, strips tags to text, and renders into the
app surface with the shared text renderer. Status line reports
DNS/connect/HTTP failures.

**Verified.** Renders live pages over SLIRP on the guest and on-device.

**Limits.** HTTP only (no TLS), no CSS/JS/images, single connection at a
time; DNS reliability on Android hosts was fixed later (Ph51).

### Ph45 — APK inspector

**Built.** `libkimi/src/apk.rs`: ZIP central-directory parsing, DEFLATE
inflate, binary-AXML manifest decoding (string pools/chunks), and DEX
header parsing. Exposed as shell `apk` and a GUI app: package name,
version, permissions, activities, DEX info.

**Why.** Stage 0 (layer L0) of the feasibility study's staged path —
bounded, zero runtime risk, and validates ZIP/inflate/format work needed
for any Android-adjacent feature.

**How.** APKs are read from KimiFS; entries are inflated on demand; the
AXML parser walks chunk headers and the string pool to reconstruct
manifest XML; the DEX header/section tables are dumped. Output was
cross-checked against host tooling (`aapt2 dump` semantics).

**Verified.** Real APKs inspected correctly on the guest (shell + GUI).

**Limits.** Inspection only — executing real APKs needs the full Android
runtime, which the study explicitly scopes out.

### Ph46 — Mini-Dalvik (DEX execution)

**Built.** `libkimi/src/dvm.rs`: a bounded register-based DEX interpreter
with a hand-written micro core library (Object, String, StringBuilder,
System.out, PrintStream). Exposed as shell `dvm <file.dex>` and a GUI
"DEX" runner (home page 2).

**Why.** Stage 2 (layer L2) of the feasibility study: run *purpose-built*
DEX (your own Java via `javac --release 8` + `d8 --debug`, or hand-written
smali) to prove the bounded path is real — while staying honest that
Play-Store APKs are a non-goal.

**How.** Frozen opcode subset: const/move/result, arithmetic in
23x/2addr/lit8/lit16 forms, branches/goto, arrays, iget/iput/sget/sput,
new-instance/new-array, all five invoke kinds + range. Single-threaded,
bump arenas (no GC), no-op monitors, no verifier (trusted DEX);
unimplemented opcodes trap with an error. The parser handles
`method_idx_diff` resetting to absolute at the start of the
virtual-method list (matches libdex). The same `dvm.rs` compiles for the
host, enabling conformance tests without QEMU.

**Verified.** Host-side 8/8 conformance (hello, arithmetic, loop, string
concat, user static method + 3-register math, negatives, int arrays, a
user class with instance field + virtual method); all 8 run on the guest
via the shell; the GUI runner executes `hello.dex` -> "Hello from
KimiOS!".

**Limits.** This is the end of the Android track. It executes
purpose-built bytecode only — real APKs still need Bionic/ART/libcore/
framework/Binder (layers L3-L6, team-scale). That verdict drove the pivot.

---

## Part V — The Pivot: Desktop Lite (Ph47-Ph49)

> **The Pivot.** Android APK compatibility is officially abandoned — the
> Bionic/ART/framework wall is team-scale (see
> [android-feasibility.md](android-feasibility.md)). KimiOS becomes a
> "Desktop Lite" environment in the WSL1 model: a Linux syscall
> translation layer runs standard statically-linked Linux binaries
> (musl/glibc) and native Rust apps, with a sub-5-second boot.

### Ph47 — Linux ABI (syscall translation layer)

**Built.** `src/linux.rs` plus Linux ELF loading in `src/elf.rs`
(`is_linux`, `load_linux`, `build_linux_stack`): standard static Linux
binaries run as ordinary KimiOS processes, with their Linux syscall
numbers translated to kernel services.

**Why.** WSL1 showed that syscall translation can run a real Linux
userland without a Linux kernel — and unlike Android, the static-binary
Linux surface is bounded and solo-achievable. This reuses everything
already built: PFA frames, KimiFS, console, scheduler.

**How.** Detection: `is_linux` recognizes an `ET_EXEC` (non-PIE) ELF
whose lowest `PT_LOAD` sits below the KimiOS user base `0x6000_0000`
(musl/glibc static binaries link at `0x40_0000`). The loader maps the
image at its own link address — the MMU splits device blocks into tables
to map low addresses — and builds a real Linux initial stack
(argc/argv/envp/auxv: `AT_PHDR`, `AT_ENTRY`, etc.). Each process carries
a `linux` flag (`src/process.rs`); the SVC handler routes flagged
processes to `linux::dispatch(frame, number)`. Translation highlights:
`write`/`writev` -> console; `read`/`readv` -> blocking console/file
reads; `exit`/`exit_group` -> process exit; `brk` -> `linux_brk`;
anonymous `mmap` -> frame allocation (`munmap`/`mprotect`/`madvise` are
no-ops); `openat`/`close`/`fstat` -> KimiFS/RamFS; plus
`getpid`/`uname`/`clock_gettime`/`getrandom` and signal no-ops. Errors
return negative errno.

**Verified.** On the guest: hand-written asm, musl C, musl Rust and
glibc-static hello worlds all print and exit 0; a malloc/brk fibonacci
program and `uname` work. Run via `run /disk/<binary>`.

**Limits.** Static non-PIE binaries only (PIE/zero-linked ELFs are
rejected); no dynamic linker; no threads/fork/execve/epoll; futex is a
no-op (safe only because execution is single-threaded); Linux socket
syscalls are not implemented (networking goes through the native API).

### Ph48 — kpkg package manager (+ fast boot)

**Built.** `user/libkimi/src/kpkg.rs`: an apt-like shell command —
`kpkg update [url]` fetches `repo.json` over HTTP (smoltcp) and caches it
at `/disk/repo.json`; `kpkg list` shows packages; `kpkg install <name>`
downloads a static binary to `/disk/bin`; `kpkg run <name>` executes it
through the Linux ABI. Default base: `http://10.0.2.2:8080`. Alongside:
**fast boot** — the legacy timer/scheduler boot demos were trimmed from
`kmain`; the kernel reaches init in ~0.4 s (was ~7-8 s), well under the
5 s Desktop Lite goal.

**Why.** A desktop environment needs software distribution; the default
base points at the host loopback through SLIRP, which is exactly where
the Android frontend's `RepoServer.kt` serves the bundled repo — so
package management works fully offline on-device.

**How.** `update` does an HTTP/1.0 GET of `repo.json` and writes it to
KimiFS; `install` looks up the package's `file`, downloads it to
`/disk/bin/<name>`; `run` spawns it via the Linux ABI. The repo manifest
marks each package `run: true/false` with a reason, keeping scope honest:
npm/Node, pip/CPython and TLS curl need threads/futex/epoll/dlopen/
sockets/TLS and are listed as not-runnable.

**Verified.** End-to-end on the guest: `update` -> `install hello` /
`install fib` -> `run` both; on-device against the bundled repo
(frontend v0.6.0). Boot timing measured from serial timestamps.

**Limits.** HTTP only (no TLS, no signature verification — you control
the repo); no dependency resolution, upgrades or removal yet; `kpkg run`
gained full argv passing in Ph49.

### Ph49 — BusyBox + expanded Linux ABI

**Built.** Real argv passing (`SYS_SPAWN_ARGS` 35,
`usermode::sys_spawn_args` -> `process::sys_spawn_args`, proper
argc/argv/envp/auxv stacks), cwd + directory descriptors, and a much
larger syscall surface in `src/linux.rs`: `getdents64`,
`newfstatat`/`fstat`, `lseek`, `fcntl`, `ioctl` (incl. `TIOCGWINSZ`),
`getcwd`/`chdir`, `mkdirat`/`unlinkat`/`renameat`/`renameat2`,
`dup`/`dup3`, `sendfile`, `pread64`/`pwrite64`, `getuid` family,
`gettimeofday`/`sysinfo`/`sched_getaffinity`, `faccessat`, `umask`, and
reserved stdin/stdout/stderr fds (0/1/2). Kernel heap grown to 8 MiB for
large binaries. Result: a statically-built **BusyBox with 402 applets**
runs on the guest.

**Why.** BusyBox is the fastest way to get a real Unix userland — one
static binary exercising dozens of syscalls at once, and instantly
useful (`ls -l`, `cp`, `wc`, ...).

**How.** Path resolution gained `resolve_at(dirfd, path)` with per-process
cwd; directory fds classify as `Target::Dir` and `getdents64` walks
children; `ioctl(TIOCGWINSZ)` reports the console geometry; `sendfile`
copies between file fds in-kernel. Applets run as
`run /disk/busybox <applet> [args]` or `kpkg run busybox <applet>`.

**Verified.** On the guest: `ls -l`, `cat`, `echo`, `pwd`, `uname -a`,
`wc`, `mkdir`, `cp`, `date` all correct; bundled in the frontend repo
from v0.7.0.

**Limits.** **Interactive `busybox sh` does not work** — it needs
fork/execve, which are not implemented (the spawn-based process model is
deliberate). Individual applets only. Some applets needing unsupported
syscalls (e.g. networking, job control) will fail.

---

## Part VI — Graphics and latency (Ph50-Ph52)

### Ph50 — Double buffering (GUI flicker fix)

**Built.** The GUI renders to an offscreen back buffer and publishes each
finished frame with a single `fb_present` copy: new syscalls
`SYS_FB_BACK` (36, map the back buffer) and `SYS_FB_PRESENT` (37,
publish), `BACK` framebuffer in `src/fb.rs`. The display never shows a
half-drawn frame — fixing the black flicker on scroll. A follow-up moved
the back buffer from file-backed `.data` to zero-init BSS, shrinking the
kernel image by ~15 MiB.

**Why.** Direct-to-screen rendering (Ph32) let VNC sample mid-draw
frames; on the Android frontend this appeared as black flicker during
scrolling and app switches.

**How.** `fb_back` maps `BACK` instead of `FRAMEBUFFER`; the GUI draws
everything offscreen, then `fb_present` copies the finished frame to the
scanout buffer in one fast pass. On the Android side, two RFB client bugs
were fixed in the same release (v0.7.1): VNC's 32bpp padding byte was 0,
which the blit treated as transparent alpha — the client now forces
opaque alpha (RGBX); and DesktopSize/Cursor pseudo-encodings dropped the
connection, causing a constant reconnect/reload loop — they are now
handled (along with LastRect) without disconnecting.

**Verified.** On-device (frontend v0.7.1): scrolling and app switches no
longer flicker; no reconnect loop in the display bridge log.

**Limits.** At this phase `fb_present` still copied the full frame
(~15.6 ms under TCG); dirty-rect present came in Ph51.

### Ph51 — DNS fix, dirty-rect present, instant press feedback, 8 ms poll

**Built.** Four latency/reliability improvements:
1. **Dirty-rect present:** `fb_present(x0, y0, x1, y1)` copies only the
   changed region of the back buffer to the display. The GUI accumulates
   a dirty bounding box (`draw.rs`: `mark_dirty`/`reset_dirty`) and
   presents just that rect, so VNC sends smaller updates.
2. **Instant press feedback:** on touch-down the GUI draws only the touch
   ring over the existing frame and presents its small bounding box
   (`press_feedback` in `user/kimi-gui/src/main.rs`) — visible response
   before the app's full repaint.
3. **8 ms input poll:** the GUI event loop's idle sleep dropped to 8 ms
   (`k::sleep_ms(8)`), cutting click-to-response latency.
4. **Browser DNS fix:** `net.rs` now lists public resolvers first
   (1.1.1.1, 8.8.8.8) with DHCP-provided servers as fallbacks, and
   smoltcp is built with `SMOLTCP_DNS_MAX_SERVER_COUNT=3`
   (`.cargo/config.toml`). QEMU's 10.0.2.3 forwarder relies on the host's
   resolver config, which is absent on some hosts (Android), and smoltcp
   waits 10 s before falling back to the next server — public DNS via
   SLIRP UDP forwarding works everywhere.

**Why.** After double buffering, the remaining guest-side costs were
full-frame copies, slow press response, coarse input polling, and DNS
timeouts that made the browser look broken on-device.

**How.** The ramfb backend's present became a rect copy; the virtio-gpu
backend (Ph52) transfers the same rect. The Android release (v0.7.2)
paired this with transport optimizations: `-vnc
127.0.0.1:0,non-adaptive=on,lossy=off` (no idle back-off, lossless), Raw +
CopyRect encodings only, and a pure row `memcpy` into an RGBX_8888
surface (no alpha OR, no color conversion, no CPU scaling —
SurfaceFlinger stretches in hardware).

**Verified.** On-device: typing/scrolling/press feedback visibly
snappier; browser DNS resolves on Android hosts; guest-side dirty-rect
present measured ~0.44 ms for a 256 px rect vs ~15.6 ms full-frame
(quantified in Ph52's `gfxbench`).

**Limits.** The VNC refresh timer (~30 ms) + encode/decode/socket remain
the end-to-end floor. The real lever is removing VNC entirely via an
ashmem/shared-memory framebuffer — designed in the frontend's
`docs/display-pipeline.md`, in progress.

### Ph52 — Hardware-independent graphics interface + VirtIO-GPU 2D

**Built.** The GUI is fully decoupled from QEMU graphics: it renders to
the back buffer and publishes via `fb_info`/`fb_back`/`fb_present`;
`src/fb.rs` dispatches to a backend selected by the `kimios.gpu=` boot
argument (`auto`/`ramfb`/`virtio`, parsed in `src/dtb.rs`, stored in a
`BACKEND` atomic). A VirtIO-GPU 2D driver (`src/virtio_gpu.rs`) is
implemented: device init + 2D feature negotiation, control/cursor
virtqueues, `RESOURCE_CREATE_2D`, `RESOURCE_ATTACH_BACKING`,
`SET_SCANOUT`, `TRANSFER_TO_HOST_2D`, `RESOURCE_FLUSH`,
`GET_DISPLAY_INFO`; `present(rect)` transfers the dirty rect into the
host resource and flushes to scanout. Added `gfxbench` (shell command,
`SYS_GFX_BENCH` 38, `fb::benchmark`): CNTPCT-timed, 50 iterations.

**Why.** The same GUI binary should run on ramfb today and on VirtIO-GPU
(or a future accelerated backend) without modification — no
QEMU-specific code in `kimi-gui`. This also stages the path to GPU
acceleration without betting the boot path on it.

**How.** `auto` (default) tries VirtIO-GPU and falls back to ramfb if it
is absent or fails; `ramfb`/`virtio` force a backend. The console routes
through the active backend too, so boot text and the shell render
identically on either. ramfb remains zero-copy guest-to-QEMU and is the
stable boot/recovery fallback.

**Verified.** `gfxbench` under TCG on the test device: ramfb full
1080x1920 present (8 MB copy) **15.66 ms** (~529 MB/s); 256x256 dirty-rect
present **0.44 ms** (~597 MB/s) — dirty rects are ~35x cheaper, which is
why scrolling/typing/press feedback feel snappy. The system boots and
renders on both the fallback and forced-ramfb paths; the frontend bundles
this kernel from v0.8.0.

**Limits (honest).**
- On the tested QEMU builds (host 11.1.2 and bundled 10.2.1),
  `RESOURCE_ATTACH_BACKING` returns `VIRTIO_GPU_RESP_ERR_UNSPEC` with no
  guest-error log — for single-page, single-large and page-sized
  (2025-entry) backing lists alike, at valid RAM addresses. The resource
  image is confirmed created (a pre-attach transfer reports "no backing
  storage", not "is a blob"). The tested builds use a refactored
  virtio-gpu whose attach path differs from upstream master; the
  shareable-image/udmabuf path likely requires a non-`none` display
  backend. **Until resolved, the backend auto-falls back to ramfb.** Next
  debugging step: run against QEMU with `-display egl-headless`/`sdl` (or
  upstream master) and trace
  `virtio_gpu_create_mapping_iov`/`init_udmabuf`.
- For a single software-rendered framebuffer shown over VNC, VirtIO-GPU
  2D is *not* a latency win over ramfb — it adds a virtqueue round-trip
  plus a guest-to-host-resource copy per flush. Its value is the
  hardware-independent interface and a foundation for acceleration.
- Accelerated backends (VirGL/OpenGL, Venus/Vulkan, rutabaga/gfxstream)
  are designed but **staged until 2D is stable**; they also require the
  GUI to render through a GPU API, which is a rewrite, not a flag.
- The full-frame 8 MB copy (~15.6 ms under TCG) is a real app-launch
  cost; on bare metal it would be ~1 ms. The biggest end-to-end win
  remains the ashmem shared-memory transport (works with either
  backend) — see [graphics-architecture.md](graphics-architecture.md)
  and the frontend's `docs/display-pipeline.md`.

---

## Where this leaves the project

Working today: a boot-to-GUI phone OS on QEMU `virt` with persistent
storage, networking, a real (bounded) Linux userland via syscall
translation, offline package management, and an Android frontend that
runs all of it on a phone.

Open fronts, in rough priority order:

1. **ashmem/shared-memory display transport** (frontend) — removes the
   VNC latency floor; design complete, implementation in progress.
2. **VirtIO-GPU attach_backing** — debug against a non-`none` display
   backend / upstream QEMU; unlocks the 2D path and, later, acceleration.
3. **Linux ABI growth** — fork/execve (interactive shells), then
   threads/futex/epoll and sockets as real binaries demand them.
4. **SMP scheduling** — per-CPU runqueues, redistributor init, timer/IPIs
   on secondary cores.
5. **Bare metal (spes)** — blocked on bootloader unlock and stock-image
   dumps; see [hardware.md](hardware.md).
