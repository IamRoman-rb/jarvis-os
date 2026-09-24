//! Texto con la fuente bitmap Noto Sans Mono (ya rasterizada: no hace falta un motor de fuentes).

use noto_sans_mono_bitmap::{FontWeight, RasterHeight, get_raster, get_raster_width};

use crate::{Canvas, Color};

pub use noto_sans_mono_bitmap::{FontWeight as Weight, RasterHeight as Size};

/// Estilo de un texto. `scale` agranda cada píxel de la fuente (títulos gigantes del HUD).
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub weight: FontWeight,
    pub size: RasterHeight,
    pub scale: i32,
    /// Espacio extra entre letras en píxeles (el HUD usa mayúsculas espaciadas).
    pub tracking: i32,
    pub color: Color,
}

impl Style {
    pub const fn new(weight: FontWeight, size: RasterHeight, color: Color) -> Self {
        Style {
            weight,
            size,
            scale: 1,
            tracking: 0,
            color,
        }
    }

    pub const fn scale(mut self, scale: i32) -> Self {
        self.scale = scale;
        self
    }

    pub const fn tracking(mut self, px: i32) -> Self {
        self.tracking = px;
        self
    }

    pub const fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn line_height(&self) -> i32 {
        self.size.val() as i32 * self.scale
    }

    fn advance(&self) -> i32 {
        get_raster_width(self.weight, self.size) as i32 * self.scale + self.tracking
    }
}

/// Ancho en píxeles que ocupa `text`.
pub fn width(text: &str, style: &Style) -> i32 {
    let n = text.chars().count() as i32;
    if n == 0 {
        0
    } else {
        n * style.advance() - style.tracking
    }
}

/// Dibuja `text` con la esquina superior izquierda en (x, y). Devuelve el ancho dibujado.
/// Los caracteres que la fuente no tiene se reemplazan por `?`.
pub fn draw(c: &mut Canvas<'_>, x: i32, y: i32, text: &str, style: &Style) -> i32 {
    let mut pen = x;
    for ch in text.chars() {
        let glyph = get_raster(ch, style.weight, style.size)
            .or_else(|| get_raster('?', style.weight, style.size));
        if let Some(glyph) = glyph {
            for (gy, row) in glyph.raster().iter().enumerate() {
                for (gx, &intensity) in row.iter().enumerate() {
                    if intensity == 0 {
                        continue;
                    }
                    for sy in 0..style.scale {
                        for sx in 0..style.scale {
                            c.blend(
                                pen + gx as i32 * style.scale + sx,
                                y + gy as i32 * style.scale + sy,
                                style.color,
                                intensity,
                            );
                        }
                    }
                }
            }
        }
        pen += style.advance();
    }
    width(text, style)
}

/// Recorta `text` para que entre en `max_width` píxeles, terminando en ".." si hubo que cortar.
pub fn fit(text: &str, style: &Style, max_width: i32) -> alloc::string::String {
    if width(text, style) <= max_width {
        return text.into();
    }
    let mut out = alloc::string::String::new();
    for ch in text.chars() {
        out.push(ch);
        out.push_str("..");
        if width(&out, style) > max_width {
            out.truncate(out.len() - 2 - ch.len_utf8());
            out.push_str("..");
            return out;
        }
        out.truncate(out.len() - 2);
    }
    out
}

/// Igual que [`draw`], pero alineado a la derecha: `right` es el borde derecho.
pub fn draw_right(c: &mut Canvas<'_>, right: i32, y: i32, text: &str, style: &Style) -> i32 {
    draw(c, right - width(text, style), y, text, style)
}

/// Texto con resplandor: primero un halo tenue alrededor, después el texto nítido encima.
pub fn draw_glowing(c: &mut Canvas<'_>, x: i32, y: i32, text: &str, style: &Style, halo: Color) {
    let halo_style = style.color(halo);
    for (dx, dy) in [
        (-2, 0),
        (2, 0),
        (0, -2),
        (0, 2),
        (-1, -1),
        (1, 1),
        (-1, 1),
        (1, -1),
    ] {
        draw(c, x + dx, y + dy, text, &halo_style);
    }
    draw(c, x, y, text, style);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use std::vec;

    const S: Style = Style::new(FontWeight::Regular, RasterHeight::Size16, Color::WHITE);

    #[test]
    fn ancho_con_escala_y_espaciado() {
        let base = width("JARVIS", &S);
        assert_eq!(width("", &S), 0);
        assert_eq!(width("JARVIS", &S.scale(2)), base * 2);
        assert_eq!(width("JARVIS", &S.tracking(3)), base + 5 * 3);
    }

    #[test]
    fn fit_recorta_con_puntos() {
        let w = width("abcdefghij", &S);
        assert_eq!(fit("abcdefghij", &S, w), "abcdefghij");
        let corto = fit("abcdefghij", &S, w / 2);
        assert!(
            corto.ends_with("..") && width(&corto, &S) <= w / 2,
            "{corto}"
        );
        assert_eq!(fit("", &S, 10), "");
    }

    #[test]
    fn dibuja_acentos_y_reemplaza_lo_desconocido() {
        let mut buf = vec![0u8; 200 * 20 * 4];
        let mut c = Canvas::new(&mut buf, 200, 20, 200, 4, PixelFormat::Rgb).unwrap();
        let w = draw(&mut c, 0, 0, "MIÉRCOLES ☃", &S);
        assert_eq!(w, width("MIÉRCOLES ☃", &S));
        assert!(buf.iter().any(|b| *b > 0));
    }
}
