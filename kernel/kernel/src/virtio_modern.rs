//! virtio "moderno" por PCI (especificación virtio 1.2, §4.1): lo común a las placas que solo
//! existen en esta versión (virtio-sound; virtio-gpu tiene su copia en virtio_gpu.rs, anterior).
//!
//! Los registros están en memoria: la lista de capacidades PCI dice en qué BAR y desplazamiento
//! están `common_cfg` (estado, características, colas), `notify` (avisar que hay trabajo) y la
//! configuración propia de la placa. Se mapean sin caché (`paging::map_mmio`).
//!
//! Cada cola vive en una página: descriptores en 0, anillo disponible en 256 y usado en 512
//! (alcanza para 16 entradas).

use alloc::alloc::{Layout, alloc_zeroed};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use crate::{paging, pci};

const VENDOR_VIRTIO: u16 = 0x1AF4;
const PCI_CAP_VENDOR: u8 = 0x09;
const CAP_COMMON: u8 = 1;
const CAP_NOTIFY: u8 = 2;
const CAP_DEVICE: u8 = 4;

const DEVICE_FEATURE_SELECT: usize = 0x00;
const DEVICE_FEATURE: usize = 0x04;
const DRIVER_FEATURE_SELECT: usize = 0x08;
const DRIVER_FEATURE: usize = 0x0C;
const DEVICE_STATUS: usize = 0x14;
const QUEUE_SELECT: usize = 0x16;
const QUEUE_SIZE: usize = 0x18;
const QUEUE_ENABLE: usize = 0x1C;
const QUEUE_NOTIFY_OFF: usize = 0x1E;
const QUEUE_DESC: usize = 0x20;
const QUEUE_DRIVER: usize = 0x28;
const QUEUE_DEVICE: usize = 0x30;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const FEATURE_VERSION_1_HI: u32 = 1;

pub const DESC_NEXT: u16 = 1;
pub const DESC_WRITE: u16 = 2;
pub const QUEUE_LEN: u16 = 16;

#[repr(C)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Una página DMA propia (física = virtual − offset: sale del heap, que mapea la RAM).
pub fn dma_page() -> Option<*mut u8> {
    let layout = Layout::from_size_align(4096, 4096).ok()?;
    // SAFETY: tamaño > 0; la página nunca se libera (el driver vive lo que el kernel).
    let p = unsafe { alloc_zeroed(layout) };
    (!p.is_null()).then_some(p)
}

pub struct Transport {
    common: *mut u8,
    pub device_cfg: *mut u8,
    notify_base: *mut u8,
    multiplier: u32,
    pub offset: u64,
    status: u8,
}

// SAFETY: registros mapeados que usa un solo driver; el kernel tiene un hilo.
unsafe impl Send for Transport {}

/// Una cola: los descriptores y los dos anillos en una página.
pub struct Queue {
    page: *mut u8,
    avail: *mut u16,
    used: *mut u16,
    notify: *mut u16,
    avail_idx: u16,
    last_used: u16,
    offset: u64,
}

// SAFETY: ídem `Transport`.
unsafe impl Send for Queue {}

impl Transport {
    /// Busca la placa virtio `device_id` (el de la especificación; en PCI es 0x1040 + id) y la
    /// lleva hasta "características aceptadas". Después: [`queue`](Self::queue) y
    /// [`ready`](Self::ready).
    pub fn init(device_id: u16, offset: u64) -> Option<Transport> {
        let dev = pci::find(VENDOR_VIRTIO, 0x1040 + device_id)?;
        dev.enable_memory_and_dma();
        let (mut common, mut notify, mut device_cfg, mut multiplier) = (None, None, None, 0u32);
        for (id, cap) in dev.capabilities() {
            if id != PCI_CAP_VENDOR {
                continue;
            }
            let cfg_type = dev.read8(cap + 3);
            let bar = dev.read8(cap + 4);
            let off = dev.read(cap + 8) as u64;
            let len = dev.read(cap + 12) as usize;
            let Some(base) = dev.bar_address(bar) else {
                continue;
            };
            match cfg_type {
                CAP_COMMON => common = paging::map_mmio(base + off, len),
                CAP_DEVICE => device_cfg = paging::map_mmio(base + off, len),
                CAP_NOTIFY => {
                    notify = paging::map_mmio(base + off, len.max(4096));
                    multiplier = dev.read(cap + 16);
                }
                _ => {}
            }
        }
        let t = Transport {
            common: common?,
            device_cfg: device_cfg?,
            notify_base: notify?,
            multiplier,
            offset,
            status: 0,
        };
        t.w8(DEVICE_STATUS, 0);
        t.w8(DEVICE_STATUS, STATUS_ACKNOWLEDGE);
        t.w8(DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        t.w32(DEVICE_FEATURE_SELECT, 1);
        if t.r32(DEVICE_FEATURE) & FEATURE_VERSION_1_HI == 0 {
            return None;
        }
        t.w32(DRIVER_FEATURE_SELECT, 0);
        t.w32(DRIVER_FEATURE, 0);
        t.w32(DRIVER_FEATURE_SELECT, 1);
        t.w32(DRIVER_FEATURE, FEATURE_VERSION_1_HI);
        let st = STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK;
        t.w8(DEVICE_STATUS, st);
        if t.r8(DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
            return None;
        }
        Some(Transport { status: st, ..t })
    }

    /// Prepara la cola `index` (hasta 16 entradas).
    pub fn queue(&self, index: u16) -> Option<Queue> {
        self.w16(QUEUE_SELECT, index);
        let max = self.r16(QUEUE_SIZE);
        if max < QUEUE_LEN {
            return None;
        }
        self.w16(QUEUE_SIZE, QUEUE_LEN);
        let page = dma_page()?;
        let q = page as u64 - self.offset;
        self.w64(QUEUE_DESC, q);
        self.w64(QUEUE_DRIVER, q + 256);
        self.w64(QUEUE_DEVICE, q + 512);
        let off = self.r16(QUEUE_NOTIFY_OFF) as usize;
        self.w16(QUEUE_ENABLE, 1);
        // SAFETY: 256 y 512 están dentro de la página de la cola; el desplazamiento de
        // notificación lo da la placa, dentro de la zona mapeada.
        unsafe {
            Some(Queue {
                page,
                avail: page.add(256) as *mut u16,
                used: page.add(512) as *mut u16,
                notify: self.notify_base.add(off * self.multiplier as usize) as *mut u16,
                avail_idx: 0,
                last_used: 0,
                offset: self.offset,
            })
        }
    }

    /// Las colas están listas: la placa puede empezar.
    pub fn ready(&self) {
        self.w8(DEVICE_STATUS, self.status | STATUS_DRIVER_OK);
    }

    /// Un `u32` de la configuración propia de la placa.
    pub fn cfg32(&self, off: usize) -> u32 {
        // SAFETY: `off` es un campo de la configuración de la placa (mapeada, alineada).
        unsafe { read_volatile(self.device_cfg.add(off) as *const u32) }
    }

    fn reg(&self, off: usize) -> *mut u8 {
        // SAFETY: `off` es uno de los registros de `common_cfg` (todos dentro de su zona).
        unsafe { self.common.add(off) }
    }
    fn w8(&self, off: usize, v: u8) {
        // SAFETY: registro de `common_cfg` mapeado sin caché.
        unsafe { write_volatile(self.reg(off), v) }
    }
    fn r8(&self, off: usize) -> u8 {
        // SAFETY: ídem `w8`.
        unsafe { read_volatile(self.reg(off)) }
    }
    fn w16(&self, off: usize, v: u16) {
        // SAFETY: ídem `w8`; alineado.
        unsafe { write_volatile(self.reg(off) as *mut u16, v) }
    }
    fn r16(&self, off: usize) -> u16 {
        // SAFETY: ídem `w16`.
        unsafe { read_volatile(self.reg(off) as *const u16) }
    }
    fn w32(&self, off: usize, v: u32) {
        // SAFETY: ídem `w8`; alineado.
        unsafe { write_volatile(self.reg(off) as *mut u32, v) }
    }
    fn r32(&self, off: usize) -> u32 {
        // SAFETY: ídem `w32`.
        unsafe { read_volatile(self.reg(off) as *const u32) }
    }
    fn w64(&self, off: usize, v: u64) {
        self.w32(off, v as u32);
        self.w32(off + 4, (v >> 32) as u32);
    }
}

impl Queue {
    pub fn phys(&self, virt: *const u8) -> u64 {
        virt as u64 - self.offset
    }

    /// Ofrece a la placa una cadena de buffers (dirección virtual, largo, la placa escribe) que
    /// empieza en el descriptor `head` (usa `head`, `head+1`, …).
    pub fn submit(&mut self, head: u16, bufs: &[(*const u8, u32, bool)]) {
        let n = bufs.len() as u16;
        // SAFETY: los descriptores `head..head+n` están en la página de la cola (n ≤ 16);
        // los buffers son memoria DMA propia del driver.
        unsafe {
            let desc = self.page as *mut Descriptor;
            for (i, (addr, len, write)) in bufs.iter().enumerate() {
                let i = i as u16;
                let mut flags = if *write { DESC_WRITE } else { 0 };
                if i + 1 < n {
                    flags |= DESC_NEXT;
                }
                write_volatile(
                    desc.add((head + i) as usize),
                    Descriptor {
                        addr: self.phys(*addr),
                        len: *len,
                        flags,
                        next: head + i + 1,
                    },
                );
            }
            let slot = 2 + (self.avail_idx % QUEUE_LEN) as usize;
            write_volatile(self.avail.add(slot), head);
            fence(Ordering::SeqCst);
            self.avail_idx = self.avail_idx.wrapping_add(1);
            write_volatile(self.avail.add(1), self.avail_idx);
            fence(Ordering::SeqCst);
            write_volatile(self.notify, 0);
        }
    }

    /// La próxima cadena que la placa terminó: (primer descriptor, bytes escritos).
    pub fn take_used(&mut self) -> Option<(u16, u32)> {
        // SAFETY: el anillo usado está en la página de la cola (4 + 8 × 16 bytes desde 512).
        unsafe {
            if read_volatile(self.used.add(1)) == self.last_used {
                return None;
            }
            fence(Ordering::SeqCst);
            let slot = (self.last_used % QUEUE_LEN) as usize;
            let elem = (self.used as *const u8).add(4 + slot * 8) as *const u32;
            let id = read_volatile(elem) as u16;
            let len = read_volatile(elem.add(1));
            self.last_used = self.last_used.wrapping_add(1);
            Some((id, len))
        }
    }

    /// Espera la próxima cadena terminada (para los pedidos de control).
    pub fn wait_used(&mut self) -> Option<(u16, u32)> {
        for _ in 0..200_000_000u64 {
            if let Some(u) = self.take_used() {
                return Some(u);
            }
            core::hint::spin_loop();
        }
        None
    }
}
