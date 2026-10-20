//! Más de `kernel32` (y de los "api sets" que van a parar ahí): datos del sistema, rutas
//! (`PathCch*`), archivos mapeados en memoria y la lista de lo que JARVIS-OS **no** puede hacer
//! todavía (crear procesos, tuberías, otros procesos, discos…).
//!
//! Lo que no se puede hacer no se simula: falla como falla en Windows (FALSE o NULL y
//! `GetLastError() == ERROR_NOT_SUPPORTED`), y el programa decide qué hacer. Es distinto de
//! "falta" (que termina el programa): el programa pidió algo, la respuesta es "no".

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::sync::Mutex;

use super::kernel32;
use crate::os;
use crate::state::{self, Object, state};
use crate::win::{self, error, wstr};

type Bool = i32;

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    let mut v = exports![
        GetCurrentThreadStackLimits,
        GetActiveProcessorCount,
        GetActiveProcessorGroupCount,
        GetMaximumProcessorCount,
        GetCurrentProcessorNumber,
        FlushInstructionCache,
        GetLongPathNameW,
        GetShortPathNameW,
        NeedCurrentDirectoryForExePathW,
        GetProcessTimes,
        GetThreadTimes,
        GetSystemTimes,
        GetDriveTypeW,
        GetDiskFreeSpaceExW,
        GetLogicalDrives,
        GetLogicalDriveStringsW,
        GetVolumePathNameW,
        GetVolumeInformationW,
        GetLocaleInfoA,
        CreateSemaphoreA,
        AddDllDirectory,
        RemoveDllDirectory,
        SetDefaultDllDirectories,
        SetDllDirectoryW,
        GetLargePageMinimum,
        GetNumaHighestNodeNumber,
        SetThreadUILanguage,
        GetThreadLocale,
        SetThreadLocale,
        CompareStringA,
        FindStringOrdinal,
        GlobalMemoryStatusEx,
        GetPhysicallyInstalledSystemMemory,
        PathCchSkipRoot,
        PathCchCombineEx,
        PathCchCombine,
        PathCchCanonicalizeEx,
        PathCchAppendEx,
        PathCchRemoveFileSpec,
        CreateFileMappingW,
        CreateFileMappingA,
        MapViewOfFile,
        MapViewOfFileEx,
        UnmapViewOfFile,
        FlushViewOfFile,
        CopyFile2,
        GetFileInformationByHandleEx2,
        SetThreadPriority,
        GetThreadPriority,
        SetPriorityClass,
        GetPriorityClass,
        GetProcessIoCounters,
        QueryUnbiasedInterruptTime,
        GetSystemFirmwareTable,
        GetComputerNameExW,
        IsValidLocaleName,
        GetConsoleTitleW,
        WTSGetActiveConsoleSessionId,
        ProcessIdToSessionId,
        DisableThreadLibraryCalls,
    ];
    // Lo que JARVIS-OS no puede hacer: falla con ERROR_NOT_SUPPORTED.
    for name in NOT_SUPPORTED {
        v.push((name, not_supported as *const () as usize));
    }
    // Las que devuelven un HANDLE: fallan con INVALID_HANDLE_VALUE.
    for name in NOT_SUPPORTED_HANDLE {
        v.push((name, not_supported_handle as *const () as usize));
    }
    v
}

/// Devuelven FALSE / NULL / 0.
const NOT_SUPPORTED: [&str; 46] = [
    "CreateProcessW",
    "CreateProcessA",
    "OpenProcess",
    "OpenThread",
    "ResumeThread",
    "SuspendThread",
    "TerminateThread",
    "GetExitCodeThread",
    "CreatePipe",
    "PeekNamedPipe",
    "SetNamedPipeHandleState",
    "ConnectNamedPipe",
    "WaitNamedPipeW",
    "GetNamedPipeHandleStateW",
    "DeviceIoControl",
    "CancelIoEx",
    "CancelIo",
    "GetOverlappedResult",
    "CreateHardLinkW",
    "CreateSymbolicLinkW",
    "PssCaptureSnapshot",
    "PssQuerySnapshot",
    "PssFreeSnapshot",
    "InitializeProcThreadAttributeList",
    "UpdateProcThreadAttribute",
    "DeleteProcThreadAttributeList",
    "GetNumaNodeProcessorMask",
    "CreateWaitableTimerExW",
    "CreateWaitableTimerW",
    "SetWaitableTimerEx",
    "SetWaitableTimer",
    "OpenFileMappingW",
    "OpenEventW",
    "OpenMutexW",
    "OpenSemaphoreW",
    "ReadProcessMemory",
    "WriteProcessMemory",
    "Module32FirstW",
    "Module32NextW",
    "Process32FirstW",
    "Process32NextW",
    "GenerateConsoleCtrlEvent",
    "FindNextVolumeW",
    "FindVolumeClose",
    "GetVolumePathNamesForVolumeNameW",
    "K32GetProcessMemoryInfo",
];

/// Devuelven INVALID_HANDLE_VALUE.
const NOT_SUPPORTED_HANDLE: [&str; 4] = [
    "CreateToolhelp32Snapshot",
    "FindFirstVolumeW",
    "CreateNamedPipeW",
    "CreateNamedPipeA",
];

unsafe extern "win64" fn not_supported() -> u64 {
    state::set_last_error(error::NOT_SUPPORTED);
    0
}

unsafe extern "win64" fn not_supported_handle() -> u64 {
    state::set_last_error(error::NOT_SUPPORTED);
    state::INVALID_HANDLE
}

// --- Sistema ------------------------------------------------------------------------------------

unsafe extern "win64" fn GetCurrentThreadStackLimits(low: *mut u64, high: *mut u64) {
    let teb = state::TEB.load(std::sync::atomic::Ordering::Relaxed);
    if teb == 0 {
        *low = 0;
        *high = 0;
        return;
    }
    let t = teb as *const u64;
    *high = *t.add(1); // StackBase
    *low = *t.add(2); // StackLimit
}

unsafe extern "win64" fn GetActiveProcessorCount(_group: u16) -> u32 {
    1
}
unsafe extern "win64" fn GetActiveProcessorGroupCount() -> u16 {
    1
}
unsafe extern "win64" fn GetMaximumProcessorCount(_group: u16) -> u32 {
    1
}
unsafe extern "win64" fn GetCurrentProcessorNumber() -> u32 {
    0
}
unsafe extern "win64" fn FlushInstructionCache(_p: u64, _a: u64, _n: usize) -> Bool {
    // x86-64 mantiene la caché de instrucciones coherente sola.
    1
}

/// Las rutas "largas" y "cortas" (8.3) son la misma: FAT32 de JARVIS-OS solo tiene nombres largos.
unsafe extern "win64" fn GetLongPathNameW(src: *const u16, dst: *mut u16, n: u32) -> u32 {
    let p = wstr(src);
    if std::fs::metadata(win::to_unix(&p)).is_err() {
        state::set_last_error(error::FILE_NOT_FOUND);
        return 0;
    }
    win::put_wide(dst, n, &p)
}
unsafe extern "win64" fn GetShortPathNameW(src: *const u16, dst: *mut u16, n: u32) -> u32 {
    GetLongPathNameW(src, dst, n)
}
unsafe extern "win64" fn NeedCurrentDirectoryForExePathW(_name: *const u16) -> Bool {
    1
}

/// Tiempos de un proceso o hilo: creación, salida, kernel y usuario (FILETIME). Se cuenta todo
/// como tiempo de usuario desde que arrancó `vino`.
unsafe fn times(c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> Bool {
    let start = kernel32::start();
    let now = win::filetime_now();
    let elapsed = (start.elapsed().as_nanos() / 100) as u64;
    *c = now - elapsed;
    *e = 0;
    *k = 0;
    *u = elapsed;
    1
}
unsafe extern "win64" fn GetProcessTimes(
    _h: u64,
    c: *mut u64,
    e: *mut u64,
    k: *mut u64,
    u: *mut u64,
) -> Bool {
    times(c, e, k, u)
}
unsafe extern "win64" fn GetThreadTimes(
    _h: u64,
    c: *mut u64,
    e: *mut u64,
    k: *mut u64,
    u: *mut u64,
) -> Bool {
    times(c, e, k, u)
}
unsafe extern "win64" fn GetSystemTimes(idle: *mut u64, k: *mut u64, u: *mut u64) -> Bool {
    for p in [idle, k, u] {
        if !p.is_null() {
            *p = 0;
        }
    }
    1
}

/// Una sola unidad, `C:`, fija (DRIVE_FIXED = 3).
unsafe extern "win64" fn GetDriveTypeW(_root: *const u16) -> u32 {
    3
}
unsafe extern "win64" fn GetLogicalDrives() -> u32 {
    1 << 2
}
unsafe extern "win64" fn GetLogicalDriveStringsW(n: u32, buf: *mut u16) -> u32 {
    let w = [b'C' as u16, b':' as u16, b'\\' as u16, 0, 0];
    if (n as usize) < w.len() {
        return w.len() as u32;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, w.len());
    4
}
unsafe extern "win64" fn GetVolumePathNameW(_p: *const u16, buf: *mut u16, n: u32) -> Bool {
    Bool::from(win::put_wide(buf, n, r"C:\") < n)
}
unsafe extern "win64" fn GetVolumeInformationW(
    _root: *const u16,
    name: *mut u16,
    nlen: u32,
    serial: *mut u32,
    maxc: *mut u32,
    flags: *mut u32,
    fs: *mut u16,
    fslen: u32,
) -> Bool {
    if !name.is_null() {
        win::put_wide(name, nlen, "JARVIS");
    }
    if !serial.is_null() {
        *serial = 0x4A41_5256;
    }
    if !maxc.is_null() {
        *maxc = 255;
    }
    if !flags.is_null() {
        *flags = 0x2; // FILE_CASE_PRESERVED_NAMES
    }
    if !fs.is_null() {
        win::put_wide(fs, fslen, "FAT32");
    }
    1
}
/// Lo que se sabe del disco: lo pide `shutil.disk_usage` y los instaladores. Sin acceso al FAT32
/// desde acá, se dice "no se sabe" (como un disco de red sin cuota).
unsafe extern "win64" fn GetDiskFreeSpaceExW(
    _p: *const u16,
    a: *mut u64,
    t: *mut u64,
    f: *mut u64,
) -> Bool {
    for p in [a, t, f] {
        if !p.is_null() {
            *p = 0;
        }
    }
    state::set_last_error(error::NOT_SUPPORTED);
    0
}

unsafe extern "win64" fn GetLocaleInfoA(_l: u32, _t: u32, _b: u64, _n: i32) -> i32 {
    state::set_last_error(error::INVALID_PARAMETER);
    0
}
unsafe extern "win64" fn CreateSemaphoreA(_sa: u64, _i: i32, _m: i32, _n: u64) -> u64 {
    state::set_last_error(0);
    state().insert(Object::Sync)
}
/// Carpetas extra donde buscar DLL: se aceptan (el cookie es el número de la carpeta).
unsafe extern "win64" fn AddDllDirectory(_dir: *const u16) -> u64 {
    1
}
unsafe extern "win64" fn RemoveDllDirectory(_cookie: u64) -> Bool {
    1
}
unsafe extern "win64" fn SetDefaultDllDirectories(_f: u32) -> Bool {
    1
}
unsafe extern "win64" fn SetDllDirectoryW(_d: *const u16) -> Bool {
    1
}
unsafe extern "win64" fn GetLargePageMinimum() -> usize {
    0
}
unsafe extern "win64" fn GetNumaHighestNodeNumber(n: *mut u32) -> Bool {
    *n = 0;
    1
}
unsafe extern "win64" fn SetThreadUILanguage(lang: u16) -> u16 {
    if lang == 0 { 0x2c0a } else { lang }
}
unsafe extern "win64" fn GetThreadLocale() -> u32 {
    0x2c0a
}
unsafe extern "win64" fn SetThreadLocale(_l: u32) -> Bool {
    1
}
unsafe extern "win64" fn CompareStringA(
    _l: u32,
    flags: u32,
    a: *const u8,
    na: i32,
    b: *const u8,
    nb: i32,
) -> i32 {
    let s = |p: *const u8, n: i32| {
        let b = if n < 0 {
            win::cstr(p)
        } else {
            std::slice::from_raw_parts(p, n as usize)
        };
        String::from_utf8_lossy(b).into_owned()
    };
    let (x, y) = (s(a, na), s(b, nb));
    let o = if flags & 1 != 0 {
        x.to_lowercase().cmp(&y.to_lowercase())
    } else {
        x.cmp(&y)
    };
    o as i32 + 2
}
/// `FindStringOrdinal`: dónde está `value` en `source` (o -1).
unsafe extern "win64" fn FindStringOrdinal(
    _flags: u32,
    src: *const u16,
    ns: i32,
    val: *const u16,
    nv: i32,
    ignore: Bool,
) -> i32 {
    let get = |p: *const u16, n: i32| {
        let len = if n < 0 {
            (0..).take_while(|&i| *p.add(i) != 0).count()
        } else {
            n as usize
        };
        win::from_wide(std::slice::from_raw_parts(p, len))
    };
    let (mut s, mut v) = (get(src, ns), get(val, nv));
    if ignore != 0 {
        s = s.to_lowercase();
        v = v.to_lowercase();
    }
    state::set_last_error(0);
    s.find(&v)
        .map_or(-1, |i| s[..i].encode_utf16().count() as i32)
}

/// MEMORYSTATUSEX: 64 bytes. Los números son los que ve un programa de Linux en JARVIS-OS
/// (los dice `sysinfo` del kernel: acá, una estimación fija y prudente de 1 GiB).
unsafe extern "win64" fn GlobalMemoryStatusEx(m: *mut u8) -> Bool {
    let total: u64 = 1 << 30;
    std::ptr::write_bytes(m.add(4), 0, 60);
    (m.add(4) as *mut u32).write_unaligned(50);
    for (o, v) in [
        (8, total),
        (16, total / 2),
        (24, total),
        (32, total / 2),
        (40, 1 << 47),
        (48, 1 << 46),
    ] {
        (m.add(o) as *mut u64).write_unaligned(v);
    }
    1
}
unsafe extern "win64" fn GetPhysicallyInstalledSystemMemory(kb: *mut u64) -> Bool {
    *kb = 1 << 20;
    1
}

// --- Rutas (api-ms-win-core-path) -----------------------------------------------------------

/// Cuánto de una ruta es la "raíz": `C:\`, `\\servidor\recurso\`, `\`.
pub fn root_len(p: &str) -> usize {
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return if b.get(2).is_some_and(|&c| c == b'\\' || c == b'/') {
            3
        } else {
            2
        };
    }
    if let Some(rest) = p.strip_prefix(r"\\") {
        // \\servidor\recurso\
        let mut parts = rest.splitn(3, ['\\', '/']);
        let a = parts.next().map_or(0, str::len);
        let b2 = parts.next().map_or(0, str::len);
        return (2 + a + 1 + b2 + 1).min(p.len());
    }
    usize::from(p.starts_with(['\\', '/']))
}

/// `PathCchCombine`: `más` relativa a `base`, sin `.` ni `..`, con `\`.
pub fn combine(base: &str, more: &str) -> String {
    let joined = if more.is_empty() {
        base.to_string()
    } else if root_len(more) > 0 || base.is_empty() {
        if more.starts_with(['\\', '/']) && !more.starts_with(r"\\") && root_len(base) >= 2 {
            // `\x` sobre `C:\a`: la raíz de esa unidad.
            format!("{}{}", &base[..2], more)
        } else {
            more.to_string()
        }
    } else if base.ends_with(['\\', '/']) {
        format!("{base}{more}")
    } else {
        format!("{base}\\{more}")
    };
    canonicalize(&joined)
}

/// Saca `.` y `..` (sin subir más arriba de la raíz) y usa `\`.
pub fn canonicalize(p: &str) -> String {
    let p = p.replace('/', "\\");
    let r = root_len(&p);
    let (root, rest) = p.split_at(r);
    let mut parts: Vec<&str> = Vec::new();
    for c in rest.split('\\') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = format!("{root}{}", parts.join("\\"));
    if p.ends_with('\\') && !out.ends_with('\\') && !parts.is_empty() {
        out.push('\\');
    }
    if out.is_empty() {
        out.push('\\');
    }
    out
}

/// HRESULT de "el buffer no alcanza" (ERROR_INSUFFICIENT_BUFFER como HRESULT).
const E_BUFFER: u32 = 0x8007_007A;

unsafe fn put_hr(dst: *mut u16, n: usize, s: &str) -> u32 {
    let w = win::to_wide_nul(s);
    if dst.is_null() || w.len() > n {
        if n > 0 && !dst.is_null() {
            *dst = 0;
        }
        return E_BUFFER;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), dst, w.len());
    0
}

unsafe extern "win64" fn PathCchSkipRoot(p: *const u16, out: *mut *const u16) -> u32 {
    let s = wstr(p);
    let r = root_len(&s);
    if r == 0 {
        return 0x8007_0057; // E_INVALIDARG
    }
    *out = p.add(s[..r].encode_utf16().count());
    0
}

unsafe extern "win64" fn PathCchCombineEx(
    out: *mut u16,
    n: usize,
    a: *const u16,
    b: *const u16,
    _f: u32,
) -> u32 {
    let r = combine(&wstr(a), &wstr(b));
    put_hr(out, n, &r)
}

unsafe extern "win64" fn PathCchCombine(
    out: *mut u16,
    n: usize,
    a: *const u16,
    b: *const u16,
) -> u32 {
    PathCchCombineEx(out, n, a, b, 0)
}

unsafe extern "win64" fn PathCchCanonicalizeEx(
    out: *mut u16,
    n: usize,
    p: *const u16,
    _f: u32,
) -> u32 {
    put_hr(out, n, &canonicalize(&wstr(p)))
}

unsafe extern "win64" fn PathCchAppendEx(
    path: *mut u16,
    n: usize,
    more: *const u16,
    _f: u32,
) -> u32 {
    let r = combine(&wstr(path), &wstr(more));
    put_hr(path, n, &r)
}

unsafe extern "win64" fn PathCchRemoveFileSpec(path: *mut u16, n: usize) -> u32 {
    let s = wstr(path);
    let r = root_len(&s);
    let cut = match s[r..].rfind('\\') {
        Some(i) => r + i,
        None => r,
    };
    if cut == s.len() {
        return 1; // S_FALSE: no había nada que sacar
    }
    put_hr(path, n, &s[..cut])
}

// --- Archivos mapeados en memoria -------------------------------------------------------------
// Un "mapping" es un HANDLE con el archivo (o nada: memoria compartida anónima) y un tamaño. Cada
// vista es memoria nuestra con una copia del archivo; al vaciarla o soltarla, lo escrito vuelve
// al archivo (si se pidió escribible). Sin memoria compartida entre procesos (hay uno solo).

struct View {
    file: Option<String>,
    offset: u64,
    len: usize,
    write: bool,
}

static VIEWS: Mutex<Option<HashMap<u64, View>>> = Mutex::new(None);
/// Un mapping: el archivo (o ninguno), su tamaño y si se puede escribir.
type Mapping = (Option<String>, u64, bool);
static MAPPINGS: Mutex<Option<HashMap<u64, Mapping>>> = Mutex::new(None);

unsafe fn create_mapping(file: u64, protect: u32, hi: u32, lo: u32) -> u64 {
    let size = ((hi as u64) << 32) | lo as u64;
    let path = match state().get(file) {
        Some(Object::File { path, .. }) => Some(path.clone()),
        _ => None,
    };
    if path.is_none() && file != state::INVALID_HANDLE {
        state::set_last_error(error::INVALID_HANDLE);
        return 0;
    }
    let size = if size == 0 {
        path.as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len())
    } else {
        size
    };
    let write = matches!(protect & 0xff, 0x04 | 0x40);
    let h = state().insert(Object::Sync);
    MAPPINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(h, (path, size, write));
    state::set_last_error(0);
    h
}

unsafe extern "win64" fn CreateFileMappingW(
    file: u64,
    _sa: u64,
    protect: u32,
    hi: u32,
    lo: u32,
    _name: *const u16,
) -> u64 {
    create_mapping(file, protect, hi, lo)
}
unsafe extern "win64" fn CreateFileMappingA(
    file: u64,
    _sa: u64,
    protect: u32,
    hi: u32,
    lo: u32,
    _name: *const u8,
) -> u64 {
    create_mapping(file, protect, hi, lo)
}

unsafe extern "win64" fn MapViewOfFile(map: u64, access: u32, hi: u32, lo: u32, n: usize) -> u64 {
    let Some((path, size, can_write)) = MAPPINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&map).cloned())
    else {
        state::set_last_error(error::INVALID_HANDLE);
        return 0;
    };
    let offset = ((hi as u64) << 32) | lo as u64;
    let len = if n == 0 {
        size.saturating_sub(offset) as usize
    } else {
        n
    };
    let Some(addr) = os::map(
        0,
        len.max(1),
        crate::pe::prot::READ | crate::pe::prot::WRITE,
        false,
    ) else {
        state::set_last_error(error::NOT_ENOUGH_MEMORY);
        return 0;
    };
    if let Some(p) = &path {
        use std::io::{Read, Seek, SeekFrom};
        if let Ok(mut f) = std::fs::File::open(p) {
            let _ = f.seek(SeekFrom::Start(offset));
            let mem = std::slice::from_raw_parts_mut(addr as *mut u8, len);
            let mut done = 0;
            while done < len {
                match f.read(&mut mem[done..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => done += k,
                }
            }
        }
    }
    let write = can_write && access & 0x2 != 0; // FILE_MAP_WRITE
    VIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(
            addr,
            View {
                file: path,
                offset,
                len,
                write,
            },
        );
    addr
}

unsafe extern "win64" fn MapViewOfFileEx(
    map: u64,
    access: u32,
    hi: u32,
    lo: u32,
    n: usize,
    _base: u64,
) -> u64 {
    MapViewOfFile(map, access, hi, lo, n)
}

/// Lo escrito en una vista vuelve a su archivo.
unsafe fn write_back(addr: u64, v: &View) {
    if let (true, Some(p)) = (v.write, &v.file) {
        use std::io::{Seek, SeekFrom, Write};
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(p) {
            let _ = f.seek(SeekFrom::Start(v.offset));
            let _ = f.write_all(std::slice::from_raw_parts(addr as *const u8, v.len));
        }
    }
}

unsafe extern "win64" fn FlushViewOfFile(addr: u64, _n: usize) -> Bool {
    if let Some(v) = VIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|m| m.get(&addr))
    {
        write_back(addr, v);
    }
    1
}

unsafe extern "win64" fn UnmapViewOfFile(addr: u64) -> Bool {
    let v = VIEWS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .and_then(|m| m.remove(&addr));
    match v {
        Some(v) => {
            write_back(addr, &v);
            os::unmap(addr, v.len.max(1));
            1
        }
        None => {
            state::set_last_error(error::INVALID_PARAMETER);
            0
        }
    }
}

// --- Varias ---------------------------------------------------------------------------------------

unsafe extern "win64" fn CopyFile2(from: *const u16, to: *const u16, _params: u64) -> u32 {
    match std::fs::copy(win::to_unix(&wstr(from)), win::to_unix(&wstr(to))) {
        Ok(_) => 0,
        Err(e) => 0x8007_0000 | win::error_code(&e),
    }
}
unsafe extern "win64" fn GetFileInformationByHandleEx2() -> Bool {
    state::set_last_error(error::NOT_SUPPORTED);
    0
}
unsafe extern "win64" fn SetThreadPriority(_h: u64, _p: i32) -> Bool {
    1
}
unsafe extern "win64" fn GetThreadPriority(_h: u64) -> i32 {
    0
}
unsafe extern "win64" fn SetPriorityClass(_h: u64, _c: u32) -> Bool {
    1
}
unsafe extern "win64" fn GetPriorityClass(_h: u64) -> u32 {
    0x20 // NORMAL_PRIORITY_CLASS
}
unsafe extern "win64" fn GetProcessIoCounters(_h: u64, c: *mut u8) -> Bool {
    std::ptr::write_bytes(c, 0, 48);
    1
}
unsafe extern "win64" fn QueryUnbiasedInterruptTime(t: *mut u64) -> Bool {
    *t = (kernel32::start().elapsed().as_nanos() / 100) as u64;
    1
}
unsafe extern "win64" fn GetSystemFirmwareTable(_p: u32, _id: u32, _b: u64, _n: u32) -> u32 {
    state::set_last_error(error::NOT_SUPPORTED);
    0
}
unsafe extern "win64" fn GetComputerNameExW(_t: u32, buf: *mut u16, len: *mut u32) -> Bool {
    let r = win::put_wide(buf, *len, "JARVIS");
    let ok = r < *len;
    *len = r;
    if !ok {
        state::set_last_error(0xEA); // ERROR_MORE_DATA
    }
    Bool::from(ok)
}
unsafe extern "win64" fn IsValidLocaleName(_n: *const u16) -> Bool {
    1
}
unsafe extern "win64" fn GetConsoleTitleW(buf: *mut u16, n: u32) -> u32 {
    win::put_wide(buf, n, "Terminal")
}
/// Sin hilos que avisar, no hay nada que apagar.
unsafe extern "win64" fn DisableThreadLibraryCalls(_h: u64) -> Bool {
    1
}
unsafe extern "win64" fn WTSGetActiveConsoleSessionId() -> u32 {
    1
}
unsafe extern "win64" fn ProcessIdToSessionId(_pid: u32, s: *mut u32) -> Bool {
    *s = 1;
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rutas_como_pathcch() {
        assert_eq!(root_len(r"C:\Python\Lib"), 3);
        assert_eq!(root_len(r"\\srv\recurso\x"), 14);
        assert_eq!(root_len(r"\x"), 1);
        assert_eq!(root_len("rel"), 0);
        assert_eq!(combine(r"C:\Python", "Lib"), r"C:\Python\Lib");
        assert_eq!(combine(r"C:\Python\", r"..\Lib\.\os.py"), r"C:\Lib\os.py");
        assert_eq!(combine(r"C:\a", r"D:\b"), r"D:\b");
        assert_eq!(combine(r"C:\a\b", r"\x"), r"C:\x");
        assert_eq!(canonicalize(r"C:\..\.."), r"C:\");
        assert_eq!(canonicalize(r"C:\a\b\..\"), r"C:\a\");
    }
}
