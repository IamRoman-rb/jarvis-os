//! El mezclador: lo que suena a la vez (una canción, la voz de JARVIS, el audio de un video) se
//! suma en un solo flujo estéreo para la placa.
//!
//! Sumar dos señales de 16 bits puede pasarse de 16 bits: se suma en 32 y se recorta
//! ("clipping"). El volumen es un porcentaje (multiplicar y dividir por 100).
//!
//! La esfera de JARVIS se mueve con **lo que está sonando**: el mezclador anota el nivel de la
//! voz de cada bloque de 20 ms junto con el cuadro en que empieza, y el escritorio pregunta por
//! el nivel del cuadro que la placa está reproduciendo ahora (lo que se mezcla va un poco
//! adelantado: la placa tiene buffers).

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use crate::resample::Source;

/// Qué es cada cosa que suena (la esfera sigue solo a la voz).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Music,
    Voice,
    Video,
}

struct Track {
    id: u32,
    kind: Kind,
    src: Box<dyn Source>,
    volume: u32,
    paused: bool,
    /// Cuadros que produjo (su posición, a la frecuencia de salida).
    played: u64,
    /// Terminó y ya se avisó.
    finished: bool,
}

pub struct Mixer {
    rate: u32,
    tracks: Vec<Track>,
    next_id: u32,
    /// Volumen general (0..=100).
    pub master: u32,
    /// Cuadros mezclados desde el principio.
    mixed: u64,
    /// (cuadro donde empieza el bloque, nivel de la voz 0..=100).
    voice_marks: VecDeque<(u64, u8)>,
    scratch: Vec<i16>,
    ended: Vec<u32>,
}

/// Largo de los bloques en los que se mide la voz (20 ms a 48 kHz).
const MARK_FRAMES: usize = 960;

impl Mixer {
    pub fn new(rate: u32) -> Mixer {
        Mixer {
            rate,
            tracks: Vec::new(),
            next_id: 0,
            master: 100,
            mixed: 0,
            voice_marks: VecDeque::new(),
            scratch: Vec::new(),
            ended: Vec::new(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Empieza a sonar algo. Devuelve su número.
    pub fn play(&mut self, src: Box<dyn Source>, kind: Kind) -> u32 {
        self.next_id += 1;
        self.tracks.push(Track {
            id: self.next_id,
            kind,
            src,
            volume: 100,
            paused: false,
            played: 0,
            finished: false,
        });
        self.next_id
    }

    fn track(&mut self, id: u32) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn stop(&mut self, id: u32) {
        self.tracks.retain(|t| t.id != id);
    }

    /// Corta todo lo de un tipo (Roman interrumpió a JARVIS: se calla la voz).
    pub fn stop_kind(&mut self, kind: Kind) {
        self.tracks.retain(|t| t.kind != kind);
    }

    pub fn pause(&mut self, id: u32, paused: bool) {
        if let Some(t) = self.track(id) {
            t.paused = paused;
        }
    }

    pub fn set_volume(&mut self, id: u32, volume: u32) {
        if let Some(t) = self.track(id) {
            t.volume = volume.min(100);
        }
    }

    /// Muestras nuevas para una transmisión (la voz).
    pub fn feed(&mut self, id: u32, pcm: &[i16], end: bool) {
        if let Some(t) = self.track(id) {
            t.src.feed(pcm, end);
        }
    }

    pub fn playing(&self, id: u32) -> bool {
        self.tracks.iter().any(|t| t.id == id)
    }

    /// Cuadros que ya se mezclaron de `id` (su posición, a la frecuencia de salida).
    pub fn position(&self, id: u32) -> Option<u64> {
        self.tracks.iter().find(|t| t.id == id).map(|t| t.played)
    }

    pub fn is_playing_kind(&self, kind: Kind) -> bool {
        self.tracks.iter().any(|t| t.kind == kind && !t.paused)
    }

    /// Los que terminaron desde la última vez (para avisarle a su app).
    pub fn take_ended(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.ended)
    }

    /// Cuadros mezclados en total (el "reloj" del audio que se entregó a la placa).
    pub fn mixed(&self) -> u64 {
        self.mixed
    }

    /// Mezcla `out.len() / 2` cuadros estéreo.
    pub fn render(&mut self, out: &mut [i16]) {
        let frames = out.len() / 2;
        let mut acc = vec![0i32; frames * 2];
        let mut voice = vec![0i32; frames * 2];
        if self.scratch.len() < frames * 2 {
            self.scratch.resize(frames * 2, 0);
        }
        for t in &mut self.tracks {
            if t.paused {
                continue;
            }
            let buf = &mut self.scratch[..frames * 2];
            let n = t.src.render(buf);
            t.played += n as u64;
            let target = if t.kind == Kind::Voice {
                &mut voice
            } else {
                &mut acc
            };
            for (a, &s) in target.iter_mut().zip(&buf[..n * 2]) {
                *a += s as i32 * t.volume as i32 / 100;
            }
            if t.src.done() {
                t.finished = true;
                self.ended.push(t.id);
            }
        }
        self.tracks.retain(|t| !t.finished);
        // El nivel de la voz, por bloques, con el cuadro en que empiezan.
        for (i, block) in voice.chunks(MARK_FRAMES * 2).enumerate() {
            let s: Vec<i16> = block
                .iter()
                .map(|&v| v.clamp(-32768, 32767) as i16)
                .collect();
            let start = self.mixed + (i * MARK_FRAMES) as u64;
            self.voice_marks
                .push_back((start, crate::level_samples(&s)));
        }
        // Se recuerda ~2 s: más que los buffers de la placa.
        while self.voice_marks.len() > 100 {
            self.voice_marks.pop_front();
        }
        for (o, (a, v)) in out.iter_mut().zip(acc.iter().zip(&voice)) {
            *o = ((a + v) * self.master as i32 / 100).clamp(-32768, 32767) as i16;
        }
        self.mixed += frames as u64;
    }

    /// El nivel de la voz en el cuadro `frame` (el que suena ahora en la placa).
    pub fn voice_level_at(&self, frame: u64) -> u8 {
        self.voice_marks
            .iter()
            .rev()
            .find(|(start, _)| *start <= frame)
            .map_or(0, |&(_, l)| l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resample::{Resampled, Stream};

    /// Un tono constante de `n` cuadros.
    struct Dc {
        value: i16,
        left: usize,
    }

    impl Source for Dc {
        fn render(&mut self, out: &mut [i16]) -> usize {
            let n = (out.len() / 2).min(self.left);
            out[..2 * n].fill(self.value);
            self.left -= n;
            n
        }
        fn done(&self) -> bool {
            self.left == 0
        }
    }

    #[test]
    fn suma_recorta_volumen_y_pausa() {
        let mut m = Mixer::new(48_000);
        let a = m.play(
            Box::new(Dc {
                value: 20_000,
                left: 1000,
            }),
            Kind::Music,
        );
        let b = m.play(
            Box::new(Dc {
                value: 20_000,
                left: 100,
            }),
            Kind::Video,
        );
        let mut out = [0i16; 2 * 100];
        m.render(&mut out);
        assert!(out.iter().all(|&s| s == 32767), "recortado");
        assert_eq!(m.take_ended(), [b]);
        m.set_volume(a, 50);
        m.render(&mut out);
        assert!(out.iter().all(|&s| s == 10_000));
        m.pause(a, true);
        m.render(&mut out);
        assert!(out.iter().all(|&s| s == 0));
        assert_eq!(m.position(a), Some(200));
        m.pause(a, false);
        m.master = 50;
        m.render(&mut out);
        assert!(out.iter().all(|&s| s == 5_000));
        m.stop(a);
        assert!(!m.playing(a));
        assert_eq!(m.mixed(), 400);
    }

    #[test]
    fn la_esfera_sigue_a_la_voz_que_suena() {
        let mut m = Mixer::new(48_000);
        let _music = m.play(
            Box::new(Dc {
                value: 30_000,
                left: 1 << 20,
            }),
            Kind::Music,
        );
        let v = m.play(
            Box::new(Resampled::new(Stream::new(48_000, 1), 48_000)),
            Kind::Voice,
        );
        let mut out = vec![0i16; 2 * MARK_FRAMES];
        // Sin voz todavía: la música no mueve la esfera.
        m.render(&mut out);
        assert_eq!(m.voice_level_at(0), 0);
        // Llega voz fuerte.
        m.feed(v, &vec![12_000i16; MARK_FRAMES * 2], false);
        m.render(&mut out);
        let loud = m.voice_level_at(MARK_FRAMES as u64 + 10);
        assert!(loud > 50, "{loud}");
        // Un bloque antes (lo que todavía suena en la placa) seguía en silencio.
        assert_eq!(m.voice_level_at(MARK_FRAMES as u64 - 1), 0);
        // Se corta la voz (Roman interrumpió).
        m.stop_kind(Kind::Voice);
        m.render(&mut out);
        assert_eq!(m.voice_level_at(2 * MARK_FRAMES as u64), 0);
        assert!(m.is_playing_kind(Kind::Music));
    }
}
