//! Los módulos cargados: el `.exe` y las DLL "de verdad" (archivos PE) que trae el programa.
//!
//! Un programa real casi nunca es un solo archivo: Python es `python.exe` + `python314.dll`, y
//! muchos programas traen sus propias DLL al lado. Cargar una DLL es lo mismo que cargar el
//! `.exe` (secciones, relocalizaciones, imports), más tres cosas:
//!
//! - **Exports**: lo que la DLL ofrece. Un import de otro módulo se resuelve buscando el nombre
//!   (o el número, "ordinal") en esa tabla; algunos son **reenvíos** a otra DLL
//!   (`"NTDLL.RtlAllocateHeap"`), que se siguen.
//! - **Dependencias en cadena**: los imports de la DLL pueden pedir otras DLL, que se cargan antes.
//!   Una DLL queda registrada **antes** de resolver sus imports, así dos DLL que se usan entre sí
//!   (pasa) no se cargan dos veces.
//! - **`DllMain`**: la función de arranque de la DLL. Windows la llama con `DLL_PROCESS_ATTACH`
//!   cuando ya está todo cargado, primero las dependencias. Acá igual: las DLL quedan en una lista
//!   de pendientes y [`init_pending`] las inicia en ese orden.
//!
//! Qué DLL se usa para cada nombre: las de Windows (`kernel32`, el CRT…) son siempre las nuestras
//! ([`crate::dll`]), aunque el programa traiga su `vcruntime140.dll`. Las demás se buscan como
//! Windows: la carpeta del programa, la del que la pide, la carpeta actual y el `PATH`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::dll::{self, Dll};
use crate::loader;
use crate::os;
use crate::pe::{self, Export, LoadError, Symbol, Target, prot};
use crate::win;

#[derive(Clone, Debug)]
pub struct Module {
    /// El nombre del archivo, en minúsculas (`python314.dll`).
    pub name: String,
    /// La ruta como la ve el programa (`C:\Programas\python\python314.dll`).
    pub path: String,
    /// La carpeta en JARVIS-OS (para buscar sus dependencias).
    pub dir: String,
    pub base: u64,
    pub size: u64,
    pub exports: Vec<Export>,
    /// `DllMain` (de una DLL), o 0.
    pub entry: u64,
    pub dll: bool,
    pub tls_callbacks: Vec<u64>,
    /// El molde de su TLS y su índice (cada hilo nuevo recibe una copia).
    pub tls: Option<(usize, Vec<u8>)>,
    /// `DisableThreadLibraryCalls`: no avisarle de los hilos.
    pub no_thread_calls: bool,
    /// `.pdata`: dónde está la tabla de funciones (dirección absoluta) y cuántas entradas tiene.
    pub pdata: (u64, usize),
    pub inited: bool,
}

static MODULES: Mutex<Vec<Module>> = Mutex::new(Vec::new());
/// El arreglo de bloques de TLS (`TEB+0x58`): uno por módulo con `__declspec(thread)`.
static TLS_ARRAY: AtomicU64 = AtomicU64::new(0);
static TLS_NEXT: AtomicUsize = AtomicUsize::new(0);
const TLS_SLOTS: usize = 64;

fn modules() -> std::sync::MutexGuard<'static, Vec<Module>> {
    MODULES.lock().unwrap_or_else(|e| e.into_inner())
}

/// El arreglo de TLS (se crea la primera vez y vive hasta el final).
pub fn tls_array() -> u64 {
    let a = TLS_ARRAY.load(Ordering::Relaxed);
    if a != 0 {
        return a;
    }
    let v = Box::leak(vec![0u64; TLS_SLOTS].into_boxed_slice()).as_ptr() as u64;
    TLS_ARRAY.store(v, Ordering::Relaxed);
    v
}

/// ¿Qué hay detrás de un nombre de DLL?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// Una de las nuestras (en Rust).
    Builtin(Dll),
    /// Un archivo PE cargado en esta dirección.
    Native(u64),
}

/// `foo` → `foo.dll`; el nombre del archivo, sin carpeta y en minúsculas.
pub fn normalize(name: &str) -> String {
    let file = name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    if file.contains('.') {
        file
    } else {
        format!("{file}.dll")
    }
}

/// `"NTDLL.RtlFoo"` → (`ntdll.dll`, `RtlFoo`); `"X.#12"` → el ordinal 12.
pub fn parse_forward(f: &str) -> Option<(String, Symbol)> {
    let (dll, func) = f.rsplit_once('.')?;
    let sym = match func.strip_prefix('#') {
        Some(n) => Symbol::Ordinal(n.parse().ok()?),
        None => Symbol::Name(func.to_string()),
    };
    Some((normalize(dll), sym))
}

/// Las carpetas donde se busca una DLL, en orden (sin repetir).
pub fn search_dirs(exe_dir: &str, importer_dir: &str, cwd: &str, path_var: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // El PATH de Linux separa con `:`; uno de Windows (con `C:\…`), con `;`.
    let sep = if path_var.contains(';') { ';' } else { ':' };
    let extra = path_var
        .split(sep)
        .filter(|d| !d.is_empty())
        .map(win::to_unix);
    for d in [
        exe_dir.to_string(),
        importer_dir.to_string(),
        cwd.to_string(),
    ]
    .into_iter()
    .chain(extra)
    {
        if !d.is_empty() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// Un archivo en `dir` sin distinguir mayúsculas (como en Windows).
pub fn find_in(dir: &str, file: &str) -> Option<String> {
    let exact = format!("{}/{file}", dir.trim_end_matches('/'));
    if std::path::Path::new(&exact).is_file() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.eq_ignore_ascii_case(file) && e.path().is_file())
                .then(|| format!("{}/{n}", dir.trim_end_matches('/')))
        })
}

fn find_file(name: &str, importer_dir: &str) -> Option<String> {
    let exe_dir = modules().first().map(|m| m.dir.clone()).unwrap_or_default();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let path = std::env::var("PATH").unwrap_or_default();
    search_dirs(&exe_dir, importer_dir, &cwd, &path)
        .iter()
        .find_map(|d| find_in(d, name))
}

pub fn find_loaded(name: &str) -> Option<u64> {
    let n = normalize(name);
    modules().iter().find(|m| m.name == n).map(|m| m.base)
}

/// El módulo que contiene `addr` (su base y su tamaño).
pub fn module_at(addr: u64) -> Option<(u64, u64)> {
    modules()
        .iter()
        .find(|m| addr >= m.base && addr < m.base + m.size)
        .map(|m| (m.base, m.size))
}

/// La tabla `.pdata` del módulo que contiene `addr`: (base del módulo, dirección de la tabla,
/// cuántas entradas). Para el desenrollador de excepciones.
pub fn pdata_at(addr: u64) -> Option<(u64, u64, usize)> {
    modules()
        .iter()
        .find(|m| addr >= m.base && addr < m.base + m.size)
        .filter(|m| m.pdata.1 > 0)
        .map(|m| (m.base, m.pdata.0, m.pdata.1))
}

pub fn path_of(base: u64) -> Option<String> {
    modules()
        .iter()
        .find(|m| m.base == base)
        .map(|m| m.path.clone())
}

pub fn is_module(base: u64) -> bool {
    modules().iter().any(|m| m.base == base)
}

/// Lo que ofrece la DLL en `base` con ese nombre u ordinal (siguiendo los reenvíos).
pub fn export_address(base: u64, sym: &Symbol) -> Option<u64> {
    let (target, dir) = {
        let ms = modules();
        let m = ms.iter().find(|m| m.base == base)?;
        let e = match sym {
            Symbol::Name(n) => m
                .exports
                .iter()
                .find(|e| e.name.as_deref() == Some(n.as_str())),
            Symbol::Ordinal(o) => m.exports.iter().find(|e| e.ordinal == *o),
        }?;
        (e.target.clone(), m.dir.clone())
    };
    match target {
        Target::Rva(r) => Some(base + r as u64),
        Target::Forward(f) => {
            let (dll, sym) = parse_forward(&f)?;
            match load(&dll, &dir).ok()? {
                Found::Builtin(d) => match &sym {
                    Symbol::Name(n) => dll::find(d, n).map(|a| a as u64),
                    Symbol::Ordinal(_) => None,
                },
                Found::Native(b) => export_address(b, &sym),
            }
        }
    }
}

/// La dirección de un import de un módulo que está en `importer_dir`: la función nuestra, la de
/// la DLL o, si no está, una que avisa qué falta.
pub fn resolve_import(dll_name: &str, sym: &Symbol, importer_dir: &str) -> u64 {
    match load(dll_name, importer_dir) {
        Ok(Found::Builtin(_)) => dll::resolve(dll_name, sym),
        Ok(Found::Native(base)) => export_address(base, sym)
            .unwrap_or_else(|| dll::missing::stub_for(dll::missing::label(dll_name, sym, None))),
        Err(why) => dll::missing::stub_for(dll::missing::label(dll_name, sym, Some(&why))),
    }
}

/// La DLL `name`: una de las nuestras, una ya cargada, o la carga del disco (y la deja pendiente
/// de [`init_pending`]).
pub fn load(name: &str, importer_dir: &str) -> Result<Found, String> {
    let n = normalize(name);
    if let Some(d) = dll::dll_of(&n) {
        return Ok(Found::Builtin(d));
    }
    if let Some(b) = find_loaded(&n) {
        return Ok(Found::Native(b));
    }
    let path = find_file(&n, importer_dir).ok_or_else(|| format!("no encontré {n}"))?;
    let file = std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?;
    load_image(&path, &file, true)
        .map(Found::Native)
        .map_err(|e| format!("{n}: {e}"))
}

/// Pone una imagen PE en memoria y la registra: el `.exe` (`dll = false`) o una DLL. Devuelve su
/// base. Las DLL quedan pendientes de `DllMain`.
pub fn load_image(unix_path: &str, file: &[u8], dll: bool) -> Result<u64, LoadError> {
    let img = if dll {
        pe::parse_dll(file)?
    } else {
        pe::parse(file)?
    };
    let size = img.size_of_image as usize;
    let rw = prot::READ | prot::WRITE;
    let base = os::map(img.image_base, size, rw, true)
        .or_else(|| {
            (!img.relocs.is_empty())
                .then(|| os::map(0, size, rw, false))
                .flatten()
        })
        .ok_or(LoadError::Broken(
            "no hay memoria para la imagen (o no se puede mover de su dirección)",
        ))?;
    // SAFETY: `mmap` acaba de dar `size` bytes de lectura y escritura en `base`, que son de
    // `vino` y nadie más los usa.
    let mem = unsafe { std::slice::from_raw_parts_mut(base as *mut u8, size) };
    loader::copy_sections(file, &img, mem)?;
    loader::relocate(&img, mem, base)?;
    let dir = match unix_path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => unix_path[..i].to_string(),
        None => ".".to_string(),
    };
    let name = normalize(unix_path);
    // Registrada antes de los imports: si una dependencia la pide de vuelta, ya está.
    modules().push(Module {
        name,
        path: win::to_windows(unix_path),
        dir: dir.clone(),
        base,
        size: size as u64,
        exports: img.exports.clone(),
        entry: if dll && img.entry_rva != 0 {
            base + img.entry_rva as u64
        } else {
            0
        },
        dll,
        tls_callbacks: Vec::new(),
        tls: None,
        no_thread_calls: false,
        pdata: (base + img.exception.0 as u64, img.exception.1 as usize / 12),
        inited: !dll,
    });
    loader::bind(&img, mem, &mut |d, s| resolve_import(d, s, &dir))?;
    if let Some(t) = loader::tls(&img, mem)? {
        let i = TLS_NEXT.fetch_add(1, Ordering::Relaxed);
        let template = (i < TLS_SLOTS).then(|| t.template.clone());
        if let Some(tpl) = &template {
            let block = Box::leak(tpl.clone().into_boxed_slice());
            // SAFETY: el arreglo tiene TLS_SLOTS lugares y vive hasta el final.
            unsafe { *(tls_array() as *mut u64).add(i) = block.as_ptr() as u64 };
            let at = t.index_rva as usize;
            mem[at..at + 4].copy_from_slice(&(i as u32).to_le_bytes());
        }
        if let Some(m) = modules().iter_mut().find(|m| m.base == base) {
            m.tls_callbacks = t.callbacks;
            m.tls = template.map(|tpl| (i, tpl));
        }
    }
    for (rva, len, p) in loader::protections(&img) {
        let start = base + (rva as u64 & !0xfff);
        let end = (base + rva as u64 + len as u64).next_multiple_of(4096);
        // Si dos secciones comparten una página, la segunda le suma sus permisos.
        let _ = os::protect(start, (end - start) as usize, p | prot::READ);
    }
    // Después de resolver los imports: así quedan detrás de sus dependencias en la lista.
    if dll {
        let mut ms = modules();
        if let Some(i) = ms.iter().position(|m| m.base == base) {
            let m = ms.remove(i);
            ms.push(m);
        }
    }
    Ok(base)
}

/// Los callbacks de TLS del módulo en `base` (para el `.exe`, que los llama `run`).
pub fn tls_callbacks(base: u64) -> Vec<u64> {
    modules()
        .iter()
        .find(|m| m.base == base)
        .map(|m| m.tls_callbacks.clone())
        .unwrap_or_default()
}

/// El arreglo de TLS de un hilo nuevo: una copia del molde de cada módulo.
pub fn new_thread_tls() -> u64 {
    let array = Box::leak(vec![0u64; TLS_SLOTS].into_boxed_slice());
    for m in modules().iter() {
        if let Some((i, template)) = &m.tls {
            array[*i] = Box::leak(template.clone().into_boxed_slice()).as_ptr() as u64;
        }
    }
    array.as_ptr() as u64
}

/// Avisa a los módulos que un hilo empieza (`DLL_THREAD_ATTACH`) o termina (`DLL_THREAD_DETACH`):
/// sus callbacks de TLS y el `DllMain` de las DLL (salvo las que pidieron no saberlo).
pub fn thread_notify(reason: u32) {
    let list: Vec<(u64, u64, Vec<u64>, bool)> = modules()
        .iter()
        .filter(|m| m.inited)
        .map(|m| {
            (
                m.base,
                m.entry,
                m.tls_callbacks.clone(),
                m.dll && !m.no_thread_calls,
            )
        })
        .collect();
    for (base, entry, callbacks, call_main) in list {
        for cb in callbacks {
            // SAFETY: un callback de TLS del módulo: PIMAGE_TLS_CALLBACK(DllHandle, Reason, 0).
            let f: unsafe extern "win64" fn(u64, u32, u64) =
                unsafe { std::mem::transmute(cb as usize) };
            // SAFETY: ídem.
            unsafe { f(base, reason, 0) };
        }
        if call_main && entry != 0 {
            // SAFETY: `DllMain` de la DLL.
            let f: unsafe extern "win64" fn(u64, u32, u64) -> i32 =
                unsafe { std::mem::transmute(entry as usize) };
            // SAFETY: ídem.
            unsafe { f(base, reason, 0) };
        }
    }
}

/// `DisableThreadLibraryCalls(h)`.
pub fn disable_thread_calls(base: u64) -> bool {
    match modules().iter_mut().find(|m| m.base == base) {
        Some(m) => {
            m.no_thread_calls = true;
            true
        }
        None => false,
    }
}

/// Llama `DllMain(base, DLL_PROCESS_ATTACH, …)` (y sus callbacks de TLS) de las DLL que todavía
/// no se iniciaron, en el orden en que quedaron listas (las dependencias primero).
pub fn init_pending(static_load: bool) -> Result<(), String> {
    loop {
        let next = modules().iter_mut().find(|m| !m.inited).map(|m| {
            m.inited = true;
            (m.base, m.entry, m.tls_callbacks.clone(), m.name.clone())
        });
        let Some((base, entry, callbacks, name)) = next else {
            return Ok(());
        };
        let reserved = u64::from(static_load);
        for cb in callbacks {
            // SAFETY: un callback de TLS de la DLL: PIMAGE_TLS_CALLBACK(DllHandle, Reason, Reserved).
            let f: unsafe extern "win64" fn(u64, u32, u64) =
                unsafe { std::mem::transmute(cb as usize) };
            // SAFETY: ídem.
            unsafe { f(base, 1, reserved) };
        }
        if entry != 0 {
            // SAFETY: el punto de entrada de una DLL es `BOOL WINAPI DllMain(HINSTANCE, DWORD, LPVOID)`.
            let f: unsafe extern "win64" fn(u64, u32, u64) -> i32 =
                unsafe { std::mem::transmute(entry as usize) };
            // SAFETY: ídem.
            if unsafe { f(base, 1, reserved) } == 0 {
                return Err(format!(
                    "la DLL {name} no pudo iniciarse (DllMain devolvió FALSE)"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombres_y_reenvios() {
        assert_eq!(normalize(r"C:\x\Python314.DLL"), "python314.dll");
        assert_eq!(normalize("ws2_32"), "ws2_32.dll");
        assert_eq!(
            parse_forward("NTDLL.RtlAllocateHeap"),
            Some(("ntdll.dll".into(), Symbol::Name("RtlAllocateHeap".into())))
        );
        assert_eq!(
            parse_forward("api-ms-win-core-heap-l1-1-0.HeapAlloc").map(|f| f.0),
            Some("api-ms-win-core-heap-l1-1-0.dll".into())
        );
        assert_eq!(
            parse_forward("WS2_32.#23"),
            Some(("ws2_32.dll".into(), Symbol::Ordinal(23)))
        );
        assert_eq!(parse_forward("sinpunto"), None);
    }

    #[test]
    fn donde_se_buscan_las_dll() {
        assert_eq!(
            search_dirs(
                "/Programas/py",
                "/Programas/py",
                "/Documentos",
                "/Programas/bin:/Otra"
            ),
            ["/Programas/py", "/Documentos", "/Programas/bin", "/Otra"]
        );
        assert_eq!(
            search_dirs("", "/a", "/a", r"C:\Programas;D:\Otra;"),
            ["/a", "/Programas", "/Otra"]
        );
    }

    #[test]
    fn busca_sin_distinguir_mayusculas() {
        let dir = "target/vino-prueba-dll";
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(format!("{dir}/Saludo.DLL"), b"MZ").unwrap();
        let f = find_in(dir, "saludo.dll").unwrap();
        // En Linux, el nombre de verdad; en Windows (el anfitrión), da igual cómo se pida.
        assert!(f.to_ascii_lowercase().ends_with("saludo.dll"), "{f}");
        assert!(find_in(dir, "otra.dll").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn las_dll_de_windows_son_las_nuestras() {
        assert_eq!(load("KERNEL32.dll", "."), Ok(Found::Builtin(Dll::Kernel32)));
        assert_eq!(load("vcruntime140.dll", "."), Ok(Found::Builtin(Dll::Crt)));
        assert!(
            load("no-existe-en-ningun-lado.dll", ".")
                .unwrap_err()
                .contains("no encontré")
        );
    }
}
