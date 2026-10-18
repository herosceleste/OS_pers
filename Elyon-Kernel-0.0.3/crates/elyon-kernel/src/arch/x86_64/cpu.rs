use core::arch::asm;

pub fn disable_interrupts() {
    unsafe { asm!("cli", options(nomem, nostack, preserves_flags)) }
}

pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        unsafe { asm!("hlt", options(nomem, nostack)) }
    }
}

pub fn read_cr0() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags)) }
    value
}

pub fn read_cr2() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) }
    value
}

pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) }
    value
}

pub fn read_cr4() -> u64 {
    let value: u64;
    unsafe { asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags)) }
    value
}

pub struct CpuVendor {
    bytes: [u8; 12],
}

impl CpuVendor {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes).unwrap_or("unknown")
    }
}

pub fn vendor_string() -> CpuVendor {
    let leaf = core::arch::x86_64::__cpuid(0);
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&leaf.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&leaf.edx.to_le_bytes());
    bytes[8..12].copy_from_slice(&leaf.ecx.to_le_bytes());
    CpuVendor { bytes }
}
