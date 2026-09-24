//! HTML → árbol de elementos (DOM), con las reglas de cierre implícito que usan las páginas
//! reales: un `<p>` se cierra al empezar un bloque, un `<li>` al empezar el siguiente, `<td>` y
//! `<tr>` igual, las etiquetas de cierre sueltas se ignoran, etc. No es el algoritmo completo
//! del estándar HTML5, pero arma árboles razonables con HTML roto.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::html::{attrs, decode_entities, decode_text};

/// Elementos sin contenido.
pub const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr", "param",
];

/// Elementos cuyo contenido es texto crudo (no se interpreta como HTML).
const RAW: [&str; 7] = [
    "script", "style", "template", "noscript", "svg", "textarea", "title",
];

/// Estos cierran un `<p>` abierto (en HTML, un párrafo no puede contener bloques).
const CLOSES_P: [&str; 24] = [
    "p",
    "div",
    "ul",
    "ol",
    "dl",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "pre",
    "blockquote",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "form",
    "hr",
    "main",
    "figure",
];

const MAX_NODES: usize = 150_000;
const MAX_DEPTH: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Element {
        name: String,
        attrs: Vec<(String, String)>,
    },
    Text(String),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Dom {
    pub nodes: Vec<Node>,
    pub title: String,
    /// Contenido de los `<style>` (en orden).
    pub styles: Vec<String>,
    /// `href` de los `<link rel="stylesheet">`.
    pub stylesheet_links: Vec<String>,
    /// `<base href>`.
    pub base: Option<String>,
    /// Cuántos `<script>` tiene (para avisar si la página depende de JavaScript).
    pub scripts: usize,
}

impl Node {
    pub fn name(&self) -> &str {
        match &self.kind {
            NodeKind::Element { name, .. } => name,
            _ => "",
        }
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        match &self.kind {
            NodeKind::Element { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
}

fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

struct Builder {
    dom: Dom,
    /// Elementos abiertos (índices en `nodes`), del de afuera al de adentro.
    open: Vec<usize>,
}

impl Builder {
    fn current(&self) -> usize {
        *self.open.last().unwrap_or(&0)
    }

    fn add(&mut self, kind: NodeKind) -> Option<usize> {
        if self.dom.nodes.len() >= MAX_NODES {
            return None;
        }
        let parent = self.current();
        let id = self.dom.nodes.len();
        self.dom.nodes.push(Node {
            kind,
            children: Vec::new(),
            parent: Some(parent),
        });
        self.dom.nodes[parent].children.push(id);
        Some(id)
    }

    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        let cur = self.current();
        if let Some(&last) = self.dom.nodes[cur].children.last()
            && let NodeKind::Text(prev) = &mut self.dom.nodes[last].kind
        {
            prev.push_str(t);
            return;
        }
        self.add(NodeKind::Text(t.to_string()));
    }

    fn open_names(&self) -> impl Iterator<Item = (usize, &str)> + '_ {
        self.open
            .iter()
            .enumerate()
            .rev()
            .map(|(i, &n)| (i, self.dom.nodes[n].name()))
    }

    /// Cierra hasta el elemento `name` (inclusive) si está abierto antes de toparse con uno de
    /// `stop`.
    fn close_until(&mut self, name: &[&str], stop: &[&str]) {
        let mut found = None;
        for (i, n) in self.open_names() {
            if name.contains(&n) {
                found = Some(i);
                break;
            }
            if stop.contains(&n) {
                return;
            }
        }
        if let Some(i) = found {
            self.open.truncate(i);
        }
    }

    fn start(&mut self, name: &str, attrs: Vec<(String, String)>, self_closing: bool) {
        if CLOSES_P.contains(&name) {
            self.close_until(
                &["p"],
                &["div", "td", "th", "li", "body", "table", "button"],
            );
        }
        match name {
            "li" => self.close_until(&["li"], &["ul", "ol", "menu"]),
            "dt" | "dd" => self.close_until(&["dt", "dd"], &["dl"]),
            "tr" => self.close_until(&["tr"], &["table", "tbody", "thead", "tfoot"]),
            "td" | "th" => self.close_until(&["td", "th"], &["tr", "table"]),
            "tbody" | "thead" | "tfoot" => {
                self.close_until(&["tbody", "thead", "tfoot"], &["table"])
            }
            "option" => self.close_until(&["option"], &["select", "datalist"]),
            "a" => self.close_until(&["a"], &["div", "p", "li", "td", "body"]),
            _ => {}
        }
        if name == "link" {
            let rel = attrs
                .iter()
                .find(|(k, _)| k == "rel")
                .map(|(_, v)| v.to_ascii_lowercase());
            let media_ok = attrs
                .iter()
                .find(|(k, _)| k == "media")
                .is_none_or(|(_, v)| !v.contains("print"));
            if rel.is_some_and(|r| r.split_whitespace().any(|t| t == "stylesheet"))
                && media_ok
                && let Some((_, href)) = attrs.iter().find(|(k, _)| k == "href")
            {
                self.dom.stylesheet_links.push(href.clone());
            }
        }
        if name == "base"
            && let Some((_, href)) = attrs.iter().find(|(k, _)| k == "href")
        {
            self.dom.base = Some(href.clone());
        }
        let Some(id) = self.add(NodeKind::Element {
            name: name.to_string(),
            attrs,
        }) else {
            return;
        };
        if !(self_closing || VOID.contains(&name)) && self.open.len() < MAX_DEPTH {
            self.open.push(id);
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            // `</p>` sin `<p>` abierto: en HTML crea un párrafo vacío. Da igual.
            "p" => self.close_until(&["p"], &["div", "td", "th", "li", "body", "table"]),
            "li" => self.close_until(&["li"], &["ul", "ol"]),
            "td" | "th" | "tr" => self.close_until(&[name], &["table"]),
            _ => {
                let found = self.open_names().find(|(_, n)| *n == name).map(|(i, _)| i);
                if let Some(i) = found {
                    self.open.truncate(i);
                }
            }
        }
    }
}

pub fn parse(html: &str) -> Dom {
    let mut dom = parse_raw(html);
    wrap_in_html(&mut dom);
    dom
}

/// Como hacen los navegadores, todo documento tiene un `<html>` (aunque la página no lo
/// escriba): así valen las reglas de `:root` y `html`.
fn wrap_in_html(dom: &mut Dom) {
    let has_html = dom.nodes[0]
        .children
        .iter()
        .any(|&c| dom.nodes[c].name() == "html");
    if has_html || dom.nodes[0].children.is_empty() {
        return;
    }
    let id = dom.nodes.len();
    let kids = core::mem::take(&mut dom.nodes[0].children);
    for &k in &kids {
        dom.nodes[k].parent = Some(id);
    }
    dom.nodes.push(Node {
        kind: NodeKind::Element {
            name: "html".into(),
            attrs: Vec::new(),
        },
        children: kids,
        parent: Some(0),
    });
    dom.nodes[0].children.push(id);
}

fn parse_raw(html: &str) -> Dom {
    let mut b = Builder {
        dom: Dom {
            nodes: alloc::vec![Node {
                kind: NodeKind::Document,
                children: Vec::new(),
                parent: None,
            }],
            ..Default::default()
        },
        open: Vec::new(),
    };
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |e| i + e);
            b.text(&decode_text(&html[i..end]));
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
            b.text("<");
            i += 1;
            continue;
        }
        let close = close.unwrap_or(0);
        let inner = &html[i + 1..i + close];
        i += close + 1;
        if inner.starts_with('!') || inner.starts_with('?') {
            continue;
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
            b.text(&format!("<{inner}>"));
            continue;
        }
        if end_tag {
            b.end(&name);
            continue;
        }
        let a = attrs(&inner[name_end..]);
        if RAW.contains(&name.as_str()) {
            let close_at = find_ci(html, &format!("</{name}"), i).unwrap_or(html.len());
            let raw = &html[i..close_at];
            match name.as_str() {
                "title" => {
                    if b.dom.title.is_empty() {
                        b.dom.title = decode_entities(raw)
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                    }
                }
                "style" => {
                    let media_ok = a
                        .iter()
                        .find(|(k, _)| k == "media")
                        .is_none_or(|(_, v)| !v.contains("print"));
                    if media_ok {
                        b.dom.styles.push(raw.to_string());
                    }
                }
                "textarea" => {
                    b.start(&name, a, false);
                    b.text(&decode_text(raw));
                    b.end(&name);
                }
                // Un dibujo vectorial: no lo dibujamos, pero ocupa su lugar (su tamaño).
                "svg" => b.start(&name, a, true),
                "script" => b.dom.scripts += 1,
                "noscript" => {
                    // Sin JavaScript, lo de <noscript> es justamente lo que hay que mostrar.
                    // Se interpreta como HTML (salvo los meta refresh que mandan a otra página).
                    if !raw.to_ascii_lowercase().contains("http-equiv") {
                        let sub = parse_raw(raw);
                        b.start("div", a, false);
                        graft(&mut b, &sub, 0);
                        b.end("div");
                    }
                }
                _ => {} // template: no se muestra
            }
            i = html[close_at..]
                .find('>')
                .map_or(html.len(), |e| close_at + e + 1);
            continue;
        }
        let self_closing = inner.ends_with('/');
        b.start(&name, a, self_closing);
    }
    b.dom
}

/// Copia los hijos del nodo `at` de otro árbol en la posición actual.
fn graft(b: &mut Builder, sub: &Dom, at: usize) {
    for &c in &sub.nodes[at].children {
        match &sub.nodes[c].kind {
            NodeKind::Text(t) => b.text(t),
            NodeKind::Element { name, attrs } => {
                let name = name.clone();
                b.start(&name, attrs.clone(), false);
                graft(b, sub, c);
                if !VOID.contains(&name.as_str()) {
                    b.end(&name);
                }
            }
            NodeKind::Document => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(d: &Dom, n: usize, out: &mut String) {
        match &d.nodes[n].kind {
            NodeKind::Element { name, .. } => {
                out.push_str(&format!("<{name}>"));
                for &c in &d.nodes[n].children {
                    outline(d, c, out);
                }
                out.push_str(&format!("</{name}>"));
            }
            NodeKind::Text(t) => out.push_str(t.trim()),
            NodeKind::Document => {
                for &c in &d.nodes[n].children {
                    outline(d, c, out);
                }
            }
        }
    }

    fn tree(html: &str) -> String {
        let d = parse_raw(html);
        let mut s = String::new();
        outline(&d, 0, &mut s);
        s
    }

    #[test]
    fn cierres_implicitos() {
        assert_eq!(tree("<p>uno<p>dos"), "<p>uno</p><p>dos</p>");
        assert_eq!(tree("<ul><li>a<li>b</ul>"), "<ul><li>a</li><li>b</li></ul>");
        assert_eq!(
            tree("<table><tr><td>1<td>2<tr><td>3</table>"),
            "<table><tr><td>1</td><td>2</td></tr><tr><td>3</td></tr></table>"
        );
        assert_eq!(tree("<p>a<div>b</div>"), "<p>a</p><div>b</div>");
        assert_eq!(tree("<b>x</i>y</b>"), "<b>xy</b>");
        assert_eq!(tree("a<br>b<img src=x>"), "a<br></br>b<img></img>");
    }

    #[test]
    fn titulo_estilos_y_enlaces_css() {
        let d = parse(
            "<html><head><title> Hola </title><style>p{color:red}</style>\
             <style media=print>x{}</style><link rel=stylesheet href=/a.css>\
             <script>no <b>esto</b></script></head><body><noscript><p>sin js</p></noscript></body>",
        );
        assert_eq!(d.title, "Hola");
        assert_eq!(d.styles, ["p{color:red}"]);
        assert_eq!(d.stylesheet_links, ["/a.css"]);
        let mut s = String::new();
        outline(&d, 0, &mut s);
        assert!(s.contains("<p>sin js</p>"), "{s}");
        assert!(!s.contains("esto"));
    }
}
