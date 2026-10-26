use core::arch::asm;

use elyon_core::user_access;

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

/// Vrai si IF est positionné sur le CPU courant.
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: lit RFLAGS via la pile, sans effet de bord.
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) }
    flags & (1 << 9) != 0
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

const CR4_MCE: u64 = 1 << 6;
const CPUID_EDX_MCE: u32 = 1 << 7;

/// CPUID.01H:EDX, où se lit la prise en charge de #MC.
pub fn cpuid_1_edx() -> u32 {
    core::arch::x86_64::__cpuid(1).edx
}

/// Active CR4.MCE quand le CPU gère #MC (K005-03). Un AP sort du trampoline
/// avec CR4 = PAE seul : sans MCE, un #MC le mettrait en arrêt (shutdown)
/// au lieu d'entrer dans le gestionnaire.
pub fn enable_machine_check() {
    if cpuid_1_edx() & CPUID_EDX_MCE == 0 {
        return;
    }
    let cr4 = read_cr4() | CR4_MCE;
    // SAFETY: CR4.MCE n'active que la livraison de #MC, dont la porte et la
    // pile IST sont en place avant l'appel.
    unsafe { asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags)) }
}

/// SMEP, SMAP et UMIP annoncés par CPUID (feuille 7, sous-feuille 0).
pub fn supervisor_protection() -> user_access::Features {
    let max_leaf = core::arch::x86_64::__cpuid(0).eax;
    let leaf = core::arch::x86_64::__cpuid_count(7, 0);
    user_access::features(max_leaf, leaf.ebx, leaf.ecx)
}

/// Active SMEP, SMAP et UMIP selon `features`, et CR0.WP (K005-04). Appelé
/// une fois les tables d'Elyon chargées : le mappage identité ne porte jamais
/// le bit U/S, alors que celles du firmware peuvent le porter. Le trampoline
/// des AP ne pose que PE et PG dans CR0 : sans WP, le noyau y écrirait sur
/// toute page en lecture seule, et une copie vers une telle page réussirait.
pub fn enable_supervisor_protection(features: user_access::Features) {
    let cr0 = read_cr0() | user_access::CR0_WP;
    let cr4 = read_cr4() | user_access::cr4_bits(features);
    // SAFETY: WP fait seulement respecter le bit W des pages au noyau, qui
    // n'écrit jamais sur ses pages en lecture seule ; SMEP et SMAP ne
    // restreignent que les accès du noyau aux pages utilisateur, que seule la
    // copie validée ouvre ; UMIP, que le ring 3.
    unsafe {
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
}

pub fn read_rflags() -> u64 {
    let flags: u64;
    // SAFETY: lit RFLAGS via la pile, sans effet de bord.
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) }
    flags
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

/// Attend la prochaine interruption IRQ actives, puis remasque les IRQ. `sti`
/// ne prend effet qu'après `hlt` : aucune interruption ne peut se glisser entre
/// le test de l'appelant et l'arrêt.
pub fn wait_for_interrupt() {
    unsafe { asm!("sti; hlt; cli", options(nomem, nostack)) }
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
