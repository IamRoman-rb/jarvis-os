//! GPT (GUID Partition Table): cómo se divide un disco en particiones en una PC con UEFI.
//!
//! - **LBA 0**: un MBR "protector". Una sola partición de tipo 0xEE que cubre todo el disco, para
//!   que las herramientas viejas no crean que está vacío.
//! - **LBA 1**: la cabecera ("EFI PART"): dónde están las entradas, cuántas son, qué sectores se
//!   pueden usar, y CRC32 de sí misma y de las entradas.
//! - **LBA 2…**: las entradas, de 128 bytes (tipo, GUID único, primer y último sector, nombre en
//!   UTF-16).
//! - Al final del disco, una **copia** de las entradas y de la cabecera, por si se rompe el
//!   principio.
//!
//! El firmware UEFI arranca desde la partición de sistema EFI (ESP, FAT32). JARVIS-OS guarda sus
//! datos en otra partición, con un GUID de tipo propio (`JARVIS_DATA`). El kernel **solo** monta
//! esa (ADR 0011). Las de Windows o Linux no las toca.
//! Referencia: especificación UEFI 2.10, cap. 5 ("GUID Partition Table Disk Layout").

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};

use crate::le;

/// Un GUID en el orden de bytes del disco (los tres primeros campos en little endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// Desde su forma de texto `AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE` (así se escriben en las
    /// especificaciones), escrito en tiempo de compilación.
    pub const fn parse(s: &str) -> Guid {
        let b = s.as_bytes();
        let mut hex = [0u8; 16];
        let (mut i, mut n) = (0, 0);
        while i < b.len() {
            if b[i] != b'-' {
                let hi = hexval(b[i]);
                let lo = hexval(b[i + 1]);
                hex[n] = hi << 4 | lo;
                n += 1;
                i += 1;
            }
            i += 1;
        }
        // Los campos 1–3 van dados vuelta en el disco.
        let mut out = hex;
        out[0] = hex[3];
        out[1] = hex[2];
        out[2] = hex[1];
        out[3] = hex[0];
        out[4] = hex[5];
        out[5] = hex[4];
        out[6] = hex[7];
        out[7] = hex[6];
        Guid(out)
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0; 16]
    }
}

const fn hexval(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

/// La partición de sistema EFI (de donde arranca el firmware).
pub const EFI_SYSTEM: Guid = Guid::parse("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
/// Los datos de JARVIS-OS (FAT32 con las carpetas del usuario). GUID propio, al azar.
pub const JARVIS_DATA: Guid = Guid::parse("4A4F5253-9A7D-4C1E-8B2F-4A4152564953");
/// "Microsoft basic data" (NTFS, FAT de Windows): la que tiene el SSD de Roman.
pub const MICROSOFT_BASIC: Guid = Guid::parse("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7");

/// CRC-32 (IEEE 802.3, el mismo de zip y Ethernet), bit a bit: la cabecera y las entradas se
/// calculan una vez, no hace falta tabla.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    pub kind: Guid,
    pub unique: Guid,
    pub first: u64,
    /// Último sector, **incluido**.
    pub last: u64,
    pub name: String,
}

impl Partition {
    pub fn sectors(&self) -> u64 {
        self.last + 1 - self.first
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub disk_guid: Guid,
    pub first_usable: u64,
    pub last_usable: u64,
    pub entries_lba: u64,
    pub entry_count: u32,
    pub entry_size: u32,
    pub entries_crc: u32,
}

const HEADER_SIZE: usize = 92;

/// Lee la cabecera de un sector. `None` si no es GPT o su CRC no da.
pub fn parse_header(sector: &[u8]) -> Option<Header> {
    if sector.len() < HEADER_SIZE || &sector[..8] != b"EFI PART" {
        return None;
    }
    let size = le(sector, 12, 4) as usize;
    if !(HEADER_SIZE..=sector.len()).contains(&size) {
        return None;
    }
    let mut copy = sector[..size].to_vec();
    copy[16..20].fill(0);
    if crc32(&copy) != le(sector, 16, 4) as u32 {
        return None;
    }
    let mut disk_guid = [0u8; 16];
    disk_guid.copy_from_slice(&sector[56..72]);
    let h = Header {
        disk_guid: Guid(disk_guid),
        first_usable: le(sector, 40, 8),
        last_usable: le(sector, 48, 8),
        entries_lba: le(sector, 72, 8),
        entry_count: le(sector, 80, 4) as u32,
        entry_size: le(sector, 84, 4) as u32,
        entries_crc: le(sector, 88, 4) as u32,
    };
    // Entradas de al menos 128 bytes, potencia de 2, y no más de 1024 (lo normal son 128).
    (h.entry_size >= 128 && h.entry_size.is_power_of_two() && h.entry_count <= 1024).then_some(h)
}

/// Las particiones usadas (tipo distinto de cero) de la tabla de entradas.
pub fn parse_entries(h: &Header, bytes: &[u8]) -> Option<Vec<Partition>> {
    let len = h.entry_count as usize * h.entry_size as usize;
    let table = bytes.get(..len)?;
    if crc32(table) != h.entries_crc {
        return None;
    }
    let mut out = Vec::new();
    for e in table.chunks(h.entry_size as usize) {
        let mut kind = [0u8; 16];
        kind.copy_from_slice(&e[..16]);
        if kind == [0; 16] {
            continue;
        }
        let mut unique = [0u8; 16];
        unique.copy_from_slice(&e[16..32]);
        let name: String = char::decode_utf16(
            e[56..128]
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0),
        )
        .map(|c| c.unwrap_or('?'))
        .collect();
        let (first, last) = (le(e, 32, 8), le(e, 40, 8));
        if last < first {
            continue;
        }
        out.push(Partition {
            kind: Guid(kind),
            unique: Guid(unique),
            first,
            last,
            name,
        });
    }
    Some(out)
}

/// Lee la GPT de un disco: la cabecera principal o, si está rota, la copia del final.
pub fn read(disk: &mut dyn BlockDevice) -> Option<Vec<Partition>> {
    let total = disk.sector_count();
    let mut sector = vec![0u8; SECTOR_SIZE];
    for lba in [1, total.checked_sub(1)?] {
        if disk.read(lba, &mut sector).is_err() {
            continue;
        }
        let Some(h) = parse_header(&sector) else {
            continue;
        };
        let bytes = (h.entry_count as usize * h.entry_size as usize).div_ceil(SECTOR_SIZE);
        let mut table = vec![0u8; bytes * SECTOR_SIZE];
        if h.entries_lba + bytes as u64 > total || disk.read(h.entries_lba, &mut table).is_err() {
            continue;
        }
        if let Some(parts) = parse_entries(&h, &table) {
            return Some(parts);
        }
    }
    None
}

/// ¿El disco está vacío? Sin firma de MBR (0x55AA en LBA 0) ni cabecera GPT (en LBA 1 ni al
/// final). Es la condición para que el instalador lo acepte (ADR 0011).
pub fn is_blank(disk: &mut dyn BlockDevice) -> Result<bool, IoError> {
    let total = disk.sector_count();
    let mut s = vec![0u8; SECTOR_SIZE];
    disk.read(0, &mut s)?;
    if s[510] == 0x55 && s[511] == 0xAA {
        return Ok(false);
    }
    for lba in [1, total.saturating_sub(1)] {
        disk.read(lba, &mut s)?;
        if &s[..8] == b"EFI PART" {
            return Ok(false);
        }
    }
    Ok(true)
}

/// ¿Este primer sector de una partición es el FAT del arranque de JARVIS-OS? El crate
/// `bootloader` le pone al volumen el nombre del archivo del kernel ("jarvis-kern", recortado a
/// 11 letras). La etiqueta está en el byte 43 en FAT12/16 y en el 71 en FAT32, cuando la firma
/// extendida (0x29) está presente. Así el instalador distingue su medio de arranque de la ESP
/// de Windows, que también es FAT y también es de tipo EFI.
pub fn is_jarvis_boot(boot_sector: &[u8]) -> bool {
    const LABEL: &[u8] = b"jarvis-kern";
    if boot_sector.len() < 512 || boot_sector[510..512] != [0x55, 0xAA] {
        return false;
    }
    let fat16 = boot_sector[38] == 0x29 && boot_sector[43..54].eq_ignore_ascii_case(LABEL);
    let fat32 = boot_sector[66] == 0x29 && boot_sector[71..82].eq_ignore_ascii_case(LABEL);
    fat16 || fat32
}

/// Una partición a crear: tipo, tamaño en sectores (0 = lo que queda) y nombre.
pub struct NewPartition<'a> {
    pub kind: Guid,
    pub sectors: u64,
    pub name: &'a str,
}

/// Cantidad de entradas (la estándar): 128 × 128 bytes = 32 sectores.
const ENTRIES: u32 = 128;
const ENTRY_SIZE: u32 = 128;
const ENTRY_SECTORS: u64 = (ENTRIES * ENTRY_SIZE) as u64 / SECTOR_SIZE as u64;
/// Las particiones empiezan alineadas a 1 MiB (2048 sectores), como hacen todas las
/// herramientas desde los discos de 4 KiB por sector.
const ALIGN: u64 = 2048;

/// Lo que hay que escribir en el disco: (sector, contenido).
pub struct Layout {
    pub writes: Vec<(u64, Vec<u8>)>,
    pub partitions: Vec<Partition>,
}

/// Arma un disco GPT nuevo de `total` sectores con esas particiones. `uniques` da un GUID único
/// para el disco y uno por partición (el llamador los saca del generador al azar).
pub fn create(total: u64, parts: &[NewPartition], uniques: &[Guid]) -> Option<Layout> {
    let first_usable = 2 + ENTRY_SECTORS;
    let last_usable = total.checked_sub(2 + ENTRY_SECTORS)?;
    let disk_guid = *uniques.first()?;
    let mut partitions = Vec::new();
    let mut next = ALIGN.max(first_usable);
    for (i, p) in parts.iter().enumerate() {
        let start = next.div_ceil(ALIGN) * ALIGN;
        let end = if p.sectors == 0 {
            last_usable
        } else {
            start + p.sectors - 1
        };
        if end > last_usable || end < start {
            return None;
        }
        partitions.push(Partition {
            kind: p.kind,
            unique: *uniques.get(i + 1)?,
            first: start,
            last: end,
            name: p.name.into(),
        });
        next = end + 1;
    }

    let mut entries = vec![0u8; (ENTRIES * ENTRY_SIZE) as usize];
    for (e, p) in entries.chunks_mut(ENTRY_SIZE as usize).zip(&partitions) {
        e[..16].copy_from_slice(&p.kind.0);
        e[16..32].copy_from_slice(&p.unique.0);
        e[32..40].copy_from_slice(&p.first.to_le_bytes());
        e[40..48].copy_from_slice(&p.last.to_le_bytes());
        for (i, c) in p.name.encode_utf16().take(36).enumerate() {
            e[56 + i * 2..58 + i * 2].copy_from_slice(&c.to_le_bytes());
        }
    }
    let entries_crc = crc32(&entries);
    let header = |my: u64, alternate: u64, entries_lba: u64| {
        let mut h = vec![0u8; SECTOR_SIZE];
        h[..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // revisión 1.0
        h[12..16].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        h[24..32].copy_from_slice(&my.to_le_bytes());
        h[32..40].copy_from_slice(&alternate.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&disk_guid.0);
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&ENTRIES.to_le_bytes());
        h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        let crc = crc32(&h[..HEADER_SIZE]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    };

    // MBR protector: una partición 0xEE desde el sector 1 hasta el final (o 0xFFFFFFFF).
    let mut mbr = vec![0u8; SECTOR_SIZE];
    let e = &mut mbr[446..462];
    e[1..4].copy_from_slice(&[0x00, 0x02, 0x00]); // CHS del inicio (sin sentido; el de siempre)
    e[4] = 0xEE;
    e[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    e[8..12].copy_from_slice(&1u32.to_le_bytes());
    e[12..16].copy_from_slice(&((total - 1).min(0xFFFF_FFFF) as u32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    let backup_entries = total - 1 - ENTRY_SECTORS;
    let writes = vec![
        (0, mbr),
        (1, header(1, total - 1, 2)),
        (2, entries.clone()),
        (backup_entries, entries),
        (total - 1, header(total - 1, 1, backup_entries)),
    ];
    Some(Layout { writes, partitions })
}

/// Una partición vista como un disco aparte: el sector 0 es su primer sector. Los accesos fuera
/// de ella son un error (no se puede pisar lo de al lado).
pub struct PartitionDevice<D> {
    disk: D,
    first: u64,
    sectors: u64,
}

impl<D: BlockDevice> PartitionDevice<D> {
    pub fn new(disk: D, first: u64, sectors: u64) -> PartitionDevice<D> {
        PartitionDevice {
            disk,
            first,
            sectors,
        }
    }

    /// El disco entero (para el instalador, que necesita ver la tabla).
    pub fn into_inner(self) -> D {
        self.disk
    }

    fn check(&self, lba: u64, len: usize) -> Result<u64, IoError> {
        let n = len.div_ceil(SECTOR_SIZE) as u64;
        if lba + n > self.sectors {
            return Err(IoError);
        }
        Ok(self.first + lba)
    }
}

impl<D: BlockDevice> BlockDevice for PartitionDevice<D> {
    fn sector_count(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let at = self.check(lba, buf.len())?;
        self.disk.read(at, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let at = self.check(lba, buf.len())?;
        self.disk.write(at, buf)
    }
}
