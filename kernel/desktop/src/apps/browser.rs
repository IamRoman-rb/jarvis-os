//! Navegador web: barra de direcciones, atrás/adelante, enlaces, buscador, estilos CSS,
//! imágenes, formularios y descargas.
//!
//! Las páginas se piden al kernel (que tiene la red) por el `Outbox` y la respuesta vuelve con
//! [`Browser::net_response`]. El HTML y el CSS se preparan (`web::html`: árbol y estilos) y se
//! maquetan en cajas del ancho de la ventana (`web::layout`), que acá se dibujan con la fuente
//! proporcional de las páginas. Después de la página se bajan sus hojas de estilo externas (la
//! página se vuelve a armar con ellas) y sus imágenes (el puente del anfitrión las convierte a
//! BMP, el formato que el kernel sabe leer).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::BlockDevice;
use jarvis_gfx::shapes::{line, rounded_outline, rounded_rect};
use jarvis_gfx::text::{self, Style};
use jarvis_gfx::webfont::{FontSpec, WebFonts};
use jarvis_gfx::{Canvas, Color, Rect, theme};

use super::{Click, Ctx};
use crate::bmp;
use crate::config::Config;
use crate::i18n::{tr, trf};
use crate::input::{Key, Mods};
use crate::system::{FetchKind, HttpResponse};
use crate::text_input::TextInput;
use crate::web::css::Media;
use crate::web::dom::NodeKind;
use crate::web::html::{self, Document, FieldKind, NONE, Options, Prepared};
use crate::web::layout::{self, Env, Page, Paint};
use crate::web::sites;
use crate::web::style::Display;
use crate::web::url::{self, Scheme, Url};
use crate::widgets::{draw_fit, field_bg, label, light, s16, window_bg};

const BAR_H: i32 = 48;
const STATUS_H: i32 = 26;
const MARGIN: i32 = 28;
const MAX_IMAGES: usize = 60;
/// Hojas de estilo por página (GitHub, con sus módulos de CSS, usa unas 20).
const MAX_CSS: usize = 32;
/// Ancho supuesto de la página hasta que se dibuja por primera vez.
const DEFAULT_W: i32 = 1000;
/// Fondo del tema oscuro ("páginas claras" apagado).
const DARK_BG: Color = Color::hex(0x0b1220);
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
<li><b>Win+Z</b>: distribuciones (mitades, tercios, cuartos...) · <b>Win+Izq.</b> y después <b>Win+Arriba/Abajo</b>: un cuarto · <b>Win+Shift+T</b>: mosaico · <b>Win+Shift+C</b>: cascada</li>\
<li>Arrastrar una ventana contra un borde la acopla a una mitad; contra una esquina, a un cuarto; arriba, la maximiza</li>\
<li><b>Win+1</b> ... <b>Win+0</b>: los íconos de la barra · <b>Win+Ctrl+Shift+B</b>: redibujar la pantalla · <b>F1</b>: esta ayuda</li></ul>\
<h2>Archivos y editor</h2><ul>\
<li><b>Ctrl+E</b> / <b>Ctrl+A</b>: seleccionar todo · <b>Shift+flechas</b>: seleccionar mientras te movés · <b>Ctrl+clic</b> / <b>Shift+clic</b>: varios archivos</li>\
<li><b>Ctrl+C</b> / <b>Ctrl+X</b> / <b>Ctrl+V</b>: copiar, cortar y pegar (archivos o texto) · <b>Supr</b>: a la Papelera · <b>F2</b>: renombrar</li>\
<li>En el editor: arrastrar con el mouse selecciona, doble clic elige la palabra, <b>Ctrl+Izq./Der.</b> salta de a palabras</li></ul>\
<h2>Navegador</h2><ul>\
<li><b>Ctrl+L</b> / <b>Alt+D</b> / <b>F6</b>: escribir una dirección · <b>Alt+Izq./Der.</b>: atrás/adelante · <b>F5</b>: recargar</li>\
<li><b>Tab</b>: siguiente enlace o campo · <b>Enter</b>: abrirlo · <b>F9</b>: modo lectura · <b>Ctrl+H</b>: inicio</li></ul>\
<h2>Terminal</h2><ul>\
<li><b>Tab</b>: completar · <b>Arriba/Abajo</b>: historial · <b>Ctrl+C</b>: cancelar · <b>Ctrl+L</b>: limpiar · <b>Ctrl+A/E/U/K/W</b>: editar la línea</li>\
<li><b>help</b> lista los comandos · <b>apt install neofetch</b> instala un programa</li></ul>";

enum State {
    Loading { id: u32, url: String, since: u64 },
    Page,
    Error(String),
}

enum Img {
    /// Todavía no se pidió.
    Pending,
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
    /// La página: árbol, estilos, enlaces y campos.
    prep: Option<Prepared>,
    empty: Document,
    state: State,
    back: Vec<String>,
    forward: Vec<String>,
    scroll: i32,
    /// (ancho, página armada)
    layout: Option<(i32, Page)>,
    /// Ancho con el que se calcularon los estilos (`@media` depende de él).
    styled_width: i32,
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
    /// Cuántas de las hojas de la página ya se pidieron.
    css_seen: usize,
    images: Vec<Img>,
    /// Las fuentes de las páginas (se cargan con la primera página).
    fonts: Option<WebFonts>,
}

impl Browser {
    pub fn new(cfg: &Config) -> Self {
        Browser {
            dirty: true,
            address: TextInput::new("", 500),
            editing: false,
            replace_on_type: false,
            url: None,
            prep: None,
            empty: Document::default(),
            state: State::Page,
            back: Vec::new(),
            forward: Vec::new(),
            scroll: 0,
            layout: None,
            styled_width: DEFAULT_W,
            focus_link: None,
            focus_field: None,
            status: String::new(),
            prefs: Prefs::of(cfg),
            reader_toggle: false,
            html: None,
            css: Vec::new(),
            css_seen: 0,
            images: Vec::new(),
            fonts: None,
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
                images: self.prefs.images,
            }
        }
    }

    /// Colores del tema oscuro en vez de los de la página ("páginas claras" apagado).
    fn dark(&self) -> bool {
        !self.prefs.light && !self.reader()
    }

    fn doc(&self) -> &Document {
        self.prep.as_ref().map_or(&self.empty, |p| &p.doc)
    }

    fn doc_mut(&mut self) -> &mut Document {
        match &mut self.prep {
            Some(p) => &mut p.doc,
            None => &mut self.empty,
        }
    }

    pub fn title(&self) -> String {
        let t = if self.doc().title.is_empty() {
            self.url
                .as_ref()
                .map(|u| u.host.clone())
                .unwrap_or_default()
        } else {
            self.doc().title.clone()
        };
        if t.is_empty() {
            tr("Navegador").into()
        } else {
            trf("{} · Navegador", &[&t])
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
        let Some(p) = &self.prep else {
            return String::new();
        };
        let mut out = String::new();
        for n in &p.dom.nodes {
            if let NodeKind::Text(t) = &n.kind
                && n.parent.is_some_and(|par| {
                    p.styled
                        .get(par)
                        .is_some_and(|s| s.display != Display::None && s.visible)
                })
                && !t.trim().is_empty()
            {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&t.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
        out
    }

    pub fn document(&self) -> &Document {
        self.doc()
    }

    /// La página ya preparada (árbol y estilos), para los tests.
    pub fn prepared(&self) -> Option<&Prepared> {
        self.prep.as_ref()
    }

    /// La página armada (lo que se dibuja), si ya se dibujó.
    pub fn laid_out(&self) -> Option<&Page> {
        self.layout.as_ref().map(|l| &l.1)
    }

    /// (imágenes del documento, listas, fallidas), para los tests y la vista previa.
    pub fn image_stats(&self) -> (usize, usize, usize) {
        let ready = self
            .images
            .iter()
            .filter(|i| matches!(i, Img::Ready(_)))
            .count();
        let failed = self
            .images
            .iter()
            .filter(|i| matches!(i, Img::Failed))
            .count();
        (self.doc().images.len(), ready, failed)
    }

    /// (hojas de estilo pedidas, las que llegaron, bytes de CSS) — para la vista previa.
    pub fn css_stats(&self) -> (usize, usize, usize) {
        let ready = self.css.iter().filter(|(_, c)| c.is_some()).count();
        let bytes = self
            .css
            .iter()
            .filter_map(|(_, c)| c.as_ref())
            .map(|c| c.len())
            .sum();
        (self.css.len(), ready, bytes)
    }

    /// Cuánto está bajada la página.
    pub fn scroll(&self) -> i32 {
        self.scroll
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
        self.css_seen = 0;
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
                    self.show_error(tr("No hay disco.").into());
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
                        } else if [".bmp", ".png", ".jpg", ".jpeg"]
                            .iter()
                            .any(|e| lower.ends_with(e))
                        {
                            self.show_image(bytes);
                        } else {
                            self.show_html(html::plain_html(&html::decode_bytes(&bytes)), ctx);
                        }
                    }
                    Err(e) => self.show_error(format!("{path}: {e}")),
                }
            }
            Scheme::Http | Scheme::Https => {
                let lower = u.path_only().to_ascii_lowercase();
                let kind = if [
                    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg",
                ]
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
                    ".snap",
                    ".flatpak",
                    ".msix",
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
                self.status = note.unwrap_or_else(|| trf("Conectando con {}...", &[&u.host]));
            }
        }
    }

    fn show(&mut self, prep: Prepared) {
        self.prep = Some(prep);
        self.state = State::Page;
        self.layout = None;
        self.status = String::new();
        self.dirty = true;
        if self.doc().needs_js {
            self.status = tr(
                "Esta página se arma con JavaScript, que JARVIS-OS todavía no ejecuta: puede verse incompleta.",
            )
            .into();
        }
    }

    fn show_error(&mut self, msg: String) {
        self.prep = None;
        self.state = State::Error(msg);
        self.layout = None;
        self.dirty = true;
    }

    fn media(&self) -> Media {
        Media {
            width: self.styled_width,
            height: 700,
        }
    }

    fn show_image(&mut self, bytes: Vec<u8>) {
        match bmp::decode_any(&bytes, bmp::DISK_MAX_SIDE) {
            Some(img) => {
                let prep = html::prepare(
                    "<body style='margin:0;background:#202124;text-align:center'>\
                     <img src=imagen style='max-width:100%'>",
                    Options::STYLED,
                    &[],
                    self.media(),
                );
                self.images = alloc::vec![Img::Ready(img)];
                self.show(prep);
            }
            None => self.show_error(tr("No se pudo leer la imagen.").into()),
        }
    }

    /// Arma la página y pide sus hojas de estilo y sus imágenes.
    fn show_html<D: BlockDevice>(&mut self, text: String, ctx: &mut Ctx<'_, D>) {
        let text = sites::adapt(self.url.as_ref(), text);
        let prep = html::prepare(&text, self.options(), &[], self.media());
        self.html = Some(text);
        self.show(prep);
        self.request_css(ctx);
        self.request_images(ctx);
    }

    fn request_css<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some(base) = self.base() else { return };
        let sheets: Vec<String> = self.doc().stylesheets.clone();
        let seen = self.css_seen;
        self.css_seen = sheets.len();
        for href in sheets.iter().skip(seen) {
            if self.css.len() >= MAX_CSS {
                break;
            }
            if let Some(u) = base.join(href)
                && matches!(u.scheme, Scheme::Http | Scheme::Https)
            {
                let id = ctx.out.fetch(&u.to_string());
                self.css.push((id, None));
            }
        }
    }

    /// La dirección contra la que se resuelven los enlaces (`<base href>` o la de la página).
    fn base(&self) -> Option<Url> {
        let u = self.url.clone()?;
        match &self.doc().base {
            Some(b) => u.join(b).or(Some(u)),
            None => Some(u),
        }
    }

    /// Pide las imágenes que faltan.
    fn request_images<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let refs: Vec<String> = self.doc().images.iter().map(|i| i.src.clone()).collect();
        while self.images.len() < refs.len() {
            self.images.push(Img::Pending);
        }
        self.images.truncate(refs.len());
        if !self.options().images {
            return;
        }
        let Some(base) = self.base() else { return };
        let mut asked = self
            .images
            .iter()
            .filter(|i| !matches!(i, Img::Pending))
            .count();
        for (i, src) in refs.iter().enumerate() {
            if !matches!(self.images[i], Img::Pending) {
                continue;
            }
            let target = base.join(src);
            let ok = asked < MAX_IMAGES
                && target
                    .as_ref()
                    .is_some_and(|u| matches!(u.scheme, Scheme::Http | Scheme::Https));
            self.images[i] = match (ok, target) {
                (true, Some(u)) => {
                    asked += 1;
                    Img::Loading(ctx.out.fetch_kind(&u.to_string(), FetchKind::Image))
                }
                _ => Img::Failed,
            };
        }
    }

    /// Vuelve a armar la página (llegaron hojas de estilo o cambió la configuración).
    fn restyle(&mut self) {
        let Some(text) = &self.html else {
            return;
        };
        let css: Vec<String> = self.css.iter().filter_map(|(_, c)| c.clone()).collect();
        let mut prep = html::prepare(text, self.options(), &css, self.media());
        // Lo que se escribió en los formularios no se pierde.
        let old = self.doc();
        if prep.doc.fields.len() == old.fields.len() {
            for (new, old) in prep.doc.fields.iter_mut().zip(&old.fields) {
                new.value = old.value.clone();
                new.checked = old.checked;
            }
        }
        // Con otras reglas cambian qué imágenes se ven: las que siguen (misma dirección) se
        // conservan, bajadas o por bajar; las nuevas se piden después.
        let old_srcs: Vec<String> = old.images.iter().map(|i| i.src.clone()).collect();
        let mut old_imgs: Vec<(String, Img)> = old_srcs
            .into_iter()
            .zip(core::mem::take(&mut self.images))
            .collect();
        self.images = prep
            .doc
            .images
            .iter()
            .map(|r| match old_imgs.iter().position(|(s, _)| *s == r.src) {
                Some(k) => old_imgs.swap_remove(k).1,
                None => Img::Pending,
            })
            .collect();
        let needs_js = prep.doc.needs_js;
        self.prep = Some(prep);
        if !needs_js && self.status.contains("JavaScript") {
            self.status.clear();
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
                // Las hojas pueden traer otras (`@import`) e imágenes de fondo.
                self.request_css(ctx);
                self.request_images(ctx);
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
                Ok(r) if r.status < 400 => {
                    bmp::decode_any(&r.body, bmp::WEB_MAX_SIDE).map_or(Img::Failed, Img::Ready)
                }
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
            Err(e) => self.show_error(trf("No se pudo abrir la página: {}", &[e])),
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
                        self.doc_mut().title = format!("Error {}", resp.status);
                    }
                } else if ct.starts_with("text/")
                    || ct.contains("json")
                    || ct.contains("xml")
                    || ct.contains("javascript")
                {
                    let text = html::plain_html(&html::decode_bytes(&resp.body));
                    self.show_html(text, ctx);
                } else if resp.status < 400 {
                    self.save_download(resp, ctx);
                    return;
                } else {
                    self.show_error(format!("Error {} ({})", resp.status, resp.content_type));
                }
                if self.status.is_empty() {
                    self.status = status;
                }
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
            self.show_error(tr("No hay disco para guardar la descarga.").into());
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
                    "<title>Descarga completa</title><body style='font-family:sans-serif;margin:32px'>\
                     <h1>Descarga completa</h1>\
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
        // Imágenes que aparecieron al rearmar la página (otro ancho, otras reglas).
        if self.images.iter().any(|i| matches!(i, Img::Pending)) && self.options().images {
            self.request_images(ctx);
        }
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
        let Some(href) = self.doc().links.get(link).cloned() else {
            return;
        };
        if let Some(anchor) = href.strip_prefix('#') {
            self.scroll_to_anchor(anchor);
            return;
        }
        if href.to_ascii_lowercase().starts_with("javascript:") {
            self.status = tr("Ese enlace necesita JavaScript.").into();
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

    /// `#seccion`: va al elemento con ese id (el primer texto que tiene adentro o después).
    fn scroll_to_anchor(&mut self, anchor: &str) {
        let Some(p) = &self.prep else { return };
        let Some(target) = (0..p.dom.nodes.len()).find(|&i| {
            p.dom.nodes[i].attr("id") == Some(anchor) || p.dom.nodes[i].attr("name") == Some(anchor)
        }) else {
            return;
        };
        // El primer texto desde ese nodo en adelante (en orden del documento).
        let first_text = (target..p.dom.nodes.len())
            .find(|&i| matches!(&p.dom.nodes[i].kind, NodeKind::Text(t) if !t.trim().is_empty()));
        let Some(t) = first_text else { return };
        let word = match &p.dom.nodes[t].kind {
            NodeKind::Text(s) => s.split_whitespace().next().unwrap_or("").to_string(),
            _ => return,
        };
        if let Some((_, page)) = &self.layout {
            let mut from = 0;
            // Se busca la palabra debajo de lo que ya se ve primero, después en toda la página.
            for pass in 0..2 {
                for pt in &page.paints {
                    if let Paint::Text { text, top, .. } = pt
                        && *text == word
                        && (pass == 1 || *top >= from)
                    {
                        self.scroll = (*top - 8).max(0);
                        self.dirty = true;
                        return;
                    }
                }
                from = 0;
            }
        }
    }

    // --- formularios --------------------------------------------------------------------------

    /// Se apretó un botón (o Enter en un campo): se arma la dirección con los campos del
    /// formulario (GET) y se va.
    fn submit<D: BlockDevice>(&mut self, from: usize, ctx: &mut Ctx<'_, D>) {
        let Some(field) = self.doc().fields.get(from) else {
            return;
        };
        let Some(form_idx) = field.form else {
            if field.kind == FieldKind::Button {
                self.status = tr("Ese botón necesita JavaScript.").into();
                self.dirty = true;
            }
            return;
        };
        let Some(form) = self.doc().forms.get(form_idx).cloned() else {
            return;
        };
        if form.post {
            self.status = tr(
                "Este formulario manda datos con POST: el puente solo acepta GET (ver ADR 0004).",
            )
            .into();
            self.dirty = true;
            return;
        }
        let mut pairs: Vec<(String, String)> = Vec::new();
        for (i, f) in self.doc().fields.iter().enumerate() {
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
        let Some(f) = self.doc().fields.get(i) else {
            return;
        };
        match f.kind {
            FieldKind::Text | FieldKind::Password | FieldKind::TextArea => {
                self.focus_field = Some(i);
                self.focus_link = None;
            }
            FieldKind::Checkbox => {
                let f = &mut self.doc_mut().fields[i];
                f.checked = !f.checked;
            }
            FieldKind::Radio => {
                let (form, name) = (f.form, f.name.clone());
                for g in &mut self.doc_mut().fields {
                    if g.kind == FieldKind::Radio && g.form == form && g.name == name {
                        g.checked = false;
                    }
                }
                self.doc_mut().fields[i].checked = true;
            }
            FieldKind::Select => {
                let f = &mut self.doc_mut().fields[i];
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

    /// Fondo de la página.
    fn page_bg(&self) -> Color {
        if self.dark() {
            return DARK_BG;
        }
        self.prep
            .as_ref()
            .and_then(html::canvas_color)
            .unwrap_or(Color::WHITE)
    }

    fn ensure_layout(&mut self, width: i32) {
        if self.layout.as_ref().is_some_and(|l| l.0 == width) {
            return;
        }
        // `@media` depende del ancho: si cambió, se recalculan los estilos.
        if width != self.styled_width && self.html.is_some() {
            self.styled_width = width;
            self.restyle();
        }
        self.styled_width = width;
        if self.fonts.is_none() {
            self.fonts = Some(WebFonts::new());
        }
        let sizes: Vec<Option<(i32, i32)>> = self
            .images
            .iter()
            .map(|i| match i {
                Img::Ready(img) => Some((img.width as i32, img.height as i32)),
                _ => None,
            })
            .collect();
        let page = match (&self.prep, &self.fonts) {
            (Some(p), Some(fonts)) => {
                let env = Env {
                    fonts,
                    image_sizes: &sizes,
                    width,
                    height: 700,
                    dark: self.dark(),
                };
                layout::layout(p, &env)
            }
            _ => Page::default(),
        };
        self.layout = Some((width, page));
    }

    fn max_scroll(&self, page: Rect) -> i32 {
        self.layout
            .as_ref()
            .map_or(0, |l| (l.1.height - page.h + 40).max(0))
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
        let Some((_, laid)) = &self.layout else {
            return;
        };
        // (es campo, índice, y)
        let mut order: Vec<(bool, usize, i32)> = Vec::new();
        for p in &laid.paints {
            let key = match p {
                Paint::Field { field, r, .. } => Some((true, *field as usize, r.y)),
                Paint::Hit { field, r, .. } if *field != NONE => Some((true, *field as usize, r.y)),
                Paint::Text { link, top, .. } if *link != NONE => {
                    Some((false, *link as usize, *top))
                }
                Paint::Image { link, r, .. } | Paint::Hit { link, r, .. } if *link != NONE => {
                    Some((false, *link as usize, r.y))
                }
                _ => None,
            };
            if let Some((is_field, k, y)) = key
                && !order.iter().any(|(f, o, _)| *f == is_field && *o == k)
            {
                // Los campos ocultos no reciben el foco.
                if is_field
                    && self
                        .doc()
                        .fields
                        .get(k)
                        .is_some_and(|f| f.kind == FieldKind::Hidden)
                {
                    continue;
                }
                order.push((is_field, k, y));
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
            self.status = self.doc().links.get(k).cloned().unwrap_or_default();
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
            && fi < self.doc().fields.len()
        {
            let kind = self.doc().fields[fi].kind;
            let typing = matches!(
                kind,
                FieldKind::Text | FieldKind::Password | FieldKind::TextArea
            );
            match key {
                Key::Enter if kind == FieldKind::TextArea && mods.shift => {
                    self.doc_mut().fields[fi].value.push('\n');
                    return true;
                }
                Key::Enter => {
                    self.focus_field = None;
                    if typing {
                        self.submit(fi, ctx);
                    } else {
                        self.activate_field(fi, ctx);
                    }
                    return true;
                }
                Key::Char(' ') if !typing => {
                    self.activate_field(fi, ctx);
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
                Key::Backspace if typing => {
                    self.doc_mut().fields[fi].value.pop();
                    return true;
                }
                Key::Char(c) if typing && !mods.ctrl && !c.is_control() => {
                    if self.doc().fields[fi].value.chars().count() < 500 {
                        self.doc_mut().fields[fi].value.push(c);
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
                    tr("Modo lectura").into()
                } else {
                    tr("Página completa").into()
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
                self.state = State::Error(tr("Carga cancelada.").into());
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
                (Some(f), _) => self.activate_field(f, ctx),
                (None, Some(link)) => {
                    self.focus_link = Some(link);
                    self.follow(link, ctx);
                }
                _ => {}
            }
            self.dirty = true;
        }
    }

    /// (campo, enlace) bajo el punto.
    fn item_at(&mut self, page: Rect, x: i32, y: i32) -> (Option<usize>, Option<usize>) {
        self.ensure_layout(page.w);
        let Some((_, laid)) = self.layout.as_ref() else {
            return (None, None);
        };
        let px = x - page.x;
        let py = y - page.y + self.scroll;
        let some = |v: u32| (v != NONE).then_some(v as usize);
        for p in laid.paints.iter().rev() {
            match p {
                Paint::Field { r, field, .. } if r.inset(-3).contains(px, py) => {
                    return (some(*field), None);
                }
                Paint::Hit { r, link, field } if r.contains(px, py) => {
                    return (some(*field), some(*link));
                }
                Paint::Text {
                    x, top, w, h, link, ..
                } if *link != NONE && Rect::new(*x, *top, *w, *h).contains(px, py) => {
                    return (None, some(*link));
                }
                Paint::Image { r, link, .. } if *link != NONE && r.contains(px, py) => {
                    return (None, some(*link));
                }
                _ => {}
            }
        }
        (None, None)
    }

    pub fn wheel(&mut self, delta: i32, content: Rect) {
        self.scroll_by(delta * 60, content);
    }

    // --- dibujo -------------------------------------------------------------------------------

    pub fn draw(&mut self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        let page = Self::page_rect(r);
        match &self.state {
            State::Loading { url, since, .. } => {
                c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
                let dots = ".".repeat(((now_ms - since) / 250 % 4) as usize);
                let msg = trf("Cargando {}{}", &[url, &dots]);
                draw_fit(
                    c,
                    page.x + MARGIN,
                    page.y + 30,
                    &msg,
                    &s16(theme::cyan()),
                    page.w - 2 * MARGIN,
                );
                let secs = (now_ms - since) / 1000;
                if secs >= 3 {
                    let wait = trf("Hace {} s. Esc cancela.", &[&secs.to_string()]);
                    text::draw(
                        c,
                        page.x + MARGIN,
                        page.y + 56,
                        &wait,
                        &light(theme::text_dim()),
                    );
                }
            }
            State::Error(msg) => {
                c.fill_rect(r.x, r.y, r.w, r.h, window_bg());
                text::draw(
                    c,
                    page.x + MARGIN,
                    page.y + 30,
                    tr("NO SE PUDO CARGAR"),
                    &label(theme::amber()),
                );
                let st = s16(theme::text());
                let mut y = page.y + 60;
                for chunk in wrap_plain(msg, &st, page.w - 2 * MARGIN) {
                    text::draw(c, page.x + MARGIN, y, &chunk, &st);
                    y += 22;
                }
            }
            State::Page => {
                let bg = self.page_bg();
                c.fill_rect(r.x, r.y, r.w, r.h, bg);
                self.draw_page(c, page, now_ms);
            }
        }
        // La barra va después: tapa lo que se asoma arriba de la página.
        self.draw_bar(c, r, now_ms);
        // Barra de estado.
        let s = Rect::new(r.x, r.y + r.h - STATUS_H, r.w, STATUS_H);
        c.fill_rect(s.x, s.y, s.w, s.h, theme::panel());
        let loading_images = self
            .images
            .iter()
            .filter(|i| matches!(i, Img::Loading(_) | Img::Pending))
            .count()
            + self.css.iter().filter(|(_, c)| c.is_none()).count();
        let msg = if !self.status.is_empty() {
            self.status.clone()
        } else if loading_images > 0 {
            trf(
                "Bajando {} imágenes y estilos...",
                &[&loading_images.to_string()],
            )
        } else if self.reader() {
            tr("Listo · modo lectura (F9)").into()
        } else {
            tr("Listo").into()
        };
        draw_fit(
            c,
            s.x + 12,
            s.y + 5,
            &msg,
            &light(theme::text_dim()),
            s.w - 24,
        );
    }

    fn draw_bar(&self, c: &mut Canvas<'_>, r: Rect, now_ms: u64) {
        c.fill_rect(r.x, r.y, r.w, BAR_H, window_bg());
        let [back, fwd, reload, home] = Self::buttons(r);
        for (b, enabled) in [
            (back, !self.back.is_empty()),
            (fwd, !self.forward.is_empty()),
            (reload, self.url.is_some()),
            (home, true),
        ] {
            let col = if enabled {
                theme::text()
            } else {
                theme::text_faint()
            };
            rounded_rect(c, b.x, b.y, b.w, b.h, 4, theme::panel(), 255);
            rounded_outline(c, b.x, b.y, b.w, b.h, 4, theme::panel_rim());
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
                c.fill_rect(cx + dx - 1, cy + dy - 1, 3, 3, theme::cyan());
            } else {
                // Casita.
                line(c, cx - 7, cy, cx, cy - 7, col);
                line(c, cx, cy - 7, cx + 7, cy, col);
                jarvis_gfx::shapes::rect_outline(c, cx - 5, cy, 11, 7, col);
            }
        }
        let a = Self::address_rect(r);
        rounded_rect(c, a.x, a.y, a.w, a.h, 4, field_bg(), 255);
        let rim = if self.editing {
            theme::cyan()
        } else {
            theme::panel_rim()
        };
        rounded_outline(c, a.x, a.y, a.w, a.h, 4, rim);
        let secure = self.url.as_ref().is_some_and(|u| u.scheme == Scheme::Https);
        let mut x = a.x + 10;
        if !self.editing && secure {
            // Candado: la conexión del puente con el sitio es TLS.
            jarvis_gfx::hud::icon(
                c,
                jarvis_gfx::hud::Icon::Lock,
                x + 6,
                a.y + 16,
                theme::cyan(),
            );
            x += 20;
        }
        let st = s16(if self.editing {
            theme::text()
        } else {
            theme::text_dim()
        });
        let shown = if self.address.text.is_empty() && !self.editing {
            tr("Buscá o escribí una dirección")
        } else {
            self.address.text.as_str()
        };
        let tw = draw_fit(c, x, a.y + 8, shown, &st, a.x + a.w - x - 14);
        if self.editing {
            c.fill_rect(x + tw + 2, a.y + 7, 2, 18, theme::cyan());
        }
        line(
            c,
            r.x,
            r.y + BAR_H - 1,
            r.x + r.w - 1,
            r.y + BAR_H - 1,
            theme::panel_rim(),
        );
    }

    fn draw_page(&mut self, c: &mut Canvas<'_>, page: Rect, now_ms: u64) {
        self.ensure_layout(page.w);
        let scroll = self.scroll;
        let focus = self.focus_link.map(|l| l as u32);
        let focus_field = self.focus_field;
        let blink = (now_ms / 530).is_multiple_of(2);
        let dark = self.dark();
        let (Some((_, laid)), Some(fonts)) = (&self.layout, &self.fonts) else {
            return;
        };
        let (ox, oy) = (page.x, page.y - scroll);
        let outer = c.saved_clip();
        c.intersect_clip(page);
        let mut clips = Vec::new();
        let visible = |r: &Rect| r.y + oy < page.y + page.h && r.y + r.h + oy > page.y;
        for p in &laid.paints {
            match p {
                Paint::None | Paint::Hit { .. } => {}
                Paint::ClipPush(r) => {
                    clips.push(c.saved_clip());
                    c.intersect_clip(Rect::new(r.x + ox, r.y + oy, r.w, r.h));
                }
                Paint::ClipPop => {
                    if let Some(s) = clips.pop() {
                        c.restore_clip(s);
                    }
                }
                Paint::Rect {
                    r, color, radius, ..
                } if visible(r) => {
                    let rr = Rect::new(r.x + ox, r.y + oy, r.w, r.h);
                    if *radius > 1 {
                        rounded_rect(c, rr.x, rr.y, rr.w, rr.h, *radius, color.c, color.a);
                    } else {
                        fill_alpha(c, rr, color.c, color.a);
                    }
                }
                Paint::Border {
                    r,
                    w,
                    c: col,
                    radius,
                    ..
                } if visible(r) => {
                    let rr = Rect::new(r.x + ox, r.y + oy, r.w, r.h);
                    if col.iter().all(|x| x.a == 0) {
                        continue;
                    }
                    if *radius > 1
                        && w.iter().all(|&x| x == w[0])
                        && col.iter().all(|x| *x == col[0])
                    {
                        for k in 0..w[0].min(4) {
                            rounded_outline(
                                c,
                                rr.x + k,
                                rr.y + k,
                                rr.w - 2 * k,
                                rr.h - 2 * k,
                                (*radius - k).max(0),
                                col[0].c,
                            );
                        }
                    } else {
                        fill_alpha(c, Rect::new(rr.x, rr.y, rr.w, w[0]), col[0].c, col[0].a);
                        fill_alpha(
                            c,
                            Rect::new(rr.x + rr.w - w[1], rr.y, w[1], rr.h),
                            col[1].c,
                            col[1].a,
                        );
                        fill_alpha(
                            c,
                            Rect::new(rr.x, rr.y + rr.h - w[2], rr.w, w[2]),
                            col[2].c,
                            col[2].a,
                        );
                        fill_alpha(c, Rect::new(rr.x, rr.y, w[3], rr.h), col[3].c, col[3].a);
                    }
                }
                Paint::Text {
                    x,
                    base,
                    top,
                    h,
                    w,
                    text: t,
                    font,
                    color,
                    underline,
                    strike,
                    link,
                } => {
                    if *top + oy > page.y + page.h || *top + *h + oy < page.y {
                        continue;
                    }
                    let focused = *link != NONE && focus == Some(*link);
                    let col = if focused {
                        if dark {
                            Color::hex(0xffd166)
                        } else {
                            Color::hex(0xe37400)
                        }
                    } else if color.a < 255 {
                        // Texto semitransparente: se mezcla con el fondo de la página.
                        self.page_bg().lerp(color.c, color.a)
                    } else {
                        color.c
                    };
                    let (px, pb) = (x + ox, base + oy);
                    fonts.draw(c, px, pb, t, *font, col);
                    if *underline || focused {
                        let uy = pb + (font.px as i32 / 8).max(1) + 1;
                        c.fill_rect(px, uy, *w, (font.px as i32 / 14).max(1), col);
                    }
                    if *strike {
                        let sy = pb - font.px as i32 / 3;
                        c.fill_rect(px, sy, *w, 1, col);
                    }
                }
                Paint::Image { r, img, link } if visible(r) => {
                    let rr = Rect::new(r.x + ox, r.y + oy, r.w, r.h);
                    if let Some(Img::Ready(im)) = self.images.get(*img as usize) {
                        draw_image(c, im, rr, page);
                    }
                    if *link != NONE && focus == Some(*link) {
                        rounded_outline(
                            c,
                            rr.x - 2,
                            rr.y - 2,
                            rr.w + 4,
                            rr.h + 4,
                            2,
                            Color::hex(0xe37400),
                        );
                    }
                }
                Paint::BgImage {
                    bx,
                    dest,
                    img,
                    repeat,
                } if visible(bx) => {
                    let Some(Img::Ready(im)) = self.images.get(*img as usize) else {
                        continue;
                    };
                    let saved = c.saved_clip();
                    let b = Rect::new(bx.x + ox, bx.y + oy, bx.w, bx.h);
                    c.intersect_clip(b);
                    let d = Rect::new(dest.x + ox, dest.y + oy, dest.w, dest.h);
                    if *repeat && d.w >= 4 && d.h >= 4 {
                        // Mosaico desde la posición pedida, cubriendo la caja.
                        let mut x0 = d.x;
                        while x0 > b.x {
                            x0 -= d.w;
                        }
                        let mut y0 = d.y;
                        while y0 > b.y {
                            y0 -= d.h;
                        }
                        let mut count = 0;
                        let mut y = y0;
                        while y < b.y + b.h && count < 400 {
                            let mut x = x0;
                            while x < b.x + b.w && count < 400 {
                                if y + d.h > page.y && y < page.y + page.h {
                                    draw_image(c, im, Rect::new(x, y, d.w, d.h), page);
                                }
                                x += d.w;
                                count += 1;
                            }
                            y += d.h;
                        }
                    } else {
                        draw_image(c, im, d, page);
                    }
                    c.restore_clip(saved);
                }
                Paint::Mask {
                    bx,
                    dest,
                    img,
                    color,
                } if visible(bx) => {
                    if let Some(Img::Ready(im)) = self.images.get(*img as usize) {
                        let saved = c.saved_clip();
                        c.intersect_clip(Rect::new(bx.x + ox, bx.y + oy, bx.w, bx.h));
                        let d = Rect::new(dest.x + ox, dest.y + oy, dest.w, dest.h);
                        im.draw_mask(c, d, color.c);
                        c.restore_clip(saved);
                    }
                }
                Paint::Field {
                    r,
                    field,
                    font,
                    color,
                } if visible(r) => {
                    if let Some(f) = self.doc().fields.get(*field as usize) {
                        let rr = Rect::new(r.x + ox, r.y + oy, r.w, r.h);
                        draw_field(
                            c,
                            fonts,
                            rr,
                            f,
                            *font,
                            color.c,
                            focus_field == Some(*field as usize),
                            blink,
                        );
                    }
                }
                Paint::Placeholder { r, label: l } if visible(r) => {
                    let rr = Rect::new(r.x + ox, r.y + oy, r.w, r.h);
                    let (fill, rim, fg) = if dark {
                        (theme::panel(), theme::panel_rim(), theme::text_dim())
                    } else {
                        (
                            Color::hex(0xf1f3f4),
                            Color::hex(0xdadce0),
                            Color::hex(0x5f6368),
                        )
                    };
                    c.fill_rect(rr.x, rr.y, rr.w, rr.h, fill);
                    jarvis_gfx::shapes::rect_outline(c, rr.x, rr.y, rr.w, rr.h, rim);
                    if !l.is_empty() && rr.w > 30 && rr.h > 14 {
                        let f = FontSpec::new(13);
                        let saved = c.saved_clip();
                        c.intersect_clip(rr);
                        fonts.draw(c, rr.x + 4, rr.y + 14, l, f, fg);
                        c.restore_clip(saved);
                    }
                }
                _ => {}
            }
        }
        c.restore_clip(outer);
        // Barra de desplazamiento.
        let total = laid.height;
        if total > page.h {
            let track = Rect::new(page.x + page.w - 6, page.y + 4, 3, page.h - 8);
            let (tc, bc) = if dark {
                (theme::panel_rim(), theme::cyan().scale(180))
            } else {
                (Color::hex(0xdadce0), Color::hex(0x9aa0a6))
            };
            c.fill_rect(track.x, track.y, track.w, track.h, tc);
            let h = (track.h as i64 * page.h as i64 / total as i64).max(16) as i32;
            let max = (total - page.h + 40).max(1);
            let y = track.y + ((track.h - h) as i64 * scroll as i64 / max as i64) as i32;
            c.fill_rect(track.x, y, track.w, h, bc);
        }
    }
}

/// Un rectángulo con transparencia.
fn fill_alpha(c: &mut Canvas<'_>, r: Rect, color: Color, a: u8) {
    if r.w <= 0 || r.h <= 0 || a == 0 {
        return;
    }
    if a == 255 {
        c.fill_rect(r.x, r.y, r.w, r.h, color);
        return;
    }
    let vis = r.clamp(c.width(), c.height());
    for y in vis.y..vis.y + vis.h {
        for x in vis.x..vis.x + vis.w {
            c.blend(x, y, color, a);
        }
    }
}

/// Dibuja una imagen escalada, solo las filas que se ven.
fn draw_image(c: &mut Canvas<'_>, im: &bmp::Image, dst: Rect, page: Rect) {
    let y0 = dst.y.max(page.y);
    let y1 = (dst.y + dst.h).min(page.y + page.h);
    if y1 <= y0 {
        return;
    }
    im.draw_rows(c, dst, y0 - dst.y, y1 - dst.y);
}

#[allow(clippy::too_many_arguments)]
fn draw_field(
    c: &mut Canvas<'_>,
    fonts: &WebFonts,
    r: Rect,
    f: &html::Field,
    font: FontSpec,
    fg: Color,
    focused: bool,
    blink: bool,
) {
    let accent = Color::hex(0x1a73e8);
    let dim = Color::hex(0x80868b);
    let vm = fonts.vmetrics(font);
    let base = r.y + (r.h - (vm.ascent + vm.descent)) / 2 + vm.ascent;
    let saved = c.saved_clip();
    match f.kind {
        FieldKind::Checkbox | FieldKind::Radio => {
            let side = r.w.min(r.h).max(10);
            let (x, y) = (r.x, r.y + (r.h - side) / 2);
            let rad = if f.kind == FieldKind::Radio {
                side / 2
            } else {
                2
            };
            rounded_rect(c, x, y, side, side, rad, Color::WHITE, 255);
            rounded_outline(
                c,
                x,
                y,
                side,
                side,
                rad,
                if focused {
                    accent
                } else {
                    Color::hex(0x767676)
                },
            );
            if f.checked {
                rounded_rect(c, x + 3, y + 3, side - 6, side - 6, rad / 2, accent, 255);
            }
        }
        FieldKind::Submit | FieldKind::Button => {
            c.intersect_clip(r.inset(-4));
            let w = fonts.width(&f.value, font);
            fonts.draw(c, r.x + (r.w - w) / 2, base, &f.value, font, fg);
            if focused {
                rounded_outline(c, r.x - 4, r.y - 2, r.w + 8, r.h + 4, 3, accent);
            }
        }
        FieldKind::Select => {
            c.intersect_clip(r);
            let shown = f
                .options
                .iter()
                .find(|(v, _)| *v == f.value)
                .map_or(f.value.as_str(), |(_, l)| l.as_str());
            fonts.draw(c, r.x + 2, base, shown, font, fg);
            fonts.draw(c, r.x + r.w - 12, base, "▾", font, fg);
            c.restore_clip(saved);
            if focused {
                rounded_outline(c, r.x - 3, r.y - 2, r.w + 6, r.h + 4, 3, accent);
            }
        }
        _ => {
            c.intersect_clip(r);
            let shown = if f.kind == FieldKind::Password {
                "•".repeat(f.value.chars().count())
            } else {
                f.value.replace('\n', " ")
            };
            let (t, col) = if shown.is_empty() && !focused {
                (f.placeholder.clone(), dim)
            } else {
                (shown, fg)
            };
            // Si no entra, se ve el final (lo último que se escribió).
            let tw = fonts.width(&t, font);
            let x = if tw > r.w - 6 {
                r.x + r.w - 6 - tw
            } else {
                r.x + 2
            };
            fonts.draw(c, x, base, &t, font, col);
            if focused && blink {
                let cx = if t.is_empty() || shown_is_placeholder(f, focused) {
                    r.x + 2
                } else {
                    x + tw + 1
                };
                c.fill_rect(cx, base - vm.ascent, 2, vm.ascent + vm.descent, accent);
            }
            c.restore_clip(saved);
            if focused {
                rounded_outline(c, r.x - 4, r.y - 3, r.w + 8, r.h + 6, 3, accent);
            }
        }
    }
    c.restore_clip(saved);
}

fn shown_is_placeholder(f: &html::Field, focused: bool) -> bool {
    f.value.is_empty() && !focused
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

/// Una carpeta del disco como página (para `file:///Descargas`).
fn dir_listing<D: BlockDevice>(fs: &mut jarvis_fs::FileSystem<D>, path: &str) -> String {
    let mut entries = fs.list(path).unwrap_or_default();
    entries.retain(|e| !e.name.starts_with('.'));
    entries.sort_by_key(|e| (!e.is_dir, e.name.to_lowercase()));
    let mut s = format!(
        "<title>{path}</title><body style='font-family:sans-serif;margin:24px'><h1>{path}</h1><ul>"
    );
    if path != "/" {
        s.push_str(&format!(
            "<li><a href=\"file://{}\">.. (subir)</a></li>",
            crate::files::parent(path)
        ));
    }
    for e in &entries {
        let full = crate::files::join(path, &e.name);
        let size = if e.is_dir {
            tr("carpeta").into()
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
