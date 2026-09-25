//! Piezas de interfaz que comparten las apps: tarjetas, botones, barras y gráficos.

use jarvis_gfx::shapes::{line, rounded_outline, rounded_rect};
use jarvis_gfx::text::{self, Size, Style, Weight};
use jarvis_gfx::{Canvas, Color, Rect, theme};

use crate::system::Series;

/// Fondo de las ventanas, de los campos de texto y de la fila elegida (según el tema).
pub fn window_bg() -> Color {
    theme::window()
}

pub fn field_bg() -> Color {
    theme::field()
}

pub fn selected_bg() -> Color {
    theme::selected()
}

/// Texto normal de la interfaz (16 px, o 20 con "texto grande" en Tipografía).
pub fn s16(color: Color) -> Style {
    Style::new(Weight::Regular, crate::look::ui_size(), color)
}

pub fn light(color: Color) -> Style {
    Style::new(Weight::Light, crate::look::ui_size(), color)
}

pub fn bold(color: Color) -> Style {
    Style::new(Weight::Bold, crate::look::ui_size(), color)
}

/// Etiqueta en mayúsculas espaciadas (el estilo del HUD).
pub fn label(color: Color) -> Style {
    Style::new(Weight::Regular, crate::look::ui_size(), color).tracking(2)
}

pub fn big(color: Color) -> Style {
    Style::new(Weight::Bold, Size::Size32, color)
}

/// Panel con borde y un título arriba a la izquierda. Devuelve la zona de adentro.
pub fn card(c: &mut Canvas<'_>, r: Rect, title: &str) -> Rect {
    rounded_rect(c, r.x, r.y, r.w, r.h, 6, theme::panel(), 255);
    rounded_outline(c, r.x, r.y, r.w, r.h, 6, theme::panel_rim());
    if !title.is_empty() {
        text::draw(c, r.x + 14, r.y + 10, title, &label(theme::text_dim()));
        Rect::new(r.x + 14, r.y + 34, r.w - 28, r.h - 44)
    } else {
        Rect::new(r.x + 12, r.y + 10, r.w - 24, r.h - 20)
    }
}

/// Botón con texto centrado. `fill`: opacidad del relleno (0 = solo borde).
pub fn button(c: &mut Canvas<'_>, r: Rect, text_label: &str, color: Color, fill: u8) {
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

/// Ancho de un botón para `text_label`.
pub fn button_width(text_label: &str) -> i32 {
    text::width(text_label, &label(theme::text())) + 24
}

/// Barra de progreso (0..=100).
pub fn bar(c: &mut Canvas<'_>, r: Rect, pct: u32, color: Color) {
    let rad = (r.h / 2).min(3);
    rounded_rect(c, r.x, r.y, r.w, r.h, rad, theme::panel_rim(), 255);
    let pct = pct.min(100) as i32;
    let filled = (r.w * pct / 100).max(if pct > 0 { 2 } else { 0 });
    rounded_rect(c, r.x, r.y, filled, r.h, rad, color, 255);
}

/// Gráfico de área de una serie. `max`: el valor que llega arriba (0 = el máximo de la serie).
pub fn graph<const N: usize>(c: &mut Canvas<'_>, r: Rect, s: &Series<N>, max: u32, color: Color) {
    c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
    // Líneas de guía cada cuarto.
    for i in 1..4 {
        let y = r.y + r.h * i / 4;
        let mut x = r.x;
        while x < r.x + r.w {
            c.put(x, y, theme::panel_rim());
            x += 4;
        }
    }
    let max = if max == 0 { s.max().max(1) } else { max } as i64;
    let n = s.capacity() as i32;
    let step = |i: i32| r.x + (r.w - 1) * i / (n - 1).max(1);
    // Las muestras quedan pegadas a la derecha (la más nueva en el borde).
    let offset = n - s.len() as i32;
    let mut prev: Option<(i32, i32)> = None;
    for (i, v) in s.iter().enumerate() {
        let x = step(offset + i as i32);
        let h = ((v as i64).min(max) * (r.h - 2) as i64 / max) as i32;
        let y = r.y + r.h - 1 - h;
        if let Some((px, py)) = prev {
            // Relleno tenue debajo de la línea.
            for xx in px..=x {
                let yy = py + (y - py) * (xx - px) / (x - px).max(1);
                for fy in yy..r.y + r.h {
                    c.blend(xx, fy, color, 40);
                }
            }
            line(c, px, py, x, y, color);
        }
        prev = Some((x, y));
    }
    rounded_outline(c, r.x, r.y, r.w, r.h, 2, theme::panel_rim());
}

/// Texto que no pasa de `max_w` (le agrega ".." si hay que cortarlo).
pub fn draw_fit(c: &mut Canvas<'_>, x: i32, y: i32, s: &str, st: &Style, max_w: i32) -> i32 {
    text::draw(c, x, y, &text::fit(s, st, max_w), st)
}

/// "512 B/s", "3.4 KiB/s"
pub fn rate(bytes_per_sec: u32) -> alloc::string::String {
    let mut s = crate::files::format_size(bytes_per_sec as u64);
    s.push_str("/s");
    s
}

/// "1 h 02 min", "5 min 03 s"
pub fn duration(ms: u64) -> alloc::string::String {
    let s = ms / 1000;
    let (h, m, sec) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        alloc::format!("{h} h {m:02} min")
    } else {
        alloc::format!("{m} min {sec:02} s")
    }
}

/// "10.0.2.15"
pub fn ip(a: [u8; 4]) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatos() {
        assert_eq!(duration(65_000), "1 min 05 s");
        assert_eq!(duration(3_720_000), "1 h 02 min");
        assert_eq!(ip([10, 0, 2, 15]), "10.0.2.15");
        assert_eq!(rate(2048), "2.0 KiB/s");
    }
}
