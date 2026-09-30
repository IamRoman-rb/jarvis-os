//! Driver de SSD NVMe (K13).
//!
//! Hardware: cualquier controlador PCI de clase 01/08/02 (NVM Express), el de QEMU
//! (`-device nvme`) incluido. Se usa el namespace 1 con una cola de administración y una de
//! E/S, de 64 entradas cada una. El formato de los comandos está en `jarvis_drivers::nvme`.
//! Referencias: NVM Express Base 1.4 §3.1 (registros), §7.6 (inicialización) y
//! <https://wiki.osdev.org/NVMe>.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, fence};

use jarvis_drivers::nvme::{
    Capabilities, Command, IO_READ, IO_WRITE, doorbell, parse_cap, parse_completion,
    parse_controller, parse_namespace,
};
use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};

use crate::mmio::{Mmio, wait_until};
use crate::{dma, interrupts, pci, serial_println, task, time};

const REG_CAP: usize = 0x00;
const REG_CC: usize = 0x14;
const REG_CSTS: usize = 0x1C;
const REG_AQA: usize = 0x24;
const REG_ASQ: usize = 0x28;
const REG_ACQ: usize = 0x30;

const QUEUE: u16 = 64;
const PAGE: usize = 4096;
/// Páginas del buffer intermedio (64 KiB por pedido; con más de dos páginas hace falta una
/// "lista de PRP", que se arma una sola vez porque el buffer es siempre el mismo).
const BOUNCE_PAGES: usize = 16;
const TIMEOUT_MS: u64 = 5000;
/// CDW12 bit 30: "Force Unit Access" (el dato llega al medio antes de avisar).
const FUA: u32 = 1 << 30;

/// Una cola de envío con su cola de terminación.
struct Queue {
    id: u16,
    sq: *mut u8,
    cq: *mut u8,
    tail: u16,
    head: u16,
    phase: bool,
}

pub struct NvmeDisk {
    regs: Mmio,
    cap: Capabilities,
    admin: Queue,
    io: Queue,
    next_id: u16,
    bounce: *mut u8,
    prp_list: *mut u64,
    /// Sectores por pedido (el mínimo entre el buffer y el máximo del controlador, MDTS).
    max_sectors: usize,
    blocks: u64,
    irq: bool,
    pub model: String,
}

// SAFETY: los punteros son memoria DMA propia de este disco; lo usa una sola tarea a la vez.
unsafe impl Send for NvmeDisk {}

pub fn probe() -> Vec<NvmeDisk> {
    let mut out = Vec::new();
    for dev in pci::find_class(0x01, 0x08, 0x02) {
        match NvmeDisk::start(dev) {
            Some(d) => {
                serial_println!(
                    "NVME_DISCO \"{}\", {} MiB",
                    d.model,
                    d.blocks * SECTOR_SIZE as u64 / (1024 * 1024)
                );
                out.push(d);
            }
            None => serial_println!(
                "NVMe {:02x}:{:02x}.{}: no se pudo iniciar",
                dev.bus,
                dev.slot,
                dev.function
            ),
        }
    }
    out
}

impl NvmeDisk {
    fn start(dev: pci::Device) -> Option<NvmeDisk> {
        let bar = dev.bar_address(0)?;
        dev.enable_memory_and_dma();
        let regs = Mmio::map(bar, 0x2000)?;
        let cap = parse_cap(regs.r64(REG_CAP));
        if cap.min_page > PAGE as u32 {
            return None;
        }
        // 1. Apagar el controlador (CC.EN = 0) y esperar CSTS.RDY = 0.
        regs.w32(REG_CC, regs.r32(REG_CC) & !1);
        if !ready(&regs, false, cap.timeout_ms) {
            return None;
        }
        // 2. Cola de administración.
        let admin = Queue::new(0)?;
        regs.w32(REG_AQA, ((QUEUE as u32 - 1) << 16) | (QUEUE as u32 - 1));
        regs.w64(REG_ASQ, dma::phys(admin.sq));
        regs.w64(REG_ACQ, dma::phys(admin.cq));
        // 3. Encender: comandos NVM, páginas de 4 KiB, entradas de 64 (2^6) y 16 (2^4) bytes.
        regs.w32(REG_CC, 1 | (6 << 16) | (4 << 20));
        if !ready(&regs, true, cap.timeout_ms) {
            serial_println!("NVMe: el controlador no quedó listo");
            return None;
        }
        let irq = interrupts::enable_msi(dev, task::EV_DISK, true);
        let bounce = dma::alloc(BOUNCE_PAGES * PAGE, PAGE)?;
        let prp_list = dma::alloc(PAGE, PAGE)? as *mut u64;
        for i in 1..BOUNCE_PAGES {
            // SAFETY: `prp_list` es una página (512 entradas); se escriben 15.
            unsafe {
                prp_list
                    .add(i - 1)
                    .write(dma::phys(bounce) + (i * PAGE) as u64)
            };
        }
        let mut disk = NvmeDisk {
            regs,
            cap,
            admin,
            io: Queue::new(1)?,
            next_id: 1,
            bounce,
            prp_list,
            max_sectors: BOUNCE_PAGES * PAGE / SECTOR_SIZE,
            blocks: 0,
            irq,
            model: String::new(),
        };
        // 4. Identificar el controlador y el namespace 1.
        let page = dma::phys(bounce);
        disk.admin_command(|id| Command::identify(id, 1, 0, page))?;
        let ctrl = parse_controller(disk.bounce_slice(PAGE))?;
        if ctrl.mdts != 0 {
            let max = (1usize << ctrl.mdts) * cap.min_page as usize / SECTOR_SIZE;
            disk.max_sectors = disk.max_sectors.min(max);
        }
        disk.admin_command(|id| Command::identify(id, 0, 1, page))?;
        let ns = parse_namespace(disk.bounce_slice(PAGE))?;
        if ns.block_size != SECTOR_SIZE as u32 {
            serial_println!(
                "NVMe: \"{}\" usa bloques de {} bytes; solo se usan los de 512",
                ctrl.model,
                ns.block_size
            );
            return None;
        }
        disk.blocks = ns.blocks;
        disk.model = ctrl.model;
        // 5. La cola de E/S: primero la de terminación, después la de envío.
        let (cq, sq) = (dma::phys(disk.io.cq), dma::phys(disk.io.sq));
        disk.admin_command(|id| Command::create_cq(id, 1, QUEUE, cq))?;
        disk.admin_command(|id| Command::create_sq(id, 1, QUEUE, sq, 1))?;
        serial_println!(
            "NVMe {:02x}:{:02x}.{}: \"{}\", MSI: {}, hasta {} KiB por pedido",
            dev.bus,
            dev.slot,
            dev.function,
            disk.model,
            if irq { "sí" } else { "no" },
            disk.max_sectors / 2
        );
        Some(disk)
    }

    pub fn name(&self) -> String {
        format!("NVMe: {}", self.model)
    }

    fn bounce_slice(&self, len: usize) -> &[u8] {
        // SAFETY: `bounce` mide BOUNCE_PAGES páginas y `len` no pasa de ahí (los llamadores
        // piden una página o un pedido de `max_sectors`).
        unsafe { core::slice::from_raw_parts(self.bounce, len) }
    }

    fn admin_command(&mut self, make: impl FnOnce(u16) -> Command) -> Option<()> {
        let id = self.take_id();
        let cmd = make(id);
        let (regs, stride, irq) = (self.regs, self.cap.doorbell_stride, self.irq);
        submit(&mut self.admin, regs, stride, irq, &cmd).then_some(())
    }

    fn take_id(&mut self) -> u16 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    fn io(&mut self, opcode: u8, lba: u64, sectors: usize) -> Result<(), IoError> {
        let bytes = sectors * SECTOR_SIZE;
        let first = dma::phys(self.bounce);
        // PRP2: sin datos en una segunda página, 0; en dos páginas, la segunda; en más, la lista.
        let prp2 = match bytes.div_ceil(PAGE) {
            0 | 1 => 0,
            2 => first + PAGE as u64,
            _ => dma::phys(self.prp_list as *const u8),
        };
        let id = self.take_id();
        let mut cmd = Command::rw(opcode, id, 1, lba, sectors as u16, [first, prp2]);
        if opcode == IO_WRITE {
            cmd.cdw[2] |= FUA;
        }
        let (regs, stride, irq) = (self.regs, self.cap.doorbell_stride, self.irq);
        if submit(&mut self.io, regs, stride, irq, &cmd) {
            Ok(())
        } else {
            serial_println!("NVMe: error (comando {opcode:#x}, sector {lba})");
            Err(IoError)
        }
    }
}

impl Queue {
    fn new(id: u16) -> Option<Queue> {
        Some(Queue {
            id,
            sq: dma::alloc(QUEUE as usize * 64, PAGE)?,
            cq: dma::alloc(QUEUE as usize * 16, PAGE)?,
            tail: 0,
            head: 0,
            phase: true,
        })
    }
}

/// Espera a que CSTS.RDY sea `on`.
fn ready(regs: &Mmio, on: bool, timeout_ms: u32) -> bool {
    let t = time::millis();
    while (regs.r32(REG_CSTS) & 1 != 0) != on {
        // CSTS.CFS: falla fatal del controlador.
        if regs.r32(REG_CSTS) & 2 != 0 || time::millis() - t > timeout_ms.max(500) as u64 {
            return false;
        }
    }
    true
}

/// Pone el comando en la cola, toca el timbre y espera su terminación.
fn submit(q: &mut Queue, regs: Mmio, stride: u32, irq: bool, cmd: &Command) -> bool {
    let bytes = cmd.to_bytes();
    // SAFETY: `sq` mide QUEUE entradas de 64 bytes y `tail < QUEUE`.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), q.sq.add(q.tail as usize * 64), 64);
    }
    q.tail = (q.tail + 1) % QUEUE;
    fence(Ordering::SeqCst);
    regs.w32(doorbell(q.id, false, stride), q.tail as u32);

    let entry = |q: &Queue| {
        // SAFETY: `cq` mide QUEUE entradas de 16 bytes y `head < QUEUE`; la escribe el
        // controlador, por eso se lee con `read_volatile`.
        let raw: [u8; 16] =
            unsafe { core::ptr::read_volatile(q.cq.add(q.head as usize * 16) as *const [u8; 16]) };
        parse_completion(&raw)
    };
    let done = wait_until(task::EV_DISK, irq, TIMEOUT_MS, || entry(q).phase == q.phase);
    if !done {
        return false;
    }
    let c = entry(q);
    q.head = (q.head + 1) % QUEUE;
    if q.head == 0 {
        q.phase = !q.phase; // dio la vuelta: las nuevas vienen con la fase invertida
    }
    regs.w32(doorbell(q.id, true, stride), q.head as u32);
    c.id == cmd.id && c.status == 0
}

impl BlockDevice for NvmeDisk {
    fn sector_count(&self) -> u64 {
        self.blocks
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let step = self.max_sectors * SECTOR_SIZE;
        for (i, chunk) in buf.chunks_mut(step).enumerate() {
            let sectors = chunk.len().div_ceil(SECTOR_SIZE);
            self.io(IO_READ, lba + (i * self.max_sectors) as u64, sectors)?;
            chunk.copy_from_slice(self.bounce_slice(chunk.len()));
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let step = self.max_sectors * SECTOR_SIZE;
        for (i, chunk) in buf.chunks(step).enumerate() {
            // SAFETY: `bounce` mide BOUNCE_PAGES páginas y `chunk` no es más largo que un pedido.
            let dst = unsafe { core::slice::from_raw_parts_mut(self.bounce, chunk.len()) };
            dst.copy_from_slice(chunk);
            let sectors = chunk.len().div_ceil(SECTOR_SIZE);
            self.io(IO_WRITE, lba + (i * self.max_sectors) as u64, sectors)?;
        }
        Ok(())
    }
}
