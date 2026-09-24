//! Heap del kernel: habilita `alloc` (Vec, String, Box).
//!
//! Se toma la región de RAM libre más grande que informa el bootloader y se le entrega a un
//! allocator de lista enlazada (`linked_list_allocator`): guarda los huecos libres en una lista
//! dentro de la misma memoria libre.
//!
//! Para escribir en esa memoria física se usa el **mapeo de toda la memoria física** que arma el
//! bootloader (`physical_memory_offset`): la dirección física `p` se ve en la virtual
//! `offset + p`. La paginación propia llega en K2. Referencia: <https://os.phil-opp.com/heap-allocation/>.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use linked_list_allocator::LockedHeap;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Tope del heap. Alcanza de sobra para los buffers de pantalla y las partículas.
const MAX_HEAP: u64 = 64 * 1024 * 1024;

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
