//! El estado de "Windows" adentro de `vino`: los HANDLE abiertos, el heap, los índices de TLS y
//! FLS, las funciones de `atexit` y los datos del proceso (línea de comandos, ruta del programa).
//!
//! Hay un solo hilo (JARVIS-OS todavía no tiene `clone`), así que un `Mutex` alcanza. Regla: no
//! se llama al código del programa con el lock tomado (puede volver a llamarnos).

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc};
use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// Lo que hay detrás de un HANDLE.
pub enum Object {
    /// Entrada, salida y error estándar (la Terminal que lanzó a `vino`): 0, 1 y 2.
    Std(u8),
    File {
        file: File,
        path: String,
    },
    /// Una carpeta abierta con `FILE_FLAG_BACKUP_SEMANTICS` (así se piden sus datos).
    Dir(PathBuf),
    /// Una búsqueda de `FindFirstFile`: los nombres y por cuál va.
    Find {
        items: Vec<FindItem>,
        pos: usize,
    },
    /// Un mutex, evento o semáforo: con un solo hilo nunca hay que esperar.
    Sync,
}

#[derive(Clone, Debug)]
pub struct FindItem {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: u64,
}

pub struct State {
    handles: BTreeMap<u64, Object>,
    next_handle: u64,
    /// Qué HANDLE devuelve `GetStdHandle` (`SetStdHandle` puede cambiarlo).
    pub std_handles: [u64; 3],
    pub tls_used: [bool; 64],
    pub fls: Vec<Option<u64>>,
    /// Las funciones de `atexit`/`_onexit` (en el orden en que se registraron).
    pub atexit: Vec<u64>,
    /// Datos del proceso, con direcciones estables (los programas guardan los punteros).
    pub command_line_w: Vec<u16>,
    pub command_line_a: Vec<u8>,
    pub image_path: String,
    pub image_base: u64,
    pub image_size: u64,
    pub argv: Vec<u64>,
    pub argc: i32,
    pub envp: Vec<u64>,
    pub keep: Vec<Vec<u8>>,
}

impl State {
    fn new() -> State {
        State {
            handles: BTreeMap::new(),
            next_handle: 0x100,
            std_handles: [STD_IN, STD_OUT, STD_ERR],
            tls_used: [false; 64],
            fls: Vec::new(),
            atexit: Vec::new(),
            command_line_w: vec![0],
            command_line_a: vec![0],
            image_path: String::new(),
            image_base: 0,
            image_size: 0,
            argv: vec![0],
            argc: 0,
            envp: vec![0],
            keep: Vec::new(),
        }
    }

    pub fn insert(&mut self, o: Object) -> u64 {
        let h = self.next_handle;
        // Los HANDLE de Windows son múltiplos de 4.
        self.next_handle += 4;
        self.handles.insert(h, o);
        h
    }

    pub fn get(&mut self, h: u64) -> Option<&mut Object> {
        if let Some(i) = [STD_IN, STD_OUT, STD_ERR].iter().position(|&s| s == h) {
            return Some(self.std_obj(i));
        }
        self.handles.get_mut(&h)
    }

    fn std_obj(&mut self, i: usize) -> &mut Object {
        // Los tres estándar no están en la tabla (no se cierran de verdad): se arman cada vez.
        self.handles
            .entry(STD_KEY + i as u64)
            .or_insert(Object::Std(i as u8))
    }

    pub fn close(&mut self, h: u64) -> bool {
        [STD_IN, STD_OUT, STD_ERR].contains(&h) || self.handles.remove(&h).is_some()
    }

    /// Guarda bytes en un lugar fijo y devuelve su dirección (se liberan al terminar).
    pub fn keep(&mut self, v: Vec<u8>) -> u64 {
        let p = v.as_ptr() as u64;
        self.keep.push(v);
        p
    }
}

pub const STD_IN: u64 = 0x10;
pub const STD_OUT: u64 = 0x14;
pub const STD_ERR: u64 = 0x18;
const STD_KEY: u64 = 1;
/// `INVALID_HANDLE_VALUE` y `GetCurrentProcess()`: los dos son -1 en Windows.
pub const INVALID_HANDLE: u64 = u64::MAX;
pub const CURRENT_PROCESS: u64 = u64::MAX;
pub const CURRENT_THREAD: u64 = u64::MAX - 1;
/// El heap del proceso (`GetProcessHeap`): uno solo, que es el `malloc` de `vino`.
pub const PROCESS_HEAP: u64 = 0x5000_0000_0000;

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub struct Guard(MutexGuard<'static, Option<State>>);

impl std::ops::Deref for Guard {
    type Target = State;
    fn deref(&self) -> &State {
        self.0.as_ref().unwrap_or_else(|| unreachable!())
    }
}

impl std::ops::DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut State {
        self.0.as_mut().unwrap_or_else(|| unreachable!())
    }
}

/// El estado (se crea la primera vez).
pub fn state() -> Guard {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() {
        *g = Some(State::new());
    }
    Guard(g)
}

// --- El último error (GetLastError) ------------------------------------------------------------

thread_local! {
    /// El TEB de este hilo (cada hilo de Windows tiene el suyo; el último error vive ahí, en
    /// `TEB+0x68`, como en Windows: hay código que lo lee directo de `gs:[0x68]`).
    static TEB: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Sin TEB (en los tests del anfitrión): uno por hilo, como en Windows.
    static LAST_ERROR: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// El TEB del hilo actual (0 si no hay: en el anfitrión o antes de arrancar el programa).
pub fn teb() -> u64 {
    TEB.with(|t| t.get())
}

pub fn set_teb(t: u64) {
    TEB.with(|c| c.set(t));
}

pub fn set_last_error(e: u32) {
    let teb = teb();
    if teb != 0 {
        // SAFETY: el TEB de este hilo lo armó `run` (o `threads`) y vive mientras el hilo.
        unsafe { ((teb as usize + crate::teb::off::TEB_LAST_ERROR) as *mut u32).write(e) };
    } else {
        LAST_ERROR.with(|l| l.set(e));
    }
}

pub fn last_error() -> u32 {
    let teb = teb();
    if teb != 0 {
        // SAFETY: ídem.
        unsafe { ((teb as usize + crate::teb::off::TEB_LAST_ERROR) as *const u32).read() }
    } else {
        LAST_ERROR.with(|l| l.get())
    }
}

// --- El heap ------------------------------------------------------------------------------------

/// `HeapAlloc`, `malloc` y compañía: el allocator de Rust con 16 bytes adelante que guardan el
/// tamaño (para `HeapSize`, `_msize` y `realloc`). Windows alinea a 16 en x86-64.
const HEADER: usize = 16;

pub fn heap_alloc(size: usize, zero: bool) -> *mut u8 {
    let Some(total) = size.checked_add(HEADER) else {
        return std::ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total, 16) else {
        return std::ptr::null_mut();
    };
    // SAFETY: el layout no es de tamaño cero (tiene al menos el encabezado).
    let p = unsafe {
        if zero {
            alloc_zeroed(layout)
        } else {
            alloc(layout)
        }
    };
    if p.is_null() {
        return p;
    }
    // SAFETY: `p` tiene `total` bytes alineados a 16; el tamaño va al principio.
    unsafe {
        (p as *mut usize).write(size);
        p.add(HEADER)
    }
}

/// El tamaño que se pidió para `p` (que dio [`heap_alloc`]).
///
/// # Safety
/// `p` lo dio [`heap_alloc`] y no se liberó.
pub unsafe fn heap_size(p: *const u8) -> usize {
    // SAFETY: lo garantiza quien llama: el encabezado está 16 bytes antes.
    unsafe { (p.sub(HEADER) as *const usize).read() }
}

/// # Safety
/// `p` es nulo o lo dio [`heap_alloc`] y no se liberó.
pub unsafe fn heap_free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: lo garantiza quien llama.
    unsafe {
        let size = heap_size(p);
        let layout = Layout::from_size_align_unchecked(size + HEADER, 16);
        dealloc(p.sub(HEADER), layout);
    }
}

/// # Safety
/// `p` es nulo o lo dio [`heap_alloc`] y no se liberó.
pub unsafe fn heap_realloc(p: *mut u8, size: usize, zero: bool) -> *mut u8 {
    if p.is_null() {
        return heap_alloc(size, zero);
    }
    let q = heap_alloc(size, zero);
    if q.is_null() {
        return q;
    }
    // SAFETY: los dos bloques son válidos y distintos; se copia lo que entra en los dos.
    unsafe {
        let old = heap_size(p);
        std::ptr::copy_nonoverlapping(p, q, old.min(size));
        heap_free(p);
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_heap_recuerda_los_tamanos() {
        let p = heap_alloc(100, true);
        assert!(!p.is_null());
        assert_eq!(p as usize % 16, 0);
        // SAFETY: `p` es de `heap_alloc`.
        unsafe {
            assert_eq!(heap_size(p), 100);
            assert_eq!(*p.add(99), 0);
            p.write(42);
            let q = heap_realloc(p, 5000, false);
            assert_eq!(heap_size(q), 5000);
            assert_eq!(*q, 42);
            heap_free(q);
        }
    }

    #[test]
    fn handles() {
        let mut s = state();
        let h = s.insert(Object::Sync);
        assert_eq!(h % 4, 0);
        assert!(matches!(s.get(STD_OUT), Some(Object::Std(1))));
        assert!(s.close(h));
        assert!(!s.close(h));
        assert!(s.get(h).is_none());
    }
}
