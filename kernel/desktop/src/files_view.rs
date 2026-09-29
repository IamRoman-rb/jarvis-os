//! Dibujo y clics de la ventana Archivos (según `design/stitch/…gestor_de_archivos…`).
//!
//! ```text
//! ┌─ (barra de título: la dibuja el gestor de ventanas) ────────────────────────────────────┐
//! │ ← ↑  [ /Documentos                        ]  +CARPETA  +ARCHIVO  RENOMBRAR  PAPELERA │
//! │ ACCESOS        │ NOMBRE                        TAMAÑO   MODIFICADO │ INSPECTOR          │
//! │ Inicio         │ ▸ Facultad                                        │ ícono, nombre,     │
//! │ Documentos     │   carpeta                              23/09 ...  │ tipo, tamaño,      │
//! │ …              │ ▸ notas.txt                   1.2 KiB             │ fechas, 8.3,       │
//! │ ALMACENAMIENTO │   texto                                           │ cluster y          │
//! │ [███░░] JARVIS │                                                   │ vista previa       │
//! ├────────────────┴───────────────────────────────────────────────────┴────────────────────┤
//! │ 5 elementos · 3.4 KiB        libre: 63.1 MiB        FAT32 · virtio-blk    ● SINCRONIZADO │
//! └──────────────────────────────────────────────────────────────────────────────────────────┘
//! ```

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::i18n::{tr, trf};
use jarvis_gfx::shapes::{circle, line, rect_outline, rounded_outline, rounded_rect};
use jarvis_gfx::text::{self, Size, Style, Weight};
use jarvis_gfx::{Canvas, Color, Rect, theme};

use crate::files::{
    Column, Dialog, FilesApp, Kind, PREVIEW_LINES, Preview, SHORTCUTS, format_date, format_size,
    join,
};

/// La barra de título ahora la dibuja el gestor de ventanas.
const TITLE_H: i32 = 0;
const TOOLBAR_H: i32 = 48;
const STATUS_H: i32 = 32;
const SIDEBAR_W: i32 = 210;
const INSPECTOR_W: i32 = 300;
pub const ROW_H: i32 = 44;
const LIST_HEADER_H: i32 = 30;
const SIDEBAR_ITEM_H: i32 = 34;
const DIALOG_W: i32 = 470;
const DIALOG_H: i32 = 190;

use crate::widgets::{field_bg, selected_bg, window_bg};

/// El panel lateral: entre el fondo de la ventana y el de los paneles.
fn sidebar_bg() -> Color {
    theme::window().lerp(theme::panel(), 110)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewFolder,
    NewFile,
    Rename,
    Delete,
    EmptyTrash,
    Restore,
    Paste,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Nothing,
    Back,
    Up,
    Action(Action),
    Shortcut(usize),
    Row(usize),
    Sort(Column),
    DialogAccept,
    DialogCancel,
}

pub struct Layout {
    pub window: Rect,
    toolbar: Rect,
    sidebar: Rect,
    list: Rect,
    inspector: Option<Rect>,
    status: Rect,
    back: Rect,
    up: Rect,
    path: Rect,
    buttons: Vec<(Action, &'static str, Rect)>,
}

fn s16(color: Color) -> Style {
    Style::new(Weight::Regular, Size::Size16, color)
}

fn label(color: Color) -> Style {
    Style::new(Weight::Regular, Size::Size16, color).tracking(2)
}

impl Layout {
    /// `window`: la zona del contenido de la ventana (el gestor de ventanas dibuja el marco y la
    /// barra de título alrededor).
    pub fn new(app: &FilesApp, window: Rect) -> Layout {
        let Rect { x, y, w, h } = window;
        let toolbar = Rect::new(x, y + TITLE_H, w, TOOLBAR_H);
        let body_y = y + TITLE_H + TOOLBAR_H;
        let body_h = h - TITLE_H - TOOLBAR_H - STATUS_H;
        let sidebar = Rect::new(x, body_y, SIDEBAR_W, body_h);
        let show_inspector = w >= SIDEBAR_W + INSPECTOR_W + 360;
        let inspector =
            show_inspector.then(|| Rect::new(x + w - INSPECTOR_W, body_y, INSPECTOR_W, body_h));
        let list_w = w - SIDEBAR_W - if show_inspector { INSPECTOR_W } else { 0 };
        let list = Rect::new(x + SIDEBAR_W, body_y, list_w, body_h);
        let status = Rect::new(x, y + h - STATUS_H, w, STATUS_H);

        let by = toolbar.y + 9;
        let back = Rect::new(x + 12, by, 32, 30);
        let up = Rect::new(x + 50, by, 32, 30);
        let mut specs: Vec<(Action, &'static str)> = Vec::new();
        if app.in_trash() {
            specs.push((Action::Restore, tr("RESTAURAR")));
            specs.push((Action::Delete, tr("BORRAR")));
            specs.push((Action::EmptyTrash, tr("VACIAR")));
        } else {
            specs.push((Action::NewFolder, tr("+ CARPETA")));
            specs.push((Action::NewFile, tr("+ ARCHIVO")));
            specs.push((Action::Rename, tr("RENOMBRAR")));
            if app.clipboard.is_some() {
                specs.push((Action::Paste, tr("PEGAR")));
            }
            specs.push((Action::Delete, tr("PAPELERA")));
        }
        let mut right = x + w - 12;
        let mut buttons = Vec::new();
        for (action, text) in specs.into_iter().rev() {
            let bw = text::width(text, &label(theme::text())) + 24;
            right -= bw;
            buttons.push((action, text, Rect::new(right, by, bw, 30)));
            right -= 8;
        }
        buttons.reverse();
        let path = Rect::new(x + 92, by, (right - 8) - (x + 92), 30);
        Layout {
            window,
            toolbar,
            sidebar,
            list,
            inspector,
            status,
            back,
            up,
            path,
            buttons,
        }
    }

    /// Filas de la lista que entran en pantalla.
    pub fn visible_rows(&self) -> usize {
        ((self.list.h - LIST_HEADER_H) / ROW_H).max(1) as usize
    }

    /// Fila visible número `visible_index` (0 = la primera que se ve).
    pub fn row_rect(&self, visible_index: usize) -> Rect {
        Rect::new(
            self.list.x,
            self.list.y + LIST_HEADER_H + visible_index as i32 * ROW_H,
            self.list.w,
            ROW_H,
        )
    }

    pub fn shortcut_rect(&self, i: usize) -> Rect {
        Rect::new(
            self.sidebar.x + 8,
            self.sidebar.y + 34 + i as i32 * SIDEBAR_ITEM_H,
            SIDEBAR_W - 16,
            SIDEBAR_ITEM_H - 4,
        )
    }

    /// Botón de la barra para `action`, si se muestra.
    pub fn action_rect(&self, action: Action) -> Option<Rect> {
        self.buttons
            .iter()
            .find(|(a, _, _)| *a == action)
            .map(|(_, _, r)| *r)
    }

    fn dialog_rect(&self) -> Rect {
        let w = &self.window;
        Rect::new(
            w.x + (w.w - DIALOG_W) / 2,
            w.y + (w.h - DIALOG_H) / 2,
            DIALOG_W,
            DIALOG_H,
        )
    }

    /// (cancelar, aceptar)
    pub fn dialog_buttons(&self) -> (Rect, Rect) {
        let d = self.dialog_rect();
        let accept = Rect::new(d.x + d.w - 20 - 130, d.y + d.h - 50, 130, 32);
        let cancel = Rect::new(accept.x - 12 - 130, accept.y, 130, 32);
        (cancel, accept)
    }

    /// Qué hay en (x, y).
    pub fn hit(&self, app: &FilesApp, x: i32, y: i32) -> Hit {
        if app.dialog.is_some() {
            let (cancel, accept) = self.dialog_buttons();
            return if accept.contains(x, y) {
                Hit::DialogAccept
            } else if cancel.contains(x, y) {
                Hit::DialogCancel
            } else {
                Hit::Nothing // el diálogo es modal: lo demás no responde
            };
        }
        if self.back.contains(x, y) {
            return Hit::Back;
        }
        if self.up.contains(x, y) {
            return Hit::Up;
        }
        if let Some((action, _, _)) = self.buttons.iter().find(|(_, _, r)| r.contains(x, y)) {
            return Hit::Action(*action);
        }
        if let Some(i) = (0..SHORTCUTS.len()).find(|&i| self.shortcut_rect(i).contains(x, y)) {
            return Hit::Shortcut(i);
        }
        let header = Rect::new(self.list.x, self.list.y, self.list.w, LIST_HEADER_H);
        if header.contains(x, y) {
            let size_x = self.list.x + self.list.w - 190;
            return Hit::Sort(if x < size_x - 110 {
                Column::Name
            } else if x < size_x + 20 {
                Column::Size
            } else {
                Column::Modified
            });
        }
        for v in 0..self.visible_rows() {
            let index = app.scroll + v;
            if index < app.entries.len() && self.row_rect(v).contains(x, y) {
                return Hit::Row(index);
            }
        }
        Hit::Nothing
    }
}

// --- íconos -----------------------------------------------------------------------------------

fn kind_color(kind: Kind) -> Color {
    match kind {
        Kind::Folder => theme::cyan(),
        Kind::Trash => theme::text_dim(),
        Kind::Text => theme::text(),
        Kind::Code => theme::amber(),
        Kind::Image => theme::particle_bright(),
        Kind::Archive => theme::amber(),
        Kind::Binary => theme::text_dim(),
    }
}

/// Ícono de 20×20 (× `s`) con la esquina superior izquierda en (x, y).
fn kind_icon(c: &mut Canvas<'_>, kind: Kind, x: i32, y: i32, s: i32) {
    let col = kind_color(kind);
    let p = |v: i32| v * s;
    match kind {
        Kind::Folder => {
            line(c, x, y + p(4), x + p(7), y + p(4), col);
            line(c, x + p(7), y + p(4), x + p(9), y + p(6), col);
            rounded_outline(c, x, y + p(6), p(20), p(12), 2, col);
        }
        Kind::Trash => {
            line(c, x + p(3), y + p(4), x + p(17), y + p(4), col);
            line(c, x + p(8), y + p(2), x + p(12), y + p(2), col);
            rect_outline(c, x + p(5), y + p(5), p(10), p(13), col);
            line(c, x + p(8), y + p(8), x + p(8), y + p(15), col);
            line(c, x + p(12), y + p(8), x + p(12), y + p(15), col);
        }
        Kind::Text | Kind::Binary | Kind::Code => {
            // Hoja con la esquina doblada.
            line(c, x + p(3), y + p(1), x + p(12), y + p(1), col);
            line(c, x + p(12), y + p(1), x + p(17), y + p(6), col);
            line(c, x + p(17), y + p(6), x + p(17), y + p(19), col);
            line(c, x + p(3), y + p(19), x + p(17), y + p(19), col);
            line(c, x + p(3), y + p(1), x + p(3), y + p(19), col);
            line(c, x + p(12), y + p(1), x + p(12), y + p(6), col);
            line(c, x + p(12), y + p(6), x + p(17), y + p(6), col);
            if kind == Kind::Code {
                line(c, x + p(8), y + p(10), x + p(6), y + p(13), col);
                line(c, x + p(6), y + p(13), x + p(8), y + p(16), col);
                line(c, x + p(12), y + p(10), x + p(14), y + p(13), col);
                line(c, x + p(14), y + p(13), x + p(12), y + p(16), col);
            } else if kind == Kind::Text {
                for row in [9, 12, 15] {
                    line(
                        c,
                        x + p(6),
                        y + p(row),
                        x + p(14),
                        y + p(row),
                        col.scale(170),
                    );
                }
            }
        }
        Kind::Image => {
            rounded_outline(c, x, y + p(2), p(20), p(16), 2, col);
            line(c, x + p(2), y + p(15), x + p(8), y + p(8), col);
            line(c, x + p(8), y + p(8), x + p(12), y + p(12), col);
            line(c, x + p(12), y + p(12), x + p(14), y + p(10), col);
            line(c, x + p(14), y + p(10), x + p(18), y + p(15), col);
            circle(c, x + p(14), y + p(6), p(2), col, false);
        }
        Kind::Archive => {
            rect_outline(c, x + p(2), y + p(4), p(16), p(14), col);
            line(c, x + p(2), y + p(8), x + p(18), y + p(8), col);
            line(c, x + p(10), y + p(4), x + p(10), y + p(18), col.scale(150));
        }
    }
}

fn arrow(c: &mut Canvas<'_>, r: Rect, up: bool, col: Color) {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    if up {
        line(c, cx, cy - 7, cx, cy + 7, col);
        line(c, cx, cy - 7, cx - 5, cy - 2, col);
        line(c, cx, cy - 7, cx + 5, cy - 2, col);
    } else {
        line(c, cx - 7, cy, cx + 7, cy, col);
        line(c, cx - 7, cy, cx - 2, cy - 5, col);
        line(c, cx - 7, cy, cx - 2, cy + 5, col);
    }
}

fn button(c: &mut Canvas<'_>, r: Rect, text_label: &str, color: Color, fill: u8) {
    rounded_rect(c, r.x, r.y, r.w, r.h, 4, color, fill);
    rounded_outline(c, r.x, r.y, r.w, r.h, 4, color.scale(200));
    let st = label(color);
    let tw = text::width(text_label, &st);
    text::draw(
        c,
        r.x + (r.w - tw) / 2,
        r.y + (r.h - 16) / 2,
        text_label,
        &st,
    );
}

// --- dibujo -----------------------------------------------------------------------------------

pub fn draw(c: &mut Canvas<'_>, app: &FilesApp, l: &Layout, has_disk: bool) {
    let w = l.window;
    c.fill_rect(w.x, w.y, w.w, w.h, window_bg());
    draw_toolbar(c, app, l);
    draw_sidebar(c, app, l);
    draw_list(c, app, l, has_disk);
    if let Some(r) = l.inspector {
        draw_inspector(c, app, r);
    }
    draw_status(c, app, l, has_disk);
    if let Some(dialog) = &app.dialog {
        draw_dialog(c, dialog, l);
    }
}

fn draw_toolbar(c: &mut Canvas<'_>, app: &FilesApp, l: &Layout) {
    for (r, up) in [(l.back, false), (l.up, true)] {
        rounded_rect(c, r.x, r.y, r.w, r.h, 4, theme::panel(), 255);
        rounded_outline(c, r.x, r.y, r.w, r.h, 4, theme::panel_rim());
        arrow(c, r, up, theme::text());
    }
    let p = l.path;
    rounded_rect(c, p.x, p.y, p.w, p.h, 4, field_bg(), 255);
    rounded_outline(c, p.x, p.y, p.w, p.h, 4, theme::panel_rim());
    let st = s16(theme::text_dim());
    text::draw(
        c,
        p.x + 10,
        p.y + 7,
        &text::fit(&app.cwd, &st, p.w - 20),
        &st,
    );
    for (action, text_label, r) in &l.buttons {
        let color = match action {
            Action::Restore | Action::Paste => theme::cyan(),
            Action::Delete if app.in_trash() => theme::crimson(),
            Action::EmptyTrash => theme::crimson(),
            Action::Delete => theme::amber(),
            _ => theme::cyan(),
        };
        button(c, *r, text_label, color, 30);
    }
    let t = l.toolbar;
    line(
        c,
        t.x + 1,
        t.y + t.h - 1,
        t.x + t.w - 2,
        t.y + t.h - 1,
        theme::panel_rim(),
    );
}

fn draw_sidebar(c: &mut Canvas<'_>, app: &FilesApp, l: &Layout) {
    let s = l.sidebar;
    c.fill_rect(s.x + 1, s.y, s.w - 1, s.h, sidebar_bg());
    line(
        c,
        s.x + s.w - 1,
        s.y,
        s.x + s.w - 1,
        s.y + s.h - 1,
        theme::panel_rim(),
    );
    text::draw(
        c,
        s.x + 16,
        s.y + 12,
        tr("ACCESOS"),
        &label(theme::text_dim()),
    );
    for (i, (name, path)) in SHORTCUTS.iter().enumerate() {
        let r = l.shortcut_rect(i);
        let current = app.cwd == *path;
        if current {
            rounded_rect(c, r.x, r.y, r.w, r.h, 4, selected_bg(), 255);
            c.fill_rect(r.x, r.y + 4, 3, r.h - 8, theme::cyan());
        }
        let kind = if *path == crate::files::TRASH {
            Kind::Trash
        } else {
            Kind::Folder
        };
        kind_icon(c, kind, r.x + 12, r.y + 5, 1);
        let col = if current {
            theme::cyan()
        } else {
            theme::text()
        };
        text::draw(c, r.x + 42, r.y + 7, tr(name), &s16(col));
    }

    // Almacenamiento: nombre, tipo y barra de uso.
    let y = l.shortcut_rect(SHORTCUTS.len()).y + 20;
    if y + 110 > s.y + s.h {
        return;
    }
    text::draw(
        c,
        s.x + 16,
        y,
        tr("ALMACENAMIENTO"),
        &label(theme::text_dim()),
    );
    let card = Rect::new(s.x + 10, y + 28, s.w - 20, 84);
    rounded_rect(c, card.x, card.y, card.w, card.h, 6, theme::panel(), 255);
    rounded_outline(c, card.x, card.y, card.w, card.h, 6, theme::panel_rim());
    text::draw(c, card.x + 12, card.y + 10, &app.label, &s16(theme::text()));
    let used = app.total_bytes.saturating_sub(app.free_bytes);
    let pct = (used * 100).checked_div(app.total_bytes).unwrap_or(0) as i32;
    text::draw_right(
        c,
        card.x + card.w - 12,
        card.y + 10,
        &format!("{pct}%"),
        &s16(theme::cyan()),
    );
    let bar = Rect::new(card.x + 12, card.y + 36, card.w - 24, 6);
    rounded_rect(c, bar.x, bar.y, bar.w, bar.h, 3, theme::panel_rim(), 255);
    let filled = (bar.w * pct / 100).max(if used > 0 { 3 } else { 0 });
    rounded_rect(c, bar.x, bar.y, filled, bar.h, 3, theme::cyan(), 255);
    let free = trf("{} libres", &[&format_size(app.free_bytes)]);
    text::draw(c, card.x + 12, card.y + 54, &free, &s16(theme::text_dim()));
}

fn draw_list(c: &mut Canvas<'_>, app: &FilesApp, l: &Layout, has_disk: bool) {
    let r = l.list;
    let size_x = r.x + r.w - 190;
    let date_x = r.x + r.w - 16;
    // El encabezado de la columna por la que se ordena va en celeste, con un triángulo.
    let (column, ascending) = app.sort;
    for (col, name, anchor) in [
        (Column::Name, tr("NOMBRE"), None),
        (Column::Size, tr("TAMAÑO"), Some(size_x)),
        (Column::Modified, tr("MODIFICADO"), Some(date_x)),
    ] {
        let active = col == column;
        let st = label(if active {
            theme::cyan()
        } else {
            theme::text_dim()
        });
        let (x0, w) = match anchor {
            None => (r.x + 20, text::draw(c, r.x + 20, r.y + 8, name, &st)),
            Some(right) => {
                let right = if active { right - 14 } else { right };
                let w = text::draw_right(c, right, r.y + 8, name, &st);
                (right - w, w)
            }
        };
        if active {
            sort_mark(c, x0 + w + 8, r.y + 15, ascending);
        }
    }
    line(
        c,
        r.x,
        r.y + LIST_HEADER_H - 1,
        r.x + r.w - 1,
        r.y + LIST_HEADER_H - 1,
        theme::panel_rim(),
    );

    if !has_disk || app.entries.is_empty() {
        let msg = if has_disk {
            tr("Carpeta vacía")
        } else {
            tr("No hay disco: arrancá QEMU con el disco virtual")
        };
        let st = s16(theme::text_dim());
        let tw = text::width(msg, &st);
        text::draw(c, r.x + (r.w - tw) / 2, r.y + r.h / 2 - 8, msg, &st);
        return;
    }
    let name_w = size_x - 110 - (r.x + 56);
    for v in 0..l.visible_rows() {
        let index = app.scroll + v;
        let Some(entry) = app.entries.get(index) else {
            break;
        };
        let row = l.row_rect(v);
        let selected = index == app.selected || app.is_marked(index);
        if selected {
            c.fill_rect(row.x + 1, row.y + 1, row.w - 2, row.h - 2, selected_bg());
            c.fill_rect(row.x + 1, row.y + 1, 3, row.h - 2, theme::cyan());
        }
        if app.is_marked(index) && index == app.selected && app.marked.len() > 1 {
            // El cursor, dentro de una selección de varios: un borde.
            jarvis_gfx::shapes::rect_outline(
                c,
                row.x + 1,
                row.y + 1,
                row.w - 2,
                row.h - 2,
                theme::cyan(),
            );
        }
        let path = join(&app.cwd, &entry.name);
        let kind = Kind::of(entry, &path);
        kind_icon(c, kind, row.x + 20, row.y + 12, 1);
        let name_col = if selected {
            theme::cyan()
        } else {
            theme::text()
        };
        let name_st = Style::new(Weight::Bold, Size::Size16, name_col);
        text::draw(
            c,
            row.x + 56,
            row.y + 5,
            &text::fit(&entry.name, &name_st, name_w),
            &name_st,
        );
        text::draw(
            c,
            row.x + 56,
            row.y + 23,
            kind.description(entry),
            &Style::new(Weight::Light, Size::Size16, theme::text_dim()),
        );
        if !entry.is_dir {
            text::draw_right(
                c,
                size_x,
                row.y + 14,
                &format_size(entry.size as u64),
                &s16(theme::text()),
            );
        }
        text::draw_right(
            c,
            date_x,
            row.y + 14,
            &format_date(entry.modified),
            &s16(theme::text_dim()),
        );
        line(
            c,
            row.x + 16,
            row.y + row.h - 1,
            row.x + row.w - 16,
            row.y + row.h - 1,
            theme::panel().lerp(theme::panel_rim(), 120),
        );
    }
    // Indicador de desplazamiento si no entra todo.
    let total = app.entries.len();
    let visible = l.visible_rows();
    if total > visible {
        let track = Rect::new(
            r.x + r.w - 5,
            r.y + LIST_HEADER_H + 4,
            3,
            r.h - LIST_HEADER_H - 8,
        );
        c.fill_rect(track.x, track.y, track.w, track.h, theme::panel_rim());
        let h = (track.h * visible as i32 / total as i32).max(12);
        let y = track.y + (track.h - h) * app.scroll as i32 / (total - visible) as i32;
        c.fill_rect(track.x, y, track.w, h, theme::cyan().scale(180));
    }
}

/// Triángulo de 7 px: hacia arriba (ascendente) o hacia abajo.
fn sort_mark(c: &mut Canvas<'_>, x: i32, y: i32, ascending: bool) {
    for i in 0..4 {
        let row = if ascending { y - 2 + i } else { y + 1 - i };
        line(c, x - i, row, x + i, row, theme::cyan());
    }
}

fn draw_inspector(c: &mut Canvas<'_>, app: &FilesApp, r: Rect) {
    line(c, r.x, r.y, r.x, r.y + r.h - 1, theme::panel_rim());
    text::draw(
        c,
        r.x + 18,
        r.y + 12,
        tr("INSPECTOR"),
        &label(theme::text_dim()),
    );
    let Some(entry) = app.selected_entry() else {
        text::draw(
            c,
            r.x + 18,
            r.y + 50,
            tr("Nada seleccionado"),
            &s16(theme::text_dim()),
        );
        return;
    };
    let path = join(&app.cwd, &entry.name);
    let kind = Kind::of(entry, &path);
    let card = Rect::new(r.x + 14, r.y + 40, r.w - 28, 96);
    rounded_rect(c, card.x, card.y, card.w, card.h, 6, theme::panel(), 255);
    rounded_outline(
        c,
        card.x,
        card.y,
        card.w,
        card.h,
        6,
        kind_color(kind).scale(120),
    );
    kind_icon(c, kind, card.x + card.w / 2 - 20, card.y + 12, 2);
    let name_st = Style::new(Weight::Bold, Size::Size16, theme::text());
    let name = text::fit(&entry.name, &name_st, card.w - 16);
    let nw = text::width(&name, &name_st);
    text::draw(c, card.x + (card.w - nw) / 2, card.y + 64, &name, &name_st);

    let size = if entry.is_dir {
        String::from("-")
    } else {
        format!("{} ({} bytes)", format_size(entry.size as u64), entry.size)
    };
    let rows: [(&str, String); 6] = [
        (tr("TIPO"), String::from(kind.description(entry))),
        (tr("TAMAÑO"), size),
        (tr("CREADO"), format_date(entry.created)),
        (tr("MODIFICADO"), format_date(entry.modified)),
        (tr("NOMBRE 8.3"), entry.short_name.clone()),
        ("CLUSTER", format!("{}", entry.first_cluster)),
    ];
    let mut y = card.y + card.h + 14;
    for (k, v) in rows {
        text::draw(
            c,
            r.x + 18,
            y,
            k,
            &Style::new(Weight::Light, Size::Size16, theme::text_dim()),
        );
        let st = s16(theme::text());
        text::draw_right(c, r.x + r.w - 18, y, &text::fit(&v, &st, r.w - 150), &st);
        y += 22;
    }

    y += 10;
    if y + 60 > r.y + r.h {
        return;
    }
    text::draw(
        c,
        r.x + 18,
        y,
        tr("VISTA PREVIA"),
        &label(theme::text_dim()),
    );
    let bx = Rect::new(r.x + 14, y + 24, r.w - 28, r.y + r.h - (y + 24) - 12);
    rounded_rect(c, bx.x, bx.y, bx.w, bx.h, 4, field_bg(), 255);
    rounded_outline(c, bx.x, bx.y, bx.w, bx.h, 4, theme::panel_rim());
    let st = Style::new(Weight::Light, Size::Size16, theme::text().scale(220));
    match &app.preview {
        Some(Preview::Text(lines)) => {
            let max_lines = ((bx.h - 12) / 18).max(0) as usize;
            for (i, l) in lines.iter().take(max_lines.min(PREVIEW_LINES)).enumerate() {
                text::draw(
                    c,
                    bx.x + 8,
                    bx.y + 6 + i as i32 * 18,
                    &text::fit(l, &st, bx.w - 16),
                    &st,
                );
            }
        }
        Some(Preview::Binary) => {
            text::draw(
                c,
                bx.x + 8,
                bx.y + 8,
                tr("Archivo binario: sin vista previa"),
                &st,
            );
        }
        None if entry.is_dir => {
            text::draw(
                c,
                bx.x + 8,
                bx.y + 8,
                tr("Enter para abrir la carpeta"),
                &st,
            );
        }
        None => {}
    }
}

fn draw_status(c: &mut Canvas<'_>, app: &FilesApp, l: &Layout, has_disk: bool) {
    let s = l.status;
    line(c, s.x + 1, s.y, s.x + s.w - 2, s.y, theme::panel_rim());
    let ty = s.y + 8;
    if let Some(toast) = &app.toast {
        let col = if toast.error {
            theme::crimson()
        } else {
            theme::cyan()
        };
        circle(c, s.x + 20, ty + 8, 4, col, true);
        text::draw(
            c,
            s.x + 32,
            ty,
            &text::fit(&toast.text, &s16(col), s.w - 64),
            &s16(col),
        );
        return;
    }
    let files: u64 = app
        .entries
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| e.size as u64)
        .sum();
    let left = if app.marked.len() > 1 {
        let size: u64 = app
            .marked
            .iter()
            .filter_map(|&i| app.entries.get(i))
            .filter(|e| !e.is_dir)
            .map(|e| e.size as u64)
            .sum();
        trf(
            "{} seleccionados de {} · {}",
            &[
                &app.marked.len().to_string(),
                &app.entries.len().to_string(),
                &format_size(size),
            ],
        )
    } else {
        trf(
            "{} elementos · {}",
            &[&app.entries.len().to_string(), &format_size(files)],
        )
    };
    text::draw(c, s.x + 16, ty, &left, &s16(theme::text()));
    let mid = trf("libre: {}", &[&format_size(app.free_bytes)]);
    text::draw(c, s.x + s.w / 3, ty, &mid, &s16(theme::text_dim()));
    let help = tr("CTRL+C/X/V · F2 RENOMBRAR · SUPR PAPELERA");
    let right = if has_disk {
        tr("SINCRONIZADO")
    } else {
        tr("SIN DISCO")
    };
    let rw = text::draw_right(
        c,
        s.x + s.w - 16,
        ty,
        right,
        &label(if has_disk {
            theme::cyan()
        } else {
            theme::amber()
        }),
    );
    circle(
        c,
        s.x + s.w - 28 - rw,
        ty + 8,
        4,
        if has_disk {
            theme::cyan()
        } else {
            theme::amber()
        },
        true,
    );
    let help_st = Style::new(
        Weight::Light,
        Size::Size16,
        theme::text_faint().lerp(theme::text_dim(), 150),
    );
    let help_right = s.x + s.w - 48 - rw;
    if help_right - text::width(help, &help_st)
        > s.x + s.w / 3 + text::width(&mid, &s16(theme::text())) + 24
    {
        text::draw_right(c, help_right, ty, help, &help_st);
    }
}

fn draw_dialog(c: &mut Canvas<'_>, dialog: &Dialog, l: &Layout) {
    // Oscurece la ventana: el diálogo es modal.
    let w = l.window;
    for y in w.y..w.y + w.h {
        for x in w.x..w.x + w.w {
            c.blend(x, y, Color::BLACK, 120);
        }
    }
    let d = l.dialog_rect();
    let accent = if dialog.is_destructive() {
        theme::crimson()
    } else {
        theme::cyan()
    };
    rounded_rect(c, d.x, d.y, d.w, d.h, 10, theme::menu(), 250);
    rounded_outline(c, d.x, d.y, d.w, d.h, 10, accent.scale(200));
    text::draw(c, d.x + 22, d.y + 18, dialog.title(), &label(accent));
    let msg_st = s16(theme::text());
    text::draw(
        c,
        d.x + 22,
        d.y + 52,
        &text::fit(&dialog.message(), &msg_st, d.w - 44),
        &msg_st,
    );
    if let Some(input) = dialog.input() {
        let f = Rect::new(d.x + 22, d.y + 78, d.w - 44, 34);
        rounded_rect(c, f.x, f.y, f.w, f.h, 4, field_bg(), 255);
        rounded_outline(c, f.x, f.y, f.w, f.h, 4, accent.scale(160));
        let shown = text::fit(&input.text, &msg_st, f.w - 30);
        let tw = text::draw(c, f.x + 10, f.y + 9, &shown, &msg_st);
        c.fill_rect(f.x + 12 + tw, f.y + 8, 2, 18, accent); // cursor
    }
    let (cancel, accept) = l.dialog_buttons();
    button(c, cancel, tr("CANCELAR"), theme::text_dim(), 20);
    button(c, accept, dialog.accept_label(), accent, 60);
}
