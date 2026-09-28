//! `cargo xtask iso`: una ISO 9660 con El Torito para arrancar JARVIS desde un CD/USB virtual o
//! una VM (ADR 0007). Escritor propio y mínimo, para no depender de `xorriso`.
//!
//! La imagen de arranque es la partición EFI (FAT) de `jarvis-os-uefi.img`, que ya tiene el
//! cargador UEFI y el kernel. El catálogo de El Torito la marca como "EFI, sin emulación": el
//! firmware la monta y ejecuta `EFI/BOOT/BOOTX64.EFI`. Sin disco, el kernel arranca en modo en
//! vivo (FAT32 en RAM).
//!
//! Sectores de 2048 bytes:
//! 0–15 vacíos · 16 descriptor primario · 17 registro de arranque (El Torito) · 18 fin de
//! descriptores · 19 catálogo de arranque · 20–21 tablas de rutas · 22 carpeta raíz · 23… la
//! imagen EFI (también visible como `EFIBOOT.IMG`).

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::{Result, target_dir};

const S: usize = 2048;
const CATALOG: u32 = 19;
const PATH_L: u32 = 20;
const PATH_M: u32 = 21;
const ROOT: u32 = 22;
const IMAGE: u32 = 23;

/// La partición EFI de la imagen GPT: sus bytes.
fn esp_from_gpt(img: &Path) -> Result<Vec<u8>> {
    let io = |e: std::io::Error| format!("{}: {e}", img.display());
    let mut f = fs::File::open(img).map_err(io)?;
    let mut hdr = [0u8; 512];
    f.seek(SeekFrom::Start(512)).map_err(io)?;
    f.read_exact(&mut hdr).map_err(io)?;
    if &hdr[..8] != b"EFI PART" {
        return Err("la imagen UEFI no tiene tabla GPT".into());
    }
    let entries = u64::from_le_bytes(hdr[72..80].try_into().expect("8"));
    let mut e = [0u8; 128];
    f.seek(SeekFrom::Start(entries * 512)).map_err(io)?;
    f.read_exact(&mut e).map_err(io)?;
    let first = u64::from_le_bytes(e[32..40].try_into().expect("8"));
    let last = u64::from_le_bytes(e[40..48].try_into().expect("8"));
    if first == 0 || last < first {
        return Err("la imagen UEFI no tiene partición EFI".into());
    }
    let mut data = vec![0u8; ((last - first + 1) * 512) as usize];
    f.seek(SeekFrom::Start(first * 512)).map_err(io)?;
    f.read_exact(&mut data).map_err(io)?;
    Ok(data)
}

fn both16(b: &mut [u8], v: u16) {
    b[..2].copy_from_slice(&v.to_le_bytes());
    b[2..4].copy_from_slice(&v.to_be_bytes());
}

fn both32(b: &mut [u8], v: u32) {
    b[..4].copy_from_slice(&v.to_le_bytes());
    b[4..8].copy_from_slice(&v.to_be_bytes());
}

/// Un registro de carpeta (ECMA-119 9.1).
fn dir_record(name: &[u8], lba: u32, len: u32, dir: bool) -> Vec<u8> {
    let mut r = vec![0u8; 33 + name.len() + (name.len() + 1) % 2];
    r[0] = r.len() as u8;
    both32(&mut r[2..10], lba);
    both32(&mut r[10..18], len);
    r[18..25].copy_from_slice(&[126, 9, 27, 12, 0, 0, 0]); // 2026-09-27
    r[25] = if dir { 2 } else { 0 };
    both16(&mut r[28..32], 1);
    r[32] = name.len() as u8;
    r[33..33 + name.len()].copy_from_slice(name);
    r
}

fn text(b: &mut [u8], s: &str) {
    b.fill(b' ');
    b[..s.len()].copy_from_slice(s.as_bytes());
}

pub fn build(uefi_img: &Path) -> Result<PathBuf> {
    let esp = esp_from_gpt(uefi_img)?;
    let esp_sectors = esp.len().div_ceil(S) as u32;
    let total = IMAGE + esp_sectors;
    let mut iso = vec![0u8; total as usize * S];

    // Descriptor primario.
    let pvd = &mut iso[16 * S..17 * S];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    text(&mut pvd[8..40], "");
    text(&mut pvd[40..72], "JARVIS_OS");
    both32(&mut pvd[80..88], total);
    both16(&mut pvd[120..124], 1);
    both16(&mut pvd[124..128], 1);
    both16(&mut pvd[128..132], S as u16);
    both32(&mut pvd[132..140], 10);
    pvd[140..144].copy_from_slice(&PATH_L.to_le_bytes());
    pvd[148..152].copy_from_slice(&PATH_M.to_be_bytes());
    pvd[156..190].copy_from_slice(&dir_record(&[0], ROOT, S as u32, true));
    for r in [
        190..318,
        318..446,
        446..574,
        574..702,
        702..739,
        739..776,
        776..813,
    ] {
        text(&mut pvd[r], "");
    }
    text(&mut pvd[574..702], "JARVIS-OS (cargo xtask iso)");
    for at in [813, 830, 847, 864] {
        pvd[at..at + 16].copy_from_slice(b"0000000000000000");
    }
    pvd[813..829].copy_from_slice(b"2026092700000000");
    pvd[881] = 1;

    // Registro de arranque de El Torito.
    let br = &mut iso[17 * S..18 * S];
    br[1..6].copy_from_slice(b"CD001");
    br[6] = 1;
    br[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
    br[71..75].copy_from_slice(&CATALOG.to_le_bytes());

    // Fin de los descriptores.
    let end = &mut iso[18 * S..19 * S];
    end[0] = 255;
    end[1..6].copy_from_slice(b"CD001");
    end[6] = 1;

    // Catálogo: entrada de validación (plataforma EFI) + la entrada por defecto.
    let cat = &mut iso[CATALOG as usize * S..(CATALOG as usize + 1) * S];
    cat[0] = 1;
    cat[1] = 0xEF;
    cat[4..14].copy_from_slice(b"JARVIS-OS ");
    cat[30] = 0x55;
    cat[31] = 0xAA;
    let sum = (0..16).fold(0u16, |acc, i| {
        acc.wrapping_add(u16::from_le_bytes([cat[i * 2], cat[i * 2 + 1]]))
    });
    cat[28..30].copy_from_slice(&0u16.wrapping_sub(sum).to_le_bytes());
    cat[32] = 0x88; // arrancable
    cat[33] = 0; // sin emulación
    // En sectores de 512; si no entra en 16 bits, el firmware usa hasta el final del volumen.
    let count = u16::try_from(esp.len() / 512).unwrap_or(0);
    cat[38..40].copy_from_slice(&count.to_le_bytes());
    cat[40..44].copy_from_slice(&IMAGE.to_le_bytes());

    // Tablas de rutas: solo la raíz.
    let mut pt = [0u8; 10];
    pt[0] = 1;
    pt[2..6].copy_from_slice(&ROOT.to_le_bytes());
    pt[6..8].copy_from_slice(&1u16.to_le_bytes());
    iso[PATH_L as usize * S..PATH_L as usize * S + 10].copy_from_slice(&pt);
    pt[2..6].copy_from_slice(&ROOT.to_be_bytes());
    pt[6..8].copy_from_slice(&1u16.to_be_bytes());
    iso[PATH_M as usize * S..PATH_M as usize * S + 10].copy_from_slice(&pt);

    // Carpeta raíz: ".", ".." y la imagen EFI.
    let mut root = dir_record(&[0], ROOT, S as u32, true);
    root.extend(dir_record(&[1], ROOT, S as u32, true));
    root.extend(dir_record(b"EFIBOOT.IMG;1", IMAGE, esp.len() as u32, false));
    iso[ROOT as usize * S..ROOT as usize * S + root.len()].copy_from_slice(&root);

    iso[IMAGE as usize * S..IMAGE as usize * S + esp.len()].copy_from_slice(&esp);
    let out = target_dir().join("jarvis-os.iso");
    fs::write(&out, &iso).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(out)
}
