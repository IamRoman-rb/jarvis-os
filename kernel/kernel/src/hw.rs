//! La lista de dispositivos que encontró el kernel, con su driver (K13): la muestra
//! Configuración → Hardware. En una PC real (sin puerto serie) es lo primero que hay que mirar
//! si algo no anda.

use alloc::string::String;
use alloc::vec::Vec;

use crate::irqlock::IrqMutex;

static DEVICES: IrqMutex<Vec<(&'static str, String)>> = IrqMutex::new(Vec::new());

/// Anota un dispositivo. `kind`: "Disco", "Red", "USB", "Sonido", "Interrupciones",
/// "Firmware" o "Procesador" (Configuración los traduce).
pub fn note(kind: &'static str, detail: String) {
    DEVICES.with(|d| d.push((kind, detail)));
}

pub fn list() -> Vec<(String, String)> {
    DEVICES.with(|d| {
        d.iter()
            .map(|(k, v)| (String::from(*k), v.clone()))
            .collect()
    })
}
