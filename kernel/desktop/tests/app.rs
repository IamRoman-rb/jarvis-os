//! La app Archivos manejada con eventos, como la usaría una persona, sobre un disco en memoria.
//! Después de cada flujo, el disco se abre con `fatfs` para verificar que el cambio quedó escrito.

use std::io::{Cursor, Write};

use fatfs::{FatType, FormatVolumeOptions, FsOptions};
use jarvis_desktop::files::Dialog;
use jarvis_desktop::files_view::{Action, Layout};
use jarvis_desktop::{Desktop, Event, Key, Mode, MousePacket};
use jarvis_fs::{FileSystem, MemDisk};
use jarvis_gfx::clock::DateTime;
use jarvis_gfx::{Canvas, PixelFormat, Rect, hud};

const W: usize = 1280;
const H: usize = 800;
const CLOCK: Option<DateTime> = Some(DateTime {
    year: 2026,
    month: 9,
    day: 23,
    hour: 23,
    minute: 5,
    second: 0,
});

fn seeded_image() -> Vec<u8> {
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

fn desktop() -> Desktop<MemDisk> {
    let fs = FileSystem::mount(MemDisk::new(seeded_image())).unwrap();
    Desktop::new(W, H, 300, Some(fs))
}

/// Los datos del disco, releídos con `fatfs`.
fn fatfs_exists(d: Desktop<MemDisk>, path: &str) -> bool {
    let img = d.into_fs().unwrap().into_device().into_inner();
    let mut disk = Cursor::new(img);
    let fs = fatfs::FileSystem::new(&mut disk, FsOptions::new()).unwrap();
    let path = path.trim_start_matches('/');
    fs.root_dir().open_dir(path).is_ok() || fs.root_dir().open_file(path).is_ok()
}

struct Driver {
    d: Desktop<MemDisk>,
    now: u64,
}

impl Driver {
    fn new() -> Self {
        Driver {
            d: desktop(),
            now: 1000,
        }
    }

    fn key(&mut self, k: Key) {
        self.now += 10;
        self.d.handle(Event::Key(k), self.now, CLOCK);
    }

    fn keys(&mut self, ks: &[Key]) {
        for &k in ks {
            self.key(k);
        }
    }

    fn type_text(&mut self, s: &str) {
        for c in s.chars() {
            self.key(Key::Char(c));
        }
    }

    fn clear_input(&mut self) {
        for _ in 0..80 {
            self.key(Key::Backspace);
        }
    }

    fn click_at(&mut self, x: i32, y: i32, gap_ms: u64) {
        let (cx, cy) = self.d.cursor();
        self.now += gap_ms;
        self.d.handle(
            Event::Mouse(MousePacket {
                dx: x - cx,
                dy: y - cy,
                ..Default::default()
            }),
            self.now,
            CLOCK,
        );
        self.d.handle(
            Event::Mouse(MousePacket {
                left: true,
                ..Default::default()
            }),
            self.now,
            CLOCK,
        );
        self.d
            .handle(Event::Mouse(MousePacket::default()), self.now, CLOCK);
    }

    fn click(&mut self, r: Rect) {
        self.click_at(r.x + r.w / 2, r.y + r.h / 2, 1000);
    }

    fn layout(&self) -> Layout {
        Layout::new(self.d.files(), W, H)
    }

    fn names(&self) -> Vec<String> {
        self.d
            .files()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    fn select_named(&mut self, name: &str) {
        let target = self
            .names()
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no está {name}"));
        self.key(Key::Home);
        for _ in 0..target {
            self.key(Key::Down);
        }
        assert_eq!(self.d.files().selected_entry().unwrap().name, name);
    }

    fn logs(&mut self) -> Vec<String> {
        self.d.take_logs()
    }
}

#[test]
fn tab_abre_archivos_en_la_raiz_ordenado() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    assert_eq!(t.d.mode(), Mode::Files);
    let logs = t.logs();
    assert!(logs.contains(&"MODO ARCHIVOS".into()));
    assert!(logs.contains(&"ARCHIVOS_ABIERTO /".into()));
    // Carpetas primero, en orden alfabético; después los archivos.
    assert_eq!(
        t.names(),
        [
            "Documentos",
            "Facultad",
            "Imágenes",
            "Papelera",
            "Proyectos",
            "LEAME.txt"
        ]
    );
    t.key(Key::Escape);
    assert_eq!(t.d.mode(), Mode::Jarvis);
}

#[test]
fn navegar_con_el_teclado_y_vista_previa() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    assert_eq!(t.d.files().cwd, "/Documentos");
    t.select_named("notas.txt");
    match &t.d.files().preview {
        Some(jarvis_desktop::files::Preview::Text(lines)) => {
            assert_eq!(lines, &["primera linea", "segunda linea"])
        }
        _ => panic!("tendría que haber vista previa de texto"),
    }
    t.key(Key::Backspace);
    assert_eq!(t.d.files().cwd, "/");
    assert_eq!(
        t.d.files().selected_entry().unwrap().name,
        "Documentos",
        "al subir queda seleccionada la carpeta de la que venía"
    );
    t.key(Key::Left); // atrás en el historial
    assert_eq!(t.d.files().cwd, "/Documentos");
}

#[test]
fn f7_crea_una_carpeta_en_el_disco() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.key(Key::F(7));
    assert!(matches!(t.d.files().dialog, Some(Dialog::NewFolder(_))));
    t.type_text("Tareas");
    t.key(Key::Enter);
    assert!(t.d.files().dialog.is_none());
    assert!(t.logs().contains(&"ARCHIVOS_CREADO /Tareas".into()));
    assert_eq!(
        t.d.files().selected_entry().unwrap().name,
        "Tareas",
        "queda seleccionada"
    );
    assert!(fatfs_exists(t.d, "/Tareas"));
}

#[test]
fn un_nombre_repetido_avisa_y_deja_el_dialogo_abierto() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.key(Key::F(7));
    t.type_text("documentos"); // ya existe (FAT no distingue mayúsculas)
    t.key(Key::Enter);
    assert!(
        matches!(t.d.files().dialog, Some(Dialog::NewFolder(_))),
        "se puede corregir el nombre"
    );
    let toast = t.d.files().toast.as_ref().unwrap();
    assert!(toast.error);
    assert!(t.logs().iter().any(|l| l.starts_with("ARCHIVOS_ERROR")));
    t.key(Key::Escape);
    assert!(t.d.files().dialog.is_none());
    assert_eq!(t.d.mode(), Mode::Files, "Esc cierra el diálogo, no la app");
}

#[test]
fn f2_renombra() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    t.select_named("notas.txt");
    t.key(Key::F(2));
    t.clear_input();
    t.type_text("apuntes de SO.txt");
    t.key(Key::Enter);
    assert!(t.names().contains(&"apuntes de SO.txt".into()));
    assert!(!t.names().contains(&"notas.txt".into()));
    assert!(fatfs_exists(t.d, "/Documentos/apuntes de SO.txt"));
}

#[test]
fn supr_manda_a_la_papelera_y_ahi_borra_definitivo() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("LEAME.txt");
    t.key(Key::Delete);
    assert!(matches!(t.d.files().dialog, Some(Dialog::ConfirmTrash(_))));
    t.key(Key::Enter);
    assert!(!t.names().contains(&"LEAME.txt".into()));
    assert!(
        t.logs()
            .contains(&"ARCHIVOS_PAPELERA /LEAME.txt -> /Papelera/LEAME.txt".into())
    );

    // Otro con el mismo nombre: en la Papelera queda como "LEAME (2).txt".
    t.key(Key::F(6));
    t.clear_input();
    t.type_text("LEAME.txt");
    t.key(Key::Enter);
    t.select_named("LEAME.txt");
    t.keys(&[Key::Delete, Key::Enter]);
    assert!(
        t.logs()
            .iter()
            .any(|l| l.ends_with("-> /Papelera/LEAME (2).txt"))
    );

    // La Papelera no se puede mandar a la Papelera.
    t.select_named("Papelera");
    t.key(Key::Delete);
    assert!(t.d.files().dialog.is_none());

    // Adentro de la Papelera, Supr borra definitivo (con confirmación).
    t.key(Key::Enter);
    assert_eq!(t.d.files().cwd, "/Papelera");
    assert_eq!(t.names(), ["LEAME (2).txt", "LEAME.txt"]);
    t.select_named("LEAME.txt");
    t.key(Key::Delete);
    assert!(matches!(t.d.files().dialog, Some(Dialog::ConfirmDelete(_))));
    t.key(Key::Enter);
    assert_eq!(t.names(), ["LEAME (2).txt"]);

    // Vaciar con el botón (mouse).
    let vaciar = t
        .layout()
        .action_rect(Action::EmptyTrash)
        .expect("en la Papelera está el botón VACIAR");
    t.click(vaciar);
    assert!(matches!(
        t.d.files().dialog,
        Some(Dialog::ConfirmEmptyTrash)
    ));
    let (_, aceptar) = t.layout().dialog_buttons();
    t.click(aceptar);
    assert!(t.names().is_empty());
    assert!(t.logs().contains(&"ARCHIVOS_PAPELERA_VACIA".into()));
    assert!(!fatfs_exists(t.d, "/Papelera/LEAME (2).txt"));
}

#[test]
fn mouse_selecciona_y_doble_clic_abre() {
    let mut t = Driver::new();
    // Desde JARVIS, clic en el ícono de la carpeta de la barra.
    let r = hud::toolbar_rect();
    t.click_at(r.x + 9 + hud::TOOLBAR_FILES * 30 + 15, r.y + r.h / 2, 1000);
    assert_eq!(t.d.mode(), Mode::Files);
    t.logs();

    let fila = t.layout().row_rect(1); // "Facultad"
    t.click(fila);
    assert_eq!(t.d.files().selected_entry().unwrap().name, "Facultad");
    assert!(t.logs().contains(&"ARCHIVOS_SELECCION Facultad".into()));
    t.click_at(fila.x + fila.w / 2, fila.y + fila.h / 2, 200); // segundo clic rápido
    assert_eq!(t.d.files().cwd, "/Facultad");

    // Acceso rápido del panel lateral.
    let docs = t.layout().shortcut_rect(1);
    t.click(docs);
    assert_eq!(t.d.files().cwd, "/Documentos");

    // Clic en el chat de la barra: vuelve a JARVIS.
    t.click_at(r.x + 9 + hud::TOOLBAR_JARVIS * 30 + 15, r.y + r.h / 2, 1000);
    assert_eq!(t.d.mode(), Mode::Jarvis);
}

#[test]
fn en_jarvis_espacio_habla() {
    let mut t = Driver::new();
    t.key(Key::Char(' '));
    assert!(t.logs().iter().any(|l| l.starts_with("JARVIS_HABLA: ")));
}

fn buffers() -> (Vec<u8>, Vec<u8>) {
    (vec![0u8; W * H * 4], vec![0u8; W * H * 4])
}

fn canvas(buf: &mut [u8]) -> Canvas<'_> {
    Canvas::new(buf, W, H, W, 4, PixelFormat::Bgr).unwrap()
}

/// Dibujar por partes (solo lo que cambia) tiene que dar lo mismo que redibujar todo.
#[test]
fn render_incremental_igual_a_redibujar_todo() {
    let mut t = Driver::new();
    let (mut bg_buf, mut frame_buf) = buffers();
    let mut bg = canvas(&mut bg_buf);
    t.d.draw_background(&mut bg, &[("NÚCLEO", "jarvis")]);
    {
        let mut frame = canvas(&mut frame_buf);
        t.d.render(&mut frame, &bg, t.now, CLOCK);
        t.key(Key::Tab);
        t.d.render(&mut frame, &bg, t.now, CLOCK);
        for k in [Key::Down, Key::Down, Key::Enter, Key::F(7)] {
            t.key(k);
            t.d.render(&mut frame, &bg, t.now, CLOCK);
        }
        t.type_text("x");
        t.d.render(&mut frame, &bg, t.now, CLOCK);
    }

    // Forzar un redibujado completo del mismo estado.
    let mut full_buf = vec![0u8; W * H * 4];
    t.d.set_mode(Mode::Jarvis, t.now);
    t.d.set_mode(Mode::Files, t.now);
    t.d.render(&mut canvas(&mut full_buf), &bg, t.now, CLOCK);
    assert!(frame_buf == full_buf, "el render incremental dejó restos");
}

#[test]
fn sin_disco_no_rompe() {
    let mut d: Desktop<MemDisk> = Desktop::new(W, H, 300, None);
    d.handle(Event::Key(Key::Tab), 10, CLOCK);
    d.handle(Event::Key(Key::F(7)), 20, CLOCK);
    let (mut bg_buf, mut frame_buf) = buffers();
    let bg = canvas(&mut bg_buf);
    let mut frame = canvas(&mut frame_buf);
    d.render(&mut frame, &bg, 30, CLOCK);
    assert_eq!(d.mode(), Mode::Files);
}
