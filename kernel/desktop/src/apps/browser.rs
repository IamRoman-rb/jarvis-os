//! Navegador web de texto: barra de direcciones, atrás/adelante, enlaces y buscador.
//!
//! Las páginas se piden al kernel (que tiene la red) por el `Outbox` y la respuesta vuelve con
//! [`Browser::net_response`]. El HTML se convierte en un documento de texto (`web::html`) y acá
//! se arma en líneas del ancho de la ventana.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::shapes::{line, rounded_outline, rounded_rect};
use jarvis_gfx::text::{self, Size, Style, Weight};
use jarvis_gfx::{Canvas, Color, Rect, theme};

use super::{Click, Ctx};
use crate::input::{Key, Mods};
use crate::system::HttpResponse;
use crate::text_input::TextInput;
use crate::web::html::{self, BlockKind, Document};
use crate::web::url::{self, Scheme, Url};
use crate::widgets::{FIELD_BG, WINDOW_BG, draw_fit, label, light, s16};

const BAR_H: i32 = 48;
const STATUS_H: i32 = 26;
const MARGIN: i32 = 28;
const LINK: Color = Color::hex(0x5cc8ff);
const LINK_FOCUS: Color = Color::hex(0xffd166);

pub const HOME: &str = "about:inicio";

const HOME_HTML: &str = "<title>Inicio</title>\
<h1>JARVIS-OS · Navegador</h1>\
<p>Escribí una dirección o lo que quieras buscar en la barra de arriba (Ctrl+L) y apretá Enter.</p>\
<h2>Para probar</h2><ul>\
<li><a href=\"http://example.com/\">example.com</a> · una página de ejemplo (HTTP directo)</li>\
<li><a href=\"http://info.cern.ch/hypertext/WWW/TheProject.html\">La primera página web</a> (CERN, 1991)</li>\
<li><a href=\"https://es.wikipedia.org/wiki/Sistema_operativo\">Wikipedia: Sistema operativo</a></li>\
<li><a href=\"https://html.duckduckgo.com/html/?q=rust+osdev\">Buscar \"rust osdev\"</a></li>\
<li><a href=\"https://lite.cnn.com/\">CNN Lite</a> · noticias en texto</li>\
</ul><h2>Cómo funciona</h2>\
<p>El kernel tiene su propio driver de placa de red (virtio-net) y una pila TCP/IP (smoltcp): \
pide una dirección por DHCP, resuelve nombres por DNS y abre la conexión TCP. Las páginas \
<b>http://</b> van directo. Las <b>https://</b> necesitan cifrado TLS, que todavía no está en el \
kernel: se piden a un puente en el anfitrión que levanta <b>cargo xtask run</b>.</p>\
<p>No hay JavaScript ni CSS: se muestra el texto, los títulos, las listas y los enlaces, como en \
un navegador de texto.</p>\
<h2>Teclas</h2><ul>\
<li>Ctrl+L: escribir una dirección · Enter: ir</li>\
<li>Tab: siguiente enlace · Enter: abrirlo · clic: abrirlo</li>\
<li>Alt+Izquierda / Retroceso: atrás · Alt+Derecha: adelante · F5: recargar</li>\
<li>Flechas, RePág, AvPág, Espacio y la rueda del mouse: moverse por la página</li></ul>";

#[derive(Clone)]
struct Item {
    x: i32,
    text: String,
    style: Style,
    link: Option<usize>,
}

struct Line {
    y: i32,
    h: i32,
    items: Vec<Item>,
    rule: bool,
}

enum State {
    Loading { id: u32, url: String, since: u64 },
    Page,
    Error(String),
}

pub struct Browser {
    pub dirty: bool,
    address: TextInput,
    editing: bool,
    /// La primera tecla reemplaza todo (como cuando se selecciona la barra en un navegador).
    replace_on_type: bool,
    pub url: Option<Url>,
    doc: Document,
    state: State,
    back: Vec<String>,
    forward: Vec<String>,
    scroll: i32,
    layout: Option<(i32, Vec<Line>, i32)>,
    focus_link: Option<usize>,
    status: String,
}

fn styles() -> [Style; 5] {
    [
        Style::new(Weight::Regular, Size::Size16, theme::TEXT),
        Style::new(Weight::Bold, Size::Size32, Color::WHITE),
        Style::new(Weight::Bold, Size::Size20, theme::CYAN),
        Style::new(Weight::Bold, Size::Size16, theme::PARTICLE_BRIGHT),
        Style::new(Weight::Bold, Size::Size16, theme::TEXT),
    ]
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    pub fn new() -> Self {
        Browser {
            dirty: true,
            address: TextInput::new("", 500),
            editing: false,
            replace_on_type: false,
            url: None,
            doc: Document::default(),
            state: State::Page,
            back: Vec::new(),
            forward: Vec::new(),
            scroll: 0,
            layout: None,
            focus_link: None,
            status: String::new(),
        }
    }

    pub fn title(&self) -> String {
        let t = if self.doc.title.is_empty() {
            self.url
                .as_ref()
                .map(|u| u.host.clone())
                .unwrap_or_default()
        } else {
            self.doc.title.clone()
        };
        if t.is_empty() {
            "Navegador".into()
        } else {
            format!("{t} · Navegador")
        }
    }

    pub fn current_url(&self) -> String {
        self.url.as_ref().map(|u| u.to_string()).unwrap_or_default()
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.state, State::Loading { .. })
    }

    /// Texto visible de la página (para los tests).
    pub fn page_text(&self) -> String {
        self.doc
            .blocks
            .iter()
            .map(|b| b.spans.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Lo escrito en la barra (o una dirección ya armada) → navegar. `record`: guardar la página
    /// actual en el historial.
    pub fn go<D: BlockDevice>(&mut self, input: &str, ctx: &mut Ctx<'_, D>) {
        self.navigate(&url::from_input(input), true, ctx);
    }

    fn navigate<D: BlockDevice>(&mut self, target: &str, record: bool, ctx: &mut Ctx<'_, D>) {
        // Los resultados de DuckDuckGo pasan por un redireccionador: se va directo al destino.
        let mut target = target.to_string();
        if let Some(u) = Url::parse(&target)
            && u.host.ends_with("duckduckgo.com")
            && u.path.starts_with("/l/")
            && let Some(real) = u.query_param("uddg")
        {
            target = real;
        }
        let Some(u) = Url::parse(&target) else {
            self.show_error(format!("Dirección inválida: {target}"));
            return;
        };
        if record && let Some(cur) = &self.url {
            self.back.push(cur.to_string());
            self.forward.clear();
        }
        self.editing = false;
        self.address.text = u.to_string();
        self.url = Some(u.clone());
        self.scroll = 0;
        self.focus_link = None;
        self.layout = None;
        self.dirty = true;
        ctx.log.push(format!("NAVEGADOR_IR {u}"));
        match u.scheme {
            Scheme::About => {
                let page = if u.path == "inicio" || u.path.is_empty() {
                    HOME_HTML
                } else {
                    "<title>No existe</title><p>Esa página interna no existe.</p>"
                };
                self.show(html::parse(page));
            }
            Scheme::File => {
                let path = url::percent_decode(u.path_only());
                match ctx.fs.as_deref_mut().map(|fs| fs.read_file(&path)) {
                    Some(Ok(bytes)) => {
                        let text = html::decode_bytes(&bytes);
                        let lower = path.to_ascii_lowercase();
                        if lower.ends_with(".html") || lower.ends_with(".htm") {
                            self.show(html::parse(&text));
                        } else {
                            self.show(html::plain(&text));
                        }
                    }
                    Some(Err(e)) => self.show_error(format!("{path}: {e}")),
                    None => self.show_error("No hay disco.".into()),
                }
            }
            Scheme::Http | Scheme::Https => {
                let id = ctx.out.fetch(&u.to_string());
                self.state = State::Loading {
                    id,
                    url: u.to_string(),
                    since: ctx.now_ms,
                };
                self.status = format!("Conectando con {}...", u.host);
            }
        }
    }

    fn show(&mut self, doc: Document) {
        self.doc = doc;
        self.state = State::Page;
        self.layout = None;
        self.status = String::new();
        self.dirty = true;
    }

    fn show_error(&mut self, msg: String) {
        self.doc = Document::default();
        self.state = State::Error(msg);
        self.layout = None;
        self.dirty = true;
    }

    pub fn net_response(&mut self, id: u32, result: &Result<HttpResponse, String>) {
        let State::Loading { id: waiting, .. } = self.state else {
            return;
        };
        if waiting != id {
            return;
        }
        match result {
            Err(e) => self.show_error(format!("No se pudo abrir la página: {e}")),
            Ok(resp) => {
                if let Some(u) = Url::parse(&resp.url) {
                    self.address.text = u.to_string();
                    self.url = Some(u);
                }
                let ct = resp.content_type.to_ascii_lowercase();
                let text = html::decode_bytes(&resp.body);
                let mut doc = if ct.contains("html") || (ct.is_empty() && text.contains("<html")) {
                    html::parse(&text)
                } else if ct.starts_with("text/") || ct.contains("json") || ct.contains("xml") {
                    html::plain(&text)
                } else {
                    html::parse(&format!(
                        "<h2>No sé mostrar este contenido</h2><p>Tipo: {} · {} bytes.</p>",
                        resp.content_type,
                        resp.body.len()
                    ))
                };
                if resp.status >= 400 {
                    doc.title = format!("Error {}", resp.status);
                }
                self.show(doc);
                self.status = format!("{} · {} bytes", resp.status, resp.body.len());
            }
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        // Mientras carga, el indicador se anima (4 veces por segundo).
        if let State::Loading { since, .. } = self.state
            && (now_ms - since) % 250 < 20
        {
            self.dirty = true;
        }
    }

    fn history_back<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(prev) = self.back.pop() {
            if let Some(cur) = &self.url {
                self.forward.push(cur.to_string());
            }
            self.navigate(&prev, false, ctx);
        }
    }

    fn history_forward<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(next) = self.forward.pop() {
            if let Some(cur) = &self.url {
                self.back.push(cur.to_string());
            }
            self.navigate(&next, false, ctx);
        }
    }

    fn reload<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(u) = self.url.clone() {
            self.navigate(&u.to_string(), false, ctx);
        }
    }

    fn follow<D: BlockDevice>(&mut self, link: usize, ctx: &mut Ctx<'_, D>) {
        let Some(href) = self.doc.links.get(link).cloned() else {
            return;
        };
        let target = match &self.url {
            Some(base) => base.join(&href),
            None => Url::parse(&href),
        };
        match target {
            Some(t) => self.navigate(&t.to_string(), true, ctx),
            None => {
                self.status = format!("Ese enlace no se puede abrir: {href}");
                self.dirty = true;
            }
        }
    }

    // --- diseño -------------------------------------------------------------------------------

    fn page_rect(content: Rect) -> Rect {
        Rect::new(
            content.x,
            content.y + BAR_H,
            content.w,
            content.h - BAR_H - STATUS_H,
        )
    }

    fn ensure_layout(&mut self, width: i32) {
        if self.layout.as_ref().is_some_and(|l| l.0 == width) {
            return;
        }
        let (lines, total) = layout(&self.doc, width - 2 * MARGIN);
        self.layout = Some((width, lines, total));
    }

    fn max_scroll(&self, page: Rect) -> i32 {
        self.layout
            .as_ref()
            .map_or(0, |l| (l.2 - page.h + 40).max(0))
    }

    fn scroll_by(&mut self, dy: i32, content: Rect) {
        let page = Self::page_rect(content);
        self.ensure_layout(page.w);
        let next = (self.scroll + dy).clamp(0, self.max_scroll(page));
        if next != self.scroll {
            self.scroll = next;
            self.dirty = true;
        }
    }

    /// Tab: el próximo enlace (lo pone a la vista).
    fn focus_next(&mut self, forward: bool, content: Rect) {
        let page = Self::page_rect(content);
        self.ensure_layout(page.w);
        let Some((_, lines, _)) = &self.layout else {
            return;
        };
        let mut order: Vec<(usize, i32)> = Vec::new();
        for l in lines {
            for it in &l.items {
                if let Some(k) = it.link
                    && !order.iter().any(|(o, _)| *o == k)
                {
                    order.push((k, l.y));
                }
            }
        }
        if order.is_empty() {
            return;
        }
        let pos = self
            .focus_link
            .and_then(|f| order.iter().position(|(k, _)| *k == f));
        let next = match (pos, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(p), true) => (p + 1) % order.len(),
            (Some(p), false) => (p + order.len() - 1) % order.len(),
        };
        let (link, y) = order[next];
        self.focus_link = Some(link);
        if y < self.scroll || y > self.scroll + page.h - 60 {
            self.scroll = (y - page.h / 3).clamp(0, self.max_scroll(page));
        }
        self.status = self.doc.links.get(link).cloned().unwrap_or_default();
        self.dirty = true;
    }

    pub fn key<D: BlockDevice>(
        &mut self,
        key: Key,
        mods: Mods,
        content: Rect,
        ctx: &mut Ctx<'_, D>,
    ) -> bool {
        self.dirty = true;
        if (mods.ctrl && matches!(key, Key::Char('l' | 'L')))
            || key == Key::F(6)
            || (mods.alt && matches!(key, Key::Char('d' | 'D')))
        {
            self.editing = true;
            self.replace_on_type = true;
            return true;
        }
        if self.editing {
            match key {
                Key::Enter => {
                    let input = self.address.text.clone();
                    if !input.trim().is_empty() {
                        self.go(&input, ctx);
                    }
                }
                Key::Escape => {
                    self.editing = false;
                    self.address.text = self.current_url();
                }
                other => {
                    if self.replace_on_type && matches!(other, Key::Char(_)) {
                        self.address.text.clear();
                    }
                    self.replace_on_type = false;
                    self.address.handle(other);
                }
            }
            return true;
        }
        let page_h = Self::page_rect(content).h;
        match key {
            Key::Left if mods.alt => self.history_back(ctx),
            Key::Right if mods.alt => self.history_forward(ctx),
            Key::Backspace => self.history_back(ctx),
            Key::F(5) => self.reload(ctx),
            Key::Char('r' | 'R') if mods.ctrl => self.reload(ctx),
            Key::Char('h' | 'H') if mods.ctrl => self.navigate(HOME, true, ctx),
            Key::Up => self.scroll_by(-40, content),
            Key::Down => self.scroll_by(40, content),
            Key::PageUp => self.scroll_by(-(page_h - 60), content),
            Key::PageDown | Key::Char(' ') => self.scroll_by(page_h - 60, content),
            Key::Home => self.scroll_by(-1_000_000, content),
            Key::End => self.scroll_by(1_000_000, content),
            Key::Tab => self.focus_next(!mods.shift, content),
            Key::Enter => {
                if let Some(link) = self.focus_link {
                    self.follow(link, ctx);
                }
            }
            Key::Escape if self.is_loading() => {
                self.state = State::Error("Carga cancelada.".into());
            }
            _ => return false,
        }
        true
    }

    fn buttons(content: Rect) -> [Rect; 4] {
        let y = content.y + 8;
        let at = |i: i32| Rect::new(content.x + 10 + i * 38, y, 32, 32);
        [at(0), at(1), at(2), at(3)]
    }

    fn address_rect(content: Rect) -> Rect {
        Rect::new(content.x + 170, content.y + 8, content.w - 184, 32)
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        let [back, fwd, reload, home] = Self::buttons(content);
        let (x, y) = (click.x, click.y);
        if back.contains(x, y) {
            self.history_back(ctx);
        } else if fwd.contains(x, y) {
            self.history_forward(ctx);
        } else if reload.contains(x, y) {
            self.reload(ctx);
        } else if home.contains(x, y) {
            self.navigate(HOME, true, ctx);
        } else if Self::address_rect(content).contains(x, y) {
            self.editing = true;
            self.replace_on_type = true;
            self.dirty = true;
        } else {
            let page = Self::page_rect(content);
            if !page.contains(x, y) {
                return;
            }
            if self.editing {
                self.editing = false;
                self.address.text = self.current_url();
                self.dirty = true;
            }
            if let Some(link) = self.link_at(page, x, y) {
                self.focus_link = Some(link);
                self.follow(link, ctx);
            }
        }
    }

    fn link_at(&mut self, page: Rect, x: i32, y: i32) -> Option<usize> {
        self.ensure_layout(page.w);
        let (_, lines, _) = self.layout.as_ref()?;
        let py = y - page.y + self.scroll;
        let px = x - page.x - MARGIN;
        let line = lines.iter().find(|l| py >= l.y && py < l.y + l.h)?;
        line.items
            .iter()
            .find(|it| px >= it.x && px < it.x + text::width(&it.text, &it.style))
            .and_then(|it| it.link)
    }

    pub fn wheel(&mut self, delta: i32, content: Rect) {
        self.scroll_by(delta * 60, content);
    }

    // --- dibujo -------------------------------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        c.fill_rect(r.x, r.y, r.w, r.h, WINDOW_BG);
        let page = Self::page_rect(r);
        match &self.state {
            State::Loading { url, since, .. } => {
                let dots = ".".repeat(((now_ms - since) / 250 % 4) as usize);
                let msg = format!("Cargando {url}{dots}");
                draw_fit(
                    c,
                    page.x + MARGIN,
                    page.y + 30,
                    &msg,
                    &s16(theme::CYAN),
                    page.w - 2 * MARGIN,
                );
                let secs = (now_ms - since) / 1000;
                if secs >= 3 {
                    let wait = format!("Hace {secs} s. Esc cancela.");
                    text::draw(
                        c,
                        page.x + MARGIN,
                        page.y + 56,
                        &wait,
                        &light(theme::TEXT_DIM),
                    );
                }
            }
            State::Error(msg) => {
                text::draw(
                    c,
                    page.x + MARGIN,
                    page.y + 30,
                    "NO SE PUDO CARGAR",
                    &label(theme::AMBER),
                );
                let st = s16(theme::TEXT);
                let mut y = page.y + 60;
                for chunk in wrap_plain(msg, &st, page.w - 2 * MARGIN) {
                    text::draw(c, page.x + MARGIN, y, &chunk, &st);
                    y += 22;
                }
            }
            State::Page => self.draw_page(c, page),
        }
        // La barra va después: tapa los renglones que se asoman arriba de la página.
        self.draw_bar(c, r, now_ms);
        // Barra de estado.
        let s = Rect::new(r.x, r.y + r.h - STATUS_H, r.w, STATUS_H);
        c.fill_rect(s.x, s.y, s.w, s.h, theme::PANEL);
        let msg = if self.status.is_empty() {
            "Listo"
        } else {
            self.status.as_str()
        };
        draw_fit(c, s.x + 12, s.y + 5, msg, &light(theme::TEXT_DIM), s.w - 24);
    }

    fn draw_bar(&self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        c.fill_rect(r.x, r.y, r.w, BAR_H, WINDOW_BG);
        let [back, fwd, reload, home] = Self::buttons(r);
        for (b, enabled) in [
            (back, !self.back.is_empty()),
            (fwd, !self.forward.is_empty()),
            (reload, self.url.is_some()),
            (home, true),
        ] {
            let col = if enabled {
                theme::TEXT
            } else {
                theme::TEXT_FAINT
            };
            rounded_rect(c, b.x, b.y, b.w, b.h, 4, theme::PANEL, 255);
            rounded_outline(c, b.x, b.y, b.w, b.h, 4, theme::PANEL_RIM);
            let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
            if b == back || b == fwd {
                let d = if b == back { -1 } else { 1 };
                line(c, cx - 6 * d, cy, cx + 6 * d, cy, col);
                line(c, cx + 6 * d, cy, cx + d, cy - 5, col);
                line(c, cx + 6 * d, cy, cx + d, cy + 5, col);
            } else if b == reload {
                let spin = if self.is_loading() {
                    (now_ms / 120 % 8) as i32
                } else {
                    0
                };
                jarvis_gfx::shapes::circle(c, cx, cy, 6, col, false);
                let (dx, dy) = [
                    (0, -6),
                    (4, -4),
                    (6, 0),
                    (4, 4),
                    (0, 6),
                    (-4, 4),
                    (-6, 0),
                    (-4, -4),
                ][spin as usize];
                c.fill_rect(cx + dx - 1, cy + dy - 1, 3, 3, theme::CYAN);
            } else {
                // Casita.
                line(c, cx - 7, cy, cx, cy - 7, col);
                line(c, cx, cy - 7, cx + 7, cy, col);
                jarvis_gfx::shapes::rect_outline(c, cx - 5, cy, 11, 7, col);
            }
        }
        let a = Self::address_rect(r);
        rounded_rect(c, a.x, a.y, a.w, a.h, 4, FIELD_BG, 255);
        let rim = if self.editing {
            theme::CYAN
        } else {
            theme::PANEL_RIM
        };
        rounded_outline(c, a.x, a.y, a.w, a.h, 4, rim);
        let secure = self.url.as_ref().is_some_and(|u| u.scheme == Scheme::Https);
        let mut x = a.x + 10;
        if !self.editing && secure {
            // Candado: la conexión del puente con el sitio es TLS.
            jarvis_gfx::hud::icon(c, jarvis_gfx::hud::Icon::Lock, x + 6, a.y + 16, theme::CYAN);
            x += 20;
        }
        let st = s16(if self.editing {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        });
        let shown = if self.address.text.is_empty() && !self.editing {
            "Buscá o escribí una dirección"
        } else {
            self.address.text.as_str()
        };
        let tw = draw_fit(c, x, a.y + 8, shown, &st, a.x + a.w - x - 14);
        if self.editing {
            c.fill_rect(x + tw + 2, a.y + 7, 2, 18, theme::CYAN);
        }
        line(
            c,
            r.x,
            r.y + BAR_H - 1,
            r.x + r.w - 1,
            r.y + BAR_H - 1,
            theme::PANEL_RIM,
        );
    }

    fn draw_page(&mut self, c: &mut Canvas<'_>, page: Rect) {
        self.ensure_layout(page.w);
        let scroll = self.scroll;
        let focus = self.focus_link;
        let Some((_, lines, total)) = &self.layout else {
            return;
        };
        let x0 = page.x + MARGIN;
        for l in lines {
            let y = page.y + 16 + l.y - scroll;
            if y + l.h < page.y || y > page.y + page.h {
                continue;
            }
            if l.rule {
                line(
                    c,
                    x0,
                    y + l.h / 2,
                    page.x + page.w - MARGIN,
                    y + l.h / 2,
                    theme::PANEL_RIM,
                );
                continue;
            }
            for it in &l.items {
                let mut st = it.style;
                if let Some(k) = it.link {
                    st = st.color(if focus == Some(k) { LINK_FOCUS } else { LINK });
                }
                let w = text::draw(c, x0 + it.x, y, &it.text, &st);
                if it.link.is_some() {
                    let uy = y + st.line_height() - 2;
                    line(c, x0 + it.x, uy, x0 + it.x + w - 1, uy, st.color.scale(140));
                }
            }
        }
        // Barra de desplazamiento.
        if *total > page.h {
            let track = Rect::new(page.x + page.w - 6, page.y + 4, 3, page.h - 8);
            c.fill_rect(track.x, track.y, track.w, track.h, theme::PANEL_RIM);
            let h = (track.h as i64 * page.h as i64 / *total as i64).max(16) as i32;
            let max = (*total - page.h + 40).max(1);
            let y = track.y + ((track.h - h) as i64 * scroll as i64 / max as i64) as i32;
            c.fill_rect(track.x, y, track.w, h, theme::CYAN.scale(180));
        }
    }
}

/// Parte un texto en renglones de `max_w` píxeles.
fn wrap_plain(s: &str, st: &Style, max_w: i32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ') {
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text::width(&candidate, st) > max_w && !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = candidate;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Arma el documento en renglones de `width` píxeles. Devuelve los renglones y el alto total.
fn layout(doc: &Document, width: i32) -> (Vec<Line>, i32) {
    let [body, h1, h2, h3, strong] = styles();
    let dim = body.color(theme::TEXT_DIM);
    let mut lines: Vec<Line> = Vec::new();
    let mut y = 0;
    for block in &doc.blocks {
        let (base, gap_before, gap_after) = match block.kind {
            BlockKind::Heading(1) => (h1, 18, 10),
            BlockKind::Heading(2) => (h2, 16, 8),
            BlockKind::Heading(_) => (h3, 12, 6),
            BlockKind::Item => (body, 2, 2),
            BlockKind::Pre => (body.color(theme::PARTICLE_BRIGHT), 6, 8),
            BlockKind::Quote => (body.color(theme::TEXT_DIM), 6, 8),
            BlockKind::Rule => (body, 8, 8),
            BlockKind::Text => (body, 6, 8),
        };
        if !lines.is_empty() {
            y += gap_before;
        }
        let lh = base.line_height() + 4;
        if block.kind == BlockKind::Rule {
            lines.push(Line {
                y,
                h: 8,
                items: Vec::new(),
                rule: true,
            });
            y += 8 + gap_after;
            continue;
        }
        let indent = block.indent as i32 * 24
            + if block.kind == BlockKind::Quote {
                16
            } else {
                0
            };
        let max_x = width.max(80);
        let space_w = text::width(" ", &base);
        let mut cur = Line {
            y,
            h: lh,
            items: Vec::new(),
            rule: false,
        };
        let mut x = indent;
        let new_line = |lines: &mut Vec<Line>, cur: &mut Line, y: &mut i32, x: &mut i32| {
            let done = core::mem::replace(
                cur,
                Line {
                    y: *y + lh,
                    h: lh,
                    items: Vec::new(),
                    rule: false,
                },
            );
            *y += lh;
            lines.push(done);
            *x = indent;
        };
        for span in &block.spans {
            let mut st = if span.bold && block.kind == BlockKind::Text {
                strong
            } else {
                base
            };
            if span.dim {
                st = dim;
            }
            if block.kind == BlockKind::Pre {
                for (i, part) in span.text.split('\n').enumerate() {
                    if i > 0 {
                        new_line(&mut lines, &mut cur, &mut y, &mut x);
                    }
                    if !part.is_empty() {
                        let w = text::width(part, &st);
                        cur.items.push(Item {
                            x,
                            text: part.to_string(),
                            style: st,
                            link: span.link,
                        });
                        x += w;
                    }
                }
                continue;
            }
            for (i, piece) in span.text.split('\n').enumerate() {
                if i > 0 {
                    new_line(&mut lines, &mut cur, &mut y, &mut x);
                }
                let mut first = true;
                for word in piece.split(' ') {
                    if !first && x > indent {
                        x += space_w;
                    }
                    first = false;
                    if word.is_empty() {
                        continue;
                    }
                    let mut w = text::width(word, &st);
                    if x + w > max_x && x > indent {
                        new_line(&mut lines, &mut cur, &mut y, &mut x);
                    }
                    // Una palabra más larga que el renglón se corta.
                    let word = if w > max_x - indent {
                        let fitted = text::fit(word, &st, max_x - indent);
                        w = text::width(&fitted, &st);
                        fitted
                    } else {
                        word.to_string()
                    };
                    // Palabras seguidas con el mismo estilo van en un solo trozo.
                    match cur.items.last_mut() {
                        Some(last)
                            if last.link == span.link
                                && last.style.color == st.color
                                && core::mem::discriminant(&last.style.weight)
                                    == core::mem::discriminant(&st.weight)
                                && last.x + text::width(&last.text, &last.style) + space_w == x =>
                        {
                            last.text.push(' ');
                            last.text.push_str(&word);
                        }
                        _ => cur.items.push(Item {
                            x,
                            text: word,
                            style: st,
                            link: span.link,
                        }),
                    }
                    x += w;
                }
            }
        }
        y += lh;
        lines.push(cur);
        y += gap_after - 4;
    }
    (lines, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arma_renglones_sin_pasarse_del_ancho() {
        let doc = html::parse(
            "<h1>Título largo de prueba</h1><p>palabra otra <a href=/x>enlace con varias palabras</a> \
             y más texto que no entra en un solo renglón porque es largo.</p><pre>a\nb</pre>",
        );
        let (lines, total) = layout(&doc, 300);
        assert!(total > 0);
        for l in &lines {
            for it in &l.items {
                assert!(
                    it.x + text::width(&it.text, &it.style) <= 300,
                    "{:?}",
                    it.text
                );
            }
        }
        let linked: String = lines
            .iter()
            .flat_map(|l| l.items.iter())
            .filter(|i| i.link == Some(0))
            .map(|i| i.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(linked, "enlace con varias palabras");
        // El <pre> respeta los saltos de línea.
        let pre: Vec<_> = lines.iter().rev().take(2).collect();
        assert_eq!(pre[0].items[0].text, "b");
        assert_eq!(pre[1].items[0].text, "a");
    }
}
