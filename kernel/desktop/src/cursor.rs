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

/// Rectángulo que ocupa el cursor con la punta en (x, y).
pub fn rect(x: i32, y: i32) -> Rect {
    Rect::new(x, y, 11, ARROW.len() as i32)
}

/// Dibuja el cursor con la punta en (x, y). Devuelve la zona que ocupó.
pub fn draw(c: &mut Canvas<'_>, x: i32, y: i32) -> Rect {
    for (dy, row) in ARROW.iter().enumerate() {
        for (dx, ch) in row.bytes().enumerate() {
            let color = match ch {
                b'#' => OUTLINE,
                b'.' => FILL,
                _ => continue,
            };
            c.put(x + dx as i32, y + dy as i32, color);
        }
    }
    rect(x, y)
}
