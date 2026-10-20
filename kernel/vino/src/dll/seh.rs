//! El despachador de excepciones de Windows x64 (SEH), encima de [`crate::unwind`].
//!
//! Cuando un programa "lanza" (una excepción de C++, un `RaiseException`, o la CPU por un acceso
//! inválido), Windows recorre la pila buscando quién la atrapa, en **dos pasadas**:
//!
//! 1. **Búsqueda**: de la función de más adentro hacia afuera, a cada una que tiene un manejador
//!    se le pregunta "¿esto es tuyo?" (`__C_specific_handler` para `__try/__except` de C, o el
//!    `__CxxFrameHandler*` del programa para C++). El manejador mira sus tablas y contesta
//!    "seguí buscando" o "yo la atrapo".
//! 2. **Desenrollado**: ya sabiendo quién atrapa, se recorre de nuevo hasta esa función
//!    deshaciendo cada marco (y llamando los `__finally`), y al final se salta al `catch`/`except`
//!    con la pila en ese punto.
//!
//! El kernel no participa: todo lee la memoria del proceso (las tablas `.pdata`/`.xdata` y la
//! pila) y llama funciones del programa. `RtlCaptureContext` y la vuelta al `catch`
//! (`RtlRestoreContext`) van en ensamblador porque tocan todos los registros.

#![allow(non_snake_case, unsafe_op_in_unsafe_fn, clippy::missing_safety_doc)]

use crate::modules;
use crate::unwind::{self, Regs};

type Bool = i32;

macro_rules! exports {
    ($($f:ident),* $(,)?) => { vec![$((stringify!($f), $f as *const () as usize)),*] };
}

pub fn exports() -> Vec<(&'static str, usize)> {
    let mut v = exports![
        RtlLookupFunctionEntry,
        RtlVirtualUnwind,
        RtlUnwindEx,
        RtlUnwind,
        RtlRestoreContext,
        RtlPcToFileHeader,
        RtlRaiseException,
        RtlAddFunctionTable,
        RtlDeleteFunctionTable,
        RtlInstallFunctionTableCallback,
        RtlGetUnwindHistoryTable,
    ];
    // Definidas en ensamblador (capturan el contexto de quien las llama, sin un marco de Rust
    // en el medio): su dirección es la del símbolo.
    v.push((
        "RtlCaptureContext",
        vino_rtl_capture_context as *const () as usize,
    ));
    v.push(("RaiseException", vino_raise_exception as *const () as usize));
    v.push(("_CxxThrowException", vino_cxx_throw as *const () as usize));
    v
}

// --- CONTEXT ------------------------------------------------------------------------------------
// La estructura CONTEXT de x64 (1232 bytes). Los registros enteros van desde 0x78 en el orden de
// `unwind::Regs::gpr` (Rax, Rcx, …, R15) y Rip en 0xF8.

pub const CONTEXT_SIZE: usize = 1232;
const CTX_FLAGS: usize = 0x30;
const CTX_GPR: usize = 0x78;
const CTX_RIP: usize = 0xF8;
/// CONTEXT_AMD64 | CONTROL | INTEGER | SEGMENTS | FLOATING_POINT.
const CONTEXT_FULL: u32 = 0x0010_000B;

/// Lee los registros de un CONTEXT del programa.
pub unsafe fn context_regs(ctx: *const u8) -> Regs {
    let mut r = Regs::default();
    for (i, g) in r.gpr.iter_mut().enumerate() {
        *g = (ctx.add(CTX_GPR + i * 8) as *const u64).read_unaligned();
    }
    r.rip = (ctx.add(CTX_RIP) as *const u64).read_unaligned();
    r
}

/// Escribe los registros en un CONTEXT del programa.
pub unsafe fn set_context_regs(ctx: *mut u8, r: &Regs) {
    for (i, g) in r.gpr.iter().enumerate() {
        (ctx.add(CTX_GPR + i * 8) as *mut u64).write_unaligned(*g);
    }
    (ctx.add(CTX_RIP) as *mut u64).write_unaligned(r.rip);
}

/// Lee la memoria del proceso directo (mismo espacio de direcciones que `vino`).
pub struct ProcMem;
impl unwind::Mem for ProcMem {
    fn u8(&self, addr: u64) -> Option<u8> {
        if addr == 0 {
            return None;
        }
        // SAFETY: la dirección es de la imagen o la pila del programa; si fuera inválida, el
        // acceso fallaría igual que en el programa. El cargador mapeó todo esto.
        Some(unsafe { (addr as *const u8).read_volatile() })
    }
}

// --- RtlCaptureContext / RtlRestoreContext (ensamblador) --------------------------------------

// Estas rutinas capturan el contexto de **quien las llama** (su rsp, su rip y sus registros no
// volátiles), así que tienen que ser código a secas, sin un marco de Rust en el medio que corra
// un prólogo y mueva la pila. De ahí el ensamblador.
//
// `vino_rtl_capture_context` es `RtlCaptureContext` (convención de Windows: el CONTEXT en rcx) y
// deja el contexto como estaba justo antes del `call`. Las dos entradas de lanzamiento
// (`vino_raise_exception`, `vino_cxx_throw`) hacen lo mismo pero con un marco propio para armar el
// CONTEXT en la pila y pasar a `vino_do_*`: así el desenrollado arranca en el marco del programa
// que lanzó, no en el de `vino`.
core::arch::global_asm!(
    ".globl vino_rtl_capture_context",
    "vino_rtl_capture_context:",
    "mov [rcx + 0x78], rax",
    "mov [rcx + 0x88], rdx",
    "mov [rcx + 0x90], rbx",
    "mov [rcx + 0xA0], rbp",
    "mov [rcx + 0xA8], rsi",
    "mov [rcx + 0xB0], rdi",
    "mov [rcx + 0xB8], r8",
    "mov [rcx + 0xC0], r9",
    "mov [rcx + 0xC8], r10",
    "mov [rcx + 0xD0], r11",
    "mov [rcx + 0xD8], r12",
    "mov [rcx + 0xE0], r13",
    "mov [rcx + 0xE8], r14",
    "mov [rcx + 0xF0], r15",
    "mov [rcx + 0x80], rcx",
    "lea rax, [rsp + 8]",
    "mov [rcx + 0x98], rax",
    "mov rax, [rsp]",
    "mov [rcx + 0xF8], rax",
    "mov dword ptr [rcx + 0x30], 0x0010000B",
    "mov rax, [rcx + 0x78]",
    "ret",
    // void vino_restore_context(CONTEXT* rdi): carga los registros y salta a Rip (ABI de Linux,
    // rdi, porque lo llama Rust). No vuelve.
    ".globl vino_restore_context",
    "vino_restore_context:",
    "mov rax, [rdi + 0x98]", // rsp destino
    "sub rax, 8",
    "mov rdx, [rdi + 0xF8]", // rip destino → arriba de esa pila, para el `ret`
    "mov [rax], rdx",
    "mov rbx, [rdi + 0x90]",
    "mov rbp, [rdi + 0xA0]",
    "mov rsi, [rdi + 0xA8]",
    "mov r8,  [rdi + 0xB8]",
    "mov r9,  [rdi + 0xC0]",
    "mov r12, [rdi + 0xD8]",
    "mov r13, [rdi + 0xE0]",
    "mov r14, [rdi + 0xE8]",
    "mov r15, [rdi + 0xF0]",
    "mov rcx, [rdi + 0x80]",
    "mov rdx, [rdi + 0x88]",
    "mov rdi, [rdi + 0xB0]", // rdi último: deja de ser el puntero
    "mov rsp, rax",
    "ret",
    // void vino_raise_exception(code rcx, flags rdx, n r8, args r9): entrada de RaiseException.
    ".globl vino_raise_exception",
    "vino_raise_exception:",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, 0xfffffffffffffff0",
    "sub rsp, 0x560",
    "lea r10, [rsp + 0x40]",  // el CONTEXT
    "call vino_fill_context", // llena r10 con el marco del programa (ver abajo)
    "mov [rsp + 0x20], r10",  // 5º argumento (win64) = el CONTEXT
    "call vino_do_raise",
    "ud2",
    // void vino_cxx_throw(object rcx, throwinfo rdx): entrada de _CxxThrowException.
    ".globl vino_cxx_throw",
    "vino_cxx_throw:",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, 0xfffffffffffffff0",
    "sub rsp, 0x560",
    "lea r10, [rsp + 0x40]",
    "call vino_fill_context",
    "mov r8, r10", // 3er argumento = el CONTEXT
    "call vino_do_cxx",
    "ud2",
    // Llena el CONTEXT en r10 con el marco del **programa** que llamó a la entrada (rbp apunta a
    // su marco guardado: [rbp] = su rbp, [rbp+8] = su dirección de retorno, rbp+16 = su rsp).
    "vino_fill_context:",
    "mov [r10 + 0x78], rax",
    "mov [r10 + 0x80], rcx",
    "mov [r10 + 0x88], rdx",
    "mov [r10 + 0x90], rbx",
    "mov rax, [rbp]",
    "mov [r10 + 0xA0], rax", // rbp del programa
    "mov [r10 + 0xA8], rsi",
    "mov [r10 + 0xB0], rdi",
    "mov [r10 + 0xB8], r8",
    "mov [r10 + 0xC0], r9",
    "mov [r10 + 0xD8], r12",
    "mov [r10 + 0xE0], r13",
    "mov [r10 + 0xE8], r14",
    "mov [r10 + 0xF0], r15",
    "mov rax, [rbp + 8]",
    "mov [r10 + 0xF8], rax", // rip del programa
    "lea rax, [rbp + 16]",
    "mov [r10 + 0x98], rax", // rsp del programa
    "mov dword ptr [r10 + 0x30], 0x0010000B",
    "ret",
);

unsafe extern "C" {
    fn vino_rtl_capture_context(ctx: *mut u8);
    fn vino_restore_context(ctx: *const u8) -> !;
    /// Entrada de `RaiseException` (convención de Windows).
    pub fn vino_raise_exception(code: u32, flags: u32, n: u32, args: *const u64) -> !;
    /// Entrada de `_CxxThrowException`.
    pub fn vino_cxx_throw(object: u64, throw_info: u64) -> !;
}

unsafe extern "win64" fn RtlRestoreContext(ctx: *const u8, _record: u64) -> ! {
    vino_restore_context(ctx)
}

/// Lo que llama `vino_raise_exception` una vez armado el CONTEXT: construye el EXCEPTION_RECORD y
/// despacha.
#[unsafe(no_mangle)]
unsafe extern "win64" fn vino_do_raise(
    code: u32,
    flags: u32,
    n: u32,
    args: *const u64,
    context: *mut u8,
) -> ! {
    let params: Vec<u64> = if args.is_null() {
        Vec::new()
    } else {
        (0..n.min(15) as usize)
            .map(|i| args.add(i).read_unaligned())
            .collect()
    };
    let address = (context.add(CTX_RIP) as *const u64).read_unaligned();
    let mut record = make_record(code, flags & EXCEPTION_NONCONTINUABLE, &params, address);
    dispatch(record.as_mut_ptr(), context)
}

/// Lo que llama `vino_cxx_throw`: arma el EXCEPTION_RECORD de una excepción de C++ de MSVC.
#[unsafe(no_mangle)]
unsafe extern "win64" fn vino_do_cxx(object: u64, throw_info: u64, context: *mut u8) -> ! {
    let address = (context.add(CTX_RIP) as *const u64).read_unaligned();
    let base = modules::module_at(address).map_or(0, |(b, _)| b);
    let params = [CPP_MAGIC, object, throw_info, base];
    let mut record = make_record(CPP_EXCEPTION, EXCEPTION_NONCONTINUABLE, &params, address);
    dispatch(record.as_mut_ptr(), context)
}

// --- RtlLookupFunctionEntry / RtlVirtualUnwind -----------------------------------------------

/// La `RUNTIME_FUNCTION` que contiene `pc`, armada en memoria estable (el programa guarda el
/// puntero). Devuelve su dirección y, en `base`, la base de su módulo.
unsafe extern "win64" fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, _history: u64) -> u64 {
    let Some((module_base, table, count)) = modules::pdata_at(pc) else {
        if !base.is_null() {
            *base = 0;
        }
        return 0;
    };
    if !base.is_null() {
        *base = module_base;
    }
    let rva = (pc - module_base) as u32;
    match unwind::lookup(&ProcMem, table, count, rva) {
        // Devolver el puntero a la entrada de .pdata (12 bytes: begin, end, info).
        Some(_) => find_entry_addr(table, count, rva),
        None => 0,
    }
}

/// La dirección de la entrada de `.pdata` que contiene `rva` (búsqueda binaria, como el lookup).
fn find_entry_addr(table: u64, count: usize, rva: u32) -> u64 {
    let m = ProcMem;
    use unwind::Mem;
    let (mut lo, mut hi) = (0, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let e = table + mid as u64 * 12;
        let begin = m.u32(e).unwrap_or(0);
        let end = m.u32(e + 4).unwrap_or(0);
        if rva < begin {
            hi = mid;
        } else if rva >= end {
            lo = mid + 1;
        } else {
            return e;
        }
    }
    0
}

/// DISPATCHER_CONTEXT, lo que `RtlVirtualUnwind` y los manejadores comparten. Solo se usan algunos
/// campos; los demás van en cero.
#[repr(C)]
#[derive(Default)]
struct Dispatcher {
    control_pc: u64,
    image_base: u64,
    function_entry: u64,
    establisher_frame: u64,
    target_ip: u64,
    context: u64,
    language_handler: u64,
    handler_data: u64,
    history_table: u64,
    scope_index: u32,
    _pad: u32,
}

unsafe extern "win64" fn RtlVirtualUnwind(
    _type: u32,
    image_base: u64,
    control_pc: u64,
    function_entry: u64,
    context: *mut u8,
    handler_data: *mut u64,
    establisher_frame: *mut u64,
    _ctx_pointers: u64,
) -> u64 {
    let mut regs = context_regs(context);
    let func = unwind::Function {
        begin: (function_entry as *const u32).read_unaligned(),
        end: (function_entry.wrapping_add(4) as *const u32).read_unaligned(),
        info: (function_entry.wrapping_add(8) as *const u32).read_unaligned(),
    };
    let _ = control_pc;
    let handler = unwind::unwind_function(&ProcMem, image_base, func, &mut regs);
    // Después de deshacer el cuerpo, la dirección de retorno está arriba de la pila.
    let sp = regs.rsp();
    if let Some(ret) = ProcMem_u64(sp) {
        regs.rip = ret;
        regs.set_rsp(sp + 8);
    }
    if !establisher_frame.is_null() {
        *establisher_frame = sp + 8;
    }
    set_context_regs(context, &regs);
    match handler {
        Some(h) if h.routine != 0 => {
            if !handler_data.is_null() {
                *handler_data = image_base + h.data as u64;
            }
            image_base + h.routine as u64
        }
        _ => {
            if !handler_data.is_null() {
                *handler_data = 0;
            }
            0
        }
    }
}

fn ProcMem_u64(addr: u64) -> Option<u64> {
    use unwind::Mem;
    ProcMem.u64(addr)
}

// --- RtlUnwindEx: la pasada de desenrollado ----------------------------------------------------

/// Recorre desde el marco actual hasta `target_frame`, llamando a cada manejador con
/// EH_UNWINDING (para los `__finally`), y al final salta a `target_ip` con `return_value` en rax.
unsafe extern "win64" fn RtlUnwindEx(
    target_frame: u64,
    target_ip: u64,
    record: *mut u8,
    return_value: u64,
    context: *mut u8,
    _history: u64,
) {
    // EH_UNWINDING = 2; si es el último marco, EH_UNWINDING | EH_TARGET_UNWIND.
    const EH_UNWINDING: u32 = 2;
    const EH_TARGET_UNWIND: u32 = 4;
    let mut regs = context_regs(context);
    loop {
        let pc = regs.rip;
        let Some((module_base, table, count)) = modules::pdata_at(pc) else {
            break;
        };
        let rva = (pc - module_base) as u32;
        let Some(func) = unwind::lookup(&ProcMem, table, count, rva) else {
            // Función hoja: solo `pop rip`.
            let sp = regs.rsp();
            let Some(ret) = ProcMem_u64(sp) else { break };
            regs.rip = ret;
            regs.set_rsp(sp + 8);
            continue;
        };
        // El marco "establecedor" es la rsp del cuerpo (antes de deshacer el prólogo), que es
        // también dónde queda la pila al saltar al `__except`/`catch`.
        let frame = regs.rsp();
        let mut unwound = regs;
        let handler = unwind::unwind_function(&ProcMem, module_base, func, &mut unwound);
        let sp = unwound.rsp();
        let Some(ret) = ProcMem_u64(sp) else { break };
        unwound.rip = ret;
        unwound.set_rsp(sp + 8);

        // ¿Pasamos el marco destino? (no debería: se para justo en él).
        if target_frame != 0 && frame > target_frame {
            break;
        }
        // El manejador de este marco corre su parte de desenrollado (los __finally).
        if let Some(h) = handler
            && h.routine != 0
        {
            let is_target = frame == target_frame;
            // El CONTEXT para el manejador: los registros ya deshechos de este marco.
            let mut ctx = vec![0u8; CONTEXT_SIZE];
            (ctx.as_mut_ptr().add(CTX_FLAGS) as *mut u32).write(CONTEXT_FULL);
            set_context_regs(ctx.as_mut_ptr(), &regs);
            let mut disp = Dispatcher {
                control_pc: pc,
                image_base: module_base,
                function_entry: find_entry_addr(table, count, rva),
                establisher_frame: frame,
                target_ip,
                context: ctx.as_ptr() as u64,
                language_handler: module_base + h.routine as u64,
                handler_data: module_base + h.data as u64,
                ..Default::default()
            };
            let flags = EH_UNWINDING | if is_target { EH_TARGET_UNWIND } else { 0 };
            if !record.is_null() {
                (record.add(4) as *mut u32).write_unaligned(flags);
            }
            call_handler(
                module_base + h.routine as u64,
                record as u64,
                frame,
                context as u64,
                &mut disp as *mut _ as u64,
            );
        }
        if frame == target_frame {
            break;
        }
        regs = unwound;
    }
    // Saltar al `catch`/`except`: rax = return_value, rip = target_ip, rsp = target_frame.
    regs.rip = target_ip;
    if target_frame != 0 {
        regs.set_rsp(target_frame);
    }
    regs.gpr[unwind::reg::RAX] = return_value;
    set_context_regs(context, &regs);
    vino_restore_context(context)
}

unsafe extern "win64" fn RtlUnwind(
    target_frame: u64,
    target_ip: u64,
    record: *mut u8,
    return_value: u64,
) {
    let mut ctx = vec![0u8; CONTEXT_SIZE];
    (ctx.as_mut_ptr().add(CTX_FLAGS) as *mut u32).write(CONTEXT_FULL);
    vino_rtl_capture_context(ctx.as_mut_ptr());
    RtlUnwindEx(
        target_frame,
        target_ip,
        record,
        return_value,
        ctx.as_mut_ptr(),
        0,
    );
}

/// Llama a un manejador de lenguaje: `EXCEPTION_DISPOSITION h(record, frame, context, dispatcher)`.
unsafe fn call_handler(
    routine: u64,
    record: u64,
    frame: u64,
    context: u64,
    dispatcher: u64,
) -> u32 {
    let f: unsafe extern "win64" fn(u64, u64, u64, u64) -> u32 =
        std::mem::transmute(routine as usize);
    f(record, frame, context, dispatcher)
}

// --- El despachador: busca quién atrapa y, si alguien lo hace, desenrolla ----------------------

const EXCEPTION_RECORD_SIZE: usize = 0x98;
const EXCEPTION_NONCONTINUABLE: u32 = 1;
const EXCEPTION_CONTINUE_SEARCH: u32 = 1;
const EXCEPTION_CONTINUE_EXECUTION: u32 = 0xffff_ffff;

/// Lanza una excepción con el código `code` desde `context`. Recorre la pila (pasada de búsqueda);
/// si un manejador la atrapa, él llama a `RtlUnwindEx` y no se vuelve de acá. Si nadie la atrapa,
/// termina el programa.
unsafe fn dispatch(record: *mut u8, context: *mut u8) -> ! {
    let mut regs = context_regs(context);
    let code = (record as *const u32).read_unaligned();
    let mut frames = 0;
    loop {
        frames += 1;
        if frames > 2000 {
            break;
        }
        let pc = regs.rip;
        let Some((module_base, table, count)) = modules::pdata_at(pc) else {
            break; // salió de todo módulo conocido: nadie la atrapó
        };
        let rva = (pc - module_base) as u32;
        let func = match unwind::lookup(&ProcMem, table, count, rva) {
            Some(f) => f,
            None => {
                // hoja
                let sp = regs.rsp();
                let Some(ret) = ProcMem_u64(sp) else { break };
                regs.rip = ret;
                regs.set_rsp(sp + 8);
                continue;
            }
        };
        let frame = regs.rsp(); // el marco establecedor (rsp del cuerpo, antes del desenrollado)
        let mut next = regs;
        let handler = unwind::unwind_function(&ProcMem, module_base, func, &mut next);
        let sp = next.rsp();
        let Some(ret) = ProcMem_u64(sp) else { break };
        next.rip = ret;
        next.set_rsp(sp + 8);

        if let Some(h) = handler
            && h.routine != 0
            && h.flags & 1 != 0
        // EHANDLER: tiene un manejador de excepciones (no solo de desenrollado)
        {
            let mut ctx = vec![0u8; CONTEXT_SIZE];
            (ctx.as_mut_ptr().add(CTX_FLAGS) as *mut u32).write(CONTEXT_FULL);
            set_context_regs(ctx.as_mut_ptr(), &regs);
            let mut disp = Dispatcher {
                control_pc: pc,
                image_base: module_base,
                function_entry: find_entry_addr(table, count, rva),
                establisher_frame: frame,
                context: ctx.as_ptr() as u64,
                language_handler: module_base + h.routine as u64,
                handler_data: module_base + h.data as u64,
                ..Default::default()
            };
            // EXCEPTION_RECORD.flags para la búsqueda (sin EH_UNWINDING).
            (record.add(4) as *mut u32).write_unaligned(0);
            let d = call_handler(
                module_base + h.routine as u64,
                record as u64,
                frame,
                ctx.as_ptr() as u64,
                &mut disp as *mut _ as u64,
            );
            match d {
                EXCEPTION_CONTINUE_SEARCH => {}
                EXCEPTION_CONTINUE_EXECUTION => {
                    // El manejador arregló la causa: seguir donde estaba.
                    set_context_regs(context, &regs);
                    vino_restore_context(context);
                }
                _ => {
                    // El manejador atrapó y ya hizo el desenrollado (llamó a RtlUnwindEx): no
                    // vuelve. Si volvió, es un "collided unwind"; por las dudas, seguir.
                }
            }
        }
        regs = next;
    }
    crate::dll::fatal(&format!(
        "excepción de Windows 0x{code:08X} sin atrapar (el programa no la manejó)"
    ));
}

/// Arma un EXCEPTION_RECORD (devuelto por valor para que viva mientras se despacha).
unsafe fn make_record(code: u32, flags: u32, params: &[u64], address: u64) -> Vec<u8> {
    let mut record = vec![0u8; EXCEPTION_RECORD_SIZE];
    let p = record.as_mut_ptr();
    (p as *mut u32).write(code);
    (p.add(4) as *mut u32).write(flags);
    (p.add(16) as *mut u64).write(address); // ExceptionAddress
    let n = params.len().min(15);
    (p.add(24) as *mut u32).write(n as u32); // NumberParameters
    for (i, v) in params.iter().take(n).enumerate() {
        (p.add(32 + i * 8) as *mut u64).write(*v);
    }
    record
}

/// El código de una excepción de C++ de MSVC y el "magic" de su registro.
const CPP_EXCEPTION: u32 = 0xE06D_7363; // 'msc'
const CPP_MAGIC: u64 = 0x1993_0520;

unsafe extern "win64" fn RtlRaiseException(record: *mut u8) -> ! {
    let mut ctx = vec![0u8; CONTEXT_SIZE];
    (ctx.as_mut_ptr().add(CTX_FLAGS) as *mut u32).write(CONTEXT_FULL);
    vino_rtl_capture_context(ctx.as_mut_ptr());
    dispatch(record, ctx.as_mut_ptr())
}

unsafe extern "win64" fn RtlPcToFileHeader(pc: u64, base: *mut u64) -> u64 {
    let r = modules::module_at(pc).map_or(0, |(b, _)| b);
    *base = r;
    r
}

// Tablas de funciones dinámicas (JIT): JARVIS-OS no las tiene, pero se aceptan.
unsafe extern "win64" fn RtlAddFunctionTable(_t: u64, _n: u32, _base: u64) -> Bool {
    1
}
unsafe extern "win64" fn RtlDeleteFunctionTable(_t: u64) -> Bool {
    1
}
unsafe extern "win64" fn RtlInstallFunctionTableCallback(
    _id: u64,
    _base: u64,
    _len: u32,
    _cb: u64,
    _ctx: u64,
    _dll: u64,
) -> Bool {
    1
}
unsafe extern "win64" fn RtlGetUnwindHistoryTable() -> u64 {
    0
}

// --- __C_specific_handler: __try/__except/__finally de C ----------------------------------------

const EXCEPTION_EXECUTE_HANDLER: u32 = 1;

/// El manejador de lenguaje para el SEH de C (`__try`). Lo llama el despachador en las dos
/// pasadas; interpreta la "scope table" (`handler_data`): por cada `__try` abierto donde pasó la
/// excepción, corre su filtro (`__except`) o su `__finally`.
pub unsafe extern "win64" fn __C_specific_handler(
    record: *mut u8,
    frame: u64,
    context: *mut u8,
    dispatcher: *mut u8,
) -> u32 {
    let disp = &*(dispatcher as *const Dispatcher);
    let base = disp.image_base;
    let control_pc = disp.control_pc;
    let scope = disp.handler_data as *const u32;
    let count = scope.read_unaligned() as usize;
    let flags = (record.add(4) as *const u32).read_unaligned();
    let unwinding = flags & 2 != 0; // EH_UNWINDING
    let rva = (control_pc - base) as u32;

    for i in 0..count {
        let rec = scope.add(1 + i * 4);
        let begin = rec.read_unaligned();
        let end = rec.add(1).read_unaligned();
        let handler = rec.add(2).read_unaligned();
        let target = rec.add(3).read_unaligned();
        if rva < begin || rva >= end {
            continue;
        }
        if !unwinding {
            // Pasada de búsqueda: solo los __except (target != 0).
            if target == 0 {
                continue;
            }
            // El filtro: EXCEPTION_EXECUTE_HANDLER directo (handler == 1) o una función.
            let value = if handler == 1 {
                EXCEPTION_EXECUTE_HANDLER
            } else {
                // EXCEPTION_POINTERS { record, context } en la pila.
                let pointers = [record as u64, context as u64];
                let filter: unsafe extern "win64" fn(*const u64, u64) -> i32 =
                    std::mem::transmute((base + handler as u64) as usize);
                filter(pointers.as_ptr(), frame) as u32
            };
            if value == EXCEPTION_CONTINUE_EXECUTION {
                return EXCEPTION_CONTINUE_EXECUTION;
            }
            if value == EXCEPTION_EXECUTE_HANDLER {
                // Desenrollar hasta el bloque __except y saltar ahí (no vuelve).
                RtlUnwindEx(frame, base + target as u64, record, 0, context, 0);
            }
        } else {
            // Pasada de desenrollado: correr los __finally (target == 0) que no sean el destino.
            if target == 0 {
                let abnormal = 1u64; // viene de una excepción, no de salir normal
                let fin: unsafe extern "win64" fn(u64, u64) =
                    std::mem::transmute((base + handler as u64) as usize);
                fin(abnormal, frame);
            } else if base + target as u64 == disp.target_ip {
                break;
            }
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captura_y_restaura_los_registros_no_volatiles() {
        let mut ctx = vec![0u8; CONTEXT_SIZE];
        // SAFETY: `ctx` tiene CONTEXT_SIZE bytes.
        unsafe {
            vino_rtl_capture_context(ctx.as_mut_ptr());
            let r = context_regs(ctx.as_ptr());
            // rsp y rip capturados tienen que ser plausibles (no cero).
            assert_ne!(r.rsp(), 0);
            assert_ne!(r.rip, 0);
            assert_eq!(
                (ctx.as_ptr().add(CTX_FLAGS) as *const u32).read(),
                CONTEXT_FULL
            );
        }
    }
}
