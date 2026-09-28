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
use crate::system::{AppKind, HISTORY, History, Series, SystemStats};
use crate::text_input::TextInput;
use crate::widgets::{bar, button, draw_fit, field_bg, graph, ip, label, light, s16, selected_bg};

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
    Launcher::App(AppKind::Brave),
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
    rounded_rect(c, x, y, w, h, 12, theme::panel(), 225);
    rounded_outline(c, x, y, w, h, 12, theme::panel_rim());
    for (i, it) in items.iter().enumerate() {
        let (ix, iy) = (slot_x(i) + SLOT / 2, y + h / 2 - 2);
        if it.active || hover == Some(i) {
            let alpha = if it.active { 90 } else { 45 };
            rounded_rect(c, ix - 13, iy - 12, 26, 24, 6, theme::vector_blue(), alpha);
        }
        let color = if it.active || hover == Some(i) {
            theme::particle_bright()
        } else {
            theme::text_dim()
        };
        icon(c, it.icon, ix, iy, color);
        if it.running {
            let wdt = if it.active { 10 } else { 4 };
            rounded_rect(c, ix - wdt / 2, y + h - 5, wdt, 2, 1, theme::cyan(), 255);
        }
    }
    // Separador antes del chat de JARVIS y de las ventanas extra.
    let sx = slot_x(LAUNCHERS.len() - 1) - 1;
    line(c, sx, y + 9, sx, y + h - 10, theme::panel_rim());
    if items.len() > LAUNCHERS.len() {
        let sx = slot_x(LAUNCHERS.len()) - 5;
        line(c, sx, y + 9, sx, y + h - 10, theme::panel_rim());
    }
    // Nombre del ícono que tiene el mouse encima.
    if let Some(i) = hover
        && let Some(it) = items.get(i)
    {
        let st = s16(theme::text());
        let tw = text::width(&it.name, &st) + 20;
        let tx = (slot_x(i) + SLOT / 2 - tw / 2).max(x);
        let ty = y + h + 6;
        rounded_rect(c, tx, ty, tw, 26, 6, theme::menu(), 240);
        rounded_outline(c, tx, ty, tw, 26, 6, theme::cyan().scale(120));
        text::draw(c, tx + 10, ty + 5, &it.name, &st);
    }
}

// --- barra de arriba (cuando hay una ventana maximizada) --------------------------------------

pub use crate::wm::TOPBAR_H;

const TOP_SLOT: i32 = 28;
const TOP_CHIP: i32 = 170;
const TOP_IP: i32 = 124;
const TOP_CLOCK: i32 = 92;

/// Una ventana abierta, como se muestra en la barra de arriba.
#[derive(Clone, Debug, PartialEq)]
pub struct TopWindow {
    pub icon: Icon,
    pub title: String,
    pub active: bool,
    pub minimized: bool,
}

/// Dónde se hizo clic en la barra de arriba.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopHit {
    /// Uno de los íconos fijos (el mismo índice que en `LAUNCHERS`).
    Launcher(usize),
    /// Una de las ventanas abiertas.
    Window(usize),
    /// Los gráficos de estado o la IP (abre el Monitor).
    Status,
    Clock,
    /// El botón de energía.
    Power,
}

pub fn topbar_rect(w: usize) -> Rect {
    Rect::new(0, 0, w as i32, TOPBAR_H)
}

fn top_launcher_x(i: usize) -> i32 {
    6 + i as i32 * TOP_SLOT
}

/// Ancho de cada gráfico de estado: con el valor al lado si la pantalla es ancha.
fn top_stat_w(w: usize) -> i32 {
    if w >= 1200 { 112 } else { 76 }
}

/// Cuántos gráficos de estado hay en la barra de arriba (se eligen en la Configuración).
fn top_stats_n() -> i32 {
    crate::look::top_stats().count_ones() as i32
}

/// "45°C", o "--" sin sensor.
pub fn temp_text(t: Option<u8>) -> String {
    match t {
        Some(c) => format!("{c}\u{b0}C"),
        None => String::from("--"),
    }
}

/// El botón de energía, en la punta derecha.
const TOP_POWER: i32 = 32;

fn top_power_rect(w: usize) -> Rect {
    Rect::new(w as i32 - TOP_POWER, 0, TOP_POWER, TOPBAR_H)
}

fn top_clock_rect(w: usize) -> Rect {
    Rect::new(w as i32 - TOP_POWER - TOP_CLOCK, 0, TOP_CLOCK, TOPBAR_H)
}

fn top_ip_rect(w: usize) -> Rect {
    Rect::new(
        w as i32 - TOP_POWER - TOP_CLOCK - TOP_IP,
        0,
        TOP_IP,
        TOPBAR_H,
    )
}

fn top_stats_rect(w: usize) -> Rect {
    let sw = top_stat_w(w) * top_stats_n();
    Rect::new(top_ip_rect(w).x - sw, 0, sw, TOPBAR_H)
}

/// Lugar de la ventana `i` de `n` en la barra (vacío si no entra ninguna).
pub fn top_chip(w: usize, n: usize, i: usize) -> Rect {
    let x0 = top_launcher_x(LAUNCHERS.len()) + 8;
    let avail = top_stats_rect(w).x - 8 - x0;
    let cw = TOP_CHIP.min(avail / n.max(1) as i32);
    if cw < 34 {
        return Rect::new(x0, 0, 0, 0);
    }
    Rect::new(x0 + i as i32 * cw, 3, cw - 4, TOPBAR_H - 6)
}

pub fn topbar_hit(w: usize, windows: usize, x: i32, y: i32) -> Option<TopHit> {
    if !topbar_rect(w).contains(x, y) {
        return None;
    }
    if let Some(i) = (0..LAUNCHERS.len())
        .find(|&i| (top_launcher_x(i)..top_launcher_x(i) + TOP_SLOT).contains(&x))
    {
        return Some(TopHit::Launcher(i));
    }
    if let Some(i) = (0..windows).find(|&i| {
        let r = top_chip(w, windows, i);
        !r.is_empty() && (r.x..r.x + r.w).contains(&x)
    }) {
        return Some(TopHit::Window(i));
    }
    if top_stats_rect(w).contains(x, y) || top_ip_rect(w).contains(x, y) {
        return Some(TopHit::Status);
    }
    if top_clock_rect(w).contains(x, y) {
        return Some(TopHit::Clock);
    }
    if top_power_rect(w).contains(x, y) {
        return Some(TopHit::Power);
    }
    None
}

/// "0K", "12K", "3M": bytes por segundo en poco espacio.
fn short_rate(b: u32) -> String {
    if b >= 1024 * 1024 {
        format!("{}M", b / (1024 * 1024))
    } else {
        format!("{}K", b.div_ceil(1024))
    }
}

/// Todo lo que muestra la barra de arriba.
pub struct TopBar<'a> {
    pub launchers: &'a [ToolbarItem],
    pub windows: &'a [TopWindow],
    pub hover: Option<TopHit>,
    pub clock: &'a str,
    pub stats: &'a SystemStats,
    pub history: &'a History,
    /// (libre, total) del disco.
    pub disk: Option<(u64, u64)>,
}

pub fn draw_topbar(c: &mut Canvas<'_>, w: usize, bar_: &TopBar<'_>) {
    let r = topbar_rect(w);
    c.fill_rect(r.x, r.y, r.w, r.h, theme::panel());
    line(c, 0, r.h - 1, r.w, r.h - 1, theme::panel_rim());
    let cy = r.h / 2;

    // Íconos fijos.
    for (i, it) in bar_.launchers.iter().take(LAUNCHERS.len()).enumerate() {
        let ix = top_launcher_x(i) + TOP_SLOT / 2;
        let hover = bar_.hover == Some(TopHit::Launcher(i));
        if it.active || hover {
            let alpha = if it.active { 90 } else { 45 };
            rounded_rect(c, ix - 12, 3, 24, r.h - 6, 5, theme::vector_blue(), alpha);
        }
        let color = if it.active || hover {
            theme::particle_bright()
        } else {
            theme::text_dim()
        };
        icon(c, it.icon, ix, cy - 1, color);
        if it.running {
            rounded_rect(c, ix - 3, r.h - 4, 6, 2, 1, theme::cyan(), 255);
        }
    }
    let sx = top_launcher_x(LAUNCHERS.len()) + 3;
    line(c, sx, 6, sx, r.h - 7, theme::panel_rim());

    // Ventanas abiertas.
    let n = bar_.windows.len();
    for (i, win) in bar_.windows.iter().enumerate() {
        let chip = top_chip(w, n, i);
        if chip.is_empty() {
            break;
        }
        let hover = bar_.hover == Some(TopHit::Window(i));
        let (bg, alpha) = if win.active {
            (theme::vector_blue(), 90)
        } else if hover {
            (theme::vector_blue(), 45)
        } else {
            (theme::panel_rim(), 110)
        };
        rounded_rect(c, chip.x, chip.y, chip.w, chip.h, 5, bg, alpha);
        let color = if win.minimized {
            theme::text_faint()
        } else if win.active {
            theme::particle_bright()
        } else {
            theme::text_dim()
        };
        icon(c, win.icon, chip.x + 14, cy - 1, color);
        if chip.w > 44 {
            let st = s16(if win.minimized {
                theme::text_faint()
            } else {
                theme::text()
            });
            draw_fit(c, chip.x + 28, cy - 8, &win.title, &st, chip.w - 34);
        }
        if win.active {
            rounded_rect(
                c,
                chip.x + 8,
                chip.y + chip.h - 2,
                chip.w - 16,
                2,
                1,
                theme::cyan(),
                255,
            );
        }
    }

    // Gráficos de estado: CPU, memoria, disco y red.
    let sr = top_stats_rect(w);
    let sw = top_stat_w(w);
    let wide = sw > 100;
    let key = light(theme::text_dim());
    let value = s16(theme::text());
    let mem_pct = (bar_.stats.heap_used * 100)
        .checked_div(bar_.stats.heap_total)
        .unwrap_or(0) as u32;
    let disk_pct = bar_
        .disk
        .map(|(free, total)| {
            (total.saturating_sub(free) * 100)
                .checked_div(total)
                .unwrap_or(0) as u32
        })
        .unwrap_or(0);
    let h = bar_.history;
    let disk_max = h.disk.max().max(64 * 1024);
    let net_max = h.rx.max().max(h.tx.max()).max(1024);
    let rx = h.rx.last().unwrap_or(0) + h.tx.last().unwrap_or(0);
    let all: [(&str, &Series<HISTORY>, u32, Color, String); 5] = [
        (
            "CPU",
            &h.cpu,
            100,
            theme::cyan(),
            format!("{}%", bar_.stats.cpu_pct),
        ),
        (
            tr("MEM"),
            &h.mem,
            100,
            theme::particle_bright(),
            format!("{mem_pct}%"),
        ),
        (
            tr("DISCO"),
            &h.disk,
            disk_max,
            theme::amber(),
            format!("{disk_pct}%"),
        ),
        (tr("RED"), &h.rx, net_max, theme::cyan(), short_rate(rx)),
        (
            "TEMP",
            &h.temp,
            100,
            theme::crimson(),
            temp_text(bar_.stats.temp_c),
        ),
    ];
    let mask = crate::look::top_stats();
    let stats = all
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, s)| s);
    for (i, (name, series, max, color, val)) in stats.enumerate() {
        let x = sr.x + i as i32 * sw;
        line(c, x, 6, x, r.h - 7, theme::panel_rim());
        text::draw(c, x + 8, cy - 8, name, &key);
        let gx = x + 8 + text::width(name, &key) + 6;
        let gw = if wide { 30 } else { (x + sw - 6 - gx).max(12) };
        sparkline(c, Rect::new(gx, 6, gw, r.h - 13), series, *max, *color);
        if wide {
            text::draw_right(c, x + sw - 6, cy - 8, val, &value);
        }
    }

    // IP.
    let ipr = top_ip_rect(w);
    line(c, ipr.x, 6, ipr.x, r.h - 7, theme::panel_rim());
    let (net, net_color) = match (bar_.stats.net.present, bar_.stats.net.ip) {
        (false, _) => (String::from(tr("sin placa")), theme::text_faint()),
        (true, None) => (String::from("DHCP..."), theme::amber()),
        (true, Some(a)) => (ip(a), theme::text()),
    };
    let st = s16(net_color);
    let tw = text::width(&net, &st);
    text::draw(c, ipr.x + (ipr.w - tw) / 2, cy - 8, &net, &st);

    // Hora.
    let cr = top_clock_rect(w);
    line(c, cr.x, 6, cr.x, r.h - 7, theme::panel_rim());
    let color = if bar_.hover == Some(TopHit::Clock) {
        theme::particle_bright()
    } else {
        theme::cyan()
    };
    let st = crate::widgets::bold(color);
    let tw = text::width(bar_.clock, &st);
    text::draw(c, cr.x + (cr.w - tw) / 2, cy - 8, bar_.clock, &st);

    // Energía: suspender, cerrar sesión, reiniciar, apagar.
    let pr = top_power_rect(w);
    line(c, pr.x, 6, pr.x, r.h - 7, theme::panel_rim());
    let color = if bar_.hover == Some(TopHit::Power) {
        theme::crimson()
    } else {
        theme::text_dim()
    };
    icon(c, Icon::Power, pr.x + pr.w / 2, cy - 1, color);
}

/// "23:05", o "11:05 PM" con el reloj de 12 horas; con `secs`, "23:05:09".
pub fn clock_text(clock: Option<DateTime>, h24: bool, secs: bool) -> String {
    let s = |t: &DateTime| {
        if secs {
            format!(":{:02}", t.second)
        } else {
            String::new()
        }
    };
    match clock {
        None => String::from("--:--"),
        Some(t) if h24 => format!("{:02}:{:02}{}", t.hour, t.minute, s(&t)),
        Some(t) => {
            let h = match t.hour % 12 {
                0 => 12,
                h => h,
            };
            let ampm = if t.hour < 12 { "AM" } else { "PM" };
            format!("{h}:{:02}{} {ampm}", t.minute, s(&t))
        }
    }
}

// --- panel de estado --------------------------------------------------------------------------

const STATUS_W: i32 = 320;
const STATUS_ROW: i32 = 26;
const STATUS_ROWS: i32 = 7;

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
    line(c, r.x, r.y + r.h, r.x + r.w, r.y + r.h, theme::panel_rim());
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
    rounded_rect(c, p.x, p.y, p.w, p.h, 8, theme::panel(), 215);
    rounded_outline(c, p.x, p.y, p.w, p.h, 8, theme::panel_rim());
    text::draw(
        c,
        p.x + 12,
        p.y + 10,
        tr("ESTADO"),
        &label(theme::text_dim()),
    );
    let hint = label(theme::text_faint().lerp(theme::text_dim(), 120));
    text::draw_right(c, p.x + p.w - 12, p.y + 10, tr("CLIC: MONITOR"), &hint);
    line(
        c,
        p.x + 1,
        p.y + 34,
        p.x + p.w - 2,
        p.y + 34,
        theme::panel_rim(),
    );

    let key = light(theme::text_dim());
    let value = s16(theme::text());
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
        theme::cyan(),
    );
    text::draw_right(c, right, row(0), &format!("{} %", st.cpu_pct), &value);

    // Memoria: barra + usada/total.
    text::draw(c, p.x + 12, row(1), tr("MEMORIA"), &key);
    let mem_pct = (st.heap_used * 100).checked_div(st.heap_total).unwrap_or(0) as u32;
    bar(
        c,
        Rect::new(gx, row(1) + 6, gw, 5),
        mem_pct,
        theme::particle_bright(),
    );
    let mem = format!("{} MiB", st.heap_used / (1024 * 1024));
    text::draw_right(c, right, row(1), &mem, &value);

    // Disco.
    text::draw(c, p.x + 12, row(2), tr("DISCO"), &key);
    match disk {
        Some((free, total)) => {
            let used = total.saturating_sub(free);
            let pct = (used * 100).checked_div(total).unwrap_or(0) as u32;
            bar(c, Rect::new(gx, row(2) + 6, gw, 5), pct, theme::amber());
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
        theme::cyan(),
    );
    let net = match (st.net.present, st.net.ip) {
        (false, _) => String::from(tr("sin placa")),
        (true, None) => String::from("DHCP..."),
        (true, Some(a)) => ip(a),
    };
    text::draw_right(c, right, row(3), &net, &value);

    text::draw(c, p.x + 12, row(4), tr("TEMPERATURA"), &key);
    match st.temp_c {
        Some(t) => {
            sparkline(
                c,
                Rect::new(gx, row(4), gw, 16),
                &hist.temp,
                100,
                theme::crimson(),
            );
            text::draw_right(c, right, row(4), &temp_text(Some(t)), &value);
        }
        None => {
            text::draw_right(c, right, row(4), tr("sin sensor"), &key);
        }
    }

    text::draw(c, p.x + 12, row(5), tr("RENDIMIENTO"), &key);
    let perf = format!("{} FPS · {} ms", st.fps, st.frame_ms);
    text::draw_right(c, right, row(5), &perf, &value);

    text::draw(c, p.x + 12, row(6), tr("CEREBRO"), &key);
    if st.brain_online {
        text::draw_right(c, right, row(6), tr("conectado"), &light(theme::cyan()));
    } else {
        text::draw_right(c, right, row(6), tr("sin conectar"), &light(theme::amber()));
    }

    // Píldora "Control de misión" (abre la vista de tareas, como Win+Tab).
    let pill = pill_rect(w, h);
    rounded_rect(c, pill.x, pill.y, pill.w, pill.h, 15, theme::panel(), 215);
    rounded_outline(c, pill.x, pill.y, pill.w, pill.h, 15, theme::panel_rim());
    circle(c, pill.x + 16, pill.y + pill.h / 2, 3, theme::cyan(), true);
    text::draw(
        c,
        pill.x + 28,
        pill.y + 7,
        tr("CONTROL DE MISIÓN"),
        &label(theme::text_dim()),
    );
}

// --- menú de inicio ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartItem {
    App(AppKind),
    /// Abrir la dirección o buscar el texto en la web.
    Web(String),
    Lock,
    Logout,
    Sleep,
    Restart,
    Shutdown,
}

pub struct StartMenu {
    pub query: TextInput,
    pub selected: usize,
}

const MENU_APPS: [AppKind; 9] = [
    AppKind::Files,
    AppKind::Brave,
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
                trf("Abrir {}", &[text])
            } else {
                trf("Buscar en la web: {}", &[text])
            };
            out.push((StartItem::Web(text.into()), label, Icon::Globe));
        }
        out
    }

    pub fn rect(_w: usize, h: usize) -> Rect {
        let top = toolbar_rect(1).y + 46;
        Rect::new(MARGIN, top, 380, (h as i32 - top - 40).min(520))
    }

    fn row(menu: Rect, i: usize) -> Rect {
        Rect::new(
            menu.x + 10,
            menu.y + 64 + i as i32 * MENU_ROW,
            menu.w - 20,
            MENU_ROW - 4,
        )
    }

    /// Dos filas: bloquear, cerrar sesión y suspender; reiniciar y apagar.
    fn power_buttons(menu: Rect) -> [(StartItem, &'static str, Rect); 5] {
        let top = menu.y + menu.h - 92;
        let bottom = menu.y + menu.h - 48;
        let bw = (menu.w - 40) / 3;
        let at = |i: i32| Rect::new(menu.x + 10 + i * (bw + 10), top, bw, 36);
        let half = (menu.w - 30) / 2;
        let half_at = |i: i32| Rect::new(menu.x + 10 + i * (half + 10), bottom, half, 36);
        [
            (StartItem::Lock, tr("BLOQUEAR"), at(0)),
            (StartItem::Logout, tr("CERRAR SESIÓN"), at(1)),
            (StartItem::Sleep, tr("SUSPENDER"), at(2)),
            (StartItem::Restart, tr("REINICIAR"), half_at(0)),
            (StartItem::Shutdown, tr("APAGAR"), half_at(1)),
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
        rounded_rect(c, m.x, m.y, m.w, m.h, 12, theme::menu(), 248);
        rounded_outline(c, m.x, m.y, m.w, m.h, 12, theme::cyan().scale(120));
        let f = Rect::new(m.x + 12, m.y + 14, m.w - 24, 36);
        rounded_rect(c, f.x, f.y, f.w, f.h, 6, field_bg(), 255);
        rounded_outline(c, f.x, f.y, f.w, f.h, 6, theme::panel_rim());
        let st = s16(theme::text());
        if self.query.text.is_empty() {
            text::draw(
                c,
                f.x + 18,
                f.y + 10,
                tr("Escribí para buscar apps o la web"),
                &light(theme::text_dim()),
            );
            c.fill_rect(f.x + 11, f.y + 9, 2, 18, theme::cyan());
        } else {
            let tw = draw_fit(c, f.x + 12, f.y + 10, &self.query.text, &st, f.w - 30);
            c.fill_rect(f.x + 14 + tw, f.y + 9, 2, 18, theme::cyan());
        }
        for (i, (_, name, ic)) in self.items().iter().enumerate() {
            let r = Self::row(m, i);
            if r.y + r.h > m.y + m.h - 56 {
                break;
            }
            if i == self.selected {
                rounded_rect(c, r.x, r.y, r.w, r.h, 6, selected_bg(), 255);
                c.fill_rect(r.x, r.y + 6, 3, r.h - 12, theme::cyan());
            }
            icon(c, *ic, r.x + 22, r.y + r.h / 2, theme::cyan());
            draw_fit(c, r.x + 46, r.y + 10, name, &st, r.w - 56);
        }
        for (item, text_label, r) in Self::power_buttons(m) {
            let col = if item == StartItem::Shutdown {
                theme::crimson()
            } else {
                theme::text_dim()
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
            theme::vector_blue(),
            80,
        );
        rounded_outline(c, r.x - 4, r.y - 4, r.w + 8, r.h + 8, 8, theme::cyan());
    }
    let img = Rect::new(r.x, r.y, r.w, r.h - 30);
    c.fill_rect(img.x, img.y, img.w, img.h, theme::void());
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
    icon(c, t.icon, r.x + 12, r.y + r.h - 14, theme::cyan());
    draw_fit(
        c,
        r.x + 26,
        r.y + r.h - 22,
        &t.title,
        &s16(theme::text()),
        r.w - 30,
    );
}

pub fn draw_switcher(c: &mut Canvas<'_>, w: usize, h: usize, thumbs: &[Thumb<'_>], sel: usize) {
    let p = switcher_rect(w, h, thumbs.len());
    rounded_rect(c, p.x, p.y, p.w, p.h, 14, theme::menu(), 240);
    rounded_outline(c, p.x, p.y, p.w, p.h, 14, theme::panel_rim());
    text::draw(
        c,
        p.x + 20,
        p.y + 14,
        tr("CAMBIAR DE VENTANA"),
        &label(theme::text_dim()),
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
    c.fill_rect(
        0,
        0,
        w as i32,
        h as i32,
        theme::void().lerp(Color::BLACK, 50),
    );
    text::draw(
        c,
        MARGIN + 8,
        MARGIN + 60,
        tr("CONTROL DE MISIÓN"),
        &label(theme::cyan()),
    );
    text::draw(
        c,
        MARGIN + 8,
        MARGIN + 84,
        tr("Clic o Enter: ir a la ventana · Supr: cerrarla · Esc: volver"),
        &light(theme::text_dim()),
    );
    if thumbs.is_empty() {
        let st = s16(theme::text_dim());
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

#[allow(clippy::too_many_arguments)]
pub fn draw_lock(
    c: &mut Canvas<'_>,
    w: usize,
    h: usize,
    now: Option<DateTime>,
    big: &mut VectorText,
    h24: bool,
    // Con PIN: (cuántos dígitos se escribieron, el último estuvo mal).
    pin: Option<(usize, bool)>,
    // Después de "Cerrar sesión": el usuario que puede entrar.
    user: Option<&str>,
) {
    use core::fmt::Write;
    let (w, h) = (w as i32, h as i32);
    c.fill_rect(0, 0, w, h, theme::void());
    jarvis_gfx::shapes::glow(c, w / 2, h / 2, h / 2, theme::vector_blue().scale(100), 70);
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
    let st = label(theme::cyan().scale(200));
    let dw = text::width(date.as_str(), &st);
    text::draw(c, (w - dw) / 2, h / 2 + 10, date.as_str(), &st);
    let hint = match (pin, user) {
        (None, None) => tr("JARVIS-OS BLOQUEADO · TOCÁ UNA TECLA O HACÉ CLIC"),
        (Some(_), None) => tr("JARVIS-OS BLOQUEADO · ESCRIBÍ TU PIN Y APRETÁ ENTER"),
        (None, Some(_)) => tr("SESIÓN CERRADA · TOCÁ UNA TECLA O HACÉ CLIC PARA ENTRAR"),
        (Some(_), Some(_)) => tr("SESIÓN CERRADA · ESCRIBÍ TU PIN Y APRETÁ ENTER"),
    };
    // El usuario, con un círculo con su inicial (como en la pantalla de inicio de Windows).
    let pin_y = if let Some(name) = user {
        let (cx, cy) = (w / 2, h / 2 + 70);
        circle(c, cx, cy, 26, theme::vector_blue(), true);
        circle(c, cx, cy, 26, theme::cyan(), false);
        let initial: String = name.chars().take(1).flat_map(char::to_uppercase).collect();
        let st = crate::widgets::bold(Color::WHITE);
        let iw = text::width(&initial, &st);
        text::draw(c, cx - iw / 2, cy - 9, &initial, &st);
        let st = s16(theme::text());
        text::draw(c, cx - text::width(name, &st) / 2, cy + 36, name, &st);
        h / 2 + 140
    } else {
        h / 2 + 60
    };
    let st = label(theme::text_dim());
    text::draw(c, (w - text::width(hint, &st)) / 2, h - 90, hint, &st);
    if let Some((n, wrong)) = pin {
        let f = Rect::new((w - 260) / 2, pin_y, 260, 44);
        rounded_rect(c, f.x, f.y, f.w, f.h, 8, field_bg(), 255);
        let rim = if wrong { theme::amber() } else { theme::cyan() };
        rounded_outline(c, f.x, f.y, f.w, f.h, 8, rim);
        for i in 0..n as i32 {
            circle(c, f.x + 24 + i * 30, f.y + f.h / 2, 6, theme::text(), true);
        }
        if n == 0 {
            let st = light(theme::text_dim());
            text::draw(c, f.x + 16, f.y + 13, "PIN", &st);
        }
        if wrong {
            let msg = tr("PIN INCORRECTO. PROBÁ OTRA VEZ.");
            let st = label(theme::amber());
            text::draw(c, (w - text::width(msg, &st)) / 2, f.y + f.h + 16, msg, &st);
        }
    }
}

pub fn power_rect(w: usize, h: usize) -> Rect {
    Rect::new((w as i32 - 480) / 2, (h as i32 - 250) / 2, 480, 250)
}

/// Opciones del diálogo de apagado, en el orden de los botones.
pub const POWER_CHOICES: usize = 5;

/// Botones del diálogo de apagado: apagar, reiniciar, suspender (arriba); cerrar sesión y
/// cancelar (abajo).
pub fn power_buttons(w: usize, h: usize) -> [Rect; POWER_CHOICES] {
    let d = power_rect(w, h);
    let bw = 136;
    let at = |col: i32, row: i32| {
        Rect::new(
            d.x + 20 + col * (bw + 16),
            d.y + d.h - 100 + row * 48,
            bw,
            36,
        )
    };
    [at(0, 0), at(1, 0), at(2, 0), at(0, 1), at(2, 1)]
}

pub fn draw_power(c: &mut Canvas<'_>, w: usize, h: usize, sel: usize) {
    c.fill_rect(
        0,
        0,
        w as i32,
        h as i32,
        theme::void().lerp(Color::BLACK, 90),
    );
    let d = power_rect(w, h);
    rounded_rect(c, d.x, d.y, d.w, d.h, 12, theme::menu(), 255);
    rounded_outline(c, d.x, d.y, d.w, d.h, 12, theme::crimson().scale(180));
    icon(c, Icon::Power, d.x + 34, d.y + 36, theme::crimson());
    text::draw(
        c,
        d.x + 58,
        d.y + 28,
        tr("APAGAR JARVIS-OS"),
        &label(theme::crimson()),
    );
    text::draw(
        c,
        d.x + 24,
        d.y + 72,
        tr("¿Qué querés que haga la computadora?"),
        &s16(theme::text()),
    );
    let labels = [
        tr("APAGAR"),
        tr("REINICIAR"),
        tr("SUSPENDER"),
        tr("CERRAR SESIÓN"),
        tr("CANCELAR"),
    ];
    for (i, r) in power_buttons(w, h).into_iter().enumerate() {
        let col = match i {
            0 => theme::crimson(),
            1 => theme::amber(),
            2 | 3 => theme::cyan(),
            _ => theme::text_dim(),
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
        let col = if *error {
            theme::amber()
        } else {
            theme::cyan()
        };
        rounded_rect(c, r.x, r.y, r.w, r.h, 8, theme::menu(), 245);
        rounded_outline(c, r.x, r.y, r.w, r.h, 8, col.scale(170));
        c.fill_rect(r.x + 1, r.y + 8, 3, r.h - 16, col);
        draw_fit(c, r.x + 16, r.y + 13, msg, &s16(theme::text()), r.w - 28);
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

// --- distribuciones de ventanas (Win+Z) --------------------------------------------------------

const THUMB_W: i32 = 120;
const THUMB_H: i32 = 76;
const THUMB_GAP: i32 = 14;

pub fn layouts_rect(w: usize, _h: usize) -> Rect {
    let n = crate::wm::LAYOUTS.len() as i32;
    let pw = n * THUMB_W + (n - 1) * THUMB_GAP + 32;
    Rect::new((w as i32 - pw) / 2, 70, pw, 44 + THUMB_H + 40)
}

/// Todas las zonas de todas las plantillas, en orden: (plantilla, zona, dónde se dibuja).
pub fn layout_zones(w: usize, h: usize) -> Vec<(usize, usize, Rect)> {
    let r = layouts_rect(w, h);
    let mut out = Vec::new();
    for (t, layout) in crate::wm::LAYOUTS.iter().enumerate() {
        let thumb = Rect::new(
            r.x + 16 + t as i32 * (THUMB_W + THUMB_GAP),
            r.y + 44,
            THUMB_W,
            THUMB_H,
        );
        for (z, zone) in layout.zones(thumb.inset(3)).into_iter().enumerate() {
            out.push((t, z, zone.inset(2)));
        }
    }
    out
}

/// La zona bajo el mouse (índice en [`layout_zones`]).
pub fn layouts_hit(w: usize, h: usize, x: i32, y: i32) -> Option<usize> {
    layout_zones(w, h)
        .iter()
        .position(|(_, _, r)| r.contains(x, y))
}

pub fn draw_layouts(c: &mut Canvas<'_>, w: usize, h: usize, sel: usize) {
    let r = layouts_rect(w, h);
    rounded_rect(c, r.x, r.y, r.w, r.h, 12, theme::menu(), 250);
    rounded_outline(c, r.x, r.y, r.w, r.h, 12, theme::cyan().scale(120));
    text::draw(
        c,
        r.x + 16,
        r.y + 14,
        tr("DISTRIBUCIONES (WIN+Z)"),
        &label(theme::text_dim()),
    );
    let zones = layout_zones(w, h);
    let chosen = zones.get(sel).map(|z| z.0);
    for (i, (t, _, z)) in zones.iter().enumerate() {
        let (bg, alpha) = if i == sel {
            (theme::cyan(), 200)
        } else if Some(*t) == chosen {
            (theme::vector_blue(), 110)
        } else {
            (theme::panel_rim(), 160)
        };
        rounded_rect(c, z.x, z.y, z.w, z.h, 3, bg, alpha);
    }
    let hint =
        tr("Flechas o mouse: la zona para esta ventana · Enter la acomoda · las demás completan");
    draw_fit(
        c,
        r.x + 16,
        r.y + r.h - 28,
        hint,
        &light(theme::text_dim()),
        r.w - 32,
    );
}

// --- monitores (Win+P) ------------------------------------------------------------------------

const PROJECT_ROW: i32 = 58;

/// El panel de Win+P, a la derecha del monitor principal (como en Windows).
pub fn project_rect(w: usize, h: usize) -> Rect {
    let n = crate::display::Mode::ALL.len() as i32;
    let ph = 48 + n * PROJECT_ROW + 12;
    Rect::new(w as i32 - 320 - MARGIN, (h as i32 - ph) / 2, 320, ph)
}

fn project_row(w: usize, h: usize, i: usize) -> Rect {
    let r = project_rect(w, h);
    Rect::new(
        r.x + 10,
        r.y + 44 + i as i32 * PROJECT_ROW,
        r.w - 20,
        PROJECT_ROW - 8,
    )
}

pub fn project_hit(w: usize, h: usize, x: i32, y: i32) -> Option<usize> {
    (0..crate::display::Mode::ALL.len()).find(|&i| project_row(w, h, i).contains(x, y))
}

pub fn draw_project(
    c: &mut Canvas<'_>,
    w: usize,
    h: usize,
    sel: usize,
    current: crate::display::Mode,
) {
    let r = project_rect(w, h);
    rounded_rect(c, r.x, r.y, r.w, r.h, 12, theme::menu(), 250);
    rounded_outline(c, r.x, r.y, r.w, r.h, 12, theme::cyan().scale(120));
    text::draw(
        c,
        r.x + 16,
        r.y + 14,
        tr("PROYECTAR (WIN+P)"),
        &label(theme::text_dim()),
    );
    for (i, mode) in crate::display::Mode::ALL.iter().enumerate() {
        let row = project_row(w, h, i);
        if i == sel {
            rounded_rect(
                c,
                row.x,
                row.y,
                row.w,
                row.h,
                6,
                crate::widgets::selected_bg(),
                255,
            );
            c.fill_rect(row.x, row.y + 8, 3, row.h - 16, theme::cyan());
        }
        // Dibujito: dos pantallas, llenas según el modo.
        let (ix, iy) = (row.x + 14, row.y + (row.h - 22) / 2);
        let on = theme::cyan();
        let off = theme::panel_rim();
        let (a, b) = match mode {
            crate::display::Mode::OnlyFirst => (on, off),
            crate::display::Mode::OnlySecond => (off, on),
            _ => (on, on),
        };
        rounded_rect(c, ix, iy, 22, 22, 3, a, 200);
        rounded_rect(c, ix + 26, iy, 22, 22, 3, b, 200);
        if *mode == crate::display::Mode::Duplicate {
            text::draw(c, ix + 7, iy + 3, "1", &s16(theme::void()));
            text::draw(c, ix + 33, iy + 3, "1", &s16(theme::void()));
        } else if *mode == crate::display::Mode::Extend {
            text::draw(c, ix + 7, iy + 3, "1", &s16(theme::void()));
            text::draw(c, ix + 33, iy + 3, "2", &s16(theme::void()));
        }
        let col = if *mode == current {
            theme::cyan()
        } else {
            theme::text()
        };
        draw_fit(
            c,
            ix + 62,
            row.y + (row.h - 16) / 2,
            tr(mode.name()),
            &s16(col),
            row.w - 80,
        );
    }
}

/// Un número grande en el medio de un monitor (Configuración → Pantallas → Identificar).
pub fn draw_screen_number(c: &mut Canvas<'_>, screen: Rect, n: usize) {
    let (cx, cy) = (screen.x + screen.w / 2, screen.y + screen.h / 2);
    rounded_rect(c, cx - 80, cy - 80, 160, 160, 20, theme::menu(), 235);
    rounded_outline(c, cx - 80, cy - 80, 160, 160, 20, theme::cyan());
    let st = text::Style::new(text::Weight::Bold, text::Size::Size32, theme::cyan()).scale(3);
    let label_ = alloc::format!("{n}");
    let tw = text::width(&label_, &st);
    text::draw(c, cx - tw / 2, cy - st.line_height() / 2, &label_, &st);
}
