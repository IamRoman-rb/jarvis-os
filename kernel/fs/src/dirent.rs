//! Entradas de directorio de FAT: la corta (8.3) y las de nombre largo (LFN).
//!
//! Entrada corta (32 bytes):
//! ```text
//! 0  nombre 8.3 (11 bytes, "NOTAS   TXT")   11 atributos   12 minúsculas (NT)
//! 13 décimas de creación   14 hora creación  16 fecha creación   18 último acceso
//! 20 cluster alto (16 bits)  22 hora modif.  24 fecha modif.  26 cluster bajo  28 tamaño
//! ```
//! Un nombre largo ocupa entradas LFN (atributo 0x0F) **antes** de la corta, de a 13 caracteres
//! UTF-16, en orden inverso: la primera en disco es la última parte y lleva el bit 0x40. Cada una
//! guarda un checksum del nombre corto, para detectar LFN "huérfanas" si otro sistema borró la
//! entrada corta sin saber de nombres largos.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{FsError, Result};

pub const ENTRY_SIZE: usize = 32;
pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LFN: u8 = 0x0F;
/// Primer byte del nombre: entrada borrada.
pub const DELETED: u8 = 0xE5;
/// Primer byte del nombre: fin del directorio (no hay más entradas después).
pub const END: u8 = 0x00;
const LFN_CHARS: usize = 13;
const MAX_NAME_LEN: usize = 255;

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

/// Entrada corta ya decodificada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawEntry {
    pub name: [u8; 11],
    pub attr: u8,
    pub ntres: u8,
    pub crt_tenth: u8,
    pub crt_time: u16,
    pub crt_date: u16,
    pub acc_date: u16,
    pub cluster: u32,
    pub wrt_time: u16,
    pub wrt_date: u16,
    pub size: u32,
}

impl RawEntry {
    pub fn parse(b: &[u8]) -> RawEntry {
        let mut name = [0u8; 11];
        name.copy_from_slice(&b[0..11]);
        RawEntry {
            name,
            attr: b[11],
            ntres: b[12],
            crt_tenth: b[13],
            crt_time: u16_at(b, 14),
            crt_date: u16_at(b, 16),
            acc_date: u16_at(b, 18),
            cluster: ((u16_at(b, 20) as u32) << 16) | u16_at(b, 26) as u32,
            wrt_time: u16_at(b, 22),
            wrt_date: u16_at(b, 24),
            size: u32_at(b, 28),
        }
    }

    pub fn write(&self, b: &mut [u8]) {
        b[0..11].copy_from_slice(&self.name);
        b[11] = self.attr;
        b[12] = self.ntres;
        b[13] = self.crt_tenth;
        b[14..16].copy_from_slice(&self.crt_time.to_le_bytes());
        b[16..18].copy_from_slice(&self.crt_date.to_le_bytes());
        b[18..20].copy_from_slice(&self.acc_date.to_le_bytes());
        b[20..22].copy_from_slice(&((self.cluster >> 16) as u16).to_le_bytes());
        b[22..24].copy_from_slice(&self.wrt_time.to_le_bytes());
        b[24..26].copy_from_slice(&self.wrt_date.to_le_bytes());
        b[26..28].copy_from_slice(&(self.cluster as u16).to_le_bytes());
        b[28..32].copy_from_slice(&self.size.to_le_bytes());
    }

    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }

    pub fn is_dot(&self) -> bool {
        self.name[0] == b'.'
    }
}

/// Checksum del nombre corto que llevan todas sus entradas LFN.
pub fn lfn_checksum(short: &[u8; 11]) -> u8 {
    short.iter().fold(0u8, |sum, &c| {
        ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(c)
    })
}

/// Posiciones (en bytes) de los 13 caracteres UTF-16 dentro de una entrada LFN.
const LFN_OFFSETS: [usize; LFN_CHARS] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

/// Datos de una entrada LFN: número de orden (con el bit 0x40 de "última"), checksum y caracteres.
pub fn parse_lfn(b: &[u8]) -> (u8, u8, [u16; LFN_CHARS]) {
    let mut chars = [0u16; LFN_CHARS];
    for (c, &off) in chars.iter_mut().zip(LFN_OFFSETS.iter()) {
        *c = u16_at(b, off);
    }
    (b[0], b[13], chars)
}

/// Arma las entradas LFN de `name`, en el orden en que van en disco (la última parte primero).
pub fn encode_lfn(name: &str, short: &[u8; 11]) -> Vec<[u8; ENTRY_SIZE]> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let parts = units.len().div_ceil(LFN_CHARS);
    let checksum = lfn_checksum(short);
    (0..parts)
        .rev()
        .map(|part| {
            let mut e = [0u8; ENTRY_SIZE];
            let ord = (part + 1) as u8 | if part + 1 == parts { 0x40 } else { 0 };
            e[0] = ord;
            e[11] = ATTR_LFN;
            e[13] = checksum;
            for (i, &off) in LFN_OFFSETS.iter().enumerate() {
                let idx = part * LFN_CHARS + i;
                // Después del nombre va un 0x0000 y el resto se rellena con 0xFFFF.
                let unit = match idx.cmp(&units.len()) {
                    core::cmp::Ordering::Less => units[idx],
                    core::cmp::Ordering::Equal => 0x0000,
                    core::cmp::Ordering::Greater => 0xFFFF,
                };
                e[off..off + 2].copy_from_slice(&unit.to_le_bytes());
            }
            e
        })
        .collect()
}

/// Nombre corto legible: "NOTAS.TXT", respetando las marcas de minúsculas que usa Windows.
pub fn short_display(raw: &RawEntry) -> String {
    let decode = |bytes: &[u8], lower: bool| -> String {
        let mut s = String::new();
        for (i, &b) in bytes.iter().enumerate() {
            let b = if i == 0 && b == 0x05 { DELETED } else { b };
            let c = b as char; // página de códigos aproximada: Latin-1
            s.push(if lower { c.to_ascii_lowercase() } else { c });
        }
        String::from(s.trim_end())
    };
    let base = decode(&raw.name[0..8], raw.ntres & 0x08 != 0);
    let ext = decode(&raw.name[8..11], raw.ntres & 0x10 != 0);
    if ext.is_empty() {
        base
    } else {
        alloc::format!("{base}.{ext}")
    }
}

/// Valida un nombre de archivo o carpeta (reglas de Windows, que son las de FAT).
pub fn validate_name(name: &str) -> Result<()> {
    let invalid =
        |c: char| c < ' ' || matches!(c, '"' | '*' | '/' | ':' | '<' | '>' | '?' | '\\' | '|');
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.encode_utf16().count() > MAX_NAME_LEN
        || name.chars().any(invalid)
        || name.ends_with(['.', ' '])
        || name.starts_with(' ')
    {
        return Err(FsError::InvalidName);
    }
    Ok(())
}

fn is_short_char(c: u8) -> bool {
    c.is_ascii_uppercase()
        || c.is_ascii_digit()
        || matches!(
            c,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'-'
                | b'@'
                | b'^'
                | b'_'
                | b'`'
                | b'{'
                | b'}'
                | b'~'
        )
}

/// Si `name` ya es un nombre 8.3 válido en mayúsculas ("README.TXT"), no hace falta LFN.
pub fn exact_short(name: &str) -> Option<[u8; 11]> {
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    let ok = |s: &str, max: usize| s.len() <= max && s.bytes().all(is_short_char);
    if base.is_empty() || !ok(base, 8) || !ok(ext, 3) {
        return None;
    }
    let mut out = [b' '; 11];
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    Some(out)
}

fn to_short_bytes(s: &str, max: usize) -> Vec<u8> {
    s.chars()
        .filter(|&c| c != ' ' && c != '.')
        .map(|c| {
            let u = c.to_ascii_uppercase();
            if u.is_ascii() && is_short_char(u as u8) {
                u as u8
            } else {
                b'_'
            }
        })
        .take(max)
        .collect()
}

/// Genera un alias corto único para un nombre largo: "Notas de álgebra.txt" → "NOTASD~1.TXT".
pub fn make_short(name: &str, existing: &[[u8; 11]]) -> Result<[u8; 11]> {
    if let Some(exact) = exact_short(name)
        && !existing.contains(&exact)
    {
        return Ok(exact);
    }
    let trimmed = name.trim_start_matches('.');
    let (base, ext) = match trimmed.rsplit_once('.') {
        Some((b, e)) if !b.is_empty() => (b, e),
        _ => (trimmed, ""),
    };
    let mut base = to_short_bytes(base, 8);
    if base.is_empty() {
        base.push(b'_');
    }
    let ext = to_short_bytes(ext, 3);
    for n in 1..1_000_000u32 {
        let tail = alloc::format!("~{n}");
        let keep = base.len().min(8 - tail.len());
        let mut out = [b' '; 11];
        out[..keep].copy_from_slice(&base[..keep]);
        out[keep..keep + tail.len()].copy_from_slice(tail.as_bytes());
        out[8..8 + ext.len()].copy_from_slice(&ext);
        if !existing.contains(&out) {
            return Ok(out);
        }
    }
    Err(FsError::NoSpace)
}

/// Comparación de nombres sin distinguir mayúsculas (como hace FAT/Windows).
pub fn same_name(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(bytes: &[u8; 11]) -> &str {
        core::str::from_utf8(bytes).unwrap()
    }

    #[test]
    fn nombres_cortos_exactos_y_alias() {
        assert_eq!(s(&exact_short("README.TXT").unwrap()), "README  TXT");
        assert!(exact_short("readme.txt").is_none());
        assert!(exact_short("NOMBRELARGO.TXT").is_none());
        let a = make_short("Notas de álgebra.txt", &[]).unwrap();
        assert_eq!(s(&a), "NOTASD~1TXT");
        let b = make_short("Notas de álgebra 2.txt", &[a]).unwrap();
        assert_eq!(s(&b), "NOTASD~2TXT");
        assert_eq!(s(&make_short(".bashrc", &[]).unwrap()), "BASHRC~1   ");
        assert_eq!(s(&make_short("a.b.c.tar.gz", &[]).unwrap()), "ABCTAR~1GZ ");
    }

    #[test]
    fn lfn_ida_y_vuelta() {
        let short = make_short("Álgebra y Geometría — resumen.txt", &[]).unwrap();
        let entries = encode_lfn("Álgebra y Geometría — resumen.txt", &short);
        assert_eq!(entries.len(), 3); // 33 caracteres → 3 entradas de 13
        assert_eq!(entries[0][0], 0x43); // última parte primero, con el bit 0x40
        let mut units = Vec::new();
        for e in entries.iter().rev() {
            let (_, checksum, chars) = parse_lfn(e);
            assert_eq!(checksum, lfn_checksum(&short));
            units.extend(chars.iter().copied().take_while(|&c| c != 0));
        }
        assert_eq!(
            String::from_utf16(&units).unwrap(),
            "Álgebra y Geometría — resumen.txt"
        );
    }

    #[test]
    fn validacion_de_nombres() {
        for bad in [
            "", ".", "..", "a/b", "a:b", "que?", "fin.", "fin ", " inicio", "\u{1}",
        ] {
            assert_eq!(validate_name(bad), Err(FsError::InvalidName), "{bad:?}");
        }
        for good in ["notas.txt", "Álgebra", ".config", "a b c", "archivo (2).md"] {
            assert_eq!(validate_name(good), Ok(()), "{good:?}");
        }
    }

    #[test]
    fn comparacion_sin_mayusculas() {
        assert!(same_name("Documentos", "DOCUMENTOS"));
        assert!(same_name("ÁLGEBRA", "álgebra"));
        assert!(!same_name("notas", "nota"));
    }

    #[test]
    fn entrada_corta_ida_y_vuelta() {
        let raw = RawEntry {
            name: *b"NOTAS   TXT",
            attr: ATTR_ARCHIVE,
            ntres: 0x18,
            crt_tenth: 0,
            crt_time: 1234,
            crt_date: 5678,
            acc_date: 5678,
            cluster: 0x0012_3456,
            wrt_time: 4321,
            wrt_date: 8765,
            size: 99_999,
        };
        let mut b = [0u8; ENTRY_SIZE];
        raw.write(&mut b);
        assert_eq!(RawEntry::parse(&b), raw);
        assert_eq!(short_display(&raw), "notas.txt");
    }
}
