//! Procesos (K11, ADR 0010): programas de Linux corriendo en el anillo 3.
//!
//! Un proceso es una tarea (K9) con su propio espacio de direcciones. La ABI de Linux (el
//! cargador, la memoria, los descriptores y las llamadas) está en `jarvis-linux`; acá está lo que
//! la une con el hardware y con el resto del sistema:
//!
//! - [`spawn`]: arma el espacio (paging.rs), carga el ELF y crea la tarea, que salta al programa.
//! - [`syscall`]: lo llama la entrada de `syscall` (syscall.rs) con el número y los argumentos.
//! - Hilos (`clone` con memoria compartida): cada hilo es otra tarea con el mismo espacio de
//!   direcciones. Comparten el estado de Linux del proceso (archivos, memoria), que se toca de a
//!   un hilo por vez (el lock del proceso). `futex`, dormir y salir van por fuera del lock: un
//!   hilo que espera no frena a los demás.
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
use core::sync::atomic::{AtomicUsize, Ordering};

use jarvis_desktop::procs::{ProcEvent, ProcReply, SpawnRequest};
use jarvis_linux::futex::Futexes;
use jarvis_linux::mm::Pages;
use jarvis_linux::sys::{Fault, FileOp, FileReply, NetOp, NetReply, System};
use jarvis_linux::{Flow, Process};
use jarvis_task::MAX_TASKS;

use crate::irqlock::IrqMutex;
use crate::syscall::Frame;
use crate::{entropy, paging, serial_println, syscall, task, time, tls};

/// La pila del kernel de cada proceso: sus llamadas al sistema corren ahí.
const KERNEL_STACK: usize = 128 * 1024;
/// Si la salida pendiente pasa esto, el programa espera a que el escritorio la muestre.
const MAX_PENDING_OUTPUT: usize = 256 * 1024;

/// Un proceso: su espacio de direcciones y su estado de Linux, que comparten todos sus hilos.
struct Proc {
    pid: u32,
    root: u64,
    process: Process,
    size: (u16, u16),
    /// Cuántos hilos vivos tiene (el último en terminar libera todo).
    threads: usize,
    /// El código de salida, si ya se decidió (`exit_group`, una violación, Ctrl+C).
    exit_code: Option<i32>,
    /// El lock del proceso: la tarea que está adentro de una llamada (+1), o 0.
    owner: AtomicUsize,
}

/// Un hilo: una tarea del planificador que corre en el espacio de su proceso.
struct Thread {
    proc: *mut Proc,
    tid: u32,
    /// `CLONE_CHILD_CLEARTID` / `set_tid_address`: al terminar, se escribe 0 ahí y se despierta
    /// a quien espere (así `pthread_join` sabe que terminó).
    clear_tid: u64,
}

/// El hilo de cada tarea (por número de tarea). Solo lo toca la tarea misma.
struct Slots(UnsafeCell<[*mut Thread; MAX_TASKS]>);
// SAFETY: cada entrada la usa solo su tarea (ver arriba), en un solo núcleo.
unsafe impl Sync for Slots {}
static SLOTS: Slots = Slots(UnsafeCell::new([ptr::null_mut(); MAX_TASKS]));

static EVENTS: IrqMutex<VecDeque<ProcEvent>> = IrqMutex::new(VecDeque::new());
static REPLIES: IrqMutex<Vec<(u32, ProcReply)>> = IrqMutex::new(Vec::new());
static KILLED: IrqMutex<Vec<u32>> = IrqMutex::new(Vec::new());
static FUTEXES: IrqMutex<Futexes> = IrqMutex::new(Futexes::new());
static PENDING_OUTPUT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// Los números de los hilos que no son el principal (el principal usa el del proceso).
static NEXT_TID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(100_000);

fn slot_of(task: usize) -> *mut Thread {
    // SAFETY: lectura de la entrada de la tarea actual (ver `Slots`).
    unsafe { (*SLOTS.0.get())[task] }
}

fn set_slot(task: usize, t: *mut Thread) {
    // SAFETY: la entrada es de la tarea `task`, que es la actual (o todavía no corrió).
    unsafe { (*SLOTS.0.get())[task] = t };
}

/// Entra al proceso para tocar su estado (sus archivos, su memoria): un hilo a la vez. Mientras
/// espera, deja correr al resto (desde un fallo de página también: por eso se encienden las
/// interrupciones).
fn lock(p: &Proc) {
    let me = task::current() + 1;
    loop {
        if p.owner
            .compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
        if killed(p.pid) {
            kill_current(130, "");
        }
        x86_64::instructions::interrupts::enable();
        task::wait(0, Some(time::millis() + 1));
    }
}

fn unlock(p: &Proc) {
    let me = task::current() + 1;
    let _ = p
        .owner
        .compare_exchange(me, 0, Ordering::Release, Ordering::Relaxed);
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
        req.pid,
        &req.image,
        req.interp.as_deref(),
        &req.path,
        &req.cwd,
        &argv,
        &envp,
        &mut sys,
    );
    let (process, entry, rsp) = match loaded {
        Ok(l) => l,
        Err(e) => {
            paging::free_space(root);
            return Err(e.to_string());
        }
    };
    let name: &'static str = Box::leak(process.name.clone().into_boxed_str());
    let proc = Box::into_raw(Box::new(Proc {
        pid: req.pid,
        root,
        process,
        size: req.size,
        threads: 1,
        exit_code: None,
        owner: AtomicUsize::new(0),
    }));
    // El hilo principal usa el número del proceso (como en Linux).
    let thread = Box::into_raw(Box::new(Thread {
        proc,
        tid: req.pid,
        clear_tid: 0,
    }));
    let addr = thread as usize;
    let spawned = task::spawn_process(name, KERNEL_STACK, root, move || {
        set_slot(task::current(), addr as *mut Thread);
        syscall::enter_user(entry, rsp)
    });
    if spawned.is_none() {
        // SAFETY: la tarea no se creó: nadie más tiene los punteros.
        unsafe {
            drop(Box::from_raw(thread));
            drop(Box::from_raw(proc));
        }
        paging::free_space(root);
        return Err("no hay lugar para otra tarea".into());
    }
    serial_println!("PROCESO_INICIO {} {}", req.pid, name);
    Ok(())
}

/// Una llamada al sistema del hilo actual. Devuelve lo que va en `rax`. `frame` son los
/// registros del programa (los copia `clone` para el hilo nuevo).
pub fn syscall(n: u64, args: [u64; 6], frame: &Frame) -> i64 {
    let t = slot_of(task::current());
    if t.is_null() {
        return -jarvis_linux::abi::errno::ENOSYS;
    }
    // SAFETY: el hilo es de esta tarea (ver `Slots`) y sigue vivo mientras la tarea corre.
    let th = unsafe { &mut *t };
    // SAFETY: el proceso vive mientras tenga hilos, y este es uno.
    let p = unsafe { &mut *th.proc };
    if killed(p.pid) {
        kill_current(130, "");
    }
    // Lo de los hilos va por fuera del lock: así un hilo que duerme o espera un futex no frena a
    // los demás.
    use jarvis_linux::abi::{errno, nr};
    match n {
        nr::CLONE => return clone(th, frame, args),
        // clone3: glibc lo prueba primero y, con ENOSYS, usa clone.
        nr::CLONE3 => return -errno::ENOSYS,
        nr::EXIT => exit_thread(args[0] as i32 & 0xFF),
        nr::EXIT_GROUP => kill_current(args[0] as i32 & 0xFF, ""),
        nr::GETTID => return th.tid as i64,
        nr::SET_TID_ADDRESS => {
            th.clear_tid = args[0];
            return th.tid as i64;
        }
        nr::FUTEX => return futex(p, args),
        nr::SCHED_YIELD => {
            task::wait(0, Some(time::millis()));
            return 0;
        }
        nr::NANOSLEEP | nr::CLOCK_NANOSLEEP => return nanosleep(p, n, args),
        nr::ARCH_PRCTL => return arch_prctl(p.root, args),
        _ => {}
    }
    lock(p);
    let mut sys = KernelSystem {
        root: p.root,
        pid: p.pid,
        size: p.size,
        dying: false,
    };
    let r = p.process.syscall(n, args, &mut sys);
    unlock(p);
    match r {
        Flow::Return(v) => v,
        Flow::Exit(code) => kill_current(code, ""),
    }
}

/// Un fallo de página en el anillo 3. `true` si se resolvió (el programa sigue).
pub fn user_fault(addr: u64, write: bool, exec: bool) -> bool {
    let t = slot_of(task::current());
    if t.is_null() {
        return false;
    }
    // SAFETY: como en `syscall`.
    let p = unsafe { &mut *(*t).proc };
    lock(p);
    let mut sys = KernelSystem {
        root: p.root,
        pid: p.pid,
        size: p.size,
        dying: false,
    };
    let ok = p.process.fault(addr, write, exec, &mut sys);
    unlock(p);
    ok
}

/// Una interrupción llegó mientras corría un programa: si lo mandaron a terminar, termina.
pub fn on_user_interrupt() {
    let t = slot_of(task::current());
    // SAFETY: como en `syscall`; solo se lee el número de proceso.
    if !t.is_null() && killed(unsafe { (*(*t).proc).pid }) {
        kill_current(130, "");
    }
}

/// Termina el **proceso** del hilo actual con `code` (128 + señal si fue una señal): los demás
/// hilos terminan en su próxima llamada, espera o interrupción. Nunca vuelve.
pub fn kill_current(code: i32, why: &str) -> ! {
    let t = slot_of(task::current());
    if !t.is_null() {
        // SAFETY: como en `syscall`.
        let p = unsafe { &mut *(*t).proc };
        if p.exit_code.is_none() {
            p.exit_code = Some(code);
            if !why.is_empty() {
                serial_println!("PROCESO_MOTIVO {} {}", p.pid, why);
            }
        }
        let pid = p.pid;
        KILLED.with(|k| {
            if !k.contains(&pid) {
                k.push(pid);
            }
        });
    }
    exit_thread(code)
}

/// Termina el **hilo** actual. Si era el último, termina el proceso: guarda sus archivos, avisa a
/// la Terminal y libera su memoria. Puede venir de una interrupción: nunca vuelve.
fn exit_thread(code: i32) -> ! {
    let id = task::current();
    // SAFETY: la entrada es de esta tarea; se saca para que nadie más la use.
    let t = unsafe { core::mem::replace(&mut (*SLOTS.0.get())[id], ptr::null_mut()) };
    if t.is_null() {
        task::exit();
    }
    // SAFETY: el puntero salió de `Box::into_raw` (en `spawn` o `clone`) y se acaba de sacar.
    let th = unsafe { Box::from_raw(t) };
    let pp = th.proc;
    // SAFETY: el proceso vive mientras tenga hilos (este, todavía).
    let p = unsafe { &mut *pp };
    unlock(p);
    // Guardar archivos o avisar es pedirle al escritorio y esperar: con interrupciones.
    x86_64::instructions::interrupts::enable();
    if th.clear_tid != 0 {
        let _ = paging::write_user(p.root, th.clear_tid, &0u32.to_le_bytes(), false);
        FUTEXES.with(|f| f.wake((p.root, th.clear_tid), 1));
        task::signal(task::EV_FUTEX);
    }
    FUTEXES.with(|f| f.cancel(id));
    let last = x86_64::instructions::interrupts::without_interrupts(|| {
        p.threads -= 1;
        p.threads == 0
    });
    if !last {
        x86_64::instructions::interrupts::disable();
        task::forget_user_context();
        task::exit()
    }
    let code = p.exit_code.unwrap_or(code);
    let mut sys = KernelSystem {
        root: p.root,
        pid: p.pid,
        size: p.size,
        dying: true,
    };
    p.process.close_all(&mut sys);
    let pid = p.pid;
    KILLED.with(|k| k.retain(|&q| q != pid));
    REPLIES.with(|r| r.retain(|(q, _)| *q != pid));
    FUTEXES.with(|f| f.forget_space(p.root));
    serial_println!("PROCESO_FIN {} {} {}", pid, p.process.name, code);
    push_event(ProcEvent::Exited {
        pid,
        code,
        why: None,
    });
    x86_64::instructions::interrupts::disable();
    task::forget_user_context();
    // SAFETY: la PML4 del kernel mapea todo lo del kernel (esta pila incluida); después de esto
    // nadie usa el espacio del proceso (era el último hilo) y se puede liberar.
    unsafe {
        x86_64::registers::control::Cr3::write_raw(
            x86_64::structures::paging::PhysFrame::containing_address(x86_64::PhysAddr::new(
                paging::kernel_root(),
            )),
            0,
        )
    };
    paging::free_space(p.root);
    // SAFETY: era el último hilo: nadie más tiene el puntero al proceso.
    drop(unsafe { Box::from_raw(pp) });
    task::exit()
}

const CLONE_VM: u64 = 0x100;
const CLONE_SETTLS: u64 = 0x8_0000;
const CLONE_PARENT_SETTID: u64 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;
const CLONE_THREAD: u64 = 0x1_0000;

/// `clone(flags, pila, &tid_padre, &tid_hijo, tls)`: un hilo nuevo en el mismo espacio. Arranca
/// en el anillo 3 con los registros del padre, salvo `rax` = 0 (así sabe que es el hijo), su
/// pila y su TLS. Sin `CLONE_VM | CLONE_THREAD` sería un `fork`, que no hay.
fn clone(th: &mut Thread, frame: &Frame, a: [u64; 6]) -> i64 {
    use jarvis_linux::abi::errno;
    let (flags, stack, ptid, ctid, tls) = (a[0], a[1], a[2], a[3], a[4]);
    if flags & (CLONE_VM | CLONE_THREAD) != CLONE_VM | CLONE_THREAD {
        return -errno::ENOSYS;
    }
    // SAFETY: el proceso vive mientras tenga hilos.
    let p = unsafe { &mut *th.proc };
    let tid = NEXT_TID.fetch_add(1, Ordering::Relaxed);
    let mut child = Frame { ..*frame };
    child.rax = 0;
    if stack != 0 {
        child.rsp = stack;
    }
    let fs = if flags & CLONE_SETTLS != 0 {
        match x86_64::VirtAddr::try_new(tls) {
            Ok(v) => v,
            Err(_) => return -errno::EINVAL,
        }
    } else {
        x86_64::registers::model_specific::FsBase::read()
    };
    let gs = x86_64::registers::model_specific::GsBase::read();
    if flags & CLONE_PARENT_SETTID != 0
        && paging::write_user(p.root, ptid, &tid.to_le_bytes(), false).is_err()
    {
        return -errno::EFAULT;
    }
    if flags & CLONE_CHILD_SETTID != 0 {
        let _ = paging::write_user(p.root, ctid, &tid.to_le_bytes(), false);
    }
    let thread = Box::into_raw(Box::new(Thread {
        proc: th.proc,
        tid,
        clear_tid: if flags & CLONE_CHILD_CLEARTID != 0 {
            ctid
        } else {
            0
        },
    }));
    x86_64::instructions::interrupts::without_interrupts(|| p.threads += 1);
    let addr = thread as usize;
    let name: &'static str = Box::leak(alloc::format!("{}/{tid}", p.process.name).into_boxed_str());
    let spawned = task::spawn_process(name, KERNEL_STACK, p.root, move || {
        set_slot(task::current(), addr as *mut Thread);
        x86_64::registers::model_specific::FsBase::write(fs);
        x86_64::registers::model_specific::GsBase::write(gs);
        syscall::resume_user(&child)
    });
    if spawned.is_none() {
        x86_64::instructions::interrupts::without_interrupts(|| p.threads -= 1);
        // SAFETY: la tarea no se creó: nadie más tiene el puntero.
        drop(unsafe { Box::from_raw(thread) });
        return -errno::EAGAIN;
    }
    tid as i64
}

fn read_u32(root: u64, addr: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    paging::read_user(root, addr, &mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

/// Un `timespec` del programa en nanosegundos.
fn read_timespec(root: u64, addr: u64) -> Option<u64> {
    let mut b = [0u8; 16];
    paging::read_user(root, addr, &mut b).ok()?;
    let secs = u64::from_le_bytes(b[0..8].try_into().ok()?);
    let nanos = u64::from_le_bytes(b[8..16].try_into().ok()?);
    Some(secs.saturating_mul(1_000_000_000).saturating_add(nanos))
}

/// `futex`: esperar mientras un entero valga lo que se dice, despertar, mover esperas.
fn futex(p: &Proc, a: [u64; 6]) -> i64 {
    use jarvis_linux::abi::errno;
    use jarvis_linux::futex::{op, wake_op};
    let (addr, opv, val) = (a[0], a[1] as u32, a[2] as u32);
    let key = (p.root, addr);
    let me = task::current();
    match opv & 0x7F {
        op::WAIT | op::WAIT_BITSET => {
            // WAIT: plazo relativo (monótono). WAIT_BITSET: un momento (monótono o real).
            let deadline = if a[3] == 0 {
                None
            } else {
                let Some(ns) = read_timespec(p.root, a[3]) else {
                    return -errno::EFAULT;
                };
                let rel = if opv & 0x7F == op::WAIT {
                    ns
                } else if opv & op::CLOCK_REALTIME != 0 {
                    ns.saturating_sub(tls::unix_ns().unwrap_or(0))
                } else {
                    ns.saturating_sub(time::nanos())
                };
                Some(time::millis() + rel.div_ceil(1_000_000))
            };
            // Comparar y anotarse sin que nadie se meta en el medio (interrupciones apagadas).
            let queued = FUTEXES.with(|f| match read_u32(p.root, addr) {
                None => Err(errno::EFAULT),
                Some(v) if v != val => Err(errno::EAGAIN),
                Some(_) => {
                    f.wait(key, me);
                    Ok(())
                }
            });
            if let Err(e) = queued {
                return -e;
            }
            loop {
                if FUTEXES.with(|f| f.take_woken(me)) {
                    return 0;
                }
                if killed(p.pid) {
                    FUTEXES.with(|f| f.cancel(me));
                    kill_current(130, "");
                }
                let now = time::millis();
                if deadline.is_some_and(|d| now >= d) {
                    FUTEXES.with(|f| f.cancel(me));
                    return -errno::ETIMEDOUT;
                }
                let until = deadline.map_or(now + 100, |d| d.min(now + 100));
                task::wait(task::EV_FUTEX, Some(until));
            }
        }
        op::WAKE | op::WAKE_BITSET => {
            let n = FUTEXES.with(|f| f.wake(key, val as usize));
            task::signal(task::EV_FUTEX);
            n as i64
        }
        op::REQUEUE | op::CMP_REQUEUE => {
            let r = FUTEXES.with(|f| {
                if opv & 0x7F == op::CMP_REQUEUE && read_u32(p.root, addr) != Some(a[5] as u32) {
                    return Err(errno::EAGAIN);
                }
                Ok(f.requeue(key, (p.root, a[4]), val as usize, a[3] as usize))
            });
            task::signal(task::EV_FUTEX);
            r.map_or_else(|e: i64| -e, |n| n as i64)
        }
        op::WAKE_OP => {
            let r = FUTEXES.with(|f| {
                let old = read_u32(p.root, a[4]).ok_or(errno::EFAULT)?;
                let (new, wake2) = wake_op(a[5] as u32, old);
                paging::write_user(p.root, a[4], &new.to_le_bytes(), false)
                    .map_err(|_| errno::EFAULT)?;
                let mut n = f.wake(key, val as usize);
                if wake2 {
                    n += f.wake((p.root, a[4]), a[3] as usize);
                }
                Ok(n)
            });
            task::signal(task::EV_FUTEX);
            r.map_or_else(|e: i64| -e, |n| n as i64)
        }
        _ => -errno::ENOSYS,
    }
}

/// `nanosleep` y `clock_nanosleep`, sin tomar el lock del proceso.
fn nanosleep(p: &Proc, n: u64, a: [u64; 6]) -> i64 {
    use jarvis_linux::abi::{errno, nr};
    let ts = if n == nr::NANOSLEEP { a[0] } else { a[2] };
    let Some(mut ns) = read_timespec(p.root, ts) else {
        return -errno::EFAULT;
    };
    // TIMER_ABSTIME: hasta un momento (del reloj real o del monótono).
    if n == nr::CLOCK_NANOSLEEP && a[1] & 1 != 0 {
        let now = if a[0] == 0 {
            tls::unix_ns().unwrap_or(0)
        } else {
            time::nanos()
        };
        ns = ns.saturating_sub(now);
    }
    let until = time::millis() + ns.div_ceil(1_000_000);
    loop {
        if killed(p.pid) {
            kill_current(130, "");
        }
        let now = time::millis();
        if now >= until {
            return 0;
        }
        task::wait(task::EV_PROC_REPLY, Some(until.min(now + 100)));
    }
}

/// `arch_prctl`: FS y GS son de cada hilo (están en los registros, que task.rs guarda).
fn arch_prctl(root: u64, a: [u64; 6]) -> i64 {
    use jarvis_linux::abi::errno;
    use x86_64::registers::model_specific::{FsBase, GsBase};
    let get = |v: u64| match paging::write_user(root, a[1], &v.to_le_bytes(), false) {
        Ok(()) => 0,
        Err(_) => -errno::EFAULT,
    };
    match a[0] {
        // Una dirección no canónica la rechaza la CPU al escribir el MSR: se rechaza antes.
        0x1001 => match x86_64::VirtAddr::try_new(a[1]) {
            Ok(v) => {
                GsBase::write(v);
                0
            }
            Err(_) => -errno::EPERM,
        },
        0x1002 => match x86_64::VirtAddr::try_new(a[1]) {
            Ok(v) => {
                FsBase::write(v);
                0
            }
            Err(_) => -errno::EPERM,
        },
        0x1003 => get(FsBase::read().as_u64()),
        0x1004 => get(GsBase::read().as_u64()),
        _ => -errno::EINVAL,
    }
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
    fn set_gs(&mut self, base: u64) {
        x86_64::registers::model_specific::GsBase::write(x86_64::VirtAddr::new(base));
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
