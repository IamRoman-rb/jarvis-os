//! La estación: buscar redes, conectarse, mantener la conexión y pasar los datos (K14).
//!
//! Es una máquina de estados **sin E/S**: recibe tramas y el paso del tiempo, y devuelve
//! [`Action`]s (cambiar de canal, mandar una trama, entregar una trama Ethernet a la pila de
//! red…). El driver de la placa las ejecuta. Así se prueba entera contra un punto de acceso de
//! mentira, sin radio.
//!
//! ```text
//!  Idle ──connect──► (busca la red) ──► Auth ──► Assoc ──► Handshake ──► Connected
//!   ▲                                    │         │           │             │
//!   └──── reintento con espera ◄─────────┴─────────┴───────────┴─────────────┘
//!         (sin respuesta, rechazo, sin beacons, desautenticada)
//! ```
//!
//! **Buscar redes**: canal por canal, con un pedido de sondeo en los que se puede transmitir
//! sin más (1–13, 36–48, 149–165) y solo escuchando en los de radar (DFS: 52–144), donde la
//! norma no deja transmitir hasta oír un beacon.

use alloc::string::String;
use alloc::vec::Vec;

use crate::ccmp::{self, Key};
use crate::eapol::{self, Output, Supplicant};
use crate::frame::{self, Bss, Security};
use crate::{BROADCAST, Mac, crypto, rsn};

/// Canales de 2,4 GHz y de 5 GHz que se recorren al buscar.
pub const CHANNELS_2G: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
pub const CHANNELS_5G: [u8; 25] = [
    36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
    149, 153, 157, 161, 165,
];

/// Canales de radar: solo escuchar.
pub fn passive(channel: u8) -> bool {
    (52..=144).contains(&channel)
}

const DWELL_ACTIVE_MS: u64 = 60;
const DWELL_PASSIVE_MS: u64 = 120;
const STEP_TIMEOUT_MS: u64 = 300;
const STEP_TRIES: u8 = 3;
const HANDSHAKE_MS: u64 = 4_000;
/// Sin beacons de la red durante este tiempo = desconectada.
const BEACON_LOSS_MS: u64 = 5_000;
/// Esperas entre reintentos (después del último, se repite).
const BACKOFF_MS: [u64; 4] = [2_000, 5_000, 10_000, 30_000];
/// Las redes que no se vuelven a ver en este tiempo salen de la lista.
const FORGET_MS: u64 = 120_000;

/// Cómo entrar a una red.
#[derive(Clone, PartialEq, Eq)]
pub enum Credential {
    Open,
    /// La PMK (de la contraseña y el SSID, [`crypto::pmk`]): la contraseña no se guarda.
    Psk([u8; 32]),
}

impl core::fmt::Debug for Credential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Nunca la clave en un log.
        f.write_str(match self {
            Credential::Open => "Open",
            Credential::Psk(_) => "Psk(…)",
        })
    }
}

/// Lo que el driver tiene que hacer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    SetChannel(u8),
    /// Aceptar los beacons de todas las redes (buscando) o solo de la propia.
    SetScanning(bool),
    /// El punto de acceso al que se va a conectar (la placa filtra por él).
    SetBssid(Mac),
    /// Calibrar la radio antes de conectarse.
    Calibrate,
    /// Asociada: avisarle al firmware, con las velocidades que se pueden usar.
    Associated {
        aid: u16,
        legacy: u16,
        ht: bool,
        vht: bool,
    },
    Disassociated,
    /// Mandar esta trama 802.11.
    Tx(Vec<u8>),
    /// Entregar esta trama Ethernet a la pila de red.
    Deliver(Vec<u8>),
    /// La conexión subió o bajó (la pila de red vuelve a pedir la dirección por DHCP).
    Link(bool),
}

/// En qué anda.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Scanning,
    Connecting,
    Connected,
    /// Esperando para volver a intentar.
    Waiting,
}

/// Por qué falló el último intento (para mostrarlo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    NotFound,
    Unsupported,
    NoResponse,
    Rejected(u16),
    /// El saludo no terminó: casi siempre, la contraseña.
    WrongPassword,
    Lost,
    Kicked(u16),
}

/// Una red de la lista (la mejor señal entre sus puntos de acceso).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    pub ssid: Vec<u8>,
    pub security: Security,
    pub rssi: i8,
    pub channel: u8,
}

#[derive(Clone, Debug)]
struct Seen {
    bss: Bss,
    rssi: i8,
    at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Auth,
    Assoc,
    Handshake,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    ssid: Vec<u8>,
    cred: Credential,
}

pub struct Station {
    mac: Mac,
    seed: [u8; 32],
    nonces: u32,
    seq: u16,
    target: Option<Target>,
    seen: Vec<Seen>,
    phase: Phase,
    // Búsqueda: canales que faltan y hasta cuándo se escucha el actual.
    scan: Vec<u8>,
    scan_until: u64,
    /// Al terminar de buscar, conectarse a `target`.
    join_after_scan: bool,
    // Conexión.
    bss: Option<Bss>,
    step: Step,
    tries: u8,
    deadline: u64,
    supplicant: Option<Supplicant>,
    msg1_seen: bool,
    ptk: Option<Key>,
    gtk: Option<Key>,
    qos: bool,
    last_beacon: u64,
    rssi: i8,
    // Reintentos.
    failures: usize,
    retry_at: u64,
    pub last_failure: Option<Failure>,
}

impl Station {
    /// `seed`: 32 bytes al azar (del generador del kernel), para los números del saludo.
    pub fn new(mac: Mac, seed: [u8; 32]) -> Station {
        Station {
            mac,
            seed,
            nonces: 0,
            seq: 0,
            target: None,
            seen: Vec::new(),
            phase: Phase::Idle,
            scan: Vec::new(),
            scan_until: 0,
            join_after_scan: false,
            bss: None,
            step: Step::Auth,
            tries: 0,
            deadline: 0,
            supplicant: None,
            msg1_seen: false,
            ptk: None,
            gtk: None,
            qos: false,
            last_beacon: 0,
            rssi: 0,
            failures: 0,
            retry_at: 0,
            last_failure: None,
        }
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn connected(&self) -> bool {
        self.phase == Phase::Connected
    }

    /// La red a la que está conectada (o intentando), con su señal.
    pub fn current(&self) -> Option<(String, i8, u8)> {
        let t = self.target.as_ref()?;
        let ch = self.bss.as_ref().map_or(0, |b| b.channel);
        Some((String::from_utf8_lossy(&t.ssid).into_owned(), self.rssi, ch))
    }

    /// Las redes vistas, de mejor a peor señal (una por nombre).
    pub fn networks(&self) -> Vec<Network> {
        let mut out: Vec<Network> = Vec::new();
        for s in &self.seen {
            if s.bss.ssid.is_empty() || s.bss.ssid.iter().all(|&c| c == 0) {
                continue; // red oculta
            }
            match out.iter_mut().find(|n| n.ssid == s.bss.ssid) {
                Some(n) if n.rssi >= s.rssi => {}
                Some(n) => {
                    n.rssi = s.rssi;
                    n.channel = s.bss.channel;
                    n.security = s.bss.security;
                }
                None => out.push(Network {
                    ssid: s.bss.ssid.clone(),
                    security: s.bss.security,
                    rssi: s.rssi,
                    channel: s.bss.channel,
                }),
            }
        }
        out.sort_by_key(|n| core::cmp::Reverse(n.rssi));
        out
    }

    fn next_seq(&mut self) -> u16 {
        let s = self.seq;
        self.seq = (self.seq + 1) & 0xfff;
        s
    }

    fn snonce(&mut self) -> [u8; 32] {
        self.nonces += 1;
        let mut n = [0u8; 32];
        crypto::prf(
            &self.seed,
            b"JARVIS-OS SNonce",
            &self.nonces.to_le_bytes(),
            &mut n,
        );
        n
    }

    // --- pedidos ---------------------------------------------------------------------------------

    /// Buscar redes (sin cambiar la conexión si hay una).
    pub fn scan(&mut self, now: u64) -> Vec<Action> {
        if matches!(self.phase, Phase::Connected | Phase::Connecting) {
            return Vec::new();
        }
        // Si estaba esperando para reintentar, el reintento va al terminar de buscar.
        self.join_after_scan |= self.target.is_some();
        self.start_scan(now)
    }

    fn start_scan(&mut self, now: u64) -> Vec<Action> {
        self.phase = Phase::Scanning;
        self.scan = CHANNELS_2G
            .iter()
            .chain(CHANNELS_5G.iter())
            .copied()
            .rev()
            .collect();
        self.scan_until = now;
        let mut out = alloc::vec![Action::SetScanning(true)];
        out.extend(self.scan_next(now));
        out
    }

    /// Conectarse a la red `ssid` (si hay una conexión, la deja).
    pub fn connect(&mut self, ssid: &[u8], cred: Credential, now: u64) -> Vec<Action> {
        let mut out = self.leave(true);
        self.target = Some(Target {
            ssid: ssid.to_vec(),
            cred,
        });
        self.failures = 0;
        self.last_failure = None;
        out.extend(self.try_join(now));
        out
    }

    /// Desconectarse y olvidar la red elegida.
    pub fn disconnect(&mut self) -> Vec<Action> {
        let out = self.leave(true);
        self.target = None;
        self.phase = Phase::Idle;
        out
    }

    /// Deja la red actual (avisándole al punto de acceso si `tell`).
    fn leave(&mut self, tell: bool) -> Vec<Action> {
        let mut out = Vec::new();
        if let Some(bss) = &self.bss {
            let bssid = bss.bssid;
            if tell && matches!(self.phase, Phase::Connected | Phase::Connecting) {
                let seq = self.next_seq();
                out.push(Action::Tx(frame::deauth(self.mac, bssid, 3, seq)));
            }
            if self.phase == Phase::Connected {
                out.push(Action::Link(false));
            }
            out.push(Action::Disassociated);
        }
        self.bss = None;
        self.supplicant = None;
        self.ptk = None;
        self.gtk = None;
        out
    }

    /// Elige el mejor punto de acceso con el nombre buscado y empieza; si no lo vio, busca.
    fn try_join(&mut self, now: u64) -> Vec<Action> {
        let Some(t) = &self.target else {
            return Vec::new();
        };
        let best = self
            .seen
            .iter()
            .filter(|s| s.bss.ssid == t.ssid && now.saturating_sub(s.at) < FORGET_MS)
            .max_by_key(|s| s.rssi)
            .map(|s| s.bss.clone());
        let Some(bss) = best else {
            self.join_after_scan = true;
            return self.start_scan(now);
        };
        let usable = matches!(
            (&t.cred, bss.security),
            (Credential::Open, Security::Open) | (Credential::Psk(_), Security::Wpa2Psk)
        );
        if !usable {
            return self.fail(Failure::Unsupported, now);
        }
        self.phase = Phase::Connecting;
        // Con WMM, la estación es "de QoS": manda datos QoS (y anuncia 802.11n/ac).
        self.qos = bss.wmm;
        let mut out = alloc::vec![
            Action::SetScanning(false),
            Action::SetChannel(bss.channel),
            Action::SetBssid(bss.bssid),
            Action::Calibrate,
        ];
        self.bss = Some(bss);
        self.step = Step::Auth;
        self.tries = 0;
        out.extend(self.send_step(now));
        out
    }

    fn send_step(&mut self, now: u64) -> Vec<Action> {
        let Some(bss) = self.bss.clone() else {
            return Vec::new();
        };
        self.tries += 1;
        let seq = self.next_seq();
        match self.step {
            Step::Auth => {
                self.deadline = now + STEP_TIMEOUT_MS;
                alloc::vec![Action::Tx(frame::auth_request(self.mac, bss.bssid, seq))]
            }
            Step::Assoc => {
                self.deadline = now + STEP_TIMEOUT_MS;
                let ie = (bss.security == Security::Wpa2Psk).then(rsn::own_element);
                alloc::vec![Action::Tx(frame::assoc_request(
                    self.mac,
                    &bss,
                    ie.as_deref(),
                    seq
                ))]
            }
            Step::Handshake => Vec::new(),
        }
    }

    /// Un intento falló: anotar por qué y esperar para reintentar.
    fn fail(&mut self, why: Failure, now: u64) -> Vec<Action> {
        let mut out = self.leave(false);
        self.last_failure = Some(why);
        if self.target.is_none() {
            self.phase = Phase::Idle;
            return out;
        }
        let wait = BACKOFF_MS[self.failures.min(BACKOFF_MS.len() - 1)];
        self.failures += 1;
        self.phase = Phase::Waiting;
        self.retry_at = now + wait;
        out.push(Action::SetScanning(false));
        out
    }

    // --- el tiempo ---------------------------------------------------------------------------------

    fn scan_next(&mut self, now: u64) -> Vec<Action> {
        let Some(ch) = self.scan.pop() else {
            return self.scan_done(now);
        };
        let mut out = alloc::vec![Action::SetChannel(ch)];
        if passive(ch) {
            self.scan_until = now + DWELL_PASSIVE_MS;
        } else {
            self.scan_until = now + DWELL_ACTIVE_MS;
            let seq = self.next_seq();
            out.push(Action::Tx(frame::probe_request(self.mac, &[], seq)));
        }
        out
    }

    fn scan_done(&mut self, now: u64) -> Vec<Action> {
        self.seen.retain(|s| now.saturating_sub(s.at) < FORGET_MS);
        self.phase = Phase::Idle;
        let mut out = alloc::vec![Action::SetScanning(false)];
        if core::mem::take(&mut self.join_after_scan) {
            let found = self
                .target
                .as_ref()
                .is_some_and(|t| self.seen.iter().any(|s| s.bss.ssid == t.ssid));
            if found {
                out.extend(self.try_join(now));
            } else {
                out.extend(self.fail(Failure::NotFound, now));
            }
        }
        out
    }

    /// Lo que hay que hacer porque pasó el tiempo (llamarla seguido: cada 10–50 ms).
    pub fn poll(&mut self, now: u64) -> Vec<Action> {
        match self.phase {
            Phase::Scanning if now >= self.scan_until => self.scan_next(now),
            Phase::Waiting if now >= self.retry_at => self.try_join(now),
            Phase::Connecting if now >= self.deadline => match self.step {
                Step::Handshake => {
                    let why = if self.msg1_seen {
                        Failure::WrongPassword
                    } else {
                        Failure::NoResponse
                    };
                    self.fail(why, now)
                }
                _ if self.tries >= STEP_TRIES => self.fail(Failure::NoResponse, now),
                _ => self.send_step(now),
            },
            Phase::Connected if now.saturating_sub(self.last_beacon) > BEACON_LOSS_MS => {
                self.fail(Failure::Lost, now)
            }
            _ => Vec::new(),
        }
    }

    // --- tramas que llegan ------------------------------------------------------------------------

    /// Una trama recibida (sin CRC), con la señal en dBm.
    pub fn receive(&mut self, f: &[u8], rssi: Option<i8>, now: u64) -> Vec<Action> {
        let Some(h) = frame::Header::parse(f) else {
            return Vec::new();
        };
        if h.addr1 != self.mac && h.addr1 != BROADCAST && h.addr1[0] & 1 == 0 {
            return Vec::new(); // no es para nosotros
        }
        match h.kind() {
            frame::TYPE_MGMT => self.receive_mgmt(f, &h, rssi, now),
            frame::TYPE_DATA => self.receive_data(f, &h, now),
            _ => Vec::new(),
        }
    }

    fn receive_mgmt(
        &mut self,
        f: &[u8],
        h: &frame::Header,
        rssi: Option<i8>,
        now: u64,
    ) -> Vec<Action> {
        let ours = self.bss.as_ref().map(|b| b.bssid);
        match h.subtype() {
            frame::BEACON | frame::PROBE_RESP => {
                let Some(bss) = frame::parse_beacon(f) else {
                    return Vec::new();
                };
                let rssi = rssi.unwrap_or(-90);
                if Some(bss.bssid) == ours {
                    self.last_beacon = now;
                    self.rssi = rssi;
                }
                match self.seen.iter_mut().find(|s| s.bss.bssid == bss.bssid) {
                    Some(s) => {
                        s.rssi = rssi;
                        s.at = now;
                        if !bss.ssid.is_empty() {
                            s.bss = bss;
                        }
                    }
                    None => self.seen.push(Seen { bss, rssi, at: now }),
                }
                Vec::new()
            }
            frame::AUTH if self.phase == Phase::Connecting && self.step == Step::Auth => {
                let Some(bssid) = ours else {
                    return Vec::new();
                };
                match frame::auth_response(f, bssid) {
                    Some(0) => {
                        self.step = Step::Assoc;
                        self.tries = 0;
                        self.send_step(now)
                    }
                    Some(code) => self.fail(Failure::Rejected(code), now),
                    None => Vec::new(),
                }
            }
            frame::ASSOC_RESP if self.phase == Phase::Connecting && self.step == Step::Assoc => {
                let Some(bssid) = ours else {
                    return Vec::new();
                };
                let Some(info) = frame::assoc_info(f, bssid) else {
                    return Vec::new();
                };
                if info.status != 0 {
                    return self.fail(Failure::Rejected(info.status), now);
                }
                self.associated(info, now)
            }
            frame::DEAUTH | frame::DISASSOC => {
                let Some(bssid) = ours else {
                    return Vec::new();
                };
                match frame::disconnected(f, bssid) {
                    Some(reason) if self.phase != Phase::Idle => {
                        self.fail(Failure::Kicked(reason), now)
                    }
                    _ => Vec::new(),
                }
            }
            frame::ACTION if self.phase == Phase::Connected => {
                let seq = self.next_seq();
                frame::addba_refusal(f, self.mac, seq)
                    .map(|r| alloc::vec![self.protect(r)])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    fn associated(&mut self, info: frame::AssocInfo, now: u64) -> Vec<Action> {
        let Some(bss) = self.bss.clone() else {
            return Vec::new();
        };
        let rates = if info.rates.is_empty() {
            &bss.rates
        } else {
            &info.rates
        };
        let mut out = alloc::vec![Action::Associated {
            aid: info.aid,
            legacy: frame::rate_bits(rates),
            // Solo si las dos puntas lo anunciaron (la estación lo anuncia con WMM).
            ht: bss.ht && bss.wmm && info.ht,
            vht: bss.vht && bss.wmm && bss.channel > 14 && info.vht,
        }];
        self.last_beacon = now;
        let Some(Target { cred, .. }) = self.target.clone() else {
            return out;
        };
        match cred {
            Credential::Open => {
                self.phase = Phase::Connected;
                self.failures = 0;
                out.push(Action::Link(true));
            }
            Credential::Psk(pmk) => {
                let snonce = self.snonce();
                self.supplicant = Some(Supplicant::new(
                    self.mac,
                    bss.bssid,
                    pmk,
                    snonce,
                    rsn::own_element(),
                    bss.rsn.clone(),
                ));
                self.msg1_seen = false;
                self.step = Step::Handshake;
                self.deadline = now + HANDSHAKE_MS;
            }
        }
        out
    }

    fn receive_data(&mut self, f: &[u8], h: &frame::Header, now: u64) -> Vec<Action> {
        let Some(bss) = &self.bss else {
            return Vec::new();
        };
        if h.addr2 != bss.bssid || h.flags() & frame::FROM_DS == 0 {
            return Vec::new();
        }
        let plain = if h.protected() {
            let group = h.addr1[0] & 1 != 0;
            let key = if group {
                self.gtk.as_mut()
            } else {
                self.ptk.as_mut()
            };
            match key.map(|k| k.decrypt(f)) {
                Some(Ok(p)) => p,
                _ => return Vec::new(), // sin clave, repetida o falsa
            }
        } else {
            f.to_vec()
        };
        let Some(eth) = frame::to_ethernet(&plain) else {
            return Vec::new();
        };
        if eth[6..12] == self.mac {
            return Vec::new(); // una difusión nuestra que el punto de acceso repite
        }
        let ethertype = u16::from_be_bytes([eth[12], eth[13]]);
        if ethertype == eapol::ETHERTYPE {
            return self.receive_eapol(&eth[14..], now);
        }
        // Con WPA2, los datos sin cifrar se descartan.
        if self.phase != Phase::Connected || (!h.protected() && self.ptk.is_some()) {
            return Vec::new();
        }
        alloc::vec![Action::Deliver(eth)]
    }

    fn receive_eapol(&mut self, body: &[u8], now: u64) -> Vec<Action> {
        let Some(s) = self.supplicant.as_mut() else {
            return Vec::new();
        };
        let outputs = match s.handle(body) {
            Ok(o) => o,
            Err(eapol::Error::Rsn) => return self.fail(Failure::Kicked(0), now),
            Err(_) => return Vec::new(),
        };
        let done_before = self.phase == Phase::Connected;
        self.msg1_seen = true;
        let mut out = Vec::new();
        for o in outputs {
            match o {
                Output::Send(eapol_frame) => {
                    if let Some(bss) = &self.bss {
                        let mut eth = Vec::with_capacity(14 + eapol_frame.len());
                        eth.extend_from_slice(&bss.bssid);
                        eth.extend_from_slice(&self.mac);
                        eth.extend_from_slice(&eapol::ETHERTYPE.to_be_bytes());
                        eth.extend_from_slice(&eapol_frame);
                        if let Some(tx) = self.data_frame(&eth) {
                            out.push(tx);
                        }
                    }
                }
                Output::Pairwise(tk) => self.ptk = Some(Key::new(&tk, 0, 0)),
                Output::Group { key, id, rsc } => {
                    if let Ok(k) = <[u8; 16]>::try_from(key.as_slice()) {
                        self.gtk = Some(Key::new(&k, id, rsc));
                    }
                }
            }
        }
        if !done_before && self.supplicant.as_ref().is_some_and(Supplicant::done) {
            self.phase = Phase::Connected;
            self.failures = 0;
            self.last_beacon = now;
            out.push(Action::Link(true));
        }
        out
    }

    // --- datos que salen ---------------------------------------------------------------------------

    /// Una trama Ethernet de la pila de red → la trama 802.11 para mandar (cifrada con WPA2).
    /// `None` si no está conectada.
    pub fn send(&mut self, eth: &[u8]) -> Option<Action> {
        if self.phase != Phase::Connected {
            return None;
        }
        self.data_frame(eth)
    }

    fn data_frame(&mut self, eth: &[u8]) -> Option<Action> {
        let bssid = self.bss.as_ref()?.bssid;
        let seq = self.next_seq();
        let plain = if self.qos {
            frame::from_ethernet_qos(eth, bssid, seq)?
        } else {
            frame::from_ethernet(eth, bssid, seq)?
        };
        Some(self.protect(plain))
    }

    /// Cifra una trama de datos si ya hay clave (las de gestión van en claro).
    fn protect(&mut self, plain: Vec<u8>) -> Action {
        let data = plain
            .first()
            .is_some_and(|b| (b >> 2) & 3 == frame::TYPE_DATA);
        match self.ptk.as_mut() {
            Some(k) if data => match k.encrypt(&plain) {
                Ok(c) => Action::Tx(c),
                Err(ccmp::Error::Malformed | ccmp::Error::Replay | ccmp::Error::Mic) => {
                    Action::Tx(plain)
                }
            },
            _ => Action::Tx(plain),
        }
    }
}
