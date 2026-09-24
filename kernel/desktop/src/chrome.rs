//! El marco de cada ventana: borde, barra de título con el ícono y el nombre de la app, y los
//! botones minimizar (—), maximizar (▢) y cerrar (✕), como en Windows.

use jarvis_gfx::hud::{Icon, icon};
use jarvis_gfx::shapes::{line, rect_outline};
use jarvis_gfx::{Canvas, Color, Rect, theme};

use crate::widgets::{draw_fit, s16};
use crate::wm::{BUTTON_W, TITLE_H};

/// Qué botón tiene el mouse encima (se ilumina).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hover {
    None,
    Minimize,
    Maximize,
    Close,
}

const TITLE_FOCUSED: Color = Color::hex(0x0c2036);
const TITLE_BLUR: Color = Color::hex(0x0a1422);

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
    let bar = if focused { TITLE_FOCUSED } else { TITLE_BLUR };
    c.fill_rect(0, 0, w, TITLE_H, bar);
    let text_col = if focused {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    icon(
        c,
        app_icon,
        18,
        TITLE_H / 2,
        if focused {
            theme::CYAN
        } else {
            theme::TEXT_DIM
        },
    );
    draw_fit(
        c,
        38,
        (TITLE_H - 16) / 2,
        title,
        &s16(text_col),
        w - 38 - 3 * BUTTON_W - 12,
    );

    // Botones: (izquierda, qué es).
    let close_x = w - BUTTON_W - 1;
    let buttons = [
        (close_x - 2 * BUTTON_W, Hover::Minimize),
        (close_x - BUTTON_W, Hover::Maximize),
        (close_x, Hover::Close),
    ];
    for (x, which) in buttons {
        let r = Rect::new(x, 1, BUTTON_W, TITLE_H - 1);
        if hover == which {
            let bg = if which == Hover::Close {
                theme::CRIMSON.scale(210)
            } else {
                theme::PANEL_RIM
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
        theme::CYAN.scale(150)
    } else {
        theme::PANEL_RIM
    };
    rect_outline(c, 0, 0, w, h, rim);
    line(c, 1, TITLE_H - 1, w - 2, TITLE_H - 1, theme::PANEL_RIM);
    // Marca de la esquina para cambiar el tamaño.
    if !maximized {
        for i in 0..3 {
            let d = 4 + i * 4;
            line(c, w - 3 - d, h - 3, w - 3, h - 3 - d, theme::PANEL_RIM);
        }
    }
}
