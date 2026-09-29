//! El cliente TLS, sin entrada/salida propia ("sans-I/O"): el llamador le pasa los bytes que
//! llegan por TCP ([`TlsClient::receive`]) y manda los que el cliente deja listos
//! ([`TlsClient::take_outgoing`]). Así el mismo código corre en los tests del host (con un
//! servidor en memoria) y en el kernel (sobre un socket de smoltcp).
//!
//! Por dentro usa la API "unbuffered" de rustls, la única disponible sin `std`: rustls dice en
//! qué estado está (hay que mandar algo, llegaron datos, se puede escribir, falta leer) y este
//! módulo reacciona hasta que no hay nada más que hacer.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use rustls::client::UnbufferedClientConnection;
use rustls::crypto::SecureRandom;
use rustls::pki_types::ServerName;
use rustls::time_provider::TimeProvider;
use rustls::unbuffered::{
    ConnectionState, EncodeError, EncryptError, InsufficientSizeError, UnbufferedStatus,
};
use rustls::{ClientConfig, RootCertStore};

use crate::provider::provider;

/// Qué salió mal.
#[derive(Debug)]
pub enum Error {
    /// El nombre del servidor no es un nombre DNS ni una IP válidos.
    BadName(String),
    /// Error de TLS: certificado inválido o vencido, el servidor no habla TLS, un registro
    /// alterado en el camino...
    Tls(rustls::Error),
    /// Se intentó escribir después de cerrar.
    Closed,
}

impl From<rustls::Error> for Error {
    fn from(e: rustls::Error) -> Self {
        Error::Tls(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadName(n) => write!(f, "nombre de servidor inválido: {n}"),
            Error::Tls(rustls::Error::InvalidCertificate(e)) => {
                write!(f, "el certificado del sitio no es válido ({e:?})")
            }
            Error::Tls(e) => write!(f, "error de TLS: {e}"),
            Error::Closed => f.write_str("la conexión segura ya estaba cerrada"),
        }
    }
}

/// Las raíces de confianza: las de Mozilla (las mismas que Firefox), compiladas en el kernel.
pub fn mozilla_roots() -> RootCertStore {
    RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// La configuración de los clientes: TLS 1.3 y 1.2, nuestro proveedor, las raíces dadas y la
/// hora (`time`, para ver si los certificados están vigentes). Se arma una vez y se comparte.
pub fn client_config(
    random: &'static dyn SecureRandom,
    time: Arc<dyn TimeProvider>,
    roots: RootCertStore,
) -> Result<Arc<ClientConfig>, Error> {
    let mut config = ClientConfig::builder_with_details(Arc::new(provider(random)), time)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    // Hablamos HTTP/1.1: si el servidor soporta ALPN, así no elige HTTP/2.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    // Sin reanudación de sesiones: cada conexión hace el handshake entero (más simple; la caché
    // de sesiones de rustls necesita `std`).
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}

/// Una conexión TLS del lado del cliente.
pub struct TlsClient {
    conn: UnbufferedClientConnection,
    /// Bytes que llegaron por TCP y rustls todavía no consumió (puede faltar el resto de un
    /// registro).
    incoming: Vec<u8>,
    /// Bytes cifrados listos para mandar por TCP.
    outgoing: Vec<u8>,
    /// Texto que el llamador quiere mandar y todavía no se pudo cifrar (handshake en curso).
    pending: Vec<u8>,
    /// Texto descifrado que el llamador todavía no leyó.
    plaintext: Vec<u8>,
    established: bool,
    close_requested: bool,
    close_sent: bool,
    peer_closed: bool,
}

impl TlsClient {
    /// Empieza una conexión con `host` (el nombre que tiene que estar en el certificado). Deja el
    /// primer mensaje (ClientHello) en [`take_outgoing`](Self::take_outgoing).
    pub fn new(config: Arc<ClientConfig>, host: &str) -> Result<TlsClient, Error> {
        let name = ServerName::try_from(String::from(host))
            .map_err(|_| Error::BadName(String::from(host)))?;
        let mut c = TlsClient {
            conn: UnbufferedClientConnection::new(config, name)?,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            pending: Vec::new(),
            plaintext: Vec::new(),
            established: false,
            close_requested: false,
            close_sent: false,
            peer_closed: false,
        };
        c.drive()?;
        Ok(c)
    }

    /// Bytes que llegaron por TCP.
    pub fn receive(&mut self, tls: &[u8]) -> Result<(), Error> {
        self.incoming.extend_from_slice(tls);
        self.drive()
    }

    /// Texto a mandar (se cifra apenas termina el handshake).
    pub fn send(&mut self, plaintext: &[u8]) -> Result<(), Error> {
        if self.close_requested || self.peer_closed {
            return Err(Error::Closed);
        }
        self.pending.extend_from_slice(plaintext);
        self.drive()
    }

    /// Avisa al servidor que no vamos a mandar más (close_notify).
    pub fn close(&mut self) -> Result<(), Error> {
        self.close_requested = true;
        self.drive()
    }

    /// Los bytes cifrados para mandar por TCP (los saca: la próxima vez vienen los nuevos).
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.outgoing)
    }

    /// El texto que llegó descifrado (lo saca).
    pub fn take_plaintext(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.plaintext)
    }

    /// ¿Terminó el handshake? (el certificado ya se validó)
    pub fn is_established(&self) -> bool {
        self.established
    }

    /// ¿El servidor cerró la conexión segura? (después de leer lo que quede, no llega más)
    pub fn peer_closed(&self) -> bool {
        self.peer_closed
    }

    /// Hace avanzar la conexión hasta que no haya nada que hacer sin más bytes del servidor.
    fn drive(&mut self) -> Result<(), Error> {
        loop {
            let UnbufferedStatus { mut discard, state } =
                self.conn.process_tls_records(&mut self.incoming);
            let progress = match state? {
                ConnectionState::ReadTraffic(mut s) => {
                    while let Some(record) = s.next_record() {
                        let record = record?;
                        discard += record.discard;
                        self.plaintext.extend_from_slice(record.payload);
                    }
                    true
                }
                ConnectionState::EncodeTlsData(mut s) => {
                    append(&mut self.outgoing, |buf| match s.encode(buf) {
                        Ok(n) => Ok(Ok(n)),
                        Err(EncodeError::InsufficientSize(InsufficientSizeError {
                            required_size,
                        })) => Err(required_size),
                        Err(e) => Ok(Err(rustls::Error::General(alloc::format!("{e}")))),
                    })?;
                    true
                }
                ConnectionState::TransmitTlsData(s) => {
                    // Los bytes ya están en `outgoing`: el llamador los manda con take_outgoing.
                    s.done();
                    true
                }
                ConnectionState::WriteTraffic(mut w) => {
                    self.established = true;
                    if !self.pending.is_empty() {
                        let data = core::mem::take(&mut self.pending);
                        append(&mut self.outgoing, |buf| match w.encrypt(&data, buf) {
                            Ok(n) => Ok(Ok(n)),
                            Err(EncryptError::InsufficientSize(InsufficientSizeError {
                                required_size,
                            })) => Err(required_size),
                            Err(e) => Ok(Err(rustls::Error::General(alloc::format!("{e}")))),
                        })?;
                        true
                    } else if self.close_requested && !self.close_sent {
                        append(&mut self.outgoing, |buf| match w.queue_close_notify(buf) {
                            Ok(n) => Ok(Ok(n)),
                            Err(EncryptError::InsufficientSize(InsufficientSizeError {
                                required_size,
                            })) => Err(required_size),
                            Err(e) => Ok(Err(rustls::Error::General(alloc::format!("{e}")))),
                        })?;
                        self.close_sent = true;
                        true
                    } else {
                        false
                    }
                }
                ConnectionState::PeerClosed | ConnectionState::Closed => {
                    self.peer_closed = true;
                    false
                }
                // Falta que llegue más del servidor (BlockedHandshake), o estados que un
                // cliente sin datos tempranos no ve.
                _ => false,
            };
            self.incoming.drain(..discard);
            if !progress {
                return Ok(());
            }
        }
    }
}

/// Escribe al final de `out` con `f`, que devuelve cuántos bytes escribió o, si no le alcanzó el
/// espacio, cuántos necesita (y se reintenta con ese tamaño).
fn append(
    out: &mut Vec<u8>,
    mut f: impl FnMut(&mut [u8]) -> Result<Result<usize, rustls::Error>, usize>,
) -> Result<(), Error> {
    let start = out.len();
    let mut room = 4096;
    loop {
        out.resize(start + room, 0);
        match f(&mut out[start..]) {
            Ok(Ok(n)) => {
                out.truncate(start + n);
                return Ok(());
            }
            Ok(Err(e)) => {
                out.truncate(start);
                return Err(e.into());
            }
            Err(needed) if needed > room => room = needed,
            // Pidió menos de lo que ya tenía: no debería pasar; evita un bucle infinito.
            Err(_) => {
                out.truncate(start);
                return Err(rustls::Error::General("buffer de TLS inconsistente".into()).into());
            }
        }
    }
}
