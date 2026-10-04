//! La placa de red del sistema, sea cual sea (K13).
//!
//! La pila TCP/IP (smoltcp, en `jarvis-net`) habla con la placa por el trait `phy::Device`.
//! virtio-net lo implementa directo (escribe las tramas en sus buffers sin copiar). Las placas
//! reales (Intel e1000, Realtek) implementan algo más simple, `Frames`: recibir una trama y
//! mandar una. `Nic` une los casos para que la tarea de la red no sepa cuál hay.
//!
//! K14: el Wi-Fi (wifi.rs) también es `Frames`. Si hay cable y Wi-Fi, se manda por el Wi-Fi
//! cuando está conectado y si no por el cable, y se recibe de los dos. Las dos usan la misma
//! dirección MAC (la del cable), así la pila de red no cambia de identidad.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use smoltcp::phy::{self, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

use crate::virtio_net::{self, VirtioNet};
use crate::wifi::Wifi;
use crate::{e1000, rtl8139, rtl8169, serial_println};

/// Lo que tiene que saber hacer un driver de placa de red.
pub trait Frames: Send {
    fn mac(&self) -> [u8; 6];
    /// Una trama recibida (sin CRC), si hay.
    fn recv(&mut self) -> Option<Vec<u8>>;
    /// ¿Hay lugar para mandar una trama?
    fn can_send(&mut self) -> bool;
    fn send(&mut self, frame: &[u8]);
}

pub enum Nic {
    Virtio(VirtioNet),
    Ring(Box<dyn Frames>),
    Wifi(Box<Wifi>),
    /// Cable y Wi-Fi. `bool`: a quién le toca recibir primero (para que uno no tape al otro).
    Both(Box<dyn Frames>, Box<Wifi>, bool),
}

/// MTU de Ethernet.
const MTU: usize = 1514;

impl Nic {
    pub fn mac(&self) -> [u8; 6] {
        match self {
            Nic::Virtio(v) => v.mac(),
            Nic::Ring(r) | Nic::Both(r, _, _) => r.mac(),
            Nic::Wifi(w) => w.mac(),
        }
    }

    /// El Wi-Fi, si hay.
    pub fn wifi(&mut self) -> Option<&mut Wifi> {
        match self {
            Nic::Wifi(w) | Nic::Both(_, w, _) => Some(w),
            _ => None,
        }
    }
}

/// Busca una placa real: Intel, después Realtek 8168 y 8139.
pub fn probe() -> Option<(Box<dyn Frames>, &'static str)> {
    if let Some(n) = e1000::probe() {
        return Some((Box::new(n), "Intel e1000"));
    }
    if let Some(n) = rtl8169::probe() {
        return Some((Box::new(n), "Realtek RTL8168"));
    }
    if let Some(n) = rtl8139::probe() {
        return Some((Box::new(n), "Realtek RTL8139"));
    }
    serial_println!("red: no hay placa de red conocida");
    None
}

pub struct RxToken(Vec<u8>);

pub enum TxToken<'a> {
    Virtio(virtio_net::TxToken<'a>),
    Ring(&'a mut dyn Frames),
}

impl phy::RxToken for RxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        match self {
            TxToken::Virtio(t) => t.consume(len, f),
            TxToken::Ring(r) => {
                let mut frame = vec![0u8; len.min(MTU)];
                let result = f(&mut frame);
                r.send(&frame);
                result
            }
        }
    }
}

impl Device for Nic {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn receive(&mut self, now: Instant) -> Option<(RxToken, TxToken<'_>)> {
        match self {
            Nic::Virtio(v) => {
                let (rx, tx) = v.receive(now)?;
                Some((RxToken(rx.into_frame()), TxToken::Virtio(tx)))
            }
            Nic::Ring(r) => {
                let frame = r.recv()?;
                Some((RxToken(frame), TxToken::Ring(&mut **r)))
            }
            Nic::Wifi(w) => {
                let frame = w.recv()?;
                Some((RxToken(frame), TxToken::Ring(&mut **w)))
            }
            Nic::Both(wired, wifi, turn) => {
                *turn = !*turn;
                let frame = if *turn {
                    wired.recv().or_else(|| wifi.recv())
                } else {
                    wifi.recv().or_else(|| wired.recv())
                }?;
                let out: &mut dyn Frames = if wifi.connected() {
                    &mut **wifi
                } else {
                    &mut **wired
                };
                Some((RxToken(frame), TxToken::Ring(out)))
            }
        }
    }

    fn transmit(&mut self, now: Instant) -> Option<TxToken<'_>> {
        match self {
            Nic::Virtio(v) => v.transmit(now).map(TxToken::Virtio),
            Nic::Ring(r) => r.can_send().then_some(TxToken::Ring(&mut **r)),
            Nic::Wifi(w) => w.can_send().then_some(TxToken::Ring(&mut **w)),
            Nic::Both(wired, wifi, _) => {
                let out: &mut dyn Frames = if wifi.connected() {
                    &mut **wifi
                } else {
                    &mut **wired
                };
                out.can_send().then_some(TxToken::Ring(out))
            }
        }
    }

    fn capabilities(&self) -> DeviceCapabilities {
        match self {
            Nic::Virtio(v) => v.capabilities(),
            Nic::Ring(_) | Nic::Wifi(_) | Nic::Both(..) => {
                let mut caps = DeviceCapabilities::default();
                caps.max_transmission_unit = MTU;
                caps.max_burst_size = None;
                caps.medium = Medium::Ethernet;
                caps
            }
        }
    }
}
