//! Un sintetizador chico: convierte las partituras de la app Música (notas y duraciones) en
//! audio de verdad para la placa, en vez de la onda cuadrada del parlante de la PC.
//!
//! Cada nota es una onda seno más dos armónicos (el doble y el triple de la frecuencia, más
//! suaves): eso le da "timbre" y suena más a un instrumento que a un diapasón. La amplitud sigue
//! una **envolvente** (ataque rápido, decae y se apaga al final de la nota) para que dos notas
//! iguales seguidas se escuchen como dos. Todo en punto fijo: la fase es un número de 32 bits que
//! da la vuelta (2^32 = una vuelta entera) y el seno sale de una tabla de 256 valores.

use alloc::vec::Vec;

use crate::resample::Source;

/// sen(2π·i/256) × 32767.
const SINE: [i16; 256] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602, 6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530, 18204, 18868, 19519, 20159, 20787,
    21403, 22005, 22594, 23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790, 27245, 27683,
    28105, 28510, 28898, 29268, 29621, 29956, 30273, 30571, 30852, 31113, 31356, 31580, 31785,
    31971, 32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757, 32767, 32757, 32728, 32678,
    32609, 32521, 32412, 32285, 32137, 31971, 31785, 31580, 31356, 31113, 30852, 30571, 30273,
    29956, 29621, 29268, 28898, 28510, 28105, 27683, 27245, 26790, 26319, 25832, 25329, 24811,
    24279, 23731, 23170, 22594, 22005, 21403, 20787, 20159, 19519, 18868, 18204, 17530, 16846,
    16151, 15446, 14732, 14010, 13279, 12539, 11793, 11039, 10278, 9512, 8739, 7962, 7179, 6393,
    5602, 4808, 4011, 3212, 2410, 1608, 804, 0, -804, -1608, -2410, -3212, -4011, -4808, -5602,
    -6393, -7179, -7962, -8739, -9512, -10278, -11039, -11793, -12539, -13279, -14010, -14732,
    -15446, -16151, -16846, -17530, -18204, -18868, -19519, -20159, -20787, -21403, -22005, -22594,
    -23170, -23731, -24279, -24811, -25329, -25832, -26319, -26790, -27245, -27683, -28105, -28510,
    -28898, -29268, -29621, -29956, -30273, -30571, -30852, -31113, -31356, -31580, -31785, -31971,
    -32137, -32285, -32412, -32521, -32609, -32678, -32728, -32757, -32767, -32757, -32728, -32678,
    -32609, -32521, -32412, -32285, -32137, -31971, -31785, -31580, -31356, -31113, -30852, -30571,
    -30273, -29956, -29621, -29268, -28898, -28510, -28105, -27683, -27245, -26790, -26319, -25832,
    -25329, -24811, -24279, -23731, -23170, -22594, -22005, -21403, -20787, -20159, -19519, -18868,
    -18204, -17530, -16846, -16151, -15446, -14732, -14010, -13279, -12539, -11793, -11039, -10278,
    -9512, -8739, -7962, -7179, -6393, -5602, -4808, -4011, -3212, -2410, -1608, -804,
];

fn sine(phase: u32) -> i32 {
    SINE[(phase >> 24) as usize] as i32
}

/// Una nota: frecuencia en Hz (0 = silencio) y duración en ms.
pub type Note = (u32, u32);

pub struct Synth {
    notes: Vec<Note>,
    rate: u32,
    index: usize,
    /// Cuadros que van de la nota actual.
    into: u32,
    phase: u32,
}

impl Synth {
    pub fn new(notes: Vec<Note>, rate: u32) -> Synth {
        Synth {
            notes,
            rate: rate.max(1),
            index: 0,
            into: 0,
            phase: 0,
        }
    }

    /// Cuánto dura todo, en ms.
    pub fn duration_ms(&self) -> u64 {
        self.notes.iter().map(|n| n.1 as u64).sum()
    }
}

impl Source for Synth {
    fn render(&mut self, out: &mut [i16]) -> usize {
        let frames = out.len() / 2;
        for n in 0..frames {
            let Some(&(hz, ms)) = self.notes.get(self.index) else {
                return n;
            };
            let len = (self.rate as u64 * ms as u64 / 1000) as u32;
            let t = self.into;
            // Envolvente en milésimos: sube en 10 ms, baja a 600 en 150 ms y se apaga en los
            // últimos 30 ms (o en el último octavo de las notas cortas).
            let attack = self.rate / 100;
            let decay = self.rate * 15 / 100;
            let release = (self.rate * 3 / 100).min(len / 8).max(1);
            let env = if hz == 0 {
                0
            } else if t < attack {
                t * 1000 / attack.max(1)
            } else if t < attack + decay {
                1000 - (t - attack) * 400 / decay.max(1)
            } else {
                600
            };
            let env = if t + release > len {
                env * len.saturating_sub(t) / release
            } else {
                env
            };
            let step = ((hz as u64) << 32) / self.rate as u64;
            let p = self.phase;
            let wave = sine(p) * 10 + sine(p.wrapping_mul(2)) * 3 + sine(p.wrapping_mul(3)) * 2;
            // wave llega a ±15 × 32767; con la envolvente (÷1000) y un margen, a ±12 000.
            let s = (wave / 15) * env as i32 / 1000 * 12 / 32;
            out[2 * n] = s as i16;
            out[2 * n + 1] = s as i16;
            self.phase = self.phase.wrapping_add(step as u32);
            self.into += 1;
            if self.into >= len {
                self.index += 1;
                self.into = 0;
            }
        }
        frames
    }

    fn done(&self) -> bool {
        self.index >= self.notes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn la_nota_la_suena_a_440() {
        // Un La de 100 ms a 48 kHz: 4800 cuadros y 44 vueltas (cruces por cero hacia arriba).
        let mut s = Synth::new(vec![(440, 100), (0, 50)], 48_000);
        assert_eq!(s.duration_ms(), 150);
        let mut out = vec![0i16; 2 * 8000];
        let n = s.render(&mut out);
        assert_eq!(n, 4800 + 2400);
        assert!(s.done());
        let left: Vec<i16> = out[..2 * 4800].iter().step_by(2).copied().collect();
        let ups = left.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count();
        assert!((43..=45).contains(&ups), "{ups}");
        let max = left.iter().map(|v| v.unsigned_abs()).max().unwrap();
        assert!((4000..=13_000).contains(&max), "{max}");
        // El silencio es silencio y los dos canales son iguales.
        assert!(out[2 * 4800..2 * 7200].iter().all(|&v| v == 0));
        assert!(out[..2 * 4800].chunks(2).all(|f| f[0] == f[1]));
        // Arranca y termina en cero (sin "clic").
        assert_eq!(left[0], 0);
        assert!(left[4799].unsigned_abs() < 300);
    }
}
