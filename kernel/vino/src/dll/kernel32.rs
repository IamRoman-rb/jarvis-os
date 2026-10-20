//! `kernel32.dll`: lo básico de Windows sobre la biblioteca estándar de Rust (es decir, sobre las
//! llamadas de Linux de JARVIS-OS).
//!
//! Las funciones tienen el nombre de Windows (por eso `non_snake_case`) y reciben punteros que
//! pasa el programa: si son inválidos, el programa termina con una violación de segmento, igual
//! que en Windows. Por eso cada una es `unsafe` y no se repite un `// SAFETY:` por línea: el
//! contrato es siempre "los punteros son los que documenta la API de Windows".
//!
//! Decisiones que se notan:
//! - El texto "ANSI" es UTF-8 (`GetACP` = 65001, como Windows con "UTF-8 en todo el sistema"):
//!   JARVIS-OS es UTF-8 y así no hay que traducir.
//! - La consola no "es una consola" (`GetConsoleMode` falla): los programas escriben con
//!   `WriteFile`, como si la salida fuera una tubería, y `vino` saca los `\r` de los `\r\n`.
//! - Un solo hilo: las esperas y los locks vuelven enseguida.

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Instant, UNIX_EPOCH};

use super::{dll_of, dll_of_handle, file_name, find, module_handle};
use crate::modules;
use crate::os;
use crate::pe::Symbol;
use crate::state::{
    self, CURRENT_PROCESS, CURRENT_THREAD, FindItem, INVALID_HANDLE, Object, PROCESS_HEAP, state,
};
use crate::win::{self, error, wstr};

type Bool = i32;
const TRUE: Bool = 1;
const FALSE: Bool = 0;

fn fail(e: u32) -> Bool {
    state::set_last_error(e);
    FALSE
}

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    exports![
        // Consola y E/S estándar
        GetStdHandle,
        SetStdHandle,
        GetConsoleMode,
        SetConsoleMode,
        WriteConsoleA,
        WriteConsoleW,
        ReadConsoleA,
        ReadConsoleW,
        GetConsoleCP,
        GetConsoleOutputCP,
        SetConsoleCP,
        SetConsoleOutputCP,
        SetConsoleCtrlHandler,
        SetConsoleTitleW,
        SetConsoleTitleA,
        SetConsoleTextAttribute,
        GetConsoleScreenBufferInfo,
        GetConsoleWindow,
        FlushConsoleInputBuffer,
        GetNumberOfConsoleInputEvents,
        AllocConsole,
        FreeConsole,
        AttachConsole,
        // Archivos
        CreateFileW,
        CreateFileA,
        CloseHandle,
        ReadFile,
        WriteFile,
        FlushFileBuffers,
        GetFileType,
        GetFileSizeEx,
        GetFileSize,
        SetFilePointerEx,
        SetFilePointer,
        SetEndOfFile,
        GetFileInformationByHandle,
        GetFileInformationByHandleEx,
        SetFileInformationByHandle,
        SetFileTime,
        GetFileTime,
        GetFileAttributesW,
        GetFileAttributesA,
        GetFileAttributesExW,
        SetFileAttributesW,
        DeleteFileW,
        DeleteFileA,
        CreateDirectoryW,
        CreateDirectoryA,
        RemoveDirectoryW,
        RemoveDirectoryA,
        MoveFileExW,
        MoveFileW,
        CopyFileW,
        FindFirstFileExW,
        FindFirstFileW,
        FindFirstFileA,
        FindNextFileW,
        FindNextFileA,
        FindClose,
        GetCurrentDirectoryW,
        GetCurrentDirectoryA,
        SetCurrentDirectoryW,
        SetCurrentDirectoryA,
        GetFullPathNameW,
        GetFullPathNameA,
        GetTempPathW,
        GetTempPathA,
        GetFinalPathNameByHandleW,
        DuplicateHandle,
        GetHandleInformation,
        SetHandleInformation,
        // Memoria
        GetProcessHeap,
        HeapCreate,
        HeapDestroy,
        HeapAlloc,
        HeapFree,
        HeapReAlloc,
        HeapSize,
        HeapValidate,
        HeapSetInformation,
        LocalAlloc,
        LocalFree,
        GlobalAlloc,
        GlobalFree,
        VirtualAlloc,
        VirtualFree,
        VirtualProtect,
        VirtualQuery,
        // Procesos, hilos y módulos
        ExitProcess,
        TerminateProcess,
        GetCurrentProcess,
        GetCurrentProcessId,
        GetCurrentThread,
        GetCommandLineW,
        GetCommandLineA,
        GetModuleHandleW,
        GetModuleHandleA,
        GetModuleHandleExW,
        GetModuleFileNameW,
        GetModuleFileNameA,
        LoadLibraryA,
        LoadLibraryW,
        LoadLibraryExW,
        LoadLibraryExA,
        FreeLibrary,
        GetProcAddress,
        GetExitCodeProcess,
        GetStartupInfoW,
        GetStartupInfoA,
        IsDebuggerPresent,
        IsProcessorFeaturePresent,
        IsThreadAFiber,
        SetThreadStackGuarantee,
        SetThreadDescription,
        GetProcessAffinityMask,
        GetSystemInfo,
        GetNativeSystemInfo,
        GetVersion,
        GetVersionExW,
        GetVersionExA,
        GetComputerNameW,
        GetComputerNameA,
        GetSystemDirectoryW,
        GetWindowsDirectoryW,
        IsWow64Process,
        GetErrorMode,
        SetErrorMode,
        // Entorno
        GetEnvironmentVariableW,
        GetEnvironmentVariableA,
        SetEnvironmentVariableW,
        SetEnvironmentVariableA,
        GetEnvironmentStringsW,
        GetEnvironmentStrings,
        FreeEnvironmentStringsW,
        FreeEnvironmentStringsA,
        ExpandEnvironmentStringsW,
        // Hora
        GetSystemTimeAsFileTime,
        GetSystemTimePreciseAsFileTime,
        GetSystemTime,
        GetLocalTime,
        FileTimeToSystemTime,
        SystemTimeToFileTime,
        FileTimeToLocalFileTime,
        GetTimeZoneInformation,
        QueryPerformanceCounter,
        QueryPerformanceFrequency,
        GetTickCount,
        GetTickCount64,
        // TLS y FLS
        TlsAlloc,
        TlsFree,
        TlsGetValue,
        TlsSetValue,
        // Sincronización (un solo hilo)
        InitializeSListHead,
        // Texto e idioma
        lstrlenW,
        lstrlenA,
        MultiByteToWideChar,
        WideCharToMultiByte,
        GetACP,
        GetOEMCP,
        IsValidCodePage,
        GetCPInfo,
        LCMapStringW,
        LCMapStringEx,
        GetStringTypeW,
        CompareStringW,
        CompareStringEx,
        CompareStringOrdinal,
        GetUserDefaultLCID,
        GetUserDefaultLocaleName,
        GetLocaleInfoW,
        GetLocaleInfoEx,
        IsDBCSLeadByte,
        GetUserDefaultUILanguage,
        GetSystemDefaultLangID,
        // Errores y excepciones
        GetLastError,
        SetLastError,
        FormatMessageW,
        FormatMessageA,
        OutputDebugStringA,
        OutputDebugStringW,
        RaiseException,
        SetUnhandledExceptionFilter,
        UnhandledExceptionFilter,
        AddVectoredExceptionHandler,
        RemoveVectoredExceptionHandler,
        EncodePointer,
        DecodePointer,
        Beep,
    ]
}

// --- Leer y escribir un HANDLE (también lo usan ntdll y el CRT) -------------------------------

/// Lee de un HANDLE. 0 bytes = fin del archivo (o Ctrl+D en la Terminal).
pub fn read_handle(h: u64, buf: &mut [u8]) -> Result<usize, u32> {
    match std_of(h)? {
        // La Terminal se lee sin el lock (puede tardar: espera a que Roman escriba).
        Some(0) => io::stdin()
            .lock()
            .read(buf)
            .map_err(|e| win::error_code(&e)),
        Some(_) => Err(error::ACCESS_DENIED),
        None => with_file(h, |f| f.read(buf)),
    }
}

/// Escribe en un HANDLE. A la Terminal, sin los `\r` de los fines de línea de Windows.
pub fn write_handle(h: u64, data: &[u8]) -> Result<usize, u32> {
    match std_of(h)? {
        Some(n @ (1 | 2)) => {
            write_console(n, data).map_err(|e| win::error_code(&e))?;
            Ok(data.len())
        }
        Some(_) => Err(error::ACCESS_DENIED),
        None => with_file(h, |f| f.write(data)),
    }
}

/// ¿Es uno de los estándar (cuál) o un archivo (`None`)?
pub fn std_of(h: u64) -> Result<Option<u8>, u32> {
    match state().get(h) {
        Some(Object::Std(n)) => Ok(Some(*n)),
        Some(Object::File { .. }) => Ok(None),
        _ => Err(error::INVALID_HANDLE),
    }
}

/// `\r\n` → `\n` (un `\r` suelto se deja).
pub fn strip_cr(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (i, &c) in data.iter().enumerate() {
        if c == b'\r' && data.get(i + 1) == Some(&b'\n') {
            continue;
        }
        out.push(c);
    }
    out
}

pub fn write_console(n: u8, data: &[u8]) -> io::Result<()> {
    let data = strip_cr(data);
    if n == 2 {
        let mut e = io::stderr().lock();
        e.write_all(&data)?;
        e.flush()
    } else {
        let mut o = io::stdout().lock();
        o.write_all(&data)?;
        // La Terminal muestra lo que llega: sin esperar al fin de línea (un "¿Cómo te llamás? ").
        o.flush()
    }
}

// --- Consola ----------------------------------------------------------------------------------

unsafe extern "win64" fn GetStdHandle(n: u32) -> u64 {
    let s = state();
    match n as i32 {
        -10 => s.std_handles[0],
        -11 => s.std_handles[1],
        -12 => s.std_handles[2],
        _ => {
            drop(s);
            state::set_last_error(error::INVALID_HANDLE);
            INVALID_HANDLE
        }
    }
}

unsafe extern "win64" fn SetStdHandle(n: u32, h: u64) -> Bool {
    let i = match n as i32 {
        -10 => 0,
        -11 => 1,
        -12 => 2,
        _ => return fail(error::INVALID_HANDLE),
    };
    state().std_handles[i] = h;
    TRUE
}

unsafe extern "win64" fn GetConsoleMode(_h: u64, _mode: *mut u32) -> Bool {
    fail(error::INVALID_HANDLE)
}

unsafe extern "win64" fn SetConsoleMode(_h: u64, _mode: u32) -> Bool {
    fail(error::INVALID_HANDLE)
}

unsafe extern "win64" fn WriteConsoleA(
    h: u64,
    buf: *const u8,
    n: u32,
    done: *mut u32,
    _r: u64,
) -> Bool {
    let data = std::slice::from_raw_parts(buf, n as usize);
    match write_handle(h, data) {
        Ok(k) => {
            if !done.is_null() {
                *done = k as u32;
            }
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn WriteConsoleW(
    h: u64,
    buf: *const u16,
    n: u32,
    done: *mut u32,
    _r: u64,
) -> Bool {
    let text = win::from_wide(std::slice::from_raw_parts(buf, n as usize));
    match write_handle(h, text.as_bytes()) {
        Ok(_) => {
            if !done.is_null() {
                *done = n;
            }
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn ReadConsoleA(
    h: u64,
    buf: *mut u8,
    n: u32,
    done: *mut u32,
    _c: u64,
) -> Bool {
    ReadFile(h, buf, n, done, 0)
}

unsafe extern "win64" fn ReadConsoleW(
    h: u64,
    buf: *mut u16,
    n: u32,
    done: *mut u32,
    _c: u64,
) -> Bool {
    // Una línea (en UTF-8) → UTF-16. Si no entra, se pierde el resto (los programas piden de a
    // muchos caracteres).
    let mut bytes = vec![0u8; n as usize];
    let k = match read_handle(h, &mut bytes) {
        Ok(k) => k,
        Err(e) => return fail(e),
    };
    let w = win::to_wide(&String::from_utf8_lossy(&bytes[..k]));
    let m = w.len().min(n as usize);
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, m);
    if !done.is_null() {
        *done = m as u32;
    }
    TRUE
}

unsafe extern "win64" fn GetConsoleCP() -> u32 {
    65001
}
unsafe extern "win64" fn GetConsoleOutputCP() -> u32 {
    65001
}
unsafe extern "win64" fn SetConsoleCP(_cp: u32) -> Bool {
    TRUE
}
unsafe extern "win64" fn SetConsoleOutputCP(_cp: u32) -> Bool {
    TRUE
}
unsafe extern "win64" fn SetConsoleCtrlHandler(_f: u64, _add: Bool) -> Bool {
    TRUE
}
unsafe extern "win64" fn SetConsoleTitleW(_t: *const u16) -> Bool {
    TRUE
}
unsafe extern "win64" fn SetConsoleTitleA(_t: *const u8) -> Bool {
    TRUE
}
unsafe extern "win64" fn SetConsoleTextAttribute(_h: u64, _a: u16) -> Bool {
    TRUE
}
unsafe extern "win64" fn GetConsoleScreenBufferInfo(_h: u64, _i: *mut u8) -> Bool {
    fail(error::INVALID_HANDLE)
}
unsafe extern "win64" fn GetConsoleWindow() -> u64 {
    0
}
unsafe extern "win64" fn FlushConsoleInputBuffer(_h: u64) -> Bool {
    TRUE
}
unsafe extern "win64" fn GetNumberOfConsoleInputEvents(_h: u64, n: *mut u32) -> Bool {
    if !n.is_null() {
        *n = 0;
    }
    TRUE
}
unsafe extern "win64" fn AllocConsole() -> Bool {
    TRUE
}
unsafe extern "win64" fn FreeConsole() -> Bool {
    TRUE
}
unsafe extern "win64" fn AttachConsole(_pid: u32) -> Bool {
    TRUE
}

// --- Archivos ---------------------------------------------------------------------------------

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const FILE_READ_DATA: u32 = 1;
const FILE_WRITE_DATA: u32 = 2;
const FILE_APPEND_DATA: u32 = 4;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;

/// Abre una ruta de JARVIS-OS como lo pide `CreateFile`.
pub fn create_file(path: &str, access: u32, disposition: u32, flags: u32) -> Result<u64, u32> {
    match path.to_ascii_lowercase().as_str() {
        "conout$" | "con" => return Ok(state().std_handles[1]),
        "conin$" => return Ok(state().std_handles[0]),
        _ => {}
    }
    let unix = win::to_unix(path);
    let meta = fs::metadata(&unix);
    let existed = meta.is_ok();
    if meta.as_ref().is_ok_and(|m| m.is_dir()) {
        if flags & FILE_FLAG_BACKUP_SEMANTICS == 0 {
            return Err(error::ACCESS_DENIED);
        }
        if disposition == 1 {
            return Err(error::FILE_EXISTS);
        }
        state::set_last_error(0);
        return Ok(state().insert(Object::Dir(unix.into())));
    }
    let read = access & (GENERIC_READ | GENERIC_ALL | FILE_READ_DATA) != 0;
    let write = access & (GENERIC_WRITE | GENERIC_ALL | FILE_WRITE_DATA) != 0;
    let append = !write && access & FILE_APPEND_DATA != 0;
    let mut o = OpenOptions::new();
    // Un archivo abierto "sin permisos" (solo para pedir sus datos) se abre para leer.
    o.read(read || !(write || append));
    o.write(write);
    o.append(append);
    match disposition {
        1 => {
            o.create_new(true);
        }
        2 => {
            o.create(true).truncate(true);
        }
        3 => {}
        4 => {
            o.create(true);
        }
        5 => {
            o.truncate(true);
        }
        _ => return Err(error::INVALID_PARAMETER),
    }
    if matches!(disposition, 1 | 2 | 4) && !(write || append) {
        // Crear algo exige escribir: Rust no deja "crear" con un archivo solo de lectura.
        o.write(true);
    }
    let file = o.open(&unix).map_err(|e| match e.kind() {
        io::ErrorKind::AlreadyExists => error::FILE_EXISTS,
        io::ErrorKind::NotFound
            if !Path::new(&unix)
                .parent()
                .is_none_or(|p| p.as_os_str().is_empty() || p.exists()) =>
        {
            error::PATH_NOT_FOUND
        }
        _ => win::error_code(&e),
    })?;
    // CREATE_ALWAYS y OPEN_ALWAYS avisan si el archivo ya estaba.
    state::set_last_error(if existed && matches!(disposition, 2 | 4) {
        error::ALREADY_EXISTS
    } else {
        0
    });
    Ok(state().insert(Object::File { file, path: unix }))
}

unsafe extern "win64" fn CreateFileW(
    name: *const u16,
    access: u32,
    _share: u32,
    _sa: u64,
    disposition: u32,
    flags: u32,
    _template: u64,
) -> u64 {
    match create_file(&wstr(name), access, disposition, flags) {
        Ok(h) => h,
        Err(e) => {
            state::set_last_error(e);
            INVALID_HANDLE
        }
    }
}

unsafe extern "win64" fn CreateFileA(
    name: *const u8,
    access: u32,
    share: u32,
    sa: u64,
    disposition: u32,
    flags: u32,
    template: u64,
) -> u64 {
    let w = win::to_wide_nul(&String::from_utf8_lossy(win::cstr(name)));
    CreateFileW(w.as_ptr(), access, share, sa, disposition, flags, template)
}

unsafe extern "win64" fn CloseHandle(h: u64) -> Bool {
    super::sync::forget(h);
    if h == CURRENT_PROCESS || h == CURRENT_THREAD || state().close(h) {
        TRUE
    } else {
        fail(error::INVALID_HANDLE)
    }
}

unsafe extern "win64" fn ReadFile(h: u64, buf: *mut u8, n: u32, done: *mut u32, _ovl: u64) -> Bool {
    let b = std::slice::from_raw_parts_mut(buf, n as usize);
    match read_handle(h, b) {
        Ok(k) => {
            if !done.is_null() {
                *done = k as u32;
            }
            TRUE
        }
        Err(e) => {
            if !done.is_null() {
                *done = 0;
            }
            fail(e)
        }
    }
}

unsafe extern "win64" fn WriteFile(
    h: u64,
    buf: *const u8,
    n: u32,
    done: *mut u32,
    _ovl: u64,
) -> Bool {
    let b = std::slice::from_raw_parts(buf, n as usize);
    match write_handle(h, b) {
        Ok(k) => {
            if !done.is_null() {
                *done = k as u32;
            }
            TRUE
        }
        Err(e) => fail(e),
    }
}

/// Lo que hace una función sobre un archivo abierto (o el error de Windows).
pub fn with_file<R>(h: u64, f: impl FnOnce(&mut File) -> io::Result<R>) -> Result<R, u32> {
    match state().get(h) {
        Some(Object::File { file, .. }) => f(file).map_err(|e| win::error_code(&e)),
        _ => Err(error::INVALID_HANDLE),
    }
}

unsafe extern "win64" fn FlushFileBuffers(h: u64) -> Bool {
    if matches!(state().get(h), Some(Object::Std(_))) {
        return TRUE;
    }
    match with_file(h, |f| f.flush()) {
        Ok(()) => TRUE,
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn GetFileType(h: u64) -> u32 {
    match state().get(h) {
        Some(Object::Std(_)) => 2, // FILE_TYPE_CHAR: una terminal
        Some(Object::File { .. } | Object::Dir(_)) => 1, // FILE_TYPE_DISK
        _ => {
            state::set_last_error(error::INVALID_HANDLE);
            0
        }
    }
}

unsafe extern "win64" fn GetFileSizeEx(h: u64, size: *mut i64) -> Bool {
    match with_file(h, |f| f.metadata()) {
        Ok(m) => {
            *size = m.len() as i64;
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn GetFileSize(h: u64, high: *mut u32) -> u32 {
    match with_file(h, |f| f.metadata()) {
        Ok(m) => {
            if !high.is_null() {
                *high = (m.len() >> 32) as u32;
            }
            m.len() as u32
        }
        Err(e) => {
            state::set_last_error(e);
            u32::MAX
        }
    }
}

pub fn seek(h: u64, dist: i64, method: u32) -> Result<u64, u32> {
    let pos = match method {
        0 => SeekFrom::Start(dist.max(0) as u64),
        1 => SeekFrom::Current(dist),
        2 => SeekFrom::End(dist),
        _ => return Err(error::INVALID_PARAMETER),
    };
    with_file(h, |f| f.seek(pos))
}

unsafe extern "win64" fn SetFilePointerEx(h: u64, dist: i64, new: *mut i64, method: u32) -> Bool {
    match seek(h, dist, method) {
        Ok(p) => {
            if !new.is_null() {
                *new = p as i64;
            }
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn SetFilePointer(h: u64, low: i32, high: *mut i32, method: u32) -> u32 {
    let dist = if high.is_null() {
        low as i64
    } else {
        ((*high as i64) << 32) | (low as u32 as i64)
    };
    match seek(h, dist, method) {
        Ok(p) => {
            if !high.is_null() {
                *high = (p >> 32) as i32;
            }
            state::set_last_error(0);
            p as u32
        }
        Err(e) => {
            state::set_last_error(e);
            u32::MAX
        }
    }
}

unsafe extern "win64" fn SetEndOfFile(h: u64) -> Bool {
    match with_file(h, |f| {
        let p = f.stream_position()?;
        f.set_len(p)
    }) {
        Ok(()) => TRUE,
        Err(e) => fail(e),
    }
}

/// Los datos de un archivo o carpeta abiertos: atributos, tamaño, fecha (FILETIME) y un número
/// que lo identifica.
pub fn handle_meta(h: u64) -> Result<(u32, u64, u64, u64), u32> {
    let (meta, path) = match state().get(h) {
        Some(Object::File { file, path }) => (file.metadata(), path.clone()),
        Some(Object::Dir(p)) => (fs::metadata(&*p), p.to_string_lossy().into_owned()),
        Some(Object::Std(_)) => return Ok((0, 0, win::filetime_now(), 0)),
        _ => return Err(error::INVALID_HANDLE),
    };
    let m = meta.map_err(|e| win::error_code(&e))?;
    let id = path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    });
    Ok((attributes(&m), m.len(), modified(&m), id))
}

fn attributes(m: &fs::Metadata) -> u32 {
    let mut a = if m.is_dir() {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_ARCHIVE
    };
    if m.permissions().readonly() {
        a |= FILE_ATTRIBUTE_READONLY;
    }
    a
}

fn modified(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| win::filetime(d.as_nanos()))
        .unwrap_or_else(win::filetime_now)
}

unsafe fn put32(p: *mut u8, o: usize, v: u32) {
    (p.add(o) as *mut u32).write_unaligned(v);
}

unsafe fn put64(p: *mut u8, o: usize, v: u64) {
    (p.add(o) as *mut u64).write_unaligned(v);
}

unsafe extern "win64" fn GetFileInformationByHandle(h: u64, info: *mut u8) -> Bool {
    let (attrs, size, time, id) = match handle_meta(h) {
        Ok(m) => m,
        Err(e) => return fail(e),
    };
    // BY_HANDLE_FILE_INFORMATION: 52 bytes.
    std::ptr::write_bytes(info, 0, 52);
    put32(info, 0, attrs);
    for o in [4, 12, 20] {
        put64(info, o, time);
    }
    put32(info, 28, 0x4A41_5256); // número de serie del volumen: "JARV"
    put32(info, 32, (size >> 32) as u32);
    put32(info, 36, size as u32);
    put32(info, 40, 1);
    put32(info, 44, (id >> 32) as u32);
    put32(info, 48, id as u32);
    TRUE
}

unsafe extern "win64" fn GetFileInformationByHandleEx(
    h: u64,
    class: u32,
    buf: *mut u8,
    size: u32,
) -> Bool {
    let (attrs, len, time, _) = match handle_meta(h) {
        Ok(m) => m,
        Err(e) => return fail(e),
    };
    match class {
        // FileBasicInfo: cuatro fechas y los atributos.
        0 if size >= 40 => {
            std::ptr::write_bytes(buf, 0, 40);
            for o in [0, 8, 16, 24] {
                put64(buf, o, time);
            }
            put32(buf, 32, attrs);
        }
        // FileStandardInfo: tamaño reservado, tamaño, enlaces, ¿se borra?, ¿es carpeta?
        1 if size >= 24 => {
            std::ptr::write_bytes(buf, 0, 24);
            put64(buf, 0, len.next_multiple_of(4096));
            put64(buf, 8, len);
            put32(buf, 16, 1);
            *buf.add(21) = u8::from(attrs & FILE_ATTRIBUTE_DIRECTORY != 0);
        }
        // FileAttributeTagInfo: atributos y "reparse tag" (no hay enlaces de Windows).
        9 if size >= 8 => {
            put32(buf, 0, attrs);
            put32(buf, 4, 0);
        }
        _ => return fail(error::INVALID_PARAMETER),
    }
    TRUE
}

unsafe extern "win64" fn SetFileInformationByHandle(
    h: u64,
    class: u32,
    buf: *const u8,
    size: u32,
) -> Bool {
    match class {
        // FileBasicInfo: las fechas (no se cambian: FAT32 de JARVIS-OS las pone solo).
        0 => TRUE,
        // FileAllocationInfo y FileEndOfFileInfo: el tamaño.
        5 | 6 if size >= 8 => {
            let len = (buf as *const i64).read_unaligned().max(0) as u64;
            match with_file(h, |f| {
                if class == 6 || f.metadata()?.len() > len {
                    f.set_len(len)?;
                }
                Ok(())
            }) {
                Ok(()) => TRUE,
                Err(e) => fail(e),
            }
        }
        _ => fail(error::INVALID_PARAMETER),
    }
}

unsafe extern "win64" fn SetFileTime(_h: u64, _c: u64, _a: u64, _w: u64) -> Bool {
    TRUE
}

unsafe extern "win64" fn GetFileTime(h: u64, c: *mut u64, a: *mut u64, w: *mut u64) -> Bool {
    match handle_meta(h) {
        Ok((_, _, t, _)) => {
            for p in [c, a, w] {
                if !p.is_null() {
                    *p = t;
                }
            }
            TRUE
        }
        Err(e) => fail(e),
    }
}

fn path_attributes(path: &str) -> Result<(u32, u64, u64), u32> {
    let m = fs::metadata(win::to_unix(path)).map_err(|e| win::error_code(&e))?;
    Ok((attributes(&m), m.len(), modified(&m)))
}

unsafe extern "win64" fn GetFileAttributesW(name: *const u16) -> u32 {
    match path_attributes(&wstr(name)) {
        Ok((a, _, _)) => a,
        Err(e) => {
            state::set_last_error(e);
            INVALID_FILE_ATTRIBUTES
        }
    }
}

unsafe extern "win64" fn GetFileAttributesA(name: *const u8) -> u32 {
    let w = win::to_wide_nul(&String::from_utf8_lossy(win::cstr(name)));
    GetFileAttributesW(w.as_ptr())
}

unsafe extern "win64" fn GetFileAttributesExW(
    name: *const u16,
    _level: u32,
    data: *mut u8,
) -> Bool {
    match path_attributes(&wstr(name)) {
        Ok((a, size, t)) => {
            // WIN32_FILE_ATTRIBUTE_DATA: 36 bytes.
            put32(data, 0, a);
            for o in [4, 12, 20] {
                put64(data, o, t);
            }
            put32(data, 28, (size >> 32) as u32);
            put32(data, 32, size as u32);
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn SetFileAttributesW(_name: *const u16, _a: u32) -> Bool {
    TRUE
}

fn result(r: io::Result<()>) -> Bool {
    match r {
        Ok(()) => TRUE,
        Err(e) => fail(win::error_code(&e)),
    }
}

/// `DeleteFile`: en JARVIS-OS borrar un archivo lo manda a la Papelera (lo hace el kernel con
/// `unlink`, regla 13 de CLAUDE.md).
unsafe extern "win64" fn DeleteFileW(name: *const u16) -> Bool {
    result(fs::remove_file(win::to_unix(&wstr(name))))
}

unsafe extern "win64" fn DeleteFileA(name: *const u8) -> Bool {
    result(fs::remove_file(win::to_unix(&String::from_utf8_lossy(
        win::cstr(name),
    ))))
}

unsafe extern "win64" fn CreateDirectoryW(name: *const u16, _sa: u64) -> Bool {
    result(fs::create_dir(win::to_unix(&wstr(name))))
}

unsafe extern "win64" fn CreateDirectoryA(name: *const u8, _sa: u64) -> Bool {
    result(fs::create_dir(win::to_unix(&String::from_utf8_lossy(
        win::cstr(name),
    ))))
}

unsafe extern "win64" fn RemoveDirectoryW(name: *const u16) -> Bool {
    result(fs::remove_dir(win::to_unix(&wstr(name))))
}

unsafe extern "win64" fn RemoveDirectoryA(name: *const u8) -> Bool {
    result(fs::remove_dir(win::to_unix(&String::from_utf8_lossy(
        win::cstr(name),
    ))))
}

unsafe extern "win64" fn MoveFileExW(from: *const u16, to: *const u16, _flags: u32) -> Bool {
    result(fs::rename(
        win::to_unix(&wstr(from)),
        win::to_unix(&wstr(to)),
    ))
}

unsafe extern "win64" fn MoveFileW(from: *const u16, to: *const u16) -> Bool {
    MoveFileExW(from, to, 0)
}

unsafe extern "win64" fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: Bool) -> Bool {
    let to = win::to_unix(&wstr(to));
    if fail_if_exists != 0 && Path::new(&to).exists() {
        return fail(error::FILE_EXISTS);
    }
    result(fs::copy(win::to_unix(&wstr(from)), to).map(|_| ()))
}

/// `*` y `?` como Windows (sin distinguir mayúsculas). `*.*` es "todo".
pub fn wildcard(pattern: &str, name: &str) -> bool {
    fn m(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => m(&p[1..], n) || (!n.is_empty() && m(p, &n[1..])),
            (Some('?'), Some(_)) => m(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a.to_lowercase().eq(b.to_lowercase()) && m(&p[1..], &n[1..]),
            _ => false,
        }
    }
    if pattern == "*.*" || pattern == "*" {
        return true;
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    m(&p, &n)
}

/// Lo que encuentra `FindFirstFile("carpeta\*.txt")`: `.` y `..` primero, como Windows.
pub fn find_items(pattern: &str) -> Result<Vec<FindItem>, u32> {
    let unix = win::to_unix(pattern);
    let (dir, pat) = match unix.rfind('/') {
        Some(i) => (&unix[..i.max(1)], &unix[i + 1..]),
        None => (".", unix.as_str()),
    };
    let dir = if dir.is_empty() { "/" } else { dir };
    let rd = fs::read_dir(dir).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => error::PATH_NOT_FOUND,
        _ => win::error_code(&e),
    })?;
    let now = win::filetime_now();
    let mut items = Vec::new();
    for special in [".", ".."] {
        if wildcard(pat, special) {
            items.push(FindItem {
                name: special.into(),
                dir: true,
                size: 0,
                modified: now,
            });
        }
    }
    let mut rest: Vec<FindItem> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let m = e.metadata().ok()?;
            wildcard(pat, &name).then(|| FindItem {
                name,
                dir: m.is_dir(),
                size: m.len(),
                modified: modified(&m),
            })
        })
        .collect();
    rest.sort_by_key(|a| a.name.to_lowercase());
    items.extend(rest);
    if items.is_empty() {
        return Err(error::FILE_NOT_FOUND);
    }
    Ok(items)
}

/// WIN32_FIND_DATAW: 592 bytes (atributos, tres fechas, tamaño, el nombre en UTF-16).
unsafe fn fill_find_w(d: *mut u8, it: &FindItem) {
    std::ptr::write_bytes(d, 0, 592);
    put32(
        d,
        0,
        if it.dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_ARCHIVE
        },
    );
    for o in [4, 12, 20] {
        put64(d, o, it.modified);
    }
    put32(d, 28, (it.size >> 32) as u32);
    put32(d, 32, it.size as u32);
    let w = win::to_wide(&it.name);
    let n = w.len().min(259);
    std::ptr::copy_nonoverlapping(w.as_ptr(), d.add(44) as *mut u16, n);
}

/// WIN32_FIND_DATAA: 320 bytes (el nombre en bytes, 260).
unsafe fn fill_find_a(d: *mut u8, it: &FindItem) {
    std::ptr::write_bytes(d, 0, 320);
    put32(
        d,
        0,
        if it.dir {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_ARCHIVE
        },
    );
    for o in [4, 12, 20] {
        put64(d, o, it.modified);
    }
    put32(d, 28, (it.size >> 32) as u32);
    put32(d, 32, it.size as u32);
    let n = it.name.len().min(259);
    std::ptr::copy_nonoverlapping(it.name.as_ptr(), d.add(44), n);
}

fn find_first(pattern: &str, fill: &dyn Fn(&FindItem)) -> u64 {
    match find_items(pattern) {
        Ok(items) => {
            fill(&items[0]);
            state::set_last_error(0);
            state().insert(Object::Find { items, pos: 1 })
        }
        Err(e) => {
            state::set_last_error(e);
            INVALID_HANDLE
        }
    }
}

unsafe extern "win64" fn FindFirstFileExW(
    name: *const u16,
    _level: u32,
    data: *mut u8,
    _op: u32,
    _filter: u64,
    _flags: u32,
) -> u64 {
    find_first(&wstr(name), &|it| fill_find_w(data, it))
}

unsafe extern "win64" fn FindFirstFileW(name: *const u16, data: *mut u8) -> u64 {
    find_first(&wstr(name), &|it| fill_find_w(data, it))
}

unsafe extern "win64" fn FindFirstFileA(name: *const u8, data: *mut u8) -> u64 {
    find_first(&String::from_utf8_lossy(win::cstr(name)), &|it| {
        fill_find_a(data, it)
    })
}

fn find_next(h: u64) -> Result<FindItem, u32> {
    match state().get(h) {
        Some(Object::Find { items, pos }) => {
            let it = items.get(*pos).cloned().ok_or(error::NO_MORE_FILES)?;
            *pos += 1;
            Ok(it)
        }
        _ => Err(error::INVALID_HANDLE),
    }
}

unsafe extern "win64" fn FindNextFileW(h: u64, data: *mut u8) -> Bool {
    match find_next(h) {
        Ok(it) => {
            fill_find_w(data, &it);
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn FindNextFileA(h: u64, data: *mut u8) -> Bool {
    match find_next(h) {
        Ok(it) => {
            fill_find_a(data, &it);
            TRUE
        }
        Err(e) => fail(e),
    }
}

unsafe extern "win64" fn FindClose(h: u64) -> Bool {
    if state().close(h) {
        TRUE
    } else {
        fail(error::INVALID_HANDLE)
    }
}

fn current_dir() -> String {
    let d = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".into());
    win::to_windows(&d)
}

unsafe extern "win64" fn GetCurrentDirectoryW(len: u32, buf: *mut u16) -> u32 {
    win::put_wide(buf, len, &current_dir())
}

unsafe extern "win64" fn GetCurrentDirectoryA(len: u32, buf: *mut u8) -> u32 {
    win::put_narrow(buf, len, &current_dir())
}

unsafe extern "win64" fn SetCurrentDirectoryW(name: *const u16) -> Bool {
    result(std::env::set_current_dir(win::to_unix(&wstr(name))))
}

unsafe extern "win64" fn SetCurrentDirectoryA(name: *const u8) -> Bool {
    result(std::env::set_current_dir(win::to_unix(
        &String::from_utf8_lossy(win::cstr(name)),
    )))
}

/// La ruta completa al estilo Windows (`C:\…`), sin `.` ni `..`.
pub fn full_path(name: &str) -> String {
    let unix = win::to_unix(name);
    let base = if unix.starts_with('/') {
        String::new()
    } else {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut parts: Vec<&str> = Vec::new();
    for c in base.split('/').chain(unix.split('/')) {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = format!(r"C:\{}", parts.join("\\"));
    if name.ends_with(['\\', '/']) && !out.ends_with('\\') {
        out.push('\\');
    }
    out
}

unsafe extern "win64" fn GetFullPathNameW(
    name: *const u16,
    len: u32,
    buf: *mut u16,
    file: *mut *mut u16,
) -> u32 {
    let full = full_path(&wstr(name));
    let r = win::put_wide(buf, len, &full);
    if !file.is_null() && r < len {
        *file = match full.rfind('\\') {
            Some(i) if i + 1 < full.len() => buf.add(full[..i + 1].encode_utf16().count()),
            _ => std::ptr::null_mut(),
        };
    }
    r
}

unsafe extern "win64" fn GetFullPathNameA(
    name: *const u8,
    len: u32,
    buf: *mut u8,
    file: *mut *mut u8,
) -> u32 {
    let full = full_path(&String::from_utf8_lossy(win::cstr(name)));
    let r = win::put_narrow(buf, len, &full);
    if !file.is_null() && r < len {
        *file = match full.rfind('\\') {
            Some(i) if i + 1 < full.len() => buf.add(i + 1),
            _ => std::ptr::null_mut(),
        };
    }
    r
}

/// La carpeta de temporales: `/tmp` si existe; si no, la carpeta actual.
fn temp_dir() -> String {
    let t = if Path::new("/tmp").is_dir() {
        r"C:\tmp\".to_string()
    } else {
        current_dir()
    };
    if t.ends_with('\\') {
        t
    } else {
        format!("{t}\\")
    }
}

unsafe extern "win64" fn GetTempPathW(len: u32, buf: *mut u16) -> u32 {
    win::put_wide(buf, len, &temp_dir())
}

unsafe extern "win64" fn GetTempPathA(len: u32, buf: *mut u8) -> u32 {
    win::put_narrow(buf, len, &temp_dir())
}

unsafe extern "win64" fn GetFinalPathNameByHandleW(
    h: u64,
    buf: *mut u16,
    len: u32,
    _flags: u32,
) -> u32 {
    let path = match state().get(h) {
        Some(Object::File { path, .. }) => path.clone(),
        Some(Object::Dir(p)) => p.to_string_lossy().into_owned(),
        _ => return fail(error::INVALID_HANDLE) as u32,
    };
    let full = format!(r"\\?\{}", full_path(&path));
    win::put_wide(buf, len, &full)
}

unsafe extern "win64" fn DuplicateHandle(
    _sp: u64,
    h: u64,
    _tp: u64,
    out: *mut u64,
    _a: u32,
    _i: Bool,
    _o: u32,
) -> Bool {
    // Los HANDLE se comparten (no hay otro proceso): el mismo número sirve.
    if !out.is_null() {
        *out = h;
    }
    TRUE
}

unsafe extern "win64" fn GetHandleInformation(_h: u64, flags: *mut u32) -> Bool {
    if !flags.is_null() {
        *flags = 0;
    }
    TRUE
}

unsafe extern "win64" fn SetHandleInformation(_h: u64, _mask: u32, _flags: u32) -> Bool {
    TRUE
}

// --- Memoria ------------------------------------------------------------------------------------

const HEAP_ZERO_MEMORY: u32 = 0x8;

unsafe extern "win64" fn GetProcessHeap() -> u64 {
    PROCESS_HEAP
}
unsafe extern "win64" fn HeapCreate(_o: u32, _i: usize, _m: usize) -> u64 {
    PROCESS_HEAP
}
unsafe extern "win64" fn HeapDestroy(_h: u64) -> Bool {
    TRUE
}

unsafe extern "win64" fn HeapAlloc(_h: u64, flags: u32, size: usize) -> *mut u8 {
    let p = state::heap_alloc(size, flags & HEAP_ZERO_MEMORY != 0);
    if p.is_null() {
        state::set_last_error(error::NOT_ENOUGH_MEMORY);
    }
    p
}

unsafe extern "win64" fn HeapFree(_h: u64, _flags: u32, p: *mut u8) -> Bool {
    state::heap_free(p);
    TRUE
}

unsafe extern "win64" fn HeapReAlloc(_h: u64, flags: u32, p: *mut u8, size: usize) -> *mut u8 {
    let old = if p.is_null() { 0 } else { state::heap_size(p) };
    let q = state::heap_realloc(p, size, false);
    if !q.is_null() && flags & HEAP_ZERO_MEMORY != 0 && size > old {
        std::ptr::write_bytes(q.add(old), 0, size - old);
    }
    q
}

unsafe extern "win64" fn HeapSize(_h: u64, _flags: u32, p: *const u8) -> usize {
    if p.is_null() {
        usize::MAX
    } else {
        state::heap_size(p)
    }
}

unsafe extern "win64" fn HeapValidate(_h: u64, _f: u32, _p: u64) -> Bool {
    TRUE
}

unsafe extern "win64" fn HeapSetInformation(_h: u64, _c: u32, _i: u64, _l: usize) -> Bool {
    TRUE
}

unsafe extern "win64" fn LocalAlloc(flags: u32, size: usize) -> *mut u8 {
    state::heap_alloc(size, flags & 0x40 != 0)
}

unsafe extern "win64" fn LocalFree(p: *mut u8) -> u64 {
    state::heap_free(p);
    0
}

unsafe extern "win64" fn GlobalAlloc(flags: u32, size: usize) -> *mut u8 {
    state::heap_alloc(size, flags & 0x40 != 0)
}

unsafe extern "win64" fn GlobalFree(p: *mut u8) -> u64 {
    state::heap_free(p);
    0
}

const MEM_COMMIT: u32 = 0x1000;
const MEM_RELEASE: u32 = 0x8000;
const MEM_DECOMMIT: u32 = 0x4000;

/// PAGE_* de Windows → PROT_* de Linux.
pub fn prot_of(page: u32) -> u32 {
    use crate::pe::prot::*;
    match page & 0xff {
        0x02 => READ,
        0x04 | 0x08 => READ | WRITE,
        0x10 => EXEC,
        0x20 => READ | EXEC,
        0x40 | 0x80 => READ | WRITE | EXEC,
        _ => os::PROT_NONE,
    }
}

unsafe extern "win64" fn VirtualAlloc(addr: u64, size: usize, kind: u32, protect: u32) -> u64 {
    let len = size.next_multiple_of(4096);
    let p = if kind & MEM_COMMIT == 0 {
        os::PROT_NONE
    } else {
        prot_of(protect)
    };
    if addr != 0 && kind & MEM_COMMIT != 0 && os::protect(addr & !0xfff, len, p) {
        // Confirmar algo ya reservado: solo cambian los permisos.
        return addr;
    }
    match os::map(addr & !0xffff, len, p, addr != 0) {
        Some(a) => a,
        None => {
            state::set_last_error(error::NOT_ENOUGH_MEMORY);
            0
        }
    }
}

unsafe extern "win64" fn VirtualFree(addr: u64, size: usize, kind: u32) -> Bool {
    if kind & MEM_RELEASE != 0 {
        // Windows libera todo lo reservado (size = 0); sin saber el tamaño, se libera lo que
        // diga el programa o una página.
        os::unmap(addr, size.max(4096));
        TRUE
    } else if kind & MEM_DECOMMIT != 0 {
        os::protect(addr, size.max(4096), os::PROT_NONE);
        TRUE
    } else {
        fail(error::INVALID_PARAMETER)
    }
}

unsafe extern "win64" fn VirtualProtect(addr: u64, size: usize, new: u32, old: *mut u32) -> Bool {
    let start = addr & !0xfff;
    let len = (addr + size as u64 - start) as usize;
    if !old.is_null() {
        *old = 0x04; // PAGE_READWRITE: no se guarda el anterior
    }
    if os::protect(start, len.next_multiple_of(4096), prot_of(new)) {
        TRUE
    } else {
        fail(error::INVALID_PARAMETER)
    }
}

unsafe extern "win64" fn VirtualQuery(_addr: u64, _info: *mut u8, _len: usize) -> usize {
    0
}

// --- Procesos, hilos y módulos ------------------------------------------------------------------

unsafe extern "win64" fn ExitProcess(code: u32) -> ! {
    super::exit_now(code as i32)
}

unsafe extern "win64" fn TerminateProcess(h: u64, code: u32) -> Bool {
    if h == CURRENT_PROCESS {
        super::exit_now(code as i32)
    }
    fail(error::ACCESS_DENIED)
}

unsafe extern "win64" fn GetCurrentProcess() -> u64 {
    CURRENT_PROCESS
}
unsafe extern "win64" fn GetCurrentProcessId() -> u32 {
    std::process::id()
}
unsafe extern "win64" fn GetCurrentThread() -> u64 {
    CURRENT_THREAD
}
unsafe extern "win64" fn SetThreadDescription(_h: u64, _d: *const u16) -> i32 {
    0
}

unsafe extern "win64" fn GetCommandLineW() -> *const u16 {
    state().command_line_w.as_ptr()
}

unsafe extern "win64" fn GetCommandLineA() -> *const u8 {
    state().command_line_a.as_ptr()
}

/// El HMODULE de un nombre de DLL o del programa mismo.
fn module_by_name(name: &str) -> Option<u64> {
    let s = state();
    let base = s.image_base;
    let exe = s
        .image_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    drop(s);
    let n = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if n == exe || format!("{n}.exe") == exe {
        return Some(base);
    }
    // Una DLL de verdad ya cargada, o una de las nuestras.
    modules::find_loaded(&n).or_else(|| dll_of(&n).map(module_handle))
}

unsafe extern "win64" fn GetModuleHandleW(name: *const u16) -> u64 {
    if name.is_null() {
        return state().image_base;
    }
    module_by_name(&wstr(name)).unwrap_or_else(|| {
        state::set_last_error(error::MOD_NOT_FOUND);
        0
    })
}

unsafe extern "win64" fn GetModuleHandleA(name: *const u8) -> u64 {
    if name.is_null() {
        return state().image_base;
    }
    module_by_name(&String::from_utf8_lossy(win::cstr(name))).unwrap_or_else(|| {
        state::set_last_error(error::MOD_NOT_FOUND);
        0
    })
}

unsafe extern "win64" fn GetModuleHandleExW(flags: u32, name: *const u16, out: *mut u64) -> Bool {
    const FROM_ADDRESS: u32 = 4;
    let h = if flags & FROM_ADDRESS != 0 {
        // `name` es una dirección adentro del módulo.
        modules::module_at(name as u64).map_or(0, |(b, _)| b)
    } else if name.is_null() {
        state().image_base
    } else {
        GetModuleHandleW(name)
    };
    if !out.is_null() {
        *out = h;
    }
    if h == 0 { FALSE } else { TRUE }
}

fn module_path(h: u64) -> Option<String> {
    let s = state();
    if h == 0 || h == s.image_base {
        return Some(s.image_path.clone());
    }
    drop(s);
    modules::path_of(h)
        .or_else(|| dll_of_handle(h).map(|d| format!(r"C:\Windows\System32\{}", file_name(d))))
}

unsafe extern "win64" fn GetModuleFileNameW(h: u64, buf: *mut u16, size: u32) -> u32 {
    let Some(p) = module_path(h) else {
        return fail(error::MOD_NOT_FOUND) as u32;
    };
    let w = win::to_wide(&p);
    if size == 0 {
        return fail(error::INSUFFICIENT_BUFFER) as u32;
    }
    // Si no entra, Windows copia lo que entra (con el cero) y avisa.
    let n = w.len().min(size as usize - 1);
    std::ptr::copy_nonoverlapping(w.as_ptr(), buf, n);
    *buf.add(n) = 0;
    if n < w.len() {
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return size;
    }
    state::set_last_error(0);
    n as u32
}

unsafe extern "win64" fn GetModuleFileNameA(h: u64, buf: *mut u8, size: u32) -> u32 {
    let Some(p) = module_path(h) else {
        return fail(error::MOD_NOT_FOUND) as u32;
    };
    if size == 0 {
        return fail(error::INSUFFICIENT_BUFFER) as u32;
    }
    let n = p.len().min(size as usize - 1);
    std::ptr::copy_nonoverlapping(p.as_ptr(), buf, n);
    *buf.add(n) = 0;
    if n < p.len() {
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return size;
    }
    n as u32
}

/// `LoadLibrary`: una de las nuestras, o una DLL de verdad del disco (que se inicia ya).
fn load_library(name: &str) -> u64 {
    match modules::load(name, &dir_of(name)) {
        Ok(modules::Found::Builtin(d)) => module_handle(d),
        Ok(modules::Found::Native(base)) => match modules::init_pending(false) {
            Ok(()) => base,
            Err(why) => {
                let _ = write_console(
                    2,
                    format!(
                        "vino: {why}
"
                    )
                    .as_bytes(),
                );
                state::set_last_error(error::DLL_INIT_FAILED);
                0
            }
        },
        Err(_) => {
            state::set_last_error(error::MOD_NOT_FOUND);
            0
        }
    }
}

/// La carpeta de una ruta pedida (si trae una), para buscar ahí primero.
fn dir_of(name: &str) -> String {
    let unix = win::to_unix(name);
    match unix.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => unix[..i].into(),
        None => ".".into(),
    }
}

unsafe extern "win64" fn LoadLibraryA(name: *const u8) -> u64 {
    load_library(&String::from_utf8_lossy(win::cstr(name)))
}
unsafe extern "win64" fn LoadLibraryW(name: *const u16) -> u64 {
    load_library(&wstr(name))
}
unsafe extern "win64" fn LoadLibraryExW(name: *const u16, _f: u64, _flags: u32) -> u64 {
    load_library(&wstr(name))
}
unsafe extern "win64" fn LoadLibraryExA(name: *const u8, _f: u64, _flags: u32) -> u64 {
    load_library(&String::from_utf8_lossy(win::cstr(name)))
}
unsafe extern "win64" fn FreeLibrary(_h: u64) -> Bool {
    TRUE
}

unsafe extern "win64" fn GetProcAddress(h: u64, name: *const u8) -> u64 {
    // Por ordinal: un "puntero" chico (las DLL nuestras no tienen ordinales).
    let sym = if (name as usize) < 0x1_0000 {
        Symbol::Ordinal(name as u16)
    } else {
        Symbol::Name(String::from_utf8_lossy(win::cstr(name)).into_owned())
    };
    let found = if modules::is_module(h) {
        modules::export_address(h, &sym)
    } else {
        match (&sym, dll_of_handle(h)) {
            (Symbol::Name(n), Some(d)) => find(d, n).map(|f| f as u64),
            _ => None,
        }
    };
    found.unwrap_or_else(|| {
        state::set_last_error(error::PROC_NOT_FOUND);
        0
    })
}

unsafe extern "win64" fn GetExitCodeProcess(_h: u64, code: *mut u32) -> Bool {
    if !code.is_null() {
        *code = 259; // STILL_ACTIVE
    }
    TRUE
}

unsafe extern "win64" fn GetStartupInfoW(info: *mut u8) {
    // STARTUPINFOW: 104 bytes; sin ventana ni HANDLE heredados.
    std::ptr::write_bytes(info, 0, 104);
    put32(info, 0, 104);
}

unsafe extern "win64" fn GetStartupInfoA(info: *mut u8) {
    GetStartupInfoW(info)
}

unsafe extern "win64" fn IsDebuggerPresent() -> Bool {
    FALSE
}

unsafe extern "win64" fn IsProcessorFeaturePresent(f: u32) -> Bool {
    // PF_XMMI_INSTRUCTIONS_AVAILABLE (6), PF_XMMI64 (10, SSE2), PF_NX_ENABLED (12),
    // PF_SSE3 (13), PF_COMPARE_EXCHANGE128 (14), PF_RDTSC (8). PF_FASTFAIL (23) no: `__fastfail`
    // usa `int 0x29`, que en JARVIS-OS termina el proceso igual.
    Bool::from(matches!(f, 3 | 6 | 7 | 8 | 10 | 12 | 13 | 14))
}

unsafe extern "win64" fn IsThreadAFiber() -> Bool {
    FALSE
}

unsafe extern "win64" fn SetThreadStackGuarantee(size: *mut u32) -> Bool {
    if !size.is_null() {
        *size = 0;
    }
    TRUE
}

unsafe extern "win64" fn GetProcessAffinityMask(_h: u64, p: *mut u64, s: *mut u64) -> Bool {
    for x in [p, s] {
        if !x.is_null() {
            *x = 1;
        }
    }
    TRUE
}

unsafe extern "win64" fn GetSystemInfo(info: *mut u8) {
    // SYSTEM_INFO: 48 bytes.
    std::ptr::write_bytes(info, 0, 48);
    (info as *mut u16).write_unaligned(9); // PROCESSOR_ARCHITECTURE_AMD64
    put32(info, 4, 4096);
    put64(info, 8, 0x1_0000);
    put64(info, 16, 0x7f_ffff_ffff);
    put64(info, 24, 1);
    put32(info, 32, 1);
    put32(info, 36, 8664);
    put32(info, 40, 0x1_0000);
    (info.add(44) as *mut u16).write_unaligned(6);
}

unsafe extern "win64" fn GetNativeSystemInfo(info: *mut u8) {
    GetSystemInfo(info)
}

unsafe extern "win64" fn GetVersion() -> u32 {
    (19045 << 16) | 10
}

/// OSVERSIONINFO: tamaño, mayor, menor, compilación, plataforma (Windows 10, 19045).
unsafe fn fill_version(v: *mut u8) {
    put32(v, 4, 10);
    put32(v, 8, 0);
    put32(v, 12, 19045);
    put32(v, 16, 2);
}

unsafe extern "win64" fn GetVersionExW(v: *mut u8) -> Bool {
    fill_version(v);
    TRUE
}
unsafe extern "win64" fn GetVersionExA(v: *mut u8) -> Bool {
    fill_version(v);
    TRUE
}

unsafe extern "win64" fn GetComputerNameW(buf: *mut u16, len: *mut u32) -> Bool {
    let r = win::put_wide(buf, *len, "JARVIS");
    if r >= *len {
        *len = r;
        return fail(error::INSUFFICIENT_BUFFER);
    }
    *len = r;
    TRUE
}

unsafe extern "win64" fn GetComputerNameA(buf: *mut u8, len: *mut u32) -> Bool {
    let r = win::put_narrow(buf, *len, "JARVIS");
    if r >= *len {
        *len = r;
        return fail(error::INSUFFICIENT_BUFFER);
    }
    *len = r;
    TRUE
}

unsafe extern "win64" fn GetSystemDirectoryW(buf: *mut u16, len: u32) -> u32 {
    win::put_wide(buf, len, r"C:\Windows\System32")
}

unsafe extern "win64" fn GetWindowsDirectoryW(buf: *mut u16, len: u32) -> u32 {
    win::put_wide(buf, len, r"C:\Windows")
}

unsafe extern "win64" fn IsWow64Process(_h: u64, out: *mut Bool) -> Bool {
    *out = FALSE;
    TRUE
}

unsafe extern "win64" fn GetErrorMode() -> u32 {
    0
}
unsafe extern "win64" fn SetErrorMode(_m: u32) -> u32 {
    0
}

// --- Entorno ------------------------------------------------------------------------------------

unsafe extern "win64" fn GetEnvironmentVariableW(
    name: *const u16,
    buf: *mut u16,
    size: u32,
) -> u32 {
    match std::env::var(wstr(name)) {
        Ok(v) => {
            state::set_last_error(0);
            win::put_wide(buf, size, &v)
        }
        Err(_) => {
            state::set_last_error(error::ENVVAR_NOT_FOUND);
            0
        }
    }
}

unsafe extern "win64" fn GetEnvironmentVariableA(name: *const u8, buf: *mut u8, size: u32) -> u32 {
    match std::env::var(String::from_utf8_lossy(win::cstr(name)).as_ref()) {
        Ok(v) => win::put_narrow(buf, size, &v),
        Err(_) => {
            state::set_last_error(error::ENVVAR_NOT_FOUND);
            0
        }
    }
}

/// Cambia una variable de entorno de `vino` (`None` la borra).
pub fn set_env(name: &str, value: Option<&str>) {
    if name.is_empty() || name.contains(['=', '\0']) {
        return;
    }
    // SAFETY: `vino` tiene un solo hilo, así que nadie lee el entorno a la vez.
    unsafe {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

unsafe extern "win64" fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> Bool {
    let v = (!value.is_null()).then(|| wstr(value));
    set_env(&wstr(name), v.as_deref());
    TRUE
}

unsafe extern "win64" fn SetEnvironmentVariableA(name: *const u8, value: *const u8) -> Bool {
    let v = (!value.is_null()).then(|| String::from_utf8_lossy(win::cstr(value)).into_owned());
    set_env(&String::from_utf8_lossy(win::cstr(name)), v.as_deref());
    TRUE
}

/// El entorno como lo da Windows: `A=1\0B=2\0\0`.
pub fn environment_block() -> String {
    let mut s = String::new();
    for (k, v) in std::env::vars() {
        s.push_str(&k);
        s.push('=');
        s.push_str(&v);
        s.push('\0');
    }
    s.push('\0');
    s
}

unsafe extern "win64" fn GetEnvironmentStringsW() -> *mut u16 {
    let w = win::to_wide(&environment_block());
    let p = state::heap_alloc(w.len() * 2, false) as *mut u16;
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
    }
    p
}

unsafe extern "win64" fn GetEnvironmentStrings() -> *mut u8 {
    let b = environment_block();
    let p = state::heap_alloc(b.len(), false);
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len());
    }
    p
}

unsafe extern "win64" fn FreeEnvironmentStringsW(p: *mut u8) -> Bool {
    state::heap_free(p);
    TRUE
}

unsafe extern "win64" fn FreeEnvironmentStringsA(p: *mut u8) -> Bool {
    state::heap_free(p);
    TRUE
}

/// `%VARIABLE%` → su valor (las que no existen quedan como están).
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

unsafe extern "win64" fn ExpandEnvironmentStringsW(
    src: *const u16,
    dst: *mut u16,
    size: u32,
) -> u32 {
    let e = expand_env(&wstr(src));
    let w = win::to_wide_nul(&e);
    if (size as usize) >= w.len() && !dst.is_null() {
        std::ptr::copy_nonoverlapping(w.as_ptr(), dst, w.len());
    }
    w.len() as u32
}

// --- Hora ---------------------------------------------------------------------------------------

/// Cuándo arrancó `vino` (para los contadores).
pub fn start() -> Instant {
    static S: OnceLock<Instant> = OnceLock::new();
    *S.get_or_init(Instant::now)
}

unsafe extern "win64" fn GetSystemTimeAsFileTime(ft: *mut u64) {
    ft.write_unaligned(win::filetime_now());
}

unsafe extern "win64" fn GetSystemTimePreciseAsFileTime(ft: *mut u64) {
    ft.write_unaligned(win::filetime_now());
}

unsafe fn put_system_time(st: *mut u16, ft: u64) {
    let v = win::system_time(ft);
    std::ptr::copy_nonoverlapping(v.as_ptr(), st, 8);
}

unsafe extern "win64" fn GetSystemTime(st: *mut u16) {
    put_system_time(st, win::filetime_now());
}

/// La hora "local" es la UTC: JARVIS-OS no le pasa la zona horaria a los programas todavía.
unsafe extern "win64" fn GetLocalTime(st: *mut u16) {
    put_system_time(st, win::filetime_now());
}

unsafe extern "win64" fn FileTimeToSystemTime(ft: *const u64, st: *mut u16) -> Bool {
    put_system_time(st, ft.read_unaligned());
    TRUE
}

unsafe extern "win64" fn SystemTimeToFileTime(st: *const u16, ft: *mut u64) -> Bool {
    let v = std::slice::from_raw_parts(st, 8);
    let days = crt::days_from_civil(v[0] as i64, v[1] as u32, v[3] as u32);
    let secs = days * 86_400 + v[4] as i64 * 3600 + v[5] as i64 * 60 + v[6] as i64;
    let ft_v = (secs + win::EPOCH_DIFF as i64) as u64 * 10_000_000 + v[7] as u64 * 10_000;
    ft.write_unaligned(ft_v);
    TRUE
}

unsafe extern "win64" fn FileTimeToLocalFileTime(a: *const u64, b: *mut u64) -> Bool {
    b.write_unaligned(a.read_unaligned());
    TRUE
}

unsafe extern "win64" fn GetTimeZoneInformation(tz: *mut u8) -> u32 {
    std::ptr::write_bytes(tz, 0, 172);
    0 // TIME_ZONE_ID_UNKNOWN: UTC
}

/// El contador de alta resolución, en centenas de nanosegundos (10 MHz, como Windows 10).
unsafe extern "win64" fn QueryPerformanceCounter(c: *mut i64) -> Bool {
    c.write_unaligned((start().elapsed().as_nanos() / 100) as i64 + 1);
    TRUE
}

unsafe extern "win64" fn QueryPerformanceFrequency(f: *mut i64) -> Bool {
    f.write_unaligned(10_000_000);
    TRUE
}

unsafe extern "win64" fn GetTickCount() -> u32 {
    GetTickCount64() as u32
}

unsafe extern "win64" fn GetTickCount64() -> u64 {
    start().elapsed().as_millis() as u64 + 1
}

// --- TLS y FLS ------------------------------------------------------------------------------------

fn teb_slot(i: u32) -> Option<*mut u64> {
    let teb = state::teb();
    (teb != 0 && i < 64)
        .then(|| (teb as usize + crate::teb::off::TEB_TLS_SLOTS + i as usize * 8) as *mut u64)
}

unsafe extern "win64" fn TlsAlloc() -> u32 {
    let mut s = state();
    match s.tls_used.iter().position(|u| !u) {
        Some(i) => {
            s.tls_used[i] = true;
            drop(s);
            if let Some(p) = teb_slot(i as u32) {
                *p = 0;
            }
            i as u32
        }
        None => u32::MAX, // TLS_OUT_OF_INDEXES
    }
}

unsafe extern "win64" fn TlsFree(i: u32) -> Bool {
    match state().tls_used.get_mut(i as usize) {
        Some(u) => {
            *u = false;
            TRUE
        }
        None => fail(error::INVALID_PARAMETER),
    }
}

unsafe extern "win64" fn TlsGetValue(i: u32) -> u64 {
    match teb_slot(i) {
        Some(p) => {
            state::set_last_error(0);
            *p
        }
        None => {
            state::set_last_error(error::INVALID_PARAMETER);
            0
        }
    }
}

unsafe extern "win64" fn TlsSetValue(i: u32, v: u64) -> Bool {
    match teb_slot(i) {
        Some(p) => {
            *p = v;
            TRUE
        }
        None => fail(error::INVALID_PARAMETER),
    }
}

// --- Listas "interlocked" (la sincronización de verdad está en sync.rs) ---------------------------

unsafe extern "win64" fn InitializeSListHead(head: *mut u8) {
    std::ptr::write_bytes(head, 0, 16);
}

// --- Texto e idioma -------------------------------------------------------------------------------

unsafe extern "win64" fn lstrlenW(s: *const u16) -> i32 {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n as i32
}

unsafe extern "win64" fn lstrlenA(s: *const u8) -> i32 {
    win::cstr(s).len() as i32
}

/// Los bytes de un texto "multibyte" según la página de códigos: 65001 (UTF-8) y la de
/// `vino` (CP_ACP = 0, CP_OEMCP = 1, CP_THREAD_ACP = 3, que son UTF-8), o Latin-1 (28591, 1252
/// aproximado: los dos coinciden salvo de 0x80 a 0x9F).
fn decode(cp: u32, b: &[u8]) -> String {
    match cp {
        1252 | 28591 => b.iter().map(|&c| c as char).collect(),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

fn encode(cp: u32, s: &str) -> Vec<u8> {
    match cp {
        1252 | 28591 => s
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect(),
        _ => s.as_bytes().to_vec(),
    }
}

unsafe extern "win64" fn MultiByteToWideChar(
    cp: u32,
    _flags: u32,
    src: *const u8,
    n: i32,
    dst: *mut u16,
    cap: i32,
) -> i32 {
    let bytes = if n < 0 {
        let s = win::cstr(src);
        // Con -1 se cuenta (y se copia) el cero final.
        std::slice::from_raw_parts(src, s.len() + 1)
    } else {
        std::slice::from_raw_parts(src, n as usize)
    };
    let w = win::to_wide(&decode(cp, bytes));
    if cap == 0 {
        return w.len() as i32;
    }
    if (cap as usize) < w.len() {
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(w.as_ptr(), dst, w.len());
    w.len() as i32
}

unsafe extern "win64" fn WideCharToMultiByte(
    cp: u32,
    _flags: u32,
    src: *const u16,
    n: i32,
    dst: *mut u8,
    cap: i32,
    _def: u64,
    used_def: *mut Bool,
) -> i32 {
    let wide = if n < 0 {
        let len = lstrlenW(src) as usize + 1;
        std::slice::from_raw_parts(src, len)
    } else {
        std::slice::from_raw_parts(src, n as usize)
    };
    let b = encode(cp, &win::from_wide(wide));
    if !used_def.is_null() {
        *used_def = FALSE;
    }
    if cap == 0 {
        return b.len() as i32;
    }
    if (cap as usize) < b.len() {
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(b.as_ptr(), dst, b.len());
    b.len() as i32
}

unsafe extern "win64" fn GetACP() -> u32 {
    65001
}
unsafe extern "win64" fn GetOEMCP() -> u32 {
    65001
}
unsafe extern "win64" fn IsValidCodePage(cp: u32) -> Bool {
    Bool::from(matches!(cp, 65001 | 1252 | 28591 | 20127 | 437 | 850))
}

unsafe extern "win64" fn GetCPInfo(cp: u32, info: *mut u8) -> Bool {
    // CPINFO: MaxCharSize, DefaultChar[2], LeadByte[12].
    std::ptr::write_bytes(info, 0, 20);
    put32(
        info,
        0,
        if matches!(cp, 0 | 1 | 3 | 65001) {
            4
        } else {
            1
        },
    );
    *info.add(4) = b'?';
    TRUE
}

fn lcmap(flags: u32, s: &[u16]) -> Vec<u16> {
    let t = win::from_wide(s);
    let t = if flags & 0x100 != 0 {
        t.to_lowercase()
    } else if flags & 0x200 != 0 {
        t.to_uppercase()
    } else {
        t
    };
    win::to_wide(&t)
}

unsafe fn wide_arg<'a>(p: *const u16, n: i32) -> &'a [u16] {
    let len = if n < 0 {
        lstrlenW(p) as usize
    } else {
        n as usize
    };
    std::slice::from_raw_parts(p, len)
}

unsafe extern "win64" fn LCMapStringW(
    _lcid: u32,
    flags: u32,
    src: *const u16,
    n: i32,
    dst: *mut u16,
    cap: i32,
) -> i32 {
    let mut out = lcmap(flags, wide_arg(src, n));
    if n < 0 {
        out.push(0);
    }
    if cap == 0 {
        return out.len() as i32;
    }
    if (cap as usize) < out.len() {
        state::set_last_error(error::INSUFFICIENT_BUFFER);
        return 0;
    }
    std::ptr::copy_nonoverlapping(out.as_ptr(), dst, out.len());
    out.len() as i32
}

unsafe extern "win64" fn LCMapStringEx(
    _name: *const u16,
    flags: u32,
    src: *const u16,
    n: i32,
    dst: *mut u16,
    cap: i32,
    _v: u64,
    _r: u64,
    _p: u64,
) -> i32 {
    LCMapStringW(0, flags, src, n, dst, cap)
}

unsafe extern "win64" fn GetStringTypeW(kind: u32, src: *const u16, n: i32, out: *mut u16) -> Bool {
    let s = wide_arg(src, n);
    for (i, &c) in s.iter().enumerate() {
        let ch = char::from_u32(c as u32).unwrap_or('\0');
        let v: u16 = if kind == 1 {
            // CT_CTYPE1
            let mut t = 0x200; // C1_DEFINED
            if ch.is_uppercase() {
                t |= 0x1;
            }
            if ch.is_lowercase() {
                t |= 0x2;
            }
            if ch.is_ascii_digit() {
                t |= 0x4;
            }
            if ch.is_whitespace() {
                t |= 0x8;
            }
            if ch.is_ascii_punctuation() {
                t |= 0x10;
            }
            if ch.is_control() {
                t |= 0x20;
            }
            if ch == ' ' || ch == '\t' {
                t |= 0x40;
            }
            if ch.is_ascii_hexdigit() {
                t |= 0x80;
            }
            if ch.is_alphabetic() {
                t |= 0x100;
            }
            t
        } else {
            0
        };
        *out.add(i) = v;
    }
    TRUE
}

fn compare(a: &[u16], b: &[u16], ignore_case: bool) -> i32 {
    let (x, y) = (win::from_wide(a), win::from_wide(b));
    let o = if ignore_case {
        x.to_lowercase().cmp(&y.to_lowercase())
    } else {
        x.cmp(&y)
    };
    o as i32 + 2 // CSTR_LESS_THAN = 1, EQUAL = 2, GREATER_THAN = 3
}

unsafe extern "win64" fn CompareStringW(
    _lcid: u32,
    flags: u32,
    a: *const u16,
    na: i32,
    b: *const u16,
    nb: i32,
) -> i32 {
    compare(wide_arg(a, na), wide_arg(b, nb), flags & 1 != 0)
}

unsafe extern "win64" fn CompareStringEx(
    _name: *const u16,
    flags: u32,
    a: *const u16,
    na: i32,
    b: *const u16,
    nb: i32,
    _v: u64,
    _r: u64,
    _p: u64,
) -> i32 {
    compare(wide_arg(a, na), wide_arg(b, nb), flags & 1 != 0)
}

unsafe extern "win64" fn CompareStringOrdinal(
    a: *const u16,
    na: i32,
    b: *const u16,
    nb: i32,
    ignore: Bool,
) -> i32 {
    compare(wide_arg(a, na), wide_arg(b, nb), ignore != 0)
}

unsafe extern "win64" fn GetUserDefaultLCID() -> u32 {
    0x2c0a // es-AR
}
unsafe extern "win64" fn GetUserDefaultUILanguage() -> u16 {
    0x2c0a
}
unsafe extern "win64" fn GetSystemDefaultLangID() -> u16 {
    0x2c0a
}
unsafe extern "win64" fn GetUserDefaultLocaleName(buf: *mut u16, len: i32) -> i32 {
    let r = win::put_wide(buf, len as u32, "es-AR");
    if r < len as u32 { r as i32 + 1 } else { 0 }
}
unsafe extern "win64" fn GetLocaleInfoW(_lcid: u32, _t: u32, _b: u64, _n: i32) -> i32 {
    fail(error::INVALID_PARAMETER)
}
unsafe extern "win64" fn GetLocaleInfoEx(_n: u64, _t: u32, _b: u64, _c: i32) -> i32 {
    fail(error::INVALID_PARAMETER)
}
unsafe extern "win64" fn IsDBCSLeadByte(_b: u8) -> Bool {
    FALSE
}

// --- Errores y excepciones ------------------------------------------------------------------------

unsafe extern "win64" fn GetLastError() -> u32 {
    state::last_error()
}

unsafe extern "win64" fn SetLastError(e: u32) {
    state::set_last_error(e)
}

unsafe extern "win64" fn FormatMessageW(
    _f: u32,
    _s: u64,
    _id: u32,
    _l: u32,
    _b: u64,
    _n: u32,
    _a: u64,
) -> u32 {
    state::set_last_error(error::CALL_NOT_IMPLEMENTED);
    0
}
unsafe extern "win64" fn FormatMessageA(
    _f: u32,
    _s: u64,
    _id: u32,
    _l: u32,
    _b: u64,
    _n: u32,
    _a: u64,
) -> u32 {
    state::set_last_error(error::CALL_NOT_IMPLEMENTED);
    0
}
unsafe extern "win64" fn OutputDebugStringA(_s: *const u8) {}
unsafe extern "win64" fn OutputDebugStringW(_s: *const u16) {}

unsafe extern "win64" fn RaiseException(code: u32, _flags: u32, _n: u32, _args: u64) -> ! {
    super::fatal(&format!(
        "el programa lanzó una excepción de Windows (0x{code:08X}) y vino todavía no las maneja"
    ))
}

unsafe extern "win64" fn SetUnhandledExceptionFilter(_f: u64) -> u64 {
    0
}
unsafe extern "win64" fn UnhandledExceptionFilter(_p: u64) -> i32 {
    1 // EXCEPTION_EXECUTE_HANDLER
}
unsafe extern "win64" fn AddVectoredExceptionHandler(_first: u32, f: u64) -> u64 {
    // Se acepta (Rust pone uno para avisar del desborde de pila) pero nunca se llama.
    f | 1
}
unsafe extern "win64" fn RemoveVectoredExceptionHandler(_h: u64) -> u32 {
    1
}
unsafe extern "win64" fn EncodePointer(p: u64) -> u64 {
    p
}
unsafe extern "win64" fn DecodePointer(p: u64) -> u64 {
    p
}
unsafe extern "win64" fn Beep(_f: u32, _d: u32) -> Bool {
    TRUE
}

use super::crt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comodines_y_rutas_completas() {
        assert!(wildcard("*.txt", "Notas.TXT"));
        assert!(wildcard("a?c", "abc"));
        assert!(!wildcard("*.txt", "notas.md"));
        assert!(wildcard("*.*", "sin-punto"));
        assert!(full_path(r"C:\a\.\b\..\c").ends_with(r"\a\c"));
        assert_eq!(full_path(r"C:\a\b\"), r"C:\a\b\");
        assert!(full_path("x.txt").starts_with(r"C:\") && full_path("x.txt").ends_with(r"\x.txt"));
    }

    #[test]
    fn entorno_al_estilo_windows() {
        set_env("VINO_PRUEBA", Some("42"));
        assert!(environment_block().contains("VINO_PRUEBA=42\0"));
        assert_eq!(
            expand_env("a %VINO_PRUEBA% b %NO_EXISTE% %"),
            "a 42 b %NO_EXISTE% %"
        );
        set_env("VINO_PRUEBA", None);
        assert!(!environment_block().contains("VINO_PRUEBA"));
    }

    #[test]
    fn archivos_con_la_api_de_windows() {
        let dir = "target/vino-prueba-k32";
        let _ = fs::remove_dir_all(dir);
        let wdir = win::to_wide_nul(&dir.replace('/', "\\"));
        // SAFETY: textos terminados en cero y buffers del tamaño que se dice.
        unsafe {
            assert_eq!(CreateDirectoryW(wdir.as_ptr(), 0), TRUE);
            assert_eq!(CreateDirectoryW(wdir.as_ptr(), 0), FALSE);
            assert_eq!(GetLastError(), error::ALREADY_EXISTS);
            let name = win::to_wide_nul(&format!(r"{}\datos.txt", dir.replace('/', "\\")));
            let h = CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, 0, 2, 0, 0);
            assert_ne!(h, INVALID_HANDLE);
            let mut done = 0u32;
            assert_eq!(
                WriteFile(h, b"uno\r\ndos\n".as_ptr(), 9, &mut done, 0),
                TRUE
            );
            assert_eq!(done, 9);
            assert_eq!(GetFileType(h), 1);
            assert_eq!(CloseHandle(h), TRUE);
            assert_eq!(CloseHandle(h), FALSE);
            // Ya existe: CREATE_ALWAYS avisa con ERROR_ALREADY_EXISTS.
            let h = CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, 0, 4, 0, 0);
            assert_eq!(GetLastError(), error::ALREADY_EXISTS);
            let mut size = 0i64;
            assert_eq!(GetFileSizeEx(h, &mut size), TRUE);
            assert_eq!(size, 9);
            let mut buf = [0u8; 32];
            assert_eq!(ReadFile(h, buf.as_mut_ptr(), 32, &mut done, 0), TRUE);
            assert_eq!(
                &buf[..done as usize],
                b"uno\r\ndos\n",
                "los archivos no se traducen"
            );
            assert_eq!(ReadFile(h, buf.as_mut_ptr(), 32, &mut done, 0), TRUE);
            assert_eq!(done, 0, "fin del archivo");
            let mut info = [0u8; 52];
            assert_eq!(GetFileInformationByHandle(h, info.as_mut_ptr()), TRUE);
            assert_eq!(u32::from_le_bytes(info[36..40].try_into().unwrap()), 9);
            CloseHandle(h);
            // Una carpeta se abre solo con FILE_FLAG_BACKUP_SEMANTICS.
            assert_eq!(CreateFileW(wdir.as_ptr(), 0, 0, 0, 3, 0, 0), INVALID_HANDLE);
            let d = CreateFileW(wdir.as_ptr(), 0, 0, 0, 3, FILE_FLAG_BACKUP_SEMANTICS, 0);
            assert_ne!(d, INVALID_HANDLE);
            let mut tag = [0u8; 8];
            assert_eq!(
                GetFileInformationByHandleEx(d, 9, tag.as_mut_ptr(), 8),
                TRUE
            );
            assert_eq!(
                tag[0] as u32 & FILE_ATTRIBUTE_DIRECTORY,
                FILE_ATTRIBUTE_DIRECTORY
            );
            CloseHandle(d);
            // FindFirstFile: ".", ".." y el archivo.
            let pat = win::to_wide_nul(&format!(r"{}\*", dir.replace('/', "\\")));
            let mut fd = [0u8; 592];
            let f = FindFirstFileExW(pat.as_ptr(), 0, fd.as_mut_ptr(), 0, 0, 0);
            assert_ne!(f, INVALID_HANDLE);
            let name_of = |fd: &[u8]| {
                let w: Vec<u16> = fd[44..44 + 520]
                    .chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&c| c != 0)
                    .collect();
                String::from_utf16(&w).unwrap()
            };
            let mut names = vec![name_of(&fd)];
            while FindNextFileW(f, fd.as_mut_ptr()) == TRUE {
                names.push(name_of(&fd));
            }
            assert_eq!(GetLastError(), error::NO_MORE_FILES);
            assert_eq!(names, [".", "..", "datos.txt"]);
            FindClose(f);
            let missing = win::to_wide_nul(&format!(r"{}\no-esta\x", dir.replace('/', "\\")));
            assert_eq!(
                CreateFileW(missing.as_ptr(), GENERIC_READ, 0, 0, 3, 0, 0),
                INVALID_HANDLE
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn la_consola_saca_los_retornos_de_carro() {
        assert_eq!(strip_cr(b"a\r\nb\rc\r\n"), b"a\nb\rc\n");
    }

    #[test]
    fn texto_utf8_y_utf16() {
        let mut w = [0u16; 16];
        // SAFETY: textos y buffers válidos.
        unsafe {
            let n = MultiByteToWideChar(
                65001,
                0,
                c"año".as_ptr().cast(),
                -1,
                std::ptr::null_mut(),
                0,
            );
            assert_eq!(n, 4, "con el cero");
            assert_eq!(
                MultiByteToWideChar(65001, 0, "año".as_ptr(), 4, w.as_mut_ptr(), 16),
                3
            );
            assert_eq!(win::from_wide(&w[..3]), "año");
            let mut b = [0u8; 16];
            assert_eq!(
                WideCharToMultiByte(
                    65001,
                    0,
                    w.as_ptr(),
                    3,
                    b.as_mut_ptr(),
                    16,
                    0,
                    std::ptr::null_mut()
                ),
                4
            );
            assert_eq!(&b[..4], "año".as_bytes());
            let a = win::to_wide_nul("Hola");
            let c = win::to_wide_nul("hola");
            assert_eq!(CompareStringW(0, 1, a.as_ptr(), -1, c.as_ptr(), -1), 2);
            assert_eq!(CompareStringW(0, 0, a.as_ptr(), -1, c.as_ptr(), -1), 1);
        }
    }
}
