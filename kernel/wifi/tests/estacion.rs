//! La estación (K14) contra puntos de acceso de mentira en un "aire" simulado: cada uno en su
//! canal, con beacons cada 100 ms, autenticación, asociación, el saludo de 4 vías (del lado del
//! punto de acceso, escrito acá aparte del de la estación), datos cifrados y desconexiones.

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use jarvis_wifi::ccmp::Key;
use jarvis_wifi::crypto::{self, Ptk};
use jarvis_wifi::frame::{self, Header, Security};
use jarvis_wifi::station::{Action, Credential, Failure, Phase, Station};
use jarvis_wifi::{Mac, rsn};

const STA: Mac = [0x5c, 0xea, 0x1d, 0x00, 0x00, 0x01];
const SERVER: Mac = [0x02, 0x00, 0x00, 0x00, 0x00, 0x99];

/// Relleno de los datos de la clave hasta múltiplo de 8: 0xDD y ceros (802.11, 12.7.2).
fn pad(data: &mut Vec<u8>) {
    if !data.len().is_multiple_of(8) {
        data.push(0xdd);
        while !data.len().is_multiple_of(8) {
            data.push(0);
        }
    }
}

/// AES Key Wrap (RFC 3394): el punto de acceso envuelve la clave de grupo.
fn key_wrap(kek: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let c = Aes128::new(GenericArray::from_slice(kek));
    let n = plain.len() / 8;
    let mut a = [0xa6u8; 8];
    let mut r = plain.to_vec();
    for j in 0..6 {
        for i in 1..=n {
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(&r[(i - 1) * 8..i * 8]);
            let mut blk = GenericArray::from(b);
            c.encrypt_block(&mut blk);
            let t = ((n * j + i) as u64).to_be_bytes();
            for k in 0..8 {
                a[k] = blk[k] ^ t[k];
            }
            r[(i - 1) * 8..i * 8].copy_from_slice(&blk[8..]);
        }
    }
    [a.to_vec(), r].concat()
}

struct Ap {
    bssid: Mac,
    ssid: Vec<u8>,
    channel: u8,
    pass: Option<&'static str>,
    ht: bool,
    on: bool,
    seq: u16,
    anonce: [u8; 32],
    replay: u64,
    ptk: Option<Ptk>,
    tk: Option<Key>,
    gtk: [u8; 16],
    gtk_key: Option<Key>,
    done: bool,
    out: Vec<Vec<u8>>,
    got: Vec<Vec<u8>>,
    probes: usize,
    last_beacon: u64,
}

impl Ap {
    fn new(bssid_last: u8, ssid: &str, channel: u8, pass: Option<&'static str>) -> Ap {
        Ap {
            bssid: [0x02, 0xaa, 0xbb, 0xcc, 0xdd, bssid_last],
            ssid: ssid.as_bytes().to_vec(),
            channel,
            pass,
            ht: true,
            on: true,
            seq: 0,
            anonce: [bssid_last; 32],
            replay: 0,
            ptk: None,
            tk: None,
            gtk: [0x77; 16],
            gtk_key: None,
            done: false,
            out: Vec::new(),
            got: Vec::new(),
            probes: 0,
            last_beacon: 0,
        }
    }

    fn rsn(&self) -> Vec<u8> {
        // El del punto de acceso: con otras capacidades que el de la estación.
        let mut ie = rsn::own_element();
        ie[20] = 0x0c;
        ie
    }

    fn beacon(&mut self, dst: Mac, subtype: u8) -> Vec<u8> {
        self.seq += 1;
        let mut f = Vec::new();
        Header::mgmt(subtype, dst, self.bssid, self.bssid, self.seq).write(&mut f);
        f.extend_from_slice(&[0; 8]);
        f.extend_from_slice(&100u16.to_le_bytes());
        let caps: u16 = 0x0001 | if self.pass.is_some() { 0x0010 } else { 0 };
        f.extend_from_slice(&caps.to_le_bytes());
        f.extend_from_slice(&[0, self.ssid.len() as u8]);
        f.extend_from_slice(&self.ssid);
        f.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
        f.extend_from_slice(&[3, 1, self.channel]);
        if self.pass.is_some() {
            f.extend_from_slice(&self.rsn());
        }
        if self.ht {
            f.extend_from_slice(&[45, 26]);
            f.extend_from_slice(&[0; 26]);
            f.extend_from_slice(&[221, 7, 0x00, 0x50, 0xf2, 0x02, 0x01, 0x01, 0x00]);
        }
        f
    }

    fn mgmt(&mut self, subtype: u8, body: &[u8]) -> Vec<u8> {
        self.seq += 1;
        let mut f = Vec::new();
        Header::mgmt(subtype, STA, self.bssid, self.bssid, self.seq).write(&mut f);
        f.extend_from_slice(body);
        f
    }

    /// Una trama de datos para la estación (FromDS), cifrada si hay clave.
    fn data(&mut self, eth: &[u8], group: bool) -> Vec<u8> {
        self.seq += 1;
        let dst: Mac = eth[..6].try_into().unwrap();
        let src: Mac = eth[6..12].try_into().unwrap();
        let h = Header {
            fc: [0x88, frame::FROM_DS],
            duration: 0,
            addr1: dst,
            addr2: self.bssid,
            addr3: src,
            seq: self.seq << 4,
            addr4: None,
            qos: Some(0),
        };
        let mut f = Vec::new();
        h.write(&mut f);
        f.extend_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0]);
        f.extend_from_slice(&eth[12..]);
        let key = if group {
            self.gtk_key.as_mut()
        } else {
            self.tk.as_mut()
        };
        match key {
            Some(k) => k.encrypt(&f).unwrap(),
            None => f,
        }
    }

    fn eapol(&self, info: u16, nonce: &[u8; 32], rsc: u64, data: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.push(2);
        d.extend_from_slice(&info.to_be_bytes());
        d.extend_from_slice(&16u16.to_be_bytes());
        d.extend_from_slice(&self.replay.to_be_bytes());
        d.extend_from_slice(nonce);
        d.extend_from_slice(&[0; 16]);
        d.extend_from_slice(&rsc.to_le_bytes());
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&[0; 16]);
        d.extend_from_slice(&(data.len() as u16).to_be_bytes());
        d.extend_from_slice(data);
        let mut f = vec![2, 3];
        f.extend_from_slice(&(d.len() as u16).to_be_bytes());
        f.extend_from_slice(&d);
        if let Some(ptk) = &self.ptk {
            let mic = crypto::eapol_mic(&ptk.kck, &f);
            f[81..97].copy_from_slice(&mic);
        }
        f
    }

    fn send_eapol(&mut self, eapol: Vec<u8>) {
        let mut eth = STA.to_vec();
        eth.extend_from_slice(&self.bssid);
        eth.extend_from_slice(&0x888eu16.to_be_bytes());
        eth.extend_from_slice(&eapol);
        let f = self.data(&eth, false);
        self.out.push(f);
    }

    fn gtk_data(&self, id: u8, gtk: &[u8; 16]) -> Vec<u8> {
        let mut kde = vec![0xdd, 22, 0x00, 0x0f, 0xac, 1, id, 0];
        kde.extend_from_slice(gtk);
        kde
    }

    fn msg1(&mut self) {
        self.replay += 1;
        self.ptk = None;
        let m = self.eapol(0x008a, &self.anonce.clone(), 0, &[]);
        self.send_eapol(m);
    }

    fn receive(&mut self, f: &[u8], now: u64) {
        let Some(h) = Header::parse(f) else { return };
        if h.kind() == frame::TYPE_MGMT {
            match h.subtype() {
                frame::PROBE_REQ => {
                    self.probes += 1;
                    let r = self.beacon(h.addr2, frame::PROBE_RESP);
                    self.out.push(r);
                }
                frame::AUTH if h.addr1 == self.bssid => {
                    let r = self.mgmt(frame::AUTH, &[0, 0, 2, 0, 0, 0]);
                    self.out.push(r);
                }
                frame::ASSOC_REQ if h.addr1 == self.bssid => {
                    let ies: Vec<_> = frame::elements(&f[28..]).collect();
                    let has_rsn = ies.iter().any(|(id, _)| *id == frame::IE_RSN);
                    let status: u16 = if has_rsn == self.pass.is_some() {
                        0
                    } else {
                        40
                    };
                    let mut body = vec![0x11, 0];
                    body.extend_from_slice(&status.to_le_bytes());
                    body.extend_from_slice(&(0xc000u16 | 1).to_le_bytes());
                    body.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
                    if self.ht && ies.iter().any(|(id, _)| *id == frame::IE_HT_CAPS) {
                        body.extend_from_slice(&[45, 26]);
                        body.extend_from_slice(&[0; 26]);
                    }
                    let r = self.mgmt(frame::ASSOC_RESP, &body);
                    self.out.push(r);
                    self.done = false;
                    self.tk = None;
                    if status == 0 && self.pass.is_some() {
                        self.msg1();
                    }
                }
                _ => {}
            }
            return;
        }
        if h.kind() != frame::TYPE_DATA || h.addr1 != self.bssid {
            return;
        }
        let plain = if h.protected() {
            match self.tk.as_mut().map(|k| k.decrypt(f)) {
                Some(Ok(p)) => p,
                _ => return,
            }
        } else {
            f.to_vec()
        };
        // ToDS: destino en addr3.
        let body = &plain[Header::parse(&plain).unwrap().size()..];
        assert_eq!(&body[..6], &[0xaa, 0xaa, 3, 0, 0, 0]);
        if body[6..8] == [0x88, 0x8e] {
            self.eapol_in(&body[8..], now);
        } else {
            let mut eth = h.addr3.to_vec();
            eth.extend_from_slice(&h.addr2);
            eth.extend_from_slice(&body[6..]);
            self.got.push(eth);
        }
    }

    fn eapol_in(&mut self, e: &[u8], _now: u64) {
        let info = u16::from_be_bytes([e[5], e[6]]);
        let mut zero = e.to_vec();
        zero[81..97].fill(0);
        let mic = &e[81..97];
        if info & (1 << 9) == 0 {
            // Mensaje 2: con el SNonce de la estación sale la PTK; si el MIC no da, la
            // contraseña es otra y el punto de acceso no contesta.
            let snonce: [u8; 32] = e[17..49].try_into().unwrap();
            let pmk = crypto::pmk(self.pass.unwrap(), &self.ssid);
            let ptk = crypto::ptk(&pmk, &self.bssid, &STA, &self.anonce, &snonce);
            if crypto::eapol_mic(&ptk.kck, &zero) != mic {
                return;
            }
            self.ptk = Some(ptk.clone());
            self.replay += 1;
            let mut data = self.rsn();
            data.extend(self.gtk_data(1, &self.gtk.clone()));
            pad(&mut data);
            let wrapped = key_wrap(&ptk.kek, &data);
            let m3 = self.eapol(0x13ca, &self.anonce.clone(), 5, &wrapped);
            self.send_eapol(m3);
        } else if info & (1 << 3) != 0 {
            // Mensaje 4: listo, instalar las claves.
            let ptk = self.ptk.clone().unwrap();
            assert_eq!(crypto::eapol_mic(&ptk.kck, &zero), mic);
            self.tk = Some(Key::new(&ptk.tk, 0, 0));
            self.gtk_key = Some(Key::new(&self.gtk.clone(), 1, 5));
            self.done = true;
        }
    }

    fn rekey(&mut self) {
        self.gtk = [0x55; 16];
        self.replay += 1;
        let ptk = self.ptk.clone().unwrap();
        let mut data = self.gtk_data(2, &self.gtk.clone());
        pad(&mut data);
        let wrapped = key_wrap(&ptk.kek, &data);
        let m = self.eapol(0x1382, &[0; 32], 0, &wrapped);
        self.send_eapol(m);
        self.gtk_key = Some(Key::new(&self.gtk.clone(), 2, 0));
    }
}

/// El aire: la estación, los puntos de acceso, el canal de la radio y lo que llegó a la pila.
struct Air {
    sta: Station,
    aps: Vec<Ap>,
    channel: u8,
    now: u64,
    delivered: Vec<Vec<u8>>,
    link: Option<bool>,
    actions: Vec<Action>,
}

impl Air {
    fn new(aps: Vec<Ap>) -> Air {
        Air {
            sta: Station::new(STA, [9; 32]),
            aps,
            channel: 1,
            now: 1_000,
            delivered: Vec::new(),
            link: None,
            actions: Vec::new(),
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match &a {
                Action::SetChannel(c) => self.channel = *c,
                Action::Tx(f) => {
                    for ap in self
                        .aps
                        .iter_mut()
                        .filter(|ap| ap.on && ap.channel == self.channel)
                    {
                        ap.receive(f, self.now);
                    }
                }
                Action::Deliver(eth) => self.delivered.push(eth.clone()),
                Action::Link(up) => self.link = Some(*up),
                _ => {}
            }
            self.actions.push(a);
        }
    }

    /// Avanza `ms` de a 10 ms: beacons, lo que mandan los puntos de acceso y el tiempo.
    fn run(&mut self, ms: u64) {
        for _ in 0..ms / 10 {
            self.now += 10;
            let mut incoming = Vec::new();
            for ap in self.aps.iter_mut().filter(|ap| ap.on) {
                if self.now - ap.last_beacon >= 100 {
                    ap.last_beacon = self.now;
                    let b = ap.beacon(jarvis_wifi::BROADCAST, frame::BEACON);
                    ap.out.push(b);
                }
                let frames = std::mem::take(&mut ap.out);
                if ap.channel == self.channel {
                    incoming.extend(frames);
                }
            }
            for f in incoming {
                let acts = self.sta.receive(&f, Some(-50), self.now);
                self.apply(acts);
            }
            let acts = self.sta.poll(self.now);
            self.apply(acts);
        }
    }
}

fn eth(dst: Mac, src: Mac, payload: &[u8]) -> Vec<u8> {
    [&dst[..], &src, &[0x08, 0x00], payload].concat()
}

#[test]
fn busca_redes_en_las_dos_bandas_y_las_lista() {
    let mut air = Air::new(vec![
        Ap::new(1, "Casa", 6, Some("clave segura")),
        Ap::new(2, "Bar", 1, None),
        Ap::new(3, "Casa-5G", 52, Some("otra")), // canal de radar
    ]);
    let acts = air.sta.scan(air.now);
    air.apply(acts);
    assert_eq!(air.sta.phase(), Phase::Scanning);
    air.run(4_000);
    assert_eq!(air.sta.phase(), Phase::Idle);
    let nets = air.sta.networks();
    let names: Vec<_> = nets
        .iter()
        .map(|n| String::from_utf8_lossy(&n.ssid))
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    let casa = nets.iter().find(|n| n.ssid == b"Casa").unwrap();
    assert_eq!((casa.security, casa.channel), (Security::Wpa2Psk, 6));
    let bar = nets.iter().find(|n| n.ssid == b"Bar").unwrap();
    assert_eq!(bar.security, Security::Open);
    // En el canal de radar no se transmitió: la red se encontró por su beacon.
    assert_eq!(air.aps[2].probes, 0);
    assert!(air.aps[0].probes > 0);
    assert!(air.actions.contains(&Action::SetScanning(false)));
}

#[test]
fn se_conecta_a_una_red_abierta_y_pasa_datos() {
    let mut air = Air::new(vec![Ap::new(2, "Bar", 11, None)]);
    let acts = air.sta.connect(b"Bar", Credential::Open, air.now);
    air.apply(acts);
    air.run(4_000);
    assert_eq!(air.sta.phase(), Phase::Connected);
    assert_eq!(air.link, Some(true));
    assert!(air.actions.contains(&Action::Calibrate));
    assert!(air.actions.iter().any(|a| matches!(
        a,
        Action::Associated {
            aid: 1,
            ht: true,
            ..
        }
    )));
    // De la pila al aire y del aire a la pila.
    let out = eth(SERVER, STA, b"hola");
    let tx = air.sta.send(&out).unwrap();
    air.apply(vec![tx]);
    assert_eq!(air.aps[0].got, [out]);
    let back = eth(STA, SERVER, b"chau");
    let f = air.aps[0].data(&back, false);
    let acts = air.sta.receive(&f, None, air.now);
    assert_eq!(acts, [Action::Deliver(back)]);
}

#[test]
fn wpa2_saludo_datos_cifrados_y_cambio_de_clave_de_grupo() {
    let mut air = Air::new(vec![Ap::new(1, "Casa", 36, Some("clave segura"))]);
    let pmk = crypto::pmk("clave segura", b"Casa");
    let acts = air.sta.connect(b"Casa", Credential::Psk(pmk), air.now);
    air.apply(acts);
    air.run(5_000);
    assert_eq!(
        air.sta.phase(),
        Phase::Connected,
        "{:?}",
        air.sta.last_failure
    );
    assert!(air.aps[0].done, "el punto de acceso recibió el mensaje 4");
    assert_eq!(air.link, Some(true));
    // Datos de la estación: cifrados (el punto de acceso los descifra con su TK).
    let out = eth(SERVER, STA, b"secreto");
    let Some(Action::Tx(f)) = air.sta.send(&out) else {
        panic!()
    };
    assert!(Header::parse(&f).unwrap().protected());
    assert!(!f.windows(7).any(|w| w == b"secreto"));
    air.apply(vec![Action::Tx(f)]);
    assert_eq!(air.aps[0].got, [out]);
    // Del punto de acceso: a la estación (PTK) y a todos (GTK).
    let back = eth(STA, SERVER, b"para vos");
    let f = air.aps[0].data(&back, false);
    assert_eq!(
        air.sta.receive(&f, None, air.now),
        [Action::Deliver(back.clone())]
    );
    assert!(air.sta.receive(&f, None, air.now).is_empty(), "repetida");
    let all = eth(jarvis_wifi::BROADCAST, SERVER, b"para todos");
    let f = air.aps[0].data(&all, true);
    assert_eq!(air.sta.receive(&f, None, air.now), [Action::Deliver(all)]);
    // Una trama sin cifrar en una red WPA2: afuera.
    let tk = air.aps[0].tk.take();
    let plain = air.aps[0].data(&back, false);
    air.aps[0].tk = tk;
    assert!(air.sta.receive(&plain, None, air.now).is_empty());
    // Cambio de la clave de grupo.
    air.aps[0].rekey();
    air.run(50);
    let all = eth(jarvis_wifi::BROADCAST, SERVER, b"clave nueva");
    let f = air.aps[0].data(&all, true);
    assert_eq!(air.sta.receive(&f, None, air.now), [Action::Deliver(all)]);
    assert_eq!(air.sta.phase(), Phase::Connected);
}

#[test]
fn con_otra_contrasena_no_conecta_y_lo_dice() {
    let mut air = Air::new(vec![Ap::new(1, "Casa", 6, Some("clave segura"))]);
    let pmk = crypto::pmk("me equivoqué", b"Casa");
    let acts = air.sta.connect(b"Casa", Credential::Psk(pmk), air.now);
    air.apply(acts);
    air.run(10_000);
    assert_ne!(air.sta.phase(), Phase::Connected);
    assert_eq!(air.sta.last_failure, Some(Failure::WrongPassword));
    assert!(!air.aps[0].done);
    assert_eq!(air.link, None, "la pila nunca vio la conexión");
}

#[test]
fn sin_beacons_se_desconecta_y_vuelve_cuando_la_red_vuelve() {
    let mut air = Air::new(vec![Ap::new(1, "Casa", 6, Some("clave segura"))]);
    let pmk = crypto::pmk("clave segura", b"Casa");
    let acts = air.sta.connect(b"Casa", Credential::Psk(pmk), air.now);
    air.apply(acts);
    air.run(6_000);
    assert_eq!(air.sta.phase(), Phase::Connected);
    // El punto de acceso se apaga.
    air.aps[0].on = false;
    air.run(6_000);
    assert_eq!(air.link, Some(false));
    assert_eq!(air.sta.last_failure, Some(Failure::Lost));
    assert!(air.actions.contains(&Action::Disassociated));
    // Vuelve: la estación reintenta sola (con espera) y se reconecta.
    air.aps[0].on = true;
    air.run(25_000);
    assert_eq!(
        air.sta.phase(),
        Phase::Connected,
        "{:?}",
        air.sta.last_failure
    );
    assert_eq!(air.link, Some(true));
}

#[test]
fn si_el_punto_de_acceso_la_echa_reintenta() {
    let mut air = Air::new(vec![Ap::new(2, "Bar", 1, None)]);
    let acts = air.sta.connect(b"Bar", Credential::Open, air.now);
    air.apply(acts);
    air.run(5_000);
    assert!(air.sta.connected());
    let deauth = air.aps[0].mgmt(frame::DEAUTH, &7u16.to_le_bytes());
    let acts = air.sta.receive(&deauth, None, air.now);
    air.apply(acts);
    assert_eq!(air.sta.last_failure, Some(Failure::Kicked(7)));
    assert_eq!(air.link, Some(false));
    air.run(4_000);
    assert!(air.sta.connected());
}

#[test]
fn rechaza_el_block_ack_y_se_desconecta_cuando_se_le_pide() {
    let mut air = Air::new(vec![Ap::new(2, "Bar", 1, None)]);
    let acts = air.sta.connect(b"Bar", Credential::Open, air.now);
    air.apply(acts);
    air.run(5_000);
    // ADDBA: categoría 3, acción 0, token 9, parámetros, tiempo, inicio.
    let addba = air.aps[0].mgmt(frame::ACTION, &[3, 0, 9, 0x02, 0x10, 0, 0, 0, 0]);
    let acts = air.sta.receive(&addba, None, air.now);
    let [Action::Tx(r)] = acts.as_slice() else {
        panic!("{acts:?}")
    };
    assert_eq!(
        &r[24..29],
        &[3, 1, 9, 37, 0],
        "respuesta, token 9, rechazado"
    );
    // Desconectarse: avisa al punto de acceso (deauth) y a la pila.
    let acts = air.sta.disconnect();
    assert!(
        acts.iter()
            .any(|a| matches!(a, Action::Tx(f) if f[0] == frame::DEAUTH << 4))
    );
    assert!(acts.contains(&Action::Link(false)));
    assert_eq!(air.sta.phase(), Phase::Idle);
}

#[test]
fn una_red_que_no_existe() {
    let mut air = Air::new(vec![Ap::new(2, "Bar", 1, None)]);
    let acts = air.sta.connect(b"Nadie", Credential::Open, air.now);
    air.apply(acts);
    air.run(5_000);
    assert_eq!(air.sta.last_failure, Some(Failure::NotFound));
    assert_eq!(air.sta.phase(), Phase::Waiting);
    // Con la seguridad equivocada (abierta vs. WPA2) ni se intenta.
    let acts = air.sta.connect(b"Bar", Credential::Psk([1; 32]), air.now);
    air.apply(acts);
    assert_eq!(air.sta.last_failure, Some(Failure::Unsupported));
}
