//! Minimal QEMU fw_cfg driver (MMIO data/selector ports plus DMA).
//!
//! QEMU's `virt` machine exposes fw_cfg at `0x0902_0000` as an
//! `qemu,fw-cfg-mmio` device:
//!
//! ```text
//!   0x00  data     (8 bytes, big-endian device)
//!   0x08  selector (2 bytes, big-endian device)
//!   0x10  DMA      (8 bytes, big-endian device)
//! ```
//!
//! Reads go through the data port. QEMU removed data-port **writes** in
//! v2.4, so writable files (such as `etc/ramfb`) must be programmed with the
//! DMA interface. All descriptor fields are big-endian in guest memory.

use core::ptr::{addr_of, read_volatile, write_volatile};

const FW_CFG_BASE: usize = 0x0902_0000;
const REG_DATA: usize = 0x00;
const REG_SELECTOR: usize = 0x08;
const REG_DMA: usize = 0x10;

/// `FW_CFG_FILE_DIR`: the fw_cfg file directory, including `etc/ramfb`.
const FILE_DIRECTORY: u16 = 0x19;

const DMA_ERROR: u32 = 1 << 0;
const DMA_SELECT: u32 = 1 << 3;
const DMA_WRITE: u32 = 1 << 4;

#[repr(C, align(16))]
struct DmaAccess {
    control: u32,
    length: u32,
    address: u64,
}

static mut DMA: DmaAccess = DmaAccess {
    control: 0,
    length: 0,
    address: 0,
};

fn select(key: u16) {
    // The selector register is a big-endian 16-bit device.
    unsafe {
        write_volatile((FW_CFG_BASE + REG_SELECTOR) as *mut u16, key.to_be());
    }
}

fn read_byte() -> u8 {
    unsafe { read_volatile((FW_CFG_BASE + REG_DATA) as *const u8) }
}

fn read_be_u16() -> u16 {
    ((read_byte() as u16) << 8) | read_byte() as u16
}

fn read_be_u32() -> u32 {
    ((read_byte() as u32) << 24)
        | ((read_byte() as u32) << 16)
        | ((read_byte() as u32) << 8)
        | read_byte() as u32
}

/// Locate a fw_cfg file by name and return its selector.
pub fn find_file(name: &str) -> Option<u16> {
    select(FILE_DIRECTORY);

    let count = read_be_u32();
    if count > 128 {
        return None;
    }

    for _ in 0..count {
        let _size = read_be_u32();
        let selector = read_be_u16();
        let _reserved = read_be_u16();

        let mut file_name = [0u8; 56];
        for byte in file_name.iter_mut() {
            *byte = read_byte();
        }

        let length = file_name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(file_name.len());
        if &file_name[..length] == name.as_bytes() {
            return Some(selector);
        }
    }

    None
}

/// Write `data` to the fw_cfg file identified by `selector` via DMA.
///
/// The data and the DMA descriptor must live in RAM reachable by QEMU; the
/// kernel is identity mapped, so any static or stack buffer qualifies. QEMU
/// performs the transfer synchronously during the register write and reports
/// success by clearing the descriptor's control word.
pub fn write_file(selector: u16, data: &[u8]) -> bool {
    let length = data.len();
    if length == 0 || length > u32::MAX as usize {
        return false;
    }

    unsafe {
        DMA = DmaAccess {
            control: (((selector as u32) << 16) | DMA_SELECT | DMA_WRITE).to_be(),
            length: (length as u32).to_be(),
            address: (data.as_ptr() as u64).to_be(),
        };
        core::arch::asm!("dmb sy", options(nomem, nostack));

        write_volatile(
            (FW_CFG_BASE + REG_DMA) as *mut u64,
            (addr_of!(DMA) as u64).to_be(),
        );
        core::arch::asm!("dmb sy", options(nomem, nostack));

        let control = read_volatile(addr_of!(DMA.control));
        u32::from_be(control) & DMA_ERROR == 0
    }
}
