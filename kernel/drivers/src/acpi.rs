//! Las tablas fijas de ACPI: cómo el firmware describe la máquina.
//!
//! Al arrancar, el firmware deja en memoria un árbol de tablas. La raíz es el **RSDP** (el
//! bootloader nos da su dirección); apunta a la **XSDT** (o la RSDT, en firmwares viejos), que es
//! una lista de punteros a las demás. Cada tabla empieza con la misma cabecera de 36 bytes (firma
//! de 4 letras, largo, suma de control…). Las que usa el kernel:
//!
//! - **APIC** (la MADT): los controladores de interrupciones. Dónde está el APIC local de cada
//!   CPU, dónde los IOAPIC, y los *overrides*: la IRQ 0 del timer, en casi todas las PC, llega
//!   por la entrada 2 del IOAPIC.
//! - **FACP** (la FADT): los registros de energía (apagar, dormir, el timer PM de 3,58 MHz), la
//!   dirección de la DSDT y si hay controlador de teclado 8042.
//! - **MCFG**: dónde está el espacio de configuración PCI Express en memoria (ECAM).
//! - **HPET**: el temporizador de alta precisión.
//!
//! La DSDT y las SSDT no son datos sino **código** (AML): esas las interpreta la crate `acpi`.
//! Referencia: especificación ACPI 6.5, §5.2 (tablas) y §5.2.12 (MADT).

use alloc::vec::Vec;

use crate::le;

/// Lectura de la memoria física (el kernel lo implementa con su mapeo de toda la RAM).
pub trait PhysRead {
    fn read(&self, phys: u64, buf: &mut [u8]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// No dice "RSD PTR " o la suma de control no da cero.
    BadRsdp,
    /// La XSDT/RSDT no tiene su firma o su suma no da.
    BadRoot,
}

/// Todos los bytes de una tabla suman 0 (mod 256): así el firmware marca que está entera.
pub fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b)) == 0
}

/// El RSDP ("Root System Description Pointer").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsdp {
    /// 0 = ACPI 1.0 (solo RSDT, punteros de 32 bits); 2 o más = ACPI 2.0+ (XSDT).
    pub revision: u8,
    pub rsdt: u32,
    pub xsdt: Option<u64>,
}

impl Rsdp {
    /// `bytes`: los 36 bytes del RSDP (20 alcanzan en ACPI 1.0).
    pub fn parse(bytes: &[u8]) -> Result<Rsdp, Error> {
        if bytes.len() < 20 || &bytes[..8] != b"RSD PTR " || !checksum_ok(&bytes[..20]) {
            return Err(Error::BadRsdp);
        }
        let revision = bytes[15];
        let rsdt = le(bytes, 16, 4) as u32;
        let mut xsdt = None;
        if revision >= 2 && bytes.len() >= 36 {
            let len = (le(bytes, 20, 4) as usize).clamp(36, bytes.len());
            if !checksum_ok(&bytes[..len]) {
                return Err(Error::BadRsdp);
            }
            xsdt = Some(le(bytes, 24, 8)).filter(|&x| x != 0);
        }
        Ok(Rsdp {
            revision,
            rsdt,
            xsdt,
        })
    }
}

/// Una tabla encontrada: su firma, dónde está y cuánto mide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub signature: [u8; 4],
    pub phys: u64,
    pub len: u32,
    /// ¿Su suma de control da? Las que no, se listan igual (para el diagnóstico) pero no se usan.
    pub valid: bool,
}

/// El índice de las tablas del firmware.
#[derive(Debug, Clone, Default)]
pub struct Tables {
    pub revision: u8,
    pub entries: Vec<Entry>,
}

/// Tope de lo que se lee de una tabla: una DSDT real ronda los 60 KiB; más de 4 MiB es basura.
const MAX_TABLE: u32 = 4 << 20;

fn header(mem: &impl PhysRead, phys: u64) -> ([u8; 4], u32) {
    let mut h = [0u8; 8];
    mem.read(phys, &mut h);
    ([h[0], h[1], h[2], h[3]], le(&h, 4, 4) as u32)
}

/// Lee una tabla entera (con su cabecera).
pub fn read_table(mem: &impl PhysRead, phys: u64) -> Vec<u8> {
    let (_, len) = header(mem, phys);
    let len = len.clamp(36, MAX_TABLE) as usize;
    let mut buf = alloc::vec![0u8; len];
    mem.read(phys, &mut buf);
    buf
}

impl Tables {
    /// Recorre el RSDP y la XSDT (o la RSDT) y anota cada tabla.
    pub fn read(mem: &impl PhysRead, rsdp_phys: u64) -> Result<Tables, Error> {
        let mut raw = [0u8; 36];
        mem.read(rsdp_phys, &mut raw);
        let rsdp = Rsdp::parse(&raw)?;
        // La XSDT tiene punteros de 8 bytes; la RSDT, de 4.
        let (root, ptr, sig) = match rsdp.xsdt {
            Some(x) => (x, 8, b"XSDT"),
            None => (rsdp.rsdt as u64, 4, b"RSDT"),
        };
        let root_bytes = read_table(mem, root);
        if &root_bytes[..4] != sig || !checksum_ok(&root_bytes) {
            return Err(Error::BadRoot);
        }
        let mut entries = Vec::new();
        for at in (36..root_bytes.len()).step_by(ptr) {
            let phys = le(&root_bytes, at, ptr);
            if phys == 0 {
                continue;
            }
            let bytes = read_table(mem, phys);
            let signature = [bytes[0], bytes[1], bytes[2], bytes[3]];
            entries.push(Entry {
                signature,
                phys,
                len: bytes.len() as u32,
                valid: checksum_ok(&bytes),
            });
            // La DSDT no está en la lista: la apunta la FADT.
            if &signature == b"FACP" {
                let fadt = Fadt::parse(&bytes);
                if fadt.dsdt != 0 {
                    let dsdt = read_table(mem, fadt.dsdt);
                    entries.push(Entry {
                        signature: *b"DSDT",
                        phys: fadt.dsdt,
                        len: dsdt.len() as u32,
                        valid: checksum_ok(&dsdt) && &dsdt[..4] == b"DSDT",
                    });
                }
            }
        }
        Ok(Tables {
            revision: rsdp.revision,
            entries,
        })
    }

    /// La primera tabla válida con esa firma.
    pub fn find(&self, signature: &[u8; 4]) -> Option<Entry> {
        self.entries
            .iter()
            .find(|e| e.valid && &e.signature == signature)
            .copied()
    }
}

// --- MADT ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpu {
    pub uid: u32,
    pub apic_id: u32,
    /// Habilitada (o que se puede encender): las otras no existen de verdad.
    pub usable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoApic {
    pub id: u8,
    pub addr: u32,
    /// La primera "interrupción global del sistema" (GSI) que atiende.
    pub gsi_base: u32,
}

/// Una IRQ ISA que no llega por la entrada del IOAPIC con su mismo número, o no con la
/// polaridad y el disparo por defecto de ISA (flanco, activa en alto).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Override {
    pub irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

/// Cómo llega una interrupción a la entrada del IOAPIC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub gsi: u32,
    pub active_low: bool,
    pub level: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Madt {
    pub lapic_addr: u64,
    /// Además hay dos 8259 (el PIC de siempre), que hay que enmascarar antes de usar el APIC.
    pub has_8259: bool,
    pub cpus: Vec<Cpu>,
    pub ioapics: Vec<IoApic>,
    pub overrides: Vec<Override>,
}

impl Madt {
    pub fn parse(t: &[u8]) -> Madt {
        let mut m = Madt {
            lapic_addr: le(t, 36, 4),
            has_8259: le(t, 40, 4) & 1 != 0,
            ..Madt::default()
        };
        let mut at = 44;
        while at + 2 <= t.len() {
            let (kind, len) = (t[at], t[at + 1] as usize);
            if len < 2 || at + len > t.len() {
                break;
            }
            let e = &t[at..at + len];
            match kind {
                0 if len >= 8 => m.cpus.push(Cpu {
                    uid: e[2] as u32,
                    apic_id: e[3] as u32,
                    usable: le(e, 4, 4) & 0b11 != 0,
                }),
                1 if len >= 12 => m.ioapics.push(IoApic {
                    id: e[2],
                    addr: le(e, 4, 4) as u32,
                    gsi_base: le(e, 8, 4) as u32,
                }),
                2 if len >= 10 => m.overrides.push(Override {
                    irq: e[3],
                    gsi: le(e, 4, 4) as u32,
                    flags: le(e, 8, 2) as u16,
                }),
                5 if len >= 12 => m.lapic_addr = le(e, 4, 8),
                9 if len >= 16 => m.cpus.push(Cpu {
                    uid: le(e, 12, 4) as u32,
                    apic_id: le(e, 4, 4) as u32,
                    usable: le(e, 8, 4) & 0b11 != 0,
                }),
                _ => {}
            }
            at += len;
        }
        m
    }

    /// Por dónde llega la IRQ ISA `irq` (0 = timer, 1 = teclado, 12 = mouse…).
    pub fn isa_route(&self, irq: u8) -> Route {
        let Some(o) = self.overrides.iter().find(|o| o.irq == irq) else {
            return Route {
                gsi: irq as u32,
                active_low: false,
                level: false,
            };
        };
        // Bits 0–1: polaridad (00 = la del bus, que en ISA es en alto; 11 = en bajo).
        // Bits 2–3: disparo (00 = el del bus, que en ISA es por flanco; 11 = por nivel).
        Route {
            gsi: o.gsi,
            active_low: o.flags & 0b11 == 0b11,
            level: (o.flags >> 2) & 0b11 == 0b11,
        }
    }

    /// El IOAPIC que atiende esa GSI y su número de entrada adentro.
    pub fn ioapic_for(
        &self,
        gsi: u32,
        entries_of: impl Fn(&IoApic) -> u32,
    ) -> Option<(IoApic, u8)> {
        self.ioapics.iter().find_map(|io| {
            let n = entries_of(io);
            (gsi >= io.gsi_base && gsi < io.gsi_base + n).then(|| (*io, (gsi - io.gsi_base) as u8))
        })
    }
}

// --- FADT ------------------------------------------------------------------------------------

/// Un registro de la FADT: en puertos de E/S (lo común en x86) o en memoria.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    Io(u16),
    Memory(u64),
}

/// La "Generic Address Structure" de ACPI (12 bytes): espacio, ancho, desplazamiento, tamaño de
/// acceso y dirección. Solo se aceptan memoria (0) y puertos (1).
fn gas(t: &[u8], at: usize) -> Option<Register> {
    if at + 12 > t.len() {
        return None;
    }
    let addr = le(t, at + 4, 8);
    if addr == 0 {
        return None;
    }
    match t[at] {
        0 => Some(Register::Memory(addr)),
        1 if addr <= 0xFFFF => Some(Register::Io(addr as u16)),
        _ => None,
    }
}

/// Primero el registro de 64 bits (ACPI 2.0+); si no está, el puerto de 32 bits de ACPI 1.0.
fn reg(t: &[u8], x_at: usize, legacy_at: usize) -> Option<Register> {
    gas(t, x_at).or_else(|| {
        let port = le(t, legacy_at, 4);
        (port != 0 && port <= 0xFFFF).then_some(Register::Io(port as u16))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fadt {
    pub facs: u64,
    pub dsdt: u64,
    /// La interrupción del sistema ACPI (botón de encendido, eventos térmicos…).
    pub sci_irq: u16,
    /// Puerto donde se escribe `acpi_enable` para pasar de modo "legacy" a ACPI.
    pub smi_cmd: u32,
    pub acpi_enable: u8,
    pub pm1a_event: Option<Register>,
    pub pm1b_event: Option<Register>,
    pub pm1_event_len: u8,
    pub pm1a_control: Option<Register>,
    pub pm1b_control: Option<Register>,
    pub pm_timer: Option<Register>,
    /// El timer PM cuenta con 32 bits (si no, 24).
    pub pm_timer_32: bool,
    pub century: u8,
    /// `IAPC_BOOT_ARCH` bit 1: hay controlador 8042 (teclado y mouse PS/2). Desde ACPI 2.0; en
    /// tablas viejas se asume que sí.
    pub has_8042: bool,
    /// Sin hardware ACPI fijo (tabletas): no hay PM1 ni timer PM.
    pub hardware_reduced: bool,
    pub reset: Option<Register>,
    pub reset_value: u8,
}

impl Fadt {
    pub fn parse(t: &[u8]) -> Fadt {
        let revision = t.get(8).copied().unwrap_or(0);
        let flags = le(t, 112, 4) as u32;
        let boot_arch = le(t, 109, 2) as u16;
        Fadt {
            facs: Some(le(t, 132, 8))
                .filter(|&x| x != 0)
                .unwrap_or(le(t, 36, 4)),
            dsdt: Some(le(t, 140, 8))
                .filter(|&x| x != 0)
                .unwrap_or(le(t, 40, 4)),
            sci_irq: le(t, 46, 2) as u16,
            smi_cmd: le(t, 48, 4) as u32,
            acpi_enable: t.get(52).copied().unwrap_or(0),
            pm1a_event: reg(t, 148, 56),
            pm1b_event: reg(t, 160, 60),
            pm1_event_len: t.get(88).copied().unwrap_or(0),
            pm1a_control: reg(t, 172, 64),
            pm1b_control: reg(t, 184, 68),
            pm_timer: reg(t, 208, 76),
            pm_timer_32: flags & (1 << 8) != 0,
            century: t.get(108).copied().unwrap_or(0),
            has_8042: revision < 2 || boot_arch & 0b10 != 0,
            hardware_reduced: flags & (1 << 20) != 0,
            reset: (flags & (1 << 10) != 0).then(|| gas(t, 116)).flatten(),
            reset_value: t.get(128).copied().unwrap_or(0),
        }
    }
}

/// Frecuencia del timer PM de ACPI (la misma en todas las PC desde 1996).
pub const PM_TIMER_HZ: u64 = 3_579_545;

/// Cuentas del timer PM entre dos lecturas, contando la vuelta (el contador es de 24 o 32 bits).
pub fn pm_timer_delta(start: u32, end: u32, bits32: bool) -> u32 {
    let mask = if bits32 { u32::MAX } else { 0x00FF_FFFF };
    end.wrapping_sub(start) & mask
}

// --- MCFG y HPET -----------------------------------------------------------------------------

/// Una zona de configuración PCI Express en memoria: 4 KiB por función, desde `base`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ecam {
    pub base: u64,
    pub segment: u16,
    pub bus_start: u8,
    pub bus_end: u8,
}

impl Ecam {
    /// Dirección física del registro `offset` de bus/dispositivo/función.
    pub fn address(&self, bus: u8, device: u8, function: u8, offset: u16) -> Option<u64> {
        if bus < self.bus_start || bus > self.bus_end || device >= 32 || function >= 8 {
            return None;
        }
        let rel = ((bus - self.bus_start) as u64) << 20
            | (device as u64) << 15
            | (function as u64) << 12
            | (offset & 0xFFF) as u64;
        Some(self.base + rel)
    }
}

pub fn parse_mcfg(t: &[u8]) -> Vec<Ecam> {
    (44..t.len())
        .step_by(16)
        .filter(|&at| at + 16 <= t.len())
        .map(|at| Ecam {
            base: le(t, at, 8),
            segment: le(t, at + 8, 2) as u16,
            bus_start: t[at + 10],
            bus_end: t[at + 11],
        })
        .filter(|e| e.base != 0 && e.bus_end >= e.bus_start)
        .collect()
}

/// Dirección de los registros del HPET.
pub fn parse_hpet(t: &[u8]) -> Option<u64> {
    match gas(t, 40)? {
        Register::Memory(a) => Some(a),
        Register::Io(_) => None,
    }
}
