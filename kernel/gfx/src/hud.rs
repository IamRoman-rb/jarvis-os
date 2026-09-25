//! Elementos de la pantalla principal de JARVIS-OS, separados en capas.
//!
//! Distribución (según la imagen de referencia): barra de íconos arriba a la izquierda,
//! "JARVIS" + hora + fecha arriba a la derecha, esfera en el centro, panel de estado y
//! "Control de misión" abajo a la izquierda, y el último mensaje del asistente abajo a la derecha.
//!
//! - **Capa estática** ([`draw_static`]): se dibuja una sola vez en el buffer de fondo.
//! - **Capas dinámicas** (esfera, reloj, mensaje): cada una tiene su rectángulo, que es lo único
//!   que se redibuja cuando cambia. La orquestación está en el escritorio (`jarvis-desktop`).

use core::fmt::Write;

use crate::canvas::Rect;
use crate::clock::{DateTime, StrBuf};
use crate::shapes::{circle, glow, line, rect_outline, rounded_outline};
use crate::sphere::{Pulse, View};
use crate::text::{self, Size, Style, Weight};
use crate::trig::FULL_TURN;
use crate::vfont::VectorText;
use crate::{Canvas, Color, theme};

pub const MARGIN: i32 = 28;
/// Una vuelta de la esfera cada 25 segundos.
const SPIN_PERIOD_MS: u64 = 25_000;
/// La fecha más larga posible, para reservar su espacio.
/// La fecha más larga (en cualquiera de los tres idiomas): reserva su lugar.
const LONGEST_DATE: &str = "QUARTA-FEIRA, 30 DE SETEMBRO DE 2026";

/// Alto de los dígitos de la hora, en píxeles.
const TIME_HEIGHT: i32 = 58;

/// La hora se dibuja con la fuente vectorial (nítida a cualquier tamaño) y un halo suave: el
/// mismo texto con un trazo mucho más grueso y casi transparente.
pub struct ClockFace {
    digits: VectorText,
    halo: VectorText,
}

impl ClockFace {
    pub const fn new() -> Self {
        ClockFace {
            digits: VectorText::new(TIME_HEIGHT, 4 * 64, 10),
            halo: VectorText::new(TIME_HEIGHT, 14 * 64, 10),
        }
    }
}

impl Default for ClockFace {
    fn default() -> Self {
        Self::new()
    }
}

fn date_style() -> Style {
    Style::new(Weight::Regular, Size::Size16, theme::CYAN.scale(190)).tracking(3)
}

// --- capa estática ----------------------------------------------------------------------------

/// Fondo, grilla y título "JARVIS". La barra de íconos y el panel de estado son dinámicos y los
/// dibuja el escritorio (`jarvis-desktop`).
pub fn draw_static(c: &mut Canvas<'_>) {
    background(c);
    draw_title(c);
}

/// Fondo de un color liso (con una luz suave detrás de la esfera) y el título.
pub fn draw_solid(c: &mut Canvas<'_>, color: Color) {
    let (w, h) = (c.width() as i32, c.height() as i32);
    c.fill(color);
    glow(
        c,
        w / 2,
        h / 2,
        h * 45 / 100,
        color.lerp(Color::WHITE, 18),
        70,
    );
    draw_title(c);
}

/// El título "JARVIS" de arriba a la derecha (va encima de cualquier fondo).
pub fn draw_title(c: &mut Canvas<'_>) {
    let right = c.width() as i32 - MARGIN - 8;
    // Título de fondo, con la fuente vectorial (antes era la bitmap agrandada ×3: borrosa).
    let mut title = VectorText::new(34, 2 * 64, 16);
    let tw = title.width("JARVIS");
    title.draw(
        c,
        right - tw,
        MARGIN - 10,
        "JARVIS",
        theme::TEXT_FAINT.lerp(theme::TEXT_DIM, 90),
    );
}

fn background(c: &mut Canvas<'_>) {
    let (w, h) = (c.width() as i32, c.height() as i32);
    c.fill(theme::VOID);
    // Grilla de puntos cada 32 px (DESIGN.md: "holographic crosshair dot grid").
    let dot = theme::VOID.lerp(theme::CYAN, 14);
    for y in (16..h).step_by(32) {
        for x in (16..w).step_by(32) {
            c.put(x, y, dot);
        }
    }
    // Luz ambiente arriba a la izquierda y detrás de la esfera.
    glow(c, w / 6, 0, h * 7 / 10, Color::hex(0x0a2350), 120);
    glow(c, w / 2, h / 2, h * 45 / 100, Color::hex(0x0a2a66), 90);
}

/// Íconos de línea de 16×16 px (centrados en el punto que se pasa).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Menú de inicio: cuatro cuadrados, como el botón de Windows.
    Start,
    Mic,
    Screen,
    Camera,
    Folder,
    Music,
    Globe,
    Chat,
    Document,
    Image,
    Power,
    Restart,
    Lock,
    Gauge,
    /// Engranaje (Configuración).
    Gear,
    /// `>_` (Terminal).
    Terminal,
    /// Caja (paquetes).
    Package,
    /// Campana (notificaciones).
    Bell,
    /// Calendario.
    Calendar,
    /// Descarga (flecha hacia abajo sobre una bandeja).
    Download,
    /// Escudo (Brave).
    Shield,
}

/// Dibuja el ícono `which` centrado en (x, y).
pub fn icon(c: &mut Canvas<'_>, which: Icon, x: i32, y: i32, col: Color) {
    match which {
        Icon::Start => {
            for (dx, dy) in [(-7, -7), (1, -7), (-7, 1), (1, 1)] {
                rect_outline(c, x + dx, y + dy, 6, 6, col);
            }
        }
        Icon::Mic => {
            rounded_outline(c, x - 3, y - 7, 7, 10, 3, col);
            line(c, x - 5, y + 1, x - 5, y + 2, col);
            line(c, x + 5, y + 1, x + 5, y + 2, col);
            line(c, x - 4, y + 4, x + 4, y + 4, col);
            line(c, x, y + 5, x, y + 7, col);
        }
        Icon::Screen => {
            rect_outline(c, x - 7, y - 6, 15, 10, col);
            line(c, x, y + 4, x, y + 6, col);
            line(c, x - 3, y + 7, x + 3, y + 7, col);
        }
        Icon::Gauge => {
            // Medio círculo con una aguja (el monitor del sistema).
            for i in 0..=8 {
                let (x0, y0) = GAUGE[i];
                let (x1, y1) = GAUGE[(i + 1).min(8)];
                line(c, x + x0, y + y0, x + x1, y + y1, col);
            }
            line(c, x - 7, y + 4, x + 7, y + 4, col);
            line(c, x, y + 3, x + 4, y - 3, col);
        }
        Icon::Camera => {
            rounded_outline(c, x - 7, y - 4, 15, 11, 2, col);
            line(c, x - 3, y - 6, x + 3, y - 6, col);
            circle(c, x, y + 1, 3, col, false);
        }
        Icon::Folder => {
            line(c, x - 7, y - 5, x - 2, y - 5, col);
            line(c, x - 2, y - 5, x, y - 3, col);
            rect_outline(c, x - 7, y - 3, 15, 10, col);
        }
        Icon::Music => {
            line(c, x - 2, y - 6, x - 2, y + 4, col);
            line(c, x + 5, y - 7, x + 5, y + 2, col);
            line(c, x - 2, y - 6, x + 5, y - 7, col);
            circle(c, x - 4, y + 4, 2, col, true);
            circle(c, x + 3, y + 3, 2, col, true);
        }
        Icon::Globe => {
            circle(c, x, y, 7, col, false);
            line(c, x - 7, y, x + 7, y, col);
            line(c, x - 6, y - 4, x + 6, y - 4, col.scale(160));
            line(c, x - 6, y + 4, x + 6, y + 4, col.scale(160));
            rounded_outline(c, x - 3, y - 7, 7, 15, 3, col.scale(200));
        }
        Icon::Chat => {
            rounded_outline(c, x - 7, y - 6, 15, 11, 3, col);
            line(c, x - 4, y + 5, x - 4, y + 7, col);
            line(c, x - 4, y + 7, x - 1, y + 5, col);
        }
        Icon::Document => {
            line(c, x - 5, y - 7, x + 2, y - 7, col);
            line(c, x + 2, y - 7, x + 6, y - 3, col);
            line(c, x + 6, y - 3, x + 6, y + 7, col);
            line(c, x - 5, y + 7, x + 6, y + 7, col);
            line(c, x - 5, y - 7, x - 5, y + 7, col);
            for row in [-1, 2, 5] {
                line(c, x - 3, y + row, x + 3, y + row, col.scale(170));
            }
        }
        Icon::Image => {
            rounded_outline(c, x - 7, y - 6, 15, 13, 2, col);
            line(c, x - 5, y + 4, x - 1, y - 1, col);
            line(c, x - 1, y - 1, x + 2, y + 2, col);
            line(c, x + 2, y + 2, x + 5, y - 1, col);
            circle(c, x + 3, y - 3, 1, col, true);
        }
        Icon::Power => {
            // Círculo abierto arriba y una raya vertical.
            for i in 0..=10 {
                let (x0, y0) = POWER[i];
                let (x1, y1) = POWER[(i + 1).min(10)];
                line(c, x + x0, y + y0, x + x1, y + y1, col);
            }
            line(c, x, y - 7, x, y, col);
        }
        Icon::Restart => {
            for i in 0..=10 {
                let (x0, y0) = POWER[i];
                let (x1, y1) = POWER[(i + 1).min(10)];
                line(c, x + x0, y + y0, x + x1, y + y1, col);
            }
            line(c, x + 3, y - 5, x + 6, y - 7, col);
            line(c, x + 3, y - 5, x + 6, y - 2, col);
        }
        Icon::Lock => {
            rounded_outline(c, x - 4, y - 7, 9, 9, 4, col);
            rect_outline(c, x - 6, y - 1, 13, 9, col);
            line(c, x, y + 2, x, y + 4, col);
        }
        Icon::Gear => {
            circle(c, x, y, 5, col, false);
            circle(c, x, y, 2, col, false);
            for (dx, dy) in [
                (0, -7),
                (0, 7),
                (-7, 0),
                (7, 0),
                (-5, -5),
                (5, 5),
                (-5, 5),
                (5, -5),
            ] {
                let (ix, iy) = (dx * 5 / 7, dy * 5 / 7);
                line(c, x + ix, y + iy, x + dx, y + dy, col);
            }
        }
        Icon::Terminal => {
            rounded_outline(c, x - 8, y - 7, 17, 14, 2, col);
            line(c, x - 5, y - 3, x - 2, y, col);
            line(c, x - 2, y, x - 5, y + 3, col);
            line(c, x, y + 3, x + 5, y + 3, col);
        }
        Icon::Package => {
            rect_outline(c, x - 7, y - 4, 15, 11, col);
            line(c, x - 7, y - 4, x - 4, y - 7, col);
            line(c, x - 4, y - 7, x + 7, y - 7, col);
            line(c, x + 7, y - 7, x + 7, y - 4, col);
            line(c, x - 2, y - 4, x - 2, y, col);
            line(c, x + 2, y - 4, x + 2, y, col);
        }
        Icon::Bell => {
            rounded_outline(c, x - 5, y - 6, 11, 11, 5, col);
            line(c, x - 7, y + 4, x + 7, y + 4, col);
            line(c, x - 5, y, x - 6, y + 4, col);
            line(c, x + 5, y, x + 6, y + 4, col);
            line(c, x - 1, y + 6, x + 1, y + 6, col);
        }
        Icon::Calendar => {
            rect_outline(c, x - 7, y - 5, 15, 12, col);
            line(c, x - 7, y - 2, x + 7, y - 2, col);
            line(c, x - 4, y - 7, x - 4, y - 4, col);
            line(c, x + 4, y - 7, x + 4, y - 4, col);
            for (dx, dy) in [(-4, 1), (0, 1), (4, 1), (-4, 4), (0, 4)] {
                c.put(x + dx, y + dy, col);
            }
        }
        Icon::Download => {
            line(c, x, y - 7, x, y + 2, col);
            line(c, x - 4, y - 2, x, y + 2, col);
            line(c, x + 4, y - 2, x, y + 2, col);
            line(c, x - 7, y + 3, x - 7, y + 6, col);
            line(c, x - 7, y + 6, x + 7, y + 6, col);
            line(c, x + 7, y + 3, x + 7, y + 6, col);
        }
        Icon::Shield => {
            // Escudo: arriba recto con dos "orejas", abajo en punta.
            line(c, x - 7, y - 5, x - 4, y - 8, col);
            line(c, x - 4, y - 8, x + 4, y - 8, col);
            line(c, x + 4, y - 8, x + 7, y - 5, col);
            line(c, x - 7, y - 5, x - 6, y + 2, col);
            line(c, x + 7, y - 5, x + 6, y + 2, col);
            line(c, x - 6, y + 2, x, y + 8, col);
            line(c, x + 6, y + 2, x, y + 8, col);
            line(c, x - 3, y - 2, x, y + 2, col);
            line(c, x + 3, y - 2, x, y + 2, col);
        }
    }
}

/// Puntos de un semicírculo de radio 7 (de izquierda a derecha, por arriba).
const GAUGE: [(i32, i32); 9] = [
    (-7, 4),
    (-7, 1),
    (-5, -3),
    (-3, -5),
    (0, -6),
    (3, -5),
    (5, -3),
    (7, 1),
    (7, 4),
];

/// Círculo de radio 6 abierto arriba (ícono de encendido).
const POWER: [(i32, i32); 11] = [
    (-3, -5),
    (-6, -2),
    (-6, 2),
    (-4, 5),
    (-1, 7),
    (1, 7),
    (4, 5),
    (6, 2),
    (6, -2),
    (3, -5),
    (3, -5),
];

// --- reloj ------------------------------------------------------------------------------------

/// Zona que ocupan la hora y la fecha (incluido el halo de la hora).
pub fn clock_rect(width: usize, _height: usize) -> Rect {
    let right = width as i32 - MARGIN - 8;
    let face = ClockFace::new();
    let ty = MARGIN + 38;
    let pad = face.halo.overhang();
    let date_w = text::width(LONGEST_DATE, &date_style());
    let time_w = face.digits.width("00:00") + 10 + pad;
    let x0 = (right - date_w.max(time_w) - 14).max(0);
    let y1 = ty + TIME_HEIGHT + 14 + date_style().line_height() + 4;
    Rect::new(x0, ty - pad, right + pad - x0, y1 - (ty - pad))
}

pub fn draw_clock(c: &mut Canvas<'_>, face: &mut ClockFace, now: Option<DateTime>, h24: bool) {
    let right = c.width() as i32 - MARGIN - 8;
    let mut time = StrBuf::<8>::new();
    let mut date = StrBuf::<48>::new();
    let mut suffix = "";
    match now {
        Some(t) if !h24 => {
            let h = match t.hour % 12 {
                0 => 12,
                h => h,
            };
            let _ = write!(time, "{h}:{:02}", t.minute);
            suffix = if t.hour < 12 { "A. M." } else { "P. M." };
            let _ = t.write_date(&mut date);
        }
        Some(t) => {
            let _ = t.write_time(&mut time);
            let _ = t.write_date(&mut date);
        }
        None => {
            let _ = time.write_str("--:--");
            let _ = date.write_str("RELOJ NO DISPONIBLE");
        }
    }
    let tw = face.digits.width(time.as_str());
    let ty = MARGIN + 38;
    let tx = right - tw - 10;
    face.halo
        .draw_alpha(c, tx, ty, time.as_str(), theme::CYAN, 22);
    face.digits.draw(c, tx, ty, time.as_str(), Color::WHITE);
    if !suffix.is_empty() {
        text::draw_right(c, tx - 12, ty + TIME_HEIGHT - 18, suffix, &date_style());
    }
    text::draw_right(
        c,
        right - 10,
        ty + TIME_HEIGHT + 14,
        date.as_str(),
        &date_style(),
    );
}

// --- mensaje del asistente --------------------------------------------------------------------

/// Zona del mensaje de abajo a la derecha (hasta ~75 letras).
pub fn message_rect(width: usize, height: usize) -> Rect {
    let (w, h) = (width as i32, height as i32);
    let right = w - MARGIN - 8;
    let x0 = (right - 620).max(0);
    Rect::new(x0, h - MARGIN - 54, right + 4 - x0, 54)
}

/// `speaking`: mientras habla, la línea de abajo dice "HABLANDO" en vez de "HACE UN INSTANTE".
pub fn draw_message(c: &mut Canvas<'_>, msg: &str, speaking: bool) {
    let (w, h) = (c.width() as i32, c.height() as i32);
    let right = w - MARGIN - 8;
    let body = Style::new(Weight::Regular, Size::Size16, theme::TEXT);
    text::draw_right(c, right, h - MARGIN - 50, msg, &body);

    let meta = Style::new(Weight::Light, Size::Size16, theme::TEXT_DIM).tracking(2);
    let who = meta.color(theme::CYAN.scale(200));
    let who_w = text::draw_right(c, right, h - MARGIN - 24, "JARVIS", &who);
    let (status, style) = if speaking {
        ("HABLANDO ·", meta.color(theme::CYAN.scale(150)))
    } else {
        ("HACE UN INSTANTE ·", meta)
    };
    text::draw_right(c, right - who_w - 12, h - MARGIN - 24, status, &style);
}

// --- esfera -----------------------------------------------------------------------------------

/// Posición, tamaño y rotación de la esfera en el instante `now_ms`.
pub fn sphere_view(width: usize, height: usize, now_ms: u64) -> View {
    View {
        cx: width as i32 / 2,
        cy: height as i32 / 2,
        radius: height as i32 * 29 / 100,
        spin: ((now_ms % SPIN_PERIOD_MS) * FULL_TURN as u64 / SPIN_PERIOD_MS) as u32,
        tilt: FULL_TURN * 56 / 1000, // ~20°
    }
}

/// Resplandor extra detrás de la esfera mientras JARVIS habla. Queda dentro de los límites de la
/// esfera (radio × 1,15 < radio × 1,2 de `ParticleCloud::bounds`).
pub fn draw_voice_glow(c: &mut Canvas<'_>, view: &View, pulse: &Pulse) {
    if pulse.glow > 0 {
        glow(
            c,
            view.cx,
            view.cy,
            view.radius * 115 / 100,
            Color::hex(0x1450c8),
            pulse.glow / 2,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use crate::sphere::ParticleCloud;
    use std::vec;

    #[test]
    fn la_capa_estatica_cubre_toda_la_pantalla_en_varios_tamanios() {
        for (w, h) in [(1280, 800), (1024, 768), (1920, 1080), (640, 480)] {
            let mut buf = vec![0u8; w * h * 4];
            let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Bgr).unwrap();
            draw_static(&mut c);
            assert_ne!(c.get(w as i32 - 1, h as i32 - 1), Some(Color::BLACK));
        }
    }

    #[test]
    fn reloj_y_mensaje_quedan_dentro_de_sus_rectangulos() {
        let (w, h) = (1280, 800);
        let now = DateTime {
            year: 2026,
            month: 9,
            day: 23,
            hour: 15,
            minute: 25,
            second: 0,
        };
        // En 24 horas y en 12 (con "P. M." a la izquierda).
        for h24 in [true, false] {
            let mut buf = vec![0u8; w * h * 4];
            let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Rgb).unwrap();
            draw_clock(&mut c, &mut ClockFace::new(), Some(now), h24);
            draw_message(&mut c, "Sistema en línea. ¿En qué te ayudo?", true);
            let (cr, mr) = (clock_rect(w, h), message_rect(w, h));
            for y in 0..h as i32 {
                for x in 0..w as i32 {
                    let px = Rect::new(x, y, 1, 1);
                    if c.get(x, y) != Some(Color::BLACK) {
                        assert!(
                            cr.intersects(&px) || mr.intersects(&px),
                            "píxel suelto en ({x}, {y}) con h24={h24}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn el_resplandor_de_voz_no_sale_de_la_esfera() {
        let view = sphere_view(1280, 800, 0);
        let pulse = Pulse {
            glow: 255,
            ..Pulse::REST
        };
        let (w, h) = (1280, 800);
        let mut buf = vec![0u8; w * h * 4];
        let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Rgb).unwrap();
        draw_voice_glow(&mut c, &view, &pulse);
        let b = ParticleCloud::bounds(&view);
        assert_eq!(c.get(b.x - 1, view.cy), Some(Color::BLACK));
        assert_ne!(c.get(view.cx, view.cy), Some(Color::BLACK));
    }

    #[test]
    fn la_esfera_da_una_vuelta_cada_25_segundos() {
        assert_eq!(sphere_view(1280, 800, 0).spin, 0);
        assert_eq!(sphere_view(1280, 800, 12_500).spin, FULL_TURN / 2);
        assert_eq!(sphere_view(1280, 800, 25_000).spin, 0);
    }
}
