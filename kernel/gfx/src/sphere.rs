//! La esfera de partículas de JARVIS, animada.
//!
//! Al arrancar se calculan una sola vez las posiciones base de las partículas (con `f32`, que en
//! el kernel es lento pero se hace una vez). Por frame todo es **punto fijo Q14** (ver `trig`):
//! rotar, desplazar según el pulso y proyectar son sumas, multiplicaciones y shifts enteros.
//!
//! Las partículas se reparten al azar pero uniformes sobre la superficie (teorema de Arquímedes:
//! si la altura es uniforme en [-1, 1] y el ángulo también, el área lo es), con un pequeño desvío
//! en el radio para el aspecto de "polvo". El generador es determinista.

use alloc::vec::Vec;
use core::f32::consts::PI;

use libm::{cosf, sinf, sqrtf};

use crate::canvas::Rect;
use crate::trig::{self, FULL_TURN, ONE, mul};
use crate::{Canvas, theme};

/// Desvío máximo del radio de cada partícula: ±2,5 %.
const JITTER: i32 = ONE * 25 / 1000;

/// Cuánto se deforma la esfera en un instante.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pulse {
    /// Escala del radio en Q14 (`ONE` = tamaño normal).
    pub scale: i32,
    /// Amplitud de las ondas que recorren la superficie, en Q14 (0 = sin ondas).
    pub wave: i32,
    /// Fase de las ondas (avanza con el tiempo: así "viajan").
    pub wave_phase: u32,
    /// Brillo extra de las partículas (0..=255).
    pub glow: u8,
}

impl Pulse {
    pub const REST: Pulse = Pulse {
        scale: ONE,
        wave: 0,
        wave_phase: 0,
        glow: 0,
    };
    /// Topes: garantizan que la esfera nunca salga de [`ParticleCloud::bounds`].
    pub const MAX_SCALE: i32 = ONE * 112 / 100;
    pub const MIN_SCALE: i32 = ONE * 90 / 100;
    pub const MAX_WAVE: i32 = ONE * 6 / 100;

    pub fn clamped(self) -> Pulse {
        Pulse {
            scale: self.scale.clamp(Self::MIN_SCALE, Self::MAX_SCALE),
            wave: self.wave.clamp(0, Self::MAX_WAVE),
            ..self
        }
    }
}

/// Dónde y cómo se ve la esfera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    pub cx: i32,
    pub cy: i32,
    /// Radio en píxeles con `Pulse::REST`.
    pub radius: i32,
    /// Giro sobre el eje vertical (unidades de vuelta).
    pub spin: u32,
    /// Inclinación hacia la cámara (unidades de vuelta).
    pub tilt: u32,
}

#[derive(Clone, Copy, Debug)]
struct Particle {
    x: i16,
    y: i16,
    z: i16,
    /// Factor de radio propio en Q14 (1 ± 2,5 %).
    radius: i16,
    /// Desfase de la onda: depende de la altura, así las ondas suben y bajan por la esfera.
    phase: u16,
}

/// Generador pseudoaleatorio xorshift: determinista (la esfera sale igual en cada arranque).
struct XorShift(u32);

impl XorShift {
    fn next_unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Punto uniforme sobre la esfera unitaria a partir de dos números en [0, 1).
/// (Una espiral de Fibonacci también es uniforme, pero deja un patrón de líneas visible.)
pub fn uniform_point(u: f32, v: f32) -> (f32, f32, f32) {
    let y = 1.0 - 2.0 * u;
    let r = sqrtf((1.0 - y * y).max(0.0));
    let theta = 2.0 * PI * v;
    (cosf(theta) * r, y, sinf(theta) * r)
}

pub struct ParticleCloud {
    particles: Vec<Particle>,
}

impl ParticleCloud {
    pub fn new(count: usize) -> Self {
        let mut rng = XorShift(0x4a41_5256); // "JARV"
        let q14 = |v: f32| (v * ONE as f32) as i16;
        let particles = (0..count)
            .map(|_| {
                let (x, y, z) = uniform_point(rng.next_unit(), rng.next_unit());
                let jitter = (rng.next_unit() - 0.5) * 2.0 * JITTER as f32;
                // Tres ondas a lo alto de la esfera, más un poco de azar para que no sean anillos.
                let phase = ((y + 1.0) * 1.5 + rng.next_unit() * 0.15) * FULL_TURN as f32;
                Particle {
                    x: q14(x),
                    y: q14(y),
                    z: q14(z),
                    radius: (ONE as f32 + jitter) as i16,
                    phase: phase as u32 as u16,
                }
            })
            .collect();
        ParticleCloud { particles }
    }

    pub fn len(&self) -> usize {
        self.particles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    /// Rectángulo que la esfera nunca excede con ningún pulso permitido. El doble buffer
    /// redibuja exactamente esta zona en cada frame.
    pub fn bounds(view: &View) -> Rect {
        let max_factor = mul(ONE + JITTER, Pulse::MAX_SCALE + Pulse::MAX_WAVE);
        let r = mul(view.radius, max_factor) + 3; // +1 del halo de cada partícula, +2 de redondeo
        Rect::new(view.cx - r, view.cy - r, 2 * r + 1, 2 * r + 1)
    }

    /// Dibuja la esfera sumando luz: donde se amontonan partículas (el borde) brilla más.
    pub fn draw(&self, c: &mut Canvas<'_>, view: &View, pulse: Pulse) {
        let pulse = pulse.clamped();
        let (s_spin, c_spin) = (trig::sin(view.spin), trig::cos(view.spin));
        let (s_tilt, c_tilt) = (trig::sin(view.tilt), trig::cos(view.tilt));
        for p in &self.particles {
            let (x, y, z) = (p.x as i32, p.y as i32, p.z as i32);
            // Rotación en Y (giro) y en X (inclinación). Todo Q14.
            let x1 = (x * c_spin + z * s_spin) >> 14;
            let z1 = (-x * s_spin + z * c_spin) >> 14;
            let y2 = (y * c_tilt - z1 * s_tilt) >> 14;
            let z2 = (y * s_tilt + z1 * c_tilt) >> 14;
            // Radio de esta partícula en este frame: su desvío × (escala + onda).
            let wave = mul(
                pulse.wave,
                trig::sin(pulse.wave_phase.wrapping_add(p.phase as u32)),
            );
            let factor = mul(p.radius as i32, pulse.scale + wave);
            let r = mul(view.radius, factor);
            let sx = view.cx + ((x1 * r) >> 14);
            let sy = view.cy - ((y2 * r) >> 14);

            // Profundidad -ONE..ONE → 0..255: atrás tenue y azul, adelante celeste.
            let t = ((z2 + ONE) * 255 / (2 * ONE)).clamp(0, 255) as u8;
            let color = theme::particle_deep().lerp(theme::particle_bright(), t);
            let strength = (70 + t as u32 / 2 + pulse.glow as u32 / 2).min(255) as u8;
            c.add(sx, sy, color.scale(strength));
            // Un halo mínimo en cruz para que cada partícula no sea un píxel duro.
            let soft = color.scale(strength / 4);
            c.add(sx + 1, sy, soft);
            c.add(sx - 1, sy, soft);
            c.add(sx, sy + 1, soft);
            c.add(sx, sy - 1, soft);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, PixelFormat};
    use std::vec;

    fn view(size: i32) -> View {
        View {
            cx: size / 2,
            cy: size / 2,
            radius: size * 29 / 100,
            spin: 9000,
            tilt: 3600,
        }
    }

    fn canvas(buf: &mut [u8], size: i32) -> Canvas<'_> {
        let s = size as usize;
        Canvas::new(buf, s, s, s, 4, PixelFormat::Rgb).unwrap()
    }

    #[test]
    fn los_puntos_estan_sobre_la_esfera_unitaria() {
        for i in 0..1000 {
            let (u, v) = (i as f32 / 1000.0, (i * 7 % 1000) as f32 / 1000.0);
            let (x, y, z) = uniform_point(u, v);
            let r = sqrtf(x * x + y * y + z * z);
            assert!((r - 1.0).abs() < 1e-3, "punto {i}: radio {r}");
        }
    }

    #[test]
    fn la_nube_es_uniforme_adelante_y_atras() {
        let cloud = ParticleCloud::new(4000);
        assert_eq!(cloud.len(), 4000);
        let adelante = cloud.particles.iter().filter(|p| p.z > 0).count() as i32;
        assert!((adelante - 2000).abs() < 200, "{adelante} de 4000 adelante");
    }

    #[test]
    fn nunca_sale_de_bounds_ni_con_el_pulso_maximo() {
        let size = 400;
        let cloud = ParticleCloud::new(6000);
        let v = view(size);
        let b = ParticleCloud::bounds(&v);
        for fase in [0, FULL_TURN / 4, FULL_TURN / 2, 3 * FULL_TURN / 4] {
            let mut buf = vec![0u8; (size * size * 4) as usize];
            let mut c = canvas(&mut buf, size);
            // Pulso fuera de rango a propósito: `draw` tiene que recortarlo.
            let pulse = Pulse {
                scale: ONE * 2,
                wave: ONE,
                wave_phase: fase,
                glow: 255,
            };
            cloud.draw(&mut c, &v, pulse);
            for y in 0..size {
                for x in 0..size {
                    let dentro = x >= b.x && x < b.x + b.w && y >= b.y && y < b.y + b.h;
                    if !dentro {
                        assert_eq!(
                            c.get(x, y),
                            Some(Color::BLACK),
                            "fuera de bounds: ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn el_pulso_agranda_la_esfera() {
        let size = 400;
        let cloud = ParticleCloud::new(3000);
        let v = view(size);
        let radio = |pulse: Pulse| {
            let mut buf = vec![0u8; (size * size * 4) as usize];
            let mut c = canvas(&mut buf, size);
            cloud.draw(&mut c, &v, pulse);
            // Distancia al centro del píxel encendido más lejano, en la fila del medio.
            (0..size)
                .filter(|&x| c.get(x, size / 2) != Some(Color::BLACK))
                .map(|x| (x - size / 2).abs())
                .max()
                .unwrap()
        };
        let reposo = radio(Pulse::REST);
        let hablando = radio(Pulse {
            scale: Pulse::MAX_SCALE,
            ..Pulse::REST
        });
        assert!(
            hablando > reposo + 5,
            "reposo {reposo}, hablando {hablando}"
        );
    }
}
