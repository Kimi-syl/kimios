//! AArch64 MMU setup with a 4 KiB granule.
//!
//! KimiOS uses an identity map: virtual address == physical address.
//!
//! Translation table walk with 4 KiB granule and 48-bit VA:
//!
//!   VA[47:39] -> L0 entry -> L1 table
//!   VA[38:30] -> L1 entry -> L2 table
//!   VA[29:21] -> L2 entry -> L3 table OR 2 MiB block
//!   VA[20:12] -> L3 entry -> 4 KiB page
//!   VA[11:0]  -> offset inside page
//!
//! KimiOS builds a three-level tree (L0 -> L1 -> L2) and uses 2 MiB block
//! descriptors at L2, so the L3 level is not needed:
//!
//!   L0[0]  -> L1
//!   L1[0]   = 1 GiB device block  (0x0000_0000 - 0x3FFF_FFFF, UART/GIC/...)
//!   L1[1]  -> L2
//!   L2[0..] = 2 MiB normal blocks (0x4000_0000 - 0x7FFF_FFFF, RAM)
//!
//! Relevant system registers:
//!   MAIR_EL1  - memory attribute indirection (Attr0 = Normal, Attr1 = Device)
//!   TCR_EL1   - translation control (granule, VA size, cacheability, PA size)
//!   TTBR0_EL1 - base address of the translation table tree
//!   SCTLR_EL1 - M bit enables the MMU, C/I bits enable data/instruction cache

use core::arch::asm;
use core::ptr::addr_of_mut;

use crate::pfa;

// Descriptor bits (ARM ARM, VMSAv8-64).
const PTE_VALID: u64 = 1 << 0;
const PTE_TABLE: u64 = 1 << 1;
const PTE_PAGE: u64 = 1 << 1;
const PTE_ATTR_NORMAL: u64 = 0 << 2;
const PTE_ATTR_DEVICE: u64 = 1 << 2;
const PTE_AP_EL0_RW: u64 = 1 << 6; // AP[1] = 1: EL0 and EL1 read/write
const PTE_SH_INNER: u64 = 3 << 8;
const PTE_AF: u64 = 1 << 10;
const PTE_PXN: u64 = 1 << 53;
const PTE_UXN: u64 = 1 << 54;
/// Physical address bits [47:12] of a page descriptor.
const PTE_PAGE_ADDR: u64 = 0x0000_ffff_ffff_f000;
/// Physical address bits [47:21] of a 2 MiB block descriptor.
const PTE_BLOCK_ADDR: u64 = 0x0000_ffff_ffe0_0000;

// MAIR_EL1: Attr0 = Normal write-back RW-allocate (0xFF),
//           Attr1 = Device-nGnRnE (0x00).
const MAIR_NORMAL_WB: u64 = 0xff;
const MAIR_VALUE: u64 = MAIR_NORMAL_WB;

// TCR_EL1:
//   T0SZ  = 16  -> 48-bit virtual addresses
//   TG0   = 00  -> 4 KiB granule
//   SH0   = 11  -> inner shareable
//   ORGN0 = 01  -> outer write-back
//   IRGN0 = 01  -> inner write-back
//   IPS   = 010 -> 40-bit physical addresses
const TCR_VALUE: u64 =
    (16) | (3 << 12) | (1 << 10) | (1 << 8) | (0b010 << 32);

#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; 512],
}

static mut L0_TABLE: PageTable = PageTable { entries: [0; 512] };
static mut L1_TABLE: PageTable = PageTable { entries: [0; 512] };
static mut L2_TABLE: PageTable = PageTable { entries: [0; 512] };
fn l1_device_block(base: u64) -> u64 {
    base | PTE_VALID | PTE_ATTR_DEVICE | PTE_AF | PTE_PXN | PTE_UXN
}

fn l2_normal_block(base: u64) -> u64 {
    // Executable at EL1 (PXN = 0), non-executable at EL0 (UXN = 1).
    base | PTE_VALID | PTE_ATTR_NORMAL | PTE_SH_INNER | PTE_AF | PTE_UXN
}

#[allow(static_mut_refs)]
pub fn init() {
    unsafe {
        // 1. Build the tables. Identity map: VA == PA.
        L0_TABLE.entries[0] =
            (addr_of_mut!(L1_TABLE) as u64) | PTE_VALID | PTE_TABLE;

        // First 1 GiB: peripherals, Device-nGnRnE, never executable.
        L1_TABLE.entries[0] = l1_device_block(0x0000_0000);

        // Second 1 GiB: RAM, Normal WB, EL1-executable, via L2.
        L1_TABLE.entries[1] =
            (addr_of_mut!(L2_TABLE) as u64) | PTE_VALID | PTE_TABLE;

        for i in 0..512 {
            let pa = 0x4000_0000 + (i as u64) * 0x0020_0000;
            L2_TABLE.entries[i] = l2_normal_block(pa);
        }

        let ttbr0 = addr_of_mut!(L0_TABLE) as u64;
        let mut sctlr: u64;

        asm!(
            "dsb sy",
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr}",
            "dsb sy",
            "isb",
            // Invalidate any stale TLB entries before turning the MMU on.
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            // SCTLR_EL1.M = 1 (MMU), .C = 1 (D-cache), .I = 1 (I-cache).
            "mrs {sctlr}, sctlr_el1",
            "mov x9, #0x1005", // M (MMU) | C (D-cache) | I (I-cache)
            "orr {sctlr}, {sctlr}, x9",
            "msr sctlr_el1, {sctlr}",
            "isb",
            mair = in(reg) MAIR_VALUE,
            tcr = in(reg) TCR_VALUE,
            ttbr = in(reg) ttbr0,
            sctlr = out(reg) sctlr,
            out("x9") _,
        );

        let _ = sctlr;
    }
}

/// Enable the MMU on a secondary CPU using the shared kernel tables.
///
/// PSCI starts secondary cores with translation and caches off at EL1. They
/// must install the same MAIR/TCR/TTBR0/SCTLR values as the boot CPU before
/// running Rust code that uses atomics or shared data.
#[allow(static_mut_refs)]
pub fn enable_secondary() {
    unsafe {
        asm!(
            "dsb sy",
            "msr mair_el1, {mair}",
            "msr tcr_el1, {tcr}",
            "msr ttbr0_el1, {ttbr}",
            "dsb sy",
            "isb",
            "tlbi vmalle1",
            "dsb sy",
            "isb",
            "mrs {sctlr}, sctlr_el1",
            "mov x9, #0x1005", // M (MMU) | C (D-cache) | I (I-cache)
            "orr {sctlr}, {sctlr}, x9",
            "msr sctlr_el1, {sctlr}",
            "isb",
            mair = in(reg) MAIR_VALUE,
            tcr = in(reg) TCR_VALUE,
            ttbr = in(reg) addr_of_mut!(L0_TABLE) as u64,
            sctlr = out(reg) _,
            out("x9") _,
        );
    }
}

/// Per-process user address space.
///
/// The kernel image and RAM are identity mapped, so every process gets a copy
/// of the kernel L0/L1/L2 tables with only the user 2 MiB blocks replaced by
/// private L3 tables. This keeps kernel code, stacks and physical memory
/// reachable while TTBR0 points at a process.
pub const MAX_FRAMES: usize = 256;

pub struct AddressSpace {
    ttbr0: u64,
    frames: [u64; MAX_FRAMES],
    frame_count: usize,
}

impl AddressSpace {
    /// Create a fresh address space from the kernel's identity tables.
    pub fn new() -> Option<Self> {
        let l0 = pfa::alloc_frame()?;
        let l1 = pfa::alloc_frame()?;
        let l2 = pfa::alloc_frame()?;

        let mut space = Self {
            ttbr0: l0,
            frames: [0; MAX_FRAMES],
            frame_count: 3,
        };
        space.frames[0] = l0;
        space.frames[1] = l1;
        space.frames[2] = l2;

        let kernel_l0 = addr_of_mut!(L0_TABLE) as u64;
        let kernel_l1 = addr_of_mut!(L1_TABLE) as u64;
        let kernel_l2 = addr_of_mut!(L2_TABLE) as u64;

        // SAFETY: all three frames are page-aligned and reserved by the PFA.
        unsafe {
            copy_page(kernel_l0, l0);
            copy_page(kernel_l1, l1);
            copy_page(kernel_l2, l2);

            // L0[0] -> this address space's L1; L1[1] -> its L2.
            (*((l0) as *mut PageTable)).entries[0] = l1 | PTE_VALID | PTE_TABLE;
            (*((l1) as *mut PageTable)).entries[1] = l2 | PTE_VALID | PTE_TABLE;
            asm!("dsb sy", "isb");
        }

        Some(space)
    }

    /// Allocate a frame and track it for teardown.
    pub fn alloc_frame(&mut self) -> Option<u64> {
        let frame = pfa::alloc_frame()?;
        if self.frame_count == MAX_FRAMES {
            pfa::free_frame(frame);
            return None;
        }
        self.frames[self.frame_count] = frame;
        self.frame_count += 1;
        Some(frame)
    }

    /// Frames still available for this address space (tables and pages).
    pub fn spare_frames(&self) -> usize {
        MAX_FRAMES - self.frame_count
    }

    /// Map one 4 KiB user page. `executable` controls UXN; PXN is always set.
    pub fn map_page(&mut self, virtual_address: u64, physical_address: u64, executable: bool) {
        self.map_entry(virtual_address, physical_address, executable);
    }

    /// Map an existing physical page without owning it (shared framebuffer).
    pub fn map_shared_page(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        executable: bool,
    ) {
        self.map_entry(virtual_address, physical_address, executable);
    }

    fn map_entry(&mut self, virtual_address: u64, physical_address: u64, executable: bool) {
        let l2_index = ((virtual_address >> 21) & 0x1ff) as usize;
        let l3_index = ((virtual_address >> 12) & 0x1ff) as usize;
        let table = self.l3_table(l2_index);
        let table = table.expect("user page-table frame pool exhausted");

        let mut flags = PTE_VALID
            | PTE_PAGE
            | PTE_ATTR_NORMAL
            | PTE_AP_EL0_RW
            | PTE_SH_INNER
            | PTE_AF
            | PTE_PXN;
        if !executable {
            flags |= PTE_UXN;
        }

        unsafe {
            (*(table as *mut PageTable)).entries[l3_index] =
                (physical_address & PTE_PAGE_ADDR) | flags;
        }
    }

    fn l3_table(&mut self, l2_index: usize) -> Option<u64> {
        let l2 = self.l2_table();
        let entry = unsafe { (*(l2 as *const PageTable)).entries[l2_index] };

        if entry & PTE_VALID != 0 && entry & PTE_TABLE != 0 {
            return Some(entry & PTE_PAGE_ADDR);
        }

        let frame = self.alloc_frame()?;
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, 4096);
            (*(l2 as *mut PageTable)).entries[l2_index] = frame | PTE_VALID | PTE_TABLE;
            asm!("dsb sy", "isb");
        }
        Some(frame)
    }

    fn l1_table(&self) -> u64 {
        unsafe {
            (*(self.ttbr0 as *const PageTable)).entries[0] & PTE_PAGE_ADDR
        }
    }

    fn l2_table(&self) -> u64 {
        unsafe {
            (*(self.l1_table() as *const PageTable)).entries[1] & PTE_PAGE_ADDR
        }
    }

    /// Translate a user virtual address through this address space.
    pub fn translate(&self, virtual_address: u64) -> Option<u64> {
        let l0_index = ((virtual_address >> 39) & 0x1ff) as usize;
        let l1_index = ((virtual_address >> 30) & 0x1ff) as usize;
        let l2_index = ((virtual_address >> 21) & 0x1ff) as usize;
        let l3_index = ((virtual_address >> 12) & 0x1ff) as usize;

        let l0_entry = unsafe { (*(self.ttbr0 as *const PageTable)).entries[l0_index] };
        if l0_entry & PTE_VALID == 0 || l0_entry & PTE_TABLE == 0 {
            return None;
        }
        let l1 = l0_entry & PTE_PAGE_ADDR;
        let l1_entry = unsafe { (*(l1 as *const PageTable)).entries[l1_index] };
        if l1_entry & PTE_VALID == 0 || l1_entry & PTE_TABLE == 0 {
            return None;
        }
        let l2 = l1_entry & PTE_PAGE_ADDR;
        let l2_entry = unsafe { (*(l2 as *const PageTable)).entries[l2_index] };
        if l2_entry & PTE_VALID == 0 {
            return None;
        }

        if l2_entry & PTE_TABLE == 0 {
            // 2 MiB block descriptor.
            return Some(
                (l2_entry & PTE_BLOCK_ADDR) | (virtual_address & 0x1f_ffff),
            );
        }

        let l3 = l2_entry & PTE_PAGE_ADDR;
        let l3_entry = unsafe { (*(l3 as *const PageTable)).entries[l3_index] };
        if l3_entry & PTE_VALID == 0 || l3_entry & PTE_PAGE == 0 {
            return None;
        }
        Some((l3_entry & PTE_PAGE_ADDR) | (virtual_address & 0xfff))
    }

    /// Write a 64-bit value into this (possibly inactive) address space.
    pub fn write_u64(&mut self, virtual_address: u64, value: u64) -> bool {
        let Some(physical) = self.translate(virtual_address) else {
            return false;
        };
        unsafe {
            core::ptr::write_volatile(physical as *mut u64, value);
        }
        true
    }

    /// Switch TTBR0 to this address space.
    pub fn activate(&self) {
        unsafe {
            asm!(
                "msr ttbr0_el1, {ttbr}",
                "dsb sy",
                "tlbi vmalle1",
                "dsb sy",
                "isb",
                ttbr = in(reg) self.ttbr0,
            );
        }
    }

    /// Release every frame owned by this address space.
    pub fn destroy(mut self) {
        self.release();
    }

    fn release(&mut self) {
        unsafe {
            asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb");
        }
        for index in 0..self.frame_count {
            pfa::free_frame(self.frames[index]);
        }
        self.frame_count = 0;
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        self.release();
    }
}

unsafe fn copy_page(source: u64, destination: u64) {
    core::ptr::copy_nonoverlapping(
        source as *const u8,
        destination as *mut u8,
        4096,
    );
}

pub fn enabled() -> bool {
    let sctlr: u64;
    unsafe {
        asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
    }
    sctlr & 1 != 0
}
