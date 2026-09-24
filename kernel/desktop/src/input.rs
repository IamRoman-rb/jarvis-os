//! Eventos de entrada, independientes del hardware (el kernel traduce teclado y mouse a esto).

/// Teclas que usa el escritorio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Escape,
    Tab,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    PrintScreen,
    /// F1..F12
    F(u8),
}

/// Teclas modificadoras apretadas. El kernel manda un [`Event::Mods`] cada vez que cambian, y
/// el escritorio las combina con la tecla siguiente (Alt+Tab, Win+D, Ctrl+C…).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// La tecla Windows (en Linux, "Super").
    pub win: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
        win: false,
    };
    pub const CTRL: Mods = Mods {
        ctrl: true,
        ..Mods::NONE
    };
    pub const ALT: Mods = Mods {
        alt: true,
        ..Mods::NONE
    };
    pub const WIN: Mods = Mods {
        win: true,
        ..Mods::NONE
    };
    pub const SHIFT: Mods = Mods {
        shift: true,
        ..Mods::NONE
    };
}

/// Un paquete del mouse: movimiento relativo y estado de los botones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MousePacket {
    pub dx: i32,
    /// Positivo = hacia abajo (como la pantalla).
    pub dy: i32,
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    /// Rueda: positivo = hacia abajo (hacia el final de la página).
    pub wheel: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    Mods(Mods),
    Mouse(MousePacket),
}

/// Arma paquetes de 3 bytes del protocolo de mouse PS/2:
///
/// ```text
/// byte 0: Y_desborde X_desborde Y_signo X_signo 1 medio derecho izquierdo
/// byte 1: movimiento X (8 bits bajos; el signo está en el byte 0)
/// byte 2: movimiento Y (positivo = hacia arriba)
/// ```
///
/// Con la rueda activada (modo "IntelliMouse", ver `kernel/src/mouse.rs`) llega un cuarto byte
/// con el giro de la rueda: 4 bits con signo.
///
/// Si se pierde un byte, los paquetes quedan corridos. Por eso se descartan bytes hasta encontrar
/// uno con el bit 3 en 1, que siempre está en el primer byte de un paquete.
/// Referencia: <https://wiki.osdev.org/PS/2_Mouse>.
#[derive(Default)]
pub struct MouseDecoder {
    buf: [u8; 4],
    len: usize,
    wheel: bool,
}

impl MouseDecoder {
    pub const fn new() -> Self {
        MouseDecoder {
            buf: [0; 4],
            len: 0,
            wheel: false,
        }
    }

    /// Paquetes de 4 bytes (mouse con rueda).
    pub const fn with_wheel() -> Self {
        MouseDecoder {
            buf: [0; 4],
            len: 0,
            wheel: true,
        }
    }

    pub fn push(&mut self, byte: u8) -> Option<MousePacket> {
        if self.len == 0 && byte & 0x08 == 0 {
            return None;
        }
        self.buf[self.len] = byte;
        self.len += 1;
        if self.len < if self.wheel { 4 } else { 3 } {
            return None;
        }
        self.len = 0;
        let flags = self.buf[0];
        if flags & 0xC0 != 0 {
            return None; // desborde: el movimiento no es confiable
        }
        let dx = self.buf[1] as i32 - if flags & 0x10 != 0 { 256 } else { 0 };
        let dy = self.buf[2] as i32 - if flags & 0x20 != 0 { 256 } else { 0 };
        let wheel = if self.wheel {
            // 4 bits con signo: 0x0F = -1 (hacia arriba), 0x01 = +1 (hacia abajo).
            ((self.buf[3] as i8) << 4 >> 4) as i32
        } else {
            0
        };
        Some(MousePacket {
            dx,
            dy: -dy,
            left: flags & 0x01 != 0,
            right: flags & 0x02 != 0,
            middle: flags & 0x04 != 0,
            wheel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodifica_movimiento_y_botones() {
        let mut d = MouseDecoder::new();
        assert_eq!(d.push(0x09), None);
        assert_eq!(d.push(5), None);
        // Y = 3 hacia arriba → dy = -3 en pantalla. Botón izquierdo apretado.
        assert_eq!(
            d.push(3),
            Some(MousePacket {
                dx: 5,
                dy: -3,
                left: true,
                right: false,
                middle: false,
                wheel: 0,
            })
        );
        // Negativos: X = -2 (signo en bit 4), Y = -1 (bit 5) → dy = +1.
        assert_eq!(
            [0x38, 0xFE, 0xFF]
                .into_iter()
                .filter_map(|b| d.push(b))
                .next(),
            Some(MousePacket {
                dx: -2,
                dy: 1,
                ..Default::default()
            })
        );
    }

    #[test]
    fn se_resincroniza_si_se_pierde_un_byte() {
        let mut d = MouseDecoder::new();
        // Basura sin el bit 3 → se descarta hasta un primer byte válido.
        let packets: Vec<_> = [0x00, 0x05, 0x08, 1, 1]
            .into_iter()
            .filter_map(|b| d.push(b))
            .collect();
        assert_eq!(
            packets,
            [MousePacket {
                dx: 1,
                dy: -1,
                ..Default::default()
            }]
        );
    }

    #[test]
    fn descarta_desbordes() {
        let mut d = MouseDecoder::new();
        assert_eq!(
            [0x48, 100, 100]
                .into_iter()
                .filter_map(|b| d.push(b))
                .next(),
            None
        );
    }

    #[test]
    fn rueda_en_el_cuarto_byte() {
        let mut d = MouseDecoder::with_wheel();
        let p: Vec<_> = [0x08, 0, 0, 0x0F, 0x08, 0, 0, 0x02]
            .into_iter()
            .filter_map(|b| d.push(b))
            .collect();
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].wheel, p[1].wheel), (-1, 2));
    }

    extern crate std;
    use std::vec::Vec;
}
