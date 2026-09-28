//! Dos escritorios completos (cada uno con su disco en memoria) sincronizando `/Sincronizado`
//! a través de un relé simulado: lo que uno manda por la conexión le llega al otro.

mod common;

use common::*;
use jarvis_desktop::input::{Event, Mods};
use jarvis_desktop::system::SystemStats;
use jarvis_desktop::{StreamEvent, StreamOp};
use jarvis_fs::Timestamp;

const CODE: &str = "ABCDE-FGHJK-LMNPQ-RSTUV";

struct Node {
    t: Driver,
    stream: Option<u32>,
    /// Lo que mandó y todavía no le llegó al otro (sin el saludo al relé).
    outgoing: Vec<u8>,
    hello_seen: bool,
}

impl Node {
    fn new(name: &str) -> Node {
        let mut t = Driver::new();
        let mut cfg = t.d.config().clone();
        cfg.sync_code = CODE.into();
        cfg.hostname = name.into();
        t.d.set_config(cfg);
        Node {
            t,
            stream: None,
            outgoing: Vec::new(),
            hello_seen: false,
        }
    }

    /// Pasa un segundo: el escritorio revisa la carpeta y pide lo que necesita.
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
                    assert_eq!((r.host.as_str(), r.port), ("10.0.2.2", 8120));
                    self.stream = Some(r.id);
                    self.t.d.stream_event(r.id, StreamEvent::Connected);
                    self.collect();
                    return;
                }
                StreamOp::Send(_, data) => self.outgoing.extend(data),
                StreamOp::Close(_) => self.stream = None,
            }
        }
    }

    /// Lo que va al otro: el relé saca el saludo (`JSR1` + grupo) y reenvía el resto.
    fn take_for_peer(&mut self) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.outgoing);
        if !self.hello_seen && out.len() >= 24 {
            assert_eq!(&out[4..8], b"JSR1");
            out.drain(..24);
            self.hello_seen = true;
        }
        out
    }

    fn deliver(&mut self, data: Vec<u8>) {
        if let (Some(id), false) = (self.stream, data.is_empty()) {
            self.t.d.stream_event(id, StreamEvent::Data(data));
            self.collect();
        }
    }

    fn write(&mut self, path: &str, data: &[u8]) {
        let fs = self.t.d.fs_mut().unwrap();
        fs.write_file(path, data, Timestamp::EPOCH).unwrap();
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        self.t.d.fs_mut().unwrap().read_file(path).ok()
    }
}

fn run(a: &mut Node, b: &mut Node, seconds: u32) {
    for _ in 0..seconds {
        a.second();
        b.second();
        for _ in 0..4 {
            let (to_b, to_a) = (a.take_for_peer(), b.take_for_peer());
            b.deliver(to_b);
            a.deliver(to_a);
        }
    }
}

#[test]
fn dos_escritorios_sincronizan_crear_editar_y_borrar() {
    let mut a = Node::new("PC1");
    let mut b = Node::new("PC2");
    run(&mut a, &mut b, 2);
    assert!(a.t.logs().iter().any(|l| l == "SYNC_PAR PC2"));

    a.write("/Sincronizado/hola.txt", b"desde A");
    run(&mut a, &mut b, 3);
    assert_eq!(
        b.read("/Sincronizado/hola.txt").as_deref(),
        Some(&b"desde A"[..])
    );

    b.write("/Sincronizado/hola.txt", b"editado en B");
    run(&mut a, &mut b, 3);
    assert_eq!(
        a.read("/Sincronizado/hola.txt").as_deref(),
        Some(&b"editado en B"[..])
    );

    a.t.d
        .fs_mut()
        .unwrap()
        .remove("/Sincronizado/hola.txt")
        .unwrap();
    run(&mut a, &mut b, 3);
    assert!(b.read("/Sincronizado/hola.txt").is_none());
    assert!(
        b.read("/Papelera/hola.txt").is_some(),
        "el borrado remoto va a la Papelera"
    );
    // El estado quedó guardado.
    let db = String::from_utf8(a.read("/Sistema/sync.db").unwrap()).unwrap();
    assert!(db.starts_with("jarvis-sync 1\n"));
}
