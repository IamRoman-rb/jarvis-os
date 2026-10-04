//! Wi-Fi de JARVIS-OS (K14, ADR 0012): la mitad que no toca la placa.
//!
//! Una placa Wi-Fi moderna (la RTL8821CE de la PC de Roman) hace la radio y poco más: las tramas
//! 802.11, la asociación con el punto de acceso y la seguridad (WPA2) las arma el sistema. Esta
//! crate es esa parte, `no_std` y probada en el host contra otra implementación (Python con
//! `cryptography`, en tests/datos/):
//!
//! - [`frame`]: tramas 802.11 (encabezado, datos ↔ Ethernet, beacons, sondeo, autenticación y
//!   asociación) y sus elementos de información.
//! - [`rsn`]: el elemento RSN, que dice qué seguridad ofrece una red.
//! - [`crypto`]: de la contraseña a las claves (PBKDF2 → PMK, PRF → PTK), el MIC de EAPOL y el
//!   desenvuelto de la clave de grupo (AES Key Wrap).
//! - [`eapol`]: el saludo de 4 vías (y el de grupo) del lado de la estación.
//! - [`ccmp`]: el cifrado de las tramas de datos (AES-CCM).
//! - [`station`]: la estación entera: buscar redes, conectarse, reintentar y pasar los datos.
//!
//! El driver de la placa está en `jarvis-drivers` (rtw88) y la unión con la pila de red, en el
//! kernel (kernel/src/wifi.rs).

#![no_std]

extern crate alloc;

pub mod ccmp;
pub mod crypto;
pub mod eapol;
pub mod frame;
pub mod rsn;
pub mod station;

/// Una dirección MAC.
pub type Mac = [u8; 6];

/// La dirección de difusión.
pub const BROADCAST: Mac = [0xff; 6];
