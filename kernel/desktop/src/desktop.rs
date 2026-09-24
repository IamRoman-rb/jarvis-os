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

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};
use jarvis_gfx::assistant::Assistant;
use jarvis_gfx::clock::DateTime;
use jarvis_gfx::hud;
use jarvis_gfx::sphere::ParticleCloud;
use jarvis_gfx::vfont::VectorText;
use jarvis_gfx::{Canvas, MAX_CLIP, PixelFormat, Rect};

use crate::apps::{
    App, Click, Ctx, SysView, TaskInfo, browser::Browser, console::Console, editor::Editor,
    files::FilesWindow, monitor::Monitor, music::Music, name_of, viewer::Viewer,
};
use crate::chrome::{self, Hover};
use crate::cursor;
use crate::input::{Event, Key, Mods, MousePacket};
use crate::shell::{
    self, LAUNCHERS, Launcher, MAX_EXTRA, StartItem, StartMenu, StatusHit, Thumb, ToolbarItem,
};
use crate::system::{
    AppKind, History, HttpResponse, Launch, NetRequest, Outbox, Power, SystemStats,
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
}

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
    drag: Option<Drag>,
    last_click: Option<(u64, i32, i32)>,
    toolbar_hover: Option<usize>,
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
    drawn_toasts: Vec<(String, bool)>,
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
        }
    };
}

fn singleton(kind: AppKind) -> bool {
    !matches!(kind, AppKind::Editor | AppKind::Viewer)
}

impl<D: BlockDevice> Desktop<D> {
    pub fn new(width: usize, height: usize, particles: usize, fs: Option<FileSystem<D>>) -> Self {
        let screen = Rect::new(0, 0, width as i32, height as i32);
        let work = Rect::new(0, WORK_TOP, width as i32, height as i32 - WORK_TOP);
        Desktop {
            width,
            height,
            fs,
            wm: WindowManager::new(screen, work),
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
            drag: None,
            last_click: None,
            toolbar_hover: None,
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
            drawn_toasts: Vec::new(),
        }
    }

    pub fn draw_background(&self, bg: &mut Canvas<'_>) {
        hud::draw_static(bg);
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

    /// Nombre del menú o panel abierto encima de todo ("" si no hay).
    pub fn overlay_name(&self) -> &'static str {
        match self.overlay {
            Overlay::None => "",
            Overlay::Start(_) => "inicio",
            Overlay::Switcher { .. } => "alt-tab",
            Overlay::TaskView { .. } => "tareas",
            Overlay::Power { .. } => "apagado",
            Overlay::Lock => "bloqueo",
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
        core::mem::take(&mut self.requests)
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
        for s in &mut self.slots {
            s.app.net_response(id, &result);
            if s.app.take_dirty() {
                s.content_dirty = true;
            }
        }
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
        let before = self.geometry();
        match event {
            Event::Key(key) => self.on_key(key, now_ms, clock),
            Event::Mods(m) => self.on_mods(m, now_ms, clock),
            Event::Mouse(p) => self.on_mouse(p, now_ms, clock),
        }
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before);
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
    fn damage_geometry(&mut self, before: &[(WinId, Rect, bool, bool, bool)]) {
        let after = self.geometry();
        for (i, now) in after.iter().enumerate() {
            let old = before.iter().position(|b| b.0 == now.0);
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
        matches!(
            self.overlay,
            Overlay::TaskView { .. } | Overlay::Power { .. } | Overlay::Lock
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
        let order: Vec<WinId> = self.wm.mru().to_vec();
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
        let mut order: Vec<WinId> = self.wm.mru().to_vec();
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
        if matches!(self.overlay, Overlay::Lock) {
            self.logs.push("ESCRITORIO_DESBLOQUEADO".into());
            self.set_overlay(Overlay::None);
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
            slot.app.key(key, m, content, &mut ctx);
            if slot.app.take_dirty() {
                slot.content_dirty = true;
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

    /// Atajos de Windows. Devuelve `true` si la tecla era uno.
    fn global_shortcut(&mut self, key: Key, m: Mods, now_ms: u64, clock: Option<DateTime>) -> bool {
        let focused = self.wm.focused();
        if m.win {
            match key {
                Key::Char('d' | 'D') => {
                    self.wm.toggle_desktop();
                    self.logs.push("ESCRITORIO_MOSTRAR".into());
                }
                Key::Char('m' | 'M') => self.wm.minimize_all(),
                Key::Char('e' | 'E') => self.launch(Launch::App(AppKind::Files), now_ms, clock),
                Key::Char('r' | 'R') => self.launch(Launch::App(AppKind::Console), now_ms, clock),
                Key::Char('x' | 'X') => self.launch(Launch::App(AppKind::Monitor), now_ms, clock),
                Key::Char('l' | 'L') => {
                    self.logs.push("ESCRITORIO_BLOQUEADO".into());
                    self.set_overlay(Overlay::Lock);
                }
                Key::Char('s' | 'S') if m.shift => self.screenshot = true,
                Key::Char('s' | 'S' | 'q' | 'Q') => self.toggle_start(),
                Key::Tab => self.open_task_view(),
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
                Key::Char(c @ '1'..='8') => {
                    let i = c as usize - '1' as usize;
                    self.toolbar_action(i, now_ms, clock);
                }
                Key::PrintScreen => self.screenshot = true,
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
                _ => return false,
            }
            return true;
        }
        match key {
            Key::Escape if m.ctrl && m.shift => {
                self.launch(Launch::App(AppKind::Monitor), now_ms, clock)
            }
            Key::Delete if m.ctrl && m.alt => {
                self.launch(Launch::App(AppKind::Monitor), now_ms, clock)
            }
            Key::PrintScreen => self.screenshot = true,
            Key::F(11) => match focused {
                Some(id) => self.wm.toggle_maximize(id),
                None => return false,
            },
            _ => return false,
        }
        true
    }

    /// Teclas para el menú o panel abierto. `true` si se usó.
    fn overlay_key(&mut self, key: Key, now_ms: u64, clock: Option<DateTime>) -> bool {
        match &mut self.overlay {
            Overlay::None | Overlay::Lock => false,
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
                        *sel = (*sel + 1) % 3;
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
        match choice {
            0 => self.power(Power::Shutdown),
            1 => self.power(Power::Reboot),
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
        self.damage_geometry(&before);
    }

    fn launch(&mut self, what: Launch, now_ms: u64, clock: Option<DateTime>) {
        let tasks = self.tasks();
        let kind = match &what {
            Launch::App(k) => *k,
            Launch::Folder(_) => AppKind::Files,
            Launch::Edit(_) => AppKind::Editor,
            Launch::Browse(_) => AppKind::Browser,
            Launch::View(_) => AppKind::Viewer,
        };
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
                let mut b = Browser::new();
                b.go(crate::apps::browser::HOME, &mut ctx);
                App::Browser(b)
            }
            Launch::Browse(u) => {
                let mut b = Browser::new();
                b.go(&u, &mut ctx);
                App::Browser(b)
            }
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
        let closed = self.slots[i].app.on_close(&mut ctx);
        if !closed {
            self.slots[i].content_dirty = true;
            return;
        }
        let slot = self.slots.remove(i);
        self.logs
            .push(format!("VENTANA_CERRADA {}", name_of(slot.app.kind())));
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
                && out.tone.is_none()
                && out.power.is_none()
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
            self.requests.net.extend(out.net);
            if out.tone.is_some() {
                self.requests.tone = out.tone;
            }
            if let Some(p) = out.power {
                self.power(p);
            }
        }
    }

    pub fn notify(&mut self, msg: impl Into<String>, error: bool, now_ms: u64) {
        let msg = msg.into();
        self.logs.push(format!("AVISO {msg}"));
        self.toasts.push((msg, error, now_ms + TOAST_MS));
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
    }

    // --- mouse --------------------------------------------------------------------------------

    fn on_mouse(&mut self, p: MousePacket, now_ms: u64, clock: Option<DateTime>) {
        self.cursor.0 = (self.cursor.0 + p.dx).clamp(0, self.width as i32 - 1);
        self.cursor.1 = (self.cursor.1 + p.dy).clamp(0, self.height as i32 - 1);
        self.cursor_visible = true;
        let (x, y) = self.cursor;

        if let Some(drag) = &self.drag
            && p.left
        {
            match *drag {
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
            let slot = &mut self.slots[i];
            slot.app.wheel(p.wheel, content, &mut ctx);
            if slot.app.take_dirty() {
                slot.content_dirty = true;
            }
        }

        let pressed = p.left && !self.left_down;
        let released = !p.left && self.left_down;
        let right_pressed = p.right && !self.right_down;
        self.left_down = p.left;
        self.right_down = p.right;
        if released {
            self.drag = None;
        }
        if pressed {
            self.click(false, now_ms, clock);
        } else if right_pressed {
            self.click(true, now_ms, clock);
        }
    }

    fn update_hover(&mut self, x: i32, y: i32) {
        let n = self.toolbar_items().len();
        self.toolbar_hover = shell::toolbar_hit(n, x, y);
        let hit = self.wm.at(x, y);
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
            Overlay::Lock => {
                self.set_overlay(Overlay::None);
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
                if shell::toolbar_hit(self.toolbar_items().len(), x, y) == Some(0) {
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

        // La barra de íconos está siempre encima.
        let n = self.toolbar_items().len();
        if let Some(i) = shell::toolbar_hit(n, x, y) {
            self.toolbar_action(i, now_ms, clock);
            return;
        }

        if let Some((id, part)) = self.wm.at(x, y) {
            if self.wm.focused() != Some(id) {
                self.wm.activate(id);
            }
            match part {
                Part::Title if double => self.wm.toggle_maximize(id),
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
                    let content = self.content_of(id);
                    let tasks = self.tasks();
                    let mut ctx = ctx!(self, now_ms, clock, &tasks);
                    let slot = &mut self.slots[i];
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
        match shell::status_hit(self.width, self.height, x, y) {
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

    fn toolbar_action(&mut self, i: usize, now_ms: u64, clock: Option<DateTime>) {
        if let Some(&l) = LAUNCHERS.get(i) {
            self.logs.push(format!("BARRA {}", shell::launcher_name(l)));
            match l {
                Launcher::Start => self.toggle_start(),
                Launcher::Capture => self.screenshot = true,
                Launcher::Jarvis => self.wm.toggle_desktop(),
                Launcher::App(kind) => {
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
        bg: &Canvas<'_>,
        now_ms: u64,
        clock: Option<DateTime>,
    ) -> Dirty {
        self.format = Some((frame.format(), frame.bytes_per_pixel()));
        if self.assistant.take_finished(now_ms) {
            self.logs.push("JARVIS_REPOSO".into());
        }
        let before = self.geometry();
        let tasks = self.tasks();
        {
            let mut ctx = ctx!(self, now_ms, clock, &tasks);
            for s in &mut self.slots {
                s.app.tick(&mut ctx);
                if s.app.take_dirty() {
                    s.content_dirty = true;
                }
            }
        }
        self.process_outbox(now_ms, clock);
        self.damage_geometry(&before);
        let expired = self.toasts.len();
        self.toasts.retain(|t| t.2 > now_ms);
        if self.toasts.len() != expired {
            self.damage
                .push(shell::toasts_rect(self.width, self.height));
        }

        let (w, h) = (self.width, self.height);
        let screen = Rect::new(0, 0, w as i32, h as i32);
        let view = hud::sphere_view(w, h, now_ms);
        let pulse = self.assistant.pulse(now_ms);
        let sphere = ParticleCloud::bounds(&view);
        let clock_rect = hud::clock_rect(w, h);
        let message_rect = hud::message_rect(w, h);
        let status_rect = shell::status_rect(w, h);

        let mut dirty = core::mem::take(&mut self.damage);
        let full = self.full_redraw;
        if full {
            dirty = Dirty::default();
            dirty.push(screen);
        }
        let fullscreen_overlay = self.overlay_is_fullscreen();
        if !fullscreen_overlay && !self.wm.covers(&sphere) {
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
        if self.drawn_stats != self.stats_version {
            dirty.push(status_rect);
        }
        let toolbar = (self.toolbar_items(), self.toolbar_hover);
        if self.drawn_toolbar.as_ref() != Some(&toolbar) {
            dirty.push(shell::toolbar_area());
        }
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
        self.drawn_toasts = toasts;

        if core::mem::take(&mut self.screenshot) {
            self.save_screenshot(frame, clock, now_ms);
        }
        dirty
    }

    fn overlay_rect(&self) -> Rect {
        let (w, h) = (self.width, self.height);
        match &self.overlay {
            Overlay::Start(_) => StartMenu::rect(w, h).inset(-4),
            Overlay::Switcher { order, .. } => shell::switcher_rect(w, h, order.len()).inset(-8),
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
            Overlay::Lock => {
                shell::draw_lock(frame, w, h, clock, &mut self.lock_font);
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
            hud::draw_clock(frame, &mut self.clock_face, clock);
        }
        if dirty.touches(&hud::message_rect(w, h)) {
            let speaking = self.assistant.is_speaking(now_ms);
            hud::draw_message(frame, self.assistant.visible_text(now_ms), speaking);
        }
        if dirty.touches(&shell::status_rect(w, h)) {
            let disk = self
                .fs
                .as_ref()
                .map(|fs| (fs.free_bytes(), fs.total_bytes()));
            shell::draw_status(frame, w, h, &self.stats, &self.history, disk);
        }
        // Ventanas, de atrás hacia adelante.
        for win in self.wm.windows().iter().filter(|w| w.visible()) {
            if !dirty.touches(&win.rect) {
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
                frame.blit(&src, win.rect.x, win.rect.y);
            }
        }
        shell::draw_toolbar(frame, toolbar, self.toolbar_hover);
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

    fn save_screenshot(&mut self, frame: &Canvas<'_>, clock: Option<DateTime>, now_ms: u64) {
        let Some(fs) = self.fs.as_mut() else {
            self.notify("No hay disco para guardar la captura.", true, now_ms);
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
        let data = crate::bmp::encode(frame);
        match fs.write_file(&path, &data, ts) {
            Ok(()) => {
                self.logs.push(format!("CAPTURA {path}"));
                self.notify(format!("Captura guardada en {path}"), false, now_ms);
            }
            Err(e) => self.notify(format!("No se pudo guardar la captura: {e}"), true, now_ms),
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
