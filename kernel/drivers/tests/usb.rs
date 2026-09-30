//! USB sin controladora: descriptores reales (un teclado, un pendrive), TRB, contextos, HID y
//! BOT/SCSI.

use jarvis_drivers::usb::{
    InputContext, Kind, SPEED_FULL, SPEED_HIGH, SPEED_LOW, SPEED_SUPER, Slot, Trb, cbw, csw,
    default_max_packet0, get_descriptor, keyboard_report, kind_of, mouse_report, parse_capacity,
    parse_configuration, parse_device, parse_event, parse_inquiry, scsi_rw10, setup, trb,
    xhci_interval,
};

/// El descriptor de dispositivo de un teclado USB común.
const KEYBOARD_DEVICE: [u8; 18] = [
    18, 1, 0x10, 0x01, 0, 0, 0, 8, 0x7A, 0x2A, 0x57, 0x8A, 0x00, 0x01, 1, 2, 0, 1,
];

/// Su configuración: una interfaz HID de arranque (teclado) con un endpoint de interrupción IN.
const KEYBOARD_CONFIG: [u8; 34] = [
    9, 2, 34, 0, 1, 1, 0, 0xA0, 50, // configuración
    9, 4, 0, 0, 1, 3, 1, 1, 0, // interfaz: HID, arranque, teclado
    9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0, // descriptor HID (se saltea)
    7, 5, 0x81, 3, 8, 0, 10, // endpoint 1 IN, interrupción, 8 bytes, 10 ms
];

/// Un pendrive: almacenamiento masivo BOT con dos endpoints masivos.
const STICK_CONFIG: [u8; 32] = [
    9, 2, 32, 0, 1, 1, 0, 0x80, 100, //
    9, 4, 0, 0, 2, 8, 6, 0x50, 0, //
    7, 5, 0x81, 2, 0, 2, 0, // bulk IN, 512
    7, 5, 0x02, 2, 0, 2, 0, // bulk OUT, 512
];

#[test]
fn descriptores() {
    let d = parse_device(&KEYBOARD_DEVICE).unwrap();
    assert_eq!((d.vendor, d.product), (0x2A7A, 0x8A57));
    assert_eq!(d.max_packet0, 8);
    assert_eq!(d.usb, 0x0110);
    let c = parse_configuration(&KEYBOARD_CONFIG).unwrap();
    assert_eq!(c.value, 1);
    assert_eq!(c.interfaces.len(), 1);
    let i = &c.interfaces[0];
    assert_eq!(kind_of(i), Some(Kind::Keyboard));
    assert_eq!(i.endpoints.len(), 1);
    let e = i.endpoints[0];
    assert!(e.in_ && e.kind == 3 && e.number == 1 && e.max_packet == 8 && e.interval == 10);
    assert_eq!(e.dci(), 3);
    let s = parse_configuration(&STICK_CONFIG).unwrap();
    assert_eq!(kind_of(&s.interfaces[0]), Some(Kind::Storage));
    let eps = &s.interfaces[0].endpoints;
    assert_eq!((eps[0].dci(), eps[1].dci()), (3, 4));
    assert_eq!(eps[0].max_packet, 512);
    // Cortada en cualquier lugar, no rompe.
    for n in 0..KEYBOARD_CONFIG.len() {
        let _ = parse_configuration(&KEYBOARD_CONFIG[..n]);
    }
}

#[test]
fn pedidos_de_control() {
    assert_eq!(get_descriptor(1, 18), [0x80, 6, 0, 1, 0, 0, 18, 0]);
    assert_eq!(setup(0x21, 0x0B, 0, 2, 0), [0x21, 0x0B, 0, 0, 2, 0, 0, 0]);
}

#[test]
fn trbs() {
    let s = Trb::setup_stage(get_descriptor(1, 18), Some(true));
    assert_eq!(s.kind(), trb::SETUP);
    assert_eq!(s.0[0], 0x0100_0680);
    assert_eq!(s.0[2], 8);
    assert_eq!(s.0[3] >> 16 & 3, 3, "TRT: datos de entrada");
    assert_ne!(s.0[3] & 1 << 6, 0, "IDT");
    let d = Trb::data_stage(0x1234_5000, 18, true);
    assert_eq!((d.kind(), d.0[2], d.0[3] >> 16 & 1), (trb::DATA, 18, 1));
    let st = Trb::status_stage(false);
    assert_eq!(st.kind(), trb::STATUS);
    let l = Trb::link(0x8000).with_cycle(true);
    assert!(l.cycle());
    assert_eq!(l.0[3] & 2, 2, "toggle cycle");
    let a = Trb::address_device(0x9000, 5);
    assert_eq!((a.kind(), a.0[3] >> 24), (trb::ADDRESS_DEVICE, 5));

    // Un evento de transferencia: slot 2, endpoint 3, paquete corto con 5 bytes sin usar.
    let e = Trb([
        0x5000,
        0,
        13 << 24 | 5,
        trb::TRANSFER_EVENT << 10 | 2 << 24 | 3 << 16 | 1,
    ]);
    let ev = parse_event(&e);
    assert_eq!(
        (
            ev.kind,
            ev.code,
            ev.slot,
            ev.endpoint,
            ev.residue,
            ev.pointer
        ),
        (trb::TRANSFER_EVENT, 13, 2, 3, 5, 0x5000)
    );
}

#[test]
fn contexto_de_entrada() {
    let mut ic = InputContext::new(false);
    ic.add(0b11);
    ic.slot(&Slot {
        route: 0x12,
        speed: SPEED_FULL,
        root_port: 3,
        context_entries: 1,
        hub_ports: None,
        tt: Some((4, 2)),
    });
    ic.endpoint(1, 4, 8, 0, 0xABC0);
    let w = |ctx: usize, word: usize| {
        let at = ctx * 32 + word * 4;
        u32::from_le_bytes(ic.bytes[at..at + 4].try_into().unwrap())
    };
    assert_eq!(w(0, 1), 0b11);
    assert_eq!(w(1, 0), 0x12 | 1 << 20 | 1 << 27);
    assert_eq!(w(1, 1), 3 << 16);
    assert_eq!(w(1, 2), 4 | 2 << 8);
    assert_eq!(w(2, 1), 3 << 1 | 4 << 3 | 8 << 16);
    assert_eq!(w(2, 2), 0xABC0 | 1);
    assert_eq!(w(2, 4), 8);
    // Contextos de 64 bytes: el del slot empieza en 64.
    let mut big = InputContext::new(true);
    big.add(1);
    big.slot(&Slot {
        route: 0,
        speed: SPEED_SUPER,
        root_port: 1,
        context_entries: 1,
        hub_ports: Some(4),
        tt: None,
    });
    assert_eq!(big.bytes.len(), 64 * 33);
    let w0 = u32::from_le_bytes(big.bytes[64..68].try_into().unwrap());
    assert_eq!(w0 >> 26 & 1, 1, "es un hub");
}

#[test]
fn velocidades_e_intervalos() {
    assert_eq!(default_max_packet0(SPEED_LOW), 8);
    assert_eq!(default_max_packet0(SPEED_HIGH), 64);
    assert_eq!(default_max_packet0(SPEED_SUPER), 512);
    // 10 ms en media velocidad → 2^6 × 125 µs = 8 ms (lo más cercano sin pasarse).
    assert_eq!(xhci_interval(SPEED_FULL, 10), 6);
    assert_eq!(xhci_interval(SPEED_LOW, 1), 3);
    // En alta velocidad bInterval ya es un exponente (4 → 2^3 microframes).
    assert_eq!(xhci_interval(SPEED_HIGH, 4), 3);
}

#[test]
fn teclado_a_set_1() {
    let none = [0u8; 8];
    // Apretar "a" (HID 0x04) → 0x1E; soltarla → 0x9E.
    let a = [0, 0, 4, 0, 0, 0, 0, 0];
    assert_eq!(keyboard_report(&none, &a), [0x1E]);
    assert_eq!(keyboard_report(&a, &none), [0x9E]);
    // Shift izquierdo + "1" → 0x2A, 0x02.
    let shift_1 = [0x02, 0, 0x1E, 0, 0, 0, 0, 0];
    assert_eq!(keyboard_report(&none, &shift_1), [0x2A, 0x02]);
    // AltGr (Alt derecho) y flecha arriba: con prefijo 0xE0.
    let altgr_up = [0x40, 0, 0x52, 0, 0, 0, 0, 0];
    assert_eq!(keyboard_report(&none, &altgr_up), [0xE0, 0x38, 0xE0, 0x48]);
    assert_eq!(keyboard_report(&altgr_up, &none), [0xE0, 0xB8, 0xE0, 0xC8]);
    // La tecla < > de los teclados ISO (latinoamericanos).
    assert_eq!(keyboard_report(&none, &[0, 0, 0x64, 0, 0, 0, 0, 0]), [0x56]);
    // Rollover: se ignora.
    assert!(keyboard_report(&a, &[0, 0, 1, 1, 1, 1, 1, 1]).is_empty());
    // Una tecla que sigue apretada no se repite.
    let ab = [0, 0, 4, 5, 0, 0, 0, 0];
    assert_eq!(keyboard_report(&a, &ab), [0x30]);
}

#[test]
fn mouse_a_ps2() {
    // Botón izquierdo, 5 a la derecha, 3 hacia abajo (USB) = −3 en PS/2, rueda hacia arriba.
    assert_eq!(
        mouse_report(&[1, 5, 3, 1]),
        Some([0x08 | 1 | 0x20, 5, 253, 0xFF])
    );
    assert_eq!(
        mouse_report(&[0, 0xFB, 0xFE]),
        Some([0x08 | 0x10, 0xFB, 2, 0])
    );
    assert_eq!(mouse_report(&[0, 1]), None);
}

#[test]
fn bot_y_scsi() {
    let c = cbw(7, 512, true, 0, &scsi_rw10(false, 0x0102_0304, 1));
    assert_eq!(&c[..4], b"USBC");
    assert_eq!(&c[4..8], &7u32.to_le_bytes());
    assert_eq!(&c[8..12], &512u32.to_le_bytes());
    assert_eq!((c[12], c[14]), (0x80, 10));
    assert_eq!(&c[15..25], &[0x28, 0, 1, 2, 3, 4, 0, 0, 1, 0]);
    let mut s = [0u8; 13];
    s[..4].copy_from_slice(b"USBS");
    s[4..8].copy_from_slice(&7u32.to_le_bytes());
    assert_eq!(csw(&s, 7), Some(0));
    assert_eq!(csw(&s, 8), None, "otro tag");
    s[12] = 1;
    assert_eq!(csw(&s, 7), Some(1));
    assert_eq!(
        parse_capacity(&[0, 0x0E, 0xFF, 0xFF, 0, 0, 2, 0]),
        Some((0x000F_0000, 512))
    );
    let mut inq = [0u8; 36];
    inq[8..16].copy_from_slice(b"Kingston");
    inq[16..32].copy_from_slice(b"DataTraveler 3.0");
    assert_eq!(
        parse_inquiry(&inq).as_deref(),
        Some("Kingston DataTraveler 3.0")
    );
}
