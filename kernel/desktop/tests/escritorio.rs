//! El gestor de ventanas y los atajos de teclado (como en Windows), las apps y el render.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::shell::{self, LAUNCHERS, StartMenu};
use jarvis_desktop::wm::{BUTTON_W, TITLE_H};
use jarvis_desktop::{
    AppKind, Desktop, Event, HttpResponse, Key, Launch, Mods, Power, SystemStats,
};
use jarvis_fs::MemDisk;
use jarvis_gfx::Rect;

fn open(t: &mut Driver, kind: AppKind) {
    t.d.open(Launch::App(kind), t.now, CLOCK);
    assert_eq!(t.d.focused_app(), Some(kind));
}

#[test]
fn alt_tab_cambia_a_la_ventana_anterior() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    open(&mut t, AppKind::Monitor);
    open(&mut t, AppKind::Console);
    // Alt+Tab y soltar: la anterior (Monitor).
    t.mods(Mods::ALT);
    t.key(Key::Tab);
    assert_eq!(t.d.overlay_name(), "alt-tab");
    t.mods(Mods::NONE);
    assert_eq!(t.d.overlay_name(), "");
    assert_eq!(t.d.focused_app(), Some(AppKind::Monitor));
    // Alt+Tab+Tab: dos para atrás (Files, porque ahora el orden es Monitor, Consola, Archivos).
    t.mods(Mods::ALT);
    t.key(Key::Tab);
    t.key(Key::Tab);
    t.mods(Mods::NONE);
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    // Alt+Shift+Tab: para el otro lado.
    t.mods(Mods {
        alt: true,
        shift: true,
        ..Mods::NONE
    });
    t.key(Key::Tab);
    t.mods(Mods::NONE);
    assert_eq!(t.d.focused_app(), Some(AppKind::Console));
}

#[test]
fn win_d_muestra_el_escritorio_y_vuelve() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    open(&mut t, AppKind::Monitor);
    t.combo(Mods::WIN, Key::Char('d'));
    assert_eq!(t.d.focused_app(), None);
    assert!(t.d.window_manager().windows().iter().all(|w| w.minimized));
    // En el escritorio, Espacio hace hablar a JARVIS.
    t.key(Key::Char(' '));
    assert!(t.logs().iter().any(|l| l.starts_with("JARVIS_HABLA")));
    t.combo(Mods::WIN, Key::Char('d'));
    assert_eq!(t.d.focused_app(), Some(AppKind::Monitor));
    assert!(t.d.window_manager().windows().iter().all(|w| !w.minimized));
}

#[test]
fn atajos_de_windows_para_abrir_apps() {
    let mut t = Driver::new();
    t.combo(Mods::WIN, Key::Char('e'));
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    t.combo(Mods::WIN, Key::Char('r'));
    assert_eq!(t.d.focused_app(), Some(AppKind::Console));
    t.combo(
        Mods {
            ctrl: true,
            shift: true,
            ..Mods::NONE
        },
        Key::Escape,
    );
    assert_eq!(t.d.focused_app(), Some(AppKind::Monitor));
    // Win+E con Archivos ya abierto: lo trae adelante (no abre otro).
    t.combo(Mods::WIN, Key::Char('e'));
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    assert_eq!(t.d.window_manager().windows().len(), 3);
    // Win+6 = sexto ícono de la barra (Archivos): como ya tiene el foco, se minimiza.
    t.combo(Mods::WIN, Key::Char('6'));
    assert_ne!(t.d.focused_app(), Some(AppKind::Files));
}

#[test]
fn acoplar_maximizar_y_minimizar_con_el_teclado() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Monitor);
    let before = t.window(AppKind::Monitor);
    t.combo(Mods::WIN, Key::Left);
    let r = t.window(AppKind::Monitor);
    assert_eq!((r.x, r.w), (0, W as i32 / 2));
    t.combo(Mods::WIN, Key::Up);
    let r = t.window(AppKind::Monitor);
    assert_eq!((r.x, r.w), (0, W as i32));
    t.combo(Mods::WIN, Key::Down);
    assert_eq!(t.window(AppKind::Monitor), before);
    t.combo(Mods::WIN, Key::Down);
    assert_eq!(t.d.focused_app(), None, "minimizada");
}

#[test]
fn botones_de_la_ventana_con_el_mouse() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Console);
    let r = t.window(AppKind::Console);
    let button = |i: i32| {
        Rect::new(
            r.x + r.w - (3 - i) * BUTTON_W - 1,
            r.y + 1,
            BUTTON_W,
            TITLE_H - 1,
        )
    };

    t.click(button(1)); // maximizar
    assert_eq!(t.window(AppKind::Console).w, W as i32);
    let r2 = t.window(AppKind::Console);
    t.click(Rect::new(
        r2.x + r2.w - 2 * BUTTON_W - 1,
        r2.y + 1,
        BUTTON_W,
        TITLE_H - 1,
    )); // restaurar
    assert_eq!(t.window(AppKind::Console), r);

    t.click(button(0)); // minimizar
    assert_eq!(t.d.focused_app(), None);
    assert!(t.logs().contains(&"VENTANA_MINIMIZADA".into()));
    // Clic en su ícono de la barra: vuelve.
    let consola = LAUNCHERS
        .iter()
        .position(|l| *l == shell::Launcher::App(AppKind::Console))
        .unwrap();
    let bar = shell::toolbar_rect(LAUNCHERS.len());
    t.click_at(
        bar.x + 9 + consola as i32 * shell::SLOT + 15,
        bar.y + bar.h / 2,
        1000,
    );
    assert_eq!(t.d.focused_app(), Some(AppKind::Console));

    t.click(button(2)); // cerrar
    assert!(t.d.window_of(AppKind::Console).is_none());
    assert!(t.logs().contains(&"VENTANA_CERRADA Consola JARVIS".into()));
}

#[test]
fn arrastrar_la_barra_de_titulo_mueve_la_ventana_y_doble_clic_maximiza() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Music);
    let r = t.window(AppKind::Music);
    let (gx, gy) = (r.x + 100, r.y + 10);
    t.move_to(gx, gy, false);
    t.move_to(gx, gy, true);
    t.move_to(gx + 50, gy + 30, true);
    t.move_to(gx + 50, gy + 30, false);
    let moved = t.window(AppKind::Music);
    assert_eq!((moved.x, moved.y), (r.x + 50, r.y + 30));
    // Esquina de abajo a la derecha: cambia el tamaño.
    let (cx, cy) = (moved.x + moved.w - 3, moved.y + moved.h - 3);
    t.move_to(cx, cy, false);
    t.move_to(cx, cy, true);
    t.move_to(cx + 40, cy + 20, true);
    t.move_to(cx + 40, cy + 20, false);
    let resized = t.window(AppKind::Music);
    assert_eq!((resized.w, resized.h), (moved.w + 40, moved.h + 20));
    t.double_click(Rect::new(resized.x + 60, resized.y + 5, 10, 10));
    assert_eq!(t.window(AppKind::Music).w, W as i32);
}

#[test]
fn menu_de_inicio_con_la_tecla_windows() {
    let mut t = Driver::new();
    // La tecla Windows sola (apretar y soltar) abre el menú.
    t.mods(Mods::WIN);
    t.mods(Mods::NONE);
    assert_eq!(t.d.overlay_name(), "inicio");
    t.type_text("moni");
    t.key(Key::Enter);
    assert_eq!(t.d.overlay_name(), "");
    assert_eq!(t.d.focused_app(), Some(AppKind::Monitor));
    // Win+D no abre el menú (la tecla Windows se usó en un atajo).
    t.combo(Mods::WIN, Key::Char('d'));
    assert_eq!(t.d.overlay_name(), "");
    // Con el mouse: ícono de inicio y después "APAGAR".
    let bar = shell::toolbar_rect(LAUNCHERS.len());
    t.click_at(bar.x + 9 + 15, bar.y + bar.h / 2, 1000);
    assert_eq!(t.d.overlay_name(), "inicio");
    let menu = StartMenu::rect(W, H);
    t.click_at(menu.x + menu.w - 60, menu.y + menu.h - 30, 1000);
    assert_eq!(t.d.take_requests().power, Some(Power::Shutdown));
}

#[test]
fn escribir_una_direccion_en_el_menu_abre_el_navegador() {
    let mut t = Driver::new();
    t.mods(Mods::WIN);
    t.mods(Mods::NONE);
    t.type_text("example.com");
    t.key(Key::Enter);
    assert_eq!(t.d.focused_app(), Some(AppKind::Browser));
    let req = t.d.take_requests();
    assert_eq!(req.net.len(), 1);
    assert_eq!(req.net[0].url, "https://example.com/");
}

#[test]
fn alt_f4_cierra_y_en_el_escritorio_ofrece_apagar() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Monitor);
    t.combo(Mods::ALT, Key::F(4));
    assert!(t.d.window_of(AppKind::Monitor).is_none());
    t.combo(Mods::ALT, Key::F(4));
    assert_eq!(t.d.overlay_name(), "apagado");
    t.key(Key::Right); // reiniciar
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().power, Some(Power::Reboot));
}

#[test]
fn win_l_bloquea_y_una_tecla_desbloquea() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    t.combo(Mods::WIN, Key::Char('l'));
    assert!(t.d.is_locked());
    t.frame();
    t.key(Key::Char('a'));
    assert!(!t.d.is_locked());
    assert_eq!(
        t.d.focused_app(),
        Some(AppKind::Files),
        "la tecla solo desbloquea"
    );
}

#[test]
fn vista_de_tareas_con_win_tab_y_control_de_mision() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    open(&mut t, AppKind::Music);
    t.combo(Mods::WIN, Key::Tab);
    assert_eq!(t.d.overlay_name(), "tareas");
    t.frame();
    // Clic en la primera tarjeta (Archivos: el orden es el de apertura).
    let card = shell::task_card(W, H, 2, 0);
    t.click(card);
    assert_eq!(t.d.overlay_name(), "");
    assert_eq!(t.d.focused_app(), Some(AppKind::Files));
    // "Control de misión" (abajo a la izquierda), con el escritorio a la vista.
    t.combo(Mods::WIN, Key::Char('d'));
    let pill = shell::status_rect(W, H);
    t.click_at(pill.x + 40, pill.y + pill.h - 12, 1000);
    assert_eq!(t.d.overlay_name(), "tareas");
    t.key(Key::Escape);
    // Clic en el panel de estado: abre el monitor.
    t.click_at(pill.x + 40, pill.y + 20, 1000);
    assert_eq!(t.d.focused_app(), Some(AppKind::Monitor));
}

#[test]
fn captura_de_pantalla_queda_en_imagenes() {
    let mut t = Driver::new();
    t.frame();
    t.key(Key::PrintScreen);
    t.frame();
    let logs = t.logs();
    let path = logs
        .iter()
        .find_map(|l| l.strip_prefix("CAPTURA "))
        .expect("se guardó la captura")
        .to_string();
    assert!(
        path.starts_with("/Imágenes/Captura 2026-09-23 23.05.00"),
        "{path}"
    );
    let data = fatfs_read(t.d, &path).unwrap();
    assert_eq!(&data[..2], b"BM");
    assert_eq!(data.len(), 54 + W * H * 3);
}

#[test]
fn navegador_con_respuesta_de_la_red() {
    let mut t = Driver::new();
    t.d.open(Launch::Browse("http://example.com".into()), t.now, CLOCK);
    let req = t.d.take_requests();
    assert_eq!(req.net[0].url, "http://example.com/");
    let html = "<html><head><title>Ejemplo</title></head><body><h1>Example Domain</h1>\
                <p>Texto. <a href=\"/mas\">Más información</a></p></body></html>";
    t.d.net_response(
        req.net[0].id,
        Ok(HttpResponse {
            status: 200,
            content_type: "text/html; charset=UTF-8".into(),
            url: "http://example.com/".into(),
            body: html.as_bytes().to_vec(),
        }),
    );
    match t.d.app(AppKind::Browser) {
        Some(App::Browser(b)) => {
            assert!(b.page_text().contains("Example Domain"));
            assert_eq!(b.title(), "Ejemplo · Navegador");
        }
        _ => panic!(),
    }
    t.frame();
    // Tab va al enlace y Enter lo abre.
    t.key(Key::Tab);
    t.key(Key::Enter);
    let req = t.d.take_requests();
    assert!(!req.net.is_empty(), "{:?}", t.logs());
    assert_eq!(req.net[0].url, "http://example.com/mas");
    // Ctrl+L, una búsqueda, Enter: va al buscador.
    t.combo(Mods::CTRL, Key::Char('l'));
    t.type_text("rust osdev");
    t.key(Key::Enter);
    let req = t.d.take_requests();
    assert!(
        req.net[0]
            .url
            .starts_with("https://html.duckduckgo.com/html/?q=rust+osdev")
    );
    // Un error de red se muestra en la página.
    t.d.net_response(req.net[0].id, Err("sin conexión".into()));
    t.frame();
}

#[test]
fn monitor_muestra_el_estado_y_finaliza_tareas() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Music);
    open(&mut t, AppKind::Monitor);
    for i in 0..5 {
        t.d.set_stats(SystemStats {
            cpu_pct: 10 + i * 5,
            heap_used: 40 << 20,
            heap_total: 256 << 20,
            uptime_ms: 1000 * i as u64,
            ..Default::default()
        });
    }
    t.frame();
    // Seleccionar Música en la lista (↓: la segunda en orden de uso) y Supr: "finalizar tarea".
    t.key(Key::Down);
    t.key(Key::Delete);
    assert!(t.d.window_of(AppKind::Music).is_none());
    t.frame();
}

#[test]
fn consola_entiende_ordenes() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Console);
    t.type_text("ls /Documentos");
    t.key(Key::Enter);
    t.type_text("abrir musica");
    t.key(Key::Enter);
    let logs = t.logs();
    assert!(logs.contains(&"CONSOLA ls /Documentos".into()));
    assert!(
        logs.iter()
            .any(|l| l.starts_with("JARVIS_HABLA: Abriendo Música"))
    );
    assert_eq!(t.d.focused_app(), Some(AppKind::Music));
    t.frame();
}

#[test]
fn musica_manda_tonos_al_parlante() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Music);
    t.key(Key::Enter); // Himno a la alegría: empieza con E4 = 329 Hz
    t.frame();
    assert_eq!(t.d.take_requests().tone, Some(329));
    // Al cerrar la ventana se apaga.
    t.combo(Mods::ALT, Key::F(4));
    t.frame();
    assert_eq!(t.d.take_requests().tone, Some(0));
}

/// Dibujar por partes (solo lo que cambia) tiene que dar lo mismo que redibujar todo.
#[test]
fn render_incremental_igual_a_redibujar_todo() {
    let mut t = Driver::new();
    let (mut bg_buf, mut frame_buf) = buffers();
    let mut bg = canvas(&mut bg_buf);
    t.d.draw_background(&mut bg);
    {
        let mut frame = canvas(&mut frame_buf);
        let mut step = |t: &mut Driver| {
            t.d.render(&mut frame, &mut bg, t.now, CLOCK);
        };
        step(&mut t);
        t.key(Key::Tab);
        step(&mut t);
        for k in [Key::Down, Key::Down, Key::Enter, Key::F(7)] {
            t.key(k);
            step(&mut t);
        }
        t.type_text("x");
        step(&mut t);
        t.key(Key::Escape);
        t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
        step(&mut t);
        let r = t.window(AppKind::Monitor);
        t.move_to(r.x + 100, r.y + 10, false);
        t.move_to(r.x + 100, r.y + 10, true);
        t.move_to(r.x + 60, r.y + 90, true);
        t.move_to(r.x + 60, r.y + 90, false);
        step(&mut t);
        t.mods(Mods::ALT);
        t.key(Key::Tab);
        step(&mut t);
        t.mods(Mods::NONE);
        step(&mut t);
        t.mods(Mods::WIN);
        t.mods(Mods::NONE);
        step(&mut t);
        t.key(Key::Escape);
        step(&mut t);
    }
    // Forzar un redibujado completo del mismo estado.
    let mut full_buf = vec![0u8; W * H * 4];
    t.d.invalidate();
    t.d.render(&mut canvas(&mut full_buf), &mut bg, t.now, CLOCK);
    let diff = frame_buf
        .iter()
        .zip(&full_buf)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        diff == 0,
        "el render incremental dejó restos ({diff} bytes distintos)"
    );
}

#[test]
fn sin_disco_no_rompe() {
    let mut d: Desktop<MemDisk> = Desktop::new(W, H, 300, None);
    for (i, e) in [
        Event::Key(Key::Tab),
        Event::Key(Key::F(7)),
        Event::Key(Key::PrintScreen),
    ]
    .into_iter()
    .enumerate()
    {
        d.handle(e, 10 + i as u64, CLOCK);
    }
    let (mut bg_buf, mut frame_buf) = buffers();
    let mut bg = canvas(&mut bg_buf);
    let mut frame = canvas(&mut frame_buf);
    d.render(&mut frame, &mut bg, 30, CLOCK);
    d.render(&mut frame, &mut bg, 40, CLOCK);
    assert_eq!(d.focused_app(), Some(AppKind::Files));
}
