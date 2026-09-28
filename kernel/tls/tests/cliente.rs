//! El cliente TLS contra un servidor de referencia (rustls con ring), en memoria: los dos se pasan
//! los bytes como si fuera un socket. Cubre cada suite, los dos intercambios de claves y los tres
//! tipos de certificado, y los errores que tienen que cortar la conexión.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use jarvis_tls::rng::{Pool, Rng, SEED_BITS};
use jarvis_tls::{Error, GetRandomFailed, SecureRandom, TimeProvider, TlsClient, UnixTime};
use rustls::crypto::ring as reference;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{CipherSuite, NamedGroup, RootCertStore, ServerConfig, ServerConnection};

const HOST: &str = "prueba.jarvis";
/// 2027-01-01: adentro de la vigencia de los certificados de prueba (2026–2126).
const NOW: u64 = 1_798_761_600;

fn datos(name: &str) -> String {
    format!("{}/tests/datos/{name}", env!("CARGO_MANIFEST_DIR"))
}

// --- Lo que en el kernel ponen entropy.rs y rtc.rs --------------------------------------------

#[derive(Debug)]
struct TestRandom(Mutex<Rng>);

impl SecureRandom for TestRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        self.0.lock().unwrap().fill(buf);
        Ok(())
    }
}

fn random() -> &'static TestRandom {
    let mut p = Pool::new();
    // Semilla distinta en cada test (el orden y la hora cambian): no hace falta que sea secreta.
    p.add(
        &std::time::SystemTime::now()
            .elapsed()
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
        0,
    );
    p.add(
        format!("{:?}", std::thread::current().id()).as_bytes(),
        SEED_BITS,
    );
    Box::leak(Box::new(TestRandom(Mutex::new(Rng::from_seed(p.seed())))))
}

#[derive(Debug)]
struct FixedTime(u64);

impl TimeProvider for FixedTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            self.0,
        )))
    }
}

fn roots(cas: &[&str]) -> RootCertStore {
    let mut r = RootCertStore::empty();
    for ca in cas {
        r.add(CertificateDer::from_pem_file(datos(&format!("{ca}.pem"))).unwrap())
            .unwrap();
    }
    r
}

fn client(cas: &[&str], now: u64, host: &str) -> Result<TlsClient, Error> {
    let config = jarvis_tls::client_config(random(), Arc::new(FixedTime(now)), roots(cas))?;
    TlsClient::new(config, host)
}

// --- El servidor de referencia ----------------------------------------------------------------

fn server(cert: &str, suite: Option<CipherSuite>, group: Option<NamedGroup>) -> ServerConnection {
    let mut provider = reference::default_provider();
    if let Some(s) = suite {
        provider.cipher_suites.retain(|c| c.suite() == s);
    }
    if let Some(g) = group {
        provider.kx_groups.retain(|k| k.name() == g);
    }
    let chain = vec![CertificateDer::from_pem_file(datos(&format!("{cert}.pem"))).unwrap()];
    let key = PrivateKeyDer::from_pem_file(datos(&format!("{cert}.key"))).unwrap();
    let config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_protocol_versions(rustls::ALL_VERSIONS)
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    ServerConnection::new(Arc::new(config)).unwrap()
}

/// Pasa bytes en los dos sentidos hasta que nadie tenga nada nuevo. `tamper` puede alterar lo
/// que va del servidor al cliente.
fn pump(
    c: &mut TlsClient,
    s: &mut ServerConnection,
    tamper: &mut dyn FnMut(&mut Vec<u8>),
) -> Result<(), Error> {
    for _ in 0..50 {
        let out = c.take_outgoing();
        let mut moved = !out.is_empty();
        if moved {
            s.read_tls(&mut &out[..]).unwrap();
            if s.process_new_packets().is_err() {
                // El servidor rechazó algo: que mande su alerta al cliente.
                let mut alert = Vec::new();
                let _ = s.write_tls(&mut alert);
                return c.receive(&alert);
            }
        }
        let mut back = Vec::new();
        while s.wants_write() {
            s.write_tls(&mut back).unwrap();
        }
        if !back.is_empty() {
            moved = true;
            tamper(&mut back);
            c.receive(&back)?;
        }
        if !moved {
            return Ok(());
        }
    }
    panic!("la conversación no terminó");
}

/// Handshake completo y un pedido HTTP de ida y vuelta.
fn round_trip(cert: &str, ca: &str, suite: Option<CipherSuite>, group: Option<NamedGroup>) {
    let mut c = client(&[ca], NOW, HOST).unwrap();
    let mut s = server(cert, suite, group);
    pump(&mut c, &mut s, &mut |_| {}).unwrap();
    assert!(
        c.is_established(),
        "{cert} {suite:?} {group:?}: sin handshake"
    );
    assert!(!s.is_handshaking());
    if let Some(suite) = suite {
        assert_eq!(s.negotiated_cipher_suite().unwrap().suite(), suite);
    }
    assert_eq!(s.alpn_protocol(), None, "el servidor no ofrece ALPN");

    let pedido = b"GET / HTTP/1.1\r\nHost: prueba.jarvis\r\n\r\n";
    c.send(pedido).unwrap();
    pump(&mut c, &mut s, &mut |_| {}).unwrap();
    let mut recibido = vec![0u8; pedido.len()];
    s.reader().read_exact(&mut recibido).unwrap();
    assert_eq!(recibido, pedido);

    // Una respuesta más grande que un registro (16 KiB): llega partida y se junta.
    let respuesta: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    s.writer().write_all(&respuesta).unwrap();
    pump(&mut c, &mut s, &mut |_| {}).unwrap();
    assert_eq!(c.take_plaintext(), respuesta);

    // Cierre ordenado en los dos sentidos.
    s.send_close_notify();
    c.close().unwrap();
    pump(&mut c, &mut s, &mut |_| {}).unwrap();
    assert!(c.peer_closed());
    assert!(matches!(c.send(b"x"), Err(Error::Closed)));
}

#[test]
fn tls13_cada_suite() {
    for suite in [
        CipherSuite::TLS13_AES_128_GCM_SHA256,
        CipherSuite::TLS13_AES_256_GCM_SHA384,
        CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
    ] {
        round_trip("srv-ec256", "ca-ec256", Some(suite), None);
    }
}

#[test]
fn tls12_cada_suite_ecdsa() {
    for suite in [
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    ] {
        round_trip("srv-ec256", "ca-ec256", Some(suite), None);
    }
}

#[test]
fn tls12_y_tls13_con_rsa() {
    for suite in [
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
        CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256,
        CipherSuite::TLS13_AES_128_GCM_SHA256,
    ] {
        round_trip("srv-rsa", "ca-rsa", Some(suite), None);
    }
}

#[test]
fn certificado_p384() {
    round_trip("srv-ec384", "ca-ec384", None, None);
    round_trip(
        "srv-ec384",
        "ca-ec384",
        Some(CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384),
        None,
    );
}

#[test]
fn cada_intercambio_de_claves() {
    for group in [NamedGroup::X25519, NamedGroup::secp256r1] {
        round_trip("srv-ec256", "ca-ec256", None, Some(group));
        round_trip(
            "srv-ec256",
            "ca-ec256",
            Some(CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256),
            Some(group),
        );
    }
}

fn expect_cert_error(result: Result<(), Error>, what: &str) {
    match result {
        Err(Error::Tls(rustls::Error::InvalidCertificate(_))) => {}
        other => panic!("{what}: se esperaba un certificado inválido, llegó {other:?}"),
    }
}

#[test]
fn autoridad_desconocida() {
    let mut c = client(&["ca-ec256"], NOW, HOST).unwrap();
    let mut s = server("srv-extrano", None, None);
    expect_cert_error(pump(&mut c, &mut s, &mut |_| {}), "autoridad extraña");
    assert!(!c.is_established());
}

#[test]
fn otro_nombre() {
    let mut c = client(&["ca-ec256"], NOW, "otro.jarvis").unwrap();
    let mut s = server("srv-ec256", None, None);
    expect_cert_error(pump(&mut c, &mut s, &mut |_| {}), "otro nombre");
}

#[test]
fn certificado_vencido_o_futuro() {
    // 2200: ya venció. 2000: todavía no era válido.
    for now in [7_258_118_400, 946_684_800] {
        let mut c = client(&["ca-ec256"], now, HOST).unwrap();
        let mut s = server("srv-ec256", None, None);
        expect_cert_error(pump(&mut c, &mut s, &mut |_| {}), "fuera de fecha");
    }
}

#[test]
fn con_las_raices_de_mozilla_un_certificado_de_prueba_no_pasa() {
    let config = jarvis_tls::client_config(
        random(),
        Arc::new(FixedTime(NOW)),
        jarvis_tls::mozilla_roots(),
    )
    .unwrap();
    assert!(jarvis_tls::mozilla_roots().len() > 100);
    let mut c = TlsClient::new(config, HOST).unwrap();
    let mut s = server("srv-ec256", None, None);
    expect_cert_error(pump(&mut c, &mut s, &mut |_| {}), "raíces de Mozilla");
}

#[test]
fn un_byte_alterado_corta_la_conexion() {
    for suite in [
        CipherSuite::TLS13_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
        CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256,
    ] {
        let mut c = client(&["ca-ec256"], NOW, HOST).unwrap();
        let mut s = server("srv-ec256", Some(suite), None);
        pump(&mut c, &mut s, &mut |_| {}).unwrap();
        assert!(c.is_established());
        s.writer().write_all(b"datos importantes").unwrap();
        let r = pump(&mut c, &mut s, &mut |b| {
            let last = b.len() - 1;
            b[last] ^= 1;
        });
        assert!(
            matches!(r, Err(Error::Tls(rustls::Error::DecryptError))),
            "{suite:?}: {r:?}"
        );
        assert!(c.take_plaintext().is_empty());
    }
}

#[test]
fn nombres_invalidos() {
    assert!(matches!(
        client(&["ca-ec256"], NOW, "no es un nombre"),
        Err(Error::BadName(_))
    ));
    // Una IP es un nombre válido para TLS (el certificado tendría que decirla).
    assert!(client(&["ca-ec256"], NOW, "10.0.2.2").is_ok());
}

#[test]
fn el_primer_mensaje_esta_listo_al_crear() {
    let mut c = client(&["ca-ec256"], NOW, HOST).unwrap();
    let hello = c.take_outgoing();
    // Registro de handshake (22), y adentro un ClientHello (1).
    assert_eq!(hello[0], 22);
    assert_eq!(hello[5], 1);
    assert!(c.take_outgoing().is_empty());
    assert!(!c.is_established());
}

#[test]
fn firma_falsa_de_una_autoridad_con_el_mismo_nombre() {
    // El emisor dice ser "ca-ec256", pero lo firmó otra clave: solo verificar la firma lo delata.
    let mut c = client(&["ca-ec256"], NOW, HOST).unwrap();
    let mut s = server("srv-impostor", None, None);
    expect_cert_error(pump(&mut c, &mut s, &mut |_| {}), "firma falsa");
    assert!(!c.is_established());
}
