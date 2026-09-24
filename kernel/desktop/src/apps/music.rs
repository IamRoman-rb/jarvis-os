//! Música por el parlante de la PC ("PC speaker"): el chip PIT genera una onda cuadrada a la
//! frecuencia de cada nota. Suena como una computadora de los 80, pero no necesita driver de
//! audio (eso llega en K11). Las canciones son de dominio público.
//!
//! Las partituras se escriben como texto: `"E4:4 D#4:8. R:8"` = mi de la 4.ª octava negra,
//! re sostenido corchea con puntillo, silencio de corchea.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text;
use jarvis_gfx::trig::{FULL_TURN, ONE, sin};
use jarvis_gfx::{Canvas, Rect, theme};

use super::{Click, Ctx};
use crate::i18n::{tr, trf};
use crate::input::Key;
use crate::widgets::{SELECTED_BG, WINDOW_BG, bar, button, label, light, s16};

pub struct Song {
    pub title: &'static str,
    pub author: &'static str,
    /// Negras por minuto.
    pub tempo: u32,
    pub score: &'static str,
}

pub const SONGS: [Song; 5] = [
    Song {
        title: "Himno a la alegría",
        author: "L. van Beethoven",
        tempo: 120,
        score: "E4:4 E4:4 F4:4 G4:4 G4:4 F4:4 E4:4 D4:4 C4:4 C4:4 D4:4 E4:4 E4:4. D4:8 D4:2 \
                E4:4 E4:4 F4:4 G4:4 G4:4 F4:4 E4:4 D4:4 C4:4 C4:4 D4:4 E4:4 D4:4. C4:8 C4:2",
    },
    Song {
        title: "Para Elisa",
        author: "L. van Beethoven",
        tempo: 72,
        score: "E5:16 D#5:16 E5:16 D#5:16 E5:16 B4:16 D5:16 C5:16 A4:8 R:16 C4:16 E4:16 A4:16 \
                B4:8 R:16 E4:16 G#4:16 B4:16 C5:8 R:16 E4:16 E5:16 D#5:16 E5:16 D#5:16 E5:16 \
                B4:16 D5:16 C5:16 A4:8 R:16 C4:16 E4:16 A4:16 B4:8 R:16 E4:16 C5:16 B4:16 A4:4",
    },
    Song {
        title: "Korobéiniki",
        author: "Canción popular rusa",
        tempo: 144,
        score: "E5:4 B4:8 C5:8 D5:4 C5:8 B4:8 A4:4 A4:8 C5:8 E5:4 D5:8 C5:8 B4:4. C5:8 D5:4 \
                E5:4 C5:4 A4:4 A4:2 R:8 D5:4. F5:8 A5:4 G5:8 F5:8 E5:4. C5:8 E5:4 D5:8 C5:8 \
                B4:4 B4:8 C5:8 D5:4 E5:4 C5:4 A4:4 A4:4 R:4",
    },
    Song {
        title: "Cumpleaños feliz",
        author: "M. y P. Hill",
        tempo: 110,
        score: "C4:8. C4:16 D4:4 C4:4 F4:4 E4:2 C4:8. C4:16 D4:4 C4:4 G4:4 F4:2 C4:8. C4:16 \
                C5:4 A4:4 F4:4 E4:4 D4:4 A#4:8. A#4:16 A4:4 F4:4 G4:4 F4:2",
    },
    Song {
        title: "Escala de prueba",
        author: "JARVIS-OS",
        tempo: 200,
        score: "C4:8 D4:8 E4:8 F4:8 G4:8 A4:8 B4:8 C5:4 R:8 C5:8 B4:8 A4:8 G4:8 F4:8 E4:8 D4:8 C4:4",
    },
];

/// Frecuencias de la octava 4 (do central a si), en centésimas de Hz.
const OCTAVE4: [u32; 12] = [
    26163, 27718, 29366, 31113, 32963, 34923, 36999, 39200, 41530, 44000, 46616, 49388,
];

/// (frecuencia en Hz, 0 = silencio; duración en ms)
pub type Note = (u32, u32);

/// Lee una partitura. Las notas mal escritas se ignoran.
pub fn parse(score: &str, tempo: u32) -> Vec<Note> {
    let quarter_ms = 60_000 / tempo.max(1);
    score
        .split_whitespace()
        .filter_map(|tok| {
            let (pitch, dur) = tok.split_once(':')?;
            let dotted = dur.ends_with('.');
            let div: u32 = dur.trim_end_matches('.').parse().ok()?;
            let mut ms = quarter_ms * 4 / div.max(1);
            if dotted {
                ms += ms / 2;
            }
            if pitch == "R" {
                return Some((0, ms));
            }
            let mut chars = pitch.chars();
            let letter = chars.next()?;
            let rest: String = chars.collect();
            let (sharp, octave) = match rest.strip_prefix('#') {
                Some(o) => (1, o),
                None => (0, rest.as_str()),
            };
            let octave: i32 = octave.parse().ok()?;
            let base = match letter {
                'C' => 0,
                'D' => 2,
                'E' => 4,
                'F' => 5,
                'G' => 7,
                'A' => 9,
                'B' => 11,
                _ => return None,
            } + sharp;
            let hz100 = OCTAVE4[base % 12];
            let hz = if octave >= 4 {
                (hz100 << (octave - 4)) / 100
            } else {
                (hz100 >> (4 - octave)) / 100
            };
            Some((hz, ms))
        })
        .collect()
}

pub struct Music {
    pub dirty: bool,
    selected: usize,
    /// (canción, notas, cuándo empezó)
    playing: Option<(usize, Vec<Note>, u64)>,
    /// La frecuencia que está sonando (para no mandarle al kernel la misma cada frame).
    current: u32,
    last_frame: u64,
}

impl Default for Music {
    fn default() -> Self {
        Self::new()
    }
}

fn row_rect(content: Rect, i: usize) -> Rect {
    Rect::new(
        content.x + 16,
        content.y + 50 + i as i32 * 44,
        content.w - 32,
        40,
    )
}

fn play_button(content: Rect) -> Rect {
    Rect::new(content.x + 16, content.y + content.h - 50, 150, 36)
}

impl Music {
    pub fn new() -> Self {
        Music {
            dirty: true,
            selected: 0,
            playing: None,
            current: 0,
            last_frame: 0,
        }
    }

    pub fn title(&self) -> String {
        match &self.playing {
            Some((i, _, _)) => trf("Música · {}", &[SONGS[*i].title]),
            None => tr("Música").into(),
        }
    }

    pub fn play<D: BlockDevice>(&mut self, i: usize, ctx: &mut Ctx<'_, D>) {
        let song = &SONGS[i];
        self.playing = Some((i, parse(song.score, song.tempo), ctx.now_ms));
        ctx.log.push(format!("MUSICA_REPRODUCE {}", song.title));
        self.dirty = true;
    }

    pub fn stop<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        self.playing = None;
        if self.current != 0 {
            self.current = 0;
            ctx.out.tone = Some(0);
        }
        self.dirty = true;
    }

    /// Qué nota suena en `elapsed` ms, y cuánto va de ella (0..=255). None = terminó.
    fn note_at(notes: &[Note], elapsed: u64) -> Option<(usize, u32, u8)> {
        let mut t = 0u64;
        for (i, &(hz, ms)) in notes.iter().enumerate() {
            // Un silencio corto al final de cada nota separa las notas repetidas.
            let gap = (ms as u64 / 8).min(40);
            if elapsed < t + ms as u64 {
                let into = elapsed - t;
                let sounding = if into + gap >= ms as u64 { 0 } else { hz };
                return Some((i, sounding, (into * 255 / ms.max(1) as u64) as u8));
            }
            t += ms as u64;
        }
        None
    }

    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some((_, notes, start)) = &self.playing else {
            return;
        };
        let elapsed = ctx.now_ms.saturating_sub(*start);
        match Self::note_at(notes, elapsed) {
            Some((_, hz, _)) => {
                if hz != self.current {
                    self.current = hz;
                    ctx.out.tone = Some(hz);
                }
            }
            None => {
                self.stop(ctx);
                return;
            }
        }
        // El visualizador se anima a ~30 cuadros por segundo.
        if ctx.now_ms - self.last_frame >= 33 {
            self.last_frame = ctx.now_ms;
            self.dirty = true;
        }
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, ctx: &mut Ctx<'_, D>) -> bool {
        match key {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(SONGS.len() - 1),
            Key::Enter | Key::Char(' ') => {
                if self.playing.as_ref().is_some_and(|p| p.0 == self.selected) {
                    self.stop(ctx);
                } else {
                    self.play(self.selected, ctx);
                }
            }
            Key::Escape => self.stop(ctx),
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        if play_button(content).contains(click.x, click.y) {
            if self.playing.is_some() {
                self.stop(ctx);
            } else {
                self.play(self.selected, ctx);
            }
            return;
        }
        for i in 0..SONGS.len() {
            if row_rect(content, i).contains(click.x, click.y) {
                self.selected = i;
                self.dirty = true;
                if click.double {
                    self.play(i, ctx);
                }
            }
        }
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        c.fill_rect(r.x, r.y, r.w, r.h, WINDOW_BG);
        text::draw(
            c,
            r.x + 16,
            r.y + 16,
            tr("CANCIONES · PARLANTE DE LA PC"),
            &label(theme::CYAN),
        );
        for (i, song) in SONGS.iter().enumerate() {
            let row = row_rect(r, i);
            let playing = self.playing.as_ref().is_some_and(|p| p.0 == i);
            if i == self.selected {
                c.fill_rect(row.x, row.y, row.w, row.h, SELECTED_BG);
                c.fill_rect(row.x, row.y, 3, row.h, theme::CYAN);
            }
            let col = if playing { theme::CYAN } else { theme::TEXT };
            text::draw(c, row.x + 16, row.y + 4, song.title, &s16(col));
            text::draw(
                c,
                row.x + 16,
                row.y + 21,
                song.author,
                &light(theme::TEXT_DIM),
            );
            if playing {
                text::draw_right(
                    c,
                    row.x + row.w - 12,
                    row.y + 12,
                    tr("SONANDO"),
                    &label(theme::CYAN),
                );
            }
        }

        // Visualizador: barras que siguen a la nota (más aguda = más a la derecha).
        let viz = Rect::new(r.x + 190, r.y + r.h - 90, r.w - 206, 76);
        let (level, pitch, progress) = match &self.playing {
            Some((_, notes, start)) => {
                let elapsed = now_ms.saturating_sub(*start);
                let total: u64 = notes.iter().map(|n| n.1 as u64).sum();
                match Self::note_at(notes, elapsed) {
                    Some((_, hz, into)) => (
                        if hz > 0 { 255 - into as i32 } else { 0 },
                        hz as i32,
                        (elapsed * 100 / total.max(1)) as u32,
                    ),
                    None => (0, 0, 100),
                }
            }
            None => (0, 0, 0),
        };
        let bars = 24;
        let bw = viz.w / bars;
        let center = (pitch - 200).clamp(0, 800) * bars / 800;
        for b in 0..bars {
            let dist = (b - center).abs();
            let wobble = sin((now_ms as u32)
                .wrapping_mul(40)
                .wrapping_add(b as u32 * FULL_TURN / 7));
            let base = (level * (bars - dist.min(bars)) / bars).max(0);
            let h =
                ((base * (viz.h - 6) / 255) as i64 * (ONE + wobble / 4) as i64 / ONE as i64) as i32;
            let h = h.clamp(2, viz.h);
            let x = viz.x + b * bw;
            let color = theme::VECTOR_BLUE.lerp(theme::CYAN, (h * 255 / viz.h.max(1)) as u8);
            c.fill_rect(x + 1, viz.y + viz.h - h, bw - 3, h, color);
        }
        bar(
            c,
            Rect::new(viz.x, viz.y + viz.h + 4, viz.w - 3, 4),
            progress,
            theme::CYAN,
        );
        let label_text = if self.playing.is_some() {
            tr("DETENER")
        } else {
            tr("REPRODUCIR")
        };
        button(c, play_button(r), label_text, theme::CYAN, 40);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_partituras() {
        let notes = parse("A4:4 C5:8. R:16 A#3:2 X9:4", 60);
        assert_eq!(notes, [(440, 1000), (523, 750), (0, 250), (233, 2000)]);
        for song in &SONGS {
            let n = parse(song.score, song.tempo);
            assert_eq!(
                n.len(),
                song.score.split_whitespace().count(),
                "{}: alguna nota mal escrita",
                song.title
            );
        }
    }

    #[test]
    fn nota_en_cada_momento() {
        let notes = [(440, 1000), (0, 500), (523, 1000)];
        assert_eq!(Music::note_at(&notes, 0).map(|n| n.1), Some(440));
        assert_eq!(
            Music::note_at(&notes, 990).map(|n| n.1),
            Some(0),
            "separación"
        );
        assert_eq!(Music::note_at(&notes, 1600).map(|n| n.1), Some(523));
        assert_eq!(Music::note_at(&notes, 2500), None);
    }
}
