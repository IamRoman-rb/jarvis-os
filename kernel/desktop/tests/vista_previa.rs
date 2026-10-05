//! Vista previa de una pantalla de JARVIS-OS sin QEMU (herramienta de desarrollo, no corre en
//! `cargo test`):
//!
//! ```text
//! cargo test -p jarvis-desktop --test vista_previa -- --ignored --nocapture
//! JARVIS_VISTA=config:10 cargo test ... (Configuración, sección 10: Sonido)
//! JARVIS_VISTA=archivos | terminal | rapida (Win+A)
//! ```
//!
//! Guarda la pantalla en `target/vista-previa.bmp`.

mod common;

use common::*;
use jarvis_desktop::{Key, Launch, Mods};

#[test]
#[ignore]
fn vista_previa_de_una_pantalla() {
    let what = std::env::var("JARVIS_VISTA").unwrap_or_else(|_| "config:0".into());
    let mut t = Driver::new();
    // JARVIS_IDIOMA=en|pt: la interfaz en otro idioma.
    if let Ok(l) = std::env::var("JARVIS_IDIOMA") {
        jarvis_desktop::i18n::set(jarvis_desktop::i18n::Lang::from_code(&l));
    }
    // JARVIS_WIFI=1: una placa Wi-Fi conectada y redes a la vista (para mirar Configuración → Red).
    if std::env::var("JARVIS_WIFI").is_ok() {
        use jarvis_desktop::{WifiInfo, WifiNetwork, WifiSecurity, WifiState};
        let net = |ssid: &str, security, rssi, channel| WifiNetwork {
            ssid: ssid.into(),
            rssi,
            channel,
            security,
        };
        let mut stats = jarvis_desktop::SystemStats::default();
        stats.net.present = true;
        stats.net.ip = Some([192, 168, 0, 23]);
        stats.net.wifi = Some(WifiInfo {
            chip: "Realtek RTL8821CE (Wi-Fi)".into(),
            state: WifiState::Connected,
            ssid: Some("Casa-5G".into()),
            rssi: -51,
            channel: 36,
            failure: None,
            networks: vec![
                net("Casa-5G", WifiSecurity::Wpa2, -51, 36),
                net("Casa", WifiSecurity::Wpa2, -55, 6),
                net("Fibertel WiFi 123", WifiSecurity::Wpa2, -71, 11),
                net("Bar de la esquina", WifiSecurity::Open, -78, 1),
                net("Vecino", WifiSecurity::Unsupported, -84, 149),
            ],
        });
        t.d.set_stats(stats);
    }
    match what.as_str() {
        "rapida" => t.combo(Mods::WIN, Key::Char('a')),
        "archivos" => t.d.open(Launch::Folder("/".into()), t.now, CLOCK),
        "terminal" => t.d.open(Launch::Terminal(None), t.now, CLOCK),
        u => {
            let n = u
                .strip_prefix("config:")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            t.d.open(Launch::Settings(n), t.now, CLOCK);
        }
    }
    if what != "rapida" {
        // Maximizada, para ver más (y que termine la transición).
        t.combo(Mods::WIN, Key::Up);
        t.now += jarvis_desktop::desktop::ANIM_MS;
    }
    t.frame();
    // Todo de nuevo (la ventana ya se dibujó una vez en otros buffers): Win+Ctrl+Shift+B.
    t.combo(
        Mods {
            win: true,
            ctrl: true,
            shift: true,
            ..Mods::NONE
        },
        Key::Char('b'),
    );
    let (mut bg, mut fr) = buffers();
    let mut bgc = canvas(&mut bg);
    let mut frame = canvas(&mut fr);
    t.d.render(&mut frame, &mut bgc, t.now, CLOCK);
    let bmp = jarvis_desktop::bmp::encode(&frame);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/vista-previa.bmp");
    std::fs::write(&out, bmp).unwrap();
    println!("vista previa: {}", out.display());
}
