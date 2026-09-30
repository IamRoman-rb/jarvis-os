//! Registros de un dispositivo mapeados en memoria (MMIO), para los drivers de K13.
//!
//! Los dispositivos PCI Express exponen sus registros en un BAR de memoria: leer o escribir esas
//! direcciones habla con el dispositivo, no con la RAM. Por eso cada acceso es `volatile` (el
//! compilador no lo puede omitir ni reordenar ni juntar) y la zona se mapea sin caché
//! (`paging::map_mmio`).

use crate::paging;

#[derive(Clone, Copy)]
pub struct Mmio {
    base: *mut u8,
    size: usize,
}

// SAFETY: `base` apunta a registros de un dispositivo que maneja un solo driver; el puntero se
// puede mandar a la tarea que use ese driver.
unsafe impl Send for Mmio {}

macro_rules! access {
    ($read:ident, $write:ident, $t:ty) => {
        pub fn $read(&self, offset: usize) -> $t {
            assert!(offset + core::mem::size_of::<$t>() <= self.size);
            // SAFETY: `offset` está dentro de la zona mapeada (lo verifica el assert) y los
            // registros de este tamaño están alineados a su tamaño (lo exige toda
            // especificación de hardware que usa el kernel).
            unsafe { core::ptr::read_volatile(self.base.add(offset) as *const $t) }
        }

        pub fn $write(&self, offset: usize, value: $t) {
            assert!(offset + core::mem::size_of::<$t>() <= self.size);
            // SAFETY: ídem.
            unsafe { core::ptr::write_volatile(self.base.add(offset) as *mut $t, value) }
        }
    };
}

impl Mmio {
    /// Mapea `size` bytes de registros desde la dirección física `phys`.
    pub fn map(phys: u64, size: usize) -> Option<Mmio> {
        Some(Mmio {
            base: paging::map_mmio(phys, size)?,
            size,
        })
    }

    /// Una parte de la zona (los registros de un puerto, de una cola…).
    pub fn sub(&self, offset: usize, size: usize) -> Mmio {
        assert!(offset + size <= self.size);
        Mmio {
            // SAFETY: `offset + size` está dentro de la zona mapeada (lo verifica el assert).
            base: unsafe { self.base.add(offset) },
            size,
        }
    }

    access!(r32, w32, u32);
    access!(r64, w64, u64);
}

/// Espera a que `done()` dé `true`, hasta `timeout_ms`. Con interrupción (`irq`) y si la tarea
/// puede dormir, duerme hasta el evento `event` (o 10 ms, por si se perdió); si no, revisa dando
/// vueltas. `false` si se venció el plazo.
pub fn wait_until(event: u32, irq: bool, timeout_ms: u64, mut done: impl FnMut() -> bool) -> bool {
    let start = crate::time::millis();
    loop {
        if done() {
            return true;
        }
        let now = crate::time::millis();
        if now.saturating_sub(start) > timeout_ms {
            return done();
        }
        if irq && crate::task::can_block() {
            crate::task::wait(event, Some(now + 10));
        } else {
            core::hint::spin_loop();
        }
    }
}
