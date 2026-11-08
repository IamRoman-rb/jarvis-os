//! Los programas de Linux vistos desde el escritorio (K11, ADR 0010).
//!
//! Un programa corre en el anillo 3, en su propia tarea del kernel. El disco (FAT32) y la
//! Terminal son de la tarea del escritorio, así que cuando un programa abre un archivo, escribe
//! en la pantalla o lee una línea, el kernel deja un [`ProcEvent`] y espera un [`ProcReply`].
//! Este módulo es ese "servidor":
//!
//! - archivos: cada pedido de `jarvis-linux` ([`FileOp`]) se hace sobre el FAT32; borrar manda a
//!   la Papelera (regla 13). Antes se miran los permisos (ADR 0020): un programa del usuario no
//!   puede escribir en las carpetas del sistema (`EACCES`), uno lanzado con `sudo` (uid 0) sí;
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

use crate::perms::{Access, Perms};
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
    /// Con qué usuario corre: 1000, o 0 (root) si se lanzó con `sudo` (ADR 0020).
    pub uid: u32,
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
    uid: u32,
    /// Para el firewall: `programas:<nombre>` (ADR 0024).
    app: String,
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
    pub fn started(&mut self, pid: u32, uid: u32, path: &str) {
        let name = path.rsplit('/').next().unwrap_or("programa");
        let app = if name.is_empty() {
            String::from(FIREWALL_APP)
        } else {
            format!("{FIREWALL_APP}:{name}")
        };
        self.list.push(Proc {
            pid,
            uid,
            app,
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
        perms: &mut Perms,
        now: jarvis_fs::Timestamp,
    ) {
        let uid = self
            .list
            .iter()
            .find(|p| p.pid == pid)
            .map_or(crate::perms::USER, |p| p.uid);
        let r = match fs {
            Some(fs) => {
                perms.ensure_loaded(fs);
                file_op_as(fs, perms, uid, op, now)
            }
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
                let stream = out.connect_as(&host, port, &p.app);
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
pub(crate) fn unix_seconds(t: jarvis_fs::Timestamp) -> u64 {
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

/// ¿Puede `uid` hacer este pedido? (`Err(errno)` si no.) Crear, borrar o renombrar pide
/// escribir en la carpeta; leer o escribir un archivo, en el archivo.
fn check_op<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    perms: &Perms,
    uid: u32,
    op: &FileOp,
) -> Result<(), i64> {
    use crate::files::parent;
    let is_dir = |fs: &mut FileSystem<D>, p: &str| p == "/" || fs.stat(p).is_ok_and(|s| s.is_dir);
    let need = |fs: &mut FileSystem<D>, p: &str, a: Access| {
        let d = is_dir(fs, p);
        if perms.allowed(uid, p, d, a) {
            Ok(())
        } else {
            Err(errno::EACCES)
        }
    };
    match op {
        FileOp::Stat(_) => Ok(()),
        FileOp::Read(p) | FileOp::List(p) => need(fs, p, Access::Read),
        FileOp::Write(p, _) => {
            if fs.exists(p) {
                need(fs, p, Access::Write)
            } else {
                need(fs, &parent(p), Access::Write)
            }
        }
        FileOp::Mkdir(p) | FileOp::Remove(p) | FileOp::Rmdir(p) => {
            need(fs, &parent(p), Access::Write)
        }
        FileOp::Rename(from, to) => {
            need(fs, &parent(from), Access::Write)?;
            need(fs, &parent(to), Access::Write)
        }
        // chmod: el dueño o root. chown: root; el dueño solo puede cambiar el grupo a uno suyo
        // (como en Linux).
        FileOp::Chmod(p, _) => {
            let d = is_dir(fs, p);
            if uid == crate::perms::ROOT || perms.get(p, d).uid == uid {
                Ok(())
            } else {
                Err(errno::EPERM)
            }
        }
        FileOp::Chown(p, new_uid, new_gid) => {
            let d = is_dir(fs, p);
            let e = perms.get(p, d);
            let same_owner = *new_uid == u32::MAX || *new_uid == e.uid;
            let own_group = *new_gid == u32::MAX || crate::perms::groups(uid).contains(new_gid);
            if uid == crate::perms::ROOT || (e.uid == uid && same_owner && own_group) {
                Ok(())
            } else {
                Err(errno::EPERM)
            }
        }
    }
}

/// Un pedido de archivos de un proceso de `uid`: primero los permisos, después el FAT32, y la
/// tabla de permisos al día (chmod, chown, renombrar, borrar).
pub fn file_op_as<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    perms: &mut Perms,
    uid: u32,
    op: FileOp,
    now: jarvis_fs::Timestamp,
) -> FileReply {
    if let Err(e) = check_op(fs, perms, uid, &op) {
        return FileReply::Error(e);
    }
    match op {
        FileOp::Stat(path) => match file_op(fs, FileOp::Stat(path.clone()), now) {
            FileReply::Meta(mut m) => {
                let e = perms.get(&path, m.is_dir);
                m.mode = u32::from(e.mode);
                m.uid = e.uid;
                m.gid = e.gid;
                FileReply::Meta(m)
            }
            other => other,
        },
        FileOp::Chmod(path, mode) => match fs.stat(&path) {
            Ok(s) => {
                perms.set_mode(&path, s.is_dir, mode as u16);
                saved(fs, perms, now)
            }
            Err(_) => FileReply::Error(errno::ENOENT),
        },
        FileOp::Chown(path, uid, gid) => match fs.stat(&path) {
            Ok(s) => {
                let mut e = perms.get(&path, s.is_dir);
                if uid != u32::MAX {
                    e.uid = uid;
                }
                if gid != u32::MAX {
                    e.gid = gid;
                }
                perms.set(&path, s.is_dir, e);
                saved(fs, perms, now)
            }
            Err(_) => FileReply::Error(errno::ENOENT),
        },
        FileOp::Rename(from, to) => {
            let r = file_op(fs, FileOp::Rename(from.clone(), to.clone()), now);
            if r == FileReply::Done && !perms.is_empty() {
                perms.renamed(&from, &to);
                let _ = saved(fs, perms, now);
            }
            r
        }
        FileOp::Remove(path) => {
            let r = file_op(fs, FileOp::Remove(path.clone()), now);
            if r == FileReply::Done && !perms.is_empty() {
                perms.removed(&path);
                let _ = saved(fs, perms, now);
            }
            r
        }
        other => file_op(fs, other, now),
    }
}

fn saved<D: BlockDevice>(
    fs: &mut FileSystem<D>,
    perms: &Perms,
    now: jarvis_fs::Timestamp,
) -> FileReply {
    match perms.save(fs, now) {
        Ok(()) => FileReply::Done,
        Err(_) => FileReply::Error(errno::EIO),
    }
}

/// Un pedido de archivos sobre el FAT32 (sin mirar permisos). Los errores, como `errno`.
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
            Ok(s) => {
                let e = Perms::default_for(&path, s.is_dir);
                FileReply::Meta(Meta {
                    is_dir: s.is_dir,
                    size: u64::from(s.size),
                    mtime: unix_seconds(s.modified),
                    mode: u32::from(e.mode),
                    uid: e.uid,
                    gid: e.gid,
                })
            }
            // La raíz no tiene entrada propia.
            Err(_) if path == "/" => FileReply::Meta(Meta {
                is_dir: true,
                size: 0,
                mtime: 0,
                mode: 0o755,
                uid: crate::perms::USER,
                gid: crate::perms::USER,
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
        // Sin tabla de permisos (lo hace `file_op_as`): no hay nada que cambiar en el FAT32.
        FileOp::Chmod(..) | FileOp::Chown(..) => FileReply::Done,
    }
}
