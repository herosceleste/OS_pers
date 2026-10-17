use x86_64::registers::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};

struct GdtState {
    table: GlobalDescriptorTable<4>,
    code: SegmentSelector,
    data: SegmentSelector,
}

static GDT: GdtState = {
    let mut table = GlobalDescriptorTable::<4>::empty();
    let code = table.append(Descriptor::kernel_code_segment());
    let data = table.append(Descriptor::kernel_data_segment());
    GdtState { table, code, data }
};

pub fn init() {
    GDT.table.load();

    unsafe {
        CS::set_reg(GDT.code);
        DS::set_reg(GDT.data);
        ES::set_reg(GDT.data);
        SS::set_reg(GDT.data);
    }
}
