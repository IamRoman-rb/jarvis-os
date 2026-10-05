//! Un programa de Windows en Rust con la biblioteca estándar entera: salida y entrada por la
//! consola, argumentos, variables de entorno, archivos, carpetas, colecciones y la hora.

use std::collections::HashMap;
use std::io::{BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("Hola desde Rust en Windows! {} argumentos", args.len());
    for a in &args[1..] {
        println!("  arg: {a}");
    }
    let path = std::env::var("PATH").unwrap_or_default();
    println!("PATH tiene {} caracteres", path.len());

    let mut mapa = HashMap::new();
    for palabra in "uno dos tres dos uno dos".split(' ') {
        *mapa.entry(palabra).or_insert(0) += 1;
    }
    let mut v: Vec<_> = mapa.into_iter().collect();
    v.sort();
    println!("conteo: {v:?}");

    std::fs::create_dir_all("carpeta-rust").expect("crear carpeta");
    std::fs::write("carpeta-rust/datos.txt", "uno\ndos\ntres\n").expect("escribir");
    let leido = std::fs::read_to_string("carpeta-rust/datos.txt").expect("leer");
    println!("archivo: {} lineas", leido.lines().count());
    let n = std::fs::read_dir("carpeta-rust").expect("listar").count();
    println!("carpeta: {n} archivo(s)");

    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("hora: {}", if t > 1_700_000_000 { "ok" } else { "mal" });
    let inicio = std::time::Instant::now();
    let mut x = 0u64;
    for i in 0..100_000u64 {
        x = x.wrapping_mul(31).wrapping_add(i);
    }
    println!("calculo {x} en {} ms", inicio.elapsed().as_millis() < 5000);

    print!("Escribi algo: ");
    std::io::stdout().flush().ok();
    let mut linea = String::new();
    std::io::stdin().lock().read_line(&mut linea).ok();
    println!("Leido: {}", linea.trim());
}
