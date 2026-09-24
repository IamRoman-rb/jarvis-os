//! JSON mínimo: lo justo para leer los datos que traen adentro algunas páginas (YouTube manda
//! la lista de videos como un objeto JSON dentro de un `<script>`).
//!
//! Los números se guardan como texto (no hace falta hacer cuentas con ellos, y así no hay que
//! convertir a punto flotante).

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn at(&self, i: usize) -> Option<&Json> {
        match self {
            Json::Arr(v) => v.get(i),
            _ => None,
        }
    }

    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(v) => v,
            _ => &[],
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) | Json::Num(s) => Some(s),
            _ => None,
        }
    }

    /// Un camino de claves: `v.path(&["a", "b"])`.
    pub fn path(&self, keys: &[&str]) -> Option<&Json> {
        let mut cur = self;
        for k in keys {
            cur = cur.get(k)?;
        }
        Some(cur)
    }
}

struct P<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    depth: u32,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        if self.depth > 200 {
            return None;
        }
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                self.depth += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        break;
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.b.get(self.i) != Some(&b':') {
                        return None;
                    }
                    self.i += 1;
                    let val = self.value()?;
                    v.push((k, val));
                    self.ws();
                    match self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                self.depth -= 1;
                Some(Json::Obj(v))
            }
            b'[' => {
                self.i += 1;
                self.depth += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.b.get(self.i) == Some(&b']') {
                        self.i += 1;
                        break;
                    }
                    v.push(self.value()?);
                    self.ws();
                    match self.b.get(self.i)? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            break;
                        }
                        _ => return None,
                    }
                }
                self.depth -= 1;
                Some(Json::Arr(v))
            }
            b'"' => self.string().map(Json::Str),
            b't' if self.s[self.i..].starts_with("true") => {
                self.i += 4;
                Some(Json::Bool(true))
            }
            b'f' if self.s[self.i..].starts_with("false") => {
                self.i += 5;
                Some(Json::Bool(false))
            }
            b'n' if self.s[self.i..].starts_with("null") => {
                self.i += 4;
                Some(Json::Null)
            }
            _ => {
                let start = self.i;
                while self.i < self.b.len()
                    && matches!(
                        self.b[self.i],
                        b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'
                    )
                {
                    self.i += 1;
                }
                (self.i > start).then(|| Json::Num(self.s[start..self.i].into()))
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' && self.b[self.i] != b'\\' {
                self.i += 1;
            }
            out.push_str(self.s.get(start..self.i)?);
            match *self.b.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    self.i += 1;
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => {}
                        b'b' | b'f' => {}
                        b'u' => {
                            let hex = self.s.get(self.i..self.i + 4)?;
                            self.i += 4;
                            let mut cp = u32::from_str_radix(hex, 16).ok()?;
                            // Pares sustitutos (emojis y demás fuera del plano básico).
                            if (0xD800..0xDC00).contains(&cp)
                                && self.s[self.i..].starts_with("\\u")
                                && let Some(lo) = self
                                    .s
                                    .get(self.i + 2..self.i + 6)
                                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                                    .filter(|l| (0xDC00..0xE000).contains(l))
                            {
                                self.i += 6;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            }
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        other => out.push(other as char),
                    }
                }
            }
        }
    }
}

/// Lee un valor JSON al principio de `s`. Devuelve el valor y cuántos bytes ocupó.
pub fn parse_prefix(s: &str) -> Option<(Json, usize)> {
    let mut p = P {
        b: s.as_bytes(),
        s,
        i: 0,
        depth: 0,
    };
    let v = p.value()?;
    Some((v, p.i))
}

pub fn parse(s: &str) -> Option<Json> {
    parse_prefix(s).map(|(v, _)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_objetos_listas_y_escapes() {
        let v = parse(r#" {"a": [1, -2.5e3, true, null], "b": {"c": "hóla\n\"x\" 😀"}} resto"#)
            .unwrap();
        assert_eq!(
            v.path(&["b", "c"]).and_then(Json::str),
            Some("hóla\n\"x\" 😀")
        );
        assert_eq!(v.get("a").unwrap().arr().len(), 4);
        assert_eq!(
            v.get("a").and_then(|a| a.at(1)).and_then(Json::str),
            Some("-2.5e3")
        );
        let (_, used) = parse_prefix(r#"{"x":1};var y"#).unwrap();
        assert_eq!(used, 7);
        assert!(parse(r#"{"roto": "#).is_none());
    }
}
