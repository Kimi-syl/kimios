//! EL0 entry and syscall implementations.
//!
//! Phase 10 loads `/init` from RamFS, maps its ELF segments into the user
//! address space, and enters it with `eret`.

use core::arch::asm;

use crate::ramfs::Vfs;
use crate::{elf, println, ramfs};

pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 2;

pub fn run_init() -> ! {
    println!();
    println!("Phase 10: VFS + RamFS + ELF loader");

    let file = ramfs::RAMFS.open("/init").expect("no /init in RamFS");
    println!("RamFS      : /init ({} bytes)", file.data.len());

    // Exercise the VFS read path (ELF magic is 0x7f 'E' 'L' 'F').
    let mut magic = [0u8; 4];
    let read = ramfs::RAMFS.read(file, 0, &mut magic);
    println!("VFS read   : {} bytes, magic {:02x?}", read, magic);

    let image = elf::load(file.data).expect("failed to load /init");
    println!(
        "ELF loaded : entry={:#x} stack_top={:#x} pages={}",
        image.entry, image.stack_top, image.pages
    );
    println!("Dropping to EL0...");

    unsafe {
        asm!(
            "msr sp_el0, {sp}",
            "msr elr_el1, {entry}",
            "msr spsr_el1, {spsr}",
            "eret",
            sp = in(reg) image.stack_top,
            entry = in(reg) image.entry,
            // EL0t, IRQs masked (kernel scheduler is kernel-only for now).
            spsr = in(reg) 0x80u64,
            options(noreturn),
        );
    }
}

/// write(fd, buf, len) -> bytes written, or u64::MAX on error.
pub fn sys_write(fd: u64, pointer: u64, length: u64) -> u64 {
    if fd != 1 || length == 0 || length > 4096 {
        return u64::MAX;
    }

    // SAFETY: the ELF loader maps the user image read/write at EL0.
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
