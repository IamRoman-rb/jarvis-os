//! Cerrar sesión, suspender y el botón de energía de la barra de arriba.

mod common;

use common::*;
use jarvis_desktop::shell::StartMenu;
use jarvis_desktop::{AppKind, Key, Launch, Mods};

fn open(t: &mut Driver, kind: AppKind) {
    t.d.open(Launch::App(kind), t.now, CLOCK);
}

/// Alt+F4 con el escritorio enfocado abre el diálogo de energía; `rights` elige el botón.
fn power_dialog(t: &mut Driver, rights: usize) {
    t.combo(Mods::WIN, Key::Char('d')); // el escritorio al frente
    t.combo(Mods::ALT, Key::F(4));
    assert_eq!(t.d.overlay_name(), "apagado");
    for _ in 0..rights {
        t.key(Key::Right);
    }
    t.key(Key::Enter);
}

#[test]
fn cerrar_sesion_cierra_todo_y_pide_entrar() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    open(&mut t, AppKind::Monitor);
    t.logs();
    power_dialog(&mut t, 3); // apagar, reiniciar, suspender, CERRAR SESIÓN
    assert!(t.d.window_of(AppKind::Files).is_none());
    assert!(t.d.window_of(AppKind::Monitor).is_none());
    assert!(t.d.session_closed());
    assert_eq!(t.d.overlay_name(), "sesion");
    assert!(t.logs().iter().any(|l| l == "SESION_CERRADA roman"));
    t.frame();
    t.key(Key::Char('a'));
    assert_eq!(t.d.overlay_name(), "");
    assert!(!t.d.session_closed());
    assert!(t.logs().iter().any(|l| l == "SESION_INICIADA roman"));
}

#[test]
fn con_cambios_sin_guardar_no_cierra_la_sesion() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Editor);
    t.type_text("texto sin guardar");
    // Desde el menú de inicio (fila de arriba, en el medio).
    t.mods(Mods::WIN);
    t.mods(Mods::NONE);
    assert_eq!(t.d.overlay_name(), "inicio");
    let m = StartMenu::rect(W, H);
    t.click_at(m.x + m.w / 2, m.y + m.h - 92 + 18, 1000);
    assert!(!t.d.session_closed());
    assert!(t.d.window_of(AppKind::Editor).is_some());
    assert!(t.logs().iter().any(|l| l == "SESION_NO_CERRADA"));
}

#[test]
fn suspender_apaga_la_pantalla_y_una_tecla_despierta() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    power_dialog(&mut t, 2); // SUSPENDER
    assert!(t.d.is_sleeping());
    assert!(t.logs().iter().any(|l| l == "SUSPENDIDO"));
    let (mut bg, mut fr) = buffers();
    let mut bgc = canvas(&mut bg);
    let mut frame = canvas(&mut fr);
    // El primer cuadro es todo negro; después no se dibuja nada.
    let first = t.d.render(&mut frame, &mut bgc, t.now, CLOCK);
    assert!(!first.is_empty());
    assert!((0..W as i32).step_by(97).all(|x| {
        frame
            .get(x, 400)
            .is_some_and(|c| (c.r, c.g, c.b) == (0, 0, 0))
    }));
    t.now += 1000;
    assert!(t.d.render(&mut frame, &mut bgc, t.now, CLOCK).is_empty());
    // Mover el mouse despierta y todo sigue como estaba.
    t.move_to(300, 300, false);
    assert!(!t.d.is_sleeping());
    assert!(t.logs().iter().any(|l| l == "DESPIERTO"));
    assert!(t.d.window_of(AppKind::Files).is_some());
    assert!(!t.d.is_locked(), "sin PIN no hace falta desbloquear");
}

#[test]
fn con_pin_al_despertar_hay_que_desbloquear() {
    let mut t = Driver::new();
    let mut cfg = t.d.config().clone();
    cfg.pin = jarvis_desktop::pin::hash_pin("4321", b"");
    t.d.set_config(cfg);
    power_dialog(&mut t, 2);
    assert!(t.d.is_sleeping());
    t.key(Key::Char('x'));
    assert!(t.d.is_locked(), "despierta en la pantalla de bloqueo");
    t.type_text("4321");
    t.key(Key::Enter);
    assert!(!t.d.is_locked());
}

#[test]
fn boton_de_energia_de_la_barra_de_arriba() {
    let mut t = Driver::new();
    open(&mut t, AppKind::Files);
    t.combo(Mods::WIN, Key::Up); // maximizada: aparece la barra de arriba
    t.now += 1000;
    t.frame();
    // La punta derecha de la barra.
    t.click_at(W as i32 - 10, jarvis_desktop::wm::TOPBAR_H / 2, 1000);
    assert_eq!(t.d.overlay_name(), "apagado");
}
