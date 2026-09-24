//! HTML (+ CSS) → documento listo para maquetar.
//!
//! El HTML se convierte en un árbol ([`super::dom`]), se le calculan los estilos
//! ([`super::style`]: los del navegador, los `<style>`, las hojas externas que ya bajó el
//! navegador y los `style="…"`) y se juntan las cosas con las que el usuario interactúa: los
//! enlaces, los formularios y sus campos, y las imágenes que hay que bajar. La maquetación en
//! cajas la hace [`super::layout`].
//!
//! **Modo lectura** ([`Options::reader`]): sin los estilos de la página ni formularios, y sin lo
//! que no es el contenido (menús, cabeceras, pies, avisos), con los colores del tema.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::Color;

use super::css::{Media, Sheet};
use super::dom::{self, Dom, NodeKind};
pub use super::style::Options;
use super::style::{self, Display, Styled};

pub const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Password,
    Hidden,
    Submit,
    /// `<button type=button>` o `<input type=button>` (sin JavaScript no hacen nada).
    Button,
    Checkbox,
    Radio,
    Select,
    TextArea,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub kind: FieldKind,
    /// Índice en [`Document::forms`].
    pub form: Option<usize>,
    pub name: String,
    pub value: String,
    pub checked: bool,
    /// (valor, texto) de un `<select>`; en un `<button>`, `[(valor que se manda, "")]`.
    pub options: Vec<(String, String)>,
    pub placeholder: String,
    /// Nodo del árbol.
    pub node: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Form {
    pub action: String,
    pub post: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub src: String,
    pub alt: String,
    /// Es el fondo (`background-image`) de una caja, no un `<img>`.
    pub background: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub title: String,
    /// Los `href` tal como vienen en la página (se resuelven contra la dirección al hacer clic).
    pub links: Vec<String>,
    pub forms: Vec<Form>,
    pub fields: Vec<Field>,
    pub images: Vec<ImageRef>,
    /// Hojas de estilo externas (`<link rel=stylesheet>` y `@import`), para que el navegador las
    /// baje.
    pub stylesheets: Vec<String>,
    pub base: Option<String>,
    /// Casi no tiene texto y sí mucho JavaScript: la arma un programa que no podemos correr.
    pub needs_js: bool,
}

/// Qué tiene cada nodo (índices en las listas del documento, o [`NONE`]).
#[derive(Clone, Debug, Default)]
pub struct NodeInfo {
    /// El enlace en el que está (el `<a>` más cercano hacia arriba).
    pub link: Vec<u32>,
    pub field: Vec<u32>,
    pub image: Vec<u32>,
    pub bg_image: Vec<u32>,
    pub mask_image: Vec<u32>,
}

impl NodeInfo {
    pub fn link_of(&self, n: usize) -> Option<usize> {
        self.link
            .get(n)
            .copied()
            .filter(|&l| l != NONE)
            .map(|l| l as usize)
    }

    pub fn field_of(&self, n: usize) -> Option<usize> {
        self.field
            .get(n)
            .copied()
            .filter(|&l| l != NONE)
            .map(|l| l as usize)
    }

    pub fn image_of(&self, n: usize) -> Option<usize> {
        self.image
            .get(n)
            .copied()
            .filter(|&l| l != NONE)
            .map(|l| l as usize)
    }

    pub fn bg_image_of(&self, n: usize) -> Option<usize> {
        self.bg_image
            .get(n)
            .copied()
            .filter(|&l| l != NONE)
            .map(|l| l as usize)
    }

    pub fn mask_image_of(&self, n: usize) -> Option<usize> {
        self.mask_image
            .get(n)
            .copied()
            .filter(|&l| l != NONE)
            .map(|l| l as usize)
    }
}

/// Una página lista para maquetar: árbol, estilos, y lo que se puede tocar.
pub struct Prepared {
    pub dom: Dom,
    pub styled: Styled,
    pub info: NodeInfo,
    pub doc: Document,
    pub opts: Options,
}

/// HTML → página, con las hojas de estilo externas ya bajadas y el tamaño de la ventana.
pub fn prepare(html: &str, opts: Options, external_css: &[String], media: Media) -> Prepared {
    let dom = dom::parse(html);
    let mut ua = Sheet::new(media);
    ua.add(style::UA_CSS);
    let mut author = Sheet::new(media);
    if opts.reader {
        ua.add(style::READER_CSS);
    } else {
        for css in external_css {
            author.add(css);
        }
        for css in &dom.styles {
            author.add(css);
        }
    }
    let styled = style::compute(&dom, &ua, &author, opts, media);
    let mut doc = Document {
        title: dom.title.clone(),
        base: dom.base.clone(),
        ..Default::default()
    };
    if !opts.reader {
        doc.stylesheets = dom.stylesheet_links.clone();
        for i in &author.imports {
            if !doc.stylesheets.contains(i) {
                doc.stylesheets.push(i.clone());
            }
        }
    }
    let info = collect(&dom, &styled, &mut doc, opts);
    doc.needs_js = needs_js(&dom, &styled);
    Prepared {
        dom,
        styled,
        info,
        doc,
        opts,
    }
}

fn rendered(styled: &Styled, n: usize) -> bool {
    styled.get(n).is_some_and(|s| s.display != Display::None)
}

/// Recorre el árbol en orden y numera enlaces, formularios, campos e imágenes.
fn collect(dom: &Dom, styled: &Styled, doc: &mut Document, opts: Options) -> NodeInfo {
    let len = dom.nodes.len();
    let mut info = NodeInfo {
        link: alloc::vec![NONE; len],
        field: alloc::vec![NONE; len],
        image: alloc::vec![NONE; len],
        bg_image: alloc::vec![NONE; len],
        mask_image: alloc::vec![NONE; len],
    };
    // (nodo, enlace heredado, formulario)
    let mut stack: Vec<(usize, u32, Option<usize>)> = alloc::vec![(0, NONE, None)];
    while let Some((n, link, form)) = stack.pop() {
        let node = &dom.nodes[n];
        let mut link = link;
        let mut form = form;
        if let NodeKind::Element { name, .. } = &node.kind {
            let shown = rendered(styled, n);
            match name.as_str() {
                "a" | "area" => {
                    if let Some(h) = node.attr("href") {
                        doc.links.push(h.trim().to_string());
                        link = (doc.links.len() - 1) as u32;
                    }
                }
                "form" if !opts.reader => {
                    doc.forms.push(Form {
                        action: node.attr("action").unwrap_or("").to_string(),
                        post: node
                            .attr("method")
                            .is_some_and(|m| m.eq_ignore_ascii_case("post")),
                    });
                    form = Some(doc.forms.len() - 1);
                }
                "input" | "button" | "select" | "textarea" if !opts.reader => {
                    if let Some(f) = field(dom, n, form) {
                        doc.fields.push(f);
                        info.field[n] = (doc.fields.len() - 1) as u32;
                    }
                }
                "img" | "image" if shown && opts.images => {
                    let src = node
                        .attr("src")
                        .filter(|s| !s.is_empty() && !s.starts_with("data:"))
                        .or_else(|| node.attr("data-src"))
                        .or_else(|| {
                            node.attr("srcset")
                                .and_then(|s| s.split(',').next())
                                .and_then(|s| s.split_whitespace().next())
                        })
                        .unwrap_or("");
                    if !src.is_empty() && !src.starts_with("data:") {
                        doc.images.push(ImageRef {
                            src: src.to_string(),
                            alt: node.attr("alt").unwrap_or("").trim().to_string(),
                            background: false,
                        });
                        info.image[n] = (doc.images.len() - 1) as u32;
                    }
                }
                _ => {}
            }
            if shown
                && opts.images
                && let Some(bg) = styled.get(n).and_then(|s| s.bg_image.as_ref())
                && !bg.starts_with("data:")
            {
                doc.images.push(ImageRef {
                    src: bg.clone(),
                    alt: String::new(),
                    background: true,
                });
                info.bg_image[n] = (doc.images.len() - 1) as u32;
            }
            if shown
                && opts.images
                && let Some(m) = styled.get(n).and_then(|s| s.mask_image.as_ref())
                && !m.starts_with("data:")
            {
                // La misma máscara (un ícono) se pide una sola vez.
                let idx = match doc.images.iter().position(|i| i.src == *m) {
                    Some(i) => i,
                    None => {
                        doc.images.push(ImageRef {
                            src: m.clone(),
                            alt: String::new(),
                            background: true,
                        });
                        doc.images.len() - 1
                    }
                };
                info.mask_image[n] = idx as u32;
            }
            // Lo que no se ve no tiene enlaces ni imágenes (pero sí campos ocultos).
            if !shown && !opts.reader {
                collect_fields_only(dom, n, form, doc, &mut info);
                continue;
            }
            if !shown {
                continue;
            }
        }
        info.link[n] = link;
        for &c in node.children.iter().rev() {
            stack.push((c, link, form));
        }
    }
    info
}

/// Adentro de algo oculto solo interesan los campos (los `<input type=hidden>` de un formulario
/// se mandan igual).
fn collect_fields_only(
    dom: &Dom,
    n: usize,
    form: Option<usize>,
    doc: &mut Document,
    info: &mut NodeInfo,
) {
    let node = &dom.nodes[n];
    if matches!(node.name(), "input" | "select" | "textarea") && info.field[n] == NONE {
        if let Some(f) = field(dom, n, form) {
            doc.fields.push(f);
            info.field[n] = (doc.fields.len() - 1) as u32;
        }
        return;
    }
    for &c in &node.children {
        collect_fields_only(dom, c, form, doc, info);
    }
}

fn inner_text(dom: &Dom, n: usize) -> String {
    fn walk(dom: &Dom, n: usize, out: &mut String) {
        for &c in &dom.nodes[n].children {
            match &dom.nodes[c].kind {
                NodeKind::Text(t) => out.push_str(t),
                _ => walk(dom, c, out),
            }
        }
    }
    let mut s = String::new();
    walk(dom, n, &mut s);
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn field(dom: &Dom, n: usize, form: Option<usize>) -> Option<Field> {
    let node = &dom.nodes[n];
    let name = node.name();
    let ty = node.attr("type").unwrap_or("").to_ascii_lowercase();
    let kind = match name {
        "select" => FieldKind::Select,
        "textarea" => FieldKind::TextArea,
        "button" => match ty.as_str() {
            "button" | "reset" | "menu" => FieldKind::Button,
            _ => FieldKind::Submit,
        },
        _ => match ty.as_str() {
            "hidden" => FieldKind::Hidden,
            "submit" | "image" => FieldKind::Submit,
            "button" | "reset" => FieldKind::Button,
            "checkbox" => FieldKind::Checkbox,
            "radio" => FieldKind::Radio,
            "password" => FieldKind::Password,
            "file" | "color" | "range" => return None,
            _ => FieldKind::Text,
        },
    };
    let mut value = node.attr("value").unwrap_or("").to_string();
    let mut options = Vec::new();
    match kind {
        FieldKind::Submit | FieldKind::Button if name == "button" => {
            options.push((value.clone(), String::new()));
            value = inner_text(dom, n);
        }
        FieldKind::Submit if value.is_empty() => {
            value = if ty == "image" {
                node.attr("alt").unwrap_or("Enviar").to_string()
            } else {
                "Enviar".into()
            }
        }
        FieldKind::Select => {
            let mut selected = None;
            let mut stack: Vec<usize> = node.children.iter().rev().copied().collect();
            while let Some(c) = stack.pop() {
                let o = &dom.nodes[c];
                if o.name() == "optgroup" {
                    stack.extend(o.children.iter().rev());
                    continue;
                }
                if o.name() != "option" {
                    continue;
                }
                let label = inner_text(dom, c);
                let v = o.attr("value").map_or(label.clone(), String::from);
                if o.attr("selected").is_some() || selected.is_none() {
                    selected = Some(v.clone());
                }
                options.push((v, label));
            }
            value = selected.unwrap_or_default();
        }
        FieldKind::TextArea => value = inner_text(dom, n),
        _ => {}
    }
    Some(Field {
        kind,
        form,
        name: node.attr("name").unwrap_or("").to_string(),
        value,
        checked: node.attr("checked").is_some(),
        options,
        placeholder: node
            .attr("placeholder")
            .or_else(|| node.attr("aria-label"))
            .or_else(|| node.attr("title"))
            .unwrap_or("")
            .to_string(),
        node: n,
    })
}

/// ¿La página es un "cascarón" que arma JavaScript? (poco texto visible y muchos `<script>`).
fn needs_js(dom: &Dom, styled: &Styled) -> bool {
    let mut visible = 0usize;
    for (i, n) in dom.nodes.iter().enumerate() {
        if let NodeKind::Text(t) = &n.kind
            && n.parent.is_some_and(|p| rendered(styled, p))
        {
            visible += t.split_whitespace().map(str::len).sum::<usize>();
        }
        let _ = i;
    }
    visible < 120 && dom.scripts >= 3
}

/// Texto plano → página (un `<pre>`).
pub fn plain_html(text: &str) -> String {
    let esc = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\t', "    ")
        .replace('\r', "");
    format!("<body style='margin:16px'><pre style='white-space:pre-wrap'>{esc}</pre></body>")
}

/// Atributos de una etiqueta: `href="x" alt='y' checked`.
pub fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        if start == i {
            i += 1;
            continue;
        }
        let name = s[start..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                let vs = i + 1;
                i = vs;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                value = decode(&s[vs..i.min(b.len())], false);
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = decode(&s[vs..i], false);
            }
        }
        out.push((name, value));
    }
    out
}

/// `&amp;` → `&`, `&#241;` → `ñ`, etc. Los signos tipográficos que la fuente de la interfaz no
/// tiene se pasan a su versión simple (— → -, “ ” → "): para títulos, la terminal, etc.
pub fn decode_entities(s: &str) -> String {
    decode(s, true)
}

/// Como [`decode_entities`], pero sin simplificar: el texto de las páginas se dibuja con una
/// fuente que sí tiene rayas, comillas, flechas y demás.
pub fn decode_text(s: &str) -> String {
    decode(s, false)
}

fn decode(s: &str, fold: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let push = |out: &mut String, c: char| {
        if fold {
            push_folded(out, c)
        } else if !matches!(
            c,
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{AD}'
        ) {
            out.push(c)
        }
    };
    while let Some(amp) = rest.find('&') {
        for c in rest[..amp].chars() {
            push(&mut out, c);
        }
        rest = &rest[amp..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|e| e + 1)
            .unwrap_or(rest.len());
        let name = &rest[1..end];
        let decoded: Option<char> = if let Some(num) = name.strip_prefix('#') {
            let n = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse().ok(),
            };
            n.and_then(char::from_u32)
        } else {
            named_entity(name)
        };
        match decoded {
            Some(c) if !name.is_empty() => {
                push(&mut out, c);
                rest = &rest[end..];
                if rest.starts_with(';') {
                    rest = &rest[1..];
                }
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    for c in rest.chars() {
        push(&mut out, c);
    }
    out
}

fn push_folded(out: &mut String, c: char) {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' => out.push('\''),
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => out.push('"'),
        '\u{2013}' | '\u{2014}' | '\u{2212}' | '\u{2010}' => out.push('-'),
        '\u{2026}' => out.push_str("..."),
        '\u{2022}' | '\u{25CF}' | '\u{25AA}' => out.push('·'),
        '\u{2192}' => out.push_str("->"),
        '\u{2190}' => out.push_str("<-"),
        '\u{203A}' | '\u{276F}' => out.push('>'),
        '\u{2039}' | '\u{276E}' => out.push('<'),
        '\u{2713}' | '\u{2714}' => out.push('v'),
        '\u{2715}' | '\u{2716}' => out.push('x'),
        '\u{00A0}' | '\u{2009}' | '\u{200A}' | '\u{2002}' | '\u{2003}' => out.push(' '),
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{AD}' => {}
        '\u{20AC}' => out.push_str("EUR"),
        '\u{2122}' => out.push_str("(TM)"),
        c => out.push(c),
    }
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" | "AMP" => '&',
        "lt" | "LT" => '<',
        "gt" | "GT" => '>',
        "quot" | "QUOT" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "shy" => '\u{AD}',
        "zwj" => '\u{200D}',
        "zwnj" => '\u{200C}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "middot" => '·',
        "bull" => '•',
        "laquo" => '«',
        "raquo" => '»',
        "lsaquo" => '‹',
        "rsaquo" => '›',
        "iexcl" => '¡',
        "iquest" => '¿',
        "ordf" => 'ª',
        "ordm" => 'º',
        "sect" => '§',
        "para" => '¶',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "minus" => '−',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "sup1" => '¹',
        "sup2" => '²',
        "sup3" => '³',
        "micro" => 'µ',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "harr" => '↔',
        "check" => '✓',
        "star" => '☆',
        "hearts" => '♥',
        "aacute" => 'á',
        "eacute" => 'é',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "Aacute" => 'Á',
        "Eacute" => 'É',
        "Iacute" => 'Í',
        "Oacute" => 'Ó',
        "Uacute" => 'Ú',
        "ntilde" => 'ñ',
        "Ntilde" => 'Ñ',
        "uuml" => 'ü',
        "Uuml" => 'Ü',
        "ccedil" => 'ç',
        "Ccedil" => 'Ç',
        "agrave" => 'à',
        "egrave" => 'è',
        "igrave" => 'ì',
        "ograve" => 'ò',
        "ugrave" => 'ù',
        "acirc" => 'â',
        "ecirc" => 'ê',
        "ocirc" => 'ô',
        "atilde" => 'ã',
        "otilde" => 'õ',
        "ouml" => 'ö',
        "auml" => 'ä',
        "euml" => 'ë',
        "iuml" => 'ï',
        "Ouml" => 'Ö',
        "Auml" => 'Ä',
        "szlig" => 'ß',
        "oslash" => 'ø',
        "aring" => 'å',
        "aelig" => 'æ',
        _ => return None,
    })
}

/// Bytes de la página → texto. Casi todo es UTF-8; si no lo es, se asume Latin-1 (cada byte es
/// un carácter), que era lo común en páginas viejas en castellano.
pub fn decode_bytes(b: &[u8]) -> String {
    match core::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// El color de fondo de la página (el de `<html>` o, si no tiene, el de `<body>`).
pub fn canvas_color(p: &Prepared) -> Option<Color> {
    let mut body_bg = None;
    for (i, n) in p.dom.nodes.iter().enumerate() {
        match n.name() {
            "html" => {
                if let Some(bg) = p.styled.get(i).and_then(|s| s.bg).filter(|c| c.a > 128) {
                    return Some(bg.c);
                }
            }
            "body" => {
                body_bg = p.styled.get(i).and_then(|s| s.bg).filter(|c| c.a > 128);
                break;
            }
            _ => {}
        }
    }
    body_bg.map(|c| c.c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(html: &str) -> Prepared {
        prepare(
            html,
            Options {
                reader: false,
                images: true,
            },
            &[],
            Media::default(),
        )
    }

    #[test]
    fn entidades_y_tipografia() {
        assert_eq!(decode_entities("a &lt;b&gt; &copy; &eacute;"), "a <b> © é");
        assert_eq!(decode_entities("“Hola” — dijo…"), "\"Hola\" - dijo...");
        assert_eq!(decode_text("“Hola” — dijo&hellip;"), "“Hola” — dijo…");
        assert_eq!(decode_entities("AT&T &unknown; &"), "AT&T &unknown; &");
        assert_eq!(decode_bytes(&[0x63, 0xF3, 0x6D, 0x6F]), "cómo");
    }

    #[test]
    fn formularios_enlaces_e_imagenes() {
        let p = styled(
            "<title>T</title><form action=/search><input type=hidden name=hl value=es><input name=q size=40>\
             <input type=submit name=btnG value='Buscar'><button>Suerte</button>\
             <select name=s><option value=a>A<option value=b selected>B</select></form>\
             <a href=/x>ir <b>ya</b></a><img src=logo.png alt=Logo width=272>\
             <div style='display:none'><img src=oculta.png><a href=/no>no</a></div>\
             <div style=\"background:url('fondo.png')\">f</div>",
        );
        let d = &p.doc;
        assert_eq!(d.title, "T");
        assert_eq!(
            d.forms,
            [Form {
                action: "/search".into(),
                post: false
            }]
        );
        let kinds: Vec<FieldKind> = d.fields.iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [
                FieldKind::Hidden,
                FieldKind::Text,
                FieldKind::Submit,
                FieldKind::Submit,
                FieldKind::Select
            ]
        );
        assert_eq!(d.fields[3].value, "Suerte");
        assert_eq!(d.fields[4].value, "b");
        assert_eq!(d.links, ["/x"], "el enlace oculto no cuenta");
        let srcs: Vec<(&str, bool)> = d
            .images
            .iter()
            .map(|i| (i.src.as_str(), i.background))
            .collect();
        assert_eq!(srcs, [("logo.png", false), ("fondo.png", true)]);
        // El texto de adentro del enlace sabe que es parte del enlace.
        let b = (0..p.dom.nodes.len())
            .find(|&i| p.dom.nodes[i].name() == "b")
            .unwrap();
        assert_eq!(p.info.link_of(b), Some(0));
        assert_eq!(p.info.link_of(p.dom.nodes[b].children[0]), Some(0));
    }

    #[test]
    fn modo_lectura_sin_formularios_ni_menus() {
        let p = prepare(
            "<nav><a href=/menu>menú</a></nav><main><p>Hola <a href=/x>x</a></p></main>\
             <form><input name=q></form>",
            Options::READER,
            &[],
            Media::default(),
        );
        assert!(p.doc.fields.is_empty());
        assert_eq!(p.doc.links, ["/x"]);
    }

    #[test]
    fn detecta_paginas_que_necesitan_javascript() {
        let p = styled(
            "<body><div id=app></div><script>1</script><script>2</script><script>3</script>\
             <noscript>Activá JavaScript</noscript></body>",
        );
        assert!(p.doc.needs_js);
        let q = styled(&format!(
            "<p>{}</p><script></script>",
            "palabra ".repeat(100)
        ));
        assert!(!q.doc.needs_js);
    }
}

impl Options {
    pub const READER: Options = Options {
        reader: true,
        images: false,
    };
    pub const STYLED: Options = Options {
        reader: false,
        images: true,
    };
}
