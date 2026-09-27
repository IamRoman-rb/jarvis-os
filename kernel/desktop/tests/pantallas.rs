//! Varios monitores en el host: extender, duplicar, llevar ventanas de uno al otro, Win+P y el
//! render por partes con dos pantallas.

mod common;

use common::*;
use jarvis_desktop::display::Mode;
use jarvis_desktop::{AppKind, Key, Launch, Mods};
use jarvis_gfx::{Canvas, PixelFormat, Rect};

const TWO: [(u32, u32); 2] = [(1280, 800), (1024, 768)];

fn two_monitors() -> Driver {
    let mut t = Driver::new();
    t.d.set_outputs(TWO.to_vec());
    t
}

#[test]
fn extender_y_llevar_una_ventana_al_otro_monitor() {
    let mut t = two_monitors();
    let l = t.d.take_display().expect("pide armar la imagen");
    assert_eq!(l.size, (2304, 800));
    assert_eq!(t.d.full_size(), (2304, 800));
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "PANTALLAS_MODO extender 2304x800")
    );

    t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
    let before = t.window(AppKind::Monitor);
    assert!(before.x + before.w <= 1280, "abre en el principal");
    // Win+Shift+→: al segundo monitor; maximizada ahí ocupa todo ese monitor.
    t.combo(
        Mods {
            win: true,
            shift: true,
            ..Mods::NONE
        },
        Key::Right,
    );
    let r = t.window(AppKind::Monitor);
    assert!(r.x >= 1280, "{r:?}");
    t.combo(Mods::WIN, Key::Up);
    assert_eq!(t.window(AppKind::Monitor), Rect::new(1280, 0, 1024, 768));
    assert!(
        !t.d.topbar_mode() || t.d.window_manager().screens().len() == 2,
        "la barra de arriba es del principal"
    );
    // El mouse llega al segundo monitor.
    t.move_to(2000, 400, false);
    assert_eq!(t.d.cursor(), (2000, 400));
}

#[test]
fn win_p_duplica_y_las_ventanas_vuelven_al_principal() {
    let mut t = two_monitors();
    t.d.take_display();
    t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
    t.combo(
        Mods {
            win: true,
            shift: true,
            ..Mods::NONE
        },
        Key::Right,
    );
    // Win+P: arranca en "Extender"; una arriba es "Duplicar".
    t.combo(Mods::WIN, Key::Char('p'));
    assert_eq!(t.d.overlay_name(), "proyectar");
    t.key(Key::Up);
    t.key(Key::Enter);
    let l = t.d.take_display().expect("otro reparto");
    assert_eq!(l.size, (1024, 768));
    assert_eq!(t.d.config().display_mode, Mode::Duplicate);
    let r = t.window(AppKind::Monitor);
    assert!(r.x + r.w / 2 < 1024, "volvió adentro de la imagen: {r:?}");
    // Con un solo monitor, Win+P avisa.
    let mut one = Driver::new();
    one.combo(Mods::WIN, Key::Char('p'));
    assert_eq!(one.d.overlay_name(), "");
}

#[test]
fn render_por_partes_con_dos_monitores() {
    let mut t = two_monitors();
    let l = t.d.take_display().unwrap();
    let (w, h) = (l.size.0 as usize, l.size.1 as usize);
    let mut bg_buf = vec![0u8; w * h * 4];
    let mut fr_buf = vec![0u8; w * h * 4];
    let mut full_buf = vec![0u8; w * h * 4];
    fn canvas_of(b: &mut [u8], w: usize, h: usize) -> Canvas<'_> {
        Canvas::new(b, w, h, w, 4, PixelFormat::Bgr).unwrap()
    }
    let mut bg = canvas_of(&mut bg_buf, w, h);
    t.d.draw_background(&mut bg);
    t.d.render(&mut canvas_of(&mut fr_buf, w, h), &mut bg, t.now, CLOCK);
    t.d.open(Launch::App(AppKind::Files), t.now, CLOCK);
    t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
    t.combo(
        Mods {
            win: true,
            shift: true,
            ..Mods::NONE
        },
        Key::Right,
    );
    for _ in 0..10 {
        t.now += 40;
        t.d.render(&mut canvas_of(&mut fr_buf, w, h), &mut bg, t.now, CLOCK);
    }
    t.d.invalidate();
    t.d.render(&mut canvas_of(&mut full_buf, w, h), &mut bg, t.now, CLOCK);
    assert!(
        fr_buf == full_buf,
        "el render por partes difiere con dos monitores"
    );
    // El reloj del HUD queda en el principal (no en la punta derecha de toda la imagen).
    let frame = canvas_of(&mut full_buf, w, h);
    let bgc = canvas_of(&mut bg_buf, w, h);
    let clock = jarvis_gfx::hud::clock_rect(1280, 800);
    let differs = (clock.x..clock.x + clock.w)
        .step_by(3)
        .any(|x| frame.get(x, clock.y + 40) != bgc.get(x, clock.y + 40));
    assert!(differs, "el reloj se dibujó en el principal");
}
