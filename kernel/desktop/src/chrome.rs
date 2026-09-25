//! El marco de cada ventana: borde, barra de título con el ícono y el nombre de la app, y los
//! botones minimizar (—), maximizar (▢) y cerrar (✕), como en Windows.

use jarvis_gfx::hud::{Icon, icon};
use jarvis_gfx::shapes::{line, rect_outline};
use jarvis_gfx::{Canvas, Color, theme};

use crate::widgets::{bold, draw_fit, s16};
use crate::wm::{BUTTON_W, TITLE_H, button_rects};

/// Qué botón tiene el mouse encima (se ilumina).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hover {
    None,
    Minimize,
    Maximize,
    Close,
}

/// Barra de título con foco: el fondo de la ventana teñido con el azul del tema.
fn title_focused() -> Color {
    theme::window().lerp(theme::vector_blue(), 30)
}

fn title_blur() -> Color {
    theme::window().lerp(theme::panel(), 128)
}

/// Dibuja el marco en un buffer de `w` × `h` (la ventana entera, con la esquina en 0, 0).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    c: &mut Canvas<'_>,
    w: i32,
    h: i32,
    app_icon: Icon,
    title: &str,
    focused: bool,
    maximized: bool,
    hover: Hover,
) {
    let bar = if focused {
        title_focused()
    } else {
        title_blur()
    };
    c.fill_rect(0, 0, w, TITLE_H, bar);
    let text_col = if focused {
        theme::text()
    } else {
        theme::text_dim()
    };
    // Con los botones a la izquierda, el ícono y el título van después de ellos.
    let left = if crate::look::buttons_left() {
        3 * BUTTON_W + 4
    } else {
        0
    };
    icon(
        c,
        app_icon,
        left + 18,
        TITLE_H / 2,
        if focused {
            theme::cyan()
        } else {
            theme::text_dim()
        },
    );
    let st = if crate::look::bold_titles() {
        bold(text_col)
    } else {
        s16(text_col)
    };
    draw_fit(
        c,
        left + 38,
        (TITLE_H - st.line_height()) / 2,
        title,
        &st,
        w - 38 - 3 * BUTTON_W - 12,
    );

    let (min, max, close) = button_rects(w);
    let buttons = [
        (min, Hover::Minimize),
        (max, Hover::Maximize),
        (close, Hover::Close),
    ];
    for (r, which) in buttons {
        if hover == which {
            let bg = if which == Hover::Close {
                theme::crimson().scale(210)
            } else {
                theme::panel_rim()
            };
            c.fill_rect(r.x, r.y, r.w, r.h, bg);
        }
        let col = if hover == which {
            Color::WHITE
        } else {
            text_col
        };
        let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
        match which {
            Hover::Minimize => line(c, cx - 5, cy, cx + 5, cy, col),
            Hover::Maximize if maximized => {
                // Dos ventanitas superpuestas: "restaurar".
                rect_outline(c, cx - 5, cy - 2, 8, 8, col);
                line(c, cx - 2, cy - 5, cx + 5, cy - 5, col);
                line(c, cx + 5, cy - 5, cx + 5, cy + 2, col);
            }
            Hover::Maximize => rect_outline(c, cx - 5, cy - 5, 11, 11, col),
            Hover::Close => {
                line(c, cx - 5, cy - 5, cx + 5, cy + 5, col);
                line(c, cx - 5, cy + 5, cx + 5, cy - 5, col);
            }
            Hover::None => {}
        }
    }
    // Borde de toda la ventana: celeste si tiene el foco.
    let rim = if focused {
        theme::cyan().scale(150)
    } else {
        theme::panel_rim()
    };
    rect_outline(c, 0, 0, w, h, rim);
    line(c, 1, TITLE_H - 1, w - 2, TITLE_H - 1, theme::panel_rim());
    // Marca de la esquina para cambiar el tamaño.
    if !maximized {
        for i in 0..3 {
            let d = 4 + i * 4;
            line(c, w - 3 - d, h - 3, w - 3, h - 3 - d, theme::panel_rim());
        }
    }
}
