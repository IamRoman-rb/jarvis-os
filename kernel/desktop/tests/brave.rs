//! Brave (ADR 0007) contra un puente falso: la app se conecta por el `Outbox`, saluda, arma la
//! página con los mosaicos, confirma cada cuadro y le pasa el mouse y el teclado a la página.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::apps::brave::{BAR_H, TABS_H};
use jarvis_desktop::remote::{
    self, Button, FromBrave, MouseKind, TabInfo, TabsState, ToBrave, compress_tile,
};
use jarvis_desktop::{AppKind, Key, Launch, Mods, StreamEvent, StreamOp};
use jarvis_gfx::Rect;

/// Abre Brave y devuelve el número de la conexión que pidió.
fn open(t: &mut Driver) -> u32 {
    t.d.open(Launch::App(AppKind::Brave), t.now, CLOCK);
    t.frame(); // el primer tick pide la conexión
    let reqs = t.d.take_requests();
    let c = reqs
        .streams
        .iter()
        .find_map(|op| match op {
            StreamOp::Connect(r) => Some(r.clone()),
            _ => None,
        })
        .expect("Brave pidió una conexión");
    assert_eq!(
        (c.host.as_str(), c.port, c.app.as_str()),
        ("10.0.2.2", 8119, "brave")
    );
    c.id
}

/// Lo que la app le mandó al puente desde la última vez.
fn sent(t: &mut Driver, id: u32) -> Vec<ToBrave> {
    let mut framer = remote::Framer::default();
    for op in t.d.take_requests().streams {
        if let StreamOp::Send(i, data) = op {
            assert_eq!(i, id);
            framer.push(&data);
        }
    }
    core::iter::from_fn(|| framer.next_message())
        .map(|b| ToBrave::decode(&b.unwrap()).unwrap())
        .collect()
}

fn deliver(t: &mut Driver, id: u32, m: FromBrave) {
    // En dos pedazos: el mensaje se tiene que armar igual.
    let bytes = m.encode();
    let (a, b) = bytes.split_at(bytes.len() / 2);
    t.d.stream_event(id, StreamEvent::Data(a.to_vec()));
    t.d.stream_event(id, StreamEvent::Data(b.to_vec()));
}

/// Zona de la página en la pantalla.
fn page(t: &Driver) -> Rect {
    let r = t.window(AppKind::Brave);
    let c =
        t.d.window_manager()
            .get(t.id(AppKind::Brave))
            .unwrap()
            .content();
    Rect::new(
        r.x + c.x,
        r.y + c.y + TABS_H + BAR_H,
        c.w,
        c.h - TABS_H - BAR_H,
    )
}

fn brave(t: &Driver) -> &jarvis_desktop::apps::brave::Brave {
    match t.d.app(AppKind::Brave) {
        Some(App::Brave(b)) => b,
        _ => panic!("Brave no está abierto"),
    }
}

#[test]
fn saluda_arma_la_pagina_y_confirma_cada_cuadro() {
    let mut t = Driver::new();
    let id = open(&mut t);
    t.d.stream_event(id, StreamEvent::Connected);
    let hello = sent(&mut t, id);
    let p = page(&t);
    assert!(
        matches!(&hello[0], ToBrave::Hello { token, w, h }
            if token.is_empty() && *w as i32 == p.w && *h as i32 == p.h),
        "{hello:?}"
    );
    assert_eq!(
        hello[1],
        ToBrave::Navigate("https://search.brave.com/".into())
    );
    assert!(
        t.logs()
            .iter()
            .any(|l| l == "BRAVE_CONECTADO 10.0.2.2:8119")
    );

    // Un cuadro: el primer mosaico rojo y el resto blanco.
    let red: Vec<u8> = [220u8, 30, 40].repeat(64 * 64);
    deliver(
        &mut t,
        id,
        FromBrave::Frame {
            w: p.w as u16,
            h: p.h as u16,
            tiles: vec![compress_tile(0, 0, 64, 64, &red)],
        },
    );
    deliver(
        &mut t,
        id,
        FromBrave::State(TabsState {
            tabs: vec![
                TabInfo {
                    title: "Brave Search".into(),
                    url: "https://search.brave.com/".into(),
                },
                TabInfo {
                    title: "Otra".into(),
                    url: "https://example.com/".into(),
                },
            ],
            active: 0,
            loading: false,
            can_back: false,
            can_forward: false,
        }),
    );
    assert_eq!(sent(&mut t, id), vec![ToBrave::FrameAck]);
    assert!(t.logs().iter().any(|l| l.starts_with("BRAVE_FRAME ")));
    assert_eq!(brave(&t).page_pixel(10, 10), Some([220, 30, 40]));
    assert_eq!(brave(&t).title(), "Brave · Brave Search");

    // En la pantalla: rojo arriba a la izquierda de la página, blanco más allá.
    let (mut bg, mut fr) = buffers();
    let mut bgc = canvas(&mut bg);
    let mut frame = canvas(&mut fr);
    t.now += jarvis_desktop::desktop::ANIM_MS * 2; // que termine la animación de abrir
    t.d.render(&mut frame, &mut bgc, t.now, CLOCK);
    let px = |x: i32, y: i32| frame.get(x, y).map(|c| (c.r, c.g, c.b));
    assert_eq!(px(p.x + 5, p.y + 5), Some((220, 30, 40)));
    assert_eq!(px(p.x + 100, p.y + 100), Some((255, 255, 255)));
}

#[test]
fn mouse_y_teclado_van_a_la_pagina() {
    let mut t = Driver::new();
    let id = open(&mut t);
    t.d.stream_event(id, StreamEvent::Connected);
    sent(&mut t, id);
    let p = page(&t);

    // Apretar, arrastrar y soltar sobre la página.
    t.move_to(p.x + 40, p.y + 30, false);
    t.move_to(p.x + 40, p.y + 30, true);
    t.move_to(p.x + 90, p.y + 30, true);
    t.move_to(p.x + 90, p.y + 30, false);
    let msgs = sent(&mut t, id);
    let mouse: Vec<(MouseKind, Button, i16, i16)> = msgs
        .iter()
        .filter_map(|m| match m {
            ToBrave::Mouse {
                kind, button, x, y, ..
            } => Some((*kind, *button, *x, *y)),
            _ => None,
        })
        .collect();
    assert_eq!(
        mouse,
        vec![
            (MouseKind::Move, Button::None, 40, 30),
            (MouseKind::Down, Button::Left, 40, 30),
            (MouseKind::Move, Button::None, 90, 30),
            (MouseKind::Up, Button::Left, 90, 30),
        ]
    );

    // Rueda y teclas.
    t.wheel(1);
    t.type_text("hola");
    t.key(Key::Enter);
    let msgs = sent(&mut t, id);
    assert!(matches!(msgs[0], ToBrave::Wheel { x: 90, y: 30, dy, .. } if dy > 0));
    let keys: Vec<String> = msgs
        .iter()
        .filter_map(|m| match m {
            ToBrave::Key { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(keys, ["h", "o", "l", "a", "Enter"]);

    // Ctrl+L: la barra de dirección. Lo escrito ya no va a la página.
    t.combo(Mods::CTRL, Key::Char('l'));
    t.type_text("brave.com");
    t.key(Key::Enter);
    assert_eq!(
        sent(&mut t, id),
        vec![ToBrave::Navigate("https://brave.com".into())]
    );
}

#[test]
fn sin_puente_avisa_y_reintenta() {
    let mut t = Driver::new();
    let id = open(&mut t);
    t.d.stream_event(
        id,
        StreamEvent::Closed(Some("10.0.2.2:8119 rechazó la conexión".into())),
    );
    assert!(brave(&t).error().unwrap().contains("cargo xtask run"));
    assert!(t.logs().iter().any(|l| l.starts_with("BRAVE_ERROR ")));
    // Un clic en el aviso reintenta.
    let p = page(&t);
    t.click_at(p.x + 100, p.y + 80, 1000);
    t.frame();
    let again = t.d.take_requests().streams;
    assert!(matches!(again.as_slice(), [StreamOp::Connect(r)] if r.id != id));
}

#[test]
fn el_firewall_puede_bloquear_a_brave() {
    let mut t = Driver::new();
    t.d.open(
        Launch::Terminal(Some("ufw deny out port 8119 app brave".into())),
        t.now,
        CLOCK,
    );
    t.d.open(Launch::App(AppKind::Brave), t.now, CLOCK);
    t.frame();
    let reqs = t.d.take_requests();
    assert!(reqs.streams.is_empty(), "no sale nada: {:?}", reqs.streams);
    assert!(brave(&t).error().unwrap().contains("firewall"));
    assert!(
        t.logs()
            .iter()
            .any(|l| l.starts_with("FIREWALL_BLOQUEO brave 10.0.2.2"))
    );
}
