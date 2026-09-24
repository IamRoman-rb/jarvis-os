//! La esfera de partículas de JARVIS.
//!
//! Las partículas se reparten al azar pero de forma uniforme sobre la superficie (teorema de
//! Arquímedes: si la altura `y` es uniforme en [-1, 1] y el ángulo también, el área lo es), con
//! un pequeño desvío en el radio para el aspecto de "polvo". Se proyectan en 2D y brillan más
//! las que están adelante. El generador es determinista: la esfera sale igual en cada arranque.

use core::f32::consts::PI;

use libm::{cosf, sinf, sqrtf};

use crate::{Canvas, theme};

#[derive(Clone, Copy, Debug)]
pub struct Sphere {
    pub cx: f32,
    pub cy: f32,
    pub radius: f32,
    pub particles: u32,
    /// Rotación alrededor del eje vertical (radianes). En K1 se anima con el timer.
    pub spin: f32,
    /// Inclinación hacia la cámara (radianes).
    pub tilt: f32,
}

/// Punto proyectado: posición en pantalla y profundidad (-1 = atrás, 1 = adelante).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projected {
    pub x: f32,
    pub y: f32,
    pub depth: f32,
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

impl Sphere {
    /// Recorre todas las partículas ya proyectadas.
    pub fn for_each_projected(&self, mut f: impl FnMut(Projected)) {
        let (s_spin, c_spin) = (sinf(self.spin), cosf(self.spin));
        let (s_tilt, c_tilt) = (sinf(self.tilt), cosf(self.tilt));
        let mut rng = XorShift(0x4a41_5256); // "JARV"
        for _ in 0..self.particles {
            let (u, v) = (rng.next_unit(), rng.next_unit());
            let (x, y, z) = uniform_point(u, v);
            // Rotación en Y (giro) y en X (inclinación).
            let (x, z) = (x * c_spin + z * s_spin, -x * s_spin + z * c_spin);
            let (y, z) = (y * c_tilt - z * s_tilt, y * s_tilt + z * c_tilt);
            // Radio con ±2,5 % de ruido: la superficie se ve granulada, no perfecta.
            let jitter = 1.0 + (rng.next_unit() - 0.5) * 0.05;
            f(Projected {
                x: self.cx + x * self.radius * jitter,
                y: self.cy - y * self.radius * jitter,
                depth: z,
            });
        }
    }

    /// Dibuja la esfera sumando luz: donde se amontonan partículas (el borde) brilla más.
    pub fn draw(&self, c: &mut Canvas<'_>) {
        self.for_each_projected(|p| {
            // depth -1..1 → 0..255: las de atrás quedan tenues y azules, las de adelante celestes.
            let t = (((p.depth + 1.0) * 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            let color = theme::PARTICLE_DEEP.lerp(theme::PARTICLE_BRIGHT, t);
            let strength = 70 + t / 2;
            let (x, y) = (p.x as i32, p.y as i32);
            c.add(x, y, color.scale(strength));
            // Un halo mínimo en cruz para que cada partícula no sea un píxel duro.
            let soft = color.scale(strength / 4);
            c.add(x + 1, y, soft);
            c.add(x - 1, y, soft);
            c.add(x, y + 1, soft);
            c.add(x, y - 1, soft);
        });
    }
}

/// Esfera centrada en el canvas, con el tamaño de la referencia (~58 % del alto).
pub fn centered(c: &Canvas<'_>, particles: u32) -> Sphere {
    Sphere {
        cx: c.width() as f32 / 2.0,
        cy: c.height() as f32 / 2.0,
        radius: c.height() as f32 * 0.29,
        particles,
        spin: 0.6,
        tilt: 0.35,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, PixelFormat};
    use std::vec;

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
    fn la_proyeccion_queda_dentro_del_radio_con_ruido() {
        let s = Sphere {
            cx: 100.0,
            cy: 100.0,
            radius: 50.0,
            particles: 2000,
            spin: 1.0,
            tilt: 0.3,
        };
        let mut n = 0;
        let (mut front, mut back) = (0i32, 0i32);
        s.for_each_projected(|p| {
            let (dx, dy) = (p.x - 100.0, p.y - 100.0);
            assert!(sqrtf(dx * dx + dy * dy) <= 50.0 * 1.026);
            assert!((-1.0..=1.0).contains(&p.depth));
            if p.depth > 0.0 {
                front += 1
            } else {
                back += 1
            }
            n += 1;
        });
        assert_eq!(n, 2000);
        // Mitad adelante, mitad atrás (±5 %): la distribución es uniforme.
        assert!((front - back).abs() < 100, "{front} vs {back}");
    }

    #[test]
    fn dibuja_mas_luz_en_el_centro_que_afuera() {
        let mut buf = vec![0u8; 200 * 200 * 4];
        let mut c = Canvas::new(&mut buf, 200, 200, 200, 4, PixelFormat::Rgb).unwrap();
        let s = centered(&c, 3000);
        s.draw(&mut c);
        let lit_inside = (60..140)
            .flat_map(|y| (60..140).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y).is_some_and(|p| p != Color::BLACK))
            .count();
        assert!(lit_inside > 1000);
        assert_eq!(c.get(2, 2), Some(Color::BLACK));
    }
}
