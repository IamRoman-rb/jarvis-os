//! Composición de la pantalla principal de JARVIS-OS.
//!
//! Distribución (según la imagen de referencia): barra de íconos arriba a la izquierda,
//! "JARVIS" + hora + fecha arriba a la derecha, esfera en el centro, panel de estado y
//! "Control de misión" abajo a la izquierda, y el último mensaje del asistente abajo a la derecha.

use core::fmt::Write;

use crate::clock::{DateTime, StrBuf};
use crate::shapes::{circle, glow, line, rect_outline, rounded_outline, rounded_rect};
use crate::sphere;
use crate::text::{self, Size, Style, Weight};
use crate::{Canvas, Color, theme};

/// Lo que el kernel sabe al momento de dibujar.
pub struct Hud<'a> {
    /// Hora local. `None` si el reloj del hardware devolvió algo inválido.
    pub now: Option<DateTime>,
    /// Último mensaje del asistente (abajo a la derecha).
    pub message: &'a str,
    /// Filas del panel de estado: (etiqueta, valor).
    pub info: &'a [(&'a str, &'a str)],
    /// Cantidad de partículas de la esfera.
    pub particles: u32,
}

const MARGIN: i32 = 28;

pub fn draw(c: &mut Canvas<'_>, hud: &Hud<'_>) {
    background(c);
    sphere::centered(c, hud.particles).draw(c);
    toolbar(c);
    clock(c, hud.now);
    status_panel(c, hud.info);
    message(c, hud.message);
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

fn toolbar(c: &mut Canvas<'_>) {
    const SLOT: i32 = 30;
    const ICONS: i32 = 6;
    let (x, y, h) = (MARGIN + 6, MARGIN + 2, 34);
    let w = SLOT * ICONS + 18;
    rounded_rect(c, x, y, w, h, 12, theme::PANEL, 210);
    rounded_outline(c, x, y, w, h, 12, theme::PANEL_RIM);
    for i in 0..ICONS {
        let (ix, iy) = (x + 9 + i * SLOT + SLOT / 2, y + h / 2);
        let active = i == ICONS - 1; // el asistente: la sección activa
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

fn clock(c: &mut Canvas<'_>, now: Option<DateTime>) {
    let right = c.width() as i32 - MARGIN - 8;
    let title = Style::new(Weight::Light, Size::Size32, theme::TEXT_FAINT)
        .scale(3)
        .tracking(10);
    text::draw_right(c, right, MARGIN - 8, "JARVIS", &title);

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
    let big = Style::new(Weight::Bold, Size::Size32, Color::WHITE)
        .scale(2)
        .tracking(4);
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

    let small = Style::new(Weight::Regular, Size::Size16, theme::CYAN.scale(190)).tracking(3);
    text::draw_right(
        c,
        right - 10,
        ty + big.line_height() + 6,
        date.as_str(),
        &small,
    );
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
    text::draw_right(
        c,
        x + w - 12,
        y + 10,
        "JARVIS-OS",
        &label.color(theme::TEXT_FAINT.lerp(theme::TEXT_DIM, 120)),
    );
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

fn message(c: &mut Canvas<'_>, msg: &str) {
    let (w, h) = (c.width() as i32, c.height() as i32);
    let right = w - MARGIN - 8;
    let body = Style::new(Weight::Regular, Size::Size16, theme::TEXT);
    text::draw_right(c, right, h - MARGIN - 50, msg, &body);

    let meta = Style::new(Weight::Light, Size::Size16, theme::TEXT_DIM).tracking(2);
    let who = meta.color(theme::CYAN.scale(200));
    let who_w = text::draw_right(c, right, h - MARGIN - 24, "JARVIS", &who);
    text::draw_right(
        c,
        right - who_w - 12,
        h - MARGIN - 24,
        "HACE UN INSTANTE ·",
        &meta,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PixelFormat;
    use std::vec;

    #[test]
    fn dibuja_toda_la_pantalla_sin_panic_en_varios_tamanios() {
        for (w, h) in [(1280, 800), (1024, 768), (1920, 1080), (640, 480)] {
            let mut buf = vec![0u8; w * h * 4];
            let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Bgr).unwrap();
            let hud = Hud {
                now: Some(DateTime {
                    year: 2026,
                    month: 9,
                    day: 23,
                    hour: 15,
                    minute: 25,
                    second: 0,
                }),
                message: "Sistema en línea. ¿En qué te ayudo?",
                info: &[("NÚCLEO", "jarvis 0.1.0"), ("MEMORIA", "511 MiB")],
                particles: 4000,
            };
            draw(&mut c, &hud);
            // El fondo no queda negro puro: se pintó algo en toda la pantalla.
            assert_ne!(c.get(w as i32 - 1, h as i32 - 1), Some(Color::BLACK));
        }
    }

    #[test]
    fn sin_reloj_muestra_guiones() {
        let (w, h) = (800, 600);
        let mut buf = vec![0u8; w * h * 4];
        let mut c = Canvas::new(&mut buf, w, h, w, 4, PixelFormat::Rgb).unwrap();
        draw(
            &mut c,
            &Hud {
                now: None,
                message: "",
                info: &[],
                particles: 100,
            },
        );
    }
}
