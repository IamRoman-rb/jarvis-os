//! La ABI de Linux x86_64 de JARVIS-OS (hito K11, ADR 0010).
//!
//! Para que un programa compilado para Linux corra sin cambios, el kernel tiene que hablar su
//! idioma: cargar el ELF donde espera, armarle la pila de arranque como Linux y responder sus
//! llamadas al sistema con los mismos números, estructuras y errores.
//!
//! - [`elf`]: qué va a dónde (static y static-pie; los dinámicos se rechazan).
//! - [`stack`]: `argc`, `argv`, `envp` y el vector auxiliar.
//! - [`mm`]: las zonas de memoria del proceso (brk, mmap, pila) y la asignación al primer uso.
//! - [`process`]: los descriptores de archivo y las llamadas al sistema.
//! - [`sys`]: lo que se le pide al resto del sistema (páginas, archivos, consola, red, reloj).
//!
//! Todo es `no_std` y sin hardware: los tests corren programas "de mentira" contra un sistema de
//! mentira; el kernel pone las tablas de páginas, `syscall` y las colas hacia el escritorio.
//! Referencias: `man 2 syscalls`, la tabla `syscall_64.tbl` del kernel Linux y la "System V ABI,
//! AMD64 Architecture Processor Supplement".

#![no_std]

extern crate alloc;

pub mod abi;
pub mod elf;
pub mod mm;
pub mod process;
pub mod stack;
pub mod sys;

pub use process::{Flow, Process};

#[cfg(test)]
mod tests;
