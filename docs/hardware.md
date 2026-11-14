# Hardware profile: Redmi Note 11 (spes/spesn)

Status: **research only**. No hardware-specific KimiOS code may be written until
the items marked *(verify on device)* are confirmed from the actual firmware.

## Device

| Item | Value | Source |
|---|---|---|
| Marketing name | Redmi Note 11 (4G) | device report |
| Model | 2201117TY | `getprop ro.product.model` |
| Codename | `spes` / `spesn` (NFC) | `getprop ro.product.device` |
| SoC | Qualcomm **SM6225** "khaje" Snapdragon 680 4G | device-tree README, kernel sources |
| CPU | 4x Cortex-A73 (Gold) + 4x Cortex-A53 (Silver) | SM6225 topology |
| GPU | Adreno 610 | device-tree README |
| RAM | 6 GiB (this unit) | `getprop` / device info |
| Display | 6.43" AMOLED, 1080x2400 | device-tree README |
| Storage | UFS 2.2, 64/128 GB | device-tree README |
| Shipped Android | 11, currently Android 13 / MIUI 14-HyperOS | getprop, firmware archives |

Important nuance: the running system reports `ro.board.platform=bengal` and the
vendor kernel defconfig is `vendor/bengal-perf_defconfig`, but Qualcomm's
SM6225 platform codename is **khaje**. Do not equate this with SM6115/bengal;
verify against the actual DTB and kernel sources.

## Boot / firmware layout (Android 13, A/B)

- A/B slots (`boot_a/boot_b`, `vendor_boot_a/b`, `dtbo_a/b`, `vbmeta_a/b`,
  `vbmeta_system_a/b`), dynamic partitions (`super`).
- `boot.img`: GKI kernel + boot header (v3 or v4 on Android 13).
- `vendor_boot.img`: vendor ramdisk, kernel modules, **DTB** for this device.
- `dtbo.img`: device-tree overlays.
- `vbmeta`: Android Verified Boot (AVB) metadata; currently locked/green.
- Reference flashing flow for custom ROMs:
  `fastboot flash boot boot.img`, `vendor_boot`, `dtbo`, then
  `fastboot --disable-verity --disable-verification flash vbmeta ...`.
- The bootloader is currently **locked** (`ro.boot.flash.locked=1`,
  `ro.boot.verifiedbootstate=green`), and the host has no `/dev/block` or
  fastboot device access from the proot. A custom KimiOS image cannot be
  booted until the bootloader is unlocked and the exact stock images are
  dumped.

## Device sources to use (do not guess)

- Device tree: <https://github.com/Jabiyeff/android_device_xiaomi_spes>
- Vendor tree: <https://github.com/Jabiyeff/android_vendor_xiaomi_spes>
- Kernel tree: <https://github.com/Jabiyeff/kernel_xiaomi_sm6225>
- SM6225 device trees: `android_kernel_xiaomi_sm6225-devicetrees`
  (`qcom/khaje-idp*`, `qcom/bengal-*` inherited fragments)
- Reference XDA platform notes:
  <https://xdaforums.com/t/info-for-redmi-note-11-and-kernel-source.4486739>
- AOSP boot image header:
  <https://source.android.com/docs/core/architecture/bootloader/boot-image-header>
- AOSP generic boot / vendor_boot partitions:
  <https://source.android.com/docs/core/architecture/partitions/generic-boot>

## Facts still to verify from this unit's firmware

1. Exact boot header versions and field values in stock `boot.img`,
   `vendor_boot.img`, `dtbo.img` (dump images from the matching fastboot ROM).
2. DTB size/offset inside `vendor_boot` and whether DTBO overlays are required
   for the panel/touch/regulators.
3. Full GPT partition map and active slot.
4. UART/early-debug path (whether a debug UART is exposed; ABL framebuffer
   address for early console). No serial console was reported by other
   spes-family bring-up work; the bootloader's framebuffer may be available.
5. GIC, timer frequency, UFS, USB, PMIC and panel addresses from the actual
   khaje DTS — not from generic QEMU virt.

Until 1-5 are confirmed, hardware milestones stay blocked; QEMU `virt` remains
the executable development target.
