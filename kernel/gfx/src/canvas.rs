//! Framebuffer genérico: un slice de bytes más su geometría y formato de píxel.

/// Color RGB de 8 bits por canal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color::hex(0x000000);
    pub const WHITE: Color = Color::hex(0xffffff);

    /// `Color::hex(0x00f0ff)`: mismo formato que el design system.
    pub const fn hex(rgb: u32) -> Color {
        Color {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
        }
    }

    /// Escala el brillo: `alpha` en 0..=255 (255 = sin cambios).
    pub const fn scale(self, alpha: u8) -> Color {
        let a = alpha as u16;
        Color {
            r: ((self.r as u16 * a) / 255) as u8,
            g: ((self.g as u16 * a) / 255) as u8,
            b: ((self.b as u16 * a) / 255) as u8,
        }
    }

    /// Interpolación lineal: `t` en 0..=255 (0 = `self`, 255 = `other`).
    pub const fn lerp(self, other: Color, t: u8) -> Color {
        const fn mix(a: u8, b: u8, t: u8) -> u8 {
            let t = t as i32;
            (a as i32 + ((b as i32 - a as i32) * t) / 255) as u8
        }
        Color {
            r: mix(self.r, other.r, t),
            g: mix(self.g, other.g, t),
            b: mix(self.b, other.b, t),
        }
    }

    /// Suma saturada: así se acumula la luz (cuantas más partículas, más brillo).
    pub const fn add(self, other: Color) -> Color {
        Color {
            r: self.r.saturating_add(other.r),
            g: self.g.saturating_add(other.g),
            b: self.b.saturating_add(other.b),
        }
    }

    /// Luminancia aproximada (para framebuffers en escala de grises).
    pub const fn luma(self) -> u8 {
        ((self.r as u32 * 77 + self.g as u32 * 150 + self.b as u32 * 29) >> 8) as u8
    }
}

/// Orden de los bytes de cada píxel en memoria.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    Gray,
}

/// Superficie de dibujo. `stride` es el ancho real de cada fila en píxeles (puede ser mayor que
/// `width`: el hardware a veces agrega relleno al final de cada fila).
pub struct Canvas<'a> {
    buf: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    format: PixelFormat,
}

impl<'a> Canvas<'a> {
    /// Devuelve `None` si el buffer es más chico de lo que dice la geometría.
    pub fn new(
        buf: &'a mut [u8],
        width: usize,
        height: usize,
        stride: usize,
        bytes_per_pixel: usize,
        format: PixelFormat,
    ) -> Option<Self> {
        let min_bpp = if format == PixelFormat::Gray { 1 } else { 3 };
        if stride < width
            || bytes_per_pixel < min_bpp
            || buf.len() < stride * height * bytes_per_pixel
        {
            return None;
        }
        Some(Canvas {
            buf,
            width,
            height,
            stride,
            bytes_per_pixel,
            format,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    fn offset(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return None;
        }
        Some((y as usize * self.stride + x as usize) * self.bytes_per_pixel)
    }

    /// Pinta un píxel. Fuera de la pantalla no hace nada (así las figuras se recortan solas).
    pub fn put(&mut self, x: i32, y: i32, c: Color) {
        let Some(o) = self.offset(x, y) else { return };
        let px = &mut self.buf[o..o + self.bytes_per_pixel];
        match self.format {
            PixelFormat::Rgb => {
                px[0] = c.r;
                px[1] = c.g;
                px[2] = c.b;
            }
            PixelFormat::Bgr => {
                px[0] = c.b;
                px[1] = c.g;
                px[2] = c.r;
            }
            PixelFormat::Gray => px[0] = c.luma(),
        }
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Color> {
        let o = self.offset(x, y)?;
        let px = &self.buf[o..o + self.bytes_per_pixel];
        Some(match self.format {
            PixelFormat::Rgb => Color {
                r: px[0],
                g: px[1],
                b: px[2],
            },
            PixelFormat::Bgr => Color {
                r: px[2],
                g: px[1],
                b: px[0],
            },
            PixelFormat::Gray => Color {
                r: px[0],
                g: px[0],
                b: px[0],
            },
        })
    }

    /// Suma luz sobre lo que ya hay (mezcla aditiva).
    pub fn add(&mut self, x: i32, y: i32, c: Color) {
        if let Some(old) = self.get(x, y) {
            self.put(x, y, old.add(c));
        }
    }

    /// Mezcla `c` sobre el píxel con opacidad `alpha` (0..=255).
    pub fn blend(&mut self, x: i32, y: i32, c: Color, alpha: u8) {
        if let Some(old) = self.get(x, y) {
            self.put(x, y, old.lerp(c, alpha));
        }
    }

    pub fn fill(&mut self, c: Color) {
        self.fill_rect(0, 0, self.width as i32, self.height as i32, c);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        for yy in y.max(0)..(y + h).min(self.height as i32) {
            for xx in x.max(0)..(x + w).min(self.width as i32) {
                self.put(xx, yy, c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    #[test]
    fn hex_y_operaciones_de_color() {
        let c = Color::hex(0x00f0ff);
        assert_eq!((c.r, c.g, c.b), (0x00, 0xf0, 0xff));
        assert_eq!(c.scale(0), Color::BLACK);
        assert_eq!(c.scale(255), c);
        assert_eq!(Color::BLACK.lerp(Color::WHITE, 255), Color::WHITE);
        assert_eq!(Color::hex(0xf0f0f0).add(Color::hex(0x202020)), Color::WHITE);
    }

    #[test]
    fn rgb_y_bgr_guardan_bytes_en_distinto_orden() {
        let rojo = Color::hex(0xff0000);
        for (format, esperado) in [
            (PixelFormat::Rgb, [255, 0, 0, 0]),
            (PixelFormat::Bgr, [0, 0, 255, 0]),
        ] {
            let mut buf = vec![0u8; 4];
            let mut canvas = Canvas::new(&mut buf, 1, 1, 1, 4, format).unwrap();
            canvas.put(0, 0, rojo);
            assert_eq!(canvas.get(0, 0), Some(rojo));
            assert_eq!(buf, esperado);
        }
    }

    #[test]
    fn respeta_el_stride_y_recorta_fuera_de_pantalla() {
        // 2x2 píxeles, pero cada fila ocupa 3 píxeles en memoria.
        let mut buf = vec![0u8; 3 * 2 * 4];
        let mut canvas = Canvas::new(&mut buf, 2, 2, 3, 4, PixelFormat::Rgb).unwrap();
        canvas.put(0, 1, Color::WHITE);
        canvas.put(5, 5, Color::WHITE);
        canvas.put(-1, 0, Color::WHITE);
        assert_eq!(buf[3 * 4], 255);
        assert_eq!(buf.iter().filter(|b| **b == 255).count(), 3);
    }

    #[test]
    fn rechaza_buffers_chicos() {
        let mut buf = vec![0u8; 10];
        assert!(Canvas::new(&mut buf, 4, 4, 4, 4, PixelFormat::Rgb).is_none());
    }
}
