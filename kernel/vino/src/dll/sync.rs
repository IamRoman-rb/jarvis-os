//! La sincronización de Windows entre hilos: secciones críticas, SRW, variables de condición,
//! `WaitOnAddress`, `InitOnce`, eventos, mutex, semáforos, y esperar HANDLE (`WaitFor…Object`).
//!
//! Todo pasa por un solo `Mutex` + `Condvar` de Rust ([`SYNC`] y [`CV`]), que en JARVIS-OS son
//! un `futex` del kernel. Cada operación mira o cambia el estado con el lock tomado y, si tiene
//! que esperar, duerme en la `Condvar`; el que cambia algo despierta a todos y cada uno vuelve a
//! mirar si ya puede seguir. Es más simple que una cola por objeto (como hace Windows) y alcanza:
//! lo caro es dormir y despertar, y eso lo hace el kernel igual.
//!
//! Los objetos que viven en la memoria del programa (CRITICAL_SECTION, SRWLOCK…) se leen y
//! escriben siempre con el lock tomado, así nadie los ve a medio cambiar.

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::state::{self, Object, state};
use crate::win::error;

type Bool = i32;
const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const WAIT_FAILED: u32 = u32::MAX;
const ERROR_TIMEOUT: u32 = 1460;

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    exports![
        InitializeCriticalSection,
        InitializeCriticalSectionEx,
        InitializeCriticalSectionAndSpinCount,
        EnterCriticalSection,
        LeaveCriticalSection,
        TryEnterCriticalSection,
        DeleteCriticalSection,
        SetCriticalSectionSpinCount,
        InitializeSRWLock,
        AcquireSRWLockExclusive,
        AcquireSRWLockShared,
        ReleaseSRWLockExclusive,
        ReleaseSRWLockShared,
        TryAcquireSRWLockExclusive,
        TryAcquireSRWLockShared,
        InitializeConditionVariable,
        SleepConditionVariableSRW,
        SleepConditionVariableCS,
        WakeConditionVariable,
        WakeAllConditionVariable,
        InitOnceInitialize,
        InitOnceBeginInitialize,
        InitOnceComplete,
        InitOnceExecuteOnce,
        WaitOnAddress,
        WakeByAddressSingle,
        WakeByAddressAll,
        CreateMutexA,
        CreateMutexW,
        CreateMutexExW,
        ReleaseMutex,
        CreateEventA,
        CreateEventW,
        CreateEventExW,
        SetEvent,
        ResetEvent,
        PulseEvent,
        CreateSemaphoreW,
        CreateSemaphoreA,
        CreateSemaphoreExW,
        ReleaseSemaphore,
        WaitForSingleObject,
        WaitForSingleObjectEx,
        WaitForMultipleObjects,
        WaitForMultipleObjectsEx,
        SignalObjectAndWait,
        SwitchToThread,
        Sleep,
        SleepEx,
        CreateWaitableTimerExW,
        CreateWaitableTimerW,
        SetWaitableTimerEx,
        SetWaitableTimer,
        CancelWaitableTimer,
    ]
}

/// Los objetos de sincronización con HANDLE (el número lo da `state`, como todos).
#[derive(Debug)]
pub enum SyncObj {
    Event {
        manual: bool,
        signaled: bool,
    },
    Mutex {
        owner: u32,
        count: u32,
    },
    Semaphore {
        count: i32,
        max: i32,
    },
    /// Un hilo: se "señala" cuando termina.
    Thread {
        done: bool,
        code: u32,
        tid: u32,
    },
    /// Un "waitable timer": se señala cuando llega `due` (y vuelve a armarse si es periódico).
    Timer {
        manual: bool,
        due: Option<Instant>,
        period: Option<Duration>,
        signaled: bool,
    },
}

#[derive(Default)]
pub struct Sync {
    pub objects: HashMap<u64, SyncObj>,
}

static SYNC: Mutex<Option<Sync>> = Mutex::new(None);
static CV: Condvar = Condvar::new();

pub fn lock() -> MutexGuard<'static, Option<Sync>> {
    let mut g = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(Sync::default());
    }
    g
}

/// Despierta a todos los que esperan: cada uno vuelve a mirar.
pub fn notify() {
    CV.notify_all();
}

/// Espera (con el lock) hasta que `ready` diga que sí o pase `ms`. `true` si se cumplió.
pub fn wait_until(ms: u32, ready: impl FnMut(&mut Sync) -> bool) -> bool {
    wait_until_every(ms, None, ready)
}

/// Como [`wait_until`], pero mira de nuevo cada `every` aunque nadie avise (los timers se
/// cumplen solos, sin que otro hilo cambie nada).
pub fn wait_until_every(
    ms: u32,
    every: Option<Duration>,
    mut ready: impl FnMut(&mut Sync) -> bool,
) -> bool {
    let deadline = (ms != INFINITE).then(|| Instant::now() + Duration::from_millis(ms as u64));
    let mut g = lock();
    loop {
        if ready(g.as_mut().unwrap_or_else(|| unreachable!())) {
            return true;
        }
        let now = Instant::now();
        if deadline.is_some_and(|d| now >= d) {
            return false;
        }
        // Cuánto dormir: hasta el plazo, o hasta volver a mirar, lo que llegue antes.
        let nap = match (deadline, every) {
            (None, None) => None,
            (Some(d), None) => Some(d - now),
            (None, Some(e)) => Some(e),
            (Some(d), Some(e)) => Some((d - now).min(e)),
        };
        g = match nap {
            None => CV.wait(g).unwrap_or_else(|e| e.into_inner()),
            Some(n) => CV.wait_timeout(g, n).unwrap_or_else(|e| e.into_inner()).0,
        };
    }
}

/// El número de este hilo (el de Linux: el principal tiene el del proceso).
pub fn tid() -> u32 {
    crate::os::gettid()
}

/// Un objeto nuevo con su HANDLE.
pub fn new_object(o: SyncObj) -> u64 {
    let h = state().insert(Object::Sync);
    if let Some(s) = lock().as_mut() {
        s.objects.insert(h, o);
    }
    notify();
    state::set_last_error(0);
    h
}

/// `CloseHandle` de un objeto de estos.
pub fn forget(h: u64) {
    if let Some(s) = lock().as_mut() {
        s.objects.remove(&h);
    }
}

unsafe fn rd32(p: *const u8, o: usize) -> u32 {
    (p.add(o) as *const u32).read_volatile()
}
unsafe fn wr32(p: *mut u8, o: usize, v: u32) {
    (p.add(o) as *mut u32).write_volatile(v)
}
unsafe fn rd64(p: *const u8, o: usize) -> u64 {
    (p.add(o) as *const u64).read_volatile()
}
unsafe fn wr64(p: *mut u8, o: usize, v: u64) {
    (p.add(o) as *mut u64).write_volatile(v)
}

// --- CRITICAL_SECTION: un mutex que el mismo hilo puede tomar varias veces --------------------
// Campos (x64): DebugInfo (0), LockCount (8, -1 = libre), RecursionCount (12),
// OwningThread (16), LockSemaphore (24), SpinCount (32).

unsafe fn cs_init(cs: *mut u8) {
    std::ptr::write_bytes(cs, 0, 40);
    wr32(cs, 8, u32::MAX);
}

unsafe extern "win64" fn InitializeCriticalSection(cs: *mut u8) {
    let _g = lock();
    cs_init(cs);
}
unsafe extern "win64" fn InitializeCriticalSectionEx(cs: *mut u8, _spin: u32, _f: u32) -> Bool {
    InitializeCriticalSection(cs);
    1
}
unsafe extern "win64" fn InitializeCriticalSectionAndSpinCount(cs: *mut u8, _spin: u32) -> Bool {
    InitializeCriticalSection(cs);
    1
}
unsafe extern "win64" fn SetCriticalSectionSpinCount(_cs: *mut u8, _spin: u32) -> u32 {
    0
}
unsafe extern "win64" fn DeleteCriticalSection(_cs: *mut u8) {}

unsafe fn cs_try(cs: *mut u8, me: u32) -> bool {
    let owner = rd64(cs, 16) as u32;
    if owner == me {
        wr32(cs, 12, rd32(cs, 12) + 1);
        return true;
    }
    if owner == 0 {
        wr64(cs, 16, me as u64);
        wr32(cs, 12, 1);
        wr32(cs, 8, 0);
        return true;
    }
    false
}

unsafe extern "win64" fn EnterCriticalSection(cs: *mut u8) {
    let me = tid();
    let p = cs as usize;
    wait_until(INFINITE, |_| cs_try(p as *mut u8, me));
}

unsafe extern "win64" fn TryEnterCriticalSection(cs: *mut u8) -> Bool {
    let _g = lock();
    Bool::from(cs_try(cs, tid()))
}

unsafe extern "win64" fn LeaveCriticalSection(cs: *mut u8) {
    {
        let _g = lock();
        let n = rd32(cs, 12).saturating_sub(1);
        wr32(cs, 12, n);
        if n == 0 {
            wr64(cs, 16, 0);
            wr32(cs, 8, u32::MAX);
        }
    }
    notify();
}

// --- SRWLOCK: lectores/escritor en un puntero (bit 0 = escritor, el resto = lectores × 2) ------

unsafe extern "win64" fn InitializeSRWLock(l: *mut u64) {
    l.write_volatile(0);
}

unsafe fn srw_try(l: *mut u64, exclusive: bool) -> bool {
    let v = l.read_volatile();
    if exclusive {
        if v == 0 {
            l.write_volatile(1);
            return true;
        }
    } else if v & 1 == 0 {
        l.write_volatile(v + 2);
        return true;
    }
    false
}

unsafe extern "win64" fn AcquireSRWLockExclusive(l: *mut u64) {
    let p = l as usize;
    wait_until(INFINITE, |_| srw_try(p as *mut u64, true));
}
unsafe extern "win64" fn AcquireSRWLockShared(l: *mut u64) {
    let p = l as usize;
    wait_until(INFINITE, |_| srw_try(p as *mut u64, false));
}
unsafe extern "win64" fn TryAcquireSRWLockExclusive(l: *mut u64) -> u8 {
    let _g = lock();
    u8::from(srw_try(l, true))
}
unsafe extern "win64" fn TryAcquireSRWLockShared(l: *mut u64) -> u8 {
    let _g = lock();
    u8::from(srw_try(l, false))
}
unsafe extern "win64" fn ReleaseSRWLockExclusive(l: *mut u64) {
    {
        let _g = lock();
        l.write_volatile(0);
    }
    notify();
}
unsafe extern "win64" fn ReleaseSRWLockShared(l: *mut u64) {
    {
        let _g = lock();
        let v = l.read_volatile();
        l.write_volatile(v.saturating_sub(2));
    }
    notify();
}

// --- Variables de condición: un contador de "avisos" en el puntero -----------------------------

unsafe extern "win64" fn InitializeConditionVariable(c: *mut u64) {
    c.write_volatile(0);
}

/// Espera un aviso en `c` (soltando el lock con `release` y volviéndolo a tomar con `acquire`).
unsafe fn cv_sleep(c: *mut u64, ms: u32, release: impl FnOnce(), acquire: impl FnOnce()) -> Bool {
    let generation = {
        let _g = lock();
        c.read_volatile()
    };
    release();
    let p = c as usize;
    let woke = wait_until(ms, |_| (p as *const u64).read_volatile() != generation);
    acquire();
    if woke {
        1
    } else {
        state::set_last_error(ERROR_TIMEOUT);
        0
    }
}

unsafe extern "win64" fn SleepConditionVariableSRW(
    c: *mut u64,
    l: *mut u64,
    ms: u32,
    flags: u32,
) -> Bool {
    // CONDITION_VARIABLE_LOCKMODE_SHARED = 1
    let shared = flags & 1 != 0;
    cv_sleep(
        c,
        ms,
        || {
            if shared {
                ReleaseSRWLockShared(l)
            } else {
                ReleaseSRWLockExclusive(l)
            }
        },
        || {
            if shared {
                AcquireSRWLockShared(l)
            } else {
                AcquireSRWLockExclusive(l)
            }
        },
    )
}

unsafe extern "win64" fn SleepConditionVariableCS(c: *mut u64, cs: *mut u8, ms: u32) -> Bool {
    // Una sección crítica tomada varias veces se suelta del todo y se recupera igual.
    let depth = {
        let _g = lock();
        rd32(cs, 12)
    };
    cv_sleep(
        c,
        ms,
        || {
            {
                let _g = lock();
                wr32(cs, 12, 0);
                wr64(cs, 16, 0);
                wr32(cs, 8, u32::MAX);
            }
            notify();
        },
        || {
            EnterCriticalSection(cs);
            let _g = lock();
            wr32(cs, 12, depth.max(1));
        },
    )
}

unsafe fn cv_wake(c: *mut u64) {
    {
        let _g = lock();
        c.write_volatile(c.read_volatile().wrapping_add(1));
    }
    notify();
}
unsafe extern "win64" fn WakeConditionVariable(c: *mut u64) {
    cv_wake(c)
}
unsafe extern "win64" fn WakeAllConditionVariable(c: *mut u64) {
    cv_wake(c)
}

// --- InitOnce: 0 = sin empezar, 1 = alguien la está haciendo, 2 = lista ------------------------

unsafe extern "win64" fn InitOnceInitialize(once: *mut u64) {
    once.write_volatile(0);
}

/// Toma el "turno" de inicializar: `Some(true)` si le toca a este hilo, `Some(false)` si ya está.
unsafe fn once_begin(once: *mut u64) -> bool {
    let p = once as usize;
    let mut mine = false;
    wait_until(INFINITE, |_| {
        let o = p as *mut u64;
        match o.read_volatile() {
            0 => {
                o.write_volatile(1);
                mine = true;
                true
            }
            2 => true,
            _ => false,
        }
    });
    mine
}

unsafe fn once_end(once: *mut u64, ok: bool) {
    {
        let _g = lock();
        once.write_volatile(if ok { 2 } else { 0 });
    }
    notify();
}

unsafe extern "win64" fn InitOnceBeginInitialize(
    once: *mut u64,
    flags: u32,
    pending: *mut Bool,
    ctx: *mut u64,
) -> Bool {
    // INIT_ONCE_CHECK_ONLY = 1: solo mirar.
    if flags & 1 != 0 {
        let done = {
            let _g = lock();
            once.read_volatile() == 2
        };
        *pending = Bool::from(!done);
        return 1;
    }
    *pending = Bool::from(once_begin(once));
    if !ctx.is_null() {
        *ctx = 0;
    }
    1
}

unsafe extern "win64" fn InitOnceComplete(once: *mut u64, flags: u32, _ctx: u64) -> Bool {
    // INIT_ONCE_INIT_FAILED = 4
    once_end(once, flags & 4 == 0);
    1
}

unsafe extern "win64" fn InitOnceExecuteOnce(
    once: *mut u64,
    f: u64,
    param: u64,
    ctx: *mut u64,
) -> Bool {
    if !once_begin(once) {
        return 1;
    }
    let f: unsafe extern "win64" fn(*mut u64, u64, *mut u64) -> Bool =
        std::mem::transmute(f as usize);
    let r = f(once, param, ctx);
    once_end(once, r != 0);
    r
}

// --- WaitOnAddress -----------------------------------------------------------------------------

unsafe fn read_sized(p: *const u8, size: usize) -> u64 {
    match size {
        1 => p.read_volatile() as u64,
        2 => (p as *const u16).read_volatile() as u64,
        4 => (p as *const u32).read_volatile() as u64,
        _ => (p as *const u64).read_volatile(),
    }
}

unsafe extern "win64" fn WaitOnAddress(
    addr: *const u8,
    cmp: *const u8,
    size: usize,
    ms: u32,
) -> Bool {
    let want = read_sized(cmp, size);
    let p = addr as usize;
    if wait_until(ms, |_| read_sized(p as *const u8, size) != want) {
        1
    } else {
        state::set_last_error(ERROR_TIMEOUT);
        0
    }
}
unsafe extern "win64" fn WakeByAddressSingle(_a: u64) {
    // El que cambió el valor ya lo hizo; despertar es que vuelvan a mirar.
    let _g = lock();
    notify();
}
unsafe extern "win64" fn WakeByAddressAll(_a: u64) {
    let _g = lock();
    notify();
}

// --- Eventos, mutex y semáforos con HANDLE -----------------------------------------------------

unsafe extern "win64" fn CreateEventW(_sa: u64, manual: Bool, initial: Bool, _n: u64) -> u64 {
    new_object(SyncObj::Event {
        manual: manual != 0,
        signaled: initial != 0,
    })
}
unsafe extern "win64" fn CreateEventA(sa: u64, manual: Bool, initial: Bool, n: u64) -> u64 {
    CreateEventW(sa, manual, initial, n)
}
unsafe extern "win64" fn CreateEventExW(_sa: u64, _n: u64, flags: u32, _a: u32) -> u64 {
    // CREATE_EVENT_MANUAL_RESET = 1, CREATE_EVENT_INITIAL_SET = 2
    new_object(SyncObj::Event {
        manual: flags & 1 != 0,
        signaled: flags & 2 != 0,
    })
}

fn set_event(h: u64, value: bool) -> Bool {
    let ok = match lock().as_mut().and_then(|s| s.objects.get_mut(&h)) {
        Some(SyncObj::Event { signaled, .. }) => {
            *signaled = value;
            true
        }
        _ => false,
    };
    notify();
    if ok {
        1
    } else {
        state::set_last_error(error::INVALID_HANDLE);
        0
    }
}
unsafe extern "win64" fn SetEvent(h: u64) -> Bool {
    set_event(h, true)
}
unsafe extern "win64" fn ResetEvent(h: u64) -> Bool {
    set_event(h, false)
}
unsafe extern "win64" fn PulseEvent(h: u64) -> Bool {
    set_event(h, true);
    std::thread::yield_now();
    set_event(h, false)
}

unsafe extern "win64" fn CreateMutexW(_sa: u64, owner: Bool, _n: u64) -> u64 {
    let me = tid();
    new_object(if owner != 0 {
        SyncObj::Mutex {
            owner: me,
            count: 1,
        }
    } else {
        SyncObj::Mutex { owner: 0, count: 0 }
    })
}
unsafe extern "win64" fn CreateMutexA(sa: u64, owner: Bool, n: u64) -> u64 {
    CreateMutexW(sa, owner, n)
}
unsafe extern "win64" fn CreateMutexExW(_sa: u64, _n: u64, flags: u32, _a: u32) -> u64 {
    // CREATE_MUTEX_INITIAL_OWNER = 1
    CreateMutexW(0, Bool::from(flags & 1 != 0), 0)
}
unsafe extern "win64" fn ReleaseMutex(h: u64) -> Bool {
    let me = tid();
    let ok = match lock().as_mut().and_then(|s| s.objects.get_mut(&h)) {
        Some(SyncObj::Mutex { owner, count }) if *owner == me && *count > 0 => {
            *count -= 1;
            if *count == 0 {
                *owner = 0;
            }
            true
        }
        _ => false,
    };
    notify();
    if ok {
        1
    } else {
        state::set_last_error(288); // ERROR_NOT_OWNER
        0
    }
}

unsafe extern "win64" fn CreateSemaphoreW(_sa: u64, initial: i32, max: i32, _n: u64) -> u64 {
    new_object(SyncObj::Semaphore {
        count: initial,
        max,
    })
}
unsafe extern "win64" fn CreateSemaphoreA(sa: u64, initial: i32, max: i32, n: u64) -> u64 {
    CreateSemaphoreW(sa, initial, max, n)
}
unsafe extern "win64" fn CreateSemaphoreExW(
    _sa: u64,
    initial: i32,
    max: i32,
    _n: u64,
    _f: u32,
    _a: u32,
) -> u64 {
    CreateSemaphoreW(0, initial, max, 0)
}
unsafe extern "win64" fn ReleaseSemaphore(h: u64, n: i32, prev: *mut i32) -> Bool {
    let r = match lock().as_mut().and_then(|s| s.objects.get_mut(&h)) {
        Some(SyncObj::Semaphore { count, max }) => {
            if *count + n > *max {
                Err(298) // ERROR_TOO_MANY_POSTS
            } else {
                if !prev.is_null() {
                    *prev = *count;
                }
                *count += n;
                Ok(())
            }
        }
        _ => Err(error::INVALID_HANDLE),
    };
    notify();
    match r {
        Ok(()) => 1,
        Err(e) => {
            state::set_last_error(e);
            0
        }
    }
}

// --- Esperar HANDLE ------------------------------------------------------------------------------

/// ¿Está "señalado"? Si es de los que se consumen al esperar (evento automático, mutex,
/// semáforo), y `take`, se consume. `None` si no es un objeto que se pueda esperar.
fn try_take(s: &mut Sync, h: u64, me: u32, take: bool) -> Option<bool> {
    match s.objects.get_mut(&h)? {
        SyncObj::Event { manual, signaled } => {
            let ok = *signaled;
            if ok && take && !*manual {
                *signaled = false;
            }
            Some(ok)
        }
        SyncObj::Mutex { owner, count } => {
            let ok = *owner == 0 || *owner == me;
            if ok && take {
                *owner = me;
                *count += 1;
            }
            Some(ok)
        }
        SyncObj::Semaphore { count, .. } => {
            let ok = *count > 0;
            if ok && take {
                *count -= 1;
            }
            Some(ok)
        }
        SyncObj::Thread { done, .. } => Some(*done),
        SyncObj::Timer {
            manual,
            due,
            period,
            signaled,
        } => {
            if let Some(d) = *due
                && Instant::now() >= d
            {
                *signaled = true;
                *due = period.map(|p| d + p);
            }
            let ok = *signaled;
            if ok && take && !*manual {
                *signaled = false;
            }
            Some(ok)
        }
    }
}

/// ¿Hay timers entre los HANDLE? (entonces hay que mirar de nuevo aunque nadie avise).
fn has_timer(s: &Sync, handles: &[u64]) -> bool {
    handles
        .iter()
        .any(|h| matches!(s.objects.get(h), Some(SyncObj::Timer { .. })))
}

fn wait_many(handles: &[u64], all: bool, ms: u32) -> u32 {
    let me = tid();
    // Los HANDLE que no son de sincronización (archivos, la consola) están siempre "listos";
    // el proceso actual (-1) no termina nunca mientras se lo espera desde adentro.
    let mut result = WAIT_TIMEOUT;
    let timers = lock().as_ref().is_some_and(|s| has_timer(s, handles));
    let every = timers.then(|| Duration::from_millis(1));
    let ok = wait_until_every(ms, every, |s| {
        let ready: Vec<Option<bool>> = handles
            .iter()
            .map(|&h| {
                if h == state::CURRENT_PROCESS {
                    Some(false)
                } else {
                    try_take(s, h, me, false).or(Some(true))
                }
            })
            .collect();
        if all {
            if ready.iter().all(|r| *r == Some(true)) {
                for &h in handles {
                    let _ = try_take(s, h, me, true);
                }
                result = WAIT_OBJECT_0;
                return true;
            }
        } else if let Some(i) = ready.iter().position(|r| *r == Some(true)) {
            let _ = try_take(s, handles[i], me, true);
            result = WAIT_OBJECT_0 + i as u32;
            return true;
        }
        false
    });
    if ok { result } else { WAIT_TIMEOUT }
}

unsafe extern "win64" fn WaitForSingleObject(h: u64, ms: u32) -> u32 {
    wait_many(&[h], false, ms)
}
unsafe extern "win64" fn WaitForSingleObjectEx(h: u64, ms: u32, _alertable: Bool) -> u32 {
    wait_many(&[h], false, ms)
}
unsafe extern "win64" fn WaitForMultipleObjects(n: u32, hs: *const u64, all: Bool, ms: u32) -> u32 {
    if n == 0 || n > 64 {
        state::set_last_error(error::INVALID_PARAMETER);
        return WAIT_FAILED;
    }
    wait_many(std::slice::from_raw_parts(hs, n as usize), all != 0, ms)
}
unsafe extern "win64" fn WaitForMultipleObjectsEx(
    n: u32,
    hs: *const u64,
    all: Bool,
    ms: u32,
    _a: Bool,
) -> u32 {
    WaitForMultipleObjects(n, hs, all, ms)
}
unsafe extern "win64" fn SignalObjectAndWait(signal: u64, wait: u64, ms: u32, _a: Bool) -> u32 {
    let released = match lock().as_ref().and_then(|s| s.objects.get(&signal)) {
        Some(SyncObj::Event { .. }) => 1,
        Some(SyncObj::Mutex { .. }) => 2,
        Some(SyncObj::Semaphore { .. }) => 3,
        _ => 0,
    };
    match released {
        1 => {
            SetEvent(signal);
        }
        2 => {
            ReleaseMutex(signal);
        }
        3 => {
            ReleaseSemaphore(signal, 1, std::ptr::null_mut());
        }
        _ => {}
    }
    wait_many(&[wait], false, ms)
}

// --- Waitable timers ------------------------------------------------------------------------------

unsafe extern "win64" fn CreateWaitableTimerExW(_sa: u64, _name: u64, flags: u32, _a: u32) -> u64 {
    // CREATE_WAITABLE_TIMER_MANUAL_RESET = 1
    new_object(SyncObj::Timer {
        manual: flags & 1 != 0,
        due: None,
        period: None,
        signaled: false,
    })
}
unsafe extern "win64" fn CreateWaitableTimerW(_sa: u64, manual: Bool, _name: u64) -> u64 {
    CreateWaitableTimerExW(0, 0, u32::from(manual != 0), 0)
}

/// El momento de un "due time" de Windows: negativo = dentro de tantas centenas de nanosegundos;
/// positivo = un FILETIME absoluto.
pub fn due_of(due: i64) -> Instant {
    if due < 0 {
        Instant::now() + Duration::from_nanos(due.unsigned_abs() * 100)
    } else {
        let now = crate::win::filetime_now() as i64;
        Instant::now() + Duration::from_nanos((due - now).max(0) as u64 * 100)
    }
}

unsafe extern "win64" fn SetWaitableTimerEx(
    h: u64,
    due: *const i64,
    period: i32,
    _f: u64,
    _a: u64,
    _ctx: u64,
    _tol: u32,
) -> Bool {
    let when = due_of(due.read_unaligned());
    let ok = match lock().as_mut().and_then(|s| s.objects.get_mut(&h)) {
        Some(SyncObj::Timer {
            due: d,
            period: p,
            signaled,
            ..
        }) => {
            *d = Some(when);
            *p = (period > 0).then(|| Duration::from_millis(period as u64));
            *signaled = false;
            true
        }
        _ => false,
    };
    notify();
    if ok {
        1
    } else {
        state::set_last_error(error::INVALID_HANDLE);
        0
    }
}
unsafe extern "win64" fn SetWaitableTimer(
    h: u64,
    due: *const i64,
    period: i32,
    f: u64,
    a: u64,
    resume: Bool,
) -> Bool {
    let _ = resume;
    SetWaitableTimerEx(h, due, period, f, a, 0, 0)
}
unsafe extern "win64" fn CancelWaitableTimer(h: u64) -> Bool {
    if let Some(SyncObj::Timer { due, period, .. }) =
        lock().as_mut().and_then(|s| s.objects.get_mut(&h))
    {
        *due = None;
        *period = None;
    }
    1
}

unsafe extern "win64" fn SwitchToThread() -> Bool {
    std::thread::yield_now();
    1
}
unsafe extern "win64" fn Sleep(ms: u32) {
    if ms == 0 {
        std::thread::yield_now();
    } else {
        std::thread::sleep(Duration::from_millis(ms as u64));
    }
}
unsafe extern "win64" fn SleepEx(ms: u32, _alertable: Bool) -> u32 {
    Sleep(ms);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn seccion_critica_entre_hilos() {
        let mut cs = Box::new([0u8; 40]);
        let p = cs.as_mut_ptr() as usize;
        // SAFETY: `cs` vive hasta el final del test (los hilos terminan antes).
        unsafe { InitializeCriticalSection(p as *mut u8) };
        let total = Arc::new(AtomicU64::new(0));
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let t = Arc::clone(&total);
                std::thread::spawn(move || {
                    for _ in 0..200 {
                        // SAFETY: ídem; el mismo hilo la toma dos veces (recursiva).
                        unsafe {
                            EnterCriticalSection(p as *mut u8);
                            EnterCriticalSection(p as *mut u8);
                            // Sin atomicidad propia: solo la sección crítica la protege.
                            let v = t.load(Ordering::Relaxed);
                            t.store(v + 1, Ordering::Relaxed);
                            LeaveCriticalSection(p as *mut u8);
                            LeaveCriticalSection(p as *mut u8);
                        }
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(total.load(Ordering::Relaxed), 800);
    }

    #[test]
    fn eventos_y_semaforos() {
        // SAFETY: HANDLE recién creados.
        unsafe {
            let e = CreateEventW(0, 0, 0, 0);
            assert_eq!(WaitForSingleObject(e, 10), WAIT_TIMEOUT);
            let e2 = e;
            let h = std::thread::spawn(move || SetEvent(e2));
            assert_eq!(WaitForSingleObject(e, 5000), WAIT_OBJECT_0);
            h.join().unwrap();
            // Automático: esperar lo apagó.
            assert_eq!(WaitForSingleObject(e, 0), WAIT_TIMEOUT);
            let s = CreateSemaphoreW(0, 2, 2, 0);
            assert_eq!(WaitForSingleObject(s, 0), WAIT_OBJECT_0);
            assert_eq!(WaitForSingleObject(s, 0), WAIT_OBJECT_0);
            assert_eq!(WaitForSingleObject(s, 0), WAIT_TIMEOUT);
            let mut prev = 0;
            assert_eq!(ReleaseSemaphore(s, 2, &mut prev), 1);
            assert_eq!(prev, 0);
            assert_eq!(ReleaseSemaphore(s, 1, &mut prev), 0, "pasa del máximo");
            let hs = [e, s];
            assert_eq!(
                WaitForMultipleObjects(2, hs.as_ptr(), 0, 0),
                WAIT_OBJECT_0 + 1
            );
            // Un timer de 30 ms (relativo: negativo, en centenas de nanosegundos).
            let t = CreateWaitableTimerExW(0, 0, 0, 0);
            let due = -300_000i64;
            assert_eq!(SetWaitableTimerEx(t, &due, 0, 0, 0, 0, 0), 1);
            assert_eq!(WaitForSingleObject(t, 0), WAIT_TIMEOUT);
            let inicio = Instant::now();
            assert_eq!(WaitForSingleObject(t, 5000), WAIT_OBJECT_0);
            assert!(inicio.elapsed() >= Duration::from_millis(25));
        }
    }

    #[test]
    fn condicion_y_wait_on_address() {
        let mut lockw = Box::new(0u64);
        let mut cond = Box::new(0u64);
        let flag = Arc::new(AtomicU64::new(0));
        let (l, c) = (
            &mut *lockw as *mut u64 as usize,
            &mut *cond as *mut u64 as usize,
        );
        let f2 = Arc::clone(&flag);
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            // SAFETY: los punteros viven hasta el final del test.
            unsafe {
                AcquireSRWLockExclusive(l as *mut u64);
                f2.store(1, Ordering::SeqCst);
                ReleaseSRWLockExclusive(l as *mut u64);
                WakeConditionVariable(c as *mut u64);
            }
        });
        // SAFETY: ídem.
        unsafe {
            AcquireSRWLockExclusive(l as *mut u64);
            while flag.load(Ordering::SeqCst) == 0 {
                assert_eq!(
                    SleepConditionVariableSRW(c as *mut u64, l as *mut u64, 5000, 0),
                    1
                );
            }
            ReleaseSRWLockExclusive(l as *mut u64);
        }
        h.join().unwrap();
        let word = Box::new(7u32);
        let wp = &*word as *const u32 as usize;
        let seven = 7u32;
        // SAFETY: ídem.
        unsafe {
            assert_eq!(
                WaitOnAddress(wp as *const u8, &seven as *const u32 as *const u8, 4, 10),
                0
            );
            assert_eq!(state::last_error(), ERROR_TIMEOUT);
            let eight = 8u32;
            assert_eq!(
                WaitOnAddress(wp as *const u8, &eight as *const u32 as *const u8, 4, 10),
                1
            );
        }
    }
}
