//! Driver de controladoras USB xHCI y de sus dispositivos: teclado, mouse, pendrive y hubs (K13).
//!
//! Hardware: cualquier controladora PCI de clase 0C/03/30 (xHCI 1.x): las AMD 1022:1639/43EE de
//! la PC de Roman y la `qemu-xhci`. En una PC nueva el teclado y el mouse son USB: sin esto no
//! se podría usar JARVIS-OS fuera de QEMU.
//!
//! Cómo anda:
//! 1. Se le saca la controladora al firmware (el "traspaso" de USB Legacy Support), se la
//!    reinicia y se le dan sus estructuras: la tabla de contextos de dispositivo (DCBAA), el
//!    anillo de comandos y el de eventos.
//! 2. Por cada puerto con algo conectado: reset del puerto, "Enable Slot" (la controladora le da
//!    un número), "Address Device" (le pone dirección), y por el endpoint 0 se leen sus
//!    descriptores (`jarvis_drivers::usb`).
//! 3. Según la interfaz: teclado o mouse (HID en modo de arranque: reportes de 8 bytes que no
//!    hace falta interpretar con el descriptor de reportes), pendrive (Bulk-Only + SCSI) o hub
//!    (se recorren sus puertos igual que los de la controladora).
//! 4. "Configure Endpoint" habilita sus endpoints, cada uno con su anillo de TRB.
//!
//! Los reportes de teclado se traducen a scancodes PS/2 y los de mouse a paquetes PS/2 (así el
//! resto del sistema no cambia). La tarea "usb" (solo si hay teclado o mouse) los atiende. Todo
//! lo demás es sincrónico: se manda un comando y se revisa el anillo de eventos con plazo.
//!
//! Límites: hubs USB 2.0 (los dispositivos SuperSpeed detrás de un hub USB 3 no se ven; los
//! USB 2 sí, por el "gemelo" USB 2 del hub). Sin isócronos (audio USB) ni HID que no sea de
//! arranque.
//! Referencias: especificación xHCI 1.2 §4.2 (inicialización), §4.3 (dispositivos), §4.6
//! (comandos), §4.9 (anillos), §7.1 (traspaso del BIOS); USB 2.0 cap. 9 y 11 (hubs); "USB Mass
//! Storage Class Bulk-Only Transport" 1.0; <https://wiki.osdev.org/XHCI>.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering, fence};

use jarvis_drivers::usb::{
    self, CODE_SHORT_PACKET, CODE_SUCCESS, EP_BULK_IN, EP_BULK_OUT, EP_CONTROL, EP_INTERRUPT_IN,
    Endpoint, Event, InputContext, Kind, SPEED_FULL, SPEED_HIGH, SPEED_LOW, SPEED_SUPER, Slot, Trb,
    trb,
};
use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};
use jarvis_task::Priority;

use crate::irqlock::IrqMutex;
use crate::mmio::Mmio;
use crate::storage::Found;
use crate::{dma, interrupts, keyboard, mouse, pci, serial_println, task, time};

const RING: usize = 64;
const EVENTS: usize = 256;
const TIMEOUT_MS: u64 = 1000;
/// Las transferencias masivas pueden tardar mucho más: un pendrive lento, o uno que se está
/// despertando, tarda segundos (la especificación de BOT habla de hasta 20 s).
const BULK_TIMEOUT_MS: u64 = 15_000;
/// Buffer de datos de las transferencias masivas: 64 KiB alineado a 64 KiB (un TRB no puede
/// cruzar un límite de 64 KiB).
const BULK: usize = 64 * 1024;

/// Un anillo de TRB (de comandos o de un endpoint): el último TRB es un enlace al primero.
struct Ring {
    trbs: *mut [u32; 4],
    enqueue: usize,
    cycle: bool,
}

impl Ring {
    fn new() -> Option<Ring> {
        let trbs = dma::alloc(RING * 16, 64)? as *mut [u32; 4];
        let mut r = Ring {
            trbs,
            enqueue: 0,
            cycle: true,
        };
        r.write(RING - 1, Trb::link(r.phys()).with_cycle(false));
        Some(r)
    }

    fn phys(&self) -> u64 {
        dma::phys(self.trbs as *const u8)
    }

    fn write(&mut self, i: usize, t: Trb) {
        // SAFETY: `i < RING`; primero las tres palabras de datos y al final la de control (con
        // el ciclo), para que la controladora nunca vea un TRB a medio escribir.
        unsafe {
            let p = self.trbs.add(i) as *mut u32;
            for (w, value) in t.0[..3].iter().enumerate() {
                core::ptr::write_volatile(p.add(w), *value);
            }
            fence(Ordering::SeqCst);
            core::ptr::write_volatile(p.add(3), t.0[3]);
        }
    }

    /// Agrega un TRB y devuelve su dirección física.
    fn push(&mut self, t: Trb) -> u64 {
        let at = self.enqueue;
        let phys = self.phys() + (at * 16) as u64;
        self.write(at, t.with_cycle(self.cycle));
        self.enqueue += 1;
        if self.enqueue == RING - 1 {
            // Pasar el enlace a la controladora (con el ciclo actual) y dar la vuelta.
            let link = Trb::link(self.phys()).with_cycle(self.cycle);
            self.write(RING - 1, link);
            self.enqueue = 0;
            self.cycle = !self.cycle;
        }
        phys
    }
}

/// Un endpoint de interrupción de un teclado o mouse, con su buffer de reporte.
struct Hid {
    kind: Kind,
    dci: u8,
    len: u16,
    buffer: *mut u8,
    /// El TRB en curso (para reconocer su evento).
    pending: u64,
    last: [u8; 8],
}

struct Storage {
    in_dci: u8,
    out_dci: u8,
    tag: u32,
    sectors: u64,
}

struct Device {
    slot: u8,
    root_port: u8,
    route: u32,
    ep0: Ring,
    rings: Vec<(u8, Ring)>,
    hids: Vec<Hid>,
    storage: Option<Storage>,
    name: String,
}

impl Device {
    fn ring(&mut self, dci: u8) -> Option<&mut Ring> {
        if dci == 1 {
            return Some(&mut self.ep0);
        }
        self.rings
            .iter_mut()
            .find(|(d, _)| *d == dci)
            .map(|(_, r)| r)
    }
}

pub struct Controller {
    op: Mmio,
    rt: Mmio,
    db: Mmio,
    csz64: bool,
    ports: u8,
    dcbaa: *mut u64,
    commands: Ring,
    events: *mut [u32; 4],
    event_index: usize,
    event_cycle: bool,
    /// Buffer para los pedidos de control (descriptores) y los CBW/CSW.
    scratch: *mut u8,
    bulk: *mut u8,
    devices: Vec<Device>,
    /// Eventos de puertos que llegaron mientras se esperaba otra cosa.
    port_changes: Vec<u8>,
}

// SAFETY: los punteros son memoria DMA propia de la controladora; todo acceso pasa por el
// `IrqMutex` de `CONTROLLERS`.
unsafe impl Send for Controller {}

static CONTROLLERS: IrqMutex<Vec<Controller>> = IrqMutex::new(Vec::new());
/// Para el registro: la primera tecla y el primer movimiento que llegan por USB.
static FIRST_KEY: AtomicBool = AtomicBool::new(true);
static FIRST_MOVE: AtomicBool = AtomicBool::new(true);

// Registros operativos (desde CAPLENGTH).
const USBCMD: usize = 0x00;
const USBSTS: usize = 0x04;
const CRCR: usize = 0x18;
const DCBAAP: usize = 0x30;
const CONFIG: usize = 0x38;
const PORTSC: usize = 0x400;
// PORTSC.
const PORT_CCS: u32 = 1 << 0;
const PORT_PED: u32 = 1 << 1;
const PORT_PR: u32 = 1 << 4;
const PORT_PRC: u32 = 1 << 21;
/// Los bits que hay que volver a escribir igual (los de solo lectura y los "RWS"); los demás
/// se escriben en 0 para no borrar avisos ni deshabilitar el puerto sin querer.
const PORT_NEUTRAL: u32 = 0x4E00_FFE9;
/// Todos los avisos de cambio (se borran escribiéndoles 1).
const PORT_CHANGES: u32 = 0x00FE_0000;

/// Busca las controladoras, enumera lo conectado y arranca la tarea "usb" si hay teclado o
/// mouse. Devuelve los discos USB (pendrives) para `storage`.
pub fn init() -> Vec<Found> {
    let mut disks = Vec::new();
    let mut hid = false;
    let mut all = Vec::new();
    for dev in pci::find_class(0x0C, 0x03, 0x30) {
        let Some(mut c) = Controller::start(dev) else {
            serial_println!(
                "xHCI {:02x}:{:02x}.{}: no se pudo iniciar",
                dev.bus,
                dev.slot,
                dev.function
            );
            continue;
        };
        for port in 1..=c.ports {
            c.attach_root(port);
        }
        let index = all.len();
        for d in &c.devices {
            hid |= !d.hids.is_empty();
            if let Some(s) = &d.storage {
                disks.push(Found {
                    name: format!("USB: {}", d.name),
                    disk: Box::new(UsbDisk {
                        controller: index,
                        slot: d.slot,
                        sectors: s.sectors,
                    }),
                });
            }
        }
        all.push(c);
    }
    let any = !all.is_empty();
    CONTROLLERS.with(|c| *c = all);
    if any {
        // Aunque no haya teclado todavía: un teclado enchufado después avisa por eventos.
        let started = task::spawn("usb", Priority::High, 64 * 1024, run).is_some();
        serial_println!(
            "USB_LISTO{}{}",
            if hid { " (teclado o mouse)" } else { "" },
            if started { "" } else { ", sin tarea" }
        );
    }
    disks
}

/// La tarea "usb": atiende los reportes de teclado y mouse y los dispositivos que se enchufan.
fn run() {
    loop {
        CONTROLLERS.with(|all| {
            for c in all.iter_mut() {
                c.poll();
            }
        });
        task::wait(task::EV_USB, Some(time::millis() + 8));
    }
}

impl Controller {
    fn start(dev: pci::Device) -> Option<Controller> {
        let bar = dev.bar_address(0)?;
        dev.enable_memory_and_dma();
        let cap = Mmio::map(bar, 0x40)?;
        let caplength = cap.r8(0) as usize;
        let hcs1 = cap.r32(0x04);
        let hcs2 = cap.r32(0x08);
        let hcc1 = cap.r32(0x10);
        let dboff = cap.r32(0x14) as usize & !0x3;
        let rtsoff = cap.r32(0x18) as usize & !0x1F;
        let max_slots = (hcs1 & 0xFF).min(32) as u8;
        let ports = (hcs1 >> 24) as u8;
        let whole = Mmio::map(
            bar,
            (dboff + 4 * 256)
                .max(rtsoff + 0x40)
                .max(caplength + 0x400 + 0x10 * ports as usize),
        )?;
        let op = whole.sub(caplength, 0x400 + 0x10 * ports as usize);
        let rt = whole.sub(rtsoff, 0x40);
        let db = whole.sub(dboff, 4 * 256);
        legacy_handoff(&whole, (hcc1 >> 16) as usize * 4);

        // Detener y reiniciar.
        op.w32(USBCMD, op.r32(USBCMD) & !1);
        let t = time::millis();
        while op.r32(USBSTS) & 1 == 0 && time::millis() - t < 100 {}
        op.w32(USBCMD, 1 << 1);
        while (op.r32(USBCMD) & 1 << 1 != 0 || op.r32(USBSTS) & 1 << 11 != 0)
            && time::millis() - t < 1000
        {}
        if op.r32(USBCMD) & 1 << 1 != 0 {
            return None;
        }
        op.w32(CONFIG, max_slots as u32);

        let dcbaa = dma::alloc((max_slots as usize + 1) * 8, 64)? as *mut u64;
        // Páginas de trabajo que pide la controladora para sí misma (scratchpad).
        let scratch_pages = ((hcs2 >> 21) & 0x1F) << 5 | (hcs2 >> 27);
        if scratch_pages > 0 {
            let array = dma::alloc(scratch_pages as usize * 8, 64)? as *mut u64;
            for i in 0..scratch_pages as usize {
                let page = dma::alloc(4096, 4096)?;
                // SAFETY: `array` tiene `scratch_pages` entradas.
                unsafe { array.add(i).write(dma::phys(page)) };
            }
            // SAFETY: la entrada 0 de la DCBAA es la del scratchpad.
            unsafe { dcbaa.write(dma::phys(array as *const u8)) };
        }
        op.w64_split(DCBAAP, dma::phys(dcbaa as *const u8));
        let commands = Ring::new()?;
        op.w64_split(CRCR, commands.phys() | 1);

        // Anillo de eventos del interruptor 0: un segmento, descripto por la ERST.
        let events = dma::alloc(EVENTS * 16, 64)? as *mut [u32; 4];
        let erst = dma::alloc(16, 64)?;
        // SAFETY: la ERST tiene una entrada de 16 bytes: base del segmento y su tamaño.
        unsafe {
            (erst as *mut u64).write(dma::phys(events as *const u8));
            (erst.add(8) as *mut u32).write(EVENTS as u32);
        }
        let ir = rt.sub(0x20, 0x20);
        ir.w32(0x08, 1); // ERSTSZ
        ir.w64_split(0x18, dma::phys(events as *const u8)); // ERDP
        ir.w64_split(0x10, dma::phys(erst)); // ERSTBA (al final)
        let msi = interrupts::enable_msi(dev, task::EV_USB, true);
        if msi {
            ir.w32(0x04, 4000); // IMOD: como mucho un aviso cada 1 ms
            ir.w32(0x00, 0b11); // IMAN: habilitada (y borrar IP)
        }
        op.w32(USBCMD, 1 | if msi { 1 << 2 } else { 0 });
        let t = time::millis();
        while op.r32(USBSTS) & 1 != 0 && time::millis() - t < 100 {}

        crate::hw::note(
            "USB",
            format!(
                "xHCI {:04x}:{:04x}, {ports} puertos",
                dev.vendor(),
                dev.device_id()
            ),
        );
        serial_println!(
            "xHCI {:02x}:{:02x}.{} ({:04x}:{:04x}): {} puertos, {} slots, contextos de {} bytes, MSI: {}",
            dev.bus,
            dev.slot,
            dev.function,
            dev.vendor(),
            dev.device_id(),
            ports,
            max_slots,
            if hcc1 & 1 << 2 != 0 { 64 } else { 32 },
            if msi { "sí" } else { "no" }
        );
        Some(Controller {
            op,
            rt,
            db,
            csz64: hcc1 & 1 << 2 != 0,
            ports,
            dcbaa,
            commands,
            events,
            event_index: 0,
            event_cycle: true,
            scratch: dma::alloc(4096, 4096)?,
            bulk: dma::alloc(BULK, BULK)?,
            devices: Vec::new(),
            port_changes: Vec::new(),
        })
    }

    fn portsc(&self, port: u8) -> u32 {
        self.op.r32(PORTSC + 0x10 * (port as usize - 1))
    }

    fn set_portsc(&self, port: u8, value: u32) {
        self.op.w32(PORTSC + 0x10 * (port as usize - 1), value);
    }

    // --- eventos y comandos ---------------------------------------------------------------------

    /// El próximo evento, si hay.
    fn next_event(&mut self) -> Option<Event> {
        // Primero la palabra de control (la del ciclo) y recién después las otras tres: si se
        // leyera el TRB entero de una vez, el puntero podría ser el viejo de un evento que la
        // controladora está terminando de escribir (pasaba con las lecturas largas del
        // instalador: un evento "bien" con puntero 0, y la transferencia quedaba sin respuesta).
        let p = self.events.wrapping_add(self.event_index) as *const u32;
        // SAFETY: `event_index < EVENTS`; la controladora escribe el anillo por DMA.
        let control = unsafe { core::ptr::read_volatile(p.add(3)) };
        if (control & 1 != 0) != self.event_cycle {
            return None;
        }
        fence(Ordering::SeqCst);
        // SAFETY: ídem; el ciclo ya dice que el TRB es de esta vuelta.
        let t = Trb(unsafe {
            [
                core::ptr::read_volatile(p),
                core::ptr::read_volatile(p.add(1)),
                core::ptr::read_volatile(p.add(2)),
                control,
            ]
        });
        self.event_index += 1;
        if self.event_index == EVENTS {
            self.event_index = 0;
            self.event_cycle = !self.event_cycle;
        }
        let erdp = dma::phys(self.events as *const u8) + (self.event_index * 16) as u64;
        let ir = self.rt.sub(0x20, 0x20);
        ir.w64_split(0x18, erdp | 1 << 3); // y borrar "Event Handler Busy"
        ir.w32(0x00, ir.r32(0x00) | 1); // IMAN.IP
        Some(usb::parse_event(&t))
    }

    /// Espera un evento que cumpla `want`; los demás se atienden al pasar.
    fn wait_event(&mut self, want: impl Fn(&Event) -> bool) -> Option<Event> {
        self.wait_event_for(TIMEOUT_MS, want)
    }

    fn wait_event_for(&mut self, timeout_ms: u64, want: impl Fn(&Event) -> bool) -> Option<Event> {
        let t = time::millis();
        while time::millis() - t < timeout_ms {
            match self.next_event() {
                Some(e) if want(&e) => return Some(e),
                Some(e) => self.dispatch(e),
                None => core::hint::spin_loop(),
            }
        }
        None
    }

    fn command(&mut self, t: Trb) -> Option<Event> {
        let at = self.commands.push(t);
        self.db.w32(0, 0);
        let e = self.wait_event(|e| e.kind == trb::COMMAND_COMPLETION && e.pointer == at)?;
        (e.code == CODE_SUCCESS).then_some(e)
    }

    /// Un pedido de control por el endpoint 0. Los datos (hasta 4 KiB) van en `scratch`.
    fn control(&mut self, slot: u8, packet: [u8; 8], data_in: bool) -> Option<usize> {
        let len = u16::from_le_bytes([packet[6], packet[7]]);
        let scratch = dma::phys(self.scratch);
        let dev = self.devices.iter_mut().find(|d| d.slot == slot)?;
        let has_data = len > 0;
        dev.ep0
            .push(Trb::setup_stage(packet, has_data.then_some(data_in)));
        if has_data {
            dev.ep0.push(Trb::data_stage(scratch, len, data_in));
        }
        let status = dev.ep0.push(Trb::status_stage(!(has_data && data_in)));
        self.db.w32(4 * slot as usize, 1);
        let e = self
            .wait_event(|e| e.kind == trb::TRANSFER_EVENT && e.slot == slot && e.endpoint == 1)?;
        // El evento del estado (o un error en cualquier etapa).
        let ok = e.code == CODE_SUCCESS || e.code == CODE_SHORT_PACKET;
        if !ok {
            return None;
        }
        if e.pointer != status {
            // Paquete corto en la etapa de datos: todavía falta el evento del estado.
            self.wait_event(|e| {
                e.kind == trb::TRANSFER_EVENT && e.slot == slot && e.pointer == status
            })?;
        }
        Some(len as usize - (e.residue as usize).min(len as usize))
    }

    fn scratch(&self, len: usize) -> &[u8] {
        // SAFETY: `scratch` mide 4 KiB y `len` no pasa de ahí (los pedidos son de ≤ 4 KiB).
        unsafe { core::slice::from_raw_parts(self.scratch, len.min(4096)) }
    }

    /// Una transferencia masiva (o de interrupción) sincrónica de `len` bytes del buffer masivo.
    fn bulk(&mut self, slot: u8, dci: u8, len: usize) -> Option<usize> {
        let buf = dma::phys(self.bulk);
        let dev = self.devices.iter_mut().find(|d| d.slot == slot)?;
        let at = dev.ring(dci)?.push(Trb::normal(buf, len as u32));
        self.db.w32(4 * slot as usize, dci as u32);
        let Some(e) = self.wait_event_for(BULK_TIMEOUT_MS, |e| {
            e.kind == trb::TRANSFER_EVENT && e.pointer == at
        }) else {
            serial_println!("USB: sin evento de la transferencia (endpoint {dci})");
            return None;
        };
        if e.code != CODE_SUCCESS && e.code != CODE_SHORT_PACKET {
            serial_println!("USB: la transferencia terminó con el código {}", e.code);
        }
        (e.code == CODE_SUCCESS || e.code == CODE_SHORT_PACKET)
            .then(|| len - (e.residue as usize).min(len))
    }

    // --- eventos asincrónicos -------------------------------------------------------------------

    /// Revisa el anillo de eventos (la tarea "usb" lo llama cada tanto).
    fn poll(&mut self) {
        while let Some(e) = self.next_event() {
            self.dispatch(e);
        }
        for port in core::mem::take(&mut self.port_changes) {
            let v = self.portsc(port);
            self.set_portsc(port, (v & PORT_NEUTRAL) | (v & PORT_CHANGES));
            let known = self
                .devices
                .iter()
                .any(|d| d.root_port == port && d.route == 0);
            if v & PORT_CCS != 0 && !known {
                self.attach_root(port);
            } else if v & PORT_CCS == 0 && known {
                self.devices.retain(|d| d.root_port != port);
                serial_println!("USB: se desconectó el puerto {port}");
            }
        }
    }

    fn dispatch(&mut self, e: Event) {
        match e.kind {
            trb::PORT_STATUS_CHANGE => {
                let port = (e.pointer >> 24) as u8;
                if !self.port_changes.contains(&port) {
                    self.port_changes.push(port);
                }
            }
            trb::TRANSFER_EVENT => self.hid_report(e),
            _ => {}
        }
    }

    /// Llegó un reporte de teclado o mouse: se traduce y se vuelve a pedir el próximo.
    fn hid_report(&mut self, e: Event) {
        let db = self.db;
        let Some(dev) = self.devices.iter_mut().find(|d| d.slot == e.slot) else {
            return;
        };
        let Some(i) = dev.hids.iter().position(|h| h.pending == e.pointer) else {
            return;
        };
        let (kind, len, buffer) = (dev.hids[i].kind, dev.hids[i].len, dev.hids[i].buffer);
        if e.code == CODE_SUCCESS || e.code == CODE_SHORT_PACKET {
            let got = (len as usize).saturating_sub(e.residue as usize).min(8);
            let mut report = [0u8; 8];
            // SAFETY: `buffer` mide `len` bytes (≤ 64) y la controladora escribió `got`.
            unsafe { core::ptr::copy_nonoverlapping(buffer, report.as_mut_ptr(), got) };
            match kind {
                Kind::Keyboard => {
                    let codes = usb::keyboard_report(&dev.hids[i].last, &report);
                    if !codes.is_empty() && FIRST_KEY.swap(false, Ordering::Relaxed) {
                        serial_println!("USB_TECLA {:02x?}", codes);
                    }
                    for code in codes {
                        keyboard::push_scancode(code);
                    }
                    dev.hids[i].last = report;
                }
                Kind::Mouse => {
                    if let Some(p) = usb::mouse_report(&report[..got]) {
                        if FIRST_MOVE.swap(false, Ordering::Relaxed) {
                            serial_println!("USB_MOVIMIENTO {:02x?}", p);
                        }
                        mouse::push_usb_packet(p);
                    }
                }
                _ => {}
            }
        }
        let dci = dev.hids[i].dci;
        if let Some(ring) = dev.ring(dci) {
            let at = ring.push(Trb::normal(dma::phys(buffer), len as u32));
            dev.hids[i].pending = at;
            db.w32(4 * dev.slot as usize, dci as u32);
        }
    }

    // --- enumeración ----------------------------------------------------------------------------

    /// Un dispositivo en un puerto de la controladora.
    fn attach_root(&mut self, port: u8) {
        let v = self.portsc(port);
        if v & PORT_CCS == 0 {
            return;
        }
        // Los puertos USB 3 se habilitan solos; los USB 2 necesitan un reset.
        if v & PORT_PED == 0 {
            self.set_portsc(port, (v & PORT_NEUTRAL) | PORT_PR);
            let t = time::millis();
            while self.portsc(port) & PORT_PRC == 0 && time::millis() - t < 500 {}
            let v = self.portsc(port);
            self.set_portsc(port, (v & PORT_NEUTRAL) | PORT_CHANGES);
            spin_ms(10); // recuperación después del reset (USB 2.0 §7.1.7.5)
        }
        let v = self.portsc(port);
        if v & PORT_PED == 0 {
            serial_println!("USB: el puerto {port} no se habilitó (PORTSC {v:#x})");
            return;
        }
        let speed = ((v >> 10) & 0xF) as u8;
        self.attach(port, 0, speed, None, 0);
    }

    /// Da de alta un dispositivo: dirección, descriptores y según su clase.
    fn attach(&mut self, root_port: u8, route: u32, speed: u8, tt: Option<(u8, u8)>, depth: u8) {
        let Some(e) = self.command(Trb::enable_slot()) else {
            serial_println!("USB: no hay slots libres");
            return;
        };
        let slot = e.slot;
        let ctx_size = if self.csz64 { 64 } else { 32 };
        let Some(out) = dma::alloc(ctx_size * 32, 64) else {
            return;
        };
        // SAFETY: la DCBAA tiene una entrada por slot (hasta MaxSlotsEn).
        unsafe { self.dcbaa.add(slot as usize).write(dma::phys(out)) };
        let Some(ep0) = Ring::new() else { return };
        let mut max0 = usb::default_max_packet0(speed);
        let slot_ctx = Slot {
            route,
            speed,
            root_port,
            context_entries: 1,
            hub_ports: None,
            tt,
        };
        let mut ic = InputContext::new(self.csz64);
        ic.add(0b11);
        ic.slot(&slot_ctx);
        ic.endpoint(1, EP_CONTROL, max0, 0, ep0.phys());
        self.devices.push(Device {
            slot,
            root_port,
            route,
            ep0,
            rings: Vec::new(),
            hids: Vec::new(),
            storage: None,
            name: String::new(),
        });
        let Some(input) = self.input(&ic) else { return };
        if self.command(Trb::address_device(input, slot)).is_none() {
            serial_println!("USB: Address Device falló (puerto {root_port}, ruta {route:#x})");
            self.devices.retain(|d| d.slot != slot);
            return;
        }
        // Los primeros 8 bytes dicen el tamaño real del paquete del endpoint 0.
        if self.control(slot, usb::get_descriptor(usb::DESC_DEVICE, 8), true) != Some(8) {
            serial_println!("USB: el dispositivo del puerto {root_port} no responde");
            return;
        }
        let real = match speed {
            SPEED_SUPER => 1u16 << self.scratch(8)[7].min(9),
            _ => self.scratch(8)[7] as u16,
        };
        if real != max0 && real >= 8 {
            max0 = real;
            let mut ic = InputContext::new(self.csz64);
            ic.add(0b10);
            let ring = self
                .devices
                .iter()
                .find(|d| d.slot == slot)
                .map(|d| d.ep0.phys());
            ic.endpoint(1, EP_CONTROL, max0, 0, ring.unwrap_or(0));
            if let Some(input) = self.input(&ic) {
                let _ = self.command(Trb::evaluate_context(input, slot));
            }
        }
        let Some(18) = self.control(slot, usb::get_descriptor(usb::DESC_DEVICE, 18), true) else {
            return;
        };
        let Some(desc) = usb::parse_device(self.scratch(18)) else {
            return;
        };
        let Some(n) = self.control(slot, usb::get_descriptor(usb::DESC_CONFIG, 9), true) else {
            return;
        };
        let total = u16::from_le_bytes([self.scratch(n)[2], self.scratch(n)[3]]).min(4096);
        let Some(n) = self.control(slot, usb::get_descriptor(usb::DESC_CONFIG, total), true) else {
            return;
        };
        let Some(config) = usb::parse_configuration(self.scratch(n)) else {
            return;
        };
        let speed_name = match speed {
            SPEED_LOW => "baja",
            SPEED_FULL => "media",
            SPEED_HIGH => "alta",
            _ => "súper",
        };
        let name = format!("{:04x}:{:04x}", desc.vendor, desc.product);
        serial_println!(
            "USB_DISPOSITIVO {name} (clase {:02x}, velocidad {speed_name}, puerto {root_port}, ruta {route:#x}, {} interfaces)",
            desc.class,
            config.interfaces.len()
        );
        if let Some(d) = self.devices.iter_mut().find(|d| d.slot == slot) {
            d.name = name;
        }
        if self
            .control(
                slot,
                usb::setup(0, usb::SET_CONFIGURATION, config.value as u16, 0, 0),
                false,
            )
            .is_none()
        {
            return;
        }
        // Una interfaz por clase (la alternativa 0).
        let mut endpoints: Vec<(Endpoint, u32)> = Vec::new();
        let mut hids = Vec::new();
        let mut storage = None;
        let mut hub = None;
        for i in config.interfaces.iter().filter(|i| i.alternate == 0) {
            match usb::kind_of(i) {
                Some(kind @ (Kind::Keyboard | Kind::Mouse)) => {
                    let Some(ep) = i.endpoints.iter().find(|e| e.in_ && e.kind == 3) else {
                        continue;
                    };
                    // Modo de arranque (reportes fijos) y, el teclado, sin repetir reportes.
                    let _ = self.control(
                        slot,
                        usb::setup(0x21, usb::HID_SET_PROTOCOL, 0, i.number as u16, 0),
                        false,
                    );
                    if kind == Kind::Keyboard {
                        let _ = self.control(
                            slot,
                            usb::setup(0x21, usb::HID_SET_IDLE, 0, i.number as u16, 0),
                            false,
                        );
                    }
                    endpoints.push((*ep, EP_INTERRUPT_IN));
                    hids.push((kind, *ep));
                }
                Some(Kind::Storage) => {
                    let bin = i.endpoints.iter().find(|e| e.in_ && e.kind == 2);
                    let bout = i.endpoints.iter().find(|e| !e.in_ && e.kind == 2);
                    if let (Some(bin), Some(bout)) = (bin, bout) {
                        endpoints.push((*bin, EP_BULK_IN));
                        endpoints.push((*bout, EP_BULK_OUT));
                        storage = Some((bin.dci(), bout.dci()));
                    }
                }
                Some(Kind::Hub) => hub = i.endpoints.iter().find(|e| e.in_ && e.kind == 3).copied(),
                None => {}
            }
        }
        if let Some(ep) = hub {
            self.attach_hub(slot, slot_ctx, ep, depth);
            return;
        }
        if endpoints.is_empty() {
            return;
        }
        if !self.configure(slot, slot_ctx, &endpoints) {
            serial_println!("USB: Configure Endpoint falló ({})", slot);
            return;
        }
        for (kind, ep) in hids {
            self.start_hid(slot, kind, ep);
        }
        if let Some((in_dci, out_dci)) = storage {
            self.start_storage(slot, in_dci, out_dci);
        }
    }

    /// Copia un contexto de entrada a memoria DMA (hay uno por comando; son chicos).
    fn input(&self, ic: &InputContext) -> Option<u64> {
        let p = dma::alloc(ic.bytes.len(), 64)?;
        // SAFETY: `p` mide lo mismo que el contexto.
        unsafe { core::ptr::copy_nonoverlapping(ic.bytes.as_ptr(), p, ic.bytes.len()) };
        Some(dma::phys(p))
    }

    /// Configure Endpoint con los endpoints pedidos (cada uno con su anillo nuevo).
    fn configure(&mut self, slot: u8, mut slot_ctx: Slot, endpoints: &[(Endpoint, u32)]) -> bool {
        let speed = slot_ctx.speed;
        let mut ic = InputContext::new(self.csz64);
        let mut add = 1u32;
        let mut rings = Vec::new();
        for (ep, kind) in endpoints {
            let Some(ring) = Ring::new() else {
                return false;
            };
            let interval = if *kind == EP_INTERRUPT_IN {
                usb::xhci_interval(speed, ep.interval)
            } else {
                0
            };
            ic.endpoint(ep.dci(), *kind, ep.max_packet, interval, ring.phys());
            add |= 1 << ep.dci();
            slot_ctx.context_entries = slot_ctx.context_entries.max(ep.dci());
            rings.push((ep.dci(), ring));
        }
        ic.add(add);
        ic.slot(&slot_ctx);
        let Some(input) = self.input(&ic) else {
            return false;
        };
        if self.command(Trb::configure_endpoint(input, slot)).is_none() {
            return false;
        }
        if let Some(d) = self.devices.iter_mut().find(|d| d.slot == slot) {
            d.rings.extend(rings);
        }
        true
    }

    fn start_hid(&mut self, slot: u8, kind: Kind, ep: Endpoint) {
        let len = ep.max_packet.clamp(3, 64);
        let Some(buffer) = dma::alloc(64, 64) else {
            return;
        };
        let db = self.db;
        let Some(dev) = self.devices.iter_mut().find(|d| d.slot == slot) else {
            return;
        };
        let Some(ring) = dev.ring(ep.dci()) else {
            return;
        };
        let pending = ring.push(Trb::normal(dma::phys(buffer), len as u32));
        dev.hids.push(Hid {
            kind,
            dci: ep.dci(),
            len,
            buffer,
            pending,
            last: [0; 8],
        });
        db.w32(4 * slot as usize, ep.dci() as u32);
        serial_println!(
            "{} {}",
            if kind == Kind::Keyboard {
                "USB_TECLADO"
            } else {
                "USB_MOUSE"
            },
            dev.name
        );
        crate::hw::note(
            "USB",
            format!(
                "{} {}",
                if kind == Kind::Keyboard {
                    "Teclado"
                } else {
                    "Mouse"
                },
                dev.name
            ),
        );
    }

    // --- almacenamiento masivo --------------------------------------------------------------------

    fn start_storage(&mut self, slot: u8, in_dci: u8, out_dci: u8) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.slot == slot) {
            d.storage = Some(Storage {
                in_dci,
                out_dci,
                tag: 1,
                sectors: 0,
            });
        }
        let name = self
            .scsi(slot, &usb::scsi_inquiry(), 36, false)
            .and_then(|n| usb::parse_inquiry(self.bulk_slice(n)))
            .unwrap_or_default();
        // Hasta que el medio esté listo (los pendrives tardan un poco al enchufarse).
        for _ in 0..10 {
            if self
                .scsi(slot, &usb::scsi_test_unit_ready(), 0, false)
                .is_some()
            {
                break;
            }
            let _ = self.scsi(slot, &usb::scsi_request_sense(), 18, false);
            spin_ms(100);
        }
        let Some((sectors, size)) = self
            .scsi(slot, &usb::scsi_read_capacity(), 8, false)
            .and_then(|n| usb::parse_capacity(self.bulk_slice(n)))
        else {
            serial_println!("USB: el disco \"{name}\" no dijo su tamaño");
            return;
        };
        if size != SECTOR_SIZE as u32 {
            serial_println!("USB: \"{name}\" usa bloques de {size} bytes; solo se usan los de 512");
            return;
        }
        if let Some(d) = self.devices.iter_mut().find(|d| d.slot == slot) {
            if let Some(s) = d.storage.as_mut() {
                s.sectors = sectors;
            }
            if !name.is_empty() {
                d.name = name.clone();
            }
        }
        serial_println!("USB_DISCO \"{name}\", {} MiB", sectors / 2048);
        crate::hw::note("Disco", format!("USB: {name}"));
    }

    fn bulk_slice(&self, len: usize) -> &[u8] {
        // SAFETY: `bulk` mide BULK bytes y `len` no pasa de ahí.
        unsafe { core::slice::from_raw_parts(self.bulk, len.min(BULK)) }
    }

    /// Un comando SCSI por Bulk-Only: CBW, datos (en el buffer masivo) y CSW. Devuelve los bytes
    /// de datos transferidos, o `None` si falló.
    fn scsi(&mut self, slot: u8, command: &[u8], len: usize, write: bool) -> Option<usize> {
        let dev = self.devices.iter_mut().find(|d| d.slot == slot)?;
        let s = dev.storage.as_mut()?;
        let (in_dci, out_dci, tag) = (s.in_dci, s.out_dci, s.tag);
        s.tag = s.tag.wrapping_add(1);
        // El CBW va en el buffer de control (el masivo tiene los datos).
        let cbw = usb::cbw(tag, len as u32, !write, 0, command);
        // SAFETY: `scratch` mide 4 KiB; el CBW, 31 bytes.
        unsafe { core::ptr::copy_nonoverlapping(cbw.as_ptr(), self.scratch, cbw.len()) };
        let scratch = dma::phys(self.scratch);
        let dev = self.devices.iter_mut().find(|d| d.slot == slot)?;
        let at = dev.ring(out_dci)?.push(Trb::normal(scratch, 31));
        self.db.w32(4 * slot as usize, out_dci as u32);
        let e = self.wait_event_for(BULK_TIMEOUT_MS, |e| {
            e.kind == trb::TRANSFER_EVENT && e.pointer == at
        });
        if e.is_none_or(|e| e.code != CODE_SUCCESS) {
            serial_println!("USB: CBW falló ({:?})", e.map(|e| e.code));
            return None;
        }
        let mut moved = 0;
        if len > 0 {
            match self.bulk(slot, if write { out_dci } else { in_dci }, len) {
                Some(n) => moved = n,
                None => {
                    serial_println!("USB: los datos fallaron ({len} bytes)");
                    return None;
                }
            }
        }
        // El CSW (13 bytes), al final del buffer de control.
        let csw_at = scratch + 512;
        let dev = self.devices.iter_mut().find(|d| d.slot == slot)?;
        let at = dev.ring(in_dci)?.push(Trb::normal(csw_at, 13));
        self.db.w32(4 * slot as usize, in_dci as u32);
        self.wait_event_for(BULK_TIMEOUT_MS, |e| {
            e.kind == trb::TRANSFER_EVENT && e.pointer == at
        })?;
        let status = usb::csw(&self.scratch(525)[512..], tag)?;
        (status == 0).then_some(moved)
    }

    fn storage_rw(
        &mut self,
        slot: u8,
        write: bool,
        lba: u64,
        buf: &mut [u8],
    ) -> Result<(), IoError> {
        let step = BULK / SECTOR_SIZE;
        for (i, chunk) in buf.chunks_mut(BULK).enumerate() {
            let sectors = chunk.len().div_ceil(SECTOR_SIZE);
            let at = lba + (i * step) as u64;
            let cmd = usb::scsi_rw10(
                write,
                u32::try_from(at).map_err(|_| IoError)?,
                sectors as u16,
            );
            if write {
                // SAFETY: `bulk` mide BULK bytes y `chunk` no es más largo.
                unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), self.bulk, chunk.len()) };
            }
            if self.scsi(slot, &cmd, chunk.len(), write) != Some(chunk.len()) {
                serial_println!("USB: error en el sector {at}");
                return Err(IoError);
            }
            if !write {
                chunk.copy_from_slice(self.bulk_slice(chunk.len()));
            }
        }
        Ok(())
    }

    // --- hubs -------------------------------------------------------------------------------------

    /// Un hub USB 2.0: se lo marca como hub en su contexto (con Configure Endpoint, junto con su
    /// endpoint de "cambios de estado"), se encienden sus puertos y se da de alta lo que tenga
    /// conectado.
    fn attach_hub(&mut self, slot: u8, mut slot_ctx: Slot, ep: Endpoint, depth: u8) {
        if slot_ctx.speed == SPEED_SUPER || depth >= 4 {
            serial_println!("USB: hub USB 3 o demasiado profundo: no se recorre");
            return;
        }
        let Some(n) = self.control(
            slot,
            usb::setup(0xA0, usb::GET_DESCRIPTOR, usb::HUB_DESCRIPTOR << 8, 0, 9),
            true,
        ) else {
            return;
        };
        let ports = self.scratch(n).get(2).copied().unwrap_or(0).min(15);
        let power_wait = self.scratch(n).get(5).copied().unwrap_or(50) as u64 * 2;
        slot_ctx.hub_ports = Some(ports);
        if !self.configure(slot, slot_ctx, &[(ep, EP_INTERRUPT_IN)]) {
            serial_println!("USB: el hub no se pudo configurar");
            return;
        }
        serial_println!("USB_HUB {ports} puertos (ruta {:#x})", slot_ctx.route);
        for p in 1..=ports {
            let _ = self.control(
                slot,
                usb::setup(0x23, usb::SET_FEATURE, usb::PORT_POWER, p as u16, 0),
                false,
            );
        }
        spin_ms(power_wait.max(100));
        let (hub_speed, route, root) = (slot_ctx.speed, slot_ctx.route, slot_ctx.root_port);
        for p in 1..=ports {
            let status = || usb::setup(0xA3, usb::GET_STATUS, 0, p as u16, 4);
            let Some(4) = self.control(slot, status(), true) else {
                continue;
            };
            if self.scratch(4)[0] & 1 == 0 {
                continue; // nada conectado
            }
            let _ = self.control(
                slot,
                usb::setup(0x23, usb::SET_FEATURE, usb::PORT_RESET, p as u16, 0),
                false,
            );
            let t = time::millis();
            let mut ok = false;
            while time::millis() - t < 500 {
                if self.control(slot, status(), true) == Some(4) && self.scratch(4)[2] & 1 << 4 != 0
                {
                    ok = true;
                    break;
                }
                spin_ms(10);
            }
            if !ok {
                continue;
            }
            let st = u16::from_le_bytes([self.scratch(4)[0], self.scratch(4)[1]]);
            for feature in [usb::C_PORT_RESET, usb::C_PORT_CONNECTION] {
                let _ = self.control(
                    slot,
                    usb::setup(0x23, usb::CLEAR_FEATURE, feature, p as u16, 0),
                    false,
                );
            }
            spin_ms(10);
            let speed = if st & 1 << 9 != 0 {
                SPEED_LOW
            } else if st & 1 << 10 != 0 {
                SPEED_HIGH
            } else {
                SPEED_FULL
            };
            // Los de baja y media velocidad detrás de un hub de alta hablan por su traductor.
            let tt = (hub_speed == SPEED_HIGH && speed != SPEED_HIGH).then_some((slot, p));
            let child_route = route | (p as u32) << (4 * depth);
            self.attach(root, child_route, speed, tt, depth + 1);
        }
    }
}

/// El traspaso de USB Legacy Support (§7.1): el firmware maneja el teclado USB hasta que el
/// sistema operativo se lo pide (así anda en el menú de UEFI).
fn legacy_handoff(regs: &Mmio, mut at: usize) {
    let mut n = 0;
    while at != 0 && n < 64 {
        let cap = regs.r32(at);
        if cap & 0xFF == 1 {
            regs.w32(at, cap | 1 << 24); // "lo quiere el sistema operativo"
            let t = time::millis();
            while regs.r32(at) & 1 << 16 != 0 && time::millis() - t < 1000 {}
            // Sin interrupciones SMI del firmware (bits 0–15 en 0), y borrar las pendientes
            // (bits 29–31, se borran escribiéndoles 1).
            regs.w32(at + 4, 0xE000_0000);
            return;
        }
        let next = ((cap >> 8) & 0xFF) as usize * 4;
        if next == 0 {
            return;
        }
        at += next;
        n += 1;
    }
}

fn spin_ms(ms: u64) {
    let t = time::millis();
    while time::millis() - t < ms {
        core::hint::spin_loop();
    }
}

/// Un disco USB (pendrive) como `BlockDevice`.
pub struct UsbDisk {
    controller: usize,
    slot: u8,
    sectors: u64,
}

impl BlockDevice for UsbDisk {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let (c, slot) = (self.controller, self.slot);
        CONTROLLERS.with(|all| {
            all.get_mut(c)
                .ok_or(IoError)?
                .storage_rw(slot, false, lba, buf)
        })
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let (c, slot) = (self.controller, self.slot);
        let mut copy = buf.to_vec();
        CONTROLLERS.with(|all| {
            all.get_mut(c)
                .ok_or(IoError)?
                .storage_rw(slot, true, lba, &mut copy)
        })
    }
}
