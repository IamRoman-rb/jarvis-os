//! Audio sin hardware: el nivel de un pedazo de PCM (para el medidor del micrófono) y la
//! información del micrófono que detectó el kernel.

use alloc::string::String;

/// Lo que el kernel sabe del micrófono (Configuración → Micrófono).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicInfo {
    /// La placa ("virtio-sound").
    pub device: String,
    pub rate: u32,
    pub channels: u8,
    /// Nivel actual (0..=100) y el pico reciente.
    pub level: u8,
    pub peak: u8,
    /// Llega audio (la placa devuelve buffers).
    pub receiving: bool,
}

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
    let rms = isqrt(sum / n as u64);
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
    }
}
