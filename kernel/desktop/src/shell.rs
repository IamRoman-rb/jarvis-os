//! Lo que está alrededor de las ventanas: la barra de arriba a la izquierda (lanzador y barra de
//! tareas a la vez), el panel de estado con gráficos, el menú de inicio, el selector de Alt+Tab,
//! la vista de tareas, la pantalla de bloqueo, el diálogo de apagado y los avisos.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::clock::{DateTime, StrBuf};
use jarvis_gfx::hud::{Icon, MARGIN, icon};
use jarvis_gfx::shapes::{circle, line, rounded_outline, rounded_rect};
use jarvis_gfx::text;
use jarvis_gfx::vfont::VectorText;
use jarvis_gfx::{Canvas, Color, Rect, theme};

use crate::apps::{icon_of, name_of};
use crate::i18n::{tr, trf};
use crate::system::{AppKind, History, SystemStats};
use crate::text_input::TextInput;
use crate::widgets::{FIELD_BG, SELECTED_BG, bar, button, draw_fit, graph, ip, label, light, s16};

// --- barra de íconos --------------------------------------------------------------------------

pub const SLOT: i32 = 30;
/// Cuántos íconos de ventanas extra (editores, visores) entran en la barra.
pub const MAX_EXTRA: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launcher {
    Start,
    App(AppKind),
    Capture,
    /// El chat: volver con JARVIS (mostrar el escritorio).
    Jarvis,
}

/// Los íconos fijos de la barra, en orden (los de la imagen de referencia + inicio y la web).
pub const LAUNCHERS: [Launcher; 10] = [
    Launcher::Start,
    Launcher::App(AppKind::Console),
    Launcher::App(AppKind::Terminal),
    Launcher::App(AppKind::Monitor),
    Launcher::Capture,
    Launcher::App(AppKind::Files),
    Launcher::App(AppKind::Music),
    Launcher::App(AppKind::Browser),
    Launcher::App(AppKind::Settings),
    Launcher::Jarvis,
];

pub fn launcher_icon(l: Launcher) -> Icon {
    match l {
        Launcher::Start => Icon::Start,
        Launcher::App(k) => icon_of(k),
        Launcher::Capture => Icon::Camera,
        Launcher::Jarvis => Icon::Chat,
    }
}

pub fn launcher_name(l: Launcher) -> &'static str {
    match l {
        Launcher::Start => tr("Inicio (Win)"),
        Launcher::App(AppKind::Console) => tr("Consola JARVIS (Win+R)"),
        Launcher::App(AppKind::Monitor) => tr("Monitor (Ctrl+Shift+Esc)"),
        Launcher::App(AppKind::Files) => tr("Archivos (Win+E)"),
        Launcher::App(AppKind::Terminal) => tr("Terminal (Ctrl+Alt+T)"),
        Launcher::App(AppKind::Settings) => tr("Configuración (Win+I)"),
        Launcher::App(k) => name_of(k),
        Launcher::Capture => tr("Captura (Impr Pant)"),
        Launcher::Jarvis => tr("JARVIS · escritorio (Win+D)"),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarItem {
    pub icon: Icon,
    pub name: String,
    /// Hay una ventana abierta de esta app (puntito abajo).
    pub running: bool,
    /// Es la app que tiene el foco (resaltada).
    pub active: bool,
}

/// Rectángulo de la barra con `n` íconos.
pub fn toolbar_rect(n: usize) -> Rect {
    let extra_gap = if n > LAUNCHERS.len() { 8 } else { 0 };
    Rect::new(MARGIN + 6, MARGIN + 2, SLOT * n as i32 + 18 + extra_gap, 38)
}

/// Toda la zona que puede ocupar la barra (con el máximo de íconos y la etiqueta de abajo).
pub fn toolbar_area() -> Rect {
    let r = toolbar_rect(LAUNCHERS.len() + MAX_EXTRA);
    Rect::new(r.x - 2, r.y - 2, r.w + 240, r.h + 34)
}

fn slot_x(i: usize) -> i32 {
    let r = toolbar_rect(1);
    let gap = if i >= LAUNCHERS.len() { 8 } else { 0 };
    r.x + 9 + i as i32 * SLOT + gap
}

pub fn toolbar_hit(n: usize, x: i32, y: i32) -> Option<usize> {
    let r = toolbar_rect(n);
    if !r.contains(x, y) {
        return None;
    }
    (0..n).find(|&i| (slot_x(i)..slot_x(i) + SLOT).contains(&x))
}

pub fn draw_toolbar(c: &mut Canvas<'_>, items: &[ToolbarItem], hover: Option<usize>) {
    let Rect { x, y, w, h } = toolbar_rect(items.len());
    rounded_rect(c, x, y, w, h, 12, theme::PANEL, 225);
    rounded_outline(c, x, y, w, h, 12, theme::PANEL_RIM);
    for (i, it) in items.iter().enumerate() {
        let (ix, iy) = (slot_x(i) + SLOT / 2, y + h / 2 - 2);
        if it.active || hover == Some(i) {
            let alpha = if it.active { 90 } else { 45 };
            rounded_rect(c, ix - 13, iy - 12, 26, 24, 6, theme::VECTOR_BLUE, alpha);
        }
        let color = if it.active || hover == Some(i) {
            theme::PARTICLE_BRIGHT
        } else {
            theme::TEXT_DIM
        };
        icon(c, it.icon, ix, iy, color);
        if it.running {
            let wdt = if it.active { 10 } else { 4 };
            rounded_rect(c, ix - wdt / 2, y + h - 5, wdt, 2, 1, theme::CYAN, 255);
        }
    }
    // Separador antes del chat de JARVIS y de las ventanas extra.
    let sx = slot_x(LAUNCHERS.len() - 1) - 1;
    line(c, sx, y + 9, sx, y + h - 10, theme::PANEL_RIM);
    if items.len() > LAUNCHERS.len() {
        let sx = slot_x(LAUNCHERS.len()) - 5;
        line(c, sx, y + 9, sx, y + h - 10, theme::PANEL_RIM);
    }
    // Nombre del ícono que tiene el mouse encima.
    if let Some(i) = hover
        && let Some(it) = items.get(i)
    {
        let st = s16(theme::TEXT);
        let tw = text::width(&it.name, &st) + 20;
        let tx = (slot_x(i) + SLOT / 2 - tw / 2).max(x);
        let ty = y + h + 6;
        rounded_rect(c, tx, ty, tw, 26, 6, Color::hex(0x0a1930), 240);
        rounded_outline(c, tx, ty, tw, 26, 6, theme::CYAN.scale(120));
        text::draw(c, tx + 10, ty + 5, &it.name, &st);
    }
}

// --- panel de estado --------------------------------------------------------------------------

const STATUS_W: i32 = 320;
const STATUS_ROW: i32 = 26;
const STATUS_ROWS: i32 = 6;

fn status_panel_rect(_w: usize, h: usize) -> Rect {
    let ph = 44 + STATUS_ROWS * STATUS_ROW + 8;
    Rect::new(MARGIN - 6, h as i32 - MARGIN - 44 - ph, STATUS_W, ph)
}

fn pill_rect(_w: usize, h: usize) -> Rect {
    Rect::new(MARGIN - 6, h as i32 - MARGIN - 30, 220, 30)
}

/// Zona del panel de estado y la píldora "Control de misión".
pub fn status_rect(w: usize, h: usize) -> Rect {
    let p = status_panel_rect(w, h);
    let pill = pill_rect(w, h);
    Rect::new(p.x, p.y, p.w, pill.y + pill.h - p.y)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusHit {
    Panel,
    MissionControl,
}

pub fn status_hit(w: usize, h: usize, x: i32, y: i32) -> Option<StatusHit> {
    if status_panel_rect(w, h).contains(x, y) {
        Some(StatusHit::Panel)
    } else if pill_rect(w, h).contains(x, y) {
        Some(StatusHit::MissionControl)
    } else {
        None
    }
}

/// Mini gráfico de barras (una por segundo) para el panel.
fn sparkline<const N: usize>(
    c: &mut Canvas<'_>,
    r: Rect,
    s: &crate::system::Series<N>,
    max: u32,
    col: Color,
) {
    let max = max.max(1) as i64;
    let values: Vec<u32> = s.iter().collect();
    let n = (r.w / 3).max(1) as usize;
    let start = values.len().saturating_sub(n);
    for (i, v) in values[start..].iter().enumerate() {
        let hh = ((*v as i64).min(max) * r.h as i64 / max).max(1) as i32;
        let x = r.x + r.w - (values.len() - start - i) as i32 * 3;
        c.fill_rect(x, r.y + r.h - hh, 2, hh, col);
    }
    line(c, r.x, r.y + r.h, r.x + r.w, r.y + r.h, theme::PANEL_RIM);
}

pub fn draw_status(
    c: &mut Canvas<'_>,
    w: usize,
    h: usize,
    st: &SystemStats,
    hist: &History,
    disk: Option<(u64, u64)>,
) {
    let p = status_panel_rect(w, h);
    rounded_rect(c, p.x, p.y, p.w, p.h, 8, theme::PANEL, 215);
    rounded_outline(c, p.x, p.y, p.w, p.h, 8, theme::PANEL_RIM);
    text::draw(c, p.x + 12, p.y + 10, tr("ESTADO"), &label(theme::TEXT_DIM));
    let hint = label(theme::TEXT_FAINT.lerp(theme::TEXT_DIM, 120));
    text::draw_right(c, p.x + p.w - 12, p.y + 10, tr("CLIC: MONITOR"), &hint);
    line(
        c,
        p.x + 1,
        p.y + 34,
        p.x + p.w - 2,
        p.y + 34,
        theme::PANEL_RIM,
    );

    let key = light(theme::TEXT_DIM);
    let value = s16(theme::TEXT);
    let row = |i: i32| p.y + 44 + i * STATUS_ROW;
    let gx = p.x + 112;
    let gw = 70;
    let right = p.x + p.w - 12;

    // CPU: gráfico + porcentaje.
    text::draw(c, p.x + 12, row(0), "CPU", &key);
    sparkline(
        c,
        Rect::new(gx, row(0), gw, 16),
        &hist.cpu,
        100,
        theme::CYAN,
    );
    text::draw_right(c, right, row(0), &format!("{} %", st.cpu_pct), &value);

    // Memoria: barra + usada/total.
    text::draw(c, p.x + 12, row(1), tr("MEMORIA"), &key);
    let mem_pct = (st.heap_used * 100).checked_div(st.heap_total).unwrap_or(0) as u32;
    bar(
        c,
        Rect::new(gx, row(1) + 6, gw, 5),
        mem_pct,
        theme::PARTICLE_BRIGHT,
    );
    let mem = format!("{} MiB", st.heap_used / (1024 * 1024));
    text::draw_right(c, right, row(1), &mem, &value);

    // Disco.
    text::draw(c, p.x + 12, row(2), tr("DISCO"), &key);
    match disk {
        Some((free, total)) => {
            let used = total.saturating_sub(free);
            let pct = (used * 100).checked_div(total).unwrap_or(0) as u32;
            bar(c, Rect::new(gx, row(2) + 6, gw, 5), pct, theme::AMBER);
            let free = trf("{} MiB libres", &[&(free / (1024 * 1024)).to_string()]);
            text::draw_right(c, right, row(2), &free, &value);
        }
        None => {
            text::draw_right(c, right, row(2), tr("sin disco"), &key);
        }
    }

    // Red: tráfico + dirección.
    text::draw(c, p.x + 12, row(3), tr("RED"), &key);
    let net_max = hist.rx.max().max(hist.tx.max()).max(1024);
    sparkline(
        c,
        Rect::new(gx, row(3), gw, 16),
        &hist.rx,
        net_max,
        theme::CYAN,
    );
    let net = match (st.net.present, st.net.ip) {
        (false, _) => String::from(tr("sin placa")),
        (true, None) => String::from("DHCP..."),
        (true, Some(a)) => ip(a),
    };
    text::draw_right(c, right, row(3), &net, &value);

    text::draw(c, p.x + 12, row(4), tr("RENDIMIENTO"), &key);
    let perf = format!("{} FPS · {} ms", st.fps, st.frame_ms);
    text::draw_right(c, right, row(4), &perf, &value);

    text::draw(c, p.x + 12, row(5), tr("CEREBRO"), &key);
    text::draw_right(c, right, row(5), tr("sin conectar"), &light(theme::AMBER));

    // Píldora "Control de misión" (abre la vista de tareas, como Win+Tab).
    let pill = pill_rect(w, h);
    rounded_rect(c, pill.x, pill.y, pill.w, pill.h, 15, theme::PANEL, 215);
    rounded_outline(c, pill.x, pill.y, pill.w, pill.h, 15, theme::PANEL_RIM);
    circle(c, pill.x + 16, pill.y + pill.h / 2, 3, theme::CYAN, true);
    text::draw(
        c,
        pill.x + 28,
        pill.y + 7,
        tr("CONTROL DE MISIÓN"),
        &label(theme::TEXT_DIM),
    );
}

// --- menú de inicio ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartItem {
    App(AppKind),
    /// Abrir la dirección o buscar el texto en la web.
    Web(String),
    Lock,
    Restart,
    Shutdown,
}

pub struct StartMenu {
    pub query: TextInput,
    pub selected: usize,
}

const MENU_APPS: [AppKind; 8] = [
    AppKind::Files,
    AppKind::Browser,
    AppKind::Terminal,
    AppKind::Settings,
    AppKind::Monitor,
    AppKind::Console,
    AppKind::Editor,
    AppKind::Music,
];
const MENU_ROW: i32 = 40;

impl Default for StartMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl StartMenu {
    pub fn new() -> Self {
        StartMenu {
            query: TextInput::new("", 80),
            selected: 0,
        }
    }

    /// Lo que se ve en la lista, según lo que se escribió.
    pub fn items(&self) -> Vec<(StartItem, String, Icon)> {
        let q = self.query.text.trim().to_lowercase();
        let mut out: Vec<(StartItem, String, Icon)> = MENU_APPS
            .iter()
            .filter(|k| q.is_empty() || name_of(**k).to_lowercase().contains(&q))
            .map(|k| (StartItem::App(*k), String::from(name_of(*k)), icon_of(*k)))
            .collect();
        if !q.is_empty() {
            let text = self.query.text.trim();
            let label = if text.contains('.') && !text.contains(' ') {
                trf("Abrir {}", &[&text])
            } else {
                trf("Buscar en la web: {}", &[&text])
            };
            out.push((StartItem::Web(text.into()), label, Icon::Globe));
        }
        out
    }

    pub fn rect(_w: usize, h: usize) -> Rect {
        let top = toolbar_rect(1).y + 46;
        Rect::new(MARGIN, top, 380, (h as i32 - top - 40).min(470))
    }

    fn row(menu: Rect, i: usize) -> Rect {
        Rect::new(
            menu.x + 10,
            menu.y + 64 + i as i32 * MENU_ROW,
            menu.w - 20,
            MENU_ROW - 4,
        )
    }

    fn power_buttons(menu: Rect) -> [(StartItem, &'static str, Rect); 3] {
        let y = menu.y + menu.h - 48;
        let bw = (menu.w - 40) / 3;
        let at = |i: i32| Rect::new(menu.x + 10 + i * (bw + 10), y, bw, 36);
        [
            (StartItem::Lock, tr("BLOQUEAR"), at(0)),
            (StartItem::Restart, tr("REINICIAR"), at(1)),
            (StartItem::Shutdown, tr("APAGAR"), at(2)),
        ]
    }

    pub fn hit(&self, w: usize, h: usize, x: i32, y: i32) -> Option<StartItem> {
        let menu = Self::rect(w, h);
        for (item, _, r) in Self::power_buttons(menu) {
            if r.contains(x, y) {
                return Some(item);
            }
        }
        self.items()
            .into_iter()
            .enumerate()
            .find(|(i, _)| Self::row(menu, *i).contains(x, y))
            .map(|(_, (item, _, _))| item)
    }

    pub fn draw(&self, c: &mut Canvas<'_>, w: usize, h: usize) {
        let m = Self::rect(w, h);
        rounded_rect(c, m.x, m.y, m.w, m.h, 12, Color::hex(0x081527), 248);
        rounded_outline(c, m.x, m.y, m.w, m.h, 12, theme::CYAN.scale(120));
        let f = Rect::new(m.x + 12, m.y + 14, m.w - 24, 36);
        rounded_rect(c, f.x, f.y, f.w, f.h, 6, FIELD_BG, 255);
        rounded_outline(c, f.x, f.y, f.w, f.h, 6, theme::PANEL_RIM);
        let st = s16(theme::TEXT);
        if self.query.text.is_empty() {
            text::draw(
                c,
                f.x + 18,
                f.y + 10,
                tr("Escribí para buscar apps o la web"),
                &light(theme::TEXT_DIM),
            );
            c.fill_rect(f.x + 11, f.y + 9, 2, 18, theme::CYAN);
        } else {
            let tw = draw_fit(c, f.x + 12, f.y + 10, &self.query.text, &st, f.w - 30);
            c.fill_rect(f.x + 14 + tw, f.y + 9, 2, 18, theme::CYAN);
        }
        for (i, (_, name, ic)) in self.items().iter().enumerate() {
            let r = Self::row(m, i);
            if r.y + r.h > m.y + m.h - 56 {
                break;
            }
            if i == self.selected {
                rounded_rect(c, r.x, r.y, r.w, r.h, 6, SELECTED_BG, 255);
                c.fill_rect(r.x, r.y + 6, 3, r.h - 12, theme::CYAN);
            }
            icon(c, *ic, r.x + 22, r.y + r.h / 2, theme::CYAN);
            draw_fit(c, r.x + 46, r.y + 10, name, &st, r.w - 56);
        }
        for (item, text_label, r) in Self::power_buttons(m) {
            let col = if item == StartItem::Shutdown {
                theme::CRIMSON
            } else {
                theme::TEXT_DIM
            };
            button(c, r, text_label, col, 25);
        }
    }
}

// --- Alt+Tab y vista de tareas ----------------------------------------------------------------

/// Una ventana en el selector: título, ícono y la imagen de su contenido.
pub struct Thumb<'a> {
    pub title: String,
    pub icon: Icon,
    pub image: Option<Canvas<'a>>,
}

const CARD_W: i32 = 200;
const CARD_H: i32 = 150;

pub fn switcher_rect(w: usize, h: usize, n: usize) -> Rect {
    let cols = n.clamp(1, 5) as i32;
    let rows = (n as i32 + 4) / 5;
    let pw = cols * (CARD_W + 12) + 28;
    let ph = rows.max(1) * (CARD_H + 12) + 52;
    Rect::new((w as i32 - pw) / 2, (h as i32 - ph) / 2, pw, ph)
}

fn card_rect(panel: Rect, i: usize) -> Rect {
    let (col, row) = ((i % 5) as i32, (i / 5) as i32);
    Rect::new(
        panel.x + 20 + col * (CARD_W + 12),
        panel.y + 44 + row * (CARD_H + 12),
        CARD_W,
        CARD_H,
    )
}

fn draw_card(c: &mut Canvas<'_>, r: Rect, t: &Thumb<'_>, selected: bool) {
    if selected {
        rounded_rect(
            c,
            r.x - 4,
            r.y - 4,
            r.w + 8,
            r.h + 8,
            8,
            theme::VECTOR_BLUE,
            80,
        );
        rounded_outline(c, r.x - 4, r.y - 4, r.w + 8, r.h + 8, 8, theme::CYAN);
    }
    let img = Rect::new(r.x, r.y, r.w, r.h - 30);
    c.fill_rect(img.x, img.y, img.w, img.h, theme::VOID);
    if let Some(src) = &t.image {
        // Miniatura sin deformar.
        let (sw, sh) = (src.width() as i32, src.height() as i32);
        let scale = (img.w * 1024 / sw.max(1)).min(img.h * 1024 / sh.max(1));
        let (tw, th) = (sw * scale / 1024, sh * scale / 1024);
        c.blit_scaled(
            src,
            Rect::new(img.x + (img.w - tw) / 2, img.y + (img.h - th) / 2, tw, th),
        );
    }
    icon(c, t.icon, r.x + 12, r.y + r.h - 14, theme::CYAN);
    draw_fit(
        c,
        r.x + 26,
        r.y + r.h - 22,
        &t.title,
        &s16(theme::TEXT),
        r.w - 30,
    );
}

pub fn draw_switcher(c: &mut Canvas<'_>, w: usize, h: usize, thumbs: &[Thumb<'_>], sel: usize) {
    let p = switcher_rect(w, h, thumbs.len());
    rounded_rect(c, p.x, p.y, p.w, p.h, 14, Color::hex(0x071222), 240);
    rounded_outline(c, p.x, p.y, p.w, p.h, 14, theme::PANEL_RIM);
    text::draw(
        c,
        p.x + 20,
        p.y + 14,
        tr("CAMBIAR DE VENTANA"),
        &label(theme::TEXT_DIM),
    );
    for (i, t) in thumbs.iter().enumerate() {
        draw_card(c, card_rect(p, i), t, i == sel);
    }
}

pub fn switcher_hit(w: usize, h: usize, n: usize, x: i32, y: i32) -> Option<usize> {
    let p = switcher_rect(w, h, n);
    (0..n).find(|&i| card_rect(p, i).contains(x, y))
}

/// Vista de tareas (Win+Tab, "Control de misión"): toda la pantalla oscurecida y las ventanas
/// abiertas como tarjetas grandes.
pub fn draw_task_view(c: &mut Canvas<'_>, w: usize, h: usize, thumbs: &[Thumb<'_>], sel: usize) {
    c.fill_rect(0, 0, w as i32, h as i32, Color::hex(0x040a14));
    text::draw(
        c,
        MARGIN + 8,
        MARGIN + 60,
        tr("CONTROL DE MISIÓN"),
        &label(theme::CYAN),
    );
    text::draw(
        c,
        MARGIN + 8,
        MARGIN + 84,
        tr("Clic o Enter: ir a la ventana · Supr: cerrarla · Esc: volver"),
        &light(theme::TEXT_DIM),
    );
    if thumbs.is_empty() {
        let st = s16(theme::TEXT_DIM);
        let msg = tr("No hay ventanas abiertas.");
        text::draw(
            c,
            (w as i32 - text::width(msg, &st)) / 2,
            h as i32 / 2,
            msg,
            &st,
        );
    }
    for (i, t) in thumbs.iter().enumerate() {
        let r = task_card(w, h, thumbs.len(), i);
        draw_card(c, r, t, i == sel);
    }
}

pub fn task_card(w: usize, h: usize, n: usize, i: usize) -> Rect {
    let cols = match n {
        0 | 1 => 1,
        2..=4 => 2,
        _ => 3,
    };
    let rows = ((n as i32 + cols - 1) / cols).max(1);
    let area = Rect::new(60, 140, w as i32 - 120, h as i32 - 200);
    let cw = ((area.w - (cols - 1) * 30) / cols).min(460);
    let ch = ((area.h - (rows - 1) * 30) / rows).min(cw * 3 / 4 + 30);
    let total_w = cols * cw + (cols - 1) * 30;
    let x0 = area.x + (area.w - total_w) / 2;
    let (col, row) = (i as i32 % cols, i as i32 / cols);
    Rect::new(x0 + col * (cw + 30), area.y + row * (ch + 30), cw, ch)
}

// --- bloqueo y apagado ------------------------------------------------------------------------

pub fn draw_lock(
    c: &mut Canvas<'_>,
    w: usize,
    h: usize,
    now: Option<DateTime>,
    big: &mut VectorText,
    h24: bool,
    // Con PIN: (cuántos dígitos se escribieron, el último estuvo mal).
    pin: Option<(usize, bool)>,
) {
    use core::fmt::Write;
    let (w, h) = (w as i32, h as i32);
    c.fill_rect(0, 0, w, h, theme::VOID);
    jarvis_gfx::shapes::glow(c, w / 2, h / 2, h / 2, Color::hex(0x0a2a66), 70);
    let mut time = StrBuf::<8>::new();
    let mut date = StrBuf::<48>::new();
    if let Some(t) = now {
        if h24 {
            let _ = t.write_time(&mut time);
        } else {
            let hour = if t.hour % 12 == 0 { 12 } else { t.hour % 12 };
            let _ = write!(time, "{hour}:{:02}", t.minute);
        }
        let _ = t.write_date(&mut date);
    }
    let tw = big.width(time.as_str());
    big.draw(c, (w - tw) / 2, h / 2 - 150, time.as_str(), Color::WHITE);
    let st = label(theme::CYAN.scale(200));
    let dw = text::width(date.as_str(), &st);
    text::draw(c, (w - dw) / 2, h / 2 + 10, date.as_str(), &st);
    let hint = match pin {
        None => tr("JARVIS-OS BLOQUEADO · TOCÁ UNA TECLA O HACÉ CLIC"),
        Some(_) => tr("JARVIS-OS BLOQUEADO · ESCRIBÍ TU PIN Y APRETÁ ENTER"),
    };
    let st = label(theme::TEXT_DIM);
    text::draw(c, (w - text::width(hint, &st)) / 2, h - 90, hint, &st);
    if let Some((n, wrong)) = pin {
        let f = Rect::new((w - 260) / 2, h / 2 + 60, 260, 44);
        rounded_rect(c, f.x, f.y, f.w, f.h, 8, FIELD_BG, 255);
        let rim = if wrong { theme::AMBER } else { theme::CYAN };
        rounded_outline(c, f.x, f.y, f.w, f.h, 8, rim);
        for i in 0..n as i32 {
            circle(c, f.x + 24 + i * 30, f.y + f.h / 2, 6, theme::TEXT, true);
        }
        if n == 0 {
            let st = light(theme::TEXT_DIM);
            text::draw(c, f.x + 16, f.y + 13, "PIN", &st);
        }
        if wrong {
            let msg = tr("PIN INCORRECTO. PROBÁ OTRA VEZ.");
            let st = label(theme::AMBER);
            text::draw(c, (w - text::width(msg, &st)) / 2, f.y + f.h + 16, msg, &st);
        }
    }
}

pub fn power_rect(w: usize, h: usize) -> Rect {
    Rect::new((w as i32 - 480) / 2, (h as i32 - 200) / 2, 480, 200)
}

/// Botones del diálogo de apagado: (apagar, reiniciar, cancelar).
pub fn power_buttons(w: usize, h: usize) -> [Rect; 3] {
    let d = power_rect(w, h);
    let bw = 136;
    let y = d.y + d.h - 56;
    [
        Rect::new(d.x + 20, y, bw, 36),
        Rect::new(d.x + 20 + bw + 16, y, bw, 36),
        Rect::new(d.x + d.w - 20 - bw, y, bw, 36),
    ]
}

pub fn draw_power(c: &mut Canvas<'_>, w: usize, h: usize, sel: usize) {
    c.fill_rect(0, 0, w as i32, h as i32, Color::hex(0x03070e));
    let d = power_rect(w, h);
    rounded_rect(c, d.x, d.y, d.w, d.h, 12, Color::hex(0x0a1930), 255);
    rounded_outline(c, d.x, d.y, d.w, d.h, 12, theme::CRIMSON.scale(180));
    icon(c, Icon::Power, d.x + 34, d.y + 36, theme::CRIMSON);
    text::draw(
        c,
        d.x + 58,
        d.y + 28,
        tr("APAGAR JARVIS-OS"),
        &label(theme::CRIMSON),
    );
    text::draw(
        c,
        d.x + 24,
        d.y + 72,
        tr("¿Qué querés que haga la computadora?"),
        &s16(theme::TEXT),
    );
    let labels = [tr("APAGAR"), tr("REINICIAR"), tr("CANCELAR")];
    for (i, r) in power_buttons(w, h).into_iter().enumerate() {
        let col = match i {
            0 => theme::CRIMSON,
            1 => theme::AMBER,
            _ => theme::TEXT_DIM,
        };
        button(c, r, labels[i], col, if i == sel { 90 } else { 20 });
        if i == sel {
            rounded_outline(c, r.x - 3, r.y - 3, r.w + 6, r.h + 6, 6, col);
        }
    }
}

// --- avisos -----------------------------------------------------------------------------------

pub const TOAST_H: i32 = 44;

/// Zona de los avisos (arriba del mensaje de JARVIS, abajo a la derecha).
pub fn toasts_rect(w: usize, h: usize) -> Rect {
    let right = w as i32 - MARGIN - 4;
    let bottom = h as i32 - MARGIN - 70;
    Rect::new(
        right - 440,
        bottom - 3 * (TOAST_H + 8),
        444,
        3 * (TOAST_H + 8),
    )
}

pub fn draw_toasts(c: &mut Canvas<'_>, w: usize, h: usize, toasts: &[(String, bool)]) {
    let area = toasts_rect(w, h);
    for (i, (msg, error)) in toasts.iter().rev().take(3).enumerate() {
        let y = area.y + area.h - (i as i32 + 1) * (TOAST_H + 8);
        let r = Rect::new(area.x + 4, y, area.w - 4, TOAST_H);
        let col = if *error { theme::AMBER } else { theme::CYAN };
        rounded_rect(c, r.x, r.y, r.w, r.h, 8, Color::hex(0x0a1930), 245);
        rounded_outline(c, r.x, r.y, r.w, r.h, 8, col.scale(170));
        c.fill_rect(r.x + 1, r.y + 8, 3, r.h - 16, col);
        draw_fit(c, r.x + 16, r.y + 13, msg, &s16(theme::TEXT), r.w - 28);
    }
}

/// Dibujo de un gráfico grande reutilizable (para quien lo necesite fuera del monitor).
pub fn big_graph<const N: usize>(
    c: &mut Canvas<'_>,
    r: Rect,
    s: &crate::system::Series<N>,
    col: Color,
) {
    graph(c, r, s, 0, col);
}
