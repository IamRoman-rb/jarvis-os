//! TLS de JARVIS-OS (K10, ADR 0009). `no_std` y sin hardware: las fuentes de entropía y la red las
//! pone el kernel.
//!
//! Por ahora tiene el generador de números al azar ([`rng`]), que es lo primero que necesita TLS:
//! las claves efímeras del handshake tienen que ser impredecibles, o todo lo demás no sirve.

#![no_std]

pub mod rng;
