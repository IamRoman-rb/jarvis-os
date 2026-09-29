//! El sonido del escritorio (K12): el mezclador que alimenta los parlantes de la placa.
//!
//! Las apps (Música, el visor de videos) y la voz de JARVIS agregan lo que quieren que suene;
//! el kernel le pide al escritorio el audio mezclado de a pedazos ([`Sound::render`]) y le
//! cuenta cuánto ya sonó ([`Sound::set_played`]). Con eso:
//!
//! - la esfera se mueve con la voz **que está sonando**, no con la que se mezcló (va ~100 ms
//!   adelantada por los buffers de la placa);
//! - un video sabe en qué momento está (el audio manda: los cuadros lo siguen).
//!
//! Sin placa de sonido, [`Sound::available`] es falso y la app Música vuelve al parlante de la PC.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use jarvis_audio::mixer::{Kind, Mixer};
use jarvis_audio::resample::{Resampled, Source, Stream};
use jarvis_audio::wav::Wav;

#[derive(Default)]
pub struct Sound {
    mixer: Option<Mixer>,
    /// Cuadros que ya sonaron (los cuenta la placa).
    played: u64,
    /// La voz de JARVIS que está llegando del cerebro.
    voice: Option<u32>,
}

impl Sound {
    /// Hay parlantes, a `rate` Hz estéreo (lo dice el kernel al arrancar).
    pub fn enable(&mut self, rate: u32) {
        self.mixer = Some(Mixer::new(rate));
    }

    pub fn available(&self) -> bool {
        self.mixer.is_some()
    }

    pub fn rate(&self) -> u32 {
        self.mixer.as_ref().map_or(48_000, Mixer::rate)
    }

    /// Empieza a sonar `src`. `None` si no hay parlantes.
    pub fn play(&mut self, src: Box<dyn Source>, kind: Kind) -> Option<u32> {
        Some(self.mixer.as_mut()?.play(src, kind))
    }

    /// Un archivo WAV (PCM o IMA ADPCM).
    pub fn play_wav(&mut self, bytes: Vec<u8>, kind: Kind) -> Result<u32, String> {
        let rate = self.rate();
        let wav = Wav::parse(bytes).map_err(|e| e.to_string())?;
        self.play(Box::new(Resampled::new(wav, rate)), kind)
            .ok_or_else(|| "no hay placa de sonido".into())
    }

    pub fn stop(&mut self, id: u32) {
        if let Some(m) = self.mixer.as_mut() {
            m.stop(id);
        }
    }

    pub fn pause(&mut self, id: u32, paused: bool) {
        if let Some(m) = self.mixer.as_mut() {
            m.pause(id, paused);
        }
    }

    pub fn playing(&self, id: u32) -> bool {
        self.mixer.as_ref().is_some_and(|m| m.playing(id))
    }

    /// Hasta dónde **se escuchó** `id`, en ms: lo mezclado menos lo que todavía está en los
    /// buffers de la placa.
    pub fn position_ms(&self, id: u32) -> Option<u64> {
        let m = self.mixer.as_ref()?;
        let pos = m.position(id)?;
        let pending = m.mixed().saturating_sub(self.played);
        Some(pos.saturating_sub(pending) * 1000 / m.rate() as u64)
    }

    /// Lo que terminó de sonar desde la última vez.
    pub fn take_ended(&mut self) -> Vec<u32> {
        let ended = self
            .mixer
            .as_mut()
            .map(Mixer::take_ended)
            .unwrap_or_default();
        if self.voice.is_some_and(|v| ended.contains(&v)) {
            self.voice = None;
        }
        ended
    }

    /// Audio mezclado para la placa: `frames` cuadros estéreo.
    pub fn render(&mut self, frames: usize) -> Vec<i16> {
        let mut out = vec![0i16; frames * 2];
        if let Some(m) = self.mixer.as_mut() {
            m.render(&mut out);
        }
        out
    }

    pub fn set_played(&mut self, played: u64) {
        self.played = played;
    }

    // --- la voz de JARVIS ------------------------------------------------------------------

    /// Llegó un pedazo de la voz (PCM mono de 16 bits a `rate` Hz). `end`: no llega más.
    pub fn voice(&mut self, rate: u32, pcm: &[i16], end: bool) {
        let out_rate = self.rate();
        let Some(m) = self.mixer.as_mut() else {
            return;
        };
        let id = match self.voice {
            Some(id) if m.playing(id) => id,
            _ => {
                let src = Resampled::new(Stream::new(rate, 1), out_rate);
                let id = m.play(Box::new(src), Kind::Voice);
                self.voice = Some(id);
                id
            }
        };
        m.feed(id, pcm, end);
    }

    /// Se calla (Roman interrumpió o pidió otra cosa).
    pub fn hush(&mut self) {
        if let Some(m) = self.mixer.as_mut() {
            m.stop_kind(Kind::Voice);
        }
        self.voice = None;
    }

    /// ¿Está hablando JARVIS por los parlantes de JARVIS-OS?
    pub fn speaking(&self) -> bool {
        self.voice.is_some()
    }

    /// El nivel de la voz que suena ahora (0..=100): mueve la esfera.
    pub fn voice_level(&self) -> u8 {
        self.mixer
            .as_ref()
            .map_or(0, |m| m.voice_level_at(self.played))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_posicion_descuenta_lo_que_todavia_no_sono() {
        let mut s = Sound::default();
        assert!(!s.available());
        assert!(s.play_wav(vec![], Kind::Music).is_err());
        s.enable(8000);
        let fmt = jarvis_audio::wav::Format {
            codec: jarvis_audio::wav::Codec::Pcm { bits: 16 },
            channels: 1,
            rate: 8000,
        };
        let pcm: Vec<u8> = (0..8000i16).flat_map(|v| v.to_le_bytes()).collect();
        let id = s
            .play_wav(jarvis_audio::wav::write(fmt, &pcm), Kind::Music)
            .unwrap();
        s.render(4000);
        // Se mezclaron 500 ms, pero la placa solo reprodujo 300.
        s.set_played(2400);
        assert_eq!(s.position_ms(id), Some(300));
        s.render(8000);
        assert!(s.take_ended().contains(&id));
        assert!(!s.playing(id));
    }

    #[test]
    fn la_voz_llega_de_a_pedazos_y_se_puede_callar() {
        let mut s = Sound::default();
        s.enable(16_000);
        s.voice(16_000, &vec![9000i16; 3200], false);
        assert!(s.speaking());
        s.render(1600);
        s.set_played(1600);
        assert!(s.voice_level() > 40, "{}", s.voice_level());
        s.hush();
        assert!(!s.speaking());
        s.render(1600);
        s.set_played(3200);
        assert_eq!(s.voice_level(), 0);
    }
}
