# KimiOS graphics architecture — hardware-independent display

The GUI/compositor is decoupled from any QEMU-specific graphics API. It renders
to a back buffer and publishes through a small syscall interface; the kernel
selects a display backend underneath. ramfb is the boot/recovery fallback; a
VirtIO-GPU 2D backend is implemented as the hardware-independent primary path.

## 1. The abstraction (GUI is backend-agnostic)

The GUI never touches QEMU devices. It uses three syscalls:

| syscall | meaning |
|---|---|
| `fb_info` | width / height / stride / format |
| `fb_back` | map the offscreen back buffer (render target) |
| `fb_present(x0,y0,x1,y1)` | publish a dirty rectangle to the display |

`src/fb.rs` dispatches these to the active backend:

```
GUI (kimi-gui, software renderer)
  -> renders to BACK buffer
  -> fb_present(dirty rect)
       |-- ramfb backend:   memcpy BACK -> FRAMEBUFFER (QEMU scans RAM)
       |-- virtio-gpu 2D:   TRANSFER_TO_HOST_2D + RESOURCE_FLUSH (virtqueue)
```

Backend selection (`kimios.gpu=` boot arg): `auto` (default: virtio-gpu if
present and working, else ramfb), `ramfb` (force fallback), `virtio` (force
virtio-gpu). The console also routes through the active backend, so boot text
and the shell render identically on either.

This means the same GUI binary runs on ramfb today and on VirtIO-GPU (or a
future accelerated backend) without modification — no QEMU-specific code in
`kimi-gui`.

## 2. ramfb backend (stable, fallback)

Zero-copy guest→QEMU: the GUI publishes into a RAM framebuffer that QEMU's ramfb
device scans directly. Simple and robust; used for boot/recovery and as the
default when VirtIO-GPU is unavailable.

## 3. VirtIO-GPU 2D backend (working — verified on QEMU 11.1.2 and 10.2.1)

`src/virtio_gpu.rs` implements the 2D (non-VirGL) protocol over virtio-mmio:
device init + feature negotiation (2D only), control + cursor virtqueues, and
`RESOURCE_CREATE_2D`, `RESOURCE_ATTACH_BACKING`, `SET_SCANOUT`,
`TRANSFER_TO_HOST_2D`, `RESOURCE_FLUSH`, `GET_DISPLAY_INFO`. `present(rect)`
issues a transfer of the dirty rect from the backing into the host resource and
flushes it to scanout; QEMU then shows the scanout via its display backend.

**Status: FIXED and verified.** The GUI renders through virtio-gpu 2D with
`-display none` + VNC on both the host QEMU 11.1.2 and the bundled Termux QEMU
10.2.1 (serial: `virtio-gpu  : 2D scanout 1080x1920 B8G8R8X8`, QMP screendump of
the GPU console shows the full GUI, ~1000 distinct colors, zero guest errors).
ramfb remains the automatic fallback when the device is absent or init fails.

### Root cause of the former ATTACH_BACKING failure (confirmed by trace)

A **guest driver opcode bug**, not a QEMU/display-backend problem: the driver
sent `0x0107` for attach, but per the virtio spec `0x0106` is
`RESOURCE_ATTACH_BACKING` and `0x0107` is `RESOURCE_DETACH_BACKING`. QEMU
dispatched the request to `virtio_gpu_resource_detach_backing()`, whose
`virtio_gpu_find_check_resource(require_backing=true)` check fails on a resource
that has never been attached:

```
trace (buggy driver, -trace enable=virtio_gpu_* -d guest_errors):
  virtio_gpu_cmd_res_create_2d res 0x1, fmt 0x2, w 1080, h 1920
  virtio_gpu_cmd_res_back_detach res 0x1                      <- 0x0107 = DETACH
  virtio_gpu_resource_detach_backing: no backing storage 1    <- LOG_GUEST_ERROR
  -> response VIRTIO_GPU_RESP_ERR_UNSPEC (0x1200)
```

This explains every reported symptom: `ERR_UNSPEC` for *all* backing-list
shapes (the payload never mattered — the command type was wrong), `CREATE_2D` /
`SET_SCANOUT` succeeding, and transfers reporting "no backing storage". The
guest error *was* emitted by QEMU, but from the **detach** function (easy to
miss when looking for attach-path errors). Fix: one constant,
`CMD_RESOURCE_ATTACH_BACKING = 0x0106` (src/virtio_gpu.rs).

Secondary latent bug fixed alongside: the request buffer was 40000 bytes, too
small for the maximum resolution's page list (1440x2560 needs 3600 entries =
57632 bytes). Now 65536; verified by booting `kimios.fb=1440x2560`.

Disproved hypotheses: the refactored attach path does **not** need a non-`none`
display backend (works with `-display none`), and udmabuf is irrelevant for 2D
resources — QEMU only calls `virtio_gpu_init_udmabuf()` when `res->image` is
NULL, which never happens for `CREATE_2D` resources.

### Experiment matrix (2026-11-17, host QEMU 11.1.2 unless noted)

All runs: `-machine virt,gic-version=3 -cpu cortex-a53 -m 128M`, ramfb present
as fallback, VNC `127.0.0.1:3`, `-trace enable=virtio_gpu_* -d guest_errors,unimp`.

| config | result |
|---|---|
| buggy driver (0x0107), `-display none` | FAIL: `res_back_detach` + "no backing storage 1" → ERR_UNSPEC; ramfb fallback |
| fixed driver, `-display none` | **OK**: attach/scanout/transfer+flush, GUI pixels verified |
| `-display egl-headless` | QEMU refuses: "no drm render node available" (proot, no GPU) |
| `-display sdl` | QEMU refuses: "No available video device" (no X/Wayland in proot) |
| `-display gtk` | QEMU refuses: "gtk initialization failed" |
| `-device virtio-gpu-pci` | guest driver is mmio-only → device not found → ramfb fallback (expected) |
| `,blob=true` | QEMU refuses: `/dev/udmabuf` missing ("need rutabaga or udmabuf") |
| `,hostmem=256` | OK, GUI renders |
| `,venus=on` | property only exists on GL devices; `virtio-gpu-gl-device,venus=on` needs `-display ...,gl=on` → unavailable here |
| `,max_outputs=1` | OK, GUI renders |
| no `kimios.gpu=` (auto) | virtio-gpu auto-selected when present, GUI renders |
| `kimios.gpu=ramfb` | ramfb baseline renders the same GUI (pixel parity) |
| `kimios.fb=1440x2560` | OK: 3600-entry backing list, GUI renders (REQ_BUF regression test) |
| no virtio device + `kimios.gpu=virtio` | clean fallback message + ramfb |
| **Termux QEMU 10.2.1** (`libqemu.so`, `-display none`) | **OK**: virtio backend init + GUI pixels (trace backend compiled out; serial + screendump evidence) |

Verification method (no image viewing): QMP `screendump` **with
`device=<gpu id>`** — the default targets the active console, which under
`-display none` is a 640x480 placeholder, not the GPU scanout — then PPM pixel
statistics (distinct colors / theme palette), plus the serial backend line and
the virtio trace (`res_back_attach` → `set_scanout` → continuous
`res_xfer_toh_2d` + `res_flush`, ~1000 transfers per 12 s session, zero
guest_errors).

Driver observability: `CREATE_2D` / `ATTACH_BACKING` / `SET_SCANOUT` failures now
log the response code to serial (the original bug was silent on the guest side).

**Performance expectation (honest):** for a single software-rendered framebuffer
shown over VNC, VirtIO-GPU 2D is *not* a latency win over ramfb — it adds a
virtqueue round-trip plus a guest→host-resource copy per flush, whereas ramfb is
zero-copy guest→QEMU. The display *transport* (VNC), not the guest pixel-push, is
the bottleneck. VirtIO-GPU's value here is the hardware-independent interface and
a foundation for acceleration, not raw 2D speed.

## 4. Benchmarks (guest-side, QEMU TCG on the test device)

`gfxbench` shell command (syscall `SYS_GFX_BENCH`), ARM CNTPCT timing, 50 iters:

| path | operation | latency | bandwidth |
|---|---|---|---|
| ramfb | full 1080×1920 present (8 MB copy) | **15.66 ms** | ~529 MB/s |
| ramfb | 256×256 dirty-rect present | **0.44 ms** | ~597 MB/s |

Takeaways:
- **Dirty rectangles cut partial-update cost ~35×** (15.66 ms → 0.44 ms). This is
  why scrolling/typing/press-feedback feel snappier than full-frame redraws.
- The **full-frame 8 MB copy (~15.6 ms under TCG) is a real app-launch cost**. On
  bare metal it would be ~1 ms; under emulation it dominates. Reducing full-frame
  copies (incremental app rendering, or a shared-memory transport that avoids the
  copy entirely) is the next lever.
- Memcpy bandwidth under TCG is ~0.5 GB/s; virtio-gpu 2D adds command
  round-trips on top of a comparable copy, hence not faster for this use case
  (guest-side `gfxbench[virtio]` numbers still to be captured; the virtio path
  is verified functionally, not benchmarked).

End-to-end frame latency to the phone panel additionally includes the VNC refresh
(~30 ms), encode/decode, and SurfaceFlinger — see `kimios-android/docs/display-pipeline.md`.

## 5. Optional accelerated backend (design — staged AFTER 2D is stable)

Acceleration is only meaningful once the GUI renders through a GPU API; the
current renderer is pure software, so these are future work, not flags:

- **VirGL (OpenGL):** QEMU `-device virtio-gpu-gl,hostmem=... -display egl-headless`
  + virglrenderer on the host. Guest gets a Gallium OpenGL driver; the GUI would
  need a GL backend (e.g. render via a small GL abstraction instead of CPU
  blits). Host needs EGL/GPU access (non-trivial on Android).
- **Venus (Vulkan):** `-device virtio-gpu-gl` with venus renderer; guest Vulkan
  driver. Higher performance ceiling, larger guest-side effort.
- **rutabaga / gfxstream:** the Android emulator's GPU streaming stack
  (`-device virtio-gpu-rutabaga` / gfxstream renderer). Best fit for an Android
  host (it is designed for it), but requires the gfxstream renderer libraries and
  a guest gfxstream driver; the GUI would target the gfxstream API.

Staging rule: the VirtIO-GPU 2D path is now stable (attach fixed + rendering
verified on both QEMU builds), so accelerated backends are unblocked as future
work. None are usable in the current test environment yet: VirGL/Venus need EGL
+ a DRM render node (`egl-headless` fails in proot), blob resources need
`/dev/udmabuf`, and rutabaga/gfxstream need the gfxstream renderer libraries.
The abstraction in §1 is designed so an accelerated backend slots in behind
`fb_back`/`fb_present` (or a richer resource/submit interface) without changing
the GUI's backend-agnostic contract.

## 6. Recommended latency path (orthogonal to backend)

The biggest end-to-end win is removing the VNC transport: an **ashmem/shared-memory
framebuffer** (QEMU `memory-backend-memfd,share=on`, app mmaps guest RAM and blits
the framebuffer directly). That eliminates encode/decode/socket and the VNC
refresh wait, and works with *either* ramfb or virtio-gpu backing. See
`kimios-android/docs/display-pipeline.md` §3.