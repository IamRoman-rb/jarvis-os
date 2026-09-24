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
//!
//! Además (ADR 0005):
//! - **Imágenes**: si el pedido trae `X-Jarvis-Imagen: bmp`, la imagen (PNG, JPEG, GIF, WebP…)
//!   se convierte a BMP, el único formato que el kernel sabe leer, y se achica si es enorme.
//! - **Repositorio de paquetes**: `http://paquetes.jarvis/…` se sirve desde la carpeta
//!   `kernel/paquetes/` del proyecto (solo lectura, sin salir de esa carpeta).

use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub const PORT: u16 = 8118;
const MAX_BODY: u64 = 32 * 1024 * 1024;
/// Lado más largo de las imágenes de la web convertidas (las del repositorio, como los fondos
/// de pantalla, pueden ser más grandes).
const MAX_SIDE: u32 = 900;
const MAX_SIDE_REPO: u32 = 1920;
pub const REPO_HOST: &str = "paquetes.jarvis";

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
    println!("[puente] HTTPS, imágenes y paquetes para JARVIS-OS en 127.0.0.1:{PORT}");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(40)))
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

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
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
    let want_bmp = header(&head, "X-Jarvis-Imagen").is_some_and(|v| v.eq_ignore_ascii_case("bmp"));
    // El repositorio de paquetes: archivos de kernel/paquetes/.
    if let Some(path) = url
        .strip_prefix(&format!("http://{REPO_HOST}/"))
        .or_else(|| (url == format!("http://{REPO_HOST}")).then_some(""))
    {
        let (status, ct, body) = serve_repo(path, want_bmp);
        println!(
            "[puente] paquetes: {path} -> {status} ({} bytes)",
            body.len()
        );
        reply(&mut stream, status, ct, "", &body);
        return Ok(());
    }
    let lang = header(&head, "Accept-Language").unwrap_or("es-AR,es;q=0.9");
    let accept = header(&head, "Accept").unwrap_or("*/*");
    match agent
        .get(url)
        .header("Accept-Language", lang)
        .header("Accept", accept)
        .call()
    {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let get = |name: &str| {
                resp.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            };
            let mut content_type = get("content-type");
            let location = get("location");
            let mut body = resp
                .body_mut()
                .with_config()
                .limit(MAX_BODY)
                .read_to_vec()
                .map_err(|e| format!("{url}: {e}"))?;
            if want_bmp && status == 200 {
                match to_bmp(&body, MAX_SIDE) {
                    Ok(bmp) => {
                        body = bmp;
                        content_type = "image/bmp".into();
                    }
                    Err(e) => {
                        println!("[puente] {url}: no se pudo convertir la imagen: {e}");
                        reply(&mut stream, 415, "text/plain", "", e.as_bytes());
                        return Ok(());
                    }
                }
            }
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

/// Cualquier imagen → BMP de 24 bits (con la transparencia sobre blanco), achicada si hace falta.
pub fn to_bmp(data: &[u8], max_side: u32) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(data)
        .map_err(|e| format!("formato de imagen no soportado: {e}"))?;
    let img = if img.width().max(img.height()) > max_side {
        img.thumbnail(max_side, max_side)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let mut rgb = image::RgbImage::new(rgba.width(), rgba.height());
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])]));
    }
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(rgb)
        .write_to(&mut out, image::ImageFormat::Bmp)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

pub fn repo_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("paquetes"))
        .unwrap_or_else(|| PathBuf::from("paquetes"))
}

/// Un archivo del repositorio. Solo nombres simples (letras, números, `-_.` y `/`), sin `..`:
/// no se puede leer nada fuera de `kernel/paquetes/`.
fn serve_repo(path: &str, want_bmp: bool) -> (u16, &'static str, Vec<u8>) {
    let path = if path.is_empty() { "indice.txt" } else { path };
    let safe = !path.contains("..")
        && !path.starts_with('/')
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if !safe {
        return (400, "text/plain", b"nombre de archivo invalido".to_vec());
    }
    let file = repo_dir().join(path);
    match std::fs::read(&file) {
        Ok(data) if want_bmp => match to_bmp(&data, MAX_SIDE_REPO) {
            Ok(bmp) => (200, "image/bmp", bmp),
            Err(e) => (415, "text/plain", e.into_bytes()),
        },
        Ok(data) => {
            let ct = if path.ends_with(".txt") || path.ends_with(".sh") || path.ends_with(".md") {
                "text/plain; charset=utf-8"
            } else {
                "application/octet-stream"
            };
            (200, ct, data)
        }
        Err(_) => (404, "text/plain", format!("no existe {path}").into_bytes()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn el_repositorio_no_deja_salir_de_su_carpeta() {
        for bad in [
            "../Cargo.toml",
            "/etc/passwd",
            "a/../../x",
            "c:\\x",
            "%2e%2e/x",
        ] {
            assert_eq!(serve_repo(bad, false).0, 400, "{bad}");
        }
        let (status, _, body) = serve_repo("", false);
        assert_eq!(status, 200, "sirve el índice");
        assert!(String::from_utf8_lossy(&body).contains("neofetch|"));
        assert_eq!(serve_repo("no-existe.txt", false).0, 404);
    }

    #[test]
    fn convierte_png_a_bmp() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2000,
            10,
            image::Rgba([255, 0, 0, 0]),
        ))
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        let bmp = to_bmp(&png, MAX_SIDE).unwrap();
        assert!(bmp.starts_with(b"BM"));
        let back = image::load_from_memory(&bmp).unwrap().to_rgb8();
        assert_eq!(back.width(), MAX_SIDE, "se achica");
        assert_eq!(
            back.get_pixel(0, 0),
            &image::Rgb([255, 255, 255]),
            "transparente sobre blanco"
        );
        assert!(to_bmp(b"no es una imagen", MAX_SIDE).is_err());
    }
}
