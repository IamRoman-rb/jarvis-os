//! La biblioteca de C de Microsoft: `ucrtbase.dll` (Visual Studio 2015 en adelante, con sus
//! "api sets" `api-ms-win-crt-*`), `msvcrt.dll` (la vieja, la de MinGW) y `vcruntime140.dll`.
//!
//! Un programa de C compilado con `/MD` trae adentro solo un pedacito del CRT: el arranque
//! (`mainCRTStartup`), que llama a estas funciones en este orden:
//!
//! 1. `_initterm_e`/`_initterm`: los inicializadores del programa (tablas de punteros a
//!    funciones entre dos símbolos que pone el enlazador);
//! 2. `_configure_narrow_argv`, `__p___argc`, `__p___argv`, `_get_initial_narrow_environment`:
//!    los argumentos de `main`, que **el CRT** saca de la línea de comandos;
//! 3. `main`, y después `exit(código)`, que corre lo registrado con `atexit` y termina.
//!
//! En la UCRT, `printf` no es una función de la DLL: es una función "inline" de los encabezados
//! que llama a `__stdio_common_vfprintf(opciones, FILE*, formato, locale, va_list)`. `stdout` es
//! `__acrt_iob_func(1)`. En `msvcrt.dll`, en cambio, `printf` sí se exporta y recibe argumentos
//! variables (ver [`varargs`]).
//!
//! Los `FILE*` que ve el programa son direcciones nuestras: el programa nunca mira adentro (la
//! UCRT no lo permite y MinGW usa funciones). Tres fijas para `stdin`, `stdout` y `stderr` (en
//! `_iob`, de 48 bytes cada una como en `msvcrt`) y una por cada `fopen`.

#![allow(
    non_snake_case,
    non_upper_case_globals,
    unsafe_op_in_unsafe_fn,
    clippy::missing_safety_doc
)]

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use super::kernel32::{self, read_handle, strip_cr, write_console};
use crate::printf::{self, VaList};
use crate::state::{self, STD_IN};
use crate::win::{self, cstr, wstr};

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    let mut v = exports![
        // Arranque y salida
        _initterm,
        _initterm_e,
        _configure_narrow_argv,
        _configure_wide_argv,
        _initialize_narrow_environment,
        _initialize_wide_environment,
        _get_initial_narrow_environment,
        _get_initial_wide_environment,
        __p___argc,
        __p___argv,
        __p___wargv,
        __p__environ,
        __p__wenviron,
        __getmainargs,
        __wgetmainargs,
        __p__commode,
        __p__fmode,
        _set_fmode,
        _get_fmode,
        _set_app_type,
        __set_app_type,
        __setusermatherr,
        _configthreadlocale,
        _set_new_mode,
        _query_new_mode,
        _callnewh,
        _seh_filter_exe,
        _seh_filter_dll,
        _register_thread_local_exe_atexit_callback,
        _crt_atexit,
        _crt_at_quick_exit,
        _register_onexit_function,
        _initialize_onexit_table,
        _execute_onexit_table,
        atexit,
        _onexit,
        exit,
        _exit,
        _Exit,
        quick_exit,
        _cexit,
        _c_exit,
        abort,
        terminate,
        _set_abort_behavior,
        _set_invalid_parameter_handler,
        _invalid_parameter_noinfo,
        _invalid_parameter_noinfo_noreturn,
        _invoke_watson,
        _set_purecall_handler,
        signal,
        raise,
        _errno,
        __doserrno,
        _get_errno,
        _set_errno,
        __acrt_iob_func,
        __iob_func,
        _get_heap_handle,
        _getpid,
        __daylight,
        __timezone,
        __tzname,
        _tzset,
        __lconv_init,
        ___lc_codepage_func,
        ___mb_cur_max_func,
        __pctype_func,
        setlocale,
        localeconv,
        _lock_file,
        _unlock_file,
        // vcruntime: memoria, excepciones y C++
        memcpy,
        memmove,
        memset,
        memcmp,
        memchr,
        __C_specific_handler,
        __CxxFrameHandler3,
        __CxxFrameHandler4,
        _CxxThrowException,
        __std_terminate,
        _purecall,
        __current_exception,
        __current_exception_context,
        __std_exception_copy,
        __std_exception_destroy,
        __std_type_info_destroy_list,
        __std_type_info_name,
        __vcrt_GetModuleFileNameW,
        // Memoria
        malloc,
        calloc,
        realloc,
        free,
        _msize,
        _recalloc,
        _aligned_malloc,
        _aligned_free,
        _aligned_realloc,
        _strdup,
        strdup,
        _wcsdup,
        // stdio
        __stdio_common_vfprintf,
        __stdio_common_vsprintf,
        __stdio_common_vsnprintf_s,
        __stdio_common_vsprintf_s,
        __stdio_common_vfwprintf,
        __stdio_common_vswprintf,
        vprintf,
        vfprintf,
        vsprintf,
        vsnprintf,
        _vsnprintf,
        _vscprintf,
        fopen,
        _wfopen,
        fopen_s,
        _wfopen_s,
        freopen,
        fclose,
        fflush,
        _flushall,
        fgets,
        fgetc,
        getc,
        _fgetc_nolock,
        getchar,
        ungetc,
        fputc,
        putc,
        _fputc_nolock,
        putchar,
        fputs,
        puts,
        fwrite,
        _fwrite_nolock,
        fread,
        _fread_nolock,
        fseek,
        _fseeki64,
        ftell,
        _ftelli64,
        rewind,
        feof,
        ferror,
        clearerr,
        _fileno,
        setvbuf,
        setbuf,
        fputws,
        fputwc,
        putwchar,
        _write,
        _read,
        _isatty,
        _setmode,
        _close,
        _get_osfhandle,
        _commit,
        remove,
        _unlink,
        _wremove,
        rename,
        _wrename,
        _mkdir,
        _wmkdir,
        _rmdir,
        _chdir,
        _getcwd,
        _access,
        _waccess,
        _fullpath,
        tmpfile,
        // Cadenas
        strlen,
        strnlen,
        strcpy,
        strncpy,
        strcat,
        strncat,
        strcmp,
        strncmp,
        _stricmp,
        _strnicmp,
        _strcmpi,
        stricmp,
        strchr,
        strrchr,
        strstr,
        strspn,
        strcspn,
        strpbrk,
        strtok,
        strtok_s,
        _strlwr,
        _strupr,
        strerror,
        strcpy_s,
        strcat_s,
        strncpy_s,
        memcpy_s,
        memmove_s,
        wcslen,
        wcsnlen,
        wcscpy,
        wcsncpy,
        wcscat,
        wcscmp,
        wcsncmp,
        _wcsicmp,
        wcschr,
        wcsrchr,
        wcsstr,
        wcscpy_s,
        wcscat_s,
        perror,
        // Caracteres
        isalpha,
        isdigit,
        isalnum,
        isspace,
        isupper,
        islower,
        isxdigit,
        ispunct,
        isprint,
        isgraph,
        iscntrl,
        isascii,
        __isascii,
        toupper,
        tolower,
        _toupper,
        _tolower,
        iswalpha,
        iswdigit,
        iswspace,
        iswalnum,
        iswupper,
        iswlower,
        towupper,
        towlower,
        isleadbyte,
        // Números
        atoi,
        atol,
        atoll,
        _atoi64,
        atof,
        strtol,
        strtoul,
        strtoll,
        strtoull,
        _strtoi64,
        _strtoui64,
        strtod,
        strtof,
        _itoa,
        _ltoa,
        _i64toa,
        _ui64toa,
        abs,
        labs,
        llabs,
        _abs64,
        rand,
        srand,
        rand_s,
        qsort,
        bsearch,
        getenv,
        _wgetenv,
        _putenv,
        system,
        // Matemática
        sin,
        cos,
        tan,
        asin,
        acos,
        atan,
        atan2,
        sinh,
        cosh,
        tanh,
        exp,
        exp2,
        log,
        log10,
        log2,
        log1p,
        expm1,
        pow,
        sqrt,
        cbrt,
        floor,
        ceil,
        round,
        trunc,
        fabs,
        fmod,
        hypot,
        _hypot,
        ldexp,
        frexp,
        modf,
        fmin,
        fmax,
        copysign,
        _copysign,
        nan,
        sinf,
        cosf,
        tanf,
        atan2f,
        expf,
        logf,
        log10f,
        powf,
        sqrtf,
        floorf,
        ceilf,
        roundf,
        truncf,
        fabsf,
        fmodf,
        lround,
        llround,
        rint,
        nearbyint,
        _isnan,
        _finite,
        _fdtest,
        _dtest,
        _dclass,
        // Hora
        time,
        _time64,
        _time32,
        clock,
        _localtime64,
        localtime,
        _gmtime64,
        gmtime,
        _localtime64_s,
        _gmtime64_s,
        localtime_s,
        gmtime_s,
        _mktime64,
        mktime,
        _mkgmtime64,
        strftime,
        _difftime64,
        difftime,
    ];
    v.extend(varargs::exports());
    // Datos (no funciones): el programa recibe la dirección de la variable.
    v.push(("_iob", IOB.0.get() as usize));
    v.push(("_commode", COMMODE.as_ptr() as usize));
    v.push(("_fmode", FMODE.as_ptr() as usize));
    v
}

// --- errno -------------------------------------------------------------------------------------

static ERRNO: AtomicI32 = AtomicI32::new(0);
static DOSERRNO: AtomicU32 = AtomicU32::new(0);

pub mod errno {
    pub const ENOENT: i32 = 2;
    pub const EBADF: i32 = 9;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EEXIST: i32 = 17;
    pub const EINVAL: i32 = 22;
    pub const ERANGE: i32 = 34;
    pub const ENOTEMPTY: i32 = 41;
}

fn set_errno(e: i32) {
    ERRNO.store(e, Ordering::Relaxed);
}

fn errno_of(e: &std::io::Error) -> i32 {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => errno::ENOENT,
        PermissionDenied => errno::EACCES,
        AlreadyExists => errno::EEXIST,
        DirectoryNotEmpty => errno::ENOTEMPTY,
        OutOfMemory => errno::ENOMEM,
        _ => errno::EINVAL,
    }
}

unsafe extern "win64" fn _errno() -> *mut i32 {
    ERRNO.as_ptr()
}
unsafe extern "win64" fn __doserrno() -> *mut u32 {
    DOSERRNO.as_ptr()
}
unsafe extern "win64" fn _get_errno(e: *mut i32) -> i32 {
    *e = ERRNO.load(Ordering::Relaxed);
    0
}
unsafe extern "win64" fn _set_errno(e: i32) -> i32 {
    set_errno(e);
    0
}

// --- Arranque y salida ---------------------------------------------------------------------------

static ARGC: AtomicI32 = AtomicI32::new(0);
static ARGV: AtomicU64 = AtomicU64::new(0);
static WARGV: AtomicU64 = AtomicU64::new(0);
static ENVP: AtomicU64 = AtomicU64::new(0);
static WENVP: AtomicU64 = AtomicU64::new(0);
static COMMODE: AtomicI32 = AtomicI32::new(0);
static FMODE: AtomicI32 = AtomicI32::new(0);

/// Un arreglo de punteros a textos terminados en cero (con el puntero nulo al final), que vive
/// hasta que termina el proceso.
fn leak_table(items: Vec<Vec<u8>>) -> u64 {
    let mut ptrs: Vec<u64> = items
        .into_iter()
        .map(|b| Box::leak(b.into_boxed_slice()).as_ptr() as u64)
        .collect();
    ptrs.push(0);
    Box::leak(ptrs.into_boxed_slice()).as_ptr() as u64
}

fn narrow(s: &str) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    b
}

fn wide_bytes(s: &str) -> Vec<u8> {
    win::to_wide_nul(s)
        .iter()
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

/// Los argumentos y el entorno de `main`/`wmain` (los arma `run` al arrancar).
pub fn set_args(args: &[String]) {
    ARGC.store(args.len() as i32, Ordering::Relaxed);
    ARGV.store(
        leak_table(args.iter().map(|a| narrow(a)).collect()),
        Ordering::Relaxed,
    );
    WARGV.store(
        leak_table(args.iter().map(|a| wide_bytes(a)).collect()),
        Ordering::Relaxed,
    );
    let env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    ENVP.store(
        leak_table(env.iter().map(|e| narrow(e)).collect()),
        Ordering::Relaxed,
    );
    WENVP.store(
        leak_table(env.iter().map(|e| wide_bytes(e)).collect()),
        Ordering::Relaxed,
    );
}

unsafe extern "win64" fn _initterm(start: *const u64, end: *const u64) {
    let mut p = start;
    while p < end {
        let f = *p;
        if f != 0 {
            super::call0(f);
        }
        p = p.add(1);
    }
}

unsafe extern "win64" fn _initterm_e(start: *const u64, end: *const u64) -> i32 {
    let mut p = start;
    while p < end {
        let f = *p;
        if f != 0 {
            let f: unsafe extern "win64" fn() -> i32 = std::mem::transmute(f as usize);
            let r = f();
            if r != 0 {
                return r;
            }
        }
        p = p.add(1);
    }
    0
}

unsafe extern "win64" fn _configure_narrow_argv(_mode: u32) -> i32 {
    0
}
unsafe extern "win64" fn _configure_wide_argv(_mode: u32) -> i32 {
    0
}
unsafe extern "win64" fn _initialize_narrow_environment() -> i32 {
    0
}
unsafe extern "win64" fn _initialize_wide_environment() -> i32 {
    0
}
unsafe extern "win64" fn _get_initial_narrow_environment() -> u64 {
    ENVP.load(Ordering::Relaxed)
}
unsafe extern "win64" fn _get_initial_wide_environment() -> u64 {
    WENVP.load(Ordering::Relaxed)
}
unsafe extern "win64" fn __p___argc() -> *mut i32 {
    ARGC.as_ptr()
}
unsafe extern "win64" fn __p___argv() -> *mut u64 {
    ARGV.as_ptr()
}
unsafe extern "win64" fn __p___wargv() -> *mut u64 {
    WARGV.as_ptr()
}
unsafe extern "win64" fn __p__environ() -> *mut u64 {
    ENVP.as_ptr()
}
unsafe extern "win64" fn __p__wenviron() -> *mut u64 {
    WENVP.as_ptr()
}

/// El arranque de `msvcrt.dll` (MinGW): los tres de una vez.
unsafe extern "win64" fn __getmainargs(
    argc: *mut i32,
    argv: *mut u64,
    env: *mut u64,
    _w: i32,
    _si: u64,
) -> i32 {
    *argc = ARGC.load(Ordering::Relaxed);
    *argv = ARGV.load(Ordering::Relaxed);
    *env = ENVP.load(Ordering::Relaxed);
    0
}

unsafe extern "win64" fn __wgetmainargs(
    argc: *mut i32,
    argv: *mut u64,
    env: *mut u64,
    _w: i32,
    _si: u64,
) -> i32 {
    *argc = ARGC.load(Ordering::Relaxed);
    *argv = WARGV.load(Ordering::Relaxed);
    *env = WENVP.load(Ordering::Relaxed);
    0
}

unsafe extern "win64" fn __p__commode() -> *mut i32 {
    COMMODE.as_ptr()
}
unsafe extern "win64" fn __p__fmode() -> *mut i32 {
    FMODE.as_ptr()
}
unsafe extern "win64" fn _set_fmode(m: i32) -> i32 {
    FMODE.store(m, Ordering::Relaxed);
    0
}
unsafe extern "win64" fn _get_fmode(m: *mut i32) -> i32 {
    *m = FMODE.load(Ordering::Relaxed);
    0
}
unsafe extern "win64" fn _set_app_type(_t: u32) {}
unsafe extern "win64" fn __set_app_type(_t: u32) {}
unsafe extern "win64" fn __setusermatherr(_f: u64) {}
unsafe extern "win64" fn _configthreadlocale(_f: i32) -> i32 {
    2 // _DISABLE_PER_THREAD_LOCALE
}
unsafe extern "win64" fn _set_new_mode(_m: i32) -> i32 {
    0
}
unsafe extern "win64" fn _query_new_mode() -> i32 {
    0
}
unsafe extern "win64" fn _callnewh(_size: usize) -> i32 {
    0
}
unsafe extern "win64" fn _seh_filter_exe(_code: u32, _p: u64) -> i32 {
    0 // EXCEPTION_CONTINUE_SEARCH
}
unsafe extern "win64" fn _seh_filter_dll(_code: u32, _p: u64) -> i32 {
    0
}
unsafe extern "win64" fn _register_thread_local_exe_atexit_callback(_f: u64) {}

fn add_atexit(f: u64) -> i32 {
    if f == 0 {
        return -1;
    }
    state::state().atexit.push(f);
    0
}

unsafe extern "win64" fn _crt_atexit(f: u64) -> i32 {
    add_atexit(f)
}
unsafe extern "win64" fn _crt_at_quick_exit(_f: u64) -> i32 {
    0
}
unsafe extern "win64" fn _register_onexit_function(_table: u64, f: u64) -> i32 {
    // Hay un solo módulo (el programa): todas las tablas son la misma.
    add_atexit(f)
}
unsafe extern "win64" fn _initialize_onexit_table(table: *mut u64) -> i32 {
    if !table.is_null() {
        std::ptr::write_bytes(table, 0, 3);
    }
    0
}
unsafe extern "win64" fn _execute_onexit_table(_table: u64) -> i32 {
    super::run_atexit();
    0
}
unsafe extern "win64" fn atexit(f: u64) -> i32 {
    add_atexit(f)
}
unsafe extern "win64" fn _onexit(f: u64) -> u64 {
    if add_atexit(f) == 0 { f } else { 0 }
}
unsafe extern "win64" fn exit(code: i32) -> ! {
    super::exit(code)
}
unsafe extern "win64" fn _exit(code: i32) -> ! {
    super::exit_now(code)
}
unsafe extern "win64" fn _Exit(code: i32) -> ! {
    super::exit_now(code)
}
unsafe extern "win64" fn quick_exit(code: i32) -> ! {
    super::exit_now(code)
}
unsafe extern "win64" fn _cexit() {
    super::run_atexit();
    flush_all();
}
unsafe extern "win64" fn _c_exit() {
    flush_all();
}

unsafe extern "win64" fn abort() -> ! {
    flush_all();
    eprintln!("vino: el programa llamó a abort()");
    std::process::exit(3)
}
unsafe extern "win64" fn terminate() -> ! {
    super::fatal("el programa llamó a terminate() (una excepción de C++ sin atrapar)")
}
unsafe extern "win64" fn _set_abort_behavior(_f: u32, _m: u32) -> u32 {
    0
}
unsafe extern "win64" fn _set_invalid_parameter_handler(_h: u64) -> u64 {
    0
}
unsafe extern "win64" fn _invalid_parameter_noinfo() {
    set_errno(errno::EINVAL);
}
unsafe extern "win64" fn _invalid_parameter_noinfo_noreturn() -> ! {
    super::fatal("el programa le pasó un parámetro inválido a una función del CRT")
}
unsafe extern "win64" fn _invoke_watson(_a: u64, _b: u64, _c: u64, _d: u32, _e: u64) -> ! {
    super::fatal("el CRT encontró un error grave (_invoke_watson)")
}
unsafe extern "win64" fn _set_purecall_handler(_h: u64) -> u64 {
    0
}
unsafe extern "win64" fn signal(_sig: i32, _f: u64) -> u64 {
    0 // SIG_DFL
}
unsafe extern "win64" fn raise(sig: i32) -> i32 {
    flush_all();
    eprintln!("vino: el programa levantó la señal {sig}");
    std::process::exit(3)
}

unsafe extern "win64" fn _get_heap_handle() -> u64 {
    state::PROCESS_HEAP
}
unsafe extern "win64" fn _getpid() -> i32 {
    std::process::id() as i32
}

static DAYLIGHT: AtomicI32 = AtomicI32::new(0);
static TIMEZONE: AtomicI32 = AtomicI32::new(0);
static TZNAME: [u64; 2] = [0, 0];

unsafe extern "win64" fn __daylight() -> *mut i32 {
    DAYLIGHT.as_ptr()
}
unsafe extern "win64" fn __timezone() -> *mut i32 {
    TIMEZONE.as_ptr()
}
unsafe extern "win64" fn __tzname() -> *const u64 {
    TZNAME.as_ptr()
}
unsafe extern "win64" fn _tzset() {}
unsafe extern "win64" fn __lconv_init() -> i32 {
    0
}
unsafe extern "win64" fn ___lc_codepage_func() -> u32 {
    65001
}
unsafe extern "win64" fn ___mb_cur_max_func() -> i32 {
    1
}

/// La tabla de tipos de carácter (`_pctype`) de los 256 bytes, para los `isalpha` "inline".
unsafe extern "win64" fn __pctype_func() -> *const u16 {
    static T: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        (0..=255u8)
            .map(|c| {
                let mut t = 0u16;
                if c.is_ascii_uppercase() {
                    t |= 0x1;
                }
                if c.is_ascii_lowercase() {
                    t |= 0x2;
                }
                if c.is_ascii_digit() {
                    t |= 0x4;
                }
                if c.is_ascii_whitespace() || c == 0x0b {
                    t |= 0x8;
                }
                if c.is_ascii_punctuation() {
                    t |= 0x10;
                }
                if c.is_ascii_control() {
                    t |= 0x20;
                }
                if c == b' ' {
                    t |= 0x40;
                }
                if c.is_ascii_hexdigit() {
                    t |= 0x80;
                }
                if c.is_ascii_alphabetic() {
                    t |= 0x100;
                }
                t
            })
            .collect()
    })
    .as_ptr()
}

static C_LOCALE: &[u8] = b"C\0";
static DOT: &[u8] = b".\0";
static EMPTY: &[u8] = b"\0";

unsafe extern "win64" fn setlocale(_cat: i32, _loc: *const u8) -> *const u8 {
    // Una sola configuración: "C" (con UTF-8).
    C_LOCALE.as_ptr()
}

unsafe extern "win64" fn localeconv() -> *const u64 {
    // `struct lconv`: decimal_point, thousands_sep, grouping y el resto vacíos.
    static L: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut v = vec![EMPTY.as_ptr() as u64; 24];
        v[0] = DOT.as_ptr() as u64;
        v
    })
    .as_ptr()
}

unsafe extern "win64" fn _lock_file(_f: u64) {}
unsafe extern "win64" fn _unlock_file(_f: u64) {}

// --- vcruntime -----------------------------------------------------------------------------------

unsafe extern "win64" fn memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    std::ptr::copy(s, d, n);
    d
}
unsafe extern "win64" fn memmove(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    std::ptr::copy(s, d, n);
    d
}
unsafe extern "win64" fn memset(d: *mut u8, c: i32, n: usize) -> *mut u8 {
    std::ptr::write_bytes(d, c as u8, n);
    d
}
unsafe extern "win64" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let (x, y) = (
        std::slice::from_raw_parts(a, n),
        std::slice::from_raw_parts(b, n),
    );
    x.iter()
        .zip(y)
        .find(|(p, q)| p != q)
        .map_or(0, |(p, q)| *p as i32 - *q as i32)
}
unsafe extern "win64" fn memchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    std::slice::from_raw_parts(s, n)
        .iter()
        .position(|&b| b == c as u8)
        .map_or(std::ptr::null(), |i| s.add(i))
}

/// Los manejadores de excepciones los llama Windows al despachar una excepción, cosa que `vino`
/// no hace: si llegaran a llamarse, "seguir buscando".
unsafe extern "win64" fn __C_specific_handler(_r: u64, _f: u64, _c: u64, _d: u64) -> i32 {
    1 // ExceptionContinueSearch
}
unsafe extern "win64" fn __CxxFrameHandler3(_r: u64, _f: u64, _c: u64, _d: u64) -> i32 {
    1
}
unsafe extern "win64" fn __CxxFrameHandler4(_r: u64, _f: u64, _c: u64, _d: u64) -> i32 {
    1
}
unsafe extern "win64" fn _CxxThrowException(_obj: u64, _info: u64) -> ! {
    super::fatal("el programa lanzó una excepción de C++ y vino todavía no las maneja")
}
unsafe extern "win64" fn __std_terminate() -> ! {
    super::fatal("el programa llamó a std::terminate()")
}
unsafe extern "win64" fn _purecall() -> ! {
    super::fatal("el programa llamó a una función virtual pura")
}
static NO_EXCEPTION: AtomicU64 = AtomicU64::new(0);
unsafe extern "win64" fn __current_exception() -> *mut u64 {
    NO_EXCEPTION.as_ptr()
}
unsafe extern "win64" fn __current_exception_context() -> *mut u64 {
    NO_EXCEPTION.as_ptr()
}
/// `std::exception` por dentro: {const char* what; bool hay_que_liberar}.
unsafe extern "win64" fn __std_exception_copy(from: *const u64, to: *mut u64) {
    *to = *from;
    *to.add(1) = 0;
}
unsafe extern "win64" fn __std_exception_destroy(_e: u64) {}
unsafe extern "win64" fn __std_type_info_destroy_list(_l: u64) {}
unsafe extern "win64" fn __std_type_info_name(_t: u64, _l: u64) -> *const u8 {
    c"?".as_ptr().cast()
}
unsafe extern "win64" fn __vcrt_GetModuleFileNameW(h: u64, buf: *mut u16, n: u32) -> u32 {
    let f: unsafe extern "win64" fn(u64, *mut u16, u32) -> u32 =
        std::mem::transmute(super::find(super::Dll::Kernel32, "GetModuleFileNameW").unwrap_or(0));
    f(h, buf, n)
}

// --- Memoria ------------------------------------------------------------------------------------

unsafe extern "win64" fn malloc(n: usize) -> *mut u8 {
    let p = state::heap_alloc(n, false);
    if p.is_null() {
        set_errno(errno::ENOMEM);
    }
    p
}
unsafe extern "win64" fn calloc(n: usize, size: usize) -> *mut u8 {
    match n.checked_mul(size) {
        Some(t) => state::heap_alloc(t, true),
        None => {
            set_errno(errno::ENOMEM);
            std::ptr::null_mut()
        }
    }
}
unsafe extern "win64" fn realloc(p: *mut u8, n: usize) -> *mut u8 {
    if n == 0 && !p.is_null() {
        state::heap_free(p);
        return std::ptr::null_mut();
    }
    state::heap_realloc(p, n, false)
}
unsafe extern "win64" fn free(p: *mut u8) {
    state::heap_free(p)
}
unsafe extern "win64" fn _msize(p: *const u8) -> usize {
    state::heap_size(p)
}
unsafe extern "win64" fn _recalloc(p: *mut u8, n: usize, size: usize) -> *mut u8 {
    let Some(t) = n.checked_mul(size) else {
        return std::ptr::null_mut();
    };
    let old = if p.is_null() { 0 } else { state::heap_size(p) };
    let q = state::heap_realloc(p, t, false);
    if !q.is_null() && t > old {
        std::ptr::write_bytes(q.add(old), 0, t - old);
    }
    q
}

/// `_aligned_malloc`: se pide de más y se guarda el puntero original justo antes del alineado.
unsafe extern "win64" fn _aligned_malloc(n: usize, align: usize) -> *mut u8 {
    let align = align.max(16).next_power_of_two();
    let raw = state::heap_alloc(n + align + 8, false);
    if raw.is_null() {
        return raw;
    }
    let p = ((raw as usize + 8 + align - 1) & !(align - 1)) as *mut u8;
    (p.sub(8) as *mut u64).write_unaligned(raw as u64);
    p
}
unsafe extern "win64" fn _aligned_free(p: *mut u8) {
    if !p.is_null() {
        state::heap_free((p.sub(8) as *const u64).read_unaligned() as *mut u8);
    }
}
unsafe extern "win64" fn _aligned_realloc(p: *mut u8, n: usize, align: usize) -> *mut u8 {
    let q = _aligned_malloc(n, align);
    if !p.is_null() && !q.is_null() {
        let raw = (p.sub(8) as *const u64).read_unaligned() as *mut u8;
        let avail = state::heap_size(raw) - (p as usize - raw as usize);
        std::ptr::copy_nonoverlapping(p, q, avail.min(n));
        _aligned_free(p);
    }
    q
}

unsafe fn dup_bytes(b: &[u8]) -> *mut u8 {
    let p = state::heap_alloc(b.len() + 1, false);
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(b.as_ptr(), p, b.len());
        *p.add(b.len()) = 0;
    }
    p
}
unsafe extern "win64" fn _strdup(s: *const u8) -> *mut u8 {
    if s.is_null() {
        std::ptr::null_mut()
    } else {
        dup_bytes(cstr(s))
    }
}
unsafe extern "win64" fn strdup(s: *const u8) -> *mut u8 {
    _strdup(s)
}
unsafe extern "win64" fn _wcsdup(s: *const u16) -> *mut u16 {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let n = wcslen(s) + 1;
    let p = state::heap_alloc(n * 2, false) as *mut u16;
    if !p.is_null() {
        std::ptr::copy_nonoverlapping(s, p, n);
    }
    p
}

// --- FILE ---------------------------------------------------------------------------------------

/// Los `FILE` de `stdin`, `stdout` y `stderr`: tres de 48 bytes seguidos, como el `_iob` de
/// `msvcrt` (MinGW calcula `stdout` como `&_iob[1]`).
struct Iob(UnsafeCell<[u8; 144]>);
// SAFETY: los bytes nunca se leen ni se escriben: solo importan sus direcciones.
unsafe impl Sync for Iob {}
static IOB: Iob = Iob(UnsafeCell::new([0; 144]));

fn std_file(i: usize) -> u64 {
    IOB.0.get() as u64 + i as u64 * 48
}

/// `stdin`, `stdout` y `stderr` de la UCRT: `__acrt_iob_func(0/1/2)`.
unsafe extern "win64" fn __acrt_iob_func(i: u32) -> u64 {
    std_file(i.min(2) as usize)
}

/// Los de `msvcrt`: el arreglo entero (`stdout` es `&__iob_func()[1]`).
unsafe extern "win64" fn __iob_func() -> u64 {
    std_file(0)
}

enum Target {
    Std(u8),
    File(File),
}

struct Stream {
    target: Target,
    wbuf: Vec<u8>,
    rbuf: Vec<u8>,
    rpos: usize,
    eof: bool,
    err: bool,
    /// Modo texto: al leer, `\r\n` → `\n` (al escribir no se agrega `\r`: los archivos de
    /// JARVIS-OS quedan con fines de línea de Linux).
    text: bool,
}

impl Stream {
    fn new(target: Target, text: bool) -> Stream {
        Stream {
            target,
            wbuf: Vec::new(),
            rbuf: Vec::new(),
            rpos: 0,
            eof: false,
            err: false,
            text,
        }
    }

    fn flush(&mut self) -> bool {
        if self.wbuf.is_empty() {
            return true;
        }
        let data = std::mem::take(&mut self.wbuf);
        let ok = match &mut self.target {
            Target::Std(n) => write_console(*n, &data).is_ok(),
            Target::File(f) => f.write_all(&data).is_ok(),
        };
        if !ok {
            self.err = true;
        }
        ok
    }

    fn write(&mut self, data: &[u8]) -> usize {
        if !self.rbuf.is_empty() {
            self.drop_read_buffer();
        }
        match self.target {
            // La Terminal, enseguida (con lo que haya quedado antes).
            Target::Std(_) => {
                self.wbuf.extend_from_slice(data);
                if !self.flush() {
                    return 0;
                }
            }
            Target::File(_) => {
                self.wbuf.extend_from_slice(data);
                if self.wbuf.len() >= 8192 && !self.flush() {
                    return 0;
                }
            }
        }
        data.len()
    }

    /// Vuelve el archivo a donde el programa cree que está (lo leído de más se "devuelve").
    fn drop_read_buffer(&mut self) {
        let extra = (self.rbuf.len() - self.rpos) as i64;
        if let Target::File(f) = &mut self.target
            && extra > 0
        {
            let _ = f.seek(SeekFrom::Current(-extra));
        }
        self.rbuf.clear();
        self.rpos = 0;
    }

    fn fill(&mut self) -> bool {
        if self.rpos < self.rbuf.len() {
            return true;
        }
        if !self.flush() {
            return false;
        }
        // Antes de leer de la Terminal, que se vea lo que se escribió (el "¿Cómo te llamás? ").
        if matches!(self.target, Target::Std(0)) {
            flush_std();
        }
        let mut buf = vec![0u8; 4096];
        let n = match &mut self.target {
            Target::Std(0) => read_handle(STD_IN, &mut buf).unwrap_or(0),
            Target::Std(_) => 0,
            Target::File(f) => f.read(&mut buf).unwrap_or(0),
        };
        buf.truncate(n);
        if self.text {
            buf = strip_cr(&buf);
        }
        self.rbuf = buf;
        self.rpos = 0;
        if self.rbuf.is_empty() {
            self.eof = true;
            return false;
        }
        true
    }

    fn getc(&mut self) -> Option<u8> {
        if !self.fill() {
            return None;
        }
        let c = self.rbuf[self.rpos];
        self.rpos += 1;
        Some(c)
    }

    fn read(&mut self, out: &mut [u8]) -> usize {
        let mut done = 0;
        while done < out.len() {
            if !self.fill() {
                break;
            }
            let n = (self.rbuf.len() - self.rpos).min(out.len() - done);
            out[done..done + n].copy_from_slice(&self.rbuf[self.rpos..self.rpos + n]);
            self.rpos += n;
            done += n;
        }
        done
    }

    fn seek(&mut self, pos: SeekFrom) -> Option<u64> {
        self.flush();
        self.drop_read_buffer();
        self.eof = false;
        match &mut self.target {
            Target::File(f) => f.seek(pos).ok(),
            Target::Std(_) => None,
        }
    }

    fn tell(&mut self) -> Option<u64> {
        let pending = self.wbuf.len() as u64;
        let unread = (self.rbuf.len() - self.rpos) as u64;
        match &mut self.target {
            Target::File(f) => f.stream_position().ok().map(|p| p + pending - unread),
            Target::Std(_) => None,
        }
    }
}

static STREAMS: Mutex<Option<HashMap<u64, Stream>>> = Mutex::new(None);

fn streams() -> MutexGuard<'static, Option<HashMap<u64, Stream>>> {
    let mut g = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        let mut m = HashMap::new();
        for i in 0..3u8 {
            m.insert(std_file(i as usize), Stream::new(Target::Std(i), true));
        }
        *g = Some(m);
    }
    g
}

/// Hace algo con el `FILE*` del programa (o devuelve `bad` si no es uno nuestro).
fn with<R>(f: u64, bad: R, op: impl FnOnce(&mut Stream) -> R) -> R {
    let mut g = streams();
    match g.as_mut().and_then(|m| m.get_mut(&f)) {
        Some(s) => op(s),
        None => {
            set_errno(errno::EBADF);
            bad
        }
    }
}

/// Vacía todos los `FILE` (al terminar, o con `fflush(NULL)`).
pub fn flush_all() {
    if let Ok(mut g) = STREAMS.try_lock()
        && let Some(m) = g.as_mut()
    {
        for s in m.values_mut() {
            s.flush();
        }
    }
}

fn flush_std() {
    // Se llama con STREAMS tomado (desde `fill`): la salida estándar se escribe sin buffer, así
    // que no hay nada pendiente salvo lo que guardó la consola de Rust.
    let _ = std::io::stdout().flush();
}

/// `"r"`, `"w+b"`, `"a"`… → cómo abrir el archivo y si es de texto.
fn open_mode(mode: &str) -> Option<(OpenOptions, bool)> {
    let mut o = OpenOptions::new();
    let plus = mode.contains('+');
    match mode.chars().next()? {
        'r' => {
            o.read(true).write(plus);
        }
        'w' => {
            o.write(true).create(true).truncate(true).read(plus);
        }
        'a' => {
            o.append(true).create(true).read(plus);
        }
        _ => return None,
    }
    let text = !mode.contains('b');
    Some((o, text))
}

fn open_stream(path: &str, mode: &str) -> Result<u64, i32> {
    let (o, text) = open_mode(mode).ok_or(errno::EINVAL)?;
    let file = o.open(win::to_unix(path)).map_err(|e| errno_of(&e))?;
    // Un bloque de 48 bytes por FILE: así cada uno tiene una dirección propia y estable.
    let key = Box::leak(Box::new([0u8; 48])).as_ptr() as u64;
    streams()
        .as_mut()
        .map(|m| m.insert(key, Stream::new(Target::File(file), text)));
    Ok(key)
}

unsafe extern "win64" fn fopen(path: *const u8, mode: *const u8) -> u64 {
    let p = String::from_utf8_lossy(cstr(path)).into_owned();
    let m = String::from_utf8_lossy(cstr(mode)).into_owned();
    open_stream(&p, &m).unwrap_or_else(|e| {
        set_errno(e);
        0
    })
}

unsafe extern "win64" fn _wfopen(path: *const u16, mode: *const u16) -> u64 {
    open_stream(&wstr(path), &wstr(mode)).unwrap_or_else(|e| {
        set_errno(e);
        0
    })
}

unsafe extern "win64" fn fopen_s(out: *mut u64, path: *const u8, mode: *const u8) -> i32 {
    let p = String::from_utf8_lossy(cstr(path)).into_owned();
    let m = String::from_utf8_lossy(cstr(mode)).into_owned();
    match open_stream(&p, &m) {
        Ok(f) => {
            *out = f;
            0
        }
        Err(e) => {
            *out = 0;
            e
        }
    }
}

unsafe extern "win64" fn _wfopen_s(out: *mut u64, path: *const u16, mode: *const u16) -> i32 {
    match open_stream(&wstr(path), &wstr(mode)) {
        Ok(f) => {
            *out = f;
            0
        }
        Err(e) => {
            *out = 0;
            e
        }
    }
}

unsafe extern "win64" fn freopen(path: *const u8, mode: *const u8, f: u64) -> u64 {
    fclose(f);
    fopen(path, mode)
}

unsafe extern "win64" fn fclose(f: u64) -> i32 {
    let mut g = streams();
    let Some(m) = g.as_mut() else { return -1 };
    if f >= std_file(0) && f < std_file(3) {
        // stdin/stdout/stderr no se cierran de verdad.
        return m.get_mut(&f).map_or(-1, |s| if s.flush() { 0 } else { -1 });
    }
    match m.remove(&f) {
        Some(mut s) => {
            if s.flush() {
                0
            } else {
                -1
            }
        }
        None => -1,
    }
}

unsafe extern "win64" fn fflush(f: u64) -> i32 {
    if f == 0 {
        flush_all();
        return 0;
    }
    with(f, -1, |s| if s.flush() { 0 } else { -1 })
}

unsafe extern "win64" fn _flushall() -> i32 {
    flush_all();
    0
}

unsafe extern "win64" fn fgets(buf: *mut u8, n: i32, f: u64) -> *mut u8 {
    if n <= 0 {
        return std::ptr::null_mut();
    }
    let line = with(f, None, |s| {
        let mut out = Vec::new();
        while out.len() + 1 < n as usize {
            match s.getc() {
                Some(c) => {
                    out.push(c);
                    if c == b'\n' {
                        break;
                    }
                }
                None => break,
            }
        }
        Some(out)
    });
    match line {
        Some(l) if !l.is_empty() => {
            std::ptr::copy_nonoverlapping(l.as_ptr(), buf, l.len());
            *buf.add(l.len()) = 0;
            buf
        }
        _ => std::ptr::null_mut(),
    }
}

unsafe extern "win64" fn fgetc(f: u64) -> i32 {
    with(f, -1, |s| s.getc().map_or(-1, |c| c as i32))
}
unsafe extern "win64" fn getc(f: u64) -> i32 {
    fgetc(f)
}
unsafe extern "win64" fn _fgetc_nolock(f: u64) -> i32 {
    fgetc(f)
}
unsafe extern "win64" fn getchar() -> i32 {
    fgetc(std_file(0))
}
unsafe extern "win64" fn ungetc(c: i32, f: u64) -> i32 {
    if c < 0 {
        return -1;
    }
    with(f, -1, |s| {
        s.rbuf.insert(s.rpos, c as u8);
        s.eof = false;
        c
    })
}

fn put(f: u64, data: &[u8]) -> usize {
    with(f, 0, |s| s.write(data))
}

unsafe extern "win64" fn fputc(c: i32, f: u64) -> i32 {
    if put(f, &[c as u8]) == 1 {
        c & 0xff
    } else {
        -1
    }
}
unsafe extern "win64" fn putc(c: i32, f: u64) -> i32 {
    fputc(c, f)
}
unsafe extern "win64" fn _fputc_nolock(c: i32, f: u64) -> i32 {
    fputc(c, f)
}
unsafe extern "win64" fn putchar(c: i32) -> i32 {
    fputc(c, std_file(1))
}
unsafe extern "win64" fn fputs(s: *const u8, f: u64) -> i32 {
    let b = cstr(s);
    if put(f, b) == b.len() { 0 } else { -1 }
}
unsafe extern "win64" fn puts(s: *const u8) -> i32 {
    let mut b = cstr(s).to_vec();
    b.push(b'\n');
    if put(std_file(1), &b) == b.len() {
        0
    } else {
        -1
    }
}
unsafe extern "win64" fn fputws(s: *const u16, f: u64) -> i32 {
    let b = wstr(s);
    if put(f, b.as_bytes()) == b.len() {
        0
    } else {
        -1
    }
}
unsafe extern "win64" fn fputwc(c: u16, f: u64) -> u16 {
    let s = char::from_u32(c as u32).unwrap_or('\u{fffd}').to_string();
    put(f, s.as_bytes());
    c
}
unsafe extern "win64" fn putwchar(c: u16) -> u16 {
    fputwc(c, std_file(1))
}

unsafe extern "win64" fn fwrite(p: *const u8, size: usize, n: usize, f: u64) -> usize {
    let total = size.saturating_mul(n);
    if total == 0 {
        return 0;
    }
    put(f, std::slice::from_raw_parts(p, total)) / size
}
unsafe extern "win64" fn _fwrite_nolock(p: *const u8, size: usize, n: usize, f: u64) -> usize {
    fwrite(p, size, n, f)
}
unsafe extern "win64" fn fread(p: *mut u8, size: usize, n: usize, f: u64) -> usize {
    let total = size.saturating_mul(n);
    if total == 0 {
        return 0;
    }
    let out = std::slice::from_raw_parts_mut(p, total);
    with(f, 0, |s| s.read(out)) / size
}
unsafe extern "win64" fn _fread_nolock(p: *mut u8, size: usize, n: usize, f: u64) -> usize {
    fread(p, size, n, f)
}

fn seek_from(off: i64, whence: i32) -> Option<SeekFrom> {
    Some(match whence {
        0 => SeekFrom::Start(off.max(0) as u64),
        1 => SeekFrom::Current(off),
        2 => SeekFrom::End(off),
        _ => return None,
    })
}

unsafe extern "win64" fn _fseeki64(f: u64, off: i64, whence: i32) -> i32 {
    let Some(pos) = seek_from(off, whence) else {
        set_errno(errno::EINVAL);
        return -1;
    };
    with(f, -1, |s| s.seek(pos).map_or(-1, |_| 0))
}
unsafe extern "win64" fn fseek(f: u64, off: i32, whence: i32) -> i32 {
    _fseeki64(f, off as i64, whence)
}
unsafe extern "win64" fn _ftelli64(f: u64) -> i64 {
    with(f, -1, |s| s.tell().map_or(-1, |p| p as i64))
}
unsafe extern "win64" fn ftell(f: u64) -> i32 {
    _ftelli64(f) as i32
}
unsafe extern "win64" fn rewind(f: u64) {
    _fseeki64(f, 0, 0);
    with(f, (), |s| s.err = false);
}
unsafe extern "win64" fn feof(f: u64) -> i32 {
    with(f, 0, |s| i32::from(s.eof))
}
unsafe extern "win64" fn ferror(f: u64) -> i32 {
    with(f, 0, |s| i32::from(s.err))
}
unsafe extern "win64" fn clearerr(f: u64) {
    with(f, (), |s| {
        s.eof = false;
        s.err = false;
    })
}
unsafe extern "win64" fn _fileno(f: u64) -> i32 {
    with(f, -1, |s| match s.target {
        Target::Std(n) => n as i32,
        Target::File(_) => 3,
    })
}
unsafe extern "win64" fn setvbuf(_f: u64, _b: u64, _mode: i32, _size: usize) -> i32 {
    0
}
unsafe extern "win64" fn setbuf(_f: u64, _b: u64) {}

// Entrada/salida "de bajo nivel" (descriptores de archivo del CRT): solo los estándar.

unsafe extern "win64" fn _write(fd: i32, buf: *const u8, n: u32) -> i32 {
    let data = std::slice::from_raw_parts(buf, n as usize);
    match fd {
        1 | 2 => put(std_file(fd as usize), data) as i32,
        _ => {
            set_errno(errno::EBADF);
            -1
        }
    }
}
unsafe extern "win64" fn _read(fd: i32, buf: *mut u8, n: u32) -> i32 {
    if fd != 0 {
        set_errno(errno::EBADF);
        return -1;
    }
    let out = std::slice::from_raw_parts_mut(buf, n as usize);
    with(std_file(0), -1, |s| s.read(out) as i32)
}
unsafe extern "win64" fn _isatty(fd: i32) -> i32 {
    i32::from((0..3).contains(&fd))
}
unsafe extern "win64" fn _setmode(_fd: i32, _mode: i32) -> i32 {
    0x4000 // _O_TEXT
}
unsafe extern "win64" fn _close(_fd: i32) -> i32 {
    0
}
unsafe extern "win64" fn _get_osfhandle(fd: i32) -> u64 {
    match fd {
        0 => state::STD_IN,
        1 => state::STD_OUT,
        2 => state::STD_ERR,
        _ => state::INVALID_HANDLE,
    }
}
unsafe extern "win64" fn _commit(_fd: i32) -> i32 {
    0
}

fn io_result(r: std::io::Result<()>) -> i32 {
    match r {
        Ok(()) => 0,
        Err(e) => {
            set_errno(errno_of(&e));
            -1
        }
    }
}

fn arg(p: *const u8) -> String {
    // SAFETY: las funciones de archivos reciben rutas terminadas en cero.
    win::to_unix(&String::from_utf8_lossy(unsafe { cstr(p) }))
}

unsafe extern "win64" fn remove(p: *const u8) -> i32 {
    io_result(std::fs::remove_file(arg(p)))
}
unsafe extern "win64" fn _unlink(p: *const u8) -> i32 {
    remove(p)
}
unsafe extern "win64" fn _wremove(p: *const u16) -> i32 {
    io_result(std::fs::remove_file(win::to_unix(&wstr(p))))
}
unsafe extern "win64" fn rename(a: *const u8, b: *const u8) -> i32 {
    io_result(std::fs::rename(arg(a), arg(b)))
}
unsafe extern "win64" fn _wrename(a: *const u16, b: *const u16) -> i32 {
    io_result(std::fs::rename(
        win::to_unix(&wstr(a)),
        win::to_unix(&wstr(b)),
    ))
}
unsafe extern "win64" fn _mkdir(p: *const u8) -> i32 {
    io_result(std::fs::create_dir(arg(p)))
}
unsafe extern "win64" fn _wmkdir(p: *const u16) -> i32 {
    io_result(std::fs::create_dir(win::to_unix(&wstr(p))))
}
unsafe extern "win64" fn _rmdir(p: *const u8) -> i32 {
    io_result(std::fs::remove_dir(arg(p)))
}
unsafe extern "win64" fn _chdir(p: *const u8) -> i32 {
    io_result(std::env::set_current_dir(arg(p)))
}
unsafe extern "win64" fn _getcwd(buf: *mut u8, n: i32) -> *mut u8 {
    let d = std::env::current_dir()
        .map(|p| win::to_windows(&p.to_string_lossy()))
        .unwrap_or_else(|_| r"C:\".into());
    if buf.is_null() {
        return dup_bytes(d.as_bytes());
    }
    if d.len() + 1 > n.max(0) as usize {
        set_errno(errno::ERANGE);
        return std::ptr::null_mut();
    }
    std::ptr::copy_nonoverlapping(d.as_ptr(), buf, d.len());
    *buf.add(d.len()) = 0;
    buf
}
unsafe extern "win64" fn _access(p: *const u8, _mode: i32) -> i32 {
    if std::path::Path::new(&arg(p)).exists() {
        0
    } else {
        set_errno(errno::ENOENT);
        -1
    }
}
unsafe extern "win64" fn _waccess(p: *const u16, _mode: i32) -> i32 {
    if std::path::Path::new(&win::to_unix(&wstr(p))).exists() {
        0
    } else {
        set_errno(errno::ENOENT);
        -1
    }
}
unsafe extern "win64" fn _fullpath(buf: *mut u8, p: *const u8, n: usize) -> *mut u8 {
    let full = kernel32::full_path(&String::from_utf8_lossy(cstr(p)));
    if buf.is_null() {
        return dup_bytes(full.as_bytes());
    }
    if full.len() + 1 > n {
        return std::ptr::null_mut();
    }
    std::ptr::copy_nonoverlapping(full.as_ptr(), buf, full.len());
    *buf.add(full.len()) = 0;
    buf
}
unsafe extern "win64" fn tmpfile() -> u64 {
    let name = format!(
        "vino-tmp-{}-{}",
        std::process::id(),
        kernel32::start().elapsed().as_nanos()
    );
    open_stream(&name, "w+b").unwrap_or(0)
}

// --- printf -------------------------------------------------------------------------------------

/// El formato con sus argumentos.
unsafe fn fmt_va(fmt: *const u8, va: *const u64, wide: bool) -> Vec<u8> {
    printf::format(cstr(fmt), wide, &mut VaList(va))
}

unsafe fn fmt_va_w(fmt: *const u16, va: *const u64) -> Vec<u8> {
    let f = wstr(fmt);
    printf::format(f.as_bytes(), true, &mut VaList(va))
}

fn print_to(f: u64, text: &[u8]) -> i32 {
    if put(f, text) == text.len() {
        text.len() as i32
    } else {
        -1
    }
}

unsafe extern "win64" fn __stdio_common_vfprintf(
    _o: u64,
    f: u64,
    fmt: *const u8,
    _l: u64,
    va: *const u64,
) -> i32 {
    print_to(f, &fmt_va(fmt, va, false))
}

unsafe extern "win64" fn __stdio_common_vfwprintf(
    _o: u64,
    f: u64,
    fmt: *const u16,
    _l: u64,
    va: *const u64,
) -> i32 {
    let t = fmt_va_w(fmt, va);
    if put(f, &t) == t.len() {
        String::from_utf8_lossy(&t).encode_utf16().count() as i32
    } else {
        -1
    }
}

/// Copiar el resultado a un buffer de `count` lugares, como `snprintf` (estándar: corta y
/// devuelve cuánto hacía falta) o como el `_snprintf` de siempre de Microsoft (si no entra, -1,
/// y sin el cero si justo entra).
unsafe fn to_buffer<T: Copy + Default>(
    text: &[T],
    buf: *mut T,
    count: usize,
    standard: bool,
) -> i32 {
    let len = text.len();
    if buf.is_null() || count == 0 {
        return len as i32;
    }
    if len < count {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, len);
        *buf.add(len) = T::default();
        return len as i32;
    }
    if standard {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, count - 1);
        *buf.add(count - 1) = T::default();
        len as i32
    } else {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, count);
        if len == count { len as i32 } else { -1 }
    }
}

/// _CRT_INTERNAL_PRINTF_STANDARD_SNPRINTF_BEHAVIOR
const STANDARD_SNPRINTF: u64 = 2;

unsafe extern "win64" fn __stdio_common_vsprintf(
    o: u64,
    buf: *mut u8,
    count: usize,
    fmt: *const u8,
    _l: u64,
    va: *const u64,
) -> i32 {
    to_buffer(
        &fmt_va(fmt, va, false),
        buf,
        count,
        o & STANDARD_SNPRINTF != 0,
    )
}

unsafe extern "win64" fn __stdio_common_vsprintf_s(
    _o: u64,
    buf: *mut u8,
    count: usize,
    fmt: *const u8,
    _l: u64,
    va: *const u64,
) -> i32 {
    let t = fmt_va(fmt, va, false);
    if t.len() >= count {
        if count > 0 {
            *buf = 0;
        }
        set_errno(errno::ERANGE);
        return -1;
    }
    to_buffer(&t, buf, count, true)
}

unsafe extern "win64" fn __stdio_common_vsnprintf_s(
    _o: u64,
    buf: *mut u8,
    size: usize,
    max: usize,
    fmt: *const u8,
    _l: u64,
    va: *const u64,
) -> i32 {
    let t = fmt_va(fmt, va, false);
    let limit = if max == usize::MAX {
        size
    } else {
        (max + 1).min(size)
    };
    let r = to_buffer(&t, buf, limit, true);
    if t.len() >= limit { -1 } else { r }
}

unsafe extern "win64" fn __stdio_common_vswprintf(
    o: u64,
    buf: *mut u16,
    count: usize,
    fmt: *const u16,
    _l: u64,
    va: *const u64,
) -> i32 {
    let w = win::to_wide(&String::from_utf8_lossy(&fmt_va_w(fmt, va)));
    to_buffer(&w, buf, count, o & STANDARD_SNPRINTF != 0)
}

unsafe extern "win64" fn vprintf(fmt: *const u8, va: *const u64) -> i32 {
    print_to(std_file(1), &fmt_va(fmt, va, false))
}
unsafe extern "win64" fn vfprintf(f: u64, fmt: *const u8, va: *const u64) -> i32 {
    print_to(f, &fmt_va(fmt, va, false))
}
unsafe extern "win64" fn vsprintf(buf: *mut u8, fmt: *const u8, va: *const u64) -> i32 {
    to_buffer(&fmt_va(fmt, va, false), buf, usize::MAX, true)
}
unsafe extern "win64" fn vsnprintf(buf: *mut u8, n: usize, fmt: *const u8, va: *const u64) -> i32 {
    to_buffer(&fmt_va(fmt, va, false), buf, n, true)
}
unsafe extern "win64" fn _vsnprintf(buf: *mut u8, n: usize, fmt: *const u8, va: *const u64) -> i32 {
    to_buffer(&fmt_va(fmt, va, false), buf, n, false)
}
unsafe extern "win64" fn _vscprintf(fmt: *const u8, va: *const u64) -> i32 {
    fmt_va(fmt, va, false).len() as i32
}

/// `printf(fmt, ...)` y compañía de `msvcrt.dll`, con argumentos variables.
///
/// En Windows x86-64 los primeros cuatro argumentos van en registros (rcx, rdx, r8, r9; un
/// `double` variable va **también** en el registro entero) y el que llama deja 32 bytes libres
/// arriba de la dirección de retorno. Si esta función guarda los cuatro registros en esos 32
/// bytes, quedan todos los argumentos seguidos en la pila: eso **es** la `va_list`. Después salta
/// (sin `call`) a la versión `v…`, que vuelve directo al programa.
mod varargs {
    use super::*;

    pub fn exports() -> Vec<(&'static str, usize)> {
        vec![
            ("printf", printf as *const () as usize),
            ("fprintf", fprintf as *const () as usize),
            ("sprintf", sprintf as *const () as usize),
            ("snprintf", snprintf as *const () as usize),
            ("_snprintf", _snprintf as *const () as usize),
        ]
    }

    #[unsafe(naked)]
    unsafe extern "win64" fn printf() {
        core::arch::naked_asm!(
            "mov [rsp + 8], rcx",
            "mov [rsp + 16], rdx",
            "mov [rsp + 24], r8",
            "mov [rsp + 32], r9",
            "lea rdx, [rsp + 16]",
            "jmp {f}",
            f = sym vprintf,
        )
    }

    #[unsafe(naked)]
    unsafe extern "win64" fn fprintf() {
        core::arch::naked_asm!(
            "mov [rsp + 8], rcx",
            "mov [rsp + 16], rdx",
            "mov [rsp + 24], r8",
            "mov [rsp + 32], r9",
            "lea r8, [rsp + 24]",
            "jmp {f}",
            f = sym vfprintf,
        )
    }

    #[unsafe(naked)]
    unsafe extern "win64" fn sprintf() {
        core::arch::naked_asm!(
            "mov [rsp + 8], rcx",
            "mov [rsp + 16], rdx",
            "mov [rsp + 24], r8",
            "mov [rsp + 32], r9",
            "lea r8, [rsp + 24]",
            "jmp {f}",
            f = sym vsprintf,
        )
    }

    #[unsafe(naked)]
    unsafe extern "win64" fn snprintf() {
        core::arch::naked_asm!(
            "mov [rsp + 8], rcx",
            "mov [rsp + 16], rdx",
            "mov [rsp + 24], r8",
            "mov [rsp + 32], r9",
            "lea r9, [rsp + 32]",
            "jmp {f}",
            f = sym vsnprintf,
        )
    }

    #[unsafe(naked)]
    unsafe extern "win64" fn _snprintf() {
        core::arch::naked_asm!(
            "mov [rsp + 8], rcx",
            "mov [rsp + 16], rdx",
            "mov [rsp + 24], r8",
            "mov [rsp + 32], r9",
            "lea r9, [rsp + 32]",
            "jmp {f}",
            f = sym _vsnprintf,
        )
    }
}

// --- Cadenas ------------------------------------------------------------------------------------

unsafe extern "win64" fn strlen(s: *const u8) -> usize {
    cstr(s).len()
}
unsafe extern "win64" fn strnlen(s: *const u8, max: usize) -> usize {
    let mut n = 0;
    while n < max && *s.add(n) != 0 {
        n += 1;
    }
    n
}
unsafe extern "win64" fn strcpy(d: *mut u8, s: *const u8) -> *mut u8 {
    let n = strlen(s);
    std::ptr::copy(s, d, n + 1);
    d
}
unsafe extern "win64" fn strncpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    let len = strnlen(s, n);
    std::ptr::copy(s, d, len);
    std::ptr::write_bytes(d.add(len), 0, n - len);
    d
}
unsafe extern "win64" fn strcat(d: *mut u8, s: *const u8) -> *mut u8 {
    strcpy(d.add(strlen(d)), s);
    d
}
unsafe extern "win64" fn strncat(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    let at = d.add(strlen(d));
    let len = strnlen(s, n);
    std::ptr::copy(s, at, len);
    *at.add(len) = 0;
    d
}
fn cmp_bytes(a: &[u8], b: &[u8]) -> i32 {
    for (x, y) in a
        .iter()
        .chain(std::iter::once(&0))
        .zip(b.iter().chain(std::iter::once(&0)))
    {
        if x != y {
            return *x as i32 - *y as i32;
        }
    }
    0
}
unsafe extern "win64" fn strcmp(a: *const u8, b: *const u8) -> i32 {
    cmp_bytes(cstr(a), cstr(b))
}
unsafe extern "win64" fn strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let (x, y) = (cstr(a), cstr(b));
    cmp_bytes(&x[..x.len().min(n)], &y[..y.len().min(n)])
}
unsafe extern "win64" fn _stricmp(a: *const u8, b: *const u8) -> i32 {
    cmp_bytes(&cstr(a).to_ascii_lowercase(), &cstr(b).to_ascii_lowercase())
}
unsafe extern "win64" fn _strnicmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let (x, y) = (cstr(a).to_ascii_lowercase(), cstr(b).to_ascii_lowercase());
    cmp_bytes(&x[..x.len().min(n)], &y[..y.len().min(n)])
}
unsafe extern "win64" fn _strcmpi(a: *const u8, b: *const u8) -> i32 {
    _stricmp(a, b)
}
unsafe extern "win64" fn stricmp(a: *const u8, b: *const u8) -> i32 {
    _stricmp(a, b)
}
unsafe extern "win64" fn strchr(s: *const u8, c: i32) -> *const u8 {
    let b = cstr(s);
    if c as u8 == 0 {
        return s.add(b.len());
    }
    b.iter()
        .position(|&x| x == c as u8)
        .map_or(std::ptr::null(), |i| s.add(i))
}
unsafe extern "win64" fn strrchr(s: *const u8, c: i32) -> *const u8 {
    let b = cstr(s);
    if c as u8 == 0 {
        return s.add(b.len());
    }
    b.iter()
        .rposition(|&x| x == c as u8)
        .map_or(std::ptr::null(), |i| s.add(i))
}
unsafe extern "win64" fn strstr(h: *const u8, n: *const u8) -> *const u8 {
    let (a, b) = (cstr(h), cstr(n));
    if b.is_empty() {
        return h;
    }
    a.windows(b.len())
        .position(|w| w == b)
        .map_or(std::ptr::null(), |i| h.add(i))
}
unsafe extern "win64" fn strspn(s: *const u8, set: *const u8) -> usize {
    let set = cstr(set);
    cstr(s).iter().take_while(|c| set.contains(c)).count()
}
unsafe extern "win64" fn strcspn(s: *const u8, set: *const u8) -> usize {
    let set = cstr(set);
    cstr(s).iter().take_while(|c| !set.contains(c)).count()
}
unsafe extern "win64" fn strpbrk(s: *const u8, set: *const u8) -> *const u8 {
    let set = cstr(set);
    cstr(s)
        .iter()
        .position(|c| set.contains(c))
        .map_or(std::ptr::null(), |i| s.add(i))
}

/// `strtok_s`: parte `s` por los separadores, guardando por dónde va en `*ctx`.
unsafe extern "win64" fn strtok_s(s: *mut u8, delim: *const u8, ctx: *mut *mut u8) -> *mut u8 {
    let mut p = if s.is_null() { *ctx } else { s };
    if p.is_null() {
        return std::ptr::null_mut();
    }
    p = p.add(strspn(p, delim));
    if *p == 0 {
        *ctx = p;
        return std::ptr::null_mut();
    }
    let end = p.add(strcspn(p, delim));
    if *end == 0 {
        *ctx = end;
    } else {
        *end = 0;
        *ctx = end.add(1);
    }
    p
}
unsafe extern "win64" fn strtok(s: *mut u8, delim: *const u8) -> *mut u8 {
    static CTX: AtomicU64 = AtomicU64::new(0);
    strtok_s(s, delim, CTX.as_ptr() as *mut *mut u8)
}
unsafe extern "win64" fn _strlwr(s: *mut u8) -> *mut u8 {
    let n = strlen(s);
    std::slice::from_raw_parts_mut(s, n).make_ascii_lowercase();
    s
}
unsafe extern "win64" fn _strupr(s: *mut u8) -> *mut u8 {
    let n = strlen(s);
    std::slice::from_raw_parts_mut(s, n).make_ascii_uppercase();
    s
}

unsafe extern "win64" fn strerror(e: i32) -> *const u8 {
    let t: &'static [u8] = match e {
        0 => b"No error\0",
        errno::ENOENT => b"No such file or directory\0",
        errno::EBADF => b"Bad file descriptor\0",
        errno::ENOMEM => b"Not enough space\0",
        errno::EACCES => b"Permission denied\0",
        errno::EEXIST => b"File exists\0",
        errno::EINVAL => b"Invalid argument\0",
        errno::ERANGE => b"Result too large\0",
        errno::ENOTEMPTY => b"Directory not empty\0",
        _ => b"Unknown error\0",
    };
    t.as_ptr()
}
unsafe extern "win64" fn perror(s: *const u8) {
    let msg = cstr(strerror(ERRNO.load(Ordering::Relaxed)));
    let prefix = if s.is_null() { &[][..] } else { cstr(s) };
    let mut t = prefix.to_vec();
    if !t.is_empty() {
        t.extend_from_slice(b": ");
    }
    t.extend_from_slice(msg);
    t.push(b'\n');
    put(std_file(2), &t);
}

unsafe extern "win64" fn strcpy_s(d: *mut u8, n: usize, s: *const u8) -> i32 {
    let len = strlen(s);
    if d.is_null() || len >= n {
        if !d.is_null() && n > 0 {
            *d = 0;
        }
        return errno::ERANGE;
    }
    std::ptr::copy(s, d, len + 1);
    0
}
unsafe extern "win64" fn strcat_s(d: *mut u8, n: usize, s: *const u8) -> i32 {
    let at = strnlen(d, n);
    if at == n {
        return errno::EINVAL;
    }
    strcpy_s(d.add(at), n - at, s)
}
unsafe extern "win64" fn strncpy_s(d: *mut u8, n: usize, s: *const u8, max: usize) -> i32 {
    let len = strnlen(s, max);
    if d.is_null() || n == 0 {
        return errno::EINVAL;
    }
    if len >= n && max != usize::MAX {
        *d = 0;
        return errno::ERANGE;
    }
    let len = len.min(n - 1);
    std::ptr::copy(s, d, len);
    *d.add(len) = 0;
    0
}
unsafe extern "win64" fn memcpy_s(d: *mut u8, n: usize, s: *const u8, count: usize) -> i32 {
    if count > n {
        std::ptr::write_bytes(d, 0, n);
        return errno::ERANGE;
    }
    std::ptr::copy(s, d, count);
    0
}
unsafe extern "win64" fn memmove_s(d: *mut u8, n: usize, s: *const u8, count: usize) -> i32 {
    if count > n {
        return errno::ERANGE;
    }
    std::ptr::copy(s, d, count);
    0
}

unsafe fn wslice<'a>(s: *const u16) -> &'a [u16] {
    std::slice::from_raw_parts(s, wcslen(s))
}
unsafe extern "win64" fn wcslen(s: *const u16) -> usize {
    let mut n = 0;
    while *s.add(n) != 0 {
        n += 1;
    }
    n
}
unsafe extern "win64" fn wcsnlen(s: *const u16, max: usize) -> usize {
    let mut n = 0;
    while n < max && *s.add(n) != 0 {
        n += 1;
    }
    n
}
unsafe extern "win64" fn wcscpy(d: *mut u16, s: *const u16) -> *mut u16 {
    std::ptr::copy(s, d, wcslen(s) + 1);
    d
}
unsafe extern "win64" fn wcsncpy(d: *mut u16, s: *const u16, n: usize) -> *mut u16 {
    let len = wcsnlen(s, n);
    std::ptr::copy(s, d, len);
    std::ptr::write_bytes(d.add(len), 0, n - len);
    d
}
unsafe extern "win64" fn wcscat(d: *mut u16, s: *const u16) -> *mut u16 {
    wcscpy(d.add(wcslen(d)), s);
    d
}
fn cmp_wide(a: &[u16], b: &[u16]) -> i32 {
    for (x, y) in a
        .iter()
        .chain(std::iter::once(&0))
        .zip(b.iter().chain(std::iter::once(&0)))
    {
        if x != y {
            return *x as i32 - *y as i32;
        }
    }
    0
}
unsafe extern "win64" fn wcscmp(a: *const u16, b: *const u16) -> i32 {
    cmp_wide(wslice(a), wslice(b))
}
unsafe extern "win64" fn wcsncmp(a: *const u16, b: *const u16, n: usize) -> i32 {
    let (x, y) = (wslice(a), wslice(b));
    cmp_wide(&x[..x.len().min(n)], &y[..y.len().min(n)])
}
unsafe extern "win64" fn _wcsicmp(a: *const u16, b: *const u16) -> i32 {
    let low = |s: &[u16]| win::to_wide(&win::from_wide(s).to_lowercase());
    cmp_wide(&low(wslice(a)), &low(wslice(b)))
}
unsafe extern "win64" fn wcschr(s: *const u16, c: u16) -> *const u16 {
    let w = wslice(s);
    if c == 0 {
        return s.add(w.len());
    }
    w.iter()
        .position(|&x| x == c)
        .map_or(std::ptr::null(), |i| s.add(i))
}
unsafe extern "win64" fn wcsrchr(s: *const u16, c: u16) -> *const u16 {
    let w = wslice(s);
    if c == 0 {
        return s.add(w.len());
    }
    w.iter()
        .rposition(|&x| x == c)
        .map_or(std::ptr::null(), |i| s.add(i))
}
unsafe extern "win64" fn wcsstr(h: *const u16, n: *const u16) -> *const u16 {
    let (a, b) = (wslice(h), wslice(n));
    if b.is_empty() {
        return h;
    }
    a.windows(b.len())
        .position(|w| w == b)
        .map_or(std::ptr::null(), |i| h.add(i))
}
unsafe extern "win64" fn wcscpy_s(d: *mut u16, n: usize, s: *const u16) -> i32 {
    let len = wcslen(s);
    if len >= n {
        if n > 0 {
            *d = 0;
        }
        return errno::ERANGE;
    }
    std::ptr::copy(s, d, len + 1);
    0
}
unsafe extern "win64" fn wcscat_s(d: *mut u16, n: usize, s: *const u16) -> i32 {
    let at = wcsnlen(d, n);
    if at == n {
        return errno::EINVAL;
    }
    wcscpy_s(d.add(at), n - at, s)
}

// --- Caracteres ---------------------------------------------------------------------------------

fn byte(c: i32) -> Option<u8> {
    (0..=255).contains(&c).then_some(c as u8)
}
unsafe extern "win64" fn isalpha(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_alphabetic()))
}
unsafe extern "win64" fn isdigit(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_digit()))
}
unsafe extern "win64" fn isalnum(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_alphanumeric()))
}
unsafe extern "win64" fn isspace(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_whitespace() || b == 0x0b))
}
unsafe extern "win64" fn isupper(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_uppercase()))
}
unsafe extern "win64" fn islower(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_lowercase()))
}
unsafe extern "win64" fn isxdigit(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_hexdigit()))
}
unsafe extern "win64" fn ispunct(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_punctuation()))
}
unsafe extern "win64" fn isprint(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| (0x20..0x7f).contains(&b)))
}
unsafe extern "win64" fn isgraph(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_graphic()))
}
unsafe extern "win64" fn iscntrl(c: i32) -> i32 {
    i32::from(byte(c).is_some_and(|b| b.is_ascii_control()))
}
unsafe extern "win64" fn isascii(c: i32) -> i32 {
    i32::from((0..128).contains(&c))
}
unsafe extern "win64" fn __isascii(c: i32) -> i32 {
    isascii(c)
}
unsafe extern "win64" fn toupper(c: i32) -> i32 {
    byte(c).map_or(c, |b| b.to_ascii_uppercase() as i32)
}
unsafe extern "win64" fn tolower(c: i32) -> i32 {
    byte(c).map_or(c, |b| b.to_ascii_lowercase() as i32)
}
unsafe extern "win64" fn _toupper(c: i32) -> i32 {
    toupper(c)
}
unsafe extern "win64" fn _tolower(c: i32) -> i32 {
    tolower(c)
}
fn wch(c: u16) -> char {
    char::from_u32(c as u32).unwrap_or('\0')
}
unsafe extern "win64" fn iswalpha(c: u16) -> i32 {
    i32::from(wch(c).is_alphabetic())
}
unsafe extern "win64" fn iswdigit(c: u16) -> i32 {
    i32::from(wch(c).is_ascii_digit())
}
unsafe extern "win64" fn iswspace(c: u16) -> i32 {
    i32::from(wch(c).is_whitespace())
}
unsafe extern "win64" fn iswalnum(c: u16) -> i32 {
    i32::from(wch(c).is_alphanumeric())
}
unsafe extern "win64" fn iswupper(c: u16) -> i32 {
    i32::from(wch(c).is_uppercase())
}
unsafe extern "win64" fn iswlower(c: u16) -> i32 {
    i32::from(wch(c).is_lowercase())
}
unsafe extern "win64" fn towupper(c: u16) -> u16 {
    let u: Vec<u16> = wch(c)
        .to_uppercase()
        .collect::<String>()
        .encode_utf16()
        .collect();
    if u.len() == 1 { u[0] } else { c }
}
unsafe extern "win64" fn towlower(c: u16) -> u16 {
    let u: Vec<u16> = wch(c)
        .to_lowercase()
        .collect::<String>()
        .encode_utf16()
        .collect();
    if u.len() == 1 { u[0] } else { c }
}
unsafe extern "win64" fn isleadbyte(_c: i32) -> i32 {
    0
}

// --- Números ------------------------------------------------------------------------------------

/// `strtol` y compañía: espacios, signo, `0x` (base 16 o 0), dígitos. Devuelve el valor (sin
/// saturar), cuántos bytes usó y si se pasó de 64 bits.
pub fn parse_int(s: &[u8], base: u32) -> (i128, usize, bool) {
    let mut i = s.iter().take_while(|c| c.is_ascii_whitespace()).count();
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut base = base;
    let hex_prefix = s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X'));
    if (base == 16 || base == 0)
        && hex_prefix
        && s.get(i + 2).is_some_and(|c| c.is_ascii_hexdigit())
    {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if s.get(i) == Some(&b'0') { 8 } else { 10 };
    }
    let start = i;
    let mut v: i128 = 0;
    let mut over = false;
    while let Some(d) = s.get(i).and_then(|c| (*c as char).to_digit(base)) {
        v = v * base as i128 + d as i128;
        if v > u64::MAX as i128 {
            over = true;
            v = u64::MAX as i128;
        }
        i += 1;
    }
    if i == start {
        return (0, 0, false);
    }
    (if neg { -v } else { v }, i, over)
}

unsafe fn strto(s: *const u8, end: *mut *const u8, base: i32, min: i128, max: i128) -> i128 {
    let (v, used, over) = parse_int(cstr(s), base as u32);
    if !end.is_null() {
        *end = s.add(used);
    }
    if over || v < min || v > max {
        set_errno(errno::ERANGE);
        return if v < 0 { min } else { max };
    }
    v
}

unsafe extern "win64" fn strtol(s: *const u8, end: *mut *const u8, base: i32) -> i32 {
    strto(s, end, base, i32::MIN as i128, i32::MAX as i128) as i32
}
unsafe extern "win64" fn strtoul(s: *const u8, end: *mut *const u8, base: i32) -> u32 {
    // Como en C, un negativo "da la vuelta".
    strto(s, end, base, -(u32::MAX as i128), u32::MAX as i128) as i64 as u32
}
unsafe extern "win64" fn strtoll(s: *const u8, end: *mut *const u8, base: i32) -> i64 {
    strto(s, end, base, i64::MIN as i128, i64::MAX as i128) as i64
}
unsafe extern "win64" fn strtoull(s: *const u8, end: *mut *const u8, base: i32) -> u64 {
    strto(s, end, base, -(u64::MAX as i128), u64::MAX as i128) as u64
}
unsafe extern "win64" fn _strtoi64(s: *const u8, end: *mut *const u8, base: i32) -> i64 {
    strtoll(s, end, base)
}
unsafe extern "win64" fn _strtoui64(s: *const u8, end: *mut *const u8, base: i32) -> u64 {
    strtoull(s, end, base)
}
unsafe extern "win64" fn atoi(s: *const u8) -> i32 {
    parse_int(cstr(s), 10).0 as i32
}
unsafe extern "win64" fn atol(s: *const u8) -> i32 {
    atoi(s)
}
unsafe extern "win64" fn atoll(s: *const u8) -> i64 {
    parse_int(cstr(s), 10).0 as i64
}
unsafe extern "win64" fn _atoi64(s: *const u8) -> i64 {
    atoll(s)
}

/// `strtod`: el pedazo más largo que es un número (con exponente, `inf` y `nan`).
pub fn parse_float(s: &[u8]) -> (f64, usize) {
    let ws = s.iter().take_while(|c| c.is_ascii_whitespace()).count();
    let t = &s[ws..];
    let mut i = 0;
    if matches!(t.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let lower: Vec<u8> = t
        .iter()
        .take(i + 8)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    for word in ["infinity", "inf", "nan"] {
        if lower[i.min(lower.len())..].starts_with(word.as_bytes()) {
            let n = i + word.len();
            let neg = t.first() == Some(&b'-');
            let v = if word == "nan" {
                f64::NAN
            } else {
                f64::INFINITY
            };
            return (if neg { -v } else { v }, ws + n);
        }
    }
    let digits_start = i;
    while t.get(i).is_some_and(|c| c.is_ascii_digit()) {
        i += 1;
    }
    if t.get(i) == Some(&b'.') {
        i += 1;
        while t.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
    }
    if i == digits_start || (i == digits_start + 1 && t[digits_start] == b'.') {
        return (0.0, 0);
    }
    if matches!(t.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(t.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if t.get(j).is_some_and(|c| c.is_ascii_digit()) {
            while t.get(j).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
            }
            i = j;
        }
    }
    let text = std::str::from_utf8(&t[..i]).unwrap_or("0");
    (text.parse().unwrap_or(0.0), ws + i)
}

unsafe extern "win64" fn strtod(s: *const u8, end: *mut *const u8) -> f64 {
    let (v, used) = parse_float(cstr(s));
    if !end.is_null() {
        *end = s.add(used);
    }
    v
}
unsafe extern "win64" fn strtof(s: *const u8, end: *mut *const u8) -> f32 {
    strtod(s, end) as f32
}
unsafe extern "win64" fn atof(s: *const u8) -> f64 {
    parse_float(cstr(s)).0
}

unsafe fn int_to_text(v: i128, buf: *mut u8, radix: i32) -> *mut u8 {
    let radix = radix.clamp(2, 36) as u32;
    let neg = v < 0 && radix == 10;
    let mut n = if neg { (-v) as u128 } else { v as u128 };
    let mut digits = Vec::new();
    loop {
        digits.push(std::char::from_digit((n % radix as u128) as u32, radix).unwrap_or('0') as u8);
        n /= radix as u128;
        if n == 0 {
            break;
        }
    }
    if neg {
        digits.push(b'-');
    }
    digits.reverse();
    std::ptr::copy_nonoverlapping(digits.as_ptr(), buf, digits.len());
    *buf.add(digits.len()) = 0;
    buf
}
unsafe extern "win64" fn _itoa(v: i32, buf: *mut u8, radix: i32) -> *mut u8 {
    let x = if radix == 10 {
        v as i128
    } else {
        v as u32 as i128
    };
    int_to_text(x, buf, radix)
}
unsafe extern "win64" fn _ltoa(v: i32, buf: *mut u8, radix: i32) -> *mut u8 {
    _itoa(v, buf, radix)
}
unsafe extern "win64" fn _i64toa(v: i64, buf: *mut u8, radix: i32) -> *mut u8 {
    let x = if radix == 10 {
        v as i128
    } else {
        v as u64 as i128
    };
    int_to_text(x, buf, radix)
}
unsafe extern "win64" fn _ui64toa(v: u64, buf: *mut u8, radix: i32) -> *mut u8 {
    int_to_text(v as i128, buf, radix)
}
unsafe extern "win64" fn abs(v: i32) -> i32 {
    v.wrapping_abs()
}
unsafe extern "win64" fn labs(v: i32) -> i32 {
    v.wrapping_abs()
}
unsafe extern "win64" fn llabs(v: i64) -> i64 {
    v.wrapping_abs()
}
unsafe extern "win64" fn _abs64(v: i64) -> i64 {
    v.wrapping_abs()
}

/// `rand()` igual que el de Microsoft (un generador lineal congruente): los programas que usan
/// `srand(n)` obtienen la misma secuencia que en Windows.
static SEED: AtomicU32 = AtomicU32::new(1);
pub fn ms_rand() -> i32 {
    let s = SEED
        .load(Ordering::Relaxed)
        .wrapping_mul(214_013)
        .wrapping_add(2_531_011);
    SEED.store(s, Ordering::Relaxed);
    ((s >> 16) & 0x7fff) as i32
}
unsafe extern "win64" fn rand() -> i32 {
    ms_rand()
}
unsafe extern "win64" fn srand(seed: u32) {
    SEED.store(seed, Ordering::Relaxed);
}
unsafe extern "win64" fn rand_s(out: *mut u32) -> i32 {
    let mut b = [0u8; 4];
    crate::os::random(&mut b);
    *out = u32::from_le_bytes(b);
    0
}

/// `qsort` con la función del programa. Ordenamiento por mezcla propio: el `sort` de Rust
/// aborta si la comparación no es coherente, y un programa de C no lo garantiza.
unsafe extern "win64" fn qsort(base: *mut u8, n: usize, size: usize, cmp: u64) {
    if n < 2 || size == 0 {
        return;
    }
    let cmp: unsafe extern "win64" fn(*const u8, *const u8) -> i32 =
        std::mem::transmute(cmp as usize);
    let items: Vec<Vec<u8>> = (0..n)
        .map(|i| std::slice::from_raw_parts(base.add(i * size), size).to_vec())
        .collect();
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tmp = idx.clone();
    let mut width = 1;
    while width < n {
        let mut i = 0;
        while i < n {
            let mid = (i + width).min(n);
            let end = (i + 2 * width).min(n);
            let (mut a, mut b, mut k) = (i, mid, i);
            while a < mid && b < end {
                if cmp(items[idx[b]].as_ptr(), items[idx[a]].as_ptr()) < 0 {
                    tmp[k] = idx[b];
                    b += 1;
                } else {
                    tmp[k] = idx[a];
                    a += 1;
                }
                k += 1;
            }
            tmp[k..k + (mid - a)].copy_from_slice(&idx[a..mid]);
            k += mid - a;
            tmp[k..k + (end - b)].copy_from_slice(&idx[b..end]);
            i += 2 * width;
        }
        std::mem::swap(&mut idx, &mut tmp);
        width *= 2;
    }
    for (pos, &i) in idx.iter().enumerate() {
        std::ptr::copy_nonoverlapping(items[i].as_ptr(), base.add(pos * size), size);
    }
}

unsafe extern "win64" fn bsearch(
    key: *const u8,
    base: *const u8,
    n: usize,
    size: usize,
    cmp: u64,
) -> *const u8 {
    let cmp: unsafe extern "win64" fn(*const u8, *const u8) -> i32 =
        std::mem::transmute(cmp as usize);
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let p = base.add(mid * size);
        match cmp(key, p) {
            0 => return p,
            c if c < 0 => hi = mid,
            _ => lo = mid + 1,
        }
    }
    std::ptr::null()
}

/// `getenv`: el texto vive hasta que el programa termina (C no dice quién lo libera).
unsafe extern "win64" fn getenv(name: *const u8) -> *const u8 {
    match std::env::var(String::from_utf8_lossy(cstr(name)).as_ref()) {
        Ok(v) => {
            let mut s = state::state();
            let p = s.keep(narrow(&v));
            p as *const u8
        }
        Err(_) => std::ptr::null(),
    }
}
unsafe extern "win64" fn _wgetenv(name: *const u16) -> *const u16 {
    match std::env::var(wstr(name)) {
        Ok(v) => {
            let mut s = state::state();
            s.keep(wide_bytes(&v)) as *const u16
        }
        Err(_) => std::ptr::null(),
    }
}
unsafe extern "win64" fn _putenv(s: *const u8) -> i32 {
    let t = String::from_utf8_lossy(cstr(s)).into_owned();
    match t.split_once('=') {
        Some((k, v)) => {
            kernel32::set_env(k, if v.is_empty() { None } else { Some(v) });
            0
        }
        None => -1,
    }
}
unsafe extern "win64" fn system(cmd: *const u8) -> i32 {
    // Sin intérprete de órdenes de Windows: `system(NULL)` dice "no hay".
    if cmd.is_null() {
        return 0;
    }
    set_errno(errno::ENOENT);
    -1
}

// --- Matemática (la de Rust, que en musl es la libm de musl) ----------------------------------

macro_rules! math1 {
    ($($name:ident => $f:expr),* $(,)?) => {
        $(unsafe extern "win64" fn $name(x: f64) -> f64 { let f: fn(f64) -> f64 = $f; f(x) })*
    };
}
macro_rules! math1f {
    ($($name:ident => $f:expr),* $(,)?) => {
        $(unsafe extern "win64" fn $name(x: f32) -> f32 { let f: fn(f32) -> f32 = $f; f(x) })*
    };
}
math1! {
    sin => f64::sin, cos => f64::cos, tan => f64::tan, asin => f64::asin, acos => f64::acos,
    atan => f64::atan, sinh => f64::sinh, cosh => f64::cosh, tanh => f64::tanh, exp => f64::exp,
    exp2 => f64::exp2, log => f64::ln, log10 => f64::log10, log2 => f64::log2,
    log1p => f64::ln_1p, expm1 => f64::exp_m1, sqrt => f64::sqrt, cbrt => f64::cbrt,
    floor => f64::floor, ceil => f64::ceil, round => f64::round, trunc => f64::trunc,
    fabs => f64::abs, rint => f64::round_ties_even, nearbyint => f64::round_ties_even,
}
math1f! {
    sinf => f32::sin, cosf => f32::cos, tanf => f32::tan, expf => f32::exp, logf => f32::ln,
    log10f => f32::log10, sqrtf => f32::sqrt, floorf => f32::floor, ceilf => f32::ceil,
    roundf => f32::round, truncf => f32::trunc, fabsf => f32::abs,
}
unsafe extern "win64" fn atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}
unsafe extern "win64" fn atan2f(y: f32, x: f32) -> f32 {
    y.atan2(x)
}
unsafe extern "win64" fn pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}
unsafe extern "win64" fn powf(x: f32, y: f32) -> f32 {
    x.powf(y)
}
unsafe extern "win64" fn fmod(x: f64, y: f64) -> f64 {
    x % y
}
unsafe extern "win64" fn fmodf(x: f32, y: f32) -> f32 {
    x % y
}
unsafe extern "win64" fn hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}
unsafe extern "win64" fn _hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}
unsafe extern "win64" fn fmin(x: f64, y: f64) -> f64 {
    x.min(y)
}
unsafe extern "win64" fn fmax(x: f64, y: f64) -> f64 {
    x.max(y)
}
unsafe extern "win64" fn copysign(x: f64, y: f64) -> f64 {
    x.copysign(y)
}
unsafe extern "win64" fn _copysign(x: f64, y: f64) -> f64 {
    x.copysign(y)
}
unsafe extern "win64" fn nan(_tag: *const u8) -> f64 {
    f64::NAN
}
unsafe extern "win64" fn ldexp(x: f64, e: i32) -> f64 {
    x * 2f64.powi(e)
}
unsafe extern "win64" fn frexp(x: f64, e: *mut i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        *e = 0;
        return x;
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // Subnormal: se normaliza multiplicando por 2^64.
        let r = frexp(x * 2f64.powi(64), e);
        *e -= 64;
        return r;
    }
    *e = exp - 1022;
    f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52))
}
unsafe extern "win64" fn modf(x: f64, ip: *mut f64) -> f64 {
    let i = x.trunc();
    *ip = i;
    x - i
}
unsafe extern "win64" fn lround(x: f64) -> i32 {
    x.round() as i32
}
unsafe extern "win64" fn llround(x: f64) -> i64 {
    x.round() as i64
}
unsafe extern "win64" fn _isnan(x: f64) -> i32 {
    i32::from(x.is_nan())
}
unsafe extern "win64" fn _finite(x: f64) -> i32 {
    i32::from(x.is_finite())
}
/// `_dclass`/`_dtest`: FP_NAN 2, FP_INFINITE 1, FP_ZERO 0, FP_SUBNORMAL -2, FP_NORMAL -1.
fn dclass(x: f64) -> i16 {
    use std::num::FpCategory::*;
    match x.classify() {
        Nan => 2,
        Infinite => 1,
        Zero => 0,
        Subnormal => -2,
        Normal => -1,
    }
}
unsafe extern "win64" fn _dclass(x: f64) -> i16 {
    dclass(x)
}
unsafe extern "win64" fn _dtest(x: *const f64) -> i16 {
    dclass(*x)
}
unsafe extern "win64" fn _fdtest(x: *const f32) -> i16 {
    dclass(*x as f64)
}

// --- Hora ---------------------------------------------------------------------------------------

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

unsafe extern "win64" fn _time64(t: *mut i64) -> i64 {
    let s = now_secs();
    if !t.is_null() {
        *t = s;
    }
    s
}
unsafe extern "win64" fn time(t: *mut i64) -> i64 {
    _time64(t)
}
unsafe extern "win64" fn _time32(t: *mut i32) -> i32 {
    let s = now_secs() as i32;
    if !t.is_null() {
        *t = s;
    }
    s
}
/// `clock()`: milisegundos desde que arrancó el programa (CLOCKS_PER_SEC = 1000 en Windows).
unsafe extern "win64" fn clock() -> i32 {
    kernel32::start().elapsed().as_millis() as i32
}

/// (año, mes, día) → días desde 1970 (el inverso de `win::civil`).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `struct tm` (nueve `int`) de unos segundos desde 1970, en UTC.
pub fn tm_of(secs: i64) -> [i32; 9] {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = win::civil(days);
    let yday = days - days_from_civil(y, 1, 1);
    [
        (rem % 60) as i32,
        (rem / 60 % 60) as i32,
        (rem / 3600) as i32,
        d as i32,
        m as i32 - 1,
        (y - 1900) as i32,
        ((days % 7 + 11) % 7) as i32,
        yday as i32,
        0,
    ]
}

/// `struct tm` → segundos (normaliza meses y días fuera de rango, como `mktime`).
pub fn secs_of(tm: &[i32; 9]) -> i64 {
    let mon = tm[4] as i64;
    let y = tm[5] as i64 + 1900 + mon.div_euclid(12);
    let m = mon.rem_euclid(12) as u32 + 1;
    let days = days_from_civil(y, m, 1) + tm[3] as i64 - 1;
    days * 86_400 + tm[2] as i64 * 3600 + tm[1] as i64 * 60 + tm[0] as i64
}

struct Tm(UnsafeCell<[i32; 9]>);
// SAFETY: un solo hilo; es el buffer estático que C dice que devuelve `localtime`.
unsafe impl Sync for Tm {}
static TM: Tm = Tm(UnsafeCell::new([0; 9]));

unsafe extern "win64" fn _localtime64(t: *const i64) -> *mut i32 {
    *TM.0.get() = tm_of(*t);
    TM.0.get() as *mut i32
}
unsafe extern "win64" fn localtime(t: *const i64) -> *mut i32 {
    _localtime64(t)
}
unsafe extern "win64" fn _gmtime64(t: *const i64) -> *mut i32 {
    _localtime64(t)
}
unsafe extern "win64" fn gmtime(t: *const i64) -> *mut i32 {
    _localtime64(t)
}
unsafe extern "win64" fn _localtime64_s(tm: *mut i32, t: *const i64) -> i32 {
    std::ptr::copy_nonoverlapping(tm_of(*t).as_ptr(), tm, 9);
    0
}
unsafe extern "win64" fn _gmtime64_s(tm: *mut i32, t: *const i64) -> i32 {
    _localtime64_s(tm, t)
}
unsafe extern "win64" fn localtime_s(tm: *mut i32, t: *const i64) -> i32 {
    _localtime64_s(tm, t)
}
unsafe extern "win64" fn gmtime_s(tm: *mut i32, t: *const i64) -> i32 {
    _localtime64_s(tm, t)
}
unsafe extern "win64" fn _mktime64(tm: *mut i32) -> i64 {
    let mut v = [0i32; 9];
    std::ptr::copy_nonoverlapping(tm, v.as_mut_ptr(), 9);
    let s = secs_of(&v);
    std::ptr::copy_nonoverlapping(tm_of(s).as_ptr(), tm, 9);
    s
}
unsafe extern "win64" fn mktime(tm: *mut i32) -> i64 {
    _mktime64(tm)
}
unsafe extern "win64" fn _mkgmtime64(tm: *mut i32) -> i64 {
    _mktime64(tm)
}
unsafe extern "win64" fn _difftime64(a: i64, b: i64) -> f64 {
    (a - b) as f64
}
unsafe extern "win64" fn difftime(a: i64, b: i64) -> f64 {
    (a - b) as f64
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `strftime` con las conversiones de siempre (el idioma es el de "C": inglés).
pub fn strftime_text(fmt: &[u8], tm: &[i32; 9]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' || i + 1 >= fmt.len() {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        let mut c = fmt[i + 1];
        i += 2;
        if c == b'#' && i < fmt.len() {
            c = fmt[i];
            i += 1;
        }
        let day = DAYS[tm[6].rem_euclid(7) as usize];
        let month = MONTHS[tm[4].rem_euclid(12) as usize];
        let t = match c {
            b'Y' => format!("{}", tm[5] + 1900),
            b'y' => format!("{:02}", (tm[5] + 1900) % 100),
            b'm' => format!("{:02}", tm[4] + 1),
            b'd' => format!("{:02}", tm[3]),
            b'e' => format!("{:2}", tm[3]),
            b'H' => format!("{:02}", tm[2]),
            b'I' => format!("{:02}", (tm[2] + 11) % 12 + 1),
            b'M' => format!("{:02}", tm[1]),
            b'S' => format!("{:02}", tm[0]),
            b'j' => format!("{:03}", tm[7] + 1),
            b'p' => (if tm[2] < 12 { "AM" } else { "PM" }).into(),
            b'a' => day[..3].into(),
            b'A' => day.into(),
            b'b' | b'h' => month[..3].into(),
            b'B' => month.into(),
            b'w' => format!("{}", tm[6]),
            b'c' => format!(
                "{:02}/{:02}/{:02} {:02}:{:02}:{:02}",
                tm[4] + 1,
                tm[3],
                (tm[5] + 1900) % 100,
                tm[2],
                tm[1],
                tm[0]
            ),
            b'x' | b'D' => format!("{:02}/{:02}/{:02}", tm[4] + 1, tm[3], (tm[5] + 1900) % 100),
            b'X' | b'T' => format!("{:02}:{:02}:{:02}", tm[2], tm[1], tm[0]),
            b'F' => format!("{}-{:02}-{:02}", tm[5] + 1900, tm[4] + 1, tm[3]),
            b'z' => "+0000".into(),
            b'Z' => "UTC".into(),
            b'n' => "\n".into(),
            b't' => "\t".into(),
            b'%' => "%".into(),
            other => format!("%{}", other as char),
        };
        out.extend_from_slice(t.as_bytes());
    }
    out
}

unsafe extern "win64" fn strftime(
    buf: *mut u8,
    max: usize,
    fmt: *const u8,
    tm: *const i32,
) -> usize {
    let mut v = [0i32; 9];
    std::ptr::copy_nonoverlapping(tm, v.as_mut_ptr(), 9);
    let t = strftime_text(cstr(fmt), &v);
    if t.len() + 1 > max {
        return 0;
    }
    std::ptr::copy_nonoverlapping(t.as_ptr(), buf, t.len());
    *buf.add(t.len()) = 0;
    t.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeros_como_strtol_y_strtod() {
        assert_eq!(parse_int(b"  -42xyz", 10), (-42, 5, false));
        assert_eq!(parse_int(b"0x1F", 0), (31, 4, false));
        assert_eq!(parse_int(b"017", 0), (15, 3, false));
        assert_eq!(parse_int(b"zz", 36), (35 * 36 + 35, 2, false));
        assert_eq!(parse_int(b"abc", 10), (0, 0, false));
        assert!(parse_int(b"99999999999999999999999", 10).2);
        assert_eq!(parse_float(b" 3.5e2 resto"), (350.0, 6));
        assert_eq!(parse_float(b"-.5"), (-0.5, 3));
        assert_eq!(parse_float(b"1e"), (1.0, 1));
        assert!(parse_float(b"-inf").0.is_infinite());
        assert_eq!(parse_float(b"."), (0.0, 0));
    }

    #[test]
    fn rand_como_el_de_microsoft() {
        // srand(1) en Windows: 41, 18467, 6334, 26500…
        SEED.store(1, Ordering::Relaxed);
        assert_eq!(
            [ms_rand(), ms_rand(), ms_rand(), ms_rand()],
            [41, 18467, 6334, 26500]
        );
    }

    #[test]
    fn hora_de_c() {
        let s = 1_792_499_696; // 2026-10-20 12:34:56 UTC, martes
        let tm = tm_of(s);
        assert_eq!(tm, [56, 34, 12, 20, 9, 126, 2, 292, 0]);
        assert_eq!(secs_of(&tm), s);
        // Mes 12 = enero del año siguiente.
        assert_eq!(tm_of(secs_of(&[0, 0, 0, 1, 12, 125, 0, 0, 0]))[5], 126);
        assert_eq!(
            strftime_text(b"%Y-%m-%d %H:%M:%S %a %B %j %%", &tm),
            b"2026-10-20 12:34:56 Tue October 293 %"
        );
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
    }

    #[test]
    fn buffers_de_snprintf() {
        let mut b = [0xffu8; 4];
        // SAFETY: `b` tiene 4 lugares.
        unsafe {
            assert_eq!(to_buffer(b"hola", b.as_mut_ptr(), 4, true), 4);
            assert_eq!(&b, b"hol\0");
            assert_eq!(
                to_buffer(b"hola", b.as_mut_ptr(), 4, false),
                4,
                "justo: sin el cero"
            );
            assert_eq!(&b, b"hola");
            assert_eq!(to_buffer(b"chau!", b.as_mut_ptr(), 4, false), -1);
            assert_eq!(to_buffer(b"ok", b.as_mut_ptr(), 4, false), 2);
            assert_eq!(&b[..3], b"ok\0");
            assert_eq!(to_buffer(b"cuanto", std::ptr::null_mut(), 0, true), 6);
        }
    }

    #[test]
    fn archivos_de_c() {
        let path = "target/vino-prueba-crt.txt";
        let p = narrow(path);
        // SAFETY: textos terminados en cero.
        unsafe {
            let f = fopen(p.as_ptr(), c"w".as_ptr().cast());
            assert_ne!(f, 0);
            assert_eq!(fputs(c"linea 1\n".as_ptr().cast(), f), 0);
            assert_eq!(fwrite(b"linea 2\r\n".as_ptr(), 1, 9, f), 9);
            assert_eq!(_ftelli64(f), 17);
            assert_eq!(fclose(f), 0);
            let f = fopen(p.as_ptr(), c"r".as_ptr().cast());
            let mut line = [0u8; 64];
            let mut lines = Vec::new();
            while !fgets(line.as_mut_ptr(), 64, f).is_null() {
                lines.push(String::from_utf8_lossy(cstr(line.as_ptr())).into_owned());
            }
            assert_eq!(lines, ["linea 1\n", "linea 2\n"], "modo texto: sin \\r");
            assert_eq!(feof(f), 1);
            assert_eq!(fseek(f, 2, 0), 0);
            assert_eq!(fgetc(f), b'n' as i32);
            assert_eq!(ungetc(b'N' as i32, f), b'N' as i32);
            assert_eq!(fgetc(f), b'N' as i32);
            fclose(f);
            assert!(fopen(c"target/no-existe/x".as_ptr().cast(), c"r".as_ptr().cast()) == 0);
            assert_eq!(ERRNO.load(Ordering::Relaxed), errno::ENOENT);
            remove(p.as_ptr());
        }
    }

    #[test]
    fn cadenas_de_c() {
        let mut buf = [0u8; 32];
        // SAFETY: textos terminados en cero y buffers grandes.
        unsafe {
            strcpy(buf.as_mut_ptr(), c"memoria".as_ptr().cast());
            strcat(buf.as_mut_ptr(), c" ok".as_ptr().cast());
            assert_eq!(cstr(buf.as_ptr()), b"memoria ok");
            assert_eq!(
                strcspn(c"Roman\r\n".as_ptr().cast(), c"\r\n".as_ptr().cast()),
                5
            );
            assert!(strcmp(c"a".as_ptr().cast(), c"b".as_ptr().cast()) < 0);
            assert_eq!(
                _stricmp(c"HoLa".as_ptr().cast(), c"hola".as_ptr().cast()),
                0
            );
            let s = strstr(c"jarvis-os".as_ptr().cast(), c"os".as_ptr().cast());
            assert_eq!(cstr(s), b"os");
            let mut t = *b"a,b,,c\0";
            let mut ctx = std::ptr::null_mut();
            let mut parts = Vec::new();
            let mut p = strtok_s(t.as_mut_ptr(), c",".as_ptr().cast(), &mut ctx);
            while !p.is_null() {
                parts.push(String::from_utf8_lossy(cstr(p)).into_owned());
                p = strtok_s(std::ptr::null_mut(), c",".as_ptr().cast(), &mut ctx);
            }
            assert_eq!(parts, ["a", "b", "c"]);
            let mut n = [0u8; 24];
            _i64toa(-1234, n.as_mut_ptr(), 10);
            assert_eq!(cstr(n.as_ptr()), b"-1234");
            _itoa(255, n.as_mut_ptr(), 16);
            assert_eq!(cstr(n.as_ptr()), b"ff");
        }
    }

    #[test]
    fn qsort_ordena_con_la_funcion_del_programa() {
        unsafe extern "win64" fn cmp(a: *const u8, b: *const u8) -> i32 {
            // SAFETY: los punteros son de enteros del arreglo.
            unsafe { (a as *const i32).read_unaligned() - (b as *const i32).read_unaligned() }
        }
        let mut v = [5i32, -1, 3, 3, 0, 9, 2];
        // SAFETY: `v` tiene 7 enteros de 4 bytes y `cmp` los compara.
        unsafe {
            qsort(v.as_mut_ptr() as *mut u8, 7, 4, cmp as *const () as u64);
            assert_eq!(v, [-1, 0, 2, 3, 3, 5, 9]);
            let key = 5i32;
            let p = bsearch(
                &key as *const i32 as *const u8,
                v.as_ptr() as *const u8,
                7,
                4,
                cmp as *const () as u64,
            );
            assert_eq!(p, v.as_ptr().add(5) as *const u8);
        }
    }

    #[test]
    fn frexp_y_ldexp() {
        let mut e = 0;
        // SAFETY: `e` es un entero válido.
        unsafe {
            assert_eq!(frexp(8.0, &mut e), 0.5);
            assert_eq!(e, 4);
            assert_eq!(ldexp(0.5, 4), 8.0);
        }
    }
}
