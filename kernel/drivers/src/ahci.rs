//! AHCI: la controladora SATA de las PC desde ~2008 (el SSD de Roman está en una).
//!
//! La controladora tiene hasta 32 **puertos**, uno por disco. Cada puerto tiene una "lista de
//! comandos" en memoria con 32 ranuras. Para leer o escribir, el driver:
//!
//! 1. arma en una ranura la **cabecera** (cuántas palabras mide el comando, si escribe, cuántos
//!    tramos de memoria usa y dónde está la tabla del comando);
//! 2. en la **tabla** pone el comando ATA como un FIS "Register Host to Device" (el mismo
//!    formato que viaja por el cable SATA) y la lista de tramos de memoria (PRDT) para el DMA;
//! 3. prende el bit de esa ranura en `PxCI` y espera a que la controladora lo apague.
//!
//! Acá se arman esos bytes y se lee la respuesta de IDENTIFY. Los registros los toca
//! `kernel/src/ahci.rs`.
//! Referencias: especificación AHCI 1.3.1 (Intel), ATA8-ACS (comandos) y
//! <https://wiki.osdev.org/AHCI>.

use alloc::string::String;

use crate::le;

/// Comandos ATA que usa el driver.
pub const ATA_READ_DMA_EXT: u8 = 0x25;
pub const ATA_WRITE_DMA_EXT: u8 = 0x35;
/// Escribe y no avisa hasta que el dato está en el medio (no en la caché del disco).
pub const ATA_WRITE_DMA_FUA_EXT: u8 = 0x3D;
pub const ATA_FLUSH_CACHE_EXT: u8 = 0xEA;
pub const ATA_IDENTIFY: u8 = 0xEC;

/// Firma de un puerto con un disco SATA (los ATAPI, como las lectoras de CD, dan 0xEB140101).
pub const SIG_SATA: u32 = 0x0000_0101;

/// Tamaño de una tabla de comando con `prdt` tramos: 128 bytes fijos + 16 por tramo.
pub fn command_table_size(prdt: usize) -> usize {
    0x80 + 16 * prdt
}

/// El FIS "Register Host to Device" (20 bytes) de un comando ATA con LBA de 48 bits.
pub fn h2d_fis(command: u8, lba: u64, count: u16) -> [u8; 20] {
    let mut f = [0u8; 20];
    f[0] = 0x27; // tipo: Register H2D
    f[1] = 0x80; // bit C: es un comando (no un control)
    f[2] = command;
    f[4] = lba as u8;
    f[5] = (lba >> 8) as u8;
    f[6] = (lba >> 16) as u8;
    f[7] = 1 << 6; // dispositivo: modo LBA
    f[8] = (lba >> 24) as u8;
    f[9] = (lba >> 32) as u8;
    f[10] = (lba >> 40) as u8;
    f[12] = count as u8;
    f[13] = (count >> 8) as u8;
    f
}

/// La cabecera de una ranura de la lista de comandos (32 bytes). `fis_dwords`: largo del FIS
/// en palabras de 4 bytes (5 para el H2D); `write`: el disco lee de la memoria.
pub fn command_header(fis_dwords: u8, write: bool, prdt_len: u16, table_phys: u64) -> [u8; 32] {
    let mut h = [0u8; 32];
    h[0] = (fis_dwords & 0x1F) | (write as u8) << 6;
    h[2..4].copy_from_slice(&prdt_len.to_le_bytes());
    h[8..16].copy_from_slice(&table_phys.to_le_bytes());
    h
}

/// Un tramo de la PRDT (16 bytes): dirección física y cantidad de bytes (hasta 4 MiB, par).
pub fn prdt_entry(phys: u64, bytes: u32) -> [u8; 16] {
    let mut e = [0u8; 16];
    e[..8].copy_from_slice(&phys.to_le_bytes());
    // Bits 0–21: bytes − 1. Bit 31: interrumpir al terminar este tramo.
    let dbc = (bytes - 1) & 0x3F_FFFF | 1 << 31;
    e[12..16].copy_from_slice(&dbc.to_le_bytes());
    e
}

/// Lo que dice un disco de sí mismo (IDENTIFY DEVICE, 512 bytes = 256 palabras).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub model: String,
    pub serial: String,
    pub sectors: u64,
    pub sector_size: u32,
    /// Admite LBA de 48 bits (sin eso no se puede pasar de 128 GiB; los que no lo tienen, no se
    /// usan).
    pub lba48: bool,
    /// Admite escrituras "FUA" (palabra 84, bit 6): así un corte de luz no deja el FAT a medias
    /// sin tener que vaciar toda la caché del disco después de cada escritura.
    pub fua: bool,
}

/// Los textos de IDENTIFY vienen con los bytes de cada palabra dados vuelta.
pub fn ata_string(words: &[u8]) -> String {
    let straight: alloc::vec::Vec<u8> = words
        .chunks(2)
        .flat_map(|pair| pair.iter().rev().copied())
        .collect();
    crate::ascii(&straight)
}

pub fn parse_identify(d: &[u8]) -> Option<Identity> {
    if d.len() < 512 {
        return None;
    }
    let word = |n: usize| le(d, n * 2, 2) as u16;
    let lba48 = word(83) & (1 << 10) != 0;
    let sectors = if lba48 {
        le(d, 100 * 2, 8)
    } else {
        le(d, 60 * 2, 4)
    };
    // Palabra 106: bit 14 = válida, bit 12 = sector lógico de más de 256 palabras (el tamaño
    // en palabras está en 117–118).
    let w106 = word(106);
    let sector_size = if w106 & 0xC000 == 0x4000 && w106 & (1 << 12) != 0 {
        le(d, 117 * 2, 4) as u32 * 2
    } else {
        512
    };
    Some(Identity {
        model: ata_string(&d[54..94]),
        serial: ata_string(&d[20..40]),
        sectors,
        sector_size,
        lba48,
        fua: word(84) & (1 << 6) != 0,
    })
}

/// Bits de `PxIS`/`PxTFD` que indican error.
pub const TFD_ERR: u32 = 1 << 0;
pub const TFD_BUSY: u32 = 1 << 7;
pub const TFD_DRQ: u32 = 1 << 3;
/// `PxIS.TFES`: la tarea terminó con error.
pub const IS_TFES: u32 = 1 << 30;
