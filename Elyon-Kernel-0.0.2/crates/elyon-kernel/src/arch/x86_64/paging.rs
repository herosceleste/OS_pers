use core::arch::asm;
use core::ptr::{addr_of_mut, write_bytes};

const ENTRIES: usize = 512;
const HUGE_PAGE_SIZE: u64 = 2 * 1024 * 1024;
const IDENTITY_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const HUGE: u64 = 1 << 7;

#[repr(C, align(4096))]
struct PageTable([u64; ENTRIES]);

static mut PML4: PageTable = PageTable([0; ENTRIES]);
static mut PDPT: PageTable = PageTable([0; ENTRIES]);
static mut PDS: [PageTable; 4] = [
    PageTable([0; ENTRIES]),
    PageTable([0; ENTRIES]),
    PageTable([0; ENTRIES]),
    PageTable([0; ENTRIES]),
];

pub fn install_identity_4g() -> u64 {
    unsafe {
        write_bytes(addr_of_mut!(PML4).cast::<u8>(), 0, core::mem::size_of::<PageTable>());
        write_bytes(addr_of_mut!(PDPT).cast::<u8>(), 0, core::mem::size_of::<PageTable>());
        write_bytes(
            addr_of_mut!(PDS).cast::<u8>(),
            0,
            core::mem::size_of::<[PageTable; 4]>(),
        );

        let pml4 = addr_of_mut!(PML4);
        let pdpt = addr_of_mut!(PDPT);
        let pds = addr_of_mut!(PDS);

        (*pml4).0[0] = pdpt as u64 | PRESENT | WRITABLE;

        for pdpt_index in 0..4usize {
            let pd = core::ptr::addr_of_mut!((*pds)[pdpt_index]);
            (*pdpt).0[pdpt_index] = pd as u64 | PRESENT | WRITABLE;

            for entry in 0..ENTRIES {
                let phys = ((pdpt_index * ENTRIES + entry) as u64) * HUGE_PAGE_SIZE;
                (*pd).0[entry] = phys | PRESENT | WRITABLE | HUGE;
            }
        }

        let pml4_phys = pml4 as u64;
        asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
        pml4_phys
    }
}

pub const fn identity_limit() -> u64 {
    IDENTITY_LIMIT
}
