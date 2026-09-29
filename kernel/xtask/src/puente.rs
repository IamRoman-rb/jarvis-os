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
//!   `kernel/paquetes/` del proyecto (solo lectura, sin salir de esa carpeta), y
//!   `http://paquetes.jarvis/usuario/…` desde `target/usuario/`: los programas de Linux
//!   compilados desde `kernel/usuario/` (K11, ADR 0010).

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
    let mut req = agent
        .get(url)
        .header("Accept-Language", lang)
        .header("Accept", accept);
    // Pocas cabeceras más, y solo estas: la API de la tienda de snaps pide la serie del
    // dispositivo.
    for name in ["Snap-Device-Series", "Snap-Device-Architecture"] {
        if let Some(v) = header(&head, name) {
            req = req.header(name, v);
        }
    }
    match req.call() {
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

fn looks_like_svg(data: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&data[..data.len().min(1024)]).to_ascii_lowercase();
    head.contains("<svg")
}

/// Un SVG (dibujo vectorial) → imagen, al tamaño que dice (achicado si es enorme).
fn svg_to_image(data: &[u8], max_side: u32) -> Result<image::DynamicImage, String> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default())
        .map_err(|e| format!("SVG inválido: {e}"))?;
    let size = tree.size();
    let (w, h) = (size.width().max(1.0), size.height().max(1.0));
    let scale = (max_side as f32 / w.max(h)).min(1.0);
    let (pw, ph) = (
        ((w * scale).ceil() as u32).max(1),
        ((h * scale).ceil() as u32).max(1),
    );
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(pw, ph).ok_or_else(|| "SVG sin tamaño".to_string())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia guarda los colores premultiplicados por la opacidad: se deshace.
    let mut rgba = image::RgbaImage::new(pw, ph);
    for (i, px) in pixmap.pixels().iter().enumerate() {
        let c = px.demultiply();
        rgba.put_pixel(
            i as u32 % pw,
            i as u32 / pw,
            image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]),
        );
    }
    Ok(image::DynamicImage::ImageRgba8(rgba))
}

/// Cualquier imagen (PNG, JPEG, GIF, WebP, ICO, SVG) → BMP, achicada si hace falta. Si tiene
/// partes transparentes, BMP de 32 bits con canal alfa (el kernel la mezcla con el fondo de la
/// página); si no, de 24 bits.
pub fn to_bmp(data: &[u8], max_side: u32) -> Result<Vec<u8>, String> {
    let img = if looks_like_svg(data) {
        svg_to_image(data, max_side)?
    } else {
        image::load_from_memory(data).map_err(|e| format!("formato de imagen no soportado: {e}"))?
    };
    let img = if img.width().max(img.height()) > max_side {
        img.thumbnail(max_side, max_side)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    if rgba.pixels().all(|p| p[3] == 255) {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(rgba).to_rgb8())
            .write_to(&mut out, image::ImageFormat::Bmp)
            .map_err(|e| e.to_string())?;
        return Ok(out.into_inner());
    }
    Ok(bmp32(&rgba))
}

/// BMP de 32 bits (BGRA, filas de abajo hacia arriba) con cabecera clásica de 40 bytes.
fn bmp32(img: &image::RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width(), img.height());
    let data_len = w * h * 4;
    let mut out = Vec::with_capacity(54 + data_len as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data_len).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // sin compresión
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
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
    // Los programas de Linux compilados desde kernel/usuario/ (K11, ADR 0010).
    let file = match path.strip_prefix("usuario/") {
        Some(program) => super::target_dir().join("usuario").join(program),
        None => repo_dir().join(path),
    };
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
        let width = u32::from_le_bytes(bmp[18..22].try_into().unwrap());
        assert_eq!(width, MAX_SIDE, "se achica");
        assert_eq!(u16::from_le_bytes([bmp[28], bmp[29]]), 32, "con canal alfa");
        assert_eq!(bmp[54 + 3], 0, "el píxel es transparente");
        assert!(to_bmp(b"no es una imagen", MAX_SIDE).is_err());
        // Un SVG se dibuja.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="#00ff00"/></svg>"##;
        let bmp = to_bmp(svg, MAX_SIDE).unwrap();
        let img = image::load_from_memory(&bmp).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (20, 10));
        assert_eq!(img.get_pixel(5, 5), &image::Rgb([0, 255, 0]));
    }
}
