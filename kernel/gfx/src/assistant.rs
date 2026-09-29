//! Estado del asistente: en reposo o hablando, y cómo eso se traduce en el pulso de la esfera.
//!
//! La **envolvente de voz** es la "fuerza" del habla en cada instante (0 = silencio, `ONE` =
//! máximo). Por ahora se sintetiza: un ritmo de sílabas de ~4,5 por segundo que baja en los
//! espacios y signos de puntuación, como las pausas al hablar. Cuando haya audio real (hito K12),
//! `level` se va a calcular desde las muestras de audio y el resto no cambia.
//!
//! Todo el tiempo se mide en milisegundos desde el arranque (lo da el timer del kernel).

use alloc::string::String;

use crate::sphere::Pulse;
use crate::trig::{self, FULL_TURN, ONE, mul};

/// Velocidad a la que "se dice" el texto (y se escribe en pantalla): ~18 letras por segundo.
pub const MS_PER_CHAR: u64 = 55;
/// Subida suave al empezar a hablar.
const FADE_IN_MS: u64 = 150;
/// El habla sigue un instante después de la última letra.
const TAIL_MS: u64 = 300;
/// Bajada suave al terminar.
const FADE_OUT_MS: u64 = 500;
/// Ritmo de sílabas: ~4,5 Hz (período de 222 ms).
const SYLLABLE_PERIOD_MS: u64 = 222;
/// Respiración en reposo: un ciclo cada 4 s.
const BREATH_PERIOD_MS: u64 = 4000;
/// Las ondas recorren la esfera ~1,4 veces por segundo.
const WAVE_PERIOD_MS: u64 = 700;

#[derive(Default)]
pub struct Assistant {
    text: String,
    started_ms: u64,
    chars: usize,
    /// Ya se avisó que terminó de hablar (para [`take_finished`](Self::take_finished)).
    finished_reported: bool,
    /// Nivel del audio real (la voz del anfitrión, 0..=100) y hasta cuándo vale.
    audio: (u8, u64),
}

impl Assistant {
    pub fn new() -> Self {
        Self::default()
    }

    /// JARVIS empieza a decir `text` en el instante `now`.
    pub fn say(&mut self, text: &str, now: u64) {
        self.text.clear();
        self.text.push_str(text);
        self.chars = text.chars().count();
        self.started_ms = now;
        self.finished_reported = false;
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn speech_end(&self) -> u64 {
        self.started_ms + self.chars as u64 * MS_PER_CHAR + TAIL_MS
    }

    pub fn is_speaking(&self, now: u64) -> bool {
        (!self.text.is_empty() && now < self.speech_end()) || now < self.audio.1
    }

    /// Cuántas letras ya se "dijeron" (y se ven en pantalla).
    pub fn visible_chars(&self, now: u64) -> usize {
        let elapsed = now.saturating_sub(self.started_ms);
        ((elapsed / MS_PER_CHAR) as usize).min(self.chars)
    }

    /// El texto dicho hasta ahora (efecto máquina de escribir).
    pub fn visible_text(&self, now: u64) -> &str {
        let n = self.visible_chars(now);
        match self.text.char_indices().nth(n) {
            Some((byte, _)) => &self.text[..byte],
            None => &self.text,
        }
    }

    /// Devuelve `true` una sola vez, cuando termina de hablar.
    pub fn take_finished(&mut self, now: u64) -> bool {
        if !self.text.is_empty() && !self.finished_reported && !self.is_speaking(now) {
            self.finished_reported = true;
            return true;
        }
        false
    }

    /// El nivel del audio que está sonando (0..=100): mientras llega, manda sobre la
    /// envolvente sintética. Si deja de llegar, a los 200 ms se vuelve a la sintética.
    pub fn set_audio_level(&mut self, level: u8, now: u64) {
        self.audio = (level.min(100), now + 200);
    }

    /// Envolvente de voz en Q14 (0..=ONE).
    pub fn level(&self, now: u64) -> i32 {
        if now < self.audio.1 {
            return self.audio.0 as i32 * ONE / 100;
        }
        if self.text.is_empty() {
            return 0;
        }
        let elapsed = now.saturating_sub(self.started_ms);
        let end = self.speech_end() - self.started_ms;
        if elapsed >= end + FADE_OUT_MS {
            return 0;
        }
        // Subida al empezar, meseta mientras habla, bajada al terminar.
        let fade = if elapsed < FADE_IN_MS {
            elapsed as i32 * ONE / FADE_IN_MS as i32
        } else if elapsed > end {
            (end + FADE_OUT_MS - elapsed) as i32 * ONE / FADE_OUT_MS as i32
        } else {
            ONE
        };
        // Sílabas: la mitad positiva de una onda de ~4,5 Hz, sobre un piso de 35 %.
        let phase = (elapsed % SYLLABLE_PERIOD_MS) * FULL_TURN as u64 / SYLLABLE_PERIOD_MS;
        let syllable = trig::sin(phase as u32).max(0);
        let mut level = mul(fade, ONE * 35 / 100 + syllable * 65 / 100);
        // Pausas: si la letra que se está diciendo es un espacio o un signo, baja.
        let current = self.text.chars().nth(self.visible_chars(now));
        if matches!(
            current,
            Some(' ' | ',' | '.' | ';' | ':' | '¿' | '?' | '¡' | '!')
        ) {
            level = level * 35 / 100;
        }
        level.clamp(0, ONE)
    }

    /// Pulso de la esfera en `now`: respiración en reposo, latidos y ondas al hablar.
    pub fn pulse(&self, now: u64) -> Pulse {
        let level = self.level(now);
        let breath_phase = (now % BREATH_PERIOD_MS) * FULL_TURN as u64 / BREATH_PERIOD_MS;
        let breath = trig::sin(breath_phase as u32);
        let wave_phase = (now % WAVE_PERIOD_MS) * FULL_TURN as u64 / WAVE_PERIOD_MS;
        Pulse {
            // ±1 % de respiración, hasta +8 % con la voz.
            scale: ONE + breath / 100 + level * 8 / 100,
            wave: level * 6 / 100,
            wave_phase: wave_phase as u32,
            glow: (level * 150 / ONE) as u8,
        }
        .clamped()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn el_audio_real_manda_mientras_llega() {
        let mut a = super::Assistant::new();
        a.set_audio_level(50, 1000);
        assert_eq!(a.level(1100), super::ONE / 2);
        assert_eq!(
            a.level(1300),
            0,
            "sin audio nuevo, vuelve a la envolvente (en silencio)"
        );
    }

    use super::*;

    #[test]
    fn en_reposo_no_hay_voz() {
        let a = Assistant::new();
        assert_eq!(a.level(1000), 0);
        assert!(!a.is_speaking(1000));
        let p = a.pulse(1000);
        assert_eq!((p.wave, p.glow), (0, 0));
        assert!((p.scale - ONE).abs() <= ONE / 100, "solo respira");
    }

    #[test]
    fn escribe_letra_por_letra_con_acentos() {
        let mut a = Assistant::new();
        a.say("¿Qué tal?", 1000);
        assert_eq!(a.visible_text(1000), "");
        assert_eq!(a.visible_text(1000 + 2 * MS_PER_CHAR), "¿Q");
        assert_eq!(a.visible_text(1000 + 3 * MS_PER_CHAR), "¿Qu");
        assert_eq!(a.visible_text(1000 + 4 * MS_PER_CHAR), "¿Qué");
        assert_eq!(a.visible_text(99_999), "¿Qué tal?");
    }

    #[test]
    fn la_envolvente_se_mueve_y_se_apaga() {
        let mut a = Assistant::new();
        a.say("Todos los sistemas funcionan con normalidad", 0);
        let niveles: std::vec::Vec<i32> = (0..40).map(|i| a.level(200 + i * 37)).collect();
        assert!(niveles.iter().all(|l| (0..=ONE).contains(l)));
        let (min, max) = (
            *niveles.iter().min().unwrap(),
            *niveles.iter().max().unwrap(),
        );
        assert!(
            max - min > ONE / 3,
            "la voz tiene que latir, no ser plana ({min}..{max})"
        );
        let fin = 43 * MS_PER_CHAR + TAIL_MS;
        assert!(a.is_speaking(fin - 1));
        assert!(!a.is_speaking(fin));
        assert!(a.level(fin + 100) > 0, "baja suave");
        assert_eq!(a.level(fin + FADE_OUT_MS), 0);
    }

    #[test]
    fn avisa_el_fin_una_sola_vez() {
        let mut a = Assistant::new();
        assert!(!a.take_finished(0), "sin haber hablado no hay fin");
        a.say("Hola", 0);
        assert!(!a.take_finished(100));
        assert!(a.take_finished(10_000));
        assert!(!a.take_finished(10_001));
    }

    #[test]
    fn hablando_la_esfera_crece_y_brilla() {
        let mut a = Assistant::new();
        a.say("mmmmmmmmmmmmmmmmmmmmmmmmmmmmmm", 0);
        let max = (200..1500)
            .step_by(10)
            .map(|t| a.pulse(t))
            .max_by_key(|p| p.scale)
            .unwrap();
        assert!(max.scale > ONE * 105 / 100);
        assert!(max.wave > 0 && max.glow > 0);
        assert!(max.scale <= Pulse::MAX_SCALE);
    }
}
