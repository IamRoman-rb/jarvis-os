//! GDT (Global Descriptor Table) y TSS (Task State Segment).
//!
//! En modo 64 bits la segmentación casi no se usa, pero la CPU igual necesita una GDT con un
//! segmento de código. Lo importante acá es la TSS: guarda la **Interrupt Stack Table**, una lista
//! de stacks alternativos. El manejador de doble fallo usa uno propio: si el fallo fue justamente
//! que se desbordó el stack, usar el mismo stack causaría un triple fallo y la máquina se reiniciaría
//! sin mostrar nada. Referencia: <https://wiki.osdev.org/GDT> y <https://wiki.osdev.org/TSS>.

use spin::Once;
use x86_64::VirtAddr;
use x86_64::instructions::segmentation::{CS, DS, ES, SS, Segment};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;

/// Índice en la IST del stack para el doble fallo.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 5 * 4096;

/// El stack de emergencia: memoria estática reservada solo para esto.
#[repr(align(16))]
#[allow(dead_code)] // nunca se lee desde Rust: la CPU usa su dirección como stack
struct Stack([u8; DOUBLE_FAULT_STACK_SIZE]);
static mut DOUBLE_FAULT_STACK: Stack = Stack([0; DOUBLE_FAULT_STACK_SIZE]);

struct Selectors {
    code: SegmentSelector,
    data: SegmentSelector,
    tss: SegmentSelector,
}

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

pub fn init() {
    let tss = TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        // Los stacks crecen hacia abajo: la IST apunta al final del bloque.
        let start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            start + DOUBLE_FAULT_STACK_SIZE as u64;
        tss
    });
    let (gdt, sel) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let tss = gdt.append(Descriptor::tss_segment(tss));
        (gdt, Selectors { code, data, tss })
    });
    gdt.load();
    // SAFETY: los selectores apuntan a descriptores válidos de la GDT recién cargada, que es
    // `'static` (vive en un `Once`), así que no se van a invalidar.
    unsafe {
        CS::set_reg(sel.code);
        SS::set_reg(sel.data);
        DS::set_reg(sel.data);
        ES::set_reg(sel.data);
        load_tss(sel.tss);
    }
}
