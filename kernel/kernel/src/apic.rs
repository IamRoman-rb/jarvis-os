//! APIC local e IOAPIC: las interrupciones de una PC moderna (K13).
//!
//! Hasta K12 todo pasaba por el PIC 8259, que QEMU emula y que las PC siguen teniendo por
//! compatibilidad. Pero en una PC con UEFI el PIC es una sombra: las líneas INTx de PCI ya no
//! tienen un número confiable (el registro 0x3C lo escribe el BIOS viejo, no UEFI), y los
//! dispositivos PCI Express avisan por **MSI**, que solo entiende el APIC.
//!
//! - El **APIC local** (uno por CPU, en 0xFEE00000) recibe las interrupciones y se le avisa el
//!   fin de cada una escribiendo en su registro EOI.
//! - Cada **IOAPIC** tiene una tabla de redirección: una entrada por "interrupción global del
//!   sistema" (GSI) que dice a qué vector y a qué CPU va. Ahí llegan el timer (IRQ0, que casi
//!   siempre entra por la GSI 2), el teclado, el mouse y las líneas INTx de PCI.
//!
//! Dónde está cada cosa lo dice la MADT (ver jarvis-drivers/acpi.rs). Si no hay MADT, el
//! kernel sigue con el PIC, como antes.
//! Referencias: Intel SDM vol. 3A cap. 11, <https://wiki.osdev.org/APIC> y
//! <https://wiki.osdev.org/IOAPIC>.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use jarvis_drivers::acpi::Madt;
use jarvis_drivers::apic::redirection;
use spin::{Mutex, Once};
use x86_64::registers::model_specific::Msr;

use crate::{interrupts, paging, serial_println};

const IA32_APIC_BASE: u32 = 0x1B;
/// Bit 11 del MSR: el APIC local está encendido.
const APIC_GLOBAL_ENABLE: u64 = 1 << 11;

// Registros del APIC local (desplazamientos desde su base).
const LAPIC_ID: usize = 0x20;
const LAPIC_TPR: usize = 0x80;
const LAPIC_EOI: usize = 0xB0;
const LAPIC_SVR: usize = 0xF0;

/// Vector de las interrupciones "espurias" del APIC (no llevan EOI).
pub const SPURIOUS_VECTOR: u8 = 0xFF;

static LAPIC: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Un IOAPIC mapeado.
struct IoApic {
    base: *mut u32,
    gsi_base: u32,
    entries: u32,
}

// SAFETY: `base` apunta a registros de un dispositivo, mapeados sin caché; el acceso pasa
// siempre por el lock de IOAPICS (la ventana IOREGSEL/IOWIN no se puede usar a medias).
unsafe impl Send for IoApic {}

impl IoApic {
    fn read(&self, reg: u32) -> u32 {
        // SAFETY: IOREGSEL (base) elige el registro e IOWIN (base + 0x10) lo lee; se llama con
        // el lock tomado.
        unsafe {
            core::ptr::write_volatile(self.base, reg);
            core::ptr::read_volatile(self.base.add(4))
        }
    }

    fn write(&self, reg: u32, value: u32) {
        // SAFETY: ídem `read`.
        unsafe {
            core::ptr::write_volatile(self.base, reg);
            core::ptr::write_volatile(self.base.add(4), value);
        }
    }

    fn set_entry(&self, n: u32, value: u64) {
        // La parte alta primero: así la entrada nunca queda desenmascarada con un destino viejo.
        self.write(0x10 + 2 * n + 1, (value >> 32) as u32);
        self.write(0x10 + 2 * n, value as u32);
    }

    fn entry(&self, n: u32) -> u64 {
        (self.read(0x10 + 2 * n + 1) as u64) << 32 | self.read(0x10 + 2 * n) as u64
    }
}

static IOAPICS: Mutex<Vec<IoApic>> = Mutex::new(Vec::new());
static MADT: Once<Madt> = Once::new();

/// ¿Las interrupciones pasan por el APIC (y no por el PIC)?
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn lapic(reg: usize) -> *mut u32 {
    (LAPIC.load(Ordering::Relaxed) as usize + reg) as *mut u32
}

/// El ID del APIC local de esta CPU (destino de los MSI).
pub fn id() -> u8 {
    if !active() {
        return 0;
    }
    // SAFETY: el APIC local está mapeado (`init`) y leer su ID no tiene efectos.
    (unsafe { core::ptr::read_volatile(lapic(LAPIC_ID)) } >> 24) as u8
}

/// Fin de interrupción en el APIC local.
pub fn eoi() {
    // SAFETY: escribir 0 en EOI marca terminada la interrupción en servicio; el APIC está
    // mapeado si `active()` (quien llama lo verificó).
    unsafe { core::ptr::write_volatile(lapic(LAPIC_EOI), 0) };
}

/// Pasa del PIC al APIC. Hay que llamarla con las interrupciones deshabilitadas. Devuelve
/// `false` (y se sigue con el PIC) si la MADT no sirve.
pub fn init(madt: Madt) -> bool {
    if madt.ioapics.is_empty() || madt.lapic_addr == 0 {
        return false;
    }
    let Some(lapic_base) = paging::map_mmio(madt.lapic_addr, 4096) else {
        return false;
    };
    let mut ioapics = Vec::new();
    for io in &madt.ioapics {
        let Some(base) = paging::map_mmio(io.addr as u64, 0x20) else {
            continue;
        };
        let mut ioapic = IoApic {
            base: base as *mut u32,
            gsi_base: io.gsi_base,
            entries: 0,
        };
        // Registro 1 (versión): bits 16–23 = cantidad de entradas − 1.
        ioapic.entries = ((ioapic.read(1) >> 16) & 0xFF) + 1;
        for n in 0..ioapic.entries {
            ioapic.set_entry(n, redirection(0, false, false, true, 0));
        }
        serial_println!(
            "IOAPIC {} en {:#x}: GSI {}–{}",
            io.id,
            io.addr,
            io.gsi_base,
            io.gsi_base + ioapic.entries - 1
        );
        ioapics.push(ioapic);
    }
    if ioapics.is_empty() {
        return false;
    }
    // El PIC, todo enmascarado: desde acá no le llega nada a la CPU por él.
    interrupts::mask_pic();

    // SAFETY: IA32_APIC_BASE existe en toda CPU con APIC (x86_64 lo tiene siempre); se prende
    // el bit de habilitación sin cambiar la dirección.
    unsafe {
        let mut msr = Msr::new(IA32_APIC_BASE);
        let v = msr.read();
        msr.write(v | APIC_GLOBAL_ENABLE);
    }
    LAPIC.store(lapic_base as u64, Ordering::Relaxed);
    // SAFETY: el APIC local se acaba de mapear. SVR: bit 8 = habilitado por software, más el
    // vector de las espurias. TPR en 0: acepta todas las prioridades.
    unsafe {
        core::ptr::write_volatile(lapic(LAPIC_SVR), 0x100 | SPURIOUS_VECTOR as u32);
        core::ptr::write_volatile(lapic(LAPIC_TPR), 0);
    }
    *IOAPICS.lock() = ioapics;
    let madt = MADT.call_once(|| madt);
    ACTIVE.store(true, Ordering::Relaxed);

    // Las IRQ ISA de siempre, a los mismos vectores que tenían con el PIC.
    let cpu = id();
    for irq in [0u8, 1, 12] {
        let r = madt.isa_route(irq);
        let vector = interrupts::PIC_1_OFFSET + irq;
        if !route_gsi(r.gsi, vector, r.active_low, r.level, cpu) {
            serial_println!("APIC: la IRQ {irq} (GSI {}) no tiene IOAPIC", r.gsi);
        }
    }
    serial_println!(
        "APIC_LISTO local {:#x} (CPU {}), {} IOAPIC, {} CPU en la MADT",
        madt.lapic_addr,
        cpu,
        madt.ioapics.len(),
        madt.cpus.iter().filter(|c| c.usable).count()
    );
    true
}

/// Manda la GSI `gsi` al vector `vector` de la CPU `dest`. `false` si ningún IOAPIC la tiene.
pub fn route_gsi(gsi: u32, vector: u8, active_low: bool, level: bool, dest: u8) -> bool {
    let ioapics = IOAPICS.lock();
    let Some(io) = ioapics
        .iter()
        .find(|io| gsi >= io.gsi_base && gsi < io.gsi_base + io.entries)
    else {
        return false;
    };
    io.set_entry(
        gsi - io.gsi_base,
        redirection(vector, active_low, level, false, dest),
    );
    true
}

/// Enmascara o habilita la entrada de una GSI.
pub fn set_gsi_masked(gsi: u32, masked: bool) {
    let ioapics = IOAPICS.lock();
    if let Some(io) = ioapics
        .iter()
        .find(|io| gsi >= io.gsi_base && gsi < io.gsi_base + io.entries)
    {
        let n = gsi - io.gsi_base;
        let e = io.entry(n);
        io.set_entry(n, (e & !(1 << 16)) | (masked as u64) << 16);
    }
}
