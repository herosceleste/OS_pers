#![no_main]
#![no_std]

extern crate alloc;

mod serial;

use core::arch::asm;
use core::mem::size_of;
use core::panic::PanicInfo;
use core::ptr::{copy_nonoverlapping, write_bytes};

use elyon_abi::{
    BootInfo, FramebufferFormat, MAX_MEMORY_REGIONS, MemoryRegion, MemoryRegionKind, PAGE_SIZE,
};
use uefi::boot::{AllocateType, MemoryType};
use uefi::fs::FileSystem;
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::table::cfg::ConfigTableEntry;
use uefi::{Status, cstr16, entry};

const KERNEL_PATH: &uefi::CStr16 = cstr16!("\\ELYON\\KERNEL.ELF");
const KERNEL_STACK_PAGES: usize = 16;
const LOW_MEMORY_LIMIT: u64 = 0xFFFF_FFFF;
const PT_LOAD: u32 = 1;
const ELF_MACHINE_X86_64: u16 = 62;

#[entry]
fn main() -> Status {
    uefi::helpers::init().expect("UEFI helper initialisation failed");
    serial::init();

    lprintln!("ELYON LOADER 0.0.5");
    lprintln!("[loader] firmware=UEFI architecture=x86_64");

    let kernel_bytes = read_kernel();
    let elf = ElfImage::parse(&kernel_bytes).unwrap_or_else(|reason| {
        lprintln!("[loader] ELF validation failed: {}", reason);
        halt()
    });

    let kernel = elf.load(&kernel_bytes).unwrap_or_else(|reason| {
        lprintln!("[loader] ELF load failed: {}", reason);
        halt()
    });

    lprintln!(
        "[loader] kernel ELF loaded phys={:#x}..{:#x} entry={:#x}",
        kernel.start,
        kernel.end,
        kernel.entry
    );

    let stack = uefi::boot::allocate_pages(
        AllocateType::MaxAddress(LOW_MEMORY_LIMIT),
        MemoryType::LOADER_DATA,
        KERNEL_STACK_PAGES,
    )
    .unwrap_or_else(|_| {
        lprintln!("[loader] kernel stack allocation failed");
        halt()
    });
    let stack_start = stack.as_ptr() as u64;
    let stack_end = stack_start + KERNEL_STACK_PAGES as u64 * PAGE_SIZE;

    let boot_info_pages = size_of::<BootInfo>().div_ceil(PAGE_SIZE as usize);
    let boot_info_mem = uefi::boot::allocate_pages(
        AllocateType::MaxAddress(LOW_MEMORY_LIMIT),
        MemoryType::LOADER_DATA,
        boot_info_pages,
    )
    .unwrap_or_else(|_| {
        lprintln!("[loader] BootInfo allocation failed");
        halt()
    });

    let boot_info_ptr = boot_info_mem.as_ptr().cast::<BootInfo>();
    unsafe {
        boot_info_ptr.write(BootInfo::empty());
        (*boot_info_ptr).kernel_phys_start = kernel.start;
        (*boot_info_ptr).kernel_phys_end = kernel.end;
        (*boot_info_ptr).kernel_stack_start = stack_start;
        (*boot_info_ptr).kernel_stack_end = stack_end;
        (*boot_info_ptr).boot_info_phys_start = boot_info_mem.as_ptr() as u64;
        (*boot_info_ptr).boot_info_phys_end =
            boot_info_mem.as_ptr() as u64 + boot_info_pages as u64 * PAGE_SIZE;
        (*boot_info_ptr).acpi_rsdp = find_acpi_rsdp();
    }

    capture_framebuffer(boot_info_ptr);

    lprintln!(
        "[loader] stack={:#x}..{:#x} bootinfo={:#x}",
        stack_start,
        stack_end,
        boot_info_ptr as u64
    );
    lprintln!("[loader] leaving UEFI Boot Services");

    // The kernel image, kernel stack and BootInfo are all allocated before this
    // boundary. No UEFI allocation or protocol call is made afterwards.
    let memory_map = unsafe { uefi::boot::exit_boot_services(None) };

    unsafe {
        populate_memory_map(&mut *boot_info_ptr, &memory_map);
        (*boot_info_ptr).boot_services_exited = 1;
        (*boot_info_ptr).sort_and_validate_memory_map();

        lprintln!(
            "[loader] memory map regions={} valid={} conventional={} MiB",
            (*boot_info_ptr).memory_region_count,
            (*boot_info_ptr).memory_map_valid,
            (*boot_info_ptr).usable_memory_bytes / 1024 / 1024
        );

        enter_kernel(kernel.entry, stack_end, boot_info_ptr.cast_const())
    }
}

fn read_kernel() -> alloc::vec::Vec<u8> {
    let proto =
        uefi::boot::get_image_file_system(uefi::boot::image_handle()).unwrap_or_else(|_| {
            lprintln!("[loader] boot filesystem unavailable");
            halt()
        });
    let mut fs = FileSystem::new(proto);
    let bytes = fs.read(KERNEL_PATH).unwrap_or_else(|_| {
        lprintln!("[loader] {} not found", KERNEL_PATH);
        halt()
    });
    lprintln!("[loader] kernel file size={} bytes", bytes.len());
    bytes
}

fn find_acpi_rsdp() -> u64 {
    let mut acpi2 = 0u64;
    let mut acpi1 = 0u64;

    uefi::system::with_config_table(|tables| {
        for entry in tables {
            if entry.guid == ConfigTableEntry::ACPI2_GUID {
                acpi2 = entry.address as u64;
            } else if entry.guid == ConfigTableEntry::ACPI_GUID {
                acpi1 = entry.address as u64;
            }
        }
    });

    let address = if acpi2 != 0 { acpi2 } else { acpi1 };
    if address != 0 {
        lprintln!("[loader] ACPI RSDP={:#x}", address);
    } else {
        lprintln!("[loader] warning: ACPI RSDP not found");
    }
    address
}

fn capture_framebuffer(info: *mut BootInfo) {
    let Ok(handle) = uefi::boot::get_handle_for_protocol::<GraphicsOutput>() else {
        lprintln!("[loader] GOP unavailable; serial-only boot");
        return;
    };
    let Ok(mut gop) = uefi::boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
        lprintln!("[loader] GOP open failed; serial-only boot");
        return;
    };

    let mode = gop.current_mode_info();
    let format = match mode.pixel_format() {
        PixelFormat::Rgb => FramebufferFormat::Rgb,
        PixelFormat::Bgr => FramebufferFormat::Bgr,
        _ => {
            lprintln!("[loader] GOP pixel format unsupported");
            return;
        }
    };

    let (width, height) = mode.resolution();
    let stride = mode.stride();
    let mut fb = gop.frame_buffer();

    unsafe {
        (*info).framebuffer_base = fb.as_mut_ptr() as u64;
        (*info).framebuffer_size = fb.size() as u64;
        (*info).framebuffer_width = width as u32;
        (*info).framebuffer_height = height as u32;
        (*info).framebuffer_stride = stride as u32;
        (*info).framebuffer_format = format;
    }

    lprintln!(
        "[loader] GOP={}x{} stride={} base={:#x}",
        width,
        height,
        stride,
        fb.as_mut_ptr() as u64
    );
}

unsafe fn populate_memory_map(info: &mut BootInfo, memory_map: &impl MemoryMap) {
    let mut index = 0usize;

    for descriptor in memory_map.entries() {
        if index >= MAX_MEMORY_REGIONS {
            info.memory_map_truncated = 1;
            break;
        }

        let kind = classify_memory(descriptor.ty);
        let bytes = descriptor.page_count.saturating_mul(PAGE_SIZE);
        let end = descriptor.phys_start.saturating_add(bytes);

        if kind == MemoryRegionKind::Usable {
            info.usable_memory_bytes = info.usable_memory_bytes.saturating_add(bytes);
        }

        if !matches!(kind, MemoryRegionKind::Mmio | MemoryRegionKind::Unusable) {
            info.highest_physical_address = info.highest_physical_address.max(end);
        }

        info.memory_regions[index] = MemoryRegion {
            start: descriptor.phys_start,
            pages: descriptor.page_count,
            uefi_type: descriptor.ty.0,
            kind,
            _padding: [0; 3],
        };
        index += 1;
    }

    info.memory_region_count = index as u32;
}

fn classify_memory(ty: MemoryType) -> MemoryRegionKind {
    if ty == MemoryType::CONVENTIONAL {
        MemoryRegionKind::Usable
    } else if ty == MemoryType::BOOT_SERVICES_CODE
        || ty == MemoryType::BOOT_SERVICES_DATA
        || ty == MemoryType::ACPI_RECLAIM
    {
        MemoryRegionKind::Reclaimable
    } else if ty == MemoryType::LOADER_CODE || ty == MemoryType::LOADER_DATA {
        MemoryRegionKind::Bootstrap
    } else if ty == MemoryType::RUNTIME_SERVICES_CODE
        || ty == MemoryType::RUNTIME_SERVICES_DATA
        || ty == MemoryType::ACPI_NON_VOLATILE
    {
        MemoryRegionKind::FirmwareReserved
    } else if ty == MemoryType::MMIO || ty == MemoryType::MMIO_PORT_SPACE {
        MemoryRegionKind::Mmio
    } else if ty == MemoryType::UNUSABLE {
        MemoryRegionKind::Unusable
    } else {
        MemoryRegionKind::Reserved
    }
}

struct LoadedKernel {
    start: u64,
    end: u64,
    entry: u64,
}

struct ElfImage {
    entry: u64,
    phoff: usize,
    phentsize: usize,
    phnum: usize,
}

impl ElfImage {
    fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 64 {
            return Err("file too small");
        }
        if &bytes[0..4] != b"\x7fELF" {
            return Err("invalid ELF magic");
        }
        if bytes[4] != 2 || bytes[5] != 1 {
            return Err("ELF must be 64-bit little-endian");
        }
        if read_u16(bytes, 18)? != ELF_MACHINE_X86_64 {
            return Err("ELF machine is not x86_64");
        }

        let entry = read_u64(bytes, 24)?;
        let phoff = read_u64(bytes, 32)? as usize;
        let phentsize = read_u16(bytes, 54)? as usize;
        let phnum = read_u16(bytes, 56)? as usize;

        if phentsize < 56 || phnum == 0 {
            return Err("invalid program header table");
        }
        let table_size = phentsize
            .checked_mul(phnum)
            .ok_or("program header overflow")?;
        if phoff
            .checked_add(table_size)
            .ok_or("program header overflow")?
            > bytes.len()
        {
            return Err("program headers outside file");
        }

        Ok(Self {
            entry,
            phoff,
            phentsize,
            phnum,
        })
    }

    fn load(&self, bytes: &[u8]) -> Result<LoadedKernel, &'static str> {
        let mut min_addr = u64::MAX;
        let mut max_addr = 0u64;
        let mut found = false;

        for i in 0..self.phnum {
            let ph = self.phoff + i * self.phentsize;
            if read_u32(bytes, ph)? != PT_LOAD {
                continue;
            }
            found = true;
            let vaddr = read_u64(bytes, ph + 16)?;
            let paddr = read_u64(bytes, ph + 24)?;
            let memsz = read_u64(bytes, ph + 40)?;
            let load_addr = if paddr != 0 { paddr } else { vaddr };
            min_addr = min_addr.min(align_down(load_addr, PAGE_SIZE));
            max_addr = max_addr.max(align_up(
                load_addr
                    .checked_add(memsz)
                    .ok_or("segment address overflow")?,
                PAGE_SIZE,
            ));
        }

        if !found || min_addr >= max_addr {
            return Err("no loadable ELF segments");
        }
        if max_addr > LOW_MEMORY_LIMIT {
            return Err("kernel must reside below 4 GiB in 0.0.5");
        }

        let pages = ((max_addr - min_addr) / PAGE_SIZE) as usize;
        let base = uefi::boot::allocate_pages(
            AllocateType::Address(min_addr),
            MemoryType::LOADER_DATA,
            pages,
        )
        .map_err(|_| "UEFI could not reserve kernel physical range")?;

        unsafe { write_bytes(base.as_ptr(), 0, pages * PAGE_SIZE as usize) };

        for i in 0..self.phnum {
            let ph = self.phoff + i * self.phentsize;
            if read_u32(bytes, ph)? != PT_LOAD {
                continue;
            }

            let offset = read_u64(bytes, ph + 8)? as usize;
            let vaddr = read_u64(bytes, ph + 16)?;
            let paddr = read_u64(bytes, ph + 24)?;
            let filesz = read_u64(bytes, ph + 32)? as usize;
            let memsz = read_u64(bytes, ph + 40)? as usize;
            let load_addr = if paddr != 0 { paddr } else { vaddr };

            if filesz > memsz {
                return Err("ELF filesz exceeds memsz");
            }
            let file_end = offset.checked_add(filesz).ok_or("segment file overflow")?;
            if file_end > bytes.len() {
                return Err("segment outside kernel file");
            }
            let load_end = load_addr
                .checked_add(memsz as u64)
                .ok_or("segment memory overflow")?;
            if load_addr < min_addr || load_end > max_addr {
                return Err("segment outside reserved kernel range");
            }

            unsafe {
                copy_nonoverlapping(bytes.as_ptr().add(offset), load_addr as *mut u8, filesz);
            }
        }

        if self.entry < min_addr || self.entry >= max_addr {
            return Err("entry point outside loaded kernel");
        }

        Ok(LoadedKernel {
            start: min_addr,
            end: max_addr,
            entry: self.entry,
        })
    }
}

unsafe fn enter_kernel(entry: u64, stack_top: u64, boot_info: *const BootInfo) -> ! {
    unsafe {
        asm!(
            "cli",
            "mov rsp, {stack}",
            "and rsp, -16",
            "sub rsp, 8",
            "mov qword ptr [rsp], 0",
            "xor rbp, rbp",
            "jmp {entry}",
            stack = in(reg) stack_top,
            entry = in(reg) entry,
            in("rdi") boot_info,
            options(noreturn)
        );
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    let raw = bytes.get(offset..offset + 2).ok_or("ELF read overflow")?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let raw = bytes.get(offset..offset + 4).ok_or("ELF read overflow")?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, &'static str> {
    let raw = bytes.get(offset..offset + 8).ok_or("ELF read overflow")?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

const fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

const fn align_up(value: u64, align: u64) -> u64 {
    value.saturating_add(align - 1) & !(align - 1)
}

fn halt() -> ! {
    loop {
        unsafe { asm!("cli; hlt", options(nomem, nostack)) };
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    lprintln!("[LOADER PANIC] {}", info);
    halt()
}
