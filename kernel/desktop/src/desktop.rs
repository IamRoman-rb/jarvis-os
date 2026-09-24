//! El escritorio: modos (JARVIS / Archivos), teclado, mouse, redibujo y cursor.
//!
//! El kernel le pasa eventos y le pide frames; no sabe nada de apps. Todo esto corre igual en el
//! host (con un disco en memoria), así que se testea sin QEMU.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem, Timestamp};
use jarvis_gfx::clock::DateTime;
use jarvis_gfx::scene::{Dirty, Scene};
use jarvis_gfx::{Canvas, Rect, hud};

use crate::cursor;
use crate::files::{FilesApp, SHORTCUTS};
use crate::files_view::{self, Action, Hit, Layout};
use crate::input::{Event, Key, MousePacket};

pub const GREETING: &str = "Sistema en línea. ¿En qué te ayudo?";
/// Frases de demo (Espacio o Enter en JARVIS). En K4 las respuestas van a venir de Claude.
pub const DEMO_PHRASES: [&str; 4] = [
    "Todos los sistemas funcionan con normalidad.",
    "Tu disco está montado: Tab abre el gestor de archivos.",
    "Todavía no tengo voz propia, pero ya sé cómo moverme cuando hable.",
    "Cuando me conectes con Claude, voy a poder responderte de verdad.",
];
const DOUBLE_CLICK_MS: u64 = 450;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Jarvis,
    Files,
}

type ClockKey = Option<(u16, u8, u8, u8, u8)>;

pub struct Desktop<D: BlockDevice> {
    scene: Scene,
    files: FilesApp,
    fs: Option<FileSystem<D>>,
    mode: Mode,
    width: usize,
    height: usize,
    cursor: (i32, i32),
    cursor_visible: bool,
    cursor_drawn: Option<Rect>,
    left_down: bool,
    last_click: Option<(u64, usize)>,
    logs: Vec<String>,
    phrase: usize,
    /// El próximo frame de Archivos se dibuja completo (recién se cambió de modo).
    full_redraw: bool,
    files_clock: Option<ClockKey>,
}

fn timestamp(clock: Option<DateTime>) -> Timestamp {
    clock.map_or(Timestamp::EPOCH, |t| Timestamp {
        year: t.year,
        month: t.month,
        day: t.day,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
    })
}

impl<D: BlockDevice> Desktop<D> {
    pub fn new(width: usize, height: usize, particles: usize, fs: Option<FileSystem<D>>) -> Self {
        Desktop {
            scene: Scene::new(width, height, particles),
            files: FilesApp::new(),
            fs,
            mode: Mode::Jarvis,
            width,
            height,
            cursor: (width as i32 / 2, height as i32 / 2),
            cursor_visible: false,
            cursor_drawn: None,
            left_down: false,
            last_click: None,
            logs: Vec::new(),
            phrase: 0,
            full_redraw: true,
            files_clock: None,
        }
    }

    pub fn draw_background(&self, bg: &mut Canvas<'_>, info: &[(&str, &str)]) {
        self.scene.draw_background(bg, info);
    }

    /// JARVIS saluda al arrancar.
    pub fn start(&mut self, now_ms: u64) {
        self.scene.assistant.say(GREETING, now_ms);
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn files(&self) -> &FilesApp {
        &self.files
    }

    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    pub fn into_fs(self) -> Option<FileSystem<D>> {
        self.fs
    }

    /// Líneas para el puerto serie (el kernel las imprime; los tests las verifican).
    pub fn take_logs(&mut self) -> Vec<String> {
        core::mem::take(&mut self.logs)
    }

    pub fn set_mode(&mut self, mode: Mode, now_ms: u64) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        match mode {
            Mode::Files => {
                self.logs.push("MODO ARCHIVOS".into());
                self.full_redraw = true;
                if let Some(fs) = self.fs.as_mut() {
                    let cwd = self.files.cwd.clone();
                    let selected = self.files.selected_entry().map(|e| e.name.clone());
                    self.files.open(fs, &cwd, now_ms, &mut self.logs);
                    self.files.reload(fs, selected.as_deref());
                }
                self.keep_selection_visible();
            }
            Mode::Jarvis => {
                self.logs.push("MODO JARVIS".into());
                self.scene.invalidate();
            }
        }
    }

    pub fn handle(&mut self, event: Event, now_ms: u64, clock: Option<DateTime>) {
        match event {
            Event::Key(key) => self.key(key, now_ms, clock),
            Event::Mouse(packet) => self.mouse(packet, now_ms, clock),
        }
    }

    fn key(&mut self, key: Key, now_ms: u64, clock: Option<DateTime>) {
        if key == Key::Tab && self.files.dialog.is_none() {
            let next = if self.mode == Mode::Jarvis {
                Mode::Files
            } else {
                Mode::Jarvis
            };
            self.set_mode(next, now_ms);
            return;
        }
        match self.mode {
            Mode::Jarvis => {
                if matches!(key, Key::Char(' ') | Key::Enter) {
                    self.speak_demo(now_ms);
                }
            }
            Mode::Files => {
                let handled = match self.fs.as_mut() {
                    Some(fs) => {
                        self.files
                            .handle_key(fs, key, timestamp(clock), now_ms, &mut self.logs)
                    }
                    None => false,
                };
                if !handled && key == Key::Escape {
                    self.set_mode(Mode::Jarvis, now_ms);
                }
                self.keep_selection_visible();
            }
        }
    }

    fn speak_demo(&mut self, now_ms: u64) {
        let text = DEMO_PHRASES[self.phrase % DEMO_PHRASES.len()];
        self.phrase += 1;
        self.scene.assistant.say(text, now_ms);
        self.logs.push(format!("JARVIS_HABLA: {text}"));
    }

    fn mouse(&mut self, p: MousePacket, now_ms: u64, clock: Option<DateTime>) {
        self.cursor.0 = (self.cursor.0 + p.dx).clamp(0, self.width as i32 - 1);
        self.cursor.1 = (self.cursor.1 + p.dy).clamp(0, self.height as i32 - 1);
        self.cursor_visible = true;
        let pressed = p.left && !self.left_down;
        self.left_down = p.left;
        if pressed {
            self.click(now_ms, clock);
        }
    }

    fn click(&mut self, now_ms: u64, clock: Option<DateTime>) {
        let (x, y) = self.cursor;
        if let Some(icon) = hud::toolbar_hit(x, y) {
            match icon {
                hud::TOOLBAR_FILES => self.set_mode(Mode::Files, now_ms),
                hud::TOOLBAR_JARVIS => self.set_mode(Mode::Jarvis, now_ms),
                _ => {}
            }
            return;
        }
        if self.mode != Mode::Files {
            return;
        }
        let layout = Layout::new(&self.files, self.width, self.height);
        let hit = layout.hit(&self.files, x, y);
        let Some(fs) = self.fs.as_mut() else { return };
        let files = &mut self.files;
        let log = &mut self.logs;
        match hit {
            Hit::Back => files.go_back(fs, now_ms, log),
            Hit::Up => files.go_up(fs, now_ms, log),
            Hit::Action(Action::NewFolder) => files.start_new_folder(),
            Hit::Action(Action::NewFile) => files.start_new_file(),
            Hit::Action(Action::Rename) => files.start_rename(),
            Hit::Action(Action::Delete) => files.start_delete(now_ms, log),
            Hit::Action(Action::EmptyTrash) => files.start_empty_trash(),
            Hit::Shortcut(i) => files.navigate(fs, SHORTCUTS[i].1, now_ms, log),
            Hit::Row(i) => {
                let double = self
                    .last_click
                    .is_some_and(|(t, row)| row == i && now_ms - t <= DOUBLE_CLICK_MS);
                files.select(fs, i, log);
                if double {
                    files.enter_selected(fs, now_ms, log);
                    self.last_click = None;
                } else {
                    self.last_click = Some((now_ms, i));
                }
            }
            Hit::DialogAccept => files.accept_dialog(fs, timestamp(clock), now_ms, log),
            Hit::DialogCancel => files.cancel_dialog(),
            Hit::Nothing => {}
        }
        self.keep_selection_visible();
    }

    fn keep_selection_visible(&mut self) {
        let rows = Layout::new(&self.files, self.width, self.height).visible_rows();
        self.files.ensure_visible(rows);
    }

    /// Compone el frame de `now_ms` en `frame` y devuelve qué zonas cambiaron.
    pub fn render(
        &mut self,
        frame: &mut Canvas<'_>,
        bg: &Canvas<'_>,
        now_ms: u64,
        clock: Option<DateTime>,
    ) -> Dirty {
        if self.scene.assistant.take_finished(now_ms) {
            self.logs.push("JARVIS_REPOSO".into());
        }
        self.files.tick(now_ms);
        match self.mode {
            Mode::Jarvis => self.scene.render(frame, bg, now_ms, clock),
            Mode::Files => self.render_files(frame, bg, clock),
        }
    }

    fn render_files(
        &mut self,
        frame: &mut Canvas<'_>,
        bg: &Canvas<'_>,
        clock: Option<DateTime>,
    ) -> Dirty {
        let (w, h) = (self.width, self.height);
        let layout = Layout::new(&self.files, w, h);
        let clock_rect = hud::clock_rect(w, h);
        let clock_key = clock.map(|t| (t.year, t.month, t.day, t.hour, t.minute));

        let mut dirty = Dirty::default();
        let full = self.full_redraw;
        if full {
            dirty.push(Rect::new(0, 0, w as i32, h as i32));
        } else {
            if self.files.dirty {
                dirty.push(layout.window);
            }
            if self.files_clock != Some(clock_key) {
                dirty.push(clock_rect);
            }
        }
        for r in dirty.iter() {
            frame.copy_from(bg, r);
        }
        frame.set_clip(dirty.iter());
        if dirty.touches(&layout.window) {
            files_view::draw(frame, &self.files, &layout, self.fs.is_some());
        }
        if dirty.touches(&clock_rect) {
            hud::draw_clock(frame, clock);
        }
        if full {
            hud::draw_toolbar(frame, hud::TOOLBAR_FILES);
        }
        frame.clear_clip();

        self.files.dirty = false;
        self.full_redraw = false;
        self.files_clock = Some(clock_key);
        dirty
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
