//! La tarea de la red (K9): la pila TCP/IP corre aparte del escritorio.
//!
//! Antes, el bucle principal atendía la red cada 4 ms entre cuadro y cuadro; si un cuadro tardaba
//! (dibujar una página grande, guardar un archivo), la red esperaba. Ahora la pila tiene su
//! propia tarea, más prioritaria: cuando llega un paquete, la interrupción la despierta, lo
//! procesa y vuelve a dormir, aunque el escritorio esté en medio de un cuadro.
//!
//! El escritorio y la red se hablan por dos colas (**paso de mensajes**): pedidos para allá
//! (descargas y conexiones largas) y respuestas para acá. Ninguno toca los datos del otro; lo
//! único compartido son las colas, cada una con su `IrqMutex`, tomadas solo para meter o sacar.
//!
//! HTTPS (K10): si `tls::init` pudo armar la configuración, el TLS lo hace esta tarea (el
//! handshake corre acá, no en el escritorio). El interruptor de Configuración que manda HTTPS
//! al puente del anfitrión llega por [`set_https_bridge`].
//!
//! Wi-Fi (K14): los pedidos (buscar, conectarse) llegan por la misma cola; el estado va en
//! `NetInfo::wifi`. Cuando la conexión sube o baja, la dirección se vuelve a pedir por DHCP.

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use jarvis_desktop::{HttpResponse, NetInfo, NetRequest, StreamEvent, StreamOp, WifiOp};
use jarvis_net::Net;
use jarvis_task::Priority;

use crate::irqlock::IrqMutex;
use crate::nic::Nic;
use crate::{serial_println, task, time, tls};

/// Lo que la tarea de la red le devuelve al escritorio.
pub enum Answer {
    Response(u32, Result<HttpResponse, String>),
    Stream(u32, StreamEvent),
}

enum Order {
    Fetch(NetRequest),
    Stream(StreamOp),
    Wifi(WifiOp),
}

static ORDERS: IrqMutex<VecDeque<Order>> = IrqMutex::new(VecDeque::new());
static ANSWERS: IrqMutex<Vec<Answer>> = IrqMutex::new(Vec::new());
/// El estado de la placa (IP, DNS), copiado para el escritorio. `None`: no hay placa.
static INFO: IrqMutex<Option<NetInfo>> = IrqMutex::new(None);
/// HTTPS por el puente del anfitrión (Configuración → Red) en vez del TLS del kernel.
static HTTPS_BRIDGE: AtomicBool = AtomicBool::new(false);
static FIRST_IRQ: AtomicBool = AtomicBool::new(true);

/// Pila de la tarea: smoltcp y el HTTP no usan mucha (lo grande va en el heap), pero el
/// handshake de TLS sí (RSA y las curvas elípticas guardan números grandes en la pila).
const STACK: usize = 512 * 1024;
/// Aunque no pase nada, se revisa cada tanto: los plazos de las descargas (20 s) y los
/// reintentos de DNS se miran en `poll`, que `poll_delay` no conoce.
const MAX_SLEEP_MS: u64 = 50;
/// Vueltas seguidas sin dormir antes de cederle un rato la CPU al resto. La red es más
/// prioritaria que el escritorio: si nunca durmiera (una ráfaga larga), lo dejaría sin CPU.
const MAX_BUSY_ROUNDS: u32 = 8;

/// Arranca la tarea con la placa. `false` si no se pudo crear.
pub fn start(mut net: Net<Nic>) -> bool {
    let info = full_info(&mut net);
    INFO.with(|i| *i = Some(info));
    task::spawn("red", Priority::High, STACK, move || run(net)).is_some()
}

/// Hay red (placa con driver y su tarea corriendo).
pub fn present() -> bool {
    INFO.with(|i| i.is_some())
}

pub fn fetch(req: NetRequest) {
    ORDERS.with(|q| q.push_back(Order::Fetch(req)));
}

/// Lo pone el escritorio en cada cuadro (es un bool: más barato que avisar solo si cambia).
pub fn set_https_bridge(on: bool) {
    HTTPS_BRIDGE.store(on, Ordering::Relaxed);
}

pub fn stream(op: StreamOp) {
    ORDERS.with(|q| q.push_back(Order::Stream(op)));
}

pub fn wifi(op: WifiOp) {
    ORDERS.with(|q| q.push_back(Order::Wifi(op)));
}

/// El estado de la red con el del Wi-Fi.
fn full_info(net: &mut Net<Nic>) -> NetInfo {
    let mut info = net.info().clone();
    info.wifi = net.device_mut().wifi().map(|w| w.info());
    info
}

/// Despierta a la tarea si hay pedidos (una vez por cuadro, no por pedido).
pub fn kick() {
    if ORDERS.with(|q| !q.is_empty()) {
        task::signal(task::EV_NET_REQUEST);
    }
}

pub fn take_answers() -> Vec<Answer> {
    ANSWERS.with(core::mem::take)
}

pub fn info() -> NetInfo {
    INFO.with(|i| i.clone()).unwrap_or_default()
}

fn run(mut net: Net<Nic>) {
    let mut last_ip = None;
    let mut last_info = full_info(&mut net);
    let mut busy = 0;
    net.set_tls(tls::config());
    serial_println!(
        "RED_HTTPS {}",
        if net.https_direct() {
            "TLS del kernel"
        } else {
            "por el puente del anfitrión (sin TLS en el kernel)"
        }
    );
    loop {
        let now = time::millis();
        net.set_https_bridge(HTTPS_BRIDGE.load(Ordering::Relaxed));
        let orders = ORDERS.with(core::mem::take);
        let mut out = Vec::new();
        for order in orders {
            match order {
                Order::Fetch(req) => {
                    if let Some((id, result)) = net.fetch(req, now) {
                        out.push(Answer::Response(id, result));
                    }
                }
                Order::Stream(op) => net.stream(op, now),
                Order::Wifi(op) => {
                    if let Some(w) = net.device_mut().wifi() {
                        w.op(op);
                    }
                }
            }
        }
        for (id, result) in net.poll(now) {
            out.push(Answer::Response(id, result));
        }
        for (id, event) in net.take_stream_events() {
            out.push(Answer::Stream(id, event));
        }
        if !out.is_empty() {
            ANSWERS.with(|a| a.extend(out));
        }
        // El Wi-Fi se conectó o se cayó: la dirección se vuelve a pedir.
        if net
            .device_mut()
            .wifi()
            .and_then(|w| w.take_link_change())
            .is_some()
        {
            net.restart_dhcp();
        }
        let info = full_info(&mut net);
        if info != last_info {
            INFO.with(|i| *i = Some(info.clone()));
            last_info = info;
        }
        if net.info().ip != last_ip {
            last_ip = net.info().ip;
            match last_ip {
                Some(ip) => serial_println!("RED_IP {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
                None => serial_println!("RED_SIN_IP"),
            }
        }

        let delay = net
            .poll_delay(time::millis())
            .map_or(MAX_SLEEP_MS, |d| d.min(MAX_SLEEP_MS));
        if delay == 0 && busy < MAX_BUSY_ROUNDS {
            busy += 1;
            continue;
        }
        busy = 0;
        // Con delay 0 igual se duerme hasta el próximo tick (≤ 4 ms): le toca al escritorio.
        let until = time::millis() + delay.max(1);
        let woke = task::wait(task::EV_NET | task::EV_NET_REQUEST, Some(until));
        if woke & task::EV_NET != 0 && FIRST_IRQ.swap(false, Ordering::Relaxed) {
            serial_println!("RED_POR_INTERRUPCION");
        }
    }
}
