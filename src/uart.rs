//! PL011 UART driver for QEMU's `virt` machine.
//!
//! The PL011 is memory-mapped at 0x0900_0000 on `virt`. All accesses use
//! volatile reads/writes so the compiler cannot cache or reorder them.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use spin::Mutex;

const UART0_BASE: usize = 0x0900_0000;

// PL011 register offsets (ARM PrimeCell PL011 TRM).
const UARTDR: usize = 0x00; // Data register
const UARTFR: usize = 0x18; // Flag register
const UARTIBRD: usize = 0x24; // Integer baud rate divisor
const UARTFBRD: usize = 0x28; // Fractional baud rate divisor
const UARTLCR_H: usize = 0x2c; // Line control register
const UARTCR: usize = 0x30; // Control register

// UARTFR bits
const FR_TXFF: u32 = 1 << 5; // TX FIFO full
const FR_RXFE: u32 = 1 << 4; // RX FIFO empty

// UARTLCR_H bits
const LCR_H_FEN: u32 = 1 << 4; // Enable FIFOs
const LCR_H_WLEN_8: u32 = 3 << 5; // 8 data bits, no parity, 1 stop bit

// UARTCR bits
const CR_UARTEN: u32 = 1 << 0; // UART enable
const CR_TXE: u32 = 1 << 8; // Transmit enable
const CR_RXE: u32 = 1 << 9; // Receive enable

pub struct Uart {
    base: usize,
}

impl Uart {
    pub const fn new(base: usize) -> Self {
        Self { base }
    }

    fn reg(&self, offset: usize) -> *mut u32 {
        (self.base + offset) as *mut u32
    }

    fn read(&self, offset: usize) -> u32 {
        unsafe { read_volatile(self.reg(offset)) }
    }

    fn write(&self, offset: usize, value: u32) {
        unsafe { write_volatile(self.reg(offset), value) }
    }

    /// Configure for 115200 baud, 8N1 with FIFOs enabled.
    ///
    /// QEMU's PL011 is clocked at 24 MHz:
    ///   divisor  = 24_000_000 / (16 * 115200) = 13.0208
    ///   IBRD     = 13
    ///   FBRD     = round(0.0208 * 64) = 1
    pub fn init(&self) {
        self.write(UARTCR, 0); // Disable while reconfiguring
        self.write(UARTIBRD, 13);
        self.write(UARTFBRD, 1);
        self.write(UARTLCR_H, LCR_H_FEN | LCR_H_WLEN_8);
        self.write(UARTCR, CR_UARTEN | CR_TXE | CR_RXE);
    }

    fn send_byte(&self, byte: u8) {
        while self.read(UARTFR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        self.write(UARTDR, byte as u32);
    }

    /// Non-blocking receive. Returns `None` if the RX FIFO is empty.
    #[allow(dead_code)]
    pub fn try_recv(&self) -> Option<u8> {
        if self.read(UARTFR) & FR_RXFE != 0 {
            None
        } else {
            Some((self.read(UARTDR) & 0xff) as u8)
        }
    }
}

impl fmt::Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // Terminals expect CRLF; translate '\n' on the way out.
            if byte == b'\n' {
                self.send_byte(b'\r');
            }
            self.send_byte(byte);
        }
        Ok(())
    }
}

pub static UART0: Mutex<Uart> = Mutex::new(Uart::new(UART0_BASE));

pub fn init() {
    UART0.lock().init();
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::arch::asm;
    use fmt::Write;

    // IRQ-safe console: a timer IRQ must not deadlock on the UART mutex
    // while the interrupted context holds it.
    let daif: u64;
    unsafe {
        asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        asm!("msr daifset, #2", options(nomem, nostack));
    }

    UART0.lock().write_fmt(args).unwrap();

    unsafe {
        asm!("msr daif, {}", in(reg) daif, options(nomem, nostack));
    }
}

/// Write formatted text to the serial console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::uart::_print(format_args!($($arg)*))
    };
}

/// Write formatted text to the serial console, followed by a newline.
#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}
