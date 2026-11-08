//! El gestor de controladores (ADR 0023): qué hardware hay, cuál tiene driver en JARVIS-OS,
//! cuál necesita firmware y cuál no tiene nada (para pedirlo).
//!
//! Hace lo que Driver Booster o Snappy Driver Installer en Windows, a la medida de JARVIS-OS:
//! esos programas instalan **drivers de Windows**, que acá no sirven (es otro kernel). Lo que sí
//! sirve de afuera es el **firmware** (el programa que corre adentro de la placa, el mismo para
//! cualquier sistema operativo, de `linux-firmware`) y saber qué falta soportar.
//!
//! - El kernel arma el inventario (PCI al arrancar; USB a medida que se conecta) y lo pasa en
//!   `SystemStats.hardware`.
//! - El **catálogo** ([`CATALOG`], `kernel/paquetes/controladores/catalogo.txt`) dice qué driver
//!   atiende cada dispositivo (por fabricante:modelo o por clase) y qué firmware hace falta, de
//!   dónde bajarlo y su SHA-256.
//! - Los nombres de fabricantes y de clases son un pedazo de la base `pci.ids` (pciutils).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// El catálogo que viene con el sistema.
pub const BUILTIN_CATALOG: &str = include_str!("../../paquetes/controladores/catalogo.txt");
/// El bajado con `controladores actualizar` (si está, gana).
pub const CATALOG: &str = "/Sistema/controladores/catalogo.txt";
pub const CATALOG_URL: &str = "http://paquetes.jarvis/controladores/catalogo.txt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Pci,
    Usb,
}

/// Un dispositivo del inventario.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HwDevice {
    pub bus: Bus,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    /// `00:1f.2` (PCI) o `puerto 3` (USB).
    pub location: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Match {
    Id(Bus, u16, u16),
    Class(Bus, u8, Option<u8>, Option<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    matcher: Match,
    pub driver: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Firmware {
    pub driver: String,
    pub path: String,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub firmware: Vec<Firmware>,
}

fn hex2(s: &str) -> Option<u8> {
    u8::from_str_radix(s, 16).ok()
}

pub fn parse_catalog(text: &str) -> Catalog {
    let mut c = Catalog::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('|').map(str::trim).collect();
        let head: Vec<&str> = fields[0].split_whitespace().collect();
        match head.as_slice() {
            ["firmware", driver] if fields.len() >= 4 => c.firmware.push(Firmware {
                driver: (*driver).into(),
                path: fields[1].into(),
                url: fields[2].into(),
                sha256: fields[3].to_lowercase(),
            }),
            [bus, "clase", spec] if fields.len() >= 3 => {
                let bus = if *bus == "usb" { Bus::Usb } else { Bus::Pci };
                let parts: Vec<&str> = spec.split(':').collect();
                let Some(class) = parts.first().and_then(|p| hex2(p)) else {
                    continue;
                };
                let sub = parts.get(1).and_then(|p| hex2(p));
                let prog = parts.get(2).and_then(|p| hex2(p));
                c.entries.push(Entry {
                    matcher: Match::Class(bus, class, sub, prog),
                    driver: fields[1].into(),
                    name: fields[2].into(),
                });
            }
            [bus, id] if fields.len() >= 3 => {
                let bus = if *bus == "usb" { Bus::Usb } else { Bus::Pci };
                let Some((v, d)) = id.split_once(':') else {
                    continue;
                };
                let (Ok(v), Ok(d)) = (u16::from_str_radix(v, 16), u16::from_str_radix(d, 16))
                else {
                    continue;
                };
                c.entries.push(Entry {
                    matcher: Match::Id(bus, v, d),
                    driver: fields[1].into(),
                    name: fields[2].into(),
                });
            }
            _ => {}
        }
    }
    c
}

impl Catalog {
    /// La entrada para un dispositivo: primero por modelo exacto, después por clase.
    pub fn find(&self, d: &HwDevice) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.matcher == Match::Id(d.bus, d.vendor, d.device))
            .or_else(|| {
                self.entries.iter().find(|e| match e.matcher {
                    Match::Class(bus, c, s, p) => {
                        bus == d.bus
                            && c == d.class
                            && s.is_none_or(|s| s == d.subclass)
                            && p.is_none_or(|p| p == d.prog_if)
                    }
                    Match::Id(..) => false,
                })
            })
    }

    pub fn firmware_for(&self, driver: &str) -> Vec<&Firmware> {
        self.firmware
            .iter()
            .filter(|f| f.driver == driver)
            .collect()
    }
}

/// Un fabricante (de `pci.ids`: los más comunes en PC y máquinas virtuales).
pub fn vendor_name(v: u16) -> &'static str {
    match v {
        0x8086 => "Intel",
        0x1022 => "AMD",
        0x1002 => "AMD/ATI",
        0x10de => "NVIDIA",
        0x10ec => "Realtek",
        0x14e4 => "Broadcom",
        0x168c => "Qualcomm Atheros",
        0x17cb => "Qualcomm",
        0x14c3 => "MediaTek",
        0x144d => "Samsung",
        0x15b7 => "SanDisk/WD",
        0x1987 => "Phison",
        0x1c5c => "SK hynix",
        0x1b4b => "Marvell",
        0x1106 => "VIA",
        0x1af4 => "Red Hat (virtio)",
        0x1b36 => "Red Hat (QEMU)",
        0x1234 => "QEMU",
        0x80ee => "Oracle (VirtualBox)",
        0x15ad => "VMware",
        0x1414 => "Microsoft (Hyper-V)",
        0x046d => "Logitech",
        0x045e => "Microsoft",
        0x05ac => "Apple",
        0x0bda => "Realtek",
        0x0781 => "SanDisk",
        0x0951 => "Kingston",
        0x058f => "Alcor",
        0x0627 => "QEMU",
        0x5843 => "Web Camera",
        _ => "",
    }
}

/// Qué es, por su clase (PCI o USB).
pub fn class_name(bus: Bus, class: u8, sub: u8) -> &'static str {
    match bus {
        Bus::Usb => match class {
            0x01 => "Audio USB",
            0x02 | 0x0a => "Comunicaciones USB",
            0x03 => "Teclado/mouse (HID)",
            0x06 => "Imágenes (cámara de fotos)",
            0x07 => "Impresora",
            0x08 => "Almacenamiento USB",
            0x09 => "Hub USB",
            0x0b => "Lector de tarjetas inteligentes",
            0x0e => "Cámara (UVC)",
            0xe0 => "Bluetooth/inalámbrico",
            0xef => "Dispositivo compuesto",
            0xff => "Del fabricante",
            _ => "USB",
        },
        Bus::Pci => match (class, sub) {
            (0x01, 0x01) => "Controladora IDE",
            (0x01, 0x06) => "Controladora SATA",
            (0x01, 0x08) => "Disco NVMe",
            (0x01, _) => "Almacenamiento",
            (0x02, 0x00) => "Red Ethernet",
            (0x02, 0x80) => "Red inalámbrica",
            (0x02, _) => "Red",
            (0x03, _) => "Video",
            (0x04, 0x03) => "Sonido (HD Audio)",
            (0x04, _) => "Multimedia",
            (0x05, _) => "Memoria",
            (0x06, 0x00) => "Puente del procesador",
            (0x06, 0x01) => "Puente ISA",
            (0x06, 0x04) => "Puente PCI",
            (0x06, _) => "Puente",
            (0x07, _) => "Comunicaciones",
            (0x08, _) => "Periférico del sistema",
            (0x0c, 0x03) => "Controladora USB",
            (0x0c, 0x05) => "SMBus",
            (0x0c, _) => "Bus serie",
            (0x0d, _) => "Inalámbrico",
            (0x11, _) => "Adquisición de señales",
            (0x12, _) => "Acelerador",
            (0x13, _) => "Instrumentación",
            _ => "Otro",
        },
    }
}

/// El estado de un dispositivo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Tiene driver en JARVIS-OS (y firmware, si lo necesita).
    Ok(String),
    /// No necesita driver (puentes, periféricos del sistema).
    NotNeeded,
    /// Tiene driver pero le falta el firmware (se puede instalar).
    NeedsFirmware(String, Vec<Firmware>),
    /// No hay driver: hay que pedirlo.
    Missing,
}

/// Una fila del informe: el dispositivo, su nombre y su estado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub dev: HwDevice,
    pub name: String,
    pub state: State,
}

/// Revisa el inventario. `has_file`: si un firmware ya está en el disco.
pub fn review(
    hw: &[HwDevice],
    catalog: &Catalog,
    has_file: &mut dyn FnMut(&str) -> bool,
) -> Vec<Row> {
    hw.iter()
        .map(|d| {
            let (name, state) = match catalog.find(d) {
                Some(e) if e.driver.starts_with('(') => (e.name.clone(), State::NotNeeded),
                Some(e) => {
                    let missing: Vec<Firmware> = catalog
                        .firmware_for(&e.driver)
                        .into_iter()
                        .filter(|f| !has_file(&f.path))
                        .cloned()
                        .collect();
                    let state = if missing.is_empty() {
                        State::Ok(e.driver.clone())
                    } else {
                        State::NeedsFirmware(e.driver.clone(), missing)
                    };
                    (e.name.clone(), state)
                }
                None => {
                    let v = vendor_name(d.vendor);
                    let c = class_name(d.bus, d.class, d.subclass);
                    (
                        if v.is_empty() {
                            c.to_string()
                        } else {
                            format!("{c} de {v}")
                        },
                        State::Missing,
                    )
                }
            };
            Row {
                dev: d.clone(),
                name,
                state,
            }
        })
        .collect()
}

pub fn id_of(d: &HwDevice) -> String {
    format!(
        "{} {:04x}:{:04x}",
        if d.bus == Bus::Pci { "pci" } else { "usb" },
        d.vendor,
        d.device
    )
}

/// La tabla de `controladores` (y el texto del reporte).
pub fn table(rows: &[Row]) -> String {
    let mut s = format!("{:<20}{:<34}{}\n", "Dispositivo", "Qué es", "Estado");
    for r in rows {
        let state = match &r.state {
            State::Ok(drv) => format!("ok (driver {drv})"),
            State::NotNeeded => "no necesita driver".into(),
            State::NeedsFirmware(drv, fw) => format!(
                "falta el firmware de {drv} ({}): sudo controladores instalar",
                fw.iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            State::Missing => "SIN DRIVER".into(),
        };
        let name: String = r.name.chars().take(33).collect();
        s.push_str(&format!("{:<20}{name:<34}{state}\n", id_of(&r.dev)));
    }
    s
}

/// El reporte para pedir drivers: lo que no tiene, con todo lo que hace falta para escribirlo.
pub fn report(rows: &[Row], hostname: &str) -> String {
    let mut s = format!(
        "Reporte de hardware de JARVIS-OS ({hostname})\n\
         Dispositivos sin driver: mandá este archivo para pedirlos.\n\n"
    );
    let missing: Vec<&Row> = rows.iter().filter(|r| r.state == State::Missing).collect();
    if missing.is_empty() {
        s.push_str("Todo el hardware tiene driver.\n");
    }
    for r in missing {
        s.push_str(&format!(
            "- {} [{}] clase {:02x}:{:02x}:{:02x} en {}\n",
            r.name,
            id_of(&r.dev),
            r.dev.class,
            r.dev.subclass,
            r.dev.prog_if,
            r.dev.location
        ));
    }
    s.push_str("\nInventario completo:\n");
    s.push_str(&table(rows));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pci(v: u16, d: u16, c: u8, s: u8, p: u8) -> HwDevice {
        HwDevice {
            bus: Bus::Pci,
            vendor: v,
            device: d,
            class: c,
            subclass: s,
            prog_if: p,
            location: "00:03.0".into(),
        }
    }

    #[test]
    fn catalogo_por_modelo_y_por_clase() {
        let c = parse_catalog(BUILTIN_CATALOG);
        assert!(c.entries.len() > 20);
        let wifi = pci(0x10ec, 0xc821, 0x02, 0x80, 0);
        let ahci = pci(0x8086, 0x2922, 0x01, 0x06, 0x01);
        let bridge = pci(0x8086, 0x1237, 0x06, 0x00, 0);
        let nvidia = pci(0x10de, 0x2504, 0x03, 0x00, 0);
        let intel_wifi = pci(0x8086, 0x2723, 0x02, 0x80, 0);
        let hw = alloc::vec![wifi, ahci, bridge, nvidia, intel_wifi];
        let mut no_files = |_: &str| false;
        let rows = review(&hw, &c, &mut no_files);
        assert!(
            matches!(&rows[0].state, State::NeedsFirmware(d, fw) if d == "rtw88" && fw[0].path.ends_with("rtw8821c_fw.bin"))
        );
        assert_eq!(rows[1].state, State::Ok("ahci".into()));
        assert_eq!(rows[2].state, State::NotNeeded);
        assert_eq!(rows[3].state, State::Ok("framebuffer".into()));
        assert_eq!(rows[4].state, State::Missing);
        assert_eq!(rows[4].name, "Red inalámbrica de Intel");
        // Con el firmware en el disco, la Wi-Fi queda bien.
        let mut all_files = |_: &str| true;
        let rows = review(&hw, &c, &mut all_files);
        assert_eq!(rows[0].state, State::Ok("rtw88".into()));
        let rep = report(&review(&hw, &c, &mut no_files), "jarvis");
        assert!(
            rep.contains("Red inalámbrica de Intel [pci 8086:2723]"),
            "{rep}"
        );
        assert!(!rep.contains("Puente del procesador ["));
        let fw = &c.firmware[0];
        assert_eq!(fw.sha256.len(), 64);
        assert!(fw.url.starts_with("https://git.kernel.org/"));
    }
}
