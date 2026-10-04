//! La estación de jarvis-wifi contra un punto de acceso hecho con otra implementación
//! (tests/datos/generar.py: hashlib, hmac y `cryptography`). Si las dos mitades interpretaran
//! distinto un campo, un largo o el orden de algo, los MIC y los cifrados no coincidirían.

use std::collections::HashMap;

use jarvis_wifi::ccmp::{self, Key};
use jarvis_wifi::eapol::{Error, Output, Supplicant};
use jarvis_wifi::frame::{self, Security};
use jarvis_wifi::{Mac, crypto, rsn};

struct Datos(HashMap<String, Vec<u8>>);

impl Datos {
    fn cargar() -> Datos {
        let text = include_str!("datos/wifi.txt");
        let map = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| {
                let bytes = (0..v.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&v[i..i + 2], 16).unwrap())
                    .collect();
                (k.to_string(), bytes)
            })
            .collect();
        Datos(map)
    }

    fn get(&self, k: &str) -> &[u8] {
        &self.0[k]
    }

    fn mac(&self, k: &str) -> Mac {
        self.get(k).try_into().unwrap()
    }

    fn arr<const N: usize>(&self, k: &str) -> [u8; N] {
        self.get(k).try_into().unwrap()
    }
}

fn supplicant(d: &Datos, pass: &str) -> (Supplicant, frame::Bss) {
    let bss = frame::parse_beacon(d.get("beacon")).unwrap();
    let pmk = crypto::pmk(pass, &bss.ssid);
    let s = Supplicant::new(
        d.mac("sta"),
        bss.bssid,
        pmk,
        d.arr("snonce"),
        rsn::own_element(),
        bss.rsn.clone(),
    );
    (s, bss)
}

fn passphrase(d: &Datos) -> String {
    String::from_utf8(d.get("passphrase").to_vec()).unwrap()
}

#[test]
fn el_beacon_dice_la_red_y_su_seguridad() {
    let d = Datos::cargar();
    let bss = frame::parse_beacon(d.get("beacon")).unwrap();
    assert_eq!(bss.ssid, d.get("ssid"));
    assert_eq!(bss.bssid, d.mac("ap"));
    assert_eq!(bss.channel, 6);
    assert_eq!(bss.security, Security::Wpa2Psk);
    assert_eq!(bss.rates, [0x82, 0x84, 0x8b, 0x96]);
}

#[test]
fn las_claves_salen_iguales() {
    let d = Datos::cargar();
    assert_eq!(crypto::pmk(&passphrase(&d), d.get("ssid")), d.arr("pmk"));
    let ptk = crypto::ptk(
        &d.arr("pmk"),
        &d.mac("ap"),
        &d.mac("sta"),
        &d.arr("anonce"),
        &d.arr("snonce"),
    );
    assert_eq!(ptk.kck, d.arr("kck"));
    assert_eq!(ptk.kek, d.arr("kek"));
    assert_eq!(ptk.tk, d.arr("tk"));
}

#[test]
fn el_saludo_de_4_vias_y_el_de_grupo() {
    let d = Datos::cargar();
    let (mut s, _) = supplicant(&d, &passphrase(&d));
    // 1 → 2: el mensaje 2 es exactamente el que espera el punto de acceso (MIC incluido).
    assert_eq!(
        s.handle(d.get("msg1")).unwrap(),
        [Output::Send(d.get("msg2").to_vec())]
    );
    // 3 → 4, con las claves.
    let out = s.handle(d.get("msg3")).unwrap();
    assert_eq!(out[0], Output::Send(d.get("msg4").to_vec()));
    assert_eq!(out[1], Output::Pairwise(d.arr("tk")));
    assert_eq!(
        out[2],
        Output::Group {
            key: d.get("gtk").to_vec(),
            id: 1,
            rsc: 0x2a
        }
    );
    assert!(s.done());
    // El mismo mensaje 3 otra vez: contador viejo.
    assert_eq!(s.handle(d.get("msg3")), Err(Error::Replay));
    // Cambio de la clave de grupo.
    let out = s.handle(d.get("group1")).unwrap();
    assert_eq!(out[0], Output::Send(d.get("group2").to_vec()));
    assert_eq!(
        out[1],
        Output::Group {
            key: d.get("gtk2").to_vec(),
            id: 2,
            rsc: 0x50
        }
    );
}

#[test]
fn otra_contrasena_o_un_rsn_cambiado_no_pasan() {
    let d = Datos::cargar();
    let (mut s, _) = supplicant(&d, "otra contraseña");
    let Output::Send(m2) = &s.handle(d.get("msg1")).unwrap()[0] else {
        panic!()
    };
    assert_ne!(m2, d.get("msg2"), "con otra PMK el MIC es otro");
    assert_eq!(s.handle(d.get("msg3")), Err(Error::Mic));
    assert!(!s.done());

    let (mut s, _) = supplicant(&d, &passphrase(&d));
    assert_eq!(
        s.handle(d.get("msg3")),
        Err(Error::State),
        "el 3 antes del 1"
    );
    s.handle(d.get("msg1")).unwrap();
    // Un byte cambiado en el camino: no pasa y no gasta el contador.
    let mut tocado = d.get("msg3").to_vec();
    tocado[120] ^= 1;
    assert_eq!(s.handle(&tocado), Err(Error::Mic));
    assert!(s.handle(d.get("msg3")).is_ok());

    // Un punto de acceso (o alguien que cambió el beacon) con otro RSN: se corta la conexión.
    let (mut s, _) = supplicant(&d, &passphrase(&d));
    s.handle(d.get("msg1")).unwrap();
    assert_eq!(s.handle(d.get("msg3_rsn")), Err(Error::Rsn));
    assert!(!s.done());
}

#[test]
fn ccmp_en_los_dos_sentidos() {
    let d = Datos::cargar();
    let mut tk = Key::new(&d.arr("tk"), 0, 0);
    // Lo que manda el punto de acceso (QoS, TID 5, con reintento) → Ethernet.
    let plain = tk.decrypt(d.get("ccmp_rx")).unwrap();
    assert_eq!(frame::to_ethernet(&plain).unwrap(), d.get("eth_rx"));
    assert_eq!(tk.decrypt(d.get("ccmp_rx")), Err(ccmp::Error::Replay));
    // Lo que manda la estación: igual byte a byte a lo que cifra `cryptography`.
    let out = frame::from_ethernet(d.get("eth_tx"), d.mac("ap"), 3).unwrap();
    assert_eq!(tk.encrypt(&out).unwrap(), d.get("ccmp_tx"));
    // Difusión con la clave de grupo: el PN tiene que superar el RSC del mensaje 3.
    let mut gtk = Key::new(&d.arr("gtk"), 1, 0x2a);
    assert!(gtk.decrypt(d.get("ccmp_bc")).is_ok());
    let mut vieja = Key::new(&d.arr("gtk"), 1, 0x2b);
    assert_eq!(vieja.decrypt(d.get("ccmp_bc")), Err(ccmp::Error::Replay));
}

#[test]
fn autenticacion_y_asociacion() {
    let d = Datos::cargar();
    let bss = frame::parse_beacon(d.get("beacon")).unwrap();
    let sta = d.mac("sta");
    let auth = frame::auth_request(sta, bss.bssid, 1);
    let h = frame::Header::parse(&auth).unwrap();
    assert_eq!((h.kind(), h.subtype()), (frame::TYPE_MGMT, frame::AUTH));
    assert_eq!(auth.len(), 30);
    // La respuesta del punto de acceso: la misma trama, al revés, en el paso 2.
    let mut resp = auth.clone();
    resp[4..10].copy_from_slice(&sta);
    resp[10..16].copy_from_slice(&bss.bssid);
    resp[26] = 2;
    assert_eq!(frame::auth_response(&resp, bss.bssid), Some(0));
    assert_eq!(
        frame::auth_response(&auth, bss.bssid),
        None,
        "es la nuestra"
    );

    let req = frame::assoc_request(sta, &bss, Some(&rsn::own_element()), 2);
    let ies: Vec<_> = frame::elements(&req[28..]).collect();
    assert_eq!(ies[0], (frame::IE_SSID, d.get("ssid")));
    assert_eq!(ies[1].1, bss.rates.as_slice());
    assert_eq!(ies[2], (frame::IE_RSN, &rsn::own_element()[2..]));
    let mut ok = resp.clone();
    ok[0] = frame::ASSOC_RESP << 4;
    ok[24..30].copy_from_slice(&[0x11, 0, 0, 0, 0x01, 0xc0]); // capacidades, estado 0, AID 1
    assert_eq!(frame::assoc_response(&ok, bss.bssid), Some((0, 1)));
    let mut deauth = resp;
    deauth[0] = frame::DEAUTH << 4;
    deauth[24..26].copy_from_slice(&15u16.to_le_bytes());
    assert_eq!(frame::disconnected(&deauth, bss.bssid), Some(15));
}
