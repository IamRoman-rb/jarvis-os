//! Del texto de una línea a una lista de comandos, como hace `sh`:
//!
//! ```text
//! ls -l "Mis cosas" | grep txt > lista.txt && echo listo; cd ~
//! └──────── tubería (2 comandos) ──┘ └ redirección      └ otra tubería
//! ```
//!
//! Las palabras se guardan "crudas" (con sus partes entre comillas, variables `$X` y
//! sustituciones `$(…)`) y se expanden recién al ejecutar, así `$?` vale lo del comando anterior.

use alloc::string::String;
use alloc::vec::Vec;

/// Un pedazo de una palabra.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Texto literal. `quoted`: venía entre comillas (no se expanden `*` ni `?`).
    Lit(String, bool),
    /// `$NOMBRE`, `${NOMBRE}`, `$?`, `$1`… `quoted`: entre comillas dobles (no se parte en
    /// palabras).
    Var(String, bool),
    /// `$(comando)`.
    Sub(String, bool),
}

pub type Word = Vec<Part>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redir {
    /// `> archivo`
    Out(Word),
    /// `>> archivo`
    Append(Word),
    /// `< archivo`
    In(Word),
    /// `2> archivo` (los errores; `2>/dev/null` los descarta)
    Err(Word),
    /// `2>&1`
    ErrToOut,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<Word>,
    pub redirs: Vec<Redir>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connector {
    /// `;` o fin de línea: el siguiente se ejecuta siempre.
    Always,
    /// `&&`: solo si el anterior salió bien.
    And,
    /// `||`: solo si el anterior falló.
    Or,
}

/// Una tubería (`a | b | c`) y cómo se une con la siguiente.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    pub next: Connector,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Word(Word),
    Pipe,
    And,
    Or,
    Semi,
    Out,
    Append,
    In,
    ErrOut,
    ErrToOut,
}

/// Lee `$algo` desde `chars[i]` (que es el `$`). Devuelve la parte y dónde sigue.
fn dollar(chars: &[char], i: usize, quoted: bool) -> Result<(Part, usize), String> {
    let next = chars.get(i + 1).copied();
    match next {
        Some('(') => {
            // $( ... ) con paréntesis anidados.
            let mut depth = 1;
            let mut j = i + 2;
            let mut inner = String::new();
            while j < chars.len() {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok((Part::Sub(inner, quoted), j + 1));
                        }
                    }
                    _ => {}
                }
                inner.push(chars[j]);
                j += 1;
            }
            Err("falta cerrar el paréntesis de $(".into())
        }
        Some('{') => {
            let end = chars[i + 2..]
                .iter()
                .position(|&c| c == '}')
                .ok_or("falta cerrar la llave de ${")?;
            let name: String = chars[i + 2..i + 2 + end].iter().collect();
            Ok((Part::Var(name, quoted), i + 3 + end))
        }
        Some(c) if c == '?' || c == '#' || c == '@' || c == '$' || c.is_ascii_digit() => {
            Ok((Part::Var(c.into(), quoted), i + 2))
        }
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                name.push(chars[j]);
                j += 1;
            }
            Ok((Part::Var(name, quoted), j))
        }
        // Un `$` suelto es un `$`.
        _ => Ok((Part::Lit("$".into(), quoted), i + 1)),
    }
}

fn push_lit(word: &mut Word, c: char, quoted: bool) {
    if let Some(Part::Lit(s, q)) = word.last_mut()
        && *q == quoted
    {
        s.push(c);
        return;
    }
    word.push(Part::Lit(c.into(), quoted));
}

fn tokenize(line: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = line.chars().collect();
    let mut toks = Vec::new();
    let mut word: Word = Vec::new();
    let mut in_word = false;
    let mut i = 0;
    macro_rules! end_word {
        () => {
            if in_word {
                toks.push(Tok::Word(core::mem::take(&mut word)));
                in_word = false;
            }
        };
    }
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => {
                end_word!();
                i += 1;
            }
            '#' if !in_word => break, // comentario hasta el final
            '\'' => {
                in_word = true;
                let end = chars[i + 1..]
                    .iter()
                    .position(|&c| c == '\'')
                    .ok_or("falta cerrar una comilla simple (')")?;
                if end == 0 {
                    word.push(Part::Lit(String::new(), true));
                }
                for &ch in &chars[i + 1..i + 1 + end] {
                    push_lit(&mut word, ch, true);
                }
                i += end + 2;
            }
            '"' => {
                in_word = true;
                let mut j = i + 1;
                let mut empty = true;
                loop {
                    let Some(&ch) = chars.get(j) else {
                        return Err("falta cerrar una comilla doble (\")".into());
                    };
                    match ch {
                        '"' => break,
                        '\\' if matches!(chars.get(j + 1), Some('"' | '\\' | '$' | '`')) => {
                            push_lit(&mut word, chars[j + 1], true);
                            j += 2;
                        }
                        '$' => {
                            let (part, next) = dollar(&chars, j, true)?;
                            word.push(part);
                            j = next;
                        }
                        _ => {
                            push_lit(&mut word, ch, true);
                            j += 1;
                        }
                    }
                    empty = false;
                }
                if empty {
                    word.push(Part::Lit(String::new(), true));
                }
                i = j + 1;
            }
            '\\' => {
                in_word = true;
                if let Some(&next) = chars.get(i + 1) {
                    push_lit(&mut word, next, true);
                }
                i += 2;
            }
            '$' => {
                in_word = true;
                let (part, next) = dollar(&chars, i, false)?;
                word.push(part);
                i = next;
            }
            '|' => {
                end_word!();
                if chars.get(i + 1) == Some(&'|') {
                    toks.push(Tok::Or);
                    i += 2;
                } else {
                    toks.push(Tok::Pipe);
                    i += 1;
                }
            }
            '&' => {
                end_word!();
                if chars.get(i + 1) == Some(&'&') {
                    toks.push(Tok::And);
                    i += 2;
                } else {
                    // Sin procesos en segundo plano: `&` se toma como `;`.
                    toks.push(Tok::Semi);
                    i += 1;
                }
            }
            ';' => {
                end_word!();
                toks.push(Tok::Semi);
                i += 1;
            }
            '>' => {
                // `2>` y `2>&1`: el 2 tiene que estar solo, pegado al `>`.
                let err = in_word && word == [Part::Lit("2".into(), false)];
                if err {
                    word.clear();
                    in_word = false;
                } else {
                    end_word!();
                }
                if err && chars.get(i + 1) == Some(&'&') && chars.get(i + 2) == Some(&'1') {
                    toks.push(Tok::ErrToOut);
                    i += 3;
                } else if err {
                    toks.push(Tok::ErrOut);
                    i += 1;
                } else if chars.get(i + 1) == Some(&'>') {
                    toks.push(Tok::Append);
                    i += 2;
                } else {
                    toks.push(Tok::Out);
                    i += 1;
                }
            }
            '<' => {
                end_word!();
                toks.push(Tok::In);
                i += 1;
            }
            _ => {
                in_word = true;
                push_lit(&mut word, c, false);
                i += 1;
            }
        }
    }
    if in_word {
        toks.push(Tok::Word(word));
    }
    Ok(toks)
}

/// Parte una línea en tuberías.
pub fn parse(line: &str) -> Result<Vec<Pipeline>, String> {
    let toks = tokenize(line)?;
    let mut out: Vec<Pipeline> = Vec::new();
    let mut cmds: Vec<Command> = Vec::new();
    let mut cmd = Command::default();
    let mut it = toks.into_iter().peekable();
    let finish_cmd = |cmd: &mut Command, cmds: &mut Vec<Command>| -> Result<(), String> {
        if cmd.words.is_empty() {
            return Err("falta un comando".into());
        }
        cmds.push(core::mem::take(cmd));
        Ok(())
    };
    while let Some(t) = it.next() {
        match t {
            Tok::Word(w) => cmd.words.push(w),
            Tok::Out | Tok::Append | Tok::In | Tok::ErrOut => {
                let Some(Tok::Word(target)) = it.next() else {
                    return Err("falta el archivo después de la redirección".into());
                };
                cmd.redirs.push(match t {
                    Tok::Out => Redir::Out(target),
                    Tok::Append => Redir::Append(target),
                    Tok::In => Redir::In(target),
                    _ => Redir::Err(target),
                });
            }
            Tok::ErrToOut => cmd.redirs.push(Redir::ErrToOut),
            Tok::Pipe => finish_cmd(&mut cmd, &mut cmds)?,
            Tok::And | Tok::Or | Tok::Semi => {
                if cmd.words.is_empty() && cmds.is_empty() && t == Tok::Semi {
                    continue; // `;;` o `;` al principio
                }
                finish_cmd(&mut cmd, &mut cmds)?;
                let next = match t {
                    Tok::And => Connector::And,
                    Tok::Or => Connector::Or,
                    _ => Connector::Always,
                };
                out.push(Pipeline {
                    commands: core::mem::take(&mut cmds),
                    next,
                });
            }
        }
    }
    if !cmd.words.is_empty() || !cmd.redirs.is_empty() {
        finish_cmd(&mut cmd, &mut cmds)?;
    }
    if !cmds.is_empty() {
        out.push(Pipeline {
            commands: cmds,
            next: Connector::Always,
        });
    } else if out.last().is_some_and(|p| p.next != Connector::Always) {
        return Err("falta un comando después de && o ||".into());
    }
    Ok(out)
}

/// ¿La palabra tiene comodines (`*`, `?`) sin comillas?
pub fn has_glob(w: &Word) -> bool {
    w.iter()
        .any(|p| matches!(p, Part::Lit(s, false) if s.contains(['*', '?'])))
}

/// `*.txt` contra `notas.txt` (sin distinguir mayúsculas, como FAT).
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn lit(s: &str) -> Word {
        alloc::vec![Part::Lit(s.to_string(), false)]
    }

    #[test]
    fn tuberias_redirecciones_y_conectores() {
        let p = parse("ls -l | grep txt > lista.txt && echo listo; cd").unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].commands.len(), 2);
        assert_eq!(p[0].commands[0].words, [lit("ls"), lit("-l")]);
        assert_eq!(p[0].commands[1].redirs, [Redir::Out(lit("lista.txt"))]);
        assert_eq!(p[0].next, Connector::And);
        assert_eq!(p[1].next, Connector::Always);
        assert_eq!(p[2].commands[0].words, [lit("cd")]);
    }

    #[test]
    fn comillas_variables_y_sustituciones() {
        let p = parse(r#"echo 'a $B' "c $D ${E}x" $(ls "x y") \$f 2>/dev/null"#).unwrap();
        let w = &p[0].commands[0].words;
        assert_eq!(w[1], [Part::Lit("a $B".into(), true)]);
        assert_eq!(
            w[2],
            [
                Part::Lit("c ".into(), true),
                Part::Var("D".into(), true),
                Part::Lit(" ".into(), true),
                Part::Var("E".into(), true),
                Part::Lit("x".into(), true)
            ]
        );
        assert_eq!(w[3], [Part::Sub("ls \"x y\"".into(), false)]);
        assert_eq!(
            w[4],
            [Part::Lit("$".into(), true), Part::Lit("f".into(), false)]
        );
        assert_eq!(p[0].commands[0].redirs, [Redir::Err(lit("/dev/null"))]);
        assert_eq!(
            parse(r#"echo """#).unwrap()[0].commands[0].words[1],
            [Part::Lit(String::new(), true)]
        );
    }

    #[test]
    fn errores_de_sintaxis() {
        for bad in ["echo 'hola", "| ls", "ls &&", "ls >", "echo $(ls"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse("").unwrap().is_empty());
        assert!(parse("   # solo un comentario").unwrap().is_empty());
    }

    #[test]
    fn comodines() {
        assert!(glob_match("*.txt", "Notas.TXT"));
        assert!(glob_match("n?tas*", "notas.txt"));
        assert!(!glob_match("*.txt", "notas.md"));
        assert!(glob_match("*", ""));
        assert!(has_glob(&lit("*.txt")));
        assert!(!has_glob(&alloc::vec![Part::Lit("*.txt".into(), true)]));
    }
}
