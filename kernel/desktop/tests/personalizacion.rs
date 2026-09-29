//! Personalización en capas: tema, acento, tipografía, ventanas y barra de arriba.
//!
//! El aspecto es global (como la paleta de colores), así que todo va en **un solo test**: si
//! hubiera varios, correrían en paralelo y uno cambiaría el tema en medio del otro.

mod common;

use common::*;
use jarvis_desktop::config::{AnimStyle, ThemeKind, TitleDouble};
use jarvis_desktop::wm::{BUTTON_W, TITLE_H};
use jarvis_desktop::{AppKind, Key, Launch, Mods};
use jarvis_gfx::theme;

fn assert_same(a: &[u8], b: &[u8]) {
    if a == b {
        return;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (W, H, 0, 0);
    for (i, (p, q)) in a.chunks(4).zip(b.chunks(4)).enumerate() {
        if p != q {
            let (x, y) = (i % W, i / W);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    panic!("el render por partes difiere en ({x0},{y0})-({x1},{y1})");
}

#[test]
fn personalizacion_en_capas() {
    let mut t = Driver::new();
    assert_eq!(
        theme::cyan(),
        jarvis_gfx::Color::hex(0x00f0ff),
        "por defecto, el HUD"
    );

    // --- Tema claro, acento violeta y botones a la izquierda ---
    let mut cfg = t.d.config().clone();
    cfg.theme = ThemeKind::Light;
    cfg.accent = 3; // violeta
    cfg.buttons_left = true;
    cfg.bold_titles = true;
    t.d.set_config(cfg.clone());
    assert!(theme::is_light());
    assert_eq!(theme::cyan(), jarvis_gfx::Color::hex(0xa66bff));

    t.d.open(Launch::App(AppKind::Files), t.now, CLOCK);
    let r = t.window(AppKind::Files);
    // Cerrar está ahora arriba a la izquierda.
    t.click_at(r.x + BUTTON_W / 2, r.y + TITLE_H / 2, 1000);
    assert!(
        t.d.window_of(AppKind::Files).is_none(),
        "cerró con el botón de la izquierda"
    );

    // Render por partes == completo, con el tema claro.
    let (mut bg_buf, mut frame_buf) = buffers();
    let mut bg = canvas(&mut bg_buf);
    t.d.draw_background(&mut bg);
    t.d.render(&mut canvas(&mut frame_buf), &mut bg, t.now, CLOCK);
    t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
    t.d.open(Launch::App(AppKind::Editor), t.now, CLOCK);
    t.type_text("hola");
    for _ in 0..12 {
        t.now += 40;
        t.d.render(&mut canvas(&mut frame_buf), &mut bg, t.now, CLOCK);
    }
    let win = t.window(AppKind::Editor);
    {
        let frame = canvas(&mut frame_buf);
        let c = frame.get(win.x + win.w / 2, win.y + win.h - 60).unwrap();
        assert!(
            (c.r as u32 + c.g as u32 + c.b as u32) / 3 > 200,
            "el fondo del editor es claro: {c:?}"
        );
    }
    let mut full = vec![0u8; W * H * 4];
    t.d.invalidate();
    t.d.render(&mut canvas(&mut full), &mut bg, t.now, CLOCK);
    assert_same(&frame_buf, &full);

    // --- Ventanas: doble clic minimiza, animación deslizar, acoplar al soltar en un borde ---
    let mut cfg = t.d.config().clone();
    cfg.title_double = TitleDouble::Minimize;
    cfg.anim_style = AnimStyle::Slide;
    cfg.buttons_left = false;
    cfg.drag_outline = true;
    t.d.set_config(cfg);
    let r = t.window(AppKind::Editor);
    t.double_click(jarvis_gfx::Rect::new(r.x + 200, r.y + 4, 20, 20));
    assert!(
        t.d.window_manager()
            .get(t.id(AppKind::Editor))
            .unwrap()
            .minimized,
        "doble clic en el título minimiza"
    );
    // Arrastrar el Monitor (solo el contorno) contra el borde izquierdo: se acopla.
    let r = t.window(AppKind::Monitor);
    t.move_to(r.x + 300, r.y + 10, false);
    t.move_to(r.x + 300, r.y + 10, true);
    t.move_to(r.x + 250, r.y + 60, true);
    assert_eq!(
        t.window(AppKind::Monitor),
        r,
        "con contorno, la ventana no se mueve todavía"
    );
    t.move_to(0, 300, true);
    t.move_to(0, 300, false);
    let snapped = t.window(AppKind::Monitor);
    assert_eq!((snapped.x, snapped.w), (0, W as i32 / 2));
    assert!(t.logs().iter().any(|l| l == "VENTANA_ACOPLADA"));

    // --- Barra de arriba: sin ella, maximizar ocupa la zona de trabajo ---
    let mut cfg = t.d.config().clone();
    cfg.topbar = false;
    t.d.set_config(cfg);
    t.combo(Mods::WIN, Key::Up);
    assert!(!t.d.topbar_mode());
    assert!(t.window(AppKind::Monitor).y > 30);

    // Volver a lo de siempre (para no molestar a nada que venga después en este proceso).
    t.d.set_config(jarvis_desktop::Config::default());
    assert!(!theme::is_light());
}
