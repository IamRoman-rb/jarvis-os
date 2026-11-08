//! Reglas tipo **YARA** para el antivirus de JARVIS-OS (ADR 0024), escritas desde cero.
//!
//! Las firmas por hash (MalwareBazaar) solo reconocen un archivo idéntico: cambiar un byte las
//! esquiva. Una **regla** describe pedazos que el malware no puede cambiar sin dejar de
//! funcionar (un texto, una secuencia de instrucciones) y una condición sobre ellos. La sintaxis
//! es la de YARA (VirusTotal), en un subconjunto, para poder leer reglas públicas:
//!
//! ```text
//! rule Minero_XMRig : minero {
//!     meta:
//!         description = "Minero de criptomonedas XMRig"
//!         severity = "alta"
//!     strings:
//!         $a = "stratum+tcp://" nocase
//!         $b = "xmrig" nocase wide ascii
//!         $c = { 4D 5A ?? 00 [2-4] 50 45 }
//!     condition:
//!         $a and ($b or #a > 2) and filesize < 20MB
//! }
//! ```
//!
//! - **Cadenas**: texto (con `nocase`, `wide` = UTF-16 y `ascii`) y hexadecimal (con `??`,
//!   comodines de medio byte `4?` y saltos `[n-m]`).
//! - **Condiciones**: `and`, `or`, `not`, paréntesis, `$a`, `#a > n` (cuántas veces),
//!   `$a at n`, `$a in (n..m)`, `any of them`, `all of them`, `2 of ($a, $b*)`, `filesize`,
//!   `uint8/16/32(n)` (y `be`), números con `KB` y `MB`, `true`, `false`.
//!
//! **Cómo busca**: todas las cadenas a la vez con un autómata de **Aho-Corasick** (una sola
//! pasada por el archivo, sin importar cuántas reglas haya): un árbol con todos los patrones y
//! "enlaces de falla" que dicen dónde seguir cuando un byte no continúa el patrón actual. Las
//! hexadecimales con comodines se buscan por su pedazo fijo más largo y después se verifican.

#![no_std]

extern crate alloc;

pub mod heuristica;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Cuántas coincidencias de una cadena se recuerdan (para `#a`): más no cambian nada.
const MAX_HITS: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub rule: String,
    pub msg: String,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.rule.is_empty() {
            f.write_str(&self.msg)
        } else {
            write!(f, "regla {}: {}", self.rule, self.msg)
        }
    }
}

/// Un elemento de una cadena hexadecimal.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Hex {
    /// Un byte con máscara (`4?` = valor 0x40, máscara 0xF0; `??` = máscara 0).
    Byte(u8, u8),
    /// Un salto de entre `min` y `max` bytes cualesquiera.
    Jump(usize, usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pattern {
    /// Bytes exactos (ya en minúsculas si `nocase`).
    Bytes {
        bytes: Vec<u8>,
        nocase: bool,
    },
    Hex(Vec<Hex>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StringDef {
    id: String,
    /// Las variantes a buscar (ascii y/o wide): todas cuentan como la misma cadena.
    variants: Vec<Pattern>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cmp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    fn eval(&self, a: u64, b: u64) -> bool {
        match self {
            Cmp::Eq => a == b,
            Cmp::Ne => a != b,
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Quant {
    Any,
    All,
    N(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Expr {
    True,
    False,
    Str(usize),
    Count(usize, Cmp, u64),
    At(usize, u64),
    In(usize, u64, u64),
    Of(Quant, Vec<usize>),
    FileSize(Cmp, u64),
    /// uint de `bytes` bytes en `offset`, big endian o no, comparado con un número.
    Int {
        bytes: usize,
        big: bool,
        offset: u64,
        cmp: Cmp,
        value: u64,
    },
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

/// Una regla compilada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub name: String,
    pub tags: Vec<String>,
    pub meta: Vec<(String, String)>,
    strings: Vec<StringDef>,
    cond: Expr,
}

impl Rule {
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.meta
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// "alta" (se pone en cuarentena), "media" (se avisa) o "baja".
    pub fn severity(&self) -> &str {
        self.meta("severity").unwrap_or("media")
    }
}

/// Una coincidencia.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub rule: String,
    pub description: String,
    pub severity: String,
}

// --- lexer ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Var(String),   // $a, $b*, $
    Count(String), // #a
    Offset(String),
    Str(String),
    Hex(String),
    Num(u64),
    Sym(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == b'"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    match b[i] {
                        b'n' => s.push('\n'),
                        b't' => s.push('\t'),
                        b'r' => s.push('\r'),
                        b'0' => s.push('\0'),
                        b'x' => {
                            let h = src.get(i + 1..i + 3).ok_or("\\x cortado")?;
                            let v = u8::from_str_radix(h, 16).map_err(|_| "\\x inválido")?;
                            s.push(char::from(v));
                            i += 2;
                        }
                        other => s.push(char::from(other)),
                    }
                    i += 1;
                } else {
                    // UTF-8 entero.
                    let ch = src[i..].chars().next().ok_or("texto inválido")?;
                    s.push(ch);
                    i += ch.len_utf8();
                }
            }
            if i >= b.len() {
                return Err("falta cerrar una comilla".into());
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if c == b'{' && matches!(out.last(), Some(Tok::Sym("="))) {
            // Una cadena hexadecimal: hasta la llave que cierra.
            let end = src[i..]
                .find('}')
                .ok_or("falta cerrar una cadena hexadecimal")?;
            out.push(Tok::Hex(src[i + 1..i + end].to_string()));
            i += end + 1;
        } else if c == b'$' || c == b'#' || c == b'@' {
            let start = i + 1;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'*') {
                i += 1;
            }
            let name = src[start..i].to_string();
            out.push(match c {
                b'$' => Tok::Var(name),
                b'#' => Tok::Count(name),
                _ => Tok::Offset(name),
            });
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            let t = &src[start..i];
            let (num, mult) = if let Some(n) = t.strip_suffix("KB") {
                (n, 1024)
            } else if let Some(n) = t.strip_suffix("MB") {
                (n, 1024 * 1024)
            } else {
                (t, 1)
            };
            let v = if let Some(h) = num.strip_prefix("0x") {
                u64::from_str_radix(h, 16)
            } else {
                num.parse()
            }
            .map_err(|_| format!("número inválido «{t}»"))?;
            out.push(Tok::Num(v.saturating_mul(mult)));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(src[start..i].to_string()));
        } else {
            let two = src.get(i..i + 2).unwrap_or("");
            let sym: &'static str = match two {
                "==" => "==",
                "!=" => "!=",
                "<=" => "<=",
                ">=" => ">=",
                ".." => "..",
                _ => match c {
                    b'{' => "{",
                    b'}' => "}",
                    b'(' => "(",
                    b')' => ")",
                    b'[' => "[",
                    b']' => "]",
                    b':' => ":",
                    b'=' => "=",
                    b'<' => "<",
                    b'>' => ">",
                    b',' => ",",
                    _ => return Err(format!("carácter inesperado «{}»", char::from(c))),
                },
            };
            i += sym.len();
            out.push(Tok::Sym(sym));
        }
    }
    Ok(out)
}

// --- parser --------------------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect_sym(&mut self, s: &str) -> Result<(), String> {
        match self.next() {
            Some(Tok::Sym(x)) if x == s => Ok(()),
            other => Err(format!("se esperaba «{s}» y vino {other:?}")),
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            other => Err(format!("se esperaba un nombre y vino {other:?}")),
        }
    }

    fn is_ident(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == word)
    }
}

fn parse_hex(src: &str) -> Result<Vec<Hex>, String> {
    let mut out = Vec::new();
    let s: String = src.chars().filter(|c| !c.is_whitespace()).collect();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' {
            let end = s[i..].find(']').ok_or("salto sin cerrar")? + i;
            let inner = &s[i + 1..end];
            let (lo, hi) = match inner.split_once('-') {
                Some((a, b)) => (
                    a.parse().map_err(|_| "salto inválido")?,
                    b.parse().map_err(|_| "salto inválido")?,
                ),
                None => {
                    let n = inner.parse().map_err(|_| "salto inválido")?;
                    (n, n)
                }
            };
            if lo > hi || hi > 4096 {
                return Err("salto inválido".into());
            }
            out.push(Hex::Jump(lo, hi));
            i = end + 1;
        } else if b[i] == b'(' || b[i] == b'|' {
            return Err("las alternativas ( | ) en hexadecimal no están soportadas".into());
        } else {
            let pair = s.get(i..i + 2).ok_or("byte hexadecimal cortado")?;
            let mut value = 0u8;
            let mut mask = 0u8;
            for (k, ch) in pair.chars().enumerate() {
                let shift = if k == 0 { 4 } else { 0 };
                if ch != '?' {
                    let d = ch.to_digit(16).ok_or("dígito hexadecimal inválido")? as u8;
                    value |= d << shift;
                    mask |= 0xF << shift;
                }
            }
            out.push(Hex::Byte(value, mask));
            i += 2;
        }
    }
    if !matches!(out.first(), Some(Hex::Byte(..))) || !matches!(out.last(), Some(Hex::Byte(..))) {
        return Err("una cadena hexadecimal no puede empezar ni terminar con un salto".into());
    }
    Ok(out)
}

fn wide(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|&b| [b, 0]).collect()
}

fn parse_strings(p: &mut Parser) -> Result<Vec<StringDef>, String> {
    let mut out: Vec<StringDef> = Vec::new();
    while let Some(Tok::Var(id)) = p.peek().cloned() {
        p.next();
        p.expect_sym("=")?;
        let def = match p.next() {
            Some(Tok::Str(s)) => {
                let (mut nocase, mut is_wide, mut ascii) = (false, false, false);
                loop {
                    if p.is_ident("nocase") {
                        nocase = true;
                    } else if p.is_ident("wide") {
                        is_wide = true;
                    } else if p.is_ident("ascii") {
                        ascii = true;
                    } else if p.is_ident("fullword") || p.is_ident("private") {
                        // Se aceptan y no cambian la búsqueda.
                    } else {
                        break;
                    }
                    p.next();
                }
                let mut raw: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
                if s.chars().any(|c| c as u32 > 0xFF) {
                    raw = s.as_bytes().to_vec();
                }
                if raw.is_empty() {
                    return Err(format!("${id} está vacía"));
                }
                if nocase {
                    raw.make_ascii_lowercase();
                }
                let mut variants = Vec::new();
                if !is_wide || ascii {
                    variants.push(Pattern::Bytes {
                        bytes: raw.clone(),
                        nocase,
                    });
                }
                if is_wide {
                    variants.push(Pattern::Bytes {
                        bytes: wide(&raw),
                        nocase,
                    });
                }
                StringDef {
                    id: id.clone(),
                    variants,
                }
            }
            Some(Tok::Hex(h)) => StringDef {
                id: id.clone(),
                variants: alloc::vec![Pattern::Hex(parse_hex(&h)?)],
            },
            other => {
                return Err(format!(
                    "${id}: se esperaba un texto o {{ hex }}, vino {other:?}"
                ));
            }
        };
        if out.iter().any(|d| d.id == def.id) {
            return Err(format!("${id} está repetida"));
        }
        out.push(def);
    }
    Ok(out)
}

fn string_index(defs: &[StringDef], id: &str) -> Result<usize, String> {
    defs.iter()
        .position(|d| d.id == id)
        .ok_or_else(|| format!("${id} no está definida"))
}

fn parse_cmp(p: &mut Parser) -> Result<Cmp, String> {
    Ok(match p.next() {
        Some(Tok::Sym("==")) => Cmp::Eq,
        Some(Tok::Sym("!=")) => Cmp::Ne,
        Some(Tok::Sym("<")) => Cmp::Lt,
        Some(Tok::Sym("<=")) => Cmp::Le,
        Some(Tok::Sym(">")) => Cmp::Gt,
        Some(Tok::Sym(">=")) => Cmp::Ge,
        other => return Err(format!("se esperaba una comparación y vino {other:?}")),
    })
}

fn parse_num(p: &mut Parser) -> Result<u64, String> {
    match p.next() {
        Some(Tok::Num(n)) => Ok(n),
        other => Err(format!("se esperaba un número y vino {other:?}")),
    }
}

fn parse_or(p: &mut Parser, defs: &[StringDef]) -> Result<Expr, String> {
    let mut left = parse_and(p, defs)?;
    while p.is_ident("or") {
        p.next();
        let right = parse_and(p, defs)?;
        left = Expr::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_and(p: &mut Parser, defs: &[StringDef]) -> Result<Expr, String> {
    let mut left = parse_not(p, defs)?;
    while p.is_ident("and") {
        p.next();
        let right = parse_not(p, defs)?;
        left = Expr::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_not(p: &mut Parser, defs: &[StringDef]) -> Result<Expr, String> {
    if p.is_ident("not") {
        p.next();
        return Ok(Expr::Not(Box::new(parse_not(p, defs)?)));
    }
    parse_primary(p, defs)
}

/// `them`, `($a, $b*)`.
fn parse_set(p: &mut Parser, defs: &[StringDef]) -> Result<Vec<usize>, String> {
    if p.is_ident("them") {
        p.next();
        return Ok((0..defs.len()).collect());
    }
    p.expect_sym("(")?;
    let mut set = Vec::new();
    loop {
        match p.next() {
            Some(Tok::Var(id)) => {
                if let Some(prefix) = id.strip_suffix('*') {
                    let before = set.len();
                    set.extend(
                        defs.iter()
                            .enumerate()
                            .filter(|(_, d)| d.id.starts_with(prefix))
                            .map(|(i, _)| i),
                    );
                    if set.len() == before {
                        return Err(format!("${id} no coincide con ninguna cadena"));
                    }
                } else {
                    set.push(string_index(defs, &id)?);
                }
            }
            other => return Err(format!("se esperaba una cadena y vino {other:?}")),
        }
        match p.next() {
            Some(Tok::Sym(",")) => {}
            Some(Tok::Sym(")")) => break,
            other => return Err(format!("se esperaba «,» o «)» y vino {other:?}")),
        }
    }
    Ok(set)
}

fn parse_primary(p: &mut Parser, defs: &[StringDef]) -> Result<Expr, String> {
    match p.next() {
        Some(Tok::Sym("(")) => {
            let e = parse_or(p, defs)?;
            p.expect_sym(")")?;
            Ok(e)
        }
        Some(Tok::Ident(w)) if w == "true" => Ok(Expr::True),
        Some(Tok::Ident(w)) if w == "false" => Ok(Expr::False),
        Some(Tok::Ident(w)) if w == "any" || w == "all" => {
            let q = if w == "any" { Quant::Any } else { Quant::All };
            if !p.is_ident("of") {
                return Err(format!("«{w}» sin «of»"));
            }
            p.next();
            Ok(Expr::Of(q, parse_set(p, defs)?))
        }
        Some(Tok::Num(n)) if p.is_ident("of") => {
            p.next();
            Ok(Expr::Of(Quant::N(n as usize), parse_set(p, defs)?))
        }
        Some(Tok::Ident(w)) if w == "filesize" => {
            let c = parse_cmp(p)?;
            Ok(Expr::FileSize(c, parse_num(p)?))
        }
        Some(Tok::Ident(w)) if w.starts_with("uint") => {
            let (bytes, big) = match w.as_str() {
                "uint8" => (1, false),
                "uint16" => (2, false),
                "uint32" => (4, false),
                "uint16be" => (2, true),
                "uint32be" => (4, true),
                _ => return Err(format!("función desconocida {w}")),
            };
            p.expect_sym("(")?;
            let offset = parse_num(p)?;
            p.expect_sym(")")?;
            let cmp = parse_cmp(p)?;
            let value = parse_num(p)?;
            Ok(Expr::Int {
                bytes,
                big,
                offset,
                cmp,
                value,
            })
        }
        Some(Tok::Var(id)) => {
            let i = string_index(defs, &id)?;
            if p.is_ident("at") {
                p.next();
                return Ok(Expr::At(i, parse_num(p)?));
            }
            if p.is_ident("in") {
                p.next();
                p.expect_sym("(")?;
                let lo = parse_num(p)?;
                p.expect_sym("..")?;
                let hi = parse_num(p)?;
                p.expect_sym(")")?;
                return Ok(Expr::In(i, lo, hi));
            }
            Ok(Expr::Str(i))
        }
        Some(Tok::Count(id)) => {
            let i = string_index(defs, &id)?;
            let c = parse_cmp(p)?;
            Ok(Expr::Count(i, c, parse_num(p)?))
        }
        other => Err(format!("condición inválida cerca de {other:?}")),
    }
}

fn parse_rule(p: &mut Parser) -> Result<Rule, Error> {
    let fail = |name: &str, msg: String| Error {
        rule: name.to_string(),
        msg,
    };
    // `private`/`global` antes de `rule`: se aceptan.
    while p.is_ident("private") || p.is_ident("global") {
        p.next();
    }
    let kw = p.ident().map_err(|m| fail("", m))?;
    if kw != "rule" {
        return Err(fail("", format!("se esperaba «rule» y vino «{kw}»")));
    }
    let name = p.ident().map_err(|m| fail("", m))?;
    let mut tags = Vec::new();
    if matches!(p.peek(), Some(Tok::Sym(":"))) {
        p.next();
        while let Some(Tok::Ident(t)) = p.peek().cloned() {
            p.next();
            tags.push(t);
        }
    }
    p.expect_sym("{").map_err(|m| fail(&name, m))?;
    let mut meta = Vec::new();
    let mut strings = Vec::new();
    let mut cond = None;
    loop {
        match p.next() {
            Some(Tok::Ident(section)) => {
                p.expect_sym(":").map_err(|m| fail(&name, m))?;
                match section.as_str() {
                    "meta" => {
                        while let Some(Tok::Ident(k)) = p.peek().cloned() {
                            if matches!(p.toks.get(p.pos + 1), Some(Tok::Sym(":"))) {
                                break; // la próxima sección
                            }
                            p.next();
                            p.expect_sym("=").map_err(|m| fail(&name, m))?;
                            let v = match p.next() {
                                Some(Tok::Str(s)) => s,
                                Some(Tok::Num(n)) => n.to_string(),
                                Some(Tok::Ident(b)) => b,
                                other => {
                                    return Err(fail(&name, format!("meta inválida: {other:?}")));
                                }
                            };
                            meta.push((k, v));
                        }
                    }
                    "strings" => strings = parse_strings(p).map_err(|m| fail(&name, m))?,
                    "condition" => cond = Some(parse_or(p, &strings).map_err(|m| fail(&name, m))?),
                    other => return Err(fail(&name, format!("sección desconocida «{other}»"))),
                }
            }
            Some(Tok::Sym("}")) => break,
            other => {
                return Err(fail(
                    &name,
                    format!("se esperaba una sección y vino {other:?}"),
                ));
            }
        }
    }
    let cond = cond.ok_or_else(|| fail(&name, "falta la condición".into()))?;
    Ok(Rule {
        name,
        tags,
        meta,
        strings,
        cond,
    })
}

/// Compila un archivo de reglas.
pub fn compile(src: &str) -> Result<Vec<Rule>, Error> {
    let toks = lex(src).map_err(|msg| Error {
        rule: String::new(),
        msg,
    })?;
    let mut p = Parser { toks, pos: 0 };
    let mut rules = Vec::new();
    // `import "pe"` y similares: no hay módulos, se saltean.
    while p.peek().is_some() {
        if p.is_ident("import") || p.is_ident("include") {
            p.next();
            p.next();
            continue;
        }
        let r = parse_rule(&mut p)?;
        if rules.iter().any(|x: &Rule| x.name == r.name) {
            return Err(Error {
                rule: r.name,
                msg: "nombre repetido".into(),
            });
        }
        rules.push(r);
    }
    Ok(rules)
}

// --- Aho-Corasick ------------------------------------------------------------------------------

/// El autómata: un árbol de todos los patrones con enlaces de falla.
struct Automaton {
    /// Las transiciones de cada nodo, ordenadas por byte (búsqueda binaria).
    next: Vec<Vec<(u8, u32)>>,
    fail: Vec<u32>,
    /// Qué patrones terminan en cada nodo (incluidos los que llegan por falla).
    out: Vec<Vec<u32>>,
    /// Largo de cada patrón.
    lens: Vec<usize>,
}

impl Automaton {
    fn new(patterns: &[Vec<u8>]) -> Automaton {
        let mut a = Automaton {
            next: alloc::vec![Vec::new()],
            fail: alloc::vec![0],
            out: alloc::vec![Vec::new()],
            lens: patterns.iter().map(Vec::len).collect(),
        };
        for (pi, pat) in patterns.iter().enumerate() {
            let mut node = 0usize;
            for &b in pat {
                node = match a.child(node, b) {
                    Some(n) => n,
                    None => {
                        let n = a.next.len();
                        a.next.push(Vec::new());
                        a.fail.push(0);
                        a.out.push(Vec::new());
                        let edges = &mut a.next[node];
                        let at = edges.partition_point(|(x, _)| *x < b);
                        edges.insert(at, (b, n as u32));
                        n
                    }
                };
            }
            a.out[node].push(pi as u32);
        }
        // Enlaces de falla por niveles (BFS).
        let mut queue = alloc::collections::VecDeque::new();
        for &(_, n) in &a.next[0].clone() {
            queue.push_back(n as usize);
        }
        while let Some(node) = queue.pop_front() {
            for (b, child) in a.next[node].clone() {
                let child = child as usize;
                queue.push_back(child);
                let mut f = a.fail[node] as usize;
                let target = loop {
                    if let Some(n) = a.child(f, b) {
                        break n;
                    }
                    if f == 0 {
                        break 0;
                    }
                    f = a.fail[f] as usize;
                };
                a.fail[child] = target as u32;
                let inherited = a.out[target].clone();
                a.out[child].extend(inherited);
            }
        }
        a
    }

    fn child(&self, node: usize, b: u8) -> Option<usize> {
        let edges = &self.next[node];
        edges
            .binary_search_by_key(&b, |(x, _)| *x)
            .ok()
            .map(|i| edges[i].1 as usize)
    }

    /// Llama a `hit(patrón, posición de inicio)` por cada aparición.
    fn scan(&self, data: &[u8], lower: bool, mut hit: impl FnMut(usize, usize)) {
        let mut node = 0usize;
        for (i, &raw) in data.iter().enumerate() {
            let b = if lower { raw.to_ascii_lowercase() } else { raw };
            loop {
                if let Some(n) = self.child(node, b) {
                    node = n;
                    break;
                }
                if node == 0 {
                    break;
                }
                node = self.fail[node] as usize;
            }
            for &p in &self.out[node] {
                let len = self.lens[p as usize];
                hit(p as usize, i + 1 - len);
            }
        }
    }
}

/// El pedazo fijo más largo de un patrón hexadecimal (para buscarlo con Aho-Corasick) y en qué
/// posición del patrón está, si antes no hay saltos (si los hay, el ancla es el principio).
fn hex_anchor(h: &[Hex]) -> (Vec<u8>, usize) {
    let mut best: (Vec<u8>, usize) = (Vec::new(), 0);
    let mut cur: Vec<u8> = Vec::new();
    let mut start = 0;
    for (i, t) in h.iter().enumerate() {
        match t {
            Hex::Byte(v, 0xFF) => {
                if cur.is_empty() {
                    start = i;
                }
                cur.push(*v);
                if cur.len() > best.0.len() {
                    best = (cur.clone(), start);
                }
            }
            Hex::Jump(..) => break,
            _ => cur.clear(),
        }
    }
    best
}

/// ¿Coincide el patrón hexadecimal en `data[at..]`? (Con saltos, prueba cada largo.)
fn hex_matches(h: &[Hex], data: &[u8], at: usize) -> bool {
    match h.split_first() {
        None => true,
        Some((Hex::Byte(v, m), rest)) => data
            .get(at)
            .is_some_and(|b| b & m == *v & m && hex_matches(rest, data, at + 1)),
        Some((Hex::Jump(lo, hi), rest)) => {
            (*lo..=*hi).any(|n| at + n <= data.len() && hex_matches(rest, data, at + n))
        }
    }
}

/// Las reglas compiladas, listas para escanear.
pub struct Scanner {
    pub rules: Vec<Rule>,
    /// Patrones exactos: (regla, cadena) de cada uno, y los autómatas (con y sin mayúsculas).
    exact: Vec<(usize, usize)>,
    exact_auto: Automaton,
    folded: Vec<(usize, usize)>,
    folded_auto: Automaton,
    /// Hexadecimales: (regla, cadena, patrón, posición del ancla en el patrón).
    hex: Vec<(usize, usize, Vec<Hex>, usize)>,
    hex_auto: Automaton,
}

impl Scanner {
    pub fn new(rules: Vec<Rule>) -> Scanner {
        let (mut exact, mut exact_p) = (Vec::new(), Vec::new());
        let (mut folded, mut folded_p) = (Vec::new(), Vec::new());
        let (mut hex, mut hex_p) = (Vec::new(), Vec::new());
        for (ri, r) in rules.iter().enumerate() {
            for (si, s) in r.strings.iter().enumerate() {
                for v in &s.variants {
                    match v {
                        Pattern::Bytes {
                            bytes,
                            nocase: false,
                        } => {
                            exact.push((ri, si));
                            exact_p.push(bytes.clone());
                        }
                        Pattern::Bytes {
                            bytes,
                            nocase: true,
                        } => {
                            folded.push((ri, si));
                            folded_p.push(bytes.clone());
                        }
                        Pattern::Hex(h) => {
                            let (anchor, pos) = hex_anchor(h);
                            hex.push((ri, si, h.clone(), pos));
                            hex_p.push(anchor);
                        }
                    }
                }
            }
        }
        Scanner {
            rules,
            exact,
            exact_auto: Automaton::new(&exact_p),
            folded,
            folded_auto: Automaton::new(&folded_p),
            hex,
            hex_auto: Automaton::new(&hex_p),
        }
    }

    /// Las reglas que coinciden con `data`.
    pub fn scan(&self, data: &[u8]) -> Vec<Match> {
        // Dónde aparece cada cadena de cada regla.
        let mut hits: BTreeMap<(usize, usize), Vec<u64>> = BTreeMap::new();
        let mut add = |key: (usize, usize), pos: usize| {
            let v = hits.entry(key).or_default();
            if v.len() < MAX_HITS {
                v.push(pos as u64);
            }
        };
        self.exact_auto
            .scan(data, false, |p, at| add(self.exact[p], at));
        if !self.folded.is_empty() {
            self.folded_auto
                .scan(data, true, |p, at| add(self.folded[p], at));
        }
        if !self.hex.is_empty() {
            self.hex_auto.scan(data, false, |p, at| {
                let (ri, si, h, anchor_pos) = &self.hex[p];
                // El ancla está a `anchor_pos` bytes del principio del patrón (sin saltos antes).
                if let Some(start) = at.checked_sub(*anchor_pos)
                    && hex_matches(h, data, start)
                {
                    add((*ri, *si), start);
                }
            });
        }
        for v in hits.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        let mut out = Vec::new();
        for (ri, r) in self.rules.iter().enumerate() {
            let get = |si: usize| hits.get(&(ri, si)).map_or(&[][..], Vec::as_slice);
            if eval(&r.cond, data, &get, r.strings.len()) {
                out.push(Match {
                    rule: r.name.clone(),
                    description: r.meta("description").unwrap_or(&r.name).to_string(),
                    severity: r.severity().to_string(),
                });
            }
        }
        out
    }
}

fn eval<'a>(e: &Expr, data: &[u8], hits: &dyn Fn(usize) -> &'a [u64], n: usize) -> bool {
    match e {
        Expr::True => true,
        Expr::False => false,
        Expr::Str(i) => !hits(*i).is_empty(),
        Expr::Count(i, c, v) => c.eval(hits(*i).len() as u64, *v),
        Expr::At(i, off) => hits(*i).binary_search(off).is_ok(),
        Expr::In(i, lo, hi) => hits(*i).iter().any(|p| p >= lo && p <= hi),
        Expr::Of(q, set) => {
            let found = set.iter().filter(|i| !hits(**i).is_empty()).count();
            match q {
                Quant::Any => found >= 1,
                Quant::All => found == set.len(),
                Quant::N(k) => found >= (*k).min(n.max(1)),
            }
        }
        Expr::FileSize(c, v) => c.eval(data.len() as u64, *v),
        Expr::Int {
            bytes,
            big,
            offset,
            cmp,
            value,
        } => {
            let Ok(at) = usize::try_from(*offset) else {
                return false;
            };
            let Some(raw) = data.get(at..at + bytes) else {
                return false;
            };
            let mut v = 0u64;
            for (k, b) in raw.iter().enumerate() {
                let shift = if *big { 8 * (bytes - 1 - k) } else { 8 * k };
                v |= u64::from(*b) << shift;
            }
            cmp.eval(v, *value)
        }
        Expr::Not(x) => !eval(x, data, hits, n),
        Expr::And(a, b) => eval(a, data, hits, n) && eval(b, data, hits, n),
        Expr::Or(a, b) => eval(a, data, hits, n) || eval(b, data, hits, n),
    }
}

#[cfg(test)]
mod tests;
