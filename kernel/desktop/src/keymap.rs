//! Distribución de teclado latinoamericana (la de la mayoría de los teclados de Argentina,
//! México, Chile…).
//!
//! El kernel decodifica las teclas con la distribución de EE. UU. (la única de `pc-keyboard`
//! parecida) y acá se traduce cada carácter a lo que dice esa misma tecla en un teclado
//! latinoamericano: la tecla de `;` es la `ñ`, la de `[` es el acento `´`, Shift+2 es `"`, AltGr+Q
//! es `@`, etc. Los acentos son **teclas muertas**: `´` y después `a` da `á`.

/// Lo que produce una tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapped {
    Char(char),
    /// Tecla muerta: se combina con la siguiente (`´` + `e` = `é`).
    Dead(char),
}

/// `us`: el carácter que dio la decodificación de EE. UU. (ya con Shift aplicado).
pub fn latam(us: char, altgr: bool) -> Mapped {
    use Mapped::*;
    if altgr {
        return Char(match us {
            'q' | 'Q' => '@',
            '`' | '~' => '¬',
            '-' | '_' => '\\',
            ']' | '}' => '~',
            '\'' | '"' => '^',
            '\\' | '|' => '`',
            other => other,
        });
    }
    match us {
        '`' => Char('|'),
        '~' => Char('°'),
        '@' => Char('"'),
        '^' => Char('&'),
        '&' => Char('/'),
        '*' => Char('('),
        '(' => Char(')'),
        ')' => Char('='),
        '-' => Char('\''),
        '_' => Char('?'),
        '=' => Char('¿'),
        '+' => Char('¡'),
        '[' => Dead('´'),
        '{' => Dead('¨'),
        ']' => Char('+'),
        '}' => Char('*'),
        ';' => Char('ñ'),
        ':' => Char('Ñ'),
        '\'' => Char('{'),
        '"' => Char('['),
        '\\' => Char('}'),
        '|' => Char(']'),
        '<' => Char(';'),
        '>' => Char(':'),
        '/' => Char('-'),
        '?' => Char('_'),
        other => Char(other),
    }
}

/// La tecla extra de los teclados ISO (a la izquierda de la Z).
pub fn latam_extra_key(shift: bool) -> char {
    if shift { '>' } else { '<' }
}

/// Combina las teclas muertas con la tecla siguiente.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeadKeys {
    pending: Option<char>,
}

impl DeadKeys {
    /// Devuelve hasta dos caracteres para escribir.
    pub fn feed(&mut self, m: Mapped) -> [Option<char>; 2] {
        match (self.pending.take(), m) {
            (None, Mapped::Char(c)) => [Some(c), None],
            (None, Mapped::Dead(d)) => {
                self.pending = Some(d);
                [None, None]
            }
            // Dos veces la tecla muerta, o muerta + espacio: el acento solo.
            (Some(d), Mapped::Char(' ')) => [Some(d), None],
            (Some(d), Mapped::Dead(d2)) if d == d2 => [Some(d), None],
            (Some(d), Mapped::Dead(d2)) => {
                self.pending = Some(d2);
                [Some(d), None]
            }
            (Some(d), Mapped::Char(c)) => match compose(d, c) {
                Some(x) => [Some(x), None],
                None => [Some(d), Some(c)],
            },
        }
    }
}

fn compose(dead: char, c: char) -> Option<char> {
    Some(match (dead, c) {
        ('´', 'a') => 'á',
        ('´', 'e') => 'é',
        ('´', 'i') => 'í',
        ('´', 'o') => 'ó',
        ('´', 'u') => 'ú',
        ('´', 'A') => 'Á',
        ('´', 'E') => 'É',
        ('´', 'I') => 'Í',
        ('´', 'O') => 'Ó',
        ('´', 'U') => 'Ú',
        ('´', 'y') => 'ý',
        ('¨', 'u') => 'ü',
        ('¨', 'U') => 'Ü',
        ('¨', 'a') => 'ä',
        ('¨', 'e') => 'ë',
        ('¨', 'i') => 'ï',
        ('¨', 'o') => 'ö',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_keys(keys: &[(char, bool)]) -> alloc::string::String {
        let mut dk = DeadKeys::default();
        let mut out = alloc::string::String::new();
        for &(k, altgr) in keys {
            for c in dk.feed(latam(k, altgr)).into_iter().flatten() {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn simbolos_ene_y_tildes() {
        let plain = |s: &str| {
            s.chars()
                .map(|c| (c, false))
                .collect::<alloc::vec::Vec<_>>()
        };
        // "canción" en un teclado latino: c a n c i [ o n  (la tecla [ es el acento).
        assert_eq!(type_keys(&plain("canci[on")), "canción");
        assert_eq!(type_keys(&plain("espa;a")), "españa");
        assert_eq!(type_keys(&plain("{u")), "ü");
        assert_eq!(
            type_keys(&plain("[x")),
            "´x",
            "sin combinación: el acento y la letra"
        );
        assert_eq!(type_keys(&[('q', true)]), "@");
        assert_eq!(type_keys(&plain("ls ` grep /")), "ls | grep -");
        assert_eq!(type_keys(&plain("&*)")), "/(=");
        assert_eq!(latam_extra_key(true), '>');
    }
}
