//! Maquetación: árbol con estilos → lista de cosas para dibujar ([`Paint`]) con su posición.
//!
//! Es el "motor de layout" de un navegador, en chico. Sigue el modelo de cajas de CSS:
//!
//! - **Flujo normal**: los bloques van uno debajo del otro, con márgenes (los verticales entre
//!   hermanos se "colapsan": gana el más grande), bordes, relleno, anchos fijos o en %,
//!   `max-width` y centrado con `margin: auto`.
//! - **Texto**: los renglones se arman palabra por palabra con los anchos reales de la fuente, y
//!   las palabras, imágenes y cajas en línea (`inline-block`) se alinean por su línea de base.
//! - **Flotantes** (`float`): se corren a un costado y el texto los rodea.
//! - **Flex** (`display: flex`): filas y columnas, `flex-grow`/`shrink`/`basis`, `wrap`,
//!   `justify-content`, `align-items`, `gap` y márgenes automáticos.
//! - **Grid**: columnas fijas, `fr`, `minmax()`, `repeat(auto-fill, …)`, áreas con nombre y
//!   elementos que ocupan varias celdas.
//! - **Tablas**: anchos de columna según el contenido, `colspan`, `rowspan`.
//! - **Posiciones**: `relative`, `absolute` (contra el ancestro posicionado) y `fixed` (contra la
//!   ventana, en la parte de arriba de la página). Lo que se esconde fuera de la pantalla
//!   (`left: -9999px`) no se dibuja.
//! - `overflow: hidden` recorta; `visibility: hidden` y `opacity: 0` ocupan lugar sin verse.
//!
//! Cada caja se arma con su esquina en (0, 0) cuando todavía no se sabe dónde va (una palabra en
//! un renglón, un elemento de una fila flex) y después se corre: así no hay que armarla dos veces.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::mem;

use jarvis_gfx::webfont::{FontSpec, WebFonts};
use jarvis_gfx::{Color, Rect};

use super::css::{Len, Rgba};
use super::dom::NodeKind;
use super::html::{FieldKind, NONE, Prepared};
use super::style::{
    Align, BgSize, Clear, Display, FlexDir, Float, GridLine, Justify, ListStyle, Position, Style,
    TextAlign, Track, Transform, VAlign, WhiteSpace,
};

/// Algo para dibujar, en coordenadas de la página (0, 0 = arriba a la izquierda).
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    /// Lugar reservado para un fondo o borde que no hace falta.
    None,
    Rect {
        r: Rect,
        color: Rgba,
        radius: i32,
        /// Elemento que la pintó (para depurar).
        node: u32,
    },
    Border {
        r: Rect,
        w: [i32; 4],
        c: [Rgba; 4],
        radius: i32,
        node: u32,
    },
    /// Un ícono hecho con `mask-image`: la forma de la imagen pintada con `color`.
    Mask {
        bx: Rect,
        dest: Rect,
        img: u32,
        color: Rgba,
    },
    Text {
        x: i32,
        /// Línea de base.
        base: i32,
        /// Renglón (para tocar con el mouse).
        top: i32,
        h: i32,
        w: i32,
        text: String,
        font: FontSpec,
        color: Rgba,
        underline: bool,
        strike: bool,
        link: u32,
    },
    Image {
        r: Rect,
        img: u32,
        link: u32,
    },
    /// Imagen de fondo: se dibuja `dest` (y sus repeticiones) recortado a `bx`.
    BgImage {
        bx: Rect,
        dest: Rect,
        img: u32,
        repeat: bool,
    },
    /// Un campo de formulario (el navegador dibuja su valor, que cambia al escribir).
    Field {
        r: Rect,
        field: u32,
        font: FontSpec,
        color: Rgba,
    },
    /// Un lugar que se puede tocar (un botón, un enlace que es una caja).
    Hit {
        r: Rect,
        link: u32,
        field: u32,
    },
    ClipPush(Rect),
    ClipPop,
    /// Texto alternativo de una imagen que no está (o no se pudo leer).
    Placeholder {
        r: Rect,
        label: String,
    },
}

impl Paint {
    fn shift(&mut self, dx: i32, dy: i32) {
        let mv = |r: &mut Rect| {
            r.x += dx;
            r.y += dy;
        };
        match self {
            Paint::None | Paint::ClipPop => {}
            Paint::Rect { r, .. }
            | Paint::Border { r, .. }
            | Paint::Image { r, .. }
            | Paint::Field { r, .. }
            | Paint::Hit { r, .. }
            | Paint::ClipPush(r)
            | Paint::Placeholder { r, .. } => mv(r),
            Paint::BgImage { bx, dest, .. } | Paint::Mask { bx, dest, .. } => {
                mv(bx);
                mv(dest);
            }
            Paint::Text { x, base, top, .. } => {
                *x += dx;
                *base += dy;
                *top += dy;
            }
        }
    }

    /// Hasta dónde llega hacia abajo.
    pub fn bottom(&self) -> i32 {
        match self {
            Paint::None | Paint::ClipPop | Paint::ClipPush(_) | Paint::Hit { .. } => 0,
            Paint::Rect { r, .. }
            | Paint::Border { r, .. }
            | Paint::Image { r, .. }
            | Paint::Field { r, .. }
            | Paint::Placeholder { r, .. } => r.y + r.h,
            Paint::BgImage { bx, .. } | Paint::Mask { bx, .. } => bx.y + bx.h,
            Paint::Text { top, h, .. } => top + h,
        }
    }
}

/// La página armada.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub paints: Vec<Paint>,
    pub width: i32,
    pub height: i32,
}

impl Page {
    /// Todo el texto visible (para los tests y para buscar en la página).
    pub fn text(&self) -> String {
        let mut out = String::new();
        let mut last_base = i32::MIN;
        for p in &self.paints {
            if let Paint::Text { text, base, .. } = p {
                if !out.is_empty() {
                    out.push(if *base != last_base { '\n' } else { ' ' });
                }
                out.push_str(text);
                last_base = *base;
            }
        }
        out
    }
}

/// Lo que la maquetación necesita del entorno.
pub struct Env<'a> {
    pub fonts: &'a WebFonts,
    /// Tamaño natural de cada imagen del documento (`None` si todavía no llegó).
    pub image_sizes: &'a [Option<(i32, i32)>],
    /// Ancho y alto de la ventana (la zona de la página).
    pub width: i32,
    pub height: i32,
    /// Colores del tema oscuro en vez de los de la página.
    pub dark: bool,
}

const DARK_TEXT: Color = Color::hex(0xdce3f0);
const DARK_LINK: Color = Color::hex(0x5cc8ff);
const DARK_RIM: Color = Color::hex(0x1d3a57);
const MAX_DEPTH: u32 = 160;

#[derive(Clone, Copy, Debug, Default)]
struct Edges {
    m: [i32; 4],
    auto_l: bool,
    auto_r: bool,
    auto_t: bool,
    auto_b: bool,
    p: [i32; 4],
    b: [i32; 4],
}

impl Edges {
    fn pb_h(&self) -> i32 {
        self.p[1] + self.p[3] + self.b[1] + self.b[3]
    }
    fn pb_v(&self) -> i32 {
        self.p[0] + self.p[2] + self.b[0] + self.b[2]
    }
    fn m_h(&self) -> i32 {
        self.m[1] + self.m[3]
    }
    fn m_v(&self) -> i32 {
        self.m[0] + self.m[2]
    }
}

fn edges(st: &Style, cbw: i32) -> Edges {
    let r = |l: &Len| l.resolve(Some(cbw)).unwrap_or(0);
    let b = st.border_widths();
    Edges {
        m: [
            r(&st.margin[0]),
            r(&st.margin[1]),
            r(&st.margin[2]),
            r(&st.margin[3]),
        ],
        auto_t: st.margin[0].is_auto(),
        auto_r: st.margin[1].is_auto(),
        auto_b: st.margin[2].is_auto(),
        auto_l: st.margin[3].is_auto(),
        p: [
            r(&st.padding[0]).max(0),
            r(&st.padding[1]).max(0),
            r(&st.padding[2]).max(0),
            r(&st.padding[3]).max(0),
        ],
        b,
    }
}

/// Ancho de caja (con borde) de algo que pide `len`.
fn to_border_box(st: &Style, e: &Edges, w: i32) -> i32 {
    if st.border_box {
        w.max(e.pb_h())
    } else {
        w + e.pb_h()
    }
}

fn to_border_box_v(st: &Style, e: &Edges, h: i32) -> i32 {
    if st.border_box {
        h.max(e.pb_v())
    } else {
        h + e.pb_v()
    }
}

/// Los flotantes de un "contexto de formato": rectángulos que los renglones esquivan.
#[derive(Default)]
struct Floats {
    list: Vec<(Rect, bool)>,
}

impl Floats {
    /// El tramo libre entre `x0` y `x1` a la altura `y..y+h`.
    fn band(&self, y: i32, h: i32, x0: i32, x1: i32) -> (i32, i32) {
        let (mut l, mut r) = (x0, x1);
        for (f, left) in &self.list {
            if f.y < y + h.max(1) && f.y + f.h > y {
                if *left {
                    l = l.max(f.x + f.w);
                } else {
                    r = r.min(f.x);
                }
            }
        }
        (l, r.max(l))
    }

    fn clear(&self, c: Clear) -> i32 {
        self.list
            .iter()
            .filter(|(_, left)| match c {
                Clear::Left => *left,
                Clear::Right => !*left,
                Clear::Both => true,
                Clear::None => false,
            })
            .map(|(f, _)| f.y + f.h)
            .max()
            .unwrap_or(i32::MIN)
    }

    fn bottom(&self) -> i32 {
        self.list
            .iter()
            .map(|(f, _)| f.y + f.h)
            .max()
            .unwrap_or(i32::MIN)
    }

    /// Dónde entra un flotante de `w`×`h` a partir de `y`.
    fn place(&self, w: i32, h: i32, left: bool, mut y: i32, x0: i32, x1: i32) -> (i32, i32) {
        for _ in 0..64 {
            let (l, r) = self.band(y, h, x0, x1);
            if r - l >= w || (l == x0 && r == x1) {
                return (if left { l } else { r - w }, y);
            }
            // Más abajo: donde termina el primer flotante que estorba.
            let next = self
                .list
                .iter()
                .map(|(f, _)| f.y + f.h)
                .filter(|&b| b > y)
                .min();
            match next {
                Some(b) => y = b,
                None => break,
            }
        }
        (if left { x0 } else { x1 - w }, y)
    }
}

/// Una caja ya armada.
#[derive(Clone, Copy, Debug, Default)]
struct Out {
    /// Alto de la caja con borde.
    h: i32,
    /// Línea de base del primer y del último renglón (en coordenadas absolutas).
    first: Option<i32>,
    last: Option<i32>,
    /// Lugares del fondo y borde (para estirarlos después).
    deco: Option<usize>,
}

struct Pending {
    node: usize,
    sx: i32,
    sy: i32,
}

/// Lo que se va poniendo en un renglón.
enum Piece {
    Word {
        text: String,
        w: i32,
        font: FontSpec,
        color: Rgba,
        underline: bool,
        strike: bool,
        link: u32,
        shift: i32,
        /// Nodo del estilo (para el alto del renglón).
        sn: usize,
        hidden: bool,
        /// Ancho del espacio que le sigue (0 = pegado a lo que viene).
        space: i32,
        /// ¿Se puede cortar el renglón después?
        brk: bool,
    },
    Atomic {
        buf: Vec<Paint>,
        /// Caja con márgenes.
        w: i32,
        h: i32,
        /// Desde arriba de la caja hasta su línea de base.
        base: i32,
        valign: VAlign,
        link: u32,
        space: i32,
        brk: bool,
    },
    Open(usize, i32),
    Close(usize, i32),
    Break,
    Float(usize),
    Abs(usize),
}

impl Piece {
    fn width(&self) -> i32 {
        match self {
            Piece::Word { w, .. } | Piece::Atomic { w, .. } => *w,
            Piece::Open(_, w) | Piece::Close(_, w) => *w,
            _ => 0,
        }
    }

    fn space(&self) -> i32 {
        match self {
            Piece::Word { space, .. } | Piece::Atomic { space, .. } => *space,
            _ => 0,
        }
    }

    fn breakable(&self) -> bool {
        match self {
            Piece::Word { brk, space, .. } => *brk && *space > 0,
            Piece::Atomic { brk, .. } => *brk,
            _ => false,
        }
    }

    fn set_space(&mut self, s: i32, can_break: bool) -> bool {
        match self {
            Piece::Word { space, brk, .. } | Piece::Atomic { space, brk, .. } => {
                if *space == 0 {
                    *space = s;
                    *brk = can_break;
                }
                true
            }
            _ => false,
        }
    }
}

/// Una celda de tabla: un elemento, o una anónima que junta lo suelto.
#[derive(Clone)]
enum TCell {
    Node(usize),
    Anon(Vec<usize>),
}

/// Algo dentro de un contenedor flex o grid.
#[derive(Clone)]
enum Kid {
    El(usize),
    /// Texto suelto: una caja anónima.
    Text(Vec<usize>),
}

struct L<'a> {
    p: &'a Prepared,
    env: &'a Env<'a>,
    paints: Vec<Paint>,
    intrinsic: Vec<Option<(i32, i32)>>,
    abs: Vec<Vec<Pending>>,
    fixed: Vec<Pending>,
    /// Mayor que cero adentro de algo invisible (se arma pero no se dibuja).
    hidden: u32,
    depth: u32,
    canvas_node: Option<usize>,
}

/// Arma la página.
pub fn layout(p: &Prepared, env: &Env<'_>) -> Page {
    let mut l = L {
        p,
        env,
        paints: Vec::new(),
        intrinsic: alloc::vec![None; p.dom.nodes.len()],
        abs: alloc::vec![Vec::new()],
        fixed: Vec::new(),
        hidden: 0,
        depth: 0,
        canvas_node: None,
    };
    // El fondo de <body> es el de toda la página si <html> no tiene uno.
    let html = p.dom.nodes[0]
        .children
        .iter()
        .copied()
        .find(|&c| p.styled.get(c).is_some());
    let mut height = 0;
    if let Some(h) = html {
        let body = p.dom.nodes[h]
            .children
            .iter()
            .copied()
            .find(|&c| p.dom.nodes[c].name() == "body");
        l.canvas_node = if p.styled.get(h).is_some_and(|s| s.bg.is_some()) {
            Some(h)
        } else {
            body
        };
        let st = l.sty(h);
        if st.display != Display::None {
            let e = edges(st, env.width);
            let bw = (env.width - e.m_h()).max(0);
            let mut floats = Floats::default();
            let out = l.layout_box(h, e.m[3], e.m[0], bw, Some(env.height), &mut floats);
            height = out.h + e.m_v();
        }
    }
    let root = l.abs.pop().unwrap_or_default();
    let fixed = mem::take(&mut l.fixed);
    let cb = Rect::new(0, 0, env.width, env.height.max(height));
    let view = Rect::new(0, 0, env.width, env.height);
    for pnd in root {
        l.layout_abs(&pnd, cb);
    }
    for pnd in fixed {
        l.layout_abs(&pnd, view);
    }
    let bottom = l.paints.iter().map(Paint::bottom).max().unwrap_or(0);
    Page {
        paints: l.paints,
        width: env.width,
        height: height.max(bottom),
    }
}

fn is_replaced(name: &str) -> bool {
    matches!(
        name,
        "img"
            | "input"
            | "select"
            | "textarea"
            | "svg"
            | "iframe"
            | "video"
            | "canvas"
            | "embed"
            | "object"
            | "image"
            | "audio"
            | "progress"
            | "meter"
    )
}

fn is_blank(t: &str) -> bool {
    t.bytes().all(|b| b.is_ascii_whitespace())
}

fn roman(mut n: u32, upper: bool) -> String {
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, t) in table {
        while n >= v {
            s.push_str(t);
            n -= v;
        }
    }
    if upper { s.to_uppercase() } else { s }
}

fn marker_text(ls: ListStyle, n: u32) -> String {
    match ls {
        ListStyle::None => String::new(),
        ListStyle::Disc => "•".into(),
        ListStyle::Circle => "◦".into(),
        ListStyle::Square => "▪".into(),
        ListStyle::Decimal => alloc::format!("{n}."),
        ListStyle::LowerAlpha | ListStyle::UpperAlpha => {
            let base = if ls == ListStyle::LowerAlpha {
                b'a'
            } else {
                b'A'
            };
            let c = (base + ((n.max(1) - 1) % 26) as u8) as char;
            alloc::format!("{c}.")
        }
        ListStyle::LowerRoman => alloc::format!("{}.", roman(n.max(1), false)),
        ListStyle::UpperRoman => alloc::format!("{}.", roman(n.max(1), true)),
    }
}

fn apply_transform(t: &str, tr: Transform, word_start: &mut bool) -> String {
    match tr {
        Transform::None => t.to_string(),
        Transform::Upper => t.to_uppercase(),
        Transform::Lower => t.to_lowercase(),
        Transform::Capitalize => {
            let mut out = String::with_capacity(t.len());
            for c in t.chars() {
                if *word_start && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    *word_start = false;
                } else {
                    if c.is_whitespace() {
                        *word_start = true;
                    }
                    out.push(c);
                }
            }
            out
        }
    }
}

impl<'a> L<'a> {
    fn sty(&self, n: usize) -> &'a Style {
        let p: &'a Prepared = self.p;
        match p.styled.get(n) {
            Some(s) => s,
            None => {
                // Un texto: el estilo del padre.
                let parent = p.dom.nodes[n].parent.unwrap_or(0);
                p.styled.get(parent).unwrap_or_else(|| static_root())
            }
        }
    }

    fn name(&self, n: usize) -> &'a str {
        let p: &'a Prepared = self.p;
        p.dom.nodes[n].name()
    }

    fn is_text(&self, n: usize) -> bool {
        matches!(self.p.dom.nodes[n].kind, NodeKind::Text(_))
    }

    fn text_of(&self, n: usize) -> &'a str {
        let p: &'a Prepared = self.p;
        match &p.dom.nodes[n].kind {
            NodeKind::Text(t) => t,
            _ => "",
        }
    }

    /// Los hijos que se ven (con `display: contents` abierto).
    fn kids(&self, n: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.kids_into(n, &mut out, 0);
        out
    }

    fn kids_into(&self, n: usize, out: &mut Vec<usize>, depth: u32) {
        for &c in &self.p.dom.nodes[n].children {
            match &self.p.dom.nodes[c].kind {
                NodeKind::Text(_) => out.push(c),
                NodeKind::Element { .. } => match self.p.styled.get(c) {
                    Some(s) if s.display == Display::None => {}
                    Some(s) if s.display == Display::Contents && depth < 32 => {
                        self.kids_into(c, out, depth + 1)
                    }
                    Some(_) => out.push(c),
                    None => {}
                },
                NodeKind::Document => {}
            }
        }
    }

    fn visible(&self, st: &Style) -> bool {
        self.hidden == 0 && st.visible
    }

    fn color(&self, c: Rgba, link: bool) -> Rgba {
        if self.env.dark {
            Rgba::opaque(if link { DARK_LINK } else { DARK_TEXT })
        } else {
            c
        }
    }

    fn text_w(&self, t: &str, st: &Style) -> i32 {
        let w = self.env.fonts.width(t, st.font);
        if st.letter_spacing != 0 {
            w + st.letter_spacing * t.chars().count() as i32
        } else {
            w
        }
    }

    fn natural(&self, img: usize) -> Option<(i32, i32)> {
        self.env
            .image_sizes
            .get(img)
            .copied()
            .flatten()
            .filter(|(w, h)| *w > 0 && *h > 0)
    }

    // --- cajas -------------------------------------------------------------------------------

    fn is_block_level(&self, n: usize) -> bool {
        if self.is_text(n) {
            return false;
        }
        let st = self.sty(n);
        !st.display.is_inline_level() && st.display != Display::Contents
    }

    /// Un elemento "en línea" que tiene bloques adentro se trata como si no tuviera caja (sus
    /// hijos van en el flujo del padre): pasa con `<a><div>tarjeta</div></a>`.
    fn inline_with_blocks(&self, n: usize) -> bool {
        if self.is_text(n) || self.sty(n).display != Display::Inline || is_replaced(self.name(n)) {
            return false;
        }
        self.kids(n).iter().any(|&c| {
            !self.is_text(c)
                && (self.is_block_level(c)
                    && self.sty(c).in_flow()
                    && self.sty(c).float == Float::None
                    || self.inline_with_blocks(c))
        })
    }

    /// Ancho con borde y márgenes horizontales de un bloque en un contenedor de `cbw`.
    fn block_width(&mut self, n: usize, cbw: i32, fill: bool) -> (i32, i32, i32) {
        let st = self.sty(n);
        let e = edges(st, cbw);
        let spec = st
            .width
            .resolve(Some(cbw))
            .map(|w| to_border_box(st, &e, w));
        let replaced = is_replaced(self.name(n));
        let mut bw = match spec {
            Some(w) => w,
            None if replaced => self.replaced_size(n, Some(cbw)).0 + e.pb_h(),
            None if fill => cbw - e.m_h(),
            None => {
                let (mn, mx) = self.intrinsic(n);
                mx.min(cbw - e.m_h()).max(mn)
            }
        };
        let mut limited = spec.is_some() || replaced || !fill;
        if let Some(mx) = st
            .max_width
            .resolve(Some(cbw))
            .map(|w| to_border_box(st, &e, w))
            && bw > mx
        {
            bw = mx;
            limited = true;
        }
        if let Some(mn) = st
            .min_width
            .resolve(Some(cbw))
            .map(|w| to_border_box(st, &e, w))
        {
            bw = bw.max(mn);
        }
        bw = bw.max(e.pb_h());
        let (mut ml, mut mr) = (e.m[3], e.m[1]);
        if limited && (e.auto_l || e.auto_r) {
            let fixed = if e.auto_l { 0 } else { ml } + if e.auto_r { 0 } else { mr };
            let free = (cbw - bw - fixed).max(0);
            match (e.auto_l, e.auto_r) {
                (true, true) => {
                    ml = free / 2;
                    mr = free - ml;
                }
                (true, false) => ml = free,
                _ => mr = free,
            }
        }
        (bw, ml, mr)
    }

    /// Pone los lugares para el fondo, la imagen de fondo y el borde.
    fn deco_slots(&mut self, n: usize, st: &Style) -> Option<usize> {
        let has_bg = st.bg.is_some() && Some(n) != self.canvas_node && !self.env.dark;
        let has_img =
            self.p.info.bg_image_of(n).is_some() || self.p.info.mask_image_of(n).is_some();
        let has_border = st.border_widths().iter().any(|&w| w > 0);
        if !(has_bg || has_img || has_border) || !self.visible(st) {
            return None;
        }
        let at = self.paints.len();
        self.paints.extend([Paint::None, Paint::None, Paint::None]);
        Some(at)
    }

    fn fill_deco(&self, paints: &mut [Paint], at: usize, n: usize, r: Rect) {
        let st = self.sty(n);
        if at + 3 > paints.len() {
            return;
        }
        let masked = st.mask_image.is_some();
        paints[at] = match st.bg {
            Some(c) if Some(n) != self.canvas_node && !self.env.dark && !masked => Paint::Rect {
                r,
                color: c,
                radius: st.radius.min(r.h / 2).min(r.w / 2),
                node: n as u32,
            },
            _ => Paint::None,
        };
        if masked {
            // Un ícono: se pinta la forma de la máscara con el color del fondo (o del texto).
            let color = st.bg.unwrap_or(st.color);
            paints[at] = match self
                .p
                .info
                .mask_image_of(n)
                .and_then(|i| Some((i, self.natural(i)?)))
            {
                Some((img, nat)) => Paint::Mask {
                    bx: r,
                    dest: self.dest_rect(st.mask_size, &st.mask_pos, r, nat),
                    img: img as u32,
                    color: self.color(color, false),
                },
                None => Paint::None,
            };
        }
        paints[at + 1] = match self.p.info.bg_image_of(n) {
            Some(img) => match self.natural(img) {
                Some(nat) => self.bg_dest(st, r, nat, img),
                None => Paint::None,
            },
            None => Paint::None,
        };
        let w = st.border_widths();
        paints[at + 2] = if w.iter().any(|&x| x > 0) {
            let c = if self.env.dark {
                [Rgba::opaque(DARK_RIM); 4]
            } else {
                st.border_color
            };
            Paint::Border {
                r,
                w,
                c,
                radius: st.radius.min(r.h / 2).min(r.w / 2),
                node: n as u32,
            }
        } else {
            Paint::None
        };
    }

    fn bg_dest(&self, st: &Style, bx: Rect, nat: (i32, i32), img: usize) -> Paint {
        Paint::BgImage {
            bx,
            dest: self.dest_rect(st.bg_size, &st.bg_pos, bx, nat),
            img: img as u32,
            repeat: st.bg_repeat,
        }
    }

    /// Dónde va una imagen de fondo (o una máscara) dentro de la caja `bx`.
    fn dest_rect(&self, size: BgSize, pos: &(Len, Len), bx: Rect, (nw, nh): (i32, i32)) -> Rect {
        let (w, h) = match size {
            BgSize::Auto => (nw, nh),
            BgSize::Cover | BgSize::Contain => {
                let sx = bx.w as i64 * 1000 / nw as i64;
                let sy = bx.h as i64 * 1000 / nh as i64;
                let s = if size == BgSize::Cover {
                    sx.max(sy)
                } else {
                    sx.min(sy)
                };
                ((nw as i64 * s / 1000) as i32, (nh as i64 * s / 1000) as i32)
            }
            BgSize::Px(w, h) => match (w, h) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => (w, w * nh / nw),
                (None, Some(h)) => (h * nw / nh, h),
                (None, None) => (nw, nh),
            },
            BgSize::Pct(pw, ph) => {
                let w = (bx.w as f32 * pw / 100.0) as i32;
                let h = ph.map_or(w * nh / nw.max(1), |p| (bx.h as f32 * p / 100.0) as i32);
                (w, h)
            }
        };
        let x = pos.0.resolve(Some(bx.w - w)).unwrap_or(0);
        let y = pos.1.resolve(Some(bx.h - h)).unwrap_or(0);
        Rect::new(bx.x + x, bx.y + y, w.max(1), h.max(1))
    }

    /// Arma la caja `n` con la esquina del borde en (`x`, `y`) y ancho `bw`.
    fn layout_box(
        &mut self,
        n: usize,
        x: i32,
        y: i32,
        bw: i32,
        cb_h: Option<i32>,
        floats: &mut Floats,
    ) -> Out {
        let st = self.sty(n);
        if self.depth > MAX_DEPTH {
            return Out::default();
        }
        self.depth += 1;
        let invisible = st.transparent;
        if invisible {
            self.hidden += 1;
        }
        let e = edges(st, bw);
        let cx = x + e.b[3] + e.p[3];
        let cy = y + e.b[0] + e.p[0];
        let cw = (bw - e.pb_h()).max(0);
        let spec_h = st
            .height
            .resolve(cb_h)
            .map(|h| (to_border_box_v(st, &e, h) - e.pb_v()).max(0));
        let start = self.paints.len();
        let deco = self.deco_slots(n, st);
        let clip = st.clip_x || st.clip_y;
        let clip_at = if clip {
            self.paints.push(Paint::ClipPush(Rect::new(0, 0, 0, 0)));
            Some(self.paints.len() - 1)
        } else {
            None
        };
        let positioned = st.position != Position::Static;
        if positioned {
            self.abs.push(Vec::new());
        }
        let mut own = Floats::default();
        let fl = if st.is_bfc() { &mut own } else { floats };
        let name = self.name(n);
        let (content_h, first, last) = if is_replaced(name) {
            let (_, h) = self.replaced_content(n, cx, cy, cw, cb_h);
            (h, Some(cy + h), Some(cy + h))
        } else {
            match st.display {
                Display::Flex | Display::InlineFlex => {
                    let h = self.layout_flex(n, cx, cy, cw, spec_h);
                    (h, None, None)
                }
                Display::Grid | Display::InlineGrid => {
                    let h = self.layout_grid(n, cx, cy, cw, spec_h);
                    (h, None, None)
                }
                Display::Table | Display::InlineTable => {
                    let kids = self.kids(n);
                    let h = self.layout_table(n, &kids, cx, cy, cw);
                    (h, None, None)
                }
                _ => {
                    let o = self.layout_flow(n, cx, cy, cw, spec_h, fl);
                    (o.h, o.first, o.last)
                }
            }
        };
        let mut h = match spec_h {
            Some(sh) if st.scroll_y => sh.max(content_h),
            Some(sh) => sh,
            None => content_h,
        };
        if st.is_bfc() {
            let fb = own.bottom();
            if fb > cy + h {
                h = fb - cy;
            }
        }
        if let Some(mx) = st
            .max_height
            .resolve(cb_h)
            .map(|v| to_border_box_v(st, &e, v) - e.pb_v())
            && h > mx
            && !st.scroll_y
        {
            h = mx.max(0);
        }
        if let Some(mn) = st
            .min_height
            .resolve(cb_h)
            .map(|v| to_border_box_v(st, &e, v) - e.pb_v())
        {
            h = h.max(mn);
        }
        let bh = h + e.pb_v();
        let r = Rect::new(x, y, bw, bh);
        if let Some(at) = deco {
            let mut paints = mem::take(&mut self.paints);
            self.fill_deco(&mut paints, at, n, r);
            self.paints = paints;
        }
        if let Some(ci) = clip_at {
            let pad = Rect::new(
                x + e.b[3],
                y + e.b[0],
                bw - e.b[1] - e.b[3],
                bh - e.b[0] - e.b[2],
            );
            self.paints[ci] = Paint::ClipPush(if st.clip_y {
                pad
            } else {
                Rect::new(pad.x, pad.y - 100_000, pad.w, 200_000 + pad.h)
            });
            self.paints.push(Paint::ClipPop);
        }
        // Un botón o un enlace que es una caja: se puede tocar en todo su rectángulo.
        let field = self.p.info.field_of(n);
        let link = if name == "a" {
            self.p.info.link_of(n)
        } else {
            None
        };
        if (field.is_some() && !is_replaced(name)) || link.is_some() {
            self.paints.push(Paint::Hit {
                r,
                link: link.map_or(NONE, |l| l as u32),
                field: field.map_or(NONE, |f| f as u32),
            });
        }
        if positioned {
            let pending = self.abs.pop().unwrap_or_default();
            let cb = Rect::new(
                x + e.b[3],
                y + e.b[0],
                bw - e.b[1] - e.b[3],
                bh - e.b[0] - e.b[2],
            );
            for p in pending {
                self.layout_abs(&p, cb);
            }
        }
        if st.display == Display::ListItem && st.list_style != ListStyle::None && self.visible(st) {
            self.marker(n, st, cx, first.unwrap_or(cy + st.font.px as i32));
        }
        if st.position == Position::Relative {
            let dx = st.inset[3]
                .resolve(Some(bw))
                .or_else(|| st.inset[1].resolve(Some(bw)).map(|r| -r))
                .unwrap_or(0);
            let dy = st.inset[0]
                .resolve(cb_h)
                .or_else(|| st.inset[2].resolve(cb_h).map(|b| -b))
                .unwrap_or(0);
            if dx != 0 || dy != 0 {
                for p in &mut self.paints[start..] {
                    p.shift(dx, dy);
                }
            }
        }
        if invisible {
            self.hidden -= 1;
        }
        self.depth -= 1;
        Out {
            h: bh,
            first,
            last,
            deco,
        }
    }

    fn marker(&mut self, n: usize, st: &Style, cx: i32, base: i32) {
        // Número del elemento en su lista.
        let mut k: i32 = 1;
        if let Some(parent) = self.p.dom.nodes[n].parent {
            let pnode = &self.p.dom.nodes[parent];
            k = pnode
                .attr("start")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(1);
            let reversed = pnode.attr("reversed").is_some();
            for &c in &pnode.children {
                if c == n {
                    break;
                }
                if self
                    .p
                    .styled
                    .get(c)
                    .is_some_and(|s| s.display == Display::ListItem)
                {
                    k += if reversed { -1 } else { 1 };
                }
            }
        }
        if let Some(v) = self.p.dom.nodes[n]
            .attr("value")
            .and_then(|v| v.trim().parse().ok())
        {
            k = v;
        }
        let text = marker_text(st.list_style, k.max(0) as u32);
        if text.is_empty() {
            return;
        }
        let mut font = st.font;
        font.bold = false;
        let w = self.env.fonts.width(&text, font);
        let gap = (st.font_size * 0.5) as i32;
        let vm = self.env.fonts.vmetrics(font);
        let x = if st.list_inside { cx } else { cx - w - gap };
        self.paints.push(Paint::Text {
            x,
            base,
            top: base - vm.ascent,
            h: vm.ascent + vm.descent,
            w,
            text,
            font,
            color: self.color(st.color, false),
            underline: false,
            strike: false,
            link: NONE,
        });
    }

    /// Arma `n` aparte, con su esquina en (0, 0): devuelve lo que dibujó y la caja.
    fn detached(&mut self, n: usize, bw: i32, cb_h: Option<i32>) -> (Vec<Paint>, Out) {
        let saved = mem::take(&mut self.paints);
        let mut floats = Floats::default();
        let out = self.layout_box(n, 0, 0, bw, cb_h, &mut floats);
        let buf = mem::replace(&mut self.paints, saved);
        (buf, out)
    }

    fn append(&mut self, mut buf: Vec<Paint>, dx: i32, dy: i32) {
        if dx != 0 || dy != 0 {
            for p in &mut buf {
                p.shift(dx, dy);
            }
        }
        self.paints.extend(buf);
    }

    /// Ancho "que se ajusta al contenido" (inline-block, flotantes, absolutos).
    fn shrink_width(&mut self, n: usize, avail: i32) -> i32 {
        let st = self.sty(n);
        let e = edges(st, avail);
        if let Some(w) = st.width.resolve(Some(avail)) {
            let mut bw = to_border_box(st, &e, w);
            if let Some(mx) = st
                .max_width
                .resolve(Some(avail))
                .map(|w| to_border_box(st, &e, w))
            {
                bw = bw.min(mx);
            }
            if let Some(mn) = st
                .min_width
                .resolve(Some(avail))
                .map(|w| to_border_box(st, &e, w))
            {
                bw = bw.max(mn);
            }
            return bw.max(e.pb_h());
        }
        if is_replaced(self.name(n)) {
            return self.replaced_size(n, Some(avail)).0 + e.pb_h();
        }
        let (mn, mx) = self.intrinsic(n);
        let mut bw = mx.min((avail - e.m_h()).max(0)).max(mn);
        if let Some(m) = st
            .max_width
            .resolve(Some(avail))
            .map(|w| to_border_box(st, &e, w))
        {
            bw = bw.min(m);
        }
        if let Some(m) = st
            .min_width
            .resolve(Some(avail))
            .map(|w| to_border_box(st, &e, w))
        {
            bw = bw.max(m);
        }
        bw.max(e.pb_h())
    }

    // --- flujo de bloques --------------------------------------------------------------------

    fn layout_flow(
        &mut self,
        n: usize,
        cx: i32,
        cy: i32,
        cw: i32,
        ch: Option<i32>,
        floats: &mut Floats,
    ) -> Out {
        let mut kids = Vec::new();
        self.flow_kids(n, &mut kids, 0);
        self.flow_list(n, &kids, cx, cy, cw, ch, floats)
    }

    /// Hijos del flujo, con los "en línea con bloques adentro" abiertos.
    fn flow_kids(&self, n: usize, out: &mut Vec<usize>, depth: u32) {
        for c in self.kids(n) {
            if depth < 16 && self.inline_with_blocks(c) {
                self.flow_kids(c, out, depth + 1);
            } else {
                out.push(c);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn flow_list(
        &mut self,
        n: usize,
        kids: &[usize],
        cx: i32,
        cy: i32,
        cw: i32,
        ch: Option<i32>,
        floats: &mut Floats,
    ) -> Out {
        let st = self.sty(n);
        let mut y = cy;
        let mut pending_mb = 0;
        let mut first = None;
        let mut last = None;
        let mut run: Vec<usize> = Vec::new();
        let mut i = 0;
        while i <= kids.len() {
            let c = kids.get(i).copied();
            i += 1;
            let is_inline = c.is_some_and(|c| {
                self.is_text(c) || {
                    let s = self.sty(c);
                    s.display.is_inline_level()
                        || (!run.is_empty() && (s.float != Float::None || !s.in_flow()))
                }
            });
            if is_inline {
                run.push(c.unwrap_or(0));
                continue;
            }
            // Se termina un tramo de texto: se arma en renglones.
            if !run.is_empty() {
                let visible = run
                    .iter()
                    .any(|&r| !self.is_text(r) || !is_blank(self.text_of(r)));
                if visible {
                    let o = self.layout_inline(n, &run, cx, y + pending_mb, cw, floats);
                    if o.h > 0 {
                        y += pending_mb + o.h;
                        pending_mb = 0;
                        first = first.or(o.first);
                        last = o.last.or(last);
                    }
                }
                run.clear();
            }
            let Some(c) = c else { break };
            let cst = self.sty(c);
            // Absolutos: se anotan donde estarían y los arma su contenedor.
            if !cst.in_flow() {
                let pend = Pending {
                    node: c,
                    sx: cx,
                    sy: y + pending_mb,
                };
                if cst.position == Position::Fixed {
                    self.fixed.push(pend);
                } else if let Some(top) = self.abs.last_mut() {
                    top.push(pend);
                }
                continue;
            }
            if cst.float != Float::None {
                self.place_float(c, cx, y + pending_mb, cw, floats);
                continue;
            }
            // Partes de tabla sueltas: una tabla anónima.
            if matches!(
                cst.display,
                Display::TableRow | Display::TableCell | Display::TableRowGroup
            ) {
                let mut group = alloc::vec![c];
                while let Some(&next) = kids.get(i) {
                    let ok = self.is_text(next) && is_blank(self.text_of(next))
                        || !self.is_text(next)
                            && matches!(
                                self.sty(next).display,
                                Display::TableRow | Display::TableCell | Display::TableRowGroup
                            );
                    if !ok {
                        break;
                    }
                    group.push(next);
                    i += 1;
                }
                y += pending_mb;
                pending_mb = 0;
                let h = self.layout_table(n, &group, cx, y, cw);
                y += h;
                continue;
            }
            let e = edges(cst, cw);
            if cst.clear != Clear::None {
                let cl = floats.clear(cst.clear);
                if cl > y + pending_mb {
                    y = cl;
                    pending_mb = 0;
                }
            }
            let mt = e.m[0];
            let gap = if pending_mb >= 0 && mt >= 0 {
                pending_mb.max(mt)
            } else if pending_mb < 0 && mt < 0 {
                pending_mb.min(mt)
            } else {
                pending_mb + mt
            };
            let by = y + gap;
            // Una caja que arma su propio contexto se pone al lado de los flotantes.
            let (mut bx0, mut avail) = (cx, cw);
            if cst.is_bfc() && !floats.list.is_empty() {
                let (l, r) = floats.band(by, 1, cx, cx + cw);
                bx0 = l;
                avail = r - l;
            }
            let fill = !matches!(cst.display, Display::Table | Display::InlineTable);
            let (bw, ml, _) = self.block_width(c, avail, fill);
            let o = self.layout_box(c, bx0 + ml, by, bw, ch, floats);
            first = first.or(o.first);
            last = o.last.or(last);
            if o.h == 0 && e.pb_v() == 0 {
                // Una caja vacía: sus márgenes se juntan con los de al lado.
                pending_mb = gap.max(e.m[2]);
                y = by - gap;
            } else {
                y = by + o.h;
                pending_mb = e.m[2];
            }
        }
        let _ = st;
        Out {
            h: (y + pending_mb.max(0) - cy).max(0),
            first,
            last,
            deco: None,
        }
    }

    fn place_float(&mut self, c: usize, cx: i32, y: i32, cw: i32, floats: &mut Floats) {
        let st = self.sty(c);
        let e = edges(st, cw);
        let bw = self.shrink_width(c, cw);
        let (buf, out) = self.detached(c, bw, None);
        let (ow, oh) = (bw + e.m_h(), out.h + e.m_v());
        let left = st.float == Float::Left;
        let y = if st.clear != Clear::None {
            y.max(floats.clear(st.clear))
        } else {
            y
        };
        let (fx, fy) = floats.place(ow, oh, left, y, cx, cx + cw);
        self.append(buf, fx + e.m[3], fy + e.m[0]);
        floats.list.push((Rect::new(fx, fy, ow, oh), left));
    }

    // --- renglones ---------------------------------------------------------------------------

    /// Junta las palabras, cajas y marcas de un tramo en línea.
    fn pieces(
        &mut self,
        items: &[usize],
        avail: i32,
        out: &mut Vec<Piece>,
        ws_prev: &mut bool,
        depth: u32,
    ) {
        for &c in items {
            if self.is_text(c) {
                self.text_pieces(c, out, ws_prev);
                continue;
            }
            let st = self.sty(c);
            if !st.in_flow() {
                out.push(Piece::Abs(c));
                continue;
            }
            if st.float != Float::None {
                out.push(Piece::Float(c));
                continue;
            }
            let name = self.name(c);
            if name == "br" {
                out.push(Piece::Break);
                *ws_prev = true;
                continue;
            }
            if name == "wbr" {
                if let Some(last) = out
                    .iter_mut()
                    .rev()
                    .find(|p| matches!(p, Piece::Word { .. }))
                {
                    last.set_space(1, true);
                }
                continue;
            }
            let inline_box = st.display == Display::Inline && !is_replaced(name) && depth < 64;
            if inline_box {
                let e = edges(st, avail);
                out.push(Piece::Open(c, e.m[3] + e.b[3] + e.p[3]));
                let kids = self.kids(c);
                self.pieces(&kids, avail, out, ws_prev, depth + 1);
                out.push(Piece::Close(c, e.m[1] + e.b[1] + e.p[1]));
                continue;
            }
            // Una caja en la línea (imagen, campo, inline-block, o un bloque perdido).
            let block = !st.display.is_inline_level() && !is_replaced(name);
            if block {
                out.push(Piece::Break);
            }
            let e = edges(st, avail);
            let bw = if block {
                self.block_width(c, avail, true).0
            } else {
                self.shrink_width(c, avail)
            };
            let (buf, o) = self.detached(c, bw, None);
            let base = match (o.last, is_replaced(name)) {
                (Some(b), false) => b + e.m[0],
                _ => o.h + e.m[0],
            };
            let link = self.p.info.link_of(c).map_or(NONE, |l| l as u32);
            let mut buf = buf;
            for p in &mut buf {
                p.shift(e.m[3], e.m[0]);
            }
            out.push(Piece::Atomic {
                buf,
                w: bw + e.m_h(),
                h: o.h + e.m_v(),
                base,
                valign: st.valign,
                link,
                space: 0,
                brk: st.white_space.wraps(),
            });
            *ws_prev = false;
            if block {
                out.push(Piece::Break);
                *ws_prev = true;
            }
        }
    }

    fn text_pieces(&mut self, n: usize, out: &mut Vec<Piece>, ws_prev: &mut bool) {
        let raw = self.text_of(n);
        let parent = self.p.dom.nodes[n].parent.unwrap_or(0);
        let st = self.sty(parent);
        let link = self.p.info.link_of(n).map_or(NONE, |l| l as u32);
        let hidden = !self.visible(st)
            || st.text_indent.resolve(Some(0)).is_some_and(|i| i < -500)
            || (st.font_size < 2.0);
        let color = self.color(st.color, link != NONE);
        let underline = st.underline;
        let shift = match st.valign {
            VAlign::Sub => (st.font_size * 0.25) as i32,
            VAlign::Super => -(st.font_size * 0.4) as i32,
            _ => 0,
        };
        let space_w = self.text_w(" ", st);
        let mut word_start = *ws_prev;
        let text = apply_transform(raw, st.transform, &mut word_start);
        let mk = |this: &Self, t: &str| Piece::Word {
            w: this.text_w(t, st),
            text: t.to_string(),
            font: st.font,
            color,
            underline,
            strike: st.strike,
            link,
            shift,
            sn: parent,
            hidden,
            space: 0,
            brk: st.white_space.wraps(),
        };
        if st.icon_font {
            // Un ícono: un cuadrado de 1 em sin dibujar.
            if !is_blank(&text) {
                out.push(Piece::Word {
                    w: st.font.px as i32,
                    text: String::new(),
                    font: st.font,
                    color,
                    underline: false,
                    strike: false,
                    link,
                    shift: 0,
                    sn: parent,
                    hidden: true,
                    space: 0,
                    brk: true,
                });
                *ws_prev = false;
            }
            return;
        }
        let add_space = |out: &mut Vec<Piece>, w: i32, brk: bool| {
            for p in out.iter_mut().rev() {
                if p.set_space(w, brk) {
                    return;
                }
                if matches!(p, Piece::Break) {
                    return;
                }
            }
        };
        match st.white_space {
            WhiteSpace::Normal | WhiteSpace::NoWrap | WhiteSpace::PreLine => {
                let wraps = st.white_space.wraps();
                let lines: Vec<&str> = if st.white_space == WhiteSpace::PreLine {
                    text.split('\n').collect()
                } else {
                    alloc::vec![text.as_str()]
                };
                for (li, line) in lines.iter().enumerate() {
                    if li > 0 {
                        out.push(Piece::Break);
                        *ws_prev = true;
                    }
                    let mut word = String::new();
                    for ch in line.chars() {
                        if ch.is_ascii_whitespace() {
                            if !word.is_empty() {
                                out.push(mk(self, &word));
                                word.clear();
                            }
                            if !*ws_prev {
                                add_space(out, space_w, wraps);
                                *ws_prev = true;
                            }
                        } else {
                            word.push(ch);
                            *ws_prev = false;
                        }
                    }
                    if !word.is_empty() {
                        out.push(mk(self, &word));
                    }
                }
            }
            WhiteSpace::Pre | WhiteSpace::PreWrap => {
                let wraps = st.white_space == WhiteSpace::PreWrap;
                let text = text.replace('\t', "        ").replace('\r', "");
                for (li, line) in text.split('\n').enumerate() {
                    if li > 0 {
                        out.push(Piece::Break);
                    }
                    if !wraps {
                        if !line.is_empty() {
                            out.push(mk(self, line));
                        }
                        continue;
                    }
                    for (wi, w) in line.split(' ').enumerate() {
                        if wi > 0 {
                            add_space(out, space_w, true);
                        }
                        if !w.is_empty() {
                            out.push(mk(self, w));
                        }
                    }
                }
                *ws_prev = false;
            }
        }
    }

    /// Arma un tramo en línea en renglones. `n` es el bloque que lo contiene.
    fn layout_inline(
        &mut self,
        n: usize,
        items: &[usize],
        cx: i32,
        cy: i32,
        cw: i32,
        floats: &mut Floats,
    ) -> Out {
        let st = self.sty(n);
        let mut pieces = Vec::new();
        let mut ws = true;
        self.pieces(items, cw, &mut pieces, &mut ws, 0);
        let strut_vm = self.env.fonts.vmetrics(st.font);
        let strut_h = st.line_px(strut_vm.line_height);
        let indent = st
            .text_indent
            .resolve(Some(cw))
            .unwrap_or(0)
            .clamp(0, cw / 2);
        // Renglones: (lista de (pieza, x), izquierda, ancho, y)
        struct Line {
            items: Vec<(usize, i32)>,
            left: i32,
            width: i32,
            y: i32,
            forced: bool,
        }
        let mut lines: Vec<Line> = Vec::new();
        let mut y = cy;
        let (l0, r0) = floats.band(y, strut_h, cx, cx + cw);
        let mut cur = Line {
            items: Vec::new(),
            left: l0 + indent,
            width: r0 - l0 - indent,
            y,
            forced: false,
        };
        let mut x = 0;
        let mut seg: Vec<usize> = Vec::new();
        // Alto aproximado de un renglón terminado (para ubicar el siguiente).
        let line_h_of = |this: &Self, line: &Line, pieces: &[Piece]| -> i32 {
            this.line_metrics(
                line.items.iter().map(|(i, _)| &pieces[*i]),
                st,
                strut_h,
                strut_vm.ascent,
                strut_vm.descent,
            )
            .map_or(if line.forced { strut_h } else { 0 }, |(a, d)| a + d)
        };
        let mut i = 0;
        let total = pieces.len();
        while i <= total {
            let p = i;
            i += 1;
            let end_seg =
                p == total || matches!(pieces[p], Piece::Break | Piece::Float(_) | Piece::Abs(_));
            if !end_seg {
                seg.push(p);
                if !pieces[p].breakable() && p + 1 < total {
                    continue;
                }
            }
            // Se ubica el segmento (lo que no se puede cortar).
            if !seg.is_empty() {
                let seg_w: i32 = seg
                    .iter()
                    .enumerate()
                    .map(|(k, &s)| {
                        pieces[s].width()
                            + if k + 1 < seg.len() {
                                pieces[s].space()
                            } else {
                                0
                            }
                    })
                    .sum();
                let wraps = st.white_space.wraps() || seg.iter().any(|&s| pieces[s].breakable());
                if x + seg_w > cur.width && !cur.items.is_empty() && wraps {
                    let lh = line_h_of(self, &cur, &pieces);
                    y += lh;
                    lines.push(mem::replace(
                        &mut cur,
                        Line {
                            items: Vec::new(),
                            left: 0,
                            width: 0,
                            y,
                            forced: false,
                        },
                    ));
                    let (l, r) = floats.band(y, strut_h, cx, cx + cw);
                    cur.left = l;
                    cur.width = r - l;
                    x = 0;
                }
                // Una palabra sola más larga que el renglón: se corta por letras.
                if seg_w > cur.width
                    && seg.len() == 1
                    && matches!(pieces[seg[0]], Piece::Word { .. })
                    && cur.width > 20
                {
                    let chunks =
                        self.split_word(&pieces[seg[0]], cur.width - x.min(cur.width - 20));
                    if chunks.len() > 1 {
                        let orig_space = pieces[seg[0]].space();
                        let orig_brk = pieces[seg[0]].breakable();
                        let base_idx = pieces.len();
                        let nchunks = chunks.len();
                        for (k, mut ch) in chunks.into_iter().enumerate() {
                            if k + 1 == nchunks {
                                ch.set_space(orig_space, orig_brk);
                            }
                            pieces.push(ch);
                        }
                        for k in 0..nchunks {
                            let idx = base_idx + k;
                            let w = pieces[idx].width();
                            if x + w > cur.width && !cur.items.is_empty() {
                                let lh = line_h_of(self, &cur, &pieces);
                                y += lh;
                                lines.push(mem::replace(
                                    &mut cur,
                                    Line {
                                        items: Vec::new(),
                                        left: 0,
                                        width: 0,
                                        y,
                                        forced: false,
                                    },
                                ));
                                let (l, r) = floats.band(y, strut_h, cx, cx + cw);
                                cur.left = l;
                                cur.width = r - l;
                                x = 0;
                            }
                            cur.items.push((idx, x));
                            x += w + pieces[idx].space();
                        }
                        seg.clear();
                        if p < total && end_seg {
                            // Sigue abajo con el Break/Float/Abs.
                        } else {
                            continue;
                        }
                    }
                }
                for &s in &seg {
                    cur.items.push((s, x));
                    x += pieces[s].width() + pieces[s].space();
                }
                seg.clear();
            }
            if p == total {
                break;
            }
            match pieces[p] {
                Piece::Break => {
                    cur.forced = true;
                    let lh = line_h_of(self, &cur, &pieces);
                    y += lh;
                    lines.push(mem::replace(
                        &mut cur,
                        Line {
                            items: Vec::new(),
                            left: 0,
                            width: 0,
                            y,
                            forced: false,
                        },
                    ));
                    let (l, r) = floats.band(y, strut_h, cx, cx + cw);
                    cur.left = l;
                    cur.width = r - l;
                    x = 0;
                }
                Piece::Float(f) => {
                    self.place_float(f, cx, cur.y, cw, floats);
                    let (l, r) = floats.band(cur.y, strut_h, cx, cx + cw);
                    if cur.items.is_empty() {
                        cur.left = l;
                    }
                    cur.width = r - cur.left;
                }
                Piece::Abs(a) => {
                    let pend = Pending {
                        node: a,
                        sx: cur.left + x,
                        sy: cur.y,
                    };
                    if self.sty(a).position == Position::Fixed {
                        self.fixed.push(pend);
                    } else if let Some(top) = self.abs.last_mut() {
                        top.push(pend);
                    }
                }
                _ => {}
            }
        }
        if !cur.items.is_empty() || cur.forced {
            lines.push(cur);
        }
        // Se dibujan los renglones.
        let mut first = None;
        let mut last = None;
        let mut y = cy;
        // Cajas en línea abiertas (fondos que siguen en el renglón siguiente).
        let mut open: Vec<usize> = Vec::new();
        for line in lines {
            let Some((a, d)) = self.line_metrics(
                line.items.iter().map(|(i, _)| &pieces[*i]),
                st,
                strut_h,
                strut_vm.ascent,
                strut_vm.descent,
            ) else {
                if line.forced {
                    y += strut_h;
                }
                continue;
            };
            let y_top = y.max(line.y);
            let base = y_top + a;
            let lh = a + d;
            first = first.or(Some(base));
            last = Some(base);
            // Ancho del contenido (sin el espacio del final) para alinear.
            let content_w = line
                .items
                .iter()
                .map(|(i, x)| x + pieces[*i].width())
                .max()
                .unwrap_or(0);
            let shift = match st.align {
                TextAlign::Center => ((line.width - content_w) / 2).max(0),
                TextAlign::Right => (line.width - content_w).max(0),
                _ => 0,
            };
            let lx = line.left + shift;
            // Fondos y bordes de las cajas en línea de este renglón.
            let mut starts: Vec<(usize, i32)> = open.iter().map(|&n| (n, lx)).collect();
            let mut decos: Vec<(usize, i32, i32)> = Vec::new();
            for (pi, x) in &line.items {
                match &pieces[*pi] {
                    Piece::Open(on, w) => {
                        let m = edges(self.sty(*on), cw).m[3];
                        starts.push((*on, lx + x + m));
                        open.push(*on);
                        let _ = w;
                    }
                    Piece::Close(on, w) => {
                        if let Some(k) = starts.iter().rposition(|(n, _)| n == on) {
                            let (_, sx) = starts.remove(k);
                            let m = edges(self.sty(*on), cw).m[1];
                            decos.push((*on, sx, lx + x + w - m));
                        }
                        if let Some(k) = open.iter().rposition(|n| n == on) {
                            open.remove(k);
                        }
                    }
                    _ => {}
                }
            }
            for (on, sx) in starts {
                decos.push((on, sx, lx + content_w));
            }
            for (on, x0, x1) in decos {
                let ist = self.sty(on);
                if !self.visible(ist) {
                    continue;
                }
                let has = (ist.bg.is_some() && !self.env.dark)
                    || ist.border_widths().iter().any(|&b| b > 0);
                if !has || x1 <= x0 {
                    continue;
                }
                let e = edges(ist, cw);
                let vm = self.env.fonts.vmetrics(ist.font);
                let top = base - vm.ascent - e.p[0] - e.b[0];
                let bot = base + vm.descent + e.p[2] + e.b[2];
                let at = self.paints.len();
                self.paints.extend([Paint::None, Paint::None, Paint::None]);
                let mut paints = mem::take(&mut self.paints);
                self.fill_deco(&mut paints, at, on, Rect::new(x0, top, x1 - x0, bot - top));
                self.paints = paints;
            }
            for (pi, x) in &line.items {
                let piece = mem::replace(&mut pieces[*pi], Piece::Break);
                match piece {
                    Piece::Word {
                        text,
                        w,
                        font,
                        color,
                        underline,
                        strike,
                        link,
                        shift,
                        hidden,
                        ..
                    } => {
                        if !hidden && !text.is_empty() {
                            self.paints.push(Paint::Text {
                                x: lx + x,
                                base: base + shift,
                                top: y_top,
                                h: lh,
                                w,
                                text,
                                font,
                                color,
                                underline,
                                strike,
                                link,
                            });
                        } else if link != NONE {
                            self.paints.push(Paint::Hit {
                                r: Rect::new(lx + x, y_top, w, lh),
                                link,
                                field: NONE,
                            });
                        }
                    }
                    Piece::Atomic {
                        buf,
                        w,
                        h,
                        base: ab,
                        valign,
                        link,
                        ..
                    } => {
                        let top = match valign {
                            VAlign::Middle => base - strut_vm.ascent / 3 - h / 2,
                            VAlign::Top => y_top,
                            VAlign::Bottom => y_top + lh - h,
                            _ => base - ab,
                        };
                        self.append(buf, lx + x, top);
                        if link != NONE {
                            self.paints.push(Paint::Hit {
                                r: Rect::new(lx + x, top, w, h),
                                link,
                                field: NONE,
                            });
                        }
                    }
                    _ => {}
                }
            }
            y = y_top + lh;
        }
        Out {
            h: (y - cy).max(0),
            first,
            last,
            deco: None,
        }
    }

    /// (sobre la base, debajo de la base) de un renglón; `None` si no tiene nada.
    fn line_metrics<'p>(
        &self,
        items: impl Iterator<Item = &'p Piece>,
        st: &Style,
        strut_h: i32,
        strut_a: i32,
        strut_d: i32,
    ) -> Option<(i32, i32)> {
        let half = (strut_h - strut_a - strut_d) / 2;
        let (mut a, mut d) = (strut_a + half, strut_h - strut_a - half);
        let mut any = false;
        for p in items {
            match p {
                Piece::Word {
                    font, sn, shift, ..
                } => {
                    any = true;
                    let vm = self.env.fonts.vmetrics(*font);
                    let lh = self.sty(*sn).line_px(vm.line_height);
                    let half = (lh - vm.ascent - vm.descent) / 2;
                    a = a.max(vm.ascent + half - shift);
                    d = d.max(lh - vm.ascent - half + shift);
                }
                Piece::Atomic {
                    h, base, valign, ..
                } => {
                    any = true;
                    match valign {
                        VAlign::Middle => {
                            let up = strut_a / 3 + h / 2;
                            a = a.max(up);
                            d = d.max(h - up);
                        }
                        VAlign::Top | VAlign::Bottom => {
                            d = d.max(h - a);
                        }
                        _ => {
                            a = a.max(*base);
                            d = d.max(h - base);
                        }
                    }
                }
                Piece::Open(..) | Piece::Close(..) => {}
                _ => {}
            }
        }
        let _ = st;
        any.then_some((a, d))
    }

    fn split_word(&self, p: &Piece, first_w: i32) -> Vec<Piece> {
        let Piece::Word {
            text,
            font,
            color,
            underline,
            strike,
            link,
            shift,
            sn,
            hidden,
            ..
        } = p
        else {
            return Vec::new();
        };
        let adv = self.env.fonts.advances(text, *font);
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0;
        let mut limit = first_w.max(20) * 64;
        let full = limit.max(20 * 64);
        for (ch, a) in text.chars().zip(adv) {
            if cur_w + a > limit && !cur.is_empty() {
                out.push((mem::take(&mut cur), (cur_w + 32) / 64));
                cur_w = 0;
                limit = full;
            }
            cur.push(ch);
            cur_w += a;
        }
        if !cur.is_empty() {
            out.push((cur, (cur_w + 32) / 64));
        }
        out.into_iter()
            .map(|(t, w)| Piece::Word {
                text: t,
                w,
                font: *font,
                color: *color,
                underline: *underline,
                strike: *strike,
                link: *link,
                shift: *shift,
                sn: *sn,
                hidden: *hidden,
                space: 0,
                brk: true,
            })
            .collect()
    }

    // --- elementos reemplazados (imágenes, campos) -------------------------------------------

    /// Tamaño del contenido de una imagen, campo, video…
    fn replaced_size(&mut self, n: usize, cbw: Option<i32>) -> (i32, i32) {
        let st = self.sty(n);
        let name = self.name(n);
        let node = &self.p.dom.nodes[n];
        let e = edges(st, cbw.unwrap_or(0));
        let content = |v: i32| {
            if st.border_box {
                (v - e.pb_h()).max(0)
            } else {
                v
            }
        };
        let content_v = |v: i32| {
            if st.border_box {
                (v - e.pb_v()).max(0)
            } else {
                v
            }
        };
        let spec_w = st.width.resolve(cbw).map(content);
        let spec_h = st.height.resolve(None).map(content_v);
        let font = st.font;
        let vm = self.env.fonts.vmetrics(font);
        let em = st.font_size as i32;
        let (nat, ratio): (Option<(i32, i32)>, Option<f32>) = match name {
            "img" | "image" => {
                let nat = self.p.info.image_of(n).and_then(|i| self.natural(i));
                (
                    nat,
                    nat.map(|(w, h)| w as f32 / h as f32).or(st.aspect_ratio),
                )
            }
            "svg" => {
                let vb = node.attr("viewbox").and_then(|v| {
                    let p: Vec<f32> = v
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    (p.len() == 4 && p[2] > 0.0 && p[3] > 0.0).then(|| (p[2], p[3]))
                });
                let nat = vb
                    .filter(|(w, h)| *w <= 64.0 && *h <= 64.0)
                    .map(|(w, h)| (w as i32, h as i32));
                (nat, vb.map(|(w, h)| w / h))
            }
            "input" => {
                let f = self
                    .p
                    .info
                    .field_of(n)
                    .and_then(|f| self.p.doc.fields.get(f));
                let lh = vm.line_height;
                let size = match f.map(|f| f.kind) {
                    Some(FieldKind::Checkbox | FieldKind::Radio) => (13, 13),
                    Some(FieldKind::Submit | FieldKind::Button) => {
                        let label = f.map_or("", |f| f.value.as_str());
                        (self.env.fonts.width(label, font), lh)
                    }
                    _ => {
                        let chars = node
                            .attr("size")
                            .and_then(|s| s.trim().parse::<i32>().ok())
                            .unwrap_or(20)
                            .clamp(1, 200);
                        (chars * em * 55 / 100, lh)
                    }
                };
                (Some(size), None)
            }
            "select" => {
                let f = self
                    .p
                    .info
                    .field_of(n)
                    .and_then(|f| self.p.doc.fields.get(f));
                let w = f
                    .map(|f| {
                        f.options
                            .iter()
                            .map(|(_, l)| self.env.fonts.width(l, font))
                            .max()
                            .unwrap_or(40)
                    })
                    .unwrap_or(40);
                (Some((w + 24, vm.line_height)), None)
            }
            "textarea" => {
                let cols = node
                    .attr("cols")
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(20);
                let rows = node
                    .attr("rows")
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(2);
                (
                    Some((
                        cols.clamp(1, 200) * em * 55 / 100,
                        rows.clamp(1, 50) * vm.line_height,
                    )),
                    None,
                )
            }
            "progress" | "meter" => (Some((160, 16)), None),
            "audio" => (Some((300, 54)), None),
            _ => (Some((300, 150)), None),
        };
        let (mut w, mut h) = match (spec_w, spec_h, nat, ratio) {
            (Some(w), Some(h), _, _) => (w, h),
            (Some(w), None, _, Some(r)) => (w, (w as f32 / r) as i32),
            (Some(w), None, Some((_, nh)), None) => (w, nh),
            (None, Some(h), _, Some(r)) => ((h as f32 * r) as i32, h),
            (None, Some(h), Some((nw, _)), None) => (nw, h),
            (None, None, Some(n), _) => n,
            (Some(w), None, None, None) => (w, 0),
            (None, Some(h), None, None) => (0, h),
            (None, None, None, _) => {
                // Imagen que todavía no llegó (o no se pudo leer), sin tamaño: su texto
                // alternativo, o nada.
                let alt = node.attr("alt").unwrap_or("");
                if alt.trim().is_empty() || name != "img" {
                    (0, 0)
                } else {
                    (
                        self.env.fonts.width(alt, font).min(300) + 8,
                        vm.line_height + 4,
                    )
                }
            }
        };
        let base = cbw;
        if let Some(mx) = st.max_width.resolve(base).map(content)
            && w > mx
        {
            if spec_h.is_none()
                && let Some(r) = ratio
            {
                h = (mx as f32 / r) as i32;
            }
            w = mx;
        }
        if let Some(mx) = st.max_height.resolve(None).map(content_v)
            && h > mx
        {
            if spec_w.is_none()
                && let Some(r) = ratio
            {
                w = (mx as f32 * r) as i32;
            }
            h = mx;
        }
        if let Some(mn) = st.min_width.resolve(base).map(content) {
            w = w.max(mn);
        }
        if let Some(mn) = st.min_height.resolve(None).map(content_v) {
            h = h.max(mn);
        }
        (w.clamp(0, 8000), h.clamp(0, 8000))
    }

    fn replaced_content(
        &mut self,
        n: usize,
        cx: i32,
        cy: i32,
        cw: i32,
        cb_h: Option<i32>,
    ) -> (i32, i32) {
        let name = self.name(n);
        let st = self.sty(n);
        let (_, mut h) = self.replaced_size(n, Some(cw));
        if let Some(sh) = st.height.resolve(cb_h) {
            let e = edges(st, cw);
            h = (to_border_box_v(st, &e, sh) - e.pb_v()).max(0);
        }
        let r = Rect::new(cx, cy, cw, h);
        if !self.visible(st) || r.w <= 0 || r.h <= 0 {
            return (cw, h);
        }
        let link = self.p.info.link_of(n).map_or(NONE, |l| l as u32);
        match name {
            "img" | "image" => match self.p.info.image_of(n) {
                Some(i) if self.natural(i).is_some() => self.paints.push(Paint::Image {
                    r,
                    img: i as u32,
                    link,
                }),
                _ => {
                    let alt = self.p.dom.nodes[n]
                        .attr("alt")
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    self.paints.push(Paint::Placeholder { r, label: alt });
                    if link != NONE {
                        self.paints.push(Paint::Hit {
                            r,
                            link,
                            field: NONE,
                        });
                    }
                }
            },
            "input" | "select" | "textarea" => {
                if let Some(f) = self.p.info.field_of(n) {
                    self.paints.push(Paint::Field {
                        r,
                        field: f as u32,
                        font: st.font,
                        color: self.color(st.color, false),
                    });
                }
            }
            "svg" => {}
            "iframe" | "video" | "embed" | "object" | "canvas" | "audio" => {
                let label = match name {
                    "video" => "video",
                    "audio" => "audio",
                    "canvas" => "",
                    _ => "contenido incrustado",
                };
                self.paints.push(Paint::Placeholder {
                    r,
                    label: label.into(),
                });
            }
            _ => {}
        }
        (cw, h)
    }

    // --- tamaños intrínsecos -----------------------------------------------------------------

    /// (mínimo, máximo) del ancho con borde de `n`: lo más angosto que puede ser sin que nada
    /// se salga (la palabra más larga) y lo que mediría sin cortar ningún renglón.
    fn intrinsic(&mut self, n: usize) -> (i32, i32) {
        if let Some(v) = self.intrinsic[n] {
            return v;
        }
        if self.depth > MAX_DEPTH {
            return (0, 0);
        }
        self.depth += 1;
        let st = self.sty(n);
        let e = edges(st, 0);
        let v = if let Some(w) = st.width.resolve(None).filter(|_| !st.width.is_relative()) {
            let bw = to_border_box(st, &e, w);
            (bw, bw)
        } else if is_replaced(self.name(n)) {
            let (w, _) = self.replaced_size(n, None);
            let bw = w + e.pb_h();
            let min = if st.max_width.is_relative() { 0 } else { bw };
            (min, bw)
        } else {
            let (mn, mx) = self.intrinsic_content(n, st);
            let mut mn = mn + e.pb_h();
            let mut mx = mx + e.pb_h();
            if let Some(m) = st
                .max_width
                .resolve(None)
                .filter(|_| !st.max_width.is_relative())
            {
                let m = to_border_box(st, &e, m);
                mx = mx.min(m);
                mn = mn.min(m);
            }
            if let Some(m) = st
                .min_width
                .resolve(None)
                .filter(|_| !st.min_width.is_relative())
            {
                let m = to_border_box(st, &e, m);
                mx = mx.max(m);
                mn = mn.max(m);
            }
            (mn, mx.max(mn))
        };
        self.depth -= 1;
        self.intrinsic[n] = Some(v);
        v
    }

    fn outer_intrinsic(&mut self, c: usize) -> (i32, i32) {
        let (mn, mx) = self.intrinsic(c);
        let e = edges(self.sty(c), 0);
        (mn + e.m_h().max(0), mx + e.m_h().max(0))
    }

    fn intrinsic_content(&mut self, n: usize, st: &Style) -> (i32, i32) {
        let kids = self.kids(n);
        match st.display {
            Display::Flex | Display::InlineFlex => {
                let gap = st.col_gap.resolve(Some(0)).unwrap_or(0);
                let items = self.flex_kids(&kids);
                let mut sizes = Vec::new();
                for k in &items {
                    sizes.push(match k {
                        Kid::El(c) => self.outer_intrinsic(*c),
                        Kid::Text(t) => self.text_intrinsic(t),
                    });
                }
                if st.flex_dir.is_row() {
                    let gaps = gap * (sizes.len() as i32 - 1).max(0);
                    let mx: i32 = sizes.iter().map(|s| s.1).sum::<i32>() + gaps;
                    let mn = if st.flex_wrap {
                        sizes.iter().map(|s| s.0).max().unwrap_or(0)
                    } else {
                        sizes.iter().map(|s| s.0).sum::<i32>() + gaps
                    };
                    (mn, mx)
                } else {
                    (
                        sizes.iter().map(|s| s.0).max().unwrap_or(0),
                        sizes.iter().map(|s| s.1).max().unwrap_or(0),
                    )
                }
            }
            Display::Grid | Display::InlineGrid => {
                let items = self.flex_kids(&kids);
                let gap = st.col_gap.resolve(Some(0)).unwrap_or(0);
                let mut sizes = Vec::new();
                for k in &items {
                    sizes.push(match k {
                        Kid::El(c) => self.outer_intrinsic(*c),
                        Kid::Text(t) => self.text_intrinsic(t),
                    });
                }
                let cols = st.grid_cols.as_ref().map_or(1, |c| c.tracks.len().max(1)) as i32;
                let fixed: i32 = st
                    .grid_cols
                    .as_ref()
                    .map(|c| {
                        c.tracks
                            .iter()
                            .map(|t| match t {
                                Track::Fixed(l) => l.resolve(None).unwrap_or(0),
                                _ => 0,
                            })
                            .sum()
                    })
                    .unwrap_or(0);
                let flex_cols = st.grid_cols.as_ref().map_or(1, |c| {
                    c.tracks
                        .iter()
                        .filter(|t| !matches!(t, Track::Fixed(_)))
                        .count()
                }) as i32;
                let item_max = sizes.iter().map(|s| s.1).max().unwrap_or(0);
                let item_min = sizes.iter().map(|s| s.0).max().unwrap_or(0);
                let gaps = gap * (cols - 1).max(0);
                (
                    fixed + flex_cols * item_min + gaps,
                    fixed + flex_cols * item_max + gaps,
                )
            }
            Display::Table | Display::InlineTable => {
                let rows = self.table_rows(&kids);
                let spacing = if st.border_collapse {
                    0
                } else {
                    st.border_spacing
                };
                let (mut mn, mut mx) = (0, 0);
                for (_, cells) in &rows {
                    let (mut a, mut b) = (0, 0);
                    for c in cells {
                        let (x, y) = self.cell_intrinsic(c);
                        a += x + spacing;
                        b += y + spacing;
                    }
                    mn = mn.max(a + spacing);
                    mx = mx.max(b + spacing);
                }
                (mn, mx)
            }
            _ => {
                let mut flat = Vec::new();
                self.flow_kids(n, &mut flat, 0);
                self.flow_intrinsic(&flat)
            }
        }
    }

    fn cell_intrinsic(&mut self, c: &TCell) -> (i32, i32) {
        match c {
            TCell::Node(n) => self.intrinsic(*n),
            TCell::Anon(kids) => self.flow_intrinsic(kids),
        }
    }

    /// (mín, máx) de una lista de hijos de un bloque.
    fn flow_intrinsic(&mut self, flat: &[usize]) -> (i32, i32) {
        {
            {
                let (mut mn, mut mx) = (0, 0);
                let mut run = Vec::new();
                for (i, &c) in flat.iter().enumerate() {
                    let inline = self.is_text(c) || {
                        let s = self.sty(c);
                        s.display.is_inline_level() || s.float != Float::None
                    };
                    if inline {
                        run.push(c);
                    }
                    if !inline || i + 1 == flat.len() {
                        if !run.is_empty() {
                            let (a, b) = self.text_intrinsic(&run);
                            mn = mn.max(a);
                            mx = mx.max(b);
                            run.clear();
                        }
                        if !inline && self.sty(c).in_flow() {
                            let (a, b) = self.outer_intrinsic(c);
                            mn = mn.max(a);
                            mx = mx.max(b);
                        }
                    }
                }
                (mn, mx)
            }
        }
    }

    /// (mín, máx) de un tramo en línea.
    fn text_intrinsic(&mut self, items: &[usize]) -> (i32, i32) {
        let saved = mem::take(&mut self.paints);
        let mut pieces = Vec::new();
        let mut ws = true;
        // Las cajas en línea se miden con su ancho máximo (sin armarlas, que es caro).
        self.measure_pieces(items, &mut pieces, &mut ws, 0);
        self.paints = saved;
        let (mut mn, mut mx, mut line, mut seg) = (0, 0, 0, 0);
        for (w, min_w, space, brk, forced) in pieces {
            if forced {
                mx = mx.max(line);
                mn = mn.max(seg);
                line = 0;
                seg = 0;
                continue;
            }
            line += w + space;
            seg += min_w;
            if brk {
                mn = mn.max(seg);
                seg = 0;
            } else {
                seg += space;
            }
        }
        (mn.max(seg), mx.max(line))
    }

    /// Como [`Self::pieces`], pero solo anchos: (ancho, ancho mínimo, espacio, ¿corta?, ¿salto?).
    fn measure_pieces(
        &mut self,
        items: &[usize],
        out: &mut Vec<(i32, i32, i32, bool, bool)>,
        ws: &mut bool,
        depth: u32,
    ) {
        for &c in items {
            if self.is_text(c) {
                let mut tmp = Vec::new();
                self.text_pieces(c, &mut tmp, ws);
                for p in tmp {
                    match p {
                        Piece::Break => out.push((0, 0, 0, false, true)),
                        p => out.push((p.width(), p.width(), p.space(), p.breakable(), false)),
                    }
                }
                continue;
            }
            let st = self.sty(c);
            if !st.in_flow() {
                continue;
            }
            let name = self.name(c);
            if name == "br" {
                out.push((0, 0, 0, false, true));
                *ws = true;
                continue;
            }
            if st.display == Display::Inline && !is_replaced(name) && depth < 64 {
                let e = edges(st, 0);
                out.push((e.m[3] + e.b[3] + e.p[3], 0, 0, false, false));
                let kids = self.kids(c);
                self.measure_pieces(&kids, out, ws, depth + 1);
                out.push((e.m[1] + e.b[1] + e.p[1], 0, 0, false, false));
                continue;
            }
            let (mn, mx) = self.outer_intrinsic(c);
            out.push((mx, mn, 0, true, false));
            *ws = false;
        }
    }

    // --- flex --------------------------------------------------------------------------------

    fn flex_kids(&self, kids: &[usize]) -> Vec<Kid> {
        let mut out: Vec<Kid> = Vec::new();
        for &c in kids {
            if self.is_text(c) {
                if is_blank(self.text_of(c)) {
                    continue;
                }
                if let Some(Kid::Text(t)) = out.last_mut() {
                    t.push(c);
                } else {
                    out.push(Kid::Text(alloc::vec![c]));
                }
                continue;
            }
            out.push(Kid::El(c));
        }
        out
    }

    /// Arma un hijo de flex/grid (o texto suelto) aparte, con ancho de caja `bw`.
    fn detached_kid(
        &mut self,
        k: &Kid,
        parent: usize,
        bw: i32,
        cb_h: Option<i32>,
    ) -> (Vec<Paint>, Out) {
        match k {
            Kid::El(c) => self.detached(*c, bw, cb_h),
            Kid::Text(t) => {
                let saved = mem::take(&mut self.paints);
                let mut floats = Floats::default();
                let o = self.layout_inline(parent, t, 0, 0, bw, &mut floats);
                let buf = mem::replace(&mut self.paints, saved);
                (buf, o)
            }
        }
    }

    fn kid_style(&self, k: &Kid, parent: usize) -> &'a Style {
        match k {
            Kid::El(c) => self.sty(*c),
            Kid::Text(_) => self.sty(parent),
        }
    }

    fn register_abs(&mut self, c: usize, sx: i32, sy: i32) {
        let pend = Pending { node: c, sx, sy };
        if self.sty(c).position == Position::Fixed {
            self.fixed.push(pend);
        } else if let Some(top) = self.abs.last_mut() {
            top.push(pend);
        }
    }

    fn layout_flex(&mut self, n: usize, cx: i32, cy: i32, cw: i32, ch: Option<i32>) -> i32 {
        let st = self.sty(n);
        let kids = self.kids(n);
        let mut items: Vec<Kid> = Vec::new();
        for k in self.flex_kids(&kids) {
            if let Kid::El(c) = k
                && !self.sty(c).in_flow()
            {
                self.register_abs(c, cx, cy);
                continue;
            }
            items.push(k);
        }
        // `order`.
        let mut ordered: Vec<(i32, usize, Kid)> = items
            .into_iter()
            .enumerate()
            .map(|(i, k)| (self.kid_style(&k, n).order, i, k))
            .collect();
        ordered.sort_by_key(|(o, i, _)| (*o, *i));
        let items: Vec<Kid> = ordered.into_iter().map(|(_, _, k)| k).collect();
        if items.is_empty() {
            return ch.unwrap_or(0);
        }
        let row_gap = st.row_gap.resolve(ch).unwrap_or(0).max(0);
        let col_gap = st.col_gap.resolve(Some(cw)).unwrap_or(0).max(0);
        if st.flex_dir.is_row() {
            self.flex_row(n, st, &items, cx, cy, cw, ch, row_gap, col_gap)
        } else {
            self.flex_column(n, st, &items, cx, cy, cw, ch, row_gap)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn flex_row(
        &mut self,
        n: usize,
        st: &Style,
        items: &[Kid],
        cx: i32,
        cy: i32,
        cw: i32,
        ch: Option<i32>,
        row_gap: i32,
        col_gap: i32,
    ) -> i32 {
        struct It {
            e: Edges,
            hyp: i32,
            min: i32,
            max: i32,
            grow: f32,
            shrink: f32,
            size: i32,
        }
        let mut its: Vec<It> = Vec::new();
        for k in items {
            let ist = self.kid_style(k, n);
            let text = matches!(k, Kid::Text(_));
            let e = if text {
                Edges::default()
            } else {
                edges(ist, cw)
            };
            let (imin, imax) = match k {
                Kid::El(c) => self.intrinsic(*c),
                Kid::Text(t) => self.text_intrinsic(t),
            };
            let conv = |w: i32| if text { w } else { to_border_box(ist, &e, w) };
            let basis = if text {
                None
            } else {
                ist.basis.resolve(Some(cw)).map(conv)
            };
            let width = if text {
                None
            } else {
                ist.width.resolve(Some(cw)).map(conv)
            };
            let base = basis.or(width).unwrap_or(match k {
                Kid::El(c) if is_replaced(self.name(*c)) => {
                    self.replaced_size(*c, Some(cw)).0 + e.pb_h()
                }
                _ => imax,
            });
            let max = if text {
                i32::MAX
            } else {
                ist.max_width
                    .resolve(Some(cw))
                    .map(conv)
                    .unwrap_or(i32::MAX)
            };
            let min = if text {
                0
            } else {
                match ist.min_width.resolve(Some(cw)).map(conv) {
                    Some(m) => m,
                    None if ist.clip_x => e.pb_h(),
                    None => imin.min(width.unwrap_or(i32::MAX)).min(max),
                }
            };
            let (grow, shrink) = if text {
                (0.0, 1.0)
            } else {
                (ist.grow, ist.shrink)
            };
            let hyp = base.clamp(min.min(max), max).max(e.pb_h());
            its.push(It {
                e,
                hyp,
                min: min.max(e.pb_h()),
                max,
                grow,
                shrink,
                size: hyp,
            });
        }
        // Líneas.
        let mut lines: Vec<(usize, usize)> = Vec::new();
        let mut start = 0;
        let mut used = 0;
        for (i, it) in its.iter().enumerate() {
            let outer = it.hyp + it.e.m_h();
            if st.flex_wrap && i > start && used + col_gap + outer > cw {
                lines.push((start, i));
                start = i;
                used = 0;
            }
            used += outer + if i > start { col_gap } else { 0 };
        }
        lines.push((start, its.len()));
        let single = lines.len() == 1;
        let mut y = cy;
        for (li, &(a, b)) in lines.iter().enumerate() {
            if li > 0 {
                y += row_gap;
            }
            let gaps = col_gap * (b - a - 1) as i32;
            let used: i32 = its[a..b].iter().map(|i| i.hyp + i.e.m_h()).sum::<i32>() + gaps;
            let free = cw - used;
            // Crecer o encoger (dos pasadas: los que chocan con su mínimo/máximo se fijan).
            for it in &mut its[a..b] {
                it.size = it.hyp;
            }
            if free > 0 {
                let mut left = free;
                for _ in 0..2 {
                    let total: f32 = its[a..b]
                        .iter()
                        .filter(|i| i.size < i.max)
                        .map(|i| i.grow)
                        .sum();
                    if total <= 0.0 || left <= 0 {
                        break;
                    }
                    let mut given = 0;
                    for it in its[a..b].iter_mut().filter(|i| i.size < i.max) {
                        let add = (left as f32 * it.grow / total) as i32;
                        let new = (it.size + add).min(it.max);
                        given += new - it.size;
                        it.size = new;
                    }
                    left -= given;
                }
            } else if free < 0 {
                let mut over = -free;
                for _ in 0..2 {
                    let total: f32 = its[a..b]
                        .iter()
                        .filter(|i| i.size > i.min)
                        .map(|i| i.shrink * i.hyp as f32)
                        .sum();
                    if total <= 0.0 || over <= 0 {
                        break;
                    }
                    let mut taken = 0;
                    for it in its[a..b].iter_mut().filter(|i| i.size > i.min) {
                        let sub = (over as f32 * it.shrink * it.hyp as f32 / total) as i32;
                        let new = (it.size - sub).max(it.min);
                        taken += it.size - new;
                        it.size = new;
                    }
                    over -= taken;
                }
            }
            // Armado de cada elemento con su ancho final.
            let mut bufs = Vec::new();
            let mut cross = 0;
            for k in a..b {
                let item_ch = if single { ch } else { None };
                let (buf, o) = self.detached_kid(&items[k], n, its[k].size, item_ch);
                cross = cross.max(o.h + its[k].e.m_v());
                bufs.push((buf, o));
            }
            if single && let Some(h) = ch {
                cross = cross.max(h);
            }
            // Posiciones en el eje principal.
            let used: i32 = its[a..b].iter().map(|i| i.size + i.e.m_h()).sum::<i32>() + gaps;
            let free = (cw - used).max(0);
            let autos: i32 = items[a..b]
                .iter()
                .zip(&its[a..b])
                .map(|(k, i)| match k {
                    Kid::El(_) => i.e.auto_l as i32 + i.e.auto_r as i32,
                    _ => 0,
                })
                .sum();
            let count = (b - a) as i32;
            let (mut x, between) = if autos > 0 {
                (0, 0)
            } else {
                match st.justify {
                    Justify::Start => (0, 0),
                    Justify::End => (free, 0),
                    Justify::Center => (free / 2, 0),
                    Justify::SpaceBetween if count > 1 => (0, free / (count - 1)),
                    Justify::SpaceBetween => (0, 0),
                    Justify::SpaceAround => (free / count / 2, free / count),
                    Justify::SpaceEvenly => (free / (count + 1), free / (count + 1)),
                }
            };
            let per_auto = if autos > 0 { free / autos } else { 0 };
            for (j, (buf, o)) in bufs.into_iter().enumerate() {
                let k = a + j;
                let it = &its[k];
                let kst = self.kid_style(&items[k], n);
                let is_el = matches!(items[k], Kid::El(_));
                if is_el && it.e.auto_l {
                    x += per_auto;
                }
                let ix = x + it.e.m[3];
                let align = if is_el {
                    kst.align_self.unwrap_or(st.align_items)
                } else {
                    st.align_items
                };
                let outer_h = o.h + it.e.m_v();
                let mut buf = buf;
                let dy = if is_el && (it.e.auto_t || it.e.auto_b) {
                    let room = cross - outer_h;
                    match (it.e.auto_t, it.e.auto_b) {
                        (true, true) => room / 2,
                        (true, false) => room,
                        _ => 0,
                    }
                } else {
                    match align {
                        Align::Center => (cross - outer_h) / 2,
                        Align::End => cross - outer_h,
                        _ => 0,
                    }
                };
                // Estirar: la caja ocupa todo el alto de la línea.
                if align == Align::Stretch
                    && is_el
                    && kst.height.is_auto()
                    && let Some(at) = o.deco
                    && let Kid::El(c) = items[k]
                {
                    let r = Rect::new(0, 0, it.size, (cross - it.e.m_v()).max(o.h));
                    self.fill_deco(&mut buf, at, c, r);
                }
                let mut px = cx + ix;
                if st.flex_dir == FlexDir::RowReverse {
                    px = cx + cw - (ix + it.size);
                }
                self.append(buf, px, y + it.e.m[0] + dy.max(0));
                x += it.size + it.e.m_h() + col_gap + between;
                if is_el && it.e.auto_r {
                    x += per_auto;
                }
            }
            y += cross;
        }
        let h = y - cy;
        ch.unwrap_or(h)
    }

    #[allow(clippy::too_many_arguments)]
    fn flex_column(
        &mut self,
        n: usize,
        st: &Style,
        items: &[Kid],
        cx: i32,
        cy: i32,
        cw: i32,
        ch: Option<i32>,
        row_gap: i32,
    ) -> i32 {
        let mut built = Vec::new();
        for k in items {
            let kst = self.kid_style(k, n);
            let is_el = matches!(k, Kid::El(_));
            let e = if is_el {
                edges(kst, cw)
            } else {
                Edges::default()
            };
            let align = if is_el {
                kst.align_self.unwrap_or(st.align_items)
            } else {
                Align::Stretch
            };
            let bw = match k {
                Kid::El(c) => {
                    if kst.width.resolve(Some(cw)).is_some()
                        || is_replaced(self.name(*c))
                        || align != Align::Stretch
                        || e.auto_l
                        || e.auto_r
                    {
                        self.shrink_width(*c, cw)
                    } else {
                        self.block_width(*c, cw, true).0
                    }
                }
                Kid::Text(_) => cw,
            };
            let (buf, o) = self.detached_kid(k, n, bw, None);
            let basis = if is_el { kst.basis.resolve(ch) } else { None };
            let main = basis.map_or(o.h, |b| b.max(o.h.min(b)));
            built.push((buf, o, e, bw, align, main));
        }
        let gaps = row_gap * (items.len() as i32 - 1);
        let used: i32 = built.iter().map(|b| b.5 + b.2.m_v()).sum::<i32>() + gaps;
        let mut free = 0;
        if let Some(h) = ch {
            free = h - used;
            if free > 0 {
                let total: f32 = items
                    .iter()
                    .map(|k| {
                        if matches!(k, Kid::El(_)) {
                            self.kid_style(k, n).grow
                        } else {
                            0.0
                        }
                    })
                    .sum();
                if total > 0.0 {
                    for (k, b) in items.iter().zip(built.iter_mut()) {
                        let g = if matches!(k, Kid::El(_)) {
                            self.kid_style(k, n).grow
                        } else {
                            0.0
                        };
                        b.5 += (free as f32 * g / total) as i32;
                    }
                    free = 0;
                }
            }
        }
        let count = items.len() as i32;
        let free = free.max(0);
        let (mut y, between) = match st.justify {
            Justify::Start => (0, 0),
            Justify::End => (free, 0),
            Justify::Center => (free / 2, 0),
            Justify::SpaceBetween if count > 1 => (0, free / (count - 1)),
            Justify::SpaceBetween => (0, 0),
            Justify::SpaceAround => (free / count / 2, free / count),
            Justify::SpaceEvenly => (free / (count + 1), free / (count + 1)),
        };
        let reverse = st.flex_dir == FlexDir::ColumnReverse;
        let total_h = ch.unwrap_or(used);
        for (k, (mut buf, o, e, bw, align, main)) in items.iter().zip(built) {
            let room = cw - bw - e.m_h();
            let dx = if e.auto_l && e.auto_r {
                room / 2
            } else if e.auto_l {
                room
            } else {
                match align {
                    Align::Center => room / 2,
                    Align::End => room,
                    _ => 0,
                }
            };
            if main != o.h
                && let Some(at) = o.deco
                && let Kid::El(c) = k
            {
                self.fill_deco(&mut buf, at, *c, Rect::new(0, 0, bw, main));
            }
            let mut py = cy + y + e.m[0];
            if reverse {
                py = cy + total_h - (y + e.m[0] + main);
            }
            self.append(buf, cx + e.m[3] + dx.max(0), py);
            y += main + e.m_v() + row_gap + between;
        }
        ch.unwrap_or(used.max(0))
    }

    // --- grid --------------------------------------------------------------------------------

    fn layout_grid(&mut self, n: usize, cx: i32, cy: i32, cw: i32, ch: Option<i32>) -> i32 {
        let st = self.sty(n);
        let kids = self.kids(n);
        let mut items: Vec<Kid> = Vec::new();
        for k in self.flex_kids(&kids) {
            if let Kid::El(c) = k
                && !self.sty(c).in_flow()
            {
                self.register_abs(c, cx, cy);
                continue;
            }
            items.push(k);
        }
        let col_gap = st.col_gap.resolve(Some(cw)).unwrap_or(0).max(0);
        let row_gap = st.row_gap.resolve(ch).unwrap_or(0).max(0);
        let areas = st.grid_areas.clone();
        // Pistas de columnas.
        let mut tracks: Vec<Track> = match &st.grid_cols {
            Some(tl) => {
                let mut t = tl.tracks.clone();
                if let Some(rep) = &tl.auto_repeat {
                    let unit: i32 =
                        rep.iter()
                            .map(|t| match t {
                                Track::Fixed(l) => l.resolve(Some(cw)).unwrap_or(0),
                                Track::MinMax(mn, _, mx) => mn.resolve(Some(cw)).unwrap_or(0).max(
                                    mx.as_ref().and_then(|m| m.resolve(Some(cw))).unwrap_or(0),
                                ),
                                _ => 0,
                            })
                            .sum::<i32>()
                            .max(1);
                    let fixed: i32 = t
                        .iter()
                        .map(|x| match x {
                            Track::Fixed(l) => l.resolve(Some(cw)).unwrap_or(0),
                            _ => 0,
                        })
                        .sum();
                    let room = cw - fixed;
                    let times = ((room + col_gap) / (unit + col_gap)).clamp(1, 64) as usize;
                    let at = tl.auto_at.min(t.len());
                    let mut expanded = Vec::new();
                    for _ in 0..times {
                        expanded.extend(rep.iter().cloned());
                    }
                    t.splice(at..at, expanded);
                }
                t
            }
            None => Vec::new(),
        };
        let area_cols = areas
            .as_ref()
            .map_or(0, |a| a.iter().map(Vec::len).max().unwrap_or(0));
        while tracks.len() < area_cols {
            tracks.push(Track::Auto);
        }
        if tracks.is_empty() {
            tracks.push(Track::Fr(1.0));
        }
        let ncols = tracks.len();
        // Ubicación de cada elemento: (fila, columna, filas, columnas).
        let mut placed: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut occupied: Vec<Vec<bool>> = Vec::new();
        let find_area = |name: &str| -> Option<(usize, usize, usize, usize)> {
            let a = areas.as_ref()?;
            let (mut r0, mut c0, mut r1, mut c1) = (usize::MAX, usize::MAX, 0, 0);
            for (r, row) in a.iter().enumerate() {
                for (c, cell) in row.iter().enumerate() {
                    if cell == name {
                        r0 = r0.min(r);
                        c0 = c0.min(c);
                        r1 = r1.max(r + 1);
                        c1 = c1.max(c + 1);
                    }
                }
            }
            (r0 != usize::MAX).then_some((r0, c0, r1 - r0, c1 - c0))
        };
        let line_idx = |l: i32, count: usize| -> usize {
            if l > 0 {
                (l - 1) as usize
            } else {
                (count as i32 + 1 + l).max(0) as usize
            }
        };
        let span_of = |start: &GridLine, end: &GridLine, count: usize| -> (Option<usize>, usize) {
            match (start, end) {
                (GridLine::Line(a), GridLine::Line(b)) => {
                    let (a, b) = (line_idx(*a, count), line_idx(*b, count));
                    let (lo, hi) = (a.min(b), a.max(b));
                    (Some(lo), (hi - lo).max(1))
                }
                (GridLine::Line(a), GridLine::Span(s)) => (Some(line_idx(*a, count)), *s as usize),
                (GridLine::Line(a), _) => (Some(line_idx(*a, count)), 1),
                (GridLine::Span(s), GridLine::Line(b)) => {
                    let b = line_idx(*b, count);
                    (Some(b.saturating_sub(*s as usize)), *s as usize)
                }
                (GridLine::Span(s), _) | (_, GridLine::Span(s)) => (None, *s as usize),
                _ => (None, 1),
            }
        };
        let mut cursor = (0usize, 0usize);
        let ensure = |occ: &mut Vec<Vec<bool>>, rows: usize| {
            while occ.len() < rows {
                occ.push(alloc::vec![false; ncols]);
            }
        };
        for k in &items {
            let kst = self.kid_style(k, n);
            let is_el = matches!(k, Kid::El(_));
            let mut pos = None;
            if is_el
                && let GridLine::Area(name) = &kst.grid_col.0
                && let Some(a) = find_area(name)
            {
                pos = Some(a);
            }
            if pos.is_none() && is_el {
                let (c0, cs) = span_of(&kst.grid_col.0, &kst.grid_col.1, ncols);
                let (r0, rs) = span_of(&kst.grid_row.0, &kst.grid_row.1, occupied.len().max(1));
                let cs = cs.min(ncols);
                match (r0, c0) {
                    (Some(r), Some(c)) => pos = Some((r, c.min(ncols - 1), rs, cs)),
                    (None, Some(c)) => {
                        // Columna fija: la primera fila libre.
                        let c = c.min(ncols - cs);
                        let mut r = 0;
                        loop {
                            ensure(&mut occupied, r + rs);
                            if (r..r + rs).all(|rr| (c..c + cs).all(|cc| !occupied[rr][cc])) {
                                break;
                            }
                            r += 1;
                        }
                        pos = Some((r, c, rs, cs));
                    }
                    (Some(r), None) => {
                        ensure(&mut occupied, r + rs);
                        let c = (0..=ncols - cs)
                            .find(|&c| {
                                (r..r + rs).all(|rr| (c..c + cs).all(|cc| !occupied[rr][cc]))
                            })
                            .unwrap_or(0);
                        pos = Some((r, c, rs, cs));
                    }
                    (None, None) if cs > 1 || rs > 1 => {
                        let (mut r, mut c) = cursor;
                        loop {
                            if c + cs > ncols {
                                c = 0;
                                r += 1;
                            }
                            ensure(&mut occupied, r + rs);
                            if (r..r + rs).all(|rr| (c..c + cs).all(|cc| !occupied[rr][cc])) {
                                break;
                            }
                            c += 1;
                        }
                        cursor = (r, c + cs);
                        pos = Some((r, c, rs, cs));
                    }
                    _ => {}
                }
            }
            let p = match pos {
                Some(p) => p,
                None => {
                    let (mut r, mut c) = cursor;
                    loop {
                        if c >= ncols {
                            c = 0;
                            r += 1;
                        }
                        ensure(&mut occupied, r + 1);
                        if !occupied[r][c] {
                            break;
                        }
                        c += 1;
                    }
                    cursor = (r, c + 1);
                    (r, c, 1, 1)
                }
            };
            let (r, c, rs, cs) = (
                p.0,
                p.1.min(ncols - 1),
                p.2.max(1),
                p.3.clamp(1, ncols - p.1.min(ncols - 1)),
            );
            ensure(&mut occupied, r + rs);
            for row in occupied.iter_mut().skip(r).take(rs) {
                for cell in row.iter_mut().skip(c).take(cs) {
                    *cell = true;
                }
            }
            placed.push((r, c, rs, cs));
        }
        let nrows = occupied
            .len()
            .max(areas.as_ref().map_or(0, |a| a.len()))
            .max(1);
        // Anchos de columnas.
        let mut widths = alloc::vec![0i32; ncols];
        let mut flex = alloc::vec![0f32; ncols];
        let mut auto_max = alloc::vec![0i32; ncols];
        for (k, &(_, c, _, cs)) in items.iter().zip(&placed) {
            if cs == 1 {
                let (_, mx) = match k {
                    Kid::El(e) => self.outer_intrinsic(*e),
                    Kid::Text(t) => self.text_intrinsic(t),
                };
                auto_max[c] = auto_max[c].max(mx);
            }
        }
        let gaps = col_gap * (ncols as i32 - 1);
        let mut used = gaps;
        for (i, t) in tracks.iter().enumerate() {
            match t {
                Track::Fixed(l) => widths[i] = l.resolve(Some(cw)).unwrap_or(0).max(0),
                Track::Fr(f) => flex[i] = *f,
                Track::MinMax(mn, f, mx) => {
                    widths[i] = mn.resolve(Some(cw)).unwrap_or(0).max(0);
                    if *f > 0.0 {
                        flex[i] = *f;
                    } else if let Some(m) = mx.as_ref().and_then(|m| m.resolve(Some(cw))) {
                        widths[i] = widths[i].max(auto_max[i].min(m));
                    }
                }
                Track::Auto => widths[i] = auto_max[i],
            }
            used += widths[i];
        }
        // Si las automáticas no entran, se achican.
        if used > cw {
            let autos: i32 = tracks
                .iter()
                .zip(&widths)
                .filter(|(t, _)| matches!(t, Track::Auto))
                .map(|(_, w)| *w)
                .sum();
            if autos > 0 {
                let over = (used - cw).min(autos);
                for (t, w) in tracks.iter().zip(widths.iter_mut()) {
                    if matches!(t, Track::Auto) {
                        *w -= (over as i64 * *w as i64 / autos as i64) as i32;
                    }
                }
                used = cw;
            }
        }
        let total_fr: f32 = flex.iter().sum();
        if total_fr > 0.0 {
            let free = (cw - used).max(0);
            let fr_base: i32 = widths
                .iter()
                .zip(&flex)
                .filter(|(_, f)| **f > 0.0)
                .map(|(w, _)| *w)
                .sum();
            let pool = free + fr_base;
            for (w, f) in widths.iter_mut().zip(&flex) {
                if *f > 0.0 {
                    *w = (*w).max((pool as f32 * f / total_fr) as i32);
                }
            }
        }
        let mut col_x = alloc::vec![0i32; ncols + 1];
        for i in 0..ncols {
            col_x[i + 1] = col_x[i] + widths[i] + col_gap;
        }
        let span_w = |c: usize, cs: usize| col_x[(c + cs).min(ncols)] - col_x[c] - col_gap;
        // Alturas de filas: se arma cada elemento.
        let mut built = Vec::new();
        let mut row_h = alloc::vec![0i32; nrows];
        let explicit_rows: Vec<Option<i32>> = (0..nrows)
            .map(|r| {
                st.grid_rows
                    .as_ref()
                    .and_then(|t| t.tracks.get(r).cloned())
                    .or_else(|| st.grid_auto_rows.clone())
                    .and_then(|t| match t {
                        Track::Fixed(l) => l.resolve(ch),
                        Track::MinMax(mn, _, _) => mn.resolve(ch),
                        _ => None,
                    })
            })
            .collect();
        for (k, &(r, c, rs, cs)) in items.iter().zip(&placed) {
            let kst = self.kid_style(k, n);
            let is_el = matches!(k, Kid::El(_));
            let area_w = span_w(c, cs).max(0);
            let e = if is_el {
                edges(kst, area_w)
            } else {
                Edges::default()
            };
            let justify = if is_el {
                kst.justify_self.unwrap_or(st.justify_items)
            } else {
                Align::Stretch
            };
            let bw = match k {
                Kid::El(el) => {
                    if justify != Align::Stretch
                        || kst.width.resolve(Some(area_w)).is_some()
                        || is_replaced(self.name(*el))
                    {
                        self.shrink_width(*el, area_w)
                    } else {
                        self.block_width(*el, area_w, true).0
                    }
                }
                Kid::Text(_) => area_w,
            };
            let (buf, o) = self.detached_kid(k, n, bw, None);
            if rs == 1 && r < nrows {
                row_h[r] = row_h[r].max(o.h + e.m_v());
            }
            built.push((buf, o, e, bw, justify));
        }
        for (r, h) in row_h.iter_mut().enumerate() {
            if let Some(x) = explicit_rows[r] {
                *h = (*h).max(x);
            }
        }
        // Los que ocupan varias filas agrandan la última si no entran.
        for ((_, o, e, _, _), &(r, _, rs, _)) in built.iter().zip(&placed) {
            if rs > 1 {
                let end = (r + rs).min(nrows);
                let have: i32 = row_h[r..end].iter().sum::<i32>() + row_gap * (end - r - 1) as i32;
                let need = o.h + e.m_v();
                if need > have && end > r {
                    row_h[end - 1] += need - have;
                }
            }
        }
        let mut row_y = alloc::vec![0i32; nrows + 1];
        for i in 0..nrows {
            row_y[i + 1] = row_y[i] + row_h[i] + row_gap;
        }
        for ((k, (mut buf, o, e, bw, justify)), &(r, c, rs, cs)) in
            items.iter().zip(built).zip(&placed)
        {
            let kst = self.kid_style(k, n);
            let is_el = matches!(k, Kid::El(_));
            let area_h = row_y[(r + rs).min(nrows)] - row_y[r.min(nrows)] - row_gap;
            let area_w = span_w(c, cs);
            let align = if is_el {
                kst.align_self.unwrap_or(st.align_items)
            } else {
                Align::Stretch
            };
            let outer_h = o.h + e.m_v();
            let dy = match align {
                Align::Center => (area_h - outer_h) / 2,
                Align::End => area_h - outer_h,
                _ => 0,
            };
            let dx = match justify {
                Align::Center => (area_w - bw - e.m_h()) / 2,
                Align::End => area_w - bw - e.m_h(),
                _ => 0,
            };
            if align == Align::Stretch
                && is_el
                && kst.height.is_auto()
                && let Some(at) = o.deco
                && let Kid::El(el) = k
            {
                self.fill_deco(
                    &mut buf,
                    at,
                    *el,
                    Rect::new(0, 0, bw, (area_h - e.m_v()).max(o.h)),
                );
            }
            self.append(
                buf,
                cx + col_x[c] + e.m[3] + dx.max(0),
                cy + row_y[r] + e.m[0] + dy.max(0),
            );
        }
        let h = (row_y[nrows] - row_gap).max(0);
        ch.unwrap_or(h)
    }

    // --- tablas ------------------------------------------------------------------------------

    /// Filas de una tabla: (nodo de la fila, celdas). Lo que no es parte de una tabla (una
    /// imagen suelta, texto) va en una celda anónima, como pide CSS.
    fn table_rows(&self, kids: &[usize]) -> Vec<(Option<usize>, Vec<TCell>)> {
        let mut heads = Vec::new();
        let mut bodies = Vec::new();
        let mut foots = Vec::new();
        let mut loose: Vec<TCell> = Vec::new();
        let mut anon: Vec<usize> = Vec::new();
        let row_cells = |this: &Self, r: usize| -> Vec<TCell> {
            let mut cells = Vec::new();
            let mut anon = Vec::new();
            for c in this.kids(r) {
                if !this.is_text(c) && this.sty(c).display == Display::TableCell {
                    if !anon.is_empty() {
                        cells.push(TCell::Anon(mem::take(&mut anon)));
                    }
                    cells.push(TCell::Node(c));
                } else if !(this.is_text(c) && is_blank(this.text_of(c))) {
                    anon.push(c);
                }
            }
            if !anon.is_empty() {
                cells.push(TCell::Anon(anon));
            }
            cells
        };
        let flush = |bodies: &mut Vec<(Option<usize>, Vec<TCell>)>,
                     loose: &mut Vec<TCell>,
                     anon: &mut Vec<usize>| {
            if !anon.is_empty() {
                loose.push(TCell::Anon(mem::take(anon)));
            }
            if !loose.is_empty() {
                bodies.push((None, mem::take(loose)));
            }
        };
        for &c in kids {
            if self.is_text(c) {
                if !is_blank(self.text_of(c)) {
                    anon.push(c);
                }
                continue;
            }
            match self.sty(c).display {
                Display::TableCaption => {}
                Display::TableRowGroup => {
                    flush(&mut bodies, &mut loose, &mut anon);
                    let target = match self.name(c) {
                        "thead" => &mut heads,
                        "tfoot" => &mut foots,
                        _ => &mut bodies,
                    };
                    for r in self.kids(c) {
                        if self.is_text(r) {
                            continue;
                        }
                        match self.sty(r).display {
                            Display::TableRow => target.push((Some(r), row_cells(self, r))),
                            Display::TableCell => target.push((None, alloc::vec![TCell::Node(r)])),
                            _ => {}
                        }
                    }
                }
                Display::TableRow => {
                    flush(&mut bodies, &mut loose, &mut anon);
                    bodies.push((Some(c), row_cells(self, c)));
                }
                Display::TableCell => {
                    if !anon.is_empty() {
                        loose.push(TCell::Anon(mem::take(&mut anon)));
                    }
                    loose.push(TCell::Node(c));
                }
                _ => anon.push(c),
            }
        }
        flush(&mut bodies, &mut loose, &mut anon);
        let mut rows = heads;
        rows.extend(bodies);
        rows.extend(foots);
        rows
    }

    /// Arma una celda aparte (con su esquina en 0, 0).
    fn detached_cell(&mut self, table: usize, c: &TCell, w: i32) -> (Vec<Paint>, Out) {
        match c {
            TCell::Node(n) => self.detached(*n, w, None),
            TCell::Anon(kids) => {
                let saved = mem::take(&mut self.paints);
                let mut floats = Floats::default();
                let o = self.flow_list(table, kids, 0, 0, w, None, &mut floats);
                let buf = mem::replace(&mut self.paints, saved);
                (buf, o)
            }
        }
    }

    fn captions(&mut self, kids: &[usize], bottom: bool, cx: i32, y: &mut i32, cw: i32) {
        for &c in kids {
            if !self.is_text(c)
                && self.sty(c).display == Display::TableCaption
                && self.sty(c).caption_bottom == bottom
            {
                let (bw, ml, _) = self.block_width(c, cw, true);
                let mut fl = Floats::default();
                let o = self.layout_box(c, cx + ml, *y, bw, None, &mut fl);
                *y += o.h;
            }
        }
    }

    fn layout_table(&mut self, n: usize, kids: &[usize], cx: i32, cy: i32, cw: i32) -> i32 {
        let st = self.sty(n);
        let is_table = matches!(st.display, Display::Table | Display::InlineTable);
        let spacing = if !is_table || st.border_collapse {
            0
        } else {
            st.border_spacing
        };
        let mut y = cy;
        // Títulos (`<caption>`) de arriba.
        self.captions(kids, false, cx, &mut y, cw);
        let rows = self.table_rows(kids);
        // Grilla con colspan/rowspan: (celda, fila, col, filas, cols)
        let mut occ: Vec<Vec<bool>> = Vec::new();
        let mut cells: Vec<(TCell, usize, usize, usize, usize)> = Vec::new();
        for (ri, (_, rc)) in rows.iter().enumerate() {
            while occ.len() <= ri {
                occ.push(Vec::new());
            }
            let mut col = 0;
            for c in rc {
                let (cs, rs) = match c {
                    TCell::Node(c) => {
                        let node = &self.p.dom.nodes[*c];
                        let num = |a: &str| {
                            node.attr(a)
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(1)
                                .clamp(1, 64)
                        };
                        (num("colspan"), num("rowspan"))
                    }
                    TCell::Anon(_) => (1, 1),
                };
                let rs = rs.min(rows.len() - ri).max(1);
                while occ[ri].get(col).copied().unwrap_or(false) {
                    col += 1;
                }
                for r in ri..ri + rs {
                    while occ.len() <= r {
                        occ.push(Vec::new());
                    }
                    if occ[r].len() < col + cs {
                        occ[r].resize(col + cs, false);
                    }
                    for cell in &mut occ[r][col..col + cs] {
                        *cell = true;
                    }
                }
                cells.push((c.clone(), ri, col, rs, cs));
                col += cs;
            }
        }
        let ncols = occ.iter().map(Vec::len).max().unwrap_or(0);
        let nrows = rows.len();
        if ncols == 0 {
            self.captions(kids, true, cx, &mut y, cw);
            return y - cy;
        }
        // Mínimos y máximos por columna.
        let mut cmin = alloc::vec![0i32; ncols];
        let mut cmax = alloc::vec![0i32; ncols];
        let mut spans = Vec::new();
        for (c, _, col, _, cs) in &cells {
            let (mut mn, mut mx) = self.cell_intrinsic(c);
            if let TCell::Node(c) = c {
                let cst = self.sty(*c);
                if let Some(w) = cst.width.resolve(Some(cw)) {
                    let w = to_border_box(cst, &edges(cst, cw), w);
                    mn = mn.max(w.min(cw));
                    mx = mx.max(w);
                }
            }
            if *cs == 1 {
                cmin[*col] = cmin[*col].max(mn);
                cmax[*col] = cmax[*col].max(mx);
            } else {
                spans.push((*col, *cs, mn, mx));
            }
        }
        for (col, cs, mn, mx) in spans {
            let end = (col + cs).min(ncols);
            let have_min: i32 = cmin[col..end].iter().sum::<i32>() + spacing * (cs as i32 - 1);
            let have_max: i32 = cmax[col..end].iter().sum::<i32>() + spacing * (cs as i32 - 1);
            let k = (end - col).max(1) as i32;
            if mn > have_min {
                for v in &mut cmin[col..end] {
                    *v += (mn - have_min) / k;
                }
            }
            if mx > have_max {
                for v in &mut cmax[col..end] {
                    *v += (mx - have_max) / k;
                }
            }
        }
        for i in 0..ncols {
            cmax[i] = cmax[i].max(cmin[i]);
        }
        let avail = (cw - spacing * (ncols as i32 + 1)).max(0);
        let smin: i32 = cmin.iter().sum();
        let smax: i32 = cmax.iter().sum();
        let fixed_width = is_table && !st.width.is_auto();
        let widths: Vec<i32> = if smax <= avail {
            if fixed_width && smax > 0 {
                let extra = avail - smax;
                cmax.iter()
                    .map(|w| w + (extra as i64 * *w as i64 / smax as i64) as i32)
                    .collect()
            } else if fixed_width {
                alloc::vec![avail / ncols as i32; ncols]
            } else {
                cmax.clone()
            }
        } else if smin >= avail {
            cmin.clone()
        } else {
            let extra = avail - smin;
            let range = (smax - smin).max(1);
            cmin.iter()
                .zip(&cmax)
                .map(|(a, b)| a + (extra as i64 * (b - a) as i64 / range as i64) as i32)
                .collect()
        };
        let mut col_x = alloc::vec![0i32; ncols + 1];
        for i in 0..ncols {
            col_x[i + 1] = col_x[i] + widths[i] + spacing;
        }
        let span_w = |col: usize, cs: usize| col_x[(col + cs).min(ncols)] - col_x[col] - spacing;
        // Cada celda.
        let mut built = Vec::new();
        let mut row_h = alloc::vec![0i32; nrows];
        for (ri, (rn, _)) in rows.iter().enumerate() {
            if let Some(r) = rn
                && let Some(h) = self.sty(*r).height.resolve(None)
            {
                row_h[ri] = h;
            }
        }
        for (c, r, col, rs, cs) in &cells {
            let w = span_w(*col, *cs).max(0);
            let (buf, o) = self.detached_cell(n, c, w);
            if *rs == 1 {
                row_h[*r] = row_h[*r].max(o.h);
            }
            built.push((buf, o, w));
        }
        for ((_, r, _, rs, _), (_, o, _)) in cells.iter().zip(&built) {
            if *rs > 1 {
                let end = (r + rs).min(nrows);
                let have: i32 = row_h[*r..end].iter().sum::<i32>() + spacing * (end - r - 1) as i32;
                if o.h > have {
                    row_h[end - 1] += o.h - have;
                }
            }
        }
        let mut row_y = alloc::vec![0i32; nrows + 1];
        row_y[0] = y + spacing;
        for i in 0..nrows {
            row_y[i + 1] = row_y[i] + row_h[i] + spacing;
        }
        let table_w = col_x[ncols] + spacing;
        // Fondos de las filas.
        for (ri, (rn, _)) in rows.iter().enumerate() {
            if let Some(r) = rn {
                let rst = self.sty(*r);
                if let Some(bg) = rst.bg
                    && !self.env.dark
                    && self.visible(rst)
                {
                    self.paints.push(Paint::Rect {
                        r: Rect::new(cx, row_y[ri], table_w, row_h[ri]),
                        color: bg,
                        radius: 0,
                        node: *r as u32,
                    });
                }
            }
        }
        for ((c, r, col, rs, _), (mut buf, o, w)) in cells.iter().zip(built) {
            let cell_h = row_y[(r + rs).min(nrows)] - row_y[*r] - spacing;
            let valign = match c {
                TCell::Node(c) => self.sty(*c).valign,
                TCell::Anon(_) => VAlign::Top,
            };
            let off = match valign {
                VAlign::Top | VAlign::Baseline => 0,
                VAlign::Bottom => cell_h - o.h,
                _ => (cell_h - o.h) / 2,
            }
            .max(0);
            if let (Some(at), TCell::Node(c)) = (o.deco, c) {
                self.fill_deco(&mut buf, at, *c, Rect::new(0, -off, w, cell_h));
            }
            self.append(buf, cx + spacing + col_x[*col], row_y[*r] + off);
        }
        y = row_y[nrows];
        self.captions(kids, true, cx, &mut y, table_w.min(cw).max(0));
        y - cy
    }

    // --- posicionados ------------------------------------------------------------------------

    fn layout_abs(&mut self, p: &Pending, cb: Rect) {
        let n = p.node;
        let st = self.sty(n);
        let e = edges(st, cb.w);
        let l = st.inset[3].resolve(Some(cb.w));
        let r = st.inset[1].resolve(Some(cb.w));
        let t = st.inset[0].resolve(Some(cb.h));
        let b = st.inset[2].resolve(Some(cb.h));
        // Escondido "fuera de la pantalla" (menús cerrados, textos para lectores de pantalla).
        if l.is_some_and(|v| v < -500)
            || t.is_some_and(|v| v < -500)
            || r.is_some_and(|v| v < -2000)
        {
            return;
        }
        let bw = if let Some(w) = st.width.resolve(Some(cb.w)) {
            to_border_box(st, &e, w)
        } else if let (Some(l), Some(r)) = (l, r) {
            if is_replaced(self.name(n)) {
                self.shrink_width(n, cb.w)
            } else {
                (cb.w - l - r - e.m_h()).max(e.pb_h())
            }
        } else {
            self.shrink_width(n, (cb.w - l.unwrap_or(0)).max(0))
        };
        let cb_h = if t.is_some() && b.is_some() && st.height.is_auto() {
            None
        } else {
            Some(cb.h)
        };
        let (buf, o) = self.detached(n, bw, cb_h);
        let mut h = o.h;
        let mut buf = buf;
        if let (Some(t), Some(b)) = (t, b)
            && st.height.is_auto()
        {
            let want = cb.h - t - b - e.m_v();
            if want > h {
                h = want;
                if let Some(at) = o.deco {
                    self.fill_deco(&mut buf, at, n, Rect::new(0, 0, bw, h));
                }
            }
        }
        if (bw <= 2 && h <= 2) || st.clip_x && bw <= 1 {
            return;
        }
        let x = match (l, r) {
            (Some(l), _) => cb.x + l + e.m[3],
            (None, Some(r)) => cb.x + cb.w - r - bw - e.m[1],
            _ => p.sx + e.m[3],
        };
        let y = match (t, b) {
            (Some(t), _) => cb.y + t + e.m[0],
            (None, Some(b)) => cb.y + cb.h - b - h - e.m[2],
            _ => p.sy + e.m[0],
        };
        if x + bw < -50 || y + h < -50 {
            return;
        }
        self.append(buf, x, y);
    }
}

fn static_root() -> &'static Style {
    // Un estilo por defecto para nodos sin padre con estilo (no debería pasar).
    use core::sync::atomic::{AtomicPtr, Ordering};
    static ROOT: AtomicPtr<Style> = AtomicPtr::new(core::ptr::null_mut());
    let p = ROOT.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: el puntero viene de `Box::leak` más abajo y nunca se libera.
        return unsafe { &*p };
    }
    let leaked: &'static mut Style = alloc::boxed::Box::leak(alloc::boxed::Box::new(Style::root()));
    ROOT.store(leaked as *mut Style, Ordering::Release);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::css::Media;
    use crate::web::html::{Options, prepare};

    fn fonts() -> &'static WebFonts {
        extern crate std;
        // Una por hilo (las cachés son `RefCell`); cargarlas es lo más lento de cada test.
        std::thread_local! {
            static T: &'static WebFonts = alloc::boxed::Box::leak(alloc::boxed::Box::new(WebFonts::new()));
        }
        T.with(|f| *f)
    }

    fn page(html: &str, width: i32) -> Page {
        let p = prepare(html, Options::STYLED, &[], Media { width, height: 600 });
        let env = Env {
            fonts: fonts(),
            image_sizes: &[],
            width,
            height: 600,
            dark: false,
        };
        layout(&p, &env)
    }

    fn text_at(p: &Page, word: &str) -> (i32, i32, i32) {
        p.paints
            .iter()
            .find_map(|x| match x {
                Paint::Text {
                    x, base, w, text, ..
                } if text == word => Some((*x, *base, *w)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no está {word:?}: {}", p.text()))
    }

    fn rect_of(p: &Page, color: u32) -> Rect {
        p.paints
            .iter()
            .find_map(|x| match x {
                Paint::Rect { r, color: c, .. } if c.c == Color::hex(color) => Some(*r),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no hay fondo {color:06x}"))
    }

    #[test]
    fn bloques_margenes_y_centrado() {
        let p = page(
            "<body style='margin:0'><div style='width:400px;margin:20px auto;padding:10px;background:#ff0000'>Hola</div>\
             <p style='margin:30px 0'>uno</p><p style='margin:10px 0'>dos</p></body>",
            1000,
        );
        let r = rect_of(&p, 0xff0000);
        assert_eq!((r.x, r.y, r.w), (290, 20, 420), "centrado con margin auto");
        let (hx, _, _) = text_at(&p, "Hola");
        assert_eq!(hx, 300, "relleno");
        let (_, b1, _) = text_at(&p, "uno");
        let (_, b2, _) = text_at(&p, "dos");
        // Entre los párrafos: 30px (el margen mayor), no 40.
        let gap = b2 - b1;
        assert!(gap > 30 && gap < 60, "márgenes colapsados: {gap}");
    }

    #[test]
    fn renglones_proporcionales_y_alineados() {
        let p = page(
            "<div style='width:200px;text-align:center'>palabra otra más larga que un renglón entero</div>",
            800,
        );
        let mut bases: Vec<i32> = p
            .paints
            .iter()
            .filter_map(|x| match x {
                Paint::Text { base, x, w, .. } => {
                    assert!(*x >= 0 && x + w <= 201, "dentro del ancho: {x} {w}");
                    Some(*base)
                }
                _ => None,
            })
            .collect();
        bases.dedup();
        assert!(bases.len() >= 2, "varios renglones");
        // El último renglón es corto: queda centrado.
        let (x, _, w) = text_at(&p, "entero");
        assert!(x > 20 && x + w < 190, "centrado: {x}");
    }

    #[test]
    fn flex_en_fila_con_espacios_y_margen_auto() {
        let p = page(
            "<body style='margin:0'><nav style='display:flex;gap:10px;align-items:center;height:50px'>\
             <a href=/a>Uno</a><a href=/b>Dos</a><span style='margin-left:auto'>Fin</span></nav>\
             <div style='display:flex'><div style='flex:1;background:#00ff00'>a</div><div style='width:300px;background:#0000ff'>b</div></div></body>",
            1000,
        );
        let (x1, b1, w1) = text_at(&p, "Uno");
        let (x2, b2, _) = text_at(&p, "Dos");
        let (x3, _, w3) = text_at(&p, "Fin");
        assert_eq!(b1, b2, "misma fila");
        assert_eq!(x2, x1 + w1 + 10, "gap");
        assert_eq!(x3 + w3, 1000, "margin-left: auto la manda a la derecha");
        assert!(b1 > 20 && b1 < 40, "centrado en 50px: {b1}");
        let g = rect_of(&p, 0x00ff00);
        let bl = rect_of(&p, 0x0000ff);
        assert_eq!((g.x, g.w), (0, 700), "flex: 1 ocupa lo que sobra");
        assert_eq!((bl.x, bl.w), (700, 300));
        assert_eq!(g.h, bl.h, "se estiran al mismo alto");
    }

    #[test]
    fn grilla_con_areas_y_columnas() {
        let p = page(
            "<body style='margin:0'><div style='display:grid;grid-template-columns:200px 1fr;grid-template-areas:\"lado principal\";column-gap:20px'>\
             <main style='grid-area:principal;background:#ff0000'>Contenido</main><aside style='grid-area:lado;background:#00ff00'>Menú</aside></div>\
             <ul style='display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));list-style:none;padding:0;margin:0'>\
             <li>a</li><li>b</li><li>c</li><li>d</li><li>e</li><li>f</li></ul></body>",
            1000,
        );
        let main = rect_of(&p, 0xff0000);
        let side = rect_of(&p, 0x00ff00);
        assert_eq!((side.x, side.w), (0, 200));
        assert_eq!((main.x, main.w), (220, 780));
        assert_eq!(main.y, side.y);
        let (ax, ab, _) = text_at(&p, "a");
        let (ex, eb, _) = text_at(&p, "e");
        let (fx, fb, _) = text_at(&p, "f");
        assert_eq!(ab, eb, "cinco por fila en 1000px");
        assert_eq!(ex, 800);
        assert!(fb > ab && fx == ax, "la sexta baja");
    }

    #[test]
    fn flotantes_y_tablas() {
        let p = page(
            "<body style='margin:0'><div style='float:left;width:100px;height:100px;background:#ff0000'></div>\
             <p style='margin:0'>texto al lado</p><div style='clear:both'>abajo</div>\
             <table><tr><td>celda uno</td><td>dos</td></tr><tr><td colspan=2>larga</td></tr></table></body>",
            800,
        );
        let (tx, tb, _) = text_at(&p, "texto");
        assert!(tx >= 100, "rodea el flotante: {tx}");
        assert!(tb < 30);
        let (_, ab, _) = text_at(&p, "abajo");
        assert!(ab > 100, "clear");
        let (x1, b1, _) = text_at(&p, "celda");
        let (x2, b2, _) = text_at(&p, "dos");
        let (x3, b3, _) = text_at(&p, "larga");
        assert_eq!(b1, b2);
        assert!(x2 > x1 + 40);
        assert!(b3 > b1 && x3 == x1);
    }

    #[test]
    fn absolutos_ocultos_y_listas() {
        let p = page(
            "<body style='margin:0'><div style='position:relative;height:100px'>\
             <span style='position:absolute;right:0;bottom:0'>esquina</span>\
             <span style='position:absolute;left:-9999px'>escondido</span>\
             <span style='visibility:hidden'>invisible</span></div>\
             <ol><li>uno</li><li>dos</li></ol></body>",
            800,
        );
        let (x, b, w) = text_at(&p, "esquina");
        assert_eq!(x + w, 800);
        assert!(b > 80 && b <= 100);
        assert!(!p.text().contains("escondido"));
        assert!(!p.text().contains("invisible"));
        assert!(p.text().contains("1."));
        assert!(p.text().contains("2."));
    }
}
