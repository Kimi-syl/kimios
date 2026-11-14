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
| 6 | Bring up secondary CPUs | next | - |
| 7 | Basic display output | planned (ramfb) | needs panel/DSI |
| 8 | Touch | planned (virtio-input) | needs touch controller |
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

Preemption of EL0 processes is not yet enabled; user processes yield through
`spawn`/`wait`/`exit`. Enabling EL0 IRQs and switching processes from the
timer path is a later step.

## Next

1. EL0 timer preemption and a scheduler for runnable user processes.
2. Display output: ramfb on QEMU, panel/DSI investigation on spes.
3. Block storage beyond QEMU: UFS and the spes partition table (blocked on
   bootloader unlock for bare-metal testing).
4. More filesystem surface: unlink, directories, reuse of freed runs.
