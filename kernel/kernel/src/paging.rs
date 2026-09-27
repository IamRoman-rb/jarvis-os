//! Mapear registros de dispositivos (MMIO) en la memoria virtual.
//!
//! El bootloader mapea la RAM en `physical_memory_offset`, pero la zona donde la placa de video
//! pone sus registros (un BAR, a veces arriba de los 4 GiB) puede no estar mapeada. Este módulo
//! agrega esas páginas a las tablas de páginas del bootloader, **sin caché**: un registro de un
//! dispositivo se tiene que leer y escribir de verdad cada vez, no desde una copia en la caché.
//!
//! Se mapean en la misma dirección que tendrían en el mapeo de la memoria física
//! (`offset + física`), así la cuenta es la de siempre. Las tablas nuevas salen del heap: está en
//! memoria física contigua y su dirección física es `virtual − offset` (ver `allocator.rs`).
//! La paginación propia completa es K8; esto es lo mínimo para los dispositivos.
//! Referencia: <https://os.phil-opp.com/paging-implementation/>.

use alloc::alloc::{Layout, alloc_zeroed};
use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::{MapToError, TranslateResult};
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
    Translate,
};
use x86_64::{PhysAddr, VirtAddr};

static OFFSET: AtomicU64 = AtomicU64::new(0);

pub fn init(physical_memory_offset: u64) {
    OFFSET.store(physical_memory_offset, Ordering::Relaxed);
}

/// Marcos de 4 KiB para tablas de páginas nuevas, sacados del heap.
struct HeapFrames {
    offset: u64,
}

// SAFETY: cada marco es una asignación nueva del heap, alineada a 4 KiB, que nunca se libera
// (las tablas de páginas viven lo que el kernel), así que no se entrega dos veces.
unsafe impl FrameAllocator<Size4KiB> for HeapFrames {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let layout = Layout::from_size_align(4096, 4096).ok()?;
        // SAFETY: el layout no es de tamaño cero.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            return None;
        }
        PhysFrame::from_start_address(PhysAddr::new(ptr as u64 - self.offset)).ok()
    }
}

/// Mapea `size` bytes de registros que empiezan en la dirección física `phys`. Devuelve el
/// puntero virtual. Si ya estaban mapeados (dentro del mapeo de la memoria física), se usan así.
pub fn map_mmio(phys: u64, size: usize) -> Option<*mut u8> {
    let offset = OFFSET.load(Ordering::Relaxed);
    if offset == 0 || size == 0 {
        return None;
    }
    let (level4, _) = Cr3::read();
    let table_virt = VirtAddr::new(offset + level4.start_address().as_u64());
    // SAFETY: el bootloader mapeó toda la memoria física en `offset`, así que la tabla de nivel 4
    // activa está en `offset + física`; nadie más la modifica mientras tanto (un solo hilo).
    let table: &mut PageTable = unsafe { &mut *table_virt.as_mut_ptr() };
    // SAFETY: ídem: toda la memoria física está mapeada en `offset`.
    let mut mapper = unsafe { OffsetPageTable::new(table, VirtAddr::new(offset)) };
    let mut frames = HeapFrames { offset };
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH
        | PageTableFlags::NO_EXECUTE;
    let first = phys & !0xFFF;
    let last = (phys + size as u64 - 1) & !0xFFF;
    let mut frame_addr = first;
    while frame_addr <= last {
        let virt = VirtAddr::new(offset + frame_addr);
        if let TranslateResult::NotMapped = mapper.translate(virt) {
            let page: Page<Size4KiB> = Page::containing_address(virt);
            let frame = PhysFrame::containing_address(PhysAddr::new(frame_addr));
            // SAFETY: la página estaba libre y el marco es de un dispositivo (no es RAM que use
            // otra parte del kernel).
            match unsafe { mapper.map_to(page, frame, flags, &mut frames) } {
                Ok(flush) => flush.flush(),
                // Una página enorme del mapeo de la memoria física ya lo cubre.
                Err(MapToError::ParentEntryHugePage) | Err(MapToError::PageAlreadyMapped(_)) => {}
                Err(MapToError::FrameAllocationFailed) => return None,
            }
        }
        frame_addr += 4096;
    }
    Some((offset + phys) as *mut u8)
}
