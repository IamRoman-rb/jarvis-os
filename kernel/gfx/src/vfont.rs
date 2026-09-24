//! Fuente **vectorial** para textos grandes (el reloj y el título "JARVIS").
//!
//! La fuente bitmap (`text.rs`) llega hasta 32 px. Para la hora se la agrandaba ×2 repitiendo
//! píxeles, y los bordes suavizados de la fuente se volvían "escalones" borrosos. Acá cada letra
//! está definida como trazos (segmentos y arcos) en una grilla de 60×100 unidades, y se rasteriza
//! al tamaño exacto que se pide: el borde siempre mide un píxel de suavizado, sea cual sea el
//! tamaño. Es la misma idea que usan las fuentes TrueType, pero con trazos en vez de contornos.
//!
//! Rasterizar es caro (para cada píxel se mide la distancia a cada trazo), así que cada letra se
//! rasteriza **una sola vez** a una máscara de opacidad y queda en caché ([`VectorText`]). Después
//! dibujar es solo mezclar la máscara. Todo es aritmética entera: nada de `f32` por frame.

use alloc::vec::Vec;

use crate::trig::{FULL_TURN, ONE, sin};
use crate::{Canvas, Color};

/// Un trazo en unidades de la grilla (60 de ancho × 100 de alto, y hacia abajo).
#[derive(Clone, Copy)]
enum Stroke {
    Line(i32, i32, i32, i32),
    /// Arco de elipse: centro, radios y ángulos inicial y final en grados (0° = derecha,
    /// 90° = abajo, porque la y crece hacia abajo). El final puede pasar de 360.
    Arc(i32, i32, i32, i32, i32, i32),
    /// Un punto (los ":" del reloj): un trazo de largo cero es un círculo.
    Dot(i32, i32),
}

use Stroke::{Arc, Dot, Line};

/// (ancho en unidades, trazos) de cada carácter.
fn glyph(ch: char) -> Option<(i32, &'static [Stroke])> {
    Some(match ch {
        '0' => (60, &[Arc(30, 50, 26, 46, 0, 360)]),
        '1' => (60, &[Line(16, 20, 34, 4), Line(34, 4, 34, 96)]),
        '2' => (
            60,
            &[
                Arc(30, 30, 25, 26, 180, 400),
                Line(49, 47, 5, 96),
                Line(5, 96, 56, 96),
            ],
        ),
        '3' => (
            60,
            &[Arc(30, 27, 23, 23, 200, 450), Arc(30, 73, 25, 23, 270, 520)],
        ),
        '4' => (
            60,
            &[Line(42, 96, 42, 4), Line(42, 4, 4, 70), Line(4, 70, 57, 70)],
        ),
        '5' => (
            60,
            &[
                Line(52, 4, 11, 4),
                Line(11, 4, 8, 48),
                Arc(30, 66, 25, 30, 215, 500),
            ],
        ),
        '6' => (
            60,
            &[Arc(30, 70, 25, 26, 0, 360), Arc(58, 70, 53, 64, 180, 245)],
        ),
        '7' => (60, &[Line(4, 4, 56, 4), Line(56, 4, 22, 96)]),
        '8' => (
            60,
            &[Arc(30, 26, 21, 22, 0, 360), Arc(30, 72, 25, 24, 0, 360)],
        ),
        '9' => (
            60,
            &[Arc(30, 30, 25, 26, 0, 360), Arc(2, 30, 53, 64, 0, 65)],
        ),
        ':' => (24, &[Arc(12, 34, 2, 2, 0, 360), Arc(12, 72, 2, 2, 0, 360)]),
        '.' => (24, &[Dot(12, 94)]),
        ' ' => (30, &[]),
        '-' => (40, &[Line(6, 54, 34, 54)]),
        'A' => (
            60,
            &[
                Line(4, 96, 30, 4),
                Line(30, 4, 56, 96),
                Line(13, 64, 47, 64),
            ],
        ),
        'I' => (20, &[Line(10, 4, 10, 96)]),
        'J' => (56, &[Line(44, 4, 44, 72), Arc(26, 72, 18, 24, 0, 180)]),
        'R' => (
            60,
            &[
                Line(8, 96, 8, 4),
                Line(8, 4, 34, 4),
                Arc(34, 27, 22, 23, 270, 450),
                Line(34, 50, 8, 50),
                Line(30, 50, 56, 96),
            ],
        ),
        'S' => (
            60,
            &[Arc(30, 27, 24, 23, 90, 340), Arc(30, 73, 25, 23, 270, 520)],
        ),
        'V' => (60, &[Line(4, 4, 30, 96), Line(30, 96, 56, 4)]),
        _ => return None,
    })
}

/// Segmentos en 1/64 de píxel, ya escalados.
fn segments(strokes: &[Stroke], scale_q8: i32) -> Vec<(i32, i32, i32, i32)> {
    // unidades → 1/64 px: u * scale_q8 / 256 * 64 = u * scale_q8 / 4
    let p = |u: i32| u * scale_q8 / 4;
    let mut out = Vec::new();
    for s in strokes {
        match *s {
            Line(x0, y0, x1, y1) => out.push((p(x0), p(y0), p(x1), p(y1))),
            Dot(x, y) => out.push((p(x), p(y), p(x), p(y))),
            Arc(cx, cy, rx, ry, a0, a1) => {
                // Un punto cada 10°: con el suavizado no se notan las aristas.
                let steps = ((a1 - a0).abs() / 10).max(2);
                let point = |i: i32| {
                    let deg = a0 + (a1 - a0) * i / steps;
                    let turn = (deg.rem_euclid(360) as u32) * FULL_TURN / 360;
                    let (s, c) = (sin(turn), sin(turn.wrapping_add(FULL_TURN / 4)));
                    // (cx + rx·cos, cy + ry·sin) en unidades ×ONE, después a 1/64 px.
                    let x = (cx * ONE + rx * c) as i64 * scale_q8 as i64 / 4 / ONE as i64;
                    let y = (cy * ONE + ry * s) as i64 * scale_q8 as i64 / 4 / ONE as i64;
                    (x as i32, y as i32)
                };
                let mut prev = point(0);
                for i in 1..=steps {
                    let next = point(i);
                    out.push((prev.0, prev.1, next.0, next.1));
                    prev = next;
                }
            }
        }
    }
    out
}

/// Raíz cuadrada entera (método de Newton).
fn isqrt(n: i64) -> i64 {
    if n < 2 {
        return n.max(0);
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Distancia (en 1/64 px) del punto al segmento.
fn distance(px: i64, py: i64, s: &(i32, i32, i32, i32)) -> i64 {
    let (ax, ay, bx, by) = (s.0 as i64, s.1 as i64, s.2 as i64, s.3 as i64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let (cx, cy) = if len2 == 0 {
        (ax, ay)
    } else {
        let t = ((px - ax) * dx + (py - ay) * dy).clamp(0, len2);
        (ax + dx * t / len2, ay + dy * t / len2)
    };
    isqrt((px - cx) * (px - cx) + (py - cy) * (py - cy))
}

/// Máscara de opacidad de una letra ya rasterizada.
struct Mask {
    ch: char,
    /// Desplazamiento de la máscara respecto de la esquina de la letra (por el grosor del trazo).
    pad: i32,
    w: i32,
    h: i32,
    alpha: Vec<u8>,
    advance: i32,
}

/// Texto vectorial de un tamaño y grosor fijos, con las letras rasterizadas en caché.
pub struct VectorText {
    height: i32,
    /// Grosor del trazo en 1/64 de píxel.
    stroke64: i32,
    tracking: i32,
    cache: Vec<Mask>,
}

impl VectorText {
    /// `height`: alto de las letras en píxeles; `stroke64`: grosor en 1/64 de píxel
    /// (por ejemplo, 3 px = 192).
    pub const fn new(height: i32, stroke64: i32, tracking: i32) -> Self {
        VectorText {
            height,
            stroke64,
            tracking,
            cache: Vec::new(),
        }
    }

    fn scale_q8(&self) -> i32 {
        self.height * 256 / 100
    }

    fn advance(&self, ch: char) -> i32 {
        let units = glyph(ch).map_or(60, |(w, _)| w);
        units * self.scale_q8() / 256
    }

    /// Ancho en píxeles de `text` (sin el halo del trazo).
    pub fn width(&self, text: &str) -> i32 {
        let n = text.chars().count() as i32;
        let letters: i32 = text.chars().map(|c| self.advance(c)).sum();
        letters + (n - 1).max(0) * self.tracking
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    fn mask(&mut self, ch: char) -> Option<&Mask> {
        if !self.cache.iter().any(|m| m.ch == ch) {
            let m = self.rasterize(ch)?;
            self.cache.push(m);
        }
        self.cache.iter().find(|m| m.ch == ch)
    }

    fn rasterize(&self, ch: char) -> Option<Mask> {
        let (units, strokes) = glyph(ch)?;
        let segs = segments(strokes, self.scale_q8());
        let half = self.stroke64 / 2;
        let pad = half / 64 + 2;
        let advance = units * self.scale_q8() / 256;
        let (w, h) = (advance + 2 * pad, self.height + 2 * pad);
        let mut alpha = alloc::vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                // Centro del píxel, en 1/64 px y en coordenadas de la letra.
                let px = ((x - pad) * 64 + 32) as i64;
                let py = ((y - pad) * 64 + 32) as i64;
                let d = segs
                    .iter()
                    .map(|s| distance(px, py, s))
                    .min()
                    .unwrap_or(i64::MAX);
                // Cobertura: 1 dentro del trazo, 0 fuera, y una rampa de 1 px en el borde.
                let cover = (half as i64 + 32 - d).clamp(0, 64);
                alpha[(y * w + x) as usize] = (cover * 255 / 64) as u8;
            }
        }
        Some(Mask {
            ch,
            pad,
            w,
            h,
            alpha,
            advance,
        })
    }

    /// Dibuja `text` con la esquina superior izquierda en (x, y). Devuelve el ancho.
    pub fn draw(&mut self, c: &mut Canvas<'_>, x: i32, y: i32, text: &str, color: Color) -> i32 {
        self.draw_alpha(c, x, y, text, color, 255)
    }

    /// Como [`draw`](Self::draw), con una opacidad general (para halos).
    pub fn draw_alpha(
        &mut self,
        c: &mut Canvas<'_>,
        x: i32,
        y: i32,
        text: &str,
        color: Color,
        opacity: u8,
    ) -> i32 {
        let tracking = self.tracking;
        let mut pen = x;
        for ch in text.chars() {
            let advance = match self.mask(ch) {
                Some(m) => {
                    for my in 0..m.h {
                        for mx in 0..m.w {
                            let a = m.alpha[(my * m.w + mx) as usize];
                            if a > 0 {
                                let a = (a as u32 * opacity as u32 / 255) as u8;
                                c.blend(pen + mx - m.pad, y + my - m.pad, color, a);
                            }
                        }
                    }
                    m.advance
                }
                None => self.advance(ch),
            };
            pen += advance + tracking;
        }
        self.width(text)
    }

    /// Margen que ocupa el trazo fuera de la caja de las letras.
    pub fn overhang(&self) -> i32 {
        self.stroke64 / 128 + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use std::vec;

    #[test]
    fn todas_las_letras_del_reloj_y_el_titulo_existen() {
        for ch in "0123456789:JARVIS -".chars() {
            assert!(glyph(ch).is_some(), "falta {ch}");
        }
    }

    #[test]
    fn el_borde_es_nitido_a_cualquier_tamanio() {
        // Un trazo vertical ("1"): en el centro del trazo la opacidad es total y a más de un
        // píxel del borde es cero, con como mucho 2 píxeles intermedios (el suavizado).
        for height in [40, 64, 96] {
            let mut t = VectorText::new(height, 4 * 64, 0);
            let m = t.mask('1').unwrap();
            let row = m.h / 2;
            let line = &m.alpha[(row * m.w) as usize..((row + 1) * m.w) as usize];
            let full = line.iter().filter(|a| **a == 255).count();
            let partial = line.iter().filter(|a| **a > 0 && **a < 255).count();
            assert!(full >= 3, "alto {height}: {line:?}");
            assert!(partial <= 2, "alto {height}: borde borroso {line:?}");
        }
    }

    #[test]
    fn ancho_y_dibujo() {
        let mut t = VectorText::new(64, 3 * 64, 4);
        assert_eq!(
            t.width("00:00"),
            4 * t.advance('0') + t.advance(':') + 4 * 4
        );
        let (w, h) = (400, 100);
        let mut buf = vec![0u8; w * h * 4];
        let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Rgb).unwrap();
        let drawn = t.draw(&mut c, 10, 10, "12:34", Color::WHITE);
        assert_eq!(drawn, t.width("12:34"));
        assert!(buf.contains(&255));
        assert_eq!(isqrt(1_000_000), 1000);
        assert_eq!(isqrt(15), 3);
    }
}
