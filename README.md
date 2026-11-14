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
  `/hello`, `/hello.exe`, `/fstest` and `/motd` remain embedded in RamFS.

## Layout

```text
.cargo/config.toml   target + linker flags
Cargo.toml           dependencies, panic=abort
build.rs             builds user/, copies init.elf + hello.elf, generates hello.exe
linker.ld            0x4008_0000, .text.boot first, vector table kept, 64 KiB stack
user/                EL0 workspace: libkimi (syscalls), kimi-init (shell), kimi-hello
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
src/diskfs.rs        KimiFS: superblock, inodes, contiguous file data
src/elf.rs           xmas-elf loader -> per-process AddressSpace
src/pe.rs            PE32+ loader + kernel32 thunks
src/process.rs       process table, spawn/exec/wait/exit, FDs, TTBR0 switching
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

The kernel exits QEMU cleanly (`status 0`) through ARM semihosting
(`SYS_EXIT_EXTENDED`, `HLT #0xF000`). This is temporary scaffolding until a
proper shutdown driver exists.

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
  12 stats, 13 create, 14 write_file, 15 diskinfo), `x0..x3` = args, result
  in `x0`.
  For SVC, `ELR_EL1` already points to the next instruction; BRK is the
  exception that must advance ELR manually.
- `exec` reuses the ELF loader, so it remaps and zeroes the user stack page.
  The kernel copies the path out of user memory first, because the user
  path buffer usually lives on that stack.
- Process switching is synchronous: `spawn`/`wait`/`exit` save the current
  `ExceptionFrame`, set `TTBR0_EL1` to the next process's table, `TLBI
  VMALLE1`, and return the next process's frame to the exception return path.
  No timer-driven EL0 preemption yet; the process model is deliberately
  `spawn`-based rather than `fork`-based.
- virtio-mmio legacy publishes a queue by page frame number and requires the
  descriptor table and available ring to be contiguous; using a 256-entry
  queue makes both rings land on separate 4 KiB pages, which also works for
  the modern transport. The modern ring registers are `0x080` (descriptor),
  `0x090` (available) and `0x0a0` (used); the `0x088`/`0x090` pair is a
  reserved gap, not the avail/used pair.
- The request queue lives in a 16 KiB page-aligned static region. Requests
  are synchronous: publish the chain, `DSB`, notify queue 0, then poll
  `used.idx`; no virtio interrupts are used.
- PE images use the Windows ARM64 calling convention (`x0-x7` arguments,
  result in `x0`). Each imported function is pointed at a 16-byte EL0 thunk
  (`mov x8, #id; svc #0; ret; nop`), so no user-visible DLL is needed.
  PE syscalls are 100 `GetStdHandle`, 101 `WriteFile`, 102 `ExitProcess`,
  103 `GetLastError`, 104 `SetLastError`.
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
