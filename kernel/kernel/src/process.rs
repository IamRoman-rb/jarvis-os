//! Procesos (K11, ADR 0010): programas de Linux corriendo en el anillo 3.
//!
//! Un proceso es una tarea (K9) con su propio espacio de direcciones. La ABI de Linux (el
//! cargador, la memoria, los descriptores y las llamadas) está en `jarvis-linux`; acá está lo que
//! la une con el hardware y con el resto del sistema:
//!
//! - [`spawn`]: arma el espacio (paging.rs), carga el ELF y crea la tarea, que salta al programa.
//! - [`syscall`]: lo llama la entrada de `syscall` (syscall.rs) con el número y los argumentos.
//! - [`user_fault`]: un fallo de página del programa; si la dirección es de una de sus zonas, se
//!   le pone una página y sigue (memoria al primer uso). Si no, "violación de segmento".
//! - Archivos y consola: el FAT32 y la Terminal viven en la tarea del escritorio. El proceso deja
//!   un pedido en una cola ([`take_events`]), despierta al escritorio y espera la respuesta
//!   ([`reply`]): paso de mensajes, como con la red.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ptr;

use jarvis_desktop::procs::{ProcEvent, ProcReply, SpawnRequest};
use jarvis_linux::mm::Pages;
use jarvis_linux::sys::{Fault, FileOp, FileReply, NetOp, NetReply, System};
use jarvis_linux::{Flow, Process};
use jarvis_task::MAX_TASKS;

use crate::irqlock::IrqMutex;
use crate::{entropy, paging, serial_println, syscall, task, time, tls};

/// La pila del kernel de cada proceso: sus llamadas al sistema corren ahí.
const KERNEL_STACK: usize = 128 * 1024;
/// Si la salida pendiente pasa esto, el programa espera a que el escritorio la muestre.
const MAX_PENDING_OUTPUT: usize = 256 * 1024;

struct Slot {
    pid: u32,
    root: u64,
    process: Process,
    size: (u16, u16),
}

/// El proceso de cada tarea (por número de tarea). Solo lo toca la tarea misma (sus llamadas,
/// sus fallos de página), así que nunca hay dos accesos a la vez.
struct Slots(UnsafeCell<[*mut Slot; MAX_TASKS]>);
// SAFETY: cada entrada la usa solo su tarea (ver arriba), en un solo núcleo.
unsafe impl Sync for Slots {}
static SLOTS: Slots = Slots(UnsafeCell::new([ptr::null_mut(); MAX_TASKS]));

static EVENTS: IrqMutex<VecDeque<ProcEvent>> = IrqMutex::new(VecDeque::new());
static REPLIES: IrqMutex<Vec<(u32, ProcReply)>> = IrqMutex::new(Vec::new());
static KILLED: IrqMutex<Vec<u32>> = IrqMutex::new(Vec::new());
static PENDING_OUTPUT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn slot_of(task: usize) -> *mut Slot {
    // SAFETY: lectura de la entrada de la tarea actual (ver `Slots`).
    unsafe { (*SLOTS.0.get())[task] }
}

fn push_event(ev: ProcEvent) {
    if let ProcEvent::Output { data, .. } = &ev {
        PENDING_OUTPUT.fetch_add(data.len(), core::sync::atomic::Ordering::Relaxed);
    }
    EVENTS.with(|q| q.push_back(ev));
    task::signal(task::EV_PROC);
}

/// Lo que dejaron los procesos para el escritorio.
pub fn take_events() -> Vec<ProcEvent> {
    PENDING_OUTPUT.store(0, core::sync::atomic::Ordering::Relaxed);
    EVENTS.with(|q| q.drain(..).collect())
}

/// La respuesta del escritorio a un pedido de `pid`.
pub fn reply(pid: u32, r: ProcReply) {
    REPLIES.with(|v| v.push((pid, r)));
    task::signal(task::EV_PROC_REPLY);
}

/// Terminar un proceso (Ctrl+C en la Terminal). Termina en su próxima llamada, espera o tick.
pub fn kill(pid: u32) {
    KILLED.with(|k| k.push(pid));
    task::signal(task::EV_PROC_REPLY);
}

fn killed(pid: u32) -> bool {
    KILLED.with(|k| k.contains(&pid))
}

/// Crea el proceso. El error es para la Terminal ("usa bibliotecas dinámicas"…).
pub fn spawn(req: SpawnRequest) -> Result<(), String> {
    if !paging::user_space() {
        return Err("el kernel no tiene lugar para procesos (la memoria baja está ocupada)".into());
    }
    let root = paging::new_space().ok_or("no hay memoria para un proceso nuevo")?;
    let mut sys = KernelSystem {
        root,
        pid: req.pid,
        size: req.size,
        dying: false,
    };
    let argv: Vec<&[u8]> = req.argv.iter().map(|a| a.as_bytes()).collect();
    let envp: Vec<&[u8]> = req.envp.iter().map(|e| e.as_bytes()).collect();
    let loaded = Process::load(
        req.pid, &req.image, &req.path, &req.cwd, &argv, &envp, &mut sys,
    );
    let (process, entry, rsp) = match loaded {
        Ok(l) => l,
        Err(e) => {
            paging::free_space(root);
            return Err(e.to_string());
        }
    };
    let name: &'static str = Box::leak(process.name.clone().into_boxed_str());
    let slot = Box::into_raw(Box::new(Slot {
        pid: req.pid,
        root,
        process,
        size: req.size,
    }));
    let addr = slot as usize;
    let spawned = task::spawn_process(name, KERNEL_STACK, root, move || {
        // SAFETY: esta tarea es la única dueña del proceso desde acá.
        unsafe { (*SLOTS.0.get())[task::current()] = addr as *mut Slot };
        syscall::enter_user(entry, rsp)
    });
    if spawned.is_none() {
        // SAFETY: la tarea no se creó: nadie más tiene el puntero.
        drop(unsafe { Box::from_raw(slot) });
        paging::free_space(root);
        return Err("no hay lugar para otra tarea".into());
    }
    serial_println!("PROCESO_INICIO {} {}", req.pid, name);
    Ok(())
}

/// Una llamada al sistema del proceso actual. Devuelve lo que va en `rax`.
pub fn syscall(n: u64, args: [u64; 6]) -> i64 {
    let s = slot_of(task::current());
    if s.is_null() {
        return -jarvis_linux::abi::errno::ENOSYS;
    }
    // SAFETY: el proceso es de esta tarea (ver `Slots`) y sigue vivo mientras la tarea corre.
    let slot = unsafe { &mut *s };
    if killed(slot.pid) {
        kill_current(130, "");
    }
    let mut sys = KernelSystem {
        root: slot.root,
        pid: slot.pid,
        size: slot.size,
        dying: false,
    };
    match slot.process.syscall(n, args, &mut sys) {
        Flow::Return(v) => v,
        Flow::Exit(code) => kill_current(code, ""),
    }
}

/// Un fallo de página en el anillo 3. `true` si se resolvió (el programa sigue).
pub fn user_fault(addr: u64, write: bool, exec: bool) -> bool {
    let s = slot_of(task::current());
    if s.is_null() {
        return false;
    }
    // SAFETY: como en `syscall`.
    let slot = unsafe { &mut *s };
    let mut sys = KernelSystem {
        root: slot.root,
        pid: slot.pid,
        size: slot.size,
        dying: false,
    };
    slot.process.fault(addr, write, exec, &mut sys)
}

/// Una interrupción llegó mientras corría un programa: si lo mandaron a terminar, termina.
pub fn on_user_interrupt() {
    let s = slot_of(task::current());
    // SAFETY: como en `syscall`; solo se lee el número de proceso.
    if !s.is_null() && killed(unsafe { (*s).pid }) {
        kill_current(130, "");
    }
}

/// Termina el proceso actual con `code` (128 + señal si fue una señal). Guarda sus archivos,
/// avisa a la Terminal y libera su memoria. Puede venir de una interrupción: nunca vuelve.
pub fn kill_current(code: i32, why: &str) -> ! {
    let id = task::current();
    // SAFETY: la entrada es de esta tarea; se saca para que nadie más la use.
    let s = unsafe { core::mem::replace(&mut (*SLOTS.0.get())[id], ptr::null_mut()) };
    if s.is_null() {
        task::exit();
    }
    // SAFETY: el puntero salió de `Box::into_raw` en `spawn` y se acaba de sacar de la tabla.
    let mut slot = unsafe { Box::from_raw(s) };
    // Guardar los archivos es pedirle al escritorio y esperar: con interrupciones.
    x86_64::instructions::interrupts::enable();
    let mut sys = KernelSystem {
        root: slot.root,
        pid: slot.pid,
        size: slot.size,
        dying: true,
    };
    slot.process.close_all(&mut sys);
    let pid = slot.pid;
    KILLED.with(|k| k.retain(|&p| p != pid));
    REPLIES.with(|r| r.retain(|(p, _)| *p != pid));
    serial_println!("PROCESO_FIN {} {} {}", pid, slot.process.name, code);
    push_event(ProcEvent::Exited {
        pid,
        code,
        why: (!why.is_empty()).then(|| why.to_string()),
    });
    x86_64::instructions::interrupts::disable();
    task::forget_user_context();
    // SAFETY: la PML4 del kernel mapea todo lo del kernel (esta pila incluida); después de esto
    // nadie usa el espacio del proceso y se puede liberar.
    unsafe {
        x86_64::registers::control::Cr3::write_raw(
            x86_64::structures::paging::PhysFrame::containing_address(x86_64::PhysAddr::new(
                paging::kernel_root(),
            )),
            0,
        )
    };
    paging::free_space(slot.root);
    drop(slot);
    task::exit()
}

/// El `System` de `jarvis-linux` sobre el hardware y las colas.
struct KernelSystem {
    root: u64,
    pid: u32,
    size: (u16, u16),
    /// Ya está terminando: no se vuelve a terminar por un Ctrl+C mientras guarda.
    dying: bool,
}

impl KernelSystem {
    /// Deja un pedido al escritorio y espera su respuesta.
    fn request(&mut self, ev: ProcEvent) -> ProcReply {
        push_event(ev);
        loop {
            let pid = self.pid;
            if let Some(r) =
                REPLIES.with(|v| v.iter().position(|(p, _)| *p == pid).map(|i| v.remove(i).1))
            {
                return r;
            }
            if !self.dying && killed(pid) {
                kill_current(130, "");
            }
            task::wait(task::EV_PROC_REPLY, Some(time::millis() + 100));
        }
    }
}

impl Pages for KernelSystem {
    fn unmap(&mut self, start: u64, end: u64) {
        paging::unmap_user(self.root, start, end);
    }
    fn protect(&mut self, start: u64, end: u64, prot: u32) {
        paging::protect_user(self.root, start, end, prot);
    }
}

impl System for KernelSystem {
    fn read_user(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), Fault> {
        paging::read_user(self.root, addr, buf)
    }
    fn write_user(&mut self, addr: u64, data: &[u8], force: bool) -> Result<(), Fault> {
        paging::write_user(self.root, addr, data, force)
    }
    fn map_page(&mut self, page: u64, prot: u32) -> bool {
        paging::map_user(self.root, page, prot)
    }
    fn set_fs(&mut self, base: u64) {
        x86_64::registers::model_specific::FsBase::write(x86_64::VirtAddr::new(base));
    }
    fn realtime_ns(&mut self) -> u64 {
        tls::unix_ns().unwrap_or(0)
    }
    fn monotonic_ns(&mut self) -> u64 {
        time::nanos()
    }
    fn random(&mut self, buf: &mut [u8]) {
        if !entropy::fill(buf) {
            // Sin generador (no hubo entropía al arrancar): algo que al menos cambie.
            let mut x = time::rdtsc() | 1;
            for b in buf {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                *b = x as u8;
            }
        }
    }
    fn sleep_ns(&mut self, ns: u64) {
        let until = time::millis() + ns.div_ceil(1_000_000);
        loop {
            if !self.dying && killed(self.pid) {
                kill_current(130, "");
            }
            let now = time::millis();
            if now >= until {
                // `sched_yield` (0 ns): igual se cede el turno.
                if ns == 0 {
                    task::wait(0, Some(now));
                }
                return;
            }
            task::wait(task::EV_PROC_REPLY, Some(until.min(now + 100)));
        }
    }
    fn file(&mut self, op: FileOp) -> FileReply {
        match self.request(ProcEvent::File { pid: self.pid, op }) {
            ProcReply::File(r) => r,
            _ => FileReply::Error(jarvis_linux::abi::errno::EIO),
        }
    }
    fn console_write(&mut self, data: &[u8]) {
        // Un programa que escribe sin parar no puede llenar la memoria: espera a que el
        // escritorio muestre lo anterior.
        while PENDING_OUTPUT.load(core::sync::atomic::Ordering::Relaxed) > MAX_PENDING_OUTPUT {
            if !self.dying && killed(self.pid) {
                kill_current(130, "");
            }
            task::wait(0, Some(time::millis() + 10));
        }
        push_event(ProcEvent::Output {
            pid: self.pid,
            data: data.to_vec(),
        });
    }
    fn console_read(&mut self, max: usize) -> Vec<u8> {
        match self.request(ProcEvent::ReadLine { pid: self.pid, max }) {
            ProcReply::Line(l) => l,
            _ => Vec::new(),
        }
    }
    fn console_size(&mut self) -> (u16, u16) {
        self.size
    }
    fn net(&mut self, op: NetOp) -> NetReply {
        match self.request(ProcEvent::Net { pid: self.pid, op }) {
            ProcReply::Net(r) => r,
            _ => NetReply::Error(jarvis_linux::abi::errno::EIO),
        }
    }
    fn log(&mut self, msg: &str) {
        serial_println!("PROCESO_LOG {}", msg);
    }
}
