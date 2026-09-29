//! Expresiones regulares básicas para `grep` (como las de `grep -E`, sin grupos):
//! `.` cualquier carácter, `*` `+` `?` repeticiones, `^` `$` principio y fin, `[abc]` `[a-z]`
//! `[^0-9]` clases y `\` para escapar.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug)]
enum Atom {
    Any,
    Char(char),
    /// (negada, rangos)
    Class(bool, Vec<(char, char)>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rep {
    One,
    Star,
    Plus,
    Opt,
}

#[derive(Clone, Debug)]
pub struct Regex {
    items: Vec<(Atom, Rep)>,
    anchored_start: bool,
    anchored_end: bool,
    ignore_case: bool,
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

impl Atom {
    fn matches(&self, c: char, ic: bool) -> bool {
        match self {
            Atom::Any => true,
            Atom::Char(x) => {
                if ic {
                    lower(*x) == lower(c)
                } else {
                    *x == c
                }
            }
            Atom::Class(neg, ranges) => {
                let test = |c: char| ranges.iter().any(|&(a, b)| a <= c && c <= b);
                let hit = test(c) || (ic && (test(lower(c)) || c.to_uppercase().any(test)));
                hit != *neg
            }
        }
    }
}

impl Regex {
    pub fn new(pattern: &str, ignore_case: bool) -> Result<Regex, &'static str> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut items: Vec<(Atom, Rep)> = Vec::new();
        let mut i = 0;
        let anchored_start = chars.first() == Some(&'^');
        if anchored_start {
            i = 1;
        }
        let mut anchored_end = false;
        while i < chars.len() {
            let c = chars[i];
            let atom = match c {
                '$' if i + 1 == chars.len() => {
                    anchored_end = true;
                    i += 1;
                    continue;
                }
                '.' => Atom::Any,
                '\\' => {
                    i += 1;
                    Atom::Char(*chars.get(i).ok_or("\\ al final del patrón")?)
                }
                '[' => {
                    let mut j = i + 1;
                    let neg = chars.get(j) == Some(&'^');
                    if neg {
                        j += 1;
                    }
                    let mut ranges = Vec::new();
                    let mut first = true;
                    while j < chars.len() && (chars[j] != ']' || first) {
                        let a = chars[j];
                        if chars.get(j + 1) == Some(&'-')
                            && chars.get(j + 2).is_some_and(|&b| b != ']')
                        {
                            ranges.push((a, chars[j + 2]));
                            j += 3;
                        } else {
                            ranges.push((a, a));
                            j += 1;
                        }
                        first = false;
                    }
                    if j >= chars.len() {
                        return Err("falta cerrar el corchete [");
                    }
                    i = j;
                    Atom::Class(neg, ranges)
                }
                '*' | '+' | '?' if items.is_empty() => Atom::Char(c),
                c => Atom::Char(c),
            };
            i += 1;
            let rep = match chars.get(i) {
                Some('*') => Rep::Star,
                Some('+') => Rep::Plus,
                Some('?') => Rep::Opt,
                _ => Rep::One,
            };
            if rep != Rep::One {
                i += 1;
            }
            items.push((atom, rep));
        }
        Ok(Regex {
            items,
            anchored_start,
            anchored_end,
            ignore_case,
        })
    }

    pub fn is_match(&self, text: &str) -> bool {
        let t: Vec<char> = text.chars().collect();
        self.find_chars(&t, 0).is_some()
    }

    /// La primera coincidencia desde `from`: (inicio, fin) en caracteres.
    pub fn find_chars(&self, t: &[char], from: usize) -> Option<(usize, usize)> {
        if self.anchored_start {
            return if from == 0 {
                self.at(t, 0, 0).map(|e| (0, e))
            } else {
                None
            };
        }
        (from..=t.len()).find_map(|s| self.at(t, s, 0).map(|e| (s, e)))
    }

    /// Dónde termina la coincidencia que empieza en `pos` (la más larga posible).
    fn at(&self, t: &[char], pos: usize, item: usize) -> Option<usize> {
        let Some((atom, rep)) = self.items.get(item) else {
            return (!self.anchored_end || pos == t.len()).then_some(pos);
        };
        let ic = self.ignore_case;
        let one = |p: usize| p < t.len() && atom.matches(t[p], ic);
        match rep {
            Rep::One => {
                if one(pos) {
                    self.at(t, pos + 1, item + 1)
                } else {
                    None
                }
            }
            Rep::Opt => (if one(pos) {
                self.at(t, pos + 1, item + 1)
            } else {
                None
            })
            .or_else(|| self.at(t, pos, item + 1)),
            Rep::Star | Rep::Plus => {
                let mut end = pos;
                while one(end) {
                    end += 1;
                }
                let min = if *rep == Rep::Plus { pos + 1 } else { pos };
                if end < min {
                    return None;
                }
                (min..=end).rev().find_map(|p| self.at(t, p, item + 1))
            }
        }
    }

    /// Reemplaza la primera coincidencia (o todas) por `rep` (`&` = lo que coincidió).
    pub fn replace(&self, text: &str, rep: &str, all: bool) -> String {
        let t: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut pos = 0;
        while pos <= t.len() {
            let Some((s, e)) = self.find_chars(&t, pos) else {
                break;
            };
            out.extend(&t[pos..s]);
            let matched: String = t[s..e].iter().collect();
            out.push_str(&rep.replace('&', &matched));
            if e == s {
                // Coincidencia vacía: se avanza un carácter para no quedar en un bucle.
                if s < t.len() {
                    out.push(t[s]);
                }
                pos = s + 1;
            } else {
                pos = e;
            }
            if !all {
                break;
            }
        }
        if pos <= t.len() {
            out.extend(&t[pos.min(t.len())..]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> bool {
        Regex::new(p, false).unwrap().is_match(t)
    }

    #[test]
    fn patrones() {
        assert!(m("hola", "¡hola mundo!"));
        assert!(m("^ho", "hola") && !m("^ol", "hola"));
        assert!(m("do$", "mundo") && !m("mun$", "mundo"));
        assert!(
            m("h.la", "hola") && m("ho*la", "hla") && m("ho+la", "hoola") && !m("ho+la", "hla")
        );
        assert!(m("colou?r", "color") && m("colou?r", "colour"));
        assert!(m("[0-9]+ kb", "tiene 12 kb") && !m("^[^0-9]*$", "a1"));
        assert!(m("\\.txt$", "notas.txt") && !m("\\.txt$", "notasXtxt"));
        assert!(Regex::new("HOLA", true).unwrap().is_match("hola"));
        assert!(Regex::new("[a", false).is_err());
        assert!(m("", "cualquier cosa"));
        let r = Regex::new("o+", false).unwrap();
        assert_eq!(r.replace("hoola mundo", "0", true), "h0la mund0");
        assert_eq!(r.replace("hoola mundo", "[&]", false), "h[oo]la mundo");
        assert_eq!(
            Regex::new(".", false).unwrap().replace("abc", "_", true),
            "___"
        );
    }
}
