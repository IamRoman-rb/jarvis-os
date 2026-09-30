//! GDT (Global Descriptor Table) y TSS (Task State Segment).
//!
//! En modo 64 bits la segmentación casi no se usa, pero la CPU igual necesita una GDT con un
//! segmento de código. Lo importante acá es la TSS: guarda la **Interrupt Stack Table**, una lista
//! de stacks alternativos. El manejador de doble fallo usa uno propio: si el fallo fue justamente
//! que se desbordó el stack, usar el mismo stack causaría un triple fallo y la máquina se reiniciaría
//! sin mostrar nada. Referencia: <https://wiki.osdev.org/GDT> y <https://wiki.osdev.org/TSS>.
//!
//! Desde K11 hay además segmentos de **usuario** (anillo 3) y la TSS guarda `rsp0`: la pila del
//! kernel a la que salta la CPU cuando una interrupción llega mientras corre un programa (en el
//! anillo 3 no se puede usar la pila del programa). Cambia con cada tarea (task.rs). El orden de
//! la GDT no es libre: `syscall`/`sysret` calculan los selectores sumando 8 y 16 a los del MSR
//! STAR, así que van código del kernel, datos del kernel, datos de usuario, código de usuario.

use core::cell::UnsafeCell;

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

pub struct Selectors {
    pub code: SegmentSelector,
    pub data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    tss: SegmentSelector,
}

/// La TSS: se escribe al arrancar y, desde K11, `rsp0` en cada cambio de tarea.
struct TssCell(UnsafeCell<TaskStateSegment>);
// SAFETY: un solo procesador; se escribe al arrancar (antes de cargarla) y después solo `rsp0`,
// con las interrupciones deshabilitadas (`set_kernel_stack`).
unsafe impl Sync for TssCell {}
static TSS: TssCell = TssCell(UnsafeCell::new(TaskStateSegment::new()));
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

/// Los selectores (después de `init`).
pub fn selectors() -> &'static Selectors {
    &GDT.get().expect("GDT sin inicializar").1
}

/// La pila del kernel para las interrupciones que llegan en el anillo 3 (`rsp0` de la TSS).
/// Hay que llamarla con las interrupciones deshabilitadas.
pub fn set_kernel_stack(top: u64) {
    // SAFETY: la CPU solo lee `rsp0` al pasar del anillo 3 al 0, y esto corre sin
    // interrupciones en el único procesador: nadie la lee ni la escribe a la vez. Se escribe un
    // solo campo alineado, a través del `UnsafeCell`.
    unsafe {
        core::ptr::write_volatile(
            &raw mut (*TSS.0.get()).privilege_stack_table[0],
            VirtAddr::new(top),
        )
    };
}

pub fn init() {
    // Los stacks crecen hacia abajo: la IST apunta al final del bloque.
    let start = VirtAddr::from_ptr(&raw const DOUBLE_FAULT_STACK);
    // SAFETY: se llama una vez, al arrancar, antes de cargar la TSS y sin interrupciones: nadie
    // más la usa todavía. La referencia `'static` solo sirve para que el descriptor tome su
    // dirección.
    let tss: &'static TaskStateSegment = unsafe {
        (*TSS.0.get()).interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] =
            start + DOUBLE_FAULT_STACK_SIZE as u64;
        &*TSS.0.get()
    };
    let (gdt, sel) = GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code = gdt.append(Descriptor::kernel_code_segment());
        let data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss = gdt.append(Descriptor::tss_segment(tss));
        (
            gdt,
            Selectors {
                code,
                data,
                user_data,
                user_code,
                tss,
            },
        )
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
