//! Driver de discos SATA por AHCI (K13).
//!
//! Hardware: cualquier controladora PCI de clase 01/06/01 (AHCI 1.x): la ICH9 que emula QEMU en
//! la máquina q35 y la AMD 1022:43EB de la PC de Roman. Cada puerto con un disco SATA se
//! convierte en un `AhciDisk` (un `BlockDevice`). El formato de los comandos está en
//! `jarvis_drivers::ahci`; acá se tocan los registros.
//! Referencias: especificación AHCI 1.3.1 §3 (registros), §5 (cómo mandar un comando) y §10.6
//! (el traspaso del BIOS), <https://wiki.osdev.org/AHCI>.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use jarvis_drivers::ahci::{
    ATA_FLUSH_CACHE_EXT, ATA_IDENTIFY, ATA_IDENTIFY_PACKET, ATA_READ_DMA_EXT, ATA_WRITE_DMA_EXT,
    ATA_WRITE_DMA_FUA_EXT, IS_TFES, SIG_ATAPI, SIG_SATA, TFD_BUSY, TFD_DRQ, TFD_ERR,
    command_header, command_table_size, h2d_fis, packet_fis, packet_header, parse_identify,
    prdt_entry,
};
use jarvis_drivers::ide::{READ_CAPACITY, parse_capacity, read10};
use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};

use crate::mmio::{Mmio, wait_until};
use crate::{dma, interrupts, pci, serial_println, task, time};

// Registros globales (HBA).
const CAP: usize = 0x00;
const GHC: usize = 0x04;
const IS: usize = 0x08;
const PI: usize = 0x0C;
const CAP2: usize = 0x24;
const BOHC: usize = 0x28;
const GHC_AE: u32 = 1 << 31;
const GHC_IE: u32 = 1 << 1;

// Registros de cada puerto (desde 0x100 + 0x80 × puerto).
const PX_CLB: usize = 0x00;
const PX_FB: usize = 0x08;
const PX_IS: usize = 0x10;
const PX_IE: usize = 0x14;
const PX_CMD: usize = 0x18;
const PX_TFD: usize = 0x20;
const PX_SIG: usize = 0x24;
const PX_SSTS: usize = 0x28;
const PX_SERR: usize = 0x30;
const PX_CI: usize = 0x38;
const CMD_ST: u32 = 1 << 0;
const CMD_SUD: u32 = 1 << 1;
const CMD_POD: u32 = 1 << 2;
const CMD_FRE: u32 = 1 << 4;
const CMD_FR: u32 = 1 << 14;
const CMD_CR: u32 = 1 << 15;

/// Sectores por pedido (64 KiB: un solo tramo de PRDT).
const BOUNCE_SECTORS: usize = 128;
const TIMEOUT_MS: u64 = 5000;

/// ¿Alguna controladora avisa por MSI? (Si no, se espera revisando.)
static USE_IRQ: AtomicBool = AtomicBool::new(false);

pub struct AhciDisk {
    hba: Mmio,
    port: Mmio,
    number: usize,
    header: *mut u8,
    table: *mut u8,
    bounce: *mut u8,
    sectors: u64,
    fua: bool,
    /// Una lectora de CD/DVD (ATAPI): bloques de 2048, solo lectura. `sectors` cuenta de a 512.
    cd: bool,
    pub model: String,
}

// SAFETY: los punteros son memoria DMA propia de este disco; lo usa una sola tarea a la vez (la
// dueña del disco, o el instalador).
unsafe impl Send for AhciDisk {}

/// Busca las controladoras AHCI y devuelve un disco por cada puerto con un disco SATA.
pub fn probe() -> Vec<AhciDisk> {
    let mut disks = Vec::new();
    for dev in pci::find_class(0x01, 0x06, 0x01) {
        let Some(bar) = dev.bar_address(5) else {
            continue;
        };
        dev.enable_memory_and_dma();
        let Some(hba) = Mmio::map(bar, 0x1100) else {
            continue;
        };
        take_ownership(&hba);
        hba.w32(GHC, hba.r32(GHC) | GHC_AE);
        let msi = interrupts::enable_msi(dev, task::EV_DISK, true);
        if msi {
            USE_IRQ.store(true, Ordering::Relaxed);
            hba.w32(IS, u32::MAX);
            hba.w32(GHC, hba.r32(GHC) | GHC_IE);
        }
        let ports = hba.r32(PI);
        let cap = hba.r32(CAP);
        serial_println!(
            "AHCI {:02x}:{:02x}.{} ({:04x}:{:04x}): {} puertos, 64 bits: {}, MSI: {}",
            dev.bus,
            dev.slot,
            dev.function,
            dev.vendor(),
            dev.device_id(),
            ports.count_ones(),
            if cap & (1 << 31) != 0 { "sí" } else { "no" },
            if msi { "sí" } else { "no" }
        );
        for n in 0..32 {
            if ports & (1 << n) == 0 {
                continue;
            }
            let port = hba.sub(0x100 + n * 0x80, 0x80);
            // DET = 3: hay un dispositivo y la conexión está establecida.
            let sig = port.r32(PX_SIG);
            if port.r32(PX_SSTS) & 0xF != 3 || (sig != SIG_SATA && sig != SIG_ATAPI) {
                continue;
            }
            match AhciDisk::start(hba, port, n, cap, msi, sig == SIG_ATAPI) {
                Some(d) => {
                    serial_println!(
                        "AHCI_DISCO puerto {n}: \"{}\", {} MiB",
                        d.model,
                        d.sectors * SECTOR_SIZE as u64 / (1024 * 1024)
                    );
                    crate::hw::note("Disco", alloc::format!("SATA {n}: {} (AHCI)", d.model));
                    disks.push(d);
                }
                None => serial_println!("AHCI: el disco del puerto {n} no respondió"),
            }
        }
    }
    disks
}

/// El traspaso del BIOS al sistema operativo (§10.6): si el firmware todavía usa la
/// controladora (para arrancar desde el disco), se le pide que la suelte.
fn take_ownership(hba: &Mmio) {
    if hba.r32(CAP2) & 1 == 0 {
        return;
    }
    hba.w32(BOHC, hba.r32(BOHC) | 1 << 1); // OOS: la quiere el sistema operativo
    let t = time::millis();
    while hba.r32(BOHC) & 1 != 0 && time::millis() - t < 25 {}
    // Si el BIOS sigue ocupado (BB), puede tardar hasta 2 s en terminar lo que hacía.
    while hba.r32(BOHC) & 1 << 4 != 0 && time::millis() - t < 2000 {}
}

impl AhciDisk {
    fn start(
        hba: Mmio,
        port: Mmio,
        number: usize,
        cap: u32,
        msi: bool,
        cd: bool,
    ) -> Option<AhciDisk> {
        // Detener el puerto antes de cambiarle las listas (§10.1.2).
        port.w32(PX_CMD, port.r32(PX_CMD) & !CMD_ST);
        let t = time::millis();
        while port.r32(PX_CMD) & CMD_CR != 0 && time::millis() - t < 500 {}
        port.w32(PX_CMD, port.r32(PX_CMD) & !CMD_FRE);
        while port.r32(PX_CMD) & CMD_FR != 0 && time::millis() - t < 1000 {}
        if port.r32(PX_CMD) & (CMD_CR | CMD_FR) != 0 {
            return None;
        }

        // Una sola ranura: la lista de comandos mide 1 KiB igual (32 × 32 bytes).
        let list = dma::alloc(1024, 1024)?;
        let fis = dma::alloc(256, 256)?;
        let table = dma::alloc(command_table_size(1), 128)?;
        let bounce = dma::alloc(BOUNCE_SECTORS * SECTOR_SIZE, 4096)?;
        let (list_phys, fis_phys) = (dma::phys(list), dma::phys(fis));
        // Sin direcciones de 64 bits (CAP.S64A), todo tiene que estar abajo de 4 GiB.
        if cap & (1 << 31) == 0 && (list_phys | fis_phys | dma::phys(bounce)) >> 32 != 0 {
            serial_println!("AHCI: la controladora no llega a la memoria del heap (> 4 GiB)");
            return None;
        }
        port.w64_split(PX_CLB, list_phys);
        port.w64_split(PX_FB, fis_phys);
        port.w32(PX_SERR, u32::MAX);
        port.w32(PX_IS, u32::MAX);
        // Interrupciones del puerto: terminó con FIS de registro (DHRS) o con error (TFES).
        port.w32(PX_IE, if msi { 1 | IS_TFES } else { 0 });
        let mut cmd = port.r32(PX_CMD) | CMD_FRE | CMD_POD;
        if cap & (1 << 27) != 0 {
            cmd |= CMD_SUD; // encendido escalonado: hay que "hacer girar" el disco
        }
        port.w32(PX_CMD, cmd);
        let t = time::millis();
        while port.r32(PX_TFD) & (TFD_BUSY | TFD_DRQ) != 0 && time::millis() - t < 1000 {}
        port.w32(PX_CMD, port.r32(PX_CMD) | CMD_ST);

        let mut disk = AhciDisk {
            hba,
            port,
            number,
            header: list,
            table,
            bounce,
            sectors: 0,
            fua: false,
            cd,
            model: String::new(),
        };
        if cd {
            disk.command(ATA_IDENTIFY_PACKET, 0, 1, false).ok()?;
            // SAFETY: ídem abajo.
            let raw = unsafe { core::slice::from_raw_parts(bounce, SECTOR_SIZE) };
            disk.model = parse_identify(raw).map_or_else(|| "CD/DVD".into(), |i| i.model);
            // La primera orden después de cambiar el disco responde "atención de unidad".
            for _ in 0..3 {
                if disk.packet(READ_CAPACITY, 8).is_ok() {
                    // SAFETY: la respuesta (8 bytes) quedó en `bounce`.
                    let r = unsafe { core::slice::from_raw_parts(bounce, 8) };
                    if let Some((blocks, 2048)) = parse_capacity(r) {
                        disk.sectors = blocks * 4;
                        break;
                    }
                }
            }
            if disk.sectors == 0 {
                // Lectora sin disco: el puerto queda apagado y sin interrupciones.
                port.w32(PX_IE, 0);
                port.w32(PX_CMD, port.r32(PX_CMD) & !(CMD_ST | CMD_FRE));
                port.w32(PX_IS, u32::MAX);
                return None;
            }
            return Some(disk);
        }
        disk.command(ATA_IDENTIFY, 0, 1, false).ok()?;
        // SAFETY: `bounce` tiene al menos 512 bytes y la controladora terminó de escribirlos.
        let raw = unsafe { core::slice::from_raw_parts(bounce, SECTOR_SIZE) };
        let id = parse_identify(raw)?;
        if !id.lba48 || id.sector_size != SECTOR_SIZE as u32 {
            serial_println!(
                "AHCI: \"{}\" no se usa (LBA48: {}, sector de {} bytes)",
                id.model,
                id.lba48,
                id.sector_size
            );
            return None;
        }
        disk.sectors = id.sectors;
        disk.fua = id.fua;
        disk.model = id.model;
        Some(disk)
    }

    /// Nombre para mostrar ("SATA 0: modelo", "CD SATA 2: modelo").
    pub fn name(&self) -> String {
        if self.cd {
            format!("CD SATA {}: {}", self.number, self.model)
        } else {
            format!("SATA {}: {}", self.number, self.model)
        }
    }

    pub fn is_cd(&self) -> bool {
        self.cd
    }

    /// Un paquete SCSI (lectoras): la respuesta, de `bytes` bytes, queda en `bounce`.
    fn packet(&mut self, pkt: [u8; 12], bytes: usize) -> Result<(), IoError> {
        let fis = packet_fis();
        let header = packet_header(1, dma::phys(self.table));
        let prdt = prdt_entry(dma::phys(self.bounce), bytes.max(2) as u32);
        // SAFETY: como en `command`; el paquete va en el área ACMD de la tabla (0x40–0x4F).
        unsafe {
            core::ptr::copy_nonoverlapping(fis.as_ptr(), self.table, fis.len());
            core::ptr::copy_nonoverlapping(pkt.as_ptr(), self.table.add(0x40), pkt.len());
            core::ptr::copy_nonoverlapping(prdt.as_ptr(), self.table.add(0x80), prdt.len());
            core::ptr::copy_nonoverlapping(header.as_ptr(), self.header, header.len());
        }
        // (Sin disco en la lectora, los paquetes fallan: no es para avisar.)
        self.issue(pkt[0], 0, false)
    }

    /// Manda un comando en la ranura 0 con `count` sectores del buffer intermedio (0: sin datos)
    /// y espera.
    fn command(&mut self, ata: u8, lba: u64, count: usize, write: bool) -> Result<(), IoError> {
        let bytes = count * SECTOR_SIZE;
        let fis = h2d_fis(ata, lba, if ata == ATA_IDENTIFY { 0 } else { count as u16 });
        let header = command_header(5, write, (count > 0) as u16, dma::phys(self.table));
        let prdt = prdt_entry(dma::phys(self.bounce), bytes.max(2) as u32);
        // SAFETY: `table` mide `command_table_size(1)` bytes (FIS en 0, PRDT en 0x80) y `header`
        // 32 bytes (la ranura 0 de la lista); son memoria DMA de este disco.
        unsafe {
            core::ptr::copy_nonoverlapping(fis.as_ptr(), self.table, fis.len());
            core::ptr::copy_nonoverlapping(prdt.as_ptr(), self.table.add(0x80), prdt.len());
            core::ptr::copy_nonoverlapping(header.as_ptr(), self.header, header.len());
        }
        self.issue(ata, lba, true)
    }

    /// Manda lo que quedó armado en la ranura 0 y espera.
    fn issue(&mut self, ata: u8, lba: u64, log: bool) -> Result<(), IoError> {
        core::sync::atomic::fence(Ordering::SeqCst);
        self.port.w32(PX_IS, u32::MAX);
        self.port.w32(PX_CI, 1);
        let port = self.port;
        let done = wait_until(
            task::EV_DISK,
            USE_IRQ.load(Ordering::Relaxed),
            TIMEOUT_MS,
            || port.r32(PX_CI) & 1 == 0 || port.r32(PX_IS) & IS_TFES != 0,
        );
        // Bajar los avisos del puerto y el suyo en el registro global.
        self.port.w32(PX_IS, u32::MAX);
        self.hba.w32(IS, 1 << self.number);
        core::sync::atomic::fence(Ordering::SeqCst);
        if !done || self.port.r32(PX_TFD) & TFD_ERR != 0 {
            if !log {
                return Err(IoError);
            }
            serial_println!(
                "AHCI: error en el puerto {} (comando {ata:#x}, sector {lba}, TFD {:#x})",
                self.number,
                self.port.r32(PX_TFD)
            );
            return Err(IoError);
        }
        Ok(())
    }
}

impl BlockDevice for AhciDisk {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        if self.cd {
            return self.read_cd(lba, buf);
        }
        for (i, chunk) in buf.chunks_mut(BOUNCE_SECTORS * SECTOR_SIZE).enumerate() {
            let count = chunk.len().div_ceil(SECTOR_SIZE);
            self.command(
                ATA_READ_DMA_EXT,
                lba + (i * BOUNCE_SECTORS) as u64,
                count,
                false,
            )?;
            // SAFETY: `bounce` mide BOUNCE_SECTORS sectores y `chunk` no es más largo.
            let src = unsafe { core::slice::from_raw_parts(self.bounce, chunk.len()) };
            chunk.copy_from_slice(src);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        if self.cd {
            return Err(IoError);
        }
        for (i, chunk) in buf.chunks(BOUNCE_SECTORS * SECTOR_SIZE).enumerate() {
            // SAFETY: ídem `read`.
            let dst = unsafe { core::slice::from_raw_parts_mut(self.bounce, chunk.len()) };
            dst.copy_from_slice(chunk);
            let count = chunk.len().div_ceil(SECTOR_SIZE);
            let ata = if self.fua {
                ATA_WRITE_DMA_FUA_EXT
            } else {
                ATA_WRITE_DMA_EXT
            };
            self.command(ata, lba + (i * BOUNCE_SECTORS) as u64, count, true)?;
        }
        if !self.fua {
            self.command(ATA_FLUSH_CACHE_EXT, 0, 0, false)?;
        }
        Ok(())
    }
}

impl AhciDisk {
    /// Lectura de una lectora vista en sectores de 512: se leen los bloques de 2048 que los
    /// contienen (de a 64 KiB, lo que entra en `bounce`).
    fn read_cd(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        const PER: u64 = (BOUNCE_SECTORS * SECTOR_SIZE / 2048) as u64;
        let start = lba * 512;
        let end = start + buf.len() as u64;
        if end > self.sectors * 512 {
            return Err(IoError);
        }
        let mut block = start / 2048;
        while block * 2048 < end {
            let n = PER.min((end - block * 2048).div_ceil(2048));
            self.packet(read10(block as u32, n as u16), n as usize * 2048)?;
            // SAFETY: la respuesta (`n` bloques, hasta 64 KiB) quedó en `bounce`.
            let data = unsafe { core::slice::from_raw_parts(self.bounce, n as usize * 2048) };
            let from = (block * 2048).max(start);
            let to = ((block + n) * 2048).min(end);
            buf[(from - start) as usize..(to - start) as usize].copy_from_slice(
                &data[(from - block * 2048) as usize..(to - block * 2048) as usize],
            );
            block += n;
        }
        Ok(())
    }
}
