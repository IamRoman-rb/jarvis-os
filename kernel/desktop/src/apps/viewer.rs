//! Visor de imágenes (BMP, PNG y JPEG) y de videos (AVI con MJPEG, K12): muestra la imagen
//! ajustada a la ventana, sin deformarla.
//!
//! Un video manda con su **audio**: el cuadro que se muestra es el que corresponde a lo que ya
//! sonó en los parlantes (`Sound::position_ms`), así imagen y sonido no se separan aunque un
//! cuadro tarde en dibujarse. Sin audio (o sin placa de sonido), el reloj es el del sistema.
//! Cada cuadro es un JPEG: se decodifica solo el que toca mostrar (si la CPU no llega, se saltean
//! cuadros, no se atrasa).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

use jarvis_audio::avi::{Avi, AviAudio};
use jarvis_audio::mixer::Kind;
use jarvis_audio::resample::Resampled;

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
    video: Option<Video>,
}

struct Video {
    file: Arc<Vec<u8>>,
    avi: Avi,
    /// El audio en el mezclador (si tiene y hay parlantes).
    track: Option<u32>,
    /// Cuándo empezó (reloj del sistema), para los videos sin audio.
    start: u64,
    /// El cuadro que se está mostrando.
    shown: Option<usize>,
    /// Pausado en este momento (ms del video).
    paused_at: Option<u64>,
    ended: bool,
}

impl Viewer {
    pub fn open<D: BlockDevice>(path: &str, ctx: &mut Ctx<'_, D>) -> Self {
        if path.to_ascii_lowercase().ends_with(".avi") {
            return Self::open_video(path, ctx);
        }
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
            video: None,
        }
    }

    fn open_video<D: BlockDevice>(path: &str, ctx: &mut Ctx<'_, D>) -> Self {
        let mut v = Viewer {
            dirty: true,
            path: path.into(),
            image: Err(tr("Cargando el video...").into()),
            video: None,
        };
        let bytes = match ctx.fs.as_deref_mut().map(|fs| fs.read_file(path)) {
            Some(Ok(b)) => b,
            Some(Err(e)) => {
                v.image = Err(crate::files::error_message(e));
                return v;
            }
            None => {
                v.image = Err(tr("No hay disco.").into());
                return v;
            }
        };
        let avi = match Avi::parse(&bytes) {
            Ok(a) => a,
            Err(e) => {
                ctx.log.push(format!("VIDEO_ERROR {path}: {e}"));
                v.image = Err(e.to_string());
                return v;
            }
        };
        let file = Arc::new(bytes);
        let track = match AviAudio::new(file.clone(), &avi) {
            Some(audio) if ctx.audio.available() => {
                let src = Resampled::new(audio, ctx.audio.rate());
                ctx.audio.play(Box::new(src), Kind::Video)
            }
            _ => None,
        };
        ctx.log.push(format!(
            "VIDEO_ABIERTO {path} {}x{}, {} cuadros, {} ms{}",
            avi.width,
            avi.height,
            avi.frames.len(),
            avi.duration_ms(),
            if track.is_some() { ", con audio" } else { "" }
        ));
        v.video = Some(Video {
            file,
            avi,
            track,
            start: ctx.now_ms,
            shown: None,
            paused_at: None,
            ended: false,
        });
        v
    }

    /// El momento del video (ms): lo que sonó del audio, o el reloj si no tiene.
    fn video_ms<D: BlockDevice>(v: &Video, ctx: &Ctx<'_, D>) -> Option<u64> {
        if let Some(ms) = v.paused_at {
            return Some(ms);
        }
        match v.track {
            // El audio terminó: el video también (o está por terminar).
            Some(t) => ctx.audio.position_ms(t),
            None => Some(ctx.now_ms.saturating_sub(v.start)),
        }
    }

    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some(v) = self.video.as_mut() else {
            return;
        };
        if v.ended {
            return;
        }
        let ms = Self::video_ms(v, ctx);
        let finished = match ms {
            None => true,
            Some(ms) => ms >= v.avi.duration_ms(),
        };
        let frame = match ms {
            Some(ms) if !finished => v.avi.frame_at(ms),
            _ => v.avi.frames.len() - 1,
        };
        if v.shown != Some(frame) {
            let (at, len) = v.avi.frames[frame];
            match bmp::decode_any(&v.file[at..at + len], bmp::DISK_MAX_SIDE) {
                Some(img) => self.image = Ok(img),
                None => ctx.log.push(format!("VIDEO_CUADRO_DANADO {frame}")),
            }
            if v.shown.is_none() {
                ctx.log.push("VIDEO_PRIMER_CUADRO".into());
            }
            v.shown = Some(frame);
            self.dirty = true;
        }
        if finished {
            v.ended = true;
            if let Some(t) = v.track.take() {
                ctx.audio.stop(t);
            }
            ctx.log.push(format!("VIDEO_FIN {}", self.path));
            self.dirty = true;
        }
    }

    /// Se cierra la ventana: se corta el audio.
    pub fn close<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(t) = self.video.as_mut().and_then(|v| v.track.take()) {
            ctx.audio.stop(t);
        }
    }

    pub fn title(&self) -> String {
        let name = self.path.rsplit('/').next().unwrap_or(&self.path);
        if let Some(v) = &self.video {
            return format!("{name} · {}×{}", v.avi.width, v.avi.height);
        }
        match &self.image {
            Ok(img) => format!("{name} · {}×{}", img.width, img.height),
            Err(_) => name.into(),
        }
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, ctx: &mut Ctx<'_, D>) -> bool {
        // Espacio: pausa o sigue el video.
        let Some(v) = self.video.as_mut() else {
            return false;
        };
        if key != Key::Char(' ') || v.ended {
            return false;
        }
        match v.paused_at.take() {
            Some(ms) => {
                if let Some(t) = v.track {
                    ctx.audio.pause(t, false);
                } else {
                    v.start = ctx.now_ms.saturating_sub(ms);
                }
                ctx.log.push("VIDEO_SIGUE".into());
            }
            None => {
                v.paused_at = Self::video_ms(v, ctx);
                if let Some(t) = v.track {
                    ctx.audio.pause(t, true);
                }
                ctx.log.push("VIDEO_PAUSA".into());
            }
        }
        self.dirty = true;
        true
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
        let info = match &self.video {
            Some(v) => {
                let at = v.shown.unwrap_or(0) as u64 * v.avi.frame_us as u64 / 1000;
                let clock = |ms: u64| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60);
                let state = if v.ended {
                    tr("terminó")
                } else if v.paused_at.is_some() {
                    tr("en pausa (espacio sigue)")
                } else {
                    tr("espacio: pausa")
                };
                format!(
                    "{} · {} / {} · {}",
                    self.path,
                    clock(at),
                    clock(v.avi.duration_ms()),
                    state
                )
            }
            None => format!("{} · {} %", self.path, scale * 100 / 1024),
        };
        text::draw(
            c,
            r.x + 12,
            r.y + r.h - 26,
            &info,
            &light(theme::text_dim()),
        );
    }
}
