//! Dos máquinas simuladas en memoria: cada una con su carpeta, su motor y su cifrado, unidas por
//! una "red" que entrega los mensajes cuando se le pide (o no, si están desconectadas).

use std::collections::BTreeMap;

use jarvis_sync::engine::conflict_name;
use jarvis_sync::wire::Sealer;
use jarvis_sync::{Action, Engine, Group, hash};

const CODE: &str = "ABCDE-FGHJK-LMNPQ-RSTUV";

struct Machine {
    engine: Engine,
    sealer: Sealer,
    files: BTreeMap<String, Vec<u8>>,
    trash: Vec<String>,
    tick: u64,
}

impl Machine {
    fn new(id: u32, name: &str) -> Machine {
        Machine {
            engine: Engine::new(id, name),
            sealer: Sealer::new(&Group::from_code(CODE).unwrap(), id, 0),
            files: BTreeMap::new(),
            trash: Vec::new(),
            tick: 0,
        }
    }

    fn write(&mut self, p: &str, data: &str) {
        self.tick += 1;
        self.files.insert(p.into(), data.as_bytes().to_vec());
    }

    /// Lo que la carpeta cambió desde la última vez, cifrado.
    fn scan(&mut self) -> Vec<Vec<u8>> {
        self.tick += 1;
        let list: Vec<_> = self
            .files
            .iter()
            .map(|(p, d)| (p.clone(), d.len() as u32, self.tick, hash(d)))
            .collect();
        let changed = self.engine.scan(&list);
        self.send(changed)
    }

    fn send(&mut self, paths: Vec<String>) -> Vec<Vec<u8>> {
        paths
            .into_iter()
            .filter_map(|p| {
                let data = self.files.get(&p).cloned().unwrap_or_default();
                self.engine.change(&p, &data)
            })
            .map(|m| self.sealer.seal(&m))
            .collect()
    }

    fn hello(&mut self) -> Vec<Vec<u8>> {
        let m = self.engine.manifest();
        vec![self.sealer.seal(&m)]
    }

    fn receive(&mut self, msgs: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for m in msgs {
            let plain = self.sealer.open(&m).expect("mensaje válido");
            let r = self.engine.receive(&plain).expect("mensaje entendible");
            for a in r.actions {
                match a {
                    Action::Write { path, data } => {
                        self.files.insert(path, data);
                    }
                    Action::Trash { path } => {
                        self.files.remove(&path);
                        self.trash.push(path);
                    }
                    Action::Rename { from, to } => {
                        let d = self.files.remove(&from).unwrap();
                        self.files.insert(to, d);
                    }
                }
            }
            out.extend(self.send(r.push));
        }
        out
    }
}

/// Entrega mensajes de ida y vuelta, y escanea las dos, hasta que no queda nada por mandar.
fn settle(a: &mut Machine, b: &mut Machine, mut to_b: Vec<Vec<u8>>, mut to_a: Vec<Vec<u8>>) {
    for _ in 0..20 {
        let from_b = b.receive(std::mem::take(&mut to_b));
        let from_a = a.receive(std::mem::take(&mut to_a));
        to_a = from_b;
        to_b = from_a;
        to_b.extend(a.scan());
        to_a.extend(b.scan());
        if to_a.is_empty() && to_b.is_empty() {
            assert_eq!(a.files, b.files, "las dos carpetas quedan iguales");
            return;
        }
    }
    panic!("no se estabiliza");
}

fn pair() -> (Machine, Machine) {
    let mut a = Machine::new(1, "PC1");
    let mut b = Machine::new(2, "PC2");
    let (ha, hb) = (a.hello(), b.hello());
    settle(&mut a, &mut b, ha, hb);
    (a, b)
}

fn s(v: &[u8]) -> &str {
    std::str::from_utf8(v).unwrap()
}

#[test]
fn crear_en_una_y_renombrar_en_la_otra() {
    let (mut a, mut b) = pair();
    a.write("notas/plan.txt", "hola");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    assert_eq!(s(&b.files["notas/plan.txt"]), "hola");

    // Renombrar en B = borrar el viejo y crear el nuevo.
    let d = b.files.remove("notas/plan.txt").unwrap();
    b.files.insert("notas/plan final.txt".into(), d);
    let m = b.scan();
    settle(&mut a, &mut b, vec![], m);
    assert!(a.files.contains_key("notas/plan final.txt"));
    assert!(!a.files.contains_key("notas/plan.txt"));
    assert_eq!(
        a.trash,
        ["notas/plan.txt"],
        "el borrado remoto va a la Papelera"
    );
}

#[test]
fn editar_de_ida_y_vuelta_no_es_conflicto() {
    let (mut a, mut b) = pair();
    a.write("x.txt", "1");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    b.write("x.txt", "2");
    let m = b.scan();
    settle(&mut a, &mut b, vec![], m);
    a.write("x.txt", "3");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    assert_eq!(s(&b.files["x.txt"]), "3");
    assert_eq!(b.files.len(), 1, "sin copias de conflicto");
}

#[test]
fn conflicto_gana_el_mas_nuevo_y_queda_copia() {
    let (mut a, mut b) = pair();
    a.write("plan.txt", "base");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    // Las dos editan a la vez; B edita dos veces, así su reloj es más alto.
    a.write("plan.txt", "de A");
    b.write("plan.txt", "de B, primero");
    b.scan();
    b.write("plan.txt", "de B");
    let (ma, mb) = (a.scan(), b.scan());
    settle(&mut a, &mut b, ma, mb);
    assert_eq!(s(&a.files["plan.txt"]), "de B", "gana el más nuevo");
    let copy = conflict_name("plan.txt", "PC1");
    assert_eq!(copy, "plan (conflicto de PC1).txt");
    assert_eq!(s(&a.files[&copy]), "de A", "la otra versión no se pierde");
    assert_eq!(a.files.len(), 2);
}

#[test]
fn desconectadas_se_ponen_al_dia_al_volver() {
    let (mut a, mut b) = pair();
    a.write("a.txt", "a");
    a.write("comun.txt", "v1");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    // Sin red: A edita y borra, B crea.
    a.write("comun.txt", "v2");
    a.files.remove("a.txt");
    a.scan();
    b.write("b.txt", "b");
    b.scan();
    // Reconectan: cada una manda su manifiesto.
    let (ha, hb) = (a.hello(), b.hello());
    settle(&mut a, &mut b, ha, hb);
    assert_eq!(s(&b.files["comun.txt"]), "v2");
    assert!(!b.files.contains_key("a.txt"));
    assert!(a.files.contains_key("b.txt"));
}

#[test]
fn un_borrado_no_le_gana_a_una_edicion() {
    let (mut a, mut b) = pair();
    a.write("x.txt", "1");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    a.files.remove("x.txt");
    b.write("x.txt", "editado");
    b.scan();
    b.scan();
    let (ma, mb) = (a.scan(), b.scan());
    settle(&mut a, &mut b, ma, mb);
    assert_eq!(s(&a.files["x.txt"]), "editado");
}

#[test]
fn el_estado_se_guarda_y_se_recupera() {
    let (mut a, mut b) = pair();
    a.write("carpeta/con espacios.txt", "x");
    let m = a.scan();
    settle(&mut a, &mut b, m, vec![]);
    let text = a.engine.save();
    let back = Engine::load(&text).unwrap();
    assert_eq!(back.entries, a.engine.entries);
    assert_eq!(back.clock, a.engine.clock);
    assert_eq!(back.name, "PC1");
    // Una ruta que intenta salir de la carpeta se rechaza.
    let mut evil = Engine::new(9, "X");
    evil.entries.clear();
    let mut e = Engine::new(9, "X");
    e.scan(&[("../fuera".into(), 1, 1, hash(b"x"))]);
    let msg = e.change("../fuera", b"x").unwrap();
    assert!(evil.receive(&msg).is_none());
}
