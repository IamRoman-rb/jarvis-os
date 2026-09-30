//! El primer programa de Linux de JARVIS-OS: argumentos, entorno, hora y memoria dinámica, con
//! la biblioteca estándar de Rust sobre musl (nada escrito para JARVIS-OS).

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("Hola desde Linux! argumentos: {:?}", &args[1..]);
    println!(
        "usuario {}, carpeta {}",
        std::env::var("USER").unwrap_or_default(),
        std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default()
    );
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let v: Vec<u64> = (1..=100_000).collect();
    println!(
        "segundos desde 1970: {secs}; suma de 1 a 100000: {}",
        v.iter().sum::<u64>()
    );
}
