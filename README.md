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

## Layout

```text
.cargo/config.toml   target + linker flags
Cargo.toml           spin dependency, panic=abort
linker.ld            0x4008_0000, .text.boot first, vector table kept, 64 KiB stack
src/boot.s           _start: EL1 entry, FP/SIMD enable, stack, .bss, kmain
src/vectors.s        16-entry exception vector table + save/restore
src/main.rs          kmain, panic handler, semihosting exit
src/uart.rs          PL011 UART + print macros
src/exceptions.rs    exception frame and Rust handler
src/mmu.rs           page tables, MAIR/TCR/TTBR/SCTLR setup
src/allocator.rs     bump allocator
src/dtb.rs           DTB discovery + /memory parsing
src/pfa.rs           bitmap physical frame allocator
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

qemu-system-aarch64 \
  -machine virt \
  -cpu cortex-a53 \
  -m 128M \
  -nographic \
  -semihosting-config enable=on,target=native \
  -dtb virt.dtb \
  -kernel target/aarch64-unknown-none/debug/kimios
```

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

Phases 1-5 complete. Halting.
```

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
- `MAIR_EL1` Attr0 = Normal write-back, Attr1 = Device-nGnRnE; the low 1 GiB
  (UART, GIC, ...) is mapped Device, the next 1 GiB (RAM) is mapped Normal.
