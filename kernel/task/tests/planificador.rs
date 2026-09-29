//! El planificador (K9): turnos, prioridades, esperas y eventos.

use jarvis_task::{Priority, QUANTUM_MS, Scheduler, State};

const DISK: u32 = 1;
const NET: u32 = 2;

/// Un kernel en miniatura: el escritorio (tarea 0), la red y la ociosa.
fn kernel() -> Scheduler {
    let mut s = Scheduler::new("escritorio", Priority::Normal, 0, 0);
    assert_eq!(s.spawn("ociosa", Priority::Idle), Some(1));
    assert_eq!(s.spawn("red", Priority::High), Some(2));
    s
}

#[test]
fn la_tarea_mas_importante_corre_primero() {
    let mut s = kernel();
    // La red está lista y es más importante que el escritorio.
    assert_eq!(s.schedule(0, 0), Some((0, 2)));
    assert_eq!(s.current(), 2);
    // La red se pone a esperar un paquete: vuelve el escritorio, no la ociosa.
    assert_eq!(s.wait(NET, None, 1), None);
    assert_eq!(s.schedule(1, 10), Some((2, 0)));
}

#[test]
fn la_ociosa_solo_corre_si_no_hay_nada_mas() {
    let mut s = kernel();
    s.schedule(0, 0); // red
    s.wait(NET, None, 0);
    s.schedule(0, 0); // escritorio
    assert_eq!(s.wait(0, Some(16), 0), None);
    assert_eq!(s.schedule(0, 0), Some((0, 1)));
    assert_eq!(s.task(1).unwrap().state, State::Running);
}

#[test]
fn un_evento_despierta_y_desaloja_si_es_mas_importante() {
    let mut s = kernel();
    s.schedule(0, 0); // red
    s.wait(NET, None, 0);
    s.schedule(0, 0); // escritorio corriendo
    // Llega un paquete: la red es más importante que el escritorio.
    assert!(s.signal(NET));
    assert_eq!(s.schedule(1, 0), Some((0, 2)));
    assert_eq!(s.woke(), NET);
}

#[test]
fn un_evento_que_no_es_urgente_no_desaloja() {
    let mut s = kernel();
    s.schedule(0, 0); // red corriendo
    // El escritorio espera el disco; el disco avisa mientras corre la red (más importante).
    s.wait(NET, None, 0);
    s.schedule(0, 0); // escritorio
    s.wait(DISK, None, 0);
    s.schedule(0, 0); // ociosa
    assert!(s.signal(DISK), "desde la ociosa, cualquiera desaloja");
    s.schedule(0, 0);
    assert_eq!(s.current(), 0);
    s.signal(NET);
    s.schedule(0, 0);
    assert_eq!(s.current(), 2);
    s.wait(NET, None, 0);
    s.schedule(0, 0);
    s.wait(DISK, None, 0);
    s.schedule(0, 0);
    // Ahora corre la ociosa; si la red está corriendo, el disco no la desaloja.
    s.signal(NET);
    s.schedule(0, 0);
    assert_eq!(s.current(), 2);
    assert!(!s.signal(DISK));
}

#[test]
fn un_aviso_que_llega_antes_de_esperar_no_se_pierde() {
    let mut s = kernel();
    s.schedule(0, 0);
    s.wait(NET, None, 0);
    s.schedule(0, 0); // escritorio
    // El disco termina antes de que el escritorio se ponga a esperar.
    assert!(!s.signal(DISK));
    assert_eq!(s.wait(DISK, None, 1), Some(DISK), "vuelve enseguida");
    // Y el aviso se consumió: la próxima espera sí bloquea.
    assert_eq!(s.wait(DISK, None, 1), None);
}

#[test]
fn el_plazo_despierta_con_cero_eventos() {
    let mut s = kernel();
    s.schedule(0, 0);
    s.wait(NET, None, 0);
    s.schedule(0, 0); // escritorio
    s.wait(DISK, Some(16), 0);
    s.schedule(0, 0); // ociosa
    assert!(!s.tick(15));
    assert!(
        s.tick(16),
        "se cumplió el plazo: el escritorio le gana a la ociosa"
    );
    assert_eq!(s.schedule(16, 0), Some((1, 0)));
    assert_eq!(s.woke(), 0);
    // Un plazo que ya pasó no bloquea.
    assert_eq!(s.wait(DISK, Some(10), 16), Some(0));
}

#[test]
fn las_de_igual_prioridad_se_turnan() {
    let mut s = Scheduler::new("a", Priority::Normal, 0, 0);
    s.spawn("b", Priority::Normal);
    s.spawn("c", Priority::Normal);
    // Dentro del turno no se cambia.
    assert!(!s.tick(QUANTUM_MS - 1));
    let mut order = Vec::new();
    let mut now = 0;
    for _ in 0..6 {
        now += QUANTUM_MS;
        assert!(s.tick(now));
        s.schedule(now, now);
        order.push(s.current());
    }
    assert_eq!(order, [1, 2, 0, 1, 2, 0]);
}

#[test]
fn sola_no_cambia_nunca() {
    let mut s = Scheduler::new("a", Priority::Normal, 0, 0);
    s.spawn("ociosa", Priority::Idle);
    assert!(!s.tick(1000));
    assert_eq!(s.schedule(1000, 0), None);
    assert_eq!(s.current(), 0);
}

#[test]
fn un_evento_despierta_a_todas_las_que_lo_esperan() {
    let mut s = Scheduler::new("a", Priority::Normal, 0, 0);
    s.spawn("b", Priority::Normal);
    s.spawn("ociosa", Priority::Idle);
    s.wait(NET, None, 0);
    s.schedule(0, 0); // b
    s.wait(NET | DISK, None, 0);
    s.schedule(0, 0); // ociosa
    s.signal(NET);
    assert_eq!(s.task(0).unwrap().state, State::Ready);
    assert_eq!(s.task(1).unwrap().state, State::Ready);
    s.schedule(0, 0);
    assert_eq!(s.woke(), NET);
}

#[test]
fn el_tiempo_de_cpu_se_cobra_a_quien_corrio() {
    let mut s = kernel();
    s.schedule(0, 100); // escritorio: 100 ciclos → red
    s.wait(NET, None, 0);
    s.schedule(0, 130); // red: 30 → escritorio
    s.charge(200); // escritorio: 70 más
    assert_eq!(s.task(0).unwrap().cpu, 170);
    assert_eq!(s.task(2).unwrap().cpu, 30);
    assert_eq!(s.task(2).unwrap().runs, 1);
    assert_eq!(s.switches(), 2);
}

#[test]
fn la_tabla_tiene_un_tope_y_se_reusa() {
    let mut s = Scheduler::new("a", Priority::Normal, 0, 0);
    for _ in 1..jarvis_task::MAX_TASKS {
        assert!(s.spawn("x", Priority::Normal).is_some());
    }
    assert_eq!(s.spawn("sobra", Priority::Normal), None);
    // Una tarea que termina libera su lugar.
    s.exit();
    assert!(s.schedule(0, 0).is_some());
    assert_eq!(s.spawn("nueva", Priority::Normal), Some(0));
    assert_eq!(s.snapshot().len(), jarvis_task::MAX_TASKS);
    // Sacar una que no corre también (la actual no se puede sacar así).
    s.remove(3);
    s.remove(s.current());
    assert_eq!(s.snapshot().len(), jarvis_task::MAX_TASKS - 1);
    assert_eq!(s.spawn("otra", Priority::Normal), Some(3));
}

#[test]
fn sin_nadie_listo_no_elige() {
    let mut s = Scheduler::new("a", Priority::Normal, 0, 0);
    assert_eq!(s.wait(DISK, None, 0), None);
    assert_eq!(s.schedule(0, 0), None);
    // Cuando llega el evento, sigue la misma tarea.
    s.signal(DISK);
    assert_eq!(s.schedule(0, 0), None);
    assert_eq!(s.task(0).unwrap().state, State::Running);
    assert_eq!(s.woke(), DISK);
}
