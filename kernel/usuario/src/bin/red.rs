//! Un cliente HTTP mínimo con `std::net::TcpStream`: `red IP:PUERTO [camino]`. Muestra la
//! primera línea de la respuesta. Los sockets de un programa pasan por el firewall de JARVIS-OS
//! (app `programas`): con `ufw deny out ... app programas` la conexión se rechaza.

use std::io::{Read, Write};
use std::net::TcpStream;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(addr) = args.get(1) else {
        eprintln!("uso: red IP:PUERTO [camino]");
        std::process::exit(2);
    };
    let path = args.get(2).map_or("/", |s| s.as_str());
    let mut s = match TcpStream::connect(addr.as_str()) {
        Ok(s) => s,
        Err(e) => {
            println!("RED_PROGRAMA_ERROR {e}");
            std::process::exit(1);
        }
    };
    let host = addr.split(':').next().unwrap_or(addr);
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if let Err(e) = s.write_all(req.as_bytes()) {
        println!("RED_PROGRAMA_ERROR {e}");
        std::process::exit(1);
    }
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let text = String::from_utf8_lossy(&body);
    println!(
        "RED_PROGRAMA {} ({} bytes)",
        text.lines().next().unwrap_or("(nada)"),
        body.len()
    );
}
