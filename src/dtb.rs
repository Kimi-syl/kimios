//! Device tree blob discovery and `/memory` parsing.
//!
//! QEMU's `virt` machine builds a DTB for the guest. For Linux-style boot
//! protocols the address is passed in `x0`, but a raw ELF `-kernel` image does
//! not always receive it, so KimiOS validates `x0` and otherwise scans RAM for
//! the `0xd00dfeed` magic.

use hermit_dtb::Dtb;

use crate::println;

pub const DTB_MAGIC: u32 = 0xd00d_feed;
const DTB_VERSION: u32 = 17;
const MAX_DTB_SIZE: u64 = 4 * 1024 * 1024;

/// Base physical address of QEMU's `virt` RAM.
const RAM_BASE: u64 = 0x4000_0000;
/// Conservative scan window: first 16 MiB of RAM.
const SCAN_END: u64 = RAM_BASE + 16 * 1024 * 1024;
/// Assumed RAM size if no DTB is available (`-m 128M`).
const FALLBACK_SIZE: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct MemoryRegion {
    pub base: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct BootInfo {
    pub dtb_addr: u64,
    pub dtb_size: usize,
    pub from_x0: bool,
    pub memory: MemoryRegion,
    pub memory_from_dtb: bool,
    pub fb_resolution: Option<(u32, u32)>,
}

fn read_be32(address: u64) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32).swap_bytes() }
}

fn slice_be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Validate a DTB header at `address` and return its total size.
fn dtb_size_at(address: u64) -> Option<usize> {
    if address < RAM_BASE || address % 4 != 0 {
        return None;
    }

    let magic = read_be32(address);
    if magic != DTB_MAGIC {
        return None;
    }

    let totalsize = read_be32(address + 4) as u64;
    let version = read_be32(address + 20);

    if version != DTB_VERSION || totalsize < 0x30 || totalsize > MAX_DTB_SIZE {
        return None;
    }

    Some(totalsize as usize)
}

fn read_cells(property: Option<&[u8]>) -> Option<usize> {
    let property = property?;
    if property.len() < 4 {
        return None;
    }
    match slice_be32(property, 0) {
        1 => Some(1),
        2 => Some(2),
        _ => None,
    }
}

/// Parse `/memory`'s `reg` property. Each entry is `#address-cells` big-endian
/// cells of base followed by `#size-cells` cells of size.
fn parse_memory(dtb: &Dtb) -> Option<MemoryRegion> {
    let address_cells = read_cells(dtb.get_property("/", "#address-cells")).unwrap_or(2);
    let size_cells = read_cells(dtb.get_property("/", "#size-cells")).unwrap_or(2);

    let reg = dtb.get_property("/memory", "reg")?;
    let entry_bytes = (address_cells + size_cells) * 4;

    let mut best: Option<MemoryRegion> = None;
    let mut offset = 0;

    while offset + entry_bytes <= reg.len() {
        let mut base = 0u64;
        for cell in 0..address_cells {
            base = (base << 32) | slice_be32(reg, offset + cell * 4) as u64;
        }

        let mut size = 0u64;
        let size_offset = offset + address_cells * 4;
        for cell in 0..size_cells {
            size = (size << 32) | slice_be32(reg, size_offset + cell * 4) as u64;
        }

        if size != 0 && best.map_or(true, |current| size > current.size) {
            best = Some(MemoryRegion { base, size });
        }

        offset += entry_bytes;
    }

    best
}

/// Parse `kimios.fb=WIDTHxHEIGHT` from `/chosen/bootargs`.
fn parse_fb_resolution(dtb: &Dtb) -> Option<(u32, u32)> {
    let args = dtb.get_property("/chosen", "bootargs")?;
    let text = core::str::from_utf8(args).ok()?.trim_matches('\0');
    for token in text.split_whitespace() {
        if let Some(value) = token.strip_prefix("kimios.fb=") {
            let (w, h) = value.split_once('x')?;
            let w: u32 = w.parse().ok()?;
            let h: u32 = h.parse().ok()?;
            if w >= 16 && h >= 16 {
                return Some((w, h));
            }
        }
    }
    None
}

fn try_parse(address: u64, from_x0: bool) -> Option<BootInfo> {
    let size = dtb_size_at(address)?;

    // SAFETY: the header was validated and the blob lies in identity-mapped RAM.
    let dtb = unsafe { Dtb::from_raw(address as *const u8)? };
    let memory = parse_memory(&dtb)?;
    let fb_resolution = parse_fb_resolution(&dtb);

    Some(BootInfo {
        dtb_addr: address,
        dtb_size: size,
        from_x0,
        memory,
        memory_from_dtb: true,
        fb_resolution,
    })
}

/// Locate and parse the DTB, or fall back to a hard-coded 128 MiB region.
pub fn probe(x0: usize) -> BootInfo {
    if let Some(info) = try_parse(x0 as u64, true) {
        return info;
    }

    // Known QEMU placements: base of RAM and near the top of 128 MiB RAM.
    for candidate in [RAM_BASE, RAM_BASE + 0x07f0_0000, RAM_BASE + 0x07ff_0000] {
        if let Some(info) = try_parse(candidate, false) {
            return info;
        }
    }

    let mut address = RAM_BASE;
    while address < SCAN_END {
        if let Some(info) = try_parse(address, false) {
            return info;
        }
        address += 4;
    }

    println!("WARNING: no DTB found; falling back to 128 MiB at {:#x}", RAM_BASE);

    BootInfo {
        dtb_addr: 0,
        dtb_size: 0,
        from_x0: false,
        memory: MemoryRegion {
            base: RAM_BASE,
            size: FALLBACK_SIZE,
        },
        memory_from_dtb: false,
        fb_resolution: None,
    }
}
