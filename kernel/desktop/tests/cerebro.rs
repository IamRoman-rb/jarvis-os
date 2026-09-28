//! La consola con el cerebro (ADR 0008), con un `jarvis serve` simulado: el escritorio se
//! conecta con el token, manda los pedidos y muestra la respuesta a medida que llega.

mod common;

use common::*;
use jarvis_desktop::apps::App;
use jarvis_desktop::input::{Event, Mods};
use jarvis_desktop::system::SystemStats;
use jarvis_desktop::{AppKind, Key, Launch, StreamEvent, StreamOp};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

struct Brain {
    t: Driver,
    stream: Option<u32>,
    /// Los renglones que mandó el escritorio.
    sent: String,
}

impl Brain {
    fn new() -> Brain {
        let mut t = Driver::new();
        t.d.set_brain(8121, TOKEN);
        Brain {
            t,
            stream: None,
            sent: String::new(),
        }
    }

    fn second(&mut self) {
        self.t.now += 1000;
        self.t.d.handle(Event::Mods(Mods::NONE), self.t.now, CLOCK);
        self.t.d.set_stats(SystemStats::default());
        self.collect();
    }

    fn collect(&mut self) {
        for op in self.t.d.take_requests().streams {
            match op {
                StreamOp::Connect(r) => {
                    assert_eq!((r.host.as_str(), r.port), ("10.0.2.2", 8121));
                    self.stream = Some(r.id);
                    self.t.d.stream_event(r.id, StreamEvent::Connected);
                    self.collect();
                    return;
                }
                StreamOp::Send(_, data) => self.sent.push_str(&String::from_utf8(data).unwrap()),
                StreamOp::Close(_) => self.stream = None,
            }
        }
    }

    fn reply(&mut self, line: &str) {
        let id = self.stream.expect("conectado");
        self.t
            .d
            .stream_event(id, StreamEvent::Data(format!("{line}\n").into_bytes()));
        self.collect();
    }

    fn console(&self) -> String {
        match self.t.d.app(AppKind::Console) {
            Some(App::Console(c)) => c.text(),
            _ => panic!("la consola no está abierta"),
        }
    }

    fn ask(&mut self, text: &str) {
        self.t.type_text(text);
        self.t.key(Key::Enter);
        self.collect();
    }
}

fn connected() -> Brain {
    let mut b = Brain::new();
    b.second();
    assert!(
        b.sent
            .starts_with(&format!("{{\"t\":\"hola\",\"token\":\"{TOKEN}\"")),
        "{}",
        b.sent
    );
    b.reply(r#"{"t":"listo"}"#);
    assert!(b.t.logs().iter().any(|l| l == "CEREBRO_CONECTADO"));
    b.second();
    b.t.d.open(Launch::App(AppKind::Console), b.t.now, CLOCK);
    b.sent.clear();
    b
}

#[test]
fn la_respuesta_llega_de_a_pedazos() {
    let mut b = connected();
    b.ask("¿qué es un \"kernel\"?");
    assert_eq!(
        b.sent,
        "{\"t\":\"pedido\",\"id\":1,\"texto\":\"¿qué es un \\\"kernel\\\"?\",\"origen\":\"consola\"}\n"
    );
    b.reply(r#"{"t":"texto","id":1,"delta":"Es el núcleo"}"#);
    assert!(b.console().contains("Es el núcleo\n"));
    b.reply(r#"{"t":"texto","id":1,"delta":" del sistema.\nManeja el hardware."}"#);
    b.reply(r#"{"t":"fin","id":1}"#);
    let text = b.console();
    assert!(
        text.contains("Es el núcleo del sistema.\nManeja el hardware.\n"),
        "{text}"
    );
    assert!(
        b.t.logs()
            .iter()
            .any(|l| l == "JARVIS_HABLA: Es el núcleo del sistema."),
        "la esfera dice la primera línea"
    );
}

#[test]
fn las_ordenes_locales_siguen_siendo_locales() {
    let mut b = connected();
    b.ask("abrir monitor");
    assert!(b.sent.is_empty(), "no fue al cerebro: {}", b.sent);
    assert_eq!(b.t.d.focused_app(), Some(AppKind::Monitor));
}

#[test]
fn esc_cancela_y_un_corte_avisa() {
    let mut b = connected();
    b.ask("contame algo largo");
    b.sent.clear();
    b.t.key(Key::Escape);
    b.collect();
    assert_eq!(b.sent, "{\"t\":\"cancelar\",\"id\":1}\n");
    b.reply(r#"{"t":"fin","id":1}"#);

    b.ask("otra cosa");
    let id = b.stream.unwrap();
    b.t.d.stream_event(id, StreamEvent::Closed(None));
    assert!(b.console().contains("Se cortó la conexión con el cerebro."));
    // Reconecta solo.
    b.stream = None;
    b.second();
    b.second();
    assert!(b.stream.is_some(), "reintenta la conexión");
}

#[test]
fn sin_cerebro_la_consola_lo_dice() {
    let mut t = Driver::new();
    t.d.open(Launch::App(AppKind::Console), t.now, CLOCK);
    t.type_text("qué es un kernel");
    t.key(Key::Enter);
    let text = match t.d.app(AppKind::Console) {
        Some(App::Console(c)) => c.text(),
        _ => unreachable!(),
    };
    assert!(text.contains("no está conectado"), "{text}");
    assert!(t.d.take_requests().streams.is_empty());
}
