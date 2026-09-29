//! Tareas del kernel (K9): el cambio de contexto y lo que une el planificador con el hardware.
//!
//! La decisión de qué tarea corre está en `jarvis-task` (testeable en el host). Acá está la
//! mecánica: cada tarea tiene su **pila** (mapeada con una página de guarda, ver paging.rs) y,
//! cuando no corre, el único rastro de dónde quedó es su `rsp` guardado. Cambiar de tarea es:
//!
//! 1. apilar los registros que una función tiene que preservar (rbx, rbp, r12–r15; los demás
//!    ya los guardó quien llamó, por la convención de llamadas System V),
//! 2. guardar `rsp` de la tarea que se va y cargar el de la que viene,
//! 3. desapilar sus registros y `ret`: se vuelve a donde *esa* tarea había llamado al cambio.
//!
//! Una tarea nueva arranca con una pila armada a mano para que ese `ret` caiga en un trampolín
//! que llama a su función. Todo cambio ocurre **con las interrupciones deshabilitadas** (en un
//! manejador de interrupción, o adentro de `without_interrupts`), y cada tarea, al volver,
//! recupera su propio estado de interrupciones al salir de ahí.
//!
//! Hay **desalojo** (preemption): el timer (cada 4 ms) y las interrupciones de disco y red
//! pueden cambiar de tarea en medio de cualquier código que tenga las interrupciones
//! habilitadas. Por eso los datos compartidos usan `IrqMutex` (irqlock.rs) y el heap y el
//! puerto serie deshabilitan las interrupciones mientras tienen su lock.
//!
//! **Procesos (K11).** Una tarea puede ser un programa del anillo 3. Además de su pila, un proceso
//! tiene cosas que la CPU guarda en registros y que hay que cambiar junto con la tarea: su
//! espacio de direcciones (CR3), la pila del kernel a la que salta una interrupción o un
//! `syscall` (rsp0 de la TSS), su registro FS (el TLS del programa) y el estado de SSE (los
//! registros XMM: el kernel no los usa, pero los programas sí, así que se guardan con `fxsave`
//! y se recuperan con `fxrstor` solo al salir de un proceso y al entrar a otro).
//! Referencia: <https://wiki.osdev.org/Kernel_Multitasking> y la ABI System V para x86_64.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use jarvis_task::{MAX_TASKS, Priority, Scheduler, State, TaskId};
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::{paging, time};

/// Eventos (bits) que despiertan tareas.
pub const EV_DISK: u32 = 1 << 0;
/// Llegó un paquete (o se liberó lugar para mandar).
pub const EV_NET: u32 = 1 << 1;
/// El escritorio le dejó pedidos a la tarea de la red.
pub const EV_NET_REQUEST: u32 = 1 << 2;
/// Un proceso le dejó algo al escritorio (salida, un pedido de archivos, que terminó).
pub const EV_PROC: u32 = 1 << 3;
/// El escritorio contestó a un proceso.
pub const EV_PROC_REPLY: u32 = 1 << 4;

/// El planificador. Se toma siempre con las interrupciones deshabilitadas (en las tareas, con
/// `without_interrupts`; en los manejadores ya lo están), así nunca está tomado cuando llega una
/// interrupción que lo necesita.
static SCHED: Mutex<Option<Scheduler>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// El `rsp` de cada tarea mientras no corre.
struct SavedStacks(UnsafeCell<[u64; MAX_TASKS]>);
// SAFETY: solo se lee y escribe durante un cambio de contexto, con las interrupciones
// deshabilitadas y en un solo núcleo: nunca hay dos accesos a la vez.
unsafe impl Sync for SavedStacks {}
static SAVED: SavedStacks = SavedStacks(UnsafeCell::new([0; MAX_TASKS]));

/// El área de `fxsave` (512 bytes, alineada a 16).
#[repr(C, align(16))]
struct Fxsave([u8; 512]);

/// Lo que se cambia además de la pila cuando la tarea es un proceso.
struct UserContext {
    user: bool,
    cr3: u64,
    /// Tope de la pila del kernel de la tarea (rsp0 y la pila de `syscall`).
    kstack: u64,
    fs: u64,
    fpu: Fxsave,
}

struct Contexts(UnsafeCell<[UserContext; MAX_TASKS]>);
// SAFETY: como `SavedStacks`: solo se toca durante un cambio de contexto o al crear la tarea, con
// las interrupciones deshabilitadas, en un solo núcleo.
unsafe impl Sync for Contexts {}
static CONTEXTS: Contexts = Contexts(UnsafeCell::new(
    [const {
        UserContext {
            user: false,
            cr3: 0,
            kstack: 0,
            fs: 0,
            fpu: Fxsave([0; 512]),
        }
    }; MAX_TASKS],
));

/// La pila del kernel del proceso que corre: la lee la entrada de `syscall` (syscall.rs).
#[unsafe(no_mangle)]
pub static JARVIS_SYSCALL_STACK: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Estado inicial de SSE/x87 (como después de `fninit`): FCW = 0x37F, MXCSR = 0x1F80 (todas las
/// excepciones enmascaradas, redondeo al más cercano).
fn fresh_fpu() -> Fxsave {
    let mut f = Fxsave([0; 512]);
    f.0[0..2].copy_from_slice(&0x037Fu16.to_le_bytes());
    f.0[24..28].copy_from_slice(&0x1F80u32.to_le_bytes());
    f.0[28..32].copy_from_slice(&0xFFFFu32.to_le_bytes()); // MXCSR_MASK
    f
}

/// Lo que va además de la pila al cambiar de `from` a `to` (con las interrupciones apagadas).
///
/// # Safety
/// Interrupciones deshabilitadas, un solo núcleo, `from` y `to` < MAX_TASKS.
unsafe fn switch_user_context(from: TaskId, to: TaskId) {
    use x86_64::registers::model_specific::FsBase;
    // SAFETY: lo garantiza quien llama (ver arriba): nadie más toca CONTEXTS a la vez.
    let c = unsafe { &mut *CONTEXTS.0.get() };
    if c[from].user {
        c[from].fs = FsBase::read().as_u64();
        // SAFETY: el área es de la tarea `from`, alineada a 16 y de 512 bytes.
        unsafe {
            core::arch::asm!("fxsave64 [{}]", in(reg) c[from].fpu.0.as_mut_ptr(), options(nostack))
        };
    }
    let t = &mut c[to];
    if t.user {
        let (current, _) = x86_64::registers::control::Cr3::read_raw();
        if current.start_address().as_u64() != t.cr3 {
            // SAFETY: `cr3` es la PML4 del proceso, que comparte la mitad del kernel (este
            // código, la pila, el heap): la ejecución sigue igual con ella.
            unsafe {
                x86_64::registers::control::Cr3::write_raw(
                    x86_64::structures::paging::PhysFrame::containing_address(
                        x86_64::PhysAddr::new(t.cr3),
                    ),
                    0,
                )
            };
        }
        crate::gdt::set_kernel_stack(t.kstack);
        JARVIS_SYSCALL_STACK.store(t.kstack, Ordering::Relaxed);
        FsBase::write(x86_64::VirtAddr::new(t.fs));
        // SAFETY: el área la llenó `fxsave` (o `fresh_fpu`) y está alineada.
        unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) t.fpu.0.as_ptr(), options(nostack)) };
    }
}

global_asm!(
    // jarvis_switch(guardar: *mut u64 [rdi], cargar: u64 [rsi])
    ".global jarvis_switch",
    "jarvis_switch:",
    "push rbp",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov [rdi], rsp",
    "mov rsp, rsi",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "pop rbp",
    "ret",
    // Primer `ret` de una tarea nueva: r12 trae su función (ver `spawn`). Al llegar acá `rsp`
    // está alineado a 16, como pide la ABI antes de un `call`.
    ".global jarvis_task_trampoline",
    "jarvis_task_trampoline:",
    "mov rdi, r12",
    "call jarvis_task_start",
    "ud2",
);

unsafe extern "C" {
    fn jarvis_switch(save: *mut u64, load: u64);
    fn jarvis_task_trampoline();
}

type Entry = Box<dyn FnOnce() + Send + 'static>;

/// Lo primero que corre una tarea nueva.
#[unsafe(no_mangle)]
extern "C" fn jarvis_task_start(entry: *mut Entry) -> ! {
    // SAFETY: `spawn` dejó en r12 un `Box::into_raw` que nadie más usa; se toma una sola vez.
    let f = unsafe { Box::from_raw(entry) };
    // Viene de un cambio de contexto (interrupciones deshabilitadas): la tarea las habilita.
    interrupts::enable();
    f();
    exit()
}

/// La tarea que está corriendo pasa a ser la primera tarea ("escritorio"), y se crea la ociosa.
/// Desde acá, el timer puede cambiar de tarea.
pub fn init(name: &'static str) {
    interrupts::without_interrupts(|| {
        *SCHED.lock() = Some(Scheduler::new(
            name,
            Priority::Normal,
            time::millis(),
            time::rdtsc(),
        ));
    });
    RUNNING.store(true, Ordering::Release);
    spawn("ociosa", Priority::Idle, 32 * 1024, || {
        loop {
            // `sti; hlt` juntos: una interrupción entre las dos no se pierde.
            interrupts::enable_and_hlt();
        }
    })
    .expect("no se pudo crear la tarea ociosa");
}

/// Crea una tarea con una pila de `stack` bytes. `None` si no hay lugar en la tabla o memoria.
pub fn spawn(
    name: &'static str,
    priority: Priority,
    stack: usize,
    f: impl FnOnce() + Send + 'static,
) -> Option<TaskId> {
    spawn_with(name, priority, stack, 0, f)
}

fn spawn_with(
    name: &'static str,
    priority: Priority,
    stack: usize,
    user_cr3: u64,
    f: impl FnOnce() + Send + 'static,
) -> Option<TaskId> {
    interrupts::without_interrupts(|| {
        let mut guard = SCHED.lock();
        let sched = guard.as_mut()?;
        let id = sched.spawn(name, priority)?;
        let Some(top) = paging::map_stack(id, stack) else {
            sched.remove(id);
            return None;
        };
        let entry: *mut Entry = Box::into_raw(Box::new(Box::new(f) as Entry));
        // La pila inicial, de arriba hacia abajo: la dirección de retorno (el trampolín) y los
        // seis registros que desapila `jarvis_switch` (r12 = la función).
        let frame = [
            0,                                          // r15
            0,                                          // r14
            0,                                          // r13
            entry as u64,                               // r12
            0,                                          // rbx
            0,                                          // rbp
            jarvis_task_trampoline as *const () as u64, // ret
        ];
        let rsp = top - (frame.len() as u64) * 8;
        // SAFETY: [rsp, top) está dentro de la pila recién mapeada (escribible, de nadie más), y
        // el lugar de `id` en SAVED y CONTEXTS no lo usa nadie: la tarea todavía no corrió.
        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), rsp as *mut u64, frame.len());
            (*SAVED.0.get())[id] = rsp;
            let c = &mut (*CONTEXTS.0.get())[id];
            c.kstack = top;
            c.user = user_cr3 != 0;
            c.cr3 = user_cr3;
            c.fs = 0;
            if c.user {
                c.fpu = fresh_fpu();
            }
        }
        Some(id)
    })
}

/// Crea la tarea de un proceso (K11): corre en el espacio `cr3` y, cuando cambia de tarea, se
/// guardan y recuperan su FS y su estado de SSE. `f` termina saltando al anillo 3.
pub fn spawn_process(
    name: &'static str,
    stack: usize,
    cr3: u64,
    f: impl FnOnce() + Send + 'static,
) -> Option<TaskId> {
    spawn_with(name, Priority::Normal, stack, cr3, f)
}

/// La tarea que corre ahora.
pub fn current() -> TaskId {
    interrupts::without_interrupts(|| SCHED.lock().as_ref().map_or(0, |s| s.current()))
}

/// La tarea actual deja de ser un proceso (antes de terminar: su espacio se libera).
pub fn forget_user_context() {
    interrupts::without_interrupts(|| {
        let id = SCHED.lock().as_ref().map_or(0, |s| s.current());
        // SAFETY: sin interrupciones y en un solo núcleo: nadie más toca CONTEXTS.
        unsafe { (*CONTEXTS.0.get())[id].user = false };
    });
}

/// Cambia a la tarea que diga el planificador. Hay que llamarla sin interrupciones.
fn reschedule() {
    let pair = match SCHED.lock().as_mut() {
        Some(s) => s.schedule(time::millis(), time::rdtsc()),
        None => None,
    };
    if let Some((from, to)) = pair {
        let saved = SAVED.0.get() as *mut u64;
        // SAFETY: interrupciones deshabilitadas (lo exige el llamador) y `from`/`to` < MAX_TASKS.
        // `to` tiene guardado un rsp válido: o lo guardó este mismo código al salir, o lo armó
        // `spawn`. La pila de `from` sigue mapeada mientras exista la tarea.
        unsafe {
            switch_user_context(from, to);
            jarvis_switch(saved.add(from), *saved.add(to))
        }
    }
}

/// La multitarea ya arrancó y se puede esperar sin dar vueltas: hay planificador y las
/// interrupciones (que son las que despiertan) están habilitadas. Durante el arranque no.
pub fn can_block() -> bool {
    RUNNING.load(Ordering::Acquire) && interrupts::are_enabled()
}

/// La tarea actual espera alguno de `events` o hasta `until` (ms). Devuelve los eventos que la
/// despertaron (0: se cumplió el plazo). Sin multitarea, vuelve enseguida con 0.
pub fn wait(events: u32, until: Option<u64>) -> u32 {
    if !RUNNING.load(Ordering::Acquire) {
        return 0;
    }
    interrupts::without_interrupts(|| {
        loop {
            let now = time::millis();
            match SCHED.lock().as_mut().map(|s| s.wait(events, until, now)) {
                Some(Some(ready)) => return ready,
                Some(None) => {}
                None => return 0,
            }
            reschedule();
            // Volvimos: o nos despertaron, o no había nadie listo (no debería pasar: la ociosa
            // siempre lo está). En ese caso, dormir hasta la próxima interrupción y reintentar.
            if let Some(s) = SCHED.lock().as_ref()
                && s.task(s.current())
                    .is_some_and(|t| t.state == State::Running)
            {
                return s.woke();
            }
            interrupts::enable_and_hlt();
            interrupts::disable();
        }
    })
}

/// Avisa `events`. Si despierta a una tarea más importante que la actual, le cede la CPU ya
/// (desde una interrupción también: se llama después del "fin de interrupción" al PIC).
pub fn signal(events: u32) {
    interrupts::without_interrupts(|| {
        let preempt = SCHED.lock().as_mut().is_some_and(|s| s.signal(events));
        if preempt {
            reschedule();
        }
    });
}

/// Desde la interrupción del timer (después del "fin de interrupción").
pub fn on_timer() {
    let switch = SCHED
        .lock()
        .as_mut()
        .is_some_and(|s| s.tick(time::millis()));
    if switch {
        reschedule();
    }
}

/// La tarea actual termina. Su pila no se libera (ninguna tarea del kernel termina todavía).
pub fn exit() -> ! {
    interrupts::disable();
    if let Some(s) = SCHED.lock().as_mut() {
        s.exit();
    }
    reschedule();
    unreachable!("una tarea terminada volvió a correr");
}

/// El nombre de la tarea `id` (para el mensaje de pila desbordada). Sin esperar el lock: se usa
/// desde un fallo.
pub fn name(id: TaskId) -> &'static str {
    SCHED
        .try_lock()
        .and_then(|g| g.as_ref().and_then(|s| s.task(id)).map(|t| t.name))
        .unwrap_or("?")
}

/// Foto de las tareas para el Monitor y `ps`, y el total de cambios de contexto.
pub fn snapshot() -> (Vec<jarvis_desktop::KernelTask>, u64) {
    let (tasks, switches) = interrupts::without_interrupts(|| {
        let mut guard = SCHED.lock();
        match guard.as_mut() {
            Some(s) => {
                s.charge(time::rdtsc());
                (s.snapshot(), s.switches())
            }
            None => (Vec::new(), 0),
        }
    });
    let tasks = tasks
        .into_iter()
        .map(|(id, t)| jarvis_desktop::KernelTask {
            pid: id as u32 + 1,
            name: String::from(t.name),
            state: match t.state {
                State::Running => jarvis_desktop::TaskState::Running,
                State::Ready => jarvis_desktop::TaskState::Ready,
                State::Waiting { .. } => jarvis_desktop::TaskState::Waiting,
            },
            idle: t.priority == Priority::Idle,
            cpu_ms: time::tsc_to_us(t.cpu) / 1000,
            runs: t.runs,
        })
        .collect();
    (tasks, switches)
}
