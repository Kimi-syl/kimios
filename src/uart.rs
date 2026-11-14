//! PL011 UART driver for QEMU's `virt` machine.
//!
//! The PL011 is memory-mapped at 0x0900_0000 on `virt`. All accesses use
//! volatile reads/writes so the compiler cannot cache or reorder them.

use core::fmt;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

const UART0_BASE: usize = 0x0900_0000;

// PL011 register offsets (ARM PrimeCell PL011 TRM).
const UARTDR: usize = 0x00; // Data register
const UARTFR: usize = 0x18; // Flag register
const UARTIBRD: usize = 0x24; // Integer baud rate divisor
const UARTFBRD: usize = 0x28; // Fractional baud rate divisor
const UARTLCR_H: usize = 0x2c; // Line control register
const UARTCR: usize = 0x30; // Control register
const UARTIFLS: usize = 0x34; // Interrupt FIFO level select
const UARTIMSC: usize = 0x38; // Interrupt mask set/clear
const UARTICR: usize = 0x44; // Interrupt clear

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

// UARTIMSC / UARTICR bits
const INT_RX: u32 = 1 << 4; // RX FIFO at/above trigger level
const INT_RT: u32 = 1 << 6; // RX timeout (FIFO non-empty but idle)

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
    pub fn try_recv(&self) -> Option<u8> {
        if self.read(UARTFR) & FR_RXFE != 0 {
            None
        } else {
            Some((self.read(UARTDR) & 0xff) as u8)
        }
    }

    /// Request an interrupt on received bytes.
    fn enable_rx_interrupt(&self) {
        // 1/8 FIFO trigger for RX and TX (only RX is used).
        self.write(UARTIFLS, 0);
        self.write(UARTIMSC, INT_RX | INT_RT);
    }

    fn clear_rx_interrupt(&self) {
        self.write(UARTICR, INT_RX | INT_RT);
    }
}

/// A second, lock-free view of the same UART for use from IRQ context.
///
/// The TX mutex is only ever held with IRQs masked (see `_print` and
/// `write_bytes`), so the RX interrupt handler cannot interleave with a
/// locked transmission. RX registers are owned solely by the IRQ handler.
const CONSOLE: Uart = Uart::new(UART0_BASE);

const LINE_CAPACITY: usize = 128;
/// Completed lines buffered so fast/pasted input is not lost while the reader
/// is busy. The IRQ handler assembles the current line in `LINE`; each
/// newline copies it into the queue consumed by `take_line`.
const QUEUE_LINES: usize = 4;

static mut LINE: [u8; LINE_CAPACITY] = [0; LINE_CAPACITY];
static mut LINE_LENGTH: usize = 0;
static mut QUEUE: [[u8; LINE_CAPACITY]; QUEUE_LINES] = [[0; LINE_CAPACITY]; QUEUE_LINES];
static mut QUEUE_LENGTH: [usize; QUEUE_LINES] = [0; QUEUE_LINES];
static mut QUEUE_READ: usize = 0;
static mut QUEUE_COUNT: usize = 0;
static LINE_READY: AtomicBool = AtomicBool::new(false);

/// Enable the PL011 receive interrupt (call after the GIC SPI is routed).
pub fn enable_rx_interrupt() {
    CONSOLE.enable_rx_interrupt();
}

/// Drain the RX FIFO, echoing and editing the pending line.
///
/// Returns `true` when a newline completed a line. Must run with interrupts
/// masked (IRQ context) so the line state has a single writer.
pub fn rx_interrupt() -> bool {
    CONSOLE.clear_rx_interrupt();

    let mut completed = false;
    while let Some(byte) = CONSOLE.try_recv() {
        unsafe {
            match byte {
                b'\r' | b'\n' => {
                    CONSOLE.send_byte(b'\r');
                    CONSOLE.send_byte(b'\n');

                    if QUEUE_COUNT < QUEUE_LINES {
                        let index = (QUEUE_READ + QUEUE_COUNT) % QUEUE_LINES;
                        let length = LINE_LENGTH;
                        QUEUE[index][..length].copy_from_slice(&LINE[..length]);
                        QUEUE_LENGTH[index] = length;
                        QUEUE_COUNT += 1;
                        LINE_READY.store(true, Ordering::Release);
                        completed = true;
                    }
                    LINE_LENGTH = 0;
                }
                0x7f | 0x08 => {
                    if LINE_LENGTH > 0 {
                        LINE_LENGTH -= 1;
                        CONSOLE.send_byte(0x08);
                        CONSOLE.send_byte(b' ');
                        CONSOLE.send_byte(0x08);
                    }
                }
                _ => {
                    if LINE_LENGTH < LINE_CAPACITY {
                        LINE[LINE_LENGTH] = byte;
                        LINE_LENGTH += 1;
                        CONSOLE.send_byte(byte);
                    }
                }
            }
        }
    }

    completed
}

/// Copy a completed line into `buffer` (truncating to its length). Returns
/// `None` while no line is ready. Runs with interrupts masked (syscall path).
pub fn take_line(buffer: &mut [u8]) -> Option<usize> {
    if !LINE_READY.load(Ordering::Acquire) {
        return None;
    }

    let (length, index) = unsafe { (QUEUE_LENGTH[QUEUE_READ], QUEUE_READ) };
    let copied = length.min(buffer.len());
    if copied > 0 {
        buffer[..copied].copy_from_slice(unsafe { &QUEUE[index][..copied] });
    }
    unsafe {
        QUEUE_READ = (QUEUE_READ + 1) % QUEUE_LINES;
        QUEUE_COUNT -= 1;
        if QUEUE_COUNT == 0 {
            LINE_READY.store(false, Ordering::Release);
        }
    }
    Some(copied)
}

/// Wait for a line with interrupts enabled. Used only when there is no other
/// runnable process to switch to.
pub fn wait_for_line() {
    crate::gic::enable_irqs();
    while !LINE_READY.load(Ordering::Acquire) {
        unsafe { core::arch::asm!("wfi") };
    }
    crate::gic::disable_irqs();
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

/// Write raw bytes (used by the write syscall). Newlines become CRLF.
pub fn write_bytes(bytes: &[u8]) {
    use core::arch::asm;

    let daif: u64;
    unsafe {
        asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        asm!("msr daifset, #2", options(nomem, nostack));
    }

    let uart = UART0.lock();
    for &byte in bytes {
        if byte == b'\n' {
            uart.send_byte(b'\r');
        }
        uart.send_byte(byte);
    }

    unsafe {
        asm!("msr daif, {}", in(reg) daif, options(nomem, nostack));
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
