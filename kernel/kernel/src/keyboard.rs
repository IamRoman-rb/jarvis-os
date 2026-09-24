//! Teclado PS/2: la interrupción guarda los scancodes en una cola sin locks y el bucle principal
//! los traduce a teclas (`pc-keyboard`) y después a los eventos del escritorio.

use jarvis_desktop::Key;
use pc_keyboard::{DecodedKey, HandleControl, KeyCode, PS2Keyboard, ScancodeSet1, layouts};

use crate::queue::ByteQueue;

static QUEUE: ByteQueue<128> = ByteQueue::new();

/// La llama el manejador de IRQ1.
pub fn push_scancode(code: u8) {
    QUEUE.push(code);
}

/// Decodificador (vive en el bucle principal). Layout US por ahora: `pc-keyboard` no trae el
/// latinoamericano.
pub struct Keyboard(PS2Keyboard<layouts::Us104Key, ScancodeSet1>);

impl Keyboard {
    pub fn new() -> Self {
        Keyboard(PS2Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore,
        ))
    }

    /// Próxima tecla presionada, ya traducida a una tecla del escritorio.
    pub fn next_key(&mut self) -> Option<Key> {
        while let Some(code) = QUEUE.pop() {
            if let Ok(Some(event)) = self.0.add_byte(code)
                && let Some(decoded) = self.0.process_keyevent(event)
                && let Some(key) = translate(decoded)
            {
                return Some(key);
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
            KeyCode::F2 => Key::F2,
            KeyCode::F6 => Key::F6,
            KeyCode::F7 => Key::F7,
            KeyCode::Delete => Key::Delete,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Escape => Key::Escape,
            KeyCode::Tab => Key::Tab,
            KeyCode::Return => Key::Enter,
            _ => return None,
        },
    })
}
