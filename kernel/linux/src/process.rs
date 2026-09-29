//! Un proceso: su memoria, sus descriptores de archivo y las llamadas al sistema.
//!
//! Una llamada llega como en Linux: el número en `rax` y hasta seis argumentos (`rdi`, `rsi`,
//! `rdx`, `r10`, `r8`, `r9`). Lo que devuelve va en `rax`: un número (≥ 0) o `-errno`. Los
//! argumentos que son punteros apuntan a la memoria del proceso: nunca se leen directo, sino con
//! [`Process::copy_in`] / [`Process::copy_out`], que verifican que la dirección sea del proceso
//! (un programa podría pasar un puntero al kernel para leerlo o pisarlo).

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::abi::{self, at, errno::*, nr, o, prot};
use crate::elf::{self, LoadError, Program};
use crate::mm::{self, Mm, PAGE, STACK_TOP, page_down, page_up};
use crate::sys::{DirEntry, Fault, FileOp, FileReply, Meta, NetOp, NetReply, System};

/// Qué hacer después de una llamada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Volver al programa con este valor en `rax`.
    Return(i64),
    /// El programa terminó con este código (128 + señal si lo mató una señal).
    Exit(i32),
}

const MAX_FDS: usize = 256;
const POLLIN: i16 = 0x1;
const POLLOUT: i16 = 0x4;
const POLLNVAL: i16 = 0x20;
const SIGABRT: i32 = 6;

#[derive(Debug)]
enum Node {
    /// La Terminal: entrada (0) y salida (1, 2).
    Console,
    /// Un archivo del disco, entero en memoria. Se escribe al cerrarlo (o con `fsync`).
    File {
        path: String,
        data: Vec<u8>,
        dirty: bool,
        mtime: u64,
    },
    Dir {
        path: String,
        entries: Vec<DirEntry>,
    },
    Null,
    Zero,
    Random,
    Socket {
        conn: Option<u32>,
        peer: [u8; 16],
    },
}

/// Una "descripción de archivo abierto": la comparten los descriptores duplicados (`dup`).
#[derive(Debug)]
struct Open {
    node: Node,
    pos: u64,
    flags: u64,
}

type Handle = Rc<RefCell<Open>>;

#[derive(Clone, Debug)]
struct Fd {
    open: Handle,
    cloexec: bool,
}

pub struct Process {
    pub pid: u32,
    /// El nombre del programa (para el log y el firewall).
    pub name: String,
    /// La ruta del ejecutable (`/proc/self/exe`).
    pub exe: String,
    pub cwd: String,
    pub mm: Mm,
    fds: Vec<Option<Fd>>,
    fs_base: u64,
}

fn err(e: i64) -> Flow {
    Flow::Return(-e)
}

/// Une `path` al directorio `base` y lo normaliza ("." y ".." incluidos, sin salir de "/").
pub fn resolve(base: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let full_iter = if path.starts_with('/') {
        path.split('/').collect::<Vec<_>>()
    } else {
        base.split('/').chain(path.split('/')).collect()
    };
    for p in full_iter {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    let mut out = String::from("/");
    out.push_str(&parts.join("/"));
    out
}

impl Process {
    /// Carga un programa: sus segmentos, la pila con `argv`/`envp` y el vector auxiliar.
    /// Devuelve el proceso, el punto de entrada y el `rsp` inicial.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        pid: u32,
        file: &[u8],
        exe: &str,
        cwd: &str,
        argv: &[&[u8]],
        envp: &[&[u8]],
        sys: &mut dyn System,
    ) -> Result<(Process, u64, u64), LoadError> {
        let program = elf::parse(file, mm::USER_END)?;
        let name = exe.rsplit('/').next().unwrap_or(exe).to_string();
        let mut p = Process {
            pid,
            name,
            exe: exe.to_string(),
            cwd: cwd.to_string(),
            mm: Mm::new(),
            fds: Vec::new(),
            fs_base: 0,
        };
        p.load_segments(file, &program, sys)?;
        p.mm.setup(program.end);
        let mut random = [0u8; 16];
        sys.random(&mut random);
        let aux = [
            (at::PHDR, program.phdr),
            (at::PHENT, program.phent),
            (at::PHNUM, program.phnum),
            (at::PAGESZ, PAGE),
            (at::BASE, 0),
            (at::FLAGS, 0),
            (at::ENTRY, program.entry),
            (at::UID, abi::UID as u64),
            (at::EUID, abi::UID as u64),
            (at::GID, abi::UID as u64),
            (at::EGID, abi::UID as u64),
            (at::SECURE, 0),
            (at::CLKTCK, 100),
            (at::HWCAP, 0),
        ];
        let (rsp, image) = crate::stack::build(STACK_TOP, argv, envp, &aux, random);
        p.copy_out(rsp, &image, sys)
            .map_err(|_| LoadError::Broken("no hay memoria para la pila"))?;
        // Entrada, salida y errores: la Terminal.
        let console = Rc::new(RefCell::new(Open {
            node: Node::Console,
            pos: 0,
            flags: o::RDWR,
        }));
        for _ in 0..3 {
            p.fds.push(Some(Fd {
                open: console.clone(),
                cloexec: false,
            }));
        }
        Ok((p, program.entry, rsp))
    }

    fn load_segments(
        &mut self,
        file: &[u8],
        program: &Program,
        sys: &mut dyn System,
    ) -> Result<(), LoadError> {
        for s in &program.segments {
            let start = page_down(s.vaddr);
            let end = page_up(s.vaddr + s.memsz).ok_or(LoadError::Broken("dirección inválida"))?;
            // Una página compartida con el segmento anterior ya está puesta.
            let mut page = start;
            while page < end {
                if self.mm.find(page).is_none() && !sys.map_page(page, s.prot | prot::WRITE) {
                    return Err(LoadError::Broken("no hay memoria para el programa"));
                }
                page += PAGE;
            }
            // Sin superponerse con el segmento anterior (la página compartida es de los dos:
            // sus permisos, la unión, se ponen abajo).
            let from = self.mm.vmas().last().map_or(start, |v| v.end.max(start));
            if from < end {
                self.mm.add_program(from, end, s.prot);
            }
            let data = &file[s.offset as usize..(s.offset + s.filesz) as usize];
            sys.write_user(s.vaddr, data, true)
                .map_err(|_| LoadError::Broken("no se pudo copiar el programa"))?;
        }
        // Ahora sí, cada página con sus permisos (se mapearon escribibles para copiar): los de
        // los segmentos que la tocan.
        for s in &program.segments {
            let mut page = page_down(s.vaddr);
            while page < s.vaddr + s.memsz {
                let p = program
                    .segments
                    .iter()
                    .filter(|w| page_down(w.vaddr) <= page && page < w.vaddr + w.memsz)
                    .fold(0, |acc, w| acc | w.prot);
                sys.protect(page, page + PAGE, p);
                page += PAGE;
            }
        }
        Ok(())
    }

    /// Un fallo de página del programa en `addr`. `true` si se resolvió (la página era de una
    /// zona válida y se le puso memoria); `false` es una violación de segmento.
    pub fn fault(&mut self, addr: u64, write: bool, exec: bool, sys: &mut dyn System) -> bool {
        match self.mm.allows(addr, write, exec) {
            Some(p) => sys.map_page(page_down(addr), p),
            None => false,
        }
    }

    /// Lee `len` bytes de la memoria del proceso (asignando las páginas que falten).
    pub fn copy_in(&mut self, addr: u64, len: usize, sys: &mut dyn System) -> Result<Vec<u8>, i64> {
        let mut buf = vec![0u8; len];
        if len == 0 {
            return Ok(buf);
        }
        addr.checked_add(len as u64).ok_or(EFAULT)?;
        loop {
            match sys.read_user(addr, &mut buf) {
                Ok(()) => return Ok(buf),
                Err(Fault::Missing(page)) if self.fault(page, false, false, sys) => {}
                Err(_) => return Err(EFAULT),
            }
        }
    }

    pub fn copy_out(&mut self, addr: u64, data: &[u8], sys: &mut dyn System) -> Result<(), i64> {
        if data.is_empty() {
            return Ok(());
        }
        addr.checked_add(data.len() as u64).ok_or(EFAULT)?;
        loop {
            match sys.write_user(addr, data, false) {
                Ok(()) => return Ok(()),
                Err(Fault::Missing(page)) if self.fault(page, true, false, sys) => {}
                Err(_) => return Err(EFAULT),
            }
        }
    }

    fn read_u64(&mut self, addr: u64, sys: &mut dyn System) -> Result<u64, i64> {
        let b = self.copy_in(addr, 8, sys)?;
        Ok(u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
    }

    /// Un texto terminado en 0 (una ruta), de hasta 4096 bytes.
    fn read_cstr(&mut self, addr: u64, sys: &mut dyn System) -> Result<String, i64> {
        let mut out = Vec::new();
        let mut a = addr;
        loop {
            // De a pedazos que no crucen una página (la siguiente puede no existir).
            let chunk = (PAGE - a % PAGE).min(256) as usize;
            let b = self.copy_in(a, chunk, sys)?;
            if let Some(z) = b.iter().position(|&c| c == 0) {
                out.extend_from_slice(&b[..z]);
                break;
            }
            out.extend_from_slice(&b);
            if out.len() > 4096 {
                return Err(ENAMETOOLONG);
            }
            a += chunk as u64;
        }
        String::from_utf8(out).map_err(|_| EINVAL)
    }

    fn path_arg(&mut self, dirfd: i64, addr: u64, sys: &mut dyn System) -> Result<String, i64> {
        let path = self.read_cstr(addr, sys)?;
        if path.is_empty() {
            return Err(ENOENT);
        }
        let base = if path.starts_with('/') || dirfd == abi::AT_FDCWD {
            self.cwd.clone()
        } else {
            match self.get(dirfd as u64)?.borrow().node {
                Node::Dir { ref path, .. } => path.clone(),
                _ => return Err(ENOTDIR),
            }
        };
        Ok(resolve(&base, &path))
    }

    fn get(&self, fd: u64) -> Result<Handle, i64> {
        self.fds
            .get(fd as usize)
            .and_then(|f| f.as_ref())
            .map(|f| f.open.clone())
            .ok_or(EBADF)
    }

    fn install(&mut self, open: Handle, min: usize, cloexec: bool) -> Result<i64, i64> {
        let fd = (min..MAX_FDS)
            .find(|&i| self.fds.get(i).is_none_or(|f| f.is_none()))
            .ok_or(EMFILE)?;
        if self.fds.len() <= fd {
            self.fds.resize(fd + 1, None);
        }
        self.fds[fd] = Some(Fd { open, cloexec });
        Ok(fd as i64)
    }

    /// Cierra el descriptor; si era el último que usaba el archivo, lo guarda.
    fn close_fd(&mut self, fd: usize, sys: &mut dyn System) -> Result<(), i64> {
        let f = self.fds.get_mut(fd).and_then(Option::take).ok_or(EBADF)?;
        if Rc::strong_count(&f.open) == 1 {
            let r = flush(&f.open, sys);
            if let Node::Socket { conn: Some(id), .. } = f.open.borrow().node {
                sys.net(NetOp::Close { id });
            }
            r?;
        }
        Ok(())
    }

    /// Al terminar: cierra todo (los archivos modificados se guardan).
    pub fn close_all(&mut self, sys: &mut dyn System) {
        for fd in 0..self.fds.len() {
            let _ = self.close_fd(fd, sys);
        }
    }

    pub fn fs_base(&self) -> u64 {
        self.fs_base
    }

    /// Atiende una llamada al sistema.
    pub fn syscall(&mut self, n: u64, a: [u64; 6], sys: &mut dyn System) -> Flow {
        match self.dispatch(n, a, sys) {
            Ok(f) => f,
            Err(e) => err(e),
        }
    }

    fn dispatch(&mut self, n: u64, a: [u64; 6], sys: &mut dyn System) -> Result<Flow, i64> {
        let ok = |v: i64| Ok(Flow::Return(v));
        match n {
            nr::READ => {
                let h = self.get(a[0])?;
                let data = read_from(&h, a[2] as usize, None, sys)?;
                self.copy_out(a[1], &data, sys)?;
                ok(data.len() as i64)
            }
            nr::PREAD64 => {
                let h = self.get(a[0])?;
                let data = read_from(&h, a[2] as usize, Some(a[3]), sys)?;
                self.copy_out(a[1], &data, sys)?;
                ok(data.len() as i64)
            }
            nr::WRITE => {
                let h = self.get(a[0])?;
                let data = self.copy_in(a[1], (a[2] as usize).min(1 << 24), sys)?;
                ok(write_to(&h, &data, None, sys)? as i64)
            }
            nr::PWRITE64 => {
                let h = self.get(a[0])?;
                let data = self.copy_in(a[1], (a[2] as usize).min(1 << 24), sys)?;
                ok(write_to(&h, &data, Some(a[3]), sys)? as i64)
            }
            nr::READV | nr::WRITEV => {
                let h = self.get(a[0])?;
                let count = (a[2] as usize).min(1024);
                let mut total = 0i64;
                for i in 0..count {
                    let base = self.read_u64(a[1] + i as u64 * 16, sys)?;
                    let len = self.read_u64(a[1] + i as u64 * 16 + 8, sys)? as usize;
                    if len == 0 {
                        continue;
                    }
                    if n == nr::WRITEV {
                        let data = self.copy_in(base, len.min(1 << 24), sys)?;
                        total += write_to(&h, &data, None, sys)? as i64;
                    } else {
                        let data = read_from(&h, len, None, sys)?;
                        self.copy_out(base, &data, sys)?;
                        total += data.len() as i64;
                        if data.len() < len {
                            break;
                        }
                    }
                }
                ok(total)
            }
            nr::OPEN => self.open(abi::AT_FDCWD, a[0], a[1], sys),
            nr::CREAT => self.open(abi::AT_FDCWD, a[0], o::CREAT | o::WRONLY | o::TRUNC, sys),
            nr::OPENAT => self.open(a[0] as i64, a[1], a[2], sys),
            nr::CLOSE => {
                self.close_fd(a[0] as usize, sys)?;
                ok(0)
            }
            nr::STAT | nr::LSTAT => {
                let path = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                let st = self.stat_path(&path, sys)?;
                self.copy_out(a[1], &st, sys)?;
                ok(0)
            }
            nr::FSTAT => {
                let h = self.get(a[0])?;
                let st = stat_open(&h.borrow());
                self.copy_out(a[1], &st, sys)?;
                ok(0)
            }
            nr::NEWFSTATAT => {
                let st = if a[3] & abi::AT_EMPTY_PATH != 0 && self.copy_in(a[1], 1, sys)? == [0] {
                    stat_open(&self.get(a[0])?.borrow())
                } else {
                    let path = self.path_arg(a[0] as i64, a[1], sys)?;
                    self.stat_path(&path, sys)?
                };
                self.copy_out(a[2], &st, sys)?;
                ok(0)
            }
            nr::ACCESS => {
                let path = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                self.stat_path(&path, sys)?;
                ok(0)
            }
            nr::FACCESSAT | nr::FACCESSAT2 => {
                let path = self.path_arg(a[0] as i64, a[1], sys)?;
                self.stat_path(&path, sys)?;
                ok(0)
            }
            nr::LSEEK => {
                let h = self.get(a[0])?;
                let mut o = h.borrow_mut();
                let size = match &o.node {
                    Node::File { data, .. } => data.len() as i64,
                    Node::Dir { entries, .. } => entries.len() as i64,
                    Node::Console | Node::Socket { .. } => return Err(ESPIPE),
                    _ => 0,
                };
                let base = match a[2] {
                    0 => 0,
                    1 => o.pos as i64,
                    2 => size,
                    _ => return Err(EINVAL),
                };
                let new = base
                    .checked_add(a[1] as i64)
                    .filter(|&p| p >= 0)
                    .ok_or(EINVAL)?;
                o.pos = new as u64;
                ok(new)
            }
            nr::GETDENTS64 => {
                let h = self.get(a[0])?;
                let out = {
                    let mut o = h.borrow_mut();
                    let pos = o.pos as usize;
                    let Node::Dir { entries, .. } = &o.node else {
                        return Err(ENOTDIR);
                    };
                    let (bytes, used) = dirents(entries, pos, a[2] as usize);
                    if used == 0 && pos < entries.len() + 2 {
                        return Err(EINVAL); // el buffer no alcanza ni para una
                    }
                    o.pos += used as u64;
                    bytes
                };
                self.copy_out(a[1], &out, sys)?;
                ok(out.len() as i64)
            }
            nr::DUP => {
                let h = self.get(a[0])?;
                ok(self.install(h, 0, false)?)
            }
            nr::DUP2 | nr::DUP3 => {
                let h = self.get(a[0])?;
                let new = a[1] as usize;
                if new >= MAX_FDS {
                    return Err(EBADF);
                }
                if a[0] == a[1] {
                    return if n == nr::DUP3 {
                        Err(EINVAL)
                    } else {
                        ok(new as i64)
                    };
                }
                let _ = self.close_fd(new, sys);
                if self.fds.len() <= new {
                    self.fds.resize(new + 1, None);
                }
                self.fds[new] = Some(Fd {
                    open: h,
                    cloexec: a[2] & o::CLOEXEC != 0,
                });
                ok(new as i64)
            }
            nr::FCNTL => {
                let h = self.get(a[0])?;
                match a[1] {
                    0 | 1030 => ok(self.install(h, a[2] as usize, a[1] == 1030)?), // F_DUPFD(_CLOEXEC)
                    1 => ok(self.fds[a[0] as usize]
                        .as_ref()
                        .map_or(0, |f| f.cloexec as i64)),
                    2 => {
                        if let Some(Some(f)) = self.fds.get_mut(a[0] as usize) {
                            f.cloexec = a[2] & 1 != 0;
                        }
                        ok(0)
                    }
                    3 => ok(h.borrow().flags as i64), // F_GETFL
                    4 => {
                        let mut o = h.borrow_mut();
                        o.flags = (o.flags & o::ACCMODE) | (a[2] & (o::APPEND | o::NONBLOCK));
                        ok(0)
                    }
                    _ => ok(0), // locks y demás: no hay otro proceso con quien pelear
                }
            }
            nr::IOCTL => {
                let h = self.get(a[0])?;
                let console = matches!(h.borrow().node, Node::Console);
                match a[1] {
                    0x5401 if console => {
                        // TCGETS: una termios en modo "cocinado" (ICANON | ECHO).
                        let mut t = [0u8; 60];
                        t[12..16].copy_from_slice(&0o000013u32.to_le_bytes());
                        self.copy_out(a[2], &t, sys)?;
                        ok(0)
                    }
                    0x5402..=0x5404 if console => ok(0), // TCSETS*: se acepta y se ignora
                    0x5413 if console => {
                        // TIOCGWINSZ
                        let (rows, cols) = sys.console_size();
                        let mut w = [0u8; 8];
                        w[0..2].copy_from_slice(&rows.to_le_bytes());
                        w[2..4].copy_from_slice(&cols.to_le_bytes());
                        self.copy_out(a[2], &w, sys)?;
                        ok(0)
                    }
                    0x540F if console => {
                        self.copy_out(a[2], &self.pid.to_le_bytes(), sys)?;
                        ok(0)
                    }
                    0x5421 => ok(0), // FIONBIO
                    _ => Err(ENOTTY),
                }
            }
            nr::POLL | nr::PPOLL => {
                let count = (a[1] as usize).min(1024);
                let mut ready = 0;
                for i in 0..count {
                    let at = a[0] + i as u64 * 8;
                    let e = self.copy_in(at, 8, sys)?;
                    let fd = i32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                    let events = i16::from_le_bytes([e[4], e[5]]);
                    let revents = if fd < 0 {
                        0
                    } else if self.get(fd as u64).is_err() {
                        POLLNVAL
                    } else {
                        events & (POLLIN | POLLOUT)
                    };
                    if revents != 0 {
                        ready += 1;
                    }
                    self.copy_out(at + 6, &revents.to_le_bytes(), sys)?;
                }
                ok(ready)
            }
            nr::FSYNC | nr::FDATASYNC => {
                let h = self.get(a[0])?;
                flush(&h, sys)?;
                ok(0)
            }
            nr::FTRUNCATE => {
                let h = self.get(a[0])?;
                let mut o = h.borrow_mut();
                match &mut o.node {
                    Node::File { data, dirty, .. } => {
                        data.resize(a[1] as usize, 0);
                        *dirty = true;
                        ok(0)
                    }
                    _ => Err(EINVAL),
                }
            }
            nr::GETCWD => {
                let mut b = self.cwd.as_bytes().to_vec();
                b.push(0);
                if b.len() > a[1] as usize {
                    return Err(ERANGE);
                }
                self.copy_out(a[0], &b, sys)?;
                ok(b.len() as i64)
            }
            nr::CHDIR => {
                let path = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                self.chdir(path, sys)
            }
            nr::FCHDIR => {
                let h = self.get(a[0])?;
                let path = match &h.borrow().node {
                    Node::Dir { path, .. } => path.clone(),
                    _ => return Err(ENOTDIR),
                };
                self.chdir(path, sys)
            }
            nr::MKDIR => {
                let path = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                file_done(sys.file(FileOp::Mkdir(path)))
            }
            nr::MKDIRAT => {
                let path = self.path_arg(a[0] as i64, a[1], sys)?;
                file_done(sys.file(FileOp::Mkdir(path)))
            }
            nr::UNLINK => {
                let path = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                file_done(sys.file(FileOp::Remove(path)))
            }
            nr::RMDIR => {
                let path = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                file_done(sys.file(FileOp::Rmdir(path)))
            }
            nr::UNLINKAT => {
                let path = self.path_arg(a[0] as i64, a[1], sys)?;
                let op = if a[2] & abi::AT_REMOVEDIR != 0 {
                    FileOp::Rmdir(path)
                } else {
                    FileOp::Remove(path)
                };
                file_done(sys.file(op))
            }
            nr::RENAME => {
                let from = self.path_arg(abi::AT_FDCWD, a[0], sys)?;
                let to = self.path_arg(abi::AT_FDCWD, a[1], sys)?;
                file_done(sys.file(FileOp::Rename(from, to)))
            }
            nr::RENAMEAT | nr::RENAMEAT2 => {
                let from = self.path_arg(a[0] as i64, a[1], sys)?;
                let to = self.path_arg(a[2] as i64, a[3], sys)?;
                file_done(sys.file(FileOp::Rename(from, to)))
            }
            nr::READLINK | nr::READLINKAT => {
                let (p, buf, len) = if n == nr::READLINK {
                    (a[0], a[1], a[2])
                } else {
                    (a[1], a[2], a[3])
                };
                let path = self.read_cstr(p, sys)?;
                if path != "/proc/self/exe" {
                    return Err(EINVAL); // no hay enlaces simbólicos en FAT32
                }
                let exe = self.exe.clone().into_bytes();
                let n = exe.len().min(len as usize);
                self.copy_out(buf, &exe[..n], sys)?;
                ok(n as i64)
            }
            nr::CHMOD | nr::UMASK => ok(0o022),
            // Memoria.
            nr::BRK => {
                let mut pages = PagesOf(sys);
                ok(self.mm.brk(a[0], &mut pages) as i64)
            }
            nr::MMAP => self.mmap(a, sys),
            nr::MUNMAP => {
                if !a[0].is_multiple_of(PAGE) || a[1] == 0 {
                    return Err(EINVAL);
                }
                let end = page_up(a[0].saturating_add(a[1])).ok_or(EINVAL)?;
                self.mm
                    .unmap(a[0], end.min(mm::USER_END), &mut PagesOf(sys));
                ok(0)
            }
            nr::MPROTECT => {
                if !a[0].is_multiple_of(PAGE) {
                    return Err(EINVAL);
                }
                let end = page_up(a[0].saturating_add(a[1])).ok_or(EINVAL)?;
                let p = (a[2] & 7) as u32;
                self.mm
                    .protect(a[0], end, p, &mut PagesOf(sys))
                    .ok_or(ENOMEM)?;
                ok(0)
            }
            nr::MADVISE => ok(0),
            // mremap: musl lo usa para medir la pila (espera ENOMEM mientras siga creciendo);
            // sin esta llamada deja de medir enseguida.
            nr::MREMAP => Err(ENOSYS),
            nr::ARCH_PRCTL => match a[0] {
                0x1002 => {
                    // ARCH_SET_FS: el puntero al bloque de TLS del hilo.
                    self.fs_base = a[1];
                    sys.set_fs(a[1]);
                    ok(0)
                }
                0x1003 => {
                    self.copy_out(a[1], &self.fs_base.to_le_bytes(), sys)?;
                    ok(0)
                }
                _ => Err(EINVAL),
            },
            // Identidad.
            nr::GETPID | nr::GETTID | nr::SET_TID_ADDRESS | nr::GETPGRP => ok(self.pid as i64),
            nr::GETPPID => ok(1),
            nr::GETUID | nr::GETEUID | nr::GETGID | nr::GETEGID => ok(abi::UID as i64),
            nr::SETPGID | nr::SETSID => ok(0),
            nr::UNAME => {
                let mut u = vec![0u8; 65 * 6];
                for (i, s) in [
                    "Linux",
                    "jarvis",
                    "6.1.0-jarvis",
                    "#1 JARVIS-OS K11",
                    "x86_64",
                    "",
                ]
                .iter()
                .enumerate()
                {
                    u[i * 65..i * 65 + s.len()].copy_from_slice(s.as_bytes());
                }
                self.copy_out(a[0], &u, sys)?;
                ok(0)
            }
            nr::SYSINFO => {
                let mut s = [0u8; 112];
                let up = sys.monotonic_ns() / 1_000_000_000;
                s[0..8].copy_from_slice(&up.to_le_bytes());
                s[104..108].copy_from_slice(&1u32.to_le_bytes());
                self.copy_out(a[0], &s, sys)?;
                ok(0)
            }
            nr::GETRLIMIT => {
                let r = rlimit(a[0]);
                self.copy_out(a[1], &r, sys)?;
                ok(0)
            }
            nr::PRLIMIT64 => {
                if a[3] != 0 {
                    let r = rlimit(a[1]);
                    self.copy_out(a[3], &r, sys)?;
                }
                ok(0)
            }
            nr::SCHED_GETAFFINITY => {
                // Un solo procesador.
                let n = (a[1] as usize).min(8);
                let mut m = [0u8; 8];
                m[0] = 1;
                self.copy_out(a[2], &m[..n], sys)?;
                ok(n as i64)
            }
            nr::SCHED_YIELD => {
                sys.sleep_ns(0);
                ok(0)
            }
            // Señales: se aceptan los manejadores pero nunca llegan (salvo las que terminan).
            nr::RT_SIGACTION => {
                if a[2] != 0 {
                    self.copy_out(a[2], &[0u8; 32], sys)?;
                }
                ok(0)
            }
            nr::RT_SIGPROCMASK => {
                if a[2] != 0 {
                    self.copy_out(a[2], &[0u8; 8], sys)?;
                }
                ok(0)
            }
            nr::SIGALTSTACK => {
                if a[1] != 0 {
                    // SS_DISABLE: no había una.
                    let mut s = [0u8; 24];
                    s[8..12].copy_from_slice(&2u32.to_le_bytes());
                    self.copy_out(a[1], &s, sys)?;
                }
                ok(0)
            }
            nr::KILL | nr::TGKILL => {
                let (pid, sig) = if n == nr::KILL {
                    (a[0], a[1])
                } else {
                    (a[1], a[2])
                };
                if pid as u32 != self.pid && pid != 0 {
                    return Err(ESRCH);
                }
                if sig == 0 {
                    return ok(0);
                }
                if sig as i32 == SIGABRT {
                    sys.console_write(b"Abortado\n");
                }
                Ok(Flow::Exit(128 + sig as i32))
            }
            nr::PRCTL | nr::SET_ROBUST_LIST => ok(0),
            nr::FUTEX => match a[1] & 0x7F {
                // Un solo hilo: nadie más puede cambiar el valor ni despertar a nadie.
                0 | 9 => Err(EAGAIN),
                _ => ok(0),
            },
            // Tiempo y azar.
            nr::CLOCK_GETTIME => {
                let ns = if a[0] == 0 || a[0] == 8 {
                    sys.realtime_ns()
                } else {
                    sys.monotonic_ns()
                };
                self.copy_out(a[1], &timespec(ns), sys)?;
                ok(0)
            }
            nr::CLOCK_GETRES => {
                if a[1] != 0 {
                    self.copy_out(a[1], &timespec(1_000_000), sys)?;
                }
                ok(0)
            }
            nr::GETTIMEOFDAY => {
                if a[0] != 0 {
                    let ns = sys.realtime_ns();
                    let mut tv = [0u8; 16];
                    tv[0..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
                    tv[8..16].copy_from_slice(&(ns % 1_000_000_000 / 1000).to_le_bytes());
                    self.copy_out(a[0], &tv, sys)?;
                }
                ok(0)
            }
            nr::TIME => {
                let s = sys.realtime_ns() / 1_000_000_000;
                if a[0] != 0 {
                    self.copy_out(a[0], &s.to_le_bytes(), sys)?;
                }
                ok(s as i64)
            }
            nr::NANOSLEEP | nr::CLOCK_NANOSLEEP => {
                let ts = if n == nr::NANOSLEEP { a[0] } else { a[2] };
                let secs = self.read_u64(ts, sys)?;
                let nanos = self.read_u64(ts + 8, sys)?;
                let mut ns = secs.saturating_mul(1_000_000_000).saturating_add(nanos);
                // TIMER_ABSTIME: hasta un momento (del reloj monótono o real).
                if n == nr::CLOCK_NANOSLEEP && a[1] & 1 != 0 {
                    let now = if a[0] == 0 {
                        sys.realtime_ns()
                    } else {
                        sys.monotonic_ns()
                    };
                    ns = ns.saturating_sub(now);
                }
                sys.sleep_ns(ns);
                ok(0)
            }
            nr::GETRANDOM => {
                let len = (a[1] as usize).min(1 << 20);
                let mut b = vec![0u8; len];
                sys.random(&mut b);
                self.copy_out(a[0], &b, sys)?;
                ok(len as i64)
            }
            // Red.
            nr::SOCKET => {
                if a[0] != 2 {
                    return Err(EAFNOSUPPORT); // solo IPv4
                }
                if a[1] & 0xF != 1 {
                    return Err(EPROTONOSUPPORT); // solo TCP
                }
                let open = Rc::new(RefCell::new(Open {
                    node: Node::Socket {
                        conn: None,
                        peer: [0; 16],
                    },
                    pos: 0,
                    flags: o::RDWR,
                }));
                ok(self.install(open, 0, a[1] & o::CLOEXEC != 0)?)
            }
            nr::CONNECT => {
                let h = self.get(a[0])?;
                let addr = self.copy_in(a[1], (a[2] as usize).min(16), sys)?;
                if addr.len() < 8 || u16::from_le_bytes([addr[0], addr[1]]) != 2 {
                    return Err(EAFNOSUPPORT);
                }
                let port = u16::from_be_bytes([addr[2], addr[3]]);
                let ip = [addr[4], addr[5], addr[6], addr[7]];
                let mut o = h.borrow_mut();
                let Node::Socket { conn, peer } = &mut o.node else {
                    return Err(ENOTSOCK);
                };
                if conn.is_some() {
                    return Err(EISCONN);
                }
                match sys.net(NetOp::Connect { ip, port }) {
                    NetReply::Connected(id) => {
                        *conn = Some(id);
                        peer[..addr.len()].copy_from_slice(&addr);
                        ok(0)
                    }
                    NetReply::Error(e) => Err(e),
                    _ => Err(ECONNREFUSED),
                }
            }
            nr::SENDTO => {
                let h = self.get(a[0])?;
                let data = self.copy_in(a[1], (a[2] as usize).min(1 << 24), sys)?;
                ok(write_to(&h, &data, None, sys)? as i64)
            }
            nr::RECVFROM => {
                let h = self.get(a[0])?;
                let data = read_from(&h, a[2] as usize, None, sys)?;
                self.copy_out(a[1], &data, sys)?;
                ok(data.len() as i64)
            }
            nr::SHUTDOWN | nr::SETSOCKOPT => ok(0),
            nr::GETSOCKOPT => {
                // SO_ERROR y demás: 0.
                if a[3] != 0 {
                    self.copy_out(a[3], &0u32.to_le_bytes(), sys)?;
                }
                ok(0)
            }
            nr::GETPEERNAME | nr::GETSOCKNAME => {
                let h = self.get(a[0])?;
                let peer = match h.borrow().node {
                    Node::Socket { peer, .. } => peer,
                    _ => return Err(ENOTSOCK),
                };
                let addr = if n == nr::GETPEERNAME {
                    peer
                } else {
                    let mut me = [0u8; 16];
                    me[0] = 2;
                    me
                };
                self.copy_out(a[1], &addr, sys)?;
                self.copy_out(a[2], &16u32.to_le_bytes(), sys)?;
                ok(0)
            }
            nr::EXIT | nr::EXIT_GROUP => Ok(Flow::Exit(a[0] as i32 & 0xFF)),
            nr::WAIT4 => Err(ECHILD),
            _ => {
                sys.log(&format!(
                    "{}: llamada al sistema {} no implementada",
                    self.name, n
                ));
                Err(ENOSYS)
            }
        }
    }

    fn chdir(&mut self, path: String, sys: &mut dyn System) -> Result<Flow, i64> {
        match sys.file(FileOp::Stat(path.clone())) {
            FileReply::Meta(m) if m.is_dir => {
                self.cwd = path;
                Ok(Flow::Return(0))
            }
            FileReply::Meta(_) => Err(ENOTDIR),
            FileReply::Error(e) => Err(e),
            _ => Err(EIO),
        }
    }

    fn stat_path(&mut self, path: &str, sys: &mut dyn System) -> Result<[u8; 144], i64> {
        if let Some(node) = device(path) {
            return Ok(stat_node(&node, 0));
        }
        match sys.file(FileOp::Stat(path.to_string())) {
            FileReply::Meta(m) => Ok(stat_meta(m)),
            FileReply::Error(e) => Err(e),
            _ => Err(EIO),
        }
    }

    fn open(
        &mut self,
        dirfd: i64,
        path: u64,
        flags: u64,
        sys: &mut dyn System,
    ) -> Result<Flow, i64> {
        let path = self.path_arg(dirfd, path, sys)?;
        let cloexec = flags & o::CLOEXEC != 0;
        let node = match device(&path) {
            Some(n) => n,
            None => self.open_disk(&path, flags, sys)?,
        };
        let open = Rc::new(RefCell::new(Open {
            node,
            pos: 0,
            flags: flags & !o::CLOEXEC,
        }));
        Ok(Flow::Return(self.install(open, 0, cloexec)?))
    }

    fn open_disk(&mut self, path: &str, flags: u64, sys: &mut dyn System) -> Result<Node, i64> {
        let meta = match sys.file(FileOp::Stat(path.to_string())) {
            FileReply::Meta(m) => Some(m),
            FileReply::Error(ENOENT) => None,
            FileReply::Error(e) => return Err(e),
            _ => return Err(EIO),
        };
        let write = flags & o::ACCMODE != o::RDONLY;
        match meta {
            Some(m) if m.is_dir => {
                if write {
                    return Err(EISDIR);
                }
                let entries = match sys.file(FileOp::List(path.to_string())) {
                    FileReply::List(l) => l,
                    FileReply::Error(e) => return Err(e),
                    _ => return Err(EIO),
                };
                Ok(Node::Dir {
                    path: path.to_string(),
                    entries,
                })
            }
            Some(_) if flags & o::DIRECTORY != 0 => Err(ENOTDIR),
            Some(_) if flags & o::CREAT != 0 && flags & o::EXCL != 0 => Err(EEXIST),
            Some(m) => {
                let data = if flags & o::TRUNC != 0 && write {
                    Vec::new()
                } else {
                    match sys.file(FileOp::Read(path.to_string())) {
                        FileReply::Data(d) => d,
                        FileReply::Error(e) => return Err(e),
                        _ => return Err(EIO),
                    }
                };
                Ok(Node::File {
                    path: path.to_string(),
                    data,
                    dirty: flags & o::TRUNC != 0 && write,
                    mtime: m.mtime,
                })
            }
            None if flags & o::CREAT != 0 => {
                // Se crea ya (vacío), como en Linux: otro `open` lo tiene que ver.
                file_done(sys.file(FileOp::Write(path.to_string(), Vec::new())))?;
                Ok(Node::File {
                    path: path.to_string(),
                    data: Vec::new(),
                    dirty: false,
                    mtime: sys.realtime_ns() / 1_000_000_000,
                })
            }
            None => Err(ENOENT),
        }
    }

    fn mmap(&mut self, a: [u64; 6], sys: &mut dyn System) -> Result<Flow, i64> {
        let (addr, len, p, flags, fd, off) = (a[0], a[1], (a[2] & 7) as u32, a[3], a[4], a[5]);
        if len == 0 || off % PAGE != 0 {
            return Err(EINVAL);
        }
        let fixed = flags & (abi::map::FIXED | abi::map::FIXED_NOREPLACE) != 0;
        // Un archivo mapeado en privado: su contenido se copia (no hay memoria compartida).
        let content = if flags & abi::map::ANONYMOUS == 0 {
            let h = self.get(fd)?;
            let o = h.borrow();
            match &o.node {
                Node::File { data, .. } => {
                    let from = (off as usize).min(data.len());
                    let to = (from + len as usize).min(data.len());
                    Some(data[from..to].to_vec())
                }
                Node::Zero | Node::Null => None,
                _ => return Err(EACCES),
            }
        } else {
            None
        };
        if flags & abi::map::SHARED != 0 && content.is_some() && p & prot::WRITE != 0 {
            // Escribir en un archivo mapeado compartido no se guardaría: mejor decirlo.
            return Err(EOPNOTSUPP);
        }
        let start = self
            .mm
            .map(addr, len, p, fixed, &mut PagesOf(sys))
            .ok_or(ENOMEM)?;
        if let Some(data) = content {
            // Se escribe aunque la zona sea de solo lectura (como lo haría el kernel).
            let mut at = 0;
            while at < data.len() {
                let page = page_down(start + at as u64);
                if !sys.map_page(page, p | prot::READ) {
                    return Err(ENOMEM);
                }
                let n =
                    (PAGE as usize - (start as usize + at) % PAGE as usize).min(data.len() - at);
                sys.write_user(start + at as u64, &data[at..at + n], true)
                    .map_err(|_| EFAULT)?;
                at += n;
            }
        }
        Ok(Flow::Return(start as i64))
    }
}

/// El `Pages` del mapa de memoria, sobre el sistema.
struct PagesOf<'a>(&'a mut dyn System);

impl mm::Pages for PagesOf<'_> {
    fn unmap(&mut self, start: u64, end: u64) {
        self.0.unmap(start, end);
    }
    fn protect(&mut self, start: u64, end: u64, p: u32) {
        self.0.protect(start, end, p);
    }
}

fn file_done(r: FileReply) -> Result<Flow, i64> {
    match r {
        FileReply::Error(e) => Err(e),
        _ => Ok(Flow::Return(0)),
    }
}

/// Los archivos especiales de /dev.
fn device(path: &str) -> Option<Node> {
    Some(match path {
        "/dev/null" => Node::Null,
        "/dev/zero" => Node::Zero,
        "/dev/random" | "/dev/urandom" => Node::Random,
        "/dev/tty" | "/dev/stdin" | "/dev/stdout" | "/dev/stderr" => Node::Console,
        _ => return None,
    })
}

/// Guarda el archivo si cambió.
fn flush(h: &Handle, sys: &mut dyn System) -> Result<(), i64> {
    let mut o = h.borrow_mut();
    if let Node::File {
        path, data, dirty, ..
    } = &mut o.node
        && *dirty
    {
        *dirty = false;
        if let FileReply::Error(e) = sys.file(FileOp::Write(path.clone(), data.clone())) {
            return Err(e);
        }
    }
    Ok(())
}

fn read_from(
    h: &Handle,
    max: usize,
    at: Option<u64>,
    sys: &mut dyn System,
) -> Result<Vec<u8>, i64> {
    let max = max.min(1 << 24);
    let mut o = h.borrow_mut();
    if o.flags & o::ACCMODE == o::WRONLY {
        return Err(EBADF);
    }
    // La consola y los sockets esperan: sin tener tomado el archivo.
    match o.node {
        Node::Console => {
            drop(o);
            return Ok(sys.console_read(max));
        }
        Node::Socket { conn, .. } => {
            let id = conn.ok_or(ENOTCONN)?;
            drop(o);
            return match sys.net(NetOp::Recv { id, max }) {
                NetReply::Data(d) => Ok(d),
                NetReply::Error(e) => Err(e),
                _ => Ok(Vec::new()),
            };
        }
        _ => {}
    }
    let pos = at.unwrap_or(o.pos) as usize;
    let out = match &o.node {
        Node::File { data, .. } => data
            .get(pos..)
            .map_or(&[][..], |d| &d[..d.len().min(max)])
            .to_vec(),
        Node::Dir { .. } => return Err(EISDIR),
        Node::Zero => vec![0; max],
        Node::Random => {
            let mut b = vec![0; max.min(1 << 16)];
            sys.random(&mut b);
            b
        }
        _ => Vec::new(),
    };
    if at.is_none() {
        o.pos += out.len() as u64;
    }
    Ok(out)
}

fn write_to(h: &Handle, data: &[u8], at: Option<u64>, sys: &mut dyn System) -> Result<usize, i64> {
    let mut o = h.borrow_mut();
    match o.node {
        Node::Console => {
            drop(o);
            sys.console_write(data);
            return Ok(data.len());
        }
        Node::Socket { conn, .. } => {
            let id = conn.ok_or(ENOTCONN)?;
            drop(o);
            return match sys.net(NetOp::Send {
                id,
                data: data.to_vec(),
            }) {
                NetReply::Sent(n) => Ok(n),
                NetReply::Error(e) => Err(e),
                _ => Err(EPIPE),
            };
        }
        Node::Dir { .. } => return Err(EISDIR),
        Node::Null | Node::Zero | Node::Random => return Ok(data.len()),
        Node::File { .. } => {}
    }
    if o.flags & o::ACCMODE == o::RDONLY {
        return Err(EBADF);
    }
    let append = o.flags & o::APPEND != 0;
    let pos = o.pos;
    let end = {
        let Node::File {
            data: content,
            dirty,
            ..
        } = &mut o.node
        else {
            return Err(EBADF);
        };
        let start = if append {
            content.len()
        } else {
            at.unwrap_or(pos) as usize
        };
        let end = start + data.len();
        if content.len() < end {
            content.resize(end, 0);
        }
        content[start..end].copy_from_slice(data);
        *dirty = true;
        end
    };
    if at.is_none() {
        o.pos = end as u64;
    }
    Ok(data.len())
}

/// Registros `linux_dirent64` a partir de la entrada `from` ("." y ".." son la 0 y la 1), hasta
/// llenar `room` bytes. Devuelve los bytes y cuántas entradas entraron.
fn dirents(entries: &[DirEntry], from: usize, room: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut used = 0;
    let total = entries.len() + 2;
    for i in from..total {
        let (name, dir) = match i {
            0 => (".", true),
            1 => ("..", true),
            _ => (entries[i - 2].name.as_str(), entries[i - 2].is_dir),
        };
        let reclen = (19 + name.len() + 1).div_ceil(8) * 8;
        if out.len() + reclen > room {
            break;
        }
        let start = out.len();
        out.resize(start + reclen, 0);
        let r = &mut out[start..];
        r[0..8].copy_from_slice(&(i as u64 + 1).to_le_bytes()); // d_ino
        r[8..16].copy_from_slice(&(i as u64 + 1).to_le_bytes()); // d_off: la siguiente
        r[16..18].copy_from_slice(&(reclen as u16).to_le_bytes());
        r[18] = if dir { 4 } else { 8 }; // DT_DIR / DT_REG
        r[19..19 + name.len()].copy_from_slice(name.as_bytes());
        used += 1;
    }
    (out, used)
}

fn timespec(ns: u64) -> [u8; 16] {
    let mut t = [0u8; 16];
    t[0..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
    t[8..16].copy_from_slice(&(ns % 1_000_000_000).to_le_bytes());
    t
}

fn rlimit(resource: u64) -> [u8; 16] {
    let v: u64 = match resource {
        3 => mm::STACK_SIZE, // RLIMIT_STACK
        7 => MAX_FDS as u64, // RLIMIT_NOFILE
        _ => u64::MAX,       // RLIM_INFINITY
    };
    let mut r = [0u8; 16];
    r[0..8].copy_from_slice(&v.to_le_bytes());
    r[8..16].copy_from_slice(&v.to_le_bytes());
    r
}

/// Un `struct stat` de x86_64 (144 bytes).
fn stat_raw(mode: u32, size: u64, mtime: u64, rdev: u64) -> [u8; 144] {
    let mut s = [0u8; 144];
    s[0..8].copy_from_slice(&1u64.to_le_bytes()); // st_dev
    s[8..16].copy_from_slice(&1u64.to_le_bytes()); // st_ino
    s[16..24].copy_from_slice(&1u64.to_le_bytes()); // st_nlink
    s[24..28].copy_from_slice(&mode.to_le_bytes());
    s[28..32].copy_from_slice(&abi::UID.to_le_bytes());
    s[32..36].copy_from_slice(&abi::UID.to_le_bytes());
    s[40..48].copy_from_slice(&rdev.to_le_bytes());
    s[48..56].copy_from_slice(&size.to_le_bytes());
    s[56..64].copy_from_slice(&4096u64.to_le_bytes()); // st_blksize
    s[64..72].copy_from_slice(&size.div_ceil(512).to_le_bytes()); // st_blocks
    for t in [72, 88, 104] {
        s[t..t + 8].copy_from_slice(&mtime.to_le_bytes());
    }
    s
}

fn stat_meta(m: Meta) -> [u8; 144] {
    if m.is_dir {
        stat_raw(abi::S_IFDIR | 0o755, 4096, m.mtime, 0)
    } else {
        stat_raw(abi::S_IFREG | 0o644, m.size, m.mtime, 0)
    }
}

fn stat_node(n: &Node, pos_mtime: u64) -> [u8; 144] {
    match n {
        Node::Console => stat_raw(abi::S_IFCHR | 0o620, 0, pos_mtime, 0x8800),
        Node::Null | Node::Zero | Node::Random => stat_raw(abi::S_IFCHR | 0o666, 0, 0, 0x103),
        Node::File { data, mtime, .. } => {
            stat_raw(abi::S_IFREG | 0o644, data.len() as u64, *mtime, 0)
        }
        Node::Dir { .. } => stat_raw(abi::S_IFDIR | 0o755, 4096, pos_mtime, 0),
        Node::Socket { .. } => stat_raw(abi::S_IFSOCK | 0o777, 0, 0, 0),
    }
}

fn stat_open(o: &Open) -> [u8; 144] {
    stat_node(&o.node, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas() {
        assert_eq!(resolve("/Documentos", "notas.txt"), "/Documentos/notas.txt");
        assert_eq!(resolve("/Documentos", "../LEAME.txt"), "/LEAME.txt");
        assert_eq!(resolve("/a/b", "/x//y/./z/.."), "/x/y");
        assert_eq!(resolve("/", "../../.."), "/");
        assert_eq!(resolve("/a", "."), "/a");
    }

    #[test]
    fn registros_de_directorio() {
        let e = vec![
            DirEntry {
                name: "notas.txt".into(),
                is_dir: false,
            },
            DirEntry {
                name: "Fotos".into(),
                is_dir: true,
            },
        ];
        let (b, n) = dirents(&e, 0, 4096);
        assert_eq!(n, 4);
        // Cada registro alineado a 8 y con su largo.
        let mut at = 0;
        let mut names = Vec::new();
        while at < b.len() {
            let reclen = u16::from_le_bytes([b[at + 16], b[at + 17]]) as usize;
            assert_eq!(reclen % 8, 0);
            let name = &b[at + 19..at + reclen];
            let end = name.iter().position(|&c| c == 0).unwrap();
            names.push(core::str::from_utf8(&name[..end]).unwrap().to_string());
            at += reclen;
        }
        assert_eq!(names, [".", "..", "notas.txt", "Fotos"]);
        // Con poco lugar entra de a una.
        let (_, n) = dirents(&e, 2, 32);
        assert_eq!(n, 1);
    }
}
