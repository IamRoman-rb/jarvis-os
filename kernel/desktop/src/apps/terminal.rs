//! Terminal: como la de Ubuntu (o Windows Terminal), con la shell `jsh` (ver `term/`).
//!
//! Entiende los colores ANSI que escriben los comandos (`\x1b[1;34m` …), edita la línea como
//! bash (flechas, Inicio/Fin, Ctrl+A/E/U/K/W), recorre el historial con ↑ ↓, completa con Tab,
//! cancela con Ctrl+C y se desplaza con RePág/AvPág o la rueda del mouse.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text::{self, Size, Style, Weight};
use jarvis_gfx::{Canvas, Color, Rect};

use super::Ctx;
use crate::input::{Key, Mods};
use crate::system::HttpResponse;
use crate::term::Shell;

const MAX_LINES: usize = 3000;
const PAD: i32 = 10;
const BG: Color = Color::hex(0x0b0f17);
const FG: Color = Color::hex(0xd6dde8);

/// Los 16 colores de la terminal (los de Ubuntu, un poco más vivos sobre fondo oscuro).
const PALETTE: [u32; 16] = [
    0x2e3440, 0xe0556b, 0x4fc97a, 0xe8c15a, 0x4f8fe8, 0xb67ae0, 0x3fc5d8, 0xd6dde8, //
    0x6b7689, 0xff7088, 0x72e89a, 0xffd97a, 0x74aaff, 0xd39bff, 0x6fe3f2, 0xffffff,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Sgr {
    fg: Option<u8>,
    bg: Option<u8>,
    bold: bool,
    reverse: bool,
}

type Line = Vec<(Sgr, String)>;

pub struct Terminal {
    pub dirty: bool,
    pub shell: Shell,
    lines: Vec<Line>,
    /// La línea que se está armando (la salida puede llegar en pedazos).
    partial: Line,
    sgr: Sgr,
    input: Vec<char>,
    cursor: usize,
    history_pos: Option<usize>,
    /// Lo escrito antes de empezar a recorrer el historial.
    draft: String,
    /// Renglones subidos (0 = al final).
    scroll: usize,
    blink_on: bool,
    last_blink: u64,
}

fn style(sgr: Sgr) -> (Style, Option<Color>) {
    let mut fg = match sgr.fg {
        Some(i) => Color::hex(PALETTE[(i as usize).min(15)]),
        None => FG,
    };
    if sgr.bold && sgr.fg.is_some_and(|i| i < 8) {
        fg = Color::hex(PALETTE[sgr.fg.unwrap_or(7) as usize + 8]);
    }
    let weight = if sgr.bold {
        Weight::Bold
    } else {
        Weight::Regular
    };
    let bg = sgr.bg.map(|i| Color::hex(PALETTE[(i as usize).min(15)]));
    if sgr.reverse {
        (Style::new(weight, Size::Size16, bg.unwrap_or(BG)), Some(fg))
    } else {
        (Style::new(weight, Size::Size16, fg), bg)
    }
}

impl Terminal {
    pub fn new(user: &str, host: &str) -> Terminal {
        let mut t = Terminal {
            dirty: true,
            shell: Shell::new(user, host),
            lines: Vec::new(),
            partial: Vec::new(),
            sgr: Sgr::default(),
            input: Vec::new(),
            cursor: 0,
            history_pos: None,
            draft: String::new(),
            scroll: 0,
            blink_on: true,
            last_blink: 0,
        };
        t.write("\x1b[1;96mJARVIS-OS 0.1\x1b[0m (hito K4) · shell jsh\n\
             Escribí \x1b[1mhelp\x1b[0m para ver los comandos. Para instalar tu primer programa: \x1b[1mapt install neofetch\x1b[0m\n\n");
        t
    }

    pub fn title(&self) -> String {
        format!(
            "{}@{}: {}",
            self.shell.user(),
            self.shell.host(),
            self.shell.short_cwd()
        )
    }

    /// Todo el texto (sin colores), para los tests.
    pub fn text(&self) -> String {
        let mut s: String = self
            .lines
            .iter()
            .map(|l| l.iter().map(|(_, t)| t.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        s.push('\n');
        s.push_str(
            &self
                .partial
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<String>(),
        );
        s
    }

    fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        match self.partial.last_mut() {
            Some((sgr, t)) if *sgr == self.sgr => t.push_str(s),
            _ => self.partial.push((self.sgr, s.to_string())),
        }
    }

    fn newline(&mut self) {
        let line = core::mem::take(&mut self.partial);
        self.lines.push(line);
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
    }

    /// Agrega texto con códigos ANSI.
    pub fn write(&mut self, s: &str) {
        let mut buf = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            match c {
                '\x1b' if it.peek() == Some(&'[') => {
                    it.next();
                    let mut params = String::new();
                    let mut end = ' ';
                    for d in it.by_ref() {
                        if d.is_ascii_digit() || d == ';' {
                            params.push(d);
                        } else {
                            end = d;
                            break;
                        }
                    }
                    self.push_text(&core::mem::take(&mut buf));
                    if end == 'm' {
                        self.apply_sgr(&params);
                    } else if end == 'J' {
                        self.lines.clear();
                        self.partial.clear();
                    }
                }
                '\n' => {
                    self.push_text(&core::mem::take(&mut buf));
                    self.newline();
                }
                '\r' => {}
                '\t' => {
                    let col: usize = self
                        .partial
                        .iter()
                        .map(|(_, t)| t.chars().count())
                        .sum::<usize>()
                        + buf.chars().count();
                    buf.push_str(&" ".repeat(8 - col % 8));
                }
                c if (c as u32) < 0x20 => {}
                c => buf.push(c),
            }
        }
        self.push_text(&buf);
        self.scroll = 0;
        self.dirty = true;
    }

    fn apply_sgr(&mut self, params: &str) {
        if params.is_empty() {
            self.sgr = Sgr::default();
            return;
        }
        for p in params.split(';') {
            match p.parse::<u8>().unwrap_or(0) {
                0 => self.sgr = Sgr::default(),
                1 => self.sgr.bold = true,
                22 => self.sgr.bold = false,
                7 => self.sgr.reverse = true,
                27 => self.sgr.reverse = false,
                n @ 30..=37 => self.sgr.fg = Some(n - 30),
                n @ 90..=97 => self.sgr.fg = Some(n - 90 + 8),
                39 => self.sgr.fg = None,
                n @ 40..=47 => self.sgr.bg = Some(n - 40),
                n @ 100..=107 => self.sgr.bg = Some(n - 100 + 8),
                49 => self.sgr.bg = None,
                2 => self.sgr.fg = Some(8),
                _ => {}
            }
        }
    }

    fn input_string(&self) -> String {
        self.input.iter().collect()
    }

    /// Escribe y ejecuta un comando como si lo hubiera tipeado el usuario.
    pub fn run_command<D: BlockDevice>(&mut self, cmd: &str, ctx: &mut Ctx<'_, D>) {
        self.input = cmd.chars().collect();
        self.cursor = self.input.len();
        self.enter(ctx);
    }

    fn enter<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let line = self.input_string();
        let prompt = self.shell.prompt();
        self.write(&format!("{prompt}{line}\n"));
        self.input.clear();
        self.cursor = 0;
        self.history_pos = None;
        if line.trim().is_empty() {
            return;
        }
        ctx.log.push(format!("TERMINAL {line}"));
        let out = self.shell.run(&line, ctx);
        self.after(out, ctx);
    }

    fn after<D: BlockDevice>(&mut self, out: String, ctx: &mut Ctx<'_, D>) {
        if core::mem::take(&mut self.shell.clear) {
            self.lines.clear();
            self.partial.clear();
        }
        self.write(&out);
        // Un comando que no terminó su salida con un salto de línea no pisa el prompt.
        if !self.partial.is_empty() && !self.shell.waiting() {
            self.write("\n");
        }
        if self.shell.exit {
            ctx.out.close_self = true;
        }
        if !self.shell.waiting() {
            ctx.log.push(format!("TERMINAL_FIN {}", self.shell.status));
        }
    }

    pub fn net_response<D: BlockDevice>(
        &mut self,
        id: u32,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
    ) {
        if let Some(out) = self.shell.net_response(id, result, ctx) {
            self.after(out, ctx);
        }
    }

    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(out) = self.shell.tick(ctx) {
            self.after(out, ctx);
        }
        if ctx.now_ms.saturating_sub(self.last_blink) >= 530 {
            self.last_blink = ctx.now_ms;
            self.blink_on = !self.blink_on;
            self.dirty = true;
        }
    }

    fn complete<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let before: String = self.input[..self.cursor].iter().collect();
        let candidates = self.shell.complete(&before, ctx);
        let start = before
            .rfind(|c: char| c.is_whitespace() || c == '|' || c == ';' || c == '>')
            .map_or(0, |i| i + 1);
        let word = &before[start..];
        let replacement = match candidates.as_slice() {
            [] => return,
            [one] => {
                let mut r = one.clone();
                if !r.ends_with('/') {
                    r.push(' ');
                }
                r
            }
            many => {
                // Lo que tienen en común; si no agrega nada, se muestran las opciones.
                let first: Vec<char> = many[0].chars().collect();
                let mut n = first.len();
                for c in &many[1..] {
                    n = n.min(
                        c.chars()
                            .zip(first.iter())
                            .take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                            .count(),
                    );
                }
                let common: String = first[..n].iter().collect();
                if common.chars().count() <= word.chars().count() {
                    let prompt = self.shell.prompt();
                    let list = many.join("  ");
                    self.write(&format!("{prompt}{before}\n{list}\n"));
                    return;
                }
                common
            }
        };
        let start_chars = before[..start].chars().count();
        let tail: Vec<char> = self.input[self.cursor..].to_vec();
        self.input.truncate(start_chars);
        self.input.extend(replacement.chars());
        self.cursor = self.input.len();
        self.input.extend(tail);
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, mods: Mods, ctx: &mut Ctx<'_, D>) -> bool {
        self.dirty = true;
        self.blink_on = true;
        self.last_blink = ctx.now_ms;
        if mods.ctrl {
            match key {
                Key::Char('c' | 'C') => {
                    if self.shell.cancel() {
                        self.write("^C\n");
                    } else {
                        let prompt = self.shell.prompt();
                        let line = self.input_string();
                        self.write(&format!("{prompt}{line}^C\n"));
                        self.input.clear();
                        self.cursor = 0;
                    }
                    return true;
                }
                Key::Char('l' | 'L') => {
                    self.lines.clear();
                    return true;
                }
                Key::Char('a' | 'A') => self.cursor = 0,
                Key::Char('e' | 'E') => self.cursor = self.input.len(),
                Key::Char('u' | 'U') => {
                    self.input.drain(..self.cursor);
                    self.cursor = 0;
                }
                Key::Char('k' | 'K') => self.input.truncate(self.cursor),
                Key::Char('w' | 'W') => {
                    let mut i = self.cursor;
                    while i > 0 && self.input[i - 1] == ' ' {
                        i -= 1;
                    }
                    while i > 0 && self.input[i - 1] != ' ' {
                        i -= 1;
                    }
                    self.input.drain(i..self.cursor);
                    self.cursor = i;
                }
                Key::Char('d' | 'D') if self.input.is_empty() => {
                    ctx.out.close_self = true;
                }
                Key::Left => {
                    while self.cursor > 0 && self.input[self.cursor - 1] == ' ' {
                        self.cursor -= 1;
                    }
                    while self.cursor > 0 && self.input[self.cursor - 1] != ' ' {
                        self.cursor -= 1;
                    }
                }
                Key::Right => {
                    while self.cursor < self.input.len() && self.input[self.cursor] == ' ' {
                        self.cursor += 1;
                    }
                    while self.cursor < self.input.len() && self.input[self.cursor] != ' ' {
                        self.cursor += 1;
                    }
                }
                _ => return false,
            }
            return true;
        }
        if self.shell.waiting() {
            // Mientras corre un comando, solo Ctrl+C y el desplazamiento.
            return match key {
                Key::PageUp => {
                    self.wheel(-5);
                    true
                }
                Key::PageDown => {
                    self.wheel(5);
                    true
                }
                _ => true,
            };
        }
        match key {
            Key::Enter => self.enter(ctx),
            Key::Char(c) => {
                self.input.insert(self.cursor, c);
                self.cursor += 1;
            }
            Key::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.input.remove(self.cursor);
            }
            Key::Delete if self.cursor < self.input.len() => {
                self.input.remove(self.cursor);
            }
            Key::Left => self.cursor = self.cursor.saturating_sub(1),
            Key::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = self.input.len(),
            Key::Tab => self.complete(ctx),
            Key::Up => {
                let h = &self.shell.history;
                if h.is_empty() {
                    return true;
                }
                if self.history_pos.is_none() {
                    self.draft = self.input_string();
                }
                let pos = self
                    .history_pos
                    .map_or(h.len() - 1, |p| p.saturating_sub(1));
                self.history_pos = Some(pos);
                self.input = h[pos].chars().collect();
                self.cursor = self.input.len();
            }
            Key::Down => {
                if let Some(p) = self.history_pos {
                    let h = &self.shell.history;
                    if p + 1 < h.len() {
                        self.history_pos = Some(p + 1);
                        self.input = h[p + 1].chars().collect();
                    } else {
                        self.history_pos = None;
                        self.input = self.draft.chars().collect();
                    }
                    self.cursor = self.input.len();
                }
            }
            Key::PageUp => self.wheel(-5),
            Key::PageDown => self.wheel(5),
            Key::Escape => {}
            _ => return false,
        }
        true
    }

    pub fn wheel(&mut self, delta: i32) {
        let max = self.lines.len();
        self.scroll = (self.scroll as i64 - delta as i64 * 3).clamp(0, max as i64) as usize;
        self.dirty = true;
    }

    /// Las líneas (con la del prompt al final) partidas al ancho de la ventana.
    fn rows(&self, cols: usize) -> (Vec<Line>, Option<(usize, usize)>) {
        let mut all: Vec<Line> = self.lines.clone();
        let mut last = self.partial.clone();
        let mut cursor = None;
        if !self.shell.waiting() {
            // El prompt (con colores) y lo escrito.
            let mut tmp = Terminal {
                dirty: false,
                shell: Shell::new("", ""),
                lines: Vec::new(),
                partial: Vec::new(),
                sgr: Sgr::default(),
                input: Vec::new(),
                cursor: 0,
                history_pos: None,
                draft: String::new(),
                scroll: 0,
                blink_on: false,
                last_blink: 0,
            };
            tmp.write(&self.shell.prompt());
            let prompt_len: usize = tmp.partial.iter().map(|(_, t)| t.chars().count()).sum();
            last.extend(tmp.partial);
            last.push((Sgr::default(), self.input_string()));
            let start: usize = self.partial.iter().map(|(_, t)| t.chars().count()).sum();
            cursor = Some(start + prompt_len + self.cursor);
        }
        all.push(last);
        let mut rows: Vec<Line> = Vec::new();
        let mut cursor_rc = None;
        let n = all.len();
        for (li, line) in all.into_iter().enumerate() {
            let mut row: Line = Vec::new();
            let mut col = 0;
            let mut abs = 0;
            for (sgr, t) in line {
                for ch in t.chars() {
                    if col == cols {
                        rows.push(core::mem::take(&mut row));
                        col = 0;
                    }
                    if li + 1 == n && cursor == Some(abs) {
                        cursor_rc = Some((rows.len(), col));
                    }
                    match row.last_mut() {
                        Some((s, text)) if *s == sgr => text.push(ch),
                        _ => row.push((sgr, ch.to_string())),
                    }
                    col += 1;
                    abs += 1;
                }
            }
            if li + 1 == n && cursor == Some(abs) {
                if col == cols {
                    rows.push(core::mem::take(&mut row));
                    col = 0;
                }
                cursor_rc = Some((rows.len(), col));
            }
            rows.push(row);
        }
        (rows, cursor_rc)
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect) {
        c.fill_rect(r.x, r.y, r.w, r.h, BG);
        let base = Style::new(Weight::Regular, Size::Size16, FG);
        let cw = text::width("M", &base).max(1);
        let lh = 19;
        let cols = ((r.w - 2 * PAD) / cw).max(10) as usize;
        let visible = ((r.h - 2 * PAD) / lh).max(1) as usize;
        self.shell.cols = cols;
        let (rows, cursor) = self.rows(cols);
        let end = rows
            .len()
            .saturating_sub(self.scroll.min(rows.len().saturating_sub(1)));
        let start = end.saturating_sub(visible);
        for (i, row) in rows[start..end].iter().enumerate() {
            let y = r.y + PAD + i as i32 * lh;
            let mut x = r.x + PAD;
            for (sgr, t) in row {
                let (st, bg) = style(*sgr);
                let w = t.chars().count() as i32 * cw;
                if let Some(bg) = bg {
                    c.fill_rect(x, y - 1, w, lh, bg);
                }
                text::draw(c, x, y, t, &st);
                x += w;
            }
        }
        if let Some((row, col)) = cursor
            && row >= start
            && row < end
            && self.blink_on
            && self.scroll == 0
        {
            let x = r.x + PAD + col as i32 * cw;
            let y = r.y + PAD + (row - start) as i32 * lh;
            c.fill_rect(x, y + lh - 4, cw, 2, FG);
            c.fill_rect(x, y, 2, lh - 2, FG.scale(120));
        }
        if self.scroll > 0 {
            let msg = crate::i18n::trf(
                "{} renglones más arriba · AvPág vuelve",
                &[&self.scroll.to_string()],
            );
            let st = Style::new(Weight::Regular, Size::Size16, Color::hex(0x6b7689));
            text::draw_right(c, r.x + r.w - PAD, r.y + PAD, &msg, &st);
        }
    }
}
