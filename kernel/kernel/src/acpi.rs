//! ACPI en el kernel (K13): las tablas del firmware y el intérprete de AML.
//!
//! Las tablas fijas (MADT, FADT, MCFG, HPET) las lee `jarvis_drivers::acpi`, propio. La DSDT y
//! las SSDT son **código**: un programa en AML (ACPI Machine Language) que el firmware trae para
//! describir la placa. Ahí está, por ejemplo, a qué entrada del IOAPIC va cada línea INTx de PCI
//! (`_PRT`), qué escribir para dormir o apagar (`\_S3`, `\_S5`) y cómo leer los sensores
//! térmicos (`_TZ`). Para eso se usa la crate `acpi` de rust-osdev (ADR 0011): este módulo le
//! presta el hardware (memoria física, puertos, espacio de configuración PCI, tiempo).
//! Referencia: especificación ACPI 6.5, cap. 5 (tablas y AML) y cap. 7 (energía).

use alloc::vec::Vec;
use core::ptr::NonNull;
use core::str::FromStr;
use core::sync::atomic::{AtomicU32, Ordering};

use acpi::aml::namespace::{AmlName, NamespaceLevelKind};
use acpi::aml::object::Object;
use acpi::aml::pci_routing::{PciRoutingTable, Pin};
use acpi::aml::resource::{InterruptPolarity, InterruptTrigger};
use acpi::aml::{AmlError, Interpreter};
use acpi::platform::AcpiPlatform;
use acpi::{AcpiTables, Handle, Handler, PciAddress, PhysicalMapping};
use jarvis_drivers::acpi::{
    Ecam, Fadt, Madt, PhysRead, Tables, parse_hpet, parse_mcfg, read_table,
};
use spin::Once;
use x86_64::instructions::port::Port;

use crate::{paging, pci, serial_println, time};

/// La memoria física, a través de `map_mmio` (lo que no esté en el mapeo de la RAM se mapea
/// sin caché).
#[derive(Clone, Copy)]
pub struct Phys;

impl PhysRead for Phys {
    fn read(&self, phys: u64, buf: &mut [u8]) {
        let Some(p) = paging::map_mmio(phys, buf.len().max(1)) else {
            buf.fill(0);
            return;
        };
        for (i, b) in buf.iter_mut().enumerate() {
            // SAFETY: `map_mmio` mapeó `buf.len()` bytes desde `phys`; son tablas del firmware.
            *b = unsafe { core::ptr::read_volatile(p.add(i)) };
        }
    }
}

/// Lee un registro de una región de memoria que declaró el AML (una `OperationRegion
/// SystemMemory`: registros de la placa). Si no se puede mapear, da 0.
fn mmio_read<T: Copy + Default>(address: usize) -> T {
    match paging::map_mmio(address as u64, core::mem::size_of::<T>()) {
        // SAFETY: `map_mmio` mapeó (sin caché) los bytes pedidos desde `address`.
        Some(p) => unsafe { core::ptr::read_volatile(p as *const T) },
        None => T::default(),
    }
}

/// Escribe un registro de una región que declaró el AML. Si no se puede mapear, no hace nada.
fn mmio_write<T: Copy>(address: usize, value: T) {
    if let Some(p) = paging::map_mmio(address as u64, core::mem::size_of::<T>()) {
        // SAFETY: ídem `mmio_read`.
        unsafe { core::ptr::write_volatile(p as *mut T, value) }
    }
}

/// Lo que el intérprete de AML necesita del kernel.
#[derive(Clone, Copy)]
pub struct KernelHandler;

static NEXT_MUTEX: AtomicU32 = AtomicU32::new(1);

fn pci_read32(a: PciAddress, offset: u16) -> u32 {
    let aligned = offset & !3;
    if aligned < 256 {
        return pci::read_config(a.bus(), a.device(), a.function(), aligned as u8);
    }
    match ecam_address(a, aligned) {
        // SAFETY: la dirección está en la zona ECAM que declaró la MCFG, mapeada sin caché.
        Some(p) => unsafe { core::ptr::read_volatile(p as *const u32) },
        None => u32::MAX,
    }
}

fn pci_write32(a: PciAddress, offset: u16, value: u32) {
    let aligned = offset & !3;
    if aligned < 256 {
        pci::write_config(a.bus(), a.device(), a.function(), aligned as u8, value);
    } else if let Some(p) = ecam_address(a, aligned) {
        // SAFETY: ídem `pci_read32`.
        unsafe { core::ptr::write_volatile(p as *mut u32, value) };
    }
}

fn ecam_address(a: PciAddress, offset: u16) -> Option<*mut u8> {
    let phys = ACPI
        .get()?
        .ecam
        .iter()
        .find(|e| e.segment == a.segment())
        .and_then(|e| e.address(a.bus(), a.device(), a.function(), offset))?;
    paging::map_mmio(phys, 4)
}

fn pci_read(a: PciAddress, offset: u16, bytes: u16) -> u32 {
    let word = pci_read32(a, offset);
    let shift = (offset & 3) * 8;
    let mask = if bytes == 4 {
        u32::MAX
    } else {
        (1 << (bytes * 8)) - 1
    };
    (word >> shift) & mask
}

fn pci_write(a: PciAddress, offset: u16, bytes: u16, value: u32) {
    if bytes == 4 {
        return pci_write32(a, offset, value);
    }
    let shift = (offset & 3) * 8;
    let mask = ((1u32 << (bytes * 8)) - 1) << shift;
    let old = pci_read32(a, offset);
    pci_write32(a, offset, (old & !mask) | ((value << shift) & mask));
}

impl Handler for KernelHandler {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        let virt = paging::map_mmio(physical_address as u64, size.max(1))
            .expect("ACPI: no se pudo mapear una tabla del firmware");
        PhysicalMapping {
            physical_start: physical_address,
            virtual_start: NonNull::new(virt as *mut T).expect("mapeo nulo"),
            region_length: size,
            mapped_length: size,
            handler: *self,
        }
    }

    // Los mapeos son parte del mapeo fijo de la memoria física: no se desarman.
    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {}

    fn read_u8(&self, address: usize) -> u8 {
        mmio_read(address)
    }
    fn read_u16(&self, address: usize) -> u16 {
        mmio_read(address)
    }
    fn read_u32(&self, address: usize) -> u32 {
        mmio_read(address)
    }
    fn read_u64(&self, address: usize) -> u64 {
        mmio_read(address)
    }
    fn write_u8(&self, address: usize, value: u8) {
        mmio_write(address, value)
    }
    fn write_u16(&self, address: usize, value: u16) {
        mmio_write(address, value)
    }
    fn write_u32(&self, address: usize, value: u32) {
        mmio_write(address, value)
    }
    fn write_u64(&self, address: usize, value: u64) {
        mmio_write(address, value)
    }

    fn read_io_u8(&self, port: u16) -> u8 {
        // SAFETY: un puerto que declaró el AML de la placa (su OperationRegion SystemIO).
        unsafe { Port::new(port).read() }
    }
    fn read_io_u16(&self, port: u16) -> u16 {
        // SAFETY: ídem `read_io_u8`.
        unsafe { Port::new(port).read() }
    }
    fn read_io_u32(&self, port: u16) -> u32 {
        // SAFETY: ídem `read_io_u8`.
        unsafe { Port::new(port).read() }
    }
    fn write_io_u8(&self, port: u16, value: u8) {
        // SAFETY: ídem `read_io_u8`.
        unsafe { Port::new(port).write(value) }
    }
    fn write_io_u16(&self, port: u16, value: u16) {
        // SAFETY: ídem `read_io_u8`.
        unsafe { Port::new(port).write(value) }
    }
    fn write_io_u32(&self, port: u16, value: u32) {
        // SAFETY: ídem `read_io_u8`.
        unsafe { Port::new(port).write(value) }
    }

    fn read_pci_u8(&self, address: PciAddress, offset: u16) -> u8 {
        pci_read(address, offset, 1) as u8
    }
    fn read_pci_u16(&self, address: PciAddress, offset: u16) -> u16 {
        pci_read(address, offset, 2) as u16
    }
    fn read_pci_u32(&self, address: PciAddress, offset: u16) -> u32 {
        pci_read(address, offset, 4)
    }
    fn write_pci_u8(&self, address: PciAddress, offset: u16, value: u8) {
        pci_write(address, offset, 1, value as u32)
    }
    fn write_pci_u16(&self, address: PciAddress, offset: u16, value: u16) {
        pci_write(address, offset, 2, value as u32)
    }
    fn write_pci_u32(&self, address: PciAddress, offset: u16, value: u32) {
        pci_write(address, offset, 4, value)
    }

    fn nanos_since_boot(&self) -> u64 {
        time::nanos()
    }

    fn stall(&self, microseconds: u64) {
        let end = time::nanos() + microseconds * 1000;
        while time::nanos() < end {
            core::hint::spin_loop();
        }
    }

    fn sleep(&self, milliseconds: u64) {
        self.stall(milliseconds * 1000);
    }

    // El AML se ejecuta de a una llamada por vez (lo serializa `AML`), así que sus mutex no
    // tienen con quién competir.
    fn create_mutex(&self) -> Handle {
        Handle(NEXT_MUTEX.fetch_add(1, Ordering::Relaxed))
    }
    fn acquire(&self, _mutex: Handle, _timeout: u16) -> Result<(), AmlError> {
        Ok(())
    }
    fn release(&self, _mutex: Handle) {}

    fn handle_fatal_error(&self, fatal_type: u8, fatal_code: u32, fatal_arg: u64) {
        serial_println!(
            "ACPI: el AML pidió un error fatal ({fatal_type}, {fatal_code:#x}, {fatal_arg:#x})"
        );
    }
}

/// Lo que se sabe de la máquina por ACPI.
#[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
pub struct Acpi {
    pub tables: Tables,
    pub madt: Option<Madt>,
    pub fadt: Option<Fadt>,
    pub ecam: Vec<Ecam>,
    pub hpet: Option<u64>,
}

struct Aml {
    platform: AcpiPlatform<KernelHandler>,
    interpreter: Interpreter<KernelHandler>,
    prt: Option<PciRoutingTable>,
}

// SAFETY: el intérprete y la plataforma guardan punteros a las tablas del firmware (memoria
// que nadie libera). Todo uso pasa por `Acpi::aml`, que lo serializa con AML_LOCK.
unsafe impl Send for Aml {}
// SAFETY: ídem.
unsafe impl Sync for Aml {}

static ACPI: Once<Acpi> = Once::new();
static AML: Once<Aml> = Once::new();
static AML_LOCK: spin::Mutex<()> = spin::Mutex::new(());

pub fn get() -> Option<&'static Acpi> {
    ACPI.get()
}

/// Lee las tablas fijas. El AML se carga aparte (`load_aml`), después del APIC: interpretar
/// la DSDT de una placa real tarda y puede fallar, y el resto del arranque no depende de eso.
pub fn init(rsdp: u64) -> Option<&'static Acpi> {
    let tables = match Tables::read(&Phys, rsdp) {
        Ok(t) => t,
        Err(e) => {
            serial_println!("ACPI: RSDP en {rsdp:#x} inválido ({e:?})");
            return None;
        }
    };
    let find = |sig: &[u8; 4]| tables.find(sig).map(|e| read_table(&Phys, e.phys));
    let madt = find(b"APIC").map(|t| Madt::parse(&t));
    let fadt = find(b"FACP").map(|t| Fadt::parse(&t));
    let ecam = find(b"MCFG").map(|t| parse_mcfg(&t)).unwrap_or_default();
    let hpet = find(b"HPET").and_then(|t| parse_hpet(&t));
    let mut names = alloc::string::String::new();
    for e in &tables.entries {
        names.push_str(core::str::from_utf8(&e.signature).unwrap_or("????"));
        names.push(if e.valid { ' ' } else { '!' });
    }
    serial_println!(
        "ACPI_TABLAS {} (revisión {})",
        names.trim_end(),
        tables.revision
    );
    crate::hw::note("Firmware", alloc::format!("ACPI: {}", names.trim_end()));
    Some(ACPI.call_once(|| Acpi {
        tables,
        madt,
        fadt,
        ecam,
        hpet,
    }))
}

/// Carga la DSDT y las SSDT en el intérprete e inicializa el espacio de nombres.
pub fn load_aml(rsdp: u64) {
    let start = time::millis();
    // SAFETY: `rsdp` es la dirección que dio el bootloader (la validó `init`).
    let tables = match unsafe { AcpiTables::from_rsdp(KernelHandler, rsdp as usize) } {
        Ok(t) => t,
        Err(e) => {
            serial_println!("ACPI: la crate no pudo leer las tablas: {e:?}");
            return;
        }
    };
    let platform = match AcpiPlatform::new(tables, KernelHandler) {
        Ok(p) => p,
        Err(e) => {
            serial_println!("ACPI: plataforma: {e:?}");
            return;
        }
    };
    let interpreter = match Interpreter::new_from_platform(&platform) {
        Ok(i) => i,
        Err(e) => {
            serial_println!("ACPI: no se pudo cargar el AML: {e:?}");
            return;
        }
    };
    interpreter.initialize_namespace();
    // `\_PIC(1)`: "el sistema usa el APIC". Sin esto, `_PRT` describe las rutas del PIC viejo
    // (en QEMU, las IRQ 5, 10 y 11 en vez de las GSI 16–23 del IOAPIC).
    if let Ok(name) = AmlName::from_str(r"\_PIC") {
        let _ = interpreter.evaluate_if_present(name, alloc::vec![Object::Integer(1).wrap()]);
    }
    let prt = ["\\_SB.PCI0._PRT", "\\_SB.PC00._PRT"].iter().find_map(|p| {
        PciRoutingTable::from_prt_path(AmlName::from_str(p).ok()?, &interpreter).ok()
    });
    serial_println!(
        "AML_LISTO en {} ms{}",
        time::millis() - start,
        if prt.is_some() {
            ", con _PRT"
        } else {
            ", sin _PRT"
        }
    );
    AML.call_once(|| Aml {
        platform,
        interpreter,
        prt,
    });
}

impl Acpi {
    fn with_aml<R>(&self, f: impl FnOnce(&Aml) -> R) -> Option<R> {
        let aml = AML.get()?;
        let _guard = AML_LOCK.lock();
        Some(f(aml))
    }

    /// ¿Hay intérprete de AML?
    #[expect(dead_code, reason = "lo usan las etapas siguientes de K13")]
    pub fn has_aml(&self) -> bool {
        AML.get().is_some()
    }

    /// La GSI (y si es por nivel y activa en bajo) de la línea INTx de un dispositivo del bus
    /// 0, según `_PRT`. Los que están detrás de un puente avisan por MSI.
    pub fn pci_gsi(&self, dev: pci::Device) -> Option<(u32, bool, bool)> {
        if dev.bus != 0 {
            return None;
        }
        let pin = match dev.read8(0x3D) {
            1 => Pin::IntA,
            2 => Pin::IntB,
            3 => Pin::IntC,
            4 => Pin::IntD,
            _ => return None,
        };
        self.with_aml(|aml| {
            let d = aml
                .prt
                .as_ref()?
                .route(dev.slot as u16, dev.function as u16, pin, &aml.interpreter)
                .ok()?;
            Some((
                d.irq,
                d.polarity == InterruptPolarity::ActiveLow,
                d.trigger == InterruptTrigger::Level,
            ))
        })?
    }

    /// Los valores SLP_TYPa y SLP_TYPb del estado `\_Sn` (3 = dormir en RAM, 5 = apagado).
    pub fn sleep_type(&self, state: u8) -> Option<(u8, u8)> {
        let name = alloc::format!("\\_S{state}_");
        self.with_aml(|aml| {
            let obj = aml
                .interpreter
                .evaluate(AmlName::from_str(&name).ok()?, Vec::new())
                .ok()?;
            let Object::Package(ref values) = *obj else {
                return None;
            };
            let get = |i: usize| match values.get(i).map(|v| &**v) {
                Some(Object::Integer(n)) => Some(*n as u8),
                _ => None,
            };
            Some((get(0)?, get(1).unwrap_or(0)))
        })?
    }

    /// Llama a un método del AML con un argumento entero (por ejemplo `\_PTS(5)` antes de
    /// apagar). Si no existe, no pasa nada.
    pub fn call(&self, path: &str, arg: Option<u64>) {
        self.with_aml(|aml| {
            let Ok(name) = AmlName::from_str(path) else {
                return;
            };
            let args = arg
                .map(|a| alloc::vec![Object::Integer(a).wrap()])
                .unwrap_or_default();
            if let Err(e) = aml.interpreter.evaluate_if_present(name, args) {
                serial_println!("ACPI: {path} falló: {e:?}");
            }
        });
    }

    /// Evalúa un objeto y lo devuelve como entero (`_TMP`, `_STA`…).
    pub fn integer(&self, path: &str) -> Option<u64> {
        self.with_aml(|aml| {
            let obj = aml
                .interpreter
                .evaluate(AmlName::from_str(path).ok()?, Vec::new())
                .ok()?;
            match *obj {
                Object::Integer(n) => Some(n),
                _ => None,
            }
        })?
    }

    /// Las zonas térmicas del AML que tienen `_TMP` (por ejemplo `\_TZ.TZ00`).
    pub fn thermal_zones(&self) -> Vec<alloc::string::String> {
        self.with_aml(|aml| {
            let mut zones = Vec::new();
            let _ = aml.interpreter.namespace.lock().traverse(|name, level| {
                if level.kind == NamespaceLevelKind::ThermalZone
                    && level.values.keys().any(|seg| seg.as_str() == "_TMP")
                {
                    zones.push(alloc::format!("{name}"));
                }
                Ok(true)
            });
            zones
        })
        .unwrap_or_default()
    }

    /// Pasa la placa a modo ACPI (si no lo estaba): desde ahí los eventos de energía son del
    /// sistema operativo y no del firmware. Hace falta antes de dormir o apagar.
    pub fn enter_acpi_mode(&self) {
        self.with_aml(|aml| {
            if let Err(e) = aml.platform.enter_acpi_mode() {
                serial_println!("ACPI: no se pudo pasar a modo ACPI: {e:?}");
            }
        });
    }
}
