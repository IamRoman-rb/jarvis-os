//! Cursor del mouse por software.
//!
//! No hay hardware que dibuje el cursor: se pinta directo sobre la pantalla **después** de copiar
//! el frame, y antes de moverlo se restaura lo que tapaba desde el frame. Así el cursor nunca
//! "ensucia" el buffer donde se compone la imagen.

use jarvis_gfx::{Canvas, Color, Rect};

/// `#` = borde oscuro, `.` = relleno claro.
const ARROW: [&str; 17] = [
    "#",
    "##",
    "#.#",
    "#..#",
    "#...#",
    "#....#",
    "#.....#",
    "#......#",
    "#.......#",
    "#........#",
    "#.....#####",
    "#..#..#",
    "#.# #..#",
    "##  #..#",
    "#    #..#",
    "     #..#",
    "      ##",
];

const OUTLINE: Color = Color::hex(0x050b14);
const FILL: Color = Color::hex(0xeafcff);

/// 1 = normal, 2 = grande (Configuración → Barra de tareas y cursor).
fn scale() -> i32 {
    if crate::look::cursor_big() { 2 } else { 1 }
}

/// Rectángulo que ocupa el cursor con la punta en (x, y).
pub fn rect(x: i32, y: i32) -> Rect {
    let k = scale();
    Rect::new(x, y, 11 * k, ARROW.len() as i32 * k)
}

/// Dibuja el cursor con la punta en (x, y). Devuelve la zona que ocupó.
pub fn draw(c: &mut Canvas<'_>, x: i32, y: i32) -> Rect {
    let k = scale();
    for (dy, row) in ARROW.iter().enumerate() {
        for (dx, ch) in row.bytes().enumerate() {
            let color = match ch {
                b'#' => OUTLINE,
                b'.' => FILL,
                _ => continue,
            };
            let (px, py) = (x + dx as i32 * k, y + dy as i32 * k);
            if k == 1 {
                c.put(px, py, color);
            } else {
                c.fill_rect(px, py, k, k, color);
            }
        }
    }
    rect(x, y)
}
