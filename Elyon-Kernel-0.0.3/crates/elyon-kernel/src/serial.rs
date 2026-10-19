use core::arch::asm;
use core::fmt::{self, Write};
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};

const COM1: u16 = 0x3F8;
const RFLAGS_IF: u64 = 1 << 9;

static SERIAL_LOCK: AtomicBool = AtomicBool::new(false);

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

pub struct SerialPort;

impl SerialPort {
    fn write_byte(&mut self, byte: u8) {
        while !transmit_empty() {
            spin_loop();
        }
        unsafe { outb(COM1, byte) };
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    let rflags = read_rflags();
    unsafe { asm!("cli", options(nomem, nostack)) };

    while SERIAL_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        spin_loop();
    }

    let _ = SerialPort.write_fmt(args);
    SERIAL_LOCK.store(false, Ordering::Release);

    if rflags & RFLAGS_IF != 0 {
        unsafe { asm!("sti", options(nomem, nostack)) };
    }
}

fn read_rflags() -> u64 {
    let flags: u64;
    unsafe {
        asm!(
            "pushfq",
            "pop {}",
            out(reg) flags,
            options(nomem, preserves_flags)
        );
    }
    flags
}

fn transmit_empty() -> bool {
    unsafe { inb(COM1 + 5) & 0x20 != 0 }
}

#[inline]
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::serial::print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprintln {
    () => { $crate::kprint!("\n") };
    ($fmt:expr) => { $crate::kprint!(concat!($fmt, "\n")) };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::kprint!(concat!($fmt, "\n"), $($arg)*)
    };
}
