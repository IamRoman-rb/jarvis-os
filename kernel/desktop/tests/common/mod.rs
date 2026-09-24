//! Lo que comparten los tests: un disco sembrado con `fatfs`, un "conductor" que maneja el
//! escritorio con teclas y mouse como lo haría una persona, y verificaciones con `fatfs`.

#![allow(dead_code)]

use std::io::{Cursor, Read, Write};

use fatfs::{FatType, FormatVolumeOptions, FsOptions};
use jarvis_desktop::apps::App;
use jarvis_desktop::files::FilesApp;
use jarvis_desktop::files_view::Layout;
use jarvis_desktop::wm::WinId;
use jarvis_desktop::{AppKind, Desktop, Event, Key, Mods, MousePacket};
use jarvis_fs::{FileSystem, MemDisk};
use jarvis_gfx::clock::DateTime;
use jarvis_gfx::{Canvas, PixelFormat, Rect};

pub const W: usize = 1280;
pub const H: usize = 800;
pub const CLOCK: Option<DateTime> = Some(DateTime {
    year: 2026,
    month: 9,
    day: 23,
    hour: 23,
    minute: 5,
    second: 0,
});

pub fn seeded_image() -> Vec<u8> {
    let mut disk = Cursor::new(vec![0u8; 40 * 1024 * 1024]);
    let opts = FormatVolumeOptions::new()
        .fat_type(FatType::Fat32)
        .bytes_per_cluster(512)
        .volume_label(*b"JARVIS     ");
    fatfs::format_volume(&mut disk, opts).unwrap();
    {
        let fs = fatfs::FileSystem::new(&mut disk, FsOptions::new()).unwrap();
        let root = fs.root_dir();
        let docs = root.create_dir("Documentos").unwrap();
        docs.create_file("notas.txt")
            .unwrap()
            .write_all(b"primera linea\nsegunda linea")
            .unwrap();
        docs.create_file("Bienvenida.txt")
            .unwrap()
            .write_all(b"hola")
            .unwrap();
        docs.create_file("pagina.html")
            .unwrap()
            .write_all(b"<title>Local</title><h1>Hola disco</h1><a href='notas.txt'>notas</a>")
            .unwrap();
        for dir in ["Proyectos", "Facultad", "Imágenes", "Papelera"] {
            root.create_dir(dir).unwrap();
        }
        root.create_file("LEAME.txt")
            .unwrap()
            .write_all(b"leame")
            .unwrap();
    }
    disk.into_inner()
}

pub fn desktop() -> Desktop<MemDisk> {
    let fs = FileSystem::mount(MemDisk::new(seeded_image())).unwrap();
    Desktop::new(W, H, 300, Some(fs))
}

fn fatfs_open<R>(
    d: Desktop<MemDisk>,
    f: impl FnOnce(&fatfs::FileSystem<&mut Cursor<Vec<u8>>>) -> R,
) -> R {
    let img = d.into_fs().unwrap().into_device().into_inner();
    let mut disk = Cursor::new(img);
    let fs = fatfs::FileSystem::new(&mut disk, FsOptions::new()).unwrap();
    f(&fs)
}

/// ¿Existe `path` en el disco? (Lo verifica `fatfs`, no nuestro FAT32.)
pub fn fatfs_exists(d: Desktop<MemDisk>, path: &str) -> bool {
    fatfs_open(d, |fs| {
        let path = path.trim_start_matches('/');
        fs.root_dir().open_dir(path).is_ok() || fs.root_dir().open_file(path).is_ok()
    })
}

pub fn fatfs_read(d: Desktop<MemDisk>, path: &str) -> Option<Vec<u8>> {
    fatfs_open(d, |fs| {
        let mut f = fs.root_dir().open_file(path.trim_start_matches('/')).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    })
}

pub fn fatfs_list(d: Desktop<MemDisk>, dir: &str) -> Vec<String> {
    fatfs_open(d, |fs| {
        let dir = dir.trim_start_matches('/');
        let d = if dir.is_empty() {
            fs.root_dir()
        } else {
            fs.root_dir().open_dir(dir).unwrap()
        };
        d.iter()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .filter(|n| n != "." && n != "..")
            .collect()
    })
}

pub struct Driver {
    pub d: Desktop<MemDisk>,
    pub now: u64,
}

impl Driver {
    pub fn new() -> Self {
        Driver {
            d: desktop(),
            now: 1000,
        }
    }

    pub fn key(&mut self, k: Key) {
        self.now += 10;
        self.d.handle(Event::Key(k), self.now, CLOCK);
    }

    pub fn keys(&mut self, ks: &[Key]) {
        for &k in ks {
            self.key(k);
        }
    }

    pub fn mods(&mut self, m: Mods) {
        self.now += 10;
        self.d.handle(Event::Mods(m), self.now, CLOCK);
    }

    /// Apretar una combinación (Win+D, Ctrl+C…) y soltar los modificadores.
    pub fn combo(&mut self, m: Mods, k: Key) {
        self.mods(m);
        self.key(k);
        self.mods(Mods::NONE);
    }

    pub fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(Key::Char(c));
        }
    }

    pub fn clear_input(&mut self) {
        for _ in 0..80 {
            self.key(Key::Backspace);
        }
    }

    pub fn move_to(&mut self, x: i32, y: i32, left: bool) {
        let (cx, cy) = self.d.cursor();
        self.now += 5;
        self.d.handle(
            Event::Mouse(MousePacket {
                dx: x - cx,
                dy: y - cy,
                left,
                ..Default::default()
            }),
            self.now,
            CLOCK,
        );
    }

    pub fn click_at(&mut self, x: i32, y: i32, gap_ms: u64) {
        self.now += gap_ms;
        self.move_to(x, y, false);
        self.move_to(x, y, true);
        self.move_to(x, y, false);
    }

    pub fn click(&mut self, r: Rect) {
        self.click_at(r.x + r.w / 2, r.y + r.h / 2, 1000);
    }

    pub fn double_click(&mut self, r: Rect) {
        self.click(r);
        self.click_at(r.x + r.w / 2, r.y + r.h / 2, 100);
    }

    pub fn wheel(&mut self, delta: i32) {
        self.now += 10;
        self.d.handle(
            Event::Mouse(MousePacket {
                wheel: delta,
                ..Default::default()
            }),
            self.now,
            CLOCK,
        );
    }

    pub fn window(&self, kind: AppKind) -> Rect {
        let id = self.d.window_of(kind).expect("la ventana está abierta");
        self.d.window_manager().get(id).unwrap().rect
    }

    pub fn id(&self, kind: AppKind) -> WinId {
        self.d.window_of(kind).unwrap()
    }

    pub fn files(&self) -> &FilesApp {
        match self.d.app(AppKind::Files) {
            Some(App::Files(w)) => &w.app,
            _ => panic!("Archivos no está abierto"),
        }
    }

    /// El diseño de la ventana de Archivos en coordenadas de la pantalla (para hacer clic).
    pub fn layout(&self) -> Layout {
        let id = self.id(AppKind::Files);
        let win = self.d.window_manager().get(id).unwrap();
        let c = win.content();
        Layout::new(
            self.files(),
            Rect::new(win.rect.x + c.x, win.rect.y + c.y, c.w, c.h),
        )
    }

    pub fn names(&self) -> Vec<String> {
        self.files()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    pub fn select_named(&mut self, name: &str) {
        let target = self
            .names()
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no está {name}: {:?}", self.names()));
        self.key(Key::Home);
        for _ in 0..target {
            self.key(Key::Down);
        }
        assert_eq!(self.files().selected_entry().unwrap().name, name);
    }

    pub fn logs(&mut self) -> Vec<String> {
        self.d.take_logs()
    }

    /// Un frame (con buffers descartables).
    pub fn frame(&mut self) {
        let (mut bg, mut fr) = buffers();
        let mut bgc = canvas(&mut bg);
        let mut frame = canvas(&mut fr);
        self.d.render(&mut frame, &mut bgc, self.now, CLOCK);
    }
}

pub fn buffers() -> (Vec<u8>, Vec<u8>) {
    (vec![0u8; W * H * 4], vec![0u8; W * H * 4])
}

pub fn canvas(buf: &mut [u8]) -> Canvas<'_> {
    Canvas::new(buf, W, H, W, 4, PixelFormat::Bgr).unwrap()
}
