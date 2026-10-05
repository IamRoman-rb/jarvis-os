//! Discos y lectoras IDE (ATA/ATAPI por PIO): lo que no toca puertos.
//!
//! Es la controladora que VirtualBox pone por defecto (PIIX3/PIIX4) y la de muchas máquinas
//! viejas. Cada canal (primario 0x1F0, secundario 0x170) tiene un "maestro" y un "esclavo".
//! Después de un reinicio de software, los registros LBA medio/alto dicen qué hay: 00 00 = disco
//! ATA, 14 EB = lectora ATAPI (CD/DVD). Los discos se leen con READ SECTORS EXT (LBA48) y las
//! lectoras con paquetes SCSI (READ(10), bloques de 2048).
//!
//! Referencias: ATA/ATAPI-6 (T13/1410D) §8 y §9 (protocolos PIO y PACKET),
//! <https://wiki.osdev.org/ATA_PIO_Mode>, <https://wiki.osdev.org/ATAPI>.

/// Comandos ATA que se usan.
pub const ATA_IDENTIFY: u8 = 0xEC;
pub const ATA_IDENTIFY_PACKET: u8 = 0xA1;
pub const ATA_READ_EXT: u8 = 0x24;
pub const ATA_WRITE_EXT: u8 = 0x34;
pub const ATA_FLUSH_EXT: u8 = 0xEA;
pub const ATA_PACKET: u8 = 0xA0;

/// Bits del registro de estado.
pub const ST_ERR: u8 = 1 << 0;
pub const ST_DRQ: u8 = 1 << 3;
pub const ST_DF: u8 = 1 << 5;
pub const ST_BSY: u8 = 1 << 7;

/// Sectores por comando de lectura/escritura (el máximo de LBA48 es 65536; 128 = 64 KiB).
pub const MAX_SECTORS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ata,
    Atapi,
}

/// Qué hay en una posición según la firma (LBA medio y alto después del reinicio).
pub fn signature(mid: u8, high: u8) -> Option<Kind> {
    match (mid, high) {
        (0x00, 0x00) => Some(Kind::Ata),
        (0x14, 0xEB) | (0x69, 0x96) => Some(Kind::Atapi),
        _ => None,
    }
}

/// El paquete SCSI READ(10): `count` bloques desde `lba`.
pub fn read10(lba: u32, count: u16) -> [u8; 12] {
    let l = lba.to_be_bytes();
    let c = count.to_be_bytes();
    [0x28, 0, l[0], l[1], l[2], l[3], 0, c[0], c[1], 0, 0, 0]
}

/// El paquete SCSI START STOP UNIT con LoEj = 1 y Start = 0: expulsar el disco.
pub const START_STOP_EJECT: [u8; 12] = [0x1B, 0, 0, 0, 0x02, 0, 0, 0, 0, 0, 0, 0];

/// El paquete SCSI READ CAPACITY(10).
pub const READ_CAPACITY: [u8; 12] = [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// La respuesta de READ CAPACITY(10): (cantidad de bloques, tamaño de bloque).
pub fn parse_capacity(r: &[u8]) -> Option<(u64, u32)> {
    let last = u32::from_be_bytes(r.get(..4)?.try_into().ok()?);
    let size = u32::from_be_bytes(r.get(4..8)?.try_into().ok()?);
    (size != 0).then_some((u64::from(last) + 1, size))
}

/// Un comando LBA48 escribe dos veces los registros de conteo y LBA: primero los bytes altos
/// (conteo 15–8, LBA 31–24, 39–32, 47–40) y después los bajos. Devuelve las dos pasadas como
/// (conteo, LBA bajo, medio, alto). `count` = 0 significa 65536.
pub fn lba48_passes(lba: u64, count: u16) -> [[u8; 4]; 2] {
    let l = lba.to_le_bytes();
    let c = count.to_le_bytes();
    [[c[1], l[3], l[4], l[5]], [c[0], l[0], l[1], l[2]]]
}
