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
const EC_IABT_LOW: u64 = 0x20;
const EC_DABT_LOW: u64 = 0x24;
const EC_IABT_SAME: u64 = 0x21;
const EC_DABT_SAME: u64 = 0x25;
const EC_ALIGNMENT: u64 = 0x26;
const EC_BRK64: u64 = 0x3c;

fn ec_name(ec: u64) -> &'static str {
    match ec {
        EC_UNKNOWN => "Unknown",
        EC_SVC64 => "SVC (syscall)",
        EC_IABT_LOW => "Instruction abort (lower EL)",
        EC_DABT_LOW => "Data abort (lower EL)",
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

        // Linux ABI: a static Linux ELF uses Linux syscall numbers. Route to
        // the translation layer instead of the native KimiOS ABI.
        if process::current_is_linux() {
            if let Some(result) = crate::linux::dispatch(frame, number) {
                frame.x[0] = result;
            }
            return;
        }

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
            usermode::SYS_INPUT_WAIT => {
                match usermode::sys_input_wait(frame) {
                    usermode::InputOutcome::Done(result) => frame.x[0] = result,
                    usermode::InputOutcome::Switched => {}
                }
                return;
            }
            usermode::SYS_SLEEP => {
                match usermode::sys_sleep(frame, arg0) {
                    process::SleepOutcome::Done(result) => frame.x[0] = result,
                    process::SleepOutcome::Switched => {}
                }
                return;
            }
            crate::pe::SYS_PE_EXIT_PROCESS => {
                usermode::pe_exit_process(frame, arg0);
                return;
            }
            crate::pe::SYS_PE_SLEEP => {
                match usermode::sys_sleep(frame, arg0) {
                    process::SleepOutcome::Done(result) => frame.x[0] = result,
                    process::SleepOutcome::Switched => {}
                }
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
            usermode::SYS_FB_INFO => usermode::sys_fb_info(arg0),
            usermode::SYS_FB_FILL => {
                usermode::sys_fb_fill(arg0, arg1, arg2, arg3, arg4)
            }
            usermode::SYS_UNLINK => usermode::sys_unlink(arg0, arg1),
            usermode::SYS_FREEBLOCKS => usermode::sys_freeblocks(),
            usermode::SYS_UPTIME => usermode::sys_uptime(),
            usermode::SYS_BRK => usermode::sys_brk(arg0),
            usermode::SYS_PS => usermode::sys_ps(arg0, arg1),
            usermode::SYS_KILL => usermode::sys_kill(arg0),
            usermode::SYS_RENAME => {
                usermode::sys_rename(arg0, arg1, arg2, arg3)
            }
            usermode::SYS_RTC => usermode::sys_rtc(arg0),
            usermode::SYS_STAT => usermode::sys_stat(arg0, arg1, arg2),
            usermode::SYS_LISTDIR => {
                usermode::sys_listdir(arg0, arg1, arg2, arg3)
            }
            usermode::SYS_MKDIR => usermode::sys_mkdir(arg0, arg1),
            usermode::SYS_RMDIR => usermode::sys_rmdir(arg0, arg1),
            usermode::SYS_FB_MAP => usermode::sys_fb_map(),
            usermode::SYS_INPUT_POLL => usermode::sys_input_poll(),
            usermode::SYS_NET => usermode::sys_net(arg0, arg1, arg2, arg3, arg4),
            crate::pe::SYS_PE_GET_STD_HANDLE => {
                usermode::pe_get_std_handle(arg0)
            }
            crate::pe::SYS_PE_WRITE_FILE => usermode::pe_write_file(
                arg0, arg1, arg2, arg3, arg4,
            ),
            crate::pe::SYS_PE_GET_LAST_ERROR => usermode::pe_get_last_error(),
            crate::pe::SYS_PE_SET_LAST_ERROR => usermode::pe_set_last_error(arg0),
            crate::pe::SYS_PE_GET_TICK_COUNT_64 => usermode::pe_get_tick_count_64(),
            crate::pe::SYS_PE_GET_TICK_COUNT => usermode::pe_get_tick_count(),
            crate::pe::SYS_PE_WRITE_CONSOLE_A => usermode::pe_write_console_a(
                arg0, arg1, arg2, arg3, arg4,
            ),
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
            EC_IABT_LOW | EC_DABT_LOW | EC_IABT_SAME | EC_DABT_SAME => 11,
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
