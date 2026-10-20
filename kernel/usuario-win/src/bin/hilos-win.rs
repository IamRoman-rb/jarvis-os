//! Hilos en un programa de Windows hecho con Rust: la biblioteca estándar usa CreateThread, el
//! TLS de Windows (`thread_local!`), WaitOnAddress (Mutex, Condvar) y WaitForSingleObject (join).

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
    let total = Arc::new(Mutex::new(0u64));
    let hilos: Vec<_> = (0..4)
        .map(|i| {
            let total = Arc::clone(&total);
            thread::spawn(move || {
                MIO.with(|m| m.set(i + 10));
                for _ in 0..1000 {
                    *total.lock().unwrap() += 1;
                }
                MIO.with(|m| m.get())
            })
        })
        .collect();
    let propios: Vec<u64> = hilos.into_iter().map(|h| h.join().unwrap()).collect();
    println!("win mutex: {}, tls: {propios:?}", *total.lock().unwrap());

    let par = Arc::new((Mutex::new(false), Condvar::new()));
    let par2 = Arc::clone(&par);
    let h = thread::spawn(move || {
        let (m, c) = &*par2;
        let mut listo = m.lock().unwrap();
        while !*listo {
            listo = c.wait(listo).unwrap();
        }
        "win condvar ok"
    });
    thread::sleep(Duration::from_millis(30));
    {
        let (m, c) = &*par;
        *m.lock().unwrap() = true;
        c.notify_one();
    }
    println!("{}", h.join().unwrap());

    let (tx, rx) = mpsc::channel();
    let p = thread::spawn(move || {
        for i in 1..=4 {
            tx.send(i * 10).unwrap();
        }
    });
    let v: Vec<i32> = rx.iter().collect();
    p.join().unwrap();
    println!("win canal: {v:?}");

    let inicio = Instant::now();
    let ds: Vec<_> = (0..3)
        .map(|_| thread::spawn(|| thread::sleep(Duration::from_millis(200))))
        .collect();
    for d in ds {
        d.join().unwrap();
    }
    println!(
        "win paralelo: {}",
        if inicio.elapsed().as_millis() < 500 { "ok" } else { "lento" }
    );

    let c = Arc::new(AtomicU64::new(0));
    let hs: Vec<_> = (0..6)
        .map(|_| {
            let c = Arc::clone(&c);
            thread::spawn(move || {
                for _ in 0..300 {
                    c.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    println!("win atomicos: {}", c.load(Ordering::SeqCst));
    println!("HILOS_WIN_OK");
}
