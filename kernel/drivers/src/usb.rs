//! USB: descriptores, pedidos de control y las estructuras de la controladora xHCI.
//!
//! Un dispositivo USB se describe a sí mismo con **descriptores**: uno del dispositivo
//! (fabricante, modelo, tamaño de paquete) y una "configuración" con sus **interfaces** (qué
//! clase de cosa es: teclado, disco, hub…) y los **endpoints** de cada una (canales de datos:
//! de interrupción para teclados y mouse, masivos para discos). Se le piden con pedidos de
//! control de 8 bytes (SETUP) por el endpoint 0.
//!
//! La controladora **xHCI** (USB 3, en toda PC desde ~2012) se maneja con anillos de TRB
//! ("Transfer Request Block", 16 bytes): uno de comandos, uno de eventos y uno por endpoint. El
//! bit de ciclo de cada TRB le dice a quién le toca: se invierte en cada vuelta del anillo.
//! Referencias: especificación USB 2.0 cap. 9 (descriptores y pedidos) y xHCI 1.2 (Intel) §4
//! (funcionamiento), §6 (estructuras de datos).

use alloc::vec::Vec;

use crate::le;

// --- pedidos de control ------------------------------------------------------------------------

/// El paquete SETUP (8 bytes).
pub fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
    let mut s = [0u8; 8];
    s[0] = request_type;
    s[1] = request;
    s[2..4].copy_from_slice(&value.to_le_bytes());
    s[4..6].copy_from_slice(&index.to_le_bytes());
    s[6..8].copy_from_slice(&length.to_le_bytes());
    s
}

pub const GET_DESCRIPTOR: u8 = 6;
pub const SET_CONFIGURATION: u8 = 9;
pub const DESC_DEVICE: u16 = 1;
pub const DESC_CONFIG: u16 = 2;
/// Pedidos de la clase HID.
pub const HID_SET_IDLE: u8 = 0x0A;
pub const HID_SET_PROTOCOL: u8 = 0x0B;
/// Pedidos de la clase hub.
pub const HUB_DESCRIPTOR: u16 = 0x29;
pub const HUB3_DESCRIPTOR: u16 = 0x2A;
pub const GET_STATUS: u8 = 0;
pub const CLEAR_FEATURE: u8 = 1;
pub const SET_FEATURE: u8 = 3;
pub const PORT_RESET: u16 = 4;
pub const PORT_POWER: u16 = 8;
pub const C_PORT_CONNECTION: u16 = 16;
pub const C_PORT_RESET: u16 = 20;

/// GET_DESCRIPTOR del dispositivo o de la configuración.
pub fn get_descriptor(kind: u16, length: u16) -> [u8; 8] {
    setup(0x80, GET_DESCRIPTOR, kind << 8, 0, length)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub usb: u16,
    pub class: u8,
    pub max_packet0: u8,
    pub vendor: u16,
    pub product: u16,
    pub configurations: u8,
}

pub fn parse_device(d: &[u8]) -> Option<DeviceDescriptor> {
    (d.len() >= 18 && d[1] == 1).then(|| DeviceDescriptor {
        usb: le(d, 2, 2) as u16,
        class: d[4],
        max_packet0: d[7],
        vendor: le(d, 8, 2) as u16,
        product: le(d, 10, 2) as u16,
        configurations: d[17],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// Número (1–15) y dirección (`in_`: del dispositivo a la PC).
    pub number: u8,
    pub in_: bool,
    /// 0 control, 1 isócrono, 2 masivo, 3 interrupción.
    pub kind: u8,
    pub max_packet: u16,
    pub interval: u8,
}

impl Endpoint {
    /// El índice del endpoint en el contexto del dispositivo de xHCI (DCI): 2 × número + (1 si
    /// es de entrada). El endpoint 0 es el 1.
    pub fn dci(&self) -> u8 {
        self.number * 2 + self.in_ as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub number: u8,
    pub alternate: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub value: u8,
    pub interfaces: Vec<Interface>,
}

/// Recorre la configuración entera (la cabecera de 9 bytes y lo que sigue: interfaces,
/// endpoints, descriptores de clase).
pub fn parse_configuration(d: &[u8]) -> Option<Configuration> {
    if d.len() < 9 || d[1] != 2 {
        return None;
    }
    let total = (le(d, 2, 2) as usize).min(d.len());
    let mut c = Configuration {
        value: d[5],
        interfaces: Vec::new(),
    };
    let mut at = 0;
    while at + 2 <= total {
        let (len, kind) = (d[at] as usize, d[at + 1]);
        if len < 2 || at + len > total {
            break;
        }
        let e = &d[at..at + len];
        match kind {
            4 if len >= 9 => c.interfaces.push(Interface {
                number: e[2],
                alternate: e[3],
                class: e[5],
                subclass: e[6],
                protocol: e[7],
                endpoints: Vec::new(),
            }),
            5 if len >= 7 => {
                if let Some(i) = c.interfaces.last_mut() {
                    i.endpoints.push(Endpoint {
                        number: e[2] & 0x0F,
                        in_: e[2] & 0x80 != 0,
                        kind: e[3] & 0b11,
                        max_packet: le(e, 4, 2) as u16 & 0x7FF,
                        interval: e[6],
                    });
                }
            }
            _ => {}
        }
        at += len;
    }
    Some(c)
}

/// Clases de interfaz que maneja JARVIS-OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyboard,
    Mouse,
    /// Almacenamiento masivo por "Bulk-Only Transport" con comandos SCSI (pendrives, discos).
    Storage,
    Hub,
}

pub fn kind_of(i: &Interface) -> Option<Kind> {
    match (i.class, i.subclass, i.protocol) {
        (3, 1, 1) => Some(Kind::Keyboard),
        (3, 1, 2) => Some(Kind::Mouse),
        (8, 6, 0x50) => Some(Kind::Storage),
        (9, _, _) => Some(Kind::Hub),
        _ => None,
    }
}

// --- xHCI ---------------------------------------------------------------------------------------

/// Tipos de TRB.
pub mod trb {
    pub const NORMAL: u32 = 1;
    pub const SETUP: u32 = 2;
    pub const DATA: u32 = 3;
    pub const STATUS: u32 = 4;
    pub const LINK: u32 = 6;
    pub const ENABLE_SLOT: u32 = 9;
    pub const ADDRESS_DEVICE: u32 = 11;
    pub const CONFIGURE_ENDPOINT: u32 = 12;
    pub const EVALUATE_CONTEXT: u32 = 13;
    pub const TRANSFER_EVENT: u32 = 32;
    pub const COMMAND_COMPLETION: u32 = 33;
    pub const PORT_STATUS_CHANGE: u32 = 34;
}

/// Un TRB: cuatro palabras de 32 bits. La última lleva el tipo (bits 10–15), el ciclo (bit 0) y
/// banderas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Trb(pub [u32; 4]);

/// Bit 5 de la palabra de control: interrumpir al terminar (IOC).
pub const IOC: u32 = 1 << 5;

impl Trb {
    pub fn kind(&self) -> u32 {
        (self.0[3] >> 10) & 0x3F
    }
    pub fn cycle(&self) -> bool {
        self.0[3] & 1 != 0
    }
    pub fn with_cycle(mut self, cycle: bool) -> Trb {
        self.0[3] = (self.0[3] & !1) | cycle as u32;
        self
    }
    fn new(kind: u32, lo: u64, status: u32, flags: u32) -> Trb {
        Trb([lo as u32, (lo >> 32) as u32, status, kind << 10 | flags])
    }

    /// Enlace al principio del anillo (TC: invierte el ciclo al pasar).
    pub fn link(to: u64) -> Trb {
        Trb::new(trb::LINK, to, 0, 1 << 1)
    }
    pub fn enable_slot() -> Trb {
        Trb::new(trb::ENABLE_SLOT, 0, 0, 0)
    }
    pub fn address_device(input: u64, slot: u8) -> Trb {
        Trb::new(trb::ADDRESS_DEVICE, input, 0, (slot as u32) << 24)
    }
    pub fn configure_endpoint(input: u64, slot: u8) -> Trb {
        Trb::new(trb::CONFIGURE_ENDPOINT, input, 0, (slot as u32) << 24)
    }
    pub fn evaluate_context(input: u64, slot: u8) -> Trb {
        Trb::new(trb::EVALUATE_CONTEXT, input, 0, (slot as u32) << 24)
    }

    /// Etapa SETUP: el paquete va adentro del TRB (IDT). TRT: 3 = hay datos de entrada, 2 = de
    /// salida, 0 = sin datos.
    pub fn setup_stage(packet: [u8; 8], data_in: Option<bool>) -> Trb {
        let trt = match data_in {
            Some(true) => 3,
            Some(false) => 2,
            None => 0,
        };
        let lo = u64::from_le_bytes(packet);
        Trb::new(trb::SETUP, lo, 8, 1 << 6 | trt << 16)
    }
    /// Etapa de datos. ISP (bit 2): si llega menos de lo pedido, avisar con un evento (así se
    /// sabe cuánto llegó de verdad).
    pub fn data_stage(buffer: u64, len: u16, in_: bool) -> Trb {
        Trb::new(trb::DATA, buffer, len as u32, 1 << 2 | (in_ as u32) << 16)
    }
    /// Etapa de estado: va en la dirección contraria a los datos (IN si no hubo datos).
    pub fn status_stage(in_: bool) -> Trb {
        Trb::new(trb::STATUS, 0, 0, IOC | (in_ as u32) << 16)
    }
    pub fn normal(buffer: u64, len: u32) -> Trb {
        Trb::new(trb::NORMAL, buffer, len & 0x1_FFFF, IOC)
    }
}

/// Un evento leído del anillo de eventos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub kind: u32,
    /// Código de terminación: 1 = bien, 13 = paquete corto (también bien para los datos).
    pub code: u8,
    pub slot: u8,
    pub endpoint: u8,
    /// Bytes que **faltaron** transferir (en los eventos de transferencia).
    pub residue: u32,
    /// El TRB al que se refiere (dirección física) o, en los de puerto, el número de puerto.
    pub pointer: u64,
}

pub fn parse_event(t: &Trb) -> Event {
    Event {
        kind: t.kind(),
        code: (t.0[2] >> 24) as u8,
        slot: (t.0[3] >> 24) as u8,
        endpoint: ((t.0[3] >> 16) & 0x1F) as u8,
        residue: t.0[2] & 0xFF_FFFF,
        pointer: t.0[0] as u64 | (t.0[1] as u64) << 32,
    }
}

pub const CODE_SUCCESS: u8 = 1;
pub const CODE_SHORT_PACKET: u8 = 13;

/// Velocidades de los puertos (PORTSC bits 10–13).
pub const SPEED_FULL: u8 = 1;
pub const SPEED_LOW: u8 = 2;
pub const SPEED_HIGH: u8 = 3;
pub const SPEED_SUPER: u8 = 4;

/// Tamaño inicial del paquete del endpoint 0 según la velocidad (después se corrige con el
/// descriptor del dispositivo).
pub fn default_max_packet0(speed: u8) -> u16 {
    match speed {
        SPEED_LOW | SPEED_FULL => 8,
        SPEED_HIGH => 64,
        _ => 512,
    }
}

/// El intervalo de un endpoint periódico en el formato de xHCI (2^n × 125 µs) a partir del
/// `bInterval` del descriptor, que en baja y alta velocidad es en ms y en alta/súper ya es un
/// exponente.
pub fn xhci_interval(speed: u8, b_interval: u8) -> u8 {
    match speed {
        SPEED_LOW | SPEED_FULL => {
            // ms → potencia de 2 de 125 µs: 1 ms = 8 = 2^3.
            let ms = b_interval.max(1) as u32;
            (31 - (ms * 8).leading_zeros()).clamp(3, 10) as u8
        }
        _ => b_interval.clamp(1, 16) - 1,
    }
}

/// El contexto de entrada de xHCI (lo que se le pasa a Address Device y Configure Endpoint):
/// un contexto de control, el del slot y uno por endpoint, de 32 o 64 bytes cada uno (CSZ).
pub struct InputContext {
    pub bytes: Vec<u8>,
    size: usize,
}

/// Datos del slot (el dispositivo).
#[derive(Debug, Clone, Copy)]
pub struct Slot {
    /// Camino de hubs (4 bits por nivel) hasta el dispositivo.
    pub route: u32,
    pub speed: u8,
    pub root_port: u8,
    /// Último endpoint en uso (DCI).
    pub context_entries: u8,
    /// Es un hub: cuántos puertos tiene.
    pub hub_ports: Option<u8>,
    /// Dispositivo de baja o media velocidad detrás de un hub de alta: el slot y el puerto de
    /// ese hub (su "traductor de transacciones").
    pub tt: Option<(u8, u8)>,
}

/// Tipos de endpoint en el contexto (EP Type).
pub const EP_CONTROL: u32 = 4;
pub const EP_BULK_OUT: u32 = 2;
pub const EP_BULK_IN: u32 = 6;
pub const EP_INTERRUPT_IN: u32 = 7;

impl InputContext {
    /// `csz64`: la controladora usa contextos de 64 bytes (HCCPARAMS1.CSZ).
    pub fn new(csz64: bool) -> InputContext {
        let size = if csz64 { 64 } else { 32 };
        InputContext {
            bytes: alloc::vec![0u8; size * 33],
            size,
        }
    }

    fn put(&mut self, ctx: usize, word: usize, value: u32) {
        let at = ctx * self.size + word * 4;
        self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Qué contextos se agregan (bit n = DCI n; bit 0 = el slot).
    pub fn add(&mut self, flags: u32) {
        self.put(0, 1, flags);
    }

    pub fn slot(&mut self, s: &Slot) {
        let mut w0 = s.route & 0xF_FFFF | (s.speed as u32) << 20 | (s.context_entries as u32) << 27;
        let mut w1 = (s.root_port as u32) << 16;
        let mut w2 = 0;
        if let Some(ports) = s.hub_ports {
            w0 |= 1 << 26; // es un hub
            w1 |= (ports as u32) << 24;
        }
        if let Some((hub_slot, hub_port)) = s.tt {
            w2 = hub_slot as u32 | (hub_port as u32) << 8;
        }
        self.put(1, 0, w0);
        self.put(1, 1, w1);
        self.put(1, 2, w2);
    }

    /// El contexto del endpoint `dci`: tipo, tamaño de paquete, intervalo y el anillo (con el
    /// ciclo inicial en 1). CErr = 3 reintentos.
    pub fn endpoint(&mut self, dci: u8, kind: u32, max_packet: u16, interval: u8, ring: u64) {
        let ctx = dci as usize + 1;
        self.put(ctx, 0, (interval as u32) << 16);
        self.put(ctx, 1, 3 << 1 | kind << 3 | (max_packet as u32) << 16);
        self.put(ctx, 2, (ring as u32 & !0xF) | 1);
        self.put(ctx, 3, (ring >> 32) as u32);
        // Largo promedio de TRB: 8 para control, un paquete para los demás.
        let avg = if kind == EP_CONTROL {
            8
        } else {
            max_packet as u32
        };
        let esit = if kind == EP_INTERRUPT_IN {
            max_packet as u32
        } else {
            0
        };
        self.put(ctx, 4, avg | esit << 16);
    }
}

// --- HID: teclado y mouse en "modo de arranque" -------------------------------------------------

/// El código de set 1 del PS/2 (el que entiende `keyboard.rs`) de una tecla HID. `0xE0xx`:
/// tecla extendida (con prefijo 0xE0). 0 = sin equivalente.
fn hid_to_set1(usage: u8) -> u16 {
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    match usage {
        0x04..=0x1D => LETTERS[(usage - 0x04) as usize] as u16,
        0x1E..=0x26 => (usage - 0x1E + 0x02) as u16, // 1–9
        0x27 => 0x0B,                                // 0
        0x28 => 0x1C,                                // Enter
        0x29 => 0x01,                                // Esc
        0x2A => 0x0E,                                // Retroceso
        0x2B => 0x0F,                                // Tab
        0x2C => 0x39,                                // Espacio
        0x2D => 0x0C,
        0x2E => 0x0D,
        0x2F => 0x1A,
        0x30 => 0x1B,
        0x31 | 0x32 => 0x2B,
        0x33 => 0x27,
        0x34 => 0x28,
        0x35 => 0x29,
        0x36 => 0x33,
        0x37 => 0x34,
        0x38 => 0x35,
        0x39 => 0x3A,                                // Bloq Mayús
        0x3A..=0x43 => (usage - 0x3A + 0x3B) as u16, // F1–F10
        0x44 => 0x57,                                // F11
        0x45 => 0x58,                                // F12
        0x46 => 0xE037,                              // Impr Pant
        0x47 => 0x46,                                // Bloq Despl
        0x49 => 0xE052,                              // Insert
        0x4A => 0xE047,                              // Inicio
        0x4B => 0xE049,                              // RePág
        0x4C => 0xE053,                              // Supr
        0x4D => 0xE04F,                              // Fin
        0x4E => 0xE051,                              // AvPág
        0x4F => 0xE04D,                              // →
        0x50 => 0xE04B,                              // ←
        0x51 => 0xE050,                              // ↓
        0x52 => 0xE048,                              // ↑
        0x53 => 0x45,                                // Bloq Num
        0x54 => 0xE035,                              // / del teclado numérico
        0x55 => 0x37,
        0x56 => 0x4A,
        0x57 => 0x4E,
        0x58 => 0xE01C, // Enter del teclado numérico
        0x59..=0x61 => {
            [0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49][(usage - 0x59) as usize]
        }
        0x62 => 0x52,
        0x63 => 0x53,
        0x64 => 0x56,   // < > de los teclados ISO (latinoamericano)
        0x65 => 0xE05D, // Menú
        _ => 0,
    }
}

/// Los modificadores del byte 0 del reporte, en orden de bit: Ctrl, Shift, Alt, Win izquierdos
/// y derechos.
const MODIFIERS: [u16; 8] = [0x1D, 0x2A, 0x38, 0xE05B, 0xE01D, 0x36, 0xE038, 0xE05C];

fn push_code(out: &mut Vec<u8>, code: u16, release: bool) {
    if code == 0 {
        return;
    }
    if code >> 8 == 0xE0 {
        out.push(0xE0);
    }
    out.push(code as u8 | if release { 0x80 } else { 0 });
}

/// Compara dos reportes de teclado (8 bytes: modificadores, reservado, 6 teclas) y devuelve los
/// scancodes de set 1 de lo que se apretó y se soltó, como los mandaría un teclado PS/2.
pub fn keyboard_report(prev: &[u8; 8], now: &[u8; 8]) -> Vec<u8> {
    let mut out = Vec::new();
    // Todas las teclas en 1 = "demasiadas a la vez" (rollover): se ignora el reporte.
    if now[2..].iter().all(|&k| k == 1) {
        return out;
    }
    for (bit, &code) in MODIFIERS.iter().enumerate() {
        let (was, is) = (prev[0] >> bit & 1 != 0, now[0] >> bit & 1 != 0);
        if was != is {
            push_code(&mut out, code, was);
        }
    }
    for &k in prev[2..].iter().filter(|&&k| k > 3) {
        if !now[2..].contains(&k) {
            push_code(&mut out, hid_to_set1(k), true);
        }
    }
    for &k in now[2..].iter().filter(|&&k| k > 3) {
        if !prev[2..].contains(&k) {
            push_code(&mut out, hid_to_set1(k), false);
        }
    }
    out
}

/// Un reporte de mouse (botones, dx, dy y, si viene, la rueda) como paquete PS/2 de 4 bytes
/// (el formato con rueda que entiende `MouseDecoder::with_wheel`). En USB el eje Y crece hacia
/// abajo; en PS/2, hacia arriba.
pub fn mouse_report(r: &[u8]) -> Option<[u8; 4]> {
    if r.len() < 3 {
        return None;
    }
    let buttons = r[0] & 0b111;
    let dx = r[1] as i8 as i16;
    let dy = -(r[2] as i8 as i16);
    let wheel = r.get(3).map_or(0, |&w| w as i8);
    let clamp = |v: i16| v.clamp(-255, 255);
    let (dx, dy) = (clamp(dx), clamp(dy));
    let mut flags = 0x08 | buttons;
    if dx < 0 {
        flags |= 0x10;
    }
    if dy < 0 {
        flags |= 0x20;
    }
    // La rueda de PS/2 (IntelliMouse) cuenta al revés de la de USB.
    Some([flags, dx as u8, dy as u8, (-wheel).clamp(-8, 7) as u8])
}

// --- almacenamiento masivo: Bulk-Only Transport y SCSI -------------------------------------------

/// El "Command Block Wrapper" (31 bytes) que envuelve un comando SCSI.
pub fn cbw(tag: u32, data_len: u32, data_in: bool, lun: u8, command: &[u8]) -> [u8; 31] {
    let mut c = [0u8; 31];
    c[..4].copy_from_slice(b"USBC");
    c[4..8].copy_from_slice(&tag.to_le_bytes());
    c[8..12].copy_from_slice(&data_len.to_le_bytes());
    c[12] = if data_in { 0x80 } else { 0 };
    c[13] = lun;
    let n = command.len().min(16);
    c[14] = n as u8;
    c[15..15 + n].copy_from_slice(&command[..n]);
    c
}

/// El "Command Status Wrapper" (13 bytes): `Some(estado)` si es válido para `tag` (0 = bien).
pub fn csw(d: &[u8], tag: u32) -> Option<u8> {
    (d.len() >= 13 && &d[..4] == b"USBS" && le(d, 4, 4) as u32 == tag).then_some(d[12])
}

pub fn scsi_inquiry() -> [u8; 6] {
    [0x12, 0, 0, 0, 36, 0]
}
pub fn scsi_test_unit_ready() -> [u8; 6] {
    [0; 6]
}
pub fn scsi_request_sense() -> [u8; 6] {
    [0x03, 0, 0, 0, 18, 0]
}
pub fn scsi_read_capacity() -> [u8; 10] {
    [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

/// READ(10) o WRITE(10) de `count` bloques desde `lba` (hasta 2 TiB con bloques de 512).
pub fn scsi_rw10(write: bool, lba: u32, count: u16) -> [u8; 10] {
    let mut c = [0u8; 10];
    c[0] = if write { 0x2A } else { 0x28 };
    c[2..6].copy_from_slice(&lba.to_be_bytes());
    c[7..9].copy_from_slice(&count.to_be_bytes());
    c
}

/// La respuesta de READ CAPACITY(10): (último bloque + 1, tamaño de bloque). Big endian.
pub fn parse_capacity(d: &[u8]) -> Option<(u64, u32)> {
    if d.len() < 8 {
        return None;
    }
    let last = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as u64;
    let size = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    Some((last + 1, size))
}

/// Fabricante y modelo de la respuesta de INQUIRY.
pub fn parse_inquiry(d: &[u8]) -> Option<alloc::string::String> {
    if d.len() < 32 {
        return None;
    }
    let vendor = crate::ascii(&d[8..16]);
    let product = crate::ascii(&d[16..32]);
    Some(alloc::format!("{vendor} {product}").trim().into())
}
