//! El Wi-Fi desde el escritorio (K14): la lista de redes en Configuración → Red, la contraseña
//! (de la que sale la clave, que es lo único que se guarda), la red abierta, desconectarse y
//! volver a conectarse solo al arrancar.

mod common;

use common::*;
use jarvis_desktop::{
    Key, Launch, NetInfo, SystemStats, WifiInfo, WifiNetwork, WifiOp, WifiSecurity, WifiState,
};

const RED: usize = 8; // Configuración → Red e Internet

fn stats(state: WifiState, ssid: Option<&str>) -> SystemStats {
    let net = |ssid: &str, security, rssi| WifiNetwork {
        ssid: ssid.into(),
        rssi,
        channel: 6,
        security,
    };
    SystemStats {
        net: NetInfo {
            present: true,
            wifi: Some(WifiInfo {
                chip: "Realtek RTL8821CE (Wi-Fi)".into(),
                state,
                ssid: ssid.map(Into::into),
                rssi: -48,
                channel: 6,
                failure: None,
                networks: vec![
                    net("Casa", WifiSecurity::Wpa2, -48),
                    net("Bar", WifiSecurity::Open, -70),
                    net("Viejo", WifiSecurity::Unsupported, -80),
                ],
            }),
            ..NetInfo::default()
        },
        ..SystemStats::default()
    }
}

/// La fila de la red N (después de: estado, MAC, puerta de enlace, tráfico, HTTPS por el
/// puente, probar, el estado del Wi-Fi y "Buscar redes").
fn select_network(t: &mut Driver, n: usize, extra_rows: usize) {
    t.keys(&vec![Key::Down; 8 + extra_rows + n]);
    t.key(Key::Enter);
}

#[test]
fn al_aparecer_la_placa_busca_redes() {
    let mut t = Driver::new();
    let _ = t.d.take_requests();
    t.d.set_stats(stats(WifiState::Idle, None));
    assert_eq!(t.d.take_requests().wifi, [WifiOp::Scan]);
    // Solo la primera vez.
    t.d.set_stats(stats(WifiState::Idle, None));
    assert!(t.d.take_requests().wifi.is_empty());
}

#[test]
fn conectarse_a_una_red_wpa2_con_su_contrasena() {
    let mut t = Driver::new();
    t.d.set_stats(stats(WifiState::Idle, None));
    let _ = t.d.take_requests();
    t.d.open(Launch::Settings(RED), t.now, CLOCK);
    select_network(&mut t, 0, 0);
    // Una contraseña corta no sirve (WPA2: de 8 a 63).
    t.type_text("corta");
    t.key(Key::Enter);
    assert!(t.d.take_requests().wifi.is_empty());
    // La fila de la contraseña quedó (antes de "Buscar redes"): se vuelve a escribir.
    t.key(Key::Up);
    t.key(Key::Enter);
    t.type_text("clave segura");
    t.key(Key::Enter);
    let pmk = jarvis_wifi::crypto::pmk("clave segura", b"Casa");
    assert_eq!(
        t.d.take_requests().wifi,
        [WifiOp::Connect {
            ssid: "Casa".into(),
            pmk: Some(pmk)
        }]
    );
    // Se guarda el nombre y la clave derivada, nunca la contraseña.
    assert_eq!(t.d.config().wifi_ssid, "Casa");
    assert_eq!(t.d.config().wifi_pmk, Some(pmk));
    let saved = t.d.config().serialize();
    assert!(saved.contains("wifi_red=43617361"));
    assert!(!saved.contains("clave segura"));
    assert!(!t.logs().iter().any(|l| l.contains("clave segura")));
    let back = jarvis_desktop::Config::parse(&saved);
    assert_eq!(
        (back.wifi_ssid.as_str(), back.wifi_pmk),
        ("Casa", Some(pmk))
    );
}

#[test]
fn una_red_abierta_y_una_no_soportada() {
    let mut t = Driver::new();
    t.d.set_stats(stats(WifiState::Idle, None));
    let _ = t.d.take_requests();
    t.d.open(Launch::Settings(RED), t.now, CLOCK);
    select_network(&mut t, 2, 0);
    assert!(t.d.take_requests().wifi.is_empty(), "WEP/WPA/WPA3: no");
    t.key(Key::Up);
    t.key(Key::Enter);
    assert_eq!(
        t.d.take_requests().wifi,
        [WifiOp::Connect {
            ssid: "Bar".into(),
            pmk: None
        }]
    );
    assert_eq!(t.d.config().wifi_pmk, None);
}

#[test]
fn desconectarse_y_la_red_guardada_al_arrancar() {
    let mut t = Driver::new();
    let mut cfg = t.d.config().clone();
    cfg.wifi_ssid = "Casa".into();
    cfg.wifi_pmk = Some([7; 32]);
    t.d.set_config(cfg);
    let _ = t.d.take_requests();
    // Al aparecer la placa: a la red guardada.
    t.d.set_stats(stats(WifiState::Connected, Some("Casa")));
    assert_eq!(
        t.d.take_requests().wifi,
        [WifiOp::Connect {
            ssid: "Casa".into(),
            pmk: Some([7; 32])
        }]
    );
    // Conectada: la fila 7 es "Desconectarse".
    t.d.open(Launch::Settings(RED), t.now, CLOCK);
    t.keys(&[Key::Down; 7]);
    t.key(Key::Enter);
    assert_eq!(t.d.take_requests().wifi, [WifiOp::Disconnect]);
    assert!(t.d.config().wifi_ssid.is_empty());
    assert_eq!(t.d.config().wifi_pmk, None);
}

#[test]
fn sin_placa_wifi_no_se_pide_nada() {
    let mut t = Driver::new();
    let _ = t.d.take_requests();
    t.d.set_stats(SystemStats::default());
    assert!(t.d.take_requests().wifi.is_empty());
}
