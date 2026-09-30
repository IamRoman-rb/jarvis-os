//! Decodificadores de imágenes de JARVIS-OS (hito K10, ADR 0009).
//!
//! Hasta K10 el puente del anfitrión convertía toda imagen a BMP. Ahora el kernel entiende solo
//! los dos formatos que son casi toda la web:
//!
//! - [`png`]: propio, con su descompresor [`inflate`] (DEFLATE + zlib). PNG es un buen ejercicio:
//!   compresión con códigos de Huffman, filtros por fila, paletas, transparencia y entrelazado.
//! - [`jpeg`]: con `zune-jpeg` (`no_std`). JPEG es otra liga (DCT, submuestreo, progresivos) y en
//!   la web abundan los progresivos: escribirlo a mano no enseña más que lo que cuesta.
//!
//! Los SVG, GIF, WebP e ICO siguen yendo al puente. Todo sale como [`Image`] en RGBA de 8 bits.
//!
//! Referencias: RFC 1950 (zlib), RFC 1951 (DEFLATE), la especificación de PNG (W3C, 3ra ed.) y
//! `puff.c` de Mark Adler (el inflate "de libro").

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

pub mod inflate;
pub mod jpeg;
pub mod png;

/// Tope de píxeles de una imagen decodificada (4096 × 4096, 64 MiB en RGBA): más que eso no
/// entra con comodidad en el heap del kernel, y una imagen chica puede declararse enorme.
pub const MAX_PIXELS: usize = 4096 * 4096;

/// Una imagen decodificada: RGBA de 8 bits por canal, fila por fila de arriba hacia abajo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Image {
    /// ¿Algún píxel es transparente (aunque sea en parte)?
    pub fn has_alpha(&self) -> bool {
        self.rgba.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
    }

    /// La achica (promediando cada caja de píxeles) para que el lado mayor no pase de `max_side`.
    /// El color se promedia pesado por la opacidad, así los bordes transparentes no se oscurecen.
    pub fn shrink_to(self, max_side: usize) -> Image {
        let side = self.width.max(self.height);
        if side <= max_side || max_side == 0 {
            return self;
        }
        let w = (self.width * max_side / side).max(1);
        let h = (self.height * max_side / side).max(1);
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let (y0, y1) = (
                y * self.height / h,
                ((y + 1) * self.height / h).max(y * self.height / h + 1),
            );
            for x in 0..w {
                let (x0, x1) = (
                    x * self.width / w,
                    ((x + 1) * self.width / w).max(x * self.width / w + 1),
                );
                let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let p = &self.rgba[(sy * self.width + sx) * 4..][..4];
                        let pa = p[3] as u64;
                        r += p[0] as u64 * pa;
                        g += p[1] as u64 * pa;
                        b += p[2] as u64 * pa;
                        a += pa;
                        n += 1;
                    }
                }
                // Todo transparente (a = 0): negro transparente.
                let avg = |v: u64| v.checked_div(a).unwrap_or(0) as u8;
                out.extend_from_slice(&[avg(r), avg(g), avg(b), (a / n) as u8]);
            }
        }
        Image {
            width: w,
            height: h,
            rgba: out,
        }
    }
}

/// Los formatos que se reconocen por sus primeros bytes ("números mágicos").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
}

/// Qué formato es, mirando la firma (no la extensión ni el `Content-Type`, que mienten).
pub fn sniff(data: &[u8]) -> Option<Format> {
    if data.starts_with(png::SIGNATURE) {
        Some(Format::Png)
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else {
        None
    }
}

/// Decodifica un PNG o un JPEG. `None` si es otro formato; `Some(Err)` si está roto.
pub fn decode(data: &[u8]) -> Option<Result<Image, Error>> {
    Some(match sniff(data)? {
        Format::Png => png::decode(data),
        Format::Jpeg => jpeg::decode(data),
    })
}

/// Por qué no se pudo decodificar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Los datos se terminan antes de tiempo.
    Truncated,
    /// Los datos no respetan el formato (el texto dice qué).
    Invalid(&'static str),
    /// Algo válido que no se soporta.
    Unsupported(&'static str),
    /// La imagen (o lo que ocupa descomprimida) pasa los topes.
    TooLarge,
    /// La suma de verificación (CRC-32 o Adler-32) no coincide.
    Checksum,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("la imagen está cortada"),
            Error::Invalid(s) => write!(f, "imagen inválida: {s}"),
            Error::Unsupported(s) => write!(f, "imagen no soportada: {s}"),
            Error::TooLarge => f.write_str("la imagen es demasiado grande"),
            Error::Checksum => f.write_str("la imagen está dañada (suma de verificación)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn reconoce_la_firma() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some(Format::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(sniff(b"GIF89a"), None);
        assert_eq!(sniff(b"<svg"), None);
        assert!(decode(b"BM....").is_none());
    }

    #[test]
    fn achicar_promedia_y_respeta_la_transparencia() {
        // 4×2: la mitad izquierda roja opaca, la derecha transparente.
        let mut rgba = vec![];
        for _ in 0..2 {
            rgba.extend_from_slice(&[255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        let img = Image {
            width: 4,
            height: 2,
            rgba,
        }
        .shrink_to(2);
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(&img.rgba, &[255, 0, 0, 255, 0, 0, 0, 0]);
        // Una caja mitad opaca: el color no se oscurece, la opacidad se promedia.
        let img = Image {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 255, 255, 0, 0, 0, 0],
        }
        .shrink_to(1);
        assert_eq!(&img.rgba, &[0, 0, 255, 127]);
    }
}
