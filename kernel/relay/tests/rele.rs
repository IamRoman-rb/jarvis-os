//! El relé reenvía los marcos entre las conexiones del mismo grupo, y a nadie más.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn frame(p: &[u8]) -> Vec<u8> {
    let mut v = (p.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(p);
    v
}

fn join(port: u16, group: u8) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut hello = b"JSR1".to_vec();
    hello.extend_from_slice(&[group; 16]);
    s.write_all(&frame(&hello)).unwrap();
    s.set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    s
}

#[test]
fn reenvia_solo_dentro_del_grupo() {
    let l = jarvis_relay::bind(false, 0).unwrap();
    let port = l.local_addr().unwrap().port();
    thread::spawn(move || jarvis_relay::serve(l));
    let mut a = join(port, 1);
    let mut b = join(port, 1);
    let mut other = join(port, 2);
    thread::sleep(Duration::from_millis(200));
    a.write_all(&frame(b"cifrado")).unwrap();
    let mut got = [0u8; 11];
    b.read_exact(&mut got).unwrap();
    assert_eq!(&got[4..], b"cifrado");
    let mut buf = [0u8; 1];
    assert!(other.read(&mut buf).is_err(), "otro grupo no recibe nada");
    assert!(
        a.read(&mut buf).is_err(),
        "el que manda no recibe su propio marco"
    );
}
