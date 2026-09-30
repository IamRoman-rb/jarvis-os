//! Memoria para DMA de los drivers de K13: buffers que un dispositivo lee o escribe por su
//! cuenta, sin pasar por la CPU.
//!
//! El dispositivo solo entiende direcciones **físicas**. El heap del kernel es una región de RAM
//! física contigua, mapeada en `offset + física` (allocator.rs), así que un buffer del heap
//! alineado a su tamaño de página es contiguo también en la memoria física, y su dirección
//! física es `virtual − offset` (regla 14 de CLAUDE.md). Los buffers de DMA nunca se liberan:
//! los drivers viven lo que vive el kernel.

use alloc::alloc::{Layout, alloc_zeroed};
use core::sync::atomic::{AtomicU64, Ordering};

static OFFSET: AtomicU64 = AtomicU64::new(0);

/// Anota dónde está mapeada la memoria física (una vez, al arrancar).
pub fn init(physical_memory_offset: u64) {
    OFFSET.store(physical_memory_offset, Ordering::Relaxed);
}

/// `size` bytes en cero, alineados a `align` (potencia de 2).
pub fn alloc(size: usize, align: usize) -> Option<*mut u8> {
    let layout = Layout::from_size_align(size.max(1), align).ok()?;
    // SAFETY: el tamaño es > 0; la memoria no se libera nunca (ver el comentario del módulo).
    let ptr = unsafe { alloc_zeroed(layout) };
    (!ptr.is_null()).then_some(ptr)
}

/// La dirección física de un buffer del heap.
pub fn phys(ptr: *const u8) -> u64 {
    ptr as u64 - OFFSET.load(Ordering::Relaxed)
}
