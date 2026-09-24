//! Imágenes BMP (el formato de mapa de bits de Windows): sin compresión, así que escribirlo y
//! leerlo es copiar píxeles. Lo usan las capturas de pantalla y el visor de imágenes.
//!
//! Estructura: cabecera de archivo (14 bytes) + cabecera de imagen (40 bytes) + filas de
//! píxeles en BGR, **de abajo hacia arriba**, cada fila rellenada hasta un múltiplo de 4 bytes.

use alloc::vec;
use alloc::vec::Vec;

use jarvis_gfx::{Canvas, Color};

pub struct Image {
    pub width: usize,
    pub height: usize,
    /// Fila por fila, de arriba hacia abajo.
    pub pixels: Vec<Color>,
}

impl Image {
    /// Dibuja la imagen estirada a `dst` (vecino más cercano).
    pub fn draw_scaled(&self, c: &mut Canvas<'_>, dst: jarvis_gfx::Rect) {
        if self.width == 0 || self.height == 0 || dst.w <= 0 || dst.h <= 0 {
            return;
        }
        for y in 0..dst.h {
            let sy = (y as i64 * self.height as i64 / dst.h as i64) as usize;
            let row = &self.pixels[sy * self.width..(sy + 1) * self.width];
            for x in 0..dst.w {
                let sx = (x as i64 * self.width as i64 / dst.w as i64) as usize;
                c.put(dst.x + x, dst.y + y, row[sx]);
            }
        }
    }
}

fn row_bytes(width: usize) -> usize {
    (width * 3).div_ceil(4) * 4
}

/// Codifica el canvas como BMP de 24 bits.
pub fn encode(c: &Canvas<'_>) -> Vec<u8> {
    let (w, h) = (c.width(), c.height());
    let row = row_bytes(w);
    let data_len = row * h;
    let mut out = Vec::with_capacity(54 + data_len);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // sin compresión
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes()); // 72 ppp
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let p = c.get(x as i32, y as i32).unwrap_or(Color::BLACK);
            out.extend_from_slice(&[p.b, p.g, p.r]);
        }
        out.resize(start + row, 0);
    }
    out
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Lee un BMP de 24 o 32 bits sin compresión. `None` si es otro formato o está roto.
pub fn decode(b: &[u8]) -> Option<Image> {
    if b.get(0..2)? != b"BM" {
        return None;
    }
    let offset = u32_at(b, 10)? as usize;
    let width = u32_at(b, 18)? as i32;
    let height = u32_at(b, 22)? as i32;
    let bpp = u16_at(b, 28)? as usize;
    let compression = u32_at(b, 30)?;
    if width <= 0
        || height == 0
        || !(bpp == 24 || bpp == 32)
        || !(compression == 0 || compression == 3)
    {
        return None;
    }
    let (w, h) = (width as usize, height.unsigned_abs() as usize);
    if w > 8192 || h > 8192 {
        return None;
    }
    let bytes = bpp / 8;
    let row = (w * bytes).div_ceil(4) * 4;
    let mut pixels = vec![Color::BLACK; w * h];
    for y in 0..h {
        // Alto positivo = filas de abajo hacia arriba.
        let src_y = if height > 0 { h - 1 - y } else { y };
        let start = offset + src_y * row;
        let line = b.get(start..start + w * bytes)?;
        for x in 0..w {
            let p = &line[x * bytes..x * bytes + 3];
            pixels[y * w + x] = Color {
                r: p[2],
                g: p[1],
                b: p[0],
            };
        }
    }
    Some(Image {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_gfx::PixelFormat;

    #[test]
    fn ida_y_vuelta() {
        let (w, h) = (5, 3); // 5 × 3 bytes = 15: cada fila lleva 1 byte de relleno
        let mut buf = vec![0u8; w * h * 4];
        let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Bgr).unwrap();
        c.put(0, 0, Color::hex(0xff0000));
        c.put(4, 2, Color::hex(0x00ff00));
        let bmp = encode(&c);
        assert_eq!(bmp.len(), 54 + 16 * 3);
        let img = decode(&bmp).unwrap();
        assert_eq!((img.width, img.height), (5, 3));
        assert_eq!(img.pixels[0], Color::hex(0xff0000));
        assert_eq!(img.pixels[2 * 5 + 4], Color::hex(0x00ff00));
        assert!(decode(b"PNG....").is_none());
        assert!(decode(&bmp[..60]).is_none(), "truncado");
    }
}
