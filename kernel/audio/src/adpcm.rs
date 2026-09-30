//! IMA ADPCM (la variante de Microsoft para WAV, formato 0x11).
//!
//! La idea: una muestra de audio se parece mucho a la anterior. En vez de guardar 16 bits, se
//! guarda cuánto cambió, en 4 bits, medido en "pasos". Si la diferencia fue grande, el paso
//! siguiente se agranda; si fue chica, se achica (la tabla [`INDEX`]). El decodificador repite
//! exactamente las mismas cuentas que el codificador, así que los dos ven la misma predicción y
//! el error no se acumula.
//!
//! El archivo va en **bloques** (`block_align` bytes). Cada bloque empieza, por canal, con la
//! primera muestra entera (16 bits) y el índice del paso; así un bloque se decodifica solo, sin
//! los anteriores (se puede saltar a cualquier parte). Después vienen los nibbles: con un canal,
//! de a bytes (primero el nibble bajo); con dos, de a 4 bytes (8 muestras) de cada canal.

use alloc::vec::Vec;

use crate::Error;

/// Los 89 tamaños de paso (crecen ~10 % cada uno).
pub const STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// Cuánto se mueve el índice del paso según el nibble (sin el signo).
pub const INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// El estado de un canal: la predicción y el índice del paso.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    pred: i32,
    index: i32,
}

impl Channel {
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEP[self.index as usize];
        // diff = (nibble + 0.5) * step / 4, con las sumas de la norma (sin multiplicar).
        let mut diff = step >> 3;
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        if nibble & 8 != 0 {
            self.pred -= diff;
        } else {
            self.pred += diff;
        }
        self.pred = self.pred.clamp(-32768, 32767);
        self.index = (self.index + INDEX[(nibble & 7) as usize]).clamp(0, 88);
        self.pred as i16
    }

    fn encode(&mut self, sample: i16) -> u8 {
        let step = STEP[self.index as usize];
        let mut diff = sample as i32 - self.pred;
        let mut nibble = 0u8;
        if diff < 0 {
            nibble = 8;
            diff = -diff;
        }
        let mut s = step;
        if diff >= s {
            nibble |= 4;
            diff -= s;
        }
        s >>= 1;
        if diff >= s {
            nibble |= 2;
            diff -= s;
        }
        s >>= 1;
        if diff >= s {
            nibble |= 1;
        }
        // El codificador actualiza su estado igual que el decodificador.
        self.decode(nibble);
        nibble
    }
}

/// Muestras por canal que trae un bloque de `block_align` bytes.
pub fn samples_per_block(block_align: usize, channels: usize) -> usize {
    if channels == 0 || block_align < 4 * channels {
        return 0;
    }
    (block_align - 4 * channels) * 2 / channels + 1
}

/// Decodifica un bloque y agrega sus muestras (intercaladas por canal) a `out`.
pub fn decode_block(block: &[u8], channels: usize, out: &mut Vec<i16>) -> Result<(), Error> {
    if !(1..=2).contains(&channels) {
        return Err(Error::Unsupported("IMA ADPCM de más de dos canales"));
    }
    if block.len() < 4 * channels {
        return Err(Error::Truncated);
    }
    let mut ch = [Channel::default(); 2];
    for (c, state) in ch.iter_mut().enumerate().take(channels) {
        let h = &block[c * 4..c * 4 + 4];
        state.pred = i16::from_le_bytes([h[0], h[1]]) as i32;
        state.index = (h[2] as i32).min(88);
    }
    let per = samples_per_block(block.len(), channels);
    let start = out.len();
    out.resize(start + per * channels, 0);
    // La primera muestra de cada canal es la del encabezado.
    for (c, state) in ch.iter().enumerate().take(channels) {
        out[start + c] = state.pred as i16;
    }
    let data = &block[4 * channels..];
    if channels == 1 {
        for (i, &b) in data.iter().enumerate() {
            out[start + 1 + 2 * i] = ch[0].decode(b & 0xF);
            out[start + 2 + 2 * i] = ch[0].decode(b >> 4);
        }
    } else {
        // De a 8 bytes: 4 del izquierdo (8 muestras) y 4 del derecho.
        for (g, group) in data.as_chunks::<8>().0.iter().enumerate() {
            for (c, state) in ch.iter_mut().enumerate() {
                for (k, &b) in group[c * 4..c * 4 + 4].iter().enumerate() {
                    let frame = 1 + g * 8 + k * 2;
                    out[start + frame * 2 + c] = state.decode(b & 0xF);
                    out[start + (frame + 1) * 2 + c] = state.decode(b >> 4);
                }
            }
        }
    }
    Ok(())
}

/// Codifica muestras intercaladas en bloques de `block_align` bytes (el último se completa con
/// silencio). Lo usan los tests y las herramientas que arman los paquetes.
pub fn encode(samples: &[i16], channels: usize, block_align: usize) -> Vec<u8> {
    let per = samples_per_block(block_align, channels);
    let mut out = Vec::new();
    let mut ch = [Channel::default(); 2];
    let frames = samples.len() / channels.max(1);
    let mut f = 0;
    while f < frames {
        let get = |frame: usize, c: usize| -> i16 {
            if frame < frames {
                samples[frame * channels + c]
            } else {
                0
            }
        };
        let start = out.len();
        for (c, state) in ch.iter_mut().enumerate().take(channels) {
            let first = get(f, c);
            state.pred = first as i32;
            out.extend_from_slice(&first.to_le_bytes());
            out.push(state.index as u8);
            out.push(0);
        }
        if channels == 1 {
            let mut k = 1;
            while k < per {
                let lo = ch[0].encode(get(f + k, 0));
                let hi = ch[0].encode(get(f + k + 1, 0));
                out.push(lo | hi << 4);
                k += 2;
            }
        } else {
            let mut k = 1;
            while k < per {
                for (c, state) in ch.iter_mut().enumerate() {
                    for j in 0..4 {
                        let lo = state.encode(get(f + k + j * 2, c));
                        let hi = state.encode(get(f + k + j * 2 + 1, c));
                        out.push(lo | hi << 4);
                    }
                }
                k += 8;
            }
        }
        out.resize(start + block_align, 0);
        f += per;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una onda que sube y baja (entera: sin punto flotante).
    fn wave(n: usize, channels: usize) -> Vec<i16> {
        (0..n * channels)
            .map(|i| {
                let t = (i / channels) as i32;
                // Triangular: ADPCM sigue bien lo que cambia de a poco (un salto brusco tarda
                // unas muestras en alcanzarlo).
                let tri = (t * 300 % 40000 - 20000).abs() * 2 - 20000;
                if i % channels == 1 {
                    (tri / 2) as i16
                } else {
                    tri as i16
                }
            })
            .collect()
    }

    #[test]
    fn un_nibble_de_libro() {
        // Paso 7, nibble 4: diff = 7/8 + 7 = 7 (entero: 0 + 7), y el índice sube 2.
        let mut c = Channel::default();
        assert_eq!(c.decode(4), 7);
        assert_eq!(c.index, 2);
        // Paso 9, nibble 0xC (negativo): diff = 9/8 + 9 = 10 → 7 - 10.
        assert_eq!(c.decode(0xC), -3);
    }

    #[test]
    fn ida_y_vuelta_mono_y_estereo() {
        for channels in [1usize, 2] {
            let block = 256 * channels;
            let per = samples_per_block(block, channels);
            assert_eq!(per, 505);
            let src = wave(per * 3 + 17, channels);
            let enc = encode(&src, channels, block);
            assert_eq!(enc.len(), block * 4, "4 bloques (el último con relleno)");
            let mut dec = Vec::new();
            for b in enc.chunks(block) {
                decode_block(b, channels, &mut dec).unwrap();
            }
            assert!(dec.len() >= src.len());
            // ADPCM pierde, pero poco: error medio chico y la primera muestra de cada bloque,
            // exacta.
            let err: i64 = src
                .iter()
                .zip(&dec)
                .map(|(a, b)| (*a as i64 - *b as i64).abs())
                .sum::<i64>()
                / src.len() as i64;
            assert!(err < 400, "{channels} canales: error medio {err}");
            assert_eq!(dec[per * channels], src[per * channels]);
        }
    }

    #[test]
    fn bloques_invalidos() {
        let mut out = Vec::new();
        assert_eq!(decode_block(&[1, 2], 1, &mut out), Err(Error::Truncated));
        assert!(decode_block(&[0; 64], 3, &mut out).is_err());
        assert_eq!(samples_per_block(3, 1), 0);
    }
}
