//! Synchronous exception handling for EL1.

use core::arch::asm;

use crate::println;
use crate::{process, usermode};

/// Register snapshot pushed by `exc_common` in vectors.s.
/// Field order and size MUST match the assembly layout exactly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExceptionFrame {
    pub x: [u64; 31],
    pub elr: u64,
    pub spsr: u64,
}

impl ExceptionFrame {
    pub const EMPTY: Self = Self {
        x: [0; 31],
        elr: 0,
        spsr: 0,
    };
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
    let from_el0 = frame.spsr & 0xf == 0;

    if ec == EC_SVC64 {
        // SVC is complete: ELR already points at the next instruction.
        let number = frame.x[8];
        let arg0 = frame.x[0];
        let arg1 = frame.x[1];
        let arg2 = frame.x[2];
        let arg3 = frame.x[3];
        let arg4 = frame.x[4];

        // These may switch to another process; the frame then belongs to the
        // next process, so do not write a result into it.
        match number {
            usermode::SYS_EXIT => {
                process::sys_exit(frame, arg0);
                return;
            }
            usermode::SYS_WAIT => {
                if let process::WaitOutcome::Status(status) =
                    process::sys_wait(frame, arg0)
                {
                    frame.x[0] = status;
                }
                return;
            }
            usermode::SYS_READ => {
                match usermode::sys_read(frame, arg0, arg1, arg2) {
                    usermode::ReadOutcome::Done(result) => frame.x[0] = result,
                    usermode::ReadOutcome::Switched => {}
                }
                return;
            }
            crate::pe::SYS_PE_EXIT_PROCESS => {
                usermode::pe_exit_process(frame, arg0);
                return;
            }
            _ => {}
        }

        let result = match number {
            usermode::SYS_WRITE => usermode::sys_write(arg0, arg1, arg2),
            usermode::SYS_OPEN => usermode::sys_open(arg0, arg1),
            usermode::SYS_READ_FILE => {
                usermode::sys_read_file(arg0, arg1, arg2, arg3)
            }
            usermode::SYS_CLOSE => usermode::sys_close(arg0),
            usermode::SYS_EXEC => usermode::sys_exec(frame, arg0, arg1),
            usermode::SYS_LIST => usermode::sys_list(arg0, arg1),
            usermode::SYS_SPAWN => usermode::sys_spawn(arg0, arg1),
            usermode::SYS_GETPID => usermode::sys_getpid(),
            usermode::SYS_STATS => usermode::sys_stats(),
            usermode::SYS_CREATE => usermode::sys_create(arg0, arg1),
            usermode::SYS_WRITE_FILE => {
                usermode::sys_write_file(arg0, arg1, arg2, arg3)
            }
            usermode::SYS_DISKINFO => usermode::sys_diskinfo(),
            crate::pe::SYS_PE_GET_STD_HANDLE => {
                usermode::pe_get_std_handle(arg0)
            }
            crate::pe::SYS_PE_WRITE_FILE => usermode::pe_write_file(
                arg0, arg1, arg2, arg3, arg4,
            ),
            crate::pe::SYS_PE_GET_LAST_ERROR => usermode::pe_get_last_error(),
            crate::pe::SYS_PE_SET_LAST_ERROR => usermode::pe_set_last_error(arg0),
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
    } else if from_el0 {
        // A user fault terminates only that process; init or the parent keeps
        // running. Exit status 128 + a small synthetic signal-ish number.
        let status = 128 + match ec {
            EC_IABT_SAME | 0x20 => 11,
            EC_DABT_SAME | 0x24 => 11,
            EC_ALIGNMENT => 7,
            _ => 4,
        };
        println!(
            "[kernel] killing pid {} after user fault (status {})",
            process::current_pid(),
            status
        );
        process::sys_exit(frame, status);
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
