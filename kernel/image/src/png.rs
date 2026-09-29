//! PNG (especificación del W3C, 3ra edición), propio.
//!
//! Un PNG es la firma y una lista de *chunks* (largo, tipo de 4 letras, datos, CRC-32):
//! - `IHDR`: ancho, alto, bits por muestra, tipo de color y si está entrelazado.
//! - `PLTE`: la paleta (imágenes indexadas). `tRNS`: la transparencia de la paleta o el color
//!   que cuenta como transparente.
//! - `IDAT`: los píxeles, comprimidos con zlib (pueden venir en varios chunks: se concatenan).
//!
//! Descomprimidos, los píxeles van por filas y cada fila empieza con un byte de **filtro**: en
//! vez del byte, se guarda la diferencia con el de la izquierda, el de arriba, su promedio o el
//! "predictor de Paeth". Las diferencias son casi siempre chicas, y eso comprime mucho mejor.
//!
//! Con **entrelazado Adam7** la imagen viene en 7 pasadas (primero 1 de cada 64 píxeles, al
//! final la mitad): cada pasada es una imagen chica con sus filas y filtros.
//!
//! Los chunks críticos (primera letra mayúscula) verifican su CRC; los demás se ignoran (como
//! los navegadores, que toleran un `tEXt` dañado). Las animaciones (APNG) muestran el primer cuadro.

use alloc::vec;
use alloc::vec::Vec;

use crate::inflate::zlib_decompress;
use crate::{Error, Image, MAX_PIXELS};

pub const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Pasadas de Adam7: (x inicial, y inicial, paso en x, paso en y).
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

#[derive(Clone, Copy)]
struct Header {
    width: usize,
    height: usize,
    depth: u8,
    color: u8,
    interlaced: bool,
}

impl Header {
    /// Muestras por píxel según el tipo de color.
    fn channels(&self) -> usize {
        match self.color {
            0 | 3 => 1, // gris, indexado
            2 => 3,     // RGB
            4 => 2,     // gris + alfa
            _ => 4,     // RGBA
        }
    }

    fn bits_per_pixel(&self) -> usize {
        self.channels() * usize::from(self.depth)
    }

    fn row_bytes(&self, width: usize) -> usize {
        (width * self.bits_per_pixel()).div_ceil(8)
    }
}

pub fn decode(data: &[u8]) -> Result<Image, Error> {
    if !data.starts_with(SIGNATURE) {
        return Err(Error::Invalid("no es un PNG"));
    }
    let mut pos = SIGNATURE.len();
    let mut header = None;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();
    loop {
        let len = be32(data, pos)? as usize;
        let kind: [u8; 4] = data
            .get(pos + 4..pos + 8)
            .ok_or(Error::Truncated)?
            .try_into()
            .map_err(|_| Error::Truncated)?;
        let body = data.get(pos + 8..pos + 8 + len).ok_or(Error::Truncated)?;
        let crc = be32(data, pos + 8 + len)?;
        let critical = kind[0].is_ascii_uppercase();
        if critical && crc32(&data[pos + 4..pos + 8 + len]) != crc {
            return Err(Error::Checksum);
        }
        pos += 12 + len;
        match &kind {
            b"IHDR" => header = Some(parse_header(body)?),
            b"PLTE" => {
                if body.len() % 3 != 0 || body.len() > 256 * 3 {
                    return Err(Error::Invalid("paleta inválida"));
                }
                palette = body.as_chunks::<3>().0.to_vec();
            }
            b"tRNS" => trns = body.to_vec(),
            b"IDAT" => {
                if header.is_none() {
                    return Err(Error::Invalid("IDAT antes de IHDR"));
                }
                idat.extend_from_slice(body);
            }
            b"IEND" => break,
            _ if critical => return Err(Error::Unsupported("chunk crítico desconocido")),
            _ => {}
        }
    }
    let h = header.ok_or(Error::Invalid("falta IHDR"))?;
    if h.color == 3 && palette.is_empty() {
        return Err(Error::Invalid("imagen indexada sin paleta"));
    }
    // Lo que tienen que ocupar los píxeles descomprimidos: ni un byte más (bomba), ni uno menos.
    let passes: Vec<(usize, usize, usize, usize, usize, usize)> = if h.interlaced {
        ADAM7
            .iter()
            .map(|&(x0, y0, dx, dy)| {
                let pw = (h.width + dx - 1 - x0) / dx;
                let ph = (h.height + dy - 1 - y0) / dy;
                (x0, y0, dx, dy, pw, ph)
            })
            .filter(|p| p.4 > 0 && p.5 > 0)
            .collect()
    } else {
        vec![(0, 0, 1, 1, h.width, h.height)]
    };
    let expected: usize = passes.iter().map(|p| p.5 * (1 + h.row_bytes(p.4))).sum();
    let raw = zlib_decompress(&idat, expected)?;
    if raw.len() < expected {
        return Err(Error::Truncated);
    }
    let mut rgba = vec![0u8; h.width * h.height * 4];
    let bpp = h.bits_per_pixel().div_ceil(8);
    let mut at = 0;
    for &(x0, y0, dx, dy, pw, ph) in &passes {
        let rb = h.row_bytes(pw);
        let mut prev = vec![0u8; rb];
        let mut cur = vec![0u8; rb];
        for py in 0..ph {
            let filter = raw[at];
            cur.copy_from_slice(&raw[at + 1..at + 1 + rb]);
            at += 1 + rb;
            unfilter(filter, &mut cur, &prev, bpp)?;
            let y = y0 + py * dy;
            for px in 0..pw {
                let x = x0 + px * dx;
                let p = pixel(&h, &cur, px, &palette, &trns);
                rgba[(y * h.width + x) * 4..][..4].copy_from_slice(&p);
            }
            core::mem::swap(&mut prev, &mut cur);
        }
    }
    Ok(Image {
        width: h.width,
        height: h.height,
        rgba,
    })
}

fn parse_header(b: &[u8]) -> Result<Header, Error> {
    if b.len() != 13 {
        return Err(Error::Invalid("IHDR con largo equivocado"));
    }
    let (width, height) = (be32(b, 0)? as usize, be32(b, 4)? as usize);
    let h = Header {
        width,
        height,
        depth: b[8],
        color: b[9],
        interlaced: b[12] == 1,
    };
    if width == 0 || height == 0 {
        return Err(Error::Invalid("imagen vacía"));
    }
    if width.saturating_mul(height) > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    let ok = match h.color {
        0 => matches!(h.depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(h.depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(h.depth, 8 | 16),
        _ => false,
    };
    if !ok {
        return Err(Error::Invalid("combinación de color y bits inválida"));
    }
    if b[10] != 0 || b[11] != 0 || b[12] > 1 {
        return Err(Error::Unsupported(
            "método de compresión, filtro o entrelazado",
        ));
    }
    Ok(h)
}

/// Deshace el filtro de una fila. `bpp` = bytes por píxel (al menos 1): "el de la izquierda" es
/// el mismo byte del píxel anterior.
fn unfilter(filter: u8, cur: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), Error> {
    match filter {
        0 => {}
        1 => {
            for i in bpp..cur.len() {
                cur[i] = cur[i].wrapping_add(cur[i - bpp]);
            }
        }
        2 => {
            for (c, &p) in cur.iter_mut().zip(prev) {
                *c = c.wrapping_add(p);
            }
        }
        3 => {
            for i in 0..cur.len() {
                let left = if i >= bpp { cur[i - bpp] } else { 0 };
                cur[i] = cur[i].wrapping_add(((u16::from(left) + u16::from(prev[i])) / 2) as u8);
            }
        }
        4 => {
            for i in 0..cur.len() {
                let (a, c) = if i >= bpp {
                    (cur[i - bpp], prev[i - bpp])
                } else {
                    (0, 0)
                };
                cur[i] = cur[i].wrapping_add(paeth(a, prev[i], c));
            }
        }
        _ => return Err(Error::Invalid("tipo de filtro desconocido")),
    }
    Ok(())
}

/// El predictor de Paeth: de izquierda, arriba y arriba-izquierda, el más parecido a
/// `izquierda + arriba - arriba-izquierda` (en caso de empate, en ese orden).
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// La muestra `i` de una fila (de 1, 2, 4, 8 o 16 bits; las de menos de 8, desde el bit más alto).
fn sample(row: &[u8], i: usize, depth: u8) -> u16 {
    match depth {
        8 => u16::from(row[i]),
        16 => u16::from_be_bytes([row[2 * i], row[2 * i + 1]]),
        d => {
            let bit = i * usize::from(d);
            let byte = row[bit / 8];
            let shift = 8 - usize::from(d) - bit % 8;
            u16::from(byte >> shift) & ((1 << d) - 1)
        }
    }
}

/// Lleva una muestra a 8 bits: la de 16 se queda con el byte alto; las chicas se escalan
/// (un gris de 1 bit vale 0 o 255, no 0 o 1).
fn to8(v: u16, depth: u8) -> u8 {
    match depth {
        16 => (v >> 8) as u8,
        8 => v as u8,
        d => (u32::from(v) * 255 / ((1 << d) - 1)) as u8,
    }
}

fn pixel(h: &Header, row: &[u8], x: usize, palette: &[[u8; 3]], trns: &[u8]) -> [u8; 4] {
    let d = h.depth;
    match h.color {
        0 => {
            let v = sample(row, x, d);
            let g = to8(v, d);
            // tRNS de gris: un valor de 2 bytes (en la profundidad original) es transparente.
            let clear = trns.len() >= 2 && u16::from_be_bytes([trns[0], trns[1]]) == v;
            [g, g, g, if clear { 0 } else { 255 }]
        }
        2 => {
            let (r, g, b) = (
                sample(row, 3 * x, d),
                sample(row, 3 * x + 1, d),
                sample(row, 3 * x + 2, d),
            );
            let clear = trns.len() >= 6
                && u16::from_be_bytes([trns[0], trns[1]]) == r
                && u16::from_be_bytes([trns[2], trns[3]]) == g
                && u16::from_be_bytes([trns[4], trns[5]]) == b;
            [to8(r, d), to8(g, d), to8(b, d), if clear { 0 } else { 255 }]
        }
        3 => {
            let i = usize::from(sample(row, x, d));
            // Un índice fuera de la paleta es un error del archivo: se pinta negro (como libpng).
            let [r, g, b] = palette.get(i).copied().unwrap_or([0, 0, 0]);
            [r, g, b, trns.get(i).copied().unwrap_or(255)]
        }
        4 => {
            let g = to8(sample(row, 2 * x, d), d);
            [g, g, g, to8(sample(row, 2 * x + 1, d), d)]
        }
        _ => [
            to8(sample(row, 4 * x, d), d),
            to8(sample(row, 4 * x + 1, d), d),
            to8(sample(row, 4 * x + 2, d), d),
            to8(sample(row, 4 * x + 3, d), d),
        ],
    }
}

fn be32(b: &[u8], at: usize) -> Result<u32, Error> {
    let s = b.get(at..at + 4).ok_or(Error::Truncated)?;
    Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// CRC-32 (el de zlib y Ethernet, polinomio 0xEDB88320), byte a byte con una tabla.
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
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
    };
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
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn paeth_de_libro() {
        assert_eq!(paeth(10, 20, 10), 20);
        assert_eq!(paeth(20, 10, 10), 20);
        assert_eq!(paeth(5, 5, 5), 5);
    }
}
