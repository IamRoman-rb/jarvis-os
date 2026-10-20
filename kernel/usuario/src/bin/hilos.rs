//! Hilos de verdad en JARVIS-OS (K15): `clone` con memoria compartida y `futex`. Varios hilos
//! que suman con un `Mutex`, se avisan con una `Condvar` y con un canal, duermen y se esperan
//! con `join`. Cada uno tiene su propio TLS.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

thread_local! {
    static MIO: Cell<u64> = const { Cell::new(0) };
}

fn main() {
    // 1. Cuatro hilos suman 1000 veces cada uno con un Mutex.
    let total = Arc::new(Mutex::new(0u64));
    let hilos: Vec<_> = (0..4)
        .map(|i| {
            let total = Arc::clone(&total);
            thread::spawn(move || {
                MIO.with(|m| m.set(i));
                for _ in 0..1000 {
                    *total.lock().unwrap() += 1;
                }
                // El TLS es de cada hilo: nadie más lo cambió.
                MIO.with(|m| m.get())
            })
        })
        .collect();
    let propios: Vec<u64> = hilos.into_iter().map(|h| h.join().unwrap()).collect();
    println!("mutex: {} (esperado 4000), tls: {propios:?}", *total.lock().unwrap());

    // 2. Una Condvar: el hilo espera a que el principal le dé permiso.
    let par = Arc::new((Mutex::new(false), Condvar::new()));
    let par2 = Arc::clone(&par);
    let h = thread::spawn(move || {
        let (m, c) = &*par2;
        let mut listo = m.lock().unwrap();
        while !*listo {
            listo = c.wait(listo).unwrap();
        }
        "condvar ok"
    });
    thread::sleep(Duration::from_millis(50));
    {
        let (m, c) = &*par;
        *m.lock().unwrap() = true;
        c.notify_one();
    }
    println!("{}", h.join().unwrap());

    // 3. Un canal entre productor y consumidor.
    let (tx, rx) = mpsc::channel();
    let productor = thread::spawn(move || {
        for i in 1..=5 {
            tx.send(i * i).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
    });
    let recibidos: Vec<i32> = rx.iter().collect();
    productor.join().unwrap();
    println!("canal: {recibidos:?}");

    // 4. Dormir en paralelo: tres hilos de 200 ms tardan ~200 ms, no 600.
    let inicio = Instant::now();
    let dormidos: Vec<_> = (0..3)
        .map(|_| thread::spawn(|| thread::sleep(Duration::from_millis(200))))
        .collect();
    for d in dormidos {
        d.join().unwrap();
    }
    let ms = inicio.elapsed().as_millis();
    println!("dormir en paralelo: {}", if ms < 500 { "ok" } else { "lento" });

    // 5. Atómicos entre hilos.
    let contador = Arc::new(AtomicU64::new(0));
    let hs: Vec<_> = (0..8)
        .map(|_| {
            let c = Arc::clone(&contador);
            thread::spawn(move || {
                for _ in 0..500 {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    println!("atomicos: {}", contador.load(Ordering::SeqCst));
    println!("HILOS_OK");
}
