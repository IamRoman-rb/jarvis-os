//! La app Archivos manejada con teclas y mouse sobre un disco en memoria. Después de cada flujo,
//! el disco se abre con `fatfs` para verificar que el cambio quedó escrito.

mod common;

use common::*;
use image::ImageEncoder;
use jarvis_desktop::apps::App;
use jarvis_desktop::files::{Column, Dialog, Preview};
use jarvis_desktop::files_view::Action;
use jarvis_desktop::{AppKind, Key, Launch, Mods};

#[test]
fn tab_abre_archivos_en_la_raiz_ordenado() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    let logs = t.logs();
    assert!(logs.contains(&"VENTANA_ABIERTA Archivos".into()));
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
}

#[test]
fn navegar_con_el_teclado_y_vista_previa() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    assert_eq!(t.files().cwd, "/Documentos");
    t.select_named("notas.txt");
    match &t.files().preview {
        Some(Preview::Text(lines)) => assert_eq!(lines, &["primera linea", "segunda linea"]),
        _ => panic!("tendría que haber vista previa de texto"),
    }
    t.key(Key::Backspace);
    assert_eq!(t.files().cwd, "/");
    assert_eq!(
        t.files().selected_entry().unwrap().name,
        "Documentos",
        "al subir queda seleccionada la carpeta de la que venía"
    );
    t.combo(Mods::ALT, Key::Left); // atrás, como en el Explorador
    assert_eq!(t.files().cwd, "/Documentos");
    t.combo(Mods::ALT, Key::Up); // subir
    assert_eq!(t.files().cwd, "/");
}

#[test]
fn buscar_tipeando_selecciona() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.type_text("pro");
    assert_eq!(t.files().selected_entry().unwrap().name, "Proyectos");
    t.now += 2000; // pasó más de un segundo: búsqueda nueva
    t.type_text("l");
    assert_eq!(t.files().selected_entry().unwrap().name, "LEAME.txt");
}

#[test]
fn f7_crea_una_carpeta_en_el_disco() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.key(Key::F(7));
    assert!(matches!(t.files().dialog, Some(Dialog::NewFolder(_))));
    t.type_text("Tareas");
    t.key(Key::Enter);
    assert!(t.files().dialog.is_none());
    assert!(t.logs().contains(&"ARCHIVOS_CREADO /Tareas".into()));
    assert_eq!(t.files().selected_entry().unwrap().name, "Tareas");
    // Ctrl+Shift+N también crea una carpeta (como en Windows).
    t.combo(
        Mods {
            ctrl: true,
            shift: true,
            ..Mods::NONE
        },
        Key::Char('N'),
    );
    assert!(matches!(t.files().dialog, Some(Dialog::NewFolder(_))));
    t.key(Key::Escape);
    assert!(fatfs_exists(t.d, "/Tareas"));
}

#[test]
fn un_nombre_repetido_avisa_y_deja_el_dialogo_abierto() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.key(Key::F(7));
    t.type_text("documentos"); // ya existe (FAT no distingue mayúsculas)
    t.key(Key::Enter);
    assert!(matches!(t.files().dialog, Some(Dialog::NewFolder(_))));
    assert!(t.files().toast.as_ref().unwrap().error);
    assert!(t.logs().iter().any(|l| l.starts_with("ARCHIVOS_ERROR")));
    t.key(Key::Escape);
    assert!(t.files().dialog.is_none());
    assert_eq!(
        t.d.focused_app(),
        Some(AppKind::Files),
        "Esc cierra el diálogo, no la app"
    );
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
    assert!(fatfs_exists(t.d, "/Documentos/apuntes de SO.txt"));
}

#[test]
fn copiar_y_pegar_en_otra_carpeta() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    t.select_named("notas.txt");
    t.combo(Mods::CTRL, Key::Char('c'));
    t.combo(Mods::ALT, Key::Up);
    t.select_named("Proyectos");
    t.key(Key::Enter);
    t.combo(Mods::CTRL, Key::Char('v'));
    assert_eq!(t.names(), ["notas.txt"]);
    // Pegar otra vez en la misma carpeta: "notas (2).txt".
    t.combo(Mods::CTRL, Key::Char('v'));
    assert_eq!(t.names(), ["notas (2).txt", "notas.txt"]);
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "ARCHIVOS_PEGADO /Proyectos/notas (2).txt")
    );
    assert_eq!(
        fatfs_read(t.d, "/Proyectos/notas (2).txt").unwrap(),
        b"primera linea\nsegunda linea"
    );
}

#[test]
fn cortar_una_carpeta_la_mueve() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Facultad");
    t.combo(Mods::CTRL, Key::Char('x'));
    t.select_named("Documentos");
    t.key(Key::Enter);
    // El botón PEGAR aparece cuando hay algo copiado.
    let pegar = t.layout().action_rect(Action::Paste).expect("botón PEGAR");
    t.click(pegar);
    assert!(t.names().contains(&"Facultad".into()));
    assert!(t.files().clipboard.is_none(), "cortar se usa una sola vez");
    let raiz = fatfs_list(t.d, "/");
    assert!(!raiz.contains(&"Facultad".into()));
}

#[test]
fn papelera_restaurar_y_borrar_definitivo() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    t.select_named("notas.txt");
    t.keys(&[Key::Delete, Key::Enter]);
    assert!(!t.names().contains(&"notas.txt".into()));
    assert!(
        t.logs()
            .contains(&"ARCHIVOS_PAPELERA /Documentos/notas.txt -> /Papelera/notas.txt".into())
    );

    // En la Papelera se ve (sin el índice oculto ".origen") y se restaura a su carpeta.
    t.combo(Mods::ALT, Key::Up);
    t.select_named("Papelera");
    t.key(Key::Enter);
    assert_eq!(t.names(), ["notas.txt"]);
    let restaurar = t
        .layout()
        .action_rect(Action::Restore)
        .expect("botón RESTAURAR");
    t.click(restaurar);
    assert!(t.names().is_empty());
    assert!(
        t.logs()
            .contains(&"ARCHIVOS_RESTAURADO /Documentos/notas.txt".into())
    );

    // Ahora LEAME.txt: a la Papelera y ahí se borra definitivo con Supr (con confirmación).
    t.combo(Mods::ALT, Key::Up);
    t.select_named("LEAME.txt");
    t.keys(&[Key::Delete, Key::Enter]);
    t.select_named("Papelera");
    t.key(Key::Enter);
    t.select_named("LEAME.txt");
    t.key(Key::Delete);
    assert!(matches!(t.files().dialog, Some(Dialog::ConfirmDelete(_))));
    t.key(Key::Enter);
    assert!(t.names().is_empty());
    assert!(fatfs_exists(t.d, "/Documentos/notas.txt"));
}

#[test]
fn vaciar_la_papelera_con_el_mouse() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("LEAME.txt");
    t.keys(&[Key::Delete, Key::Enter]);
    t.select_named("Papelera");
    t.key(Key::Enter);
    let vaciar = t.layout().action_rect(Action::EmptyTrash).unwrap();
    t.click(vaciar);
    assert!(matches!(t.files().dialog, Some(Dialog::ConfirmEmptyTrash)));
    let (_, aceptar) = t.layout().dialog_buttons();
    t.click(aceptar);
    assert!(t.names().is_empty());
    assert!(t.logs().contains(&"ARCHIVOS_PAPELERA_VACIA".into()));
    assert!(!fatfs_exists(t.d, "/Papelera/LEAME.txt"));
}

#[test]
fn mouse_selecciona_doble_clic_abre_y_ordena_por_columnas() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.logs();
    let fila = t.layout().row_rect(1); // "Facultad"
    t.click(fila);
    assert_eq!(t.files().selected_entry().unwrap().name, "Facultad");
    assert!(t.logs().contains(&"ARCHIVOS_SELECCION Facultad".into()));
    t.double_click(fila);
    assert_eq!(t.files().cwd, "/Facultad");

    let docs = t.layout().shortcut_rect(1);
    t.click(docs);
    assert_eq!(t.files().cwd, "/Documentos");

    // Clic en "TAMAÑO": de menor a mayor (las carpetas siempre primero).
    let l = t.layout();
    let header = l.row_rect(0);
    t.click_at(header.x + header.w - 220, header.y - 15, 1000);
    assert_eq!(t.files().sort.0, Column::Size);
    assert_eq!(t.names(), ["Bienvenida.txt", "notas.txt", "pagina.html"]);
}

#[test]
fn doble_clic_en_un_texto_abre_el_editor_y_guarda() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    t.select_named("notas.txt");
    t.key(Key::Enter);
    assert_eq!(t.d.focused_app(), Some(AppKind::Editor));
    t.combo(Mods::CTRL, Key::End);
    t.key(Key::Enter);
    t.type_text("tercera línea");
    match t.d.app(AppKind::Editor) {
        Some(App::Editor(e)) => assert!(e.title().starts_with("* notas.txt")),
        _ => panic!(),
    }
    t.combo(Mods::CTRL, Key::Char('s'));
    assert!(
        t.logs()
            .contains(&"EDITOR_GUARDADO /Documentos/notas.txt".into())
    );
    assert_eq!(
        fatfs_read(t.d, "/Documentos/notas.txt").unwrap(),
        "primera linea\nsegunda linea\ntercera línea".as_bytes()
    );
}

#[test]
fn cerrar_el_editor_con_cambios_avisa_primero() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("LEAME.txt");
    t.key(Key::Enter);
    t.type_text("x");
    t.combo(Mods::ALT, Key::F(4));
    assert_eq!(
        t.d.focused_app(),
        Some(AppKind::Editor),
        "la primera vez avisa"
    );
    t.combo(Mods::ALT, Key::F(4));
    assert!(t.d.window_of(AppKind::Editor).is_none());
    assert_eq!(
        fatfs_read(t.d, "/LEAME.txt").unwrap(),
        b"leame",
        "no se guardó"
    );
}

#[test]
fn cada_tipo_se_abre_con_la_app_predeterminada_que_se_elija() {
    let mut t = Driver::new();
    let open_page = |t: &mut Driver| {
        t.d.open(Launch::Folder("/Documentos".into()), t.now, CLOCK);
        t.key(Key::Tab);
        t.select_named("pagina.html");
        t.key(Key::Enter);
    };
    // De fábrica, una página guardada se abre en el editor (Brave no ve el disco de JARVIS-OS).
    open_page(&mut t);
    assert_eq!(t.d.focused_app(), Some(AppKind::Editor));
    t.combo(Mods::ALT, Key::F(4));
    // Configuración → Aplicaciones predeterminadas: Navegador web, Buscador, Carpetas, Texto,
    // Código y Páginas web guardadas (la sexta fila): → elige la terminal.
    t.d.open(Launch::Settings(19), t.now, CLOCK);
    let Some(App::Settings(st)) = t.d.app(AppKind::Settings) else {
        panic!("no se abrió Configuración");
    };
    assert_eq!(st.section.name(), "Aplicaciones predeterminadas");
    t.keys(&[Key::Down; 5]);
    t.key(Key::Right);
    assert_eq!(
        t.d.config()
            .default_apps
            .get(jarvis_desktop::defaults::FileType::Html),
        jarvis_desktop::defaults::Handler::Terminal
    );
    assert!(t.d.config().serialize().contains("app_html=terminal"));
    t.combo(Mods::ALT, Key::F(4));
    open_page(&mut t);
    assert_eq!(t.d.focused_app(), Some(AppKind::Terminal));
}

/// Un PNG (con transparencia) y un JPEG en el disco los abre el visor: los decodifica
/// `jarvis-image`, sin el puente (K10).
#[test]
fn el_visor_abre_png_y_jpeg() {
    let (w, h) = (40u32, 24u32);
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            [
                (i % w * 6) as u8,
                (i / w * 10) as u8,
                90,
                if i % 3 == 0 { 0 } else { 255 },
            ]
        })
        .collect();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
        .unwrap();
    let rgb: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpg)
        .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .unwrap();
    for (name, data) in [("foto.png", &png), ("foto.jpg", &jpg)] {
        let mut t = Driver::new();
        let path = format!("/Imágenes/{name}");
        t.d.fs_mut()
            .unwrap()
            .write_file(&path, data, jarvis_fs::Timestamp::EPOCH)
            .unwrap();
        t.d.open(Launch::View(path), t.now, CLOCK);
        let Some(App::Viewer(v)) = t.d.app(AppKind::Viewer) else {
            panic!("no se abrió el visor");
        };
        assert!(v.title().ends_with("40×24"), "{name}: {}", v.title());
        t.frame();
    }
}
