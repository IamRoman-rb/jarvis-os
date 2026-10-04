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

// --- memoria de DMA de 32 bits (K14) ---------------------------------------------------------

/// Tamaño del banco de abajo de 4 GiB: los anillos y buffers de la placa Wi-Fi entran holgados.
pub const LOW_POOL: u64 = 4 * 1024 * 1024;
const FOUR_GIB: u64 = 1 << 32;

/// El banco: [próximo libre, fin) en direcciones físicas; 0 = no hay (el heap ya está abajo de
/// 4 GiB y se usa directo).
static LOW_NEXT: AtomicU64 = AtomicU64::new(0);
static LOW_END: AtomicU64 = AtomicU64::new(0);
static HEAP_LOW: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Elige dónde va el banco de 32 bits, antes de armar la paginación (que no entrega esos
/// marcos). Algunas placas (la RTL8821CE, la RTL8139) solo ven direcciones de 32 bits, y en una
/// PC con mucha RAM el heap (la región más grande) suele quedar arriba de 4 GiB.
pub fn pick_low(usable: impl Iterator<Item = (u64, u64)>, heap: (u64, u64)) -> Option<(u64, u64)> {
    if heap.0 + heap.1 <= FOUR_GIB {
        HEAP_LOW.store(true, Ordering::Relaxed);
        return None;
    }
    usable
        .map(|(s, e)| (s.max(0x10_0000).next_multiple_of(4096), e.min(FOUR_GIB)))
        .filter(|(s, e)| e > s && e - s >= LOW_POOL)
        // Que no se pise con el heap.
        .filter(|(s, e)| *e <= heap.0 || *s >= heap.0 + heap.1)
        .max_by_key(|(s, e)| e - s)
        .map(|(s, _)| (s, s + LOW_POOL))
}

/// Activa el banco elegido por [`pick_low`] (después de `init`).
pub fn init_low(pool: Option<(u64, u64)>) {
    if let Some((start, end)) = pool {
        LOW_NEXT.store(start, Ordering::Relaxed);
        LOW_END.store(end, Ordering::Relaxed);
    }
}

/// Como [`alloc`], pero con dirección física abajo de 4 GiB. `None` si no hay lugar.
pub fn alloc32(size: usize, align: usize) -> Option<*mut u8> {
    if HEAP_LOW.load(Ordering::Relaxed) {
        let p = alloc(size, align)?;
        return (phys(p) + size as u64 <= FOUR_GIB).then_some(p);
    }
    let end = LOW_END.load(Ordering::Relaxed);
    let mut cur = LOW_NEXT.load(Ordering::Relaxed);
    loop {
        if end == 0 {
            return None;
        }
        let start = cur.next_multiple_of(align.max(1) as u64);
        let next = start + size as u64;
        if next > end {
            return None;
        }
        match LOW_NEXT.compare_exchange(cur, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => {
                let ptr = (OFFSET.load(Ordering::Relaxed) + start) as *mut u8;
                // SAFETY: [start, next) es parte del banco: RAM usable que la paginación no
                // entrega y que nadie más usa (el contador avanzó atómicamente), mapeada en
                // `offset + física` como el resto de la RAM.
                unsafe { core::ptr::write_bytes(ptr, 0, size) };
                return Some(ptr);
            }
            Err(now) => cur = now,
        }
    }
}
