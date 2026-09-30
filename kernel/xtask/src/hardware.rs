//! K13: arrancar JARVIS-OS con el hardware "real" que emula QEMU, en vez de virtio.
//!
//! `cargo xtask test-hardware` hace varias corridas, cada una con otra combinación: disco SATA
//! (la AHCI de la q35) o NVMe, con una tabla GPT como la que deja el instalador. Después
//! verifica con `fatfs` lo que quedó **adentro de la partición** de JARVIS.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;

use jarvis_drivers::gpt::{self, EFI_SYSTEM, Guid, JARVIS_DATA, NewPartition};

use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskBus {
    /// virtio-blk (lo de siempre): el disco entero es un FAT32.
    Virtio,
    /// La controladora AHCI de la q35, con un disco SATA.
    Ahci,
    Nvme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetCard {
    Virtio,
    /// Intel 82574L (PCI Express, con MSI).
    E1000e,
    /// Realtek RTL8139 (PCI, sin MSI: el kernel la revisa cada tanto).
    Rtl8139,
}

#[derive(Clone, Copy, Debug)]
pub struct Hw {
    pub disk: DiskBus,
    pub net: NetCard,
}

pub const VIRTIO: Hw = Hw {
    disk: DiskBus::Virtio,
    net: NetCard::Virtio,
};

static HW: Mutex<Hw> = Mutex::new(VIRTIO);

/// El hardware de las próximas máquinas que arranque `qemu()`.
pub fn set(hw: Hw) {
    *HW.lock().unwrap_or_else(|e| e.into_inner()) = hw;
}

pub fn get() -> Hw {
    *HW.lock().unwrap_or_else(|e| e.into_inner())
}

/// Los argumentos de QEMU del disco de datos.
pub fn disk_args(cmd: &mut Command, disk: &Path) {
    cmd.arg("-drive").arg(format!(
        "if=none,id=disco,format=raw,file={}",
        disk.display()
    ));
    match get().disk {
        // La interfaz legacy (por puertos de E/S), que es la que implementa el driver.
        DiskBus::Virtio => cmd.args(["-device", "virtio-blk-pci,drive=disco,disable-modern=on"]),
        // La q35 ya trae la controladora AHCI (ICH9, 00:1f.2). En su puerto 0 está la imagen de
        // arranque (`-drive` sin `if=` va ahí): el disco de datos va en el 1.
        DiskBus::Ahci => cmd.args(["-device", "ide-hd,drive=disco,bus=ide.1"]),
        DiskBus::Nvme => cmd.args(["-device", "nvme,serial=JARVIS0001,drive=disco"]),
    };
}

/// El `-device` de la placa de red (conectada a la red "user" de QEMU, `red`).
pub fn net_device() -> &'static str {
    match get().net {
        NetCard::Virtio => "virtio-net-pci,netdev=red,disable-modern=on",
        NetCard::E1000e => "e1000e,netdev=red",
        NetCard::Rtl8139 => "rtl8139,netdev=red",
    }
}

/// Sectores de 512 bytes.
const SECTOR: u64 = 512;

/// Arma un disco GPT como el que deja el instalador (una ESP chica y la partición de JARVIS) con
/// el FAT32 de `fat` adentro. Devuelve el primer sector de la partición de JARVIS.
pub fn gpt_disk(fat: &Path, out: &Path) -> Result<u64> {
    let io = |e: std::io::Error| format!("{}: {e}", out.display());
    let data = fs::read(fat).map_err(|e| format!("{}: {e}", fat.display()))?;
    let fat_sectors = data.len() as u64 / SECTOR;
    let esp = 8 * 2048;
    // 1 MiB de alineación al principio, la ESP, el FAT32 y 1 MiB al final para la GPT de respaldo.
    let total = 2048 + esp + fat_sectors + 2048;
    let uniques: Vec<Guid> = (1..=3u8).map(|n| Guid([n; 16])).collect();
    let layout = gpt::create(
        total,
        &[
            NewPartition {
                kind: EFI_SYSTEM,
                sectors: esp,
                name: "EFI",
            },
            NewPartition {
                kind: JARVIS_DATA,
                sectors: fat_sectors,
                name: "JARVIS-OS",
            },
        ],
        &uniques,
    )
    .ok_or("la GPT de prueba no entra en el disco")?;
    let mut file = fs::File::create(out).map_err(io)?;
    file.set_len(total * SECTOR).map_err(io)?;
    for (lba, bytes) in &layout.writes {
        file.seek(SeekFrom::Start(lba * SECTOR)).map_err(io)?;
        file.write_all(bytes).map_err(io)?;
    }
    let start = layout.partitions[1].first;
    file.seek(SeekFrom::Start(start * SECTOR)).map_err(io)?;
    file.write_all(&data).map_err(io)?;
    Ok(start)
}

/// Una partición de un archivo de disco, para abrirla con `fatfs`.
pub struct Partition {
    file: fs::File,
    start: u64,
    len: u64,
    pos: u64,
}

impl Partition {
    /// La partición de JARVIS de un disco GPT (la busca con nuestro lector de GPT).
    pub fn open(disk: &Path) -> Result<Partition> {
        let io = |e: std::io::Error| format!("{}: {e}", disk.display());
        let bytes = fs::read(disk).map_err(io)?;
        let mut mem = jarvis_fs::MemDisk::new(bytes);
        let parts = gpt::read(&mut mem).ok_or("el disco no tiene GPT")?;
        let p = parts
            .iter()
            .find(|p| p.kind == JARVIS_DATA)
            .ok_or("el disco no tiene partición de JARVIS")?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(disk)
            .map_err(io)?;
        Ok(Partition {
            file,
            start: p.first * SECTOR,
            len: p.sectors() * SECTOR,
            pos: 0,
        })
    }
}

impl Read for Partition {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.len.saturating_sub(self.pos) as usize);
        self.file.seek(SeekFrom::Start(self.start + self.pos))?;
        let n = self.file.read(&mut buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Write for Partition {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = buf.len().min(self.len.saturating_sub(self.pos) as usize);
        self.file.seek(SeekFrom::Start(self.start + self.pos))?;
        let n = self.file.write(&buf[..n])?;
        self.pos += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Seek for Partition {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => self.len as i64 + d,
        };
        if pos < 0 {
            return Err(std::io::Error::other("antes del principio de la partición"));
        }
        self.pos = pos as u64;
        Ok(self.pos)
    }
}
