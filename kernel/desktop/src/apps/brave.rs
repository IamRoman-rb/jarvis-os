//! Brave: el navegador de verdad, dibujado en una ventana de JARVIS-OS (ADR 0007).
//!
//! Brave corre en el anfitrión sin ventana y el puente (`xtask/src/brave.rs`) nos manda la
//! página en mosaicos. Esta app:
//! - abre una conexión larga con el puente (por el `Outbox`, así pasa por el firewall);
//! - arma la imagen con los mosaicos que llegan y confirma cada cuadro;
//! - dibuja las pestañas, atrás/adelante/recargar y la barra de dirección (la interfaz de Brave
//!   no viaja: solo la página);
//! - le pasa el mouse, la rueda y el teclado a la página.
//!
//! El protocolo está en [`crate::remote`].

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::shapes::{line, rounded_outline, rounded_rect};
use jarvis_gfx::text;
use jarvis_gfx::{Canvas, PixelFormat, Rect, theme};

use super::{Click, Ctx, Pointer, PointerKind};
use crate::config::{Config, parse_server};
use crate::i18n::{tr, trf};
use crate::input::{Key, Mods};
use crate::remote::{
    self, Button, FromBrave, MOD_ALT, MOD_CTRL, MOD_SHIFT, MouseKind, TabsState, ToBrave,
};
use crate::system::StreamEvent;
use crate::text_input::TextInput;
use crate::widgets::{FIELD_BG, WINDOW_BG, draw_fit, label, light, s16};

pub const TABS_H: i32 = 32;
pub const BAR_H: i32 = 44;
const TAB_MAX_W: i32 = 220;
const NEW_TAB_W: i32 = 30;
/// Cada cuánto como mucho se le avisa al puente que cambió el tamaño (mientras se arrastra el
/// borde de la ventana llegan decenas de cambios por segundo).
const RESIZE_MS: u64 = 150;
/// Reintento automático si el puente no estaba (por ejemplo, QEMU abierto sin `xtask run`).
const RETRY_MS: u64 = 5_000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Link {
    /// Todavía no se pidió la conexión (se pide en el primer `tick`).
    Idle,
    Connecting,
    Open,
    Failed(String),
}

pub struct Brave {
    pub dirty: bool,
    link: Link,
    stream: Option<u32>,
    framer: remote::Framer,
    /// La página, en RGB (3 bytes por píxel), del tamaño que mandó el puente.
    fb: Vec<u8>,
    fb_w: usize,
    fb_h: usize,
    tabs: TabsState,
    address: TextInput,
    editing: bool,
    /// El primer carácter reemplaza la dirección (como al hacer clic en la barra).
    replace_on_type: bool,
    /// Tamaño de la zona de la página: el último que se vio al dibujar y el último que se mandó.
    want: (u16, u16),
    sent: (u16, u16),
    last_resize: u64,
    frames: u64,
    retry_at: Option<u64>,
    last_pos: (i16, i16),
    server: String,
    token: String,
    home: String,
}

impl Brave {
    pub fn new(cfg: &Config) -> Brave {
        Brave {
            dirty: true,
            link: Link::Idle,
            stream: None,
            framer: remote::Framer::default(),
            fb: Vec::new(),
            fb_w: 0,
            fb_h: 0,
            tabs: TabsState::default(),
            address: TextInput::new("", 2000),
            editing: false,
            replace_on_type: false,
            want: (1000, 560),
            sent: (0, 0),
            last_resize: 0,
            frames: 0,
            retry_at: None,
            last_pos: (0, 0),
            server: cfg.brave_server.clone(),
            token: cfg.brave_token.clone(),
            home: cfg.brave_home.clone(),
        }
    }

    pub fn set_config(&mut self, cfg: &Config) {
        let reconnect = cfg.brave_server != self.server || cfg.brave_token != self.token;
        self.server = cfg.brave_server.clone();
        self.token = cfg.brave_token.clone();
        self.home = cfg.brave_home.clone();
        if reconnect && self.link != Link::Open {
            self.link = Link::Idle;
        }
    }

    pub fn title(&self) -> String {
        match self.active_tab().map(|t| t.title.as_str()) {
            Some(t) if !t.is_empty() => format!("Brave · {t}"),
            _ => "Brave".into(),
        }
    }

    fn active_tab(&self) -> Option<&remote::TabInfo> {
        self.tabs.tabs.get(self.tabs.active as usize)
    }

    /// Para los tests y el log.
    pub fn is_connected(&self) -> bool {
        self.link == Link::Open
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn error(&self) -> Option<&str> {
        match &self.link {
            Link::Failed(e) => Some(e),
            _ => None,
        }
    }

    pub fn current_url(&self) -> String {
        self.active_tab().map(|t| t.url.clone()).unwrap_or_default()
    }

    /// Color de un píxel de la página (para los tests).
    pub fn page_pixel(&self, x: usize, y: usize) -> Option<[u8; 3]> {
        (x < self.fb_w && y < self.fb_h).then(|| {
            let i = (y * self.fb_w + x) * 3;
            [self.fb[i], self.fb[i + 1], self.fb[i + 2]]
        })
    }

    fn send<D: BlockDevice>(&mut self, m: ToBrave, ctx: &mut Ctx<'_, D>) {
        if let (Some(id), Link::Open) = (self.stream, &self.link) {
            ctx.out.send(id, m.encode());
        }
    }

    fn connect<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some((host, port)) = parse_server(&self.server) else {
            self.link = Link::Failed(trf("Dirección del puente inválida: {}", &[&self.server]));
            return;
        };
        self.stream = Some(ctx.out.connect(host, port));
        self.link = Link::Connecting;
        self.retry_at = None;
        self.dirty = true;
    }

    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let retry = self.retry_at.is_some_and(|t| ctx.now_ms >= t);
        if self.link == Link::Idle || retry {
            self.connect(ctx);
        }
        if self.link == Link::Open
            && self.want != self.sent
            && ctx.now_ms.saturating_sub(self.last_resize) >= RESIZE_MS
        {
            let (w, h) = self.want;
            self.sent = self.want;
            self.last_resize = ctx.now_ms;
            self.send(ToBrave::Resize { w, h }, ctx);
        }
    }

    pub fn stream_event<D: BlockDevice>(
        &mut self,
        id: u32,
        event: &StreamEvent,
        ctx: &mut Ctx<'_, D>,
    ) {
        if self.stream != Some(id) {
            return;
        }
        match event {
            StreamEvent::Connected => {
                self.link = Link::Open;
                self.framer = remote::Framer::default();
                self.sent = self.want;
                ctx.log.push(format!("BRAVE_CONECTADO {}", self.server));
                let (w, h) = self.want;
                let hello = ToBrave::Hello {
                    token: self.token.clone(),
                    w,
                    h,
                };
                self.send(hello, ctx);
                let home = self.home.clone();
                self.send(ToBrave::Navigate(home), ctx);
            }
            StreamEvent::Data(d) => {
                self.framer.push(d);
                while let Some(m) = self.framer.next_message() {
                    match m.ok().and_then(|b| FromBrave::decode(&b)) {
                        Some(msg) => self.on_message(msg, ctx),
                        None => {
                            self.fail(tr("El puente mandó algo que no se entiende").into(), ctx);
                            ctx.out.close_stream(id);
                            return;
                        }
                    }
                }
            }
            StreamEvent::Closed(why) => {
                self.stream = None;
                let msg = match why {
                    Some(e) if self.link == Link::Connecting => trf(
                        "No se pudo conectar con el puente de Brave en {} ({}). Se abre con cargo xtask run.",
                        &[&self.server, e],
                    ),
                    Some(e) => trf("Se cortó la conexión con Brave ({})", &[e]),
                    None => tr("Brave cerró la conexión").into(),
                };
                // Un error que mandó el puente (más claro) no se pisa.
                if !matches!(self.link, Link::Failed(_)) {
                    self.fail(msg, ctx);
                }
                self.retry_at = Some(ctx.now_ms + RETRY_MS);
            }
        }
        self.dirty = true;
    }

    fn fail<D: BlockDevice>(&mut self, msg: String, ctx: &mut Ctx<'_, D>) {
        ctx.log.push(format!("BRAVE_ERROR {msg}"));
        self.link = Link::Failed(msg);
    }

    fn on_message<D: BlockDevice>(&mut self, m: FromBrave, ctx: &mut Ctx<'_, D>) {
        match m {
            FromBrave::Frame { w, h, tiles } => {
                let (w, h) = (w as usize, h as usize);
                if (w, h) != (self.fb_w, self.fb_h) {
                    self.fb = vec![255; w * h * 3];
                    (self.fb_w, self.fb_h) = (w, h);
                }
                for t in &tiles {
                    let Some(px) = remote::tile_pixels(t) else {
                        continue;
                    };
                    let (tx, ty, tw) = (t.x as usize, t.y as usize, t.w as usize);
                    for row in 0..t.h as usize {
                        if ty + row >= h || tx + tw > w {
                            break;
                        }
                        let dst = ((ty + row) * w + tx) * 3;
                        self.fb[dst..dst + tw * 3].copy_from_slice(&px[row * tw * 3..][..tw * 3]);
                    }
                }
                self.frames += 1;
                if self.frames == 1 {
                    ctx.log.push(format!("BRAVE_FRAME {w}x{h}"));
                }
                self.send(ToBrave::FrameAck, ctx);
            }
            FromBrave::State(s) => {
                self.tabs = s;
                if !self.editing {
                    self.address.text = self.current_url();
                }
            }
            FromBrave::Error(e) => self.fail(e, ctx),
        }
    }

    pub fn on_close<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        if let Some(id) = self.stream.take() {
            ctx.out.close_stream(id);
        }
    }

    // --- geometría ------------------------------------------------------------------------------

    pub fn page_rect(content: Rect) -> Rect {
        Rect::new(
            content.x,
            content.y + TABS_H + BAR_H,
            content.w,
            (content.h - TABS_H - BAR_H).max(0),
        )
    }

    fn tab_w(&self, content: Rect) -> i32 {
        let n = self.tabs.tabs.len().max(1) as i32;
        ((content.w - NEW_TAB_W - 16) / n).clamp(60, TAB_MAX_W)
    }

    fn tab_rect(&self, content: Rect, i: usize) -> Rect {
        let w = self.tab_w(content);
        Rect::new(
            content.x + 6 + i as i32 * w,
            content.y + 4,
            w - 4,
            TABS_H - 4,
        )
    }

    fn new_tab_rect(&self, content: Rect) -> Rect {
        let n = self.tabs.tabs.len() as i32;
        let x = content.x + 6 + n * self.tab_w(content);
        Rect::new(x, content.y + 5, NEW_TAB_W - 6, TABS_H - 8)
    }

    fn buttons(content: Rect) -> [Rect; 3] {
        let y = content.y + TABS_H + 6;
        let at = |i: i32| Rect::new(content.x + 8 + i * 36, y, 32, 32);
        [at(0), at(1), at(2)]
    }

    fn address_rect(content: Rect) -> Rect {
        Rect::new(content.x + 124, content.y + TABS_H + 6, content.w - 136, 32)
    }

    /// Posición en la página (coordenadas de Brave).
    fn page_pos(content: Rect, x: i32, y: i32) -> (i16, i16) {
        let p = Self::page_rect(content);
        (
            (x - p.x).clamp(-1000, 30000) as i16,
            (y - p.y).clamp(-1000, 30000) as i16,
        )
    }

    // --- entrada --------------------------------------------------------------------------------

    fn go<D: BlockDevice>(&mut self, input: &str, ctx: &mut Ctx<'_, D>) {
        let url = to_url(input.trim());
        self.editing = false;
        self.address.text = url.clone();
        if self.link == Link::Open {
            self.send(ToBrave::Navigate(url), ctx);
        } else {
            // Todavía no hay conexión: será la primera página.
            self.home = url;
        }
        self.dirty = true;
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        let (x, y) = (click.x, click.y);
        self.dirty = true;
        if matches!(self.link, Link::Failed(_)) && Self::page_rect(content).contains(x, y) {
            // "Reintentar": un clic en el aviso.
            self.connect(ctx);
            return;
        }
        for i in 0..self.tabs.tabs.len() {
            let r = self.tab_rect(content, i);
            if r.contains(x, y) {
                let close = x >= r.x + r.w - 22;
                let m = if close {
                    ToBrave::TabClose(i as u16)
                } else {
                    ToBrave::TabSelect(i as u16)
                };
                self.send(m, ctx);
                return;
            }
        }
        if self.new_tab_rect(content).contains(x, y) {
            self.send(ToBrave::TabNew, ctx);
            return;
        }
        let [back, fwd, reload] = Self::buttons(content);
        if back.contains(x, y) {
            self.send(ToBrave::Back, ctx);
        } else if fwd.contains(x, y) {
            self.send(ToBrave::Forward, ctx);
        } else if reload.contains(x, y) {
            self.send(ToBrave::Reload, ctx);
        } else if Self::address_rect(content).contains(x, y) {
            self.editing = true;
            self.replace_on_type = true;
        } else if Self::page_rect(content).contains(x, y) {
            if self.editing {
                self.editing = false;
                self.address.text = self.current_url();
            }
            let (px, py) = Self::page_pos(content, x, y);
            self.last_pos = (px, py);
            let button = if click.right {
                Button::Right
            } else {
                Button::Left
            };
            let clicks = if click.double { 2 } else { 1 };
            self.send(mouse(MouseKind::Down, button, px, py, clicks), ctx);
            if click.right {
                // El botón derecho no tiene "soltar" aparte en el escritorio.
                self.send(mouse(MouseKind::Up, button, px, py, 1), ctx);
            }
        }
    }

    /// Movimiento y "soltar" del mouse (el escritorio los manda solo a las apps que los piden).
    pub fn pointer<D: BlockDevice>(&mut self, p: Pointer, content: Rect, ctx: &mut Ctx<'_, D>) {
        let (px, py) = Self::page_pos(content, p.x, p.y);
        match p.kind {
            PointerKind::Move => {
                if (px, py) != self.last_pos {
                    self.last_pos = (px, py);
                    self.send(mouse(MouseKind::Move, Button::None, px, py, 0), ctx);
                }
            }
            PointerKind::Up => self.send(mouse(MouseKind::Up, Button::Left, px, py, 1), ctx),
        }
    }

    pub fn wheel<D: BlockDevice>(&mut self, delta: i32, ctx: &mut Ctx<'_, D>) {
        let (x, y) = self.last_pos;
        let dy = (delta * 100).clamp(-3000, 3000) as i16;
        self.send(ToBrave::Wheel { x, y, dx: 0, dy }, ctx);
    }

    pub fn key<D: BlockDevice>(&mut self, key: Key, mods: Mods, ctx: &mut Ctx<'_, D>) -> bool {
        self.dirty = true;
        if self.editing {
            match key {
                Key::Enter => {
                    let t = self.address.text.clone();
                    self.go(&t, ctx);
                }
                Key::Escape => {
                    self.editing = false;
                    self.address.text = self.current_url();
                }
                k => {
                    if self.replace_on_type && matches!(k, Key::Char(_)) {
                        self.address.text.clear();
                    }
                    self.replace_on_type = false;
                    self.address.handle(k);
                }
            }
            return true;
        }
        // Atajos del navegador (como en Brave).
        match (key, mods.ctrl, mods.alt) {
            (Key::Char('l' | 'L'), true, false) | (Key::F(6), false, false) => {
                self.editing = true;
                self.replace_on_type = true;
                return true;
            }
            (Key::Char('t' | 'T'), true, false) => {
                self.send(ToBrave::TabNew, ctx);
                return true;
            }
            (Key::Char('w' | 'W'), true, false) => {
                let i = self.tabs.active;
                self.send(ToBrave::TabClose(i), ctx);
                return true;
            }
            (Key::F(5), _, _) | (Key::Char('r' | 'R'), true, false) => {
                self.send(ToBrave::Reload, ctx);
                return true;
            }
            (Key::Left, false, true) => {
                self.send(ToBrave::Back, ctx);
                return true;
            }
            (Key::Right, false, true) => {
                self.send(ToBrave::Forward, ctx);
                return true;
            }
            _ => {}
        }
        let Some((name, vk, text)) = key_info(key) else {
            return false;
        };
        let mut m = 0;
        if mods.shift {
            m |= MOD_SHIFT;
        }
        if mods.ctrl {
            m |= MOD_CTRL;
        }
        if mods.alt {
            m |= MOD_ALT;
        }
        self.send(
            ToBrave::Key {
                key: name,
                vk,
                text,
                mods: m,
            },
            ctx,
        );
        true
    }

    // --- dibujo ---------------------------------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        let page = Self::page_rect(r);
        self.want = (
            page.w.clamp(200, 3840) as u16,
            page.h.clamp(150, 2160) as u16,
        );
        c.fill_rect(page.x, page.y, page.w, page.h, WINDOW_BG);
        if self.fb_w > 0 && self.link != Link::Idle {
            let (w, h) = (self.fb_w, self.fb_h);
            if let Some(src) = Canvas::new(&mut self.fb, w, h, w, 3, PixelFormat::Rgb) {
                let saved = c.saved_clip();
                c.intersect_clip(page);
                c.blit(&src, page.x, page.y);
                c.restore_clip(saved);
            }
        }
        match &self.link {
            Link::Failed(e) => {
                let card = Rect::new(page.x + 40, page.y + 40, (page.w - 80).min(720), 150);
                rounded_rect(c, card.x, card.y, card.w, card.h, 8, theme::PANEL, 245);
                rounded_outline(c, card.x, card.y, card.w, card.h, 8, theme::AMBER);
                text::draw(
                    c,
                    card.x + 20,
                    card.y + 18,
                    tr("BRAVE NO ESTÁ DISPONIBLE"),
                    &label(theme::AMBER),
                );
                let st = s16(theme::TEXT);
                let mut y = card.y + 50;
                for chunk in wrap(e, card.w - 40, &st) {
                    text::draw(c, card.x + 20, y, &chunk, &st);
                    y += 22;
                }
                text::draw(
                    c,
                    card.x + 20,
                    card.y + card.h - 30,
                    tr("Clic para reintentar"),
                    &light(theme::CYAN),
                );
            }
            Link::Idle | Link::Connecting => {
                let dots = ".".repeat((now_ms / 300 % 4) as usize);
                let msg = trf("Conectando con Brave{}", &[&dots]);
                text::draw(c, page.x + 40, page.y + 40, &msg, &s16(theme::CYAN));
            }
            Link::Open if self.fb_w == 0 => {
                text::draw(
                    c,
                    page.x + 40,
                    page.y + 40,
                    tr("Abriendo Brave..."),
                    &s16(theme::CYAN),
                );
            }
            Link::Open => {}
        }
        self.draw_tabs(c, r);
        self.draw_bar(c, r, now_ms);
    }

    fn draw_tabs(&self, c: &mut Canvas<'_>, r: Rect) {
        c.fill_rect(r.x, r.y, r.w, TABS_H, theme::VOID);
        for (i, t) in self.tabs.tabs.iter().enumerate() {
            let tr_ = self.tab_rect(r, i);
            let active = i == self.tabs.active as usize;
            let bg = if active { WINDOW_BG } else { theme::PANEL };
            rounded_rect(c, tr_.x, tr_.y, tr_.w, tr_.h + 4, 6, bg, 255);
            if active {
                c.fill_rect(tr_.x + 6, tr_.y, tr_.w - 12, 2, theme::CYAN);
            }
            let title = if t.title.is_empty() {
                tr("Nueva pestaña")
            } else {
                t.title.as_str()
            };
            let col = if active { theme::TEXT } else { theme::TEXT_DIM };
            draw_fit(c, tr_.x + 10, tr_.y + 6, title, &light(col), tr_.w - 36);
            // La cruz para cerrar.
            let (cx, cy) = (tr_.x + tr_.w - 13, tr_.y + tr_.h / 2);
            line(c, cx - 4, cy - 4, cx + 4, cy + 4, theme::TEXT_DIM);
            line(c, cx - 4, cy + 4, cx + 4, cy - 4, theme::TEXT_DIM);
        }
        let n = self.new_tab_rect(r);
        let (cx, cy) = (n.x + n.w / 2, n.y + n.h / 2);
        line(c, cx - 6, cy, cx + 6, cy, theme::TEXT);
        line(c, cx, cy - 6, cx, cy + 6, theme::TEXT);
    }

    fn draw_bar(&self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        let y0 = r.y + TABS_H;
        c.fill_rect(r.x, y0, r.w, BAR_H, WINDOW_BG);
        let [back, fwd, reload] = Self::buttons(r);
        for (b, enabled) in [
            (back, self.tabs.can_back),
            (fwd, self.tabs.can_forward),
            (reload, self.link == Link::Open),
        ] {
            let col = if enabled {
                theme::TEXT
            } else {
                theme::TEXT_FAINT
            };
            let (cx, cy) = (b.x + b.w / 2, b.y + b.h / 2);
            if b == reload {
                if self.tabs.loading {
                    // Cargando: una X para cortar sería lo de Brave; acá, un punto que gira.
                    let spin = (now_ms / 120 % 8) as usize;
                    let (dx, dy) = [
                        (0, -6),
                        (4, -4),
                        (6, 0),
                        (4, 4),
                        (0, 6),
                        (-4, 4),
                        (-6, 0),
                        (-4, -4),
                    ][spin];
                    jarvis_gfx::shapes::circle(c, cx, cy, 6, col, false);
                    c.fill_rect(cx + dx - 1, cy + dy - 1, 3, 3, theme::CYAN);
                } else {
                    jarvis_gfx::shapes::circle(c, cx, cy, 6, col, false);
                    c.fill_rect(cx + 4, cy - 7, 4, 4, WINDOW_BG);
                    line(c, cx + 6, cy - 7, cx + 6, cy - 3, col);
                    line(c, cx + 2, cy - 3, cx + 6, cy - 3, col);
                }
            } else {
                let d = if b == back { -1 } else { 1 };
                line(c, cx - 6 * d, cy, cx + 6 * d, cy, col);
                line(c, cx + 6 * d, cy, cx + d, cy - 5, col);
                line(c, cx + 6 * d, cy, cx + d, cy + 5, col);
            }
        }
        let a = Self::address_rect(r);
        rounded_rect(c, a.x, a.y, a.w, a.h, 16, FIELD_BG, 255);
        let rim = if self.editing {
            theme::CYAN
        } else {
            theme::PANEL_RIM
        };
        rounded_outline(c, a.x, a.y, a.w, a.h, 16, rim);
        let mut x = a.x + 14;
        if !self.editing && self.current_url().starts_with("https://") {
            jarvis_gfx::hud::icon(c, jarvis_gfx::hud::Icon::Lock, x + 6, a.y + 16, theme::CYAN);
            x += 20;
        }
        let shown = if self.address.text.is_empty() && !self.editing {
            tr("Buscá con Brave o escribí una dirección")
        } else {
            self.address.text.as_str()
        };
        let col = if self.editing {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        let tw = draw_fit(c, x, a.y + 8, shown, &s16(col), a.x + a.w - x - 14);
        if self.editing {
            c.fill_rect(x + tw + 2, a.y + 7, 2, 18, theme::CYAN);
        }
        line(
            c,
            r.x,
            y0 + BAR_H - 1,
            r.x + r.w - 1,
            y0 + BAR_H - 1,
            theme::PANEL_RIM,
        );
    }
}

fn mouse(kind: MouseKind, button: Button, x: i16, y: i16, clicks: u8) -> ToBrave {
    ToBrave::Mouse {
        kind,
        button,
        x,
        y,
        clicks,
        mods: 0,
    }
}

/// Lo que se escribe en la barra: una dirección o una búsqueda en Brave Search.
pub fn to_url(input: &str) -> String {
    if input.contains("://") || input.starts_with("about:") {
        return input.into();
    }
    if !input.is_empty() && !input.contains(' ') && input.contains('.') {
        return format!("https://{input}");
    }
    let mut q = String::new();
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                q.push(b as char)
            }
            b' ' => q.push('+'),
            _ => q.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://search.brave.com/search?q={q}")
}

/// (nombre del DOM, código virtual de Windows, texto) de una tecla.
fn key_info(key: Key) -> Option<(String, u16, String)> {
    let special = |name: &str, vk: u16| Some((name.to_string(), vk, String::new()));
    match key {
        Key::Char(c) => {
            let vk = if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase() as u16
            } else if c == ' ' {
                32
            } else {
                0
            };
            Some((c.to_string(), vk, c.to_string()))
        }
        Key::Enter => Some(("Enter".into(), 13, "\r".into())),
        Key::Backspace => special("Backspace", 8),
        Key::Tab => special("Tab", 9),
        Key::Escape => special("Escape", 27),
        Key::Delete => special("Delete", 46),
        Key::Up => special("ArrowUp", 38),
        Key::Down => special("ArrowDown", 40),
        Key::Left => special("ArrowLeft", 37),
        Key::Right => special("ArrowRight", 39),
        Key::Home => special("Home", 36),
        Key::End => special("End", 35),
        Key::PageUp => special("PageUp", 33),
        Key::PageDown => special("PageDown", 34),
        Key::Insert => special("Insert", 45),
        Key::F(n) if (1..=12).contains(&n) => {
            Some((format!("F{n}"), 111 + n as u16, String::new()))
        }
        _ => None,
    }
}

fn wrap(s: &str, max_w: i32, st: &text::Style) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ') {
        let cand = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if text::width(&cand, st) > max_w && !cur.is_empty() {
            out.push(core::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direcciones_y_busquedas() {
        assert_eq!(to_url("brave.com"), "https://brave.com");
        assert_eq!(to_url("http://10.0.2.2:8000/"), "http://10.0.2.2:8000/");
        assert_eq!(
            to_url("clima en córdoba"),
            "https://search.brave.com/search?q=clima+en+c%C3%B3rdoba"
        );
    }

    #[test]
    fn teclas_para_la_pagina() {
        assert_eq!(key_info(Key::Char('a')), Some(("a".into(), 65, "a".into())));
        assert_eq!(key_info(Key::Enter).unwrap().1, 13);
        assert_eq!(
            key_info(Key::F(5)).unwrap(),
            ("F5".into(), 116, String::new())
        );
        assert_eq!(key_info(Key::Left).unwrap().0, "ArrowLeft");
    }
}
