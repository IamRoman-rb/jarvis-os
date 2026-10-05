//! CD/DVD con ISO 9660 + El Torito: dónde está la imagen de arranque EFI.
//!
//! El instalador la necesita cuando JARVIS-OS arrancó de la ISO (`cargo xtask iso`) en vez de
//! un pendrive: la ESP del sistema no está en una partición GPT sino adentro del CD, como la
//! "imagen de arranque" que el catálogo de El Torito le señala al firmware.
//!
//! Recorrido (bloques de 2048 bytes):
//! 1. Bloque 17: el registro de arranque ("CD001" + "EL TORITO SPECIFICATION") con el bloque
//!    del catálogo.
//! 2. El catálogo: una entrada de validación (plataforma, firma 55 AA, suma cero), la entrada
//!    inicial y, opcionalmente, secciones (cabecera 0x90/0x91 + entradas) por plataforma. Las
//!    ISO de `xorriso` ponen x86 en la validación y EFI en una sección; la nuestra pone EFI en la
//!    validación.
//! 3. La entrada EFI "sin emulación" dice dónde empieza la imagen. Su tamaño en el catálogo es
//!    de 16 bits (sectores de 512): las imágenes de más de 32 MiB ponen 0. Por eso el tamaño
//!    sale del propio FAT (el BPB del primer sector de la imagen).
//!
//! Referencia: "El Torito" Bootable CD-ROM Format Specification 1.0 (§2.0–2.5); UEFI 2.10
//! §13.3.2.1 (plataforma 0xEF).

use alloc::vec;

/// Tamaño de bloque de un CD.
pub const BLOCK: usize = 2048;
/// El identificador de plataforma de UEFI en El Torito.
pub const PLATFORM_EFI: u8 = 0xEF;

/// La imagen de arranque EFI de un CD, en sectores de 512 bytes (como la ve un `BlockDevice`
/// que muestra el CD en sectores de 512).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootImage {
    pub first: u64,
    pub sectors: u64,
}

/// El bloque del catálogo, si `block17` es un registro de arranque de El Torito.
pub fn catalog_block(block17: &[u8]) -> Option<u32> {
    if block17.len() < 75 || block17[0] != 0 || &block17[1..6] != b"CD001" {
        return None;
    }
    if &block17[7..30] != b"EL TORITO SPECIFICATION" {
        return None;
    }
    Some(u32::from_le_bytes(block17[71..75].try_into().ok()?))
}

/// El bloque (de 2048) donde empieza la primera imagen EFI arrancable y su tamaño según el
/// catálogo (sectores de 512; 0 = "no entra en 16 bits").
pub fn efi_entry(catalog: &[u8]) -> Option<(u32, u16)> {
    let v = catalog.get(..32)?;
    if v[0] != 1 || v[30..32] != [0x55, 0xAA] {
        return None;
    }
    let sum = (0..16).fold(0u16, |acc, i| {
        acc.wrapping_add(u16::from_le_bytes([v[i * 2], v[i * 2 + 1]]))
    });
    if sum != 0 {
        return None;
    }
    // Una entrada arrancable y sin emulación: (bloque, sectores).
    let entry = |e: &[u8]| -> Option<(u32, u16)> {
        (e[0] == 0x88 && e[1] & 0x0F == 0).then(|| {
            (
                u32::from_le_bytes([e[8], e[9], e[10], e[11]]),
                u16::from_le_bytes([e[6], e[7]]),
            )
        })
    };
    if v[1] == PLATFORM_EFI
        && let Some(found) = entry(catalog.get(32..64)?)
    {
        return Some(found);
    }
    // Las secciones: cabecera (0x90 = hay más, 0x91 = la última) y sus entradas.
    let mut at = 64;
    while let Some(h) = catalog.get(at..at + 32) {
        if h[0] != 0x90 && h[0] != 0x91 {
            break;
        }
        let count = u16::from_le_bytes([h[2], h[3]]) as usize;
        for i in 0..count {
            let e = catalog.get(at + 32 * (i + 1)..at + 32 * (i + 2))?;
            if h[1] == PLATFORM_EFI
                && let Some(found) = entry(e)
            {
                return Some(found);
            }
        }
        if h[0] == 0x91 {
            break;
        }
        at += 32 * (count + 1);
    }
    None
}

/// Sectores de 512 de un volumen FAT según su BPB (0 si no parece un FAT).
pub fn fat_sectors(boot_sector: &[u8]) -> u64 {
    if boot_sector.len() < 36 || boot_sector[510..512] != [0x55, 0xAA] {
        return 0;
    }
    let bytes_per_sector = u64::from(u16::from_le_bytes([boot_sector[11], boot_sector[12]]));
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) {
        return 0;
    }
    let small = u64::from(u16::from_le_bytes([boot_sector[19], boot_sector[20]]));
    let large = u64::from(u32::from_le_bytes(
        boot_sector[32..36].try_into().unwrap_or([0; 4]),
    ));
    let total = if small != 0 { small } else { large };
    total * bytes_per_sector / 512
}

/// Busca la imagen de arranque EFI de un CD. `read` lee un bloque de 2048 bytes.
pub fn find_efi_image<E>(
    mut read: impl FnMut(u32, &mut [u8]) -> Result<(), E>,
) -> Option<BootImage> {
    let mut block = vec![0u8; BLOCK];
    read(17, &mut block).ok()?;
    let catalog = catalog_block(&block)?;
    read(catalog, &mut block).ok()?;
    let (start, count) = efi_entry(&block)?;
    read(start, &mut block).ok()?;
    let sectors = fat_sectors(&block[..512]).max(u64::from(count));
    (sectors > 0).then_some(BootImage {
        first: u64::from(start) * (BLOCK as u64 / 512),
        sectors,
    })
}
