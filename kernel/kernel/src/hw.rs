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

/// Todo el hardware PCI (se lee una vez al arrancar) y el USB (lo anota xhci.rs al conectarse),
/// para el gestor de controladores (ADR 0023).
static PCI_INVENTORY: IrqMutex<Vec<jarvis_desktop::drivers::HwDevice>> = IrqMutex::new(Vec::new());
static USB_INVENTORY: IrqMutex<Vec<(u8, jarvis_desktop::drivers::HwDevice)>> =
    IrqMutex::new(Vec::new());

/// Recorre el bus PCI y guarda lo que hay (fabricante, modelo y clase de cada función).
pub fn scan_pci() {
    use jarvis_desktop::drivers::{Bus, HwDevice};
    let list: Vec<HwDevice> = crate::pci::devices()
        .into_iter()
        .map(|d| {
            let (class, subclass, prog_if) = d.class();
            HwDevice {
                bus: Bus::Pci,
                vendor: d.vendor(),
                device: d.device_id(),
                class,
                subclass,
                prog_if,
                location: alloc::format!("{:02x}:{:02x}.{}", d.bus, d.slot, d.function),
            }
        })
        .collect();
    PCI_INVENTORY.with(|p| *p = list);
}

/// Un dispositivo USB nuevo (por su slot de xHCI).
pub fn usb_add(slot: u8, vendor: u16, product: u16, class: u8, subclass: u8, port: u8) {
    use jarvis_desktop::drivers::{Bus, HwDevice};
    let dev = HwDevice {
        bus: Bus::Usb,
        vendor,
        device: product,
        class,
        subclass,
        prog_if: 0,
        location: alloc::format!("puerto {port}"),
    };
    USB_INVENTORY.with(|u| {
        u.retain(|(s, _)| *s != slot);
        u.push((slot, dev));
    });
}

pub fn usb_remove(slot: u8) {
    USB_INVENTORY.with(|u| u.retain(|(s, _)| *s != slot));
}

pub fn inventory() -> Vec<jarvis_desktop::drivers::HwDevice> {
    let mut v = PCI_INVENTORY.with(|p| p.clone());
    v.extend(USB_INVENTORY.with(|u| u.iter().map(|(_, d)| d.clone()).collect::<Vec<_>>()));
    v
}

pub fn list() -> Vec<(String, String)> {
    DEVICES.with(|d| {
        d.iter()
            .map(|(k, v)| (String::from(*k), v.clone()))
            .collect()
    })
}
