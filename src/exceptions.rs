//! Synchronous exception handling for EL1.

use core::arch::asm;

use crate::println;
use crate::usermode;

/// Register snapshot pushed by `exc_common` in vectors.s.
/// Field order and size MUST match the assembly layout exactly.
#[repr(C)]
pub struct ExceptionFrame {
    pub x: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
}

const EC_UNKNOWN: u64 = 0x00;
const EC_SVC64: u64 = 0x15;
const EC_IABT_SAME: u64 = 0x21;
const EC_DABT_SAME: u64 = 0x25;
const EC_ALIGNMENT: u64 = 0x26;
const EC_BRK64: u64 = 0x3c;

fn ec_name(ec: u64) -> &'static str {
    match ec {
        EC_UNKNOWN => "Unknown",
        EC_SVC64 => "SVC (syscall)",
        EC_IABT_SAME => "Instruction abort (same EL)",
        EC_DABT_SAME => "Data abort (same EL)",
        EC_ALIGNMENT => "Alignment fault",
        EC_BRK64 => "Breakpoint (BRK)",
        _ => "Other",
    }
}

#[no_mangle]
pub extern "C" fn exception_handler(frame: &mut ExceptionFrame) {
    let esr: u64;
    let far: u64;
    unsafe {
        asm!("mrs {}, esr_el1", out(reg) esr, options(nomem, nostack));
        asm!("mrs {}, far_el1", out(reg) far, options(nomem, nostack));
    }

    let ec = (esr >> 26) & 0x3f;
    let iss = esr & 0x01ff_ffff;

    if ec == EC_SVC64 {
        // SVC is complete: ELR already points at the next instruction.
        let number = frame.x[8];
        let arg0 = frame.x[0];
        let arg1 = frame.x[1];
        let arg2 = frame.x[2];
        let arg3 = frame.x[3];

        let result = match number {
            usermode::SYS_WRITE => usermode::sys_write(arg0, arg1, arg2),
            usermode::SYS_EXIT => usermode::sys_exit(arg0),
            usermode::SYS_READ => usermode::sys_read(arg0, arg1, arg2),
            usermode::SYS_OPEN => usermode::sys_open(arg0, arg1),
            usermode::SYS_READ_FILE => {
                usermode::sys_read_file(arg0, arg1, arg2, arg3)
            }
            usermode::SYS_CLOSE => usermode::sys_close(arg0),
            usermode::SYS_EXEC => usermode::sys_exec(frame, arg0, arg1),
            usermode::SYS_LIST => usermode::sys_list(arg0, arg1),
            _ => {
                println!("Unknown syscall {} (x0={:#x})", number, arg0);
                u64::MAX
            }
        };

        frame.x[0] = result;
        return;
    }

    println!();
    println!("=== EXCEPTION ===");
    println!("Type     : {} (EC={:#04x})", ec_name(ec), ec);
    println!("ESR_EL1  : {:#018x}", esr);
    println!("ISS      : {:#x}", iss);
    println!("FAR_EL1  : {:#018x}", far);
    println!("ELR_EL1  : {:#018x}", frame.elr);
    println!("SPSR_EL1 : {:#018x}", frame.spsr);
    println!("x0/x1    : {:#x} / {:#x}", frame.x[0], frame.x[1]);

    if ec == EC_BRK64 {
        // BRK is 4 bytes; skip it and return to the next instruction.
        println!("Breakpoint handled, resuming.");
        println!();
        frame.elr = frame.elr.wrapping_add(4);
    } else {
        println!("Fatal: no recovery for this exception.");
        crate::semihosting_exit(0x20023, 1);
    }
}

/// Install the vector table in VBAR_EL1.
pub fn init() {
    extern "C" {
        static __vector_table: u8;
    }
    unsafe {
        let vbar = core::ptr::addr_of!(__vector_table) as u64;
        asm!("msr vbar_el1, {}", "isb", in(reg) vbar, options(nostack, preserves_flags));
        println!("VBAR_EL1 set to {:#018x}", vbar);
    }
}
