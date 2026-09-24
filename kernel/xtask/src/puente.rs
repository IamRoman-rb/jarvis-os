//! Puente HTTPS del anfitrión.
//!
//! El kernel de JARVIS-OS ya tiene red propia (driver virtio-net, TCP/IP, DHCP y DNS) y descarga
//! páginas `http://` directamente. Lo que todavía no tiene es **TLS**, el cifrado de `https://`
//! (casi toda la web). Hasta que lo tenga, esas páginas se le piden a este puente, que corre en
//! la computadora anfitriona mientras dura `cargo xtask run`:
//!
//! ```text
//! JARVIS-OS ──HTTP──▶ 10.0.2.2:8118 (QEMU lo lleva al 127.0.0.1 del anfitrión) ──HTTPS──▶ sitio
//! ```
//!
//! El pedido llega como a un proxy HTTP (`GET https://sitio/camino HTTP/1.1`). El puente hace la
//! conexión HTTPS (verificando el certificado del sitio), y devuelve la respuesta en texto plano
//! y ya descomprimida. Solo acepta GET y solo escucha en 127.0.0.1: desde otra máquina no se
//! puede usar. Las redirecciones no las sigue: se las devuelve al kernel, que las maneja.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

pub const PORT: u16 = 8118;
const MAX_BODY: u64 = 8 * 1024 * 1024;

/// Levanta el puente en un hilo. Si el puerto está ocupado (otro `xtask run` abierto), avisa y
/// sigue sin puente.
pub fn start() {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "[puente] no pude escuchar en 127.0.0.1:{PORT} ({e}): sin HTTPS en el navegador"
            );
            return;
        }
    };
    println!("[puente] HTTPS para el navegador de JARVIS-OS en 127.0.0.1:{PORT}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(25)))
        .user_agent("Mozilla/5.0 (compatible; JARVIS-OS/0.1; navegador de texto)")
        .build()
        .into();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let agent = agent.clone();
            thread::spawn(move || {
                if let Err(e) = handle(stream, &agent) {
                    eprintln!("[puente] {e}");
                }
            });
        }
    });
}

fn read_head(stream: &mut TcpStream) -> Result<String, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 || head.len() > 16 * 1024 {
            return Err("pedido incompleto".into());
        }
        head.extend_from_slice(&buf[..n]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

fn reply(stream: &mut TcpStream, status: u16, content_type: &str, extra: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

fn handle(mut stream: TcpStream, agent: &ureq::Agent) -> Result<(), String> {
    let head = read_head(&mut stream)?;
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let (method, url) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "GET" || !(url.starts_with("https://") || url.starts_with("http://")) {
        reply(
            &mut stream,
            405,
            "text/plain",
            "",
            b"El puente solo acepta GET a direcciones completas.",
        );
        return Err(format!("pedido rechazado: {line}"));
    }
    let lang = head
        .lines()
        .find_map(|l| l.strip_prefix("Accept-Language: "))
        .unwrap_or("es-AR,es;q=0.9");
    match agent.get(url).header("Accept-Language", lang).call() {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let header = |name: &str| {
                resp.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            };
            let content_type = header("content-type");
            let location = header("location");
            let body = resp
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()
                .map_err(|e| format!("{url}: {e}"))?;
            println!("[puente] GET {url} -> {status} ({} bytes)", body.len());
            let extra = if location.is_empty() {
                String::new()
            } else {
                format!("Location: {location}\r\n")
            };
            reply(&mut stream, status, &content_type, &extra, &body);
            Ok(())
        }
        Err(e) => {
            let msg = format!("El puente no pudo abrir {url}: {e}");
            println!("[puente] {msg}");
            reply(
                &mut stream,
                502,
                "text/plain; charset=utf-8",
                "",
                msg.as_bytes(),
            );
            Ok(())
        }
    }
}

/// Servidor HTTP de prueba (para los tests y las capturas): contesta siempre la misma página.
pub fn test_server(html: &'static str) -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            if read_head(&mut stream).is_ok() {
                reply(
                    &mut stream,
                    200,
                    "text/html; charset=utf-8",
                    "",
                    html.as_bytes(),
                );
            }
        }
    });
    Ok(port)
}
