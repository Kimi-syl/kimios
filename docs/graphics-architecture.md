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

## 3. VirtIO-GPU 2D backend (implemented; attach blocked on this QEMU)

`src/virtio_gpu.rs` implements the 2D (non-VirGL) protocol over virtio-mmio:
device init + feature negotiation (2D only), control + cursor virtqueues, and
`RESOURCE_CREATE_2D`, `RESOURCE_ATTACH_BACKING`, `SET_SCANOUT`,
`TRANSFER_TO_HOST_2D`, `RESOURCE_FLUSH`, `GET_DISPLAY_INFO`. `present(rect)`
issues a transfer of the dirty rect from the backing into the host resource and
flushes it to scanout; QEMU then shows the scanout via its display backend.

**Status (honest):** device discovery, `CREATE_2D`, `GET_DISPLAY_INFO`,
`SET_SCANOUT` and the transfer/flush command framing all work. On the QEMU
builds tested (host 11.1.2 and bundled 10.2.1) `RESOURCE_ATTACH_BACKING` returns
`VIRTIO_GPU_RESP_ERR_UNSPEC` with no guest-error log, for single-page,
single-large, and page-sized (2025-entry) backing lists alike, at valid RAM
addresses. The resource image is confirmed created (a pre-attach transfer reports
"no backing storage", not "is a blob"). The tested builds use a refactored
virtio-gpu ("no backing storage %d") whose attach path differs from upstream
master; the shareable-image/udmabuf path likely requires a non-`none` display
backend. **Until this is resolved the backend falls back to ramfb automatically**,
so the system always boots and renders. Next debugging step: run against a QEMU
with `-display egl-headless`/`sdl` (or upstream master) and trace
`virtio_gpu_create_mapping_iov`/`init_udmabuf`.

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
- Memcpy bandwidth under TCG is ~0.5 GB/s; virtio-gpu 2D would add command
  round-trips on top of a comparable copy, hence not faster for this use case.

End-to-end frame latency to the phone panel additionally includes the VNC refresh
(~30 ms), encode/decode, and SurfaceFlinger — see `kimios-android/docs/display-pipeline.md`.

## 5. Optional accelerated backend (design — staged AFTER 2D is stable)

Acceleration is only meaningful once the GUI renders through a GPU API; the
current renderer is pure software, so these are future work, not flags:

- **VirGL (OpenGL):** QEMU `-device virtio-gpu-gl,hostmem=... -display egl-headless`
  + virglrenderer on the host. Guest gets a Gallium OpenGL driver; the GUI would
  need a GL backend (e.g. render via a small GL abstraction instead of CPU
  blits). Host needs EGL/GPU access (non-trivial on Android).
- **Venus (Vulkan):** `-device virtio-gpu-gl` withvenus renderer; guest Vulkan
  driver. Higher performance ceiling, larger guest-side effort.
- **rutabaga / gfxstream:** the Android emulator's GPU streaming stack
  (`-device virtio-gpu-rutabaga` / gfxstream renderer). Best fit for an Android
  host (it is designed for it), but requires the gfxstream renderer libraries and
  a guest gfxstream driver; the GUI would target the gfxstream API.

Staging rule (per the task): do **not** start these until the VirtIO-GPU 2D path
is stable (attach_backing resolved + verified rendering). The abstraction in §1
is designed so an accelerated backend slots in behind `fb_back`/`fb_present` (or
a richer resource/submit interface) without changing the GUI's backend-agnostic
contract.

## 6. Recommended latency path (orthogonal to backend)

The biggest end-to-end win is removing the VNC transport: an **ashmem/shared-memory
framebuffer** (QEMU `memory-backend-memfd,share=on`, app mmaps guest RAM and blits
the framebuffer directly). That eliminates encode/decode/socket and the VNC
refresh wait, and works with *either* ramfb or virtio-gpu backing. See
`kimios-android/docs/display-pipeline.md` §3.