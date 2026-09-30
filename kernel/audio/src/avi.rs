//! AVI: el contenedor de video RIFF de Microsoft (1992), con cuadros **MJPEG** (cada cuadro es
//! un JPEG entero: los decodifica `jarvis-image`) y audio PCM o IMA ADPCM.
//!
//! ```text
//! RIFF 'AVI '
//!   LIST 'hdrl'
//!     'avih'  microsegundos por cuadro, cantidad de cuadros, ancho, alto…
//!     LIST 'strl'  'strh' (tipo: 'vids' o 'auds', escala y tasa)  'strf' (formato)
//!     LIST 'strl'  …un flujo por cada lista
//!   LIST 'movi'
//!     '00dc' un cuadro del flujo 0 ("dc": video comprimido)
//!     '01wb' audio del flujo 1 ("wb": wave bytes)
//!     …intercalados, a veces agrupados en LIST 'rec '
//!   'idx1'   un índice (se ignora: se recorre 'movi', que es la verdad)
//! ```
//!
//! MJPEG es el formato de video más simple que existe: no hay cuadros que dependan de otros
//! (como en H.264), así que cualquier cuadro se puede mostrar solo. Ocupa más, pero para eso no
//! hace falta un decodificador de video de verdad.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::resample::Decoder;
use crate::wav::Format;
use crate::{Error, u32_at};

#[derive(Clone, Debug)]
pub struct Avi {
    pub width: u32,
    pub height: u32,
    /// Duración de un cuadro, en microsegundos.
    pub frame_us: u32,
    /// Dónde está cada cuadro (desplazamiento y largo, en el archivo).
    pub frames: Vec<(usize, usize)>,
    /// El audio: su formato y sus pedazos, en orden.
    pub audio: Option<(Format, Vec<(usize, usize)>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Video,
    Audio,
    Other,
}

impl Avi {
    pub fn parse(file: &[u8]) -> Result<Avi, Error> {
        if file.get(0..4) != Some(b"RIFF") || file.get(8..12) != Some(b"AVI ") {
            return Err(Error::Invalid("no es un AVI"));
        }
        let mut avi = Avi {
            width: 0,
            height: 0,
            frame_us: 0,
            frames: Vec::new(),
            audio: None,
        };
        let mut streams: Vec<Stream> = Vec::new();
        let mut movi = None;
        walk(file, 12, file.len(), &mut |id, list, body, end| {
            match (id, list) {
                (b"LIST", Some(b"movi")) => {
                    movi = Some((body, end));
                    return Ok(false); // se recorre después, ya sabiendo los flujos
                }
                (b"avih", _) => {
                    avi.frame_us = u32_at(file, body)?;
                    avi.width = u32_at(file, body + 32)?;
                    avi.height = u32_at(file, body + 36)?;
                }
                (b"strh", _) => {
                    let kind = match file.get(body..body + 4) {
                        Some(b"vids") => Stream::Video,
                        Some(b"auds") => Stream::Audio,
                        _ => Stream::Other,
                    };
                    streams.push(kind);
                    if kind == Stream::Video {
                        let handler = file.get(body + 4..body + 8).unwrap_or(&[]);
                        let scale = u32_at(file, body + 20)?;
                        let rate = u32_at(file, body + 24)?;
                        if rate > 0 && scale > 0 {
                            avi.frame_us = (scale as u64 * 1_000_000 / rate as u64) as u32;
                        }
                        let mjpeg =
                            handler.eq_ignore_ascii_case(b"MJPG") || handler == [0, 0, 0, 0];
                        if !mjpeg {
                            return Err(Error::Unsupported(
                                "video que no es MJPEG (H.264, MPEG-4…)",
                            ));
                        }
                    }
                }
                (b"strf", _) => match streams.last() {
                    Some(Stream::Audio) if avi.audio.is_none() => {
                        let f = Format::parse(&file[body..end])?;
                        avi.audio = Some((f, Vec::new()));
                    }
                    Some(Stream::Video) => {
                        let comp = file.get(body + 16..body + 20).unwrap_or(&[]);
                        if !comp.eq_ignore_ascii_case(b"MJPG") {
                            return Err(Error::Unsupported("video que no es MJPEG"));
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            Ok(true)
        })?;
        let (start, end) = movi.ok_or(Error::Invalid("falta la lista movi"))?;
        let video = streams.iter().position(|&s| s == Stream::Video);
        let audio = streams.iter().position(|&s| s == Stream::Audio);
        walk(file, start, end, &mut |id, _list, body, end| {
            let n = stream_number(id);
            if n.is_some() && n == video && &id[2..] != b"wb" {
                if end > body {
                    avi.frames.push((body, end - body));
                }
            } else if n.is_some()
                && n == audio
                && let Some((_, chunks)) = avi.audio.as_mut()
            {
                chunks.push((body, end - body));
            }
            Ok(true)
        })?;
        if avi.frames.is_empty() {
            return Err(Error::Invalid("no tiene cuadros de video"));
        }
        if avi.frame_us == 0 {
            avi.frame_us = 40_000; // 25 cuadros por segundo si no lo dice
        }
        Ok(avi)
    }

    /// Cuánto dura, en ms.
    pub fn duration_ms(&self) -> u64 {
        self.frames.len() as u64 * self.frame_us as u64 / 1000
    }

    /// El cuadro que corresponde al momento `ms`.
    pub fn frame_at(&self, ms: u64) -> usize {
        ((ms * 1000 / self.frame_us.max(1) as u64) as usize).min(self.frames.len() - 1)
    }
}

/// "01wb" → Some(1).
fn stream_number(id: &[u8]) -> Option<usize> {
    let d = |c: u8| c.is_ascii_digit().then(|| (c - b'0') as usize);
    Some(d(id[0])? * 10 + d(id[1])?)
}

/// Lo que se hace con cada chunk: `(id, tipo de lista, cuerpo, fin)` → ¿entrar a la lista?
type Visitor<'a> = dyn FnMut(&[u8], Option<&[u8]>, usize, usize) -> Result<bool, Error> + 'a;

/// Recorre los chunks de `[start, end)`. `f(id, tipo de lista, cuerpo, fin)`; si devuelve
/// `true` y es una lista, entra a ella.
fn walk(file: &[u8], start: usize, end: usize, f: &mut Visitor<'_>) -> Result<(), Error> {
    let mut at = start;
    while at + 8 <= end {
        let id = &file[at..at + 4];
        let len = u32_at(file, at + 4)? as usize;
        let body = at + 8;
        let chunk_end = body.checked_add(len).ok_or(Error::Truncated)?.min(end);
        if id == b"LIST" || id == b"RIFF" {
            let kind = file.get(body..body + 4).ok_or(Error::Truncated)?;
            if f(id, Some(kind), body + 4, chunk_end)? {
                walk(file, body + 4, chunk_end, f)?;
            }
        } else {
            f(id, None, body, chunk_end)?;
        }
        at = body + len + (len & 1);
    }
    Ok(())
}

/// El audio de un AVI como [`Decoder`] (comparte los bytes del archivo con el video).
pub struct AviAudio {
    file: Arc<Vec<u8>>,
    format: Format,
    chunks: Vec<(usize, usize)>,
    next: usize,
}

impl AviAudio {
    pub fn new(file: Arc<Vec<u8>>, avi: &Avi) -> Option<AviAudio> {
        let (format, chunks) = avi.audio.clone()?;
        Some(AviAudio {
            file,
            format,
            chunks,
            next: 0,
        })
    }
}

impl Decoder for AviAudio {
    fn rate(&self) -> u32 {
        self.format.rate
    }
    fn channels(&self) -> usize {
        self.format.channels
    }
    fn next_chunk(&mut self, out: &mut Vec<i16>) {
        if let Some(&(at, len)) = self.chunks.get(self.next) {
            self.next += 1;
            let _ = self.format.decode(&self.file[at..at + len], out);
        }
    }
}

/// Arma un AVI MJPEG (lo usan los tests; el video de ejemplo lo arma un script igual).
pub fn write(
    width: u32,
    height: u32,
    fps: u32,
    frames: &[Vec<u8>],
    audio: Option<(Format, Vec<Vec<u8>>)>,
) -> Vec<u8> {
    fn chunk(id: &[u8], body: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    }
    fn list(kind: &[u8], parts: &[Vec<u8>]) -> Vec<u8> {
        let mut body = kind.to_vec();
        for p in parts {
            body.extend_from_slice(p);
        }
        chunk(b"LIST", &body)
    }
    let le = |v: u32| v.to_le_bytes();
    let mut avih = Vec::new();
    for v in [
        1_000_000 / fps,
        0,
        0,
        0x10,
        frames.len() as u32,
        0,
        1 + audio.is_some() as u32,
        0,
        width,
        height,
        0,
        0,
        0,
        0,
    ] {
        avih.extend_from_slice(&le(v));
    }
    let mut strh = b"vidsMJPG".to_vec();
    for v in [0, 0, 0, 1, fps, 0, frames.len() as u32, 0, 0, 0] {
        strh.extend_from_slice(&le(v));
    }
    strh.extend_from_slice(&[0; 8]);
    let mut strf = Vec::new();
    for v in [40, width, height] {
        strf.extend_from_slice(&le(v));
    }
    strf.extend_from_slice(&1u16.to_le_bytes());
    strf.extend_from_slice(&24u16.to_le_bytes());
    strf.extend_from_slice(b"MJPG");
    strf.extend_from_slice(&le(width * height * 3));
    strf.extend_from_slice(&[0; 16]);
    let mut hdrl = vec_of(&[
        chunk(b"avih", &avih),
        list(b"strl", &[chunk(b"strh", &strh), chunk(b"strf", &strf)]),
    ]);
    let mut movi = Vec::new();
    let audio_chunks = if let Some((format, chunks)) = &audio {
        let wav = crate::wav::write(*format, &[]);
        let fmt_len = u32::from_le_bytes([wav[16], wav[17], wav[18], wav[19]]) as usize;
        let mut ah = b"auds".to_vec();
        ah.extend_from_slice(&[0; 16]);
        ah.extend_from_slice(&le(1));
        ah.extend_from_slice(&le(format.rate));
        ah.extend_from_slice(&[0; 24]);
        hdrl.push(list(
            b"strl",
            &[chunk(b"strh", &ah), chunk(b"strf", &wav[20..20 + fmt_len])],
        ));
        chunks.clone()
    } else {
        Vec::new()
    };
    for (i, f) in frames.iter().enumerate() {
        movi.push(chunk(b"00dc", f));
        if let Some(a) = audio_chunks.get(i) {
            movi.push(chunk(b"01wb", a));
        }
    }
    for a in audio_chunks.iter().skip(frames.len()) {
        movi.push(chunk(b"01wb", a));
    }
    let mut body = b"AVI ".to_vec();
    body.extend_from_slice(&list(b"hdrl", &hdrl));
    body.extend_from_slice(&list(b"movi", &movi));
    chunk(b"RIFF", &body)
}

fn vec_of(parts: &[Vec<u8>]) -> Vec<Vec<u8>> {
    parts.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::Codec;
    use alloc::vec;

    #[test]
    fn video_con_audio() {
        let frames: Vec<Vec<u8>> = (0..10u8).map(|i| vec![0xFF, 0xD8, i, 0xFF, 0xD9]).collect();
        let fmt = Format {
            codec: Codec::Pcm { bits: 16 },
            channels: 1,
            rate: 8000,
        };
        let audio: Vec<Vec<u8>> = (0..10i16)
            .map(|i| (0..800).flat_map(|_| (i * 100).to_le_bytes()).collect())
            .collect();
        let file = write(64, 48, 10, &frames, Some((fmt, audio)));
        let avi = Avi::parse(&file).unwrap();
        assert_eq!((avi.width, avi.height, avi.frame_us), (64, 48, 100_000));
        assert_eq!(avi.frames.len(), 10);
        let (at, len) = avi.frames[3];
        assert_eq!(&file[at..at + len], &[0xFF, 0xD8, 3, 0xFF, 0xD9]);
        assert_eq!(avi.duration_ms(), 1000);
        assert_eq!(avi.frame_at(250), 2);
        assert_eq!(avi.frame_at(99_999), 9);
        // El audio, pedazo a pedazo.
        let file = Arc::new(file);
        let mut a = AviAudio::new(file, &avi).unwrap();
        assert_eq!((a.rate(), a.channels()), (8000, 1));
        let mut out = Vec::new();
        for _ in 0..11 {
            a.next_chunk(&mut out);
        }
        assert_eq!(out.len(), 8000);
        assert_eq!(out[800 * 7], 700);
    }

    #[test]
    fn sin_audio_y_formatos_que_no() {
        let frames = vec![vec![0xFF, 0xD8, 0xFF, 0xD9]; 3];
        let file = write(8, 8, 25, &frames, None);
        let avi = Avi::parse(&file).unwrap();
        assert!(avi.audio.is_none());
        assert_eq!(avi.frame_us, 40_000);
        // Un video H.264: se dice que no.
        let mut h264 = file.clone();
        let i = h264.windows(4).position(|w| w == b"MJPG").unwrap();
        h264[i..i + 4].copy_from_slice(b"H264");
        assert!(matches!(Avi::parse(&h264), Err(Error::Unsupported(_))));
        assert!(Avi::parse(b"RIFF\0\0\0\0WAVE").is_err());
        for cut in [12, 40, file.len() / 2] {
            let _ = Avi::parse(&file[..cut]);
        }
    }
}
