//! Un proceso "de mentira" de punta a punta: el ELF mínimo de `elf::tests` cargado sobre un
//! sistema en memoria (páginas en un mapa, un disco de juguete, la consola y la red en vectores),
//! y las llamadas al sistema hechas como las haría el programa.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::abi::{errno::*, nr, prot};
use crate::mm::{self, PAGE, page_down};
use crate::sys::*;
use crate::{Flow, Process};

#[derive(Default)]
struct Fake {
    pages: BTreeMap<u64, (Vec<u8>, u32)>,
    files: BTreeMap<String, Vec<u8>>,
    dirs: Vec<String>,
    trash: Vec<String>,
    out: Vec<u8>,
    input: Vec<Vec<u8>>,
    logs: Vec<String>,
    fs: u64,
    sent: Vec<u8>,
    net_ops: Vec<String>,
}

impl mm::Pages for Fake {
    fn unmap(&mut self, start: u64, end: u64) {
        self.pages.retain(|&p, _| p < start || p >= end);
    }
    fn protect(&mut self, start: u64, end: u64, p: u32) {
        for (_, v) in self.pages.range_mut(start..end) {
            v.1 = p;
        }
    }
}

impl System for Fake {
    fn read_user(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), Fault> {
        for (i, b) in buf.iter_mut().enumerate() {
            let a = addr + i as u64;
            if a >= mm::USER_END {
                return Err(Fault::Denied);
            }
            let (page, p) = self
                .pages
                .get(&page_down(a))
                .ok_or(Fault::Missing(page_down(a)))?;
            if *p == prot::NONE {
                return Err(Fault::Denied);
            }
            *b = page[(a % PAGE) as usize];
        }
        Ok(())
    }
    fn write_user(&mut self, addr: u64, data: &[u8], force: bool) -> Result<(), Fault> {
        for (i, &b) in data.iter().enumerate() {
            let a = addr + i as u64;
            if a >= mm::USER_END {
                return Err(Fault::Denied);
            }
            let (page, p) = self
                .pages
                .get_mut(&page_down(a))
                .ok_or(Fault::Missing(page_down(a)))?;
            if !force && *p & prot::WRITE == 0 {
                return Err(Fault::Denied);
            }
            page[(a % PAGE) as usize] = b;
        }
        Ok(())
    }
    fn map_page(&mut self, page: u64, p: u32) -> bool {
        self.pages
            .entry(page)
            .or_insert_with(|| (vec![0; PAGE as usize], p));
        true
    }
    fn set_fs(&mut self, base: u64) {
        self.fs = base;
    }
    fn realtime_ns(&mut self) -> u64 {
        1_790_000_000_000_000_000
    }
    fn monotonic_ns(&mut self) -> u64 {
        5_000_000_000
    }
    fn random(&mut self, buf: &mut [u8]) {
        buf.fill(0xAB);
    }
    fn sleep_ns(&mut self, _ns: u64) {}
    fn file(&mut self, op: FileOp) -> FileReply {
        match op {
            FileOp::Stat(p) => {
                if self.dirs.contains(&p) {
                    FileReply::Meta(Meta {
                        is_dir: true,
                        size: 0,
                        mtime: 7,
                    })
                } else if let Some(d) = self.files.get(&p) {
                    FileReply::Meta(Meta {
                        is_dir: false,
                        size: d.len() as u64,
                        mtime: 7,
                    })
                } else {
                    FileReply::Error(ENOENT)
                }
            }
            FileOp::Read(p) => self
                .files
                .get(&p)
                .cloned()
                .map_or(FileReply::Error(ENOENT), FileReply::Data),
            FileOp::Write(p, d) => {
                self.files.insert(p, d);
                FileReply::Done
            }
            FileOp::List(p) => {
                let prefix = if p == "/" { p.clone() } else { p.clone() + "/" };
                let mut l = Vec::new();
                for (name, dir) in self
                    .files
                    .keys()
                    .map(|k| (k, false))
                    .chain(self.dirs.iter().map(|k| (k, true)))
                {
                    if let Some(rest) = name.strip_prefix(&prefix)
                        && !rest.is_empty()
                        && !rest.contains('/')
                    {
                        l.push(DirEntry {
                            name: rest.to_string(),
                            is_dir: dir,
                        });
                    }
                }
                FileReply::List(l)
            }
            FileOp::Mkdir(p) => {
                self.dirs.push(p);
                FileReply::Done
            }
            FileOp::Remove(p) => match self.files.remove(&p) {
                Some(_) => {
                    self.trash.push(p);
                    FileReply::Done
                }
                None => FileReply::Error(ENOENT),
            },
            FileOp::Rmdir(_) | FileOp::Rename(..) => FileReply::Error(EPERM),
        }
    }
    fn console_write(&mut self, data: &[u8]) {
        self.out.extend_from_slice(data);
    }
    fn console_read(&mut self, _max: usize) -> Vec<u8> {
        if self.input.is_empty() {
            Vec::new()
        } else {
            self.input.remove(0)
        }
    }
    fn console_size(&mut self) -> (u16, u16) {
        (30, 100)
    }
    fn net(&mut self, op: NetOp) -> NetReply {
        match op {
            NetOp::Connect { ip, port } => {
                self.net_ops.push(alloc::format!("connect {ip:?}:{port}"));
                if port == 25 {
                    NetReply::Error(EACCES) // "el firewall"
                } else {
                    NetReply::Connected(7)
                }
            }
            NetOp::Send { data, .. } => {
                self.sent.extend_from_slice(&data);
                NetReply::Sent(data.len())
            }
            NetOp::Recv { .. } => NetReply::Data(b"HTTP/1.0 200 OK\r\n\r\n".to_vec()),
            NetOp::Close { id } => {
                self.net_ops.push(alloc::format!("close {id}"));
                NetReply::Done
            }
        }
    }
    fn log(&mut self, msg: &str) {
        self.logs.push(msg.to_string());
    }
}

/// Carga el ELF de juguete sobre un disco con /Documentos/notas.txt.
fn start() -> (Fake, Process) {
    let mut sys = Fake::default();
    sys.dirs.push("/".into());
    sys.dirs.push("/Documentos".into());
    sys.files
        .insert("/Documentos/notas.txt".into(), b"primera linea\n".to_vec());
    let elf = crate::elf::tests::tiny(2, false);
    let (p, entry, rsp) = Process::load(
        42,
        &elf,
        None,
        "/bin/juguete",
        "/Documentos",
        &[b"juguete", b"hola"],
        &[b"HOME=/"],
        &mut sys,
    )
    .unwrap();
    assert_eq!(entry, 0x40_1000);
    assert_eq!(rsp % 16, 0);
    (sys, p)
}

fn call(p: &mut Process, sys: &mut Fake, n: u64, a: &[u64]) -> i64 {
    let mut args = [0u64; 6];
    args[..a.len()].copy_from_slice(a);
    match p.syscall(n, args, sys) {
        Flow::Return(v) => v,
        Flow::Exit(c) => panic!("salió con {c}"),
    }
}

/// Un buffer en la memoria del proceso (un mmap anónimo) con `data` adentro.
fn buf(p: &mut Process, sys: &mut Fake, data: &[u8]) -> u64 {
    let len = (data.len() as u64).max(1);
    let a = call(p, sys, nr::MMAP, &[0, len, 3, 0x22, u64::MAX, 0]) as u64;
    p.copy_out(a, data, sys).unwrap();
    a
}

fn cstr(p: &mut Process, sys: &mut Fake, s: &str) -> u64 {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    buf(p, sys, &b)
}

const CWD: u64 = (-100i64) as u64;

#[test]
fn el_cargador_pone_cada_cosa_en_su_lugar() {
    let (mut sys, mut p) = start();
    // Código R-X con el contenido del archivo, datos RW- y .bss en cero.
    let (code, cp) = &sys.pages[&0x40_1000];
    assert_eq!(&code[..2], &[0x0F, 0x05]);
    assert_eq!(*cp, prot::READ | prot::EXEC);
    let (data, dp) = &sys.pages[&0x40_2000];
    assert_eq!(&data[0x800..0x810], b"datos iniciales!");
    assert_eq!(*dp, prot::READ | prot::WRITE);
    // El .bss (más allá de lo que trae el archivo) se asigna al tocarlo, en cero.
    assert!(p.fault(0x40_4000, true, false, &mut sys));
    assert!(sys.pages[&0x40_4000].0.iter().all(|&b| b == 0));
    // Escribir en el código: violación de segmento.
    assert!(!p.fault(0x40_1000, true, false, &mut sys));
    // Un puntero nulo: también.
    assert!(!p.fault(0, false, false, &mut sys));
}

#[test]
fn escribir_en_la_consola_y_en_archivos() {
    let (mut sys, mut p) = start();
    let msg = buf(&mut p, &mut sys, b"hola desde Linux\n");
    assert_eq!(call(&mut p, &mut sys, nr::WRITE, &[1, msg, 17]), 17);
    assert_eq!(sys.out, b"hola desde Linux\n");

    // Crear un archivo relativo al directorio actual, escribir y cerrar: queda en el disco.
    let path = cstr(&mut p, &mut sys, "salida.txt");
    let fd = call(&mut p, &mut sys, nr::OPENAT, &[CWD, path, 0o1101, 0o644]);
    assert_eq!(fd, 3);
    assert_eq!(call(&mut p, &mut sys, nr::WRITE, &[fd as u64, msg, 5]), 5);
    assert_eq!(
        sys.files["/Documentos/salida.txt"], b"",
        "se guarda al cerrar"
    );
    assert_eq!(call(&mut p, &mut sys, nr::CLOSE, &[fd as u64]), 0);
    assert_eq!(sys.files["/Documentos/salida.txt"], b"hola ");
    assert_eq!(call(&mut p, &mut sys, nr::CLOSE, &[fd as u64]), -EBADF);

    // Leer uno que existe, con fstat y lseek.
    let path = cstr(&mut p, &mut sys, "/Documentos/notas.txt");
    let fd = call(&mut p, &mut sys, nr::OPEN, &[path, 0]) as u64;
    let st = buf(&mut p, &mut sys, &[0; 144]);
    assert_eq!(call(&mut p, &mut sys, nr::FSTAT, &[fd, st]), 0);
    let size = p.copy_in(st + 48, 8, &mut sys).unwrap();
    assert_eq!(u64::from_le_bytes(size.try_into().unwrap()), 14);
    let out = buf(&mut p, &mut sys, &[0; 64]);
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[fd, out, 7]), 7);
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[fd, out + 7, 64]), 7);
    assert_eq!(
        call(&mut p, &mut sys, nr::READ, &[fd, out, 64]),
        0,
        "fin del archivo"
    );
    assert_eq!(p.copy_in(out + 7, 7, &mut sys).unwrap(), b" linea\n");
    assert_eq!(call(&mut p, &mut sys, nr::LSEEK, &[fd, 0, 0]), 0);
    // Escribir en un archivo abierto solo para leer: EBADF.
    assert_eq!(call(&mut p, &mut sys, nr::WRITE, &[fd, out, 1]), -EBADF);

    // No existe; y borrar manda a la Papelera.
    let missing = cstr(&mut p, &mut sys, "no-existe");
    assert_eq!(call(&mut p, &mut sys, nr::OPEN, &[missing, 0]), -ENOENT);
    let salida = cstr(&mut p, &mut sys, "salida.txt");
    assert_eq!(call(&mut p, &mut sys, nr::UNLINK, &[salida]), 0);
    assert_eq!(sys.trash, ["/Documentos/salida.txt"]);
}

#[test]
fn listar_un_directorio() {
    let (mut sys, mut p) = start();
    let path = cstr(&mut p, &mut sys, ".");
    let fd = call(&mut p, &mut sys, nr::OPENAT, &[CWD, path, 0o200000]) as u64;
    let out = buf(&mut p, &mut sys, &[0; 256]);
    let n = call(&mut p, &mut sys, nr::GETDENTS64, &[fd, out, 256]);
    assert!(n > 0);
    let b = p.copy_in(out, n as usize, &mut sys).unwrap();
    assert!(b.windows(9).any(|w| w == b"notas.txt"));
    assert_eq!(
        call(&mut p, &mut sys, nr::GETDENTS64, &[fd, out, 256]),
        0,
        "ya no quedan"
    );
    // getcwd y chdir.
    assert_eq!(call(&mut p, &mut sys, nr::GETCWD, &[out, 256]), 12);
    assert_eq!(p.copy_in(out, 12, &mut sys).unwrap(), b"/Documentos\0");
    let up = cstr(&mut p, &mut sys, "..");
    assert_eq!(call(&mut p, &mut sys, nr::CHDIR, &[up]), 0);
    assert_eq!(p.cwd, "/");
    let file = cstr(&mut p, &mut sys, "/Documentos/notas.txt");
    assert_eq!(call(&mut p, &mut sys, nr::CHDIR, &[file]), -ENOTDIR);
}

#[test]
fn memoria_brk_mmap_y_mprotect() {
    let (mut sys, mut p) = start();
    let base = call(&mut p, &mut sys, nr::BRK, &[0]) as u64;
    assert_eq!(
        call(&mut p, &mut sys, nr::BRK, &[base + 10_000]) as u64,
        base + 10_000
    );
    // Las páginas del heap aparecen al usarlas.
    assert!(!sys.pages.contains_key(&base));
    assert!(p.fault(base + 5000, true, false, &mut sys));
    assert!(sys.pages.contains_key(&(base + 4096)));
    // Un mmap grande no gasta nada hasta que se usa.
    let before = sys.pages.len();
    let big = call(
        &mut p,
        &mut sys,
        nr::MMAP,
        &[0, 1 << 30, 3, 0x22, u64::MAX, 0],
    ) as u64;
    assert_eq!(sys.pages.len(), before);
    assert!(p.fault(big + (1 << 29), true, false, &mut sys));
    assert_eq!(sys.pages.len(), before + 1);
    // mprotect a nada: una página de guarda.
    assert_eq!(call(&mut p, &mut sys, nr::MPROTECT, &[big, 4096, 0]), 0);
    assert!(!p.fault(big, false, false, &mut sys));
    // munmap: la página se libera y la dirección deja de valer.
    assert_eq!(call(&mut p, &mut sys, nr::MUNMAP, &[big, 1 << 30]), 0);
    assert!(!sys.pages.contains_key(&(big + (1 << 29))));
    assert!(!p.fault(big + 4096, false, false, &mut sys));
}

#[test]
fn punteros_del_kernel_dan_efault() {
    let (mut sys, mut p) = start();
    // Un programa malicioso le pasa al kernel una dirección del kernel para que la lea o la pise.
    let kernel = 0xFFFF_8000_0000_0000;
    assert_eq!(call(&mut p, &mut sys, nr::WRITE, &[1, kernel, 16]), -EFAULT);
    let zero = cstr(&mut p, &mut sys, "/dev/zero");
    let fd = call(&mut p, &mut sys, nr::OPEN, &[zero, 0]) as u64;
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[fd, kernel, 16]), -EFAULT);
    // Una sin mapear, fuera de toda zona.
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[fd, 0x1000, 16]), -EFAULT);
    // Sobre el código (solo lectura) tampoco se puede escribir.
    assert_eq!(
        call(&mut p, &mut sys, nr::READ, &[fd, 0x40_1000, 16]),
        -EFAULT
    );
    assert_eq!(sys.pages[&0x40_1000].0[0], 0x0F);
}

#[test]
fn identidad_tiempo_azar_y_llamadas_que_faltan() {
    let (mut sys, mut p) = start();
    assert_eq!(call(&mut p, &mut sys, nr::GETPID, &[]), 42);
    assert_eq!(call(&mut p, &mut sys, nr::GETUID, &[]), 1000);
    let u = buf(&mut p, &mut sys, &[0; 390]);
    assert_eq!(call(&mut p, &mut sys, nr::UNAME, &[u]), 0);
    assert_eq!(p.copy_in(u, 5, &mut sys).unwrap(), b"Linux");
    assert_eq!(p.copy_in(u + 65 * 4, 6, &mut sys).unwrap(), b"x86_64");
    let ts = buf(&mut p, &mut sys, &[0; 16]);
    assert_eq!(call(&mut p, &mut sys, nr::CLOCK_GETTIME, &[1, ts]), 0);
    assert_eq!(p.copy_in(ts, 8, &mut sys).unwrap(), 5u64.to_le_bytes());
    assert_eq!(call(&mut p, &mut sys, nr::GETRANDOM, &[ts, 16, 0]), 16);
    assert_eq!(p.copy_in(ts, 16, &mut sys).unwrap(), [0xAB; 16]);
    assert_eq!(call(&mut p, &mut sys, nr::ARCH_PRCTL, &[0x1002, 0x7000]), 0);
    assert_eq!(sys.fs, 0x7000);
    assert_eq!(call(&mut p, &mut sys, 9999, &[]), -ENOSYS);
    assert!(sys.logs[0].contains("9999"), "{:?}", sys.logs);
    let mut a = [0u64; 6];
    a[0] = 3;
    assert_eq!(p.syscall(nr::EXIT_GROUP, a, &mut sys), Flow::Exit(3));
    // abort(): SIGABRT → 128 + 6.
    a[1] = 42;
    a[2] = 6;
    assert_eq!(p.syscall(nr::TGKILL, a, &mut sys), Flow::Exit(134));
}

#[test]
fn la_entrada_viene_de_la_terminal() {
    let (mut sys, mut p) = start();
    sys.input.push(b"Roman\n".to_vec());
    let b = buf(&mut p, &mut sys, &[0; 64]);
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[0, b, 64]), 6);
    assert_eq!(p.copy_in(b, 6, &mut sys).unwrap(), b"Roman\n");
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[0, b, 64]), 0, "Ctrl+D");
    let ws = buf(&mut p, &mut sys, &[0; 8]);
    assert_eq!(call(&mut p, &mut sys, nr::IOCTL, &[1, 0x5413, ws]), 0);
    assert_eq!(p.copy_in(ws, 4, &mut sys).unwrap(), [30, 0, 100, 0]);
    let notas = cstr(&mut p, &mut sys, "notas.txt");
    let fd = call(&mut p, &mut sys, nr::OPEN, &[notas, 0]) as u64;
    assert_eq!(
        call(&mut p, &mut sys, nr::IOCTL, &[fd, 0x5413, ws]),
        -ENOTTY,
        "un archivo no es una terminal"
    );
}

#[test]
fn sockets_tcp_por_el_sistema() {
    let (mut sys, mut p) = start();
    assert_eq!(
        call(&mut p, &mut sys, nr::SOCKET, &[10, 1, 0]),
        -EAFNOSUPPORT,
        "IPv6"
    );
    assert_eq!(
        call(&mut p, &mut sys, nr::SOCKET, &[2, 2, 0]),
        -EPROTONOSUPPORT,
        "UDP"
    );
    let s = call(&mut p, &mut sys, nr::SOCKET, &[2, 1 | 0o2000000, 0]) as u64;
    // sockaddr_in: familia 2, puerto 80 (big endian), 93.184.215.14.
    let sa = [2, 0, 0, 80, 93, 184, 215, 14, 0, 0, 0, 0, 0, 0, 0, 0];
    let addr = buf(&mut p, &mut sys, &sa);
    assert_eq!(call(&mut p, &mut sys, nr::CONNECT, &[s, addr, 16]), 0);
    assert_eq!(sys.net_ops[0], "connect [93, 184, 215, 14]:80");
    let req = buf(&mut p, &mut sys, b"GET / HTTP/1.0\r\n\r\n");
    assert_eq!(
        call(&mut p, &mut sys, nr::SENDTO, &[s, req, 18, 0, 0, 0]),
        18
    );
    assert_eq!(sys.sent, b"GET / HTTP/1.0\r\n\r\n");
    let out = buf(&mut p, &mut sys, &[0; 64]);
    assert_eq!(call(&mut p, &mut sys, nr::READ, &[s, out, 64]), 19);
    assert_eq!(call(&mut p, &mut sys, nr::CLOSE, &[s]), 0);
    assert_eq!(sys.net_ops[1], "close 7");
    // El firewall dice que no: EACCES.
    let s = call(&mut p, &mut sys, nr::SOCKET, &[2, 1, 0]) as u64;
    let smtp = buf(
        &mut p,
        &mut sys,
        &[2, 0, 0, 25, 1, 2, 3, 4, 0, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_eq!(call(&mut p, &mut sys, nr::CONNECT, &[s, smtp, 16]), -EACCES);
}

#[test]
fn dup_comparte_la_posicion_y_close_all_guarda() {
    let (mut sys, mut p) = start();
    let path = cstr(&mut p, &mut sys, "log.txt");
    // O_RDWR | O_CREAT | O_APPEND
    let fd = call(&mut p, &mut sys, nr::OPEN, &[path, 0o102 | 0o2000]) as u64;
    let dup = call(&mut p, &mut sys, nr::DUP, &[fd]) as u64;
    let a = buf(&mut p, &mut sys, b"uno ");
    let b = buf(&mut p, &mut sys, b"dos");
    call(&mut p, &mut sys, nr::WRITE, &[fd, a, 4]);
    call(&mut p, &mut sys, nr::WRITE, &[dup, b, 3]);
    call(&mut p, &mut sys, nr::CLOSE, &[fd]);
    assert_eq!(
        sys.files["/Documentos/log.txt"], b"",
        "el dup lo sigue usando"
    );
    p.close_all(&mut sys);
    assert_eq!(sys.files["/Documentos/log.txt"], b"uno dos");
}

#[test]
fn un_programa_dinamico_arranca_por_su_interprete() {
    use crate::elf::{INTERP_BASE, PIE_BASE};
    let mut sys = Fake::default();
    sys.dirs.push("/".into());
    let prog = crate::elf::tests::tiny(3, true);
    // Sin el intérprete en el disco: el error dice qué instalar.
    let e = Process::load(7, &prog, None, "/bin/dyn", "/", &[b"dyn"], &[], &mut sys)
        .err()
        .unwrap();
    assert!(e.to_string().contains("libc6"), "{e}");
    // Quien lo lanza busca el intérprete (Debian lo pone en /usr/lib/x86_64-linux-gnu;
    // /lib64/ld-linux-x86-64.so.2 es un enlace, que FAT32 no tiene).
    let paths = crate::elf::interpreter_paths("/lib64/ld-linux-x86-64.so.2");
    assert_eq!(paths[2], "/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2");
    let ld = crate::elf::tests::tiny(3, false);
    let (mut p, entry, rsp) = Process::load(
        7,
        &prog,
        Some(&ld),
        "/bin/dyn",
        "/",
        &[b"dyn"],
        &[],
        &mut sys,
    )
    .unwrap();
    assert_eq!(entry, INTERP_BASE + 0x1000, "arranca por el intérprete");
    assert!(
        sys.pages.contains_key(&(PIE_BASE + 0x1000)),
        "y el programa está cargado"
    );
    // El vector auxiliar (después de argc, argv[0], el fin de argv y el de envp) le dice al
    // intérprete dónde quedó él (AT_BASE) y dónde empieza el programa (AT_ENTRY).
    let mut aux = Vec::new();
    let mut at = rsp + 8 * 4;
    loop {
        let k = u64::from_le_bytes(p.copy_in(at, 8, &mut sys).unwrap().try_into().unwrap());
        let v = u64::from_le_bytes(p.copy_in(at + 8, 8, &mut sys).unwrap().try_into().unwrap());
        if k == 0 {
            break;
        }
        aux.push((k, v));
        at += 16;
    }
    assert!(aux.contains(&(7, INTERP_BASE)), "{aux:x?}");
    assert!(aux.contains(&(9, PIE_BASE + 0x1000)), "{aux:x?}");
}
