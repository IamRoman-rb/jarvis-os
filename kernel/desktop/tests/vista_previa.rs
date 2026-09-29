//! Vista previa del navegador sin QEMU (herramienta de desarrollo, no corre en `cargo test`):
//!
//! ```text
//! cargo test -p jarvis-desktop --test vista_previa -- --ignored --nocapture
//! JARVIS_URL=https://es.wikipedia.org/wiki/Rust cargo test ... (otra página)
//! ```
//!
//! Baja la página, sus hojas de estilo y sus imágenes con `curl` (en el anfitrión), convierte las
//! imágenes que no son PNG ni JPEG a BMP como el puente, y guarda cómo la arma el navegador en `target/vista-previa.bmp`.

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

/// Como `to_bmp` del puente (xtask/src/puente.rs), en chico.
fn to_bmp(data: &[u8]) -> Result<Vec<u8>, String> {
    let head = String::from_utf8_lossy(&data[..data.len().min(1024)]).to_ascii_lowercase();
    let img = if head.contains("<svg") {
        let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default())
            .map_err(|e| e.to_string())?;
        let s = tree.size();
        let (w, h) = (s.width().ceil() as u32, s.height().ceil() as u32);
        let mut pm =
            resvg::tiny_skia::Pixmap::new(w.clamp(1, 900), h.clamp(1, 900)).ok_or("svg")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pm.as_mut(),
        );
        let mut rgba = image::RgbaImage::new(pm.width(), pm.height());
        for (i, px) in pm.pixels().iter().enumerate() {
            let c = px.demultiply();
            rgba.put_pixel(
                i as u32 % pm.width(),
                i as u32 / pm.width(),
                image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]),
            );
        }
        image::DynamicImage::ImageRgba8(rgba)
    } else {
        image::load_from_memory(data).map_err(|e| e.to_string())?
    };
    let img = if img.width().max(img.height()) > 900 {
        img.thumbnail(900, 900)
    } else {
        img
    }
    .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let mut out = Vec::new();
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + w * h * 4).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let p = img.get_pixel(x, y);
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    Ok(out)
}

#[test]
#[ignore]
fn vista_previa_de_una_pagina() {
    let url = std::env::var("JARVIS_URL").unwrap_or_else(|_| "https://www.google.com/".into());
    let mut t = Driver::new();
    // JARVIS_IDIOMA=en|pt: la interfaz en otro idioma.
    if let Ok(l) = std::env::var("JARVIS_IDIOMA") {
        jarvis_desktop::i18n::set(jarvis_desktop::i18n::Lang::from_code(&l));
    }
    // JARVIS_URL=config:N abre la Configuración en la sección N; `archivos`, los Archivos.
    let what = match url.as_str() {
        u if u.starts_with("config:") => Launch::Settings(u[7..].parse().unwrap_or(0)),
        "archivos" => Launch::Folder("/".into()),
        "terminal" => Launch::Terminal(None),
        _ => Launch::Browse(url.clone()),
    };
    t.d.open(what, t.now, CLOCK);
    let serve = |t: &mut Driver| {
        for _ in 0..6 {
            let reqs = t.d.take_requests().net;
            if reqs.is_empty() {
                break;
            }
            for r in reqs {
                let resp = if r.kind == jarvis_desktop::FetchKind::Image {
                    // Los PNG y JPEG los decodifica el navegador (jarvis-image, como en el
                    // kernel); los demás los convierte el "puente".
                    let res = curl(&r.url).and_then(|mut x| {
                        if jarvis_image::sniff(&x.body).is_none() {
                            x.body = to_bmp(&x.body)?;
                            x.content_type = "image/bmp".into();
                        }
                        Ok(x)
                    });
                    if let Err(e) = &res {
                        println!("imagen {}: {e}", r.url);
                    }
                    res
                } else {
                    curl(&r.url)
                };
                t.d.net_response(r.id, resp);
            }
        }
    };
    serve(&mut t);
    // Maximizada, para ver más; JARVIS_BAJAR=N baja N pantallas.
    t.combo(jarvis_desktop::Mods::WIN, jarvis_desktop::Key::Up);
    // (Que termine la transición.)
    t.now += jarvis_desktop::desktop::ANIM_MS;
    // Al dibujar con el ancho real se rearman los estilos: puede pedir más imágenes.
    t.frame();
    serve(&mut t);
    let down: usize = std::env::var("JARVIS_BAJAR")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for _ in 0..down {
        t.key(jarvis_desktop::Key::PageDown);
    }
    // Todo de nuevo (la ventana ya se dibujó una vez en otros buffers): Win+Ctrl+Shift+B.
    t.combo(
        jarvis_desktop::Mods {
            win: true,
            ctrl: true,
            shift: true,
            ..jarvis_desktop::Mods::NONE
        },
        jarvis_desktop::Key::Char('b'),
    );
    let (mut bg, mut fr) = buffers();
    let mut bgc = canvas(&mut bg);
    let mut frame = canvas(&mut fr);
    t.d.render(&mut frame, &mut bgc, t.now, CLOCK);
    if let Some(jarvis_desktop::apps::App::Browser(b)) = t.d.app(jarvis_desktop::AppKind::Browser) {
        println!("imágenes (total, listas, fallidas): {:?}", b.image_stats());
        println!(
            "hojas de estilo (pedidas, llegaron, bytes): {:?}",
            b.css_stats()
        );
    }
    // JARVIS_ID=x: el estilo calculado de ese elemento y de sus ancestros.
    if let (Ok(id), Some(jarvis_desktop::apps::App::Browser(b))) = (
        std::env::var("JARVIS_ID"),
        t.d.app(jarvis_desktop::AppKind::Browser),
    ) {
        let prep = b.prepared().unwrap();
        let mut cur =
            (0..prep.dom.nodes.len()).find(|&i| prep.dom.nodes[i].attr("id") == Some(&id));
        while let Some(n) = cur {
            let node = &prep.dom.nodes[n];
            if let Some(st) = prep.styled.get(n) {
                println!(
                    "<{} class='{}'> display={:?} float={:?} pos={:?} width={:?} margin={:?} align={:?}",
                    node.name(),
                    node.attr("class").unwrap_or(""),
                    st.display,
                    st.float,
                    st.position,
                    st.width,
                    st.margin,
                    st.align
                );
            }
            cur = node.parent;
        }
    }
    // JARVIS_PUNTO=x,y: qué elementos pintaron ese punto de la página (para depurar).
    if let (Ok(pt), Some(jarvis_desktop::apps::App::Browser(b))) = (
        std::env::var("JARVIS_PUNTO"),
        t.d.app(jarvis_desktop::AppKind::Browser),
    ) {
        let (x, y) = pt.split_once(',').unwrap();
        let (x, y): (i32, i32) = (x.parse().unwrap(), y.parse().unwrap());
        let prep = b.prepared().unwrap();
        let desc = |n: u32| {
            let node = &prep.dom.nodes[n as usize];
            format!(
                "<{} class='{}' id='{}'>",
                node.name(),
                node.attr("class").unwrap_or(""),
                node.attr("id").unwrap_or("")
            )
        };
        for p in &b.laid_out().unwrap().paints {
            use jarvis_desktop::web::layout::Paint;
            match p {
                Paint::Rect { r, node, color, .. } if r.contains(x, y) => {
                    println!("fondo {r:?} {color:?} {}", desc(*node))
                }
                Paint::Border { r, node, w, .. } if r.contains(x, y) => {
                    println!("borde {r:?} {w:?} {}", desc(*node))
                }
                Paint::Mask {
                    bx,
                    dest,
                    img,
                    color,
                } if bx.contains(x, y) => {
                    println!("máscara {bx:?} {dest:?} imagen {img} {color:?}")
                }
                Paint::Placeholder { r, label } if r.contains(x, y) => {
                    println!("lugar de imagen {r:?} {label:?}")
                }
                Paint::Image { r, img, .. } if r.contains(x, y) => println!("imagen {r:?} {img}"),
                Paint::Text {
                    x: tx,
                    top,
                    w,
                    h,
                    text,
                    ..
                } if jarvis_gfx::Rect::new(*tx, *top, *w, *h).contains(x, y) => {
                    println!("texto {text:?}")
                }
                _ => {}
            }
        }
    }
    let bmp = jarvis_desktop::bmp::encode(&frame);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/vista-previa.bmp");
    std::fs::write(&out, bmp).unwrap();
    println!("vista previa: {}", out.display());
}
