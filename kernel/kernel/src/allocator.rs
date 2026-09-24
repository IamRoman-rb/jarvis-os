//! Heap del kernel: habilita `alloc` (Vec, String, Box).
//!
//! Se toma la región de RAM libre más grande que informa el bootloader y se le entrega a un
//! allocator de lista enlazada (`linked_list_allocator`): guarda los huecos libres en una lista
//! dentro de la misma memoria libre.
//!
//! Para escribir en esa memoria física se usa el **mapeo de toda la memoria física** que arma el
//! bootloader (`physical_memory_offset`): la dirección física `p` se ve en la virtual
//! `offset + p`. La paginación propia llega más adelante (ver docs/kernel.md). Referencia: <https://os.phil-opp.com/heap-allocation/>.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use linked_list_allocator::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Tope del heap: los buffers de pantalla, uno por ventana, las páginas web y la pila de red.
const MAX_HEAP: u64 = 256 * 1024 * 1024;

/// Devuelve el tamaño del heap en bytes, o `None` si no hay una región usable.
pub fn init(regions: &MemoryRegions, physical_memory_offset: u64) -> Option<u64> {
    let region = regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable)
        .max_by_key(|r| r.end - r.start)?;
    let size = (region.end - region.start).min(MAX_HEAP);
    let start = (physical_memory_offset + region.start) as *mut u8;
    // SAFETY: la región es RAM libre según el bootloader (no la usa nadie más), está mapeada en
    // `physical_memory_offset + físico`, y el allocator se inicializa una sola vez.
    unsafe { ALLOCATOR.lock().init(start, size as usize) };
    Some(size)
}

/// (usados, total) del heap en bytes.
pub fn usage() -> (u64, u64) {
    // `try_lock`: si justo lo tiene tomado otra parte del kernel, se informa 0 en vez de esperar.
    match ALLOCATOR.try_lock() {
        Some(heap) => (heap.used() as u64, heap.size() as u64),
        None => (0, 0),
    }
}
