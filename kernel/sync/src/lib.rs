//! Sincronización de la carpeta `/Sincronizado` entre máquinas de JARVIS (ADR 0007).
//!
//! No sabe de discos ni de red: el llamador le cuenta qué archivos hay y le pasa los mensajes
//! que llegan; el motor le devuelve qué escribir, qué mandar a la Papelera y qué enviar.
//!
//! - [`pair`]: el código de emparejado y, de él, el id de grupo (lo ve el relé) y la clave.
//! - [`wire`]: el cifrado de cada mensaje (ChaCha20-Poly1305) y el marco para el relé.
//! - [`engine`]: el estado por archivo (hash, reloj de Lamport, versión de origen) y las reglas
//!   de conflicto: gana el cambio más nuevo y el otro queda como copia.

#![no_std]

extern crate alloc;

pub mod engine;
pub mod pair;
pub mod wire;

pub use engine::{Action, Engine, Hash, Reply, hash};
pub use pair::Group;
