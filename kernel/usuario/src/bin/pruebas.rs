//! Pruebas del espacio de usuario (las corre `cargo xtask test`): archivos, carpetas, memoria y
//! lo que pasa cuando un programa se porta mal.
//!
//! - `pruebas` (sin argumentos): archivos y memoria. Termina con "PRUEBAS_OK" o el error.
//! - `pruebas segv`: lee un puntero nulo (el kernel lo tiene que terminar, no caerse).
//! - `pruebas bucle`: calcula para siempre (Ctrl+C lo tiene que poder terminar).
//! - `pruebas abort`: `abort()` (SIGABRT).

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};

fn check(ok: bool, what: &str) -> Result<(), String> {
    if ok {
        println!("  ok: {what}");
        Ok(())
    } else {
        Err(what.to_string())
    }
}

fn files() -> Result<(), String> {
    let e = |e: std::io::Error| e.to_string();
    let dir = "prueba-k11";
    let _ = fs::remove_dir_all(dir);
    fs::create_dir(dir).map_err(e)?;
    let path = format!("{dir}/datos.txt");
    fs::write(&path, "uno\ndos\n").map_err(e)?;
    let mut f = fs::OpenOptions::new().append(true).open(&path).map_err(e)?;
    f.write_all(b"tres\n").map_err(e)?;
    drop(f);
    let text = fs::read_to_string(&path).map_err(e)?;
    check(text == "uno\ndos\ntres\n", "escribir, agregar y leer")?;
    let mut f = fs::File::open(&path).map_err(e)?;
    f.seek(SeekFrom::Start(4)).map_err(e)?;
    let mut rest = String::new();
    f.read_to_string(&mut rest).map_err(e)?;
    check(rest == "dos\ntres\n", "seek")?;
    check(
        fs::metadata(&path).map_err(e)?.len() == 13,
        "metadata (tamaño)",
    )?;
    fs::rename(&path, format!("{dir}/renombrado.txt")).map_err(e)?;
    let names: Vec<String> = fs::read_dir(dir)
        .map_err(e)?
        .filter_map(|d| d.ok())
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .collect();
    check(names == ["renombrado.txt"], "rename y read_dir")?;
    check(
        fs::read(format!("{dir}/no-existe")).is_err(),
        "un archivo que no existe da error",
    )?;
    fs::remove_file(format!("{dir}/renombrado.txt")).map_err(e)?;
    fs::remove_dir(dir).map_err(e)?;
    check(fs::metadata(dir).is_err(), "borrar (a la Papelera) y rmdir")?;
    Ok(())
}

fn memory() -> Result<(), String> {
    // 32 MiB en el heap (mmap de musl), tocando cada página.
    let mut big = vec![0u8; 32 << 20];
    for (i, b) in big.iter_mut().enumerate().step_by(4096) {
        *b = (i >> 12) as u8;
    }
    let sum: u64 = big.iter().step_by(4096).map(|&b| b as u64).sum();
    check(sum == (0..8192u64).map(|i| i & 0xFF).sum::<u64>(), "32 MiB de memoria")?;
    drop(big);
    // Muchas asignaciones chicas (brk).
    let v: Vec<String> = (0..20_000).map(|i| format!("cadena {i}")).collect();
    check(v[19_999] == "cadena 19999", "20000 asignaciones chicas")?;
    // Punto flotante (SSE): el kernel guarda los registros XMM de cada programa.
    let x: f64 = (1..=1000).map(|i| 1.0 / (i as f64 * i as f64)).sum();
    check((x - 1.643_934_566_681_56).abs() < 1e-9, "punto flotante")?;
    // Una recursión que usa la pila (páginas que aparecen al primer uso).
    fn depth(n: u32) -> u32 {
        let buf = [n as u8; 512];
        if n == 0 { buf[0] as u32 } else { depth(n - 1) + buf[511] as u32 / 256 }
    }
    check(depth(4000) == 0, "4000 llamadas anidadas (2 MiB de pila)")?;
    Ok(())
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("segv") => {
            println!("leyendo un puntero nulo...");
            // SAFETY: no lo es: es a propósito, para ver que el kernel termina el programa.
            let v = unsafe { std::ptr::read_volatile(std::ptr::null::<u64>()) };
            println!("no deberia llegar aca: {v}");
        }
        Some("bucle") => {
            println!("calculando para siempre (Ctrl+C lo termina)");
            let mut x: u64 = 1;
            loop {
                x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                std::hint::black_box(x);
            }
        }
        Some("abort") => std::process::abort(),
        _ => {
            println!("pruebas del espacio de usuario:");
            match files().and_then(|()| memory()) {
                Ok(()) => println!("PRUEBAS_OK"),
                Err(e) => {
                    println!("PRUEBAS_ERROR {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}
