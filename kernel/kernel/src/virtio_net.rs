//! Driver de placa de red virtio-net (interfaz "legacy" por puertos de E/S).
//!
//! Es el mismo mecanismo que el disco (`virtio_blk.rs`), pero con **dos** virtqueues:
//! - **recepción** (cola 0): el kernel le deja al dispositivo buffers vacíos; cuando llega un
//!   paquete, el dispositivo lo escribe en uno y lo anota en el anillo "usado".
//! - **transmisión** (cola 1): el kernel deja un paquete armado y "toca el timbre".
//!
//! Cada paquete va precedido de una cabecera virtio de 10 bytes (acá, toda en cero: sin
//! descarga de checksums ni segmentación). El resto es una trama Ethernet que arma y entiende la
//! pila TCP/IP (smoltcp): este driver implementa su trait `Device`.
//!
//! Cuando llega un paquete, el dispositivo sube su línea PCI y la interrupción despierta a la
//! tarea de la red (K9, nettask.rs). Las transmisiones terminadas no interrumpen (no hace falta:
//! los buffers se recuperan al mandar el próximo). Los buffers viven en el heap (DMA: física =
//! virtual − offset). Referencia: especificación virtio 1.x, "Network Device" e "Legacy Interface".

use alloc::alloc::{Layout, alloc_zeroed};
use alloc::vec::Vec;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering, fence};

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;
use x86_64::instructions::port::Port;

use crate::{pci, serial_println};

const VENDOR_VIRTIO: u16 = 0x1AF4;
/// virtio-net "transitional" (con interfaz legacy).
const DEVICE_NET_LEGACY: u16 = 0x1000;

const REG_DEVICE_FEATURES: u16 = 0x00;
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_ADDRESS: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SELECT: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
/// Registro ISR: leerlo dice si la placa avisó y baja la interrupción.
const REG_ISR: u16 = 0x13;
/// En el anillo disponible: "no me interrumpas cuando uses esto".
const AVAIL_NO_INTERRUPT: u16 = 1;
/// Configuración del dispositivo: acá empieza la MAC.
const REG_MAC: u16 = 0x14;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
/// Característica: la MAC está en la configuración.
const FEATURE_MAC: u32 = 1 << 5;

const DESC_WRITE: u16 = 2;
const HEADER: usize = 10;
const BUF_SIZE: usize = 2048;
const MTU: usize = 1514;

/// Bytes recibidos y enviados desde el arranque (para el monitor).
pub static RX_BYTES: AtomicU64 = AtomicU64::new(0);
pub static TX_BYTES: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Una virtqueue con un buffer propio por descriptor.
struct Queue {
    index: u16,
    size: usize,
    base: *mut u8,
    used_offset: usize,
    buffers: *mut u8,
    avail_idx: u16,
    last_used: u16,
}

fn align_up(v: usize, align: usize) -> usize {
    v.div_ceil(align) * align
}

fn dma_alloc(size: usize) -> Option<*mut u8> {
    let layout = Layout::from_size_align(align_up(size, 4096), 4096).ok()?;
    // SAFETY: tamaño > 0; la memoria no se libera nunca (el driver vive lo que el kernel).
    let ptr = unsafe { alloc_zeroed(layout) };
    (!ptr.is_null()).then_some(ptr)
}

impl Queue {
    fn desc(&self, i: usize) -> *mut Descriptor {
        // La tabla de descriptores está al principio; `wrapping_add` no necesita `unsafe` (usar
        // el puntero sí: quien lo usa garantiza i < size).
        (self.base as *mut Descriptor).wrapping_add(i)
    }

    fn buffer(&self, i: usize) -> *mut u8 {
        // Hay `size` buffers de BUF_SIZE bytes (quien usa el puntero garantiza i < size).
        self.buffers.wrapping_add(i * BUF_SIZE)
    }

    fn avail(&self) -> *mut u16 {
        // SAFETY: el anillo disponible va justo después de los descriptores.
        unsafe { self.base.add(16 * self.size) as *mut u16 }
    }

    fn used_idx(&self) -> u16 {
        // SAFETY: el anillo usado empieza en `used_offset`; su índice está en el byte 2.
        unsafe { read_volatile(self.base.add(self.used_offset + 2) as *const u16) }
    }

    /// (id, largo) de la entrada `n` del anillo usado.
    fn used_elem(&self, n: u16) -> (usize, usize) {
        let slot = n as usize % self.size;
        // SAFETY: cada entrada del anillo usado son dos u32 (id, largo) a partir del byte 4.
        unsafe {
            let p = self.base.add(self.used_offset + 4 + slot * 8) as *const u32;
            (read_volatile(p) as usize, read_volatile(p.add(1)) as usize)
        }
    }

    /// Publica el descriptor `i` en el anillo disponible.
    fn publish(&mut self, i: usize) {
        let avail = self.avail();
        // SAFETY: la entrada del anillo es `avail_idx % size` (en rango) y el índice va en [1].
        unsafe {
            write_volatile(avail.add(2 + self.avail_idx as usize % self.size), i as u16);
            fence(Ordering::SeqCst); // la entrada antes que el índice
            self.avail_idx = self.avail_idx.wrapping_add(1);
            write_volatile(avail.add(1), self.avail_idx);
            fence(Ordering::SeqCst);
        }
    }
}

pub struct VirtioNet {
    io_base: u16,
    rx: Queue,
    tx: Queue,
    /// Próximo buffer de transmisión a usar y cuántos están en vuelo.
    tx_next: usize,
    tx_in_flight: usize,
    mac: [u8; 6],
    physical_memory_offset: u64,
    /// Línea PCI por la que avisa (la eligió el firmware).
    irq_line: Option<u8>,
    /// El dispositivo PCI (con APIC, su ruta de interrupción sale de `_PRT`).
    pci: pci::Device,
}

// SAFETY: los punteros son memoria del heap que solo usa este driver, y lo usa una sola tarea
// (la de la red, K9). La interrupción solo lee el registro ISR.
unsafe impl Send for VirtioNet {}

impl VirtioNet {
    pub fn init(physical_memory_offset: u64) -> Option<Self> {
        let dev = pci::find(VENDOR_VIRTIO, DEVICE_NET_LEGACY)?;
        let bar0 = dev.bar(0);
        if bar0 & 1 == 0 {
            serial_println!("virtio-net: el BAR0 no es de E/S (¿falta disable-modern=on?)");
            return None;
        }
        dev.enable_io_and_dma();
        let io_base = (bar0 & 0xFFFC) as u16;
        let mut net = VirtioNet {
            io_base,
            rx: empty_queue(),
            tx: empty_queue(),
            tx_next: 0,
            tx_in_flight: 0,
            mac: [0; 6],
            physical_memory_offset,
            irq_line: dev.interrupt_line(),
            pci: dev,
        };
        net.out8(REG_STATUS, 0);
        net.out8(REG_STATUS, STATUS_ACKNOWLEDGE);
        net.out8(REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        let offered = net.in32(REG_DEVICE_FEATURES);
        net.out32(REG_GUEST_FEATURES, offered & FEATURE_MAC);

        net.rx = net.setup_queue(0)?;
        net.tx = net.setup_queue(1)?;
        // SAFETY: el primer u16 del anillo disponible son sus flags (la cola es propia).
        unsafe { write_volatile(net.tx.avail(), AVAIL_NO_INTERRUPT) };
        // Todos los buffers de recepción quedan a disposición del dispositivo.
        for i in 0..net.rx.size {
            let addr = net.phys(net.rx.buffer(i));
            // SAFETY: i < size (descriptor propio de la cola de recepción).
            unsafe {
                write_volatile(
                    net.rx.desc(i),
                    Descriptor {
                        addr,
                        len: BUF_SIZE as u32,
                        flags: DESC_WRITE,
                        next: 0,
                    },
                );
            }
            net.rx.publish(i);
        }
        net.out8(
            REG_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK,
        );
        net.out16(REG_QUEUE_NOTIFY, 0);

        if offered & FEATURE_MAC != 0 {
            for (i, b) in net.mac.iter_mut().enumerate() {
                // SAFETY: la MAC está en la configuración del dispositivo, 6 bytes desde 0x14.
                *b = unsafe { Port::new(io_base + REG_MAC + i as u16).read() };
            }
        } else {
            net.mac = [0x02, 0x4a, 0x41, 0x52, 0x56, 0x53]; // "JARVS", administrada localmente
        }
        serial_println!(
            "virtio-net: placa de red en PCI {:02x}:{:02x}.{}, MAC {}",
            dev.bus,
            dev.slot,
            dev.function,
            jarvis_net::mac_string(net.mac)
        );
        Some(net)
    }

    pub fn mac(&self) -> [u8; 6] {
        self.mac
    }

    /// (dispositivo, línea, puerto del registro ISR), para anotarlo en interrupts.rs.
    pub fn irq(&self) -> Option<(pci::Device, u8, u16)> {
        Some((self.pci, self.irq_line?, self.io_base + REG_ISR))
    }

    fn setup_queue(&mut self, index: u16) -> Option<Queue> {
        self.out16(REG_QUEUE_SELECT, index);
        let size = self.in16(REG_QUEUE_SIZE) as usize;
        if size == 0 {
            serial_println!("virtio-net: la cola {} no existe", index);
            return None;
        }
        let avail_end = 16 * size + 6 + 2 * size;
        let used_offset = align_up(avail_end, 4096);
        let total = used_offset + align_up(6 + 8 * size, 4096);
        let base = dma_alloc(total)?;
        let buffers = dma_alloc(size * BUF_SIZE)?;
        let pfn = ((base as u64 - self.physical_memory_offset) >> 12) as u32;
        self.out32(REG_QUEUE_ADDRESS, pfn);
        Some(Queue {
            index,
            size,
            base,
            used_offset,
            buffers,
            avail_idx: 0,
            last_used: 0,
        })
    }

    fn phys(&self, virt: *const u8) -> u64 {
        virt as u64 - self.physical_memory_offset
    }

    fn out8(&self, reg: u16, v: u8) {
        // SAFETY: `io_base` es el BAR0 de E/S del dispositivo virtio (lo dio el bus PCI).
        unsafe { Port::new(self.io_base + reg).write(v) }
    }
    fn out16(&self, reg: u16, v: u16) {
        // SAFETY: ídem `out8`.
        unsafe { Port::new(self.io_base + reg).write(v) }
    }
    fn out32(&self, reg: u16, v: u32) {
        // SAFETY: ídem `out8`.
        unsafe { Port::new(self.io_base + reg).write(v) }
    }
    fn in16(&self, reg: u16) -> u16 {
        // SAFETY: ídem `out8`.
        unsafe { Port::new(self.io_base + reg).read() }
    }
    fn in32(&self, reg: u16) -> u32 {
        // SAFETY: ídem `out8`.
        unsafe { Port::new(self.io_base + reg).read() }
    }

    /// Libera los buffers de transmisión que el dispositivo ya mandó.
    fn reclaim_tx(&mut self) {
        while self.tx.used_idx() != self.tx.last_used {
            self.tx.last_used = self.tx.last_used.wrapping_add(1);
            self.tx_in_flight = self.tx_in_flight.saturating_sub(1);
        }
    }

    /// Un paquete recibido (sin la cabecera virtio), si hay.
    fn recv_frame(&mut self) -> Option<Vec<u8>> {
        if self.rx.used_idx() == self.rx.last_used {
            return None;
        }
        fence(Ordering::SeqCst);
        let (id, len) = self.rx.used_elem(self.rx.last_used);
        self.rx.last_used = self.rx.last_used.wrapping_add(1);
        let id = id % self.rx.size;
        let len = len.clamp(HEADER, BUF_SIZE);
        // SAFETY: el buffer `id` tiene BUF_SIZE bytes y el dispositivo escribió `len`.
        let frame = unsafe {
            core::slice::from_raw_parts(self.rx.buffer(id).add(HEADER), len - HEADER).to_vec()
        };
        RX_BYTES.fetch_add(frame.len() as u64, Ordering::Relaxed);
        // El buffer vuelve a quedar disponible para el próximo paquete.
        self.rx.publish(id);
        self.out16(REG_QUEUE_NOTIFY, self.rx.index);
        Some(frame)
    }

    fn send_frame<R>(&mut self, len: usize, fill: impl FnOnce(&mut [u8]) -> R) -> R {
        let i = self.tx_next;
        self.tx_next = (self.tx_next + 1) % self.tx.size;
        let buf = self.tx.buffer(i);
        let len = len.min(BUF_SIZE - HEADER);
        // SAFETY: el buffer `i` es propio (no está en vuelo: lo garantiza `transmit`) y mide
        // BUF_SIZE bytes; el descriptor `i` está en rango.
        unsafe {
            core::ptr::write_bytes(buf, 0, HEADER);
            let result = fill(core::slice::from_raw_parts_mut(buf.add(HEADER), len));
            write_volatile(
                self.tx.desc(i),
                Descriptor {
                    addr: self.phys(buf),
                    len: (HEADER + len) as u32,
                    flags: 0,
                    next: 0,
                },
            );
            self.tx.publish(i);
            self.tx_in_flight += 1;
            TX_BYTES.fetch_add(len as u64, Ordering::Relaxed);
            self.out16(REG_QUEUE_NOTIFY, self.tx.index);
            result
        }
    }
}

fn empty_queue() -> Queue {
    Queue {
        index: 0,
        size: 0,
        base: core::ptr::null_mut(),
        used_offset: 0,
        buffers: core::ptr::null_mut(),
        avail_idx: 0,
        last_used: 0,
    }
}

pub struct RxToken(Vec<u8>);
pub struct TxToken<'a>(&'a mut VirtioNet);

impl RxToken {
    /// La trama recibida (para `nic.rs`).
    pub fn into_frame(self) -> Vec<u8> {
        self.0
    }
}

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        self.0.send_frame(len, f)
    }
}

impl Device for VirtioNet {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, _now: Instant) -> Option<(RxToken, TxToken<'_>)> {
        self.reclaim_tx();
        let frame = self.recv_frame()?;
        Some((RxToken(frame), TxToken(self)))
    }

    fn transmit(&mut self, _now: Instant) -> Option<TxToken<'_>> {
        self.reclaim_tx();
        // Se deja un lugar libre: el anillo nunca se llena del todo.
        (self.tx_in_flight + 1 < self.tx.size).then_some(TxToken(self))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = MTU;
        // Sin tope de ráfaga: la cola de recepción tiene tantos buffers como el dispositivo
        // ofrece (256 en QEMU). Con `Some(1)`, smoltcp achicaba la ventana TCP a un solo
        // paquete por ida y vuelta, y las descargas no pasaban de ~90 KB/s.
        caps.max_burst_size = None;
        caps.medium = Medium::Ethernet;
        caps
    }
}
