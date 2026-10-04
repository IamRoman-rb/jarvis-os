//! El Wi-Fi como placa de red (K14, ADR 0012): la RTL8821CE + la estación de `jarvis-wifi`.
//!
//! Para la pila TCP/IP (smoltcp) esto es una placa Ethernet más ([`Frames`]): recibe y manda
//! tramas Ethernet. Por dentro, cada trama pasa por la estación (que la convierte en 802.11 y la
//! cifra) y por el driver de la placa. La estación necesita que la llamen seguido (búsqueda de
//! redes, reintentos, pérdida de beacons): eso pasa en cada `recv`, que la tarea de la red llama
//! al menos cada 50 ms y enseguida que la placa avisa por MSI.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use jarvis_desktop::{WifiFailure, WifiInfo, WifiNetwork, WifiOp, WifiSecurity, WifiState};
use jarvis_drivers::rtw88::chip::{Link, Rtw8821c};
use jarvis_wifi::frame::Security;
use jarvis_wifi::station::{Action, Credential, Failure, Phase, Station};

use crate::nic::Frames;
use crate::rtw88::PciBus;
use crate::{entropy, rtw88, serial_println, time};

/// Dónde está el firmware en el disco (lo pone `cargo xtask`).
pub const FIRMWARE: &str = "/Sistema/firmware/rtw8821c_fw.bin";

pub struct Wifi {
    bus: PciBus,
    chip: Rtw8821c,
    sta: Station,
    rx: VecDeque<Vec<u8>>,
    /// La conexión subió o bajó desde la última vez que se preguntó.
    link_change: Option<bool>,
    pub name: String,
}

impl Wifi {
    /// Busca la placa y la arranca con `firmware`. `mac`: la dirección que usa la pila de red
    /// (si ya hay otra placa, la misma: así la conexión sigue con la misma IP); `None`, la del
    /// efuse.
    pub fn probe(firmware: Option<&[u8]>, mac: Option<[u8; 6]>) -> Option<Wifi> {
        let (mut bus, dev) = rtw88::probe()?;
        let Some(fw) = firmware else {
            serial_println!(
                "WIFI RTL8821CE: falta el firmware ({FIRMWARE}); sin Wi-Fi (lo copia cargo xtask)"
            );
            return None;
        };
        let t = time::millis();
        let mut chip = match Rtw8821c::start(&mut bus, fw) {
            Ok(c) => c,
            Err(e) => {
                serial_println!("WIFI RTL8821CE: {e}");
                return None;
            }
        };
        if let Some(m) = mac {
            chip.set_mac(&mut bus, m);
        }
        let msi = rtw88::enable_msi(dev);
        bus.enable_irqs(true);
        let (v, s, i) = chip.fw_version;
        let m = chip.mac;
        serial_println!(
            "WIFI RTL8821CE: firmware {v}.{s}.{i}, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, antena tipo {}, versión {}, MSI: {}, arrancó en {} ms",
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5],
            chip.efuse.rfe_option,
            chip.cut,
            if msi { "sí" } else { "no" },
            time::millis() - t
        );
        let mut seed = [0u8; 32];
        if !entropy::fill(&mut seed) {
            // Sin generador del kernel: el reloj (los números del saludo dejan de ser secretos,
            // pero WPA2 sigue funcionando: la clave sale también del número del punto de acceso).
            seed[..8].copy_from_slice(&time::rdtsc().to_le_bytes());
        }
        Some(Wifi {
            sta: Station::new(chip.mac, seed),
            bus,
            chip,
            rx: VecDeque::new(),
            link_change: None,
            name: "Realtek RTL8821CE (Wi-Fi)".into(),
        })
    }

    pub fn connected(&self) -> bool {
        self.sta.connected()
    }

    /// ¿Subió o bajó la conexión? (Una vez por cambio.)
    pub fn take_link_change(&mut self) -> Option<bool> {
        self.link_change.take()
    }

    pub fn op(&mut self, op: WifiOp) {
        let now = time::millis();
        let actions = match op {
            WifiOp::Scan => self.sta.scan(now),
            WifiOp::Connect { ssid, pmk } => {
                let cred = pmk.map_or(Credential::Open, Credential::Psk);
                serial_println!("WIFI conectar a {ssid:?}");
                self.sta.connect(ssid.as_bytes(), cred, now)
            }
            WifiOp::Disconnect => self.sta.disconnect(),
        };
        self.apply(actions);
    }

    pub fn info(&self) -> WifiInfo {
        let state = match self.sta.phase() {
            Phase::Idle => WifiState::Idle,
            Phase::Scanning => WifiState::Scanning,
            Phase::Connecting => WifiState::Connecting,
            Phase::Connected => WifiState::Connected,
            Phase::Waiting => WifiState::Waiting,
        };
        let (ssid, rssi, channel) = self
            .sta
            .current()
            .map_or((None, 0, 0), |(s, r, c)| (Some(s), r, c));
        WifiInfo {
            chip: self.name.clone(),
            state,
            ssid,
            rssi,
            channel,
            failure: self.sta.last_failure.map(failure),
            networks: self
                .sta
                .networks()
                .into_iter()
                .map(|n| WifiNetwork {
                    ssid: String::from_utf8_lossy(&n.ssid).into_owned(),
                    rssi: n.rssi,
                    channel: n.channel,
                    security: match n.security {
                        Security::Open => WifiSecurity::Open,
                        Security::Wpa2Psk => WifiSecurity::Wpa2,
                        Security::Unsupported => WifiSecurity::Unsupported,
                    },
                })
                .collect(),
        }
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::SetChannel(ch) => self.chip.set_channel(&mut self.bus, ch),
                Action::SetScanning(on) => self.chip.set_scanning(&mut self.bus, on),
                Action::SetBssid(b) => self.chip.set_bssid(&mut self.bus, &b),
                Action::Calibrate => {
                    if !self.chip.calibrate(&mut self.bus) {
                        serial_println!("WIFI calibración de la radio (IQK) sin respuesta");
                    }
                }
                Action::Associated {
                    aid,
                    legacy,
                    ht,
                    vht,
                } => {
                    serial_println!("WIFI asociada (AID {aid}, HT {ht}, VHT {vht})");
                    let link = Link { legacy, ht, vht };
                    self.chip.set_associated(&mut self.bus, Some(aid), &link);
                }
                Action::Disassociated => {
                    self.chip
                        .set_associated(&mut self.bus, None, &Link::default());
                }
                Action::Tx(frame) => {
                    if !self.chip.send(&mut self.bus, &frame) {
                        serial_println!("WIFI cola llena: se descarta una trama");
                    }
                }
                Action::Deliver(eth) => {
                    if self.rx.len() < 256 {
                        self.rx.push_back(eth);
                    }
                }
                Action::Link(up) => {
                    serial_println!("WIFI {}", if up { "CONECTADA" } else { "DESCONECTADA" });
                    self.link_change = Some(up);
                }
            }
        }
    }

    /// Lo que llegó por la radio y lo que toca por el tiempo.
    fn service(&mut self) {
        self.bus.ack_irqs();
        let now = time::millis();
        for buf in self.bus.take_rx() {
            if let Some((frame, rssi)) = self.chip.receive(&buf) {
                let actions = self.sta.receive(frame, rssi, now);
                self.apply(actions);
            }
        }
        let actions = self.sta.poll(now);
        self.apply(actions);
    }
}

fn failure(f: Failure) -> WifiFailure {
    match f {
        Failure::NotFound => WifiFailure::NotFound,
        Failure::Unsupported => WifiFailure::Unsupported,
        Failure::NoResponse => WifiFailure::NoResponse,
        Failure::Rejected(c) => WifiFailure::Rejected(c),
        Failure::WrongPassword => WifiFailure::WrongPassword,
        Failure::Lost => WifiFailure::Lost,
        Failure::Kicked(c) => WifiFailure::Kicked(c),
    }
}

impl Frames for Wifi {
    fn mac(&self) -> [u8; 6] {
        self.sta.mac()
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        if self.rx.is_empty() {
            self.service();
        }
        self.rx.pop_front()
    }

    fn can_send(&mut self) -> bool {
        self.sta.connected()
    }

    fn send(&mut self, frame: &[u8]) {
        if let Some(Action::Tx(f)) = self.sta.send(frame) {
            self.chip.send(&mut self.bus, &f);
        }
    }
}
