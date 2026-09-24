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

/// Rectángulo en píxeles (esquina superior izquierda + tamaño).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub const fn intersects(&self, other: &Rect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    /// Recorta el rectángulo a una pantalla de `width` × `height`.
    pub fn clamp(&self, width: usize, height: usize) -> Rect {
        let x0 = self.x.clamp(0, width as i32);
        let y0 = self.y.clamp(0, height as i32);
        let x1 = (self.x + self.w).clamp(0, width as i32);
        let y1 = (self.y + self.h).clamp(0, height as i32);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// Orden de los bytes de cada píxel en memoria.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb,
    Bgr,
    Gray,
}

/// Máximo de rectángulos en la zona de recorte (alcanza para los 3 de cada frame).
pub const MAX_CLIP: usize = 4;

/// Superficie de dibujo. `stride` es el ancho real de cada fila en píxeles (puede ser mayor que
/// `width`: el hardware a veces agrega relleno al final de cada fila).
pub struct Canvas<'a> {
    buf: &'a mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    format: PixelFormat,
    /// Si hay recorte, solo se pinta dentro de estos rectángulos (ver [`Canvas::set_clip`]).
    clip: [Option<Rect>; MAX_CLIP],
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
            clip: [None; MAX_CLIP],
        })
    }

    /// Limita el dibujo a la unión de `rects` (hasta [`MAX_CLIP`]). Cada píxel se pinta una sola
    /// vez aunque los rectángulos se superpongan, porque el recorte se chequea por píxel.
    pub fn set_clip(&mut self, rects: impl IntoIterator<Item = Rect>) {
        self.clip = [None; MAX_CLIP];
        for (slot, r) in self.clip.iter_mut().zip(rects) {
            *slot = Some(r);
        }
    }

    /// Vuelve a permitir dibujar en toda la superficie.
    pub fn clear_clip(&mut self) {
        self.clip = [None; MAX_CLIP];
    }

    fn clipped_out(&self, x: i32, y: i32) -> bool {
        self.clip[0].is_some() && !self.clip.iter().flatten().any(|r| r.contains(x, y))
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
        if self.clipped_out(x, y) {
            return;
        }
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

    /// ¿Los dos canvas tienen la misma geometría en memoria? (requisito de [`copy_from`](Self::copy_from))
    pub fn same_layout(&self, other: &Canvas<'_>) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.stride == other.stride
            && self.bytes_per_pixel == other.bytes_per_pixel
            && self.format == other.format
    }

    /// Copia el rectángulo `r` de `src` a este canvas, fila por fila (`copy_from_slice` compila a
    /// un `memcpy`). Es la base del doble buffer. Si las geometrías no coinciden, no hace nada.
    pub fn copy_from(&mut self, src: &Canvas<'_>, r: Rect) -> bool {
        if !self.same_layout(src) {
            return false;
        }
        let r = r.clamp(self.width, self.height);
        if r.is_empty() {
            return true;
        }
        let bpp = self.bytes_per_pixel;
        let len = r.w as usize * bpp;
        for y in r.y as usize..(r.y + r.h) as usize {
            let start = (y * self.stride + r.x as usize) * bpp;
            self.buf[start..start + len].copy_from_slice(&src.buf[start..start + len]);
        }
        true
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
    fn copy_from_copia_solo_el_rectangulo() {
        let mut a = vec![0u8; 8 * 8 * 4];
        let mut b = vec![0u8; 8 * 8 * 4];
        let mut src = Canvas::new(&mut a, 8, 8, 8, 4, PixelFormat::Bgr).unwrap();
        src.fill(Color::WHITE);
        let mut dst = Canvas::new(&mut b, 8, 8, 8, 4, PixelFormat::Bgr).unwrap();
        // Rectángulo que se sale por la izquierda: se recorta.
        assert!(dst.copy_from(&src, Rect::new(-2, 2, 5, 3)));
        for y in 0..8 {
            for x in 0..8 {
                let dentro = (0..3).contains(&x) && (2..5).contains(&y);
                let esperado = if dentro { Color::WHITE } else { Color::BLACK };
                assert_eq!(dst.get(x, y), Some(esperado), "({x}, {y})");
            }
        }
    }

    #[test]
    fn copy_from_rechaza_geometrias_distintas() {
        let mut a = vec![0u8; 8 * 8 * 4];
        let mut b = vec![0u8; 8 * 8 * 4];
        let src = Canvas::new(&mut a, 8, 8, 8, 4, PixelFormat::Rgb).unwrap();
        let mut dst = Canvas::new(&mut b, 8, 8, 8, 4, PixelFormat::Bgr).unwrap();
        assert!(!dst.copy_from(&src, Rect::new(0, 0, 8, 8)));
    }

    #[test]
    fn el_recorte_limita_donde_se_pinta() {
        let mut buf = vec![0u8; 10 * 10 * 4];
        let mut c = Canvas::new(&mut buf, 10, 10, 10, 4, PixelFormat::Rgb).unwrap();
        c.set_clip([Rect::new(0, 0, 3, 3), Rect::new(2, 2, 3, 3)]);
        c.fill(Color::WHITE);
        assert_eq!(c.get(1, 1), Some(Color::WHITE));
        assert_eq!(c.get(4, 4), Some(Color::WHITE));
        assert_eq!(c.get(0, 4), Some(Color::BLACK));
        c.clear_clip();
        c.put(9, 9, Color::WHITE);
        assert_eq!(c.get(9, 9), Some(Color::WHITE));
    }

    #[test]
    fn interseccion_de_rectangulos() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.intersects(&Rect::new(9, 9, 5, 5)));
        assert!(!r.intersects(&Rect::new(10, 0, 5, 5)));
        assert!(!r.intersects(&Rect::new(2, 2, 0, 5)));
    }

    #[test]
    fn rechaza_buffers_chicos() {
        let mut buf = vec![0u8; 10];
        assert!(Canvas::new(&mut buf, 4, 4, 4, 4, PixelFormat::Rgb).is_none());
    }
}
