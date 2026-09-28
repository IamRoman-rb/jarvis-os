//! Driver de disco virtio-blk (interfaz "legacy" por puertos de E/S).
//!
//! virtio es el estándar de dispositivos virtuales de QEMU/KVM. En vez de imitar un disco real
//! registro por registro, el kernel y el dispositivo comparten una **virtqueue** en memoria:
//!
//! ```text
//! tabla de descriptores   anillo "disponible"          anillo "usado"
//! [dir, largo, flags]     el kernel anota qué          el dispositivo anota qué
//! cada uno, un buffer     pedidos dejó listos          pedidos terminó
//! ```
//!
//! Un pedido son 3 descriptores encadenados: cabecera (leer o escribir, qué sector), datos y un
//! byte de estado que completa el dispositivo. El kernel lo publica en el anillo disponible,
//! "toca el timbre" (queue notify) y espera a que aparezca en el anillo usado. Un pedido por vez.
//!
//! Esperar (K9): con multitarea, la tarea que pidió el sector **se duerme** hasta que el disco
//! avisa por su interrupción (el dispositivo sube su línea PCI al completar el pedido; ver
//! interrupts.rs) y mientras tanto corren las demás. Durante el arranque, antes de que haya
//! tareas, se espera dando vueltas (polling) como antes.
//!
//! El dispositivo accede a la memoria por **DMA** con direcciones **físicas**. Por eso la cola y
//! un buffer intermedio viven en el heap, donde física = virtual − `physical_memory_offset`.
//! Referencia: especificación virtio 1.x, sección "Legacy Interface", y
//! <https://wiki.osdev.org/Virtio>.

use alloc::alloc::{Layout, alloc_zeroed};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering, fence};

use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};
use x86_64::instructions::port::Port;

use crate::{pci, serial_println, task, time};

const VENDOR_VIRTIO: u16 = 0x1AF4;
/// virtio-blk "transitional" (con interfaz legacy).
const DEVICE_BLOCK_LEGACY: u16 = 0x1001;

// Registros legacy (desplazamientos desde el BAR0 de E/S).
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_ADDRESS: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SELECT: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_STATUS: u16 = 0x12;
/// Registro ISR: qué pasó (bit 0: se usó una cola). Leerlo lo borra y baja la interrupción.
pub const REG_ISR: u16 = 0x13;
const REG_CAPACITY: u16 = 0x14;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;

const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2; // el dispositivo escribe en este buffer

const REQUEST_IN: u32 = 0; // leer del disco
const REQUEST_OUT: u32 = 1; // escribir al disco

/// Bytes leídos y escritos desde el arranque (para el monitor).
pub static READ_BYTES: AtomicU64 = AtomicU64::new(0);
pub static WRITTEN_BYTES: AtomicU64 = AtomicU64::new(0);
/// Pedidos hechos y ciclos del TSC esperando al disco (para el log de frames lentos).
pub static REQUESTS: AtomicU64 = AtomicU64::new(0);
pub static WRITE_REQUESTS: AtomicU64 = AtomicU64::new(0);
pub static WAIT_TSC: AtomicU64 = AtomicU64::new(0);

/// Tamaño del buffer intermedio: los pedidos más grandes se parten.
const BOUNCE_SECTORS: usize = 128; // 64 KiB
/// Vueltas de espera antes de dar el pedido por perdido.
const POLL_LIMIT: u64 = 500_000_000;
/// Con interrupciones: plazo total de un pedido, y cada cuánto se revisa el anillo por si la
/// interrupción no llegó.
const IRQ_TIMEOUT_MS: u64 = 10_000;
const IRQ_RECHECK_MS: u64 = 20;
/// Pedidos que terminaron despertando a la tarea por la interrupción (para el log).
pub static IRQ_WAKEUPS: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
struct Descriptor {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// Cabecera de pedido + byte de estado, juntos en memoria DMA.
#[repr(C)]
struct RequestHeader {
    kind: u32,
    reserved: u32,
    sector: u64,
    status: u8,
}

pub struct VirtioBlk {
    io_base: u16,
    queue_size: u16,
    /// Dirección virtual de la cola (descriptores, anillo disponible, anillo usado).
    queue: *mut u8,
    used_offset: usize,
    avail_idx: u16,
    last_used: u16,
    header: *mut RequestHeader,
    bounce: *mut u8,
    capacity: u64,
    physical_memory_offset: u64,
    /// Línea PCI por la que avisa (la eligió el firmware).
    irq_line: Option<u8>,
    /// Ya está anotado en su línea: se puede dormir esperándolo.
    use_irq: bool,
}

// SAFETY: los punteros apuntan a memoria del heap que solo usa este driver, y lo usa una sola
// tarea (la del escritorio, dueña del disco). La interrupción solo lee el registro ISR.
unsafe impl Send for VirtioBlk {}

fn align_up(v: usize, align: usize) -> usize {
    v.div_ceil(align) * align
}

/// Memoria para DMA: alineada a 4 KiB y en el heap (física contigua y conocida).
fn dma_alloc(size: usize) -> Option<*mut u8> {
    let layout = Layout::from_size_align(align_up(size, 4096), 4096).ok()?;
    // SAFETY: el tamaño es > 0 y la memoria nunca se libera (el driver vive lo que el kernel).
    let ptr = unsafe { alloc_zeroed(layout) };
    (!ptr.is_null()).then_some(ptr)
}

impl VirtioBlk {
    /// Busca el disco en el bus PCI y lo inicializa. `None` si no hay disco o falla.
    pub fn init(physical_memory_offset: u64) -> Option<Self> {
        let dev = pci::find(VENDOR_VIRTIO, DEVICE_BLOCK_LEGACY)?;
        let bar0 = dev.bar(0);
        if bar0 & 1 == 0 {
            serial_println!("virtio-blk: el BAR0 no es de E/S (¿falta disable-modern=on?)");
            return None;
        }
        dev.enable_io_and_dma();
        let io_base = (bar0 & 0xFFFC) as u16;

        let mut blk = VirtioBlk {
            io_base,
            queue_size: 0,
            queue: core::ptr::null_mut(),
            used_offset: 0,
            avail_idx: 0,
            last_used: 0,
            header: core::ptr::null_mut(),
            bounce: core::ptr::null_mut(),
            capacity: 0,
            physical_memory_offset,
            irq_line: dev.interrupt_line(),
            use_irq: false,
        };
        // Secuencia de arranque de un dispositivo virtio: reset → "te vi" → "tengo driver".
        blk.out8(REG_STATUS, 0);
        blk.out8(REG_STATUS, STATUS_ACKNOWLEDGE);
        blk.out8(REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        blk.out32(REG_GUEST_FEATURES, 0); // no usamos ninguna característica opcional

        blk.out16(REG_QUEUE_SELECT, 0);
        let size = blk.in16(REG_QUEUE_SIZE) as usize;
        if size == 0 {
            serial_println!("virtio-blk: la cola 0 no existe");
            return None;
        }
        // Distribución legacy: descriptores y anillo disponible juntos, el usado en otra página.
        let avail_end = 16 * size + 6 + 2 * size;
        let used_offset = align_up(avail_end, 4096);
        let total = used_offset + align_up(6 + 8 * size, 4096);
        blk.queue = dma_alloc(total)?;
        blk.queue_size = size as u16;
        blk.used_offset = used_offset;
        blk.header = dma_alloc(core::mem::size_of::<RequestHeader>())? as *mut RequestHeader;
        blk.bounce = dma_alloc(BOUNCE_SECTORS * SECTOR_SIZE)?;

        let pfn = (blk.phys(blk.queue) >> 12) as u32;
        blk.out32(REG_QUEUE_ADDRESS, pfn);
        blk.out8(
            REG_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK,
        );

        let lo = blk.in32(REG_CAPACITY) as u64;
        let hi = blk.in32(REG_CAPACITY + 4) as u64;
        blk.capacity = hi << 32 | lo;
        serial_println!(
            "virtio-blk: disco de {} MiB en PCI {:02x}:{:02x}.{} (cola de {})",
            blk.capacity * SECTOR_SIZE as u64 / (1024 * 1024),
            dev.bus,
            dev.slot,
            dev.function,
            size
        );
        Some(blk)
    }

    /// (línea, puerto del registro ISR), para anotarlo en interrupts.rs.
    pub fn irq(&self) -> Option<(u8, u16)> {
        Some((self.irq_line?, self.io_base + REG_ISR))
    }

    /// Ya está anotado en su línea: desde ahora, esperar al disco es dormir.
    pub fn use_irq(&mut self) {
        self.use_irq = true;
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

    /// Hace un pedido de `sectors` sectores usando el buffer intermedio y espera que termine.
    fn request(&mut self, kind: u32, lba: u64, sectors: usize) -> Result<(), IoError> {
        let len = sectors * SECTOR_SIZE;
        let n = self.queue_size as usize;
        // SAFETY: `header`, `bounce` y `queue` son memoria DMA propia de este driver, del tamaño
        // calculado en `init`; los índices de descriptor (0..3) y de anillo (`% n`) están en rango.
        unsafe {
            write_volatile(
                self.header,
                RequestHeader {
                    kind,
                    reserved: 0,
                    sector: lba,
                    status: 0xFF,
                },
            );
            let desc = self.queue as *mut Descriptor;
            let header_phys = self.phys(self.header as *const u8);
            let data_flags = if kind == REQUEST_IN {
                DESC_NEXT | DESC_WRITE
            } else {
                DESC_NEXT
            };
            write_volatile(
                desc,
                Descriptor {
                    addr: header_phys,
                    len: 16,
                    flags: DESC_NEXT,
                    next: 1,
                },
            );
            write_volatile(
                desc.add(1),
                Descriptor {
                    addr: self.phys(self.bounce),
                    len: len as u32,
                    flags: data_flags,
                    next: 2,
                },
            );
            write_volatile(
                desc.add(2),
                Descriptor {
                    addr: header_phys + 16,
                    len: 1,
                    flags: DESC_WRITE,
                    next: 0,
                },
            );

            // Publicar el pedido (descriptor 0) en el anillo disponible.
            let avail = self.queue.add(16 * n) as *mut u16;
            write_volatile(avail.add(2 + self.avail_idx as usize % n), 0);
            fence(Ordering::SeqCst); // el anillo tiene que estar escrito antes que el índice
            self.avail_idx = self.avail_idx.wrapping_add(1);
            write_volatile(avail.add(1), self.avail_idx);
            fence(Ordering::SeqCst);
            self.out16(REG_QUEUE_NOTIFY, 0);

            // Esperar a que el dispositivo avance el índice del anillo usado.
            REQUESTS.fetch_add(1, Ordering::Relaxed);
            if kind == REQUEST_OUT {
                WRITE_REQUESTS.fetch_add(1, Ordering::Relaxed);
            }
            let waiting = time::rdtsc();
            let used_idx = self.queue.add(self.used_offset + 2) as *const u16;
            let mut spins = 0u64;
            let since = time::millis();
            while read_volatile(used_idx) == self.last_used {
                if self.use_irq && task::can_block() {
                    // Dormir hasta la interrupción (o un rato, por si se perdió) y revisar.
                    let now = time::millis();
                    if now - since > IRQ_TIMEOUT_MS {
                        serial_println!("virtio-blk: el disco no respondió (sector {})", lba);
                        return Err(IoError);
                    }
                    if task::wait(task::EV_DISK, Some(now + IRQ_RECHECK_MS)) & task::EV_DISK != 0
                        && IRQ_WAKEUPS.fetch_add(1, Ordering::Relaxed) == 0
                    {
                        serial_println!("DISCO_POR_INTERRUPCION");
                    }
                    continue;
                }
                spins += 1;
                if spins > POLL_LIMIT {
                    serial_println!("virtio-blk: el disco no respondió (sector {})", lba);
                    return Err(IoError);
                }
                core::hint::spin_loop();
            }
            WAIT_TSC.fetch_add(time::rdtsc() - waiting, Ordering::Relaxed);
            self.last_used = self.last_used.wrapping_add(1);
            fence(Ordering::SeqCst);
            let status = read_volatile(core::ptr::addr_of!((*self.header).status));
            if status != 0 {
                serial_println!("virtio-blk: error {} en el sector {}", status, lba);
                return Err(IoError);
            }
        }
        Ok(())
    }
}

impl BlockDevice for VirtioBlk {
    fn sector_count(&self) -> u64 {
        self.capacity
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        READ_BYTES.fetch_add(buf.len() as u64, Ordering::Relaxed);
        for (i, chunk) in buf.chunks_mut(BOUNCE_SECTORS * SECTOR_SIZE).enumerate() {
            let sectors = chunk.len().div_ceil(SECTOR_SIZE);
            self.request(REQUEST_IN, lba + (i * BOUNCE_SECTORS) as u64, sectors)?;
            // SAFETY: `bounce` tiene BOUNCE_SECTORS sectores y `chunk` no es más largo.
            let src = unsafe { core::slice::from_raw_parts(self.bounce, chunk.len()) };
            chunk.copy_from_slice(src);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        WRITTEN_BYTES.fetch_add(buf.len() as u64, Ordering::Relaxed);
        for (i, chunk) in buf.chunks(BOUNCE_SECTORS * SECTOR_SIZE).enumerate() {
            // SAFETY: ídem `read`.
            let dst = unsafe { core::slice::from_raw_parts_mut(self.bounce, chunk.len()) };
            dst.copy_from_slice(chunk);
            let sectors = chunk.len().div_ceil(SECTOR_SIZE);
            self.request(REQUEST_OUT, lba + (i * BOUNCE_SECTORS) as u64, sectors)?;
        }
        Ok(())
    }
}
