//! Navegador web: barra de direcciones, atrás/adelante, enlaces, buscador, estilos CSS,
//! imágenes, formularios y descargas.
//!
//! Las páginas se piden al kernel (que tiene la red) por el `Outbox` y la respuesta vuelve con
//! [`Browser::net_response`]. El HTML y el CSS se convierten en un documento (`web::html`) y
//! acá se arma en renglones del ancho de la ventana. Después de la página se bajan sus hojas de
//! estilo externas (la página se vuelve a armar con ellas) y sus imágenes (el puente del
//! anfitrión las convierte a BMP, el formato que el kernel sabe leer).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::shapes::{line, rounded_outline, rounded_rect};
use jarvis_gfx::text::{self, Size, Style, Weight};
use jarvis_gfx::{Canvas, Color, Rect, theme};

use super::{Click, Ctx};
use crate::bmp;
use crate::config::Config;
use crate::input::{Key, Mods};
use crate::system::{FetchKind, HttpResponse};
use crate::text_input::TextInput;
use crate::web::html::{self, Align, BlockKind, Document, FieldKind, Options};
use crate::web::url::{self, Scheme, Url};
use crate::widgets::{FIELD_BG, WINDOW_BG, draw_fit, label, light, s16};

const BAR_H: i32 = 48;
const STATUS_H: i32 = 26;
const MARGIN: i32 = 28;
const FIELD_H: i32 = 30;
const MAX_IMAGES: usize = 40;
const MAX_CSS: usize = 6;
const DOWNLOADS: &str = "/Descargas";

pub const HOME: &str = "about:inicio";

const HOME_HTML: &str = "<title>Inicio</title>\
<style>\
body{background:#0b1220;color:#dce3f0}\
.logo{text-align:center;font-size:32px;font-weight:bold;color:#00f0ff}\
.sub{text-align:center;color:#7f93ab}\
.buscar{text-align:center}\
.caja{background:#101c30}\
h2{color:#8fd0ff}\
a{color:#5cc8ff}\
</style>\
<body><p></p><div class=logo>JARVIS · NAVEGADOR</div>\
<p class=sub>Escribí una dirección o lo que quieras buscar.</p>\
<form class=buscar action=\"https://html.duckduckgo.com/html/\"><input name=q size=50 placeholder=\"Buscar en DuckDuckGo\"> <input type=submit value=Buscar></form>\
<form class=buscar action=\"https://search.brave.com/search\"><input name=q size=50 placeholder=\"Buscar con Brave Search\"> <input type=submit value=\"Brave Search\"></form>\
<div class=caja><h2>Para probar</h2><ul>\
<li><a href=\"https://www.google.com/\">Google</a> · con sus colores (para buscar usa tu buscador: Google pide JavaScript)</li>\
<li><a href=\"https://es.wikipedia.org/wiki/Sistema_operativo\">Wikipedia: Sistema operativo</a></li>\
<li><a href=\"http://info.cern.ch/hypertext/WWW/TheProject.html\">La primera página web</a> (CERN, 1991, HTTP directo)</li>\
<li><a href=\"https://lite.cnn.com/\">CNN Lite</a> · noticias en texto</li>\
<li><a href=\"about:ayuda\">Atajos de teclado de JARVIS-OS</a></li>\
</ul></div>\
<h2>Cómo funciona</h2>\
<p>El kernel tiene su propio driver de placa de red (virtio-net) y una pila TCP/IP (smoltcp): \
pide una dirección por DHCP, resuelve nombres por DNS y abre la conexión TCP. Las páginas \
<b>http://</b> van directo. Las <b>https://</b> necesitan cifrado TLS, que todavía no está en el \
kernel: se piden a un puente en el anfitrión que levanta <b>cargo xtask run</b>. El puente también \
convierte las imágenes a BMP.</p>\
<p>Se entiende CSS (colores, fondos, tamaños, alineación, <i>display</i>, variables) y los \
formularios con GET. No hay JavaScript. F9 cambia al modo lectura; la Configuración (Win+I) \
tiene el buscador, la página de inicio y el tema de las páginas.</p>";

const HELP_HTML: &str = "<title>Atajos de teclado</title>\
<style>body{background:#0b1220;color:#dce3f0} h1{color:#00f0ff} h2{color:#8fd0ff} b{color:#ffd166}</style>\
<h1>Atajos de teclado de JARVIS-OS</h1>\
<h2>Ventanas y escritorio (como en Windows)</h2><ul>\
<li><b>Win</b>: menú de inicio · <b>Win+S</b> / <b>Ctrl+Esc</b>: buscar</li>\
<li><b>Alt+Tab</b> / <b>Alt+Shift+Tab</b>: cambiar de ventana · <b>Alt+Esc</b>: la siguiente sin selector</li>\
<li><b>Win+Tab</b>: vista de tareas · <b>Win+D</b> / <b>Win+,</b>: mostrar el escritorio</li>\
<li><b>Win+M</b>: minimizar todo · <b>Win+Shift+M</b>: restaurar · <b>Win+Inicio</b>: minimizar las demás</li>\
<li><b>Win+Arriba</b> / <b>Win+Abajo</b>: maximizar / restaurar · <b>Win+Izq.</b> / <b>Win+Der.</b>: acoplar · <b>Win+Shift+Arriba</b>: estirar a lo alto</li>\
<li><b>Win+Ctrl+D</b>: escritorio virtual nuevo · <b>Win+Ctrl+Izq./Der.</b>: cambiar · <b>Win+Ctrl+F4</b>: cerrarlo</li>\
<li><b>Alt+Espacio</b>: menú de la ventana · <b>Alt+F4</b>: cerrar · <b>Ctrl+W</b>: cerrar (si la app no lo usa) · <b>F11</b>: maximizar</li>\
<li><b>Win+X</b>: enlaces rápidos · <b>Win+A</b>: configuración rápida · <b>Win+N</b> / <b>Win+Alt+D</b>: notificaciones y calendario</li>\
<li><b>Win+I</b>: Configuración · <b>Win+E</b>: Archivos · <b>Win+R</b>: consola de JARVIS · <b>Ctrl+Shift+Esc</b>: monitor</li>\
<li><b>Ctrl+Alt+T</b> / <b>Win+Enter</b>: terminal (como Ubuntu)</li>\
<li><b>Win+L</b>: bloquear · <b>Impr Pant</b> / <b>Win+Shift+S</b>: captura · <b>Alt+Impr Pant</b>: captura de la ventana</li>\
<li><b>Win+1</b> ... <b>Win+0</b>: los íconos de la barra · <b>Win+Ctrl+Shift+B</b>: redibujar la pantalla · <b>F1</b>: esta ayuda</li></ul>\
<h2>Navegador</h2><ul>\
<li><b>Ctrl+L</b> / <b>Alt+D</b> / <b>F6</b>: escribir una dirección · <b>Alt+Izq./Der.</b>: atrás/adelante · <b>F5</b>: recargar</li>\
<li><b>Tab</b>: siguiente enlace o campo · <b>Enter</b>: abrirlo · <b>F9</b>: modo lectura · <b>Ctrl+H</b>: inicio</li></ul>\
<h2>Terminal</h2><ul>\
<li><b>Tab</b>: completar · <b>Arriba/Abajo</b>: historial · <b>Ctrl+C</b>: cancelar · <b>Ctrl+L</b>: limpiar · <b>Ctrl+A/E/U/K/W</b>: editar la línea</li>\
<li><b>help</b> lista los comandos · <b>apt install neofetch</b> instala un programa</li></ul>";

#[derive(Clone)]
enum ItemKind {
    Text {
        text: String,
        style: Style,
        underline: bool,
        bg: Option<Color>,
    },
    Image(usize),
    Field(usize),
}

#[derive(Clone)]
struct Item {
    x: i32,
    w: i32,
    h: i32,
    kind: ItemKind,
    link: Option<usize>,
}

struct Line {
    y: i32,
    h: i32,
    items: Vec<Item>,
    rule: bool,
    bg: Option<Color>,
}

enum State {
    Loading { id: u32, url: String, since: u64 },
    Page,
    Error(String),
}

enum Img {
    Loading(u32),
    Ready(bmp::Image),
    Failed,
}

/// Lo de la configuración que usa el navegador.
#[derive(Clone)]
struct Prefs {
    light: bool,
    images: bool,
    reader: bool,
    search: &'static str,
    search_name: &'static str,
}

impl Prefs {
    fn of(c: &Config) -> Prefs {
        Prefs {
            light: c.light_pages,
            images: c.load_images,
            reader: c.reader_mode,
            search: c.search.url(),
            search_name: c.search.name(),
        }
    }
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
    /// Campo de formulario con el foco (lo que se escribe va ahí).
    focus_field: Option<usize>,
    status: String,
    prefs: Prefs,
    /// Modo lectura solo para esta ventana (F9).
    reader_toggle: bool,
    /// El HTML de la página (para volver a armarla cuando llegan las hojas de estilo).
    html: Option<String>,
    /// (pedido, contenido) de cada hoja de estilo externa.
    css: Vec<(u32, Option<String>)>,
    images: Vec<Img>,
}

fn size_of(px: u8) -> Size {
    match px {
        0..=17 => Size::Size16,
        18..=21 => Size::Size20,
        22..=27 => Size::Size24,
        _ => Size::Size32,
    }
}

impl Browser {
    pub fn new(cfg: &Config) -> Self {
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
            focus_field: None,
            status: String::new(),
            prefs: Prefs::of(cfg),
            reader_toggle: false,
            html: None,
            css: Vec::new(),
            images: Vec::new(),
        }
    }

    /// La configuración cambió (tema de las páginas, imágenes, modo lectura, buscador).
    pub fn set_config(&mut self, cfg: &Config) {
        let p = Prefs::of(cfg);
        let restyle = p.light != self.prefs.light
            || p.reader != self.prefs.reader
            || p.images != self.prefs.images;
        self.prefs = p;
        if restyle {
            self.restyle();
        }
    }

    fn reader(&self) -> bool {
        self.prefs.reader != self.reader_toggle
    }

    fn options(&self) -> Options {
        if self.reader() {
            Options::READER
        } else {
            Options {
                reader: false,
                colors: self.prefs.light,
                images: self.prefs.images,
            }
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

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Lo escrito en la barra (o una dirección ya armada) → navegar.
    pub fn go<D: BlockDevice>(&mut self, input: &str, ctx: &mut Ctx<'_, D>) {
        let search = self.prefs.search;
        self.navigate(&url::from_input_with(input, search), true, ctx);
    }

    fn navigate<D: BlockDevice>(&mut self, target: &str, record: bool, ctx: &mut Ctx<'_, D>) {
        let mut target = target.to_string();
        // Los resultados de DuckDuckGo pasan por un redireccionador: se va directo al destino.
        if let Some(u) = Url::parse(&target)
            && u.host.ends_with("duckduckgo.com")
            && u.path.starts_with("/l/")
            && let Some(real) = u.query_param("uddg")
        {
            target = real;
        }
        // Google ya no muestra resultados sin JavaScript: se busca con el buscador elegido.
        let mut note = None;
        if let Some(u) = Url::parse(&target)
            && u.host.contains("google.")
            && u.path.starts_with("/search")
            && let Some(q) = u.query_param("q")
        {
            target = format!("{}{}", self.prefs.search, url::percent_encode(&q));
            note = Some(format!(
                "Google pide JavaScript para buscar: se buscó con {}.",
                self.prefs.search_name
            ));
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
        self.focus_field = None;
        self.layout = None;
        self.html = None;
        self.css.clear();
        self.images.clear();
        self.dirty = true;
        ctx.log.push(format!("NAVEGADOR_IR {u}"));
        match u.scheme {
            Scheme::About => {
                let page = match u.path.as_str() {
                    "inicio" | "" => HOME_HTML,
                    "ayuda" => HELP_HTML,
                    _ => "<title>No existe</title><p>Esa página interna no existe.</p>",
                };
                self.show_html(page.to_string(), ctx);
            }
            Scheme::File => {
                let path = url::percent_decode(u.path_only());
                let Some(fs) = ctx.fs.as_deref_mut() else {
                    self.show_error("No hay disco.".into());
                    return;
                };
                let is_dir = path == "/" || fs.stat(&path).is_ok_and(|s| s.is_dir);
                if is_dir {
                    let page = dir_listing(fs, &path);
                    self.show_html(page, ctx);
                    return;
                }
                match fs.read_file(&path) {
                    Ok(bytes) => {
                        let lower = path.to_ascii_lowercase();
                        if lower.ends_with(".html") || lower.ends_with(".htm") {
                            self.show_html(html::decode_bytes(&bytes), ctx);
                        } else if lower.ends_with(".bmp") {
                            self.show_image(bytes);
                        } else {
                            self.show(html::plain(&html::decode_bytes(&bytes)));
                        }
                    }
                    Err(e) => self.show_error(format!("{path}: {e}")),
                }
            }
            Scheme::Http | Scheme::Https => {
                let lower = u.path_only().to_ascii_lowercase();
                let kind = if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico"]
                    .iter()
                    .any(|e| lower.ends_with(e))
                {
                    FetchKind::Image
                } else if [
                    ".exe",
                    ".msi",
                    ".zip",
                    ".7z",
                    ".rar",
                    ".deb",
                    ".rpm",
                    ".iso",
                    ".tar",
                    ".gz",
                    ".xz",
                    ".bz2",
                    ".pdf",
                    ".bin",
                    ".dmg",
                    ".appimage",
                    ".apk",
                    ".img",
                    ".mp3",
                    ".mp4",
                ]
                .iter()
                .any(|e| lower.ends_with(e))
                {
                    FetchKind::Download
                } else {
                    FetchKind::Page
                };
                let id = ctx.out.fetch_kind(&u.to_string(), kind);
                self.state = State::Loading {
                    id,
                    url: u.to_string(),
                    since: ctx.now_ms,
                };
                self.status = note.unwrap_or_else(|| format!("Conectando con {}...", u.host));
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

    fn show_image(&mut self, bytes: Vec<u8>) {
        match bmp::decode(&bytes) {
            Some(img) => {
                let mut doc = html::render(
                    "<body style='text-align:center'><img src=imagen>",
                    Options::STYLED,
                    &[],
                );
                doc.bg = Some(Color::hex(0x202124));
                self.images = alloc::vec![Img::Ready(img)];
                self.show(doc);
            }
            None => self.show_error("No se pudo leer la imagen.".into()),
        }
    }

    /// Arma la página y pide sus hojas de estilo y sus imágenes.
    fn show_html<D: BlockDevice>(&mut self, text: String, ctx: &mut Ctx<'_, D>) {
        let doc = html::render(&text, self.options(), &[]);
        self.html = Some(text);
        self.show(doc);
        if let Some(base) = self.base() {
            for href in self.doc.stylesheets.iter().take(MAX_CSS) {
                if let Some(u) = base.join(href)
                    && matches!(u.scheme, Scheme::Http | Scheme::Https)
                {
                    let id = ctx.out.fetch(&u.to_string());
                    self.css.push((id, None));
                }
            }
        }
        self.request_images(ctx);
    }

    /// La dirección contra la que se resuelven los enlaces (`<base href>` o la de la página).
    fn base(&self) -> Option<Url> {
        let u = self.url.clone()?;
        match &self.doc.base {
            Some(b) => u.join(b).or(Some(u)),
            None => Some(u),
        }
    }

    fn request_images<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        self.images.clear();
        if !self.options().images {
            return;
        }
        let Some(base) = self.base() else { return };
        for (i, img) in self.doc.images.iter().enumerate() {
            let target = base.join(&img.src);
            let ok = i < MAX_IMAGES
                && target.as_ref().is_some_and(|u| {
                    matches!(u.scheme, Scheme::Http | Scheme::Https)
                        && !u.path_only().to_ascii_lowercase().ends_with(".svg")
                });
            self.images.push(match (ok, target) {
                (true, Some(u)) => {
                    Img::Loading(ctx.out.fetch_kind(&u.to_string(), FetchKind::Image))
                }
                _ => Img::Failed,
            });
        }
    }

    /// Vuelve a armar la página (llegaron hojas de estilo o cambió la configuración).
    fn restyle(&mut self) {
        let Some(text) = &self.html else {
            return;
        };
        let css: Vec<String> = self.css.iter().filter_map(|(_, c)| c.clone()).collect();
        let mut doc = html::render(text, self.options(), &css);
        // Lo que se escribió en los formularios no se pierde.
        if doc.fields.len() == self.doc.fields.len() {
            for (new, old) in doc.fields.iter_mut().zip(&self.doc.fields) {
                new.value = old.value.clone();
                new.checked = old.checked;
            }
        }
        let same_images = doc.images == self.doc.images;
        self.doc = doc;
        if !same_images {
            // Con otras reglas cambian qué imágenes se ven: se descartan las viejas.
            self.images.clear();
        }
        self.layout = None;
        self.dirty = true;
    }

    pub fn net_response<D: BlockDevice>(
        &mut self,
        id: u32,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
    ) {
        // ¿Una hoja de estilo?
        if let Some(slot) = self.css.iter_mut().find(|(i, _)| *i == id) {
            slot.1 = Some(match result {
                Ok(r) if r.status < 400 => html::decode_bytes(&r.body),
                _ => String::new(),
            });
            if self.css.iter().all(|(_, c)| c.is_some()) {
                self.restyle();
                if self.images.is_empty() {
                    self.request_images(ctx);
                }
            }
            return;
        }
        // ¿Una imagen?
        if let Some(slot) = self
            .images
            .iter_mut()
            .find(|s| matches!(s, Img::Loading(i) if *i == id))
        {
            *slot = match result {
                Ok(r) if r.status < 400 => bmp::decode(&r.body).map_or(Img::Failed, Img::Ready),
                _ => Img::Failed,
            };
            self.layout = None;
            self.dirty = true;
            return;
        }
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
                let status = format!("{} · {} bytes", resp.status, resp.body.len());
                if ct.starts_with("image/bmp")
                    || resp.body.starts_with(b"BM") && ct.starts_with("image/")
                {
                    self.show_image(resp.body.clone());
                } else if ct.contains("html")
                    || (ct.is_empty()
                        && resp
                            .body
                            .windows(5)
                            .any(|w| w.eq_ignore_ascii_case(b"<html")))
                {
                    let text = html::decode_bytes(&resp.body);
                    self.show_html(text, ctx);
                    if resp.status >= 400 {
                        self.doc.title = format!("Error {}", resp.status);
                    }
                } else if ct.starts_with("text/")
                    || ct.contains("json")
                    || ct.contains("xml")
                    || ct.contains("javascript")
                {
                    self.show(html::plain(&html::decode_bytes(&resp.body)));
                } else if resp.status < 400 {
                    self.save_download(resp, ctx);
                    return;
                } else {
                    self.show_error(format!("Error {} ({})", resp.status, resp.content_type));
                }
                self.status = status;
            }
        }
    }

    /// Lo que no es una página (un .exe, un .zip…) se guarda en /Descargas.
    fn save_download<D: BlockDevice>(&mut self, resp: &HttpResponse, ctx: &mut Ctx<'_, D>) {
        let now = ctx.timestamp();
        let name = self
            .url
            .as_ref()
            .map(|u| url::percent_decode(crate::files::basename(u.path_only())))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "descarga.bin".into());
        let Some(fs) = ctx.fs.as_deref_mut() else {
            self.show_error("No hay disco para guardar la descarga.".into());
            return;
        };
        let mut path = format!("{DOWNLOADS}/{name}");
        let mut n = 2;
        while fs.exists(&path) {
            path = format!("{DOWNLOADS}/{}", crate::files::with_suffix(&name, n));
            n += 1;
        }
        let saved = crate::term::apt::ensure_dirs(fs, DOWNLOADS, now)
            .and_then(|()| fs.write_file(&path, &resp.body, now));
        match saved {
            Ok(()) => {
                ctx.log
                    .push(format!("DESCARGA {path} ({} bytes)", resp.body.len()));
                ctx.out.notify(format!("Descargado: {path}"), false);
                let what =
                    crate::term::binfmt::describe(&resp.body[..resp.body.len().min(256 * 1024)]);
                let run = if crate::term::binfmt::why_not(&resp.body).is_some() {
                    "<p>Es un programa de otro sistema: JARVIS-OS todavía no puede ejecutarlo \
                     (le falta espacio de usuario y la API que espera). Lo podés inspeccionar en \
                     la terminal con <b>file</b>, <b>strings</b> o <b>xxd</b>.</p>"
                } else {
                    ""
                };
                let page = format!(
                    "<title>Descarga completa</title><h1>Descarga completa</h1>\
                     <p><b>{path}</b> · {} · {what}</p>{run}\
                     <p><a href=\"file://{DOWNLOADS}\">Ver la carpeta Descargas</a></p>",
                    crate::files::format_size(resp.body.len() as u64)
                );
                self.show_html(page, ctx);
            }
            Err(e) => self.show_error(format!("No se pudo guardar {path}: {e}")),
        }
    }

    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        // Mientras carga, el indicador se anima (4 veces por segundo).
        if let State::Loading { since, .. } = self.state
            && (ctx.now_ms - since) % 250 < 20
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
        if href.starts_with('#') {
            return; // ancla en la misma página
        }
        if href.to_ascii_lowercase().starts_with("javascript:") {
            self.status = "Ese enlace necesita JavaScript.".into();
            self.dirty = true;
            return;
        }
        let target = match self.base() {
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

    // --- formularios --------------------------------------------------------------------------

    /// Se apretó un botón (o Enter en un campo): se arma la dirección con los campos del
    /// formulario (GET) y se va.
    fn submit<D: BlockDevice>(&mut self, from: usize, ctx: &mut Ctx<'_, D>) {
        let Some(field) = self.doc.fields.get(from) else {
            return;
        };
        let Some(form_idx) = field.form else {
            if field.kind == FieldKind::Button {
                self.status = "Ese botón necesita JavaScript.".into();
                self.dirty = true;
            }
            return;
        };
        let Some(form) = self.doc.forms.get(form_idx).cloned() else {
            return;
        };
        if form.post {
            self.status =
                "Este formulario manda datos con POST: el puente solo acepta GET (ver ADR 0004)."
                    .into();
            self.dirty = true;
            return;
        }
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (i, f) in self.doc.fields.iter().enumerate() {
            if f.form != Some(form_idx) || f.name.is_empty() {
                continue;
            }
            let value = match f.kind {
                FieldKind::Text
                | FieldKind::Password
                | FieldKind::Hidden
                | FieldKind::TextArea
                | FieldKind::Select => Some(f.value.clone()),
                FieldKind::Checkbox | FieldKind::Radio if f.checked => {
                    Some(if f.value.is_empty() {
                        "on".into()
                    } else {
                        f.value.clone()
                    })
                }
                // Solo el botón que se apretó.
                FieldKind::Submit if i == from => Some(
                    f.options
                        .first()
                        .map(|(v, _)| v.clone())
                        .unwrap_or_else(|| f.value.clone()),
                ),
                _ => None,
            };
            if let Some(v) = value {
                pairs.push((f.name.clone(), v));
            }
        }
        let query: Vec<String> = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", url::percent_encode(k), url::percent_encode(v)))
            .collect();
        let action = if form.action.is_empty() {
            self.current_url()
        } else {
            form.action.clone()
        };
        let Some(base) = self.base() else { return };
        let Some(mut target) = base.join(&action) else {
            self.status = format!("Formulario con destino inválido: {action}");
            return;
        };
        let path = target.path_only().to_string();
        target.path = format!("{path}?{}", query.join("&"));
        ctx.log.push(format!("NAVEGADOR_FORMULARIO {target}"));
        self.navigate(&target.to_string(), true, ctx);
    }

    /// Clic (o Enter) sobre un campo.
    fn activate_field<D: BlockDevice>(&mut self, i: usize, ctx: &mut Ctx<'_, D>) {
        let Some(f) = self.doc.fields.get(i) else {
            return;
        };
        match f.kind {
            FieldKind::Text | FieldKind::Password | FieldKind::TextArea => {
                self.focus_field = Some(i);
                self.focus_link = None;
            }
            FieldKind::Checkbox => {
                self.doc.fields[i].checked = !self.doc.fields[i].checked;
            }
            FieldKind::Radio => {
                let (form, name) = (f.form, f.name.clone());
                for g in &mut self.doc.fields {
                    if g.kind == FieldKind::Radio && g.form == form && g.name == name {
                        g.checked = false;
                    }
                }
                self.doc.fields[i].checked = true;
            }
            FieldKind::Select => {
                let f = &mut self.doc.fields[i];
                if !f.options.is_empty() {
                    let pos = f
                        .options
                        .iter()
                        .position(|(v, _)| *v == f.value)
                        .unwrap_or(0);
                    f.value = f.options[(pos + 1) % f.options.len()].0.clone();
                }
            }
            FieldKind::Submit | FieldKind::Button => self.submit(i, ctx),
            FieldKind::Hidden => {}
        }
        self.dirty = true;
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

    fn colors(&self) -> (Color, Color, Color) {
        // (fondo, texto, enlaces)
        if self.options().colors {
            (
                self.doc.bg.unwrap_or(Color::WHITE),
                self.doc.fg.unwrap_or(Color::hex(0x202124)),
                Color::hex(0x1a0dab),
            )
        } else {
            (WINDOW_BG, theme::TEXT, Color::hex(0x5cc8ff))
        }
    }

    fn ensure_layout(&mut self, width: i32) {
        if self.layout.as_ref().is_some_and(|l| l.0 == width) {
            return;
        }
        let (_, fg, _) = self.colors();
        let (lines, total) = layout(
            &self.doc,
            &self.images,
            width - 2 * MARGIN,
            fg,
            !self.options().colors,
        );
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

    /// Tab: el próximo enlace o campo (lo pone a la vista).
    fn focus_next(&mut self, forward: bool, content: Rect) {
        let page = Self::page_rect(content);
        self.ensure_layout(page.w);
        let Some((_, lines, _)) = &self.layout else {
            return;
        };
        // (es campo, índice, y)
        let mut order: Vec<(bool, usize, i32)> = Vec::new();
        for l in lines {
            for it in &l.items {
                let key = match (&it.kind, it.link) {
                    (ItemKind::Field(f), _) => Some((true, *f)),
                    (_, Some(k)) => Some((false, k)),
                    _ => None,
                };
                if let Some((is_field, k)) = key
                    && !order.iter().any(|(f, o, _)| *f == is_field && *o == k)
                {
                    order.push((is_field, k, l.y));
                }
            }
        }
        if order.is_empty() {
            return;
        }
        let cur = match (self.focus_field, self.focus_link) {
            (Some(f), _) => order.iter().position(|(isf, k, _)| *isf && *k == f),
            (None, Some(l)) => order.iter().position(|(isf, k, _)| !*isf && *k == l),
            _ => None,
        };
        let next = match (cur, forward) {
            (None, true) => 0,
            (None, false) => order.len() - 1,
            (Some(p), true) => (p + 1) % order.len(),
            (Some(p), false) => (p + order.len() - 1) % order.len(),
        };
        let (is_field, k, y) = order[next];
        if is_field {
            self.focus_field = Some(k);
            self.focus_link = None;
            self.status = String::new();
        } else {
            self.focus_link = Some(k);
            self.focus_field = None;
            self.status = self.doc.links.get(k).cloned().unwrap_or_default();
        }
        if y < self.scroll || y > self.scroll + page.h - 60 {
            self.scroll = (y - page.h / 3).clamp(0, self.max_scroll(page));
        }
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
            self.focus_field = None;
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
        // Escribiendo en un campo de la página.
        if let Some(fi) = self.focus_field
            && fi < self.doc.fields.len()
        {
            match key {
                Key::Enter => {
                    self.focus_field = None;
                    self.submit(fi, ctx);
                    return true;
                }
                Key::Escape => {
                    self.focus_field = None;
                    return true;
                }
                Key::Tab => {
                    self.focus_next(!mods.shift, content);
                    return true;
                }
                Key::Backspace => {
                    self.doc.fields[fi].value.pop();
                    return true;
                }
                Key::Char(c) if !mods.ctrl && !c.is_control() => {
                    if self.doc.fields[fi].value.chars().count() < 500 {
                        self.doc.fields[fi].value.push(c);
                    }
                    return true;
                }
                _ => {}
            }
        }
        let page_h = Self::page_rect(content).h;
        match key {
            Key::Left if mods.alt => self.history_back(ctx),
            Key::Right if mods.alt => self.history_forward(ctx),
            Key::Backspace => self.history_back(ctx),
            Key::F(5) => self.reload(ctx),
            Key::F(9) => {
                self.reader_toggle = !self.reader_toggle;
                self.status = if self.reader() {
                    "Modo lectura".into()
                } else {
                    "Página completa".into()
                };
                self.restyle();
            }
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
            self.focus_field = None;
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
            self.focus_field = None;
            match self.item_at(page, x, y) {
                Some((Some(f), _)) => self.activate_field(f, ctx),
                Some((None, Some(link))) => {
                    self.focus_link = Some(link);
                    self.follow(link, ctx);
                }
                _ => {}
            }
            self.dirty = true;
        }
    }

    /// (campo, enlace) bajo el punto.
    fn item_at(&mut self, page: Rect, x: i32, y: i32) -> Option<(Option<usize>, Option<usize>)> {
        self.ensure_layout(page.w);
        let (_, lines, _) = self.layout.as_ref()?;
        let py = y - page.y - 16 + self.scroll;
        let px = x - page.x - MARGIN;
        let line = lines.iter().find(|l| py >= l.y && py < l.y + l.h)?;
        let it = line
            .items
            .iter()
            .find(|it| px >= it.x && px < it.x + it.w)?;
        let field = match it.kind {
            ItemKind::Field(f) => Some(f),
            _ => None,
        };
        Some((field, it.link))
    }

    pub fn wheel(&mut self, delta: i32, content: Rect) {
        self.scroll_by(delta * 60, content);
    }

    // --- dibujo -------------------------------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        let page = Self::page_rect(r);
        let (bg, _, _) = self.colors();
        match &self.state {
            State::Loading { url, since, .. } => {
                c.fill_rect(r.x, r.y, r.w, r.h, WINDOW_BG);
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
                c.fill_rect(r.x, r.y, r.w, r.h, WINDOW_BG);
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
            State::Page => {
                c.fill_rect(r.x, r.y, r.w, r.h, bg);
                self.draw_page(c, page, now_ms);
            }
        }
        // La barra va después: tapa los renglones que se asoman arriba de la página.
        self.draw_bar(c, r, now_ms);
        // Barra de estado.
        let s = Rect::new(r.x, r.y + r.h - STATUS_H, r.w, STATUS_H);
        c.fill_rect(s.x, s.y, s.w, s.h, theme::PANEL);
        let loading_images = self
            .images
            .iter()
            .filter(|i| matches!(i, Img::Loading(_)))
            .count()
            + self.css.iter().filter(|(_, c)| c.is_none()).count();
        let msg = if !self.status.is_empty() {
            self.status.clone()
        } else if loading_images > 0 {
            format!("Bajando {loading_images} imágenes y estilos...")
        } else if self.reader() {
            "Listo · modo lectura (F9)".into()
        } else {
            "Listo".into()
        };
        draw_fit(
            c,
            s.x + 12,
            s.y + 5,
            &msg,
            &light(theme::TEXT_DIM),
            s.w - 24,
        );
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

    fn draw_page(&mut self, c: &mut Canvas<'_>, page: Rect, now_ms: u64) {
        self.ensure_layout(page.w);
        let scroll = self.scroll;
        let (_, fg, link_color) = self.colors();
        let styled = self.options().colors;
        let focus = self.focus_link;
        let focus_field = self.focus_field;
        let blink = (now_ms / 530).is_multiple_of(2);
        let Some((_, lines, total)) = &self.layout else {
            return;
        };
        let x0 = page.x + MARGIN;
        let rule = if styled {
            Color::hex(0xdadce0)
        } else {
            theme::PANEL_RIM
        };
        for l in lines {
            let y = page.y + 16 + l.y - scroll;
            if y + l.h < page.y || y > page.y + page.h {
                continue;
            }
            if let Some(bg) = l.bg {
                c.fill_rect(page.x + MARGIN / 2, y, page.w - MARGIN, l.h, bg);
            }
            if l.rule {
                line(
                    c,
                    x0,
                    y + l.h / 2,
                    page.x + page.w - MARGIN,
                    y + l.h / 2,
                    rule,
                );
                continue;
            }
            for it in &l.items {
                let ix = x0 + it.x;
                let iy = y + l.h - it.h - 2;
                match &it.kind {
                    ItemKind::Text {
                        text: t,
                        style,
                        underline,
                        bg,
                    } => {
                        let mut st = *style;
                        if let Some(k) = it.link {
                            let focused = focus == Some(k);
                            if focused {
                                st = st.color(if styled {
                                    Color::hex(0xe37400)
                                } else {
                                    Color::hex(0xffd166)
                                });
                            } else if st.color == fg {
                                st = st.color(link_color);
                            }
                        }
                        if let Some(bg) = bg {
                            c.fill_rect(ix - 2, iy - 1, it.w + 4, it.h + 2, *bg);
                        }
                        text::draw(c, ix, iy, t, &st);
                        if *underline || (it.link.is_some() && focus == it.link) {
                            let uy = iy + st.line_height() - 2;
                            line(c, ix, uy, ix + it.w - 1, uy, st.color.scale(160));
                        }
                    }
                    ItemKind::Image(i) => {
                        let area = Rect::new(ix, iy, it.w, it.h);
                        match self.images.get(*i) {
                            Some(Img::Ready(img)) => img.draw_scaled(c, area),
                            _ => {
                                let ph = if styled {
                                    Color::hex(0xf1f3f4)
                                } else {
                                    theme::PANEL
                                };
                                c.fill_rect(area.x, area.y, area.w, area.h, ph);
                                jarvis_gfx::shapes::rect_outline(
                                    c, area.x, area.y, area.w, area.h, rule,
                                );
                                if let Some(alt) = self.doc.images.get(*i).map(|im| im.alt.as_str())
                                    && area.w > 40
                                    && area.h > 20
                                {
                                    let st = Style::new(
                                        Weight::Regular,
                                        Size::Size16,
                                        if styled {
                                            Color::hex(0x5f6368)
                                        } else {
                                            theme::TEXT_DIM
                                        },
                                    );
                                    draw_fit(c, area.x + 6, area.y + 4, alt, &st, area.w - 12);
                                }
                            }
                        }
                        if it.link.is_some() && focus == it.link {
                            rounded_outline(
                                c,
                                ix - 2,
                                iy - 2,
                                it.w + 4,
                                it.h + 4,
                                2,
                                Color::hex(0xe37400),
                            );
                        }
                    }
                    ItemKind::Field(f) => {
                        if let Some(field) = self.doc.fields.get(*f) {
                            draw_field(
                                c,
                                Rect::new(ix, iy, it.w, it.h),
                                field,
                                styled,
                                focus_field == Some(*f),
                                blink,
                            );
                        }
                    }
                }
            }
        }
        // Barra de desplazamiento.
        if *total > page.h {
            let track = Rect::new(page.x + page.w - 6, page.y + 4, 3, page.h - 8);
            c.fill_rect(
                track.x,
                track.y,
                track.w,
                track.h,
                if styled {
                    Color::hex(0xdadce0)
                } else {
                    theme::PANEL_RIM
                },
            );
            let h = (track.h as i64 * page.h as i64 / *total as i64).max(16) as i32;
            let max = (*total - page.h + 40).max(1);
            let y = track.y + ((track.h - h) as i64 * scroll as i64 / max as i64) as i32;
            c.fill_rect(
                track.x,
                y,
                track.w,
                h,
                if styled {
                    Color::hex(0x9aa0a6)
                } else {
                    theme::CYAN.scale(180)
                },
            );
        }
    }
}

fn field_label(f: &html::Field) -> String {
    match f.kind {
        FieldKind::Select => {
            let shown = f
                .options
                .iter()
                .find(|(v, _)| *v == f.value)
                .map_or(f.value.as_str(), |(_, l)| l.as_str());
            format!("{shown}  v")
        }
        _ => f.value.clone(),
    }
}

fn field_size(f: &html::Field) -> (i32, i32) {
    let st = Style::new(Weight::Regular, Size::Size16, Color::BLACK);
    match f.kind {
        FieldKind::Checkbox | FieldKind::Radio => (18, 18),
        FieldKind::Submit | FieldKind::Button | FieldKind::Select => (
            (text::width(&field_label(f), &st) + 28).clamp(40, 400),
            FIELD_H,
        ),
        FieldKind::TextArea => (f.width, FIELD_H * 2),
        _ => (f.width, FIELD_H),
    }
}

fn draw_field(
    c: &mut Canvas<'_>,
    r: Rect,
    f: &html::Field,
    styled: bool,
    focused: bool,
    blink: bool,
) {
    let (bg, border, fg, dim, accent) = if styled {
        (
            Color::WHITE,
            Color::hex(0x9aa0a6),
            Color::hex(0x202124),
            Color::hex(0x80868b),
            Color::hex(0x1a73e8),
        )
    } else {
        (
            FIELD_BG,
            theme::PANEL_RIM,
            theme::TEXT,
            theme::TEXT_DIM,
            theme::CYAN,
        )
    };
    let st = Style::new(Weight::Regular, Size::Size16, fg);
    match f.kind {
        FieldKind::Checkbox | FieldKind::Radio => {
            let rad = if f.kind == FieldKind::Radio { 9 } else { 3 };
            rounded_rect(c, r.x, r.y, r.w, r.h, rad, bg, 255);
            rounded_outline(
                c,
                r.x,
                r.y,
                r.w,
                r.h,
                rad,
                if focused { accent } else { border },
            );
            if f.checked {
                rounded_rect(c, r.x + 4, r.y + 4, r.w - 8, r.h - 8, rad / 2, accent, 255);
            }
        }
        FieldKind::Submit | FieldKind::Button | FieldKind::Select => {
            let fill = if styled {
                Color::hex(0xf1f3f4)
            } else {
                theme::PANEL
            };
            rounded_rect(c, r.x, r.y, r.w, r.h, 4, fill, 255);
            rounded_outline(
                c,
                r.x,
                r.y,
                r.w,
                r.h,
                4,
                if focused { accent } else { border },
            );
            let label = field_label(f);
            let tw = text::width(&label, &st);
            text::draw(c, r.x + (r.w - tw) / 2, r.y + (r.h - 16) / 2, &label, &st);
        }
        _ => {
            rounded_rect(c, r.x, r.y, r.w, r.h, 4, bg, 255);
            rounded_outline(
                c,
                r.x,
                r.y,
                r.w,
                r.h,
                4,
                if focused { accent } else { border },
            );
            let shown = if f.kind == FieldKind::Password {
                "*".repeat(f.value.chars().count())
            } else {
                f.value.clone()
            };
            let (t, style) = if shown.is_empty() && !focused {
                (f.placeholder.clone(), st.color(dim))
            } else {
                (shown, st)
            };
            let tw = draw_fit(c, r.x + 8, r.y + 8, &t, &style, r.w - 16);
            if focused && blink {
                c.fill_rect(r.x + 9 + tw, r.y + 6, 2, r.h - 12, accent);
            }
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

/// Tamaño de una imagen en la página: el que pide el HTML, o el suyo, sin pasarse del ancho.
fn image_size(doc: &Document, images: &[Img], i: usize, max_w: i32) -> Option<(i32, i32)> {
    let r = doc.images.get(i)?;
    let natural = match images.get(i) {
        Some(Img::Ready(img)) => Some((img.width as i32, img.height as i32)),
        _ => None,
    };
    let (w, h) = match (r.width, r.height, natural) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some((nw, nh))) => (w, w * nh / nw.max(1)),
        (None, Some(h), Some((nw, nh))) => (h * nw / nh.max(1), h),
        (_, _, Some(n)) => n,
        // Sin tamaño y sin imagen todavía: un lugar chico para el texto alternativo.
        (Some(w), None, None) => (w, 24),
        (None, Some(h), None) => (h * 2, h),
        (None, None, None) => return None,
    };
    if w <= 0 || h <= 0 {
        return None;
    }
    let max_w = max_w.max(40);
    Some(if w > max_w {
        (max_w, h * max_w / w)
    } else {
        (w, h.min(2000))
    })
}

/// Arma el documento en renglones de `width` píxeles. Devuelve los renglones y el alto total.
fn layout(doc: &Document, images: &[Img], width: i32, fg: Color, dark: bool) -> (Vec<Line>, i32) {
    let mut lines: Vec<Line> = Vec::new();
    let mut y = 0;
    let mut prev_bg: Option<Color> = None;
    for block in &doc.blocks {
        let (gap_before, gap_after) = match block.kind {
            BlockKind::Heading(1) => (18, 10),
            BlockKind::Heading(2) => (16, 8),
            BlockKind::Heading(_) => (12, 6),
            BlockKind::Item => (2, 2),
            BlockKind::Pre | BlockKind::Quote => (6, 8),
            BlockKind::Rule => (8, 8),
            BlockKind::Text => (6, 8),
        };
        if !lines.is_empty() {
            y += gap_before;
        }
        // Dos bloques seguidos con el mismo fondo son la misma caja: sin franja entre ellos.
        let mut extend_up = match lines.last() {
            Some(last) if block.bg.is_some() && block.bg == prev_bg => {
                (y - (last.y + last.h)).max(0)
            }
            _ => 0,
        };
        prev_bg = block.bg;
        if block.kind == BlockKind::Rule {
            lines.push(Line {
                y,
                h: 8,
                items: Vec::new(),
                rule: true,
                bg: block.bg,
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
        let mut cur: Vec<Item> = Vec::new();
        let mut x = indent;
        let mut finish = |items: &mut Vec<Item>, lines: &mut Vec<Line>, y: &mut i32| {
            let line_w = items.iter().map(|i| i.x + i.w).max().unwrap_or(indent) - indent;
            let shift = match block.align {
                Align::Left => 0,
                Align::Center => ((max_x - indent - line_w) / 2).max(0),
                Align::Right => (max_x - indent - line_w).max(0),
            };
            let h = items.iter().map(|i| i.h).max().unwrap_or(18) + 4;
            for it in items.iter_mut() {
                it.x += shift;
            }
            let up = core::mem::take(&mut extend_up);
            lines.push(Line {
                y: *y - up,
                h: h + up,
                items: core::mem::take(items),
                rule: false,
                bg: block.bg,
            });
            *y += h;
        };
        for span in &block.spans {
            let color = if dark {
                match block.kind {
                    BlockKind::Pre => theme::PARTICLE_BRIGHT,
                    BlockKind::Quote => theme::TEXT_DIM,
                    BlockKind::Heading(1) => Color::WHITE,
                    BlockKind::Heading(2) => theme::CYAN,
                    BlockKind::Heading(_) => theme::PARTICLE_BRIGHT,
                    _ if span.dim => theme::TEXT_DIM,
                    _ => fg,
                }
            } else {
                span.color
                    .unwrap_or(if span.dim { Color::hex(0x70757a) } else { fg })
            };
            let weight = if span.bold {
                Weight::Bold
            } else {
                Weight::Regular
            };
            let st = Style::new(weight, size_of(span.size), color);
            let lh = st.line_height();
            // Imágenes y campos: una caja.
            let boxed = match (span.image, span.field) {
                (Some(i), _) => image_size(doc, images, i, max_x - indent)
                    .map(|(w, h)| (ItemKind::Image(i), w, h)),
                (_, Some(f)) => doc.fields.get(f).map(|fl| {
                    let (w, h) = field_size(fl);
                    (ItemKind::Field(f), w.min(max_x - indent), h)
                }),
                _ => None,
            };
            if let Some((kind, w, h)) = boxed {
                if x + w > max_x && x > indent {
                    finish(&mut cur, &mut lines, &mut y);
                    x = indent;
                }
                cur.push(Item {
                    x,
                    w,
                    h,
                    kind,
                    link: span.link,
                });
                x += w + 6;
                continue;
            }
            if span.image.is_some() {
                // Imagen sin tamaño todavía: se muestra su texto alternativo.
                if span.text.is_empty() {
                    continue;
                }
            }
            let space_w = text::width(" ", &st);
            let pieces: Vec<&str> = span.text.split('\n').collect();
            for (pi, piece) in pieces.iter().enumerate() {
                if pi > 0 {
                    finish(&mut cur, &mut lines, &mut y);
                    x = indent;
                }
                let words: Vec<&str> = if block.kind == BlockKind::Pre {
                    alloc::vec![*piece]
                } else {
                    piece.split(' ').collect()
                };
                for (wi, word) in words.iter().enumerate() {
                    if wi > 0 && x > indent {
                        x += space_w;
                    }
                    if word.is_empty() {
                        continue;
                    }
                    let mut w = text::width(word, &st);
                    if x + w > max_x && x > indent && block.kind != BlockKind::Pre {
                        finish(&mut cur, &mut lines, &mut y);
                        x = indent;
                    }
                    // Una palabra más larga que el renglón se corta.
                    let word = if w > max_x - indent {
                        let fitted = text::fit(word, &st, max_x - indent);
                        w = text::width(&fitted, &st);
                        fitted
                    } else {
                        word.to_string()
                    };
                    let bg = if dark { None } else { span.bg };
                    // Palabras seguidas con el mismo estilo van en un solo trozo.
                    match cur.last_mut() {
                        Some(Item {
                            x: lx,
                            w: lw,
                            kind:
                                ItemKind::Text {
                                    text: lt,
                                    style: ls,
                                    underline: lu,
                                    bg: lb,
                                },
                            link,
                            ..
                        }) if *link == span.link
                            && ls.color == st.color
                            && core::mem::discriminant(&ls.weight)
                                == core::mem::discriminant(&st.weight)
                            && core::mem::discriminant(&ls.size)
                                == core::mem::discriminant(&st.size)
                            && *lu == span.underline
                            && *lb == bg
                            && *lx + *lw + space_w == x =>
                        {
                            lt.push(' ');
                            lt.push_str(&word);
                            *lw = x + w - *lx;
                        }
                        _ => cur.push(Item {
                            x,
                            w,
                            h: lh,
                            kind: ItemKind::Text {
                                text: word,
                                style: st,
                                underline: span.underline,
                                bg,
                            },
                            link: span.link,
                        }),
                    }
                    x += w;
                }
            }
        }
        finish(&mut cur, &mut lines, &mut y);
        y += gap_after - 4;
    }
    (lines, y)
}

/// Una carpeta del disco como página (para `file:///Descargas`).
fn dir_listing<D: BlockDevice>(fs: &mut jarvis_fs::FileSystem<D>, path: &str) -> String {
    let mut entries = fs.list(path).unwrap_or_default();
    entries.retain(|e| !e.name.starts_with('.'));
    entries.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
    let mut s = format!("<title>{path}</title><h1>{path}</h1><ul>");
    if path != "/" {
        s.push_str(&format!(
            "<li><a href=\"file://{}\">.. (subir)</a></li>",
            crate::files::parent(path)
        ));
    }
    for e in &entries {
        let full = crate::files::join(path, &e.name);
        let size = if e.is_dir {
            "carpeta".into()
        } else {
            crate::files::format_size(e.size as u64)
        };
        s.push_str(&format!(
            "<li><a href=\"file://{full}\">{}</a> · {size}</li>",
            e.name
        ));
    }
    s.push_str("</ul>");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items_text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .flat_map(|l| l.items.iter())
            .filter_map(|i| match &i.kind {
                ItemKind::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn arma_renglones_sin_pasarse_del_ancho() {
        let doc = html::parse(
            "<h1>Título largo de prueba</h1><p>palabra otra <a href=/x>enlace con varias palabras</a> \
             y más texto que no entra en un solo renglón porque es largo.</p><pre>a\nb</pre>",
        );
        let (lines, total) = layout(&doc, &[], 300, theme::TEXT, true);
        assert!(total > 0);
        for l in &lines {
            for it in &l.items {
                assert!(
                    it.x + it.w <= 300,
                    "{:?}",
                    items_text(core::slice::from_ref(l))
                );
            }
        }
        let linked: String = lines
            .iter()
            .flat_map(|l| l.items.iter())
            .filter(|i| i.link == Some(0))
            .filter_map(|i| match &i.kind {
                ItemKind::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(linked, "enlace con varias palabras");
        // El <pre> respeta los saltos de línea.
        let t = items_text(&lines);
        assert_eq!(&t[t.len() - 2..], ["a", "b"]);
    }

    #[test]
    fn centra_y_pone_campos_e_imagenes() {
        let doc = html::render(
            "<div style='text-align:center'>hola</div><form><input name=q size=20><input type=submit value=Ir></form><img src=x width=100 height=50>",
            Options::STYLED,
            &[],
        );
        let (lines, _) = layout(&doc, &[], 600, Color::BLACK, false);
        let hola = &lines[0].items[0];
        assert!(hola.x > 250, "centrado: {}", hola.x);
        let kinds: Vec<(i32, i32)> = lines[1].items.iter().map(|i| (i.w, i.h)).collect();
        assert_eq!(kinds[0], (180, FIELD_H));
        assert!(matches!(lines[2].items[0].kind, ItemKind::Image(0)));
        assert_eq!((lines[2].items[0].w, lines[2].items[0].h), (100, 50));
    }
}
