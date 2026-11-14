# KimiOS

A small bare-metal operating system for ARM64 (AArch64), written in Rust with
`#![no_std]` and `#![no_main]`, running on QEMU's `virt` machine.

No bootloader is used: QEMU loads the ELF directly with `-kernel`.

## Current features

- **Phase 1 - Boot:** linker script at `0x4008_0000`, `_start`, stack setup,
  `.bss` zeroing, secondary cores parked, panic handler.
- **Phase 2 - UART:** PL011 driver at `0x0900_0000`, 115200 8N1, `core::fmt::Write`,
  `print!` / `println!` macros, `spin::Mutex` for safe console access.
- **Phase 3 - Exceptions:** 2 KiB-aligned AArch64 vector table in `VBAR_EL1`,
  full register save/restore, synchronous exception handler that decodes
  `ESR_EL1` / `FAR_EL1` and resumes past `BRK`.
- **Phase 4 - Memory:** 4 KiB-granule translation tables (L0 -> L1 -> L2,
  2 MiB blocks), identity map with Device memory for peripherals and Normal
  memory for RAM, MMU + caches enabled via `SCTLR_EL1`, and a 1 MiB bump heap.
- **Phase 5 - DTB + PFA:** captures `x0`, validates/scans for the
  `0xd00dfeed` DTB, parses `/memory` with `hermit-dtb`, and manages 4 KiB
  physical frames with a bitmap allocator (alloc/free/stats).
- **Phase 6 - Interrupts:** GICv3 distributor/redistributor, system-register
  CPU interface (`ICC_SRE_EL1`, `ICC_PMR_EL1`, `ICC_IGRPEN1_EL1`), EL1
  physical timer on PPI 30, separate synchronous/IRQ vector paths, IRQ-safe
  console output.
- **Phases 7-8 - Threads + scheduler:** callee-saved context switch
  (`x19-x30`, SP, DAIF), 64 KiB kernel thread stacks from the bump heap,
  thread trampoline, `thread_exit`, and a preemptive round-robin scheduler
  driven by the timer IRQ.
- **Phase 9 - EL0 + syscalls:** L3 4 KiB pages (AP=EL0 RW, UXN controlled by
  the ELF `PF_X` flag, PXN=1), `eret` into EL0, SVC decoding from
  `ESR_EL1.EC`, and `write`/`exit` syscalls.
- **Phase 10 - VFS + RamFS + ELF:** `Vfs` trait (`open`/`read`/`write`/`close`),
  a read-only RamFS embedding `kimi-init` via `include_bytes!`, and an
  `xmas-elf` loader that allocates PFA frames, copies `PT_LOAD` segments,
  zeroes BSS, and maps them with user permissions. `build.rs` builds the
  user program with a separate target directory.
- **Phase 11 - User shell:** a `libkimi` user crate with syscall wrappers
  (`write`, `exit`, `read`, `open`, `read_file`, `close`, `exec`, `list`),
  a shell (`kimi-init`) with `ls`, `cat`, `exec`, `help` and `exit`, and a
  second ELF (`kimi-hello`) that `exec` replaces the shell with. `build.rs`
  builds the whole `user/` Cargo workspace and embeds both binaries.
- **Phase 12 - PE + WinAPI translation:** a PE32+ (AArch64) loader that maps
  sections, parses the import table, and patches the IAT with generated EL0
  thunks. The `kernel32.dll` calls `GetStdHandle`, `WriteFile`, `ExitProcess`,
  `GetLastError` and `SetLastError` are translated to KimiOS syscalls
  (100-104), so the generated `hello.exe` runs at EL0 and prints through the
  kernel console.
- **Phase 13 - Processes:** per-process address spaces (private TTBR0 L3
  tables), PID/parent/state tracking, exit statuses and reaping, `spawn` /
  `exec` / `wait` / `getpid` (no `fork`), per-process file descriptor tables
  with ownership, and eager teardown of address-space frames. `init` is PID 1
  and adopts orphans. The shell gained `spawn`, `run`, `wait` and `mem`.
- **Phase 14 - Persistent storage:** a block-device trait with a virtio-mmio
  driver (legacy v1 and modern v2 transports, polling request queue) and
  KimiFS, a small on-disk filesystem with a superblock, fixed inode table and
  contiguous file data. Files under `/disk/` survive reboots; `/init`,
  `/hello`, `/hello.exe`, `/fstest`, `/spin` and `/motd` remain embedded in
  RamFS.
- **Phase 15 - Preemptive EL0 scheduling:** the timer IRQ now round-robins
  runnable user processes with a 50 ms quantum, saving/restoring the EL0
  `ExceptionFrame` and switching `TTBR0_EL1` from the IRQ path. `spawn /spin`
  twice demonstrates interleaved execution.
- **Phase 16 - Blocking console input:** the PL011 receive interrupt (SPI 33)
  drains the RX FIFO, echoes/edits characters and queues completed lines. When
  `read` has no line it marks the process `Blocked` and rewinds its `SVC`; the
  console IRQ wakes the reader so background processes keep running while the
  shell waits at the prompt.
- **Phase 17 - Framebuffer:** a minimal fw_cfg driver (MMIO selector/data plus
  the DMA interface) programs QEMU's `ramfb` file with a 640x480 XRGB8888
  linear buffer in kernel RAM. Drawing is a plain store to the identity-mapped
  buffer; no GPU or 3D is involved. Verified by QEMU `screendump`.
- **Phase 18 - Text console:** an embedded public-domain 8x8 bitmap font
  renders all console output (80x60 cells) on the framebuffer, mirroring both
  kernel `println!` and user `write` syscalls. Lines wrap and scroll; typed
  input and backspace are echoed. Verified by exact pixel comparison against
  the font tables and a 60-line scroll test.
- **Phase 19 - Graphics syscalls:** EL0 can query the framebuffer
  (`fb_info`: width, height, stride, format) and fill clipped rectangles
  (`fb_fill`) with `0x00RRGGBB` colours. A `kimi-fbtest` user program draws a
  known pattern and blocks until Enter; pixel values were verified exactly.
- **Phase 20 - SMP bring-up:** PSCI 1.1 over HVC powers on secondary CPUs.
  Each core gets a 16 KiB stack, enables FP/SIMD, installs the vector table and
  turns on the MMU with the shared kernel tables, runs a bounded checksum
  workload with atomics, then parks in `wfe`. Verified with `-smp 4`.
- **Phase 21 - KimiFS space reuse:** a block bitmap rebuilt from the inode
  table at mount drives first-fit allocation. `unlink` and truncation return
  runs to the bitmap, so deleting and recreating files reuses blocks. New
  `unlink` (18) and `freeblocks` (19) syscalls plus an `rm` shell command;
  `df` now reports free data blocks.
- **Phase 22 - Sleep and uptime:** `uptime` (20) reads `CNTPCT_EL0` in
  milliseconds; `sleep` (21) parks the process until a timer deadline. With
  another runnable process it switches away (rewound SVC); alone, the kernel
  idles with `wfi`. Timer IRQs wake sleepers, and child exit no longer wakes a
  sleeping parent. New shell `time` and `sleep <ms>` commands.
- **Phase 23 - More WinAPI translation:** the PE loader now resolves 9
  `kernel32.dll` imports. `Sleep`, `GetTickCount`, `GetTickCount64` and
  `WriteConsoleA` join the original five; the generated AArch64 PE calls all
  of them, sleeping twice and printing through both `WriteFile` and
  `WriteConsoleA`.
- **Phase 24 - User heap:** `brk` (22) queries or grows a per-process heap at
  `0x6080_0000`. New pages are zeroed, mapped EL0 RW/UXN, and tracked in the
  address space; the break only moves after a success. Requests round up to a
  page, growing fails safely when frames run out, and shrinking is a no-op.
  New `/heaptest` verifies a 4-page growth byte by byte.
- **Phase 25 - Process control:** `ps` (23) copies `[pid, parent, state,
  exit]` records into user memory; `kill` (24) turns another process into a
  zombie with status 137, clears it as console reader, adopts its orphans and
  wakes a parent blocked on it. Shell `ps` prints the table and `kill <pid>`
  terminates a child; `/sleeper` is a quiet long-lived test target.
- **Phase 26 - Rename:** `rename` (25) rewrites a KimiFS inode's name field
  and flushes it, refusing missing sources and taken destinations. Shell `mv`
  moves disk files without copying data.
- **Phase 27 - Wall clock:** `rtc` (26) reads the PL031 data register at
  `0x0901_0000` and converts Unix seconds to UTC civil time (Hinnant's
  `civil_from_days`); shell `date` prints `YYYY-MM-DD HH:MM:SS UTC`.
- **Phase 28 - File metadata:** `stat` (27) reports `[kind, size, blocks]`
  for RamFS and KimiFS paths; `ls` now aligns names and shows sizes, and the
  shell gained a `stat <file>` command.
- **Phase 29 - User-space allocator:** `libkimi::Heap` implements
  `GlobalAlloc` over `brk` with a first-fit free list, 16-byte alignment and
  page-rounded growth. User binaries can now use `alloc::vec::Vec` and
  friends; `/alloctest` exercises reuse and a 40 KiB vector.

## Layout

```text
.cargo/config.toml   target + linker flags
Cargo.toml           dependencies, panic=abort
build.rs             builds user/, copies init.elf + hello.elf, generates hello.exe
linker.ld            0x4008_0000, .text.boot first, vector table kept, 64 KiB stack
user/                EL0 workspace: libkimi (syscalls), kimi-init (shell), kimi-hello,
                     kimi-fstest, kimi-spin, kimi-fbtest
src/boot.s           _start: EL1 entry, FP/SIMD enable, stack, .bss, kmain
src/vectors.s        16-entry exception vector table + save/restore
src/main.rs          kmain, panic handler, semihosting exit
src/uart.rs          PL011 UART + print macros
src/exceptions.rs    exception frame and Rust handler
src/mmu.rs           page tables, MAIR/TCR/TTBR/SCTLR setup
src/allocator.rs     bump allocator
src/dtb.rs           DTB discovery + /memory parsing
src/pfa.rs           bitmap physical frame allocator
src/gic.rs           GICv3 distributor/redistributor + ICC_* CPU interface
src/timer.rs         ARM generic timer (CNTP) periodic ticks
src/irq.rs           IRQ acknowledge/EOI + timer tick handler
src/sched.rs         thread struct, spawn, context switch, round-robin
src/switch.s         context_switch + thread_entry trampoline
src/usermode.rs      EL0 entry, syscall implementations
src/ramfs.rs         VFS trait + embedded RamFS
src/block.rs         block-device trait + registered device
src/virtio_blk.rs    virtio-mmio block driver (legacy v1 + modern v2)
src/diskfs.rs        KimiFS: superblock, inodes, block bitmap, first-fit runs
src/fwcfg.rs         fw_cfg MMIO selector/data + DMA descriptor interface
src/fb.rs            ramfb configuration, 8x8 text console, scrolling
src/font.rs          public-domain font8x8_basic glyph table (ASCII 0x20-0x7e)
src/elf.rs           xmas-elf loader -> per-process AddressSpace
src/pe.rs            PE32+ loader + kernel32 thunks
src/process.rs       process table, spawn/exec/wait/exit, FDs, TTBR0 switching
src/smp.rs           PSCI CPU_ON, secondary stacks/entry, parallel self-test
virt.dtb             trimmed QEMU virt DTB (regenerate if -m/-smp changes)
```

## Prerequisites

```bash
rustup target add aarch64-unknown-none
# Debian / Kali: qemu-system-aarch64 lives in qemu-system-arm
sudo apt install qemu-system-arm
```

## Build and run

```bash
cargo build

# Optional 4 MiB disk image. Without it the kernel runs from RamFS only.
truncate -s 4M disk.img

qemu-system-aarch64 \
  -machine virt,gic-version=3 \
  -cpu cortex-a53 \
  -m 128M \
  -nographic \
  -semihosting-config enable=on,target=native \
  -dtb virt.dtb \
  -drive if=none,format=raw,file=disk.img,id=hd0 \
  -device virtio-blk-device,drive=hd0 \
  -kernel target/aarch64-unknown-none/debug/kimios
```

QEMU 11 defaults virtio-mmio to the legacy (v1) transport, which the driver
supports. Add `-global virtio-mmio.force-legacy=false` to exercise the modern
(v2) path.

For display output add `-display none -device ramfb` to the command line (or
keep the default window). Add `-device virtio-keyboard-device` for keyboard
input; QEMU routes monitor/QMP key events to it even without a graphical
window. `-display none` is convenient in headless shells;
`screendump` on the monitor still captures the framebuffer.

Add `-smp 4` to exercise the Phase 20 PSCI bring-up (CPU 1-3). The default
single CPU boot prints `not present` for the missing cores and continues.

Expected output ends with:

```text
MMU enabled: SCTLR_EL1.M = 1
Heap block: 0x000000004008b000..0x000000004008b100 (readback 0xab)
Heap usage : 256 / 1048576 bytes

Phase 5: device tree and physical memory
DTB         : 0x0000000040000000 (36384 bytes, source: RAM scan)
/memory     : 0x0000000040000000..0x0000000048000000 (128 MiB)
PFA         : 32768 frames (128 MiB), 32339 free
allocated   : 0x401ad000, 0x401ae000, 0x401af000
freed       : 0x401ae000; reallocated 0x401ae000 (reused = true)
PFA free    : 32336 frames

Phase 6: GICv3 and timer interrupts
CNTFRQ_EL0  : 62500000 Hz
Timer       : PPI 30, 31250000 ticks per tick (500 ms)
Unmasking IRQs...
Tick! (1)
...
Tick! (5)

Received 5 timer interrupts.

Phases 7-8: kernel threads and round-robin scheduler
Spawned A, B and one-shot C (64 KiB stacks)
Quantum: 100 ms; running for 30 ticks...
AB
[C] one-shot thread ran; returning to scheduler
ABABABABABABABABAB
Scheduler stats: A=10 B=10 ticks=35
Phases 1-8 complete.

Phase 12: RamFS, ELF/PE loaders, EL0 shell
RamFS      : /init (72880 bytes)
VFS read   : 4 bytes, magic [7f, 45, 4c, 46]
  PT_LOAD va=0x60000000 filesz=76 memsz=76 flags=X R
  PT_LOAD va=0x60001000 filesz=88 memsz=88 flags=  R
ELF loaded : entry=0x60000000 stack_top=0x61001000 pages=3
Dropping to EL0...

KimiOS user shell (libkimi)
Commands: ls, cat <file>, exec <file>, help, exit
$ ls
/init
/hello
/hello.exe
/motd
$ cat /motd
Welcome to KimiOS!
This file lives in RamFS.
$ exec /hello
exec: loading /hello
  PT_LOAD va=0x60000000 filesz=76 memsz=76 flags=X R
  PT_LOAD va=0x60001000 filesz=53 memsz=53 flags=  R
exec: /hello -> entry 0x60000000
Hello from /hello, loaded by exec!
Cheerio from EL0.

[kernel] EL0 exit(0)
```

`exec` also detects PE images by their `MZ` signature. For `/hello.exe` the
loader maps the image at `0x6200_0000`, resolves the `kernel32.dll` imports to
thunks at `0x6300_0000`, and jumps to the PE entry point with a fresh stack:

```text
$ exec /hello.exe
exec: loading /hello.exe
  PE section va=0x62001000 vsize=1024 raw=1024 X
  import kernel32.dll!GetStdHandle
  import kernel32.dll!WriteFile
  import kernel32.dll!ExitProcess
PE loaded  : base=0x62000000 entry=0x62001000 pages=3 imports=3
exec: /hello.exe -> entry 0x62001000
Hello from an ARM64 PE on KimiOS!

[kernel] PE ExitProcess(0)
```

`hello.exe` is generated by `build.rs` (there is no Windows ARM64 toolchain in
the project): it contains a hand-assembled `_start` that calls
`GetStdHandle(STD_OUTPUT_HANDLE)`, `WriteFile` and `ExitProcess`, plus a real
PE32+ header, section table, import descriptor, ILT/IAT and hint/name table.

The shell reads lines with the `read` syscall (echoing and handling backspace),
`ls` returns NUL-separated names via the `list` syscall, and `cat` opens a file
and copies it out in chunks with `read_file`. `exec` loads another embedded
ELF into the same address space and rewrites the exception return frame
(`ELR_EL1`) and `SP_EL0`, so the syscall returns directly into the new program
at EL0.

### Phase 13 - processes

Every process gets its own `AddressSpace`: a private copy of the kernel's L2
table whose L1 entries point at fresh L3 tables for the user region. Frames
are allocated from the PFA, mapped with the right UXN/PXN/AP bits, and released
when the process exits (including after `exec`). The scheduler-free process
model uses `spawn` as the primitive: the parent blocks in `wait`, the kernel
switches TTBR0 and the saved exception frame, and resumes the parent with the
child's exit status. PID 1 (`/init`) adopts orphans; when it exits and nothing
is runnable, the kernel shuts QEMU down.

```text
Phase 13: per-process address spaces, spawn/exec/wait
  PT_LOAD va=0x60000000 filesz=5580 memsz=5580 flags=X R
  PT_LOAD va=0x60002000 filesz=884 memsz=884 flags=  R
[kernel] created pid 1 (parent 0) at 0x60000034

KimiOS user shell (libkimi), pid 1
Commands: ls, cat, spawn, run, wait, mem, exec, help, exit
$ run /hello
[kernel] created pid 2 (parent 1) at 0x6000001c
spawned pid 2
Hello from /hello, pid 2!
Cheerio from EL0.
[kernel] pid 2 exited with status 0
pid 2 exited with status 0
$ spawn /hello
[kernel] created pid 4 (parent 1) at 0x6000001c
spawned pid 4
$ spawn /hello.exe
[kernel] created pid 5 (parent 1) at 0x62001000
spawned pid 5
$ wait 0
Hello from /hello, pid 4!
Cheerio from EL0.
[kernel] pid 4 exited with status 0
child exited with status 0
$ wait 0
Hello from an ARM64 PE on KimiOS!

[kernel] PE ExitProcess(0)
[kernel] pid 5 exited with status 0
child exited with status 0
$ mem
free frames: 32267
$ exit
bye
[kernel] pid 1 exited with status 0
[kernel] no runnable process; shutting down
```

The free-frame count is identical before and after repeated spawn/exit cycles,
which verifies that address spaces, L3 tables and user frames are returned to
the PFA.

### Phase 14 - persistent storage

`block::BlockDevice` is the hardware-independent interface: read/write one
512-byte sector, plus flush. `virtio_blk` scans the 32 virtio-mmio slots at
`0x0a00_0000`, initialises queue 0 (256 descriptors for legacy, 8 for modern),
and submits synchronous three-descriptor chains (header, sector buffer,
status). The driver supports both the legacy `QueuePFN` transport and the
modern v2 register map, so it works with QEMU's default and with
`-global virtio-mmio.force-legacy=false`.

KimiFS puts a 64-byte superblock in LBA 0, 64 fixed 128-byte inodes in LBA
1-16, and file data from LBA 17. Each file is one contiguous run; growing a
file allocates a new run and copies the old data. `spawn`/`exec` still load
executables from RamFS, while `/disk/<name>` is served by the disk filesystem
through the same VFS descriptor table:

```text
Phase 14: persistent storage
virtio-blk  : 8192 sectors (4 MiB), mmio v1
[disk] KimiFS mounted: 8192 sectors, 3 file(s), next free 22

$ df
disk sectors: 8192, files: 3
$ run /fstest
fstest: PASS (800 bytes across 2 sectors)
$ write /disk/hello Greetings from persistent storage
$ cat /disk/hello
Greetings from persistent storage
$ exit
```

Rebooting with the same `disk.img` mounts the filesystem instead of
formatting it, and `cat /disk/hello` returns the saved text. The free-frame
count and file contents confirm that process teardown and disk updates both
work.

### Phase 15 - preemptive EL0 scheduling

The EL1 physical timer is restarted before dropping to `/init`, and init's
`SPSR_EL1` now has IRQs unmasked, so timer interrupts arrive from EL0. The
lower-EL IRQ vector saves the same 272-byte `ExceptionFrame`; `irq_handler`
receives that frame, re-arms the timer, EOI's the GIC, and calls
`process::preempt`. It scans forward from the current slot for a `Ready`
process, saves the interrupted frame and `SP_EL0`, installs the next
process's frame and `TTBR0_EL1`, and `eret`s into it. A tick with no other
runnable process keeps the current one running.

```text
$ spawn /spin
[kernel] created pid 2 (parent 1) at 0x60000000
spawned pid 2
$ spawn /spin
[kernel] created pid 3 (parent 1) at 0x60000000
spawned pid 3
$ wait 0
22223322233332222333322223333222233322
[kernel] pid 2 exited with status 0
child exited with status 0
$ wait 0
3333333
[kernel] pid 3 exited with status 0
child exited with status 0
```

Kernel syscalls still run with IRQs masked by the exception entry. The console
`read` was the one syscall that used to busy-wait holding the CPU; Phase 16
replaces that with a blocking process state.

### Phase 16 - blocking console input

The PL011 receive interrupt is routed through the GIC as SPI 33. The IRQ
handler drains the FIFO, echoes characters, handles backspace and assembles a
line; every newline is appended to a four-line queue so fast or pasted input
is not lost.

When `read` finds no completed line it marks the process `Blocked`, rewinds
`ELR_EL1` by one instruction and switches to another runnable process. On
console input the IRQ handler moves that reader back to `Ready`, and the timer
IRQ's round-robin (or an immediate switch from the console IRQ) resumes it.
Because `ELR_EL1` points back at the `svc`, EL0 re-executes the syscall, which
now finds the queued line. If nothing else is runnable, the kernel idles with
`wfi` until input instead of spinning.

The kernel exits QEMU cleanly (`status 0`) through ARM semihosting
(`SYS_EXIT_EXTENDED`, `HLT #0xF000`). This is temporary scaffolding until a
proper shutdown driver exists.

```text
$ spawn /spin
[kernel] created pid 2 (parent 1) at 0x60000000
spawned pid 2
$ 222222222222222help
mem
  ls             list RamFS files
  ...
$ free frames: 32211
$ 222222222222222222222222
[kernel] pid 2 exited with status 0
```

The echoed `help`/`mem` lines show input arriving while the shell was parked;
the interleaved `2`s show the child kept running on the CPU in the meantime.

### Phase 17 - framebuffer via ramfb

QEMU's `ramfb` device presents an `etc/ramfb` fw_cfg file. QEMU removed writes
to the fw_cfg data port, so the kernel uses the DMA interface: a 28-byte
big-endian `RAMFBCfg` (address, fourcc, flags, width, height, stride) is
written into RAM together with an aligned DMA descriptor, and QEMU copies it
synchronously when the kernel kicks the DMA register.

The pixel buffer is a 4 KiB-aligned `[u32; 640*480]` static in `.bss`. It is
inside the identity-mapped RAM window, so QEMU can read it directly; drawing
is ordinary memory stores and the display refreshes without extra calls.

```text
$ qemu-system-aarch64 ... -display none -device ramfb
...
Phase 17: framebuffer display
ramfb       : 640x480 XRGB8888 at 0x40100000 (1200 KiB)
```

The initial bring-up drew a colour ramp, an eight-bar SMPTE-style strip and
four corner markers, verified by issuing `screendump` on the QEMU monitor and
decoding the PPM. Phase 18 replaces that pattern with a text console. Without
`-device ramfb` the kernel prints `ramfb: not configured` and continues.

### Phase 18 - framebuffer text console

`src/font.rs` embeds the public-domain `font8x8_basic` table for ASCII
`0x20..=0x7e`. The framebuffer is divided into 80x60 cells of 8x8 pixels;
`fb::putc` handles `\n`, `\r` and backspace, wraps at column 80, and scrolls
by copying the pixel buffer up one text row.

The UART driver mirrors every logical byte into the framebuffer in all three
output paths: `fmt::Write` (kernel `print!`), the `write` syscall, and the RX
interrupt echo. The framebuffer has its own spin mutex, while all writers run
with IRQs masked, so console output and input echo cannot interleave inside a
glyph.

```text
$ qemu-system-aarch64 ... -display none -device ramfb
Phases 17-18: framebuffer display and text console
ramfb       : 640x480 XRGB8888 at 0x400f8000 (1200 KiB, 80x60 text)
...
$ help
  ls             list RamFS files
  ...
```

Verification compared the framebuffer against glyphs regenerated from the same
font table: the first mirrored line matched pixel-for-pixel, and after seven
`help` commands the top row was exactly the expected help line (proving the
60-row scroll), with all 60 text rows in use.

### Phase 19 - EL0 graphics syscalls

Two syscalls expose the framebuffer to user processes:

- `fb_info(u64 *out)` (16) writes `[width, height, stride, format]` and returns
  0; `format` is the DRM fourcc `XR24` (`0x3432_5258`).
- `fb_fill(x, y, width, height, color)` (17) fills a rectangle with an
  `0x00RRGGBB` colour. Coordinates are clipped to the screen; the console
  cursor is untouched.

`kimi-fbtest` queries the geometry, paints the screen and four coloured
rectangles, then blocks in `read` so a screenshot can be taken before it exits.
The test verified `red/green/blue/yellow` at the rectangle centres and the
background elsewhere, both while the process was blocked and after it exited:

```text
$ run /fbtest
fbtest: 640x480 stride 2560 format 875713112
fbtest: drew pattern; press enter to exit
```

The `format` value is `0x34325258`; stride is 2560 bytes. Text printed after a
fill is drawn on top of it, because the console and the syscall share one
framebuffer mutex but keep independent cursors.

### Phase 20 - secondary CPUs (PSCI)

The checked-in `virt.dtb` advertises `/psci/method = "hvc"` and PSCI 1.1, so
the kernel calls `PSCI_VERSION` and `PSCI_CPU_ON` with `hvc #0`. Targets are
the MPIDR affinity values 1, 2 and 3; absent CPUs simply return `-2`
(`INVALID_PARAMETERS`), which keeps a default `-smp 1` boot working.

PSCI starts a secondary at EL1 with translation and caches off. The
`_secondary_start` trampoline selects a private 16 KiB stack from a static
array, enables `CPACR_EL1.FPEN`, installs `VBAR_EL1`, and calls
`mmu::enable_secondary`, which loads the same `MAIR_EL1`/`TCR_EL1`/TTBR0 and
turns on the MMU and caches before any Rust shared state is touched.

Each core computes `sum(value ^ cpu)` over 300,000 iterations and publishes it
with atomics; the boot CPU waits on `ONLINE`/`DONE`, re-computes the expected
sum, and prints the results. The secondaries then park in `wfe`.

```text
Phase 20: symmetric multiprocessing (PSCI)
PSCI        : 1.1 (HVC conduit)
CPU1        : CPU_ON accepted (MPIDR target 1)
CPU2        : CPU_ON accepted (MPIDR target 2)
CPU3        : CPU_ON accepted (MPIDR target 3)
CPU1        : MPIDR 0x80000001, checksum 0xa7a333810 (ok)
CPU2        : MPIDR 0x80000002, checksum 0xa7a333810 (ok)
CPU3        : MPIDR 0x80000003, checksum 0xa7a333810 (ok)
SMP         : 4 CPU(s) online, 3/3 checksums ok (300000 iterations each)
```

This is bring-up, not SMP scheduling: the GIC redistributor setup, timer and EL0
scheduler still run only on CPU 0, and the secondaries stay parked.

### Phase 21 - KimiFS free-space reuse

KimiFS still stores each file as one contiguous run, but allocation is no
longer a bump pointer. `mount` rebuilds an in-RAM bitmap from the inode table
(reserving the superblock/inode area), and `write` grows a file with a
first-fit search over that bitmap. `unlink` and `create`-truncation clear the
old run, so space returns to the allocator. The superblock's `next_free` field
is kept as a high-water mark for information only.

A new `rm` shell command removes `/disk/...` files, and `df` reports free data
blocks in addition to the sector count and file count:

```text
$ df
disk sectors: 8192, files: 3
free data blocks: 8171
$ rm /disk/fstest.bin
removed /disk/fstest.bin
$ df
disk sectors: 8192, files: 2
free data blocks: 8173
$ write /disk/reuse hello-block
$ cat /disk/reuse
hello-block
$ df
free data blocks: 8172
```

The 800-byte `fstest.bin` occupied two blocks; removing it returned both, and
creating a one-block file consumed one. Reboots preserve the namespace and the
bitmap is reconstructed from the inodes, so the free count matches after a
restart. Disks larger than 65,536 sectors (32 MiB) are not mounted because the
bitmap is a fixed 8 KiB static.

### Phase 22 - sleeping processes and uptime

`uptime_ms` divides `CNTPCT_EL0` by `CNTFRQ_EL0`, so it is independent of the
timer's current interval (which changes during boot from 500 ms to 50 ms).
`sleep(ms)` stores an absolute deadline, marks the process `Blocked`, rewinds
its `SVC`, and switches. Every timer IRQ calls `wake_sleepers(now)` before
preemption, so an expired sleeper can be picked on the same tick. When the
retried syscall runs, `sleep_done` makes it return 0 instead of sleeping again.

If no other process is runnable the kernel keeps the sleeping slot and idles
with IRQs enabled around `wfi` until the deadline, then resumes in the kernel
and returns 0. A sleeper is also excluded from the child-exit wake path: only a
parent blocked in `wait` should be woken by an exiting child.

```text
$ time
uptime ms: 19889
$ sleep 1000
slept 1020 ms
$ spawn /spin
spawned pid 2
$ sleep 600
22222222222222222
slept 602 ms
```

`slept` is measured by the shell itself, so it includes the timer tick and
scheduling latency (one 50 ms quantum). The interleaved `2`s show the child
running while the shell slept. `time` and `sleep <ms>` were added to the shell
help.

### Phase 23 - extending the WinAPI translation layer

The PE thunk page now holds nine 16-byte `mov x8, #id; svc #0; ret; nop`
entries. The import names map to:

| Import | SVC | Behaviour |
| --- | --- | --- |
| `GetStdHandle` | 100 | returns console handle 1 |
| `WriteFile` | 101 | writes the buffer to the console |
| `ExitProcess` | 102 | exits only the PE process |
| `GetLastError` / `SetLastError` | 103 / 104 | per-process error slot |
| `Sleep` | 105 | sleeps like `sleep(2)` (may switch processes) |
| `GetTickCount64` / `GetTickCount` | 106 / 107 | ms since boot, 64/32-bit |
| `WriteConsoleA` | 108 | alias of console write, sets `written` |

`build.rs` generates the test image entirely by hand: it now calls
`GetStdHandle`, `WriteFile`, `Sleep(50)`, `WriteConsoleA`, `GetTickCount64`,
`Sleep(100)`, `WriteConsoleA`, `GetTickCount` and `ExitProcess`. The larger
IAT/ILT forced the section offsets to grow (ILT at `0x330`, IAT at `0x380`,
import descriptor at `0x3d0`) but the image still fits in the 1 KiB section.

```text
$ run /hello.exe
  import kernel32.dll!Sleep
  import kernel32.dll!GetTickCount64
  import kernel32.dll!GetTickCount
  import kernel32.dll!WriteConsoleA
PE loaded  : base=0x62000000 entry=0x62001000 pages=3 imports=9
spawned pid 2
Hello from an ARM64 PE on KimiOS!
PE called WriteConsoleA and Sleep(50).
PE is still alive after sleeping.
[kernel] PE ExitProcess(0)
pid 2 exited with status 0
```

The two messages after the first sleep prove `Sleep` returns to the PE image
alive, and `GetTickCount*` complete without an unknown-syscall fault.

### Phase 24 - growing a user heap

Each process starts with `brk = HEAP_START = 0x6080_0000`; `exec` resets it.
`brk(0)` returns the current break, and a larger request maps fresh zeroed
pages from `0x6080_0000` up to `HEAP_LIMIT = 0x60C0_0000`:

```rust
let start = k::brk();                 // 0x6080_0000
let end = k::sbrk(start + 4 * 4096);  // break moves by exactly four pages
// write and read back every byte...
```

Two guards keep the toy allocator honest. Requests round up to whole pages,
and the syscall refuses to start unless `spare_frames()` covers the pages plus
one possible L3 table, so a nearly full address space returns the old break
instead of panicking in `map_page`. `MAX_FRAMES` rose from 64 to 256 per
address space to leave room for a real heap. Shrinking is deliberately not
supported; there is no way to unmap yet, so `brk(smaller)` leaves the break
alone rather than leaking pages invisibly.

```text
$ run /heaptest
heaptest: start 1619001344
heaptest: end 1619017728
heaptest: wrote and verified 16384 bytes
heaptest: PASS
```

`start` is `0x60800000`, and the 16 KiB range spans four pages that were not
mapped when the ELF loaded. The test also asserts that a one-byte over-page
request rounds up by a full page, that the last byte of that page is writable,
and that `sbrk(start)` does not move the break.

### Phase 25 - inspecting and killing processes

`ps` returns a flat array of four `u64`s per live slot: PID, parent PID, state
(`0 free, 1 ready, 2 running, 3 blocked, 4 zombie`) and exit status. Zombie
slots stay in the table until their parent reaps them, so a finished child is
visible before `wait`:

```text
$ spawn /sleeper
sleeper: pid 2 sleeping
$ ps
PID PPID STATE EXIT
1 0 run   0
2 1 block 0
$ kill 2
killed pid 2
$ ps
PID PPID STATE EXIT
1 0 run   0
2 1 zombi 137
$ wait 2
pid 2 exited with status 137
$ ps
PID PPID STATE EXIT
1 0 run   0
```

`kill` refuses PID 0, itself, missing processes and existing zombies. It
reuses the exit path's bookkeeping: the console-reader slot is cleared,
children are reparented to init, and a parent blocked on exactly that child
receives the status through its saved `x0` before the target slot is reaped.
Because only the current process executes on this uniprocessor kernel, there
is no need to stop or switch away from the target.

### Phase 26 - renaming disk files

`rename` takes the two `/disk/...` paths in registers (`x0/x1` = old pointer
and length, `x2/x3` = new pointer and length) and rewrites the 32-byte name
field of the existing inode. No data blocks move:

```text
$ write disk/a.txt hello
$ mv disk/a.txt disk/b.txt
renamed /disk/a.txt -> /disk/b.txt
$ cat disk/b.txt
hello
$ write disk/c.txt x
$ mv disk/b.txt disk/c.txt
mv: failed (disk files only, new name must be free)
```

The inode is flushed before returning, so the new name survives a reboot. A
rename onto itself is rejected as a collision, and names still cannot contain
`/`; directories remain future work.

### Phase 27 - wall-clock time

QEMU `virt` includes a PL031 RTC in the device-mapped first GiB. Reading its
data register yields whole seconds since the Unix epoch; QEMU defaults to
`-rtc base=utc,clock=host`, so it tracks the host clock in UTC:

```text
$ date
2026-11-14 06:54:07 UTC (1794639247)
```

`rtc(out)` returns `[epoch, year, month, day, hour, minute, second]`. The
kernel does the calendar conversion with Howard Hinnant's branch-light
`civil_from_days`, so user programs can format dates without carrying the
algorithm themselves. A zero register reads as "no RTC" and returns an error.

### Phase 29 - a heap for user programs

`libkimi::Heap` plugs into Rust's `alloc` crate, so `Vec`, `Box` and `String`
work in user binaries:

```rust
#[global_allocator]
static HEAP: libkimi::Heap = libkimi::Heap;
```

The allocator keeps a singly linked free list of 16-byte blocks and carves new
memory from a bump pointer that calls `sbrk` one page at a time. Freed blocks
are reused first-fit and split when the remainder can hold another block.
There is no coalescing and no realloc-specific path: `GlobalAlloc`'s default
`realloc` allocates, copies and frees. Alignment above 16 bytes bypasses the
free list and is served directly from the bump pointer.

```text
$ run /alloctest
alloctest: checksum 21216
alloctest: large vector sum 12497500, brk 1619267584
alloctest: PASS
```

The checksum proves 64 rounds of 200-element vectors (`Vec` growth and drops)
reused memory correctly; the second phase pushes 5000 `u64`s, forcing the
break past sixty pages. No lock is needed: the allocator state is private to
each process address space and the kernel never preempts inside its
bookkeeping.

### Phase 30 - a virtio keyboard

`-device virtio-keyboard-device` attaches an input device (virtio id 18) to a
virtio-mmio slot. The driver configures both queues (event and status), posts
eight 8-byte event buffers with `VIRTQ_DESC_F_WRITE`, and enables the slot's
SPI. Each EV_KEY press is translated from its Linux keycode to ASCII, with
shift and its symbol/uppercase variants, then pushed through `uart::inject`;
the shell cannot tell keyboard bytes from serial bytes.

```text
$ datx<backspace>e
2026-11-14 07:36:04 UTC (1794641764)
$ A
unknown command: A
$ ps
PID PPID STATE EXIT
1 0 run   0
```

Two transport bugs surfaced while verifying:

- virtio status bits are `DRIVER_OK = 4` and `FEATURES_OK = 8`. The block and
  input drivers had them swapped; legacy QEMU did not enforce `DRIVER_OK`, so
  disk I/O worked while the input device never started its queues. Both are
  fixed, and modern virtio (`force-legacy=false`) now negotiates correctly.
- The DTB lists virtio-mmio interrupts as SPI numbers (`16 + slot`), so the
  GIC INTID is `48 + slot`. Using `16 + slot` enabled the wrong SPI and left
  keyboard events to be picked up only by polling; the correct INTID (78 for
  slot 30) delivers the level interrupt as expected.

QMP drives the test without a GUI:

```python
{"execute":"input-send-event","arguments":{"events":[
  {"type":"key","data":{"down":True,"key":{"type":"qcode","data":"d"}}}]}}
```

### Phase 28 - file metadata

`stat(path, out)` fills `[kind, size, blocks]`: kind 0 is a RamFS entry (no
blocks), kind 1 is a KimiFS inode. The shell uses it for both a `stat`
command and the new size column in `ls`:

```text
$ ls
/init                       84176
/hello                      70824
/hello.exe                  1536
/motd                       45
/disk/b.txt                 6
$ stat disk/b.txt
kind: disk, size: 6 bytes
$ stat motd
kind: ramfs, size: 45 bytes
```

The disk side re-reads the inode under the filesystem lock, so the size is
the on-disk value rather than a cached copy; block counts are available to
callers even though `stat` does not print them.

## ARM64 details worth knowing

- QEMU enters `_start` at **EL1** with `x0` = DTB address. With a raw ELF and
  `-kernel`, some QEMU versions pass `0` here; the DTB can still be supplied
  explicitly with `-dtb`.
- `CPACR_EL1.FPEN` is 0 after reset, so compiler-generated NEON instructions
  trap until `_start` sets bits 20-21.
- `VBAR_EL1` needs a 2 KiB-aligned table; each of the 16 entries is `0x80` bytes.
  KimiOS runs with `SP_EL1`, so the "Current EL with SPx" synchronous entry at
  offset `0x200` is the one used.
- A raw ELF `-kernel` does not use the Linux boot protocol, so QEMU leaves
  `x0 = 0`. KimiOS therefore validates `x0` and scans RAM for the DTB magic;
  passing `-dtb virt.dtb` guarantees a blob is present. Regenerate it after
  changing `-m`/`-smp`:
  `qemu-system-aarch64 -machine virt,dumpdtb=virt.dtb -m 128M -nographic`
  (the dump is padded to 1 MiB; only the first 8 KiB are needed).
- The PFA reserves every frame below `__kernel_end`, covering the DTB, boot
  area, kernel image, bump heap, boot stack and the PFA bitmap itself.
- QEMU virt's generic timer runs at 62.5 MHz (`CNTFRQ_EL0`); the timer PPI is
  INTID 30. GICv3 needs `-machine virt,gic-version=3`.
- `ICC_IAR1_EL1` acknowledges a Group 1 interrupt, `ICC_EOIR1_EL1` signals
  end-of-interrupt; the timer must be re-armed before EOI because PPI 30 is
  level-sensitive.
- EL0 isolation uses 4 KiB L3 pages: the ELF user region at `0x6000_0000` has
  AP=EL0/EL1 RW, code is EL0-executable (UXN=0, PXN=1), the stack is
  UXN=1. Kernel RAM stays EL1-only, and the UART/device MMIO is EL1-only.
- SVC syscall ABI: `x8` = number (1 write, 2 exit, 3 read, 4 open,
  5 read_file, 6 close, 7 exec, 8 list, 9 spawn, 10 wait, 11 getpid,
  12 stats, 13 create, 14 write_file, 15 diskinfo, 16 fb_info,
  17 fb_fill, 18 unlink, 19 freeblocks, 20 uptime, 21 sleep, 22 brk,
  23 ps, 24 kill, 25 rename, 26 rtc, 27 stat), `x0..x4` = args,
  result in `x0`.
  For SVC, `ELR_EL1` already points to the next instruction; BRK is the
  exception that must advance ELR manually.
- `exec` reuses the ELF loader, so it remaps and zeroes the user stack page.
  The kernel copies the path out of user memory first, because the user
  path buffer usually lives on that stack.
- Process switching is synchronous: `spawn`/`wait`/`exit` save the current
  `ExceptionFrame`, set `TTBR0_EL1` to the next process's table, `TLBI
  VMALLE1`, and return the next process's frame to the exception return path.
  EL0 processes are preempted by the timer; kernel syscalls still run with
  interrupts masked, so syscalls themselves are not preempted. The process
  model is deliberately `spawn`-based rather than `fork`-based.
- virtio-mmio legacy publishes a queue by page frame number and requires the
  descriptor table and available ring to be contiguous; using a 256-entry
  queue makes both rings land on separate 4 KiB pages, which also works for
  the modern transport. The modern ring registers are `0x080` (descriptor),
  `0x090` (available) and `0x0a0` (used); the `0x088`/`0x090` pair is a
  reserved gap, not the avail/used pair.
- The request queue lives in a 16 KiB page-aligned static region. Requests
  are synchronous: publish the chain, `DSB`, notify queue 0, then poll
  `used.idx`; no virtio interrupts are used.
- An IRQ from EL0 uses the "Lower EL, AArch64" vector at offset `0x480`, but
  it runs on the same kernel `SP_EL1` stack and saves the same frame layout as
  an EL1 exception. `SPSR_EL1 & 0xf == 0` identifies the interrupted context
  as EL0, which is how `irq_handler` decides between user preemption and the
  old kernel-thread scheduler.
- The user preemption quantum is `CNTFRQ_EL0 / 20` (50 ms here). The timer
  must be re-armed before `ICC_EOIR1_EL1` because PPI 30 is level-sensitive.
- PE images use the Windows ARM64 calling convention (`x0-x7` arguments,
  result in `x0`). Each imported function is pointed at a 16-byte EL0 thunk
  (`mov x8, #id; svc #0; ret; nop`), so no user-visible DLL is needed.
  PE syscalls are 100 `GetStdHandle`, 101 `WriteFile`, 102 `ExitProcess`,
  103 `GetLastError`, 104 `SetLastError`, 105 `Sleep`, 106 `GetTickCount64`,
  107 `GetTickCount`, 108 `WriteConsoleA`. Thunks are 16 bytes each at
  `0x6300_0000`, indexed by `id - 100`, so ids must stay contiguous.
- The PE image is fixed at `0x6200_0000`, thunks at `0x6300_0000`, and the
  PE stack at `0x6400_0000`; all are inside the identity-mapped 1-2 GiB
  region, where L2 entries can be replaced with L3 user tables. The L3 pool
  holds eight 2 MiB blocks (ELF shell, helper and PE process).
- The user ELF is linked at `0x6000_0000`; its stack page is mapped at
  `0x6100_0000`. `user/linker.ld` page-aligns each section so no two
  `PT_LOAD` segments share a page. The loader parses an aligned copy of the
  embedded bytes because `xmas-elf` requires natural alignment.
- `build.rs` invokes cargo recursively for `user/` with a separate
  `target/user-build` directory and explicit `RUSTFLAGS`; this keeps the
  kernel and user program builds independent.
- `MAIR_EL1` Attr0 = Normal write-back, Attr1 = Device-nGnRnE; the low 1 GiB
  (UART, GIC, ...) is mapped Device, the next 1 GiB (RAM) is mapped Normal.
- fw_cfg lives at `0x0902_0000` (`qemu,fw-cfg-mmio`) with big-endian selector
  and DMA registers. `write_file` stores every descriptor field big-endian and
  passes the guest physical address of `etc/ramfb`; QEMU performs the transfer
  during the register write. ramfb accepts 640x480 (min 16, max 2560x1600) and
  requires `stride == width * 4` for XRGB8888.
- font8x8 rows are bitmaps with bit 0 as the leftmost pixel. The RGB channels
  are `0x00RRGGBB`; XRGB8888 stores bytes as B, G, R, X in little-endian
  memory, which QEMU maps to the pixman `x8r8g8b8` format.
- QEMU's PSCI conduit is advertised in `/psci/method`; this DTB says `hvc`.
  Use `hvc #0`, not `smc #0`, or the CPU takes an unknown-reason exception.
  `PSCI_CPU_ON` is `0xC400_0003` with target/entry/context in `x1..x3`.
- Secondary cores start with MMU and caches off, so the trampoline runs in
  physical (identity) addresses until `mmu::enable_secondary` executes. Each
  core must also set `CPACR_EL1.FPEN` and `VBAR_EL1` for itself.
- `sleep` reuses the read-blocking pattern: rewind `ELR_EL1` by 4 and mark the
  process `Blocked`. A per-process `sleep_done` flag lets the retried SVC
  return immediately. The child-exit path must not wake sleepers, so it checks
  `sleep_at == 0` in addition to `read_wait`.
- `CNTPCT_EL0` is a free-running 64-bit counter; `CNTFRQ_EL0` gives its rate
  (62.5 MHz on QEMU virt). Compute milliseconds with a 128-bit product to
  avoid overflow over long uptimes.
- `rename` resolves both names while holding the filesystem lock: checking
  the destination and updating the source inode in one critical section
  prevents two racing renames from claiming the same name.
- `kill` never frees a target that the parent has not waited for: the slot
  becomes a zombie, exactly like a normal exit. This keeps `wait(pid)`
  meaningful and avoids resurrecting a PID before its status is collected.
- `brk` maps pages eagerly and only advances the break at the end, so a
  caller that ignores a short return can never see a half-grown heap. The
  heap window sits in L2 index 4, clear of the ELF image (index 0), the stack
  (index 8) and the PE regions (index 16+).
- `fb_info`/`fb_fill` validate the info pointer against the coarse user
  window `0x6000_0000..0x6500_0000` (ELF, stack, PE image/thunks/stack) and
  require 8-byte alignment. Rectangle arguments use saturating arithmetic so
  huge or off-screen values clip instead of wrapping.
