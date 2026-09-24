//! Red de JARVIS-OS: pila TCP/IP, DHCP, DNS y descargas HTTP.
//!
//! La pila TCP/IP es [smoltcp](https://github.com/smoltcp-rs/smoltcp): una implementación en
//! Rust, sin `std` ni hilos, pensada justamente para sistemas embebidos y kernels. Esta crate la
//! conecta con el resto del sistema:
//!
//! - **DHCP**: al arrancar, pide una dirección IP, la puerta de enlace y el DNS (en QEMU los da
//!   la red "user": 10.0.2.15, 10.0.2.2 y 10.0.2.3).
//! - **DNS**: traduce nombres ("example.com") a direcciones IP.
//! - **Descargas**: el escritorio pide una dirección; acá se resuelve el nombre, se abre la
//!   conexión TCP, se manda el pedido HTTP, se junta la respuesta y se siguen las redirecciones
//!   (`jarvis_desktop::web::http::Fetch`).
//!
//! Todo es por *polling*: el bucle principal del kernel llama a [`Net::poll`] en cada vuelta.
//! La placa de red es cualquier `smoltcp::phy::Device`: en el kernel, el driver virtio-net; en
//! los tests, un dispositivo "loopback" en memoria.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use jarvis_desktop::web::http::{Connect, Fetch, PROXY_HOST, PROXY_PORT, Step, Target, request};
use jarvis_desktop::{HttpResponse, NetInfo, NetRequest};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::Device;
use smoltcp::socket::dns::{self, GetQueryResultError};
use smoltcp::socket::{dhcpv4, tcp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    DnsQueryType, EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address,
    Ipv4Cidr,
};

/// Tiempo máximo para cada etapa (resolver el nombre, conectar, recibir).
const STAGE_TIMEOUT_MS: u64 = 20_000;
/// DNS de respaldo, por si el que da el DHCP no contesta (se pregunta en orden).
const FALLBACK_DNS: [[u8; 4]; 2] = [[1, 1, 1, 1], [8, 8, 8, 8]];

/// Tope de una página (para que una descarga enorme no se coma toda la memoria).
const MAX_BODY: usize = 8 * 1024 * 1024;
const TCP_RX: usize = 64 * 1024;
const TCP_TX: usize = 8 * 1024;

enum Stage {
    /// Esperando la IP del servidor.
    Resolve {
        host: String,
        port: u16,
        query: Option<dns::QueryHandle>,
    },
    /// Conexión TCP abierta (o abriéndose): mandando el pedido y juntando la respuesta.
    Transfer {
        socket: SocketHandle,
        sent: usize,
        established: bool,
    },
}

struct Job {
    id: u32,
    fetch: Fetch,
    request: Vec<u8>,
    stage: Stage,
    since: u64,
    raw: Vec<u8>,
    /// El DNS falló y se está usando el puente del anfitrión.
    via_proxy_fallback: bool,
}

pub struct Net<D: Device> {
    dev: D,
    iface: Interface,
    sockets: SocketSet<'static>,
    dhcp: Option<SocketHandle>,
    dns: SocketHandle,
    info: NetInfo,
    jobs: Vec<Job>,
    next_port: u16,
    proxy: IpEndpoint,
}

fn instant(ms: u64) -> Instant {
    Instant::from_millis(ms as i64)
}

fn v4(a: Ipv4Address) -> [u8; 4] {
    a.octets()
}

impl<D: Device> Net<D> {
    /// Con DHCP (lo normal). `seed`: un número al azar para los números de secuencia de TCP.
    pub fn new(mut dev: D, mac: [u8; 6], seed: u64, now_ms: u64) -> Self {
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = seed;
        let iface = Interface::new(config, &mut dev, instant(now_ms));
        let mut sockets = SocketSet::new(vec![]);
        let dhcp = sockets.add(dhcpv4::Socket::new());
        let dns = sockets.add(dns::Socket::new(&[], vec![]));
        Net {
            dev,
            iface,
            sockets,
            dhcp: Some(dhcp),
            dns,
            info: NetInfo {
                present: true,
                mac,
                ..Default::default()
            },
            jobs: Vec::new(),
            next_port: 49152,
            proxy: IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(PROXY_HOST)), PROXY_PORT),
        }
    }

    /// Con dirección fija (sin DHCP): para los tests.
    pub fn with_static(dev: D, mac: [u8; 6], ip: [u8; 4], prefix: u8, now_ms: u64) -> Self {
        let mut net = Net::new(dev, mac, 1, now_ms);
        if let Some(h) = net.dhcp.take() {
            net.sockets.remove(h);
        }
        net.iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(Ipv4Address::from(ip)), prefix));
        });
        net.info.ip = Some(ip);
        net
    }

    /// A dónde se mandan los pedidos HTTPS (el puente del anfitrión).
    pub fn set_proxy(&mut self, ip: [u8; 4], port: u16) {
        self.proxy = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(ip)), port);
    }

    pub fn info(&self) -> &NetInfo {
        &self.info
    }

    pub fn device(&self) -> &D {
        &self.dev
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.dev
    }

    /// Para los tests: agregar un socket propio (por ejemplo, un servidor) a la pila.
    #[doc(hidden)]
    pub fn sockets_mut(&mut self) -> &mut SocketSet<'static> {
        &mut self.sockets
    }

    /// Hay descargas en curso.
    pub fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// Empieza a descargar `req.url`. La respuesta sale de [`poll`](Self::poll) con el mismo
    /// número de pedido.
    pub fn fetch(
        &mut self,
        req: NetRequest,
        now_ms: u64,
    ) -> Option<(u32, Result<HttpResponse, String>)> {
        let (fetch, step) = Fetch::start(&req.url);
        let connect = match step {
            Step::Connect(c) => c,
            Step::Failed(e) => return Some((req.id, Err(e))),
            Step::Done(r) => return Some((req.id, Ok(r))),
        };
        let mut job = Job {
            id: req.id,
            fetch,
            request: Vec::new(),
            stage: Stage::Resolve {
                host: String::new(),
                port: 0,
                query: None,
            },
            since: now_ms,
            raw: Vec::new(),
            via_proxy_fallback: false,
        };
        if let Err(e) = self.begin(&mut job, connect, now_ms) {
            return Some((req.id, Err(e)));
        }
        self.jobs.push(job);
        None
    }

    fn begin(&mut self, job: &mut Job, c: Connect, now_ms: u64) -> Result<(), String> {
        job.request = c.request;
        job.raw.clear();
        job.since = now_ms;
        match c.target {
            Target::Proxy => {
                let proxy = self.proxy;
                job.stage = self.connect(proxy)?;
            }
            Target::Direct { host, port } => {
                // ¿Ya es una dirección IP? Entonces no hace falta DNS.
                match parse_ipv4(&host) {
                    Some(ip) => {
                        let ep = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(ip)), port);
                        job.stage = self.connect(ep)?;
                    }
                    None => {
                        job.stage = Stage::Resolve {
                            host,
                            port,
                            query: None,
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn connect(&mut self, to: IpEndpoint) -> Result<Stage, String> {
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; TCP_RX]),
            tcp::SocketBuffer::new(vec![0; TCP_TX]),
        );
        let port = self.next_port;
        self.next_port = if self.next_port >= 65000 {
            49152
        } else {
            self.next_port + 1
        };
        socket
            .connect(self.iface.context(), to, port)
            .map_err(|e| format!("no se pudo abrir la conexión: {e:?}"))?;
        let handle = self.sockets.add(socket);
        Ok(Stage::Transfer {
            socket: handle,
            sent: 0,
            established: false,
        })
    }

    /// Una vuelta de la pila de red: procesa los paquetes que llegaron, manda los pendientes y
    /// avanza las descargas. Devuelve las que terminaron.
    pub fn poll(&mut self, now_ms: u64) -> Vec<(u32, Result<HttpResponse, String>)> {
        let now = instant(now_ms);
        self.iface.poll(now, &mut self.dev, &mut self.sockets);
        self.poll_dhcp();

        let mut done = Vec::new();
        let mut jobs = core::mem::take(&mut self.jobs);
        jobs.retain_mut(|job| match self.advance(job, now_ms) {
            Ok(None) => true,
            Ok(Some(resp)) => {
                done.push((job.id, Ok(resp)));
                false
            }
            Err(e) => {
                self.drop_socket(job);
                done.push((job.id, Err(e)));
                false
            }
        });
        jobs.append(&mut self.jobs);
        self.jobs = jobs;
        if !done.is_empty() {
            // Que salgan ya los paquetes de cierre de las conexiones terminadas.
            self.iface.poll(now, &mut self.dev, &mut self.sockets);
        }
        done
    }

    fn poll_dhcp(&mut self) {
        let Some(h) = self.dhcp else { return };
        // Se copia lo que interesa: el evento apunta adentro del socket.
        let event = match self.sockets.get_mut::<dhcpv4::Socket>(h).poll() {
            None => return,
            Some(dhcpv4::Event::Configured(cfg)) => Some((
                cfg.address,
                cfg.router,
                cfg.dns_servers
                    .iter()
                    .copied()
                    .collect::<Vec<Ipv4Address>>(),
            )),
            Some(dhcpv4::Event::Deconfigured) => None,
        };
        match event {
            Some((addr, router, dns_servers)) => {
                let addr: Ipv4Cidr = addr;
                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    let _ = addrs.push(IpCidr::Ipv4(addr));
                });
                match router {
                    Some(r) => {
                        let _ = self.iface.routes_mut().add_default_ipv4_route(r);
                    }
                    None => {
                        self.iface.routes_mut().remove_default_ipv4_route();
                    }
                }
                let mut servers: Vec<IpAddress> =
                    dns_servers.iter().map(|s| IpAddress::Ipv4(*s)).collect();
                for ip in FALLBACK_DNS {
                    let ip = IpAddress::Ipv4(Ipv4Address::from(ip));
                    if !servers.contains(&ip) {
                        servers.push(ip);
                    }
                }
                servers.truncate(4);
                self.sockets
                    .get_mut::<dns::Socket>(self.dns)
                    .update_servers(&servers);
                self.info.ip = Some(v4(addr.address()));
                self.info.gateway = router.map(v4);
                self.info.dns = dns_servers.first().copied().map(v4);
            }
            None => {
                self.iface.update_ip_addrs(|addrs| addrs.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                self.info.ip = None;
                self.info.gateway = None;
                self.info.dns = None;
            }
        }
    }

    fn drop_socket(&mut self, job: &mut Job) {
        if let Stage::Transfer { socket, .. } = job.stage {
            self.sockets.get_mut::<tcp::Socket>(socket).abort();
            self.sockets.remove(socket);
            // Para que no se use dos veces.
            job.stage = Stage::Resolve {
                host: String::new(),
                port: 0,
                query: None,
            };
        }
    }

    /// Avanza una descarga. `Ok(Some(..))` = terminó.
    fn advance(&mut self, job: &mut Job, now_ms: u64) -> Result<Option<HttpResponse>, String> {
        if now_ms.saturating_sub(job.since) > STAGE_TIMEOUT_MS {
            return Err(match &job.stage {
                Stage::Resolve { host, .. } => format!("{host}: el DNS no respondió a tiempo"),
                Stage::Transfer {
                    established: false, ..
                } => "el servidor no respondió a tiempo".into(),
                Stage::Transfer { .. } => "la página tardó demasiado en llegar".into(),
            });
        }
        match &mut job.stage {
            Stage::Resolve { host, port, query } => {
                if self.info.dns.is_none() && self.dhcp.is_some() {
                    return Ok(None); // todavía no hay DNS (esperando al DHCP)
                }
                let dns_socket = self.sockets.get_mut::<dns::Socket>(self.dns);
                let q = match query {
                    Some(q) => *q,
                    None => {
                        let q = dns_socket
                            .start_query(self.iface.context(), host, DnsQueryType::A)
                            .map_err(|e| format!("{host}: nombre inválido ({e:?})"))?;
                        *query = Some(q);
                        q
                    }
                };
                match dns_socket.get_query_result(q) {
                    Ok(addrs) => {
                        let ip = addrs
                            .iter()
                            .find_map(|a| match a {
                                IpAddress::Ipv4(v) => Some(*v),
                                #[allow(unreachable_patterns)]
                                _ => None,
                            })
                            .ok_or_else(|| format!("{host} no tiene dirección IPv4"))?;
                        let ep = IpEndpoint::new(IpAddress::Ipv4(ip), *port);
                        job.stage = self.connect(ep)?;
                        job.since = now_ms;
                        Ok(None)
                    }
                    Err(GetQueryResultError::Pending) => Ok(None),
                    Err(GetQueryResultError::Failed) => {
                        // Sin DNS: la página se pide por el puente del anfitrión, que resuelve
                        // el nombre con el DNS de la computadora anfitriona.
                        let request = request(&job.fetch.url, true);
                        let proxy = self.proxy;
                        match self.connect(proxy) {
                            Ok(stage) => {
                                job.request = request;
                                job.stage = stage;
                                job.since = now_ms;
                                job.via_proxy_fallback = true;
                                Ok(None)
                            }
                            Err(_) => Err(format!("no se encontró {host} (DNS)")),
                        }
                    }
                }
            }
            Stage::Transfer {
                socket,
                sent,
                established,
            } => {
                let s = self.sockets.get_mut::<tcp::Socket>(*socket);
                if s.state() == tcp::State::Established {
                    *established = true;
                }
                if s.can_send() && *sent < job.request.len() {
                    *sent += s
                        .send_slice(&job.request[*sent..])
                        .map_err(|e| format!("error al mandar el pedido: {e:?}"))?;
                }
                while s.can_recv() {
                    let mut buf = [0u8; 4096];
                    let n = s.recv_slice(&mut buf).map_err(|e| format!("{e:?}"))?;
                    if n == 0 {
                        break;
                    }
                    *established = true;
                    job.raw.extend_from_slice(&buf[..n]);
                    job.since = now_ms; // mientras lleguen datos, no vence
                    if job.raw.len() > MAX_BODY {
                        return Err("la página es demasiado grande (más de 8 MiB)".into());
                    }
                }
                let finished = *established && !s.may_recv() && !s.can_recv();
                if !*established && s.state() == tcp::State::Closed {
                    return Err(if job.via_proxy_fallback {
                        "el DNS no respondió y el puente del anfitrión no está (¿QEMU se abrió sin cargo xtask run?)".into()
                    } else {
                        "el servidor rechazó la conexión".into()
                    });
                }
                if !finished {
                    return Ok(None);
                }
                s.close();
                let handle = *socket;
                self.sockets.remove(handle);
                let raw = core::mem::take(&mut job.raw);
                match job.fetch.on_response(&raw) {
                    Step::Done(resp) => Ok(Some(resp)),
                    Step::Failed(e) => Err(e),
                    Step::Connect(c) => {
                        // Redirección: otra conexión.
                        job.stage = Stage::Resolve {
                            host: String::new(),
                            port: 0,
                            query: None,
                        };
                        self.begin(job, c, now_ms)?;
                        Ok(None)
                    }
                }
            }
        }
    }
}

/// "10.0.2.2" → [10, 0, 2, 2]
pub fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split('.');
    for o in &mut out {
        *o = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Dirección MAC como texto (para el log).
pub fn mac_string(m: [u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
        .to_string()
}
