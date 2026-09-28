//! El servicio de sincronización: une el motor ([`jarvis_sync`]) con el disco y la red.
//!
//! Con un código de emparejado en la Configuración, se conecta al relé, manda el manifiesto y,
//! cada 2 segundos, revisa `/Sincronizado` y manda lo que cambió. Lo que llega se escribe en el
//! disco; los borrados remotos van a la Papelera. El estado queda en `/Sistema/sync.db`.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use jarvis_fs::{BlockDevice, FileSystem, Timestamp};
use jarvis_gfx::clock::DateTime;
use jarvis_sync::wire::{self, Deframer, Sealer};
use jarvis_sync::{Action, Engine, Group, hash};

use crate::apps::timestamp;
use crate::config::parse_server;
use crate::files::{FilesApp, basename, join, parent};
use crate::system::{Outbox, StreamEvent};

pub const DIR: &str = "/Sincronizado";
pub const DB: &str = "/Sistema/sync.db";
const SCAN_EVERY_MS: u64 = 2000;
/// Los archivos más grandes no se mandan (entran en un mensaje de hasta 9 MiB).
pub const MAX_FILE: usize = 8 * 1024 * 1024;
/// La etiqueta de las conexiones para el firewall.
pub const APP: &str = "sincronizacion";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Sin código de emparejado.
    Off,
    Connecting,
    /// Conectado al relé.
    Online,
    /// Sin relé: reintenta solo.
    Offline,
}

pub struct SyncService {
    engine: Option<Engine>,
    sealer: Option<Sealer>,
    group: Option<Group>,
    code: String,
    relay: String,
    stream: Option<u32>,
    connected: bool,
    deframer: Deframer,
    next_try: u64,
    backoff: u64,
    next_scan: u64,
    pub status: Status,
    /// El nombre de la otra máquina, cuando llegó su manifiesto.
    pub peer: Option<String>,
    pub sent: u32,
    pub received: u32,
    /// Líneas para el log serie (los tests las esperan).
    pub logs: Vec<String>,
}

impl Default for SyncService {
    fn default() -> SyncService {
        SyncService {
            engine: None,
            sealer: None,
            group: None,
            code: String::new(),
            relay: String::new(),
            stream: None,
            connected: false,
            deframer: Deframer::default(),
            next_try: 0,
            backoff: 2000,
            next_scan: 0,
            status: Status::Off,
            peer: None,
            sent: 0,
            received: 0,
            logs: Vec::new(),
        }
    }
}

/// Un reloj que crece entre reinicios (para el contador del cifrado): microsegundos
/// aproximados desde 2000, con los milisegundos desde el arranque para desempatar.
fn epoch_us(clock: Option<DateTime>, now_ms: u64) -> u64 {
    let secs = clock.map_or(0, |t| {
        let days = (t.year.saturating_sub(2000) as u64) * 372 + t.month as u64 * 31 + t.day as u64;
        days * 86_400 + t.hour as u64 * 3600 + t.minute as u64 * 60 + t.second as u64
    });
    secs * 1_000_000 + (now_ms % 1_000_000) * 1000
}

fn mtime(t: Timestamp) -> u64 {
    let (d, h) = t.to_fat();
    ((d as u64) << 16) | h as u64
}

impl SyncService {
    /// Aplica la configuración: con otro código o relé, reconecta desde cero.
    #[allow(clippy::too_many_arguments)]
    pub fn configure<D: BlockDevice>(
        &mut self,
        code: &str,
        relay: &str,
        name: &str,
        fs: &mut FileSystem<D>,
        now_ms: u64,
        clock: Option<DateTime>,
        out: &mut Outbox,
    ) {
        if code == self.code && relay == self.relay && self.engine.is_some() == !code.is_empty() {
            return;
        }
        if let Some(id) = self.stream.take() {
            out.close_stream(id);
        }
        *self = SyncService {
            code: code.to_string(),
            relay: relay.to_string(),
            logs: core::mem::take(&mut self.logs),
            ..SyncService::default()
        };
        let Some(group) = Group::from_code(code) else {
            return;
        };
        let now = timestamp(clock);
        if !fs.exists(DIR) {
            let _ = fs.mkdir(DIR, now);
        }
        let saved = fs
            .read_file(DB)
            .ok()
            .and_then(|d| Engine::load(&String::from_utf8_lossy(&d)));
        let mut engine = saved.unwrap_or_else(|| {
            // Un id de máquina al azar (lo suficiente para distinguir dos o tres máquinas).
            let seed = format!("{code}{name}{}", epoch_us(clock, now_ms));
            let h = hash(seed.as_bytes());
            let id = u32::from_le_bytes([h[0], h[1], h[2], h[3]]).max(1);
            Engine::new(id, name)
        });
        engine.name = name.to_string();
        self.sealer = Some(Sealer::new(&group, engine.machine, epoch_us(clock, now_ms)));
        self.logs.push(format!(
            "SYNC_GRUPO {} maquina {}",
            &group.id_hex()[..8],
            engine.machine
        ));
        self.engine = Some(engine);
        self.group = Some(group);
        self.status = Status::Offline;
    }

    /// Una vez por segundo: conectar si hace falta y revisar la carpeta.
    pub fn tick<D: BlockDevice>(
        &mut self,
        fs: &mut FileSystem<D>,
        now_ms: u64,
        clock: Option<DateTime>,
        out: &mut Outbox,
    ) {
        if self.engine.is_none() {
            return;
        }
        if self.stream.is_none() && now_ms >= self.next_try {
            let Some((host, port)) = parse_server(&self.relay) else {
                return;
            };
            self.stream = Some(out.connect_as(host, port, APP));
            self.status = Status::Connecting;
            // Si no conecta, el reintento espera cada vez más (hasta 30 s).
            self.next_try = now_ms + self.backoff;
            self.backoff = (self.backoff * 2).min(30_000);
        }
        if self.connected && now_ms >= self.next_scan {
            self.next_scan = now_ms + SCAN_EVERY_MS;
            let changed = self.scan(fs);
            if !changed.is_empty() {
                self.send(&changed, fs, out);
                self.save(fs, clock);
            }
        }
    }

    /// Un evento de conexión. `true` si era la del relé.
    pub fn stream_event<D: BlockDevice>(
        &mut self,
        id: u32,
        event: &StreamEvent,
        fs: &mut FileSystem<D>,
        now_ms: u64,
        clock: Option<DateTime>,
        out: &mut Outbox,
    ) -> bool {
        if self.stream != Some(id) {
            return false;
        }
        match event {
            StreamEvent::Connected => {
                self.connected = true;
                self.status = Status::Online;
                self.backoff = 2000;
                self.deframer.clear();
                self.logs.push("SYNC_CONECTADO".into());
                if let Some(g) = &self.group {
                    out.send(id, wire::hello(g));
                }
                // Primero lo que cambió mientras no había conexión, así el manifiesto es actual.
                self.scan(fs);
                if let Some(m) = self.engine.as_ref().map(|e| e.manifest()) {
                    self.seal_and_send(id, &m, out);
                }
                self.save(fs, clock);
            }
            StreamEvent::Data(data) => {
                self.deframer.push(data);
                loop {
                    match self.deframer.next_frame() {
                        Ok(Some(f)) => self.on_frame(&f, fs, clock, out),
                        Ok(None) => break,
                        Err(_) => {
                            out.close_stream(id);
                            break;
                        }
                    }
                }
            }
            StreamEvent::Closed(why) => {
                self.stream = None;
                self.connected = false;
                self.status = Status::Offline;
                self.peer = None;
                self.logs.push(format!(
                    "SYNC_DESCONECTADO {}",
                    why.as_deref().unwrap_or("")
                ));
                self.next_try = self.next_try.max(now_ms + 2000);
            }
        }
        true
    }

    fn on_frame<D: BlockDevice>(
        &mut self,
        frame: &[u8],
        fs: &mut FileSystem<D>,
        clock: Option<DateTime>,
        out: &mut Outbox,
    ) {
        let (Some(sealer), Some(engine)) = (self.sealer.as_mut(), self.engine.as_mut()) else {
            return;
        };
        let Some(plain) = sealer.open(frame) else {
            self.logs.push("SYNC_MENSAJE_INVALIDO".into());
            return;
        };
        let Some(reply) = engine.receive(&plain) else {
            return;
        };
        if let Some(p) = reply.peer {
            // Si la otra se conectó después, no vio nuestro manifiesto: se lo mandamos (una sola
            // vez por conexión, así no se contestan para siempre).
            if self.peer.is_none() {
                self.logs.push(format!("SYNC_PAR {p}"));
                if let (Some(id), Some(m)) =
                    (self.stream, self.engine.as_ref().map(|e| e.manifest()))
                {
                    self.seal_and_send(id, &m, out);
                }
            }
            self.peer = Some(p);
        }
        let now = timestamp(clock);
        for a in reply.actions {
            self.received += 1;
            match a {
                Action::Write { path, data } => {
                    let full = join(DIR, &path);
                    let dir = parent(&full);
                    if mkdir_all(fs, &dir, now) && fs.write_file(&full, &data, now).is_ok() {
                        self.logs.push(format!("SYNC_RECIBIDO {path}"));
                    }
                }
                Action::Trash { path } => {
                    let full = join(DIR, &path);
                    if fs.exists(&full) && FilesApp::move_to_trash(fs, &full, now).is_ok() {
                        self.logs.push(format!("SYNC_PAPELERA {path}"));
                    }
                }
                Action::Rename { from, to } => {
                    let full = join(DIR, &from);
                    if fs.rename(&full, basename(&to)).is_ok() {
                        self.logs.push(format!("SYNC_CONFLICTO {to}"));
                    }
                }
            }
        }
        if !reply.push.is_empty() {
            self.send(&reply.push, fs, out);
        }
        self.save(fs, clock);
    }

    /// Recorre la carpeta y le cuenta al motor lo que hay; devuelve lo que cambió.
    fn scan<D: BlockDevice>(&mut self, fs: &mut FileSystem<D>) -> Vec<String> {
        let Some(engine) = self.engine.as_mut() else {
            return Vec::new();
        };
        let mut files = Vec::new();
        let mut dirs = Vec::from([String::new()]);
        while let Some(rel) = dirs.pop() {
            let Ok(list) = fs.list(&join(DIR, &rel)) else {
                // Si no se puede leer la carpeta, no se informa nada (no son borrados).
                return Vec::new();
            };
            for e in list {
                if e.name == "." || e.name == ".." {
                    continue;
                }
                let path = if rel.is_empty() {
                    e.name.clone()
                } else {
                    format!("{rel}/{}", e.name)
                };
                if e.is_dir {
                    dirs.push(path);
                    continue;
                }
                let m = mtime(e.modified);
                let h = if engine.stale(&path, e.size, m) {
                    match fs.read_file(&join(DIR, &path)) {
                        Ok(d) => hash(&d),
                        Err(_) => continue,
                    }
                } else {
                    engine.known_hash(&path).unwrap_or_default()
                };
                files.push((path, e.size, m, h));
            }
        }
        engine.scan(&files)
    }

    fn send<D: BlockDevice>(&mut self, paths: &[String], fs: &mut FileSystem<D>, out: &mut Outbox) {
        let (Some(id), true) = (self.stream, self.connected) else {
            return;
        };
        for p in paths {
            let data = fs.read_file(&join(DIR, p)).unwrap_or_default();
            if data.len() > MAX_FILE {
                self.logs.push(format!("SYNC_MUY_GRANDE {p}"));
                continue;
            }
            let Some(m) = self.engine.as_mut().and_then(|e| e.change(p, &data)) else {
                continue;
            };
            self.seal_and_send(id, &m, out);
            self.sent += 1;
            self.logs.push(format!("SYNC_ENVIADO {p}"));
        }
    }

    fn seal_and_send(&mut self, id: u32, plain: &[u8], out: &mut Outbox) {
        if let Some(s) = self.sealer.as_mut() {
            out.send(id, wire::frame(&s.seal(plain)));
        }
    }

    fn save<D: BlockDevice>(&self, fs: &mut FileSystem<D>, clock: Option<DateTime>) {
        if let Some(e) = &self.engine {
            let now = timestamp(clock);
            if mkdir_all(fs, "/Sistema", now) {
                let _ = fs.write_file(DB, e.save().as_bytes(), now);
            }
        }
    }
}

/// Crea las carpetas que falten de `dir`.
fn mkdir_all<D: BlockDevice>(fs: &mut FileSystem<D>, dir: &str, now: Timestamp) -> bool {
    let mut cur = String::new();
    for part in dir.split('/').filter(|s| !s.is_empty()) {
        cur.push('/');
        cur.push_str(part);
        if !fs.exists(&cur) && fs.mkdir(&cur, now).is_err() {
            return false;
        }
    }
    true
}
