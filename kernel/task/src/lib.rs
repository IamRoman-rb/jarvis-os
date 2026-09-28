//! La multitarea de JARVIS-OS (hito K9): el planificador, sin hardware.
//!
//! Una **tarea** es un hilo de ejecución del kernel con su propia pila. La CPU es una sola, así
//! que las tareas se turnan: el planificador decide, cada vez que se lo consulta, cuál sigue. El
//! kernel lo consulta en tres momentos:
//!
//! - **La tarea espera** algo ([`Scheduler::wait`]): un evento (el disco terminó, llegó un
//!   paquete, hay un pedido para la red) o un plazo (el próximo cuadro). Mientras tanto no gasta
//!   CPU: el planificador ni la mira hasta que pase eso.
//! - **Llega una interrupción** que despierta a alguien ([`Scheduler::signal`]). Si la tarea
//!   despertada es más importante que la que está corriendo, se la **desaloja** (preemption).
//! - **El timer** ([`Scheduler::tick`]): despierta a las que esperaban un plazo y, si la que corre
//!   ya usó su turno ([`QUANTUM_MS`]) y hay otra igual de importante lista, le toca a esa.
//!
//! El cambio de contexto en sí (guardar registros, cambiar de pila) lo hace el kernel
//! (`kernel/src/task.rs`): acá solo está la decisión, así se prueba en el host.
//!
//! **Eventos**: son bits de un `u32`. Si un evento llega cuando nadie lo espera, queda
//! **anotado** y la próxima espera de ese evento vuelve enseguida. Sin eso habría una carrera
//! clásica: la tarea mira el anillo del disco (todavía no terminó), llega la interrupción, y
//! recién después la tarea se pone a esperar... un aviso que ya pasó ("lost wakeup").
//!
//! Referencias: <https://wiki.osdev.org/Scheduling_Algorithms>, "Operating Systems: Three Easy
//! Pieces" (caps. 7–9, planificación, y 26–30, concurrencia).

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

/// Cuántas tareas puede haber a la vez. Es un arreglo fijo a propósito: el planificador corre
/// dentro de la interrupción del timer, y ahí no se puede pedir memoria (el heap tiene un lock
/// que la tarea interrumpida podría tener tomado).
pub const MAX_TASKS: usize = 16;

/// Cuánto puede correr una tarea antes de dejarle la CPU a otra de su misma prioridad.
pub const QUANTUM_MS: u64 = 10;

/// Número de tarea (su lugar en la tabla).
pub type TaskId = usize;

/// Las tareas más importantes corren primero; entre iguales se turnan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Solo cuando no hay nada más (la tarea ociosa: `hlt`).
    Idle,
    Normal,
    /// Poco trabajo y urgente (la red: atender el paquete que llegó y volver a esperar).
    High,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Ready,
    /// Esperando alguno de los eventos de `events`, o hasta `until` (en ms), lo que pase antes.
    Waiting {
        events: u32,
        until: Option<u64>,
    },
}

#[derive(Clone, Debug)]
pub struct Task {
    pub name: &'static str,
    pub priority: Priority,
    pub state: State,
    /// Tiempo de CPU usado, en las unidades del reloj que pasa el kernel (ciclos del TSC).
    pub cpu: u64,
    /// Veces que le tocó la CPU.
    pub runs: u64,
    /// Qué eventos la despertaron la última vez (0: se cumplió el plazo).
    woke: u32,
}

pub struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],
    current: TaskId,
    /// Eventos que llegaron sin nadie esperándolos.
    latched: u32,
    /// Cuándo empezó el turno de la tarea actual (ms).
    slice_start: u64,
    /// Lectura del reloj en el último cambio (para repartir el tiempo de CPU).
    last_clock: u64,
    switches: u64,
}

impl Scheduler {
    /// El planificador arranca con una tarea: la que lo crea (en el kernel, la del arranque, que
    /// sigue como el escritorio).
    pub fn new(name: &'static str, priority: Priority, now_ms: u64, clock: u64) -> Scheduler {
        let mut tasks = [const { None }; MAX_TASKS];
        tasks[0] = Some(Task {
            name,
            priority,
            state: State::Running,
            cpu: 0,
            runs: 1,
            woke: 0,
        });
        Scheduler {
            tasks,
            current: 0,
            latched: 0,
            slice_start: now_ms,
            last_clock: clock,
            switches: 0,
        }
    }

    /// Anota una tarea nueva, lista para correr. `None` si la tabla está llena.
    pub fn spawn(&mut self, name: &'static str, priority: Priority) -> Option<TaskId> {
        let id = self.tasks.iter().position(Option::is_none)?;
        self.tasks[id] = Some(Task {
            name,
            priority,
            state: State::Ready,
            cpu: 0,
            runs: 0,
            woke: 0,
        });
        Some(id)
    }

    pub fn current(&self) -> TaskId {
        self.current
    }

    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.get(id)?.as_ref()
    }

    /// Las tareas vivas, con su número.
    pub fn tasks(&self) -> impl Iterator<Item = (TaskId, &Task)> {
        self.tasks
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.as_ref().map(|t| (i, t)))
    }

    /// Cambios de contexto desde el arranque.
    pub fn switches(&self) -> u64 {
        self.switches
    }

    fn cur(&mut self) -> &mut Task {
        // La tarea actual siempre existe: `exit` nunca deja `current` apuntando a un lugar
        // vacío sin elegir otra en `schedule`.
        self.tasks[self.current]
            .as_mut()
            .expect("la tarea actual no existe")
    }

    /// La tarea actual quiere esperar `events` o hasta `until`. Si ya se puede seguir (el evento
    /// había llegado, o el plazo pasó) devuelve `Some(eventos)` y no cambia nada; si no, la deja
    /// esperando y devuelve `None`: el kernel tiene que llamar a [`schedule`](Self::schedule).
    pub fn wait(&mut self, events: u32, until: Option<u64>, now_ms: u64) -> Option<u32> {
        let ready = self.latched & events;
        if ready != 0 {
            self.latched &= !ready;
            return Some(ready);
        }
        if until.is_some_and(|t| t <= now_ms) {
            return Some(0);
        }
        self.cur().state = State::Waiting { events, until };
        None
    }

    /// Saca una tarea que no es la actual (por ejemplo, si no se le pudo dar una pila).
    pub fn remove(&mut self, id: TaskId) {
        if id != self.current && id < MAX_TASKS {
            self.tasks[id] = None;
        }
    }

    /// Los eventos que despertaron a la tarea actual (lo que devuelve su espera).
    pub fn woke(&self) -> u32 {
        self.task(self.current).map_or(0, |t| t.woke)
    }

    /// La tarea actual termina. Hay que llamar a `schedule` enseguida.
    pub fn exit(&mut self) {
        self.tasks[self.current] = None;
    }

    /// Llegaron `events` (normalmente desde una interrupción). Despierta a todas las tareas que
    /// esperaban alguno; los que nadie esperaba quedan anotados. Devuelve si conviene desalojar a
    /// la tarea actual (se despertó una más importante).
    pub fn signal(&mut self, events: u32) -> bool {
        let mut unclaimed = events;
        let mut preempt = false;
        let current = self.current_priority();
        for task in self.tasks.iter_mut().flatten() {
            if let State::Waiting { events: want, .. } = task.state
                && want & events != 0
            {
                task.state = State::Ready;
                task.woke = want & events;
                unclaimed &= !want;
                preempt |= current.is_none_or(|p| task.priority > p);
            }
        }
        self.latched |= unclaimed;
        preempt
    }

    fn current_priority(&self) -> Option<Priority> {
        self.task(self.current)
            .filter(|t| t.state == State::Running)
            .map(|t| t.priority)
    }

    /// Despierta a las que esperaban hasta `now_ms`. Devuelve si alguna es más importante que la
    /// actual.
    fn wake_expired(&mut self, now_ms: u64) -> bool {
        let current = self.current_priority();
        let mut preempt = false;
        for task in self.tasks.iter_mut().flatten() {
            if let State::Waiting { until: Some(t), .. } = task.state
                && t <= now_ms
            {
                task.state = State::Ready;
                task.woke = 0;
                preempt |= current.is_none_or(|p| task.priority > p);
            }
        }
        preempt
    }

    /// La interrupción del timer. Devuelve si hay que cambiar de tarea: despertó una más
    /// importante, o la actual agotó su turno y hay otra de su prioridad (o más) esperando.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        if self.wake_expired(now_ms) {
            return true;
        }
        let Some(p) = self.current_priority() else {
            return true;
        };
        now_ms.saturating_sub(self.slice_start) >= QUANTUM_MS
            && self.tasks.iter().enumerate().any(|(i, t)| {
                i != self.current
                    && t.as_ref()
                        .is_some_and(|t| t.state == State::Ready && t.priority >= p)
            })
    }

    /// Elige la próxima tarea: la lista más importante; entre iguales, la primera *después* de
    /// la actual (así se turnan en ronda). Le cobra a la actual el tiempo de CPU hasta `clock`.
    /// Devuelve `(de, a)` si hay que cambiar de contexto, o `None` si sigue la misma.
    pub fn schedule(&mut self, now_ms: u64, clock: u64) -> Option<(TaskId, TaskId)> {
        self.wake_expired(now_ms);
        let from = self.current;
        if let Some(t) = self.tasks[from].as_mut() {
            t.cpu += clock.saturating_sub(self.last_clock);
        }
        self.last_clock = clock;
        // La actual, si puede seguir, compite como una más (y queda última en su ronda).
        if let Some(t) = self.tasks[from].as_mut()
            && t.state == State::Running
        {
            t.state = State::Ready;
        }
        let mut best: Option<(TaskId, Priority)> = None;
        for k in 1..=MAX_TASKS {
            let i = (from + k) % MAX_TASKS;
            if let Some(t) = &self.tasks[i]
                && t.state == State::Ready
                && best.is_none_or(|(_, p)| t.priority > p)
            {
                best = Some((i, t.priority));
            }
        }
        let Some((to, _)) = best else {
            // Nadie puede correr (en el kernel no pasa: la tarea ociosa siempre está lista).
            return None;
        };
        let t = self.tasks[to]
            .as_mut()
            .expect("elegida entre las que existen");
        t.state = State::Running;
        self.slice_start = now_ms;
        if to == from {
            return None;
        }
        t.runs += 1;
        self.current = to;
        self.switches += 1;
        Some((from, to))
    }

    /// Le cobra a la tarea actual el tiempo hasta `clock` sin cambiar de tarea (para mostrar
    /// números al día).
    pub fn charge(&mut self, clock: u64) {
        let used = clock.saturating_sub(self.last_clock);
        self.last_clock = clock;
        if let Some(t) = self.tasks[self.current].as_mut() {
            t.cpu += used;
        }
    }

    /// Foto de las tareas (para `ps` y el Monitor).
    pub fn snapshot(&self) -> Vec<(TaskId, Task)> {
        self.tasks().map(|(i, t)| (i, t.clone())).collect()
    }
}
