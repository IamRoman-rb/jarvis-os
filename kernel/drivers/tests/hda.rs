//! HDA sin placa: verbos, listas de conexiones y el camino de salida en un grafo como el del
//! Realtek ALC887 (el codec típico de las placas madre como la de Roman) y en el de QEMU.

use jarvis_drivers::hda::{
    SET_PIN_CONTROL, Step, Widget, WidgetType, amp_0db, amp_in_unmute, amp_out_unmute, connections,
    output_path, verb, verb16,
};

fn widget(nid: u8, kind: WidgetType, connections: &[u8]) -> Widget {
    Widget {
        nid,
        kind,
        caps: 0,
        pin_caps: 0,
        config: 0,
        connections: connections.to_vec(),
        amp_out_caps: 0,
    }
}

/// Un pin de salida: `device` 0 línea, 1 parlante, 2 auriculares; `jack` si hay conector.
fn pin(nid: u8, device: u32, jack: bool, connections: &[u8]) -> Widget {
    let mut w = widget(nid, WidgetType::Pin, connections);
    w.pin_caps = 1 << 4;
    w.config = device << 20 | if jack { 0 } else { 1 << 30 };
    w
}

#[test]
fn verbos() {
    // Codec 0, nodo 0x14, "Set Pin Widget Control" = salida.
    assert_eq!(verb(0, 0x14, SET_PIN_CONTROL, 0x40), 0x0147_0740);
    // Formato del convertidor 0x02: 48 kHz, 16 bits, estéreo.
    assert_eq!(verb16(0, 0x02, 0x2, 0x0011), 0x0022_0011);
    assert_eq!(amp_out_unmute(0x57), 0xB057);
    assert_eq!(amp_in_unmute(1, 0), 0x7100);
    assert_eq!(amp_0db(0x8005_2757), 0x57);
}

#[test]
fn lista_de_conexiones() {
    // Forma corta: 4 por respuesta.
    assert_eq!(connections(3, &[0x000E_0D0C]), [0x0C, 0x0D, 0x0E]);
    // Con rango: 0x0C, y "hasta 0x0F" (bit 7).
    assert_eq!(connections(2, &[0x0000_8F0C]), [0x0C, 0x0D, 0x0E, 0x0F]);
    // Forma larga: 2 por respuesta, de 16 bits.
    assert_eq!(connections(0x82, &[0x0003_0002]), [2, 3]);
}

/// El grafo de un ALC887: DAC 0x02–0x05, mezcladores 0x0C–0x0F (DAC + lazo analógico 0x0B),
/// salida trasera 0x14, auriculares del frente 0x1B (con selector) y un pin sin conector.
fn alc887() -> Vec<Widget> {
    vec![
        widget(0x02, WidgetType::Output, &[]),
        widget(0x03, WidgetType::Output, &[]),
        widget(0x0B, WidgetType::Mixer, &[0x18, 0x19]),
        widget(0x0C, WidgetType::Mixer, &[0x02, 0x0B]),
        widget(0x0D, WidgetType::Mixer, &[0x03, 0x0B]),
        widget(0x18, WidgetType::Pin, &[]),
        widget(0x19, WidgetType::Pin, &[]),
        pin(0x14, 0, true, &[0x0C]),
        pin(0x1B, 2, true, &[0x0C, 0x0D]),
        pin(0x15, 1, false, &[0x0D]),
    ]
}

#[test]
fn camino_en_un_alc887() {
    // Sin parlante conectado: gana auriculares, por el selector (entrada 0 = 0x0C) y el
    // mezclador (que no elige) hasta el DAC 0x02.
    assert_eq!(
        output_path(&alc887()),
        Some(vec![
            Step {
                nid: 0x1B,
                select: Some(0)
            },
            Step {
                nid: 0x0C,
                select: None
            },
            Step {
                nid: 0x02,
                select: None
            },
        ])
    );
    // Sin auriculares: la salida de línea trasera.
    let mut g = alc887();
    g.retain(|w| w.nid != 0x1B);
    let path = output_path(&g).unwrap();
    assert_eq!(path[0].nid, 0x14);
    assert_eq!(path.last().unwrap().nid, 0x02);
}

#[test]
fn camino_en_qemu_y_ciclos() {
    // hda-output de QEMU: un pin de línea conectado directo a un DAC.
    let g = vec![widget(2, WidgetType::Output, &[]), pin(3, 0, true, &[2])];
    assert_eq!(
        output_path(&g),
        Some(vec![
            Step {
                nid: 3,
                select: None
            },
            Step {
                nid: 2,
                select: None
            }
        ])
    );
    // Un ciclo entre mezcladores sin DAC: no hay camino (y no se cuelga).
    let g = vec![
        widget(0x0C, WidgetType::Mixer, &[0x0D]),
        widget(0x0D, WidgetType::Mixer, &[0x0C]),
        pin(0x14, 0, true, &[0x0C]),
    ];
    assert_eq!(output_path(&g), None);
    // Sin pines de salida: nada.
    assert_eq!(output_path(&[widget(2, WidgetType::Output, &[])]), None);
}
