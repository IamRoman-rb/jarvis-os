//! La app Archivos dentro de una ventana: conecta `FilesApp` (estado y operaciones) y
//! `files_view` (dibujo y clics) con el gestor de ventanas.

use alloc::format;
use alloc::string::String;

use jarvis_fs::BlockDevice;
use jarvis_gfx::{Canvas, Rect};

use super::{Click, Ctx, SysView};
use crate::files::{FilesApp, Kind, SHORTCUTS, join};
use crate::files_view::{self, Action, Hit, Layout};
use crate::input::{Key, Mods};
use crate::system::Launch;

pub struct FilesWindow {
    pub app: FilesApp,
    pub dirty: bool,
}

impl FilesWindow {
    pub fn new<D: BlockDevice>(path: &str, ctx: &mut Ctx<'_, D>) -> Self {
        let mut w = FilesWindow {
            app: FilesApp::new(),
            dirty: true,
        };
        if let Some(fs) = ctx.fs.as_deref_mut() {
            w.app.open(fs, path, ctx.now_ms, ctx.log);
        }
        w
    }

    pub fn title(&self) -> String {
        crate::i18n::trf("Archivos · {}", &[&self.app.cwd])
    }

    /// Ir a otra carpeta (cuando se pide abrir Archivos en una carpeta y ya estaba abierto).
    pub fn navigate<D: BlockDevice>(&mut self, path: &str, ctx: &mut Ctx<'_, D>) {
        if let Some(fs) = ctx.fs.as_deref_mut() {
            self.app.navigate(fs, path, ctx.now_ms, ctx.log);
        }
        self.sync();
    }

    fn sync(&mut self) {
        self.dirty |= core::mem::take(&mut self.app.dirty);
    }

    /// Si la app pidió abrir un archivo, se abre con la app que corresponde.
    fn open_requested<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        let Some(path) = self.app.open_request.take() else {
            return;
        };
        let Some(entry) = self
            .app
            .entries
            .iter()
            .find(|e| join(&self.app.cwd, &e.name) == path)
        else {
            return;
        };
        let lower = entry.name.to_ascii_lowercase();
        let launch = match Kind::of(entry, &path) {
            _ if lower.ends_with(".html") || lower.ends_with(".htm") => {
                Launch::Browse(format!("file://{path}"))
            }
            // Un script: se ejecuta en la terminal.
            _ if lower.ends_with(".sh") => Launch::Terminal(Some(format!("sh '{path}'"))),
            // Programas de Windows: la terminal muestra qué son y por qué no corren (todavía).
            _ if [".exe", ".msi", ".dll", ".com"]
                .iter()
                .any(|e| lower.ends_with(e)) =>
            {
                Launch::Terminal(Some(format!("file '{path}' && wine '{path}'")))
            }
            Kind::Text | Kind::Code => Launch::Edit(path),
            Kind::Image => Launch::View(path),
            Kind::Binary | Kind::Archive => Launch::Terminal(Some(format!("file '{path}'"))),
            _ => {
                ctx.out.notify(
                    format!("No hay una app para abrir \"{}\".", entry.name),
                    true,
                );
                return;
            }
        };
        ctx.log.push(format!("ARCHIVOS_ABRIR {launch:?}"));
        ctx.out.launch.push(launch);
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, content: Rect, sys: &SysView<'_>) {
        let layout = Layout::new(&self.app, content);
        files_view::draw(c, &self.app, &layout, sys.disk.is_some());
    }

    pub fn key<D: BlockDevice>(
        &mut self,
        key: Key,
        mods: Mods,
        content: Rect,
        ctx: &mut Ctx<'_, D>,
    ) -> bool {
        let now = ctx.timestamp();
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return false;
        };
        let handled = self.app.handle_key(fs, key, mods, now, ctx.now_ms, ctx.log);
        let rows = Layout::new(&self.app, content).visible_rows();
        self.app.ensure_visible(rows);
        self.sync();
        self.open_requested(ctx);
        handled
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        let layout = Layout::new(&self.app, content);
        let hit = layout.hit(&self.app, click.x, click.y);
        let now = ctx.timestamp();
        let Some(fs) = ctx.fs.as_deref_mut() else {
            return;
        };
        let (app, log, now_ms) = (&mut self.app, &mut *ctx.log, ctx.now_ms);
        match hit {
            Hit::Back => app.go_back(fs, now_ms, log),
            Hit::Up => app.go_up(fs, now_ms, log),
            Hit::Action(Action::NewFolder) => app.start_new_folder(),
            Hit::Action(Action::NewFile) => app.start_new_file(),
            Hit::Action(Action::Rename) => app.start_rename(),
            Hit::Action(Action::Delete) => app.start_delete(now_ms, log),
            Hit::Action(Action::EmptyTrash) => app.start_empty_trash(),
            Hit::Action(Action::Restore) => app.restore_selected(fs, now, now_ms, log),
            Hit::Action(Action::Paste) => app.paste(fs, now, now_ms, log),
            Hit::Shortcut(i) => app.navigate(fs, SHORTCUTS[i].1, now_ms, log),
            Hit::Sort(column) => app.sort_by(column),
            Hit::Row(i) => {
                app.select(fs, i, log);
                if click.double {
                    app.enter_selected(fs, now_ms, log);
                }
            }
            Hit::DialogAccept => app.accept_dialog(fs, now, now_ms, log),
            Hit::DialogCancel => app.cancel_dialog(),
            Hit::Nothing => {}
        }
        let rows = layout.visible_rows();
        self.app.ensure_visible(rows);
        self.sync();
        self.open_requested(ctx);
    }

    pub fn wheel<D: BlockDevice>(&mut self, delta: i32, content: Rect, _ctx: &mut Ctx<'_, D>) {
        let rows = Layout::new(&self.app, content).visible_rows();
        self.app.scroll_by(delta * 3, rows);
        self.sync();
    }

    pub fn tick(&mut self, now_ms: u64) {
        self.app.tick(now_ms);
        self.sync();
    }
}
