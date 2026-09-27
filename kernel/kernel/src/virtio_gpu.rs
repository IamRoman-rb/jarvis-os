//! Driver de la placa de video virtio-gpu (interfaz virtio **moderna**, por memoria).
//!
//! La pantalla del arranque es un framebuffer que da el firmware (GOP): una sola pantalla, de
//! resolución fija. Para tener **varios monitores** hace falta hablarle a la placa. En QEMU,
//! `virtio-vga` es una virtio-gpu que además arranca como una VGA común (así el firmware y el
//! bootloader tienen dónde dibujar). Cuando este driver toma el control, cada salida
//! ("scanout") muestra la parte que se le indique de una imagen en la RAM.
//!
//! El modelo 2D de virtio-gpu:
//! 1. `GET_DISPLAY_INFO`: cuántas salidas hay y de qué tamaño (en QEMU, una ventana por salida).
//! 2. `RESOURCE_CREATE_2D` + `ATTACH_BACKING`: una imagen del host respaldada por memoria nuestra
//!    (el `frame` que ya compone el escritorio).
//! 3. `SET_SCANOUT`: qué rectángulo de esa imagen muestra cada salida. Así se hace "extender"
//!    (cada una muestra su parte) o "duplicar" (las dos muestran lo mismo).
//! 4. En cada frame, `TRANSFER_TO_HOST_2D` + `RESOURCE_FLUSH` de lo que cambió.
//!
//! A diferencia del disco y la red (virtio "legacy", por puertos), virtio-gpu solo existe en la
//! versión moderna: sus registros están en memoria (un BAR), en lugares que se leen de la lista de
//! capacidades PCI. Se mapean sin caché con `paging::map_mmio`.
//! Referencias: especificación virtio 1.2 (4.1.4 "Virtio Structure PCI Capabilities" y 5.7 "GPU
//! Device") y <https://wiki.osdev.org/Virtio>.

use alloc::alloc::{Layout, alloc_zeroed};
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use crate::{paging, pci, serial_println};

const VENDOR_VIRTIO: u16 = 0x1AF4;
/// virtio-gpu (y virtio-vga): 0x1040 + 16.
const DEVICE_GPU: u16 = 0x1050;

// Capacidades virtio en el espacio de configuración PCI (id 0x09 = "del fabricante").
const PCI_CAP_VENDOR: u8 = 0x09;
const CAP_COMMON: u8 = 1;
const CAP_NOTIFY: u8 = 2;
const CAP_DEVICE: u8 = 4;
/// En la configuración propia de virtio-gpu: cuántas salidas tiene (`num_scanouts`).
const CFG_NUM_SCANOUTS: usize = 8;

// Registros de `common_cfg` (desplazamientos).
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
/// VIRTIO_F_VERSION_1 (bit 32): "hablo virtio moderno". Es la única que pedimos.
const FEATURE_VERSION_1_HI: u32 = 1;

const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

// Comandos y respuestas de virtio-gpu.
const CMD_GET_DISPLAY_INFO: u32 = 0x0100;
const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_RESOURCE_UNREF: u32 = 0x0102;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const RESP_OK_NODATA: u32 = 0x1100;
const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
/// Píxeles de 4 bytes en orden B, G, R, X: lo mismo que el `frame` (formato Bgr de 4 bytes).
const FORMAT_B8G8R8X8: u32 = 2;
pub const MAX_SCANOUTS: usize = 16;

const QUEUE_LEN: usize = 16;
const POLL_LIMIT: u64 = 200_000_000;

#[repr(C)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Una salida (un monitor): su tamaño preferido y si está conectada.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
}

pub struct VirtioGpu {
    common: *mut u8,
    /// Configuración propia de la placa (cuántas salidas).
    device_cfg: *mut u8,
    notify: *mut u16,
    /// Cola de control: descriptores, anillo disponible y anillo usado, en una página.
    queue: *mut u8,
    avail: *mut u16,
    used: *mut u16,
    avail_idx: u16,
    last_used: u16,
    /// Pedido y respuesta (memoria DMA).
    req: *mut u8,
    resp: *mut u8,
    offset: u64,
    /// Recurso de imagen actual (id, ancho, alto).
    resource: Option<(u32, u32, u32)>,
    next_resource: u32,
}

// SAFETY: los punteros son de memoria del heap o de registros mapeados que solo usa este driver;
// el kernel tiene un solo hilo y el driver no se usa desde interrupciones.
unsafe impl Send for VirtioGpu {}

fn dma_page() -> Option<*mut u8> {
    let layout = Layout::from_size_align(4096, 4096).ok()?;
    // SAFETY: tamaño > 0; la página nunca se libera (el driver vive lo que el kernel).
    let p = unsafe { alloc_zeroed(layout) };
    (!p.is_null()).then_some(p)
}

impl VirtioGpu {
    /// Busca la placa, la inicializa y prepara la cola de control. `None` si no hay (una PC real
    /// o QEMU con la VGA común): entonces sigue el framebuffer del firmware.
    pub fn init(offset: u64) -> Option<VirtioGpu> {
        let dev = pci::find(VENDOR_VIRTIO, DEVICE_GPU)?;
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
        let (Some(common), Some(notify_base), Some(device_cfg)) = (common, notify, device_cfg)
        else {
            serial_println!("virtio-gpu: sin registros modernos (capacidades)");
            return None;
        };
        let page = dma_page()?;
        let mut gpu = VirtioGpu {
            common,
            device_cfg,
            notify: core::ptr::null_mut(),
            queue: page,
            avail: core::ptr::null_mut(),
            used: core::ptr::null_mut(),
            avail_idx: 0,
            last_used: 0,
            req: dma_page()?,
            resp: dma_page()?,
            offset,
            resource: None,
            next_resource: 1,
        };
        // Arranque: reset → "te vi" → "tengo driver" → características → "listo".
        gpu.w8(DEVICE_STATUS, 0);
        gpu.w8(DEVICE_STATUS, STATUS_ACKNOWLEDGE);
        gpu.w8(DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        gpu.w32(DEVICE_FEATURE_SELECT, 1);
        if gpu.r32(DEVICE_FEATURE) & FEATURE_VERSION_1_HI == 0 {
            serial_println!("virtio-gpu: no es un dispositivo moderno");
            return None;
        }
        gpu.w32(DRIVER_FEATURE_SELECT, 0);
        gpu.w32(DRIVER_FEATURE, 0);
        gpu.w32(DRIVER_FEATURE_SELECT, 1);
        gpu.w32(DRIVER_FEATURE, FEATURE_VERSION_1_HI);
        let st = STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK;
        gpu.w8(DEVICE_STATUS, st);
        if gpu.r8(DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
            serial_println!("virtio-gpu: no aceptó las características");
            return None;
        }
        // Cola 0 (control): descriptores en 0, anillo disponible en 256, usado en 512.
        gpu.w16(QUEUE_SELECT, 0);
        let max = gpu.r16(QUEUE_SIZE) as usize;
        if max == 0 {
            return None;
        }
        let n = QUEUE_LEN.min(max);
        gpu.w16(QUEUE_SIZE, n as u16);
        // SAFETY: `queue` es una página propia de 4 KiB; 256 y 512 quedan adentro y alineados.
        unsafe {
            gpu.avail = gpu.queue.add(256) as *mut u16;
            gpu.used = gpu.queue.add(512) as *mut u16;
        }
        let q = gpu.phys(gpu.queue);
        gpu.w64(QUEUE_DESC, q);
        gpu.w64(QUEUE_DRIVER, q + 256);
        gpu.w64(QUEUE_DEVICE, q + 512);
        let notify_off = gpu.r16(QUEUE_NOTIFY_OFF) as usize;
        // SAFETY: el desplazamiento lo da el dispositivo dentro de la zona de notificación mapeada.
        gpu.notify = unsafe { notify_base.add(notify_off * multiplier as usize) } as *mut u16;
        gpu.w16(QUEUE_ENABLE, 1);
        gpu.w8(DEVICE_STATUS, st | STATUS_DRIVER_OK);
        serial_println!(
            "virtio-gpu: lista en PCI {:02x}:{:02x}.{} (cola de {})",
            dev.bus,
            dev.slot,
            dev.function,
            n
        );
        Some(gpu)
    }

    fn phys(&self, virt: *const u8) -> u64 {
        virt as u64 - self.offset
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
        // SAFETY: ídem `w8`; los registros de 16 bits están alineados.
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
    /// Los registros de 64 bits se escriben en dos mitades (la especificación lo permite).
    fn w64(&self, off: usize, v: u64) {
        self.w32(off, v as u32);
        self.w32(off + 4, (v >> 32) as u32);
    }

    /// Manda el pedido de `req_len` bytes que está en `req` y espera la respuesta (`resp_len`
    /// bytes en `resp`). Devuelve el tipo de respuesta.
    fn command(&mut self, req_len: usize, resp_len: usize) -> Option<u32> {
        let n = QUEUE_LEN as u16;
        // SAFETY: descriptores 0 y 1 y los anillos están en la página propia de la cola; `req` y
        // `resp` son páginas DMA propias.
        unsafe {
            let desc = self.queue as *mut Descriptor;
            write_volatile(
                desc,
                Descriptor {
                    addr: self.phys(self.req),
                    len: req_len as u32,
                    flags: DESC_NEXT,
                    next: 1,
                },
            );
            write_volatile(
                desc.add(1),
                Descriptor {
                    addr: self.phys(self.resp),
                    len: resp_len as u32,
                    flags: DESC_WRITE,
                    next: 0,
                },
            );
            write_volatile(self.avail.add(2 + (self.avail_idx % n) as usize), 0);
            fence(Ordering::SeqCst);
            self.avail_idx = self.avail_idx.wrapping_add(1);
            write_volatile(self.avail.add(1), self.avail_idx);
            fence(Ordering::SeqCst);
            write_volatile(self.notify, 0);
            let mut spins = 0u64;
            while read_volatile(self.used.add(1)) == self.last_used {
                spins += 1;
                if spins > POLL_LIMIT {
                    serial_println!("virtio-gpu: la placa no respondió");
                    return None;
                }
                core::hint::spin_loop();
            }
            self.last_used = self.last_used.wrapping_add(1);
            fence(Ordering::SeqCst);
            Some(read_volatile(self.resp as *const u32))
        }
    }

    /// Arma un pedido: cabecera (tipo) + los `u32` del cuerpo. Devuelve el largo.
    fn put(&mut self, kind: u32, body: &[u32]) -> usize {
        // SAFETY: `req` es una página propia; cabecera (24 bytes) + cuerpo < 4 KiB.
        unsafe {
            core::ptr::write_bytes(self.req, 0, 24);
            write_volatile(self.req as *mut u32, kind);
            let b = self.req.add(24) as *mut u32;
            for (i, v) in body.iter().enumerate() {
                write_volatile(b.add(i), *v);
            }
        }
        24 + body.len() * 4
    }

    fn simple(&mut self, kind: u32, body: &[u32]) -> bool {
        let len = self.put(kind, body);
        self.command(len, 24) == Some(RESP_OK_NODATA)
    }

    /// Las salidas (monitores) que tiene la placa.
    pub fn outputs(&mut self) -> Vec<Output> {
        let len = self.put(CMD_GET_DISPLAY_INFO, &[]);
        let resp_len = 24 + MAX_SCANOUTS * 24;
        if self.command(len, resp_len) != Some(RESP_OK_DISPLAY_INFO) {
            return Vec::new();
        }
        // SAFETY: la configuración de virtio-gpu tiene `num_scanouts` en el byte 8 (alineado).
        let n =
            unsafe { read_volatile(self.device_cfg.add(CFG_NUM_SCANOUTS) as *const u32) } as usize;
        let mut out = Vec::new();
        for i in 0..n.clamp(1, MAX_SCANOUTS) {
            // SAFETY: la respuesta tiene 16 entradas de 24 bytes después de la cabecera.
            let (w, h, enabled) = unsafe {
                let p = self.resp.add(24 + i * 24) as *const u32;
                (
                    read_volatile(p.add(2)),
                    read_volatile(p.add(3)),
                    read_volatile(p.add(4)),
                )
            };
            out.push(Output {
                width: w,
                height: h,
                enabled: enabled != 0,
            });
        }
        // Una salida que QEMU todavía no conectó no informa tamaño: el de la primera (se conecta
        // cuando se le da una imagen).
        let first = out.first().copied().unwrap_or_default();
        for o in &mut out {
            if o.width == 0 || o.height == 0 {
                o.width = if first.width > 0 { first.width } else { 1280 };
                o.height = if first.height > 0 { first.height } else { 800 };
            }
        }
        out
    }

    /// Crea la imagen del escritorio (`width` × `height`, respaldada por `backing`, que tiene que
    /// estar en el heap) y descarta la anterior.
    pub fn set_image(&mut self, backing: *const u8, width: u32, height: u32) -> bool {
        if let Some((old, _, _)) = self.resource.take() {
            self.simple(CMD_RESOURCE_UNREF, &[old, 0]);
        }
        let id = self.next_resource;
        self.next_resource += 1;
        if !self.simple(
            CMD_RESOURCE_CREATE_2D,
            &[id, FORMAT_B8G8R8X8, width, height],
        ) {
            return false;
        }
        let addr = self.phys(backing);
        let bytes = width * height * 4;
        let ok = self.simple(
            CMD_RESOURCE_ATTACH_BACKING,
            &[id, 1, addr as u32, (addr >> 32) as u32, bytes, 0],
        );
        if ok {
            self.resource = Some((id, width, height));
        }
        ok
    }

    /// Qué parte de la imagen muestra la salida `scanout` (`None` = apagada).
    pub fn show(&mut self, scanout: u32, rect: Option<(u32, u32, u32, u32)>) -> bool {
        let id = self.resource.map_or(0, |r| r.0);
        match rect {
            Some((x, y, w, h)) => self.simple(CMD_SET_SCANOUT, &[x, y, w, h, scanout, id]),
            None => self.simple(CMD_SET_SCANOUT, &[0, 0, 0, 0, scanout, 0]),
        }
    }

    /// Copia al host y muestra un rectángulo de la imagen que cambió.
    pub fn flush(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let Some((id, rw, rh)) = self.resource else {
            return;
        };
        let (x, y) = (x.min(rw), y.min(rh));
        let (w, h) = (w.min(rw - x), h.min(rh - y));
        if w == 0 || h == 0 {
            return;
        }
        let offset = (y as u64 * rw as u64 + x as u64) * 4;
        self.simple(
            CMD_TRANSFER_TO_HOST_2D,
            &[x, y, w, h, offset as u32, (offset >> 32) as u32, id, 0],
        );
        self.simple(CMD_RESOURCE_FLUSH, &[x, y, w, h, id, 0]);
    }
}
