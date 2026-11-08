//! Cámara (ADR 0022): la webcam en vivo, fotos y videos.
//!
//! La imagen llega del **anfitrión**: mientras esta ventana está abierta, el cerebro lee la
//! cámara de la PC (OpenCV) y manda cuadros JPEG chicos (`camara_cuadro`); al cerrarla, la
//! apaga. Nada se guarda en el anfitrión. En una PC real sin anfitrión, la fuente va a ser el
//! driver UVC del kernel (`drivers/src/uvc.rs`).
//!
//! - **Espacio** o **F**: foto → `/Imágenes/foto-AAAAMMDD-HHMMSS.jpg` (el JPEG tal cual llegó).
//! - **V**: empezar y terminar un video → `/Videos/video-AAAAMMDD-HHMMSS.avi` (AVI con MJPEG: los
//!   cuadros ya son JPEG, no hay que recomprimir nada).
//! - Mientras la cámara está prendida se ve un punto rojo "EN VIVO" (y "REC" al grabar).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Color, Rect, theme};

use super::Ctx;
use crate::bmp::{self, Image};
use crate::i18n::{tr, trf};
use crate::input::Key;
use crate::widgets::{light, s16, window_bg};

/// Lo más largo que se graba (a ~10 cuadros por segundo, unos 30 MB).
const MAX_VIDEO_FRAMES: usize = 1800;
/// Cuadros por segundo con los que se anota el video (los que manda el anfitrión).
pub const VIDEO_FPS: u32 = 10;

pub struct Camera {
    pub dirty: bool,
    frame: Option<Image>,
    /// El último JPEG (para la foto).
    jpeg: Vec<u8>,
    frames_seen: u64,
    recording: Option<Vec<Vec<u8>>>,
    message: String,
    /// El anfitrión avisa si no hay cámara (o si faltan las bibliotecas).
    pub status: String,
}

impl Camera {
    pub fn new<D: BlockDevice>(ctx: &mut Ctx<'_, D>) -> Camera {
        ctx.out.brain.push(crate::brain::BrainOp::CameraView(true));
        ctx.log.push("CAMARA_ABIERTA".into());
        Camera {
            dirty: true,
            frame: None,
            jpeg: Vec::new(),
            frames_seen: 0,
            recording: None,
            message: String::new(),
            status: String::new(),
        }
    }

    pub fn title(&self) -> String {
        tr("Cámara").into()
    }

    /// Cuántos cuadros llegaron desde que se abrió.
    pub fn frames(&self) -> u64 {
        self.frames_seen
    }

    /// Llegó un cuadro del anfitrión.
    pub fn frame(&mut self, jpeg: Vec<u8>) {
        if let Some(img) = bmp::decode_any(&jpeg, 1280) {
            self.frame = Some(img);
            self.frames_seen += 1;
            if let Some(rec) = &mut self.recording
                && rec.len() < MAX_VIDEO_FRAMES
            {
                rec.push(jpeg.clone());
            }
            self.jpeg = jpeg;
            self.dirty = true;
        }
    }

    fn stamp<D: BlockDevice>(ctx: &Ctx<'_, D>) -> String {
        let t = ctx.timestamp();
        format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            t.year, t.month, t.day, t.hour, t.minute, t.second
        )
    }

    fn save<D: BlockDevice>(
        ctx: &mut Ctx<'_, D>,
        dir: &str,
        path: &str,
        data: &[u8],
    ) -> Result<(), String> {
        let now = ctx.timestamp();
        let fs = ctx
            .fs
            .as_deref_mut()
            .ok_or_else(|| String::from(tr("No hay disco.")))?;
        crate::term::apt::ensure_dirs(fs, dir, now)
            .and_then(|()| fs.write_file(path, data, now))
            .map_err(|e| format!("{e}"))
    }

    pub fn photo<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if self.jpeg.is_empty() {
            self.message = tr("Todavía no llegó imagen de la cámara.").into();
            self.dirty = true;
            return;
        }
        let path = format!("/Imágenes/foto-{}.jpg", Self::stamp(ctx));
        let jpeg = self.jpeg.clone();
        self.message = match Self::save(ctx, "/Imágenes", &path, &jpeg) {
            Ok(()) => {
                ctx.log.push(format!("CAMARA_FOTO {path}"));
                trf("Foto guardada en {}", &[&path])
            }
            Err(e) => trf("No se pudo guardar la foto: {}", &[&e]),
        };
        self.dirty = true;
    }

    pub fn toggle_video<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        match self.recording.take() {
            None => {
                self.recording = Some(Vec::new());
                ctx.log.push("CAMARA_GRABANDO".into());
                self.message = tr("Grabando: V termina.").into();
            }
            Some(frames) if frames.is_empty() => {
                self.message = tr("No llegó ningún cuadro: no se guardó el video.").into();
            }
            Some(frames) => {
                let (w, h) = self
                    .frame
                    .as_ref()
                    .map_or((320, 240), |f| (f.width as u32, f.height as u32));
                let avi = jarvis_audio::avi::write(w, h, VIDEO_FPS, &frames, None);
                let path = format!("/Videos/video-{}.avi", Self::stamp(ctx));
                self.message = match Self::save(ctx, "/Videos", &path, &avi) {
                    Ok(()) => {
                        ctx.log
                            .push(format!("CAMARA_VIDEO {path} {}", frames.len()));
                        trf("Video guardado en {}", &[&path])
                    }
                    Err(e) => trf("No se pudo guardar el video: {}", &[&e]),
                };
            }
        }
        self.dirty = true;
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, ctx: &mut Ctx<'_, D>) -> bool {
        match key {
            Key::Char(' ' | 'f' | 'F') | Key::Enter => self.photo(ctx),
            Key::Char('v' | 'V') => self.toggle_video(ctx),
            _ => return false,
        }
        true
    }

    /// Se cierra la ventana: la cámara del anfitrión se apaga (y un video a medias se guarda).
    pub fn close<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if self.recording.is_some() {
            self.toggle_video(ctx);
        }
        ctx.out.brain.push(crate::brain::BrainOp::CameraView(false));
        ctx.log.push("CAMARA_CERRADA".into());
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect) {
        c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
        let area = Rect::new(r.x + 8, r.y + 8, r.w - 16, r.h - 60);
        match &self.frame {
            Some(img) if img.width > 0 && img.height > 0 => {
                let sx = area.w as i64 * 1024 / img.width as i64;
                let sy = area.h as i64 * 1024 / img.height as i64;
                let scale = sx.min(sy).min(4096);
                let (w, h) = (
                    (img.width as i64 * scale / 1024) as i32,
                    (img.height as i64 * scale / 1024) as i32,
                );
                let (x0, y0) = (area.x + (area.w - w) / 2, area.y + (area.h - h) / 2);
                for y in 0..h {
                    let sy = (y as i64 * img.height as i64 / h.max(1) as i64) as usize;
                    for x in 0..w {
                        let sx = (x as i64 * img.width as i64 / w.max(1) as i64) as usize;
                        c.put(x0 + x, y0 + y, img.pixels[sy * img.width + sx]);
                    }
                }
                // Indicador: la cámara está prendida (y grabando).
                let red = Color {
                    r: 230,
                    g: 40,
                    b: 50,
                };
                jarvis_gfx::shapes::circle(c, x0 + 16, y0 + 16, 6, red, true);
                let label = if self.recording.is_some() {
                    tr("REC")
                } else {
                    tr("EN VIVO")
                };
                text::draw(c, x0 + 28, y0 + 8, label, &s16(red));
            }
            _ => {
                let msg = if self.status.is_empty() {
                    tr("Esperando la imagen de la cámara de la PC...")
                } else {
                    self.status.as_str()
                };
                text::draw(c, area.x + 16, area.y + 16, msg, &s16(theme::amber()));
            }
        }
        let hint = if self.message.is_empty() {
            tr("Espacio: foto · V: grabar video · Esc cierra")
        } else {
            self.message.as_str()
        };
        text::draw(c, r.x + 12, r.y + r.h - 40, hint, &light(theme::text_dim()));
    }
}
