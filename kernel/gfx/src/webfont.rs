//! Fuente **proporcional** para las páginas web: DejaVu Sans Condensed (de anchos parecidos a
//! Arial, la que suponen casi todas las páginas) y DejaVu Sans Mono para el código,
//! rasterizada en el momento con [`fontdue`] al tamaño exacto que pide la página.
//!
//! La fuente bitmap del HUD (`text.rs`) es monoespaciada y viene en cuatro tamaños (16, 20, 24
//! y 32 px): alcanza para la interfaz, pero una página con letra de 13 px y de 40 px se veía como
//! una terminal. Acá cada letra se dibuja con su ancho real (una "i" ocupa menos que una "m") y en
//! cualquier tamaño de 6 a 96 px.
//!
//! Rasterizar una letra es caro (y en el kernel no hay unidad de punto flotante: los `f32` son por
//! software), así que cada letra se rasteriza **una sola vez** por tamaño y estilo y queda en
//! caché; los anchos también. Medir y dibujar después es aritmética entera.
//!
//! Los archivos están en `gfx/fonts/` (licencia libre de Bitstream Vera/DejaVu, en
//! `gfx/fonts/LICENSE`).
//!
//! La cursiva no tiene archivo propio: se inclina la letra normal al dibujarla (como hacen los
//! navegadores cuando falta la variante).

use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

use fontdue::{Font, FontSettings};
use hashbrown::HashMap;

use crate::{Canvas, Color};

/// Cómo se ve un texto de la página.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FontSpec {
    /// Tamaño en píxeles (la altura "em").
    pub px: u16,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
}

impl FontSpec {
    pub const fn new(px: u16) -> FontSpec {
        FontSpec {
            px,
            bold: false,
            italic: false,
            mono: false,
        }
    }

    fn face(&self) -> usize {
        match (self.mono, self.bold) {
            (true, _) => 2,
            (false, true) => 1,
            (false, false) => 0,
        }
    }

    fn clamped(self) -> FontSpec {
        FontSpec {
            px: self.px.clamp(6, 96),
            ..self
        }
    }
}

/// Una letra ya rasterizada: su máscara de opacidad y dónde va respecto de la línea de base.
struct Glyph {
    w: i32,
    h: i32,
    /// Desde el origen de la letra hasta el borde izquierdo de la máscara.
    xmin: i32,
    /// Desde la línea de base hasta el borde de arriba de la máscara (hacia arriba, positivo).
    top: i32,
    mask: Vec<u8>,
}

/// Medidas verticales de un tamaño de letra.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VMetrics {
    /// Desde la línea de base hasta arriba de las letras más altas.
    pub ascent: i32,
    /// Desde la línea de base hasta abajo (positivo).
    pub descent: i32,
    /// Alto "normal" de un renglón (`line-height: normal`).
    pub line_height: i32,
}

/// Las fuentes de las páginas, con sus cachés.
pub struct WebFonts {
    faces: [Font; 3],
    /// Ancho de cada letra, en 1/64 de píxel.
    advances: RefCell<HashMap<(char, u16, u8), i32>>,
    glyphs: RefCell<HashMap<(char, u16, u8), Rc<Glyph>>>,
    vmetrics: RefCell<HashMap<(u16, u8), VMetrics>>,
}

fn load(bytes: &'static [u8]) -> Font {
    let settings = FontSettings {
        // Sin sustituciones (ligaduras, formas árabes): no las usamos y ahorran memoria.
        load_substitutions: false,
        ..FontSettings::default()
    };
    Font::from_bytes(bytes, settings).expect("fuente DejaVu válida")
}

impl Default for WebFonts {
    fn default() -> Self {
        Self::new()
    }
}

impl WebFonts {
    pub fn new() -> WebFonts {
        WebFonts {
            faces: [
                load(include_bytes!("../fonts/DejaVuSansCondensed.ttf")),
                load(include_bytes!("../fonts/DejaVuSansCondensed-Bold.ttf")),
                load(include_bytes!("../fonts/DejaVuSansMono.ttf")),
            ],
            advances: RefCell::new(HashMap::new()),
            glyphs: RefCell::new(HashMap::new()),
            vmetrics: RefCell::new(HashMap::new()),
        }
    }

    fn key(spec: FontSpec) -> u8 {
        spec.face() as u8 | ((spec.italic as u8) << 2) | ((spec.bold as u8) << 3)
    }

    /// Ancho de una letra en 1/64 de píxel.
    fn advance64(&self, ch: char, spec: FontSpec) -> i32 {
        let k = (ch, spec.px, spec.face() as u8);
        if let Some(a) = self.advances.borrow().get(&k) {
            return *a;
        }
        let font = &self.faces[spec.face()];
        let m = font.metrics(ch, spec.px as f32);
        let mut a = (m.advance_width * 64.0) as i32;
        // La negrita del mono se simula (un píxel más gruesa): no cambia el ancho.
        if a <= 0 && !ch.is_whitespace() && !font.has_glyph(ch) {
            a = spec.px as i32 * 32;
        }
        self.advances.borrow_mut().insert(k, a);
        a
    }

    /// Ancho en píxeles de `text`.
    pub fn width(&self, text: &str, spec: FontSpec) -> i32 {
        let spec = spec.clamped();
        let sum: i32 = text.chars().map(|c| self.advance64(c, spec)).sum();
        (sum + 32) / 64
    }

    /// Ancho de cada letra de `text`, en 1/64 de píxel (para cortar palabras largas).
    pub fn advances(&self, text: &str, spec: FontSpec) -> Vec<i32> {
        let spec = spec.clamped();
        text.chars().map(|c| self.advance64(c, spec)).collect()
    }

    pub fn vmetrics(&self, spec: FontSpec) -> VMetrics {
        let spec = spec.clamped();
        let k = (spec.px, spec.face() as u8);
        if let Some(m) = self.vmetrics.borrow().get(&k) {
            return *m;
        }
        let px = spec.px as f32;
        let m = match self.faces[spec.face()].horizontal_line_metrics(px) {
            Some(l) => VMetrics {
                ascent: (l.ascent + 0.5) as i32,
                descent: (-l.descent + 0.5) as i32,
                line_height: (l.new_line_size + 0.5) as i32,
            },
            None => VMetrics {
                ascent: spec.px as i32 * 4 / 5,
                descent: spec.px as i32 / 5,
                line_height: spec.px as i32 * 6 / 5,
            },
        };
        self.vmetrics.borrow_mut().insert(k, m);
        m
    }

    fn glyph(&self, ch: char, spec: FontSpec) -> Rc<Glyph> {
        let k = (ch, spec.px, Self::key(spec));
        if let Some(g) = self.glyphs.borrow().get(&k) {
            return g.clone();
        }
        let (m, mask) = self.faces[spec.face()].rasterize(ch, spec.px as f32);
        let g = Rc::new(Glyph {
            w: m.width as i32,
            h: m.height as i32,
            xmin: m.xmin,
            top: m.ymin + m.height as i32,
            mask,
        });
        // Un tope para que una página con miles de letras distintas no llene la memoria.
        let mut cache = self.glyphs.borrow_mut();
        if cache.len() > 20_000 {
            cache.clear();
        }
        cache.insert(k, g.clone());
        g
    }

    /// Dibuja `text` con la línea de base en `baseline`, empezando en `x`. Devuelve el ancho.
    pub fn draw(
        &self,
        c: &mut Canvas<'_>,
        x: i32,
        baseline: i32,
        text: &str,
        spec: FontSpec,
        color: Color,
    ) -> i32 {
        let spec = spec.clamped();
        // Negrita del mono: se dibuja dos veces corrida un píxel.
        let fake_bold = spec.mono && spec.bold;
        let mut pen64 = x * 64;
        for ch in text.chars() {
            let adv = self.advance64(ch, spec);
            if !ch.is_whitespace() {
                let g = self.glyph(ch, spec);
                let gx = (pen64 + 32) / 64 + g.xmin;
                let gy = baseline - g.top;
                for row in 0..g.h {
                    // Cursiva: cada fila se corre según su altura sobre la línea de base.
                    let shear = if spec.italic {
                        (baseline - (gy + row)) / 5
                    } else {
                        0
                    };
                    let line = &g.mask[(row * g.w) as usize..((row + 1) * g.w) as usize];
                    for (col, &a) in line.iter().enumerate() {
                        if a == 0 {
                            continue;
                        }
                        let px = gx + col as i32 + shear;
                        c.blend(px, gy + row, color, a);
                        if fake_bold {
                            c.blend(px + 1, gy + row, color, a);
                        }
                    }
                }
            }
            pen64 += adv;
        }
        (pen64 - x * 64 + 32) / 64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn es_proporcional_y_de_cualquier_tamano() {
        let f = WebFonts::new();
        let s = FontSpec::new(16);
        assert!(f.width("iiii", s) < f.width("mmmm", s), "proporcional");
        assert!(f.width("hola", FontSpec::new(13)) < f.width("hola", s));
        assert!(f.width("hola", FontSpec { bold: true, ..s }) >= f.width("hola", s));
        let m = f.vmetrics(s);
        assert!(
            m.ascent > 10 && m.descent > 2 && m.line_height >= 17,
            "{m:?}"
        );
        // Letras que la fuente bitmap no tenía (comillas, rayas, flechas, euro).
        assert!(f.width("“—→€”", s) > 30);
    }

    #[test]
    fn dibuja_con_la_base_donde_se_pide() {
        let f = WebFonts::new();
        let mut buf = alloc::vec![0u8; 80 * 40 * 3];
        let mut c = Canvas::new(&mut buf, 80, 40, 80, 3, crate::PixelFormat::Rgb).unwrap();
        let w = f.draw(&mut c, 2, 30, "Hx", FontSpec::new(20), Color::WHITE);
        assert!(w > 15);
        // Nada debajo de la línea de base (la "H" y la "x" no bajan) y algo arriba.
        let lit = |c: &Canvas<'_>, y: i32| (0..80).any(|x| c.get(x, y).is_some_and(|p| p.r > 0));
        assert!(lit(&c, 25));
        assert!(!lit(&c, 33));
    }
}
