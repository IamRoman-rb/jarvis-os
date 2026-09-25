//! Selección y atajos (Shift+flechas, Ctrl+E, varios archivos a la vez, portapapeles) y las
//! distribuciones de ventanas (Win+Z, cuartos, mosaico, cascada, esquinas).

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::files::Dialog;
use jarvis_desktop::wm::{LAYOUTS, Layout};
use jarvis_desktop::{AppKind, Key, Launch, Mods};
use jarvis_gfx::Rect;

fn editor_text(t: &Driver) -> (String, String) {
    match t.d.app(AppKind::Editor) {
        Some(App::Editor(e)) => (e.text(), e.selected_text()),
        _ => panic!("el Editor no está abierto"),
    }
}

fn shift_keys(t: &mut Driver, keys: &[Key]) {
    t.mods(Mods::SHIFT);
    for &k in keys {
        t.key(k);
    }
    t.mods(Mods::NONE);
}

#[test]
fn editor_selecciona_copia_y_pega() {
    let mut t = Driver::new();
    t.d.open(Launch::App(AppKind::Editor), t.now, CLOCK);
    t.type_text("hola mundo");
    // Shift+Inicio: todo el renglón. Ctrl+C, fin, Enter, Ctrl+V.
    shift_keys(&mut t, &[Key::Home]);
    assert_eq!(editor_text(&t).1, "hola mundo");
    t.combo(Mods::CTRL, Key::Char('c'));
    t.key(Key::End);
    t.key(Key::Enter);
    t.combo(Mods::CTRL, Key::Char('v'));
    assert_eq!(editor_text(&t).0, "hola mundo\nhola mundo");
    // Shift+← de a una letra y escribir reemplaza lo elegido.
    shift_keys(
        &mut t,
        &[Key::Left, Key::Left, Key::Left, Key::Left, Key::Left],
    );
    assert_eq!(editor_text(&t).1, "mundo");
    t.type_text("gente");
    assert_eq!(editor_text(&t).0, "hola mundo\nhola gente");
    // Ctrl+E (como en Office en castellano): todo. Ctrl+X lo corta.
    t.combo(Mods::CTRL, Key::Char('e'));
    assert_eq!(editor_text(&t).1, "hola mundo\nhola gente");
    t.combo(Mods::CTRL, Key::Char('x'));
    assert_eq!(editor_text(&t).0, "");
    t.combo(Mods::CTRL, Key::Char('v'));
    assert_eq!(editor_text(&t).0, "hola mundo\nhola gente");

    // Arrastrar con el mouse sobre "hola" del primer renglón.
    let win = t.window(AppKind::Editor);
    let content =
        t.d.window_manager()
            .get(t.id(AppKind::Editor))
            .unwrap()
            .content();
    // (El ancho de una letra, con la misma fuente que usa el editor.)
    let cw = jarvis_gfx::text::width(
        "M",
        &jarvis_gfx::text::Style::new(
            jarvis_gfx::text::Weight::Regular,
            jarvis_gfx::text::Size::Size16,
            jarvis_gfx::Color::WHITE,
        ),
    );
    let (x0, y0) = (win.x + content.x + 56 + 1, win.y + content.y + 12);
    t.move_to(x0, y0, false);
    t.move_to(x0, y0, true);
    t.move_to(x0 + 4 * cw, y0, true);
    t.move_to(x0 + 4 * cw, y0, false);
    assert_eq!(editor_text(&t).1, "hola");
}

#[test]
fn archivos_varios_con_shift_ctrl_e_y_la_papelera() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    assert_eq!(t.names(), ["Bienvenida.txt", "notas.txt", "pagina.html"]);
    // Shift+↓ dos veces: los tres.
    t.key(Key::Home);
    shift_keys(&mut t, &[Key::Down, Key::Down]);
    assert_eq!(t.files().targets().len(), 3);
    // ↓ sin Shift: vuelve a ser uno.
    t.key(Key::Up);
    assert_eq!(t.files().targets(), ["notas.txt"]);
    // Ctrl+clic suma, Shift+clic hace un rango desde el último.
    let l = t.layout();
    t.mods(Mods::CTRL);
    t.click(l.row_rect(0));
    t.mods(Mods::NONE);
    assert_eq!(t.files().targets(), ["Bienvenida.txt", "notas.txt"]);
    t.click(l.row_rect(2)); // un clic solo: uno
    assert_eq!(t.files().targets(), ["pagina.html"]);
    t.mods(Mods::SHIFT);
    t.click(l.row_rect(1));
    t.mods(Mods::NONE);
    assert_eq!(t.files().targets(), ["notas.txt", "pagina.html"]);

    // Ctrl+E y Supr: una sola confirmación para los tres.
    t.combo(Mods::CTRL, Key::Char('e'));
    assert!(t.logs().iter().any(|l| l == "ARCHIVOS_SELECCION_TODO 3"));
    t.key(Key::Delete);
    assert!(matches!(&t.files().dialog, Some(Dialog::ConfirmTrash(n)) if n.len() == 3));
    t.key(Key::Enter);
    assert!(t.names().is_empty());
    let papelera = fatfs_list(t.d, "/Papelera");
    for n in ["Bienvenida.txt", "notas.txt", "pagina.html"] {
        assert!(
            papelera.iter().any(|p| p == n),
            "{n} no está en {papelera:?}"
        );
    }
}

#[test]
fn archivos_copiar_varios() {
    let mut t = Driver::new();
    t.key(Key::Tab);
    t.select_named("Documentos");
    t.key(Key::Enter);
    t.combo(Mods::CTRL, Key::Char('a'));
    t.combo(Mods::CTRL, Key::Char('c'));
    t.key(Key::Backspace); // subir
    t.select_named("Proyectos");
    t.key(Key::Enter);
    t.combo(Mods::CTRL, Key::Char('v'));
    assert_eq!(t.names(), ["Bienvenida.txt", "notas.txt", "pagina.html"]);
    assert_eq!(
        fatfs_read(t.d, "/Proyectos/notas.txt").unwrap(),
        b"primera linea\nsegunda linea"
    );
}

fn rect_of(t: &Driver, kind: AppKind) -> Rect {
    t.window(kind)
}

#[test]
fn distribuciones_cuartos_mosaico_y_cascada() {
    let mut t = Driver::new();
    for k in [AppKind::Files, AppKind::Monitor, AppKind::Terminal] {
        t.d.open(Launch::App(k), t.now, CLOCK);
    }
    let work = t.d.window_manager().work_area();

    // Win+Z, ↓ (tercios), Enter: la Terminal a la primera zona y las otras completan.
    t.combo(Mods::WIN, Key::Char('z'));
    assert_eq!(t.d.overlay_name(), "distribuciones");
    t.key(Key::Down);
    t.key(Key::Enter);
    let thirds = Layout::Thirds.zones(work);
    assert_eq!(rect_of(&t, AppKind::Terminal), thirds[0]);
    let mut rest = [rect_of(&t, AppKind::Files), rect_of(&t, AppKind::Monitor)];
    rest.sort_by_key(|r| r.x);
    assert_eq!(rest, [thirds[1], thirds[2]]);
    assert_eq!(t.d.focused_app(), Some(AppKind::Terminal));

    // Con el mouse: la zona 2 de "cuartos".
    t.combo(Mods::WIN, Key::Char('z'));
    let zones = jarvis_desktop::shell::layout_zones(W, H);
    let i = zones
        .iter()
        .position(|(tpl, z, _)| LAYOUTS[*tpl] == Layout::Quarters && *z == 1)
        .unwrap();
    let r = zones[i].2;
    t.click_at(r.x + r.w / 2, r.y + r.h / 2, 1000);
    assert_eq!(
        rect_of(&t, AppKind::Terminal),
        Layout::Quarters.zones(work)[1]
    );

    // Win+← (la primera vez restaura, como en Windows, porque estaba acoplada en otro lado;
    // la segunda la acopla) y Win+↑: el cuarto de arriba a la izquierda; Win+↓ vuelve a la mitad.
    t.combo(Mods::WIN, Key::Left);
    t.combo(Mods::WIN, Key::Left);
    t.combo(Mods::WIN, Key::Up);
    let q = Layout::Quarters.zones(work);
    assert_eq!(rect_of(&t, AppKind::Terminal), q[0]);
    t.combo(Mods::WIN, Key::Down);
    assert_eq!(
        rect_of(&t, AppKind::Terminal),
        Layout::Halves.zones(work)[0]
    );

    // Mosaico con cinco ventanas: sin huecos ni superposiciones.
    for k in [AppKind::Editor, AppKind::Music] {
        t.d.open(Launch::App(k), t.now, CLOCK);
    }
    let shift_win = Mods {
        win: true,
        shift: true,
        ..Mods::NONE
    };
    t.combo(shift_win, Key::Char('T'));
    assert!(t.logs().iter().any(|l| l == "VENTANAS_MOSAICO 5"));
    let rects: Vec<Rect> =
        t.d.window_manager()
            .windows()
            .iter()
            .map(|w| w.rect)
            .collect();
    let area: i64 = rects.iter().map(|r| r.w as i64 * r.h as i64).sum();
    assert_eq!(
        area,
        work.w as i64 * work.h as i64,
        "cubren la zona de trabajo"
    );
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            assert!(a.intersection(b).is_none(), "{a:?} y {b:?} se superponen");
        }
    }
    // Cascada: en escalera.
    t.combo(shift_win, Key::Char('C'));
    let xs: Vec<i32> =
        t.d.window_manager()
            .windows()
            .iter()
            .map(|w| w.rect.x)
            .collect();
    assert!(xs.windows(2).all(|p| p[1] > p[0]), "{xs:?}");
}

#[test]
fn arrastrar_a_una_esquina_acopla_un_cuarto() {
    let mut t = Driver::new();
    t.d.open(Launch::App(AppKind::Monitor), t.now, CLOCK);
    let r = t.window(AppKind::Monitor);
    t.move_to(r.x + 200, r.y + 10, false);
    t.move_to(r.x + 200, r.y + 10, true);
    t.move_to(W as i32 - 1, H as i32 - 5, true);
    t.move_to(W as i32 - 1, H as i32 - 5, false);
    let work = t.d.window_manager().work_area();
    assert_eq!(t.window(AppKind::Monitor), Layout::Quarters.zones(work)[3]);
}

#[test]
fn el_selector_de_distribuciones_se_dibuja_igual_por_partes() {
    let mut t = Driver::new();
    t.d.open(Launch::App(AppKind::Files), t.now, CLOCK);
    let (mut bg_buf, mut frame_buf) = buffers();
    let mut bg = canvas(&mut bg_buf);
    t.d.draw_background(&mut bg);
    t.now += 1000;
    t.d.render(&mut canvas(&mut frame_buf), &mut bg, t.now, CLOCK);
    t.combo(Mods::WIN, Key::Char('z'));
    for k in [Key::Right, Key::Right, Key::Down] {
        t.key(k);
        t.now += 40;
        t.d.render(&mut canvas(&mut frame_buf), &mut bg, t.now, CLOCK);
    }
    let mut full = vec![0u8; W * H * 4];
    t.d.invalidate();
    t.d.render(&mut canvas(&mut full), &mut bg, t.now, CLOCK);
    assert!(frame_buf == full, "el render por partes difiere");
}
