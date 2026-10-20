//! `vino`: programas de Windows en JARVIS-OS, como Wine en Linux (ADR 0018).
//!
//! JARVIS-OS habla solo la ABI de Linux. Un programa de Windows no le habla al kernel directo:
//! llama funciones de DLL (`kernel32.dll`, el CRT en `ucrtbase.dll`…). `vino` es un programa de
//! Linux común que:
//!
//! 1. lee el `.exe` ([`pe`]) y lo pone en memoria con `mmap`: secciones, relocalizaciones e
//!    imports ([`loader`]);
//! 2. llena cada import con una función **nuestra** escrita en Rust con la convención de llamada
//!    de Windows (`extern "win64"`): son las DLL "builtin" ([`dll`]). Por dentro usan la
//!    biblioteca estándar de Rust (musl), o sea las llamadas de Linux del kernel;
//! 3. arma lo que un programa de Windows espera encontrar: el TEB en el registro GS (con
//!    `arch_prctl(ARCH_SET_GS)`, una llamada de Linux), el PEB y el TLS ([`teb`]);
//! 4. salta al punto de entrada del programa ([`run`], solo en Linux).
//!
//! Si el programa pide algo que no tenemos, se resuelve a una función que avisa "falta
//! KERNEL32!Foo" y termina ([`dll::missing`]), en vez de romper en otro lado.
//!
//! Todo menos `run` y `os` es portable: los tests corren en el anfitrión.

pub mod dll;
pub mod loader;
pub mod modules;
pub mod os;
pub mod pe;
pub mod printf;
pub mod state;
pub mod teb;
pub mod unwind;
pub mod win;

#[cfg(target_os = "linux")]
pub mod run;
