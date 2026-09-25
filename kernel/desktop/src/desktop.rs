//! El escritorio: ventanas, atajos de teclado, mouse, el HUD de JARVIS de fondo y la
//! composición de cada frame.
//!
//! El kernel le pasa eventos (teclas, modificadores, mouse), el estado de la máquina y las
//! respuestas de la red, y le pide frames. A cambio, el escritorio le deja pedidos: páginas web,
//! tonos para el parlante, apagar. No sabe nada del hardware: todo esto corre igual en el host
//! (con un disco en memoria), así que se testea sin QEMU.
//!
//! **Composición**: cada ventana tiene su propio buffer, que se redibuja solo cuando su app
//! cambia. En cada frame se juntan las zonas que cambiaron (la esfera, el reloj, una ventana que
//! se movió…), se restauran desde el fondo y se vuelven a pintar las capas que las tocan, de
//! atrás hacia adelante: esfera → reloj y mensaje → panel de estado → ventanas → barra →
//! menús y avisos. Después el kernel copia solo esas zonas a la pantalla.
//!
//! **Barra de arriba**: con una ventana maximizada, la barra flotante de íconos se reemplaza por
//! una barra que ocupa todo el ancho arriba, con los íconos, las ventanas abiertas, gráficos de
//! CPU, memoria, disco y red, la IP y la hora.
//!
//! **Transiciones**: abrir, cerrar, minimizar, restaurar, maximizar y acoplar ventanas se anima
//! (si las animaciones están activadas). Cada animación depende solo de la hora del frame, así
//! que dibujar por partes sigue dando lo mismo que redibujar todo.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};
use jarvis_gfx::assistant::Assistant;
use jarvis_gfx::clock::DateTime;
use jarvis_gfx::hud;
use jarvis_gfx::sphere::ParticleCloud;
use jarvis_gfx::vfont::VectorText;
use jarvis_gfx::{Canvas, Color, MAX_CLIP, PixelFormat, Rect, theme};

use crate::apps::{
    App, Click, Ctx, Pointer, PointerKind, SysView, TaskInfo, brave::Brave, browser::Browser,
    console::Console, editor::Editor, files::FilesWindow, monitor::Monitor, music::Music, name_of,
    settings::Settings, terminal::Terminal, viewer::Viewer,
};
use crate::chrome::{self, Hover};
use crate::config::{self, AnimStyle, Config, TitleDouble, Wallpaper};
use crate::cursor;
use crate::i18n::{tr, trf};
use crate::input::{Event, Key, Mods, MousePacket};
use crate::panels::{self, Action, Menu, QuickButton, QuickHit};
use crate::shell::{
    self, LAUNCHERS, Launcher, MAX_EXTRA, StartItem, StartMenu, StatusHit, Thumb, ToolbarItem,
    TopHit, TopWindow,
};
use crate::system::{
    AppKind, History, HttpResponse, Launch, NetRequest, Outbox, Power, StreamEvent, StreamOp,
    SystemStats,
};
use crate::wm::{Part, Side, WinId, WindowManager};

pub const GREETING: &str = "Sistema en línea. ¿En qué te ayudo?";
/// Frases de demo (Espacio o Enter en el escritorio). En K4 las respuestas van a venir de Claude.
pub const DEMO_PHRASES: [&str; 4] = [
    "Todos los sistemas funcionan con normalidad.",
    "Win+E abre Archivos, Alt+Tab cambia de ventana y Win+D vuelve conmigo.",
    "Todavía no tengo voz propia, pero ya sé cómo moverme cuando hable.",
    "Cuando me conecten con Claude, voy a poder responderte de verdad.",
];
const DOUBLE_CLICK_MS: u64 = 450;
const TOAST_MS: u64 = 4500;
/// Arriba queda libre para la barra de íconos (siempre visible, como la barra de tareas).
const WORK_TOP: i32 = 76;
/// Duración de las transiciones de las ventanas.
pub const ANIM_MS: u64 = 180;

/// Zonas de la pantalla que cambiaron en un frame.
#[derive(Clone, Debug, Default)]
pub struct Dirty {
    rects: Vec<Rect>,
}

impl Dirty {
    /// Agrega una zona. Si ya hay demasiadas, se juntan todas en una (el recorte tiene un
    /// máximo de rectángulos).
    pub fn push(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        if self.rects.iter().any(|d| d.covers(&r)) {
            return;
        }
        self.rects.retain(|d| !r.covers(d));
        if self.rects.len() + 1 >= MAX_CLIP {
            let all = self.rects.iter().fold(r, |acc, d| acc.union(d));
            self.rects.clear();
            self.rects.push(all);
        } else {
            self.rects.push(r);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Rect> + '_ {
        self.rects.iter().copied()
    }

    pub fn touches(&self, r: &Rect) -> bool {
        self.rects.iter().any(|d| d.intersects(r))
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

/// Una ventana: la app y el buffer donde se dibuja.
struct Slot {
    id: WinId,
    app: App,
    buf: Vec<u8>,
    size: (i32, i32),
    content_dirty: bool,
    chrome_dirty: bool,
    hover: Hover,
}

enum Overlay {
    None,
    Start(StartMenu),
    /// Alt+Tab: ventanas en orden de uso y la elegida.
    Switcher {
        order: Vec<WinId>,
        sel: usize,
    },
    /// Win+Tab / "Control de misión".
    TaskView {
        order: Vec<WinId>,
        sel: usize,
    },
    Power {
        sel: usize,
    },
    Lock,
    /// Suspendido: pantalla negra, nada se redibuja; una tecla o el mouse despiertan.
    Sleep,
    /// Win+X y Alt+Espacio.
    Menu(Menu),
    /// Win+A: configuración rápida.
    Quick {
        sel: usize,
    },
    /// Win+N: notificaciones y calendario.
    Notices,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnimKind {
    /// Aparece, agrandándose un poco.
    Open,
    /// Se va, achicándose (con una copia de su último dibujo).
    Close,
    /// Se minimiza: se achica hacia arriba y se desvanece.
    Hide,
    /// Vuelve de minimizada.
    Show,
    /// Cambió de lugar o de tamaño de golpe (maximizar, restaurar, acoplar).
    Move,
}

/// Una transición de una ventana, de `from` a `to`, que empieza en `start`.
struct Anim {
    id: WinId,
    kind: AnimKind,
    from: Rect,
    to: Rect,
    start: u64,
    /// Cuánto dura (Configuración → Ventanas → velocidad).
    dur: u64,
    /// Solo para `Close`: el dibujo de la ventana que ya se cerró.
    buf: Vec<u8>,
    size: (i32, i32),
}

impl Anim {
    /// Avance de 0 a 1000, con desaceleración al final (ease-out cúbico).
    fn progress(&self, now_ms: u64) -> i64 {
        let dur = self.dur.max(1);
        let t = (now_ms.saturating_sub(self.start).min(dur) * 1000 / dur) as i64;
        let r = 1000 - t;
        1000 - r * r * r / 1_000_000
    }

    fn done(&self, now_ms: u64) -> bool {
        now_ms >= self.start + self.dur
    }

    /// Dónde se dibuja y con qué opacidad en `now_ms`.
    fn frame(&self, now_ms: u64) -> (Rect, u8) {
        let e = self.progress(now_ms);
        let lerp = |a: i32, b: i32| a + ((b - a) as i64 * e / 1000) as i32;
        let r = Rect::new(
            lerp(self.from.x, self.to.x),
            lerp(self.from.y, self.to.y),
            lerp(self.from.w, self.to.w).max(1),
            lerp(self.from.h, self.to.h).max(1),
        );
        let alpha = match self.kind {
            AnimKind::Open | AnimKind::Show => (e * 255 / 1000) as u8,
            AnimKind::Close | AnimKind::Hide => (255 - e * 255 / 1000) as u8,
            AnimKind::Move => 255,
        };
        (r, alpha)
    }

    /// Toda la zona que ocupa mientras dura.
    fn area(&self) -> Rect {
        self.from.union(&self.to)
    }
}

/// De dónde viene una ventana que aparece (y a dónde va una que se cierra), según el estilo.
fn appear_from(style: AnimStyle, r: Rect) -> Rect {
    match style {
        AnimStyle::Slide => Rect::new(r.x, r.y + 48, r.w, r.h),
        AnimStyle::Fade | AnimStyle::None => r,
        AnimStyle::Zoom => shrink(r, 92),
    }
}

/// `r` achicado al `pct` % alrededor de su centro.
fn shrink(r: Rect, pct: i32) -> Rect {
    let (w, h) = (r.w * pct / 100, r.h * pct / 100);
    Rect::new(r.x + (r.w - w) / 2, r.y + (r.h - h) / 2, w, h)
}

/// A dónde va una ventana al minimizarse: chiquita, arriba (hacia la barra).
fn hidden(r: Rect) -> Rect {
    let (w, h) = (r.w / 5, r.h / 5);
    Rect::new(r.x + (r.w - w) / 2, 0, w, h)
}

/// Lo que se dibujó en la barra de arriba (para saber si hay que redibujarla).
type TopKey = (
    Vec<ToolbarItem>,
    Vec<TopWindow>,
    Option<TopHit>,
    String,
    u64,
);

enum Drag {
    Move {
        id: WinId,
        dx: i32,
        dy: i32,
    },
    Resize {
        id: WinId,
        right: bool,
        bottom: bool,
        start: Rect,
        mx: i32,
        my: i32,
    },
}

/// Lo que el kernel tiene que hacer después de cada frame.
#[derive(Debug, Default)]
pub struct Requests {
    pub net: Vec<NetRequest>,
    /// Conexiones largas (ya pasaron por el firewall).
    pub streams: Vec<StreamOp>,
    /// Frecuencia del parlante (0 = silencio), si cambió.
    pub tone: Option<u32>,
    pub power: Option<Power>,
}

type ClockKey = Option<(u16, u8, u8, u8, u8)>;

pub struct Desktop<D: BlockDevice> {
    width: usize,
    height: usize,
    fs: Option<FileSystem<D>>,
    wm: WindowManager,
    slots: Vec<Slot>,
    overlay: Overlay,
    overlay_dirty: bool,
    assistant: Assistant,
    cloud: ParticleCloud,
    clock_face: hud::ClockFace,
    lock_font: VectorText,
    stats: SystemStats,
    history: History,
    stats_version: u64,
    mods: Mods,
    /// Se apretó la tecla Windows y todavía no se usó con otra: al soltarla, menú de inicio.
    win_alone: bool,
    cursor: (i32, i32),
    cursor_visible: bool,
    cursor_drawn: Option<Rect>,
    left_down: bool,
    right_down: bool,
    /// Arrastrando "con contorno": dónde está el contorno.
    outline: Option<Rect>,
    /// "Cerrar sesión": la pantalla de bloqueo pasa a ser la de iniciar sesión.
    session_closed: bool,
    /// Ventana que recibe los movimientos del mouse hasta que se suelte el botón (se apretó
    /// sobre el contenido de una app que los pide).
    pointer_capture: Option<WinId>,
    drag: Option<Drag>,
    last_click: Option<(u64, i32, i32)>,
    toolbar_hover: Option<usize>,
    top_hover: Option<TopHit>,
    toasts: Vec<(String, bool, u64)>,
    logs: Vec<String>,
    out: Outbox,
    requests: Requests,
    phrase: usize,
    screenshot: bool,
    format: Option<(PixelFormat, usize)>,
    // Estado de lo último dibujado, para saber qué cambió.
    damage: Dirty,
    full_redraw: bool,
    drawn_clock: Option<ClockKey>,
    drawn_message: Option<(usize, bool)>,
    drawn_stats: u64,
    drawn_toolbar: Option<(Vec<ToolbarItem>, Option<usize>)>,
    drawn_top: Option<TopKey>,
    /// ¿El último frame tenía la barra de arriba (una ventana maximizada)?
    drawn_top_mode: bool,
    /// Transiciones de ventanas en curso.
    anims: Vec<Anim>,
    drawn_toasts: Vec<(String, bool)>,
    config: Config,
    /// Hay que volver a dibujar el fondo (cambió el fondo de pantalla).
    bg_dirty: bool,
    /// Última vez que se tocó el teclado o el mouse (para bloquear por inactividad).
    last_input: u64,
    /// PIN que se está escribiendo en la pantalla de bloqueo (y si el último estuvo mal).
    pin_input: String,
    pin_wrong: bool,
    /// La última hora que llegó (para lo que pasa fuera de un evento, como una respuesta de red).
    last_clock: Option<DateTime>,
    last_now: u64,
    /// Historial de avisos para el centro de notificaciones: (texto, error, hora).
    notices: Vec<(String, bool, String)>,
    /// Hasta cuándo suena el pitido de un aviso de error.
    beep_until: Option<u64>,
    /// Alt+Impr Pant: captura solo de la ventana activa.
    window_screenshot: bool,
    /// Frames que la esfera se sigue dibujando después de hablar (para que vuelva a su forma).
    sphere_settle: u32,
    /// Pedidos que bloqueó el firewall: se les contesta con un error en `take_requests`.
    fw_blocked: Vec<(u32, String)>,
    /// Conexiones largas que bloqueó el firewall.
    fw_blocked_streams: Vec<(u32, String)>,
}

/// Arma un `Ctx` con campos separados de `self` (así se puede usar junto con `self.slots`).
macro_rules! ctx {
    ($s:ident, $now:expr, $clock:expr, $tasks:expr) => {
        Ctx {
            fs: $s.fs.as_mut(),
            now_ms: $now,
            clock: $clock,
            log: &mut $s.logs,
            out: &mut $s.out,
            stats: &$s.stats,
            tasks: $tasks,
            config: &$s.config,
        }
    };
}

/// A nombre de quién salen los pedidos de red de cada app (para el firewall).
fn app_tag(kind: AppKind) -> &'static str {
    match kind {
        AppKind::Browser => "navegador",
        AppKind::Terminal => "terminal",
        AppKind::Settings => "configuracion",
        AppKind::Console => "jarvis",
        AppKind::Brave => "brave",
        _ => "sistema",
    }
}

fn singleton(kind: AppKind) -> bool {
    !matches!(kind, AppKind::Editor | AppKind::Viewer)
}

impl<D: BlockDevice> Desktop<D> {
    pub fn new(
        width: usize,
        height: usize,
        particles: usize,
        mut fs: Option<FileSystem<D>>,
    ) -> Self {
        let config = fs
            .as_mut()
            .and_then(|fs| fs.read_file(config::PATH).ok())
            .map(|b| Config::parse(&String::from_utf8_lossy(&b)))
            .unwrap_or_default();
        crate::i18n::set(config.language);
        crate::look::apply(&config);
        let screen = Rect::new(0, 0, width as i32, height as i32);
        let work = Rect::new(0, WORK_TOP, width as i32, height as i32 - WORK_TOP);
        let mut wm = WindowManager::new(screen, work);
        wm.set_topbar(config.topbar);
        Desktop {
            width,
            height,
            fs,
            wm,
            slots: Vec::new(),
            overlay: Overlay::None,
            overlay_dirty: false,
            assistant: Assistant::new(),
            cloud: ParticleCloud::new(particles),
            clock_face: hud::ClockFace::new(),
            lock_font: VectorText::new(140, 5 * 64, 16),
            stats: SystemStats::default(),
            history: History::default(),
            stats_version: 0,
            mods: Mods::NONE,
            win_alone: false,
            cursor: (width as i32 / 2, height as i32 / 2),
            cursor_visible: false,
            cursor_drawn: None,
            left_down: false,
            right_down: false,
            outline: None,
            session_closed: false,
            pointer_capture: None,
            drag: None,
            last_click: None,
            toolbar_hover: None,
            top_hover: None,
            toasts: Vec::new(),
            logs: Vec::new(),
            out: Outbox::default(),
            requests: Requests::default(),
            phrase: 0,
            screenshot: false,
            format: None,
            damage: Dirty::default(),
            full_redraw: true,
            drawn_clock: None,
            drawn_message: None,
            drawn_stats: u64::MAX,
            drawn_toolbar: None,
            drawn_top: None,
            drawn_top_mode: false,
            anims: Vec::new(),
            drawn_toasts: Vec::new(),
            config,
            bg_dirty: true,
            last_input: 0,
            pin_input: String::new(),
            pin_wrong: false,
            notices: Vec::new(),
            beep_until: None,
            window_screenshot: false,
            sphere_settle: 0,
            fw_blocked: Vec::new(),
            fw_blocked_streams: Vec::new(),
            last_clock: None,
            last_now: 0,
        }
    }

    /// La capa de fondo: el HUD, un color o una imagen (según la configuración).
    pub fn draw_background(&mut self, bg: &mut Canvas<'_>) {
        self.bg_dirty = false;
        match self.config.wallpaper.clone() {
            Wallpaper::Hud => hud::draw_static(bg),
            Wallpaper::Solid(_) => {
                let color = self
                    .config
                    .wallpaper_color()
                    .unwrap_or(jarvis_gfx::theme::void());
                hud::draw_solid(bg, color);
            }
            Wallpaper::Image(path) => {
                let image = self
                    .fs
                    .as_mut()
                    .and_then(|fs| fs.read_file(&path).ok())
                    .and_then(|b| crate::bmp::decode(&b));
                match image {
                    Some(img) => {
                        let (w, h) = (bg.width() as i32, bg.height() as i32);
                        img.draw_scaled(bg, Rect::new(0, 0, w, h));
                        hud::draw_title(bg);
                    }
                    None => {
                        self.logs.push(format!("FONDO_ERROR {path}"));
                        hud::draw_static(bg);
                    }
                }
            }
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Diferencia con UTC de la zona horaria elegida (el kernel lee el reloj en UTC).
    pub fn utc_offset(&self) -> i8 {
        self.config.utc_offset
    }

    /// ¿Teclado latinoamericano? (El kernel traduce las teclas.)
    pub fn latam_keyboard(&self) -> bool {
        self.config.latam_keyboard
    }

    /// JARVIS saluda al arrancar.
    pub fn start(&mut self, now_ms: u64) {
        self.assistant.say(GREETING, now_ms);
    }

    // --- consultas (para el kernel y los tests) -----------------------------------------------

    pub fn window_manager(&self) -> &WindowManager {
        &self.wm
    }

    pub fn app(&self, kind: AppKind) -> Option<&App> {
        self.slots
            .iter()
            .find(|s| s.app.kind() == kind)
            .map(|s| &s.app)
    }

    /// La ventana de la app `kind` (la primera, si hay varias).
    pub fn window_of(&self, kind: AppKind) -> Option<WinId> {
        self.slots
            .iter()
            .find(|s| s.app.kind() == kind)
            .map(|s| s.id)
    }

    /// El próximo frame se dibuja entero, incluidas las ventanas.
    pub fn invalidate(&mut self) {
        self.full_redraw = true;
        for s in &mut self.slots {
            s.content_dirty = true;
            s.chrome_dirty = true;
        }
    }

    pub fn focused_app(&self) -> Option<AppKind> {
        let id = self.wm.focused()?;
        self.slots.iter().find(|s| s.id == id).map(|s| s.app.kind())
    }

    pub fn is_locked(&self) -> bool {
        matches!(self.overlay, Overlay::Lock)
    }

    /// Suspendido: el kernel puede dormir la CPU sin dibujar.
    pub fn is_sleeping(&self) -> bool {
        matches!(self.overlay, Overlay::Sleep)
    }

    /// Se cerró la sesión (la pantalla de bloqueo es la de "iniciar sesión").
    pub fn session_closed(&self) -> bool {
        self.session_closed
    }

    /// Nombre del menú o panel abierto encima de todo ("" si no hay).
    pub fn overlay_name(&self) -> &'static str {
        match &self.overlay {
            Overlay::None => "",
            Overlay::Start(_) => "inicio",
            Overlay::Switcher { .. } => "alt-tab",
            Overlay::TaskView { .. } => "tareas",
            Overlay::Power { .. } => "apagado",
            Overlay::Lock if self.session_closed => "sesion",
            Overlay::Lock => "bloqueo",
            Overlay::Sleep => "suspendido",
            Overlay::Menu(m) if m.title.starts_with("VENTANA") => "ventana",
            Overlay::Menu(_) => "enlaces",
            Overlay::Quick { .. } => "rapida",
            Overlay::Notices => "notificaciones",
        }
    }

    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    pub fn into_fs(self) -> Option<FileSystem<D>> {
        self.fs
    }

    pub fn fs_mut(&mut self) -> Option<&mut FileSystem<D>> {
        self.fs.as_mut()
    }

    /// Líneas para el puerto serie (el kernel las imprime; los tests las verifican).
    pub fn take_logs(&mut self) -> Vec<String> {
        core::mem::take(&mut self.logs)
    }

    /// Pedidos para el kernel (red, parlante, apagado).
    pub fn take_requests(&mut self) -> Requests {
        // Lo que bloqueó el firewall no llega al kernel: la app recibe el error acá.
        for (id, msg) in core::mem::take(&mut self.fw_blocked) {
            self.net_response(id, Err(msg));
        }
        for (id, msg) in core::mem::take(&mut self.fw_blocked_streams) {
            self.stream_event(id, StreamEvent::Closed(Some(msg)));
        }
        core::mem::take(&mut self.requests)
    }

    /// ¿Deja salir este pedido el firewall?
    fn firewall_check(&self, req: &NetRequest) -> Result<(), String> {
        let Some(u) = crate::web::url::Url::parse(&req.url) else {
            return Ok(());
        };
        if !matches!(
            u.scheme,
            crate::web::url::Scheme::Http | crate::web::url::Scheme::Https
        ) {
            return Ok(());
        }
        self.config.firewall.check_out(&u.host, u.port, &req.app)
    }

    fn firewall_block(
        &mut self,
        req: NetRequest,
        why: String,
        now_ms: u64,
        clock: Option<DateTime>,
    ) {
        let host = crate::web::url::Url::parse(&req.url)
            .map(|u| u.host)
            .unwrap_or_default();
        let msg = self.firewall_record(
            &req.app,
            &host,
            &req.url,
            &why,
            req.kind != crate::system::FetchKind::Image,
            now_ms,
            clock,
        );
        self.fw_blocked.push((req.id, msg));
    }

    /// Anota un bloqueo del firewall (log del puerto serie, `/Sistema/firewall.log` y aviso) y
    /// devuelve el error para la app.
    #[allow(clippy::too_many_arguments)]
    fn firewall_record(
        &mut self,
        app: &str,
        host: &str,
        target: &str,
        why: &str,
        notify: bool,
        now_ms: u64,
        clock: Option<DateTime>,
    ) -> String {
        self.logs
            .push(format!("FIREWALL_BLOQUEO {app} {host} ({why})"));
        if self.config.firewall.log
            && let Some(fs) = self.fs.as_mut()
        {
            let when = clock.map_or(String::new(), |t| {
                format!(
                    "{}-{:02}-{:02} {:02}:{:02}:{:02} ",
                    t.year, t.month, t.day, t.hour, t.minute, t.second
                )
            });
            let mut log = fs
                .read_file(crate::firewall::LOG_PATH)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            log.push_str(&format!("{when}BLOQUEADO {app} -> {target} ({why})\n"));
            // Solo las últimas 200 líneas.
            let lines: Vec<&str> = log.lines().collect();
            let keep = lines[lines.len().saturating_sub(200)..].join("\n") + "\n";
            let stamp = crate::apps::timestamp(clock);
            let _ = crate::term::apt::ensure_dirs(fs, "/Sistema", stamp)
                .and_then(|()| fs.write_file(crate::firewall::LOG_PATH, keep.as_bytes(), stamp));
        }
        if notify {
            self.notify(trf("Firewall: bloqueó {} ({})", &[host, app]), true, now_ms);
        }
        format!("bloqueado por el firewall: {why}")
    }

    /// Una conexión larga cambió (el kernel la atiende): se le avisa a quien la abrió.
    pub fn stream_event(&mut self, id: u32, event: StreamEvent) {
        match &event {
            StreamEvent::Connected => self.logs.push(format!("CONEXION_ABIERTA {id}")),
            StreamEvent::Closed(None) => self.logs.push(format!("CONEXION_CERRADA {id}")),
            StreamEvent::Closed(Some(e)) => self.logs.push(format!("CONEXION_ERROR {id} {e}")),
            StreamEvent::Data(_) => {}
        }
        let tasks = self.tasks();
        let now_ms = self.last_now;
        {
            let mut ctx = ctx!(self, now_ms, self.last_clock, &tasks);
            for s in &mut self.slots {
                ctx.out.app = app_tag(s.app.kind());
                s.app.stream_event(id, &event, &mut ctx);
                if s.app.take_dirty() {
                    s.content_dirty = true;
                }
            }
        }
        let clock = self.last_clock;
        let before = self.geometry();
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before, now_ms);
    }

    /// El kernel mide la máquina una vez por segundo.
    pub fn set_stats(&mut self, stats: SystemStats) {
        self.history.push(&stats);
        self.stats = stats;
        self.stats_version += 1;
        for s in &mut self.slots {
            if s.app.kind() == AppKind::Monitor {
                s.content_dirty = true;
            }
        }
    }

    pub fn net_response(&mut self, id: u32, result: Result<HttpResponse, String>) {
        match &result {
            Ok(r) => self.logs.push(format!(
                "RED_RESPUESTA {} {} ({} bytes)",
                r.status,
                r.url,
                r.body.len()
            )),
            Err(e) => self.logs.push(format!("RED_ERROR {e}")),
        }
        let tasks = self.tasks();
        let now_ms = self.last_now;
        {
            let mut ctx = ctx!(self, now_ms, self.last_clock, &tasks);
            for s in &mut self.slots {
                ctx.out.app = app_tag(s.app.kind());
                s.app.net_response(id, &result, &mut ctx);
                if s.app.take_dirty() {
                    s.content_dirty = true;
                }
            }
        }
        let clock = self.last_clock;
        let before = self.geometry();
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before, now_ms);
    }

    fn tasks(&self) -> Vec<TaskInfo> {
        self.wm
            .mru()
            .iter()
            .filter_map(|&id| {
                let slot = self.slots.iter().find(|s| s.id == id)?;
                let win = self.wm.get(id)?;
                Some(TaskInfo {
                    id,
                    title: slot.app.title(),
                    kind: slot.app.kind(),
                    minimized: win.minimized,
                })
            })
            .collect()
    }

    fn slot_index(&self, id: WinId) -> Option<usize> {
        self.slots.iter().position(|s| s.id == id)
    }

    // --- eventos ------------------------------------------------------------------------------

    pub fn handle(&mut self, event: Event, now_ms: u64, clock: Option<DateTime>) {
        self.last_input = now_ms;
        self.last_now = now_ms;
        self.last_clock = clock;
        let before = self.geometry();
        match event {
            Event::Key(key) => self.on_key(key, now_ms, clock),
            Event::Mods(m) => self.on_mods(m, now_ms, clock),
            Event::Mouse(p) => self.on_mouse(p, now_ms, clock),
        }
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before, now_ms);
    }

    /// (id, zona, visible, tiene el foco, maximizada) de cada ventana, de atrás hacia adelante.
    fn geometry(&self) -> Vec<(WinId, Rect, bool, bool, bool)> {
        self.wm
            .windows()
            .iter()
            .map(|w| {
                (
                    w.id,
                    w.rect,
                    w.visible(),
                    self.wm.focused() == Some(w.id),
                    w.maximized,
                )
            })
            .collect()
    }

    /// Compara la geometría de antes con la de ahora y marca como sucio lo que cambió.
    fn damage_geometry(&mut self, before: &[(WinId, Rect, bool, bool, bool)], now_ms: u64) {
        let after = self.geometry();
        for (i, now) in after.iter().enumerate() {
            let old = before.iter().position(|b| b.0 == now.0);
            self.start_anim(old.map(|j| (before[j].1, before[j].2)), now, now_ms);
            let changed = match old {
                Some(j) => before[j] != *now || j != i,
                None => true,
            };
            if !changed {
                continue;
            }
            if let Some(j) = old
                && before[j].2
            {
                self.damage.push(before[j].1);
            }
            if now.2 {
                self.damage.push(now.1);
            }
            if let Some(slot) = self.slots.iter_mut().find(|s| s.id == now.0) {
                slot.chrome_dirty = true;
            }
        }
        for b in before {
            if b.2 && !after.iter().any(|a| a.0 == b.0) {
                self.damage.push(b.1); // se cerró
            }
        }
    }

    /// Empieza la transición de una ventana que cambió: `old` es (zona, visible) de antes.
    fn start_anim(
        &mut self,
        old: Option<(Rect, bool)>,
        now: &(WinId, Rect, bool, bool, bool),
        now_ms: u64,
    ) {
        let dur = self.config.anim_ms();
        if dur == 0 {
            return;
        }
        let style = self.config.anim_style;
        let (id, rect, visible) = (now.0, now.1, now.2);
        let current = self
            .anims
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.frame(now_ms).0);
        let (kind, from, to) = match old {
            None if visible => (AnimKind::Open, appear_from(style, rect), rect),
            Some((_, true)) if !visible => (AnimKind::Hide, current.unwrap_or(rect), hidden(rect)),
            Some((_, false)) if visible => (AnimKind::Show, hidden(rect), rect),
            Some((before, true)) if visible && before != rect && self.drag.is_none() => {
                (AnimKind::Move, current.unwrap_or(before), rect)
            }
            Some((before, true)) if visible && before != rect => {
                // Se arrastra con el mouse: la transición que tenía se corta, así la ventana
                // sigue al mouse.
                if let Some(a) = self.anims.iter().find(|a| a.id == id) {
                    self.damage.push(a.area());
                }
                self.anims.retain(|a| a.id != id);
                return;
            }
            _ => return,
        };
        if let Some(a) = self.anims.iter().find(|a| a.id == id) {
            self.damage.push(a.area());
        }
        self.anims.retain(|a| a.id != id);
        self.anims.push(Anim {
            id,
            kind,
            from,
            to,
            start: now_ms,
            dur,
            buf: Vec::new(),
            size: (0, 0),
        });
    }

    fn on_mods(&mut self, m: Mods, now_ms: u64, clock: Option<DateTime>) {
        let prev = self.mods;
        self.mods = m;
        if m.win && !prev.win {
            self.win_alone = true;
        }
        if !m.win && prev.win && self.win_alone {
            // La tecla Windows sola: abre o cierra el menú de inicio.
            self.win_alone = false;
            self.toggle_start();
        }
        if !m.alt && prev.alt && matches!(self.overlay, Overlay::Switcher { .. }) {
            self.commit_switcher(now_ms, clock);
        }
    }

    fn set_overlay(&mut self, o: Overlay) {
        let was_full = self.overlay_is_fullscreen();
        self.overlay = o;
        self.overlay_dirty = true;
        if was_full || self.overlay_is_fullscreen() {
            self.full_redraw = true;
        }
        let name = self.overlay_name();
        if !name.is_empty() {
            self.logs.push(format!("ESCRITORIO_MENU {name}"));
        }
    }

    fn overlay_is_fullscreen(&self) -> bool {
        // (Menús y paneles chicos no: se dibujan encima de lo que hay.)
        matches!(
            self.overlay,
            Overlay::TaskView { .. } | Overlay::Power { .. } | Overlay::Lock | Overlay::Sleep
        )
    }

    fn toggle_start(&mut self) {
        if matches!(self.overlay, Overlay::Start(_)) {
            self.set_overlay(Overlay::None);
        } else {
            self.set_overlay(Overlay::Start(StartMenu::new()));
        }
    }

    fn open_switcher(&mut self, backwards: bool) {
        let order: Vec<WinId> = self.wm.mru_here();
        if order.is_empty() {
            return;
        }
        match &mut self.overlay {
            Overlay::Switcher { order, sel } => {
                let n = order.len();
                *sel = if backwards {
                    (*sel + n - 1) % n
                } else {
                    (*sel + 1) % n
                };
                self.overlay_dirty = true;
            }
            _ => {
                let n = order.len();
                // Alt+Tab arranca en la segunda ventana (la anterior), como en Windows.
                let sel = if backwards { n - 1 } else { 1 % n };
                self.set_overlay(Overlay::Switcher { order, sel });
            }
        }
    }

    fn commit_switcher(&mut self, _now_ms: u64, _clock: Option<DateTime>) {
        if let Overlay::Switcher { order, sel } = &self.overlay
            && let Some(&id) = order.get(*sel)
        {
            self.wm.activate(id);
            self.log_focus(id);
        }
        self.set_overlay(Overlay::None);
    }

    fn log_focus(&mut self, id: WinId) {
        if let Some(i) = self.slot_index(id) {
            let title = self.slots[i].app.title();
            self.logs.push(format!("VENTANA_FOCO {title}"));
        }
    }

    fn open_task_view(&mut self) {
        let mut order: Vec<WinId> = self.wm.mru_here();
        order.sort_unstable();
        self.set_overlay(Overlay::TaskView { order, sel: 0 });
    }

    fn focused_slot(&self) -> Option<usize> {
        self.wm.focused().and_then(|id| self.slot_index(id))
    }

    fn content_of(&self, id: WinId) -> Rect {
        self.wm
            .get(id)
            .map_or(Rect::new(0, 0, 0, 0), |w| w.content())
    }

    fn on_key(&mut self, key: Key, now_ms: u64, clock: Option<DateTime>) {
        if self.is_sleeping() {
            self.wake();
            return;
        }
        if matches!(self.overlay, Overlay::Lock) {
            self.lock_key(key);
            return;
        }
        let m = self.mods;
        if m.win {
            self.win_alone = false;
        }
        if self.global_shortcut(key, m, now_ms, clock) {
            return;
        }
        if self.overlay_key(key, now_ms, clock) {
            return;
        }
        if let Some(i) = self.focused_slot() {
            let id = self.slots[i].id;
            let content = self.content_of(id);
            let tasks = self.tasks();
            let mut ctx = ctx!(self, now_ms, clock, &tasks);
            let slot = &mut self.slots[i];
            ctx.out.app = app_tag(slot.app.kind());
            let used = slot.app.key(key, m, content, &mut ctx);
            if slot.app.take_dirty() {
                slot.content_dirty = true;
            }
            // Ctrl+W cierra la ventana si la app no lo usó (como una pestaña).
            if !used && m.ctrl && matches!(key, Key::Char('w' | 'W')) {
                self.close_window(id, now_ms, clock);
            }
            return;
        }
        // El escritorio (JARVIS) tiene el foco.
        match key {
            Key::Char(' ') | Key::Enter => self.speak_demo(now_ms),
            Key::Tab => self.launch(Launch::App(AppKind::Files), now_ms, clock),
            _ => {}
        }
    }

    /// En la pantalla de bloqueo: sin PIN, cualquier tecla desbloquea; con PIN, hay que
    /// escribirlo y apretar Enter.
    fn lock_key(&mut self, key: Key) {
        if self.config.pin.is_empty() {
            self.unlock();
            return;
        }
        match key {
            Key::Char(c) if c.is_ascii_digit() && self.pin_input.len() < 8 => {
                self.pin_input.push(c);
                self.pin_wrong = false;
            }
            Key::Backspace => {
                self.pin_input.pop();
            }
            Key::Escape => self.pin_input.clear(),
            Key::Enter => {
                if self.pin_input == self.config.pin {
                    self.unlock();
                    return;
                }
                self.pin_input.clear();
                self.pin_wrong = true;
                self.logs.push("ESCRITORIO_PIN_INCORRECTO".into());
            }
            _ => {}
        }
        self.overlay_dirty = true;
    }

    fn unlock(&mut self) {
        self.pin_input.clear();
        self.pin_wrong = false;
        if core::mem::take(&mut self.session_closed) {
            let user = self.config.user.clone();
            self.logs.push(format!("SESION_INICIADA {user}"));
        } else {
            self.logs.push("ESCRITORIO_DESBLOQUEADO".into());
        }
        self.set_overlay(Overlay::None);
    }

    /// Cerrar sesión: se cierran todas las apps (el Editor puede frenarlo si hay cambios sin
    /// guardar), se guarda la configuración y queda la pantalla de inicio de sesión.
    fn logout(&mut self, now_ms: u64, clock: Option<DateTime>) {
        let ids: Vec<WinId> = self.slots.iter().map(|s| s.id).collect();
        for id in ids {
            self.close_window(id, now_ms, clock);
        }
        if !self.slots.is_empty() {
            // Alguna app no se dejó cerrar (el Editor pregunta si guardar).
            self.notify(
                tr("No se cerró la sesión: hay cambios sin guardar."),
                true,
                now_ms,
            );
            self.logs.push("SESION_NO_CERRADA".into());
            return;
        }
        self.anims.clear();
        // (La configuración ya está guardada: se escribe en cada cambio.)
        self.requests.tone = Some(0);
        self.session_closed = true;
        self.pin_input.clear();
        self.pin_wrong = false;
        self.logs
            .push(format!("SESION_CERRADA {}", self.config.user));
        self.set_overlay(Overlay::Lock);
    }

    /// Suspender: la pantalla queda negra y no se dibuja nada hasta que llegue una tecla o el
    /// mouse. (El S3 de ACPI, que apaga casi toda la máquina, llega con el hardware real.)
    fn suspend(&mut self) {
        self.requests.tone = Some(0);
        self.logs.push("SUSPENDIDO".into());
        self.set_overlay(Overlay::Sleep);
    }

    fn wake(&mut self) {
        self.logs.push("DESPIERTO".into());
        // Con PIN (o si la sesión estaba cerrada) hay que volver a entrar.
        if self.session_closed || !self.config.pin.is_empty() {
            self.pin_input.clear();
            self.pin_wrong = false;
            self.set_overlay(Overlay::Lock);
        } else {
            self.set_overlay(Overlay::None);
        }
    }

    fn lock(&mut self) {
        self.pin_input.clear();
        self.pin_wrong = false;
        self.logs.push("ESCRITORIO_BLOQUEADO".into());
        self.set_overlay(Overlay::Lock);
    }

    /// Atajos de Windows (y algunos de Ubuntu). Devuelve `true` si la tecla era uno.
    fn global_shortcut(&mut self, key: Key, m: Mods, now_ms: u64, clock: Option<DateTime>) -> bool {
        let focused = self.wm.focused();
        let app = Launch::App;
        if m.win && m.ctrl {
            match key {
                // Escritorios virtuales.
                Key::Char('d' | 'D') => {
                    self.wm.new_desktop();
                    self.desktop_changed(now_ms);
                }
                Key::Left | Key::Right => {
                    let (cur, n) = self.wm.desktops();
                    let next = if key == Key::Left {
                        cur.checked_sub(1)
                    } else {
                        (cur + 1 < n).then_some(cur + 1)
                    };
                    if let Some(next) = next {
                        self.wm.switch_desktop(next);
                        self.desktop_changed(now_ms);
                    }
                }
                Key::F(4) => {
                    self.wm.close_desktop();
                    self.desktop_changed(now_ms);
                }
                // Win+Ctrl+Shift+B en Windows reinicia el driver de video: acá, redibuja todo.
                Key::Char('b' | 'B') if m.shift => {
                    self.bg_dirty = true;
                    self.invalidate();
                    self.logs.push("ESCRITORIO_REDIBUJADO".into());
                }
                _ => return false,
            }
            return true;
        }
        if m.win {
            match key {
                Key::Char('d' | 'D') if m.alt => self.toggle_overlay(Overlay::Notices),
                Key::Char('d' | 'D' | ',') => {
                    self.wm.toggle_desktop();
                    self.logs.push("ESCRITORIO_MOSTRAR".into());
                }
                Key::Char('m' | 'M') if m.shift => self.wm.restore_all(),
                Key::Char('m' | 'M') => self.wm.minimize_all(),
                Key::Home => self.wm.minimize_others(),
                Key::Char('e' | 'E') => self.launch(app(AppKind::Files), now_ms, clock),
                Key::Char('r' | 'R') => self.launch(app(AppKind::Console), now_ms, clock),
                Key::Char('i' | 'I') => self.launch(app(AppKind::Settings), now_ms, clock),
                Key::Enter => self.launch(app(AppKind::Terminal), now_ms, clock),
                Key::Char('x' | 'X') => self.toggle_overlay(Overlay::Menu(panels::quick_links())),
                Key::Char('a' | 'A') => self.toggle_overlay(Overlay::Quick { sel: 0 }),
                Key::Char('n' | 'N') => self.toggle_overlay(Overlay::Notices),
                Key::Char('l' | 'L') => self.lock(),
                Key::Char('s' | 'S') if m.shift => self.screenshot = true,
                Key::Char('s' | 'S' | 'q' | 'Q') => self.toggle_start(),
                Key::Tab => self.open_task_view(),
                Key::Up if m.shift => {
                    if let Some(id) = focused {
                        self.wm.stretch_vertical(id);
                    }
                }
                Key::Up => {
                    if let Some(id) = focused
                        && !self.wm.get(id).is_some_and(|w| w.maximized)
                    {
                        self.wm.toggle_maximize(id);
                    }
                }
                Key::Down => {
                    if let Some(id) = focused {
                        self.wm.restore_or_minimize(id);
                    }
                }
                Key::Left | Key::Right => {
                    if let Some(id) = focused {
                        let side = if key == Key::Left {
                            Side::Left
                        } else {
                            Side::Right
                        };
                        self.wm.snap(id, side);
                    }
                }
                Key::Char(c @ '1'..='9') => {
                    let i = c as usize - '1' as usize;
                    self.toolbar_action(i, now_ms, clock);
                }
                Key::Char('0') => self.toolbar_action(9, now_ms, clock),
                Key::PrintScreen => self.screenshot = true,
                _ => return false,
            }
            return true;
        }
        if m.ctrl && m.alt {
            match key {
                // Ubuntu: Ctrl+Alt+T abre una terminal.
                Key::Char('t' | 'T') => self.launch(app(AppKind::Terminal), now_ms, clock),
                Key::Delete => self.launch(app(AppKind::Monitor), now_ms, clock),
                _ => return false,
            }
            return true;
        }
        if m.alt {
            match key {
                Key::Tab => self.open_switcher(m.shift),
                Key::F(4) => match focused {
                    Some(id) => self.close_window(id, now_ms, clock),
                    None => self.set_overlay(Overlay::Power { sel: 0 }),
                },
                Key::Char(' ') => {
                    if let Some(id) = focused
                        && let Some(w) = self.wm.get(id)
                    {
                        let at = (w.rect.x + 8, w.rect.y + 30);
                        let menu = panels::window_menu(id, at, w.maximized);
                        self.set_overlay(Overlay::Menu(menu));
                    }
                }
                // Alt+Esc: la ventana siguiente, sin selector.
                Key::Escape => {
                    let order = self.wm.mru_here();
                    if let Some(&next) = order.last()
                        && order.len() > 1
                    {
                        self.wm.activate(next);
                        self.log_focus(next);
                    }
                }
                Key::PrintScreen => self.window_screenshot = true,
                _ => return false,
            }
            return true;
        }
        match key {
            Key::Escape if m.ctrl && m.shift => self.launch(app(AppKind::Monitor), now_ms, clock),
            Key::Escape if m.ctrl => self.toggle_start(),
            Key::PrintScreen => self.screenshot = true,
            Key::F(1) => self.launch(Launch::Browse("about:ayuda".into()), now_ms, clock),
            Key::F(11) => match focused {
                Some(id) => self.wm.toggle_maximize(id),
                None => return false,
            },
            _ => return false,
        }
        true
    }

    /// Abre el panel, o lo cierra si ya estaba abierto (como Win+A dos veces).
    fn toggle_overlay(&mut self, o: Overlay) {
        let same = core::mem::discriminant(&o) == core::mem::discriminant(&self.overlay)
            && match (&o, &self.overlay) {
                (Overlay::Menu(a), Overlay::Menu(b)) => a.title == b.title,
                _ => true,
            };
        self.set_overlay(if same { Overlay::None } else { o });
    }

    fn desktop_changed(&mut self, now_ms: u64) {
        let (cur, n) = self.wm.desktops();
        self.logs
            .push(format!("ESCRITORIO_VIRTUAL {} de {n}", cur + 1));
        self.notify(
            trf(
                "Escritorio {} de {}",
                &[&(cur + 1).to_string(), &n.to_string()],
            ),
            false,
            now_ms,
        );
    }

    /// Hace lo que se eligió en un menú (Win+X, Alt+Espacio).
    fn menu_action(&mut self, a: Action, now_ms: u64, clock: Option<DateTime>) {
        self.set_overlay(Overlay::None);
        match a {
            Action::Launch(l) => self.launch(l, now_ms, clock),
            Action::PowerMenu => self.set_overlay(Overlay::Power { sel: 0 }),
            Action::ShowDesktop => self.wm.toggle_desktop(),
            Action::Lock => self.lock(),
            Action::Logout => self.logout(now_ms, clock),
            Action::Sleep => self.suspend(),
            Action::Search => self.toggle_start(),
            Action::NewDesktop => {
                self.wm.new_desktop();
                self.desktop_changed(now_ms);
            }
            Action::Restore(id) => self.wm.restore_or_minimize(id),
            Action::Minimize(id) => self.wm.minimize(id),
            Action::Maximize(id) => self.wm.toggle_maximize(id),
            Action::Snap(id, side) => self.wm.snap(id, side),
            Action::Close(id) => self.close_window(id, now_ms, clock),
        }
    }

    /// Un cambio de la configuración rápida (Win+A).
    fn quick_hit(&mut self, hit: QuickHit, now_ms: u64, clock: Option<DateTime>) {
        match hit {
            QuickHit::Toggle(q) => {
                let mut cfg = self.config.clone();
                panels::quick_toggle(&mut cfg, q);
                self.apply_config(cfg, now_ms);
                self.overlay_dirty = true;
            }
            QuickHit::Button(b) => {
                self.set_overlay(Overlay::None);
                match b {
                    QuickButton::Settings => {
                        self.launch(Launch::App(AppKind::Settings), now_ms, clock)
                    }
                    QuickButton::Lock => self.lock(),
                    QuickButton::Power => self.set_overlay(Overlay::Power { sel: 0 }),
                }
            }
        }
    }

    /// Aplica y guarda una configuración nueva (lo mismo que hace la app Configuración; lo
    /// usan los tests).
    pub fn set_config(&mut self, cfg: Config) {
        let now_ms = self.last_now;
        self.apply_config(cfg, now_ms);
    }

    /// Aplica y guarda una configuración nueva.
    fn apply_config(&mut self, cfg: Config, now_ms: u64) {
        if cfg == self.config {
            return;
        }
        let old = core::mem::replace(&mut self.config, cfg);
        if old.wallpaper != self.config.wallpaper {
            self.bg_dirty = true;
        }
        if old.language != self.config.language {
            // Otro idioma: se redibuja todo (barra, ventanas, HUD).
            crate::i18n::set(self.config.language);
            self.bg_dirty = true;
            for s in &mut self.slots {
                s.chrome_dirty = true;
            }
        }
        if crate::look::palette(&old) != crate::look::palette(&self.config) {
            // Otro tema o acento: cambia el fondo (el HUD) y todo lo demás.
            self.bg_dirty = true;
        }
        crate::look::apply(&self.config);
        self.wm.set_topbar(self.config.topbar);
        let look_changed = old.theme != self.config.theme
            || old.accent != self.config.accent
            || old.ui_large != self.config.ui_large
            || old.bold_titles != self.config.bold_titles
            || old.buttons_left != self.config.buttons_left
            || old.cursor_big != self.config.cursor_big;
        if look_changed {
            for s in &mut self.slots {
                s.chrome_dirty = true;
            }
        }
        if old.clock_24h != self.config.clock_24h
            || old.status_panel != self.config.status_panel
            || old.animations != self.config.animations
            || old.language != self.config.language
            || old.topbar != self.config.topbar
            || old.top_stats != self.config.top_stats
            || old.clock_seconds != self.config.clock_seconds
            || look_changed
        {
            self.full_redraw = true;
        }
        // Las apps que dependen de la configuración la leen de nuevo.
        for s in &mut self.slots {
            match &mut s.app {
                App::Browser(b) => b.set_config(&self.config),
                App::Brave(b) => b.set_config(&self.config),
                App::Settings(st) => st.sync(&self.config),
                _ => {}
            }
            s.app.set_dirty();
            s.content_dirty = true;
        }
        let ts = crate::apps::timestamp(self.last_clock);
        let text = self.config.serialize();
        let saved = self.fs.as_mut().map(|fs| {
            crate::term::apt::ensure_dirs(fs, "/Sistema", ts)
                .and_then(|()| fs.write_file(config::PATH, text.as_bytes(), ts))
        });
        match saved {
            Some(Ok(())) => self.logs.push("CONFIG_GUARDADA".into()),
            Some(Err(e)) => self.notify(
                trf("No se pudo guardar la configuración: {}", &[&e.to_string()]),
                true,
                now_ms,
            ),
            None => {}
        }
    }

    /// Teclas para el menú o panel abierto. `true` si se usó.
    fn overlay_key(&mut self, key: Key, now_ms: u64, clock: Option<DateTime>) -> bool {
        match &mut self.overlay {
            Overlay::None | Overlay::Lock | Overlay::Sleep => false,
            Overlay::Menu(menu) => {
                let n = menu.items.len();
                match key {
                    Key::Escape => self.set_overlay(Overlay::None),
                    Key::Up => {
                        menu.sel = (menu.sel + n - 1) % n.max(1);
                        self.overlay_dirty = true;
                    }
                    Key::Down | Key::Tab => {
                        menu.sel = (menu.sel + 1) % n.max(1);
                        self.overlay_dirty = true;
                    }
                    Key::Enter => {
                        if let Some(a) = menu.items.get(menu.sel).map(|i| i.action.clone()) {
                            self.menu_action(a, now_ms, clock);
                        }
                    }
                    _ => {}
                }
                true
            }
            Overlay::Quick { sel } => {
                let n = panels::QUICK.len() + 3;
                match key {
                    Key::Escape => self.set_overlay(Overlay::None),
                    Key::Right | Key::Tab => *sel = (*sel + 1) % n,
                    Key::Left => *sel = (*sel + n - 1) % n,
                    Key::Down => *sel = (*sel + 3).min(n - 1),
                    Key::Up => *sel = sel.saturating_sub(3),
                    Key::Enter | Key::Char(' ') => {
                        let i = *sel;
                        let hit = match panels::QUICK.get(i) {
                            Some((q, _)) => QuickHit::Toggle(*q),
                            None => QuickHit::Button(
                                [QuickButton::Settings, QuickButton::Lock, QuickButton::Power]
                                    [(i - panels::QUICK.len()).min(2)],
                            ),
                        };
                        self.quick_hit(hit, now_ms, clock);
                    }
                    _ => {}
                }
                self.overlay_dirty = true;
                true
            }
            Overlay::Notices => {
                match key {
                    Key::Escape => self.set_overlay(Overlay::None),
                    Key::Delete => {
                        self.notices.clear();
                        self.overlay_dirty = true;
                    }
                    _ => {}
                }
                true
            }
            Overlay::Start(menu) => {
                let n = menu.items().len();
                match key {
                    Key::Escape => {
                        self.set_overlay(Overlay::None);
                        return true;
                    }
                    Key::Up => menu.selected = menu.selected.saturating_sub(1),
                    Key::Down => menu.selected = (menu.selected + 1).min(n.saturating_sub(1)),
                    Key::Enter => {
                        let item = menu.items().get(menu.selected).map(|i| i.0.clone());
                        self.set_overlay(Overlay::None);
                        if let Some(item) = item {
                            self.start_item(item, now_ms, clock);
                        }
                        return true;
                    }
                    other => {
                        if menu.query.handle(other) {
                            menu.selected = 0;
                        }
                    }
                }
                self.overlay_dirty = true;
                true
            }
            Overlay::Switcher { order, sel } => {
                let n = order.len();
                match key {
                    Key::Escape => {
                        self.set_overlay(Overlay::None);
                        return true;
                    }
                    Key::Enter => {
                        self.commit_switcher(now_ms, clock);
                        return true;
                    }
                    Key::Right => *sel = (*sel + 1) % n,
                    Key::Left => *sel = (*sel + n - 1) % n,
                    _ => {}
                }
                self.overlay_dirty = true;
                true
            }
            Overlay::TaskView { order, sel } => {
                let n = order.len();
                match key {
                    Key::Escape => self.set_overlay(Overlay::None),
                    Key::Right | Key::Tab | Key::Down if n > 0 => {
                        *sel = (*sel + 1) % n;
                        self.overlay_dirty = true;
                    }
                    Key::Left | Key::Up if n > 0 => {
                        *sel = (*sel + n - 1) % n;
                        self.overlay_dirty = true;
                    }
                    Key::Enter => {
                        let id = order.get(*sel).copied();
                        self.set_overlay(Overlay::None);
                        if let Some(id) = id {
                            self.wm.activate(id);
                            self.log_focus(id);
                        }
                    }
                    Key::Delete => {
                        if let Some(&id) = order.get(*sel) {
                            self.close_window(id, now_ms, clock);
                            self.open_task_view();
                        }
                    }
                    _ => {}
                }
                true
            }
            Overlay::Power { sel } => {
                match key {
                    Key::Escape => self.set_overlay(Overlay::None),
                    Key::Left => {
                        *sel = sel.saturating_sub(1);
                        self.overlay_dirty = true;
                    }
                    Key::Right | Key::Tab => {
                        *sel = (*sel + 1) % shell::POWER_CHOICES;
                        self.overlay_dirty = true;
                    }
                    Key::Enter => {
                        let choice = *sel;
                        self.power_choice(choice);
                    }
                    _ => {}
                }
                true
            }
        }
    }

    fn power_choice(&mut self, choice: usize) {
        self.set_overlay(Overlay::None);
        let (now_ms, clock) = (self.last_now, self.last_clock);
        match choice {
            0 => self.power(Power::Shutdown),
            1 => self.power(Power::Reboot),
            2 => self.suspend(),
            3 => self.logout(now_ms, clock),
            _ => {}
        }
    }

    fn power(&mut self, p: Power) {
        // Antes de apagar, se cierran las apps (la música deja de sonar, etc.).
        self.logs.push(format!("ESCRITORIO_ENERGIA {p:?}"));
        self.requests.tone = Some(0);
        self.requests.power = Some(p);
    }

    fn start_item(&mut self, item: StartItem, now_ms: u64, clock: Option<DateTime>) {
        match item {
            StartItem::App(k) => self.launch(Launch::App(k), now_ms, clock),
            StartItem::Web(text) => self.launch(Launch::Browse(text), now_ms, clock),
            StartItem::Lock => self.set_overlay(Overlay::Lock),
            StartItem::Logout => self.logout(now_ms, clock),
            StartItem::Sleep => self.suspend(),
            StartItem::Restart => self.power(Power::Reboot),
            StartItem::Shutdown => self.power(Power::Shutdown),
        }
    }

    fn speak_demo(&mut self, now_ms: u64) {
        let text = DEMO_PHRASES[self.phrase % DEMO_PHRASES.len()];
        self.phrase += 1;
        self.say(text, now_ms);
    }

    fn say(&mut self, text: &str, now_ms: u64) {
        self.assistant.say(text, now_ms);
        self.logs.push(format!("JARVIS_HABLA: {text}"));
    }

    // --- ventanas -----------------------------------------------------------------------------

    /// Abre una app (o trae adelante la que ya estaba abierta), y atiende lo que pida al abrir
    /// (por ejemplo, el navegador pide su primera página).
    pub fn open(&mut self, what: Launch, now_ms: u64, clock: Option<DateTime>) {
        let before = self.geometry();
        self.launch(what, now_ms, clock);
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before, now_ms);
    }

    fn launch(&mut self, what: Launch, now_ms: u64, clock: Option<DateTime>) {
        let tasks = self.tasks();
        let kind = match &what {
            Launch::App(k) => *k,
            Launch::Folder(_) => AppKind::Files,
            Launch::Edit(_) => AppKind::Editor,
            Launch::Browse(_) => AppKind::Browser,
            Launch::View(_) => AppKind::Viewer,
            Launch::Terminal(_) => AppKind::Terminal,
            Launch::Settings(_) => AppKind::Settings,
        };
        // Lo que pida la app al abrirse sale a su nombre (firewall).
        self.out.app = app_tag(kind);
        // ¿Ya está abierta?
        let existing = self.slots.iter().position(|s| match (&what, &s.app) {
            (Launch::Edit(p), App::Editor(e)) => e.path.as_deref() == Some(p.as_str()),
            (Launch::View(p), App::Viewer(v)) => v.path == *p,
            (Launch::App(AppKind::Editor), _) => false,
            _ => singleton(kind) && s.app.kind() == kind,
        });
        if let Some(i) = existing {
            let id = self.slots[i].id;
            let mut ctx = ctx!(self, now_ms, clock, &tasks);
            let slot = &mut self.slots[i];
            match (&what, &mut slot.app) {
                (Launch::Folder(p), App::Files(f)) => f.navigate(p, &mut ctx),
                (Launch::Browse(u), App::Browser(b)) => b.go(u, &mut ctx),
                (Launch::Terminal(Some(cmd)), App::Terminal(t)) => {
                    if !t.shell.waiting() {
                        t.run_command(cmd, &mut ctx);
                    }
                }
                (Launch::Settings(n), App::Settings(st)) => {
                    let section = crate::apps::settings::SECTIONS
                        [(*n).min(crate::apps::settings::SECTIONS.len() - 1)];
                    st.enter(section, &mut ctx);
                }
                _ => {}
            }
            if slot.app.take_dirty() {
                slot.content_dirty = true;
            }
            self.wm.activate(id);
            self.log_focus(id);
            return;
        }
        let mut ctx = ctx!(self, now_ms, clock, &tasks);
        let app = match what {
            Launch::App(AppKind::Files) => App::Files(FilesWindow::new("/", &mut ctx)),
            Launch::Folder(p) => App::Files(FilesWindow::new(&p, &mut ctx)),
            Launch::App(AppKind::Monitor) => App::Monitor(Monitor::new()),
            Launch::App(AppKind::Console) => App::Console(Console::new()),
            Launch::App(AppKind::Music) => App::Music(Music::new()),
            Launch::App(AppKind::Editor) => App::Editor(Editor::new()),
            Launch::Edit(p) => App::Editor(Editor::open(&p, &mut ctx)),
            Launch::App(AppKind::Viewer) => App::Files(FilesWindow::new("/Imágenes", &mut ctx)),
            Launch::View(p) => App::Viewer(Viewer::open(&p, &mut ctx)),
            Launch::App(AppKind::Browser) => {
                let mut b = Browser::new(ctx.config);
                let home = ctx.config.homepage.clone();
                b.go(&home, &mut ctx);
                App::Browser(b)
            }
            Launch::Browse(u) => {
                let mut b = Browser::new(ctx.config);
                b.go(&u, &mut ctx);
                App::Browser(b)
            }
            Launch::App(AppKind::Brave) => App::Brave(Brave::new(ctx.config)),
            Launch::App(AppKind::Terminal) | Launch::Terminal(None) => {
                App::Terminal(Terminal::new(&ctx.config.user, &ctx.config.hostname))
            }
            Launch::Terminal(Some(cmd)) => {
                let mut t = Terminal::new(&ctx.config.user, &ctx.config.hostname);
                t.run_command(&cmd, &mut ctx);
                App::Terminal(t)
            }
            Launch::App(AppKind::Settings) => App::Settings(Settings::new(
                crate::apps::settings::Section::System,
                &mut ctx,
            )),
            Launch::Settings(n) => App::Settings(Settings::new(
                crate::apps::settings::SECTIONS[n.min(crate::apps::settings::SECTIONS.len() - 1)],
                &mut ctx,
            )),
        };
        let (w, h) = app.default_size();
        let rect = self.wm.place(w, h);
        let id = self.wm.open(rect);
        self.logs
            .push(format!("VENTANA_ABIERTA {}", name_of(app.kind())));
        self.slots.push(Slot {
            id,
            app,
            buf: Vec::new(),
            size: (0, 0),
            content_dirty: true,
            chrome_dirty: true,
            hover: Hover::None,
        });
    }

    fn close_window(&mut self, id: WinId, now_ms: u64, clock: Option<DateTime>) {
        let Some(i) = self.slot_index(id) else { return };
        let tasks = self.tasks();
        let mut ctx = ctx!(self, now_ms, clock, &tasks);
        ctx.out.app = app_tag(self.slots[i].app.kind());
        let closed = self.slots[i].app.on_close(&mut ctx);
        if !closed {
            self.slots[i].content_dirty = true;
            return;
        }
        let mut slot = self.slots.remove(i);
        self.logs
            .push(format!("VENTANA_CERRADA {}", name_of(slot.app.kind())));
        if let Some(a) = self.anims.iter().find(|a| a.id == id) {
            self.damage.push(a.area());
        }
        self.anims.retain(|a| a.id != id);
        let dur = self.config.anim_ms();
        if let Some(win) = self.wm.get(id)
            && dur > 0
            && win.visible()
            && slot.size == (win.rect.w, win.rect.h)
        {
            self.anims.push(Anim {
                id,
                kind: AnimKind::Close,
                from: win.rect,
                to: appear_from(self.config.anim_style, win.rect),
                start: now_ms,
                dur,
                buf: core::mem::take(&mut slot.buf),
                size: slot.size,
            });
        }
        self.wm.close(id);
    }

    /// Hace lo que las apps pidieron (abrir otras apps, avisos, cerrar ventanas…) y deja lo que
    /// es para el kernel en `requests`.
    fn process_outbox(&mut self, now_ms: u64, clock: Option<DateTime>) {
        for _ in 0..4 {
            let out = core::mem::take(&mut self.out);
            if out.launch.is_empty()
                && out.close.is_empty()
                && out.notify.is_empty()
                && out.activate.is_none()
                && out.say.is_none()
                && !out.power_menu
                && !out.screenshot
                && out.net.is_empty()
                && out.streams.is_empty()
                && out.tone.is_none()
                && out.power.is_none()
                && out.config.is_none()
                && !out.lock
                && !out.close_self
            {
                self.out = out;
                return;
            }
            self.out.next_net = out.next_net;
            for l in out.launch {
                self.launch(l, now_ms, clock);
            }
            for id in out.close {
                self.close_window(id, now_ms, clock);
            }
            if let Some(id) = out.activate {
                self.wm.activate(id);
            }
            for (msg, error) in out.notify {
                self.notify(msg, error, now_ms);
            }
            if let Some(text) = out.say {
                self.say(&text, now_ms);
            }
            if out.power_menu {
                self.set_overlay(Overlay::Power { sel: 0 });
            }
            self.screenshot |= out.screenshot;
            for req in out.net {
                match self.firewall_check(&req) {
                    Ok(()) => self.requests.net.push(req),
                    Err(why) => self.firewall_block(req, why, now_ms, clock),
                }
            }
            for op in out.streams {
                if let StreamOp::Connect(r) = &op
                    && let Err(why) = self.config.firewall.check_out(&r.host, r.port, &r.app)
                {
                    let target = format!("{}:{}", r.host, r.port);
                    let msg =
                        self.firewall_record(&r.app, &r.host, &target, &why, true, now_ms, clock);
                    self.fw_blocked_streams.push((r.id, msg));
                    continue;
                }
                self.requests.streams.push(op);
            }
            if out.tone.is_some() {
                self.requests.tone = out.tone;
            }
            if let Some(p) = out.power {
                self.power(p);
            }
            if let Some(cfg) = out.config {
                self.apply_config(cfg, now_ms);
            }
            if out.lock {
                self.lock();
            }
            if out.close_self
                && let Some(id) = self.wm.focused()
            {
                self.close_window(id, now_ms, clock);
            }
        }
    }

    pub fn notify(&mut self, msg: impl Into<String>, error: bool, now_ms: u64) {
        let msg = msg.into();
        self.logs.push(format!("AVISO {msg}"));
        let time = self
            .last_clock
            .map(|t| format!("{:02}:{:02}", t.hour, t.minute))
            .unwrap_or_default();
        self.notices.push((msg.clone(), error, time));
        if self.notices.len() > 30 {
            self.notices.remove(0);
        }
        if error && self.config.sounds {
            self.requests.tone = Some(880);
            self.beep_until = Some(now_ms + 120);
        }
        self.toasts.push((msg, error, now_ms + TOAST_MS));
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    // --- mouse --------------------------------------------------------------------------------

    fn on_mouse(&mut self, p: MousePacket, now_ms: u64, clock: Option<DateTime>) {
        if self.is_sleeping() {
            if p.dx != 0 || p.dy != 0 || p.left || p.right || p.wheel != 0 {
                self.left_down = p.left;
                self.right_down = p.right;
                self.wake();
            }
            return;
        }
        let f = self.config.mouse_factor();
        self.cursor.0 = (self.cursor.0 + p.dx * f / 4).clamp(0, self.width as i32 - 1);
        self.cursor.1 = (self.cursor.1 + p.dy * f / 4).clamp(0, self.height as i32 - 1);
        self.cursor_visible = true;
        let (x, y) = self.cursor;

        if let Some(drag) = &self.drag
            && p.left
        {
            match *drag {
                Drag::Move { id, dx, dy } if self.config.drag_outline => {
                    let size = self.wm.get(id).map(|w| (w.rect.w, w.rect.h));
                    if let Some((w, h)) = size {
                        let r = Rect::new(x - dx, y - dy, w, h);
                        if let Some(old) = self.outline.replace(r) {
                            self.damage.push(old.inset(-2));
                        }
                        self.damage.push(r.inset(-2));
                    }
                }
                Drag::Move { id, dx, dy } => self.wm.move_to(id, x - dx, y - dy),
                Drag::Resize {
                    id,
                    right,
                    bottom,
                    start,
                    mx,
                    my,
                } => {
                    let w = if right { start.w + x - mx } else { start.w };
                    let h = if bottom { start.h + y - my } else { start.h };
                    self.wm.resize(id, w, h);
                }
            }
        }
        self.update_hover(x, y);

        if p.wheel != 0
            && let Some((id, _)) = self.wm.at(x, y)
            && let Some(i) = self.slot_index(id)
            && matches!(self.overlay, Overlay::None)
        {
            let content = self.content_of(id);
            let tasks = self.tasks();
            let mut ctx = ctx!(self, now_ms, clock, &tasks);
            let mut delta = p.wheel * self.config.wheel_lines as i32;
            delta = if delta.abs() < 3 {
                delta.signum()
            } else {
                delta / 3
            };
            if self.config.invert_wheel {
                delta = -delta;
            }
            let slot = &mut self.slots[i];
            ctx.out.app = app_tag(slot.app.kind());
            slot.app.wheel(delta, content, &mut ctx);
            if slot.app.take_dirty() {
                slot.content_dirty = true;
            }
        }

        let pressed = p.left && !self.left_down;
        let released = !p.left && self.left_down;
        let right_pressed = p.right && !self.right_down;
        self.left_down = p.left;
        self.right_down = p.right;
        if released && let Some(drag) = self.drag.take() {
            self.end_drag(drag, x, y);
        }
        if p.dx != 0 || p.dy != 0 || released {
            let kind = if released {
                PointerKind::Up
            } else {
                PointerKind::Move
            };
            self.send_pointer(kind, now_ms, clock);
        }
        if pressed {
            self.click(false, now_ms, clock);
        } else if right_pressed {
            self.click(true, now_ms, clock);
        }
    }

    /// Le pasa el movimiento (o el "soltar") a la app que está debajo del mouse, o a la que
    /// tiene capturado el mouse, si lo pide.
    fn send_pointer(&mut self, kind: PointerKind, now_ms: u64, clock: Option<DateTime>) {
        if !matches!(self.overlay, Overlay::None) {
            return;
        }
        let (x, y) = self.cursor;
        let target = match self.pointer_capture {
            Some(id) => Some(id),
            None => match self.wm.at(x, y) {
                Some((id, Part::Content(..))) => Some(id),
                _ => None,
            },
        };
        if kind == PointerKind::Up {
            self.pointer_capture = None;
        }
        let Some(id) = target else { return };
        let Some(i) = self.slot_index(id) else { return };
        if !self.slots[i].app.wants_pointer() {
            return;
        }
        let Some(r) = self.wm.get(id).map(|w| w.rect) else {
            return;
        };
        let content = self.content_of(id);
        let tasks = self.tasks();
        let mut ctx = ctx!(self, now_ms, clock, &tasks);
        let slot = &mut self.slots[i];
        ctx.out.app = app_tag(slot.app.kind());
        let p = Pointer {
            x: x - r.x,
            y: y - r.y,
            kind,
        };
        slot.app.pointer(p, content, &mut ctx);
        if slot.app.take_dirty() {
            slot.content_dirty = true;
        }
    }

    /// Se soltó el mouse después de arrastrar: el contorno pasa a ser la ventana, y contra un
    /// borde la ventana se acopla (arriba se maximiza), como en Windows.
    fn end_drag(&mut self, drag: Drag, x: i32, y: i32) {
        let Drag::Move { id, dx, dy } = drag else {
            return;
        };
        if let Some(r) = self.outline.take() {
            self.damage.push(r.inset(-2));
            self.wm.move_to(id, x - dx, y - dy);
        }
        if !self.config.snap_edges {
            return;
        }
        let (w, _) = (self.width as i32, self.height as i32);
        if x <= 1 {
            self.wm.snap(id, Side::Left);
        } else if x >= w - 2 {
            self.wm.snap(id, Side::Right);
        } else if y <= 1 && !self.wm.get(id).is_some_and(|w| w.maximized) {
            self.wm.toggle_maximize(id);
        } else {
            return;
        }
        self.logs.push("VENTANA_ACOPLADA".into());
    }

    fn update_hover(&mut self, x: i32, y: i32) {
        if self.topbar_mode() {
            self.toolbar_hover = None;
            self.top_hover = shell::topbar_hit(self.width, self.slots.len(), x, y);
        } else {
            let n = self.toolbar_items().len();
            self.toolbar_hover = shell::toolbar_hit(n, x, y);
            self.top_hover = None;
        }
        let hit = self.wm.at(x, y);
        // El foco sigue al mouse (como en muchos escritorios de Linux).
        if self.config.focus_follows
            && self.drag.is_none()
            && matches!(self.overlay, Overlay::None)
            && let Some((id, _)) = hit
            && self.wm.focused() != Some(id)
        {
            self.wm.activate(id);
            self.log_focus(id);
        }
        for s in &mut self.slots {
            let hover = match hit {
                Some((id, Part::Minimize)) if id == s.id => Hover::Minimize,
                Some((id, Part::Maximize)) if id == s.id => Hover::Maximize,
                Some((id, Part::Close)) if id == s.id => Hover::Close,
                _ => Hover::None,
            };
            if hover != s.hover {
                s.hover = hover;
                s.chrome_dirty = true;
            }
        }
    }

    fn click(&mut self, right: bool, now_ms: u64, clock: Option<DateTime>) {
        let (x, y) = self.cursor;
        let double = !right
            && self.last_click.is_some_and(|(t, cx, cy)| {
                now_ms - t <= DOUBLE_CLICK_MS && (cx - x).abs() <= 4 && (cy - y).abs() <= 4
            });
        self.last_click = if double || right {
            None
        } else {
            Some((now_ms, x, y))
        };

        // Menús y paneles encima de todo.
        match &self.overlay {
            // (Dormido no llega acá: el mouse despierta en on_mouse.)
            Overlay::Sleep => return,
            Overlay::Lock => {
                if self.config.pin.is_empty() {
                    self.unlock();
                }
                return;
            }
            Overlay::Menu(menu) => {
                let (w, h) = (self.width, self.height);
                let hit = menu.hit(w, h, x, y).and_then(|i| menu.items.get(i));
                let inside = menu.rect(w, h).contains(x, y);
                match hit.map(|i| i.action.clone()) {
                    Some(a) => self.menu_action(a, now_ms, clock),
                    None if !inside => self.set_overlay(Overlay::None),
                    None => {}
                }
                return;
            }
            Overlay::Quick { .. } => {
                let (w, h) = (self.width, self.height);
                if let Some(hit) = panels::quick_hit(w, h, x, y) {
                    self.quick_hit(hit, now_ms, clock);
                } else if !panels::quick_rect(w, h).contains(x, y) {
                    self.set_overlay(Overlay::None);
                }
                return;
            }
            Overlay::Notices => {
                let (w, h) = (self.width, self.height);
                if panels::clear_button(w, h).contains(x, y) {
                    self.notices.clear();
                    self.overlay_dirty = true;
                } else if !panels::notices_rect(w, h).contains(x, y) {
                    self.set_overlay(Overlay::None);
                }
                return;
            }
            Overlay::Power { .. } => {
                let buttons = shell::power_buttons(self.width, self.height);
                if let Some(i) = buttons.iter().position(|b| b.contains(x, y)) {
                    self.power_choice(i);
                }
                return;
            }
            Overlay::Start(menu) => {
                let hit = menu.hit(self.width, self.height, x, y);
                let inside = StartMenu::rect(self.width, self.height).contains(x, y);
                if let Some(item) = hit {
                    self.set_overlay(Overlay::None);
                    self.start_item(item, now_ms, clock);
                    return;
                }
                if inside {
                    return;
                }
                self.set_overlay(Overlay::None);
                // Un clic en el ícono de inicio con el menú abierto solo lo cierra.
                let start_icon = if self.topbar_mode() {
                    shell::topbar_hit(self.width, self.slots.len(), x, y)
                        == Some(TopHit::Launcher(0))
                } else {
                    shell::toolbar_hit(self.toolbar_items().len(), x, y) == Some(0)
                };
                if start_icon {
                    return;
                }
            }
            Overlay::Switcher { order, .. } => {
                let hit = shell::switcher_hit(self.width, self.height, order.len(), x, y);
                if let Some(i) = hit {
                    if let Overlay::Switcher { sel, .. } = &mut self.overlay {
                        *sel = i;
                    }
                    self.commit_switcher(now_ms, clock);
                } else {
                    self.set_overlay(Overlay::None);
                }
                return;
            }
            Overlay::TaskView { order, .. } => {
                let n = order.len();
                let hit = (0..n)
                    .find(|&i| shell::task_card(self.width, self.height, n, i).contains(x, y));
                let id = hit.and_then(|i| order.get(i).copied());
                self.set_overlay(Overlay::None);
                if let Some(id) = id {
                    self.wm.activate(id);
                    self.log_focus(id);
                }
                return;
            }
            Overlay::None => {}
        }

        // La barra de íconos (o la de arriba) está siempre encima.
        if self.topbar_mode() {
            if let Some(hit) = shell::topbar_hit(self.width, self.slots.len(), x, y) {
                self.topbar_action(hit, now_ms, clock);
                return;
            }
        } else {
            let n = self.toolbar_items().len();
            if let Some(i) = shell::toolbar_hit(n, x, y) {
                self.toolbar_action(i, now_ms, clock);
                return;
            }
        }

        if let Some((id, part)) = self.wm.at(x, y) {
            if self.wm.focused() != Some(id) {
                self.wm.activate(id);
            }
            match part {
                Part::Title if double => match self.config.title_double {
                    TitleDouble::Maximize => self.wm.toggle_maximize(id),
                    TitleDouble::Minimize => self.wm.minimize(id),
                    TitleDouble::Nothing => {}
                },
                Part::Title => {
                    let r = self.wm.get(id).map_or(Rect::new(0, 0, 0, 0), |w| w.rect);
                    // Si está maximizada, al arrastrarla vuelve a su tamaño bajo el mouse.
                    let restore_w = self.wm.get(id).and_then(|w| w.restore).map(|r| r.w);
                    let dx = match restore_w {
                        Some(rw) => (x - r.x) * rw / r.w.max(1),
                        None => x - r.x,
                    };
                    self.drag = Some(Drag::Move {
                        id,
                        dx,
                        dy: y - r.y,
                    });
                }
                Part::Minimize => {
                    self.wm.minimize(id);
                    self.logs.push("VENTANA_MINIMIZADA".into());
                }
                Part::Maximize => {
                    self.wm.toggle_maximize(id);
                    self.logs.push("VENTANA_MAXIMIZADA".into());
                }
                Part::Close => self.close_window(id, now_ms, clock),
                Part::Resize { right, bottom } => {
                    let start = self.wm.get(id).map_or(Rect::new(0, 0, 0, 0), |w| w.rect);
                    self.drag = Some(Drag::Resize {
                        id,
                        right,
                        bottom,
                        start,
                        mx: x,
                        my: y,
                    });
                }
                Part::Content(cx, cy) => {
                    let Some(i) = self.slot_index(id) else { return };
                    if !right && self.slots[i].app.wants_pointer() {
                        self.pointer_capture = Some(id);
                    }
                    let content = self.content_of(id);
                    let tasks = self.tasks();
                    let mut ctx = ctx!(self, now_ms, clock, &tasks);
                    let slot = &mut self.slots[i];
                    ctx.out.app = app_tag(slot.app.kind());
                    slot.app.click(
                        Click {
                            x: cx,
                            y: cy,
                            double,
                            right,
                        },
                        content,
                        &mut ctx,
                    );
                    if slot.app.take_dirty() {
                        slot.content_dirty = true;
                    }
                }
            }
            return;
        }

        // El escritorio.
        self.wm.focus_desktop();
        let status = if self.config.status_panel {
            shell::status_hit(self.width, self.height, x, y)
        } else {
            None
        };
        match status {
            Some(StatusHit::Panel) => self.launch(Launch::App(AppKind::Monitor), now_ms, clock),
            Some(StatusHit::MissionControl) => self.open_task_view(),
            None => {
                let view = hud::sphere_view(self.width, self.height, now_ms);
                let (dx, dy) = (x - view.cx, y - view.cy);
                if dx * dx + dy * dy <= view.radius * view.radius && !right {
                    self.speak_demo(now_ms);
                }
            }
        }
    }

    /// Los íconos de la barra: los fijos y uno por cada editor o visor abierto.
    fn toolbar_items(&self) -> Vec<ToolbarItem> {
        let focused_kind = self.focused_app();
        let focused = self.wm.focused();
        let mut items: Vec<ToolbarItem> = LAUNCHERS
            .iter()
            .map(|&l| {
                let (running, active) = match l {
                    Launcher::App(k) => (
                        self.slots.iter().any(|s| s.app.kind() == k),
                        focused_kind == Some(k),
                    ),
                    Launcher::Start => (false, matches!(self.overlay, Overlay::Start(_))),
                    Launcher::Jarvis => (false, focused.is_none()),
                    Launcher::Capture => (false, false),
                };
                ToolbarItem {
                    icon: shell::launcher_icon(l),
                    name: shell::launcher_name(l).into(),
                    running,
                    active,
                }
            })
            .collect();
        for s in self
            .slots
            .iter()
            .filter(|s| !singleton(s.app.kind()))
            .take(MAX_EXTRA)
        {
            items.push(ToolbarItem {
                icon: s.app.icon(),
                name: s.app.title(),
                running: true,
                active: focused == Some(s.id),
            });
        }
        items
    }

    /// ¿Se muestra la barra de arriba? (Hay una ventana maximizada y nada tapa toda la
    /// pantalla.)
    pub fn topbar_mode(&self) -> bool {
        self.config.topbar && self.wm.any_maximized() && !self.overlay_is_fullscreen()
    }

    /// Las ventanas abiertas, en el orden en que se abrieron (para la barra de arriba).
    fn top_windows(&self) -> Vec<TopWindow> {
        let focused = self.wm.focused();
        self.slots
            .iter()
            .filter_map(|s| {
                let win = self.wm.get(s.id)?;
                Some(TopWindow {
                    icon: s.app.icon(),
                    title: s.app.title(),
                    active: focused == Some(s.id),
                    minimized: !win.visible(),
                })
            })
            .collect()
    }

    fn topbar_action(&mut self, hit: TopHit, now_ms: u64, clock: Option<DateTime>) {
        match hit {
            TopHit::Launcher(i) => self.toolbar_action(i, now_ms, clock),
            TopHit::Window(i) => {
                let Some(id) = self.slots.get(i).map(|s| s.id) else {
                    return;
                };
                self.logs.push(format!("BARRA_ARRIBA ventana {i}"));
                let visible = self.wm.get(id).is_some_and(|w| w.visible());
                if self.wm.focused() == Some(id) && visible {
                    self.wm.minimize(id);
                } else {
                    self.wm.activate(id);
                    self.log_focus(id);
                }
            }
            TopHit::Status => {
                self.logs.push("BARRA_ARRIBA estado".into());
                self.launch(Launch::App(AppKind::Monitor), now_ms, clock);
            }
            TopHit::Clock => {
                self.logs.push("BARRA_ARRIBA hora".into());
                self.toggle_overlay(Overlay::Notices);
            }
            TopHit::Power => {
                self.logs.push("BARRA_ARRIBA energia".into());
                self.set_overlay(Overlay::Power { sel: 0 });
            }
        }
    }

    fn toolbar_action(&mut self, i: usize, now_ms: u64, clock: Option<DateTime>) {
        if let Some(&l) = LAUNCHERS.get(i) {
            self.logs.push(format!("BARRA {}", shell::launcher_name(l)));
            match l {
                Launcher::Start => self.toggle_start(),
                Launcher::Capture => self.screenshot = true,
                Launcher::Jarvis => self.wm.toggle_desktop(),
                Launcher::App(kind) => {
                    // El navegador principal se elige en la Configuración.
                    let kind = if kind == AppKind::Brave && !self.config.brave_default {
                        AppKind::Browser
                    } else {
                        kind
                    };
                    let existing = self
                        .slots
                        .iter()
                        .find(|s| s.app.kind() == kind)
                        .map(|s| s.id);
                    match existing {
                        // Como la barra de tareas de Windows: si ya tiene el foco, se minimiza.
                        Some(id) if self.wm.focused() == Some(id) => self.wm.minimize(id),
                        Some(id) => self.wm.activate(id),
                        None => self.launch(Launch::App(kind), now_ms, clock),
                    }
                }
            }
            return;
        }
        let extra: Vec<WinId> = self
            .slots
            .iter()
            .filter(|s| !singleton(s.app.kind()))
            .map(|s| s.id)
            .collect();
        if let Some(&id) = extra.get(i - LAUNCHERS.len()) {
            if self.wm.focused() == Some(id) {
                self.wm.minimize(id);
            } else {
                self.wm.activate(id);
            }
        }
    }

    // --- dibujo -------------------------------------------------------------------------------

    /// Compone el frame de `now_ms` en `frame` y devuelve qué zonas cambiaron.
    pub fn render(
        &mut self,
        frame: &mut Canvas<'_>,
        bg: &mut Canvas<'_>,
        now_ms: u64,
        clock: Option<DateTime>,
    ) -> Dirty {
        self.format = Some((frame.format(), frame.bytes_per_pixel()));
        self.last_now = now_ms;
        self.last_clock = clock;
        if self.is_sleeping() {
            let mut dirty = Dirty::default();
            if core::mem::take(&mut self.full_redraw) | core::mem::take(&mut self.overlay_dirty) {
                frame.fill(Color::BLACK);
                dirty.push(Rect::new(0, 0, self.width as i32, self.height as i32));
            }
            return dirty;
        }
        if self.bg_dirty {
            self.draw_background(bg);
            self.full_redraw = true;
        }
        if let Some(t) = self.beep_until
            && now_ms >= t
        {
            self.beep_until = None;
            self.requests.tone = Some(0);
        }
        // Bloqueo por inactividad.
        let idle_ms = self.config.lock_minutes as u64 * 60_000;
        if idle_ms > 0
            && now_ms.saturating_sub(self.last_input) >= idle_ms
            && !matches!(self.overlay, Overlay::Lock)
        {
            self.lock();
            self.logs.push("ESCRITORIO_BLOQUEO_INACTIVIDAD".into());
        }
        if self.assistant.take_finished(now_ms) {
            self.logs.push("JARVIS_REPOSO".into());
        }
        let before = self.geometry();
        let tasks = self.tasks();
        {
            let mut ctx = ctx!(self, now_ms, clock, &tasks);
            for s in &mut self.slots {
                ctx.out.app = app_tag(s.app.kind());
                s.app.tick(&mut ctx);
                if s.app.take_dirty() {
                    s.content_dirty = true;
                }
            }
        }
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before, now_ms);
        let expired = self.toasts.len();
        self.toasts.retain(|t| t.2 > now_ms);
        if self.toasts.len() != expired {
            self.damage
                .push(shell::toasts_rect(self.width, self.height));
        }

        let (w, h) = (self.width, self.height);
        let screen = Rect::new(0, 0, w as i32, h as i32);
        let anim_ms = if self.config.animations { now_ms } else { 0 };
        let view = hud::sphere_view(w, h, anim_ms);
        let pulse = self.assistant.pulse(now_ms);
        let sphere = ParticleCloud::bounds(&view);
        let clock_rect = hud::clock_rect(w, h);
        let message_rect = hud::message_rect(w, h);
        let status_rect = shell::status_rect(w, h);

        let top = self.topbar_mode();
        if top != self.drawn_top_mode {
            self.full_redraw = true;
        }
        let mut dirty = core::mem::take(&mut self.damage);
        let full = self.full_redraw;
        if full {
            dirty = Dirty::default();
            dirty.push(screen);
        }
        let fullscreen_overlay = self.overlay_is_fullscreen();
        if self.assistant.is_speaking(now_ms) {
            self.sphere_settle = 90;
        }
        let sphere_moves = self.config.animations || self.sphere_settle > 0;
        self.sphere_settle = self.sphere_settle.saturating_sub(1);
        // (Durante una transición la esfera puede quedar a la vista aunque las ventanas la tapen.)
        let covered = self.wm.covers(&sphere) && self.anims.is_empty();
        if !fullscreen_overlay && !covered && sphere_moves {
            dirty.push(sphere);
        }
        let clock_key = clock.map(|t| (t.year, t.month, t.day, t.hour, t.minute));
        if self.drawn_clock != Some(clock_key) {
            dirty.push(if matches!(self.overlay, Overlay::Lock) {
                screen
            } else {
                clock_rect
            });
        }
        let speaking = self.assistant.is_speaking(now_ms);
        let message_key = (self.assistant.visible_chars(now_ms), speaking);
        if self.drawn_message != Some(message_key) {
            dirty.push(message_rect);
        }
        if self.drawn_stats != self.stats_version && self.config.status_panel {
            dirty.push(status_rect);
        }
        let toolbar = (self.toolbar_items(), self.toolbar_hover);
        let top_key: Option<TopKey> = top.then(|| {
            (
                toolbar.0.clone(),
                self.top_windows(),
                self.top_hover,
                shell::clock_text(clock, self.config.clock_24h, self.config.clock_seconds),
                self.stats_version,
            )
        });
        if top {
            if self.drawn_top != top_key {
                dirty.push(shell::topbar_rect(w));
            }
        } else if self.drawn_toolbar.as_ref() != Some(&toolbar) {
            dirty.push(shell::toolbar_area());
        }
        // Transiciones: toda su zona cambia en cada frame (y una vez más al terminar).
        for a in &self.anims {
            dirty.push(a.area());
            // (Si la ventana se movió mientras tanto, también donde está ahora.)
            if let Some(win) = self.wm.get(a.id)
                && win.visible()
            {
                dirty.push(win.rect);
            }
        }
        self.anims.retain(|a| !a.done(now_ms));
        let toasts: Vec<(String, bool)> = self.toasts.iter().map(|t| (t.0.clone(), t.1)).collect();
        if toasts != self.drawn_toasts {
            dirty.push(shell::toasts_rect(w, h));
        }
        if self.overlay_dirty {
            dirty.push(self.overlay_rect());
        }

        // Ventanas: se redibujan en su buffer las que cambiaron.
        self.render_windows(&mut dirty, now_ms, clock, &tasks);

        if !dirty.is_empty() {
            for r in dirty.iter() {
                frame.copy_from(bg, r);
            }
            frame.set_clip(dirty.iter());
            self.compose(
                frame, &dirty, now_ms, clock, &view, pulse, &toolbar.0, &toasts,
            );
            frame.clear_clip();
        }

        self.full_redraw = false;
        self.overlay_dirty = false;
        self.drawn_clock = Some(clock_key);
        self.drawn_message = Some(message_key);
        self.drawn_stats = self.stats_version;
        self.drawn_toolbar = Some(toolbar);
        self.drawn_top = top_key;
        self.drawn_top_mode = top;
        self.drawn_toasts = toasts;

        if core::mem::take(&mut self.screenshot) {
            self.save_screenshot(frame, clock, now_ms, None);
        }
        if core::mem::take(&mut self.window_screenshot) {
            let area = self
                .wm
                .focused()
                .and_then(|id| self.wm.get(id))
                .map(|w| w.rect);
            self.save_screenshot(frame, clock, now_ms, area);
        }
        dirty
    }

    fn overlay_rect(&self) -> Rect {
        let (w, h) = (self.width, self.height);
        match &self.overlay {
            Overlay::Start(_) => StartMenu::rect(w, h).inset(-4),
            Overlay::Switcher { order, .. } => shell::switcher_rect(w, h, order.len()).inset(-8),
            Overlay::Menu(m) => m.rect(w, h).inset(-4),
            Overlay::Quick { .. } => panels::quick_rect(w, h).inset(-4),
            Overlay::Notices => panels::notices_rect(w, h).inset(-4),
            _ => Rect::new(0, 0, w as i32, h as i32),
        }
    }

    fn render_windows(
        &mut self,
        dirty: &mut Dirty,
        now_ms: u64,
        clock: Option<DateTime>,
        tasks: &[TaskInfo],
    ) {
        let Some((format, bpp)) = self.format else {
            return;
        };
        let disk = self
            .fs
            .as_ref()
            .map(|fs| (fs.label().to_string(), fs.free_bytes(), fs.total_bytes()));
        let sys = SysView {
            stats: &self.stats,
            history: &self.history,
            tasks,
            disk: disk.as_ref().map(|(l, f, t)| (l.as_str(), *f, *t)),
            now_ms,
            clock,
            screen: (self.width, self.height),
        };
        let focused = self.wm.focused();
        for slot in &mut self.slots {
            let Some(win) = self.wm.get(slot.id) else {
                continue;
            };
            if !win.visible() {
                continue;
            }
            let size = (win.rect.w, win.rect.h);
            let resized = slot.size != size;
            if !(resized || slot.content_dirty || slot.chrome_dirty) {
                continue;
            }
            if resized {
                slot.buf.clear();
                slot.buf.resize((size.0 * size.1) as usize * bpp, 0);
                slot.size = size;
            }
            let Some(mut c) = Canvas::new(
                &mut slot.buf,
                size.0 as usize,
                size.1 as usize,
                size.0 as usize,
                bpp,
                format,
            ) else {
                continue;
            };
            let content = win.content();
            if resized || slot.content_dirty {
                c.set_clip([content]);
                slot.app.draw(&mut c, content, &sys);
                c.clear_clip();
            }
            chrome::draw(
                &mut c,
                size.0,
                size.1,
                slot.app.icon(),
                &slot.app.title(),
                focused == Some(slot.id),
                win.maximized,
                slot.hover,
            );
            slot.content_dirty = false;
            slot.chrome_dirty = false;
            dirty.push(win.rect);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compose(
        &mut self,
        frame: &mut Canvas<'_>,
        dirty: &Dirty,
        now_ms: u64,
        clock: Option<DateTime>,
        view: &jarvis_gfx::sphere::View,
        pulse: jarvis_gfx::sphere::Pulse,
        toolbar: &[ToolbarItem],
        toasts: &[(String, bool)],
    ) {
        let (w, h) = (self.width, self.height);
        let Some((format, bpp)) = self.format else {
            return;
        };
        match &self.overlay {
            Overlay::Sleep => {
                frame.fill(Color::BLACK);
                return;
            }
            Overlay::Lock => {
                let pin =
                    (!self.config.pin.is_empty()).then_some((self.pin_input.len(), self.pin_wrong));
                let user = self.session_closed.then_some(self.config.user.as_str());
                shell::draw_lock(
                    frame,
                    w,
                    h,
                    clock,
                    &mut self.lock_font,
                    self.config.clock_24h,
                    pin,
                    user,
                );
                return;
            }
            Overlay::Power { sel } => {
                shell::draw_power(frame, w, h, *sel);
                return;
            }
            Overlay::TaskView { order, sel } => {
                let (order, sel) = (order.clone(), *sel);
                let thumbs = self.thumbs(&order, format, bpp);
                shell::draw_task_view(frame, w, h, &thumbs, sel);
                drop(thumbs);
                shell::draw_toolbar(frame, toolbar, self.toolbar_hover);
                return;
            }
            _ => {}
        }
        // Fondo: la esfera, el reloj, el mensaje de JARVIS y el panel de estado.
        let sphere = ParticleCloud::bounds(view);
        if dirty.touches(&sphere) {
            hud::draw_voice_glow(frame, view, &pulse);
            self.cloud.draw(frame, view, pulse);
        }
        if dirty.touches(&hud::clock_rect(w, h)) {
            hud::draw_clock(frame, &mut self.clock_face, clock, self.config.clock_24h);
        }
        if dirty.touches(&hud::message_rect(w, h)) {
            let speaking = self.assistant.is_speaking(now_ms);
            hud::draw_message(frame, self.assistant.visible_text(now_ms), speaking);
        }
        if self.config.status_panel && dirty.touches(&shell::status_rect(w, h)) {
            let disk = self
                .fs
                .as_ref()
                .map(|fs| (fs.free_bytes(), fs.total_bytes()));
            shell::draw_status(frame, w, h, &self.stats, &self.history, disk);
        }
        // Ventanas, de atrás hacia adelante (las que están en transición, escaladas).
        for win in self.wm.windows().iter().filter(|w| w.visible()) {
            let anim = self
                .anims
                .iter()
                .find(|a| a.id == win.id)
                .map(|a| (a.area(), a.frame(now_ms)));
            if !dirty.touches(&anim.map_or(win.rect, |a| a.0)) {
                continue;
            }
            let Some(slot) = self.slots.iter_mut().find(|s| s.id == win.id) else {
                continue;
            };
            if slot.size != (win.rect.w, win.rect.h) {
                continue;
            }
            if let Some(src) = Canvas::new(
                &mut slot.buf,
                slot.size.0 as usize,
                slot.size.1 as usize,
                slot.size.0 as usize,
                bpp,
                format,
            ) {
                match anim {
                    Some((_, (r, alpha))) => frame.blit_scaled_alpha(&src, r, alpha),
                    None => frame.blit(&src, win.rect.x, win.rect.y),
                }
            }
        }
        if let Some(r) = self.outline {
            jarvis_gfx::shapes::rect_outline(frame, r.x, r.y, r.w, r.h, theme::cyan());
            jarvis_gfx::shapes::rect_outline(
                frame,
                r.x - 1,
                r.y - 1,
                r.w + 2,
                r.h + 2,
                theme::cyan(),
            );
        }
        // Las que se cierran o se minimizan, encima de todo.
        for a in &mut self.anims {
            if !matches!(a.kind, AnimKind::Close | AnimKind::Hide) || !dirty.touches(&a.area()) {
                continue;
            }
            let (r, alpha) = a.frame(now_ms);
            let (buf, size) = if a.kind == AnimKind::Close {
                (&mut a.buf, a.size)
            } else {
                match self.slots.iter_mut().find(|s| s.id == a.id) {
                    Some(s) => (&mut s.buf, s.size),
                    None => continue,
                }
            };
            if size.0 <= 0 || buf.len() < (size.0 * size.1) as usize * bpp {
                continue;
            }
            if let Some(src) = Canvas::new(
                buf,
                size.0 as usize,
                size.1 as usize,
                size.0 as usize,
                bpp,
                format,
            ) {
                frame.blit_scaled_alpha(&src, r, alpha);
            }
        }
        if self.topbar_mode() {
            let windows = self.top_windows();
            let clock_s =
                shell::clock_text(clock, self.config.clock_24h, self.config.clock_seconds);
            let disk = self
                .fs
                .as_ref()
                .map(|fs| (fs.free_bytes(), fs.total_bytes()));
            let bar = shell::TopBar {
                launchers: toolbar,
                windows: &windows,
                hover: self.top_hover,
                clock: &clock_s,
                stats: &self.stats,
                history: &self.history,
                disk,
            };
            shell::draw_topbar(frame, w, &bar);
        } else {
            shell::draw_toolbar(frame, toolbar, self.toolbar_hover);
        }
        if !toasts.is_empty() {
            shell::draw_toasts(frame, w, h, toasts);
        }
        match &self.overlay {
            Overlay::Start(menu) => menu.draw(frame, w, h),
            Overlay::Switcher { order, sel } => {
                let (order, sel) = (order.clone(), *sel);
                let thumbs = self.thumbs(&order, format, bpp);
                shell::draw_switcher(frame, w, h, &thumbs, sel);
            }
            Overlay::Menu(m) => m.draw(frame, w, h),
            Overlay::Quick { sel } => {
                let net = match self.stats.net.ip {
                    Some(a) => format!("RED {}", crate::widgets::ip(a)),
                    None => tr("SIN RED").into(),
                };
                panels::draw_quick(frame, w, h, &self.config, *sel, &net);
            }
            Overlay::Notices => panels::draw_notices(frame, w, h, &self.notices, clock),
            _ => {}
        }
    }

    /// Miniaturas de las ventanas `order` (su buffer, si ya se dibujó alguna vez).
    fn thumbs(&mut self, order: &[WinId], format: PixelFormat, bpp: usize) -> Vec<Thumb<'_>> {
        let mut found: Vec<(usize, Thumb<'_>)> = Vec::new();
        for slot in self.slots.iter_mut() {
            let Some(pos) = order.iter().position(|&id| id == slot.id) else {
                continue;
            };
            let (sw, sh) = slot.size;
            let title = slot.app.title();
            let icon = slot.app.icon();
            let image = if sw > 0 {
                Canvas::new(
                    &mut slot.buf,
                    sw as usize,
                    sh as usize,
                    sw as usize,
                    bpp,
                    format,
                )
            } else {
                None
            };
            found.push((pos, Thumb { title, icon, image }));
        }
        found.sort_by_key(|(pos, _)| *pos);
        found.into_iter().map(|(_, t)| t).collect()
    }

    fn save_screenshot(
        &mut self,
        frame: &Canvas<'_>,
        clock: Option<DateTime>,
        now_ms: u64,
        area: Option<Rect>,
    ) {
        let Some(fs) = self.fs.as_mut() else {
            self.notify(tr("No hay disco para guardar la captura."), true, now_ms);
            return;
        };
        let ts = crate::apps::timestamp(clock);
        let name = format!(
            "Captura {}-{:02}-{:02} {:02}.{:02}.{:02}.bmp",
            ts.year, ts.month, ts.day, ts.hour, ts.minute, ts.second
        );
        let dir = "/Imágenes";
        if !fs.exists(dir) {
            let _ = fs.mkdir(dir, ts);
        }
        let path = format!("{dir}/{name}");
        let data = match area.and_then(|a| {
            a.intersection(&Rect::new(
                0,
                0,
                frame.width() as i32,
                frame.height() as i32,
            ))
        }) {
            Some(a) => {
                let bpp = frame.bytes_per_pixel();
                let mut buf = alloc::vec![0u8; (a.w * a.h) as usize * bpp];
                match Canvas::new(
                    &mut buf,
                    a.w as usize,
                    a.h as usize,
                    a.w as usize,
                    bpp,
                    frame.format(),
                ) {
                    Some(mut c) => {
                        c.blit(frame, -a.x, -a.y);
                        crate::bmp::encode(&c)
                    }
                    None => crate::bmp::encode(frame),
                }
            }
            None => crate::bmp::encode(frame),
        };
        match fs.write_file(&path, &data, ts) {
            Ok(()) => {
                self.logs.push(format!("CAPTURA {path}"));
                self.notify(trf("Captura guardada en {}", &[&path]), false, now_ms);
            }
            Err(e) => self.notify(
                trf("No se pudo guardar la captura: {}", &[&e.to_string()]),
                true,
                now_ms,
            ),
        }
    }

    /// Copia a la pantalla lo que cambió y dibuja el cursor encima.
    pub fn present(&mut self, screen: &mut Canvas<'_>, frame: &Canvas<'_>, dirty: &Dirty) {
        for r in dirty.iter() {
            screen.copy_from(frame, r);
        }
        if let Some(old) = self.cursor_drawn.take() {
            screen.copy_from(frame, old); // borra el cursor anterior
        }
        if self.cursor_visible {
            self.cursor_drawn = Some(cursor::draw(screen, self.cursor.0, self.cursor.1));
        }
    }
}
