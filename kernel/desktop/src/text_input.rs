//! Campo de texto de una línea (para nombres de archivos y carpetas).

use alloc::string::String;

use crate::input::Key;

pub struct TextInput {
    pub text: String,
    max_chars: usize,
}

impl TextInput {
    pub fn new(initial: &str, max_chars: usize) -> Self {
        TextInput {
            text: initial.into(),
            max_chars,
        }
    }

    /// Aplica una tecla. Devuelve `true` si el texto cambió.
    pub fn handle(&mut self, key: Key) -> bool {
        match key {
            Key::Char(c) if !c.is_control() && self.text.chars().count() < self.max_chars => {
                self.text.push(c);
                true
            }
            Key::Backspace => self.text.pop().is_some(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escribe_borra_y_respeta_el_maximo() {
        let mut t = TextInput::new("ab", 4);
        assert!(t.handle(Key::Char('c')));
        assert!(t.handle(Key::Char('d')));
        assert!(!t.handle(Key::Char('e')), "llegó al máximo");
        assert!(
            !t.handle(Key::Char('\u{7}')),
            "los caracteres de control no se escriben"
        );
        assert!(t.handle(Key::Backspace));
        assert_eq!(t.text, "abc");
        let mut vacio = TextInput::new("", 10);
        assert!(!vacio.handle(Key::Backspace));
    }
}
