//! El estado de la carpeta y las reglas.
//!
//! Por archivo (ruta relativa a `/Sincronizado`, con `/`) se guarda:
//! - el **hash** del contenido (SHA-256, 16 bytes; cero = borrado);
//! - un **reloj de Lamport** y la máquina que hizo el cambio: ordenan los cambios entre máquinas
//!   sin relojes de pared;
//! - el **origen** (`parent`): la versión que tenían las dos la última vez que coincidieron.
//!
//! Un cambio que llega cuyo origen no es la versión local, cuando la local también cambió, es un
//! **conflicto**: gana el reloj más alto (y, si empatan, el id de máquina), y la otra versión queda
//! como `nombre (conflicto de PC2).ext`, con el nombre de la máquina que perdió. Las dos máquinas
//! llegan al mismo resultado sin hablar más. Un borrado nunca le gana a una edición.
//!
//! Mensajes (en claro, antes del [`crate::wire::Sealer`]):
//! - `1` **manifiesto**: nombre de la máquina + todas sus entradas (sin contenido). Se manda al
//!   conectar; cada una envía lo que tiene más nuevo.
//! - `2` **cambio**: una entrada + el contenido (vacío si es un borrado).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

pub type Hash = [u8; 16];
pub const ZERO: Hash = [0; 16];

/// El hash del contenido de un archivo (nunca es [`ZERO`]).
pub fn hash(data: &[u8]) -> Hash {
    let d = Sha256::digest(data);
    let mut h = [0u8; 16];
    h.copy_from_slice(&d[..16]);
    if h == ZERO {
        h[0] = 1;
    }
    h
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Hash> {
    if s.len() != 32 {
        return None;
    }
    let mut h = ZERO;
    for (i, b) in h.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(h)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub hash: Hash,
    pub clock: u64,
    pub machine: u32,
    pub parent: Hash,
    /// La otra máquina tiene esta versión (la mandamos o vino de ella).
    pub synced: bool,
    /// Tamaño y fecha la última vez que se calculó el hash (para no releer lo que no cambió).
    pub size: u32,
    pub mtime: u64,
}

impl Entry {
    fn deleted(&self) -> bool {
        self.hash == ZERO
    }
    fn rank(&self) -> (u64, u32) {
        (self.clock, self.machine)
    }
    /// El origen de un cambio nuevo sobre esta versión.
    fn next_parent(&self) -> Hash {
        if self.synced { self.hash } else { self.parent }
    }
}

/// Lo que el llamador tiene que hacer en el disco (rutas relativas a `/Sincronizado`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Crear o reemplazar (con las carpetas que falten).
    Write { path: String, data: Vec<u8> },
    /// Mover a la Papelera.
    Trash { path: String },
    /// Renombrar la versión local (la copia del conflicto).
    Rename { from: String, to: String },
}

/// La respuesta a un mensaje: qué hacer y qué rutas mandarle a la otra máquina.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub actions: Vec<Action>,
    pub push: Vec<String>,
    /// El nombre de la otra máquina, si llegó su manifiesto.
    pub peer: Option<String>,
}

pub struct Engine {
    pub machine: u32,
    pub name: String,
    pub clock: u64,
    pub entries: BTreeMap<String, Entry>,
}

/// Una copia de conflicto: `notas/plan.txt` → `notas/plan (conflicto de PC2).txt`.
pub fn conflict_name(path: &str, machine: &str) -> String {
    let (dir, file) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let (stem, ext) = match file.rfind('.') {
        Some(i) if i > 0 => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    format!("{dir}{stem} (conflicto de {machine}){ext}")
}

impl Engine {
    pub fn new(machine: u32, name: &str) -> Engine {
        Engine {
            machine,
            name: name.to_string(),
            clock: 0,
            entries: BTreeMap::new(),
        }
    }

    /// Hay que releer el archivo para saber su hash (es nuevo o cambió su tamaño o fecha).
    pub fn stale(&self, path: &str, size: u32, mtime: u64) -> bool {
        self.entries
            .get(path)
            .is_none_or(|e| e.deleted() || e.size != size || e.mtime != mtime)
    }

    /// El hash que se conoce de un archivo (si no está [`Engine::stale`]).
    pub fn known_hash(&self, path: &str) -> Option<Hash> {
        self.entries.get(path).map(|e| e.hash)
    }

    /// Compara lo que hay en la carpeta (ruta, tamaño, fecha, hash) con lo anotado. Devuelve las
    /// rutas que cambiaron acá (creadas, editadas o borradas), para mandarlas.
    pub fn scan(&mut self, files: &[(String, u32, u64, Hash)]) -> Vec<String> {
        let mut changed = Vec::new();
        for (path, size, mtime, h) in files {
            match self.entries.get_mut(path) {
                Some(e) if e.hash == *h => {
                    e.size = *size;
                    e.mtime = *mtime;
                }
                other => {
                    let parent = other.map(|e| e.next_parent()).unwrap_or(ZERO);
                    self.clock += 1;
                    self.entries.insert(
                        path.clone(),
                        Entry {
                            hash: *h,
                            clock: self.clock,
                            machine: self.machine,
                            parent,
                            synced: false,
                            size: *size,
                            mtime: *mtime,
                        },
                    );
                    changed.push(path.clone());
                }
            }
        }
        let gone: Vec<String> = self
            .entries
            .iter()
            .filter(|(p, e)| !e.deleted() && !files.iter().any(|f| &f.0 == *p))
            .map(|(p, _)| p.clone())
            .collect();
        for path in gone {
            self.clock += 1;
            let e = self.entries.get_mut(&path).expect("está");
            e.parent = e.next_parent();
            e.hash = ZERO;
            e.clock = self.clock;
            e.machine = self.machine;
            e.synced = false;
            e.size = 0;
            e.mtime = 0;
            changed.push(path);
        }
        changed
    }

    /// El manifiesto para mandar al conectar.
    pub fn manifest(&self) -> Vec<u8> {
        let mut m = Vec::from([1u8]);
        put_str(&mut m, &self.name);
        m.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (p, e) in &self.entries {
            put_meta(&mut m, p, e);
        }
        m
    }

    /// El mensaje con un cambio local (`data` vacío si se borró). Queda como enviado.
    pub fn change(&mut self, path: &str, data: &[u8]) -> Option<Vec<u8>> {
        let e = self.entries.get_mut(path)?;
        e.synced = true;
        let mut m = Vec::from([2u8]);
        put_str(&mut m, &self.name);
        put_meta(&mut m, path, e);
        m.extend_from_slice(&(data.len() as u32).to_le_bytes());
        m.extend_from_slice(data);
        Some(m)
    }

    /// Procesa un mensaje de la otra máquina (ya descifrado).
    pub fn receive(&mut self, msg: &[u8]) -> Option<Reply> {
        let mut r = Reader(msg);
        match r.u8()? {
            1 => {
                let peer = r.str()?;
                let n = r.u32()? as usize;
                let mut theirs = BTreeMap::new();
                for _ in 0..n.min(1 << 20) {
                    let (p, e) = r.meta()?;
                    self.clock = self.clock.max(e.clock);
                    theirs.insert(p, e);
                }
                let mut reply = Reply {
                    peer: Some(peer),
                    ..Reply::default()
                };
                for (p, mine) in self.entries.iter_mut() {
                    match theirs.get(p) {
                        Some(t) if t.hash == mine.hash => {
                            mine.synced = true;
                            mine.parent = mine.hash;
                        }
                        Some(t) if mine.rank() > t.rank() => reply.push.push(p.clone()),
                        None if !mine.deleted() => reply.push.push(p.clone()),
                        _ => {}
                    }
                }
                Some(reply)
            }
            2 => {
                let peer = r.str()?;
                let (path, t) = r.meta()?;
                let len = r.u32()? as usize;
                let data = r.take(len)?.to_vec();
                if path.is_empty() || path.split('/').any(|s| s.is_empty() || s == "..") {
                    return None;
                }
                Some(self.apply(&peer, path, t, data))
            }
            _ => None,
        }
    }

    fn apply(&mut self, peer: &str, path: String, t: Entry, data: Vec<u8>) -> Reply {
        self.clock = self.clock.max(t.clock);
        let mut reply = Reply::default();
        let theirs = Entry {
            parent: t.hash,
            synced: true,
            size: 0,
            mtime: 0,
            ..t
        };
        let Some(mine) = self.entries.get(&path).cloned() else {
            if !t.deleted() {
                reply.actions.push(Action::Write {
                    path: path.clone(),
                    data,
                });
            }
            self.entries.insert(path, theirs);
            return reply;
        };
        if mine.hash == t.hash {
            let e = self.entries.get_mut(&path).expect("está");
            if t.rank() > e.rank() {
                e.clock = t.clock;
                e.machine = t.machine;
            }
            e.synced = true;
            e.parent = t.hash;
            return reply;
        }
        // ¿La versión local es la de origen del cambio (o no cambió desde la última común)?
        let clean = mine.hash == t.parent || (mine.synced && mine.parent == mine.hash);
        if clean || mine.deleted() {
            if t.deleted() {
                reply.actions.push(Action::Trash { path: path.clone() });
            } else {
                reply.actions.push(Action::Write {
                    path: path.clone(),
                    data,
                });
            }
            self.entries.insert(path, theirs);
            return reply;
        }
        // Conflicto: las dos cambiaron. Un borrado nunca le gana a una edición.
        if t.deleted() {
            let e = self.entries.get_mut(&path).expect("está");
            e.synced = false;
            reply.push.push(path);
            return reply;
        }
        if t.rank() > mine.rank() {
            // Gana la otra: la local queda como copia con el nombre de esta máquina.
            reply.actions.push(Action::Rename {
                from: path.clone(),
                to: conflict_name(&path, &self.name),
            });
            reply.actions.push(Action::Write {
                path: path.clone(),
                data,
            });
            self.entries.insert(path, theirs);
        } else {
            // Gana la local: la otra queda como copia con el nombre de su máquina.
            reply.actions.push(Action::Write {
                path: conflict_name(&path, peer),
                data,
            });
            let e = self.entries.get_mut(&path).expect("está");
            e.parent = t.hash;
            e.synced = false;
            reply.push.push(path);
        }
        reply
    }

    /// El estado como texto, para `/Sistema/sync.db`.
    pub fn save(&self) -> String {
        let mut s = format!(
            "jarvis-sync 1\nmaquina {}\nnombre {}\nreloj {}\n",
            self.machine, self.name, self.clock
        );
        for (p, e) in &self.entries {
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                hex(&e.hash),
                e.clock,
                e.machine,
                hex(&e.parent),
                u8::from(e.synced),
                e.size,
                e.mtime,
                p
            ));
        }
        s
    }

    pub fn load(text: &str) -> Option<Engine> {
        let mut lines = text.lines();
        if lines.next()? != "jarvis-sync 1" {
            return None;
        }
        let machine = lines.next()?.strip_prefix("maquina ")?.parse().ok()?;
        let name = lines.next()?.strip_prefix("nombre ")?;
        let clock = lines.next()?.strip_prefix("reloj ")?.parse().ok()?;
        let mut e = Engine::new(machine, name);
        e.clock = clock;
        for l in lines {
            let f: Vec<&str> = l.splitn(8, '\t').collect();
            if f.len() != 8 {
                continue;
            }
            e.entries.insert(
                f[7].to_string(),
                Entry {
                    hash: unhex(f[0])?,
                    clock: f[1].parse().ok()?,
                    machine: f[2].parse().ok()?,
                    parent: unhex(f[3])?,
                    synced: f[4] == "1",
                    size: f[5].parse().ok()?,
                    mtime: f[6].parse().ok()?,
                },
            );
        }
        Some(e)
    }
}

fn put_str(m: &mut Vec<u8>, s: &str) {
    m.extend_from_slice(&(s.len() as u16).to_le_bytes());
    m.extend_from_slice(s.as_bytes());
}

fn put_meta(m: &mut Vec<u8>, path: &str, e: &Entry) {
    put_str(m, path);
    m.extend_from_slice(&e.hash);
    m.extend_from_slice(&e.clock.to_le_bytes());
    m.extend_from_slice(&e.machine.to_le_bytes());
    m.extend_from_slice(&e.parent);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn hash(&mut self) -> Option<Hash> {
        self.take(16)?.try_into().ok()
    }
    fn str(&mut self) -> Option<String> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize;
        Some(String::from(core::str::from_utf8(self.take(n)?).ok()?))
    }
    fn meta(&mut self) -> Option<(String, Entry)> {
        let path = self.str()?;
        let e = Entry {
            hash: self.hash()?,
            clock: self.u64()?,
            machine: self.u32()?,
            parent: self.hash()?,
            synced: false,
            size: 0,
            mtime: 0,
        };
        Some((path, e))
    }
}
