//! Audio y video de JARVIS-OS (hito K12), sin hardware y sin punto flotante.
//!
//! El kernel compila sin SSE: el punto flotante es por software y cada suma de `f32` es una
//! llamada a una función. Por eso acá todo es aritmética entera (punto fijo): los códecs que se
//! eligieron también lo son. MP3 y Vorbis decodifican con `float` (con SSE es gratis; sin SSE
//! no llegarían a tiempo real), así que no están; sí:
//!
//! - [`wav`]: el contenedor RIFF/WAVE con PCM de 8, 16 o 24 bits…
//! - [`adpcm`]: …o comprimido con **IMA ADPCM** (4 bits por muestra: un cuarto de PCM). Es un
//!   códec "de libro": cada muestra se predice con la anterior y se guarda solo la diferencia,
//!   cuantizada con un paso que se adapta.
//! - [`resample`]: cualquier frecuencia y cantidad de canales → la de la placa (estéreo).
//! - [`mixer`]: suma lo que suena (música, la voz de JARVIS, el audio de un video), con volumen,
//!   y recuerda el nivel de la voz de cada momento: con eso se mueve la esfera.
//! - [`synth`]: un sintetizador para las partituras de la app Música.
//! - [`avi`]: el contenedor de video AVI (cuadros MJPEG, que decodifica `jarvis-image`, y audio
//!   PCM o ADPCM).
//!
//! Referencias: "Multimedia Programming Interface and Data Specifications 1.0" (Microsoft/IBM,
//! RIFF y WAVE), "Recommended Practices for Enhancing Digital Audio Compatibility in Multimedia
//! Systems" (IMA, 1992) y "AVI RIFF File Reference" (Microsoft).

#![no_std]

extern crate alloc;

pub mod adpcm;
pub mod avi;
pub mod mixer;
pub mod resample;
pub mod synth;
pub mod wav;

/// Nivel 0..=100 de PCM de 16 bits little endian (todos los canales juntos), en escala
/// logarítmica como el oído: ~50 de energía → 0, ~10 000 → 100.
pub fn level(pcm: &[u8]) -> u8 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0;
    }
    let mut sum: u64 = 0;
    for s in pcm.as_chunks::<2>().0 {
        let v = i16::from_le_bytes(*s) as i64;
        sum += (v * v) as u64;
    }
    level_of_energy(sum / n as u64)
}

/// Lo mismo, de muestras ya decodificadas.
pub fn level_samples(pcm: &[i16]) -> u8 {
    if pcm.is_empty() {
        return 0;
    }
    let sum: u64 = pcm.iter().map(|&v| (v as i64 * v as i64) as u64).sum();
    level_of_energy(sum / pcm.len() as u64)
}

fn level_of_energy(mean_square: u64) -> u8 {
    let rms = isqrt(mean_square);
    if rms < 50 {
        return 0;
    }
    // log2 en punto fijo: 50 → 0, 10 000 (≈ 50·2^7.64) → 100.
    let ratio_q8 = (rms << 8) / 50; // 256 = ×1
    let log2_q8 = ilog2_q8(ratio_q8);
    (log2_q8 * 100 / (764 * 256 / 100)).min(100) as u8
}

fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// log2 de un número en Q8 (256 = 1), el resultado también en Q8.
fn ilog2_q8(v: u64) -> u64 {
    if v <= 256 {
        return 0;
    }
    let int = 63 - v.leading_zeros() as u64 - 8;
    // Parte fraccionaria por interpolación lineal entre potencias de 2.
    let base = 256u64 << int;
    let frac = (v - base) * 256 / base;
    int * 256 + frac
}

/// Por qué no se pudo leer un archivo de audio o video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Invalid(&'static str),
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("el archivo está cortado"),
            Error::Invalid(s) => write!(f, "archivo inválido: {s}"),
            Error::Unsupported(s) => write!(f, "formato no soportado: {s}"),
        }
    }
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> Result<u16, Error> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(Error::Truncated)
}

pub(crate) fn u32_at(b: &[u8], at: usize) -> Result<u32, Error> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(Error::Truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn tone(amp: i16) -> Vec<u8> {
        (0..640)
            .flat_map(|i| {
                let v = if i % 2 == 0 { amp } else { -amp };
                v.to_le_bytes()
            })
            .collect()
    }

    #[test]
    fn niveles() {
        assert_eq!(level(&[]), 0);
        assert_eq!(level(&tone(0)), 0);
        assert_eq!(level(&tone(40)), 0);
        let low = level(&tone(300));
        let high = level(&tone(8000));
        assert!(0 < low && low < high, "{low} {high}");
        assert_eq!(level(&tone(i16::MAX)), 100);
        let s: Vec<i16> = (0..640)
            .map(|i| if i % 2 == 0 { 8000 } else { -8000 })
            .collect();
        assert_eq!(level_samples(&s), high);
    }
}
