//! Los descriptores de las placas de red, contra los ejemplos de los manuales.

use jarvis_drivers::nic::{
    R8169_EOR, R8169_FS, R8169_LS, R8169_OWN, R8169_RX_RES, RTL8139_RX_RING, e1000_rx_desc,
    e1000_rx_done, e1000_tx_desc, e1000_tx_done, r8169_rx_desc, r8169_rx_done, r8169_tx_desc,
    rtl8139_capr, rtl8139_rx,
};

#[test]
fn e1000_recepcion_y_envio() {
    let mut d = e1000_rx_desc(0x1234_5000);
    assert_eq!(&d[..8], &0x1234_5000u64.to_le_bytes());
    assert_eq!(e1000_rx_done(&d), None, "la placa todavía no la llenó");
    d[8..10].copy_from_slice(&60u16.to_le_bytes());
    d[12] = 0b11; // DD | EOP
    assert_eq!(e1000_rx_done(&d), Some(Some(60)));
    d[13] = 0x01; // error de CRC
    assert_eq!(e1000_rx_done(&d), Some(None));
    d[13] = 0;
    d[12] = 0b01; // sin EOP: vino partida
    assert_eq!(e1000_rx_done(&d), Some(None));

    let mut t = e1000_tx_desc(0x8000, 1514);
    assert_eq!(&t[8..10], &1514u16.to_le_bytes());
    assert_eq!(t[11], 0x0B);
    assert!(!e1000_tx_done(&t));
    t[12] = 1;
    assert!(e1000_tx_done(&t));
}

#[test]
fn rtl8139_anillo_con_vuelta() {
    let mut ring = vec![0u8; RTL8139_RX_RING + 1600];
    // Una trama de 64 bytes (60 + CRC) en el desplazamiento 100.
    ring[100..102].copy_from_slice(&1u16.to_le_bytes());
    ring[102..104].copy_from_slice(&64u16.to_le_bytes());
    assert_eq!(rtl8139_rx(&ring, 100), Some((104, 60, 168)));
    // Cerca del final: la próxima da la vuelta.
    let at = RTL8139_RX_RING - 40;
    ring[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
    ring[at + 2..at + 4].copy_from_slice(&100u16.to_le_bytes());
    let (_, len, next) = rtl8139_rx(&ring, at).unwrap();
    assert_eq!(len, 96);
    assert_eq!(next, (at + 104) % RTL8139_RX_RING);
    // Estado con error, o largo imposible: cabecera inválida.
    ring[100..102].copy_from_slice(&0b101u16.to_le_bytes());
    assert_eq!(rtl8139_rx(&ring, 100), None);
    ring[100..102].copy_from_slice(&1u16.to_le_bytes());
    ring[102..104].copy_from_slice(&9000u16.to_le_bytes());
    assert_eq!(rtl8139_rx(&ring, 100), None);
    assert_eq!(rtl8139_capr(168), 152);
    assert_eq!(rtl8139_capr(0), 0xFFF0);
}

#[test]
fn rtl8168_descriptores() {
    let d = r8169_rx_desc(0xABC000, 1536, true);
    let opts1 = u32::from_le_bytes(d[..4].try_into().unwrap());
    assert_eq!(opts1, R8169_OWN | R8169_EOR | 1536);
    assert_eq!(&d[8..16], &0xABC000u64.to_le_bytes());
    assert_eq!(r8169_rx_done(opts1), None, "todavía es de la placa");
    assert_eq!(r8169_rx_done(R8169_FS | R8169_LS | 64), Some(Some(60)));
    assert_eq!(
        r8169_rx_done(R8169_FS | R8169_LS | R8169_RX_RES | 64),
        Some(None)
    );
    assert_eq!(r8169_rx_done(R8169_FS | 64), Some(None), "partida");
    let t = r8169_tx_desc(0x5000, 60, false);
    let opts1 = u32::from_le_bytes(t[..4].try_into().unwrap());
    assert_eq!(opts1, R8169_OWN | R8169_FS | R8169_LS | 60);
}
