//! EL0 demo: map a user code + stack page, drop to EL0, and handle syscalls.
//!
//! Isolation model (Phase 9):
//!   * Kernel RAM blocks keep AP = EL1-only, so EL0 cannot touch the kernel.
//!   * Two dedicated 4 KiB pages (code, stack) are mapped at 0x5000_0000 and
//!     0x5000_1000 with AP = EL0/EL1 RW and EL0-execute permission.
//!   * The user program is a position-independent blob copied from the kernel
//!     image into the code page.

use core::arch::asm;
use core::ptr::addr_of;

use crate::{mmu, println};

const USER_CODE_VA: u64 = 0x5000_0000;
const USER_STACK_VA: u64 = 0x5000_1000;
const PAGE_SIZE: usize = 4096;

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;

#[repr(align(4096))]
#[allow(dead_code)] // Accessed through its address only.
struct UserPage([u8; PAGE_SIZE]);

static mut CODE_PAGE: UserPage = UserPage([0; PAGE_SIZE]);
static mut STACK_PAGE: UserPage = UserPage([0; PAGE_SIZE]);

extern "C" {
    static __user_prog_start: u8;
    static __user_prog_end: u8;
}

/// Load the embedded user program and enter EL0. Does not return.
pub fn run() -> ! {
    unsafe {
        let code_pa = addr_of!(CODE_PAGE) as *mut u8;
        let stack_pa = addr_of!(STACK_PAGE) as *mut u8;
        let blob_start = addr_of!(__user_prog_start);
        let blob_end = addr_of!(__user_prog_end);
        let blob_len = blob_end as usize - blob_start as usize;

        println!();
        println!("Phase 9: user mode (EL0) and syscalls");
        println!("User image : {} bytes", blob_len);

        core::ptr::copy_nonoverlapping(blob_start, code_pa, blob_len);
        core::ptr::write_bytes(stack_pa, 0, PAGE_SIZE);

        mmu::map_user_pages(
            USER_CODE_VA,
            code_pa as u64,
            USER_STACK_VA,
            stack_pa as u64,
        );

        // The code was just written with data stores; make it visible to the
        // instruction cache before executing it at EL0.
        asm!("dsb sy", "ic iallu", "dsb sy", "isb");

        println!(
            "Dropping to EL0: entry={:#x} sp_el0={:#x}",
            USER_CODE_VA,
            USER_STACK_VA + PAGE_SIZE as u64
        );

        asm!(
            "msr sp_el0, {sp}",
            "msr elr_el1, {entry}",
            "msr spsr_el1, {spsr}",
            "eret",
            sp = in(reg) USER_STACK_VA + PAGE_SIZE as u64,
            entry = in(reg) USER_CODE_VA,
            // EL0t, IRQs masked (the timer scheduler is kernel-only for now).
            spsr = in(reg) 0x80u64,
            options(noreturn),
        );
    }
}

/// write(fd, buf, len) -> bytes written, or u64::MAX on error.
pub fn sys_write(fd: u64, pointer: u64, length: u64) -> u64 {
    if fd != 1 || length == 0 || length > PAGE_SIZE as u64 {
        return u64::MAX;
    }

    // SAFETY: the user program owns the mapped user pages; bounded by length.
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length as usize) };
    crate::uart::write_bytes(bytes);
    length
}

/// exit(status) -> never returns.
pub fn sys_exit(status: u64) -> ! {
    println!();
    println!("[kernel] EL0 exit({})", status);
    crate::semihosting_exit(0x20026, 0);
}
