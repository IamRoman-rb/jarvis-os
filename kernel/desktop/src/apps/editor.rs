//! Editor de texto (como el Bloc de notas): abrir, editar y guardar archivos del disco.
//!
//! El texto se guarda como una lista de líneas y el cursor es (línea, columna) contando
//! caracteres, no bytes: así las tildes y la ñ ocupan una sola posición.
//!
//! **Selección**: un *ancla* (donde empezó) y el cursor (donde está ahora); lo seleccionado es lo
//! que queda entre los dos, en el orden que sea. Shift + cualquier movimiento deja el ancla
//! quieta y mueve el cursor, igual que arrastrar con el mouse. Ctrl+E (o Ctrl+A) selecciona todo;
//! Ctrl+C, Ctrl+X y Ctrl+V usan el portapapeles del sistema.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Rect, theme};

use super::{Click, Ctx};
use crate::files::{error_message, format_size};
use crate::i18n::{tr, trf};
use crate::input::{Key, Mods};
use crate::text_input::TextInput;
use crate::widgets::{draw_fit, field_bg, label, light, s16, window_bg};

const GUTTER: i32 = 56;
const STATUS_H: i32 = 30;
/// Letra del editor (Configuración → Tipografía).
fn text_style(color: jarvis_gfx::Color) -> jarvis_gfx::text::Style {
    jarvis_gfx::text::Style::new(
        jarvis_gfx::text::Weight::Regular,
        crate::look::editor_size(),
        color,
    )
}

/// Alto de un renglón.
fn line_h() -> i32 {
    text_style(jarvis_gfx::Color::WHITE).line_height() + 4
}

/// Ancho de un carácter (la fuente es monoespaciada).
fn char_w() -> i32 {
    jarvis_gfx::text::width("M", &text_style(jarvis_gfx::Color::WHITE)).max(1)
}
const MAX_FILE: usize = 512 * 1024;

pub struct Editor {
    pub dirty: bool,
    /// Ruta en el disco (None = documento nuevo, sin guardar todavía).
    pub path: Option<String>,
    lines: Vec<String>,
    row: usize,
    col: usize,
    top: usize,
    modified: bool,
    /// Se intentó cerrar con cambios sin guardar: el próximo intento cierra igual.
    close_warned: bool,
    /// "Guardar como": se está escribiendo la ruta.
    save_as: Option<TextInput>,
    status: Option<(String, bool, u64)>,
    /// Donde empezó la selección (línea, columna). `None` = no hay selección.
    anchor: Option<(usize, usize)>,
    /// Se está arrastrando con el mouse para seleccionar.
    dragging: bool,
}

type Pos = (usize, usize);

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Índice en bytes de la columna `col` (en caracteres).
fn byte_at(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map_or(s.len(), |(i, _)| i)
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            dirty: true,
            path: None,
            lines: alloc::vec![String::new()],
            row: 0,
            col: 0,
            top: 0,
            modified: false,
            close_warned: false,
            save_as: None,
            status: None,
            anchor: None,
            dragging: false,
        }
    }

    /// Abre `path`. Si no existe, queda un documento vacío que se va a guardar ahí.
    pub fn open<D: BlockDevice>(path: &str, ctx: &mut Ctx<'_, D>) -> Self {
        let mut ed = Editor::new();
        ed.path = Some(path.into());
        if let Some(fs) = ctx.fs.as_deref_mut() {
            match fs.read_prefix(path, MAX_FILE + 1) {
                Ok(bytes) if bytes.len() > MAX_FILE => {
                    ed.notice(
                        tr("El archivo es muy grande: se muestra el principio."),
                        true,
                        ctx.now_ms,
                    );
                    ed.load(&bytes[..MAX_FILE]);
                }
                Ok(bytes) => ed.load(&bytes),
                Err(jarvis_fs::FsError::NotFound) => {
                    ed.notice(tr("Archivo nuevo: Ctrl+S lo crea."), false, ctx.now_ms)
                }
                Err(e) => ed.notice(error_message(e), true, ctx.now_ms),
            }
        }
        ctx.log.push(format!("EDITOR_ABIERTO {path}"));
        ed
    }

    fn load(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        self.lines = text
            .split('\n')
            .map(|l| l.trim_end_matches('\r').replace('\t', "    "))
            .collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_modified(&self) -> bool {
        self.modified
    }

    fn notice(&mut self, text: impl Into<String>, error: bool, now_ms: u64) {
        self.status = Some((text.into(), error, now_ms + 4000));
        self.dirty = true;
    }

    pub fn title(&self) -> String {
        let name = self
            .path
            .as_deref()
            .map(|p| p.rsplit('/').next().unwrap_or(p))
            .unwrap_or(tr("Sin título"));
        trf(
            "{}{} · Editor",
            &[if self.modified { "* " } else { "" }, name],
        )
    }

    pub fn tick(&mut self, now_ms: u64) {
        if self.status.as_ref().is_some_and(|s| now_ms >= s.2) {
            self.status = None;
            self.dirty = true;
        }
    }

    fn visible_rows(content: Rect) -> usize {
        ((content.h - STATUS_H - 12) / line_h()).max(1) as usize
    }

    fn visible_cols(content: Rect) -> usize {
        ((content.w - GUTTER - 20) / char_w()).max(1) as usize
    }

    fn keep_cursor_visible(&mut self, content: Rect) {
        let rows = Self::visible_rows(content);
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + rows {
            self.top = self.row + 1 - rows;
        }
    }

    fn line_len(&self) -> usize {
        char_len(&self.lines[self.row])
    }

    /// Lo seleccionado, del principio al final (vacío = `None`).
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        let b = (self.row, self.col);
        match a.cmp(&b) {
            core::cmp::Ordering::Equal => None,
            core::cmp::Ordering::Less => Some((a, b)),
            core::cmp::Ordering::Greater => Some((b, a)),
        }
    }

    pub fn selected_text(&self) -> String {
        let Some(((r1, c1), (r2, c2))) = self.selection() else {
            return String::new();
        };
        if r1 == r2 {
            let l = &self.lines[r1];
            return l[byte_at(l, c1)..byte_at(l, c2)].into();
        }
        let mut out = String::from(&self.lines[r1][byte_at(&self.lines[r1], c1)..]);
        for l in &self.lines[r1 + 1..r2] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[r2][..byte_at(&self.lines[r2], c2)]);
        out
    }

    /// Borra lo seleccionado (el cursor queda donde empezaba). `false` si no había nada.
    fn delete_selection(&mut self) -> bool {
        let Some(((r1, c1), (r2, c2))) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let tail = self.lines[r2][byte_at(&self.lines[r2], c2)..].to_string();
        let head_end = byte_at(&self.lines[r1], c1);
        self.lines[r1].truncate(head_end);
        self.lines[r1].push_str(&tail);
        self.lines.drain(r1 + 1..=r2);
        (self.row, self.col) = (r1, c1);
        self.anchor = None;
        self.edit();
        true
    }

    /// Inserta texto que puede tener varias líneas (pegar).
    fn insert_text(&mut self, s: &str) {
        let mut parts = s.split('\n');
        if let Some(first) = parts.next() {
            self.insert(first.trim_end_matches('\r'));
        }
        for part in parts {
            let at = byte_at(&self.lines[self.row], self.col);
            let rest = self.lines[self.row].split_off(at);
            self.row += 1;
            self.lines.insert(self.row, rest);
            self.col = 0;
            self.insert(part.trim_end_matches('\r'));
        }
        self.edit();
    }

    pub fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        self.row = self.lines.len() - 1;
        self.col = self.line_len();
    }

    /// Mueve el cursor una palabra (Ctrl+← / Ctrl+→).
    fn word_move(&mut self, forward: bool) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        let mut c = self.col;
        if forward {
            if c >= chars.len() {
                if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
                return;
            }
            while c < chars.len() && chars[c].is_alphanumeric() {
                c += 1;
            }
            while c < chars.len() && !chars[c].is_alphanumeric() {
                c += 1;
            }
        } else {
            if c == 0 {
                if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len();
                }
                return;
            }
            while c > 0 && !chars[c - 1].is_alphanumeric() {
                c -= 1;
            }
            while c > 0 && chars[c - 1].is_alphanumeric() {
                c -= 1;
            }
        }
        self.col = c;
    }

    fn edit(&mut self) {
        self.modified = true;
        self.close_warned = false;
    }

    fn insert(&mut self, s: &str) {
        let at = byte_at(&self.lines[self.row], self.col);
        self.lines[self.row].insert_str(at, s);
        self.col += char_len(s);
        self.edit();
    }

    fn newline(&mut self) {
        let at = byte_at(&self.lines[self.row], self.col);
        let rest = self.lines[self.row].split_off(at);
        // Mantiene la sangría de la línea anterior.
        let indent: String = self.lines[self.row]
            .chars()
            .take_while(|c| *c == ' ')
            .collect();
        self.row += 1;
        self.lines.insert(self.row, format!("{indent}{rest}"));
        self.col = char_len(&indent);
        self.edit();
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let end = byte_at(line, self.col);
            let start = byte_at(line, self.col - 1);
            line.replace_range(start..end, "");
            self.col -= 1;
            self.edit();
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len();
            self.lines[self.row].push_str(&line);
            self.edit();
        }
    }

    fn delete(&mut self) {
        if self.col < self.line_len() {
            let line = &mut self.lines[self.row];
            let start = byte_at(line, self.col);
            let end = byte_at(line, self.col + 1);
            line.replace_range(start..end, "");
            self.edit();
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
            self.edit();
        }
    }

    fn save<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some(path) = self.path.clone() else {
            self.save_as = Some(TextInput::new("/Documentos/nota.txt", 120));
            self.dirty = true;
            return;
        };
        let now = ctx.timestamp();
        let Some(fs) = ctx.fs.as_deref_mut() else {
            self.notice(tr("No hay disco."), true, ctx.now_ms);
            return;
        };
        let text = self.text();
        match fs.write_file(&path, text.as_bytes(), now) {
            Ok(()) => {
                self.modified = false;
                ctx.log.push(format!("EDITOR_GUARDADO {path}"));
                self.notice(
                    format!("Guardado: {} ({}).", path, format_size(text.len() as u64)),
                    false,
                    ctx.now_ms,
                );
            }
            Err(e) => self.notice(error_message(e), true, ctx.now_ms),
        }
    }

    pub fn key<D: BlockDevice>(
        &mut self,
        key: Key,
        mods: Mods,
        content: Rect,
        ctx: &mut Ctx<'_, D>,
    ) -> bool {
        self.dirty = true;
        if let Some(input) = self.save_as.as_mut() {
            match key {
                Key::Escape => self.save_as = None,
                Key::Enter => {
                    let path = input.text.trim().to_string();
                    self.save_as = None;
                    if path.starts_with('/') {
                        self.path = Some(path);
                        self.save(ctx);
                    } else {
                        self.notice(tr("La ruta tiene que empezar con /"), true, ctx.now_ms);
                    }
                }
                other => {
                    input.handle(other);
                }
            }
            return true;
        }
        let moves = matches!(
            key,
            Key::Left
                | Key::Right
                | Key::Up
                | Key::Down
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
        );
        if moves {
            // Con Shift, el ancla queda donde estaba el cursor antes de moverse.
            if mods.shift {
                if self.anchor.is_none() {
                    self.anchor = Some((self.row, self.col));
                }
            } else {
                self.anchor = None;
            }
        }
        if mods.ctrl {
            match key {
                Key::Char('s' | 'S') => self.save(ctx),
                Key::Char('e' | 'E' | 'a' | 'A') => self.select_all(),
                Key::Char('c' | 'C') => {
                    if self.selection().is_some() {
                        *ctx.clipboard = self.selected_text();
                    }
                }
                Key::Char('x' | 'X') => {
                    if self.selection().is_some() {
                        *ctx.clipboard = self.selected_text();
                        self.delete_selection();
                    }
                }
                Key::Char('v' | 'V') => {
                    self.delete_selection();
                    let text = ctx.clipboard.clone();
                    self.insert_text(&text);
                }
                Key::Home => {
                    self.row = 0;
                    self.col = 0;
                }
                Key::End => {
                    self.row = self.lines.len() - 1;
                    self.col = self.line_len();
                }
                Key::Left => self.word_move(false),
                Key::Right => self.word_move(true),
                _ => return false,
            }
            self.keep_cursor_visible(content);
            return true;
        }
        let page = Self::visible_rows(content);
        match key {
            Key::Char(c) if !c.is_control() => {
                self.delete_selection();
                let mut buf = [0u8; 4];
                self.insert(c.encode_utf8(&mut buf));
            }
            Key::Tab => {
                self.delete_selection();
                self.insert("    ");
            }
            Key::Enter => {
                self.delete_selection();
                self.newline();
            }
            Key::Backspace => {
                if !self.delete_selection() {
                    self.backspace();
                }
            }
            Key::Delete => {
                if !self.delete_selection() {
                    self.delete();
                }
            }
            Key::Escape if self.anchor.is_some() => self.anchor = None,
            Key::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.line_len();
                }
            }
            Key::Right => {
                if self.col < self.line_len() {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            Key::Up => self.row = self.row.saturating_sub(1),
            Key::Down => self.row = (self.row + 1).min(self.lines.len() - 1),
            Key::PageUp => self.row = self.row.saturating_sub(page),
            Key::PageDown => self.row = (self.row + page).min(self.lines.len() - 1),
            Key::Home => self.col = 0,
            Key::End => self.col = self.line_len(),
            Key::Escape => return false,
            _ => return false,
        }
        self.col = self.col.min(self.line_len());
        self.keep_cursor_visible(content);
        true
    }

    /// (línea, columna) bajo un punto de la ventana.
    fn pos_at(&self, x: i32, y: i32, content: Rect) -> Pos {
        let row = (self.top as i32 + (y - content.y - 6).div_euclid(line_h()))
            .clamp(0, self.lines.len() as i32 - 1) as usize;
        let col = ((x - content.x - GUTTER + char_w() / 2) / char_w()).max(0) as usize;
        (row, col.min(char_len(&self.lines[row])))
    }

    pub fn click(&mut self, click: Click, content: Rect) {
        if click.y > content.y + content.h - STATUS_H {
            return;
        }
        let (row, col) = self.pos_at(click.x, click.y, content);
        (self.row, self.col) = (row, col);
        self.dirty = true;
        if click.double {
            // Doble clic: la palabra entera.
            self.word_move(false);
            let start = (self.row, self.col);
            (self.row, self.col) = (row, col);
            self.word_move(true);
            // (`word_move` saltea también los espacios de después: se vuelven a sacar.)
            let chars: Vec<char> = self.lines[self.row].chars().collect();
            while self.col > start.1
                && self.row == start.0
                && !chars[self.col - 1].is_alphanumeric()
            {
                self.col -= 1;
            }
            self.anchor = Some(start);
            self.dragging = false;
            return;
        }
        self.anchor = Some((row, col));
        self.dragging = !click.right;
    }

    /// Arrastrar con el botón apretado selecciona.
    pub fn pointer(&mut self, p: super::Pointer, content: Rect) {
        if !self.dragging {
            return;
        }
        match p.kind {
            super::PointerKind::Move => {
                let (row, col) = self.pos_at(p.x, p.y, content);
                if (row, col) != (self.row, self.col) {
                    (self.row, self.col) = (row, col);
                    self.keep_cursor_visible(content);
                    self.dirty = true;
                }
            }
            super::PointerKind::Up => {
                self.dragging = false;
                if self.selection().is_none() {
                    self.anchor = None;
                }
            }
        }
    }

    pub fn wheel(&mut self, delta: i32, content: Rect) {
        let max = self.lines.len().saturating_sub(Self::visible_rows(content));
        self.top = (self.top as i64 + delta as i64 * 3).clamp(0, max as i64) as usize;
        self.dirty = true;
    }

    /// Al cerrar con cambios sin guardar, la primera vez avisa (como el Bloc de notas).
    pub fn on_close<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) -> bool {
        if self.modified && !self.close_warned {
            self.close_warned = true;
            self.notice(
                tr("Hay cambios sin guardar: Ctrl+S guarda; cerrá de nuevo para descartarlos."),
                true,
                ctx.now_ms,
            );
            return false;
        }
        true
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, _now_ms: u64) {
        c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
        c.fill_rect(r.x, r.y, GUTTER - 8, r.h - STATUS_H, field_bg());
        let rows = Self::visible_rows(r);
        let cols = Self::visible_cols(r);
        // Desplazamiento horizontal: lo justo para que el cursor se vea.
        let left = self.col.saturating_sub(cols.saturating_sub(4));
        let text_st = text_style(theme::text());
        for i in 0..rows {
            let n = self.top + i;
            let Some(line) = self.lines.get(n) else { break };
            let y = r.y + 6 + i as i32 * line_h();
            let num_st = light(if n == self.row {
                theme::cyan()
            } else {
                theme::text_faint().lerp(theme::text_dim(), 140)
            });
            text::draw_right(c, r.x + GUTTER - 16, y, &format!("{}", n + 1), &num_st);
            if let Some(((r1, c1), (r2, c2))) = self.selection()
                && (r1..=r2).contains(&n)
            {
                let from = if n == r1 { c1 } else { 0 };
                // Hasta el final del renglón (y un poquito más, para que se vea el salto).
                let to = if n == r2 { c2 } else { char_len(line) + 1 };
                let (a, b) = (from.saturating_sub(left), to.saturating_sub(left).min(cols));
                if b > a {
                    c.fill_rect(
                        r.x + GUTTER + a as i32 * char_w(),
                        y - 2,
                        (b - a) as i32 * char_w(),
                        line_h(),
                        theme::selected().lerp(theme::cyan(), 50),
                    );
                }
            }
            let visible: String = line.chars().skip(left).take(cols).collect();
            text::draw(c, r.x + GUTTER, y, &visible, &text_st);
            if n == self.row && self.save_as.is_none() {
                let cx = r.x + GUTTER + (self.col - left) as i32 * char_w();
                c.fill_rect(cx, y - 1, 2, line_h() - 2, theme::cyan());
            }
        }

        // Barra de estado.
        let s = Rect::new(r.x, r.y + r.h - STATUS_H, r.w, STATUS_H);
        c.fill_rect(s.x, s.y, s.w, s.h, theme::panel());
        let ty = s.y + 7;
        match &self.status {
            Some((msg, error, _)) => {
                let col = if *error {
                    theme::amber()
                } else {
                    theme::cyan()
                };
                draw_fit(c, s.x + 12, ty, msg, &s16(col), s.w - 260);
            }
            None => {
                let path = self.path.as_deref().unwrap_or(tr("(sin guardar)"));
                draw_fit(c, s.x + 12, ty, path, &light(theme::text_dim()), s.w - 260);
            }
        }
        let pos = trf(
            "LÍN {} · COL {} · CTRL+S GUARDAR",
            &[&(self.row + 1).to_string(), &(self.col + 1).to_string()],
        );
        text::draw_right(c, s.x + s.w - 12, ty, &pos, &label(theme::text_dim()));

        if let Some(input) = &self.save_as {
            let d = Rect::new(r.x + (r.w - 460) / 2, r.y + 60, 460, 120);
            jarvis_gfx::shapes::rounded_rect(c, d.x, d.y, d.w, d.h, 8, theme::panel(), 255);
            jarvis_gfx::shapes::rounded_outline(c, d.x, d.y, d.w, d.h, 8, theme::cyan().scale(180));
            text::draw(
                c,
                d.x + 18,
                d.y + 16,
                tr("GUARDAR COMO"),
                &label(theme::cyan()),
            );
            let f = Rect::new(d.x + 18, d.y + 46, d.w - 36, 32);
            c.fill_rect(f.x, f.y, f.w, f.h, field_bg());
            let tw = draw_fit(c, f.x + 8, f.y + 8, &input.text, &text_st, f.w - 20);
            c.fill_rect(f.x + 10 + tw, f.y + 7, 2, 18, theme::cyan());
            text::draw(
                c,
                d.x + 18,
                d.y + 90,
                tr("Enter guarda · Esc cancela"),
                &light(theme::text_dim()),
            );
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edicion_con_tildes_y_lineas() {
        let mut e = Editor::new();
        e.load("hola\nmundo".as_bytes());
        e.row = 0;
        e.col = 4;
        e.insert(" ñandú");
        assert_eq!(e.lines[0], "hola ñandú");
        e.backspace();
        assert_eq!(e.lines[0], "hola ñand");
        e.newline();
        assert_eq!((e.row, e.col), (1, 0));
        e.backspace();
        assert_eq!(e.lines[0], "hola ñand");
        e.col = 0;
        e.row = 0;
        e.delete();
        assert_eq!(e.text(), "ola ñand\nmundo");
        e.col = e.line_len();
        e.delete(); // une con la línea siguiente
        assert_eq!(e.text(), "ola ñandmundo");
        assert!(e.is_modified());
    }

    #[test]
    fn seleccionar_copiar_y_reemplazar() {
        let mut e = Editor::new();
        e.load("uno dos\ntres cuatro\ncinco".as_bytes());
        (e.row, e.col) = (0, 4);
        e.anchor = Some((0, 4));
        (e.row, e.col) = (1, 4);
        assert_eq!(e.selected_text(), "dos\ntres");
        e.delete_selection();
        assert_eq!(e.text(), "uno  cuatro\ncinco");
        assert_eq!((e.row, e.col), (0, 4));
        e.insert_text("A\nB");
        assert_eq!(e.text(), "uno A\nB cuatro\ncinco");
        assert_eq!((e.row, e.col), (1, 1));
        e.select_all();
        assert_eq!(e.selected_text(), e.text());
        // Al revés (el ancla después del cursor) es lo mismo.
        e.anchor = Some((2, 3));
        (e.row, e.col) = (2, 1);
        assert_eq!(e.selected_text(), "in");
    }

    #[test]
    fn la_sangria_se_mantiene() {
        let mut e = Editor::new();
        e.load(b"    if x {");
        e.col = e.line_len();
        e.newline();
        assert_eq!(e.lines[1], "    ");
        assert_eq!(e.col, 4);
    }
}
