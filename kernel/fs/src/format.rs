//! Formatear un FAT32 vacío (el disco en RAM del modo en vivo, cuando se arranca desde la ISO).
//!
//! Estructura, según la especificación de Microsoft:
//! - sector 0: el sector de arranque con el BPB; sector 1: FSInfo; sector 6: copia del 0;
//! - 32 sectores reservados, y después las dos copias de la FAT;
//! - el cluster 2 es la carpeta raíz.

use alloc::vec;

use crate::device::{BlockDevice, IoError, SECTOR_SIZE};

/// FAT32 necesita al menos 65 525 clusters.
const MIN_CLUSTERS: u64 = 65_525;
const RESERVED: u64 = 32;

/// Formatea todo el dispositivo. Elige el cluster más grande que deje al menos 65 525 clusters
/// (hasta 4 KiB). `label`: hasta 11 caracteres ASCII.
pub fn format_fat32<D: BlockDevice>(dev: &mut D, label: &str) -> Result<(), IoError> {
    let total = dev.sector_count();
    let mut spc = 8u64;
    while spc > 1 && total / spc < MIN_CLUSTERS + 1024 {
        spc /= 2;
    }
    // Tamaño de cada FAT: 4 bytes por cluster (más los dos primeros) redondeado a sectores.
    let clusters_guess = (total - RESERVED) / spc;
    let fat_size = ((clusters_guess + 2) * 4).div_ceil(SECTOR_SIZE as u64);
    let data_start = RESERVED + 2 * fat_size;
    let clusters = (total - data_start) / spc;
    if clusters < MIN_CLUSTERS {
        return Err(IoError);
    }

    let mut bs = [0u8; SECTOR_SIZE];
    bs[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"JARVISOS");
    bs[11..13].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    bs[13] = spc as u8;
    bs[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    bs[16] = 2; // FATs
    bs[21] = 0xF8; // disco fijo
    bs[24..26].copy_from_slice(&63u16.to_le_bytes());
    bs[26..28].copy_from_slice(&255u16.to_le_bytes());
    bs[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    bs[36..40].copy_from_slice(&(fat_size as u32).to_le_bytes());
    bs[44..48].copy_from_slice(&2u32.to_le_bytes()); // carpeta raíz
    bs[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo
    bs[50..52].copy_from_slice(&6u16.to_le_bytes()); // copia del arranque
    bs[64] = 0x80;
    bs[66] = 0x29;
    bs[67..71].copy_from_slice(&0x4A52_5653u32.to_le_bytes());
    let mut name = [b' '; 11];
    for (d, s) in name.iter_mut().zip(label.bytes().filter(u8::is_ascii)) {
        *d = s.to_ascii_uppercase();
    }
    bs[71..82].copy_from_slice(&name);
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xAA;

    let mut info = [0u8; SECTOR_SIZE];
    info[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    info[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    info[488..492].copy_from_slice(&((clusters - 1) as u32).to_le_bytes()); // libres
    info[492..496].copy_from_slice(&3u32.to_le_bytes()); // el próximo libre
    info[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());

    // Los reservados en cero, salvo arranque, FSInfo y sus copias.
    let zero = vec![0u8; SECTOR_SIZE * 32];
    dev.write(0, &zero)?;
    dev.write(0, &bs)?;
    dev.write(1, &info)?;
    dev.write(6, &bs)?;
    dev.write(7, &info)?;

    // Las FAT: las dos primeras entradas reservadas y el cluster 2 (la raíz) como fin de cadena.
    let mut first = [0u8; SECTOR_SIZE];
    first[..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    first[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    first[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    for fat in 0..2 {
        let start = RESERVED + fat * fat_size;
        let mut lba = start;
        while lba < start + fat_size {
            let n = (start + fat_size - lba).min(32);
            dev.write(lba, &zero[..n as usize * SECTOR_SIZE])?;
            lba += n;
        }
        dev.write(start, &first)?;
    }
    // La carpeta raíz, vacía.
    dev.write(data_start, &zero[..spc as usize * SECTOR_SIZE])
}
