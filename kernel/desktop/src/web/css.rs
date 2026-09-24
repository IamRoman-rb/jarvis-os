//! CSS: hojas de estilo, selectores y la cascada.
//!
//! Lo que se entiende (lo que más cambia cómo se ve una página sin maquetación compleja):
//!
//! - **Selectores**: etiqueta, `#id`, `.clase`, `*`, combinados (`div.nota#x`), descendiente
//!   (`nav a`) e hijo (`ul > li`), y listas (`h1, h2`). Los que tienen pseudoclases (`:hover`) o
//!   atributos (`[type=x]`) se ignoran (salvo `:root`, `:link` y `:visited`).
//! - **Propiedades**: `color`, `background(-color)`, `font-weight`, `font-size`, `text-align`,
//!   `display`, `visibility`, `text-decoration`, `text-transform`, `white-space`, `float`,
//!   `width`/`height` (imágenes y campos) y variables (`--x` y `var(--x, respaldo)`).
//! - **Cascada**: especificidad, orden, `!important` y estilos en línea (`style="…"`).
//! - **@media**: se aplican las reglas para pantallas anchas; `print` y las de celulares no.
//!
//! Las reglas se indexan por lo que exige su último compuesto (id, clase o etiqueta), como hacen
//! los navegadores: así cada elemento solo prueba las reglas que le pueden tocar.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Comb {
    Descendant,
    Child,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Clone, Debug)]
struct Selector {
    /// De izquierda a derecha; el combinador de cada parte es el que la une con la anterior.
    parts: Vec<(Comb, Compound)>,
    specificity: u32,
}

#[derive(Clone, Debug)]
pub struct Decl {
    pub prop: String,
    pub value: String,
    pub important: bool,
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Selector,
    order: u32,
    decls: Vec<Decl>,
}

/// Un elemento, como lo ve el que compara selectores.
#[derive(Clone, Debug, Default)]
pub struct ElemInfo {
    pub tag: String,
    pub id: String,
    pub classes: Vec<String>,
}

#[derive(Default)]
pub struct Sheet {
    rules: Vec<Rule>,
    by_id: BTreeMap<String, Vec<usize>>,
    by_class: BTreeMap<String, Vec<usize>>,
    by_tag: BTreeMap<String, Vec<usize>>,
    universal: Vec<usize>,
    /// Variables (`--algo: valor`) definidas en `:root`, `html` o `body`.
    pub vars: BTreeMap<String, String>,
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

/// Busca la llave que cierra la que abre en `open` (contando anidadas).
fn matching_brace(s: &str, open: usize) -> usize {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + i;
                }
            }
            _ => {}
        }
    }
    s.len()
}

/// ¿Una condición `@media` vale para una pantalla de escritorio (~1280 px)?
fn media_ok(cond: &str) -> bool {
    let c = cond.to_ascii_lowercase();
    if c.contains("print") && !c.contains("screen") {
        return false;
    }
    if c.contains("prefers-color-scheme: dark") || c.contains("prefers-color-scheme:dark") {
        return false;
    }
    // max-width: N px → solo si N >= 1000; min-width: N px → solo si N <= 1280.
    let num_after = |key: &str| -> Option<i32> {
        let at = c.find(key)? + key.len();
        let rest = c[at..].trim_start_matches([':', ' ']);
        let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        let n: i32 = digits.parse().ok()?;
        Some(if rest[digits.len()..].starts_with("em") {
            n * 16
        } else {
            n
        })
    };
    if let Some(n) = num_after("max-width")
        && n < 1000
    {
        return false;
    }
    if let Some(n) = num_after("min-width")
        && n > 1280
    {
        return false;
    }
    true
}

pub fn parse_decls(block: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    // Los `;` adentro de paréntesis (url(data:…;base64)) no separan.
    let mut depth = 0;
    let mut start = 0;
    let bytes = block.as_bytes();
    let mut pieces = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
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
        let mut value = value.trim().to_string();
        let important = value.to_ascii_lowercase().ends_with("!important");
        if important {
            value.truncate(value.len() - "!important".len());
            value = value.trim().to_string();
        }
        if !prop.is_empty() && !value.is_empty() {
            out.push(Decl {
                prop,
                value,
                important,
            });
        }
    }
    out
}

fn parse_compound(s: &str) -> Option<Compound> {
    let mut c = Compound::default();
    let mut rest = s;
    // Pseudoclases que no cambian nada para nosotros; las demás descartan la regla.
    for ok in [":root", ":link", ":visited", "::before", "::after"] {
        if let Some(p) = rest.find(ok) {
            if ok.starts_with("::") {
                return None; // contenido generado: no lo mostramos
            }
            let mut owned = String::from(&rest[..p]);
            owned.push_str(&rest[p + ok.len()..]);
            if ok == ":root" && owned.is_empty() {
                return Some(Compound {
                    tag: Some("html".into()),
                    ..Default::default()
                });
            }
            return parse_compound(&owned);
        }
    }
    if rest.contains([':', '[', '+', '~']) {
        return None;
    }
    let tag_end = rest.find(['.', '#']).unwrap_or(rest.len());
    let tag = &rest[..tag_end];
    if !tag.is_empty() && tag != "*" {
        c.tag = Some(tag.to_ascii_lowercase());
    }
    rest = &rest[tag_end..];
    while !rest.is_empty() {
        let kind = rest.as_bytes()[0];
        let end = rest[1..].find(['.', '#']).map_or(rest.len(), |e| e + 1);
        let name = rest[1..end].to_string();
        if name.is_empty() {
            return None;
        }
        if kind == b'#' {
            c.id = Some(name);
        } else {
            c.classes.push(name);
        }
        rest = &rest[end..];
    }
    Some(c)
}

fn parse_selector(s: &str) -> Option<Selector> {
    let s = s.trim().replace('>', " > ");
    let mut parts = Vec::new();
    let mut comb = Comb::Descendant;
    for tok in s.split_whitespace() {
        if tok == ">" {
            comb = Comb::Child;
            continue;
        }
        let c = parse_compound(tok)?;
        parts.push((comb, c));
        comb = Comb::Descendant;
    }
    if parts.is_empty() {
        return None;
    }
    let (mut a, mut b, mut c) = (0u32, 0u32, 0u32);
    for (_, p) in &parts {
        a += p.id.is_some() as u32;
        b += p.classes.len() as u32;
        c += p.tag.is_some() as u32;
    }
    Some(Selector {
        parts,
        specificity: a * 10_000 + b * 100 + c,
    })
}

impl Sheet {
    pub fn new() -> Sheet {
        Sheet::default()
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
            let head = css[i..open].trim();
            let close = matching_brace(css, open);
            let body = &css[open + 1..close.min(css.len())];
            i = close + 1;
            // Reglas `@` sin bloque antes de esta (`@import …;`, `@charset …;`).
            let head = head.rsplit(';').next().unwrap_or(head).trim();
            if let Some(at) = head.strip_prefix('@') {
                let lower = at.to_ascii_lowercase();
                let nested = (lower.starts_with("media") && media_ok(&lower[5..]))
                    || lower.starts_with("supports")
                    || lower.starts_with("layer")
                    || lower.starts_with("container");
                if nested && depth < 4 {
                    self.add_rules(body, depth + 1);
                }
                continue;
            }
            let decls = parse_decls(body);
            if decls.is_empty() {
                continue;
            }
            for sel in head.split(',') {
                let Some(selector) = parse_selector(sel) else {
                    continue;
                };
                // Variables globales.
                let last = &selector.parts[selector.parts.len() - 1].1;
                if selector.parts.len() == 1
                    && last.id.is_none()
                    && last.classes.is_empty()
                    && matches!(last.tag.as_deref(), Some("html" | "body") | None)
                {
                    for d in decls.iter().filter(|d| d.prop.starts_with("--")) {
                        self.vars.insert(d.prop.clone(), d.value.clone());
                    }
                }
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
                    decls: decls.clone(),
                });
            }
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Las declaraciones que le tocan a `elem` (el último de `path`), ya ordenadas por la
    /// cascada: las que ganan quedan al final.
    pub fn matching(&self, path: &[ElemInfo]) -> Vec<&Decl> {
        let Some(elem) = path.last() else {
            return Vec::new();
        };
        let mut cands: Vec<usize> = Vec::new();
        if !elem.id.is_empty()
            && let Some(v) = self.by_id.get(&elem.id)
        {
            cands.extend(v);
        }
        for c in &elem.classes {
            if let Some(v) = self.by_class.get(c) {
                cands.extend(v);
            }
        }
        if let Some(v) = self.by_tag.get(&elem.tag) {
            cands.extend(v);
        }
        cands.extend(&self.universal);
        cands.sort_unstable();
        cands.dedup();
        let mut hits: Vec<&Rule> = cands
            .into_iter()
            .map(|i| &self.rules[i])
            .filter(|r| matches(&r.selector, path))
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

    /// Reemplaza `var(--x, respaldo)`.
    pub fn resolve<'a>(
        &self,
        value: &'a str,
        locals: &BTreeMap<String, String>,
    ) -> alloc::borrow::Cow<'a, str> {
        if !value.contains("var(") {
            return alloc::borrow::Cow::Borrowed(value);
        }
        let mut s = value.to_string();
        for _ in 0..8 {
            let Some(start) = s.find("var(") else { break };
            let mut depth = 0;
            let mut end = s.len();
            for (i, c) in s[start..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = start + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let inner = s[start + 4..end.min(s.len())].to_string();
            let (name, fallback) = match inner.split_once(',') {
                Some((n, f)) => (n.trim().to_string(), f.trim().to_string()),
                None => (inner.trim().to_string(), String::new()),
            };
            let v = locals
                .get(&name)
                .or_else(|| self.vars.get(&name))
                .cloned()
                .unwrap_or(fallback);
            s.replace_range(start..(end + 1).min(s.len()), &v);
        }
        alloc::borrow::Cow::Owned(s)
    }
}

fn compound_matches(c: &Compound, e: &ElemInfo) -> bool {
    c.tag.as_ref().is_none_or(|t| *t == e.tag)
        && c.id.as_ref().is_none_or(|i| *i == e.id)
        && c.classes.iter().all(|cl| e.classes.iter().any(|x| x == cl))
}

fn matches(sel: &Selector, path: &[ElemInfo]) -> bool {
    let n = sel.parts.len();
    if !compound_matches(&sel.parts[n - 1].1, &path[path.len() - 1]) {
        return false;
    }
    // Hacia atrás: cada parte anterior tiene que estar en un ancestro.
    let mut pos = path.len() - 1; // elemento que coincidió con la parte k
    for k in (0..n - 1).rev() {
        let comb = sel.parts[k + 1].0;
        let want = &sel.parts[k].1;
        match comb {
            Comb::Child => {
                if pos == 0 || !compound_matches(want, &path[pos - 1]) {
                    return false;
                }
                pos -= 1;
            }
            Comb::Descendant => {
                let found = (0..pos).rev().find(|&j| compound_matches(want, &path[j]));
                match found {
                    Some(j) => pos = j,
                    None => return false,
                }
            }
        }
    }
    true
}

// --- valores ----------------------------------------------------------------------------------

const NAMED: [(&str, u32); 40] = [
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
    ("lightgray", 0xd3d3d3),
    ("lightgrey", 0xd3d3d3),
    ("lightblue", 0xadd8e6),
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
];

fn hex_digit(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Un color de CSS. `None` si no se entiende o es transparente.
pub fn parse_color(v: &str) -> Option<Color> {
    let v = v.trim().to_ascii_lowercase();
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
        return (a >= 80).then_some(Color { r, g, b: bl });
    }
    if let Some(inner) = v
        .strip_prefix("rgba(")
        .or_else(|| v.strip_prefix("rgb("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        let comp = |p: &str| -> Option<u8> {
            if let Some(pct) = p.strip_suffix('%') {
                return pct.parse::<f32>().ok().map(|x| (x * 2.55) as u8);
            }
            p.parse::<f32>().ok().map(|x| x.clamp(0.0, 255.0) as u8)
        };
        let alpha = parts
            .get(3)
            .and_then(|a| match a.strip_suffix('%') {
                Some(p) => p.parse::<f32>().ok().map(|x| x / 100.0),
                None => a.parse::<f32>().ok(),
            })
            .unwrap_or(1.0);
        if alpha < 0.3 {
            return None;
        }
        return Some(Color {
            r: comp(parts.first()?)?,
            g: comp(parts.get(1)?)?,
            b: comp(parts.get(2)?)?,
        });
    }
    NAMED
        .iter()
        .find(|(n, _)| *n == v)
        .map(|(_, c)| Color::hex(*c))
}

/// El primer color que aparezca en un `background: …` (que puede tener imágenes, etc.).
pub fn color_in(v: &str) -> Option<Color> {
    if let Some(c) = parse_color(v) {
        return Some(c);
    }
    let mut depth = 0;
    let mut start = 0;
    let mut tokens = Vec::new();
    for (i, ch) in v.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ' ' if depth == 0 => {
                tokens.push(&v[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    tokens.push(&v[start..]);
    tokens.into_iter().find_map(parse_color)
}

/// Un largo en píxeles (`12px`, `1.5em` del tamaño de letra `em`, `50%` de `pct`).
pub fn parse_length(v: &str, em: i32, pct: i32) -> Option<i32> {
    let v = v.trim().to_ascii_lowercase();
    let num = |s: &str| s.trim().parse::<f32>().ok();
    if let Some(n) = v.strip_suffix("px") {
        return num(n).map(|x| x as i32);
    }
    if let Some(n) = v.strip_suffix("rem") {
        return num(n).map(|x| (x * 16.0) as i32);
    }
    if let Some(n) = v.strip_suffix("em") {
        return num(n).map(|x| (x * em as f32) as i32);
    }
    if let Some(n) = v.strip_suffix('%') {
        return num(n).map(|x| (x * pct as f32 / 100.0) as i32);
    }
    if let Some(n) = v.strip_suffix("pt") {
        return num(n).map(|x| (x * 4.0 / 3.0) as i32);
    }
    if v == "0" {
        return Some(0);
    }
    None
}

pub fn font_size(v: &str, parent: i32) -> Option<i32> {
    let size = match v.trim() {
        "xx-small" | "x-small" => 11,
        "small" | "smaller" => 13,
        "medium" => 16,
        "large" | "larger" => 19,
        "x-large" => 24,
        "xx-large" | "xxx-large" => 32,
        other => parse_length(other, parent, parent)?,
    };
    Some(size.clamp(8, 72))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elem(tag: &str, id: &str, classes: &[&str]) -> ElemInfo {
        ElemInfo {
            tag: tag.into(),
            id: id.into(),
            classes: classes.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn values(sheet: &Sheet, path: &[ElemInfo], prop: &str) -> Vec<String> {
        sheet
            .matching(path)
            .into_iter()
            .filter(|d| d.prop == prop)
            .map(|d| d.value.clone())
            .collect()
    }

    #[test]
    fn cascada_y_especificidad() {
        let mut s = Sheet::new();
        s.add(
            "/* comentario */ p { color: red } .nota { color: blue } #x.nota { color: green }\
             div p { color: gray !important } a:hover { color: pink } ul > li { color: navy }\
             @media print { p { color: black } } @media (min-width: 600px) { p { font-size: 20px } }\
             @media (max-width: 480px) { p { font-size: 9px } }",
        );
        let p = [elem("body", "", &[]), elem("p", "x", &["nota"])];
        assert_eq!(values(&s, &p, "color"), ["red", "blue", "green"]);
        let dp = [
            elem("div", "", &[]),
            elem("span", "", &[]),
            elem("p", "", &[]),
        ];
        assert_eq!(
            values(&s, &dp, "color").last().unwrap(),
            "gray",
            "!important gana"
        );
        assert_eq!(values(&s, &dp, "font-size"), ["20px"]);
        let li = [elem("ul", "", &[]), elem("li", "", &[])];
        assert_eq!(values(&s, &li, "color"), ["navy"]);
        let li2 = [
            elem("ul", "", &[]),
            elem("div", "", &[]),
            elem("li", "", &[]),
        ];
        assert!(values(&s, &li2, "color").is_empty(), "> es hijo directo");
    }

    #[test]
    fn colores_largos_y_variables() {
        assert_eq!(parse_color("#fff"), Some(Color::hex(0xffffff)));
        assert_eq!(parse_color("#1A73E8"), Some(Color::hex(0x1a73e8)));
        assert_eq!(parse_color("rgb(255, 0, 10)"), Some(Color::hex(0xff000a)));
        assert_eq!(parse_color("rgba(0,0,0,0)"), None);
        assert_eq!(parse_color("transparent"), None);
        assert_eq!(
            color_in("url(x.png) no-repeat #202124"),
            Some(Color::hex(0x202124))
        );
        assert_eq!(parse_length("1.5em", 16, 800), Some(24));
        assert_eq!(parse_length("50%", 16, 800), Some(400));
        assert_eq!(font_size("x-large", 16), Some(24));
        let mut s = Sheet::new();
        s.add(
            ":root { --fondo: #101010; --texto: var(--fondo) } body { color: var(--nada, blue) }",
        );
        let locals = BTreeMap::new();
        assert_eq!(s.resolve("var(--texto)", &locals), "#101010");
        assert_eq!(s.resolve("var(--nada, blue)", &locals), "blue");
        assert_eq!(
            parse_decls("color: red; background: url(data:a;b) #fff !important").len(),
            2
        );
    }
}
