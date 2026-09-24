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
    F2,
    F6,
    F7,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
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
/// Si se pierde un byte, los paquetes quedan corridos. Por eso se descartan bytes hasta encontrar
/// uno con el bit 3 en 1, que siempre está en el primer byte de un paquete.
/// Referencia: <https://wiki.osdev.org/PS/2_Mouse>.
#[derive(Default)]
pub struct MouseDecoder {
    buf: [u8; 3],
    len: usize,
}

impl MouseDecoder {
    pub const fn new() -> Self {
        MouseDecoder {
            buf: [0; 3],
            len: 0,
        }
    }

    pub fn push(&mut self, byte: u8) -> Option<MousePacket> {
        if self.len == 0 && byte & 0x08 == 0 {
            return None;
        }
        self.buf[self.len] = byte;
        self.len += 1;
        if self.len < 3 {
            return None;
        }
        self.len = 0;
        let flags = self.buf[0];
        if flags & 0xC0 != 0 {
            return None; // desborde: el movimiento no es confiable
        }
        let dx = self.buf[1] as i32 - if flags & 0x10 != 0 { 256 } else { 0 };
        let dy = self.buf[2] as i32 - if flags & 0x20 != 0 { 256 } else { 0 };
        Some(MousePacket {
            dx,
            dy: -dy,
            left: flags & 0x01 != 0,
            right: flags & 0x02 != 0,
            middle: flags & 0x04 != 0,
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
                middle: false
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

    extern crate std;
    use std::vec::Vec;
}
