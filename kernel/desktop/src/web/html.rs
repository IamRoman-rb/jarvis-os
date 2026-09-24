//! HTML (+ CSS) → documento listo para armar en renglones.
//!
//! El HTML se convierte en un árbol ([`super::dom`]), se le aplican los estilos
//! ([`super::css`]: los `<style>`, las hojas externas que ya bajó el navegador y los
//! `style="…"`) y se "aplana" en bloques (párrafos, títulos, elementos de lista…) con trozos de
//! texto que llevan su color, tamaño y enlace, más imágenes y campos de formulario.
//!
//! No hay maquetación de CSS (cajas, flex, grillas, posiciones): los bloques van uno debajo del
//! otro. Como aproximación, los hijos de un contenedor `display: flex` (en fila) o flotantes van
//! en la misma línea, que es como se ven la mayoría de los menús.
//!
//! **Modo lectura** ([`Options::reader`]): sin estilos ni formularios, y sin lo que no es el
//! contenido (menús, cabeceras, pies, avisos), como hacen los navegadores de texto.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::Color;

use super::css::{self, ElemInfo, Sheet};
use super::dom::{self, Dom, NodeKind};

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// Índice en [`Document::links`].
    pub link: Option<usize>,
    pub bold: bool,
    /// Texto secundario (el `alt` de una imagen que no se carga, un marcador de lista).
    pub dim: bool,
    /// Color del texto (si la página lo define).
    pub color: Option<Color>,
    /// Fondo del texto (`<mark>`, botones…).
    pub bg: Option<Color>,
    /// Tamaño de letra en píxeles (16 = normal).
    pub size: u8,
    pub underline: bool,
    /// Un campo de formulario ([`Document::fields`]) en vez de texto.
    pub field: Option<usize>,
    /// Una imagen ([`Document::images`]) en vez de texto.
    pub image: Option<usize>,
}

impl Span {
    pub fn text(text: &str) -> Span {
        Span {
            text: text.into(),
            link: None,
            bold: false,
            dim: false,
            color: None,
            bg: None,
            size: 16,
            underline: false,
            field: None,
            image: None,
        }
    }

    fn same_style(&self, o: &Span) -> bool {
        self.link == o.link
            && self.bold == o.bold
            && self.dim == o.dim
            && self.color == o.color
            && self.bg == o.bg
            && self.size == o.size
            && self.underline == o.underline
            && self.field.is_none()
            && o.field.is_none()
            && self.image.is_none()
            && o.image.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Sangría (listas anidadas, citas).
    pub indent: u8,
    pub spans: Vec<Span>,
    pub align: Align,
    /// Fondo del bloque (una caja con `background`).
    pub bg: Option<Color>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Password,
    Hidden,
    Submit,
    /// `<button>` o `<input type=button>` (sin JavaScript no hacen nada salvo enviar).
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
    /// (valor, texto) de un `<select>`.
    pub options: Vec<(String, String)>,
    pub placeholder: String,
    /// Ancho en píxeles.
    pub width: i32,
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
    /// Tamaño pedido por la página (atributos o CSS), si lo dice.
    pub width: Option<i32>,
    pub height: Option<i32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub title: String,
    pub blocks: Vec<Block>,
    /// Los `href` tal como vienen en la página (se resuelven contra la dirección al hacer clic).
    pub links: Vec<String>,
    /// Fondo y color de texto de la página (de `<body>`).
    pub bg: Option<Color>,
    pub fg: Option<Color>,
    pub forms: Vec<Form>,
    pub fields: Vec<Field>,
    pub images: Vec<ImageRef>,
    /// Hojas de estilo externas (`<link rel=stylesheet>`), para que el navegador las baje.
    pub stylesheets: Vec<String>,
    pub base: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Modo lectura: sin estilos ni formularios, sin menús ni cabeceras.
    pub reader: bool,
    /// Aplicar los colores de la página (si no, se usan los del tema del navegador).
    pub colors: bool,
    /// Mostrar imágenes (si no, su texto alternativo).
    pub images: bool,
}

impl Options {
    pub const READER: Options = Options {
        reader: true,
        colors: false,
        images: false,
    };
    pub const STYLED: Options = Options {
        reader: false,
        colors: true,
        images: true,
    };
}

/// Elementos que empiezan y terminan un bloque.
const BLOCKS: [&str; 30] = [
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
    "legend",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "body",
];

/// Palabras de clases e ids que marcan partes que no son el contenido (modo lectura).
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
fn reader_hidden(name: &str, a: &dom::Node, in_main: bool) -> bool {
    // Los contenedores de toda la página nunca (en Wikipedia, `<body>` tiene la clase
    // "vector-toc-available", y ocultarlo dejaba la página vacía).
    if matches!(name, "html" | "body" | "main" | "article") {
        return false;
    }
    let chrome = matches!(
        name,
        "nav" | "aside" | "form" | "button" | "select" | "dialog" | "input" | "textarea"
    ) || (!in_main && matches!(name, "header" | "footer"));
    let role = a.attr("role").unwrap_or("");
    let style: String = a
        .attr("style")
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let noise = |v: &str| {
        v.split(|c: char| c.is_whitespace() || c == '-' || c == '_')
            .any(|t| {
                let t = t.to_ascii_lowercase();
                NOISE.contains(&t.as_str())
                    || (!in_main && matches!(t.as_str(), "header" | "footer" | "sidebar"))
            })
    };
    chrome
        || noise(a.attr("class").unwrap_or(""))
        || noise(a.attr("id").unwrap_or(""))
        || a.attr("aria-hidden") == Some("true")
        || matches!(role, "navigation" | "banner" | "contentinfo" | "search")
        || style.contains("display:none")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Display {
    Inline,
    Block,
    None,
    ListItem,
    /// Contenedor cuyos hijos van en fila (`display: flex`, `inline-flex`).
    Row,
    /// `inline-block`: va en la línea, y lo de adentro también (aunque sean bloques).
    InlineBlock,
}

/// Estilo de un elemento (lo que se hereda y lo que no).
#[derive(Clone, Debug)]
struct Style {
    display: Display,
    color: Option<Color>,
    /// Fondo del elemento (no se hereda: se pinta su caja).
    bg: Option<Color>,
    bold: bool,
    size: i32,
    align: Align,
    underline: bool,
    no_underline: bool,
    upper: bool,
    pre: bool,
    /// `list-style: none` (se hereda de la lista a sus elementos).
    no_marker: bool,
    /// `flex-direction: column`: un flex que no es en fila.
    column: bool,
    float: bool,
    width: Option<i32>,
    height: Option<i32>,
}

impl Style {
    fn root() -> Style {
        Style {
            display: Display::Block,
            color: None,
            bg: None,
            bold: false,
            size: 16,
            align: Align::Left,
            underline: false,
            no_underline: false,
            upper: false,
            pre: false,
            no_marker: false,
            column: false,
            float: false,
            width: None,
            height: None,
        }
    }

    /// Lo que un hijo hereda.
    fn inherit(&self) -> Style {
        Style {
            display: Display::Inline,
            bg: None,
            float: false,
            column: false,
            width: None,
            height: None,
            ..self.clone()
        }
    }
}

struct Flat<'a> {
    opts: Options,
    sheet: &'a Sheet,
    dom: &'a Dom,
    doc: Document,
    current: Block,
    /// Fondo de los bloques que se están armando (la caja con color más cercana).
    block_bg: Vec<Option<Color>>,
    link: Option<usize>,
    form: Option<usize>,
    pre: u32,
    quote: u8,
    /// (ordenada, número del próximo elemento)
    lists: Vec<(bool, u32)>,
    path: Vec<ElemInfo>,
    in_main: u32,
    /// Los hijos del elemento actual van en fila (es `display: flex` o `inline-block`).
    row: bool,
    align: Vec<Align>,
}

fn attr_len(v: Option<&str>) -> Option<i32> {
    let v = v?.trim().trim_end_matches("px");
    v.parse::<i32>().ok().filter(|n| *n > 0 && *n < 10_000)
}

impl Flat<'_> {
    fn indent(&self) -> u8 {
        (self.lists.len() as u8).saturating_add(self.quote)
    }

    fn new_block(&self, kind: BlockKind) -> Block {
        Block {
            kind,
            indent: self.indent(),
            spans: Vec::new(),
            align: *self.align.last().unwrap_or(&Align::Left),
            bg: self.block_bg.last().copied().flatten(),
        }
    }

    fn flush(&mut self) {
        let kind = if self.pre > 0 {
            BlockKind::Pre
        } else if self.quote > 0 {
            BlockKind::Quote
        } else {
            BlockKind::Text
        };
        self.flush_as(kind);
    }

    /// Cierra el bloque actual (si tiene algo) y empieza uno de tipo `next`.
    fn flush_as(&mut self, next: BlockKind) {
        let only_marker = self.current.kind == BlockKind::Item
            && self.current.spans.len() == 1
            && self.current.spans[0].dim
            && self.current.spans[0].link.is_none()
            && next != BlockKind::Item;
        if only_marker {
            return;
        }
        let has_content = self.current.kind == BlockKind::Rule
            || self
                .current
                .spans
                .iter()
                .any(|s| !s.text.trim().is_empty() || s.image.is_some() || s.field.is_some());
        let next_block = self.new_block(next);
        if has_content {
            if let Some(last) = self.current.spans.last_mut() {
                let trimmed = last.text.trim_end_matches(' ').len();
                last.text.truncate(trimmed);
            }
            let done = core::mem::replace(&mut self.current, next_block);
            self.doc.blocks.push(done);
        } else {
            self.current = next_block;
        }
    }

    fn push(&mut self, mut span: Span) {
        if self.pre == 0 && span.field.is_none() && span.image.is_none() {
            // Colapsa los espacios como hace HTML.
            let mut out = String::with_capacity(span.text.len());
            let mut space = self
                .current
                .spans
                .last()
                .is_none_or(|s| s.text.ends_with(' ') || s.text.ends_with('\n'));
            for c in span.text.chars() {
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
            span.text = out;
            if span.text.is_empty() {
                return;
            }
        }
        if let Some(last) = self.current.spans.last_mut()
            && last.same_style(&span)
            && span.field.is_none()
            && span.image.is_none()
        {
            last.text.push_str(&span.text);
            return;
        }
        self.current.spans.push(span);
    }

    fn text_span(&self, text: &str, st: &Style) -> Span {
        let heading = matches!(self.current.kind, BlockKind::Heading(_));
        Span {
            text: if st.upper {
                text.to_uppercase()
            } else {
                text.to_string()
            },
            link: self.link,
            bold: st.bold || heading,
            dim: false,
            color: if self.opts.colors { st.color } else { None },
            bg: if self.opts.colors { st.bg } else { None },
            size: st.size.clamp(8, 72) as u8,
            underline: (self.link.is_some() && !st.no_underline) || st.underline,
            field: None,
            image: None,
        }
    }

    /// Aplica las reglas CSS (y los atributos de estilo) al elemento `n`.
    fn style_of(&self, n: usize, name: &str, parent: &Style) -> Style {
        let node = &self.dom.nodes[n];
        let mut st = parent.inherit();
        // Estilos por defecto de cada etiqueta (lo que trae cualquier navegador).
        st.display = if BLOCKS.contains(&name)
            || matches!(
                name,
                "ul" | "ol" | "menu" | "pre" | "blockquote" | "hr" | "html"
            ) {
            Display::Block
        } else if name == "li" {
            Display::ListItem
        } else if matches!(
            name,
            "head" | "script" | "style" | "template" | "meta" | "link" | "title"
        ) {
            Display::None
        } else {
            Display::Inline
        };
        match name {
            "b" | "strong" | "th" => st.bold = true,
            "h1" => (st.size, st.bold) = (32, true),
            "h2" => (st.size, st.bold) = (24, true),
            "h3" => (st.size, st.bold) = (20, true),
            "h4" | "h5" | "h6" => st.bold = true,
            "small" | "sub" | "sup" => st.size = (st.size * 13 / 16).max(10),
            "big" => st.size = st.size * 5 / 4,
            "u" | "ins" => st.underline = true,
            "center" => st.align = Align::Center,
            "pre" | "textarea" => st.pre = true,
            _ => {}
        }
        if node.attr("hidden").is_some() {
            st.display = Display::None;
        }
        if !self.opts.reader {
            match node.attr("align").map(str::to_ascii_lowercase).as_deref() {
                Some("center" | "middle") if st.display != Display::Inline => {
                    st.align = Align::Center
                }
                Some("right") if st.display != Display::Inline => st.align = Align::Right,
                _ => {}
            }
            if let Some(c) = node.attr("bgcolor").and_then(css::parse_color) {
                st.bg = Some(c);
            }
            if name == "font"
                && let Some(c) = node.attr("color").and_then(css::parse_color)
            {
                st.color = Some(c);
            }
            if name == "body" {
                if let Some(c) = node.attr("text").and_then(css::parse_color) {
                    st.color = Some(c);
                }
                if let Some(c) = node.attr("link").and_then(css::parse_color) {
                    let _ = c; // el color de los enlaces lo da la regla `a`
                }
            }
            st.width = attr_len(node.attr("width"));
            st.height = attr_len(node.attr("height"));
            // Reglas de las hojas de estilo y el `style="…"` (que gana).
            let inline = node.attr("style").map(css::parse_decls).unwrap_or_default();
            let mut locals: BTreeMap<String, String> = BTreeMap::new();
            let matched = self.sheet.matching(&self.path);
            for d in matched.iter().copied().chain(inline.iter()) {
                if d.prop.starts_with("--") {
                    locals.insert(d.prop.clone(), d.value.clone());
                }
            }
            for d in matched.iter().copied().chain(inline.iter()) {
                let v = self.sheet.resolve(&d.value, &locals);
                self.apply(&mut st, &d.prop, v.trim(), parent);
            }
            if st.display == Display::Row && st.column {
                st.display = Display::Block;
            }
        }
        st
    }

    fn apply(&self, st: &mut Style, prop: &str, v: &str, parent: &Style) {
        let lower = v.to_ascii_lowercase();
        match prop {
            "color" => {
                if lower == "inherit" || lower == "currentcolor" {
                    st.color = parent.color;
                } else if let Some(c) = css::parse_color(v) {
                    st.color = Some(c);
                }
            }
            "background-color" | "background" => {
                if lower == "none" || lower == "transparent" {
                    st.bg = None;
                } else if let Some(c) = css::color_in(v) {
                    st.bg = Some(c);
                }
            }
            "font-weight" => {
                st.bold = match lower.as_str() {
                    "bold" | "bolder" => true,
                    "normal" | "lighter" => false,
                    n => n.parse::<u32>().map_or(st.bold, |w| w >= 600),
                }
            }
            "font-size" => {
                if let Some(px) = css::font_size(&lower, parent.size) {
                    st.size = px;
                }
            }
            "font" => {
                // `font: bold 13px/27px Arial`: se sacan el peso y el tamaño.
                for tok in lower.split_whitespace() {
                    if tok == "bold" || tok.parse::<u32>().is_ok_and(|w| w >= 600) {
                        st.bold = true;
                    }
                    let size = tok.split('/').next().unwrap_or(tok);
                    if (size.ends_with("px") || size.ends_with("em") || size.ends_with("pt"))
                        && let Some(px) = css::font_size(size, parent.size)
                    {
                        st.size = px;
                    }
                }
            }
            "text-align" => {
                st.align = match lower.as_str() {
                    "center" | "-webkit-center" => Align::Center,
                    "right" | "end" => Align::Right,
                    _ => Align::Left,
                }
            }
            "display" => {
                st.display = match lower.as_str() {
                    "none" => Display::None,
                    "block" | "table" | "table-row" | "grid" | "flow-root" | "table-caption"
                    | "inline-grid" => Display::Block,
                    "list-item" => Display::ListItem,
                    "flex" | "inline-flex" => Display::Row,
                    "inline-block" | "inline-table" => Display::InlineBlock,
                    "contents" | "inline" | "table-cell" => Display::Inline,
                    _ => st.display,
                }
            }
            "flex-direction" => st.column = lower.starts_with("column"),
            "visibility" if lower == "hidden" || lower == "collapse" => st.display = Display::None,
            "text-decoration" | "text-decoration-line" => {
                st.no_underline = lower.contains("none");
                st.underline = lower.contains("underline");
            }
            "text-transform" => st.upper = lower == "uppercase",
            "list-style" | "list-style-type" => st.no_marker = lower.starts_with("none"),
            "white-space" => st.pre = lower.starts_with("pre"),
            "float" => st.float = lower == "left" || lower == "right",
            "width" => {
                if let Some(px) = css::parse_length(&lower, st.size, 800) {
                    st.width = Some(px);
                }
            }
            "height" => {
                if let Some(px) = css::parse_length(&lower, st.size, 600) {
                    st.height = Some(px);
                }
            }
            // Cosas posicionadas fuera de la pantalla ("solo para lectores de pantalla").
            "clip" | "clip-path" if lower.contains("rect(0") || lower.contains("inset(50%") => {
                st.display = Display::None
            }
            _ => {}
        }
    }

    fn walk(&mut self, n: usize, parent: &Style) {
        let node = &self.dom.nodes[n];
        let (name, attrs) = match &node.kind {
            NodeKind::Text(t) => {
                if parent.display != Display::None {
                    let span = self.text_span(t, parent);
                    self.push(span);
                }
                return;
            }
            NodeKind::Document => {
                for &c in &node.children.clone() {
                    self.walk(c, parent);
                }
                return;
            }
            NodeKind::Element { name, attrs } => (name.as_str(), attrs),
        };
        if self.opts.reader && reader_hidden(name, node, self.in_main > 0) {
            return;
        }
        let classes: Vec<String> = node
            .attr("class")
            .unwrap_or("")
            .split_whitespace()
            .map(String::from)
            .collect();
        self.path.push(ElemInfo {
            tag: name.to_string(),
            id: node.attr("id").unwrap_or("").to_string(),
            classes,
        });
        let st = self.style_of(n, name, parent);
        if st.display == Display::None {
            self.path.pop();
            return;
        }
        if (name == "body" || name == "html") && self.opts.colors {
            if let Some(bg) = st.bg {
                self.doc.bg = Some(bg);
            }
            if let Some(fg) = st.color {
                self.doc.fg = Some(fg);
            }
        }
        let is_main = matches!(name, "main" | "article");
        if is_main {
            self.in_main += 1;
        }
        // En una fila (flex) o flotando, los bloques se ponen uno al lado del otro.
        let in_row = self.row || st.float;
        let block = !in_row
            && matches!(
                st.display,
                Display::Block | Display::ListItem | Display::Row
            );
        let own_bg = st
            .bg
            .filter(|_| self.opts.colors && block && !matches!(name, "body" | "html"));
        if block {
            let kind = match name {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    BlockKind::Heading(name.as_bytes()[1] - b'0')
                }
                "pre" => BlockKind::Pre,
                _ if st.display == Display::ListItem => BlockKind::Item,
                _ => {
                    if self.pre > 0 {
                        BlockKind::Pre
                    } else if self.quote > 0 {
                        BlockKind::Quote
                    } else {
                        BlockKind::Text
                    }
                }
            };
            if own_bg.is_some() {
                self.flush();
                self.block_bg.push(own_bg);
            }
            self.align.push(st.align);
            self.flush_as(kind);
        } else if in_row
            && matches!(
                st.display,
                Display::Block | Display::ListItem | Display::Row
            )
        {
            // Separador entre los elementos de una fila.
            if self
                .current
                .spans
                .last()
                .is_some_and(|s| !s.text.ends_with(' '))
            {
                let sep = self.text_span("   ", parent);
                self.current.spans.push(sep);
            }
        }
        // Solo los hijos directos de un flex van en fila; los de más adentro, normal.
        let saved_row = self.row;
        self.row = matches!(st.display, Display::Row | Display::InlineBlock);
        self.element(n, name, attrs, &st, block);
        self.row = saved_row;
        if block {
            self.flush();
            self.align.pop();
            if own_bg.is_some() {
                self.block_bg.pop();
                let next = self.new_block(self.current.kind);
                self.current = next;
            }
        }
        if is_main {
            self.in_main -= 1;
        }
        self.path.pop();
    }

    fn children(&mut self, n: usize, st: &Style) {
        for c in self.dom.nodes[n].children.clone() {
            self.walk(c, st);
        }
    }

    /// Lo propio de cada etiqueta (listas, enlaces, imágenes, formularios…).
    fn element(
        &mut self,
        n: usize,
        name: &str,
        _attrs: &[(String, String)],
        st: &Style,
        block: bool,
    ) {
        let node = &self.dom.nodes[n];
        match name {
            "ul" | "ol" | "menu" => {
                self.lists.push((name == "ol", 1));
                self.flush();
                self.children(n, st);
                self.flush();
                self.lists.pop();
            }
            "li" => {
                if block && !st.no_marker {
                    let marker = match self.lists.last_mut() {
                        Some((true, k)) => {
                            *k += 1;
                            format!("{}. ", *k - 1)
                        }
                        _ => "- ".into(),
                    };
                    let mut m = self.text_span(&marker, st);
                    m.link = None;
                    m.dim = true;
                    m.underline = false;
                    self.push(m);
                }
                self.children(n, st);
            }
            "pre" => {
                self.pre += 1;
                self.children(n, st);
                self.pre -= 1;
            }
            "blockquote" => {
                self.quote += 1;
                self.flush();
                self.children(n, st);
                self.flush();
                self.quote -= 1;
            }
            "br" => {
                let mut s = self.text_span("\n", st);
                s.text = "\n".into();
                if let Some(last) = self.current.spans.last_mut() {
                    last.text.push('\n');
                } else {
                    self.current.spans.push(s);
                }
            }
            "hr" => {
                self.flush_as(BlockKind::Rule);
                let mut s = Span::text(" ");
                s.dim = true;
                self.current.spans.push(s);
                self.flush();
            }
            "a" => {
                let saved = self.link;
                if let Some(last) = self.current.spans.last()
                    && last.link.is_some()
                    && !last.text.ends_with([' ', '\n'])
                {
                    let sp = self.text_span(" ", st);
                    let mut sp = sp;
                    sp.link = None;
                    sp.underline = false;
                    self.current.spans.push(sp);
                }
                if let Some(h) = node.attr("href") {
                    self.doc.links.push(h.to_string());
                    self.link = Some(self.doc.links.len() - 1);
                }
                self.children(n, st);
                self.link = saved;
            }
            "td" | "th" => {
                if self
                    .current
                    .spans
                    .last()
                    .is_some_and(|s| !s.text.ends_with(' '))
                {
                    let sp = self.text_span("  ", st);
                    self.current.spans.push(sp);
                }
                self.children(n, st);
            }
            "img" => self.image(n, st),
            "form" => {
                let saved = self.form;
                if !self.opts.reader {
                    self.doc.forms.push(Form {
                        action: node.attr("action").unwrap_or("").to_string(),
                        post: node
                            .attr("method")
                            .is_some_and(|m| m.eq_ignore_ascii_case("post")),
                    });
                    self.form = Some(self.doc.forms.len() - 1);
                }
                self.children(n, st);
                self.form = saved;
            }
            "input" | "button" | "select" | "textarea" => self.field(n, name, st),
            "svg" | "iframe" | "object" | "embed" | "canvas" | "video" | "audio" => {}
            _ => self.children(n, st),
        }
    }

    fn image(&mut self, n: usize, st: &Style) {
        let node = &self.dom.nodes[n];
        let alt = node.attr("alt").unwrap_or("").trim().to_string();
        let src = node
            .attr("src")
            .or_else(|| node.attr("data-src"))
            .unwrap_or("")
            .to_string();
        let tiny = st.width.is_some_and(|w| w <= 2) || st.height.is_some_and(|h| h <= 2);
        if self.opts.images && !src.is_empty() && !src.starts_with("data:") && !tiny {
            self.doc.images.push(ImageRef {
                src,
                alt: alt.clone(),
                width: st.width,
                height: st.height,
            });
            let mut s = self.text_span(&alt, st);
            s.image = Some(self.doc.images.len() - 1);
            s.underline = false;
            self.current.spans.push(s);
        } else if !alt.is_empty() {
            let mut s = self.text_span(&format!("[{alt}]"), st);
            s.dim = self.link.is_none();
            self.push(s);
        }
    }

    fn field(&mut self, n: usize, name: &str, st: &Style) {
        let node = &self.dom.nodes[n];
        let kind = match name {
            "select" => FieldKind::Select,
            "textarea" => FieldKind::TextArea,
            "button" => match node.attr("type").map(str::to_ascii_lowercase).as_deref() {
                Some("button") | Some("reset") => FieldKind::Button,
                _ => FieldKind::Submit,
            },
            _ => match node
                .attr("type")
                .unwrap_or("text")
                .to_ascii_lowercase()
                .as_str()
            {
                "hidden" => FieldKind::Hidden,
                "submit" | "image" => FieldKind::Submit,
                "button" | "reset" => FieldKind::Button,
                "checkbox" => FieldKind::Checkbox,
                "radio" => FieldKind::Radio,
                "password" => FieldKind::Password,
                "file" | "color" | "range" => return,
                _ => FieldKind::Text,
            },
        };
        // Texto de un botón o de las opciones (sus hijos).
        let inner_text = |dom: &Dom, n: usize| -> String {
            fn collect(dom: &Dom, n: usize, out: &mut String) {
                for &c in &dom.nodes[n].children {
                    match &dom.nodes[c].kind {
                        NodeKind::Text(t) => out.push_str(t),
                        _ => collect(dom, c, out),
                    }
                }
            }
            let mut s = String::new();
            collect(dom, n, &mut s);
            s.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        let mut value = node.attr("value").unwrap_or("").to_string();
        let mut options = Vec::new();
        match kind {
            FieldKind::Submit | FieldKind::Button if name == "button" => {
                let t = inner_text(self.dom, n);
                if value.is_empty() || !t.is_empty() {
                    // El texto visible; el valor que se manda queda en `options[0]`.
                    options.push((value.clone(), String::new()));
                    value = t;
                }
            }
            FieldKind::Submit if value.is_empty() => value = "Enviar".into(),
            FieldKind::Select => {
                let mut selected = None;
                for &c in &node.children {
                    let o = &self.dom.nodes[c];
                    if o.name() != "option" {
                        continue;
                    }
                    let label = inner_text(self.dom, c);
                    let v = o.attr("value").map_or(label.clone(), String::from);
                    if o.attr("selected").is_some() || selected.is_none() {
                        selected = Some(v.clone());
                    }
                    options.push((v, label));
                }
                value = selected.unwrap_or_default();
            }
            FieldKind::TextArea => value = inner_text(self.dom, n),
            _ => {}
        }
        let size_attr = node.attr("size").and_then(|s| s.parse::<i32>().ok());
        let width = st.width.filter(|w| *w >= 40).unwrap_or(match kind {
            FieldKind::Text | FieldKind::Password => {
                size_attr.map_or(220, |s| (s * 9).clamp(80, 560))
            }
            FieldKind::TextArea => 420,
            FieldKind::Checkbox | FieldKind::Radio => 18,
            _ => 0, // botones y listas: según su texto
        });
        self.doc.fields.push(Field {
            kind,
            form: self.form,
            name: node.attr("name").unwrap_or("").to_string(),
            value,
            checked: node.attr("checked").is_some(),
            options,
            placeholder: node
                .attr("placeholder")
                .or_else(|| node.attr("title"))
                .or_else(|| node.attr("aria-label"))
                .unwrap_or("")
                .to_string(),
            width,
        });
        if kind != FieldKind::Hidden {
            let mut s = self.text_span("", st);
            s.field = Some(self.doc.fields.len() - 1);
            s.underline = false;
            s.link = None;
            self.current.spans.push(s);
        }
    }
}

/// HTML → documento, con las opciones y las hojas de estilo externas ya bajadas.
pub fn render(html: &str, opts: Options, external_css: &[String]) -> Document {
    let dom = dom::parse(html);
    let mut sheet = Sheet::new();
    if !opts.reader {
        for css in external_css {
            sheet.add(css);
        }
        for css in &dom.styles {
            sheet.add(css);
        }
    }
    let mut f = Flat {
        opts,
        sheet: &sheet,
        dom: &dom,
        doc: Document {
            title: dom.title.clone(),
            stylesheets: if opts.reader {
                Vec::new()
            } else {
                dom.stylesheet_links.clone()
            },
            base: dom.base.clone(),
            ..Default::default()
        },
        current: Block {
            kind: BlockKind::Text,
            indent: 0,
            spans: Vec::new(),
            align: Align::Left,
            bg: None,
        },
        block_bg: Vec::new(),
        link: None,
        form: None,
        pre: 0,
        quote: 0,
        lists: Vec::new(),
        path: Vec::new(),
        in_main: 0,
        row: false,
        align: Vec::new(),
    };
    f.walk(0, &Style::root());
    f.flush();
    f.doc
}

/// Modo lectura, sin estilos (lo que hacía el navegador de texto).
pub fn parse(html: &str) -> Document {
    render(html, Options::READER, &[])
}

/// Texto plano → un bloque preformateado.
pub fn plain(text: &str) -> Document {
    Document {
        blocks: alloc::vec![Block {
            kind: BlockKind::Pre,
            indent: 0,
            spans: alloc::vec![Span::text(&text.replace('\t', "    ").replace('\r', ""))],
            align: Align::Left,
            bg: None,
        }],
        ..Default::default()
    }
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
        '\u{203A}' | '\u{276F}' => out.push('>'),
        '\u{2039}' | '\u{276E}' => out.push('<'),
        '\u{2713}' | '\u{2714}' => out.push('v'),
        '\u{2715}' | '\u{2716}' => out.push('x'),
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

    #[test]
    fn estilos_colores_y_alineacion() {
        let d = render(
            "<html><head><style>body{background:#fff;color:#202124} .logo{color:#4285f4;font-size:32px;text-align:center}             .oculto{display:none} a{color:#1a0dab;text-decoration:none}</style></head>             <body><div class=logo>Google</div><p class=oculto>no</p><p>ver <a href=/x>enlace</a></p></body></html>",
            Options::STYLED,
            &[],
        );
        assert_eq!(d.bg, Some(Color::hex(0xffffff)));
        assert_eq!(d.fg, Some(Color::hex(0x202124)));
        assert_eq!(texts(&d), ["Google", "ver enlace"]);
        let logo = &d.blocks[0];
        assert_eq!(logo.align, Align::Center);
        assert_eq!(
            (logo.spans[0].color, logo.spans[0].size),
            (Some(Color::hex(0x4285f4)), 32)
        );
        let link = d.blocks[1].spans.iter().find(|s| s.link.is_some()).unwrap();
        assert!(!link.underline, "text-decoration: none");
    }

    #[test]
    fn formularios_e_imagenes() {
        let d = render(
            "<form action=/search><input type=hidden name=hl value=es><input name=q size=40>             <input type=submit name=btnG value='Buscar'><button>Suerte</button>             <select name=s><option value=a>A<option value=b selected>B</select></form>             <img src=logo.png alt=Logo width=272 height=92>",
            Options::STYLED,
            &[],
        );
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
        assert_eq!(d.fields[1].width, 360);
        assert_eq!(d.fields[3].value, "Suerte");
        assert_eq!(d.fields[4].value, "b");
        assert_eq!(d.images[0].width, Some(272));
        // En modo lectura no hay formularios y la imagen es su texto.
        let r = parse("<form><input name=q></form><img src=x alt=Logo>");
        assert!(r.fields.is_empty());
        assert_eq!(texts(&r), ["[Logo]"]);
    }

    #[test]
    fn flex_pone_los_bloques_en_fila() {
        let d = render(
            "<style>.menu{display:flex}</style><div class=menu><div>Uno</div><div>Dos</div></div><p>Fin</p>",
            Options::STYLED,
            &[],
        );
        assert_eq!(texts(&d), ["Uno   Dos", "Fin"]);
    }
}
