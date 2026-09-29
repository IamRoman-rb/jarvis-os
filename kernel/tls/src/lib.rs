//! TLS de JARVIS-OS (K10, ADR 0009). `no_std` y sin hardware: las fuentes de entropía, la hora y
//! la red las pone el kernel.
//!
//! - [`rng`]: el generador de números al azar. Es lo primero que necesita TLS: las claves
//!   efímeras del handshake tienen que ser impredecibles, o todo lo demás no sirve.
//! - [`provider`]: la criptografía para rustls (AEAD, ECDHE, firmas), sobre RustCrypto.
//! - [`client`]: el cliente TLS 1.3/1.2, sin entrada/salida propia.

#![no_std]

extern crate alloc;

pub mod client;
pub mod provider;
pub mod rng;

pub use client::{Error, TlsClient, client_config, mozilla_roots};
pub use rustls::ClientConfig;
pub use rustls::crypto::{GetRandomFailed, SecureRandom};
pub use rustls::pki_types::UnixTime;
pub use rustls::time_provider::TimeProvider;
