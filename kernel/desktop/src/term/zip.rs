//! Lector de archivos ZIP (para `winget install` de programas "portables", ADR 0021).
//!
//! Un ZIP se lee desde el **final**: el "fin del directorio central" (EOCD, firma `PK\5\6`) dice
//! dónde empieza el directorio central, que tiene una entrada por archivo (`PK\1\2`) con su
//! nombre, tamaños, CRC-32, método de compresión y dónde está su cabecera local (`PK\3\4`). Los
//! datos van justo después de la cabecera local. Métodos: 0 (guardado tal cual) y 8 (DEFLATE,
//! con el `inflate` propio de `jarvis-image`). Especificación: PKWARE APPNOTE.TXT.
//!
//! Seguridad: los nombres con `..`, rutas absolutas o `\` se rechazan ("zip slip"), y cada
//! archivo y el total tienen un tope (una "bomba zip" no llena la memoria).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Lo más grande que puede quedar un archivo descomprimido.
pub const MAX_FILE: usize = 128 * 1024 * 1024;
/// Lo más que puede sumar todo lo descomprimido.
pub const MAX_TOTAL: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// La ruta adentro del ZIP, con `/` (`ripgrep-14.1.1/rg.exe`).
    pub name: String,
    pub is_dir: bool,
    method: u16,
    crc: u32,
    compressed: usize,
    pub size: usize,
    local_offset: usize,
    /// Los permisos de Unix, si el ZIP los trae (los arma un ZIP hecho en Linux).
    pub unix_mode: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotZip,
    Truncated,
    /// Un nombre peligroso (`../`, absoluto).
    BadName(String),
    Method(u16),
    TooBig,
    Corrupt,
    Encrypted,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotZip => f.write_str("no es un archivo ZIP"),
            Error::Truncated => f.write_str("el ZIP está cortado"),
            Error::BadName(n) => write!(f, "nombre peligroso adentro del ZIP: {n}"),
            Error::Method(m) => write!(f, "método de compresión {m} no soportado"),
            Error::TooBig => f.write_str("demasiado grande al descomprimir"),
            Error::Corrupt => f.write_str("el ZIP está dañado (CRC incorrecto)"),
            Error::Encrypted => f.write_str("el ZIP tiene contraseña"),
        }
    }
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, Error> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, Error> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated)
}

/// ¿Es un nombre seguro para extraer? Sin `..`, sin raíz, sin `\` ni `:` (rutas de Windows).
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.contains('\0')
        && name.split('/').all(|seg| seg != "..")
}

/// El directorio central: la lista de archivos.
pub fn entries(data: &[u8]) -> Result<Vec<Entry>, Error> {
    // El EOCD mide 22 bytes más un comentario de hasta 64 KiB: se busca desde el final.
    if data.len() < 22 {
        return Err(Error::NotZip);
    }
    let min = data.len().saturating_sub(22 + 0xFFFF);
    let eocd = (min..=data.len() - 22)
        .rev()
        .find(|&i| data[i..i + 4] == *b"PK\x05\x06")
        .ok_or(Error::NotZip)?;
    let count = usize::from(u16_at(data, eocd + 10)?);
    let mut at = u32_at(data, eocd + 16)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if data.get(at..at + 4) != Some(b"PK\x01\x02") {
            return Err(Error::Truncated);
        }
        let made_by_unix = u16_at(data, at + 4)? >> 8 == 3;
        let flags = u16_at(data, at + 8)?;
        let method = u16_at(data, at + 10)?;
        let crc = u32_at(data, at + 16)?;
        let compressed = u32_at(data, at + 20)? as usize;
        let size = u32_at(data, at + 24)? as usize;
        let name_len = usize::from(u16_at(data, at + 28)?);
        let extra_len = usize::from(u16_at(data, at + 30)?);
        let comment_len = usize::from(u16_at(data, at + 32)?);
        let external = u32_at(data, at + 38)?;
        let local_offset = u32_at(data, at + 42)? as usize;
        let raw = data
            .get(at + 46..at + 46 + name_len)
            .ok_or(Error::Truncated)?;
        let name = String::from_utf8_lossy(raw).to_string();
        if flags & 1 != 0 {
            return Err(Error::Encrypted);
        }
        if !safe_name(&name) {
            return Err(Error::BadName(name));
        }
        let is_dir = name.ends_with('/');
        out.push(Entry {
            name: name.trim_end_matches('/').to_string(),
            is_dir,
            method,
            crc,
            compressed,
            size,
            local_offset,
            unix_mode: made_by_unix.then_some(((external >> 16) & 0o7777) as u16),
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(out)
}

/// Los datos de una entrada, descomprimidos y verificados con su CRC-32.
pub fn read(data: &[u8], e: &Entry) -> Result<Vec<u8>, Error> {
    if e.size > MAX_FILE {
        return Err(Error::TooBig);
    }
    let at = e.local_offset;
    if data.get(at..at + 4) != Some(b"PK\x03\x04") {
        return Err(Error::Truncated);
    }
    let name_len = usize::from(u16_at(data, at + 26)?);
    let extra_len = usize::from(u16_at(data, at + 28)?);
    let start = at + 30 + name_len + extra_len;
    let raw = data
        .get(start..start + e.compressed)
        .ok_or(Error::Truncated)?;
    let out = match e.method {
        0 => raw.to_vec(),
        8 => jarvis_image::inflate::inflate_raw(raw, e.size.max(1)).map_err(|_| Error::Corrupt)?,
        m => return Err(Error::Method(m)),
    };
    if out.len() != e.size || crc32(&out) != e.crc {
        return Err(Error::Corrupt);
    }
    Ok(out)
}

/// Todos los archivos (no las carpetas), con su contenido. Respeta [`MAX_TOTAL`].
pub fn extract_all(data: &[u8]) -> Result<Vec<(Entry, Vec<u8>)>, Error> {
    let mut total = 0usize;
    let mut out = Vec::new();
    for e in entries(data)? {
        if e.is_dir {
            continue;
        }
        total = total.saturating_add(e.size);
        if total > MAX_TOTAL {
            return Err(Error::TooBig);
        }
        let bytes = read(data, &e)?;
        out.push((e, bytes));
    }
    Ok(out)
}

/// CRC-32 (el de ZIP, PNG y Ethernet: polinomio 0xEDB88320, reflejado).
pub fn crc32(data: &[u8]) -> u32 {
    const fn table() -> [u32; 256] {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    }
    static TABLE: [u32; 256] = table();
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_conocido() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn nombres() {
        assert!(safe_name("a/b/c.exe"));
        assert!(!safe_name("../x"));
        assert!(!safe_name("a/../../x"));
        assert!(!safe_name("/etc/passwd"));
        assert!(!safe_name("C:\\Windows\\x"));
        assert!(!safe_name("a\\b"));
    }
}
