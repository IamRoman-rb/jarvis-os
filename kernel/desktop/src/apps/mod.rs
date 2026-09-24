//! Las apps que viven dentro de las ventanas.
//!
//! Cada app sabe dibujarse en una zona (`content`, relativa a su ventana) y reaccionar a teclas y
//! clics. No sabe dónde está su ventana en la pantalla ni si está encima de otra: de eso se ocupa
//! el gestor de ventanas. Lo que necesita del resto del sistema lo recibe en un [`Ctx`] (disco,
//! hora, pedidos al kernel) y lo que quiere pedir lo deja en el [`Outbox`].

pub mod browser;
pub mod console;
pub mod editor;
pub mod files;
pub mod monitor;
pub mod music;
pub mod settings;
pub mod terminal;
pub mod viewer;

use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem, Timestamp};
use jarvis_gfx::clock::DateTime;
use jarvis_gfx::hud::Icon;
use jarvis_gfx::{Canvas, Rect};

use crate::i18n::tr;
use crate::input::{Key, Mods};
use crate::system::{AppKind, History, HttpResponse, Outbox, SystemStats};
use crate::wm::WinId;

/// Una ventana abierta, como la ve el monitor del sistema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInfo {
    pub id: WinId,
    pub title: String,
    pub kind: AppKind,
    pub minimized: bool,
}

/// Información del sistema para dibujar (solo lectura).
pub struct SysView<'a> {
    pub stats: &'a SystemStats,
    pub history: &'a History,
    pub tasks: &'a [TaskInfo],
    /// (etiqueta, libres, total) del disco, si hay.
    pub disk: Option<(&'a str, u64, u64)>,
    pub now_ms: u64,
    pub clock: Option<DateTime>,
    /// Tamaño de la pantalla.
    pub screen: (usize, usize),
}

/// Lo que recibe una app cuando maneja un evento.
pub struct Ctx<'a, D: BlockDevice> {
    pub fs: Option<&'a mut FileSystem<D>>,
    pub now_ms: u64,
    pub clock: Option<DateTime>,
    pub log: &'a mut Vec<String>,
    pub out: &'a mut Outbox,
    pub stats: &'a SystemStats,
    pub tasks: &'a [TaskInfo],
    pub config: &'a crate::config::Config,
}

impl<D: BlockDevice> Ctx<'_, D> {
    /// La hora para las fechas de los archivos.
    pub fn timestamp(&self) -> Timestamp {
        timestamp(self.clock)
    }
}

pub fn timestamp(clock: Option<DateTime>) -> Timestamp {
    clock.map_or(Timestamp::EPOCH, |t| Timestamp {
        year: t.year,
        month: t.month,
        day: t.day,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
    })
}

/// Un clic: dónde (relativo a la ventana) y si fue doble.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Click {
    pub x: i32,
    pub y: i32,
    pub double: bool,
    pub right: bool,
}

// Una sola por ventana: que el navegador sea más grande que las demás no importa.
#[allow(clippy::large_enum_variant)]
pub enum App {
    Files(files::FilesWindow),
    Monitor(monitor::Monitor),
    Console(console::Console),
    Editor(editor::Editor),
    Music(music::Music),
    Viewer(viewer::Viewer),
    Browser(browser::Browser),
    Terminal(terminal::Terminal),
    Settings(settings::Settings),
}

macro_rules! each {
    ($self:expr, $a:ident => $e:expr) => {
        match $self {
            App::Files($a) => $e,
            App::Monitor($a) => $e,
            App::Console($a) => $e,
            App::Editor($a) => $e,
            App::Music($a) => $e,
            App::Viewer($a) => $e,
            App::Browser($a) => $e,
            App::Terminal($a) => $e,
            App::Settings($a) => $e,
        }
    };
}

impl App {
    pub fn kind(&self) -> AppKind {
        match self {
            App::Files(_) => AppKind::Files,
            App::Monitor(_) => AppKind::Monitor,
            App::Console(_) => AppKind::Console,
            App::Editor(_) => AppKind::Editor,
            App::Music(_) => AppKind::Music,
            App::Viewer(_) => AppKind::Viewer,
            App::Browser(_) => AppKind::Browser,
            App::Terminal(_) => AppKind::Terminal,
            App::Settings(_) => AppKind::Settings,
        }
    }

    pub fn title(&self) -> String {
        each!(self, a => a.title())
    }

    /// Tamaño con el que se abre (ancho, alto), incluida la barra de título.
    pub fn default_size(&self) -> (i32, i32) {
        match self {
            App::Files(_) => (1180, 600),
            App::Monitor(_) => (960, 600),
            App::Console(_) => (760, 460),
            App::Editor(_) => (820, 560),
            App::Music(_) => (640, 440),
            App::Viewer(_) => (760, 560),
            App::Browser(_) => (1060, 620),
            App::Terminal(_) => (860, 520),
            App::Settings(_) => (1000, 620),
        }
    }

    /// ¿Hay que volver a dibujarla? (Lo consulta el escritorio en cada frame.)
    pub fn take_dirty(&mut self) -> bool {
        each!(self, a => core::mem::take(&mut a.dirty))
    }

    pub fn set_dirty(&mut self) {
        each!(self, a => a.dirty = true)
    }

    pub fn draw(&mut self, c: &mut Canvas<'_>, content: Rect, sys: &SysView<'_>) {
        match self {
            App::Files(a) => a.draw(c, content, sys),
            App::Monitor(a) => a.draw(c, content, sys),
            App::Console(a) => a.draw(c, content),
            App::Editor(a) => a.draw(c, content, sys.now_ms),
            App::Music(a) => a.draw(c, content, sys.now_ms),
            App::Viewer(a) => a.draw(c, content),
            App::Browser(a) => a.draw(c, content, sys.now_ms),
            App::Terminal(a) => a.draw(c, content),
            App::Settings(a) => a.draw(c, content, sys),
        }
    }

    /// Devuelve `true` si la tecla se usó.
    pub fn key<D: BlockDevice>(
        &mut self,
        key: Key,
        mods: Mods,
        content: Rect,
        ctx: &mut Ctx<'_, D>,
    ) -> bool {
        match self {
            App::Files(a) => a.key(key, mods, content, ctx),
            App::Monitor(a) => a.key(key, ctx),
            App::Console(a) => a.key(key, mods, ctx),
            App::Editor(a) => a.key(key, mods, content, ctx),
            App::Music(a) => a.key(key, ctx),
            App::Viewer(a) => a.key(key, ctx),
            App::Browser(a) => a.key(key, mods, content, ctx),
            App::Terminal(a) => a.key(key, mods, ctx),
            App::Settings(a) => a.key(key, mods, ctx),
        }
    }

    pub fn click<D: BlockDevice>(&mut self, click: Click, content: Rect, ctx: &mut Ctx<'_, D>) {
        match self {
            App::Files(a) => a.click(click, content, ctx),
            App::Monitor(a) => a.click(click, content, ctx),
            App::Console(_) => {}
            App::Editor(a) => a.click(click, content),
            App::Music(a) => a.click(click, content, ctx),
            App::Viewer(_) => {}
            App::Browser(a) => a.click(click, content, ctx),
            App::Terminal(_) => {}
            App::Settings(a) => a.click(click, content, ctx),
        }
    }

    /// Rueda del mouse: positivo = hacia abajo.
    pub fn wheel<D: BlockDevice>(&mut self, delta: i32, content: Rect, ctx: &mut Ctx<'_, D>) {
        match self {
            App::Files(a) => a.wheel(delta, content, ctx),
            App::Editor(a) => a.wheel(delta, content),
            App::Browser(a) => a.wheel(delta, content),
            App::Console(a) => a.wheel(delta),
            App::Terminal(a) => a.wheel(delta),
            _ => {}
        }
    }

    /// Una vez por frame: animaciones, avisos que vencen, música.
    pub fn tick<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) {
        match self {
            App::Files(a) => a.tick(ctx.now_ms),
            App::Music(a) => a.tick(ctx),
            App::Editor(a) => a.tick(ctx.now_ms),
            App::Browser(a) => a.tick(ctx),
            App::Terminal(a) => a.tick(ctx),
            App::Settings(a) => a.tick(ctx),
            _ => {}
        }
    }

    pub fn net_response<D: BlockDevice>(
        &mut self,
        id: u32,
        result: &Result<HttpResponse, String>,
        ctx: &mut Ctx<'_, D>,
    ) {
        match self {
            App::Browser(b) => b.net_response(id, result, ctx),
            App::Terminal(t) => t.net_response(id, result, ctx),
            App::Settings(s) => s.net_response(id, result),
            _ => {}
        }
    }

    /// Se va a cerrar la ventana. `false` = todavía no (por ejemplo, cambios sin guardar).
    pub fn on_close<D: BlockDevice>(&mut self, ctx: &mut Ctx<'_, D>) -> bool {
        match self {
            App::Editor(a) => a.on_close(ctx),
            App::Music(a) => {
                a.stop(ctx);
                true
            }
            _ => true,
        }
    }

    pub fn icon(&self) -> Icon {
        icon_of(self.kind())
    }
}

pub fn icon_of(kind: AppKind) -> Icon {
    match kind {
        AppKind::Console => Icon::Mic,
        AppKind::Monitor => Icon::Gauge,
        AppKind::Files => Icon::Folder,
        AppKind::Music => Icon::Music,
        AppKind::Browser => Icon::Globe,
        AppKind::Editor => Icon::Document,
        AppKind::Viewer => Icon::Image,
        AppKind::Terminal => Icon::Terminal,
        AppKind::Settings => Icon::Gear,
    }
}

/// Nombre para mostrar (menú de inicio, Alt+Tab).
pub fn name_of(kind: AppKind) -> &'static str {
    match kind {
        AppKind::Console => tr("Consola JARVIS"),
        AppKind::Monitor => tr("Monitor del sistema"),
        AppKind::Files => tr("Archivos"),
        AppKind::Music => tr("Música"),
        AppKind::Browser => tr("Navegador"),
        AppKind::Editor => tr("Editor de texto"),
        AppKind::Viewer => tr("Visor de imágenes"),
        AppKind::Terminal => "Terminal",
        AppKind::Settings => tr("Configuración"),
    }
}
