//! Hilos de Windows sobre hilos de Linux (`clone` en el kernel de JARVIS-OS, K15).
//!
//! `CreateThread` crea un hilo de Rust (que es un `pthread` de musl, o sea un `clone`) y, ya
//! adentro, le arma lo que un hilo de Windows espera:
//!
//! - su **TEB** en GS (con su pila, su último error, sus `TlsSlots`) que apunta al mismo PEB;
//! - su copia del **TLS** de cada módulo (`__declspec(thread)`);
//! - el aviso a las DLL: `DllMain(DLL_THREAD_ATTACH)` y los callbacks de TLS (Rust y el CRT los
//!   usan para inicializar y para los destructores de las variables por hilo).
//!
//! Al terminar, lo mismo con `DLL_THREAD_DETACH`, los destructores de FLS, y el HANDLE del hilo
//! queda "señalado" para quien lo espere con `WaitForSingleObject`.

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use super::crt::{errno, set_errno};
use super::sync::{self, SyncObj};
use crate::modules;
use crate::os;
use crate::state::{self, CURRENT_THREAD};
use crate::teb;

type Bool = i32;
const DLL_THREAD_ATTACH: u32 = 2;
const DLL_THREAD_DETACH: u32 = 3;
const CREATE_SUSPENDED: u32 = 4;

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    exports![
        CreateThread,
        ExitThread,
        ResumeThread,
        GetExitCodeThread,
        GetCurrentThreadId,
        GetThreadId,
        FlsAlloc,
        FlsFree,
        FlsGetValue,
        FlsSetValue,
        GetCurrentThreadStackLimits,
    ]
}

pub fn crt_exports() -> Vec<(&'static str, usize)> {
    exports![_beginthreadex, _endthreadex, _beginthread, _endthread]
}

/// El PEB del proceso (los TEB de todos los hilos apuntan a él) y el bloque que armó `run`.
static PEB: AtomicU64 = AtomicU64::new(0);
static STACK_DEFAULT: AtomicU64 = AtomicU64::new(8 << 20);

pub fn set_process(peb: u64, stack_reserve: u64) {
    PEB.store(peb, Ordering::Relaxed);
    STACK_DEFAULT.store(stack_reserve.clamp(1 << 20, 64 << 20), Ordering::Relaxed);
}

thread_local! {
    /// Los límites de la pila de este hilo (para el TEB y `GetCurrentThreadStackLimits`).
    static STACK: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    /// El HANDLE de este hilo (0 en el principal) y su código de salida.
    static ME: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Los valores de FLS de este hilo.
    static FLS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Cómo arranca el hilo: `DWORD WINAPI f(void*)` (CreateThread, _beginthreadex) o
/// `void __cdecl f(void*)` (_beginthread). En x64 la convención es la misma.
#[derive(Clone, Copy)]
enum Kind {
    ReturnsCode,
    Void,
}

struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
}

/// Lo que queda abierto mientras el hilo esté suspendido (`CREATE_SUSPENDED`).
static SUSPENDED: Mutex<Vec<(u64, Arc<Gate>)>> = Mutex::new(Vec::new());

/// Arma el TEB de este hilo y lo pone en GS.
fn setup_teb(stack_size: u64) {
    let marker = 0u8;
    let sp = &marker as *const u8 as u64;
    let base = (sp + 4096).next_multiple_of(4096);
    let limit = base.saturating_sub(stack_size);
    STACK.with(|s| s.set((limit, base)));
    let block = Box::leak(vec![0u64; teb::BLOCK_SIZE / 8].into_boxed_slice());
    let at = block.as_ptr() as u64;
    // SAFETY: el bloque es de `BLOCK_SIZE` bytes y vive para siempre (leak).
    let bytes =
        unsafe { std::slice::from_raw_parts_mut(block.as_mut_ptr() as *mut u8, teb::BLOCK_SIZE) };
    let tid = os::gettid() as u64;
    let a = teb::fill(
        bytes,
        at,
        &teb::Info {
            image_base: state::state().image_base,
            stack_base: base,
            stack_limit: limit,
            pid: std::process::id() as u64,
            tid,
            tls_array: modules::new_thread_tls(),
            heap: state::PROCESS_HEAP,
            std_handles: [state::STD_IN, state::STD_OUT, state::STD_ERR],
            image_path: "",
            command_line: "",
            current_dir: "",
            environment: 0,
        },
    );
    // El PEB es uno solo, el del proceso.
    let peb = PEB.load(Ordering::Relaxed);
    if peb != 0 {
        bytes[teb::off::TEB_PEB..teb::off::TEB_PEB + 8].copy_from_slice(&peb.to_le_bytes());
    }
    os::set_gs(a.teb);
    state::set_teb(a.teb);
}

/// Lo que hace un hilo al terminar (normalmente o con `ExitThread`).
fn finish(code: u32) {
    modules::thread_notify(DLL_THREAD_DETACH);
    run_fls_callbacks();
    let h = ME.with(|m| m.get());
    if h != 0 {
        if let Some(SyncObj::Thread { done, code: c, .. }) =
            sync::lock().as_mut().and_then(|s| s.objects.get_mut(&h))
        {
            *done = true;
            *c = code;
        }
        sync::notify();
    }
}

/// Crea el hilo. Devuelve su HANDLE y su número.
fn create(start: u64, param: u64, stack: usize, flags: u32, kind: Kind) -> Option<(u64, u32)> {
    let h = sync::new_object(SyncObj::Thread {
        done: false,
        code: 259, // STILL_ACTIVE
        tid: 0,
    });
    let gate = Arc::new(Gate {
        open: Mutex::new(flags & CREATE_SUSPENDED == 0),
        cv: Condvar::new(),
    });
    if flags & CREATE_SUSPENDED != 0 {
        SUSPENDED.lock().ok()?.push((h, Arc::clone(&gate)));
    }
    let size = if stack == 0 {
        STACK_DEFAULT.load(Ordering::Relaxed)
    } else {
        (stack as u64).max(256 << 10)
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .stack_size(size as usize)
        .spawn(move || {
            let tid = os::gettid();
            let _ = tx.send(tid);
            ME.with(|m| m.set(h));
            {
                let mut open = gate.open.lock().unwrap_or_else(|e| e.into_inner());
                while !*open {
                    open = gate.cv.wait(open).unwrap_or_else(|e| e.into_inner());
                }
            }
            setup_teb(size);
            modules::thread_notify(DLL_THREAD_ATTACH);
            let code = match kind {
                Kind::ReturnsCode => {
                    // SAFETY: la función de arranque que pasó el programa.
                    let f: unsafe extern "win64" fn(u64) -> u32 =
                        unsafe { std::mem::transmute(start as usize) };
                    // SAFETY: ídem.
                    unsafe { f(param) }
                }
                Kind::Void => {
                    // SAFETY: ídem, la de `_beginthread`.
                    let f: unsafe extern "win64" fn(u64) =
                        unsafe { std::mem::transmute(start as usize) };
                    // SAFETY: ídem.
                    unsafe { f(param) };
                    0
                }
            };
            finish(code);
        });
    if spawned.is_err() {
        sync::forget(h);
        state::state().close(h);
        return None;
    }
    let tid = rx.recv().unwrap_or(0);
    if let Some(SyncObj::Thread { tid: t, .. }) =
        sync::lock().as_mut().and_then(|s| s.objects.get_mut(&h))
    {
        *t = tid;
    }
    Some((h, tid))
}

unsafe extern "win64" fn CreateThread(
    _sa: u64,
    stack: usize,
    start: u64,
    param: u64,
    flags: u32,
    tid: *mut u32,
) -> u64 {
    match create(start, param, stack, flags, Kind::ReturnsCode) {
        Some((h, t)) => {
            if !tid.is_null() {
                *tid = t;
            }
            h
        }
        None => {
            state::set_last_error(8); // ERROR_NOT_ENOUGH_MEMORY
            0
        }
    }
}

/// `ExitThread`: termina el hilo actual sin volver a su función (ni a nada de Rust de abajo).
unsafe extern "win64" fn ExitThread(code: u32) -> ! {
    if ME.with(|m| m.get()) == 0 {
        // El hilo principal: en vino, eso termina el programa.
        super::exit(code as i32)
    }
    finish(code);
    os::thread_exit()
}

unsafe extern "win64" fn ResumeThread(h: u64) -> u32 {
    let gate = SUSPENDED.lock().ok().and_then(|mut v| {
        let i = v.iter().position(|(x, _)| *x == h)?;
        Some(v.remove(i).1)
    });
    match gate {
        Some(g) => {
            *g.open.lock().unwrap_or_else(|e| e.into_inner()) = true;
            g.cv.notify_all();
            1 // estaba suspendido una vez
        }
        None => 0,
    }
}

unsafe extern "win64" fn GetExitCodeThread(h: u64, code: *mut u32) -> Bool {
    match sync::lock().as_ref().and_then(|s| s.objects.get(&h)) {
        Some(SyncObj::Thread { code: c, .. }) => {
            *code = *c;
            1
        }
        _ => {
            state::set_last_error(crate::win::error::INVALID_HANDLE);
            0
        }
    }
}

unsafe extern "win64" fn GetCurrentThreadId() -> u32 {
    os::gettid()
}

unsafe extern "win64" fn GetThreadId(h: u64) -> u32 {
    if h == CURRENT_THREAD {
        return os::gettid();
    }
    match sync::lock().as_ref().and_then(|s| s.objects.get(&h)) {
        Some(SyncObj::Thread { tid, .. }) => *tid,
        _ => 0,
    }
}

unsafe extern "win64" fn GetCurrentThreadStackLimits(low: *mut u64, high: *mut u64) {
    let (mut l, mut h) = STACK.with(|s| s.get());
    if h == 0 {
        // El hilo principal: lo dice su TEB.
        let t = state::teb();
        if t != 0 {
            let p = t as *const u64;
            h = *p.add(1);
            l = *p.add(2);
        }
    }
    *low = l;
    *high = h;
}

/// Para `run`: los límites de la pila del hilo principal.
pub fn set_main_stack(limit: u64, base: u64) {
    STACK.with(|s| s.set((limit, base)));
}

// --- _beginthreadex y compañía (el CRT) --------------------------------------------------------

unsafe extern "win64" fn _beginthreadex(
    _sa: u64,
    stack: u32,
    start: u64,
    arg: u64,
    flags: u32,
    tid: *mut u32,
) -> u64 {
    match create(start, arg, stack as usize, flags, Kind::ReturnsCode) {
        Some((h, t)) => {
            if !tid.is_null() {
                *tid = t;
            }
            h
        }
        None => {
            set_errno(11); // EAGAIN
            0
        }
    }
}

unsafe extern "win64" fn _endthreadex(code: u32) -> ! {
    ExitThread(code)
}

/// `_beginthread`: el HANDLE se cierra solo cuando el hilo termina (devuelve -1 si falla).
unsafe extern "win64" fn _beginthread(start: u64, stack: u32, arg: u64) -> i64 {
    match create(start, arg, stack as usize, 0, Kind::Void) {
        Some((h, _)) => h as i64,
        None => {
            set_errno(errno::EINVAL);
            -1
        }
    }
}

unsafe extern "win64" fn _endthread() -> ! {
    ExitThread(0)
}

// --- FLS: como TLS, pero con un destructor por índice -------------------------------------------

/// Los índices pedidos y su destructor (0 si no tiene).
static FLS_SLOTS: Mutex<Vec<Option<u64>>> = Mutex::new(Vec::new());

unsafe extern "win64" fn FlsAlloc(callback: u64) -> u32 {
    let mut v = FLS_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    match v.iter().position(Option::is_none) {
        Some(i) => {
            v[i] = Some(callback);
            i as u32
        }
        None => {
            v.push(Some(callback));
            (v.len() - 1) as u32
        }
    }
}

fn fls_valid(i: u32) -> bool {
    FLS_SLOTS
        .lock()
        .map(|v| v.get(i as usize).is_some_and(Option::is_some))
        .unwrap_or(false)
}

unsafe extern "win64" fn FlsFree(i: u32) -> Bool {
    let mut v = FLS_SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    match v.get_mut(i as usize) {
        Some(s @ Some(_)) => {
            *s = None;
            1
        }
        _ => {
            state::set_last_error(crate::win::error::INVALID_PARAMETER);
            0
        }
    }
}

unsafe extern "win64" fn FlsGetValue(i: u32) -> u64 {
    if !fls_valid(i) {
        state::set_last_error(crate::win::error::INVALID_PARAMETER);
        return 0;
    }
    state::set_last_error(0);
    FLS.with(|f| f.borrow().get(i as usize).copied().unwrap_or(0))
}

unsafe extern "win64" fn FlsSetValue(i: u32, v: u64) -> Bool {
    if !fls_valid(i) {
        state::set_last_error(crate::win::error::INVALID_PARAMETER);
        return 0;
    }
    FLS.with(|f| {
        let mut f = f.borrow_mut();
        if f.len() <= i as usize {
            f.resize(i as usize + 1, 0);
        }
        f[i as usize] = v;
    });
    1
}

/// Al terminar un hilo: el destructor de cada valor de FLS que no sea nulo.
fn run_fls_callbacks() {
    let values = FLS.with(|f| std::mem::take(&mut *f.borrow_mut()));
    let callbacks: Vec<Option<u64>> = FLS_SLOTS.lock().map(|v| v.clone()).unwrap_or_default();
    for (i, v) in values.into_iter().enumerate() {
        if let (Some(Some(cb)), true) = (callbacks.get(i), v != 0)
            && *cb != 0
        {
            // SAFETY: el destructor que pasó el programa a FlsAlloc: `void WINAPI f(void*)`.
            let f: unsafe extern "win64" fn(u64) = unsafe { std::mem::transmute(*cb as usize) };
            // SAFETY: ídem.
            unsafe { f(v) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fls_es_de_cada_hilo() {
        // SAFETY: índices recién pedidos.
        unsafe {
            let i = FlsAlloc(0);
            assert_eq!(FlsSetValue(i, 42), 1);
            assert_eq!(FlsGetValue(i), 42);
            let other = std::thread::spawn(move || FlsGetValue(i)).join().unwrap();
            assert_eq!(other, 0, "otro hilo no ve el valor");
            assert_eq!(FlsFree(i), 1);
            assert_eq!(FlsGetValue(i), 0);
            assert_eq!(state::last_error(), crate::win::error::INVALID_PARAMETER);
        }
    }

    #[test]
    fn hilos_de_windows_en_el_anfitrion() {
        unsafe extern "win64" fn doble(p: u64) -> u32 {
            (p * 2) as u32
        }
        // SAFETY: `doble` es una función `win64` de verdad.
        unsafe {
            let mut tid = 0;
            let h = CreateThread(
                0,
                0,
                doble as *const () as u64,
                21,
                CREATE_SUSPENDED,
                &mut tid,
            );
            assert_ne!(h, 0);
            let mut code = 0;
            GetExitCodeThread(h, &mut code);
            assert_eq!(code, 259, "suspendido: todavía no corrió");
            assert_eq!(ResumeThread(h), 1);
            let w: unsafe extern "win64" fn(u64, u32) -> u32 = std::mem::transmute(
                super::super::find(super::super::Dll::Kernel32, "WaitForSingleObject").unwrap(),
            );
            assert_eq!(w(h, 5000), 0);
            GetExitCodeThread(h, &mut code);
            assert_eq!(code, 42);
        }
    }
}
