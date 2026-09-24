//! HTML → documento de texto con enlaces.
//!
//! No es un motor de navegador completo (no hay CSS ni JavaScript): se queda con la estructura
//! que importa para leer — títulos, párrafos, listas, citas, código y enlaces — como hacen los
//! navegadores de texto (Lynx, w3m). Muchas páginas se leen bien así; las que dependen de
//! JavaScript para mostrar su contenido, no.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Text,
    /// h1..h6
    Heading(u8),
    /// Elemento de una lista (el guion o número ya está en el texto).
    Item,
    /// Texto preformateado (código): se respetan espacios y saltos de línea.
    Pre,
    Quote,
    /// Línea horizontal (`<hr>`).
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// Índice en [`Document::links`].
    pub link: Option<usize>,
    pub bold: bool,
    /// Texto secundario (el `alt` de una imagen, un campo de formulario).
    pub dim: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Sangría (listas anidadas, citas).
    pub indent: u8,
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub title: String,
    pub blocks: Vec<Block>,
    /// Los `href` tal como vienen en la página (se resuelven contra la dirección al hacer clic).
    pub links: Vec<String>,
}

/// Elementos cuyo contenido no se muestra.
const SKIP: [&str; 8] = [
    "script", "style", "template", "svg", "iframe", "noembed", "object", "canvas",
];

/// Elementos que empiezan y terminan un bloque.
const BLOCKS: [&str; 22] = [
    "p",
    "div",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "main",
    "aside",
    "form",
    "table",
    "tr",
    "dl",
    "dt",
    "dd",
    "figure",
    "figcaption",
    "center",
    "address",
    "details",
    "summary",
    "fieldset",
];

/// Elementos sin contenido (no tienen etiqueta de cierre).
const VOID: [&str; 13] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

/// Palabras de clases e ids que marcan partes que no son el contenido.
const NOISE: [&str; 21] = [
    "nav",
    "navbar",
    "navbox",
    "navigation",
    "menu",
    "dropdown",
    "breadcrumb",
    "breadcrumbs",
    "toc",
    "portlet",
    "noprint",
    "interlanguage",
    "jump",
    "skip",
    "cookie",
    "cookies",
    "banner",
    "share",
    "social",
    "advert",
    "ads",
];

/// "Modo lectura": lo que no es el contenido (menús, cabeceras y pies del sitio, formularios,
/// cosas ocultas) no se muestra. Las cabeceras de un `<article>` o `<main>` sí (tienen el título).
fn hidden(name: &str, a: &[(String, String)], in_main: bool) -> bool {
    // Los contenedores de toda la página nunca (en Wikipedia, `<body>` tiene la clase
    // "vector-toc-available", y ocultarlo dejaba la página vacía).
    if matches!(name, "html" | "body" | "main" | "article") {
        return false;
    }
    let chrome = matches!(
        name,
        "nav" | "aside" | "form" | "button" | "select" | "dialog"
    ) || (!in_main && matches!(name, "header" | "footer"));
    let role = attr(a, "role").unwrap_or("");
    let style: String = attr(a, "style")
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // Clases e ids que en casi todos los sitios son menús, índices o avisos.
    let noise = |v: &str| {
        v.split(|c: char| c.is_whitespace() || c == '-' || c == '_')
            .any(|t| {
                let t = t.to_ascii_lowercase();
                NOISE.contains(&t.as_str())
                    || (!in_main && matches!(t.as_str(), "header" | "footer" | "sidebar"))
            })
    };
    chrome
        || noise(attr(a, "class").unwrap_or(""))
        || noise(attr(a, "id").unwrap_or(""))
        || attr(a, "hidden").is_some()
        || attr(a, "aria-hidden") == Some("true")
        || matches!(role, "navigation" | "banner" | "contentinfo" | "search")
        || style.contains("display:none")
}

struct Builder {
    doc: Document,
    current: Block,
    bold: u32,
    link: Option<usize>,
    pre: u32,
    quote: u8,
    /// (ordenada, número del próximo elemento)
    lists: Vec<(bool, u32)>,
}

impl Builder {
    fn indent(&self) -> u8 {
        (self.lists.len() as u8).saturating_add(self.quote)
    }

    fn flush(&mut self) {
        let kind = if self.pre > 0 {
            BlockKind::Pre
        } else {
            BlockKind::Text
        };
        self.flush_as(kind);
    }

    /// Cierra el bloque actual (si tiene algo) y empieza uno de tipo `next`.
    fn flush_as(&mut self, next: BlockKind) {
        let has_text = self.current.kind == BlockKind::Rule
            || self.current.spans.iter().any(|s| !s.text.trim().is_empty());
        if has_text {
            // Sin espacios colgando al final.
            if let Some(last) = self.current.spans.last_mut() {
                let trimmed = last.text.trim_end_matches(' ').len();
                last.text.truncate(trimmed);
            }
            let next_block = Block {
                kind: next,
                indent: self.indent(),
                spans: Vec::new(),
            };
            let done = core::mem::replace(&mut self.current, next_block);
            self.doc.blocks.push(done);
        } else {
            self.current = Block {
                kind: next,
                indent: self.indent(),
                spans: Vec::new(),
            };
        }
        if self.quote > 0 && next == BlockKind::Text {
            self.current.kind = BlockKind::Quote;
        }
    }

    fn push_text(&mut self, text: &str, dim: bool) {
        let bold = self.bold > 0 || matches!(self.current.kind, BlockKind::Heading(_));
        let mut text = text.to_string();
        if self.pre == 0 {
            // Colapsa los espacios como hace HTML.
            let mut out = String::with_capacity(text.len());
            let mut space = self
                .current
                .spans
                .last()
                .is_none_or(|s| s.text.ends_with(' ') || s.text.ends_with('\n'));
            for c in text.chars() {
                if c.is_whitespace() {
                    if !space {
                        out.push(' ');
                        space = true;
                    }
                } else {
                    out.push(c);
                    space = false;
                }
            }
            text = out;
        }
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.current.spans.last_mut()
            && last.link == self.link
            && last.bold == bold
            && last.dim == dim
        {
            last.text.push_str(&text);
            return;
        }
        self.current.spans.push(Span {
            text,
            link: self.link,
            bold,
            dim,
        });
    }
}

fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    (from..h.len().saturating_sub(n.len() - 1)).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Atributos de una etiqueta: `href="x" alt='y' checked`.
fn attrs(s: &str) -> Vec<(String, String)> {
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
                value = decode_entities(&s[vs..i.min(b.len())]);
                i += 1;
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                value = decode_entities(&s[vs..i]);
            }
        }
        out.push((name, value));
    }
    out
}

fn attr<'a>(list: &'a [(String, String)], name: &str) -> Option<&'a str> {
    list.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

pub fn parse(html: &str) -> Document {
    let mut b = Builder {
        doc: Document::default(),
        current: Block {
            kind: BlockKind::Text,
            indent: 0,
            spans: Vec::new(),
        },
        bold: 0,
        link: None,
        pre: 0,
        quote: 0,
        lists: Vec::new(),
    };
    let bytes = html.as_bytes();
    let mut i = 0;
    // Elemento que se está salteando (nombre y cuántos del mismo nombre hay abiertos adentro).
    let mut hide: Option<(String, u32)> = None;
    let mut in_main = 0u32;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |e| i + e);
            if hide.is_none() {
                b.push_text(&decode_entities(&html[i..end]), false);
            }
            i = end;
            continue;
        }
        if html[i..].starts_with("<!--") {
            i = html[i..].find("-->").map_or(html.len(), |e| i + e + 3);
            continue;
        }
        let next = bytes.get(i + 1).copied().unwrap_or(b' ');
        let close = html[i..].find('>');
        if !(next.is_ascii_alphabetic() || matches!(next, b'/' | b'!' | b'?')) || close.is_none() {
            // Un "<" suelto ("<3", "a < b") es texto.
            b.push_text("<", false);
            i += 1;
            continue;
        }
        let close = close.unwrap_or(0);
        let inner = &html[i + 1..i + close];
        i += close + 1;
        if inner.starts_with('!') || inner.starts_with('?') {
            continue; // <!DOCTYPE>, <?xml?>
        }
        let (end_tag, inner) = match inner.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, inner),
        };
        let name_end = inner
            .find(|c: char| c.is_ascii_whitespace() || c == '/')
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        if name.is_empty() || !name.as_bytes()[0].is_ascii_alphabetic() {
            b.push_text(&format!("<{inner}>"), false);
            continue;
        }
        if !end_tag && (SKIP.contains(&name.as_str()) || name == "title") {
            // Contenido crudo hasta el cierre.
            let close_at = find_ci(html, &format!("</{name}"), i).unwrap_or(html.len());
            if name == "title" && b.doc.title.is_empty() {
                b.doc.title = collapse(&decode_entities(&html[i..close_at]));
            }
            i = html[close_at..]
                .find('>')
                .map_or(html.len(), |e| close_at + e + 1);
            continue;
        }
        let a = if end_tag {
            Vec::new()
        } else {
            attrs(&inner[name_end..])
        };
        let void = VOID.contains(&name.as_str()) || inner.ends_with('/');
        if let Some((hidden_name, depth)) = &mut hide {
            if name == *hidden_name && !void {
                if end_tag {
                    *depth -= 1;
                    if *depth == 0 {
                        hide = None;
                    }
                } else {
                    *depth += 1;
                }
            }
            continue;
        }
        if matches!(name.as_str(), "main" | "article") {
            if end_tag {
                in_main = in_main.saturating_sub(1);
            } else {
                in_main += 1;
            }
        }
        if !end_tag && !void && hidden(&name, &a, in_main > 0) {
            hide = Some((name, 1));
            continue;
        }
        match (name.as_str(), end_tag) {
            (n, _) if BLOCKS.contains(&n) => b.flush(),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                let level = name.as_bytes()[1] - b'0';
                b.flush_as(BlockKind::Heading(level));
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => b.flush(),
            ("ul" | "ol" | "menu", false) => {
                b.flush();
                b.lists.push((name == "ol", 1));
                b.flush();
            }
            ("ul" | "ol" | "menu", true) => {
                b.flush();
                b.lists.pop();
                b.flush();
            }
            ("li", false) => {
                b.flush_as(BlockKind::Item);
                let marker = match b.lists.last_mut() {
                    Some((true, n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "- ".into(),
                };
                let saved = b.link.take();
                b.push_text(&marker, true);
                b.link = saved;
            }
            ("li", true) => b.flush(),
            ("pre", false) => {
                b.flush_as(BlockKind::Pre);
                b.pre += 1;
            }
            ("pre", true) => {
                b.pre = b.pre.saturating_sub(1);
                b.flush();
            }
            ("blockquote", false) => {
                b.quote += 1;
                b.flush();
            }
            ("blockquote", true) => {
                b.flush();
                b.quote = b.quote.saturating_sub(1);
                b.flush();
            }
            ("br", _) => {
                b.push_text("\n", false);
                if let Some(last) = b.current.spans.last_mut()
                    && !last.text.ends_with('\n')
                {
                    last.text.push('\n');
                }
            }
            ("hr", _) => {
                b.flush_as(BlockKind::Rule);
                b.current.spans.push(Span {
                    text: " ".into(),
                    link: None,
                    bold: false,
                    dim: true,
                });
                b.flush();
            }
            ("a", false) => {
                b.link = attr(&a, "href").map(|h| {
                    b.doc.links.push(h.to_string());
                    b.doc.links.len() - 1
                });
            }
            ("a", true) => b.link = None,
            ("b" | "strong", false) => b.bold += 1,
            ("b" | "strong", true) => b.bold = b.bold.saturating_sub(1),
            ("td" | "th", _) => b.push_text("  ", false),
            ("img", false) => {
                if let Some(alt) = attr(&a, "alt").filter(|s| !s.trim().is_empty()) {
                    b.push_text(&format!("[{}]", alt.trim()), true);
                }
            }
            ("input", false) => {
                let kind = attr(&a, "type").unwrap_or("text").to_ascii_lowercase();
                match kind.as_str() {
                    "hidden" | "checkbox" | "radio" => {}
                    "submit" | "button" => {
                        let v = attr(&a, "value").unwrap_or("Enviar").to_string();
                        b.push_text(&format!("[{v}]"), true);
                    }
                    _ => b.push_text("[________]", true),
                }
            }
            _ => {}
        }
    }
    b.flush();
    b.doc
}

/// Texto plano → un bloque preformateado.
pub fn plain(text: &str) -> Document {
    Document {
        title: String::new(),
        blocks: alloc::vec![Block {
            kind: BlockKind::Pre,
            indent: 0,
            spans: alloc::vec![Span {
                text: text.replace('\t', "    ").replace('\r', ""),
                link: None,
                bold: false,
                dim: false,
            }],
        }],
        links: Vec::new(),
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `&amp;` → `&`, `&#241;` → `ñ`, etc. Los signos tipográficos que la fuente no tiene se pasan
/// a su versión simple (— → -, “ ” → ").
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
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
                push_folded(&mut out, c);
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
        push_folded(&mut out, c);
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
        '\u{00A0}' | '\u{2009}' | '\u{200A}' | '\u{2002}' | '\u{2003}' => out.push(' '),
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => {}
        '\u{20AC}' => out.push_str("EUR"),
        '\u{2122}' => out.push_str("(TM)"),
        c => out.push(c),
    }
}

fn named_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "copy" => '©',
        "reg" => '®',
        "deg" => '°',
        "middot" => '·',
        "bull" => '·',
        "laquo" => '«',
        "raquo" => '»',
        "iexcl" => '¡',
        "iquest" => '¿',
        "ordf" => 'ª',
        "ordm" => 'º',
        "euro" => '€',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "times" => '×',
        "divide" => '÷',
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
        "agrave" => 'à',
        "egrave" => 'è',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
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

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(d: &Document) -> Vec<String> {
        d.blocks
            .iter()
            .map(|b| b.spans.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn estructura_basica() {
        let d = parse(
            "<!DOCTYPE html><html><head><title> Hola &amp; chau </title><style>p{}</style>\
             <script>alert('<p>no')</script></head><body><h1>Título</h1>\
             <p>Un   párrafo\ncon <a href=\"/x\">un enlace</a> y <b>negrita</b>.</p>\
             <ul><li>uno</li><li>dos<ol><li>a</li></ol></li></ul><hr><pre>  código\n  aquí</pre>\
             <!-- comentario --><p>fin&nbsp;&#241;&#x41;</p></body></html>",
        );
        assert_eq!(d.title, "Hola & chau");
        assert_eq!(
            texts(&d),
            [
                "Título",
                "Un párrafo con un enlace y negrita.",
                "- uno",
                "- dos",
                "1. a",
                "",
                "  código\n  aquí",
                "fin ñA"
            ]
        );
        assert_eq!(d.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(d.links, ["/x"]);
        let p = &d.blocks[1];
        assert!(
            p.spans
                .iter()
                .any(|s| s.link == Some(0) && s.text == "un enlace")
        );
        assert!(p.spans.iter().any(|s| s.bold && s.text == "negrita"));
        assert_eq!(d.blocks[4].indent, 2, "lista anidada");
        assert_eq!(d.blocks[5].kind, BlockKind::Rule);
        assert_eq!(d.blocks[6].kind, BlockKind::Pre);
    }

    #[test]
    fn entidades_y_tipografia() {
        assert_eq!(decode_entities("a &lt;b&gt; &copy; &eacute;"), "a <b> © é");
        assert_eq!(decode_entities("“Hola” — dijo…"), "\"Hola\" - dijo...");
        assert_eq!(decode_entities("AT&T &unknown; &"), "AT&T &unknown; &");
        assert_eq!(decode_bytes(&[0x63, 0xF3, 0x6D, 0x6F]), "cómo");
    }

    #[test]
    fn modo_lectura_saltea_menus_y_ocultos() {
        let d = parse(
            "<body class='vector-toc-available'><header><a href=/>Logo</a></header><nav><ul><li>Menú<nav>x</nav></li></ul></nav>             <div hidden><p>oculto</p></div><div style='display: none'>tampoco</div>             <main><article><header><h1>Título</h1></header><p>Contenido</p></article></main>             <form><input name=q><button>Buscar</button></form><footer>pie</footer><p>fin</p>             <div class='vector-dropdown'>idiomas</div><a class=mw-jump-link href=#c>saltar</a>             </body>",
        );
        assert_eq!(texts(&d), ["Título", "Contenido", "fin"]);
        assert!(d.links.is_empty(), "los enlaces del menú no cuentan");
    }

    #[test]
    fn html_roto_no_rompe() {
        for bad in [
            "<",
            "<p",
            "<a href='x>texto",
            "</>",
            "<<>>",
            "&#99999999;",
            "<script>sin fin",
        ] {
            let _ = parse(bad);
        }
        let d = parse("texto <3 corazón");
        assert_eq!(texts(&d), ["texto <3 corazón"]);
    }
}
