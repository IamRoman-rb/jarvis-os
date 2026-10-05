//! La ISO de `cargo xtask iso` y las de `xorriso`: dónde está la imagen EFI. Y los comandos IDE.

use jarvis_drivers::ide::{self, Kind};
use jarvis_drivers::iso::{self, BLOCK, BootImage};

/// Un CD de `blocks` bloques en memoria.
struct Cd(Vec<u8>);

impl Cd {
    fn new(blocks: usize) -> Cd {
        Cd(vec![0; blocks * BLOCK])
    }
    fn block(&mut self, n: usize) -> &mut [u8] {
        &mut self.0[n * BLOCK..(n + 1) * BLOCK]
    }
    fn read(&self, n: u32, buf: &mut [u8]) -> Result<(), ()> {
        let at = n as usize * BLOCK;
        buf.copy_from_slice(self.0.get(at..at + BLOCK).ok_or(())?);
        Ok(())
    }
}

fn boot_record(cd: &mut Cd, catalog: u32) {
    let b = cd.block(17);
    b[1..6].copy_from_slice(b"CD001");
    b[6] = 1;
    b[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
    b[71..75].copy_from_slice(&catalog.to_le_bytes());
}

fn validation(cat: &mut [u8], platform: u8) {
    cat[0] = 1;
    cat[1] = platform;
    cat[30] = 0x55;
    cat[31] = 0xAA;
    let sum = (0..16).fold(0u16, |a, i| {
        a.wrapping_add(u16::from_le_bytes([cat[i * 2], cat[i * 2 + 1]]))
    });
    cat[28..30].copy_from_slice(&0u16.wrapping_sub(sum).to_le_bytes());
}

fn entry(e: &mut [u8], block: u32, count: u16) {
    e[0] = 0x88;
    e[6..8].copy_from_slice(&count.to_le_bytes());
    e[8..12].copy_from_slice(&block.to_le_bytes());
}

/// El primer sector de un FAT de `sectors` sectores de 512.
fn fat(b: &mut [u8], sectors: u32) {
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    if sectors < 0x10000 {
        b[19..21].copy_from_slice(&(sectors as u16).to_le_bytes());
    } else {
        b[32..36].copy_from_slice(&sectors.to_le_bytes());
    }
    b[510] = 0x55;
    b[511] = 0xAA;
}

#[test]
fn la_iso_de_xtask_con_imagen_grande() {
    // Como `xtask/src/iso.rs`: EFI en la validación, imagen en el bloque 23, conteo 0 (> 32 MiB).
    let mut cd = Cd::new(30);
    boot_record(&mut cd, 19);
    let cat = cd.block(19);
    validation(cat, iso::PLATFORM_EFI);
    entry(&mut cat[32..64], 23, 0);
    fat(cd.block(23), 73_728);
    let img = iso::find_efi_image(|n, b| cd.read(n, b));
    assert_eq!(
        img,
        Some(BootImage {
            first: 23 * 4,
            sectors: 73_728
        })
    );
}

#[test]
fn la_iso_de_xorriso_con_seccion_efi() {
    // Validación x86 (BIOS) + una sección EFI.
    let mut cd = Cd::new(40);
    boot_record(&mut cd, 20);
    let cat = cd.block(20);
    validation(cat, 0);
    entry(&mut cat[32..64], 30, 4); // la de BIOS
    cat[64] = 0x91;
    cat[65] = iso::PLATFORM_EFI;
    cat[66] = 1;
    entry(&mut cat[96..128], 31, 5760);
    fat(cd.block(31), 5760);
    assert_eq!(
        iso::find_efi_image(|n, b| cd.read(n, b)),
        Some(BootImage {
            first: 31 * 4,
            sectors: 5760
        })
    );
}

#[test]
fn sin_el_torito_o_catalogo_roto_no_hay_imagen() {
    let mut cd = Cd::new(30);
    assert_eq!(iso::find_efi_image(|n, b| cd.read(n, b)), None);
    boot_record(&mut cd, 19);
    let cat = cd.block(19);
    validation(cat, iso::PLATFORM_EFI);
    entry(&mut cat[32..64], 23, 0);
    cat[4] ^= 1; // la suma ya no da cero
    fat(cd.block(23), 100);
    assert_eq!(iso::find_efi_image(|n, b| cd.read(n, b)), None);
    // Solo BIOS: tampoco.
    let cat = cd.block(19);
    cat.fill(0);
    validation(cat, 0);
    entry(&mut cat[32..64], 23, 4);
    assert_eq!(iso::find_efi_image(|n, b| cd.read(n, b)), None);
}

#[test]
fn comandos_ide() {
    assert_eq!(ide::signature(0, 0), Some(Kind::Ata));
    assert_eq!(ide::signature(0x14, 0xEB), Some(Kind::Atapi));
    assert_eq!(ide::signature(0xFF, 0xFF), None);
    assert_eq!(
        ide::read10(0x0102_0304, 2),
        [0x28, 0, 1, 2, 3, 4, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(
        ide::parse_capacity(&[0, 0, 0x48, 0x16, 0, 0, 8, 0]),
        Some((18455, 2048))
    );
    assert_eq!(ide::parse_capacity(&[0; 8]), None);
    assert_eq!(
        ide::lba48_passes(0x0605_0403_0201, 0x0180),
        [[0x01, 0x04, 0x05, 0x06], [0x80, 0x01, 0x02, 0x03]]
    );
}
