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
// Pool of L3 tables for user mappings. Each 2 MiB block that receives a
// user page needs one table; four blocks is plenty for the loader + stack.
const USER_TABLE_POOL: usize = 4;

const EMPTY_TABLE: PageTable = PageTable { entries: [0; 512] };

static mut L3_POOL: [PageTable; USER_TABLE_POOL] = [EMPTY_TABLE; USER_TABLE_POOL];
static mut L3_BLOCK: [usize; USER_TABLE_POOL] = [usize::MAX; USER_TABLE_POOL];

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

/// Return (allocating if necessary) the L3 table that backs a 2 MiB block.
/// Replacing the L2 block descriptor removes the identity mapping for that
/// block and installs a table descriptor instead.
#[allow(static_mut_refs)]
fn user_l3_table(l2_index: usize) -> *mut PageTable {
    unsafe {
        for slot in 0..USER_TABLE_POOL {
            if L3_BLOCK[slot] == l2_index {
                return addr_of_mut!(L3_POOL[slot]);
            }
        }

        for slot in 0..USER_TABLE_POOL {
            if L3_BLOCK[slot] == usize::MAX {
                L3_BLOCK[slot] = l2_index;
                let table = addr_of_mut!(L3_POOL[slot]);
                (*table).entries = [0; 512];
                L2_TABLE.entries[l2_index] =
                    (table as u64) | PTE_VALID | PTE_TABLE;
                return table;
            }
        }
    }

    panic!("user page-table pool exhausted");
}

/// Map one 4 KiB page into the user address space.
///
/// `executable` controls UXN: EL0 can execute code pages but not the stack.
/// PXN is always set, so EL1 never executes user pages.
#[allow(static_mut_refs)]
pub fn map_user_page(virtual_address: u64, physical_address: u64, executable: bool) {
    let l2_index = ((virtual_address >> 21) & 0x1ff) as usize;
    let l3_index = ((virtual_address >> 12) & 0x1ff) as usize;

    let table = user_l3_table(l2_index);

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
        (*table).entries[l3_index] = (physical_address & !(0xfff)) | flags;
        asm!("dsb sy", "tlbi vmalle1", "dsb sy", "isb");
    }
}

pub fn enabled() -> bool {
    let sctlr: u64;
    unsafe {
        asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
    }
    sctlr & 1 != 0
}
