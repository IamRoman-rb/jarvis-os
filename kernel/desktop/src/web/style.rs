//! Estilo calculado de cada elemento: la cascada ([`super::css`]) aplicada sobre los estilos por
//! defecto del navegador, con herencia, variables (`--x` / `var()`) y los atributos de
//! presentación viejos (`bgcolor`, `width`, `align`…).
//!
//! El resultado es un [`Style`] por elemento, con todo lo que necesita la maquetación
//! ([`super::layout`]): cajas (márgenes, bordes, relleno, tamaños), flujo (`display`,
//! `position`, `float`), flex y grid, texto y colores.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_gfx::Color;
use jarvis_gfx::webfont::FontSpec;

use super::css::{self, Len, Matcher, Media, Rgba, Sheet, Units};
use super::dom::{Dom, NodeKind};

/// Los estilos que trae cualquier navegador (como el "user agent stylesheet" de Chrome).
pub const UA_CSS: &str = "
html,body,div,p,h1,h2,h3,h4,h5,h6,ul,ol,dl,dt,dd,section,article,header,footer,nav,main,aside,
form,fieldset,figure,figcaption,blockquote,pre,address,details,summary,hr,center,menu,dialog,
legend,hgroup,search,noscript,listing,plaintext,xmp,frameset,frame,optgroup{display:block}
li{display:list-item}
head,script,style,template,meta,link,title,base,datalist,param,source,track,area,map,col,
colgroup,option,rp,input[type=hidden]{display:none}
[hidden]{display:none}
dialog:not([open]){display:none}
details:not([open])>:not(summary){display:none}
table{display:table;border-spacing:2px;border-collapse:separate;box-sizing:border-box;text-indent:0}
caption{display:table-caption;text-align:center}
thead,tbody,tfoot{display:table-row-group;vertical-align:middle}
tr{display:table-row;vertical-align:inherit}
td,th{display:table-cell;padding:1px;vertical-align:inherit}
th{font-weight:bold;text-align:center}
body{margin:8px}
p,dl,menu{margin:1em 0}
blockquote,figure{margin:1em 40px}
ul,ol{margin:1em 0;padding-left:40px}
ul{list-style-type:disc}
ol{list-style-type:decimal}
ul ul,ol ul{list-style-type:circle}
ul ul ul,ol ul ul,ul ol ul{list-style-type:square}
ul ul,ul ol,ol ul,ol ol{margin-top:0;margin-bottom:0}
dd{margin-left:40px}
h1{font-size:2em;margin:.67em 0;font-weight:bold}
h2{font-size:1.5em;margin:.83em 0;font-weight:bold}
h3{font-size:1.17em;margin:1em 0;font-weight:bold}
h4{margin:1.33em 0;font-weight:bold}
h5{font-size:.83em;margin:1.67em 0;font-weight:bold}
h6{font-size:.67em;margin:2.33em 0;font-weight:bold}
b,strong{font-weight:bold}
i,em,cite,var,dfn,address{font-style:italic}
u,ins{text-decoration:underline}
s,strike,del{text-decoration:line-through}
code,kbd,samp,pre,tt,listing,xmp,plaintext{font-family:monospace}
pre,listing,xmp,plaintext{white-space:pre;margin:1em 0}
small{font-size:smaller}
big{font-size:larger}
sub,sup{font-size:smaller}
sub{vertical-align:sub}
sup{vertical-align:super}
mark{background-color:yellow;color:black}
a:link{color:#0000ee;text-decoration:underline}
hr{border-width:1px 0 0 0;border-style:solid;border-color:#c0c0c0;margin:.5em 0}
center{text-align:center}
nobr{white-space:nowrap}
img,svg,video,canvas,iframe,embed,object{display:inline-block}
input,button,select,textarea{display:inline-block;font-size:13.333px;color:black}
input,select,textarea{background-color:white;border:1px solid #767676;padding:2px 3px}
textarea{white-space:pre-wrap}
button,input[type=submit],input[type=button],input[type=reset],input[type=image]{background-color:#efefef;border:1px solid #767676;border-radius:3px;padding:1px 6px;text-align:center}
input[type=checkbox],input[type=radio]{border:0;padding:0;margin:3px 3px 3px 4px}
select{border-radius:3px}
fieldset{margin:0 2px;padding:.35em .75em .625em;border:2px groove #c0c0c0}
legend{padding:0 2px}
iframe{border:2px inset #c0c0c0}
abbr[title]{text-decoration:underline}
q{font-style:normal}
";

/// Estilos del modo lectura: sin los de la página, letra grande, columna angosta y los colores
/// del tema de JARVIS.
pub const READER_CSS: &str = "
body{background:#0b1220;color:#dce3f0;font-size:17px;line-height:1.55;margin:0;padding:24px 32px}
body>*{max-width:760px;margin-left:auto;margin-right:auto}
a:link{color:#5cc8ff;text-decoration:none}
h1{color:#ffffff;font-size:30px;line-height:1.25}
h2{color:#00f0ff;font-size:23px}
h3,h4,h5,h6{color:#8fd0ff}
pre,code{color:#a8ffe4}
blockquote{color:#9fb0c6;border-left:3px solid #1d3a57;padding-left:14px;margin-left:0}
th,td{border-bottom:1px solid #1d3a57;padding:4px 8px;text-align:left}
hr{border-color:#1d3a57}
img{max-width:100%;height:auto}
";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Display {
    None,
    Block,
    Inline,
    InlineBlock,
    ListItem,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    Table,
    InlineTable,
    TableRowGroup,
    TableRow,
    TableCell,
    TableCaption,
    /// `display: contents`: el elemento no tiene caja; sus hijos van en su lugar.
    Contents,
}

impl Display {
    /// ¿Va en la línea, como una palabra (o una caja en la línea)?
    pub fn is_inline_level(self) -> bool {
        matches!(
            self,
            Display::Inline
                | Display::InlineBlock
                | Display::InlineFlex
                | Display::InlineGrid
                | Display::InlineTable
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Float {
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    NoWrap,
    Pre,
    PreWrap,
    PreLine,
}

impl WhiteSpace {
    pub fn collapses(self) -> bool {
        matches!(
            self,
            WhiteSpace::Normal | WhiteSpace::NoWrap | WhiteSpace::PreLine
        )
    }

    pub fn wraps(self) -> bool {
        !matches!(self, WhiteSpace::NoWrap | WhiteSpace::Pre)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transform {
    None,
    Upper,
    Lower,
    Capitalize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlexDir {
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDir {
    pub fn is_row(self) -> bool {
        matches!(self, FlexDir::Row | FlexDir::RowReverse)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
    Baseline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    Baseline,
    Top,
    Middle,
    Bottom,
    Sub,
    Super,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineHeight {
    Normal,
    /// Múltiplo del tamaño de letra.
    Factor(f32),
    Px(i32),
}

/// Una pista (columna o fila) de una grilla.
#[derive(Clone, Debug, PartialEq)]
pub enum Track {
    Fixed(Len),
    /// `1fr`.
    Fr(f32),
    /// `auto`, `min-content`, `max-content`.
    Auto,
    /// `minmax(min, max)`: `max` puede ser `fr`.
    MinMax(Len, f32, Option<Len>),
}

/// Cuántas pistas genera `repeat(auto-fill, …)`: se resuelve al maquetar (depende del ancho).
#[derive(Clone, Debug, PartialEq)]
pub struct TrackList {
    pub tracks: Vec<Track>,
    /// `repeat(auto-fill|auto-fit, …)`: estas pistas se repiten las veces que entren.
    pub auto_repeat: Option<Vec<Track>>,
    /// Posición de la repetición automática dentro de `tracks`.
    pub auto_at: usize,
}

/// Dónde empieza o termina un elemento en la grilla.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum GridLine {
    #[default]
    Auto,
    /// Línea numerada (desde 1; negativa desde el final).
    Line(i32),
    Span(u32),
    /// Nombre de un área (`grid-area: main`).
    Area(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BgSize {
    Auto,
    Cover,
    Contain,
    /// Ancho y alto (`None` = auto).
    Px(Option<i32>, Option<i32>),
    Pct(f32, Option<f32>),
}

#[derive(Clone, Debug)]
pub struct Style {
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
    pub width: Len,
    pub height: Len,
    pub min_width: Len,
    pub max_width: Len,
    pub min_height: Len,
    pub max_height: Len,
    /// Arriba, derecha, abajo, izquierda.
    pub margin: [Len; 4],
    pub padding: [Len; 4],
    /// Ancho pedido de cada borde (vale solo si tiene estilo: ver [`Style::border_widths`]).
    pub border: [i32; 4],
    /// ¿El borde tiene un estilo que se ve (`solid`, `dashed`…)?
    pub border_on: [bool; 4],
    /// El color del borde es el del texto (`currentColor`, lo que vale si no se dice otro).
    pub border_current: [bool; 4],
    pub border_color: [Rgba; 4],
    pub radius: i32,
    /// `top`, `right`, `bottom`, `left`.
    pub inset: [Len; 4],
    pub border_box: bool,
    pub bg: Option<Rgba>,
    pub bg_image: Option<String>,
    pub bg_size: BgSize,
    pub bg_pos: (Len, Len),
    pub bg_repeat: bool,
    /// Ícono con `mask-image`: la forma sale de la imagen y el color, del fondo.
    pub mask_image: Option<String>,
    pub mask_size: BgSize,
    pub mask_pos: (Len, Len),
    /// `caption-side: bottom`.
    pub caption_bottom: bool,
    pub color: Rgba,
    pub font: FontSpec,
    pub font_size: f32,
    pub line_height: LineHeight,
    pub align: TextAlign,
    pub white_space: WhiteSpace,
    pub underline: bool,
    pub strike: bool,
    pub transform: Transform,
    pub visible: bool,
    /// `opacity: 0`: ocupa lugar pero no se ve.
    pub transparent: bool,
    pub clip_x: bool,
    pub clip_y: bool,
    /// Lo que se ve en `overflow: auto` (se muestra entero en vez de con barra propia).
    pub scroll_y: bool,
    pub list_style: ListStyle,
    pub list_inside: bool,
    pub flex_dir: FlexDir,
    pub flex_wrap: bool,
    pub justify: Justify,
    pub align_items: Align,
    pub align_self: Option<Align>,
    pub align_content: Justify,
    pub justify_items: Align,
    pub justify_self: Option<Align>,
    pub grow: f32,
    pub shrink: f32,
    pub basis: Len,
    pub order: i32,
    pub row_gap: Len,
    pub col_gap: Len,
    pub grid_cols: Option<Rc<TrackList>>,
    pub grid_rows: Option<Rc<TrackList>>,
    pub grid_areas: Option<Rc<Vec<Vec<String>>>>,
    pub grid_auto_rows: Option<Track>,
    pub grid_col: (GridLine, GridLine),
    pub grid_row: (GridLine, GridLine),
    pub grid_flow_column: bool,
    pub valign: VAlign,
    pub text_indent: Len,
    pub z_index: i32,
    /// Una fuente de íconos (Material Icons, Font Awesome): su texto son nombres o letras
    /// privadas, no palabras. No se muestra.
    pub icon_font: bool,
    pub border_collapse: bool,
    pub border_spacing: i32,
    pub table_fixed: bool,
    pub aspect_ratio: Option<f32>,
    pub letter_spacing: i32,
    pub word_break: bool,
    pub vars: Rc<BTreeMap<String, String>>,
}

impl Style {
    pub fn root() -> Style {
        let zero = Len::Px(0.0);
        let black = Rgba::opaque(Color::BLACK);
        Style {
            display: Display::Block,
            position: Position::Static,
            float: Float::None,
            clear: Clear::None,
            width: Len::Auto,
            height: Len::Auto,
            min_width: Len::Auto,
            max_width: Len::Auto,
            min_height: Len::Auto,
            max_height: Len::Auto,
            margin: [zero.clone(), zero.clone(), zero.clone(), zero.clone()],
            padding: [zero.clone(), zero.clone(), zero.clone(), zero.clone()],
            border: [3; 4],
            border_on: [false; 4],
            border_current: [true; 4],
            border_color: [black; 4],
            radius: 0,
            inset: [Len::Auto, Len::Auto, Len::Auto, Len::Auto],
            border_box: false,
            bg: None,
            bg_image: None,
            bg_size: BgSize::Auto,
            bg_pos: (Len::Px(0.0), Len::Px(0.0)),
            bg_repeat: true,
            mask_image: None,
            mask_size: BgSize::Contain,
            mask_pos: (Len::Pct(50.0), Len::Pct(50.0)),
            caption_bottom: false,
            color: black,
            font: FontSpec::new(16),
            font_size: 16.0,
            line_height: LineHeight::Normal,
            align: TextAlign::Left,
            white_space: WhiteSpace::Normal,
            underline: false,
            strike: false,
            transform: Transform::None,
            visible: true,
            transparent: false,
            clip_x: false,
            clip_y: false,
            scroll_y: false,
            list_style: ListStyle::Disc,
            list_inside: false,
            flex_dir: FlexDir::Row,
            flex_wrap: false,
            justify: Justify::Start,
            align_items: Align::Stretch,
            align_self: None,
            align_content: Justify::Start,
            justify_items: Align::Stretch,
            justify_self: None,
            grow: 0.0,
            shrink: 1.0,
            basis: Len::Auto,
            order: 0,
            row_gap: Len::Px(0.0),
            col_gap: Len::Px(0.0),
            grid_cols: None,
            grid_rows: None,
            grid_areas: None,
            grid_auto_rows: None,
            grid_col: (GridLine::Auto, GridLine::Auto),
            grid_row: (GridLine::Auto, GridLine::Auto),
            grid_flow_column: false,
            valign: VAlign::Baseline,
            text_indent: Len::Px(0.0),
            z_index: 0,
            icon_font: false,
            border_collapse: false,
            border_spacing: 0,
            table_fixed: false,
            aspect_ratio: None,
            letter_spacing: 0,
            word_break: false,
            vars: Rc::new(BTreeMap::new()),
        }
    }

    /// Lo que hereda un hijo: el texto, los colores de letra, la alineación, las variables.
    fn inherit(&self) -> Style {
        let mut s = Style::root();
        s.display = Display::Inline;
        s.color = self.color;
        s.font = self.font;
        s.font_size = self.font_size;
        s.line_height = self.line_height;
        s.align = self.align;
        s.white_space = self.white_space;
        // La decoración de texto no se hereda, pero se pinta sobre los descendientes.
        s.underline = self.underline;
        s.strike = self.strike;
        s.transform = self.transform;
        s.visible = self.visible;
        s.list_style = self.list_style;
        s.list_inside = self.list_inside;
        s.text_indent = self.text_indent.clone();
        s.icon_font = self.icon_font;
        s.border_collapse = self.border_collapse;
        s.border_spacing = self.border_spacing;
        s.justify_items = Align::Stretch;
        s.letter_spacing = self.letter_spacing;
        s.word_break = self.word_break;
        s.vars = self.vars.clone();
        s
    }

    /// Anchos de los bordes que se ven (sin estilo, un borde no existe).
    pub fn border_widths(&self) -> [i32; 4] {
        let mut w = [0; 4];
        for i in 0..4 {
            if self.border_on[i] {
                w[i] = self.border[i];
            }
        }
        w
    }

    /// Alto de un renglón con esta letra.
    pub fn line_px(&self, normal: i32) -> i32 {
        match self.line_height {
            LineHeight::Normal => normal,
            LineHeight::Factor(f) => (self.font_size * f) as i32,
            LineHeight::Px(p) => p,
        }
        .max(1)
    }

    pub fn in_flow(&self) -> bool {
        !matches!(self.position, Position::Absolute | Position::Fixed)
    }

    /// ¿La caja arma su propio "contexto de formato" (contiene sus flotantes)?
    pub fn is_bfc(&self) -> bool {
        self.float != Float::None
            || !self.in_flow()
            || self.clip_x
            || self.clip_y
            || matches!(
                self.display,
                Display::InlineBlock
                    | Display::TableCell
                    | Display::Flex
                    | Display::Grid
                    | Display::InlineFlex
                    | Display::InlineGrid
                    | Display::TableCaption
            )
    }
}

/// Opciones del armado de la página.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Modo lectura: sin los estilos de la página, sin menús ni formularios.
    pub reader: bool,
    /// Mostrar imágenes.
    pub images: bool,
}

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
fn reader_hidden(dom: &Dom, n: usize, in_main: bool) -> bool {
    let node = &dom.nodes[n];
    let name = node.name();
    // Los contenedores de toda la página nunca (en Wikipedia, `<body>` tiene la clase
    // "vector-toc-available", y ocultarlo dejaba la página vacía).
    if matches!(name, "html" | "body" | "main" | "article") {
        return false;
    }
    let chrome = matches!(
        name,
        "nav" | "aside" | "form" | "button" | "select" | "dialog" | "input" | "textarea"
    ) || (!in_main && matches!(name, "header" | "footer"));
    let role = node.attr("role").unwrap_or("");
    let style: String = node
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
        || noise(node.attr("class").unwrap_or(""))
        || noise(node.attr("id").unwrap_or(""))
        || node.attr("aria-hidden") == Some("true")
        || matches!(role, "navigation" | "banner" | "contentinfo" | "search")
        || style.contains("display:none")
}

/// El estilo de cada nodo (los de texto no tienen: usan el de su padre).
pub struct Styled {
    pub styles: Vec<Option<Rc<Style>>>,
}

impl Styled {
    pub fn get(&self, n: usize) -> Option<&Style> {
        self.styles.get(n).and_then(|s| s.as_deref())
    }
}

struct Ctx<'a> {
    dom: &'a Dom,
    m: Matcher<'a>,
    ua: &'a Sheet,
    author: &'a Sheet,
    opts: Options,
    media: Media,
    root_font: f32,
    out: Vec<Option<Rc<Style>>>,
}

/// Calcula los estilos de todo el árbol.
pub fn compute(dom: &Dom, ua: &Sheet, author: &Sheet, opts: Options, media: Media) -> Styled {
    let mut ctx = Ctx {
        dom,
        m: Matcher::new(dom),
        ua,
        author,
        opts,
        media,
        root_font: 16.0,
        out: alloc::vec![None; dom.nodes.len()],
    };
    let root = Rc::new(Style::root());
    // Un recorrido con pila (sin recursión: el árbol puede ser muy profundo).
    let mut stack: Vec<(usize, Rc<Style>, bool)> = Vec::new();
    for &c in dom.nodes[0].children.iter().rev() {
        stack.push((c, root.clone(), false));
    }
    while let Some((n, parent, in_main)) = stack.pop() {
        if !matches!(dom.nodes[n].kind, NodeKind::Element { .. }) {
            continue;
        }
        if opts.reader && reader_hidden(dom, n, in_main) {
            continue;
        }
        let st = Rc::new(ctx.style_of(n, &parent));
        if st.display == Display::None {
            ctx.out[n] = Some(st);
            continue;
        }
        let main = in_main || matches!(dom.nodes[n].name(), "main" | "article");
        for &c in dom.nodes[n].children.iter().rev() {
            stack.push((c, st.clone(), main));
        }
        ctx.out[n] = Some(st);
    }
    Styled { styles: ctx.out }
}

fn attr_len(v: Option<&str>) -> Option<Len> {
    let v = v?.trim();
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().map(Len::Pct);
    }
    let v = v.trim_end_matches("px");
    v.parse::<f32>()
        .ok()
        .filter(|n| *n >= 0.0 && *n < 10_000.0)
        .map(Len::Px)
}

impl Ctx<'_> {
    fn units(&self, st: &Style) -> Units {
        Units {
            em: st.font_size,
            rem: self.root_font,
            vw: self.media.width as f32,
            vh: self.media.height as f32,
        }
    }

    fn style_of(&mut self, n: usize, parent: &Style) -> Style {
        let node = &self.dom.nodes[n];
        let name = node.name();
        let mut st = parent.inherit();
        let ua = self.ua.matching(&self.m, n);
        let author: Vec<&css::Decl> = if self.opts.reader {
            Vec::new()
        } else {
            self.author.matching(&self.m, n)
        };
        let inline = if self.opts.reader {
            Vec::new()
        } else {
            node.attr("style").map(css::parse_decls).unwrap_or_default()
        };
        // Variables: se heredan; las propias pisan a las del padre.
        let mut own_vars: Vec<(&str, &str)> = Vec::new();
        for d in author.iter().copied().chain(inline.iter()) {
            if d.prop.starts_with("--") {
                own_vars.push((&d.prop, &d.value));
            }
        }
        if !own_vars.is_empty() {
            let parent_vars = st.vars.clone();
            let mut own: BTreeMap<String, String> = BTreeMap::new();
            for (k, v) in own_vars {
                own.insert(k.to_string(), v.to_string());
            }
            // Las variables que usan otras se resuelven acá (una vez). Una que se usa a sí
            // misma (`--x: var(--x, 1rem)`) toma el valor del padre o el de respaldo.
            let mut vars = (*parent_vars).clone();
            for (k, v) in &own {
                let value = if v.contains("var(") {
                    let mut lookup = (*parent_vars).clone();
                    for (k2, v2) in &own {
                        if k2 != k && !v2.contains("var(") {
                            lookup.insert(k2.clone(), v2.clone());
                        }
                    }
                    resolve_vars(v, &lookup)
                } else {
                    v.clone()
                };
                vars.insert(k.clone(), value);
            }
            st.vars = Rc::new(vars);
        }
        // `font-size` primero: los `em` de todo lo demás dependen de él.
        let decls: Vec<&css::Decl> = ua.iter().copied().collect();
        self.apply_all(&mut st, parent, &decls, n, true);
        self.presentational(&mut st, parent, n, name);
        let decls: Vec<&css::Decl> = author.iter().copied().chain(inline.iter()).collect();
        self.apply_all(&mut st, parent, &decls, n, false);
        // Un flotante o algo posicionado es un bloque (salvo `display: none`).
        if st.display != Display::None
            && (st.float != Float::None
                || matches!(st.position, Position::Absolute | Position::Fixed))
        {
            st.display = match st.display {
                Display::Inline | Display::InlineBlock | Display::Contents => Display::Block,
                Display::InlineFlex => Display::Flex,
                Display::InlineGrid => Display::Grid,
                Display::InlineTable => Display::Table,
                d => d,
            };
        }
        if name == "html" {
            self.root_font = st.font_size;
        }
        for i in 0..4 {
            if st.border_current[i] {
                st.border_color[i] = st.color;
            }
        }
        st
    }

    fn apply_all(
        &mut self,
        st: &mut Style,
        parent: &Style,
        decls: &[&css::Decl],
        n: usize,
        ua: bool,
    ) {
        let resolve = |v: &str, st: &Style| -> String {
            if v.contains("var(") {
                resolve_vars(v, &st.vars)
            } else {
                v.to_string()
            }
        };
        // Primero lo que cambia las unidades (letra), después el resto.
        for d in decls.iter() {
            if matches!(d.prop.as_str(), "font-size" | "font") {
                let v = resolve(&d.value, st);
                self.apply(st, parent, &d.prop, v.trim(), n, ua);
            }
        }
        for d in decls.iter() {
            if d.prop.starts_with("--") || matches!(d.prop.as_str(), "font-size") {
                continue;
            }
            let v = resolve(&d.value, st);
            self.apply(st, parent, &d.prop, v.trim(), n, ua);
        }
    }

    /// Atributos viejos de presentación: valen como reglas de la página con especificidad 0.
    fn presentational(&mut self, st: &mut Style, parent: &Style, n: usize, name: &str) {
        let node = &self.dom.nodes[n];
        if self.opts.reader {
            return;
        }
        if let Some(c) = node.attr("bgcolor").and_then(css::parse_rgba) {
            st.bg = Some(c);
        }
        if let Some(bg) = node.attr("background") {
            st.bg_image = Some(bg.to_string());
        }
        if matches!(name, "font" | "basefont")
            && let Some(c) = node.attr("color").and_then(css::parse_rgba)
        {
            st.color = c;
        }
        if name == "font"
            && let Some(sz) = node.attr("size")
        {
            let px = match sz.trim() {
                "1" | "-2" => 10.0,
                "2" | "-1" => 13.0,
                "3" => 16.0,
                "4" | "+1" => 18.0,
                "5" | "+2" => 24.0,
                "6" | "+3" => 32.0,
                "7" | "+4" => 48.0,
                _ => st.font_size,
            };
            st.font_size = px;
            st.font.px = px as u16;
        }
        if name == "body"
            && let Some(c) = node.attr("text").and_then(css::parse_rgba)
        {
            st.color = c;
        }
        match node.attr("align").map(str::to_ascii_lowercase).as_deref() {
            Some("center" | "middle") if name != "img" => st.align = TextAlign::Center,
            Some("right") if name == "img" || name == "table" => st.float = Float::Right,
            Some("left") if name == "img" || name == "table" => st.float = Float::Left,
            Some("right") => st.align = TextAlign::Right,
            _ => {}
        }
        if name == "table"
            && node
                .attr("align")
                .is_some_and(|a| a.eq_ignore_ascii_case("center"))
        {
            st.margin[1] = Len::Auto;
            st.margin[3] = Len::Auto;
        }
        match node.attr("valign").map(str::to_ascii_lowercase).as_deref() {
            Some("top") => st.valign = VAlign::Top,
            Some("bottom") => st.valign = VAlign::Bottom,
            Some("middle" | "center") => st.valign = VAlign::Middle,
            _ => {}
        }
        if matches!(
            name,
            "img"
                | "table"
                | "td"
                | "th"
                | "iframe"
                | "video"
                | "canvas"
                | "svg"
                | "embed"
                | "object"
                | "col"
                | "hr"
                | "input"
        ) {
            if let Some(w) = attr_len(node.attr("width")) {
                st.width = w;
            }
            if let Some(h) = attr_len(node.attr("height")) {
                st.height = h;
            }
            // Con `width` y `height`, la imagen sabe su forma antes de llegar (así la página no
            // "salta" cuando carga, aunque el CSS diga `height: auto`).
            if let (Some(Len::Px(w)), Some(Len::Px(h))) =
                (attr_len(node.attr("width")), attr_len(node.attr("height")))
                && h > 0.0
            {
                st.aspect_ratio = Some(w / h);
            }
        }
        if name == "table" {
            if let Some(b) = node
                .attr("border")
                .and_then(|b| b.trim().parse::<i32>().ok())
            {
                let b = if node.attr("border") == Some("") {
                    1
                } else {
                    b
                };
                let grey = Rgba::opaque(Color::hex(0x808080));
                st.border = [b; 4];
                st.border_on = [b > 0; 4];
                st.border_current = [false; 4];
                st.border_color = [grey; 4];
            }
            if let Some(s) = node
                .attr("cellspacing")
                .and_then(|b| b.trim().parse::<i32>().ok())
            {
                st.border_spacing = s;
            }
        }
        if matches!(name, "td" | "th") {
            // `cellpadding` y `border` de la tabla pasan a las celdas.
            let mut cur = self.dom.nodes[n].parent;
            while let Some(p) = cur {
                let pn = &self.dom.nodes[p];
                if pn.name() == "table" {
                    if let Some(cp) = pn
                        .attr("cellpadding")
                        .and_then(|v| v.trim().parse::<f32>().ok())
                    {
                        let l = Len::Px(cp);
                        st.padding = [l.clone(), l.clone(), l.clone(), l];
                    }
                    if pn.attr("border").is_some_and(|b| b.trim() != "0") {
                        st.border = [1; 4];
                        st.border_on = [true; 4];
                        st.border_current = [false; 4];
                        st.border_color = [Rgba::opaque(Color::hex(0x808080)); 4];
                    }
                    break;
                }
                cur = pn.parent;
            }
            if node.attr("nowrap").is_some() {
                st.white_space = WhiteSpace::NoWrap;
            }
        }
        if name == "hr"
            && let Some(c) = node.attr("color").and_then(css::parse_rgba)
        {
            st.border_color = [c; 4];
            st.border_current = [false; 4];
        }
        let _ = parent;
    }

    fn apply(&mut self, st: &mut Style, parent: &Style, prop: &str, v: &str, n: usize, ua: bool) {
        let lower = v.to_ascii_lowercase();
        let u = self.units(st);
        let len = |v: &str| css::parse_len(v, u);
        let inherit = lower == "inherit";
        let _ = ua;
        match prop {
            "display" => {
                st.display = match lower.split_whitespace().collect::<Vec<_>>().as_slice() {
                    ["none"] => Display::None,
                    ["block"]
                    | ["flow-root"]
                    | ["block", "flow-root"]
                    | ["block", "flow"]
                    | ["-webkit-box"]
                    | ["run-in"] => Display::Block,
                    ["inline"] | ["inline", "flow"] | ["ruby"] | ["ruby-text"] => Display::Inline,
                    ["inline-block"] | ["inline", "flow-root"] | ["-webkit-inline-box"] => {
                        Display::InlineBlock
                    }
                    ["list-item"] | ["block", "list-item"] => Display::ListItem,
                    ["flex"] | ["block", "flex"] | ["-webkit-flex"] | ["-ms-flexbox"] => {
                        Display::Flex
                    }
                    ["inline-flex"] | ["inline", "flex"] | ["-webkit-inline-flex"] => {
                        Display::InlineFlex
                    }
                    ["grid"] | ["block", "grid"] | ["-ms-grid"] => Display::Grid,
                    ["inline-grid"] | ["inline", "grid"] => Display::InlineGrid,
                    ["table"] => Display::Table,
                    ["inline-table"] => Display::InlineTable,
                    ["table-row-group"] | ["table-header-group"] | ["table-footer-group"] => {
                        Display::TableRowGroup
                    }
                    ["table-row"] => Display::TableRow,
                    ["table-cell"] => Display::TableCell,
                    ["table-caption"] => Display::TableCaption,
                    ["table-column"] | ["table-column-group"] => Display::None,
                    ["contents"] => Display::Contents,
                    _ => st.display,
                }
            }
            "position" => {
                st.position = match lower.as_str() {
                    "relative" => Position::Relative,
                    "absolute" => Position::Absolute,
                    "fixed" => Position::Fixed,
                    "sticky" | "-webkit-sticky" => Position::Sticky,
                    "static" => Position::Static,
                    _ => st.position,
                }
            }
            "float" => {
                st.float = match lower.as_str() {
                    "left" | "inline-start" => Float::Left,
                    "right" | "inline-end" => Float::Right,
                    _ => Float::None,
                }
            }
            "clear" => {
                st.clear = match lower.as_str() {
                    "left" | "inline-start" => Clear::Left,
                    "right" | "inline-end" => Clear::Right,
                    "both" => Clear::Both,
                    _ => Clear::None,
                }
            }
            "width" | "inline-size" => {
                if let Some(l) = len(v) {
                    st.width = l;
                }
            }
            "height" | "block-size" => {
                if let Some(l) = len(v) {
                    st.height = l;
                }
            }
            "min-width" | "min-inline-size" => {
                if let Some(l) = len(v) {
                    st.min_width = l;
                }
            }
            "max-width" | "max-inline-size" => {
                if let Some(l) = len(v) {
                    st.max_width = l;
                }
            }
            "min-height" | "min-block-size" => {
                if let Some(l) = len(v) {
                    st.min_height = l;
                }
            }
            "max-height" | "max-block-size" => {
                if let Some(l) = len(v) {
                    st.max_height = l;
                }
            }
            "margin" => {
                if let Some(q) = quad(v, |t| len(t)) {
                    st.margin = q;
                }
            }
            "padding" => {
                if let Some(q) = quad(v, |t| len(t)) {
                    st.padding = q;
                }
            }
            "margin-inline" | "padding-inline" | "margin-block" | "padding-block"
            | "inset-inline" | "inset-block" => {
                let parts = css::tokens(v);
                let a = parts.first().and_then(|t| len(t));
                let b = parts.get(1).and_then(|t| len(t)).or_else(|| a.clone());
                if let (Some(a), Some(b)) = (a, b) {
                    let (i, j) = if prop.ends_with("inline") {
                        (3, 1)
                    } else {
                        (0, 2)
                    };
                    let target = if prop.starts_with("margin") {
                        &mut st.margin
                    } else if prop.starts_with("padding") {
                        &mut st.padding
                    } else {
                        &mut st.inset
                    };
                    target[i] = a;
                    target[j] = b;
                }
            }
            "inset" => {
                if let Some(q) = quad(v, |t| len(t)) {
                    st.inset = q;
                }
            }
            "top" | "right" | "bottom" | "left" | "inset-block-start" | "inset-inline-end"
            | "inset-block-end" | "inset-inline-start" => {
                if let Some(l) = len(v) {
                    st.inset[side(prop)] = l;
                }
            }
            p if (p.starts_with("margin-") || p.starts_with("padding-"))
                && side_opt(p).is_some() =>
            {
                if let (Some(l), Some(i)) = (len(v), side_opt(p)) {
                    if p.starts_with("margin") {
                        st.margin[i] = l;
                    } else {
                        st.padding[i] = l;
                    }
                }
            }
            "border"
            | "border-top"
            | "border-right"
            | "border-bottom"
            | "border-left"
            | "border-inline"
            | "border-block"
            | "border-inline-start"
            | "border-inline-end"
            | "border-block-start"
            | "border-block-end" => {
                let (w, on, c) = border_short(v, u, st.color);
                let sides: &[usize] = match prop {
                    "border" => &[0, 1, 2, 3],
                    "border-inline" => &[1, 3],
                    "border-block" => &[0, 2],
                    p => &[side(p)],
                };
                for &i in sides {
                    st.border[i] = w;
                    st.border_on[i] = on;
                    match c {
                        Some(c) => {
                            st.border_color[i] = c;
                            st.border_current[i] = false;
                        }
                        None => st.border_current[i] = true,
                    }
                }
            }
            "border-width" => {
                if let Some(q) = quad(v, |t| border_width(t, u)) {
                    st.border = q;
                }
            }
            "border-style" => {
                if let Some(q) = quad(v, |t| Some(t.to_ascii_lowercase())) {
                    for (i, s) in q.iter().enumerate() {
                        st.border_on[i] = !(s == "none" || s == "hidden");
                    }
                }
            }
            "border-color" => {
                if let Some(q) = quad(v, |t| color_value(t, st.color)) {
                    st.border_color = q;
                    st.border_current = [lower.contains("currentcolor"); 4];
                }
            }
            p if p.starts_with("border-") && p.ends_with("-width") => {
                if let Some(w) = border_width(v, u) {
                    st.border[side(&p[7..p.len() - 6])] = w;
                }
            }
            p if p.starts_with("border-") && p.ends_with("-color") => {
                if let Some(c) = color_value(v, st.color) {
                    let i = side(&p[7..p.len() - 6]);
                    st.border_color[i] = c;
                    st.border_current[i] = lower == "currentcolor";
                }
            }
            p if p.starts_with("border-") && p.ends_with("-style") => {
                let i = side(&p[7..p.len() - 6]);
                st.border_on[i] = !(lower == "none" || lower == "hidden");
            }
            "border-radius" => {
                if let Some(first) = css::tokens(v).first() {
                    st.radius = css::parse_px(first, u).unwrap_or(0).clamp(0, 200);
                    if first.ends_with('%') {
                        st.radius = 999; // un círculo o una "píldora"
                    }
                }
            }
            "box-sizing" | "-webkit-box-sizing" | "-moz-box-sizing" => {
                st.border_box = lower == "border-box"
            }
            "background-color" if inherit => st.bg = parent.bg,
            "background-color" => {
                if let Some(c) = color_value(v, st.color) {
                    st.bg = Some(c).filter(|c| c.a > 0);
                }
            }
            "background" => {
                st.bg = if lower == "none" || lower == "transparent" {
                    None
                } else {
                    css::color_in(&resolve_current(v, st.color)).filter(|c| c.a > 0)
                };
                st.bg_image = css::url_in(v);
                if st.bg_image.is_some() {
                    self.bg_details(st, v, u);
                }
            }
            "background-image" => st.bg_image = css::url_in(v),
            "background-size" => st.bg_size = bg_size(&lower, u),
            "background-position" => {
                st.bg_pos = bg_pos(&lower, u);
            }
            "background-repeat" => st.bg_repeat = !lower.starts_with("no-repeat"),
            "mask-image" | "-webkit-mask-image" | "mask" | "-webkit-mask" => {
                st.mask_image = css::url_in(v).filter(|_| lower != "none");
                if prop.ends_with("mask") && st.mask_image.is_some() {
                    let mut tmp = st.clone();
                    self.bg_details(&mut tmp, v, u);
                    st.mask_pos = tmp.bg_pos;
                    if tmp.bg_size != BgSize::Auto {
                        st.mask_size = tmp.bg_size;
                    }
                }
            }
            "mask-size" | "-webkit-mask-size" => st.mask_size = bg_size(&lower, u),
            "mask-position" | "-webkit-mask-position" => st.mask_pos = bg_pos(&lower, u),
            "caption-side" => st.caption_bottom = lower == "bottom",
            "color" => {
                if inherit {
                    st.color = parent.color;
                } else if let Some(c) = color_value(v, parent.color) {
                    st.color = c;
                }
            }
            "font-size" => {
                if let Some(px) = css::font_size(&lower, parent.font_size, u) {
                    st.font_size = px;
                    st.font.px = px.round_px();
                }
            }
            "font-weight" => st.font.bold = weight(&lower, parent.font.bold),
            "font-style" => {
                st.font.italic = lower.starts_with("italic") || lower.starts_with("oblique")
            }
            "font-family" => self.family(st, &lower),
            "font" => {
                // `font: italic bold 13px/1.4 Arial, sans-serif`.
                if matches!(lower.as_str(), "inherit" | "initial" | "unset") {
                    return;
                }
                let toks = css::tokens(&lower);
                st.font.bold = false;
                st.font.italic = false;
                st.line_height = LineHeight::Normal;
                for (i, t) in toks.iter().enumerate() {
                    if *t == "bold" || *t == "bolder" || t.parse::<u32>().is_ok_and(|w| w >= 600) {
                        st.font.bold = true;
                    } else if *t == "italic" || *t == "oblique" {
                        st.font.italic = true;
                    } else if let Some(px) = t
                        .split('/')
                        .next()
                        .filter(|s| s.ends_with(|c: char| c.is_alphabetic() || c == '%'))
                        .and_then(|s| css::font_size(s, parent.font_size, u))
                        .filter(|_| {
                            t.starts_with(|c: char| c.is_ascii_digit() || c == '.')
                                || t.contains("small")
                                || t.contains("large")
                                || *t == "medium"
                        })
                    {
                        st.font_size = px;
                        st.font.px = px.round_px();
                        if let Some((_, lh)) = t.split_once('/') {
                            st.line_height = line_height(lh, Units { em: px, ..u });
                        }
                        let family = toks[i + 1..].join(" ");
                        self.family(st, &family);
                        break;
                    }
                }
            }
            "line-height" => {
                st.line_height = if inherit {
                    parent.line_height
                } else {
                    line_height(&lower, u)
                }
            }
            "text-align" | "text-align-last" if prop == "text-align" => {
                st.align = match lower.as_str() {
                    "center" | "-webkit-center" | "-moz-center" => TextAlign::Center,
                    "right" | "-webkit-right" => TextAlign::Right,
                    "end" => TextAlign::Right,
                    "justify" => TextAlign::Justify,
                    "inherit" => parent.align,
                    _ => TextAlign::Left,
                }
            }
            "white-space" | "white-space-collapse" | "text-wrap" | "text-wrap-mode" => {
                st.white_space = match lower.as_str() {
                    "nowrap" => WhiteSpace::NoWrap,
                    "pre" => WhiteSpace::Pre,
                    "pre-wrap" | "break-spaces" | "preserve" => WhiteSpace::PreWrap,
                    "pre-line" | "preserve-breaks" => WhiteSpace::PreLine,
                    "normal" | "wrap" | "collapse" => WhiteSpace::Normal,
                    _ => st.white_space,
                }
            }
            "text-decoration" | "text-decoration-line" => {
                if lower.contains("none") {
                    st.underline = false;
                    st.strike = false;
                }
                if lower.contains("underline") {
                    st.underline = true;
                }
                if lower.contains("line-through") {
                    st.strike = true;
                }
            }
            "text-transform" => {
                st.transform = match lower.as_str() {
                    "uppercase" => Transform::Upper,
                    "lowercase" => Transform::Lower,
                    "capitalize" => Transform::Capitalize,
                    _ => Transform::None,
                }
            }
            "visibility" => st.visible = !(lower == "hidden" || lower == "collapse"),
            "opacity" => {
                st.transparent = lower
                    .trim_end_matches('%')
                    .parse::<f32>()
                    .is_ok_and(|o| o <= 0.05)
            }
            "overflow" | "overflow-x" | "overflow-y" => {
                let clip = matches!(lower.split_whitespace().next(), Some("hidden" | "clip"));
                let scroll = matches!(
                    lower.split_whitespace().next(),
                    Some("auto" | "scroll" | "overlay")
                );
                if prop != "overflow-y" {
                    st.clip_x = clip || scroll;
                }
                if prop != "overflow-x" {
                    st.clip_y = clip;
                    st.scroll_y = scroll;
                }
            }
            "list-style" | "list-style-type" => {
                for t in lower.split_whitespace() {
                    if let Some(ls) = list_style(t) {
                        st.list_style = ls;
                    }
                    if t == "inside" {
                        st.list_inside = true;
                    } else if t == "outside" {
                        st.list_inside = false;
                    }
                }
                if prop == "list-style" && lower.contains("url(") && !lower.contains("none") {
                    // Una imagen como viñeta: se usa el punto.
                    st.list_style = ListStyle::Disc;
                }
            }
            "list-style-position" => st.list_inside = lower == "inside",
            "flex-direction" | "-webkit-flex-direction" => st.flex_dir = flex_dir(&lower),
            "-webkit-box-orient" => {
                st.flex_dir = if lower == "vertical" {
                    FlexDir::Column
                } else {
                    FlexDir::Row
                }
            }
            "flex-wrap" | "-webkit-flex-wrap" => st.flex_wrap = lower.starts_with("wrap"),
            "flex-flow" => {
                for t in lower.split_whitespace() {
                    if t.starts_with("row") || t.starts_with("column") {
                        st.flex_dir = flex_dir(t);
                    } else if t.starts_with("wrap") {
                        st.flex_wrap = true;
                    } else if t == "nowrap" {
                        st.flex_wrap = false;
                    }
                }
            }
            "justify-content" | "-webkit-justify-content" | "-webkit-box-pack" => {
                st.justify = justify(&lower)
            }
            "align-items" | "-webkit-align-items" | "-webkit-box-align" => {
                st.align_items = align(&lower).unwrap_or(st.align_items)
            }
            "align-self" | "-webkit-align-self" => st.align_self = align(&lower),
            "align-content" => st.align_content = justify(&lower),
            "justify-items" => st.justify_items = align(&lower).unwrap_or(Align::Stretch),
            "justify-self" => st.justify_self = align(&lower),
            "place-items" => {
                let mut it = lower.split_whitespace();
                if let Some(a) = it.next().and_then(align) {
                    st.align_items = a;
                    st.justify_items = it.next().and_then(align).unwrap_or(a);
                }
            }
            "place-content" => {
                let mut it = lower.split_whitespace();
                if let Some(a) = it.next() {
                    st.align_content = justify(a);
                    st.justify = justify(it.next().unwrap_or(a));
                }
            }
            "place-self" => {
                let mut it = lower.split_whitespace();
                st.align_self = it.next().and_then(align);
                st.justify_self = it.next().and_then(align).or(st.align_self);
            }
            "flex" | "-webkit-flex" | "-ms-flex" => self.flex_short(st, &lower, u),
            "-webkit-box-flex" => st.grow = lower.parse().unwrap_or(0.0),
            "flex-grow" | "-webkit-flex-grow" => st.grow = lower.parse().unwrap_or(0.0),
            "flex-shrink" | "-webkit-flex-shrink" => st.shrink = lower.parse().unwrap_or(1.0),
            "flex-basis" | "-webkit-flex-basis" => {
                if lower == "content" {
                    st.basis = Len::Auto;
                } else if let Some(l) = len(v) {
                    st.basis = l;
                }
            }
            "order" | "-webkit-order" => st.order = lower.parse().unwrap_or(0),
            "gap" | "grid-gap" => {
                let parts = css::tokens(v);
                if let Some(r) = parts.first().and_then(|t| len(t)) {
                    st.col_gap = parts
                        .get(1)
                        .and_then(|t| len(t))
                        .unwrap_or_else(|| r.clone());
                    st.row_gap = r;
                }
            }
            "row-gap" | "grid-row-gap" => {
                if let Some(l) = len(v) {
                    st.row_gap = l;
                }
            }
            "column-gap" | "grid-column-gap" => {
                if let Some(l) = len(v) {
                    st.col_gap = l;
                }
            }
            "grid-template-columns" => st.grid_cols = track_list(v, u).map(Rc::new),
            "grid-template-rows" => st.grid_rows = track_list(v, u).map(Rc::new),
            "grid-auto-rows" => {
                st.grid_auto_rows = track_list(v, u).and_then(|t| t.tracks.first().cloned())
            }
            "grid-template-areas" => st.grid_areas = areas(v).map(Rc::new),
            "grid-template" => {
                // `grid-template: "a b" "c d" / 200px 1fr`
                let (rows, cols) = v.split_once('/').unwrap_or((v, ""));
                if rows.contains('"') || rows.contains('\'') {
                    st.grid_areas = areas(rows).map(Rc::new);
                } else {
                    st.grid_rows = track_list(rows, u).map(Rc::new);
                }
                if !cols.trim().is_empty() {
                    st.grid_cols = track_list(cols, u).map(Rc::new);
                }
            }
            "grid-auto-flow" => st.grid_flow_column = lower.starts_with("column"),
            "grid-column" => st.grid_col = grid_span(v),
            "grid-row" => st.grid_row = grid_span(v),
            "grid-column-start" => st.grid_col.0 = grid_line(v),
            "grid-column-end" => st.grid_col.1 = grid_line(v),
            "grid-row-start" => st.grid_row.0 = grid_line(v),
            "grid-row-end" => st.grid_row.1 = grid_line(v),
            "grid-area" => {
                let parts: Vec<&str> = v.split('/').map(str::trim).collect();
                if parts.len() == 1
                    && !parts[0].starts_with(|c: char| c.is_ascii_digit() || c == '-')
                    && parts[0] != "auto"
                {
                    st.grid_col = (GridLine::Area(parts[0].to_string()), GridLine::Auto);
                    st.grid_row = (GridLine::Area(parts[0].to_string()), GridLine::Auto);
                } else {
                    st.grid_row.0 = parts.first().map_or(GridLine::Auto, |p| grid_line(p));
                    st.grid_col.0 = parts.get(1).map_or(GridLine::Auto, |p| grid_line(p));
                    st.grid_row.1 = parts.get(2).map_or(GridLine::Auto, |p| grid_line(p));
                    st.grid_col.1 = parts.get(3).map_or(GridLine::Auto, |p| grid_line(p));
                }
            }
            "vertical-align" => {
                st.valign = match lower.as_str() {
                    "top" | "text-top" => VAlign::Top,
                    "middle" => VAlign::Middle,
                    "bottom" | "text-bottom" => VAlign::Bottom,
                    "sub" => VAlign::Sub,
                    "super" => VAlign::Super,
                    "inherit" => parent.valign,
                    _ => VAlign::Baseline,
                }
            }
            "text-indent" => {
                if let Some(l) = len(v) {
                    st.text_indent = l;
                }
            }
            "z-index" => st.z_index = lower.parse().unwrap_or(0),
            "border-collapse" => st.border_collapse = lower == "collapse",
            "border-spacing" => {
                st.border_spacing = css::tokens(v)
                    .first()
                    .and_then(|t| css::parse_px(t, u))
                    .unwrap_or(0)
            }
            "table-layout" => st.table_fixed = lower == "fixed",
            "aspect-ratio" => {
                let r: Vec<f32> = lower
                    .split('/')
                    .filter_map(|p| p.trim().split_whitespace().next()?.parse().ok())
                    .collect();
                st.aspect_ratio = match r.as_slice() {
                    [a, b] if *b > 0.0 => Some(a / b),
                    [a] if *a > 0.0 => Some(*a),
                    _ => None,
                };
            }
            "letter-spacing" => st.letter_spacing = css::parse_px(v, u).unwrap_or(0).clamp(-4, 20),
            "word-break" | "overflow-wrap" | "word-wrap" => {
                st.word_break = matches!(lower.as_str(), "break-all" | "break-word" | "anywhere")
            }
            "clip" | "clip-path" => {
                if lower.contains("rect(0")
                    || lower.contains("rect(1px")
                    || lower.contains("inset(50%")
                    || lower.contains("inset(100%")
                    || lower.starts_with("circle(0")
                {
                    st.transparent = true;
                    st.clip_x = true;
                    st.clip_y = true;
                    st.width = Len::Px(0.0);
                    st.height = Len::Px(0.0);
                }
            }
            "transform" => {
                // `transform: scale(0)` esconde; `translateX(-100%)` saca de la pantalla un menú.
                if lower.contains("scale(0)")
                    || lower.contains("scale(0,")
                    || lower.contains("scalex(0)")
                    || lower.contains("scaley(0)")
                {
                    st.transparent = true;
                }
            }
            "content-visibility" if lower == "hidden" => st.display = Display::None,
            _ => {}
        }
        let _ = n;
    }

    fn family(&self, st: &mut Style, lower: &str) {
        let first = lower
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '\'']);
        st.font.mono = matches!(
            first,
            "monospace"
                | "courier"
                | "courier new"
                | "consolas"
                | "menlo"
                | "monaco"
                | "sfmono-regular"
                | "ui-monospace"
                | "lucida console"
                | "source code pro"
                | "dejavu sans mono"
                | "roboto mono"
                | "fira code"
                | "jetbrains mono"
        ) || first.ends_with(" mono");
        st.icon_font = [
            "icon",
            "material symbols",
            "fontawesome",
            "font awesome",
            "glyphicons",
            "dashicons",
            "genericons",
            "ionicons",
            "remixicon",
            "feather",
            "octicons",
            "codicon",
            "bootstrap-icons",
            "icomoon",
            "wikimedia",
        ]
        .iter()
        .any(|k| first.contains(k));
    }

    fn flex_short(&self, st: &mut Style, lower: &str, u: Units) {
        let toks: Vec<&str> = lower.split_whitespace().collect();
        match toks.as_slice() {
            ["none"] => (st.grow, st.shrink, st.basis) = (0.0, 0.0, Len::Auto),
            ["auto"] => (st.grow, st.shrink, st.basis) = (1.0, 1.0, Len::Auto),
            ["initial"] => (st.grow, st.shrink, st.basis) = (0.0, 1.0, Len::Auto),
            [g] if g.parse::<f32>().is_ok() => {
                (st.grow, st.shrink, st.basis) = (g.parse().unwrap_or(0.0), 1.0, Len::Px(0.0))
            }
            [b] => {
                if let Some(l) = css::parse_len(b, u) {
                    (st.grow, st.shrink, st.basis) = (1.0, 1.0, l);
                }
            }
            [g, s] if s.parse::<f32>().is_ok() => {
                st.grow = g.parse().unwrap_or(0.0);
                st.shrink = s.parse().unwrap_or(1.0);
                st.basis = Len::Px(0.0);
            }
            [g, b] => {
                st.grow = g.parse().unwrap_or(0.0);
                st.shrink = 1.0;
                st.basis = css::parse_len(b, u).unwrap_or(Len::Auto);
            }
            [g, s, b, ..] => {
                st.grow = g.parse().unwrap_or(0.0);
                st.shrink = s.parse().unwrap_or(1.0);
                st.basis = if *b == "content" {
                    Len::Auto
                } else {
                    css::parse_len(b, u).unwrap_or(Len::Auto)
                };
            }
            _ => {}
        }
    }

    fn bg_details(&self, st: &mut Style, v: &str, u: Units) {
        let lower = v.to_ascii_lowercase();
        st.bg_repeat = !lower.contains("no-repeat");
        // Posición y tamaño: `center / cover`, `0 -20px`.
        let without_url = match (lower.find("url("), lower.find(')')) {
            (Some(a), Some(b)) if b > a => [&lower[..a], &lower[b + 1..]].concat(),
            _ => lower.clone(),
        };
        let (pos, size) = without_url.split_once('/').unwrap_or((&without_url, ""));
        let pos_toks: Vec<&str> = pos
            .split_whitespace()
            .filter(|t| {
                t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.')
                    || matches!(*t, "center" | "top" | "bottom" | "left" | "right")
            })
            .collect();
        if !pos_toks.is_empty() {
            st.bg_pos = bg_pos(&pos_toks.join(" "), u);
        }
        if !size.trim().is_empty() {
            let s: Vec<&str> = size.split_whitespace().take(2).collect();
            st.bg_size = bg_size(&s.join(" "), u);
        }
    }
}

trait RoundPx {
    fn round_px(self) -> u16;
}

impl RoundPx for f32 {
    fn round_px(self) -> u16 {
        (self + 0.5).clamp(6.0, 96.0) as u16
    }
}

/// Reemplaza `var(--x, respaldo)` con los valores de `vars`.
pub fn resolve_vars(value: &str, vars: &BTreeMap<String, String>) -> String {
    let mut s = value.to_string();
    for _ in 0..16 {
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
        let v = vars.get(&name).cloned().unwrap_or(fallback);
        s.replace_range(start..(end + 1).min(s.len()), &v);
    }
    s
}

fn resolve_current(v: &str, color: Rgba) -> String {
    if v.to_ascii_lowercase().contains("currentcolor") {
        let hex = alloc::format!("#{:02x}{:02x}{:02x}", color.c.r, color.c.g, color.c.b);
        v.replace("currentColor", &hex)
            .replace("currentcolor", &hex)
    } else {
        v.to_string()
    }
}

fn color_value(v: &str, current: Rgba) -> Option<Rgba> {
    let lower = v.trim().to_ascii_lowercase();
    match lower.as_str() {
        "currentcolor" | "inherit" => Some(current),
        "initial" | "unset" => None,
        // Colores del sistema (los usa el estilo por defecto de los formularios).
        "buttonface" => Some(Rgba::opaque(Color::hex(0xefefef))),
        "buttontext" | "canvastext" | "fieldtext" => Some(Rgba::opaque(Color::BLACK)),
        "canvas" | "field" | "window" => Some(Rgba::opaque(Color::WHITE)),
        "graytext" => Some(Rgba::opaque(Color::hex(0x6d6d6d))),
        "linktext" => Some(Rgba::opaque(Color::hex(0x0000ee))),
        "highlight" => Some(Rgba::opaque(Color::hex(0x3390ff))),
        _ => css::parse_rgba(&lower).or_else(|| css::color_in(&lower)),
    }
}

/// `1px 2px` → arriba, derecha, abajo, izquierda (como `margin`).
fn quad<T: Clone>(v: &str, f: impl Fn(&str) -> Option<T>) -> Option<[T; 4]> {
    let parts: Vec<T> = css::tokens(v)
        .into_iter()
        .map(f)
        .collect::<Option<Vec<_>>>()?;
    Some(match parts.as_slice() {
        [a] => [a.clone(), a.clone(), a.clone(), a.clone()],
        [a, b] => [a.clone(), b.clone(), a.clone(), b.clone()],
        [a, b, c] => [a.clone(), b.clone(), c.clone(), b.clone()],
        [a, b, c, d, ..] => [a.clone(), b.clone(), c.clone(), d.clone()],
        [] => return None,
    })
}

fn side_opt(p: &str) -> Option<usize> {
    let s = p.rsplit_once('-').map_or(p, |x| x.1);
    let full = p.split_once('-').map_or(p, |x| x.1);
    Some(match (full, s) {
        (_, "top") | ("block-start", _) => 0,
        (_, "right") | ("inline-end", _) => 1,
        (_, "bottom") | ("block-end", _) => 2,
        (_, "left") | ("inline-start", _) => 3,
        _ => return None,
    })
}

fn side(p: &str) -> usize {
    side_opt(p).unwrap_or(match p {
        "top" | "block-start" => 0,
        "right" | "inline-end" => 1,
        "bottom" | "block-end" => 2,
        _ => 3,
    })
}

fn border_width(t: &str, u: Units) -> Option<i32> {
    match t.trim().to_ascii_lowercase().as_str() {
        "thin" => Some(1),
        "medium" => Some(3),
        "thick" => Some(5),
        other => css::parse_px(other, u).map(|w| w.clamp(0, 50)),
    }
}

/// `border: 1px solid #ccc` → (ancho, ¿se ve?, color si lo dice).
fn border_short(v: &str, u: Units, current: Rgba) -> (i32, bool, Option<Rgba>) {
    let lower = v.to_ascii_lowercase();
    if lower == "none" || lower == "0" || lower == "hidden" {
        return (3, false, None);
    }
    let mut w = None;
    let mut c = None;
    let mut style = false;
    for t in css::tokens(&lower) {
        if let Some(x) = border_width(t, u).filter(|_| {
            t.starts_with(|ch: char| ch.is_ascii_digit() || ch == '.')
                || matches!(t, "thin" | "medium" | "thick")
        }) {
            w = Some(x);
        } else if matches!(
            t,
            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset"
        ) {
            style = true;
        } else if t == "none" || t == "hidden" {
            return (3, false, None);
        } else if t == "currentcolor" {
            c = None;
        } else if let Some(col) = color_value(t, current) {
            c = Some(col);
        }
    }
    (w.unwrap_or(3), style, c)
}

fn weight(v: &str, parent: bool) -> bool {
    match v {
        "bold" | "bolder" => true,
        "normal" | "lighter" => false,
        "inherit" => parent,
        n => n.parse::<u32>().map_or(parent, |w| w >= 600),
    }
}

fn line_height(v: &str, u: Units) -> LineHeight {
    let v = v.trim();
    if v == "normal" || v.is_empty() {
        return LineHeight::Normal;
    }
    if let Ok(f) = v.parse::<f32>() {
        return LineHeight::Factor(f.clamp(0.5, 4.0));
    }
    match css::parse_len(v, u).and_then(|l| l.resolve(Some(u.em as i32))) {
        Some(px) if px > 0 => LineHeight::Px(px.min(400)),
        _ => LineHeight::Normal,
    }
}

fn list_style(t: &str) -> Option<ListStyle> {
    Some(match t {
        "none" => ListStyle::None,
        "disc" => ListStyle::Disc,
        "circle" => ListStyle::Circle,
        "square" => ListStyle::Square,
        "decimal" | "decimal-leading-zero" => ListStyle::Decimal,
        "lower-alpha" | "lower-latin" => ListStyle::LowerAlpha,
        "upper-alpha" | "upper-latin" => ListStyle::UpperAlpha,
        "lower-roman" => ListStyle::LowerRoman,
        "upper-roman" => ListStyle::UpperRoman,
        _ => return None,
    })
}

fn flex_dir(v: &str) -> FlexDir {
    match v {
        "row-reverse" => FlexDir::RowReverse,
        "column" => FlexDir::Column,
        "column-reverse" => FlexDir::ColumnReverse,
        _ => FlexDir::Row,
    }
}

fn justify(v: &str) -> Justify {
    match v.split_whitespace().last().unwrap_or("") {
        "center" => Justify::Center,
        "flex-end" | "end" | "right" => Justify::End,
        "space-between" | "justify" => Justify::SpaceBetween,
        "space-around" | "distribute" => Justify::SpaceAround,
        "space-evenly" => Justify::SpaceEvenly,
        _ => Justify::Start,
    }
}

fn align(v: &str) -> Option<Align> {
    Some(match v.split_whitespace().last().unwrap_or("") {
        "center" => Align::Center,
        "flex-start" | "start" | "self-start" | "left" | "normal" if v != "normal" => Align::Start,
        "flex-end" | "end" | "self-end" | "right" => Align::End,
        "stretch" | "normal" => Align::Stretch,
        "baseline" | "first" | "last" => Align::Baseline,
        "auto" => return None,
        _ => return None,
    })
}

fn bg_size(v: &str, u: Units) -> BgSize {
    match v.trim() {
        "cover" => BgSize::Cover,
        "contain" => BgSize::Contain,
        "auto" | "" | "auto auto" => BgSize::Auto,
        other => {
            let parts: Vec<&str> = css::tokens(other);
            if parts.first().is_some_and(|p| p.ends_with('%')) {
                let w = parts[0].trim_end_matches('%').parse().unwrap_or(100.0);
                let h = parts
                    .get(1)
                    .and_then(|p| p.trim_end_matches('%').parse().ok());
                return BgSize::Pct(w, h);
            }
            let px = |p: Option<&&str>| p.and_then(|p| css::parse_px(p, u));
            BgSize::Px(px(parts.first()), px(parts.get(1)))
        }
    }
}

fn bg_pos(v: &str, u: Units) -> (Len, Len) {
    let word = |t: &str| -> Option<(Option<bool>, Len)> {
        // (¿es horizontal?, posición)
        Some(match t {
            "left" => (Some(true), Len::Pct(0.0)),
            "right" => (Some(true), Len::Pct(100.0)),
            "top" => (Some(false), Len::Pct(0.0)),
            "bottom" => (Some(false), Len::Pct(100.0)),
            "center" => (None, Len::Pct(50.0)),
            other => (None, css::parse_len(other, u)?),
        })
    };
    let toks: Vec<(Option<bool>, Len)> = v.split_whitespace().filter_map(word).collect();
    match toks.as_slice() {
        [] => (Len::Px(0.0), Len::Px(0.0)),
        [(Some(false), y)] => (Len::Pct(50.0), y.clone()),
        [(_, x)] => (x.clone(), Len::Pct(50.0)),
        [(Some(false), y), (_, x), ..] => (x.clone(), y.clone()),
        [(_, x), (_, y), ..] => (x.clone(), y.clone()),
    }
}

/// `grid-template-columns: 200px repeat(3, 1fr) minmax(0, 1fr)`.
fn track_list(v: &str, u: Units) -> Option<TrackList> {
    let lower = v.trim().to_ascii_lowercase();
    if lower == "none" || lower == "subgrid" || lower.starts_with("masonry") {
        return None;
    }
    let mut list = TrackList {
        tracks: Vec::new(),
        auto_repeat: None,
        auto_at: 0,
    };
    for t in css::tokens(&lower) {
        if t.starts_with('[') {
            continue; // nombres de líneas
        }
        if let Some(inner) = t.strip_prefix("repeat(").and_then(|s| s.strip_suffix(')')) {
            let (count, what) = inner.split_once(',')?;
            let tracks: Vec<Track> = css::tokens(what.trim())
                .into_iter()
                .filter(|x| !x.starts_with('['))
                .filter_map(|x| track(x, u))
                .collect();
            match count.trim() {
                "auto-fill" | "auto-fit" => {
                    list.auto_at = list.tracks.len();
                    list.auto_repeat = Some(tracks);
                }
                n => {
                    let n: usize = n.parse().ok()?;
                    for _ in 0..n.min(64) {
                        list.tracks.extend(tracks.iter().cloned());
                    }
                }
            }
            continue;
        }
        list.tracks.push(track(t, u)?);
    }
    (!list.tracks.is_empty() || list.auto_repeat.is_some()).then_some(list)
}

fn track(t: &str, u: Units) -> Option<Track> {
    if let Some(fr) = t.strip_suffix("fr") {
        return fr.parse().ok().map(Track::Fr);
    }
    if matches!(t, "auto" | "min-content" | "max-content") || t.starts_with("fit-content") {
        return Some(Track::Auto);
    }
    if let Some(inner) = t.strip_prefix("minmax(").and_then(|s| s.strip_suffix(')')) {
        let (a, b) = inner.split_once(',')?;
        let min = css::parse_len(a.trim(), u).unwrap_or(Len::Px(0.0));
        let min = if min.is_auto() { Len::Px(0.0) } else { min };
        let b = b.trim();
        if let Some(fr) = b.strip_suffix("fr") {
            return Some(Track::MinMax(min, fr.parse().unwrap_or(1.0), None));
        }
        let max = css::parse_len(b, u).filter(|l| !l.is_auto());
        return Some(Track::MinMax(min, 0.0, max));
    }
    css::parse_len(t, u).map(|l| {
        if l.is_auto() {
            Track::Auto
        } else {
            Track::Fixed(l)
        }
    })
}

fn areas(v: &str) -> Option<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut rest = v;
    while let Some(start) = rest.find(['"', '\'']) {
        let q = rest.as_bytes()[start] as char;
        let after = &rest[start + 1..];
        let end = after.find(q)?;
        let row: Vec<String> = after[..end].split_whitespace().map(String::from).collect();
        if !row.is_empty() {
            rows.push(row);
        }
        rest = &after[end + 1..];
    }
    (!rows.is_empty()).then_some(rows)
}

fn grid_line(v: &str) -> GridLine {
    let t = v.trim().to_ascii_lowercase();
    if t == "auto" || t.is_empty() {
        return GridLine::Auto;
    }
    if let Some(n) = t.strip_prefix("span") {
        return GridLine::Span(n.trim().parse().unwrap_or(1).clamp(1, 64));
    }
    match t.parse::<i32>() {
        Ok(n) if n != 0 => GridLine::Line(n),
        _ => GridLine::Area(v.trim().to_string()),
    }
}

fn grid_span(v: &str) -> (GridLine, GridLine) {
    match v.split_once('/') {
        Some((a, b)) => (grid_line(a), grid_line(b)),
        None => (grid_line(v), GridLine::Auto),
    }
}

/// Cómo se ve el texto de un nodo: el estilo del padre, o el propio si es un elemento.
pub fn text_style<'a>(styled: &'a Styled, dom: &Dom, n: usize) -> Option<&'a Style> {
    styled
        .get(n)
        .or_else(|| dom.nodes[n].parent.and_then(|p| styled.get(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(html: &str, css: &str) -> (Dom, Styled) {
        let dom = super::super::dom::parse(html);
        let mut ua = Sheet::new(Media::default());
        ua.add(UA_CSS);
        let mut author = Sheet::new(Media::default());
        author.add(css);
        let s = compute(
            &dom,
            &ua,
            &author,
            Options {
                reader: false,
                images: true,
            },
            Media::default(),
        );
        (dom, s)
    }

    fn by_id<'a>(dom: &Dom, s: &'a Styled, id: &str) -> &'a Style {
        let n = (0..dom.nodes.len())
            .find(|&i| dom.nodes[i].attr("id") == Some(id))
            .unwrap();
        s.get(n).unwrap()
    }

    #[test]
    fn herencia_variables_y_cajas() {
        let (dom, s) = styles(
            "<body><div id=a class=caja><p id=b>x</p><span id=c>y</span></div></body>",
            ":root{--acento:#1a73e8;--esp:12px} .caja{color:var(--acento);padding:var(--esp) 4px;\
             margin:0 auto;border:2px solid;font:italic bold 20px/1.5 Arial;--local:red}\
             p{background:var(--local)} span{display:flex;flex:1 0 200px;gap:4px 8px}",
        );
        let a = by_id(&dom, &s, "a");
        assert_eq!(a.color.c, Color::hex(0x1a73e8));
        assert_eq!(a.padding[0], Len::Px(12.0));
        assert_eq!(a.padding[1], Len::Px(4.0));
        assert!(a.margin[1].is_auto());
        assert_eq!(a.border_widths(), [2; 4]);
        assert_eq!(a.border_color[0].c, Color::hex(0x1a73e8), "currentColor");
        assert!(a.font.bold && a.font.italic);
        assert_eq!(a.font.px, 20);
        assert_eq!(a.line_height, LineHeight::Factor(1.5));
        let b = by_id(&dom, &s, "b");
        assert_eq!(b.color.c, Color::hex(0x1a73e8), "hereda el color");
        assert_eq!(b.display, Display::Block);
        assert_eq!(
            b.margin[0],
            Len::Px(20.0),
            "1em del UA con la letra heredada"
        );
        assert_eq!(
            b.bg.map(|c| c.c),
            Some(Color::hex(0xff0000)),
            "variable heredada"
        );
        let c = by_id(&dom, &s, "c");
        assert_eq!(c.display, Display::Flex);
        assert_eq!((c.grow, c.shrink), (1.0, 0.0));
        assert_eq!(c.basis, Len::Px(200.0));
        assert_eq!(
            (c.row_gap.clone(), c.col_gap.clone()),
            (Len::Px(4.0), Len::Px(8.0))
        );
    }

    #[test]
    fn grillas_y_posiciones() {
        let (dom, s) = styles(
            "<div id=g><i id=h>a</i></div><img id=f src=x style='float:left'>",
            "#g{display:grid;grid-template-columns:200px repeat(2, minmax(0, 1fr));\
             grid-template-areas:'a b' 'c d'} #h{grid-column:1 / -1;position:absolute;top:0}",
        );
        let g = by_id(&dom, &s, "g");
        let cols = g.grid_cols.as_ref().unwrap();
        assert_eq!(cols.tracks.len(), 3);
        assert_eq!(cols.tracks[1], Track::MinMax(Len::Px(0.0), 1.0, None));
        assert_eq!(g.grid_areas.as_ref().unwrap().len(), 2);
        let h = by_id(&dom, &s, "h");
        assert_eq!(h.grid_col, (GridLine::Line(1), GridLine::Line(-1)));
        assert_eq!(h.display, Display::Block, "absoluto es bloque");
        let f = by_id(&dom, &s, "f");
        assert_eq!(f.float, Float::Left);
        assert_eq!(f.display, Display::Block);
    }
}
