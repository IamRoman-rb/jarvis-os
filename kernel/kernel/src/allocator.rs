//! Heap del kernel: habilita `alloc` (Vec, String, Box).
//!
//! Se toma la región de RAM libre más grande que informa el bootloader y se le entrega a un
//! allocator de lista enlazada (`linked_list_allocator`): guarda los huecos libres en una lista
//! dentro de la misma memoria libre.
//!
//! Para escribir en esa memoria física se usa el **mapeo de toda la memoria física** que arma el
//! bootloader (`physical_memory_offset`): la dirección física `p` se ve en la virtual
//! `offset + p`; desde K8 ese mapeo lo arma el kernel (paging.rs), igual que el del bootloader. Los
//! buffers de DMA salen de acá: su dirección física es `virtual − offset`. Referencia: <https://os.phil-opp.com/heap-allocation/>.

use core::alloc::{GlobalAlloc, Layout};

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use linked_list_allocator::LockedHeap;
use x86_64::instructions::interrupts::without_interrupts;

/// El heap con su lock, pero sin interrupciones mientras se usa (K9): si el timer desalojara a
/// una tarea en medio de `alloc`, las demás darían vueltas esperando el lock (ver irqlock.rs).
struct IrqSafeHeap(LockedHeap);

// SAFETY: delega en `LockedHeap`, que ya es un allocator correcto; solo agrega que no haya
// interrupciones (ni cambios de tarea) mientras tiene el lock.
unsafe impl GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: mismo contrato que el llamador cumple para `GlobalAlloc::alloc`.
        without_interrupts(|| unsafe { self.0.alloc(layout) })
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: mismo contrato que el llamador cumple para `GlobalAlloc::dealloc`.
        without_interrupts(|| unsafe { self.0.dealloc(ptr, layout) })
    }
}

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap(LockedHeap::empty());

/// Tope del heap: los buffers de pantalla (el escritorio hasta 4K, ver `display::capacity`), uno
/// por ventana, y la pila de red.
const MAX_HEAP: u64 = 512 * 1024 * 1024;

/// Devuelve dónde quedó el heap en la memoria física (inicio, tamaño en bytes), o `None` si no
/// hay una región usable. La paginación (paging.rs) no entrega esos marcos.
pub fn init(regions: &MemoryRegions, physical_memory_offset: u64) -> Option<(u64, u64)> {
    let region = regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .max_by_key(|r| r.end - r.start)?;
    let size = (region.end - region.start).min(MAX_HEAP);
    let start = (physical_memory_offset + region.start) as *mut u8;
    // SAFETY: la región es RAM libre según el bootloader (no la usa nadie más), está mapeada en
    // `physical_memory_offset + físico`, y el allocator se inicializa una sola vez.
    unsafe { ALLOCATOR.0.lock().init(start, size as usize) };
    Some((region.start, size))
}

/// (usados, total) del heap en bytes.
pub fn usage() -> (u64, u64) {
    // `try_lock`: si justo lo tiene tomado otra parte del kernel, se informa 0 en vez de esperar.
    match ALLOCATOR.0.try_lock() {
        Some(heap) => (heap.used() as u64, heap.size() as u64),
        None => (0, 0),
    }
}
