//! Lee líneas de la entrada estándar (la Terminal) y las devuelve en mayúsculas, hasta el fin
//! de la entrada (Ctrl+D).

use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    let mut n = 0;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        n += 1;
        let _ = writeln!(out, "{}", line.to_uppercase());
        let _ = out.flush();
    }
    println!("eco: {n} lineas");
}
