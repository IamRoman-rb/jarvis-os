//! La entrada/salida "de bajo nivel" del CRT: descriptores de archivo (`int fd`) como los de
//! POSIX, encima de los HANDLE de Windows.
//!
//! En Windows, `_open` devuelve un número chico (0, 1, 2 son la entrada, la salida y el error) y
//! el CRT guarda en una tabla a qué HANDLE corresponde cada uno. Python usa esto para todos sus
//! archivos (`FileIO` llama a `_wopen`, `_read`, `_write`, `_lseeki64`, `_fstat64`), así que acá
//! está la tabla: cada descriptor es un HANDLE de [`super::kernel32`].

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use std::io::Write;
use std::sync::Mutex;

use super::crt::{errno, set_errno};
use super::kernel32::{self, create_file, read_handle, std_of, write_handle};
use crate::state::{self, Object, STD_ERR, STD_IN, STD_OUT, state};
use crate::win::{self, cstr, wstr};

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    exports![
        _open,
        _wopen,
        _sopen_s,
        _wsopen_s,
        _read,
        _write,
        _close,
        _lseek,
        _lseeki64,
        _tell,
        _telli64,
        _eof,
        _dup,
        _dup2,
        _fstat64,
        _fstat64i32,
        _fstat,
        _stat64,
        _stat64i32,
        _wstat64,
        _wstat64i32,
        _stat,
        _wstat,
        _isatty,
        _setmode,
        _get_osfhandle,
        _open_osfhandle,
        _commit,
        _chsize,
        _chsize_s,
        _locking,
        _umask,
        _umask_s,
        _kbhit,
        _getch,
        _getche,
        _getwch,
        _getwche,
        _putch,
        _putwch,
        _ungetch,
        _ungetwch,
        _filelength,
        _filelengthi64,
    ]
}

/// Un descriptor: su HANDLE y si es de texto (`\r\n` → `\n` al leer).
#[derive(Clone, Copy)]
struct Fd {
    handle: u64,
    text: bool,
}

static FDS: Mutex<Vec<Option<Fd>>> = Mutex::new(Vec::new());

fn fds() -> std::sync::MutexGuard<'static, Vec<Option<Fd>>> {
    let mut g = FDS.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_empty() {
        for h in [STD_IN, STD_OUT, STD_ERR] {
            g.push(Some(Fd {
                handle: h,
                text: true,
            }));
        }
    }
    g
}

fn get(fd: i32) -> Option<Fd> {
    usize::try_from(fd)
        .ok()
        .and_then(|i| fds().get(i).copied().flatten())
}

/// El HANDLE de un descriptor (o `None` y errno = EBADF).
fn handle(fd: i32) -> Option<u64> {
    let h = get(fd).map(|f| f.handle);
    if h.is_none() {
        set_errno(errno::EBADF);
    }
    h
}

/// El HANDLE de un descriptor (para `_fdopen`).
pub fn handle_of(fd: i32) -> Option<u64> {
    get(fd).map(|f| f.handle)
}

/// Un descriptor nuevo (el número libre más chico, como POSIX).
pub fn new_fd(handle: u64, text: bool) -> i32 {
    let mut t = fds();
    let fd = Fd { handle, text };
    match t.iter().position(Option::is_none) {
        Some(i) => {
            t[i] = Some(fd);
            i as i32
        }
        None => {
            t.push(Some(fd));
            (t.len() - 1) as i32
        }
    }
}

const O_WRONLY: i32 = 0x1;
const O_RDWR: i32 = 0x2;
const O_APPEND: i32 = 0x8;
const O_CREAT: i32 = 0x100;
const O_TRUNC: i32 = 0x200;
const O_EXCL: i32 = 0x400;
const O_TEXT: i32 = 0x4000;
const O_BINARY: i32 = 0x8000;

/// `_open(ruta, flags)` → el CreateFile que corresponde.
fn open(path: &str, flags: i32) -> i32 {
    let access = match flags & 3 {
        O_WRONLY => 0x4000_0000,
        O_RDWR => 0xC000_0000,
        _ => 0x8000_0000,
    } | if flags & O_APPEND != 0 { 4 } else { 0 };
    let disposition = match (
        flags & O_CREAT != 0,
        flags & O_EXCL != 0,
        flags & O_TRUNC != 0,
    ) {
        (true, true, _) => 1,      // CREATE_NEW
        (true, false, true) => 2,  // CREATE_ALWAYS
        (true, false, false) => 4, // OPEN_ALWAYS
        (false, _, true) => 5,     // TRUNCATE_EXISTING
        _ => 3,                    // OPEN_EXISTING
    };
    // Las carpetas también se abren (Python lo hace para saber si existen).
    match create_file(path, access, disposition, 0x0200_0000) {
        Ok(h) => {
            if flags & O_APPEND != 0 {
                let _ = kernel32::seek(h, 0, 2);
            }
            let text = flags & O_BINARY == 0 && (flags & O_TEXT != 0);
            new_fd(h, text)
        }
        Err(e) => {
            set_errno(match e {
                win::error::FILE_NOT_FOUND | win::error::PATH_NOT_FOUND => errno::ENOENT,
                win::error::FILE_EXISTS | win::error::ALREADY_EXISTS => errno::EEXIST,
                win::error::ACCESS_DENIED => errno::EACCES,
                _ => errno::EINVAL,
            });
            -1
        }
    }
}

unsafe extern "win64" fn _open(path: *const u8, flags: i32, _mode: i32) -> i32 {
    open(&String::from_utf8_lossy(cstr(path)), flags)
}

unsafe extern "win64" fn _wopen(path: *const u16, flags: i32, _mode: i32) -> i32 {
    open(&wstr(path), flags)
}

unsafe extern "win64" fn _sopen_s(
    out: *mut i32,
    path: *const u8,
    flags: i32,
    _sh: i32,
    _mode: i32,
) -> i32 {
    let fd = open(&String::from_utf8_lossy(cstr(path)), flags);
    *out = fd;
    if fd < 0 { super::crt::get_errno() } else { 0 }
}

unsafe extern "win64" fn _wsopen_s(
    out: *mut i32,
    path: *const u16,
    flags: i32,
    _sh: i32,
    _mode: i32,
) -> i32 {
    let fd = open(&wstr(path), flags);
    *out = fd;
    if fd < 0 { super::crt::get_errno() } else { 0 }
}

unsafe extern "win64" fn _read(fd: i32, buf: *mut u8, n: u32) -> i32 {
    let Some(f) = get(fd) else {
        set_errno(errno::EBADF);
        return -1;
    };
    let out = std::slice::from_raw_parts_mut(buf, n as usize);
    match read_handle(f.handle, out) {
        Ok(k) if f.text => {
            // Modo texto: los `\r\n` del archivo llegan como `\n`.
            let clean = kernel32::strip_cr(&out[..k]);
            out[..clean.len()].copy_from_slice(&clean);
            clean.len() as i32
        }
        Ok(k) => k as i32,
        Err(_) => {
            set_errno(errno::EBADF);
            -1
        }
    }
}

unsafe extern "win64" fn _write(fd: i32, buf: *const u8, n: u32) -> i32 {
    let Some(h) = handle(fd) else { return -1 };
    match write_handle(h, std::slice::from_raw_parts(buf, n as usize)) {
        Ok(k) => k as i32,
        Err(_) => {
            set_errno(errno::EBADF);
            -1
        }
    }
}

unsafe extern "win64" fn _close(fd: i32) -> i32 {
    let taken = usize::try_from(fd)
        .ok()
        .and_then(|i| fds().get_mut(i).and_then(Option::take));
    match taken {
        Some(f) => {
            // Si otro descriptor (de `_dup`) usa el mismo HANDLE, queda abierto.
            let shared = fds().iter().flatten().any(|o| o.handle == f.handle);
            if !shared && fd > 2 {
                state().close(f.handle);
            }
            0
        }
        None => {
            set_errno(errno::EBADF);
            -1
        }
    }
}

unsafe extern "win64" fn _lseeki64(fd: i32, off: i64, whence: i32) -> i64 {
    let Some(h) = handle(fd) else { return -1 };
    match kernel32::seek(h, off, whence as u32) {
        Ok(p) => p as i64,
        Err(_) => {
            set_errno(errno::EINVAL);
            -1
        }
    }
}

unsafe extern "win64" fn _lseek(fd: i32, off: i32, whence: i32) -> i32 {
    _lseeki64(fd, off as i64, whence) as i32
}

unsafe extern "win64" fn _telli64(fd: i32) -> i64 {
    _lseeki64(fd, 0, 1)
}

unsafe extern "win64" fn _tell(fd: i32) -> i32 {
    _lseeki64(fd, 0, 1) as i32
}

unsafe extern "win64" fn _filelengthi64(fd: i32) -> i64 {
    let Some(h) = handle(fd) else { return -1 };
    kernel32::handle_meta(h).map_or(-1, |m| m.1 as i64)
}

unsafe extern "win64" fn _filelength(fd: i32) -> i32 {
    _filelengthi64(fd) as i32
}

unsafe extern "win64" fn _eof(fd: i32) -> i32 {
    let Some(h) = handle(fd) else { return -1 };
    let pos = kernel32::seek(h, 0, 1).unwrap_or(0);
    let len = kernel32::handle_meta(h).map_or(0, |m| m.1);
    i32::from(pos >= len)
}

unsafe extern "win64" fn _dup(fd: i32) -> i32 {
    match get(fd) {
        Some(f) => new_fd(f.handle, f.text),
        None => {
            set_errno(errno::EBADF);
            -1
        }
    }
}

unsafe extern "win64" fn _dup2(from: i32, to: i32) -> i32 {
    let Some(f) = get(from) else {
        set_errno(errno::EBADF);
        return -1;
    };
    let Ok(i) = usize::try_from(to) else {
        set_errno(errno::EBADF);
        return -1;
    };
    let mut t = fds();
    if t.len() <= i {
        t.resize(i + 1, None);
    }
    t[i] = Some(f);
    0
}

/// Los datos de `struct _stat64` (o `_stat64i32`, con el tamaño en 32 bits).
#[derive(Debug, PartialEq, Eq)]
pub struct Stat {
    pub mode: u16,
    pub size: u64,
    pub mtime: i64,
}

const S_IFDIR: u16 = 0x4000;
const S_IFCHR: u16 = 0x2000;
const S_IFREG: u16 = 0x8000;

/// Atributos de Windows → el `st_mode` que arma el CRT de Microsoft: lectura siempre, escritura
/// si no es de solo lectura, ejecución si es carpeta o `.exe`/`.bat`/`.cmd`/`.com`; los permisos
/// se repiten para el grupo y los demás.
pub fn mode_of(attrs: u32, name: &str) -> u16 {
    let dir = attrs & 0x10 != 0;
    let mut m: u16 = if dir { S_IFDIR } else { S_IFREG };
    let mut perm: u16 = 0o4;
    if attrs & 1 == 0 {
        perm |= 0o2;
    }
    let lower = name.to_ascii_lowercase();
    if dir
        || [".exe", ".bat", ".cmd", ".com"]
            .iter()
            .any(|e| lower.ends_with(e))
    {
        perm |= 0o1;
    }
    m |= perm << 6 | perm << 3 | perm;
    m
}

fn unix_secs(filetime: u64) -> i64 {
    (filetime / 10_000_000) as i64 - win::EPOCH_DIFF as i64
}

/// Escribe `st` en la estructura del CRT: 56 bytes (`_stat64`) o 48 (`_stat64i32`).
unsafe fn put_stat(buf: *mut u8, st: &Stat, size64: bool) {
    let len = if size64 { 56 } else { 48 };
    std::ptr::write_bytes(buf, 0, len);
    (buf.add(6) as *mut u16).write_unaligned(st.mode);
    (buf.add(8) as *mut i16).write_unaligned(1); // st_nlink
    let times = if size64 {
        (buf.add(24) as *mut u64).write_unaligned(st.size);
        32
    } else {
        (buf.add(20) as *mut u32).write_unaligned(st.size as u32);
        24
    };
    for i in 0..3 {
        (buf.add(times + i * 8) as *mut i64).write_unaligned(st.mtime);
    }
}

fn stat_fd(fd: i32) -> Option<Stat> {
    let h = handle(fd)?;
    if let Ok(Some(_)) = std_of(h) {
        // La Terminal: un dispositivo de caracteres.
        return Some(Stat {
            mode: S_IFCHR | 0o666,
            size: 0,
            mtime: 0,
        });
    }
    let name = match state().get(h) {
        Some(Object::File { path, .. }) => path.clone(),
        _ => String::new(),
    };
    let (attrs, size, ft, _) = kernel32::handle_meta(h).ok()?;
    Some(Stat {
        mode: mode_of(attrs, &name),
        size,
        mtime: unix_secs(ft),
    })
}

fn stat_path(path: &str) -> Option<Stat> {
    let unix = win::to_unix(path);
    match std::fs::metadata(&unix) {
        Ok(m) => {
            let attrs =
                if m.is_dir() { 0x10 } else { 0x20 } | u32::from(m.permissions().readonly());
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Some(Stat {
                mode: mode_of(attrs, &unix),
                size: m.len(),
                mtime,
            })
        }
        Err(_) => {
            set_errno(errno::ENOENT);
            None
        }
    }
}

unsafe fn fstat(fd: i32, buf: *mut u8, size64: bool) -> i32 {
    match stat_fd(fd) {
        Some(st) => {
            put_stat(buf, &st, size64);
            0
        }
        None => -1,
    }
}

unsafe fn stat(path: &str, buf: *mut u8, size64: bool) -> i32 {
    match stat_path(path) {
        Some(st) => {
            put_stat(buf, &st, size64);
            0
        }
        None => -1,
    }
}

unsafe extern "win64" fn _fstat64(fd: i32, buf: *mut u8) -> i32 {
    fstat(fd, buf, true)
}
unsafe extern "win64" fn _fstat64i32(fd: i32, buf: *mut u8) -> i32 {
    fstat(fd, buf, false)
}
unsafe extern "win64" fn _fstat(fd: i32, buf: *mut u8) -> i32 {
    fstat(fd, buf, false)
}
unsafe extern "win64" fn _stat64(p: *const u8, buf: *mut u8) -> i32 {
    stat(&String::from_utf8_lossy(cstr(p)), buf, true)
}
unsafe extern "win64" fn _stat64i32(p: *const u8, buf: *mut u8) -> i32 {
    stat(&String::from_utf8_lossy(cstr(p)), buf, false)
}
unsafe extern "win64" fn _stat(p: *const u8, buf: *mut u8) -> i32 {
    stat(&String::from_utf8_lossy(cstr(p)), buf, false)
}
unsafe extern "win64" fn _wstat64(p: *const u16, buf: *mut u8) -> i32 {
    stat(&wstr(p), buf, true)
}
unsafe extern "win64" fn _wstat64i32(p: *const u16, buf: *mut u8) -> i32 {
    stat(&wstr(p), buf, false)
}
unsafe extern "win64" fn _wstat(p: *const u16, buf: *mut u8) -> i32 {
    stat(&wstr(p), buf, false)
}

unsafe extern "win64" fn _isatty(fd: i32) -> i32 {
    match get(fd) {
        Some(f) => i32::from(matches!(std_of(f.handle), Ok(Some(_)))),
        None => {
            set_errno(errno::EBADF);
            0
        }
    }
}

unsafe extern "win64" fn _setmode(fd: i32, mode: i32) -> i32 {
    let Ok(i) = usize::try_from(fd) else {
        set_errno(errno::EBADF);
        return -1;
    };
    match fds().get_mut(i).and_then(Option::as_mut) {
        Some(f) => {
            let old = if f.text { O_TEXT } else { O_BINARY };
            f.text = mode & O_TEXT != 0;
            old
        }
        None => {
            set_errno(errno::EBADF);
            -1
        }
    }
}

unsafe extern "win64" fn _get_osfhandle(fd: i32) -> u64 {
    handle(fd).unwrap_or(state::INVALID_HANDLE)
}

unsafe extern "win64" fn _open_osfhandle(h: u64, flags: i32) -> i32 {
    new_fd(h, flags & O_TEXT != 0)
}

unsafe extern "win64" fn _commit(fd: i32) -> i32 {
    let Some(h) = handle(fd) else { return -1 };
    let _ = kernel32::with_file(h, |f| f.flush());
    0
}

unsafe extern "win64" fn _chsize_s(fd: i32, size: i64) -> i32 {
    let Some(h) = handle(fd) else {
        return errno::EBADF;
    };
    match kernel32::with_file(h, |f| f.set_len(size.max(0) as u64)) {
        Ok(()) => 0,
        Err(_) => errno::EACCES,
    }
}

unsafe extern "win64" fn _chsize(fd: i32, size: i32) -> i32 {
    if _chsize_s(fd, size as i64) == 0 {
        0
    } else {
        -1
    }
}

/// Bloquear partes de un archivo: con un solo proceso usándolo, siempre se puede.
unsafe extern "win64" fn _locking(_fd: i32, _mode: i32, _n: i32) -> i32 {
    0
}

unsafe extern "win64" fn _umask(_m: i32) -> i32 {
    0
}

unsafe extern "win64" fn _umask_s(_m: i32, old: *mut i32) -> i32 {
    if !old.is_null() {
        *old = 0;
    }
    0
}

// --- La consola de a una tecla (conio.h) ---------------------------------------------------------
// La Terminal manda la entrada por renglones: `_getch` espera el Enter y devuelve de a un byte.

static UNGOT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn getch() -> i32 {
    if let Some(c) = UNGOT.lock().ok().and_then(|mut u| u.pop()) {
        return c as i32;
    }
    let mut b = [0u8; 1];
    match read_handle(STD_IN, &mut b) {
        Ok(1) => b[0] as i32,
        _ => -1,
    }
}

unsafe extern "win64" fn _kbhit() -> i32 {
    0
}
unsafe extern "win64" fn _getch() -> i32 {
    getch()
}
unsafe extern "win64" fn _getche() -> i32 {
    getch()
}
unsafe extern "win64" fn _getwch() -> u16 {
    getch() as u16
}
unsafe extern "win64" fn _getwche() -> u16 {
    getch() as u16
}
unsafe extern "win64" fn _ungetch(c: i32) -> i32 {
    if let Ok(mut u) = UNGOT.lock() {
        u.push(c as u8);
    }
    c
}
unsafe extern "win64" fn _ungetwch(c: u16) -> u16 {
    _ungetch(c as i32);
    c
}
unsafe extern "win64" fn _putch(c: i32) -> i32 {
    let _ = kernel32::write_console(1, &[c as u8]);
    c
}
unsafe extern "win64" fn _putwch(c: u16) -> u16 {
    let s = char::from_u32(c as u32).unwrap_or('?').to_string();
    let _ = kernel32::write_console(1, s.as_bytes());
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modos_como_el_crt() {
        assert_eq!(mode_of(0x20, "a.txt"), S_IFREG | 0o666);
        assert_eq!(mode_of(0x21, "a.txt"), S_IFREG | 0o444);
        assert_eq!(mode_of(0x10, "carpeta"), S_IFDIR | 0o777);
        assert_eq!(mode_of(0x20, "x.EXE"), S_IFREG | 0o777);
    }

    #[test]
    fn descriptores_sobre_archivos() {
        let path = "target/vino-prueba-lowio.bin";
        let w = win::to_wide_nul(path);
        // SAFETY: rutas terminadas en cero y buffers del tamaño indicado.
        unsafe {
            let fd = _wopen(w.as_ptr(), O_WRONLY | O_CREAT | O_TRUNC | O_BINARY, 0o666);
            assert!(fd > 2, "los primeros tres son los estándar: {fd}");
            assert_eq!(_write(fd, b"hola\r\nchau".as_ptr(), 10), 10);
            assert_eq!(_close(fd), 0);
            assert_eq!(_close(fd), -1);
            let fd = _wopen(w.as_ptr(), O_TEXT, 0);
            let mut buf = [0u8; 32];
            assert_eq!(_read(fd, buf.as_mut_ptr(), 32), 9, "modo texto: sin el \\r");
            assert_eq!(&buf[..9], b"hola\nchau");
            assert_eq!(_lseeki64(fd, 2, 0), 2);
            let mut st = [0u8; 56];
            assert_eq!(_fstat64(fd, st.as_mut_ptr()), 0);
            assert_eq!(u64::from_le_bytes(st[24..32].try_into().unwrap()), 10);
            assert_eq!(u16::from_le_bytes([st[6], st[7]]) & 0xF000, S_IFREG);
            let d = _dup(fd);
            assert_ne!(d, fd);
            assert_eq!(_close(fd), 0);
            assert_eq!(_lseeki64(d, 0, 1), 2, "el duplicado sigue abierto");
            _close(d);
            let mut st = [0u8; 48];
            assert_eq!(
                _wstat64i32(win::to_wide_nul("target").as_ptr(), st.as_mut_ptr()),
                0
            );
            assert_eq!(u16::from_le_bytes([st[6], st[7]]) & 0xF000, S_IFDIR);
            assert_eq!(
                _wstat64i32(
                    win::to_wide_nul("target/no-existe").as_ptr(),
                    st.as_mut_ptr()
                ),
                -1
            );
        }
        let _ = std::fs::remove_file(path);
    }
}
