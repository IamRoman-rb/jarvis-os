//! Teclado PS/2: la interrupción guarda los scancodes en una cola sin locks y el bucle principal
//! los traduce a teclas (`pc-keyboard`) y después a los eventos del escritorio.
//!
//! Los modificadores (Shift, Ctrl, Alt, Windows) se siguen acá, mirando cuándo se aprietan y se
//! sueltan, y se le avisan al escritorio con un evento aparte: así sabe, por ejemplo, cuándo se
//! soltó Alt para terminar un Alt+Tab, o que la tecla Windows se apretó sola (menú de inicio).

use jarvis_desktop::{Event, Key, Mods};
use pc_keyboard::{
    DecodedKey, HandleControl, KeyCode, KeyEvent, KeyState, PS2Keyboard, ScancodeSet1, layouts,
};

use crate::queue::ByteQueue;

static QUEUE: ByteQueue<128> = ByteQueue::new();

/// La llama el manejador de IRQ1.
pub fn push_scancode(code: u8) {
    QUEUE.push(code);
}

/// Decodificador (vive en el bucle principal). Layout US por ahora: `pc-keyboard` no trae el
/// latinoamericano.
pub struct Keyboard {
    decoder: PS2Keyboard<layouts::Us104Key, ScancodeSet1>,
    mods: Mods,
    /// Se apretaron los dos Shift o los dos Ctrl: se cuentan por separado.
    held: [bool; 6],
}

const L_SHIFT: usize = 0;
const R_SHIFT: usize = 1;
const L_CTRL: usize = 2;
const R_CTRL: usize = 3;
const ALT: usize = 4;
const WIN: usize = 5;

impl Keyboard {
    pub fn new() -> Self {
        Keyboard {
            decoder: PS2Keyboard::new(
                ScancodeSet1::new(),
                layouts::Us104Key,
                HandleControl::Ignore,
            ),
            mods: Mods::NONE,
            held: [false; 6],
        }
    }

    /// Si la tecla es un modificador, actualiza el estado y devuelve el evento (si cambió).
    fn modifier(&mut self, ev: &KeyEvent) -> Option<Option<Event>> {
        let slot = match ev.code {
            KeyCode::LShift => L_SHIFT,
            KeyCode::RShift => R_SHIFT,
            KeyCode::LControl => L_CTRL,
            KeyCode::RControl => R_CTRL,
            KeyCode::LAlt => ALT,
            KeyCode::LWin | KeyCode::RWin => WIN,
            _ => return None,
        };
        self.held[slot] = ev.state != KeyState::Up;
        let mods = Mods {
            shift: self.held[L_SHIFT] || self.held[R_SHIFT],
            ctrl: self.held[L_CTRL] || self.held[R_CTRL],
            alt: self.held[ALT],
            win: self.held[WIN],
        };
        if mods == self.mods {
            return Some(None);
        }
        self.mods = mods;
        Some(Some(Event::Mods(mods)))
    }

    /// Próximo evento del teclado: una tecla, o un cambio en los modificadores.
    pub fn next_event(&mut self) -> Option<Event> {
        while let Some(code) = QUEUE.pop() {
            let Ok(Some(ev)) = self.decoder.add_byte(code) else {
                continue;
            };
            if let Some(result) = self.modifier(&ev) {
                // El decodificador también tiene que enterarse (para Shift → mayúsculas).
                let _ = self.decoder.process_keyevent(ev);
                match result {
                    Some(e) => return Some(e),
                    None => continue,
                }
            }
            if let Some(decoded) = self.decoder.process_keyevent(ev)
                && let Some(key) = translate(decoded)
            {
                return Some(Event::Key(key));
            }
        }
        None
    }
}

fn translate(key: DecodedKey) -> Option<Key> {
    Some(match key {
        DecodedKey::Unicode('\n' | '\r') => Key::Enter,
        DecodedKey::Unicode('\u{8}') => Key::Backspace,
        DecodedKey::Unicode('\u{1b}') => Key::Escape,
        DecodedKey::Unicode('\t') => Key::Tab,
        DecodedKey::Unicode('\u{7f}') => Key::Delete,
        DecodedKey::Unicode(c) => Key::Char(c),
        DecodedKey::RawKey(code) => match code {
            KeyCode::ArrowUp => Key::Up,
            KeyCode::ArrowDown => Key::Down,
            KeyCode::ArrowLeft => Key::Left,
            KeyCode::ArrowRight => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            KeyCode::PageUp => Key::PageUp,
            KeyCode::PageDown => Key::PageDown,
            KeyCode::Insert => Key::Insert,
            KeyCode::PrintScreen | KeyCode::SysRq => Key::PrintScreen,
            KeyCode::F1 => Key::F(1),
            KeyCode::F2 => Key::F(2),
            KeyCode::F3 => Key::F(3),
            KeyCode::F4 => Key::F(4),
            KeyCode::F5 => Key::F(5),
            KeyCode::F6 => Key::F(6),
            KeyCode::F7 => Key::F(7),
            KeyCode::F8 => Key::F(8),
            KeyCode::F9 => Key::F(9),
            KeyCode::F10 => Key::F(10),
            KeyCode::F11 => Key::F(11),
            KeyCode::F12 => Key::F(12),
            KeyCode::Delete => Key::Delete,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Escape => Key::Escape,
            KeyCode::Tab => Key::Tab,
            KeyCode::Return => Key::Enter,
            _ => return None,
        },
    })
}
