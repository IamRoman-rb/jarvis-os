//! Vista previa del navegador sin QEMU (herramienta de desarrollo, no corre en `cargo test`):
//!
//! ```text
//! cargo test -p jarvis-desktop --test vista_previa -- --ignored --nocapture
//! JARVIS_URL=https://es.wikipedia.org/wiki/Rust cargo test ... (otra página)
//! ```
//!
//! Baja la página y sus hojas de estilo con `curl` (en el anfitrión) y guarda cómo la arma el
//! navegador en `target/vista-previa.bmp`. Las imágenes no se bajan (se ven sus lugares).

mod common;

use std::process::Command;

use common::*;
use jarvis_desktop::{HttpResponse, Launch};

fn curl(url: &str) -> Result<HttpResponse, String> {
    let out = Command::new("curl")
        .args([
            "-s",
            "-L",
            "-A",
            "Mozilla/5.0 (compatible; JARVIS-OS/0.1; navegador de texto)",
        ])
        .args(["-w", "\n%{content_type}", url])
        .output()
        .map_err(|e| e.to_string())?;
    let mut body = out.stdout;
    let split = body.iter().rposition(|&b| b == b'\n').unwrap_or(body.len());
    let ct = String::from_utf8_lossy(&body[split..]).trim().to_string();
    body.truncate(split);
    Ok(HttpResponse {
        status: 200,
        content_type: ct,
        url: url.into(),
        body,
    })
}

#[test]
#[ignore]
fn vista_previa_de_una_pagina() {
    let url = std::env::var("JARVIS_URL").unwrap_or_else(|_| "https://www.google.com/".into());
    let mut t = Driver::new();
    t.d.open(Launch::Browse(url), t.now, CLOCK);
    for _ in 0..4 {
        let reqs = t.d.take_requests().net;
        for r in reqs {
            let resp = if r.kind == jarvis_desktop::FetchKind::Image {
                Err("sin imágenes en la vista previa".into())
            } else {
                curl(&r.url)
            };
            t.d.net_response(r.id, resp);
        }
    }
    if std::env::var("JARVIS_BLOQUES").is_ok()
        && let Some(jarvis_desktop::apps::App::Browser(b)) =
            t.d.app(jarvis_desktop::AppKind::Browser)
    {
        for (i, bl) in b.document().blocks.iter().enumerate().take(60) {
            let text: String = bl.spans.iter().map(|s| s.text.as_str()).collect();
            println!(
                "{i:3} {:?} ind={} bg={:?} {:?}",
                bl.kind,
                bl.indent,
                bl.bg,
                text.chars().take(50).collect::<String>()
            );
        }
    }
    // Maximizada, para ver más; JARVIS_BAJAR=N baja N pantallas.
    t.combo(jarvis_desktop::Mods::WIN, jarvis_desktop::Key::Up);
    let down: usize = std::env::var("JARVIS_BAJAR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for _ in 0..down {
        t.key(jarvis_desktop::Key::PageDown);
    }
    let (mut bg, mut fr) = buffers();
    let mut bgc = canvas(&mut bg);
    let mut frame = canvas(&mut fr);
    t.d.render(&mut frame, &mut bgc, t.now, CLOCK);
    let bmp = jarvis_desktop::bmp::encode(&frame);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/vista-previa.bmp");
    std::fs::write(&out, bmp).unwrap();
    println!("vista previa: {}", out.display());
}
