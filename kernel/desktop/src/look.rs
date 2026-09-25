//! Cómo se ve el escritorio (Configuración → Apariencia, Tipografía, Ventanas y Barra de tareas).
//!
//! Los colores viven en `jarvis_gfx::theme`; acá está el resto de lo que se dibuja distinto
//! según la configuración: tamaños de letra, de qué lado van los botones de las ventanas, el
//! cursor grande, qué gráficos muestra la barra de arriba. Son valores globales (atómicos, como
//! la paleta) porque los usan piezas que no reciben la configuración: el marco de las ventanas,
//! los estilos de texto de todas las apps, el gestor de ventanas.
//!
//! [`apply`] los pone a partir de la [`Config`]; el escritorio lo llama al arrancar y en cada
//! cambio de configuración.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use jarvis_gfx::text::Size;
use jarvis_gfx::theme::{self, Palette};

use crate::config::{Config, ThemeKind};

/// Colores de acento para elegir (0 = el del tema).
pub const ACCENTS: [(&str, u32); 9] = [
    ("Del tema", 0),
    ("Cian", 0x00f0ff),
    ("Azul", 0x3d8bff),
    ("Violeta", 0xa66bff),
    ("Rosa", 0xff5ca8),
    ("Rojo", 0xff4d4d),
    ("Naranja", 0xff9d00),
    ("Verde", 0x33d17a),
    ("Amarillo", 0xf5d90a),
];

/// Bits de [`top_stats`]: qué gráficos muestra la barra de arriba.
pub const STAT_CPU: u8 = 1;
pub const STAT_MEM: u8 = 2;
pub const STAT_DISK: u8 = 4;
pub const STAT_NET: u8 = 8;
pub const STAT_TEMP: u8 = 16;
pub const STATS_ALL: u8 = 31;

static UI_LARGE: AtomicBool = AtomicBool::new(false);
static BOLD_TITLES: AtomicBool = AtomicBool::new(false);
static TERM_SIZE: AtomicU8 = AtomicU8::new(16);
static EDITOR_SIZE: AtomicU8 = AtomicU8::new(16);
static BUTTONS_LEFT: AtomicBool = AtomicBool::new(false);
static CURSOR_BIG: AtomicBool = AtomicBool::new(false);
static TOP_STATS: AtomicU8 = AtomicU8::new(STATS_ALL);

/// La paleta que corresponde a la configuración.
pub fn palette(c: &Config) -> Palette {
    let base = match c.theme {
        ThemeKind::Hud => Palette::HUD,
        ThemeKind::Light => Palette::LIGHT,
        ThemeKind::Contrast => Palette::CONTRAST,
    };
    match ACCENTS.get(c.accent as usize) {
        Some(&(_, rgb)) if rgb != 0 => base.with_accent(rgb),
        _ => base,
    }
}

pub fn apply(c: &Config) {
    theme::apply(&palette(c));
    UI_LARGE.store(c.ui_large, Ordering::Relaxed);
    BOLD_TITLES.store(c.bold_titles, Ordering::Relaxed);
    TERM_SIZE.store(c.term_size, Ordering::Relaxed);
    EDITOR_SIZE.store(c.editor_size, Ordering::Relaxed);
    BUTTONS_LEFT.store(c.buttons_left, Ordering::Relaxed);
    CURSOR_BIG.store(c.cursor_big, Ordering::Relaxed);
    TOP_STATS.store(c.top_stats, Ordering::Relaxed);
}

fn size(px: u8) -> Size {
    match px {
        0..=17 => Size::Size16,
        18..=21 => Size::Size20,
        22..=27 => Size::Size24,
        _ => Size::Size32,
    }
}

/// Tamaño del texto de la interfaz (menús, listas, botones).
pub fn ui_size() -> Size {
    if UI_LARGE.load(Ordering::Relaxed) {
        Size::Size20
    } else {
        Size::Size16
    }
}

pub fn bold_titles() -> bool {
    BOLD_TITLES.load(Ordering::Relaxed)
}

pub fn term_size() -> Size {
    size(TERM_SIZE.load(Ordering::Relaxed))
}

pub fn editor_size() -> Size {
    size(EDITOR_SIZE.load(Ordering::Relaxed))
}

/// Minimizar, maximizar y cerrar a la izquierda (como en macOS o en algunos temas de KDE).
pub fn buttons_left() -> bool {
    BUTTONS_LEFT.load(Ordering::Relaxed)
}

pub fn cursor_big() -> bool {
    CURSOR_BIG.load(Ordering::Relaxed)
}

pub fn top_stats() -> u8 {
    TOP_STATS.load(Ordering::Relaxed)
}
