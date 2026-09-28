//! Consola de JARVIS: le escribís una orden y JARVIS responde (y la esfera "habla").
//!
//! Es el lugar del micrófono de la barra: hasta que haya audio (K12) y conexión con Claude (K7),
//! a JARVIS se le habla escribiendo. Entiende un puñado de órdenes locales (abrir apps, ver
//! archivos, navegar, estado de la máquina); lo demás lo va a responder Claude.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::Ctx;
use crate::files::format_size;
use crate::i18n::tr;
use crate::input::{Key, Mods};
use crate::system::{AppKind, Launch, Power};
use crate::text_input::TextInput;
use crate::widgets::{draw_fit, field_bg, ip, label, light, s16, window_bg};

const LINE_H: i32 = 20;
const MAX_LINES: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Who {
    User,
    Jarvis,
    Error,
}

pub struct Console {
    pub dirty: bool,
    lines: Vec<(Who, String)>,
    input: TextInput,
    /// Órdenes anteriores (↑ ↓ las recorren, como en una terminal).
    history: Vec<String>,
    history_pos: Option<usize>,
    /// Cuántas líneas se subió con la rueda o RePág (0 = pegado al final).
    scroll: usize,
    /// Esperando la respuesta del cerebro (Esc la corta).
    waiting: bool,
    /// La orden que se está procesando vino de la voz.
    by_voice: bool,
    /// La línea de la respuesta que se está armando (llega de a pedazos).
    streaming: Option<usize>,
}

const HELP: &[&str] = &[
    "Órdenes que entiendo por ahora:",
    "  abrir <archivos|monitor|música|navegador|editor>",
    "  ir <dirección>        abre una página (ej.: ir example.com)",
    "  buscar <texto>        busca en la web",
    "  ls [carpeta]          lista una carpeta del disco",
    "  cat <archivo>         muestra un archivo de texto",
    "  editar <archivo>      lo abre en el editor",
    "  hora · estado · red · captura · limpiar",
    "  decir <texto>         lo digo en voz alta (la esfera)",
    "  apagar · reiniciar",
];

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    pub fn new() -> Self {
        Console {
            dirty: true,
            lines: alloc::vec![(
                Who::Jarvis,
                tr("Consola de JARVIS. Escribí \"ayuda\" para ver qué sé hacer.").into()
            ),],
            input: TextInput::new("", 200),
            history: Vec::new(),
            history_pos: None,
            scroll: 0,
            waiting: false,
            by_voice: false,
            streaming: None,
        }
    }

    /// Todo lo que muestra la consola (para los tests).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for (_, l) in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    pub fn title(&self) -> String {
        tr("Consola JARVIS").into()
    }

    fn say(&mut self, who: Who, text: impl Into<String>) {
        let text = text.into();
        for line in text.lines() {
            self.lines.push((who, line.into()));
        }
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect) {
        c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
        let field = Rect::new(r.x + 12, r.y + r.h - 46, r.w - 24, 34);
        let area_h = field.y - r.y - 16;
        let visible = (area_h / LINE_H).max(1) as usize;
        let end = self.lines.len().saturating_sub(self.scroll);
        let start = end.saturating_sub(visible);
        for (i, (who, line)) in self.lines[start..end].iter().enumerate() {
            let y = r.y + 10 + i as i32 * LINE_H;
            let (prefix, st) = match who {
                Who::User => ("> ", s16(theme::cyan())),
                Who::Jarvis => ("", s16(theme::text())),
                Who::Error => ("", s16(theme::amber())),
            };
            let shown = format!("{prefix}{line}");
            draw_fit(c, r.x + 16, y, &shown, &st, r.w - 32);
        }
        jarvis_gfx::shapes::rounded_rect(c, field.x, field.y, field.w, field.h, 4, field_bg(), 255);
        jarvis_gfx::shapes::rounded_outline(
            c,
            field.x,
            field.y,
            field.w,
            field.h,
            4,
            theme::cyan().scale(150),
        );
        text::draw(
            c,
            field.x + 10,
            field.y + 9,
            "JARVIS>",
            &label(theme::cyan()),
        );
        let st = s16(theme::text());
        let x0 = field.x + 96;
        let shown = tail_fit(&self.input.text, &st, field.w - 110);
        let tw = text::draw(c, x0, field.y + 9, &shown, &st);
        c.fill_rect(x0 + tw + 2, field.y + 8, 2, 18, theme::cyan());
        if self.scroll > 0 {
            text::draw_right(
                c,
                r.x + r.w - 16,
                field.y - 22,
                tr("(RePág/AvPág para moverse)"),
                &light(theme::text_dim()),
            );
        }
    }

    pub fn wheel(&mut self, delta: i32) {
        let max = self.lines.len().saturating_sub(1);
        self.scroll = (self.scroll as i64 - delta as i64 * 3).clamp(0, max as i64) as usize;
        self.dirty = true;
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, mods: Mods, ctx: &mut Ctx<'_, D>) -> bool {
        if mods.ctrl && matches!(key, Key::Char('l' | 'L')) {
            self.lines.clear();
            self.dirty = true;
            return true;
        }
        match key {
            Key::Escape if self.waiting => {
                ctx.out.brain.push(crate::brain::BrainOp::Cancel);
            }
            Key::Enter => {
                let cmd = core::mem::take(&mut self.input.text);
                let cmd = cmd.trim().to_string();
                self.history_pos = None;
                self.scroll = 0;
                if !cmd.is_empty() {
                    self.say(Who::User, cmd.clone());
                    self.history.push(cmd.clone());
                    ctx.log.push(format!("CONSOLA {cmd}"));
                    self.run(&cmd, ctx);
                }
            }
            Key::Up if !self.history.is_empty() => {
                let pos = self
                    .history_pos
                    .map_or(self.history.len() - 1, |p| p.saturating_sub(1));
                self.history_pos = Some(pos);
                self.input.text = self.history[pos].clone();
            }
            Key::Down => {
                if let Some(p) = self.history_pos {
                    if p + 1 < self.history.len() {
                        self.history_pos = Some(p + 1);
                        self.input.text = self.history[p + 1].clone();
                    } else {
                        self.history_pos = None;
                        self.input.text.clear();
                    }
                }
            }
            Key::PageUp => self.wheel(-3),
            Key::PageDown => self.wheel(3),
            other => {
                if !self.input.handle(other) {
                    return false;
                }
            }
        }
        self.dirty = true;
        true
    }

    /// JARVIS responde: en la consola y en voz (la esfera y el mensaje del escritorio).
    fn answer<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>, text: impl Into<String>) {
        let text = text.into();
        ctx.out.say = Some(text.lines().next().unwrap_or("").into());
        // Con el cerebro conectado, la respuesta local también se dice (si hay voz).
        if ctx.stats.brain_online {
            ctx.out.brain.push(crate::brain::BrainOp::Speak(
                text.lines().next().unwrap_or("").into(),
            ));
        }
        self.say(Who::Jarvis, text);
    }

    fn fail(&mut self, text: impl Into<String>) {
        self.say(Who::Error, text);
    }

    fn run<D: BlockDevice>(&mut self, cmd: &str, ctx: &mut Ctx<'_, D>) {
        let (verb, arg) = match cmd.split_once(' ') {
            Some((v, a)) => (v, a.trim()),
            None => (cmd, ""),
        };
        let verb = verb.to_lowercase();
        match verb.as_str() {
            "ayuda" | "help" | "?" => {
                for l in HELP {
                    self.say(Who::Jarvis, *l);
                }
            }
            "hola" if !ctx.stats.brain_online => self.answer(ctx, "Hola. Estoy acá: escribí \"ayuda\" para ver qué puedo hacer."),
            "limpiar" | "clear" | "cls" => self.lines.clear(),
            "hora" | "fecha" => {
                let msg = match ctx.clock {
                    Some(t) => {
                        let mut date = jarvis_gfx::clock::StrBuf::<48>::new();
                        let _ = t.write_date(&mut date);
                        format!(
                            "Son las {:02}:{:02}. Hoy es {}.",
                            t.hour,
                            t.minute,
                            date.as_str().to_lowercase()
                        )
                    }
                    None => "No tengo reloj: el hardware no me dio la hora.".into(),
                };
                self.answer(ctx, msg);
            }
            // "abrir monitor" es local; "abrí el navegador y buscá…" es para Claude.
            "abrir" | "abri" | "abrí" | "open"
                if super::app_by_name(arg).is_some() || !ctx.stats.brain_online =>
            {
                let kind = super::app_by_name(arg);
                match kind {
                    Some(k) => {
                        ctx.out.launch.push(Launch::App(k));
                        self.answer(ctx, format!("Abriendo {}.", super::name_of(k)));
                    }
                    None => self.fail(format!("No conozco la app \"{arg}\".")),
                }
            }
            "ir" | "navegar" | "abrir-web" => {
                if arg.is_empty() {
                    self.fail("¿A dónde? Por ejemplo: ir example.com");
                } else {
                    ctx.out.launch.push(Launch::Browse(arg.into()));
                    self.answer(ctx, format!("Abriendo {arg} en el navegador."));
                }
            }
            "buscar" | "googlear" => {
                if arg.is_empty() {
                    self.fail("¿Qué busco?");
                } else {
                    ctx.out
                        .launch
                        .push(Launch::Browse(format!("? {arg}")));
                    self.answer(ctx, format!("Buscando \"{arg}\"."));
                }
            }
            "ls" | "dir" => {
                let path = if arg.is_empty() { "/" } else { arg };
                let Some(fs) = ctx.fs.as_deref_mut() else {
                    self.fail(tr("No hay disco."));
                    return;
                };
                match fs.list(path) {
                    Ok(entries) => {
                        let lines: Vec<String> = entries
                            .iter()
                            .filter(|e| !e.name.starts_with('.'))
                            .map(|e| {
                                if e.is_dir {
                                    format!("  {}/", e.name)
                                } else {
                                    format!("  {:<40} {}", e.name, format_size(e.size as u64))
                                }
                            })
                            .collect();
                        if lines.is_empty() {
                            self.say(Who::Jarvis, "  (vacía)");
                        }
                        for l in lines {
                            self.say(Who::Jarvis, l);
                        }
                    }
                    Err(e) => self.fail(format!("{path}: {e}")),
                }
            }
            "cat" | "ver" | "type" => {
                let Some(fs) = ctx.fs.as_deref_mut() else {
                    self.fail(tr("No hay disco."));
                    return;
                };
                match fs.read_prefix(arg, 16 * 1024) {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).replace('\t', "    ");
                        for l in text.lines().take(200) {
                            self.say(Who::Jarvis, l.to_string());
                        }
                    }
                    Err(e) => self.fail(format!("{arg}: {e}")),
                }
            }
            "editar" | "edit" | "nano" => {
                if arg.is_empty() {
                    ctx.out.launch.push(Launch::App(AppKind::Editor));
                } else {
                    ctx.out.launch.push(Launch::Edit(arg.into()));
                }
                self.answer(ctx, "Abriendo el editor.");
            }
            "estado" | "top" => {
                let s = ctx.stats;
                let msg = format!(
                    "CPU {} % · memoria {} de {} · {} FPS · encendido hace {}",
                    s.cpu_pct,
                    format_size(s.heap_used),
                    format_size(s.heap_total),
                    s.fps,
                    crate::widgets::duration(s.uptime_ms)
                );
                self.answer(ctx, msg);
            }
            "red" | "ip" | "ipconfig" => {
                let n = &ctx.stats.net;
                let msg = match (n.present, n.ip) {
                    (false, _) => "No encontré una placa de red.".into(),
                    (true, None) => "La placa de red está, pero todavía no tengo dirección (DHCP).".into(),
                    (true, Some(a)) => format!(
                        "Mi dirección es {}{}{}.",
                        ip(a),
                        n.gateway.map(|g| format!(", la puerta de enlace {}", ip(g))).unwrap_or_default(),
                        n.dns.map(|d| format!(" y el DNS {}", ip(d))).unwrap_or_default()
                    ),
                };
                self.answer(ctx, msg);
            }
            "captura" | "screenshot" => {
                ctx.out.screenshot = true;
                self.answer(ctx, "Captura en camino: queda en /Imágenes.");
            }
            "decir" | "di" | "say" => {
                if arg.is_empty() {
                    self.fail("¿Qué digo?");
                } else {
                    self.answer(ctx, arg.to_string());
                }
            }
            "apagar" | "shutdown" => {
                self.answer(ctx, "Apagando. Hasta luego.");
                ctx.out.power = Some(Power::Shutdown);
            }
            "reiniciar" | "reboot" => {
                self.answer(ctx, "Reiniciando.");
                ctx.out.power = Some(Power::Reboot);
            }
            // Lo que no es una orden local va al cerebro (Claude, en el anfitrión).
            _ if ctx.stats.brain_online => {
                self.waiting = true;
                self.streaming = None;
                ctx.out
                    .brain
                    .push(crate::brain::BrainOp::Ask(cmd.to_string(), self.by_voice));
            }
            _ => self.answer(
                ctx,
                tr("No entiendo eso y el cerebro (Claude) no está conectado: abrí JARVIS con \"cargo xtask run\", que lo levanta. Escribí \"ayuda\" para las órdenes locales."),
            ),
        }
    }

    /// Roman lo dijo en voz alta: como si lo hubiera escrito (una orden local o un pedido a
    /// Claude, que responde también en voz alta).
    pub fn heard<D: BlockDevice>(&mut self, text: &str, ctx: &mut Ctx<'_, D>) {
        self.scroll = 0;
        self.dirty = true;
        self.say(Who::User, format!("({}) {text}", tr("voz")));
        ctx.log.push(format!("CONSOLA_VOZ {text}"));
        self.by_voice = true;
        self.run(text, ctx);
        self.by_voice = false;
    }

    /// Lo que llega del cerebro: la respuesta se va escribiendo a medida que llega.
    pub fn brain_event(&mut self, ev: &crate::brain::BrainEvent) {
        use crate::brain::BrainEvent;
        self.dirty = true;
        self.scroll = 0;
        match ev {
            BrainEvent::Text(delta) => {
                for (i, part) in delta.split('\n').enumerate() {
                    match self.streaming {
                        Some(n) if i == 0 && n < self.lines.len() => {
                            self.lines[n].1.push_str(part);
                        }
                        _ => {
                            self.lines.push((Who::Jarvis, part.into()));
                            self.streaming = Some(self.lines.len() - 1);
                        }
                    }
                }
                if self.lines.len() > MAX_LINES {
                    let cut = self.lines.len() - MAX_LINES;
                    self.lines.drain(..cut);
                    self.streaming = self.streaming.map(|n| n.saturating_sub(cut));
                }
            }
            BrainEvent::Action { tool, .. } => {
                self.lines.push((Who::User, format!("  · {tool}")));
                self.streaming = None;
            }
            BrainEvent::Confirm { .. }
            | BrainEvent::Project { .. }
            | BrainEvent::Heard(_)
            | BrainEvent::Listening(_)
            | BrainEvent::VoiceLevel(_) => {}
            BrainEvent::End => {
                self.waiting = false;
                self.streaming = None;
            }
            BrainEvent::Error(m) => {
                self.waiting = false;
                self.streaming = None;
                self.fail(m.clone());
            }
        }
    }
}

/// Los últimos caracteres de `s` que entran en `max_w` (el cursor siempre queda a la vista).
fn tail_fit(s: &str, st: &text::Style, max_w: i32) -> String {
    let mut out: Vec<char> = Vec::new();
    for ch in s.chars().rev() {
        out.push(ch);
        let candidate: String = out.iter().rev().collect();
        if text::width(&candidate, st) > max_w {
            out.pop();
            break;
        }
    }
    out.iter().rev().collect()
}
