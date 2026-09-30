//! HTTPS hecho por el kernel (K10), de punta a punta en memoria: la descarga va por la placa
//! "loopback" a un servidor TLS de referencia (rustls con ring, otra criptografía) sentado sobre
//! un socket de smoltcp. Se prueba que el pedido llega cifrado y sin pasar por el puente, y que
//! un certificado que no es de confianza corta la descarga.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use jarvis_desktop::{HttpResponse, NetRequest};
use jarvis_net::Net;
use jarvis_tls::rng::{Pool, Rng, SEED_BITS};
use jarvis_tls::{GetRandomFailed, SecureRandom, TimeProvider, UnixTime};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{RootCertStore, ServerConfig, ServerConnection};
use smoltcp::iface::SocketHandle;
use smoltcp::phy::{Loopback, Medium};
use smoltcp::socket::tcp;

const MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
/// 2027-01-01: adentro de la vigencia de los certificados de prueba.
const NOW: u64 = 1_798_761_600;

fn datos(name: &str) -> String {
    format!("{}/../tls/tests/datos/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[derive(Debug)]
struct TestRandom(Mutex<Rng>);

impl SecureRandom for TestRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        self.0.lock().unwrap().fill(buf);
        Ok(())
    }
}

#[derive(Debug)]
struct FixedTime;

impl TimeProvider for FixedTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            NOW,
        )))
    }
}

/// La configuración del cliente, confiando solo en la autoridad `ca`.
fn client_config(ca: &str) -> Arc<jarvis_tls::ClientConfig> {
    let mut p = Pool::new();
    p.add(
        format!("{:?}", std::time::SystemTime::now()).as_bytes(),
        SEED_BITS,
    );
    let random: &'static TestRandom =
        Box::leak(Box::new(TestRandom(Mutex::new(Rng::from_seed(p.seed())))));
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(datos(&format!("{ca}.pem"))).unwrap())
        .unwrap();
    jarvis_tls::client_config(random, Arc::new(FixedTime), roots).unwrap()
}

fn tls_server() -> ServerConnection {
    let chain = vec![CertificateDer::from_pem_file(datos("srv-ip.pem")).unwrap()];
    let key = PrivateKeyDer::from_pem_file(datos("srv-ip.key")).unwrap();
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(rustls::ALL_VERSIONS)
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
    ServerConnection::new(Arc::new(config)).unwrap()
}

fn listen(net: &mut Net<Loopback>, port: u16) -> SocketHandle {
    let mut s = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 64 * 1024]),
        tcp::SocketBuffer::new(vec![0; 64 * 1024]),
    );
    s.listen(port).unwrap();
    net.sockets_mut().add(s)
}

/// El servidor: pasa los bytes entre el socket y rustls; cuando llega un pedido HTTP completo,
/// contesta, manda close_notify y cierra. Guarda el pedido en `seen` (ya descifrado).
struct Server {
    socket: SocketHandle,
    tls: ServerConnection,
    request: Vec<u8>,
    seen: Option<String>,
    body: Vec<u8>,
}

impl Server {
    fn step(&mut self, net: &mut Net<Loopback>) {
        let s = net.sockets_mut().get_mut::<tcp::Socket>(self.socket);
        while s.can_recv() {
            let mut buf = [0u8; 4096];
            let n = s.recv_slice(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            self.tls.read_tls(&mut &buf[..n]).unwrap();
            if self.tls.process_new_packets().is_err() {
                // El cliente rechazó algo (o al revés): la alerta sale igual.
                break;
            }
            let _ = self.tls.reader().read_to_end(&mut self.request);
        }
        if self.seen.is_none() && self.request.windows(4).any(|w| w == b"\r\n\r\n") {
            self.seen = Some(String::from_utf8_lossy(&self.request).into_owned());
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
                self.body.len()
            );
            let w = &mut self.tls.writer();
            w.write_all(head.as_bytes()).unwrap();
            w.write_all(&self.body).unwrap();
            self.tls.send_close_notify();
        }
        let mut out = Vec::new();
        while self.tls.wants_write() {
            self.tls.write_tls(&mut out).unwrap();
        }
        if !out.is_empty() {
            assert_eq!(
                s.send_slice(&out).unwrap(),
                out.len(),
                "buffer del servidor"
            );
        }
        if self.seen.is_some() && !self.tls.wants_write() && s.send_queue() == 0 {
            s.close();
        }
    }
}

fn net() -> Net<Loopback> {
    Net::with_static(Loopback::new(Medium::Ethernet), MAC, [127, 0, 0, 1], 8, 0)
}

fn run(net: &mut Net<Loopback>, url: &str, srv: &mut Server) -> Result<HttpResponse, String> {
    let mut now = 10;
    let req = NetRequest {
        id: 3,
        url: url.into(),
        kind: Default::default(),
        app: "navegador".into(),
    };
    if let Some((_, r)) = net.fetch(req, now) {
        return r;
    }
    for _ in 0..20_000 {
        now += 1;
        if let Some((id, r)) = net.poll(now).into_iter().next() {
            assert_eq!(id, 3);
            return r;
        }
        srv.step(net);
    }
    panic!("la descarga no terminó");
}

fn server(net: &mut Net<Loopback>, body: Vec<u8>) -> Server {
    Server {
        socket: listen(net, 8443),
        tls: tls_server(),
        request: Vec::new(),
        seen: None,
        body,
    }
}

#[test]
fn https_directo_sin_el_puente() {
    let mut n = net();
    n.set_tls(Some(client_config("ca-ec256")));
    assert!(n.https_direct());
    // Varios registros TLS (de 16 KiB como mucho) y varios segmentos TCP.
    let body = "<p>JARVIS</p>".repeat(5000).into_bytes();
    let mut srv = server(&mut n, body.clone());
    let r = run(&mut n, "https://127.0.0.1:8443/hola", &mut srv).expect("respuesta");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, body);
    assert_eq!(r.url, "https://127.0.0.1:8443/hola");
    // Al servidor le llegó el pedido de un servidor común (no el de un proxy) y cifrado.
    assert!(srv.seen.unwrap().starts_with("GET /hola HTTP/1.1\r\n"));
}

#[test]
fn certificado_de_otra_autoridad_corta_la_descarga() {
    let mut n = net();
    // El cliente solo confía en ca-rsa; el servidor presenta uno firmado por ca-ec256.
    n.set_tls(Some(client_config("ca-rsa")));
    let mut srv = server(&mut n, b"secreto".to_vec());
    let e = run(&mut n, "https://127.0.0.1:8443/", &mut srv).unwrap_err();
    assert!(e.contains("certificado"), "{e}");
    assert!(srv.seen.is_none(), "el pedido no tiene que salir");
}

#[test]
fn con_el_puente_elegido_https_va_al_anfitrion() {
    let mut n = net();
    n.set_tls(Some(client_config("ca-ec256")));
    n.set_https_bridge(true);
    assert!(!n.https_direct());
    n.set_proxy([127, 0, 0, 1], 8118);
    let proxy = listen(&mut n, 8118);
    let mut now = 10;
    let req = NetRequest {
        id: 1,
        url: "https://example.com/".into(),
        kind: Default::default(),
        app: "navegador".into(),
    };
    assert!(n.fetch(req, now).is_none());
    let mut seen = Vec::new();
    for _ in 0..2000 {
        now += 1;
        n.poll(now);
        let s = n.sockets_mut().get_mut::<tcp::Socket>(proxy);
        if s.can_recv() {
            let mut buf = [0u8; 4096];
            let k = s.recv_slice(&mut buf).unwrap();
            seen.extend_from_slice(&buf[..k]);
            break;
        }
    }
    assert!(seen.starts_with(b"GET https://example.com/ HTTP/1.1\r\n"));
}
