//! Configuración, atajos nuevos (Win+X, Win+A, Win+N, Alt+Espacio, escritorios virtuales), PIN
//! de bloqueo y el navegador con estilos y formularios.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::{AppKind, Key, Launch, Mods};

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
    // Asistente (IA) es la última sección (RePág desde Sistema da la vuelta); antes están
    // Aplicaciones predeterminadas, Hardware (K13), Micrófono, Sincronización, Antivirus,
    // Firewall y Privacidad y seguridad.
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Asistente (IA)");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Aplicaciones predeterminadas");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Hardware");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Micrófono");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Sincronización");
    t.key(Key::PageUp);
    assert_eq!(settings_section(&t), "Antivirus");
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
    // F1: la ayuda (los comandos y atajos) en la terminal.
    t.key(Key::F(1));
    assert_eq!(
        t.d.focused_app(),
        Some(AppKind::Terminal),
        "F1 abre la ayuda"
    );
}

#[test]
fn el_volumen_desde_win_a_y_la_configuracion() {
    use jarvis_desktop::panels;

    let mut t = Driver::new();
    t.d.enable_sound(48_000);
    assert_eq!(t.d.config().volume, 80);
    assert_eq!(
        t.d.volume(),
        80,
        "el mezclador arranca con el volumen guardado"
    );
    t.combo(Mods::WIN, Key::Char('a'));
    // Con el teclado: dos filas abajo (de los interruptores al volumen) y ← lo baja de a 1,
    // como en Windows (con Ctrl, de a 10).
    t.keys(&[Key::Down, Key::Down, Key::Left, Key::Left]);
    assert_eq!(t.d.config().volume, 78);
    assert_eq!(t.d.volume(), 78);
    t.mods(Mods::CTRL);
    t.key(Key::Left);
    t.mods(Mods::NONE);
    assert_eq!(t.d.config().volume, 68);
    // Con el mouse: un clic al principio de la barra lo silencia y al final lo pone al máximo.
    let bar = panels::volume_bar(W, H);
    t.click_at(bar.x - 4, bar.y + 4, 500);
    assert_eq!(t.d.config().volume, 0);
    t.click_at(bar.x + bar.w / 2, bar.y + 4, 500);
    assert_eq!(t.d.config().volume, 50);
    t.click_at(bar.x + bar.w + 20, bar.y + 4, 500);
    assert_eq!(t.d.config().volume, 100);
    assert_eq!(t.d.overlay_name(), "rapida", "el panel sigue abierto");
    // Arrastrar la perilla: suena en vivo y se guarda una sola vez, al soltar.
    t.logs();
    t.move_to(bar.x + bar.w, bar.y + 2, false);
    t.move_to(bar.x + bar.w, bar.y + 2, true);
    t.move_to(bar.x + bar.w / 4, bar.y + 2, true);
    assert_eq!(
        t.d.volume(),
        25,
        "suena con el volumen nuevo mientras se arrastra"
    );
    assert!(!t.logs().iter().any(|l| l == "CONFIG_GUARDADA"));
    t.move_to(bar.x + bar.w / 5, bar.y + 2, true);
    t.move_to(bar.x + bar.w / 5, bar.y + 2, false);
    assert_eq!(t.d.config().volume, 20);
    assert_eq!(
        t.logs().iter().filter(|l| *l == "CONFIG_GUARDADA").count(),
        1
    );
    t.click_at(bar.x + bar.w + 20, bar.y + 4, 500);
    t.frame();
    // Se guarda en el disco y se vuelve a leer.
    let text = t.d.config().serialize();
    assert!(text.contains("volumen=100"));
    assert_eq!(jarvis_desktop::Config::parse(&text).volume, 100);
    assert_eq!(jarvis_desktop::Config::parse("volumen=250").volume, 100);
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

#[test]
fn firewall_desde_la_configuracion_bloquea_una_app() {
    let mut t = Driver::new();
    t.d.open(Launch::Settings(15), t.now, CLOCK);
    assert_eq!(settings_section(&t), "Firewall");
    // Fila 5: "Permitir: Brave" (se apaga = regla que bloquea a Brave).
    t.keys(&[Key::Down; 5]);
    t.key(Key::Enter);
    assert_eq!(
        t.d.config().firewall.rules[0].to_line(),
        "denegar salida app brave"
    );
    // Brave quiere conectarse a su puente: no sale.
    t.d.open(Launch::Browse("http://example.com/".into()), t.now, CLOCK);
    let req = t.d.take_requests();
    assert!(req.streams.is_empty(), "no sale nada a la red");
    t.frame();
    assert!(
        t.logs()
            .iter()
            .any(|l| l.contains("FIREWALL_BLOQUEO brave")),
        "{:?}",
        t.logs()
    );
}

#[test]
fn la_barra_lateral_de_configuracion_se_desplaza_con_la_rueda() {
    let mut t = Driver::new();
    t.combo(Mods::WIN, Key::Char('i'));
    let r = t.window(AppKind::Settings);
    let x = r.x + 60;
    // Dónde queda cada sección clickeando la barra lateral de abajo hacia arriba.
    let sections_at = |t: &mut Driver| {
        let mut seen = Vec::new();
        for y in (r.y + 80..r.y + r.h - 4).step_by(17) {
            t.click_at(x, y, 400);
            seen.push(settings_section(t));
        }
        seen
    };
    let before = sections_at(&mut t);
    assert!(
        !before.iter().any(|s| s == "Sincronización"),
        "con 17 secciones la última no entra: {before:?}"
    );
    t.move_to(x, r.y + 300, false);
    t.wheel(3);
    let after = sections_at(&mut t);
    assert!(
        after.iter().any(|s| s == "Sincronización"),
        "después de la rueda se llega a la última: {after:?}"
    );
}

#[test]
fn los_botones_de_la_ventana_cambian_de_lado_desde_su_menu() {
    use jarvis_desktop::input::{Event, MousePacket};
    let mut t = Driver::new();
    t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
    let r = t.window(AppKind::Monitor);
    assert!(!t.d.config().buttons_left);
    // Clic derecho en la barra de título (lejos de los botones): el menú de la ventana.
    t.move_to(r.x + r.w / 2, r.y + 12, false);
    t.now += 10;
    t.d.handle(
        Event::Mouse(MousePacket {
            right: true,
            ..Default::default()
        }),
        t.now,
        CLOCK,
    );
    t.d.handle(Event::Mouse(MousePacket::default()), t.now + 5, CLOCK);
    assert_eq!(t.d.overlay_name(), "ventana");
    // Maximizar, Minimizar, Acoplar ×2 y "Botones a la izquierda".
    t.keys(&[Key::Down, Key::Down, Key::Down, Key::Down, Key::Enter]);
    assert!(t.d.config().buttons_left, "quedan a la izquierda");
    // Y con Alt+Espacio vuelven.
    t.combo(Mods::ALT, Key::Char(' '));
    t.keys(&[Key::Down, Key::Down, Key::Down, Key::Down, Key::Enter]);
    assert!(!t.d.config().buttons_left);
}

#[test]
fn la_barra_de_volumen_de_configuracion_es_como_la_de_windows() {
    use jarvis_desktop::apps::settings::{SOUND, Settings};

    let mut t = Driver::new();
    t.d.enable_sound(48_000);
    t.d.open(Launch::Settings(SOUND), t.now, CLOCK);
    let Some(App::Settings(s)) = t.d.app(AppKind::Settings) else {
        panic!("Configuración no se abrió");
    };
    assert_eq!(s.section.name(), "Sonido");
    // La primera fila es el volumen: ← → de a 1, Ctrl+→ de a 10 y Enter no lo cambia.
    t.key(Key::Right);
    assert_eq!(t.d.config().volume, 81);
    t.mods(Mods::CTRL);
    t.key(Key::Right);
    t.mods(Mods::NONE);
    assert_eq!(t.d.config().volume, 91);
    t.key(Key::Enter);
    assert_eq!(t.d.config().volume, 91);
    // Con el mouse: un clic en el riel lleva la perilla ahí y se puede arrastrar.
    let id = t.id(AppKind::Settings);
    let win = t.d.window_manager().get(id).unwrap();
    let (wx, wy, content) = (win.rect.x, win.rect.y, win.content());
    let track = Settings::slider_rect(content, 0);
    let (x0, y) = (wx + track.x, wy + track.y + 2);
    t.logs();
    t.move_to(x0 + track.w / 2, y, false);
    t.move_to(x0 + track.w / 2, y, true);
    assert_eq!(t.d.volume(), 50);
    t.move_to(x0 + track.w / 10, y, true);
    assert_eq!(t.d.volume(), 10, "suena en vivo mientras se arrastra");
    assert!(!t.logs().iter().any(|l| l == "CONFIG_GUARDADA"));
    // Afuera del riel se queda en el extremo, y al soltar se guarda.
    t.move_to(x0 - 200, y, true);
    t.move_to(x0 - 200, y, false);
    assert_eq!(t.d.config().volume, 0);
    assert_eq!(t.d.volume(), 0);
    assert!(t.logs().iter().any(|l| l == "CONFIG_GUARDADA"));
    assert!(t.d.config().serialize().contains("volumen=0"));
}
