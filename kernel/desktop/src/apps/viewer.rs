//! Visor de imágenes (BMP): muestra la imagen ajustada a la ventana, sin deformarla.

use alloc::format;
use alloc::string::String;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::Ctx;
use crate::bmp::{self, Image};
use crate::i18n::tr;
use crate::input::Key;
use crate::widgets::{light, s16, window_bg};

pub struct Viewer {
    pub dirty: bool,
    pub path: String,
    image: Result<Image, String>,
}

impl Viewer {
    pub fn open<D: BlockDevice>(path: &str, ctx: &mut Ctx<'_, D>) -> Self {
        let image = match ctx.fs.as_deref_mut() {
            None => Err(tr("No hay disco.").into()),
            Some(fs) => match fs.read_file(path) {
                Ok(bytes) => bmp::decode_any(&bytes, bmp::DISK_MAX_SIDE).ok_or_else(|| {
                    String::from(tr("Solo puedo mostrar imágenes BMP, PNG y JPEG."))
                }),
                Err(e) => Err(crate::files::error_message(e)),
            },
        };
        ctx.log.push(format!("VISOR_ABIERTO {path}"));
        match &image {
            Ok(img) => ctx
                .log
                .push(format!("VISOR_IMAGEN {}x{}", img.width, img.height)),
            Err(e) => ctx.log.push(format!("VISOR_ERROR {e}")),
        }
        Viewer {
            dirty: true,
            path: path.into(),
            image,
        }
    }

    pub fn title(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        match &self.image {
            Ok(img) => format!("{name} · {}×{}", img.width, img.height),
            Err(_) => name.into(),
        }
    }

    pub fn key<D: BlockDevice>(&mut self, _key: Key, _ctx: &mut Ctx<'_, D>) -> bool {
        false
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect) {
        c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
        let img = match &self.image {
            Ok(img) => img,
            Err(msg) => {
                text::draw(c, r.x + 20, r.y + 20, msg, &s16(theme::amber()));
                return;
            }
        };
        // Escala para que entre entera (y sin agrandar más de ×4).
        let area = Rect::new(r.x + 8, r.y + 8, r.w - 16, r.h - 40);
        let sx = area.w as i64 * 1024 / img.width as i64;
        let sy = area.h as i64 * 1024 / img.height as i64;
        let scale = sx.min(sy).min(4096);
        let (w, h) = (
            (img.width as i64 * scale / 1024) as i32,
            (img.height as i64 * scale / 1024) as i32,
        );
        let (x0, y0) = (area.x + (area.w - w) / 2, area.y + (area.h - h) / 2);
        for y in 0..h {
            let src_y = (y as i64 * img.height as i64 / h.max(1) as i64) as usize;
            for x in 0..w {
                let src_x = (x as i64 * img.width as i64 / w.max(1) as i64) as usize;
                c.put(x0 + x, y0 + y, img.pixels[src_y * img.width + src_x]);
            }
        }
        let info = format!("{} · {} %", self.path, scale * 100 / 1024);
        text::draw(
            c,
            r.x + 12,
            r.y + r.h - 26,
            &info,
            &light(theme::text_dim()),
        );
    }
}
