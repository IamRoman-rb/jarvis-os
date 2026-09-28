//! Configuración, atajos nuevos (Win+X, Win+A, Win+N, Alt+Espacio, escritorios virtuales), PIN
//! de bloqueo y el navegador con estilos y formularios.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::{AppKind, HttpResponse, Key, Launch, Mods};

fn win_ctrl() -> Mods {
    Mods {
        win: true,
        ctrl: true,
        ..Mods::NONE
    }
}

fn settings_section(t: &Driver) -> String {
    match t.d.app(AppKind::Settings) {
        Some(App::Settings(s)) => s.section.name().to_string(),
        _ => panic!("Configuración no está abierta"),
    }
}

#[test]
fn configuracion_cambia_el_fondo_y_queda_guardada() {
    let mut t = Driver::new();
    t.combo(Mods::WIN, Key::Char('i'));
    assert_eq!(t.d.focused_app(), Some(AppKind::Settings));
    assert_eq!(settings_section(&t), "Sistema");
    // AvPág: Personalización. La primera fila es el fondo: → pasa al primer color.
    t.keys(&[Key::PageDown, Key::PageDown]);
    assert_eq!(settings_section(&t), "Personalización");
    t.key(Key::Right);
    t.frame();
    assert!(t.logs().iter().any(|l| l == "CONFIG_GUARDADA"));
    // Después vienen las capas de personalización y Hora e idioma.
    t.key(Key::PageDown);
    assert_eq!(settings_section(&t), "Apariencia");
    t.keys(&[Key::PageDown, Key::PageDown, Key::PageDown]);
    assert_eq!(settings_section(&t), "Barra y cursor");
    // Reloj de 12 horas (Hora e idioma, tercera fila).
    t.key(Key::PageDown);
    assert_eq!(settings_section(&t), "Hora e idioma");
    t.keys(&[Key::Down, Key::Down]);
    t.key(Key::Enter);
    assert!(!t.d.config().clock_24h);
    t.frame();
    let cfg = fatfs_read(t.d, "/Sistema/config.ini").expect("se guardó la configuración");
    let cfg = String::from_utf8(cfg).unwrap();
    assert!(cfg.contains("fondo=color:0"), "{cfg}");
    assert!(cfg.contains("reloj_24h=no"), "{cfg}");
}

#[test]
fn la_configuracion_se_lee_al_arrancar() {
    let mut t = Driver::new();
    t.d.open(
        Launch::Terminal(Some(
            "mkdir -p /Sistema && echo zona_utc=5 > /Sistema/config.ini".into(),
        )),
        t.now,
        CLOCK,
    );
    let img = t.d.into_fs().unwrap();
    let d: jarvis_desktop::Desktop<jarvis_fs::MemDisk> =
        jarvis_desktop::Desktop::new(W, H, 100, Some(img));
    assert_eq!(d.utc_offset(), 5);
    assert!(
        d.config().latam_keyboard,
        "lo que no está en el archivo queda por defecto"
    );
}

#[test]
fn pin_de_bloqueo() {
    let mut t = Driver::new();
    t.combo(Mods::WIN, Key::Char('i'));
    // Sincronización es la última sección (RePág desde Sistema da la vuelta); antes están
    // Firewall y Privacidad y seguridad.
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Sincronización");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Firewall");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Privacidad y seguridad");
    t.key(Key::Enter); // editar el PIN
    t.type_text("12a34");
    t.key(Key::Enter);
    assert_eq!(t.d.config().pin, "1234", "las letras no entran en el PIN");
    t.combo(Mods::WIN, Key::Char('l'));
    assert!(t.d.is_locked());
    t.key(Key::Char(' '));
    assert!(t.d.is_locked(), "con PIN, una tecla no alcanza");
    t.type_text("9999");
    t.key(Key::Enter);
    assert!(t.d.is_locked());
    assert!(t.logs().iter().any(|l| l == "ESCRITORIO_PIN_INCORRECTO"));
    t.type_text("1234");
    t.key(Key::Enter);
    assert!(!t.d.is_locked());
}

#[test]
fn win_x_win_a_win_n_y_alt_espacio() {
    let mut t = Driver::new();
    // Win+X: enlaces rápidos; la cuarta opción es la terminal.
    t.combo(Mods::WIN, Key::Char('x'));
    assert_eq!(t.d.overlay_name(), "enlaces");
    for _ in 0..3 {
        t.key(Key::Down);
    }
    t.key(Key::Enter);
    assert_eq!(t.d.focused_app(), Some(AppKind::Terminal));
    // Alt+Espacio: el menú de la ventana; "Maximizar".
    t.combo(Mods::ALT, Key::Char(' '));
    assert_eq!(t.d.overlay_name(), "ventana");
    t.key(Key::Enter);
    let id = t.d.window_of(AppKind::Terminal).unwrap();
    assert!(t.d.window_manager().get(id).unwrap().maximized);
    // Win+A: configuración rápida; Enter cambia el primero (sonidos).
    t.combo(Mods::WIN, Key::Char('a'));
    assert_eq!(t.d.overlay_name(), "rapida");
    let before = t.d.config().sounds;
    t.key(Key::Enter);
    assert_ne!(t.d.config().sounds, before);
    t.combo(Mods::WIN, Key::Char('a'));
    assert_eq!(t.d.overlay_name(), "", "Win+A otra vez lo cierra");
    // Win+N: notificaciones (con lo que se avisó recién).
    t.d.notify("Aviso de prueba", false, t.now);
    t.combo(Mods::WIN, Key::Char('n'));
    assert_eq!(t.d.overlay_name(), "notificaciones");
    t.frame();
    t.key(Key::Escape);
    // F1: la ayuda de atajos en el navegador.
    t.key(Key::F(1));
    match t.d.app(AppKind::Browser) {
        Some(App::Browser(b)) => assert!(b.page_text().contains("Atajos de teclado")),
        _ => panic!("F1 abre la ayuda"),
    }
}

#[test]
fn escritorios_virtuales_con_win_ctrl() {
    let mut t = Driver::new();
    t.combo(Mods::WIN, Key::Char('e'));
    t.combo(win_ctrl(), Key::Char('d'));
    assert_eq!(t.d.window_manager().desktops(), (1, 2));
    assert_eq!(t.d.focused_app(), None);
    t.combo(Mods::WIN, Key::Char('i'));
    t.combo(win_ctrl(), Key::Left);
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    // Alt+Tab solo muestra las del escritorio actual.
    t.mods(Mods::ALT);
    t.key(Key::Tab);
    t.mods(Mods::NONE);
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    t.combo(win_ctrl(), Key::Right);
    t.combo(win_ctrl(), Key::F(4));
    assert_eq!(t.d.window_manager().desktops(), (0, 1));
    assert!(
        t.d.window_manager()
            .windows()
            .iter()
            .all(|w| w.visible() || w.minimized)
    );
    t.frame();
}

fn page(status: u16, ct: &str, url: &str, body: &str) -> Result<HttpResponse, String> {
    Ok(HttpResponse {
        status,
        content_type: ct.into(),
        url: url.into(),
        body: body.as_bytes().to_vec(),
    })
}

#[test]
fn navegador_con_estilos_formularios_e_imagenes() {
    let mut t = Driver::new();
    t.d.open(
        Launch::Browse("https://www.google.com/".into()),
        t.now,
        CLOCK,
    );
    let req = t.d.take_requests();
    let html = "<html><head><title>Google</title><style>body{background:#fff}\
                .logo{text-align:center} a{color:#1a0dab}</style>\
                <link rel=stylesheet href=/estilo.css></head><body>\
                <div class=logo><img src=/logo.png alt=Google width=272 height=92></div>\
                <form action=/search><input type=hidden name=hl value=es><input name=q size=40>\
                <input type=submit name=btnG value='Buscar con Google'></form></body></html>";
    t.d.net_response(
        req.net[0].id,
        page(200, "text/html", "https://www.google.com/", html),
    );
    // Después de la página se piden la hoja de estilo y la imagen (convertida a BMP).
    let req = t.d.take_requests();
    let urls: Vec<(&str, jarvis_desktop::FetchKind)> =
        req.net.iter().map(|r| (r.url.as_str(), r.kind)).collect();
    assert!(
        urls.contains(&(
            "https://www.google.com/estilo.css",
            jarvis_desktop::FetchKind::Page
        )),
        "{urls:?}"
    );
    assert!(
        urls.contains(&(
            "https://www.google.com/logo.png",
            jarvis_desktop::FetchKind::Image
        )),
        "{urls:?}"
    );
    for r in &req.net {
        if r.url.ends_with(".css") {
            t.d.net_response(
                r.id,
                page(200, "text/css", &r.url, ".logo{background:#eee}"),
            );
        } else {
            // Una imagen BMP de 2×2.
            let mut buf = vec![0u8; 16];
            let c = jarvis_gfx::Canvas::new(&mut buf, 2, 2, 2, 4, jarvis_gfx::PixelFormat::Bgr)
                .unwrap();
            let bmp = jarvis_desktop::bmp::encode(&c);
            t.d.net_response(
                r.id,
                Ok(HttpResponse {
                    status: 200,
                    content_type: "image/bmp".into(),
                    url: r.url.clone(),
                    body: bmp,
                }),
            );
        }
    }
    t.frame();
    match t.d.app(AppKind::Browser) {
        Some(App::Browser(b)) => {
            let d = b.document();
            let p = b.prepared().unwrap();
            assert_eq!(
                jarvis_desktop::web::html::canvas_color(p),
                Some(jarvis_gfx::Color::WHITE)
            );
            assert_eq!(d.fields.len(), 3);
            let logo = (0..p.dom.nodes.len())
                .find(|&i| p.dom.nodes[i].attr("class") == Some("logo"))
                .unwrap();
            assert_eq!(
                p.styled.get(logo).and_then(|s| s.bg).map(|c| c.c),
                Some(jarvis_gfx::Color::hex(0xeeeeee)),
                "se aplicó la hoja externa"
            );
        }
        _ => panic!(),
    }
    // Tab va al campo de búsqueda, se escribe y Enter manda el formulario. Google pide
    // JavaScript para buscar: se busca con el buscador elegido (DuckDuckGo).
    t.key(Key::Tab);
    t.type_text("rust osdev");
    t.key(Key::Enter);
    let req = t.d.take_requests();
    assert!(
        req.net[0]
            .url
            .starts_with("https://html.duckduckgo.com/html/?q=rust+osdev"),
        "{}",
        req.net[0].url
    );
    assert!(t.logs().iter().any(|l| {
        l.contains("NAVEGADOR_FORMULARIO https://www.google.com/search?hl=es&q=rust+osdev")
    }));
}

#[test]
fn el_navegador_guarda_descargas_en_descargas() {
    let mut t = Driver::new();
    t.d.open(
        Launch::Browse("http://ejemplo.com/instalador.exe".into()),
        t.now,
        CLOCK,
    );
    let req = t.d.take_requests();
    assert_eq!(req.net[0].kind, jarvis_desktop::FetchKind::Download);
    let mut exe = vec![0u8; 0x200];
    exe[0..2].copy_from_slice(b"MZ");
    t.d.net_response(
        req.net[0].id,
        Ok(HttpResponse {
            status: 200,
            content_type: "application/x-msdownload".into(),
            url: "http://ejemplo.com/instalador.exe".into(),
            body: exe,
        }),
    );
    assert!(
        t.logs()
            .iter()
            .any(|l| l.starts_with("DESCARGA /Descargas/instalador.exe"))
    );
    assert!(fatfs_exists(t.d, "/Descargas/instalador.exe"));
}

#[test]
fn firewall_desde_la_configuracion_bloquea_una_app() {
    let mut t = Driver::new();
    t.d.open(Launch::Settings(15), t.now, CLOCK);
    assert_eq!(settings_section(&t), "Firewall");
    // Fila 5: "Permitir: Navegador web" (se apaga = regla que bloquea al navegador).
    t.keys(&[Key::Down; 5]);
    t.key(Key::Enter);
    assert_eq!(
        t.d.config().firewall.rules[0].to_line(),
        "denegar salida app navegador"
    );
    t.d.open(Launch::Browse("http://example.com/".into()), t.now, CLOCK);
    let req = t.d.take_requests();
    assert!(req.net.is_empty(), "no sale nada a la red");
    // El error llega en el próximo pedido (como una respuesta).
    let _ = t.d.take_requests();
    t.frame();
    assert!(
        t.logs()
            .iter()
            .any(|l| l.contains("FIREWALL_BLOQUEO navegador example.com"))
    );
}
