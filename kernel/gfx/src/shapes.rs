//! Figuras simples con aritmética entera (en el kernel el punto flotante es por software y lento).

use crate::{Canvas, Color};

/// Línea de Bresenham: solo sumas y comparaciones enteras.
pub fn line(c: &mut Canvas<'_>, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    loop {
        c.put(x, y, color);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

pub fn rect_outline(c: &mut Canvas<'_>, x: i32, y: i32, w: i32, h: i32, color: Color) {
    line(c, x, y, x + w - 1, y, color);
    line(c, x, y + h - 1, x + w - 1, y + h - 1, color);
    line(c, x, y, x, y + h - 1, color);
    line(c, x + w - 1, y, x + w - 1, y + h - 1, color);
}

/// ¿El punto (px, py) cae dentro del rectángulo redondeado?
fn inside_rounded(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if px < x || py < y || px >= x + w || py >= y + h {
        return false;
    }
    // Centro de la esquina más cercana; si el punto está en una esquina, medir distancia.
    let cx = if px < x + r {
        x + r
    } else if px >= x + w - r {
        x + w - r - 1
    } else {
        px
    };
    let cy = if py < y + r {
        y + r
    } else if py >= y + h - r {
        y + h - r - 1
    } else {
        py
    };
    let (dx, dy) = (px - cx, py - cy);
    dx * dx + dy * dy <= r * r
}

/// Rectángulo redondeado relleno con opacidad (paneles "de vidrio").
#[allow(clippy::too_many_arguments)] // geometría + estilo: agruparlos no lo haría más claro
pub fn rounded_rect(
    c: &mut Canvas<'_>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    r: i32,
    color: Color,
    alpha: u8,
) {
    for py in y..y + h {
        for px in x..x + w {
            if inside_rounded(px, py, x, y, w, h, r) {
                c.blend(px, py, color, alpha);
            }
        }
    }
}

/// Borde de 1 px de un rectángulo redondeado.
pub fn rounded_outline(c: &mut Canvas<'_>, x: i32, y: i32, w: i32, h: i32, r: i32, color: Color) {
    for py in y..y + h {
        for px in x..x + w {
            let inside = inside_rounded(px, py, x, y, w, h, r);
            let inner = inside_rounded(px, py, x + 1, y + 1, w - 2, h - 2, (r - 1).max(0));
            if inside && !inner {
                c.put(px, py, color);
            }
        }
    }
}

/// Círculo relleno (`filled`) o solo el borde de 1 px.
pub fn circle(c: &mut Canvas<'_>, cx: i32, cy: i32, r: i32, color: Color, filled: bool) {
    let (outer, inner) = (r * r, (r - 1) * (r - 1));
    for dy in -r..=r {
        for dx in -r..=r {
            let d = dx * dx + dy * dy;
            if d <= outer && (filled || d > inner) {
                c.put(cx + dx, cy + dy, color);
            }
        }
    }
}

/// Resplandor radial: suma luz que se apaga con la distancia al centro.
pub fn glow(c: &mut Canvas<'_>, cx: i32, cy: i32, radius: i32, color: Color, strength: u8) {
    let r2 = (radius as i64) * (radius as i64);
    for y in (cy - radius).max(0)..(cy + radius).min(c.height() as i32) {
        for x in (cx - radius).max(0)..(cx + radius).min(c.width() as i32) {
            let (dx, dy) = ((x - cx) as i64, (y - cy) as i64);
            let d2 = dx * dx + dy * dy;
            if d2 < r2 {
                // Caída cuadrática: suave en el centro, se apaga rápido hacia el borde.
                let t = ((r2 - d2) * 255 / r2) as u32;
                let a = (t * t / 255 * strength as u32 / 255) as u8;
                c.add(x, y, color.scale(a));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use std::vec;

    fn lit(buf: &[u8]) -> usize {
        buf.chunks(4)
            .filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
            .count()
    }

    #[test]
    fn linea_diagonal_toca_ambos_extremos() {
        let mut buf = vec![0u8; 10 * 10 * 4];
        let mut c = Canvas::new(&mut buf, 10, 10, 10, 4, PixelFormat::Rgb).unwrap();
        line(&mut c, 0, 0, 9, 9, Color::WHITE);
        assert_eq!(c.get(0, 0), Some(Color::WHITE));
        assert_eq!(c.get(9, 9), Some(Color::WHITE));
        assert_eq!(lit(&buf), 10);
    }

    #[test]
    fn rectangulo_redondeado_no_pinta_las_esquinas() {
        let mut buf = vec![0u8; 20 * 20 * 4];
        let mut c = Canvas::new(&mut buf, 20, 20, 20, 4, PixelFormat::Rgb).unwrap();
        rounded_rect(&mut c, 0, 0, 20, 20, 6, Color::WHITE, 255);
        assert_eq!(c.get(0, 0), Some(Color::BLACK));
        assert_eq!(c.get(10, 10), Some(Color::WHITE));
        assert_eq!(c.get(10, 0), Some(Color::WHITE));
    }

    #[test]
    fn el_resplandor_es_mas_fuerte_en_el_centro() {
        let mut buf = vec![0u8; 40 * 40 * 4];
        let mut c = Canvas::new(&mut buf, 40, 40, 40, 4, PixelFormat::Rgb).unwrap();
        glow(&mut c, 20, 20, 15, Color::WHITE, 255);
        let centro = c.get(20, 20).unwrap().r;
        let borde = c.get(20, 33).unwrap().r;
        assert!(centro > borde);
        assert_eq!(c.get(0, 0), Some(Color::BLACK));
    }
}
