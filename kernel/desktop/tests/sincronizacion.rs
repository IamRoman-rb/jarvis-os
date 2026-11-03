//! Dos escritorios completos (cada uno con su disco en memoria) sincronizando `/Sincronizado`
//! a través de un relé simulado: lo que uno manda por la conexión le llega al otro.

mod common;

use common::*;
use jarvis_desktop::input::{Event, Mods};
use jarvis_desktop::system::SystemStats;
use jarvis_desktop::{StreamEvent, StreamOp};
use jarvis_fs::{FileSystem, Timestamp};

const CODE: &str = "ABCDE-FGHJK-LMNPQ-RSTUV";

struct Node {
    t: Driver,
    stream: Option<u32>,
    /// Lo que mandó y todavía no le llegó al otro (sin el saludo al relé).
    outgoing: Vec<u8>,
    hello_seen: bool,
    /// ¿Tiene hora? (Sin reloj, el escritorio recibe `None`.)
    with_clock: bool,
    /// El contador del nonce de cada sobre que mandó, en orden.
    counters: Vec<u64>,
}

impl Node {
    fn new(name: &str) -> Node {
        let mut t = Driver::new();
        configure(&mut t, name);
        Node {
            t,
            stream: None,
            outgoing: Vec::new(),
            hello_seen: false,
            with_clock: true,
            counters: Vec::new(),
        }
    }

    /// Se apaga y vuelve a arrancar con el mismo disco: el tiempo desde el arranque vuelve a
    /// empezar y, si `with_clock` es falso, sin hora.
    fn reboot(&mut self, name: &str, with_clock: bool) {
        let old = std::mem::replace(&mut self.t, Driver::new());
        let disk = old.d.into_fs().unwrap().into_device();
        self.t.d = jarvis_desktop::Desktop::new(W, H, 300, Some(FileSystem::mount(disk).unwrap()));
        configure(&mut self.t, name);
        self.stream = None;
        self.outgoing.clear();
        self.hello_seen = false;
        self.with_clock = with_clock;
    }

    /// Pasa un segundo: el escritorio revisa la carpeta y pide lo que necesita.
    fn second(&mut self) {
        self.t.now += 1000;
        let clock = if self.with_clock { CLOCK } else { None };
        self.t.d.handle(Event::Mods(Mods::NONE), self.t.now, clock);
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
        // Cada marco: largo (4 bytes) + sobre (id de máquina, 4 + contador, 8 + cifrado).
        let mut rest = &out[..];
        while rest.len() >= 16 {
            let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            self.counters
                .push(u64::from_le_bytes(rest[8..16].try_into().unwrap()));
            rest = &rest[(4 + len).min(rest.len())..];
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

fn configure(t: &mut Driver, name: &str) {
    let mut cfg = t.d.config().clone();
    cfg.sync_code = CODE.into();
    cfg.hostname = name.into();
    t.d.set_config(cfg);
}

/// Una máquina se reinicia sin hora (sin RTC, o con la pila del CMOS gastada): el contador del
/// cifrado no puede volver atrás. Si volviera, repetiría nonces con la misma clave y la otra
/// máquina, que sigue prendida, descartaría todo lo que manda como repeticiones.
#[test]
fn reiniciar_sin_hora_no_repite_nonces() {
    let mut a = Node::new("PC1");
    let mut b = Node::new("PC2");
    run(&mut a, &mut b, 2);
    a.write("/Sincronizado/antes.txt", b"antes");
    run(&mut a, &mut b, 3);
    assert!(b.read("/Sincronizado/antes.txt").is_some());
    let before = std::mem::take(&mut a.counters);
    let last = *before.iter().max().unwrap();

    a.reboot("PC1", false);
    run(&mut a, &mut b, 2);
    // Después de reconectar (lo creado sin conexión mientras la otra seguía prendida no viaja
    // hasta que ella se reconecte: el manifiesto se contesta una vez por conexión).
    a.write("/Sincronizado/despues.txt", b"despues");
    run(&mut a, &mut b, 3);

    assert!(
        a.counters.len() >= 2,
        "volvió a mandar: manifiesto y archivo"
    );
    let first = a.counters[0];
    assert!(first > last, "el contador volvió atrás: {first} <= {last}");
    assert!(a.counters.windows(2).all(|w| w[0] < w[1]), "siempre crece");
    assert_eq!(
        b.read("/Sincronizado/despues.txt").as_deref(),
        Some(&b"despues"[..]),
        "la otra máquina aceptó lo nuevo"
    );
    assert!(!b.t.logs().iter().any(|l| l == "SYNC_MENSAJE_INVALIDO"));
    // La reserva quedó en el disco, para el próximo arranque.
    let db = String::from_utf8(a.read("/Sistema/sync.db").unwrap()).unwrap();
    let floor: u64 = db
        .lines()
        .find_map(|l| l.strip_prefix("nonce "))
        .unwrap()
        .parse()
        .unwrap();
    assert!(floor >= *a.counters.last().unwrap());
}
