//! Descargas de punta a punta en memoria: la pila TCP/IP habla consigo misma por una placa
//! "loopback" y un servidor HTTP de juguete (otro socket de smoltcp) contesta.

use jarvis_desktop::NetRequest;
use jarvis_net::{Net, parse_ipv4};
use smoltcp::iface::SocketHandle;
use smoltcp::phy::{Loopback, Medium};
use smoltcp::socket::tcp;

const MAC: [u8; 6] = [0x02, 0, 0, 0, 0, 1];

fn server(net: &mut Net<Loopback>, port: u16) -> SocketHandle {
    let mut s = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 16 * 1024]),
        tcp::SocketBuffer::new(vec![0; 64 * 1024]),
    );
    s.listen(port).unwrap();
    net.sockets_mut().add(s)
}

/// Atiende un pedido en `h` con `respond(pedido) -> respuesta`. Devuelve true si respondió.
fn serve(
    net: &mut Net<Loopback>,
    h: SocketHandle,
    seen: &mut Vec<String>,
    respond: &dyn Fn(&str) -> Vec<u8>,
) {
    let s = net.sockets_mut().get_mut::<tcp::Socket>(h);
    if s.can_recv() {
        let mut buf = [0u8; 4096];
        let n = s.recv_slice(&mut buf).unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        if req.contains("\r\n\r\n") {
            let resp = respond(&req);
            s.send_slice(&resp).unwrap();
            s.close();
            seen.push(req);
        }
    }
}

fn run(
    net: &mut Net<Loopback>,
    servers: &[SocketHandle],
    url: &str,
    respond: &dyn Fn(&str) -> Vec<u8>,
) -> (Result<jarvis_desktop::HttpResponse, String>, Vec<String>) {
    let mut seen = Vec::new();
    let mut now = 10;
    if let Some((_, r)) = net.fetch(
        NetRequest {
            id: 7,
            url: url.into(),
            kind: Default::default(),
            app: "terminal".into(),
        },
        now,
    ) {
        return (r, seen);
    }
    for _ in 0..20_000 {
        now += 1;
        if let Some((id, r)) = net.poll(now).into_iter().next() {
            assert_eq!(id, 7);
            return (r, seen);
        }
        for &h in servers {
            serve(net, h, &mut seen, respond);
        }
    }
    panic!("la descarga no terminó");
}

fn net() -> Net<Loopback> {
    Net::with_static(Loopback::new(Medium::Ethernet), MAC, [127, 0, 0, 1], 8, 0)
}

#[test]
fn descarga_con_redireccion() {
    let mut n = net();
    let servers = [server(&mut n, 8080), server(&mut n, 8080)];
    let (r, seen) = run(&mut n, &servers, "http://127.0.0.1:8080/viejo", &|req| {
        if req.starts_with("GET /viejo ") {
            b"HTTP/1.1 301 Moved\r\nLocation: /nuevo\r\nContent-Length: 0\r\n\r\n".to_vec()
        } else {
            let body = "<h1>JARVIS</h1>".repeat(2000); // 30 KB: varios segmentos TCP
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .into_bytes()
        }
    });
    let r = r.expect("respuesta");
    assert_eq!(r.status, 200);
    assert_eq!(r.url, "http://127.0.0.1:8080/nuevo");
    assert_eq!(r.body.len(), 15 * 2000);
    assert_eq!(seen.len(), 2);
    assert!(seen[0].contains("Host: 127.0.0.1\r\n"));
    assert!(seen[1].starts_with("GET /nuevo HTTP/1.1\r\n"));
}

#[test]
fn https_va_por_el_puente_del_anfitrion() {
    let mut n = net();
    n.set_proxy([127, 0, 0, 1], 8118);
    let servers = [server(&mut n, 8118)];
    let (r, seen) = run(&mut n, &servers, "https://example.com/hola", &|_| {
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhola!\r\n0\r\n\r\n"
            .to_vec()
    });
    assert_eq!(r.unwrap().body, b"hola!");
    assert!(seen[0].starts_with("GET https://example.com/hola HTTP/1.1\r\nHost: example.com\r\n"));
}

#[test]
fn conexion_rechazada_da_error() {
    let mut n = net();
    let (r, _) = run(&mut n, &[], "http://127.0.0.1:9/", &|_| Vec::new());
    assert!(r.unwrap_err().contains("rechazó"));
    let (r, _) = run(&mut n, &[], "ftp://x", &|_| Vec::new());
    assert!(r.is_err());
    assert_eq!(parse_ipv4("10.0.2.15"), Some([10, 0, 2, 15]));
    assert_eq!(parse_ipv4("example.com"), None);
}
