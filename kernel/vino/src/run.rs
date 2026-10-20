//! Correr el programa: lo único de `vino` que no se puede probar en el anfitrión.
//!
//! 1. `mmap` de la imagen entera (en su `ImageBase` si está libre: `MAP_FIXED_NOREPLACE`; si no,
//!    donde haya lugar, y se relocaliza), copiar las secciones, relocalizar y llenar la IAT.
//! 2. Permisos de cada sección con `mprotect` (código: leer y ejecutar; datos: leer y escribir).
//! 3. La pila de Windows (otra zona de `mmap`, del tamaño que pide el programa) y el bloque con
//!    el TEB, el PEB y los parámetros; el TLS del programa; `arch_prctl(ARCH_SET_GS, teb)`.
//! 4. Cambiar a la pila nueva, llamar a los callbacks de TLS y al punto de entrada:
//!    `mainCRTStartup(PEB*)`. Si vuelve, su resultado es el código de salida.

use std::sync::atomic::Ordering;

use crate::dll;
use crate::modules;
use crate::os;
use crate::pe::{self, LoadError, prot};
use crate::state::{self, PROCESS_HEAP, STD_ERR, STD_IN, STD_OUT};
use crate::teb;
use crate::win;

/// Lo que necesita el arranque en la pila nueva.
struct Start {
    entry: u64,
    peb: u64,
    base: u64,
}

/// Carga `file` (el `.exe` en `path`) y lo corre con `args` (`args[0]` es el programa). No
/// vuelve: el proceso termina cuando termina el programa.
pub fn run(
    path: &str,
    file: &[u8],
    args: &[String],
) -> Result<std::convert::Infallible, LoadError> {
    let img = pe::parse(file)?;
    let size = img.size_of_image as usize;
    // El programa y, en cadena, las DLL que trae (ver `modules`).
    let base = modules::load_image(path, file, false)?;

    // La pila de Windows: lo que pide el programa (1 MiB, lo normal), con un piso de 8 MiB
    // porque las páginas recién se asignan al usarlas (no cuesta nada reservar de más).
    let stack_size = (img.stack_reserve as usize).clamp(8 << 20, 256 << 20);
    let stack = os::map(0, stack_size, prot::READ | prot::WRITE, false)
        .ok_or(LoadError::Broken("no hay memoria para la pila"))?;
    let stack_top = stack + stack_size as u64;

    let image_path = win::to_windows(path);
    let command_line = win::command_line(args);
    let env_block: Vec<u16> = win::to_wide(&dll::kernel32::environment_block());
    let env_block = Box::leak(env_block.into_boxed_slice());
    let cwd = std::env::current_dir()
        .map(|p| win::to_windows(&p.to_string_lossy()))
        .unwrap_or_else(|_| r"C:\".into());
    let block = Box::leak(vec![0u64; teb::BLOCK_SIZE / 8].into_boxed_slice());
    let block_addr = block.as_ptr() as u64;
    // SAFETY: el bloque es de `BLOCK_SIZE` bytes y vive hasta el final (lo dejó `leak`).
    let bytes =
        unsafe { std::slice::from_raw_parts_mut(block.as_mut_ptr() as *mut u8, teb::BLOCK_SIZE) };
    let pid = std::process::id() as u64;
    let addrs = teb::fill(
        bytes,
        block_addr,
        &teb::Info {
            image_base: base,
            stack_base: stack_top,
            // Todo el tamaño: el kernel asigna las páginas al primer uso, así que `__chkstk`
            // no necesita "tocar" la página de guarda de a una.
            stack_limit: stack,
            pid,
            tid: pid,
            tls_array: modules::tls_array(),
            heap: PROCESS_HEAP,
            std_handles: [STD_IN, STD_OUT, STD_ERR],
            image_path: &image_path,
            command_line: &command_line,
            current_dir: &cwd,
            environment: env_block.as_ptr() as u64,
        },
    );

    {
        let mut s = state::state();
        s.image_base = base;
        s.image_size = size as u64;
        s.image_path = image_path.clone();
        s.command_line_w = win::to_wide_nul(&command_line);
        let mut a = command_line.clone().into_bytes();
        a.push(0);
        s.command_line_a = a;
    }
    dll::crt::set_args(args);
    if !os::set_gs(addrs.teb) {
        return Err(LoadError::Broken("el kernel no dejó poner el TEB en GS"));
    }
    state::TEB.store(addrs.teb, Ordering::Relaxed);

    let start = Box::leak(Box::new(Start {
        entry: base + img.entry_rva as u64,
        peb: addrs.peb,
        base,
    }));
    // Alineada a 16: después del `call`, el programa ve la pila como la deja cualquier llamada.
    let top = stack_top & !0xf;
    // SAFETY: `top` es el tope de una pila nueva de `stack_size` bytes, `on_windows_stack` no
    // vuelve, y `start` vive para siempre (leak).
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "call {f}",
            "ud2",
            top = in(reg) top,
            f = in(reg) on_windows_stack as *const () as usize,
            in("rdi") start as *mut Start,
            options(noreturn),
        )
    }
}

/// Ya en la pila de Windows: `DllMain` de las DLL, TLS, punto de entrada y salida.
extern "C" fn on_windows_stack(start: *mut Start) -> ! {
    // SAFETY: es el `Start` que dejó `run` (vive para siempre).
    let s = unsafe { &*start };
    if let Err(why) = modules::init_pending(true) {
        dll::fatal(&why);
    }
    for cb in modules::tls_callbacks(s.base) {
        // PIMAGE_TLS_CALLBACK(DllHandle, Reason, Reserved); DLL_PROCESS_ATTACH = 1.
        // SAFETY: es una función del programa, de la lista de callbacks de TLS.
        let f: unsafe extern "win64" fn(u64, u32, u64) =
            unsafe { std::mem::transmute(cb as usize) };
        // SAFETY: ídem.
        unsafe { f(s.base, 1, 0) };
    }
    // SAFETY: el punto de entrada de un `.exe`: `DWORD WINAPI mainCRTStartup(PEB*)`.
    let entry: unsafe extern "win64" fn(u64) -> u32 =
        unsafe { std::mem::transmute(s.entry as usize) };
    // SAFETY: ídem.
    let code = unsafe { entry(s.peb) };
    dll::exit(code as i32)
}
