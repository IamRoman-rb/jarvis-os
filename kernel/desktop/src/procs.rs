//! Los programas de Linux vistos desde el escritorio (K11, ADR 0010).
//!
//! Un programa corre en el anillo 3, en su propia tarea del kernel. El disco (FAT32) y la
//! Terminal son de la tarea del escritorio, así que cuando un programa abre un archivo, escribe
//! en la pantalla o lee una línea, el kernel deja un [`ProcEvent`] y espera un [`ProcReply`].
//! Este módulo es ese "servidor":
//!
//! - archivos: cada pedido de `jarvis-linux` ([`FileOp`]) se hace sobre el FAT32; borrar manda a
//!   la Papelera (regla 13);
//! - consola: la salida va a la Terminal que lanzó el programa y las líneas que se tipean ahí son
//!   su entrada;
//! - red: `connect`/`send`/`recv` son conexiones largas del `Outbox`, así que pasan por el
//!   firewall con el nombre de app `programas` (regla 20).

use alloc::collections::VecDeque;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_fs::{BlockDevice, FileSystem};
use jarvis_linux::abi::errno;
use jarvis_linux::sys::{DirEntry, FileOp, FileReply, Meta, NetOp, NetReply};

use crate::system::{Outbox, StreamEvent};

/// La app con la que los programas aparecen en el firewall.
pub const FIREWALL_APP: &str = "programas";

/// Crear un proceso (lo pide la Terminal; lo hace el kernel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRequest {
    pub pid: u32,
    /// La ruta del ejecutable.
    pub path: String,
    /// El archivo entero.
    pub image: Vec<u8>,
    /// Su intérprete (`ld.so`), si es un programa dinámico: el kernel no puede leer el disco
    /// mientras carga (lo hace desde la tarea del escritorio, que es la dueña del disco).
    pub interp: Option<Vec<u8>>,
    pub argv: Vec<String>,
    pub envp: Vec<String>,
    pub cwd: String,
    /// Filas y columnas de la Terminal.
    pub size: (u16, u16),
}

/// Lo que llega de un proceso.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcEvent {
    Output {
        pid: u32,
        data: Vec<u8>,
    },
    /// Terminó (128 + señal si lo mató una señal). `why`: el motivo, si no fue normal.
    Exited {
        pid: u32,
        code: i32,
        why: Option<String>,
    },
    File {
        pid: u32,
        op: FileOp,
    },
    ReadLine {
        pid: u32,
        max: usize,
    },
    Net {
        pid: u32,
        op: NetOp,
    },
}

impl ProcEvent {
    pub fn pid(&self) -> u32 {
        match self {
            ProcEvent::Output { pid, .. }
            | ProcEvent::Exited { pid, .. }
            | ProcEvent::File { pid, .. }
            | ProcEvent::ReadLine { pid, .. }
            | ProcEvent::Net { pid, .. } => *pid,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcReply {
    File(FileReply),
    Line(Vec<u8>),
    Net(NetReply),
}

enum SockState {
    Connecting,
    Open,
    Closed,
}

struct Sock {
    stream: u32,
    state: SockState,
    rx: Vec<u8>,
    /// Un `recv` esperando (con cuántos bytes como mucho).
    waiting: Option<usize>,
}

struct Proc {
    pid: u32,
    /// Líneas tipeadas que el programa todavía no leyó. Un vector vacío es fin de la entrada.
    input: VecDeque<Vec<u8>>,
    /// El programa está esperando una línea (con cuántos bytes como mucho).
    reading: Option<usize>,
    socks: Vec<Sock>,
}

/// Los procesos vivos y lo que hay que contestarles.
#[derive(Default)]
pub struct Procs {
    list: Vec<Proc>,
    replies: Vec<(u32, ProcReply)>,
}

impl Procs {
    /// Un proceso nuevo (lo pidió la Terminal).
    pub fn started(&mut self, pid: u32) {
        self.list.push(Proc {
            pid,
            input: VecDeque::new(),
            reading: None,
            socks: Vec::new(),
        });
    }

    pub fn running(&self, pid: u32) -> bool {
        self.list.iter().any(|p| p.pid == pid)
    }

    pub fn take_replies(&mut self) -> Vec<(u32, ProcReply)> {
        core::mem::take(&mut self.replies)
    }

    /// La Terminal mandó una línea (vacía: Ctrl+D, fin de la entrada).
    pub fn input(&mut self, pid: u32, line: Vec<u8>) {
        if let Some(p) = self.list.iter_mut().find(|p| p.pid == pid) {
            p.input.push_back(line);
            Self::serve_read(p, &mut self.replies);
        }
    }

    fn serve_read(p: &mut Proc, replies: &mut Vec<(u32, ProcReply)>) {
        let Some(max) = p.reading else {
            return;
        };
        let Some(front) = p.input.front_mut() else {
            return;
        };
        let n = front.len().min(max.max(1));
        let data: Vec<u8> = front.drain(..n).collect();
        // Una línea leída a medias queda para la próxima lectura; la vacía (EOF) se consume.
        if front.is_empty() {
            p.input.pop_front();
        }
        p.reading = None;
        replies.push((p.pid, ProcReply::Line(data)));
    }

    /// El proceso terminó: se olvidan sus pedidos y se cierran sus conexiones.
    pub fn exited(&mut self, pid: u32, out: &mut Outbox) {
        if let Some(i) = self.list.iter().position(|p| p.pid == pid) {
            let p = self.list.remove(i);
            for s in p.socks {
                if !matches!(s.state, SockState::Closed) {
                    out.close_stream(s.stream);
                }
            }
        }
        self.replies.retain(|(p, _)| *p != pid);
    }

    /// Un pedido de archivos: se contesta enseguida.
    pub fn file<D: BlockDevice>(
        &mut self,
        pid: u32,
        op: FileOp,
        fs: Option<&mut FileSystem<D>>,
        now: jarvis_fs::Timestamp,
    ) {
        let r = match fs {
            Some(fs) => file_op(fs, op, now),
            None => FileReply::Error(errno::EIO),
        };
        self.replies.push((pid, ProcReply::File(r)));
    }

    /// El programa quiere leer de la Terminal.
    pub fn read_line(&mut self, pid: u32, max: usize) {
        if let Some(p) = self.list.iter_mut().find(|p| p.pid == pid) {
            p.reading = Some(max);
            Self::serve_read(p, &mut self.replies);
        } else {
            self.replies.push((pid, ProcReply::Line(Vec::new())));
        }
    }

    /// Un pedido de red.
    pub fn net(&mut self, pid: u32, op: NetOp, out: &mut Outbox) {
        let Some(p) = self.list.iter_mut().find(|p| p.pid == pid) else {
            return;
        };
        let reply = match op {
            NetOp::Connect { ip, port } => {
                let host = format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
                let stream = out.connect_as(&host, port, FIREWALL_APP);
                p.socks.push(Sock {
                    stream,
                    state: SockState::Connecting,
                    rx: Vec::new(),
                    waiting: None,
                });
                None // se contesta cuando se conecte (o falle)
            }
            NetOp::Send { id, data } => match p.socks.iter().find(|s| s.stream == id) {
                Some(s) if matches!(s.state, SockState::Open) => {
                    let n = data.len();
                    out.send(id, data);
                    Some(NetReply::Sent(n))
                }
                _ => Some(NetReply::Error(errno::EPIPE)),
            },
            NetOp::Recv { id, max } => match p.socks.iter_mut().find(|s| s.stream == id) {
                Some(s) if !s.rx.is_empty() => {
                    let n = s.rx.len().min(max.max(1));
                    Some(NetReply::Data(s.rx.drain(..n).collect()))
                }
                Some(s) if matches!(s.state, SockState::Closed) => Some(NetReply::Data(Vec::new())),
                Some(s) => {
                    s.waiting = Some(max);
                    None
                }
                None => Some(NetReply::Error(errno::ENOTCONN)),
            },
            NetOp::Close { id } => {
                if let Some(i) = p.socks.iter().position(|s| s.stream == id) {
                    let s = p.socks.remove(i);
                    if !matches!(s.state, SockState::Closed) {
                        out.close_stream(id);
                    }
                }
                Some(NetReply::Done)
            }
        };
        if let Some(r) = reply {
            self.replies.push((pid, ProcReply::Net(r)));
        }
    }

    /// Un evento de una conexión. `true` si era de un programa.
    pub fn stream_event(&mut self, id: u32, ev: &StreamEvent) -> bool {
        for p in &mut self.list {
            let Some(s) = p.socks.iter_mut().find(|s| s.stream == id) else {
                continue;
            };
            let connecting = matches!(s.state, SockState::Connecting);
            match ev {
                StreamEvent::Connected => {
                    s.state = SockState::Open;
                    self.replies
                        .push((p.pid, ProcReply::Net(NetReply::Connected(id))));
                }
                StreamEvent::Data(d) => {
                    s.rx.extend_from_slice(d);
                    if let Some(max) = s.waiting.take() {
                        let n = s.rx.len().min(max.max(1));
                        let data = s.rx.drain(..n).collect();
                        self.replies
                            .push((p.pid, ProcReply::Net(NetReply::Data(data))));
                    }
                }
                StreamEvent::Closed(why) => {
                    s.state = SockState::Closed;
                    if connecting {
                        // El firewall o el otro lado dijeron que no.
                        let e = match why {
                            Some(w) if w.contains("firewall") => errno::EACCES,
                            Some(w) if w.contains("DNS") || w.contains("ruta") => {
                                errno::ENETUNREACH
                            }
                            _ => errno::ECONNREFUSED,
                        };
                        p.socks.retain(|x| x.stream != id);
                        self.replies
                            .push((p.pid, ProcReply::Net(NetReply::Error(e))));
                    } else if let Some(_max) = s.waiting.take() {
                        let data = core::mem::take(&mut s.rx);
                        self.replies
                            .push((p.pid, ProcReply::Net(NetReply::Data(data))));
                    }
                }
            }
            return true;
        }
        false
    }
}

/// Segundos desde 1970 de una fecha del FAT32 (tomada como UTC).
fn unix_seconds(t: jarvis_fs::Timestamp) -> u64 {
    // days_from_civil (Howard Hinnant): días desde 1970-01-01 del calendario gregoriano.
    let (y, m, d) = (i64::from(t.year), i64::from(t.month), i64::from(t.day));
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs =
        days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.minute) * 60 + i64::from(t.second);
    secs.max(0) as u64
}

/// Un pedido de archivos sobre el FAT32. Los errores, como `errno`.
pub fn file_op<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    op: FileOp,
    now: jarvis_fs::Timestamp,
) -> FileReply {
    use crate::files::{FilesApp, basename, join, parent};
    use jarvis_fs::FsError;
    let code = |e: FsError| {
        FileReply::Error(match e {
            FsError::NotFound => errno::ENOENT,
            FsError::AlreadyExists => errno::EEXIST,
            FsError::NotADirectory => errno::ENOTDIR,
            FsError::IsADirectory => errno::EISDIR,
            FsError::NoSpace => errno::ENOSPC,
            FsError::InvalidName => errno::EINVAL,
            FsError::RootNotAllowed | FsError::MoveIntoItself => errno::EPERM,
            _ => errno::EIO,
        })
    };
    let done = |r: Result<(), FsError>| match r {
        Ok(()) => FileReply::Done,
        Err(e) => code(e),
    };
    match op {
        FileOp::Stat(path) => match fs.stat(&path) {
            Ok(s) => FileReply::Meta(Meta {
                is_dir: s.is_dir,
                size: u64::from(s.size),
                mtime: unix_seconds(s.modified),
            }),
            // La raíz no tiene entrada propia.
            Err(_) if path == "/" => FileReply::Meta(Meta {
                is_dir: true,
                size: 0,
                mtime: 0,
            }),
            Err(e) => code(e),
        },
        FileOp::Read(path) => match fs.read_file(&path) {
            Ok(d) => FileReply::Data(d),
            Err(e) => code(e),
        },
        FileOp::Write(path, data) => done(fs.write_file(&path, &data, now)),
        FileOp::List(path) => match fs.list(&path) {
            Ok(l) => FileReply::List(
                l.into_iter()
                    .filter(|e| e.name != "." && e.name != "..")
                    .map(|e| DirEntry {
                        name: e.name,
                        is_dir: e.is_dir,
                    })
                    .collect(),
            ),
            Err(e) => code(e),
        },
        FileOp::Mkdir(path) => done(fs.mkdir(&path, now)),
        FileOp::Remove(path) => match fs.stat(&path) {
            Ok(s) if s.is_dir => FileReply::Error(errno::EISDIR),
            Ok(_) => match FilesApp::move_to_trash(fs, &path, now) {
                Ok(_) => FileReply::Done,
                Err(e) => code(e),
            },
            Err(e) => code(e),
        },
        FileOp::Rmdir(path) => match fs.list(&path) {
            Ok(l) if l.iter().any(|e| e.name != "." && e.name != "..") => {
                FileReply::Error(errno::ENOTEMPTY)
            }
            Ok(_) => done(fs.remove(&path)),
            Err(e) => code(e),
        },
        FileOp::Rename(from, to) => {
            if fs.stat(&from).is_err() {
                return FileReply::Error(errno::ENOENT);
            }
            // rename() reemplaza el destino si existe: lo que se pisa va a la Papelera.
            if fs.stat(&to).is_ok_and(|s| !s.is_dir) {
                let _ = FilesApp::move_to_trash(fs, &to, now);
            }
            let (from_dir, to_dir) = (parent(&from), parent(&to));
            let r = if from_dir == to_dir {
                fs.rename(&from, basename(&to))
            } else {
                fs.move_to(&from, &to_dir)
                    .and_then(|()| fs.rename(&join(&to_dir, basename(&from)), basename(&to)))
            };
            done(r)
        }
    }
}
