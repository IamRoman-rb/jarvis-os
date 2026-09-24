//! Paneles y menús que se abren con atajos de Windows:
//!
//! - **Menú de lista** ([`Menu`]): Win+X (enlaces rápidos) y Alt+Espacio (menú de la ventana).
//! - **Configuración rápida** (Win+A): interruptores de lo más usado y accesos directos.
//! - **Centro de notificaciones** (Win+N): calendario del mes y los últimos avisos.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_gfx::clock::DateTime;
use jarvis_gfx::hud::{Icon, MARGIN, icon};
use jarvis_gfx::shapes::{rounded_outline, rounded_rect};
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, Color, Rect, theme};

use crate::config::Config;
use crate::system::Launch;
use crate::widgets::{SELECTED_BG, button, draw_fit, label, light, s16};
use crate::wm::{Side, WinId};

const PANEL_BG: Color = Color::hex(0x081527);
/// Opacidad de los paneles (casi opacos: lo de atrás no se lee a través).
const PANEL_ALPHA: u8 = 255;

// --- menú de lista ----------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Launch(Launch),
    PowerMenu,
    ShowDesktop,
    Lock,
    Search,
    Restore(WinId),
    Minimize(WinId),
    Maximize(WinId),
    Snap(WinId, Side),
    Close(WinId),
    NewDesktop,
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub hint: &'static str,
    pub icon: Option<Icon>,
    pub action: Action,
}

impl MenuItem {
    pub fn new(label: &str, hint: &'static str, icon: Option<Icon>, action: Action) -> MenuItem {
        MenuItem {
            label: label.into(),
            hint,
            icon,
            action,
        }
    }
}

pub struct Menu {
    pub title: &'static str,
    pub items: Vec<MenuItem>,
    pub sel: usize,
    /// Esquina de arriba a la izquierda.
    pub at: (i32, i32),
}

const ROW: i32 = 34;
const MENU_W: i32 = 330;

impl Menu {
    pub fn rect(&self, w: usize, h: usize) -> Rect {
        let height = 44 + self.items.len() as i32 * ROW + 8;
        let x = self.at.0.clamp(0, (w as i32 - MENU_W).max(0));
        let y = self.at.1.clamp(0, (h as i32 - height).max(0));
        Rect::new(x, y, MENU_W, height)
    }

    fn row(&self, r: Rect, i: usize) -> Rect {
        Rect::new(r.x + 8, r.y + 40 + i as i32 * ROW, r.w - 16, ROW - 2)
    }

    pub fn hit(&self, w: usize, h: usize, x: i32, y: i32) -> Option<usize> {
        let r = self.rect(w, h);
        (0..self.items.len()).find(|&i| self.row(r, i).contains(x, y))
    }

    pub fn draw(&self, c: &mut Canvas<'_>, w: usize, h: usize) {
        let r = self.rect(w, h);
        rounded_rect(c, r.x, r.y, r.w, r.h, 10, PANEL_BG, PANEL_ALPHA);
        rounded_outline(c, r.x, r.y, r.w, r.h, 10, theme::CYAN.scale(120));
        text::draw(c, r.x + 16, r.y + 14, self.title, &label(theme::TEXT_DIM));
        for (i, it) in self.items.iter().enumerate() {
            let rr = self.row(r, i);
            if i == self.sel {
                rounded_rect(c, rr.x, rr.y, rr.w, rr.h, 5, SELECTED_BG, 255);
                c.fill_rect(rr.x, rr.y + 6, 3, rr.h - 12, theme::CYAN);
            }
            if let Some(ic) = it.icon {
                icon(c, ic, rr.x + 20, rr.y + rr.h / 2, theme::CYAN);
            }
            draw_fit(
                c,
                rr.x + 40,
                rr.y + 8,
                &it.label,
                &s16(theme::TEXT),
                rr.w - 150,
            );
            text::draw_right(
                c,
                rr.x + rr.w - 10,
                rr.y + 8,
                it.hint,
                &light(theme::TEXT_DIM),
            );
        }
    }
}

/// Win+X: los "enlaces rápidos" (el menú del botón derecho sobre Inicio en Windows).
pub fn quick_links() -> Menu {
    use crate::system::AppKind::*;
    let app = |k| Action::Launch(Launch::App(k));
    Menu {
        title: "ENLACES RÁPIDOS (WIN+X)",
        items: alloc::vec![
            MenuItem::new(
                "Aplicaciones instaladas",
                "",
                Some(Icon::Package),
                Action::Launch(Launch::Settings(7))
            ),
            MenuItem::new(
                "Monitor del sistema",
                "Ctrl+Shift+Esc",
                Some(Icon::Gauge),
                app(Monitor)
            ),
            MenuItem::new("Configuración", "Win+I", Some(Icon::Gear), app(Settings)),
            MenuItem::new(
                "Terminal",
                "Ctrl+Alt+T",
                Some(Icon::Terminal),
                app(Terminal)
            ),
            MenuItem::new("Archivos", "Win+E", Some(Icon::Folder), app(Files)),
            MenuItem::new("Buscar", "Win+S", Some(Icon::Globe), Action::Search),
            MenuItem::new(
                "Ejecutar (consola JARVIS)",
                "Win+R",
                Some(Icon::Mic),
                app(Console)
            ),
            MenuItem::new(
                "Escritorio nuevo",
                "Win+Ctrl+D",
                Some(Icon::Screen),
                Action::NewDesktop
            ),
            MenuItem::new(
                "Mostrar el escritorio",
                "Win+D",
                Some(Icon::Chat),
                Action::ShowDesktop
            ),
            MenuItem::new("Bloquear", "Win+L", Some(Icon::Lock), Action::Lock),
            MenuItem::new(
                "Apagar o reiniciar",
                "Alt+F4",
                Some(Icon::Power),
                Action::PowerMenu
            ),
        ],
        sel: 0,
        at: (MARGIN, 88),
    }
}

/// Alt+Espacio: el menú de la ventana.
pub fn window_menu(id: WinId, at: (i32, i32), maximized: bool) -> Menu {
    let mut items = Vec::new();
    if maximized {
        items.push(MenuItem::new(
            "Restaurar",
            "Win+Abajo",
            None,
            Action::Restore(id),
        ));
    } else {
        items.push(MenuItem::new(
            "Maximizar",
            "Win+Arriba",
            None,
            Action::Maximize(id),
        ));
    }
    items.push(MenuItem::new(
        "Minimizar",
        "Win+Abajo",
        None,
        Action::Minimize(id),
    ));
    items.push(MenuItem::new(
        "Acoplar a la izquierda",
        "Win+Izq.",
        None,
        Action::Snap(id, Side::Left),
    ));
    items.push(MenuItem::new(
        "Acoplar a la derecha",
        "Win+Der.",
        None,
        Action::Snap(id, Side::Right),
    ));
    items.push(MenuItem::new(
        "Cerrar",
        "Alt+F4",
        Some(Icon::Power),
        Action::Close(id),
    ));
    Menu {
        title: "VENTANA (ALT+ESPACIO)",
        items,
        sel: 0,
        at,
    }
}

// --- configuración rápida (Win+A) -------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    Sounds,
    Animations,
    StatusPanel,
    LightPages,
    Reader,
    Clock24,
}

pub const QUICK: [(Quick, &str); 6] = [
    (Quick::Sounds, "Sonidos"),
    (Quick::Animations, "Animaciones"),
    (Quick::StatusPanel, "Panel de estado"),
    (Quick::LightPages, "Páginas claras"),
    (Quick::Reader, "Modo lectura"),
    (Quick::Clock24, "Reloj 24 h"),
];

pub fn quick_value(cfg: &Config, q: Quick) -> bool {
    match q {
        Quick::Sounds => cfg.sounds,
        Quick::Animations => cfg.animations,
        Quick::StatusPanel => cfg.status_panel,
        Quick::LightPages => cfg.light_pages,
        Quick::Reader => cfg.reader_mode,
        Quick::Clock24 => cfg.clock_24h,
    }
}

pub fn quick_toggle(cfg: &mut Config, q: Quick) {
    match q {
        Quick::Sounds => cfg.sounds = !cfg.sounds,
        Quick::Animations => cfg.animations = !cfg.animations,
        Quick::StatusPanel => cfg.status_panel = !cfg.status_panel,
        Quick::LightPages => cfg.light_pages = !cfg.light_pages,
        Quick::Reader => cfg.reader_mode = !cfg.reader_mode,
        Quick::Clock24 => cfg.clock_24h = !cfg.clock_24h,
    }
}

/// Botones de abajo del panel rápido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickButton {
    Settings,
    Lock,
    Power,
}

const QB: [(QuickButton, &str); 3] = [
    (QuickButton::Settings, "CONFIGURACIÓN"),
    (QuickButton::Lock, "BLOQUEAR"),
    (QuickButton::Power, "APAGAR"),
];

pub fn quick_rect(w: usize, _h: usize) -> Rect {
    Rect::new(w as i32 - MARGIN - 420, 200, 420, 268)
}

fn tile(r: Rect, i: usize) -> Rect {
    let (col, row) = ((i % 3) as i32, (i / 3) as i32);
    let tw = (r.w - 16 * 2 - 10 * 2) / 3;
    Rect::new(r.x + 16 + col * (tw + 10), r.y + 50 + row * 76, tw, 66)
}

fn quick_button(r: Rect, i: usize) -> Rect {
    let bw = (r.w - 32 - 20) / 3;
    Rect::new(r.x + 16 + i as i32 * (bw + 10), r.y + r.h - 54, bw, 36)
}

pub enum QuickHit {
    Toggle(Quick),
    Button(QuickButton),
}

pub fn quick_hit(w: usize, h: usize, x: i32, y: i32) -> Option<QuickHit> {
    let r = quick_rect(w, h);
    for (i, (q, _)) in QUICK.iter().enumerate() {
        if tile(r, i).contains(x, y) {
            return Some(QuickHit::Toggle(*q));
        }
    }
    QB.iter()
        .enumerate()
        .find(|(i, _)| quick_button(r, *i).contains(x, y))
        .map(|(_, (b, _))| QuickHit::Button(*b))
}

pub fn draw_quick(c: &mut Canvas<'_>, w: usize, h: usize, cfg: &Config, sel: usize, net: &str) {
    let r = quick_rect(w, h);
    rounded_rect(c, r.x, r.y, r.w, r.h, 12, PANEL_BG, PANEL_ALPHA);
    rounded_outline(c, r.x, r.y, r.w, r.h, 12, theme::CYAN.scale(120));
    text::draw(
        c,
        r.x + 16,
        r.y + 16,
        "CONFIGURACIÓN RÁPIDA",
        &label(theme::TEXT_DIM),
    );
    text::draw_right(c, r.x + r.w - 16, r.y + 16, net, &light(theme::CYAN));
    for (i, (q, name)) in QUICK.iter().enumerate() {
        let t = tile(r, i);
        let on = quick_value(cfg, *q);
        let fill = if on { theme::VECTOR_BLUE } else { theme::PANEL };
        rounded_rect(c, t.x, t.y, t.w, t.h, 8, fill, if on { 200 } else { 255 });
        let rim = if i == sel {
            theme::CYAN
        } else {
            theme::PANEL_RIM
        };
        rounded_outline(c, t.x, t.y, t.w, t.h, 8, rim);
        draw_fit(c, t.x + 12, t.y + 14, name, &s16(theme::TEXT), t.w - 20);
        text::draw(
            c,
            t.x + 12,
            t.y + 38,
            if on { "Activado" } else { "Desactivado" },
            &light(if on { theme::TEXT } else { theme::TEXT_DIM }),
        );
    }
    for (i, (b, name)) in QB.iter().enumerate() {
        let col = if *b == QuickButton::Power {
            theme::CRIMSON
        } else {
            theme::CYAN
        };
        let sel_here = sel == QUICK.len() + i;
        button(
            c,
            quick_button(r, i),
            name,
            col,
            if sel_here { 90 } else { 25 },
        );
    }
}

// --- centro de notificaciones (Win+N) ---------------------------------------------------------

pub fn notices_rect(w: usize, h: usize) -> Rect {
    let height = (h as i32 - 220).min(560);
    Rect::new(w as i32 - MARGIN - 420, 200, 420, height)
}

pub fn clear_button(w: usize, h: usize) -> Rect {
    let r = notices_rect(w, h);
    Rect::new(r.x + r.w - 150, r.y + 12, 134, 30)
}

const MONTHS: [&str; 12] = [
    "Enero",
    "Febrero",
    "Marzo",
    "Abril",
    "Mayo",
    "Junio",
    "Julio",
    "Agosto",
    "Septiembre",
    "Octubre",
    "Noviembre",
    "Diciembre",
];

/// (texto, es error, hora "HH:MM")
pub fn draw_notices(
    c: &mut Canvas<'_>,
    w: usize,
    h: usize,
    notices: &[(String, bool, String)],
    now: Option<DateTime>,
) {
    let r = notices_rect(w, h);
    rounded_rect(c, r.x, r.y, r.w, r.h, 12, PANEL_BG, PANEL_ALPHA);
    rounded_outline(c, r.x, r.y, r.w, r.h, 12, theme::CYAN.scale(120));
    text::draw(
        c,
        r.x + 16,
        r.y + 18,
        "NOTIFICACIONES",
        &label(theme::TEXT_DIM),
    );
    button(c, clear_button(w, h), "BORRAR TODO", theme::TEXT_DIM, 20);
    let mut y = r.y + 56;
    if notices.is_empty() {
        text::draw(
            c,
            r.x + 16,
            y,
            "No hay notificaciones nuevas.",
            &light(theme::TEXT_DIM),
        );
        y += 30;
    }
    let cal_h = 250;
    for (msg, error, time) in notices.iter().rev() {
        if y + 50 > r.y + r.h - cal_h {
            break;
        }
        let col = if *error { theme::AMBER } else { theme::CYAN };
        let n = Rect::new(r.x + 12, y, r.w - 24, 46);
        rounded_rect(c, n.x, n.y, n.w, n.h, 6, theme::PANEL, 255);
        c.fill_rect(n.x + 1, n.y + 8, 3, n.h - 16, col);
        draw_fit(c, n.x + 14, n.y + 6, msg, &s16(theme::TEXT), n.w - 28);
        text::draw(c, n.x + 14, n.y + 25, time, &light(theme::TEXT_DIM));
        y += 52;
    }
    // Calendario del mes.
    let Some(t) = now else { return };
    let top = r.y + r.h - cal_h + 10;
    c.fill_rect(r.x + 12, top - 8, r.w - 24, 1, theme::PANEL_RIM);
    let title = format!("{} de {}", MONTHS[t.month as usize - 1], t.year);
    text::draw(c, r.x + 16, top, &title, &s16(theme::TEXT));
    let cw = (r.w - 32) / 7;
    for (i, d) in ["do", "lu", "ma", "mi", "ju", "vi", "sá"]
        .iter()
        .enumerate()
    {
        text::draw(
            c,
            r.x + 16 + i as i32 * cw + 10,
            top + 30,
            d,
            &light(theme::TEXT_DIM),
        );
    }
    let first = DateTime { day: 1, ..t }.weekday();
    let days = (28..=31)
        .rev()
        .find(|&d| DateTime { day: d, ..t }.is_valid())
        .unwrap_or(28);
    for d in 1..=days {
        let cell = first + d as usize - 1;
        let (col, row) = ((cell % 7) as i32, (cell / 7) as i32);
        let x = r.x + 16 + col * cw;
        let y = top + 56 + row * 30;
        let st = s16(if d == t.day { theme::VOID } else { theme::TEXT });
        if d == t.day {
            rounded_rect(c, x + 2, y - 5, cw - 4, 26, 13, theme::CYAN, 255);
        }
        let s = format!("{d:>2}");
        text::draw(c, x + (cw - text::width(&s, &st)) / 2, y, &s, &st);
    }
}
