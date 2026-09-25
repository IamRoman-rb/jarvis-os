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

// --- Conexiones largas -------------------------------------------------------------------

use jarvis_desktop::{StreamEvent, StreamOp, StreamRequest};

/// Un servidor "eco" que devuelve todo lo que recibe, y cierra cuando recibe `FIN\n`.
fn echo(net: &mut Net<Loopback>, h: SocketHandle, echoed: &mut usize) {
    let s = net.sockets_mut().get_mut::<tcp::Socket>(h);
    while s.can_recv() && s.can_send() {
        let mut buf = [0u8; 4096];
        let room = s.send_capacity() - s.send_queue();
        let n = s.recv_slice(&mut buf[..room.min(4096)]).unwrap();
        if n == 0 {
            break;
        }
        s.send_slice(&buf[..n]).unwrap();
        *echoed += n;
        if buf[..n].ends_with(b"FIN\n") {
            s.close();
        }
    }
}

fn connect(net: &mut Net<Loopback>, id: u32, port: u16) {
    net.stream(
        StreamOp::Connect(StreamRequest {
            id,
            host: "127.0.0.1".into(),
            port,
            app: "brave".into(),
        }),
        0,
    );
}

#[test]
fn conexion_larga_manda_y_recibe_mucho_y_cierra() {
    let mut n = net();
    let srv = server(&mut n, 7000);
    connect(&mut n, 3, 7000);
    // 300 KB en trozos: más que los buffers de los dos lados.
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut sent = false;
    let (mut got, mut connected, mut closed, mut echoed) = (Vec::new(), false, None, 0);
    for now in 1..200_000u64 {
        n.poll(now);
        echo(&mut n, srv, &mut echoed);
        for (id, ev) in n.take_stream_events() {
            assert_eq!(id, 3);
            match ev {
                StreamEvent::Connected => connected = true,
                StreamEvent::Data(d) => got.extend_from_slice(&d),
                StreamEvent::Closed(e) => closed = Some(e),
            }
        }
        if connected && !sent {
            for chunk in payload.chunks(10_000) {
                n.stream(StreamOp::Send(3, chunk.to_vec()), now);
            }
            n.stream(StreamOp::Send(3, b"FIN\n".to_vec()), now);
            sent = true;
        }
        if closed.is_some() {
            break;
        }
    }
    assert!(connected);
    assert_eq!(closed, Some(None), "cierre normal");
    assert_eq!(got.len(), payload.len() + 4);
    assert_eq!(&got[..payload.len()], &payload[..]);
    assert_eq!(n.open_streams(), 0);
}

#[test]
fn conexion_larga_rechazada_y_cierre_propio() {
    let mut n = net();
    connect(&mut n, 1, 9);
    let mut events = Vec::new();
    for now in 1..5_000 {
        n.poll(now);
        events.extend(n.take_stream_events());
        if !events.is_empty() {
            break;
        }
    }
    assert!(matches!(&events[0], (1, StreamEvent::Closed(Some(e))) if e.contains("rechaz")));

    // Cierre pedido por la app: el servidor ve el FIN.
    let srv = server(&mut n, 7001);
    connect(&mut n, 2, 7001);
    let mut state = None;
    for now in 5_000..20_000 {
        n.poll(now);
        for (_, ev) in n.take_stream_events() {
            if ev == StreamEvent::Connected {
                n.stream(StreamOp::Close(2), now);
            }
            if let StreamEvent::Closed(e) = ev {
                assert_eq!(e, None);
            }
        }
        let st = n.sockets_mut().get::<tcp::Socket>(srv).state();
        if st == tcp::State::CloseWait {
            state = Some(st);
            break;
        }
    }
    assert_eq!(
        state,
        Some(tcp::State::CloseWait),
        "el servidor recibió el FIN"
    );
}
