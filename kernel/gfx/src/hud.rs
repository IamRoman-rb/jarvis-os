//! Elementos de la pantalla principal de JARVIS-OS, separados en capas.
//!
//! Distribución (según la imagen de referencia): barra de íconos arriba a la izquierda,
//! "JARVIS" + hora + fecha arriba a la derecha, esfera en el centro, panel de estado y
//! "Control de misión" abajo a la izquierda, y el último mensaje del asistente abajo a la derecha.
//!
//! - **Capa estática** ([`draw_static`]): se dibuja una sola vez en el buffer de fondo.
//! - **Capas dinámicas** (esfera, reloj, mensaje): cada una tiene su rectángulo, que es lo único
//!   que se redibuja cuando cambia. La orquestación está en `scene`.

use core::fmt::Write;

use crate::canvas::Rect;
use crate::clock::{DateTime, StrBuf};
use crate::shapes::{circle, glow, line, rect_outline, rounded_outline, rounded_rect};
use crate::sphere::{Pulse, View};
use crate::text::{self, Size, Style, Weight};
use crate::trig::FULL_TURN;
use crate::{Canvas, Color, theme};

const MARGIN: i32 = 28;
/// Una vuelta de la esfera cada 25 segundos.
const SPIN_PERIOD_MS: u64 = 25_000;
/// La fecha más larga posible, para reservar su espacio.
const LONGEST_DATE: &str = "MIÉRCOLES, 30 DE SEPTIEMBRE DE 2026";

fn time_style() -> Style {
    Style::new(Weight::Bold, Size::Size32, Color::WHITE)
        .scale(2)
        .tracking(4)
}

fn date_style() -> Style {
    Style::new(Weight::Regular, Size::Size16, theme::CYAN.scale(190)).tracking(3)
}

// --- capa estática ----------------------------------------------------------------------------

/// Fondo, grilla, título "JARVIS" y panel de estado. La barra de íconos es dinámica (marca la
/// sección activa): ver [`draw_toolbar`].
pub fn draw_static(c: &mut Canvas<'_>, info: &[(&str, &str)]) {
    background(c);
    let right = c.width() as i32 - MARGIN - 8;
    let title = Style::new(Weight::Light, Size::Size32, theme::TEXT_FAINT)
        .scale(3)
        .tracking(10);
    text::draw_right(c, right, MARGIN - 8, "JARVIS", &title);
    status_panel(c, info);
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

const TOOLBAR_SLOT: i32 = 30;
const TOOLBAR_ICONS: i32 = 6;
/// Ícono de la carpeta (app Archivos) en la barra.
pub const TOOLBAR_FILES: i32 = 3;
/// Ícono del chat (JARVIS) en la barra.
pub const TOOLBAR_JARVIS: i32 = 5;

/// Zona de la barra de íconos de arriba a la izquierda.
pub fn toolbar_rect() -> Rect {
    Rect::new(
        MARGIN + 6,
        MARGIN + 2,
        TOOLBAR_SLOT * TOOLBAR_ICONS + 18,
        34,
    )
}

/// Qué ícono de la barra está en (x, y), si hay alguno.
pub fn toolbar_hit(x: i32, y: i32) -> Option<i32> {
    let r = toolbar_rect();
    if !r.contains(x, y) {
        return None;
    }
    let i = (x - r.x - 9) / TOOLBAR_SLOT;
    (0..TOOLBAR_ICONS).contains(&i).then_some(i)
}

/// Barra de íconos con el ícono `active` resaltado (la sección que se está usando).
pub fn draw_toolbar(c: &mut Canvas<'_>, active: i32) {
    const SLOT: i32 = TOOLBAR_SLOT;
    const ICONS: i32 = TOOLBAR_ICONS;
    let Rect { x, y, w, h } = toolbar_rect();
    rounded_rect(c, x, y, w, h, 12, theme::PANEL, 210);
    rounded_outline(c, x, y, w, h, 12, theme::PANEL_RIM);
    for i in 0..ICONS {
        let (ix, iy) = (x + 9 + i * SLOT + SLOT / 2, y + h / 2);
        let active = i == active;
        if active {
            rounded_rect(c, ix - 12, iy - 12, 24, 24, 6, theme::VECTOR_BLUE, 90);
        }
        let color = if active {
            theme::PARTICLE_BRIGHT
        } else {
            theme::TEXT_DIM
        };
        icon(c, i, ix, iy, color);
    }
    // Separador antes del asistente.
    let sx = x + 9 + (ICONS - 1) * SLOT;
    line(c, sx, y + 9, sx, y + h - 10, theme::PANEL_RIM);
}

/// Íconos de 14×14 px dibujados con líneas: micrófono, pantalla, cámara, carpeta, música, chat.
fn icon(c: &mut Canvas<'_>, which: i32, x: i32, y: i32, col: Color) {
    match which {
        0 => {
            rounded_outline(c, x - 3, y - 7, 7, 10, 3, col);
            line(c, x - 5, y + 1, x - 5, y + 2, col);
            line(c, x + 5, y + 1, x + 5, y + 2, col);
            line(c, x - 4, y + 4, x + 4, y + 4, col);
            line(c, x, y + 5, x, y + 7, col);
        }
        1 => {
            rect_outline(c, x - 7, y - 6, 15, 10, col);
            line(c, x, y + 4, x, y + 6, col);
            line(c, x - 3, y + 7, x + 3, y + 7, col);
        }
        2 => {
            rounded_outline(c, x - 7, y - 4, 15, 11, 2, col);
            line(c, x - 3, y - 6, x + 3, y - 6, col);
            circle(c, x, y + 1, 3, col, false);
        }
        3 => {
            line(c, x - 7, y - 5, x - 2, y - 5, col);
            line(c, x - 2, y - 5, x, y - 3, col);
            rect_outline(c, x - 7, y - 3, 15, 10, col);
        }
        4 => {
            line(c, x - 2, y - 6, x - 2, y + 4, col);
            line(c, x + 5, y - 7, x + 5, y + 2, col);
            line(c, x - 2, y - 6, x + 5, y - 7, col);
            circle(c, x - 4, y + 4, 2, col, true);
            circle(c, x + 3, y + 3, 2, col, true);
        }
        _ => {
            rounded_outline(c, x - 7, y - 6, 15, 11, 3, col);
            line(c, x - 4, y + 5, x - 4, y + 7, col);
            line(c, x - 4, y + 7, x - 1, y + 5, col);
        }
    }
}

fn status_panel(c: &mut Canvas<'_>, rows: &[(&str, &str)]) {
    let h = c.height() as i32;
    let row_h = 22;
    let (w, ph) = (300, 44 + rows.len() as i32 * row_h + 8);
    let (x, y) = (MARGIN - 6, h - MARGIN - 44 - ph);

    rounded_rect(c, x, y, w, ph, 8, theme::PANEL, 200);
    rounded_outline(c, x, y, w, ph, 8, theme::PANEL_RIM);

    let label = Style::new(Weight::Regular, Size::Size16, theme::TEXT_DIM).tracking(2);
    text::draw(c, x + 12, y + 10, "ESTADO", &label);
    let faint = label.color(theme::TEXT_FAINT.lerp(theme::TEXT_DIM, 120));
    text::draw_right(c, x + w - 12, y + 10, "JARVIS-OS", &faint);
    line(c, x + 1, y + 34, x + w - 2, y + 34, theme::PANEL_RIM);

    let key = Style::new(Weight::Light, Size::Size16, theme::TEXT_DIM);
    let value = Style::new(Weight::Regular, Size::Size16, theme::TEXT);
    for (i, (k, v)) in rows.iter().enumerate() {
        let ry = y + 44 + i as i32 * row_h;
        text::draw(c, x + 12, ry, k, &key);
        text::draw_right(c, x + w - 12, ry, v, &value);
    }

    // Píldora "Control de misión" debajo del panel.
    let (px, py, pw, phh) = (x, h - MARGIN - 30, 210, 30);
    rounded_rect(c, px, py, pw, phh, 15, theme::PANEL, 200);
    rounded_outline(c, px, py, pw, phh, 15, theme::PANEL_RIM);
    circle(c, px + 16, py + phh / 2, 3, theme::CYAN, true);
    let pill = Style::new(Weight::Regular, Size::Size16, theme::TEXT_DIM).tracking(2);
    text::draw(c, px + 28, py + 7, "CONTROL DE MISIÓN", &pill);
}

// --- reloj ------------------------------------------------------------------------------------

/// Zona que ocupan la hora y la fecha (incluido el halo de la hora).
pub fn clock_rect(width: usize, _height: usize) -> Rect {
    let right = width as i32 - MARGIN - 8;
    let ty = MARGIN + 34;
    let date_w = text::width(LONGEST_DATE, &date_style());
    let time_w = text::width("00:00", &time_style()) + 18;
    let x0 = (right - date_w.max(time_w) - 14).max(0);
    let y1 = ty + time_style().line_height() + 6 + date_style().line_height() + 4;
    Rect::new(x0, ty - 4, right + 4 - x0, y1 - (ty - 4))
}

pub fn draw_clock(c: &mut Canvas<'_>, now: Option<DateTime>) {
    let right = c.width() as i32 - MARGIN - 8;
    let mut time = StrBuf::<8>::new();
    let mut date = StrBuf::<48>::new();
    match now {
        Some(t) => {
            let _ = t.write_time(&mut time);
            let _ = t.write_date(&mut date);
        }
        None => {
            let _ = time.write_str("--:--");
            let _ = date.write_str("RELOJ NO DISPONIBLE");
        }
    }
    let big = time_style();
    let tw = text::width(time.as_str(), &big);
    let ty = MARGIN + 34;
    text::draw_glowing(
        c,
        right - tw - 18,
        ty,
        time.as_str(),
        &big,
        theme::CYAN.scale(40),
    );
    text::draw_right(
        c,
        right - 10,
        ty + big.line_height() + 6,
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
            draw_static(
                &mut c,
                &[("NÚCLEO", "jarvis 0.1.0"), ("MEMORIA", "511 MiB")],
            );
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
        let mut buf = vec![0u8; w * h * 4];
        let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Rgb).unwrap();
        draw_clock(&mut c, Some(now));
        draw_message(&mut c, "Sistema en línea. ¿En qué te ayudo?", true);
        let (cr, mr) = (clock_rect(w, h), message_rect(w, h));
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let px = Rect::new(x, y, 1, 1);
                if c.get(x, y) != Some(Color::BLACK) {
                    assert!(
                        cr.intersects(&px) || mr.intersects(&px),
                        "píxel suelto en ({x}, {y})"
                    );
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
