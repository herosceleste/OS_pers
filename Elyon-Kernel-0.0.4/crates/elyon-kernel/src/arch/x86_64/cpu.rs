use core::arch::asm;

// `cli`/`sti` délimitent des sections critiques : sans `nomem`, le compilateur
// les traite comme des accès mémoire et n'y déplace pas les lectures/écritures.

pub fn disable_interrupts() {
    // SAFETY: masque les interruptions du CPU courant ; aucun invariant mémoire.
    unsafe { asm!("cli", options(nostack)) }
}

pub fn enable_interrupts() {
    // SAFETY: appelé seulement une fois l'IDT et les sources d'IRQ configurées.
    unsafe { asm!("sti", options(nostack)) }
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

pub struct ApicCapabilities {
    pub apic: bool,
    pub x2apic: bool,
}

pub fn apic_capabilities() -> ApicCapabilities {
    let leaf = core::arch::x86_64::__cpuid(1);
    ApicCapabilities {
        apic: leaf.edx & (1 << 9) != 0,
        x2apic: leaf.ecx & (1 << 21) != 0,
    }
}

/// Renvoie (feuille hyperviseur maximale, EBX de 0x40000010), ou (0, 0) hors
/// hyperviseur : sans le bit CPUID.1:ECX[31], la plage 0x4000_xxxx n'a pas de
/// sens et peut refléter la dernière feuille standard.
pub fn hypervisor_timing_leaf() -> (u32, u32) {
    if core::arch::x86_64::__cpuid(1).ecx & (1 << 31) == 0 {
        return (0, 0);
    }
    let max_leaf = core::arch::x86_64::__cpuid(0x4000_0000).eax;
    if max_leaf < 0x4000_0010 {
        return (max_leaf, 0);
    }
    (max_leaf, core::arch::x86_64::__cpuid(0x4000_0010).ebx)
}

pub unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags)
        );
    }
    ((high as u64) << 32) | low as u64
}

pub unsafe fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags)
        );
    }
}

pub unsafe fn in_u8(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

pub unsafe fn out_u8(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub fn read_tr() -> u16 {
    let selector: u16;
    unsafe {
        asm!(
            "str ax",
            out("ax") selector,
            options(nomem, nostack, preserves_flags)
        );
    }
    selector
}

pub fn idle_forever() -> ! {
    loop {
        unsafe { asm!("sti; hlt", options(nomem, nostack)) }
    }
}

pub fn read_rsp() -> u64 {
    let value: u64;
    unsafe {
        asm!(
            "mov {}, rsp",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}
