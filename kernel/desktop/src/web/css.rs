//! CSS: hojas de estilo, selectores, la cascada y los valores (colores, largos, `calc()`).
//!
//! - **Selectores**: etiqueta, `#id`, `.clase`, `*`, atributos (`[type=text]`, `[href^=http]`,
//!   `[class~=x]`…), combinadores (descendiente, `>`, `+`, `~`), listas (`h1, h2`) y
//!   pseudoclases estructurales (`:first-child`, `:nth-child(2n+1)`, `:last-of-type`, `:empty`,
//!   `:root`), de estado de formularios (`:checked`, `:disabled`) y lógicas (`:not()`, `:is()`,
//!   `:where()`). Las que dependen del mouse o del foco (`:hover`, `:focus`) nunca coinciden: la
//!   página se ve como recién cargada. Los pseudoelementos (`::before`) se ignoran.
//! - **Cascada**: especificidad, orden, `!important` y estilos en línea (`style="…"`).
//! - **@media** se evalúa con el ancho real de la ventana (`min-width`, `max-width`, rangos
//!   `width >= 600px`, `print`, `prefers-color-scheme`), así las páginas "responsive" se arman
//!   para el tamaño que tienen. `@supports`, `@layer` y `@container` se aplican.
//!
//! Las reglas se indexan por lo que exige su último compuesto (id, clase o etiqueta), como hacen
//! los navegadores: así cada elemento solo prueba las reglas que le pueden tocar.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::Color;

use super::dom::{Dom, NodeKind};

// --- selectores --------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Comb {
    Descendant,
    Child,
    /// `a + b`: el hermano inmediatamente anterior.
    Next,
    /// `a ~ b`: algún hermano anterior.
    Subsequent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AttrOp {
    Exists,
    Eq,
    /// `~=`: una de las palabras.
    Includes,
    /// `|=`: igual o seguido de `-`.
    Dash,
    Prefix,
    Suffix,
    Substr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AttrSel {
    name: String,
    op: AttrOp,
    value: String,
    /// `[x=y i]`: sin distinguir mayúsculas.
    ci: bool,
}

#[derive(Clone, Debug)]
enum Pseudo {
    FirstChild,
    LastChild,
    OnlyChild,
    /// `:nth-child(an+b)` (y desde el final).
    Nth {
        a: i32,
        b: i32,
        from_end: bool,
        of_type: bool,
    },
    FirstOfType,
    LastOfType,
    OnlyOfType,
    Empty,
    Root,
    Checked,
    Disabled,
    Enabled,
    /// `a[href]`, `:any-link`.
    Link,
    PlaceholderShown,
    Not(Vec<Selector>),
    /// `:is()` / `:where()` / `:matches()`.
    Is(Vec<Selector>),
    /// `:has()`: se aproxima mirando los descendientes.
    Has(Vec<Selector>),
    /// `:hover`, `:focus`, `:target`…: la página recién cargada no está en ese estado.
    Never,
}

#[derive(Clone, Debug, Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<AttrSel>,
    pseudos: Vec<Pseudo>,
}

#[derive(Clone, Debug)]
struct Selector {
    /// De izquierda a derecha; el combinador de cada parte es el que la une con la anterior.
    parts: Vec<(Comb, Compound)>,
    specificity: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    pub prop: String,
    pub value: String,
    pub important: bool,
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Selector,
    order: u32,
    decls: Rc<Vec<Decl>>,
}

/// Lo que se precalcula de cada elemento para comparar selectores rápido.
#[derive(Clone, Debug, Default)]
pub struct ElemData {
    pub tag: String,
    pub id: String,
    pub classes: Vec<String>,
    /// Posición entre los hermanos que son elementos (desde 1) y cuántos hay.
    pub index: u32,
    pub count: u32,
    /// Lo mismo, contando solo los de la misma etiqueta.
    pub type_index: u32,
    pub type_count: u32,
    /// Elemento hermano anterior.
    pub prev: Option<usize>,
    /// Elemento padre.
    pub parent: Option<usize>,
}

/// El árbol con los datos de cada elemento, listo para comparar selectores.
pub struct Matcher<'a> {
    pub dom: &'a Dom,
    pub elems: Vec<ElemData>,
}

impl<'a> Matcher<'a> {
    pub fn new(dom: &'a Dom) -> Matcher<'a> {
        let mut elems: Vec<ElemData> = alloc::vec![ElemData::default(); dom.nodes.len()];
        for (i, n) in dom.nodes.iter().enumerate() {
            if let NodeKind::Element { name, .. } = &n.kind {
                let e = &mut elems[i];
                e.tag = name.clone();
                e.id = n.attr("id").unwrap_or("").to_string();
                e.classes = n
                    .attr("class")
                    .unwrap_or("")
                    .split_ascii_whitespace()
                    .map(String::from)
                    .collect();
            }
        }
        for (i, n) in dom.nodes.iter().enumerate() {
            let kids: Vec<usize> = n
                .children
                .iter()
                .copied()
                .filter(|&c| matches!(dom.nodes[c].kind, NodeKind::Element { .. }))
                .collect();
            let count = kids.len() as u32;
            let mut prev = None;
            let mut per_tag: BTreeMap<String, u32> = BTreeMap::new();
            for (k, &c) in kids.iter().enumerate() {
                let t = elems[c].tag.clone();
                let ti = per_tag.entry(t).or_insert(0);
                *ti += 1;
                let e = &mut elems[c];
                e.index = k as u32 + 1;
                e.count = count;
                e.type_index = *ti;
                e.prev = prev;
                e.parent = if matches!(n.kind, NodeKind::Element { .. }) {
                    Some(i)
                } else {
                    None
                };
                prev = Some(c);
            }
            for &c in &kids {
                let tc = per_tag[&elems[c].tag];
                elems[c].type_count = tc;
            }
        }
        Matcher { dom, elems }
    }

    fn attr(&self, n: usize, name: &str) -> Option<&str> {
        self.dom.nodes[n].attr(name)
    }

    fn compound(&self, c: &Compound, n: usize) -> bool {
        let e = &self.elems[n];
        if e.tag.is_empty() {
            return false;
        }
        if c.tag.as_ref().is_some_and(|t| *t != e.tag)
            || c.id.as_ref().is_some_and(|i| *i != e.id)
            || !c.classes.iter().all(|cl| e.classes.iter().any(|x| x == cl))
        {
            return false;
        }
        for a in &c.attrs {
            let Some(v) = self.attr(n, &a.name) else {
                return false;
            };
            let (v, want) = if a.ci {
                (v.to_ascii_lowercase(), a.value.to_ascii_lowercase())
            } else {
                (v.to_string(), a.value.clone())
            };
            let ok = match a.op {
                AttrOp::Exists => true,
                AttrOp::Eq => v == want,
                AttrOp::Includes => v.split_ascii_whitespace().any(|w| w == want),
                AttrOp::Dash => v == want || v.starts_with(&(want.clone() + "-")),
                AttrOp::Prefix => !want.is_empty() && v.starts_with(&want),
                AttrOp::Suffix => !want.is_empty() && v.ends_with(&want),
                AttrOp::Substr => !want.is_empty() && v.contains(&want),
            };
            if !ok {
                return false;
            }
        }
        c.pseudos.iter().all(|p| self.pseudo(p, n))
    }

    fn pseudo(&self, p: &Pseudo, n: usize) -> bool {
        let e = &self.elems[n];
        let nth = |a: i32, b: i32, pos: i32| -> bool {
            if a == 0 {
                pos == b
            } else {
                (pos - b) % a == 0 && (pos - b) / a >= 0
            }
        };
        match p {
            Pseudo::FirstChild => e.index == 1,
            Pseudo::LastChild => e.index == e.count,
            Pseudo::OnlyChild => e.count == 1,
            Pseudo::FirstOfType => e.type_index == 1,
            Pseudo::LastOfType => e.type_index == e.type_count,
            Pseudo::OnlyOfType => e.type_count == 1,
            Pseudo::Nth {
                a,
                b,
                from_end,
                of_type,
            } => {
                let (idx, cnt) = if *of_type {
                    (e.type_index, e.type_count)
                } else {
                    (e.index, e.count)
                };
                let pos = if *from_end { cnt + 1 - idx } else { idx } as i32;
                nth(*a, *b, pos)
            }
            Pseudo::Empty => self.dom.nodes[n]
                .children
                .iter()
                .all(|&c| matches!(&self.dom.nodes[c].kind, NodeKind::Text(t) if t.is_empty())),
            Pseudo::Root => e.tag == "html",
            Pseudo::Checked => {
                self.attr(n, "checked").is_some() || self.attr(n, "selected").is_some()
            }
            Pseudo::Disabled => self.attr(n, "disabled").is_some(),
            Pseudo::Enabled => {
                matches!(e.tag.as_str(), "input" | "button" | "select" | "textarea")
                    && self.attr(n, "disabled").is_none()
            }
            Pseudo::Link => {
                matches!(e.tag.as_str(), "a" | "area") && self.attr(n, "href").is_some()
            }
            Pseudo::PlaceholderShown => {
                self.attr(n, "placeholder").is_some()
                    && self.attr(n, "value").is_none_or(str::is_empty)
            }
            Pseudo::Not(list) => !list.iter().any(|s| self.matches(s, n)),
            Pseudo::Is(list) => list.iter().any(|s| self.matches(s, n)),
            Pseudo::Has(list) => self.has_descendant(n, list, 0),
            Pseudo::Never => false,
        }
    }

    fn has_descendant(&self, n: usize, list: &[Selector], depth: u32) -> bool {
        if depth > 12 {
            return false;
        }
        self.dom.nodes[n].children.iter().any(|&c| {
            !self.elems[c].tag.is_empty()
                && (list.iter().any(|s| self.matches(s, c))
                    || self.has_descendant(c, list, depth + 1))
        })
    }

    fn matches(&self, sel: &Selector, n: usize) -> bool {
        self.match_from(sel, sel.parts.len() - 1, n)
    }

    /// ¿Coincide la parte `k` (y las anteriores) con el elemento `n`?
    fn match_from(&self, sel: &Selector, k: usize, n: usize) -> bool {
        if !self.compound(&sel.parts[k].1, n) {
            return false;
        }
        if k == 0 {
            return true;
        }
        match sel.parts[k].0 {
            Comb::Child => self.elems[n]
                .parent
                .is_some_and(|p| self.match_from(sel, k - 1, p)),
            Comb::Descendant => {
                let mut cur = self.elems[n].parent;
                while let Some(p) = cur {
                    if self.match_from(sel, k - 1, p) {
                        return true;
                    }
                    cur = self.elems[p].parent;
                }
                false
            }
            Comb::Next => self.elems[n]
                .prev
                .is_some_and(|p| self.match_from(sel, k - 1, p)),
            Comb::Subsequent => {
                let mut cur = self.elems[n].prev;
                while let Some(p) = cur {
                    if self.match_from(sel, k - 1, p) {
                        return true;
                    }
                    cur = self.elems[p].prev;
                }
                false
            }
        }
    }
}

/// Lee selectores letra por letra (los valores de atributos y los paréntesis pueden tener
/// espacios y comas).
struct SelParser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b >= 0x80 || b == b'\\'
}

impl<'a> SelParser<'a> {
    fn new(src: &'a str) -> Self {
        SelParser {
            s: src.as_bytes(),
            src,
            i: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip_ws(&mut self) -> bool {
        let start = self.i;
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.i += 1;
        }
        self.i > start
    }

    fn ident(&mut self) -> String {
        let start = self.i;
        while let Some(b) = self.peek() {
            if b == b'\\' {
                self.i += 2; // `.md\:flex`: el carácter escapado es parte del nombre
                continue;
            }
            if !is_ident(b) {
                break;
            }
            self.i += 1;
        }
        let raw = &self.src[start..self.i.min(self.s.len())];
        if raw.contains('\\') {
            let mut out = String::new();
            let mut chars = raw.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                } else {
                    out.push(c);
                }
            }
            out
        } else {
            raw.to_string()
        }
    }

    /// Lo que hay entre paréntesis (con anidados).
    fn parens(&mut self) -> Option<&'a str> {
        if self.peek() != Some(b'(') {
            return None;
        }
        let start = self.i + 1;
        let mut depth = 0;
        while let Some(b) = self.peek() {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        self.i += 1;
                        return Some(&self.src[start..self.i - 1]);
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        None
    }

    fn compound(&mut self) -> Option<Compound> {
        let mut c = Compound::default();
        let mut any = false;
        if self.peek() == Some(b'*') {
            self.i += 1;
            any = true;
        } else if self.peek().is_some_and(is_ident) {
            c.tag = Some(self.ident().to_ascii_lowercase());
            any = true;
        }
        loop {
            match self.peek() {
                Some(b'#') => {
                    self.i += 1;
                    c.id = Some(self.ident());
                }
                Some(b'.') => {
                    self.i += 1;
                    let name = self.ident();
                    if name.is_empty() {
                        return None;
                    }
                    c.classes.push(name);
                }
                Some(b'[') => {
                    self.i += 1;
                    c.attrs.push(self.attr()?);
                }
                Some(b':') => {
                    self.i += 1;
                    if self.peek() == Some(b':') {
                        return None; // pseudoelemento: no se muestra
                    }
                    let name = self.ident().to_ascii_lowercase();
                    let args = self.parens();
                    c.pseudos.push(pseudo(&name, args)?);
                }
                _ => break,
            }
            any = true;
        }
        any.then_some(c)
    }

    fn attr(&mut self) -> Option<AttrSel> {
        self.skip_ws();
        let name = self.ident().to_ascii_lowercase();
        self.skip_ws();
        let op = match self.peek()? {
            b']' => {
                self.i += 1;
                return Some(AttrSel {
                    name,
                    op: AttrOp::Exists,
                    value: String::new(),
                    ci: false,
                });
            }
            b'=' => AttrOp::Eq,
            b'~' => AttrOp::Includes,
            b'|' => AttrOp::Dash,
            b'^' => AttrOp::Prefix,
            b'$' => AttrOp::Suffix,
            b'*' => AttrOp::Substr,
            _ => return None,
        };
        self.i += if op == AttrOp::Eq { 1 } else { 2 };
        self.skip_ws();
        let value = match self.peek()? {
            q @ (b'"' | b'\'') => {
                self.i += 1;
                let start = self.i;
                while self.peek().is_some_and(|b| b != q) {
                    self.i += 1;
                }
                let v = self.src.get(start..self.i)?.to_string();
                self.i += 1;
                v
            }
            _ => self.ident(),
        };
        self.skip_ws();
        let mut ci = false;
        if matches!(self.peek(), Some(b'i' | b'I')) {
            ci = true;
            self.i += 1;
            self.skip_ws();
        } else if matches!(self.peek(), Some(b's' | b'S')) {
            self.i += 1;
            self.skip_ws();
        }
        if self.peek() != Some(b']') {
            return None;
        }
        self.i += 1;
        Some(AttrSel {
            name,
            op,
            value,
            ci,
        })
    }

    fn selector(&mut self) -> Option<Selector> {
        let mut parts = Vec::new();
        let mut comb = Comb::Descendant;
        self.skip_ws();
        loop {
            let c = self.compound()?;
            parts.push((comb, c));
            let ws = self.skip_ws();
            comb = match self.peek() {
                None | Some(b',') => break,
                Some(b'>') => Comb::Child,
                Some(b'+') => Comb::Next,
                Some(b'~') => Comb::Subsequent,
                Some(_) if ws => {
                    comb = Comb::Descendant;
                    continue;
                }
                Some(_) => return None,
            };
            self.i += 1;
            self.skip_ws();
        }
        let specificity = parts.iter().map(|(_, c)| spec_of(c)).sum();
        Some(Selector { parts, specificity })
    }
}

fn spec_of(c: &Compound) -> u32 {
    let mut s = c.id.is_some() as u32 * 10_000
        + (c.classes.len() + c.attrs.len()) as u32 * 100
        + c.tag.is_some() as u32;
    for p in &c.pseudos {
        s += match p {
            Pseudo::Is(l) | Pseudo::Not(l) | Pseudo::Has(l) => {
                l.iter().map(|x| x.specificity).max().unwrap_or(0)
            }
            _ => 100,
        };
    }
    s
}

/// Una lista de selectores (`a, b.c`). `None` si alguno no se entiende.
fn selector_list(s: &str) -> Option<Vec<Selector>> {
    let mut p = SelParser::new(s);
    let mut out = Vec::new();
    loop {
        out.push(p.selector()?);
        p.skip_ws();
        match p.peek() {
            Some(b',') => p.i += 1,
            None => return Some(out),
            _ => return None,
        }
    }
}

/// `2n+1`, `odd`, `even`, `3`, `-n+2`.
fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let s: String = s
        .split(" of ")
        .next()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    match s.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(np) = s.find('n') {
        let a = match &s[..np] {
            "" | "+" => 1,
            "-" => -1,
            x => x.parse().ok()?,
        };
        let b = if np + 1 < s.len() {
            s[np + 1..].trim_start_matches('+').parse().ok()?
        } else {
            0
        };
        Some((a, b))
    } else {
        Some((0, s.parse().ok()?))
    }
}

fn pseudo(name: &str, args: Option<&str>) -> Option<Pseudo> {
    Some(match name {
        "first-child" => Pseudo::FirstChild,
        "last-child" => Pseudo::LastChild,
        "only-child" => Pseudo::OnlyChild,
        "first-of-type" => Pseudo::FirstOfType,
        "last-of-type" => Pseudo::LastOfType,
        "only-of-type" => Pseudo::OnlyOfType,
        "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
            let (a, b) = parse_nth(args?)?;
            Pseudo::Nth {
                a,
                b,
                from_end: name.contains("last"),
                of_type: name.ends_with("of-type"),
            }
        }
        "empty" => Pseudo::Empty,
        "root" => Pseudo::Root,
        "checked" | "default" => Pseudo::Checked,
        "disabled" | "read-only" => Pseudo::Disabled,
        "enabled" | "read-write" => Pseudo::Enabled,
        "link" | "any-link" => Pseudo::Link,
        "placeholder-shown" => Pseudo::PlaceholderShown,
        // Sin visitas guardadas, sin idioma elegido, siempre "válido": no cambian nada.
        "visited" | "focus-visible" | "focus-within" | "hover" | "active" | "focus" | "target"
        | "indeterminate" | "invalid" | "user-invalid" | "fullscreen" | "modal"
        | "popover-open" | "open" => Pseudo::Never,
        "lang" | "dir" | "valid" | "required" | "optional" | "in-range" | "defined" | "scope" => {
            Pseudo::Is(Vec::new()).always()
        }
        "not" => Pseudo::Not(selector_list(args?)?),
        "is" | "where" | "matches" | "-webkit-any" | "-moz-any" => {
            // Con `:is(…)` los selectores que no entendemos se descartan (es "perdonador").
            let list: Vec<Selector> = args?
                .split(',')
                .filter_map(|s| selector_list(s).and_then(|mut v| v.pop()))
                .collect();
            if list.is_empty() {
                return None;
            }
            if name == "where" {
                // `:where` no suma especificidad.
                let list = list
                    .into_iter()
                    .map(|mut s| {
                        s.specificity = 0;
                        s
                    })
                    .collect();
                return Some(Pseudo::Is(list));
            }
            Pseudo::Is(list)
        }
        "has" => Pseudo::Has(selector_list(args?.trim_start_matches(['>', ' ']))?),
        _ => return None,
    })
}

impl Pseudo {
    /// Una pseudoclase que siempre coincide.
    fn always(self) -> Pseudo {
        Pseudo::Not(Vec::new())
    }
}

// --- hojas de estilo ---------------------------------------------------------------------------

/// El entorno para evaluar `@media`.
#[derive(Clone, Copy, Debug)]
pub struct Media {
    pub width: i32,
    pub height: i32,
}

impl Default for Media {
    fn default() -> Self {
        Media {
            width: 1024,
            height: 700,
        }
    }
}

#[derive(Default)]
pub struct Sheet {
    rules: Vec<Rule>,
    by_id: BTreeMap<String, Vec<usize>>,
    by_class: BTreeMap<String, Vec<usize>>,
    by_tag: BTreeMap<String, Vec<usize>>,
    universal: Vec<usize>,
    media: Media,
    /// `@import` que encontró (para que el navegador los baje).
    pub imports: Vec<String>,
}

/// Saca los comentarios `/* … */`.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Busca la llave que cierra la que abre en `open` (contando anidadas y saltando textos).
fn matching_brace(s: &str, open: usize) -> usize {
    let mut depth = 0;
    let mut quote: Option<u8> = None;
    let b = s.as_bytes();
    let mut i = open;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            if c == b'\\' {
                i += 1;
            } else if c == q {
                quote = None;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = Some(c),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return i;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    s.len()
}

/// ¿Vale una condición `@media` para esta ventana?
pub fn media_ok(cond: &str, m: Media) -> bool {
    let c = cond.to_ascii_lowercase();
    // Una lista con comas vale si vale alguna.
    if c.contains(',') {
        return c.split(',').any(|part| media_ok(part, m));
    }
    let c = c.trim();
    if let Some(rest) = c.strip_prefix("not ") {
        return !media_ok(rest, m);
    }
    let c = c.strip_prefix("only ").unwrap_or(c);
    for part in c.split(" and ") {
        let p = part.trim();
        let p = match p.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
            Some(inner) => inner.trim(),
            None => p,
        };
        if p.is_empty() || p == "screen" || p == "all" {
            continue;
        }
        if p == "print" || p == "speech" || p == "tv" {
            return false;
        }
        if !feature_ok(p, m) {
            return false;
        }
    }
    true
}

fn media_len(v: &str) -> Option<f32> {
    let v = v.trim();
    if v.contains('(') {
        // `calc(640px - 1px)`
        return parse_len(v, Units::default())?
            .resolve(Some(0))
            .map(|x| x as f32);
    }
    let num = |s: &str| s.trim().parse::<f32>().ok();
    if let Some(n) = v.strip_suffix("px") {
        return num(n);
    }
    if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        return num(n).map(|x| x * 16.0);
    }
    num(v)
}

fn feature_ok(p: &str, m: Media) -> bool {
    let (w, h) = (m.width as f32, m.height as f32);
    // Sintaxis de rangos: `width >= 600px`, `400px <= width <= 700px`.
    for (op, f) in [
        (">=", (|a: f32, b: f32| a >= b) as fn(f32, f32) -> bool),
        ("<=", |a, b| a <= b),
        (">", |a, b| a > b),
        ("<", |a, b| a < b),
    ] {
        if p.contains(op) {
            let parts: Vec<&str> = p.split(op).map(str::trim).collect();
            let val = |name: &str| match name {
                "width" => Some(w),
                "height" => Some(h),
                _ => media_len(name),
            };
            return parts
                .windows(2)
                .all(|pair| match (val(pair[0]), val(pair[1])) {
                    (Some(a), Some(b)) => f(a, b),
                    _ => true,
                });
        }
    }
    let Some((name, value)) = p.split_once(':') else {
        // `(hover)`, `(color)`: sí.
        return true;
    };
    let (name, value) = (name.trim(), value.trim());
    match name {
        "min-width" => media_len(value).is_none_or(|v| w >= v),
        "max-width" => media_len(value).is_none_or(|v| w <= v),
        "min-height" => media_len(value).is_none_or(|v| h >= v),
        "max-height" => media_len(value).is_none_or(|v| h <= v),
        "min-device-width" => media_len(value).is_none_or(|v| w >= v),
        "max-device-width" => media_len(value).is_none_or(|v| w <= v),
        "prefers-color-scheme" => value == "light",
        "prefers-reduced-motion" => value == "reduce",
        "prefers-contrast" | "forced-colors" | "inverted-colors" => value == "no-preference",
        "orientation" => (value == "landscape") == (w >= h),
        "hover" | "any-hover" => value == "hover",
        "pointer" | "any-pointer" => value == "fine",
        "min-resolution" | "-webkit-min-device-pixel-ratio" | "min--moz-device-pixel-ratio" => {
            value.starts_with('1') && !value.starts_with("1.")
        }
        _ => true,
    }
}

/// `@supports`: se aceptan las propiedades que entendemos (y, por simplicidad, casi todo lo
/// demás). `not (…)` de algo que sí soportamos es falso.
fn supports_ok(cond: &str) -> bool {
    let c = cond.trim().to_ascii_lowercase();
    if let Some(rest) = c.strip_prefix("not") {
        let inner = rest.trim().trim_start_matches('(').trim_end_matches(')');
        return !supported_decl(inner);
    }
    true
}

fn supported_decl(d: &str) -> bool {
    let Some((p, v)) = d.split_once(':') else {
        return !d.contains("selector(");
    };
    let (p, v) = (p.trim(), v.trim());
    matches!(
        p,
        "display" | "position" | "gap" | "flex-wrap" | "grid-template-columns" | "width" | "color"
    ) && !v.contains("subgrid")
        && !v.contains("contents")
}

pub fn parse_decls(block: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    // Los `;` adentro de paréntesis (url(data:…;base64)) o de comillas no separan.
    let mut depth = 0;
    let mut quote: Option<u8> = None;
    let mut start = 0;
    let bytes = block.as_bytes();
    let mut pieces = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'"' | b'\'' => quote = Some(b),
            b'(' => depth += 1,
            b')' => depth -= 1,
            b';' if depth <= 0 => {
                pieces.push(&block[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&block[start..]);
    for d in pieces {
        let Some((prop, value)) = d.split_once(':') else {
            continue;
        };
        let prop = prop.trim().to_ascii_lowercase();
        // Bloques anidados (CSS "nesting") no son declaraciones.
        if prop.contains(['{', '}', ' ', '&']) {
            continue;
        }
        let mut value = value.trim().to_string();
        let lower = value.to_ascii_lowercase();
        let important = lower.ends_with("important") && lower.contains('!');
        if important && let Some(bang) = value.rfind('!') {
            value.truncate(bang);
            value = value.trim().to_string();
        }
        // Los "hacks" de IE (`*zoom: 1`, `_height: 1px`) no son propiedades.
        if !prop.is_empty()
            && !value.is_empty()
            && !prop.starts_with(['*', '_'])
            && !value.contains("expression(")
        {
            out.push(Decl {
                prop,
                value,
                important,
            });
        }
    }
    out
}

impl Sheet {
    pub fn new(media: Media) -> Sheet {
        Sheet {
            media,
            ..Default::default()
        }
    }

    /// Agrega una hoja de estilo (se pueden agregar varias; el orden importa).
    pub fn add(&mut self, css: &str) {
        let css = strip_comments(css);
        self.add_rules(&css, 0);
    }

    fn add_rules(&mut self, css: &str, depth: u32) {
        let mut i = 0;
        while i < css.len() {
            let Some(open) = css[i..].find('{').map(|o| i + o) else {
                break;
            };
            let head_all = &css[i..open];
            let close = matching_brace(css, open);
            let body = &css[open + 1..close.min(css.len())];
            i = close + 1;
            // Reglas `@` sin bloque antes de esta (`@import …;`, `@charset …;`).
            let mut pieces: Vec<&str> = head_all.split(';').collect();
            let head = pieces.pop().unwrap_or("").trim();
            for p in pieces {
                self.at_statement(p.trim());
            }
            if let Some(at) = head.strip_prefix('@') {
                let lower = at.to_ascii_lowercase();
                let nested = if let Some(cond) = lower.strip_prefix("media") {
                    media_ok(cond, self.media)
                } else if let Some(cond) = lower.strip_prefix("supports") {
                    supports_ok(cond)
                } else {
                    lower.starts_with("layer")
                        || lower.starts_with("container")
                        || lower.starts_with("scope")
                        || lower.starts_with("document")
                        || lower.starts_with("-moz-document")
                };
                if nested && depth < 6 {
                    self.add_rules(body, depth + 1);
                }
                continue;
            }
            let decls = parse_decls(body);
            if decls.is_empty() {
                continue;
            }
            let decls = Rc::new(decls);
            // Cada selector de la lista se prueba por separado: uno que no entendemos no
            // descarta a los demás (en los navegadores sí, pero así se aprovecha más).
            for sel in split_top(head, b',') {
                let Some(mut list) = selector_list(sel) else {
                    continue;
                };
                let Some(selector) = list.pop() else { continue };
                self.push_rule(selector, decls.clone());
            }
        }
        // Lo que queda después de la última regla (`@import x;` al final).
        if let Some(rest) = css.get(i..) {
            for p in rest.split(';') {
                self.at_statement(p.trim());
            }
        }
    }

    fn at_statement(&mut self, s: &str) {
        let lower = s.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("@import") else {
            return;
        };
        // `@import url("x.css") screen;` o `@import "x.css";`
        let raw = &s[s.len() - rest.len()..];
        let target = raw
            .trim()
            .trim_start_matches("url(")
            .split([')', ' '])
            .next()
            .unwrap_or("")
            .trim_matches(['"', '\'']);
        let media = rest.rsplit(')').next().unwrap_or("");
        if !target.is_empty()
            && self.imports.len() < 8
            && (media.trim().is_empty() || media_ok(media, self.media))
        {
            self.imports.push(target.to_string());
        }
    }

    fn push_rule(&mut self, selector: Selector, decls: Rc<Vec<Decl>>) {
        let idx = self.rules.len();
        let key = &selector.parts[selector.parts.len() - 1].1;
        if let Some(id) = &key.id {
            self.by_id.entry(id.clone()).or_default().push(idx);
        } else if let Some(cl) = key.classes.first() {
            self.by_class.entry(cl.clone()).or_default().push(idx);
        } else if let Some(t) = &key.tag {
            self.by_tag.entry(t.clone()).or_default().push(idx);
        } else {
            self.universal.push(idx);
        }
        self.rules.push(Rule {
            selector,
            order: idx as u32,
            decls,
        });
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Las declaraciones que le tocan al elemento `n`, ya ordenadas por la cascada: las que
    /// ganan quedan al final.
    pub fn matching<'s>(&'s self, m: &Matcher<'_>, n: usize) -> Vec<&'s Decl> {
        let e = &m.elems[n];
        let mut cands: Vec<usize> = Vec::new();
        if !e.id.is_empty()
            && let Some(v) = self.by_id.get(&e.id)
        {
            cands.extend(v);
        }
        for c in &e.classes {
            if let Some(v) = self.by_class.get(c) {
                cands.extend(v);
            }
        }
        if let Some(v) = self.by_tag.get(&e.tag) {
            cands.extend(v);
        }
        cands.extend(&self.universal);
        cands.sort_unstable();
        cands.dedup();
        let mut hits: Vec<&Rule> = cands
            .into_iter()
            .map(|i| &self.rules[i])
            .filter(|r| m.matches(&r.selector, n))
            .collect();
        hits.sort_by_key(|r| (r.selector.specificity, r.order));
        let mut out: Vec<&Decl> = Vec::new();
        for imp in [false, true] {
            for r in &hits {
                out.extend(r.decls.iter().filter(|d| d.important == imp));
            }
        }
        out
    }
}

/// Parte `s` en `sep` sin mirar lo que está entre paréntesis, corchetes o comillas.
pub fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
            continue;
        }
        match b {
            b'"' | b'\'' => quote = Some(b),
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            _ if b == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Parte un valor en palabras (espacios fuera de paréntesis): `1px solid rgb(0, 0, 0)`.
pub fn tokens(v: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, c) in v.char_indices() {
        match c {
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&v[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&v[s..]);
    }
    out
}

// --- valores ------------------------------------------------------------------------------------

/// Un color con opacidad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub c: Color,
    /// 0 = transparente, 255 = opaco.
    pub a: u8,
}

impl Rgba {
    pub const fn opaque(c: Color) -> Rgba {
        Rgba { c, a: 255 }
    }
}

const NAMED: [(&str, u32); 74] = [
    ("black", 0x000000),
    ("white", 0xffffff),
    ("red", 0xff0000),
    ("green", 0x008000),
    ("blue", 0x0000ff),
    ("yellow", 0xffff00),
    ("orange", 0xffa500),
    ("purple", 0x800080),
    ("gray", 0x808080),
    ("grey", 0x808080),
    ("silver", 0xc0c0c0),
    ("maroon", 0x800000),
    ("navy", 0x000080),
    ("teal", 0x008080),
    ("olive", 0x808000),
    ("lime", 0x00ff00),
    ("aqua", 0x00ffff),
    ("cyan", 0x00ffff),
    ("fuchsia", 0xff00ff),
    ("magenta", 0xff00ff),
    ("darkblue", 0x00008b),
    ("darkgreen", 0x006400),
    ("darkred", 0x8b0000),
    ("darkgray", 0xa9a9a9),
    ("darkgrey", 0xa9a9a9),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("lightgray", 0xd3d3d3),
    ("lightgrey", 0xd3d3d3),
    ("lightblue", 0xadd8e6),
    ("lightgreen", 0x90ee90),
    ("lightyellow", 0xffffe0),
    ("lightcyan", 0xe0ffff),
    ("lightpink", 0xffb6c1),
    ("whitesmoke", 0xf5f5f5),
    ("gainsboro", 0xdcdcdc),
    ("brown", 0xa52a2a),
    ("pink", 0xffc0cb),
    ("gold", 0xffd700),
    ("beige", 0xf5f5dc),
    ("ivory", 0xfffff0),
    ("crimson", 0xdc143c),
    ("steelblue", 0x4682b4),
    ("royalblue", 0x4169e1),
    ("dodgerblue", 0x1e90ff),
    ("tomato", 0xff6347),
    ("orangered", 0xff4500),
    ("darkorange", 0xff8c00),
    ("coral", 0xff7f50),
    ("salmon", 0xfa8072),
    ("khaki", 0xf0e68c),
    ("violet", 0xee82ee),
    ("indigo", 0x4b0082),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("lightslategray", 0x778899),
    ("darkslategray", 0x2f4f4f),
    ("midnightblue", 0x191970),
    ("cornflowerblue", 0x6495ed),
    ("skyblue", 0x87ceeb),
    ("deepskyblue", 0x00bfff),
    ("seagreen", 0x2e8b57),
    ("forestgreen", 0x228b22),
    ("limegreen", 0x32cd32),
    ("firebrick", 0xb22222),
    ("chocolate", 0xd2691e),
    ("tan", 0xd2b48c),
    ("wheat", 0xf5deb3),
    ("linen", 0xfaf0e6),
    ("snow", 0xfffafa),
    ("honeydew", 0xf0fff0),
    ("aliceblue", 0xf0f8ff),
    ("ghostwhite", 0xf8f8ff),
    ("rebeccapurple", 0x663399),
];

fn hex_digit(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color {
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |mut t: f32| -> u8 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
    };
    Color {
        r: f(h + 1.0 / 3.0),
        g: f(h),
        b: f(h - 1.0 / 3.0),
    }
}

/// Un color de CSS con su opacidad. `None` si no se entiende.
pub fn parse_rgba(v: &str) -> Option<Rgba> {
    let v = v.trim().to_ascii_lowercase();
    if v == "transparent" {
        return Some(Rgba {
            c: Color::BLACK,
            a: 0,
        });
    }
    if let Some(h) = v.strip_prefix('#') {
        let b = h.as_bytes();
        let (r, g, bl, a) = match b.len() {
            3 | 4 => {
                let d = |i: usize| hex_digit(b[i]).map(|x| x * 17);
                (d(0)?, d(1)?, d(2)?, if b.len() == 4 { d(3)? } else { 255 })
            }
            6 | 8 => {
                let d = |i: usize| Some(hex_digit(b[i])? * 16 + hex_digit(b[i + 1])?);
                (d(0)?, d(2)?, d(4)?, if b.len() == 8 { d(6)? } else { 255 })
            }
            _ => return None,
        };
        return Some(Rgba {
            c: Color { r, g, b: bl },
            a,
        });
    }
    let func = v.find('(').map(|p| (&v[..p], &v[p + 1..]));
    if let Some((name, rest)) = func {
        let inner = rest.strip_suffix(')').unwrap_or(rest);
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        let alpha = parts
            .get(3)
            .and_then(|a| match a.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok().map(|x| x / 100.0),
                None => a.parse::<f32>().ok(),
            })
            .unwrap_or(1.0);
        let a = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        match name {
            "rgb" | "rgba" => {
                let comp = |p: &str| -> Option<u8> {
                    if let Some(pct) = p.strip_suffix('%') {
                        return pct.parse::<f32>().ok().map(|x| (x * 2.55) as u8);
                    }
                    p.parse::<f32>().ok().map(|x| x.clamp(0.0, 255.0) as u8)
                };
                return Some(Rgba {
                    c: Color {
                        r: comp(parts.first()?)?,
                        g: comp(parts.get(1)?)?,
                        b: comp(parts.get(2)?)?,
                    },
                    a,
                });
            }
            "hsl" | "hsla" => {
                let h = parts.first()?.trim_end_matches("deg").parse::<f32>().ok()?;
                let pct = |p: &str| {
                    p.trim_end_matches('%')
                        .parse::<f32>()
                        .ok()
                        .map(|x| x / 100.0)
                };
                return Some(Rgba {
                    c: hsl_to_rgb(h, pct(parts.get(1)?)?, pct(parts.get(2)?)?),
                    a,
                });
            }
            _ => return None,
        }
    }
    NAMED
        .iter()
        .find(|(n, _)| *n == v)
        .map(|(_, c)| Rgba::opaque(Color::hex(*c)))
}

/// Un color de CSS. `None` si no se entiende o es casi transparente.
pub fn parse_color(v: &str) -> Option<Color> {
    parse_rgba(v).filter(|c| c.a >= 80).map(|c| c.c)
}

/// El color de un `background: …` (que puede tener imágenes, posiciones, degradados).
pub fn color_in(v: &str) -> Option<Rgba> {
    if let Some(c) = parse_rgba(v) {
        return Some(c);
    }
    for t in tokens(v) {
        if let Some(c) = parse_rgba(t) {
            return Some(c);
        }
        // Un degradado: se usa su primer color (mejor que nada).
        let lower = t.to_ascii_lowercase();
        if lower.contains("gradient(")
            && let Some(inner) = lower.split_once('(').map(|x| x.1)
        {
            for piece in split_top(inner.trim_end_matches(')'), b',') {
                if let Some(c) = tokens(piece.trim()).first().and_then(|t| parse_rgba(t)) {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// Lo de adentro de `url(…)`.
pub fn url_in(v: &str) -> Option<String> {
    let lower = v.to_ascii_lowercase();
    let start = lower.find("url(")? + 4;
    let end = v[start..].find(')')? + start;
    let u = v[start..end].trim().trim_matches(['"', '\'']);
    (!u.is_empty()).then(|| u.to_string())
}

/// Un largo de CSS: fijo, porcentaje de algo que se sabe al maquetar, o una cuenta (`calc()`,
/// `min()`, `max()`, `clamp()`) que mezcla los dos.
#[derive(Clone, Debug, PartialEq)]
pub enum Len {
    Auto,
    Px(f32),
    Pct(f32),
    Calc(Rc<Calc>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Calc {
    Px(f32),
    Pct(f32),
    Num(f32),
    Add(Box<Calc>, Box<Calc>),
    Sub(Box<Calc>, Box<Calc>),
    Mul(Box<Calc>, Box<Calc>),
    Div(Box<Calc>, Box<Calc>),
    Min(Vec<Calc>),
    Max(Vec<Calc>),
}

impl Calc {
    fn eval(&self, base: f32) -> f32 {
        match self {
            Calc::Px(v) | Calc::Num(v) => *v,
            Calc::Pct(p) => p * base / 100.0,
            Calc::Add(a, b) => a.eval(base) + b.eval(base),
            Calc::Sub(a, b) => a.eval(base) - b.eval(base),
            Calc::Mul(a, b) => a.eval(base) * b.eval(base),
            Calc::Div(a, b) => {
                let d = b.eval(base);
                if d == 0.0 { 0.0 } else { a.eval(base) / d }
            }
            Calc::Min(v) => v.iter().map(|c| c.eval(base)).fold(f32::MAX, f32::min),
            Calc::Max(v) => v.iter().map(|c| c.eval(base)).fold(f32::MIN, f32::max),
        }
    }

    fn uses_pct(&self) -> bool {
        match self {
            Calc::Pct(_) => true,
            Calc::Px(_) | Calc::Num(_) => false,
            Calc::Add(a, b) | Calc::Sub(a, b) | Calc::Mul(a, b) | Calc::Div(a, b) => {
                a.uses_pct() || b.uses_pct()
            }
            Calc::Min(v) | Calc::Max(v) => v.iter().any(Calc::uses_pct),
        }
    }
}

impl Len {
    /// En píxeles; `base` es contra qué se miden los porcentajes (`None` si no se sabe).
    pub fn resolve(&self, base: Option<i32>) -> Option<i32> {
        match self {
            Len::Auto => None,
            Len::Px(v) => Some(*v as i32),
            Len::Pct(p) => base.map(|b| (p * b as f32 / 100.0) as i32),
            Len::Calc(c) => {
                if c.uses_pct() && base.is_none() {
                    None
                } else {
                    Some(c.eval(base.unwrap_or(0) as f32) as i32)
                }
            }
        }
    }

    pub fn is_auto(&self) -> bool {
        matches!(self, Len::Auto)
    }

    /// ¿Depende del tamaño del contenedor?
    pub fn is_relative(&self) -> bool {
        match self {
            Len::Pct(_) => true,
            Len::Calc(c) => c.uses_pct(),
            _ => false,
        }
    }
}

/// Contra qué se miden las unidades relativas.
#[derive(Clone, Copy, Debug)]
pub struct Units {
    /// Tamaño de letra del elemento (`em`).
    pub em: f32,
    /// Tamaño de letra de la raíz (`rem`).
    pub rem: f32,
    pub vw: f32,
    pub vh: f32,
}

impl Default for Units {
    fn default() -> Self {
        Units {
            em: 16.0,
            rem: 16.0,
            vw: 1024.0,
            vh: 700.0,
        }
    }
}

/// Un número con unidad → cuenta (`Calc::Px`, `Calc::Pct` o `Calc::Num`).
fn dimension(t: &str, u: Units) -> Option<Calc> {
    let t = t.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e'))
        .unwrap_or(t.len());
    // "e" puede ser el comienzo de "em"/"ex".
    let (num, unit) = if t[..split].ends_with('e') && split < t.len() {
        (&t[..split - 1], &t[split - 1..])
    } else {
        (&t[..split], &t[split..])
    };
    let n: f32 = num.parse().ok()?;
    let unit = unit.to_ascii_lowercase();
    Some(match unit.as_str() {
        "" => Calc::Num(n),
        "px" => Calc::Px(n),
        "%" => Calc::Pct(n),
        "em" => Calc::Px(n * u.em),
        "rem" => Calc::Px(n * u.rem),
        "ex" | "ch" => Calc::Px(n * u.em / 2.0),
        "cap" | "lh" => Calc::Px(n * u.em * 1.2),
        "vw" | "svw" | "lvw" | "dvw" | "vi" => Calc::Px(n * u.vw / 100.0),
        "vh" | "svh" | "lvh" | "dvh" | "vb" => Calc::Px(n * u.vh / 100.0),
        "vmin" => Calc::Px(n * u.vw.min(u.vh) / 100.0),
        "vmax" => Calc::Px(n * u.vw.max(u.vh) / 100.0),
        "pt" => Calc::Px(n * 4.0 / 3.0),
        "pc" => Calc::Px(n * 16.0),
        "in" => Calc::Px(n * 96.0),
        "cm" => Calc::Px(n * 96.0 / 2.54),
        "mm" => Calc::Px(n * 96.0 / 25.4),
        "q" => Calc::Px(n * 96.0 / 101.6),
        "fr" => return None,
        _ => return None,
    })
}

/// Parser de cuentas: `100% - 2 * (1rem + 3px)`.
struct CalcParser<'a> {
    s: &'a str,
    i: usize,
    u: Units,
}

impl CalcParser<'_> {
    fn ws(&mut self) {
        while self.s[self.i..].starts_with(char::is_whitespace) {
            self.i += 1;
        }
    }

    fn expr(&mut self) -> Option<Calc> {
        let mut left = self.term()?;
        loop {
            self.ws();
            let rest = &self.s[self.i..];
            if rest.starts_with('+') {
                self.i += 1;
                left = Calc::Add(Box::new(left), Box::new(self.term()?));
            } else if rest.starts_with('-') {
                self.i += 1;
                left = Calc::Sub(Box::new(left), Box::new(self.term()?));
            } else {
                return Some(left);
            }
        }
    }

    fn term(&mut self) -> Option<Calc> {
        let mut left = self.factor()?;
        loop {
            self.ws();
            let rest = &self.s[self.i..];
            if rest.starts_with('*') {
                self.i += 1;
                left = Calc::Mul(Box::new(left), Box::new(self.factor()?));
            } else if rest.starts_with('/') {
                self.i += 1;
                left = Calc::Div(Box::new(left), Box::new(self.factor()?));
            } else {
                return Some(left);
            }
        }
    }

    fn factor(&mut self) -> Option<Calc> {
        self.ws();
        let rest = &self.s[self.i..];
        if rest.starts_with('(') {
            self.i += 1;
            let e = self.expr()?;
            self.ws();
            if self.s[self.i..].starts_with(')') {
                self.i += 1;
            }
            return Some(e);
        }
        // Funciones anidadas: calc(), min(), max(), clamp().
        for f in ["calc(", "min(", "max(", "clamp(", "-webkit-calc("] {
            if rest.to_ascii_lowercase().starts_with(f) {
                let open = self.i + f.len() - 1;
                let close = close_paren(self.s, open)?;
                let inner = &self.s[open + 1..close];
                self.i = close + 1;
                return func(&f[..f.len() - 1], inner, self.u);
            }
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '+' | '*' | '/' | ')' | ','))
            .unwrap_or(rest.len());
        // Un "-" pegado es el signo del número (`-2px`); uno con espacios, una resta.
        let end = if end == 0 && rest.starts_with(['+', '-']) {
            rest[1..]
                .find(|c: char| c.is_whitespace() || matches!(c, '+' | '*' | '/' | ')' | ','))
                .map_or(rest.len(), |e| e + 1)
        } else {
            end
        };
        let tok = &rest[..end];
        self.i += end;
        dimension(tok, self.u)
    }
}

fn close_paren(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

fn func(name: &str, inner: &str, u: Units) -> Option<Calc> {
    let args = || -> Option<Vec<Calc>> {
        split_top(inner, b',')
            .into_iter()
            .map(|a| {
                let mut p = CalcParser { s: a, i: 0, u };
                p.expr()
            })
            .collect()
    };
    match name.to_ascii_lowercase().as_str() {
        "calc" | "-webkit-calc" => {
            let mut p = CalcParser { s: inner, i: 0, u };
            p.expr()
        }
        "min" => Some(Calc::Min(args()?)),
        "max" => Some(Calc::Max(args()?)),
        "clamp" => {
            let mut a = args()?;
            if a.len() != 3 {
                return None;
            }
            let max = a.pop()?;
            let val = a.pop()?;
            let min = a.pop()?;
            Some(Calc::Max(alloc::vec![
                min,
                Calc::Min(alloc::vec![val, max])
            ]))
        }
        _ => None,
    }
}

/// Un largo (`12px`, `1.5em`, `50%`, `calc(100% - 20px)`, `auto`).
pub fn parse_len(v: &str, u: Units) -> Option<Len> {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "auto"
            | "none"
            | "initial"
            | "unset"
            | "fit-content"
            | "max-content"
            | "min-content"
            | "-webkit-fill-available"
            | "stretch"
            | "revert"
            | "normal"
    ) {
        return Some(Len::Auto);
    }
    if lower.starts_with("fit-content(") {
        return Some(Len::Auto);
    }
    for f in ["calc(", "min(", "max(", "clamp(", "-webkit-calc("] {
        if lower.starts_with(f) {
            let open = f.len() - 1;
            let close = close_paren(v, open)?;
            let c = func(&f[..f.len() - 1], &v[open + 1..close], u)?;
            return Some(match c {
                Calc::Px(p) | Calc::Num(p) => Len::Px(p),
                Calc::Pct(p) => Len::Pct(p),
                other => {
                    if other.uses_pct() {
                        Len::Calc(Rc::new(other))
                    } else {
                        Len::Px(other.eval(0.0))
                    }
                }
            });
        }
    }
    match dimension(&lower, u)? {
        Calc::Px(p) => Some(Len::Px(p)),
        Calc::Pct(p) => Some(Len::Pct(p)),
        // Un número sin unidad solo vale si es 0.
        Calc::Num(n) if n == 0.0 => Some(Len::Px(0.0)),
        _ => None,
    }
}

/// Un largo en píxeles, sin porcentajes (bordes, `gap` fijos, etc.).
pub fn parse_px(v: &str, u: Units) -> Option<i32> {
    parse_len(v, u)?.resolve(Some(0))
}

/// Un tamaño de letra (`font-size`), según el del padre.
pub fn font_size(v: &str, parent: f32, u: Units) -> Option<f32> {
    let size = match v.trim().to_ascii_lowercase().as_str() {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "xxx-large" => 48.0,
        "smaller" => parent * 5.0 / 6.0,
        "larger" => parent * 1.2,
        "inherit" => parent,
        other => {
            let u = Units { em: parent, ..u };
            let len = parse_len(other, u)?;
            len.resolve(Some(parent as i32))? as f32
        }
    };
    Some(size.clamp(6.0, 96.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(html: &str, css: &str, find: &str, prop: &str) -> Vec<String> {
        let dom = super::super::dom::parse(html);
        let m = Matcher::new(&dom);
        let mut s = Sheet::new(Media::default());
        s.add(css);
        let n = (0..dom.nodes.len())
            .find(|&i| dom.nodes[i].attr("id") == Some(find))
            .expect("elemento");
        s.matching(&m, n)
            .into_iter()
            .filter(|d| d.prop == prop)
            .map(|d| d.value.clone())
            .collect()
    }

    #[test]
    fn cascada_y_especificidad() {
        let css = "/* comentario */ p { color: red } .nota { color: blue } #x.nota { color: green }\
             a:hover { color: pink } @media print { p { color: black } }\
             @media (min-width: 600px) { p { font-size: 20px } }\
             @media (max-width: 480px) { p { font-size: 9px } }";
        let html = "<body><p id=x class=nota>hola</p></body>";
        assert_eq!(values(html, css, "x", "color"), ["red", "blue", "green"]);
        assert_eq!(values(html, css, "x", "font-size"), ["20px"]);
        let imp = "div p { color: gray !important } #y { color: red }";
        assert_eq!(
            values("<div><span><p id=y>x</p></span></div>", imp, "y", "color")
                .last()
                .unwrap(),
            "gray",
            "!important gana"
        );
    }

    #[test]
    fn selectores_completos() {
        let html = "<ul id=l><li id=a class='x y'>1</li><li id=b>2</li><li id=c lang=es-AR>3</li></ul>\
                    <input id=i type=checkbox checked><label id=lab>x</label><p id=e></p>";
        let has = |css: &str, id: &str| !values(html, css, id, "color").is_empty();
        assert!(has("ul > li:first-child { color: red }", "a"));
        assert!(!has("ul > li:first-child { color: red }", "b"));
        assert!(has("li:last-child { color: red }", "c"));
        assert!(has("li:nth-child(2n+1) { color: red }", "c"));
        assert!(!has("li:nth-child(odd) { color: red }", "b"));
        assert!(has("li + li { color: red }", "b"));
        assert!(has("#a ~ li { color: red }", "c"));
        assert!(has("[class~=y] { color: red }", "a"));
        assert!(has("[lang|=es] { color: red }", "c"));
        assert!(has("li:not(.x) { color: red }", "b"));
        assert!(!has("li:not(.x) { color: red }", "a"));
        assert!(has(":is(h1, li.y) { color: red }", "a"));
        assert!(has("input:checked + label { color: red }", "lab"));
        assert!(has("input[type=\"checkbox\" i] { color: red }", "i"));
        assert!(has("p:empty { color: red }", "e"));
        assert!(!has("li:hover { color: red }", "a"));
        assert!(!has("li::before { color: red }", "a"));
        assert!(
            has("li::before, #b { color: red }", "b"),
            "uno malo no tapa al resto"
        );
        assert!(has("ul:has(> li.x) { color: red }", "l"));
    }

    #[test]
    fn media_con_el_ancho_de_la_ventana() {
        let m = Media {
            width: 800,
            height: 600,
        };
        assert!(media_ok("screen and (min-width: 720px)", m));
        assert!(!media_ok("(min-width: 1000px)", m));
        assert!(media_ok("(max-width: 50em)", m));
        assert!(media_ok("(width >= 600px)", m));
        assert!(!media_ok("(400px <= width <= 700px)", m));
        assert!(!media_ok("print", m));
        assert!(media_ok("print, (min-width: 100px)", m));
        assert!(!media_ok("all and (max-width:calc(640px - 1px))", m));
        assert!(!media_ok("(prefers-color-scheme: dark)", m));
        assert!(!media_ok("not screen", m));
    }

    #[test]
    fn colores_largos_y_cuentas() {
        assert_eq!(parse_color("#fff"), Some(Color::hex(0xffffff)));
        assert_eq!(parse_color("#1A73E8"), Some(Color::hex(0x1a73e8)));
        assert_eq!(parse_color("rgb(255, 0, 10)"), Some(Color::hex(0xff000a)));
        assert_eq!(
            parse_color("rgb(255 0 10 / 50%)"),
            Some(Color::hex(0xff000a))
        );
        assert_eq!(parse_rgba("rgba(0,0,0,.5)").unwrap().a, 128);
        assert_eq!(parse_color("rgba(0,0,0,0)"), None);
        assert_eq!(parse_color("hsl(0, 100%, 50%)"), Some(Color::hex(0xff0000)));
        assert_eq!(
            color_in("url(x.png) no-repeat #202124").map(|c| c.c),
            Some(Color::hex(0x202124))
        );
        assert_eq!(
            color_in("linear-gradient(to right, #fff 0%, #000 100%)").map(|c| c.c),
            Some(Color::WHITE)
        );
        assert_eq!(url_in("url('a/b.png') center"), Some("a/b.png".into()));
        let u = Units::default();
        assert_eq!(parse_len("1.5em", u).unwrap().resolve(None), Some(24));
        assert_eq!(parse_len("50%", u).unwrap().resolve(Some(800)), Some(400));
        assert_eq!(parse_len("50%", u).unwrap().resolve(None), None);
        let c = parse_len("calc(100% - 2 * 10px)", u).unwrap();
        assert_eq!(c.resolve(Some(500)), Some(480));
        let m = parse_len("min(100%, 1200px)", u).unwrap();
        assert_eq!(m.resolve(Some(800)), Some(800));
        assert_eq!(m.resolve(Some(2000)), Some(1200));
        let cl = parse_len("clamp(1rem, 2.5vw, 2rem)", u).unwrap();
        assert_eq!(cl.resolve(Some(0)), Some(25));
        assert_eq!(parse_len("-1px", u).unwrap().resolve(None), Some(-1));
        assert_eq!(font_size("x-large", 16.0, u), Some(24.0));
        assert_eq!(font_size("0.875rem", 20.0, u), Some(14.0));
        assert_eq!(font_size("80%", 20.0, u), Some(16.0));
        assert_eq!(
            parse_decls("color: red; background: url(data:a;b) #fff !important; *zoom: 1").len(),
            2
        );
        assert_eq!(
            tokens("1px solid rgb(0, 0, 0)"),
            ["1px", "solid", "rgb(0, 0, 0)"]
        );
    }
}
