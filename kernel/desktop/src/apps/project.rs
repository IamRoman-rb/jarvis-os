//! Proyecto: el avance del agente de código que JARVIS abrió en el anfitrión ("abrí jarvis-os y
//! seguí"). Muestra lo que dice, las herramientas que usa y cómo terminó; Esc lo detiene.
//! Las ediciones y los comandos del agente se confirman en el diálogo de siempre (ADR 0008).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::Ctx;
use crate::brain::BrainOp;
use crate::i18n::tr;
use crate::input::{Key, Mods};
use crate::widgets::{draw_fit, label, light, s16, window_bg};

const LINE_H: i32 = 20;
const MAX_LINES: usize = 2000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Tool,
    Error,
}

pub struct Project {
    pub dirty: bool,
    name: String,
    lines: Vec<(Kind, String)>,
    running: bool,
    scroll: usize,
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}

impl Project {
    pub fn new() -> Self {
        Project {
            dirty: true,
            name: String::new(),
            lines: Vec::new(),
            running: false,
            scroll: 0,
        }
    }

    pub fn title(&self) -> String {
        if self.name.is_empty() {
            tr("Proyecto").into()
        } else {
            format!("{} · {}", tr("Proyecto"), self.name)
        }
    }

    /// Todo lo que muestra (para los tests).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (_, l) in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    pub fn running(&self) -> bool {
        self.running
    }

    fn push(&mut self, kind: Kind, text: &str) {
        for l in text.lines() {
            self.lines.push((kind, l.into()));
        }
        if self.lines.len() > MAX_LINES {
            let cut = self.lines.len() - MAX_LINES;
            self.lines.drain(..cut);
        }
    }

    /// Un evento del agente: `inicio`, `texto`, `herramienta`, `fin` o `error`.
    pub fn event(&mut self, name: &str, ev: &str, text: &str) {
        self.dirty = true;
        self.scroll = 0;
        match ev {
            "inicio" => {
                self.name = name.into();
                self.running = true;
                if !self.lines.is_empty() {
                    self.lines.push((Kind::Tool, String::new()));
                }
                self.push(Kind::Tool, &format!("> {text}"));
            }
            "texto" => self.push(Kind::Text, text),
            "herramienta" => self.push(Kind::Tool, &format!("  · {text}")),
            "error" => {
                self.running = false;
                self.push(Kind::Error, text);
            }
            _ => {
                self.running = false;
                self.push(Kind::Tool, &format!("-- {text}"));
            }
        }
    }

    pub fn wheel(&mut self, delta: i32) {
        let max = self.lines.len();
        self.scroll = (self.scroll as i32 - delta).clamp(0, max as i32) as usize;
        self.dirty = true;
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, _mods: Mods, ctx: &mut Ctx<'_, D>) -> bool {
        match key {
            Key::Escape if self.running => {
                ctx.out.brain.push(BrainOp::StopProject);
                self.push(Kind::Tool, tr("Deteniendo..."));
            }
            Key::PageUp => self.wheel(-10),
            Key::PageDown => self.wheel(10),
            _ => return false,
        }
        self.dirty = true;
        true
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect) {
        c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
        let status = if self.running {
            tr("TRABAJANDO · ESC DETIENE")
        } else if self.name.is_empty() {
            tr("PEDILE A JARVIS QUE ABRA UN PROYECTO")
        } else {
            tr("TERMINADO")
        };
        text::draw(
            c,
            r.x + 14,
            r.y + 12,
            status,
            &label(if self.running {
                theme::cyan()
            } else {
                theme::text_dim()
            }),
        );
        let top = r.y + 40;
        let visible = ((r.h - 50) / LINE_H).max(1) as usize;
        let end = self.lines.len().saturating_sub(self.scroll);
        let start = end.saturating_sub(visible);
        for (i, (kind, line)) in self.lines[start..end].iter().enumerate() {
            let st = match kind {
                Kind::Text => s16(theme::text()),
                Kind::Tool => light(theme::text_dim()),
                Kind::Error => s16(theme::crimson()),
            };
            draw_fit(c, r.x + 14, top + i as i32 * LINE_H, line, &st, r.w - 28);
        }
    }
}
