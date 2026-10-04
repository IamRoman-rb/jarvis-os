//! La RTL8821CE (K14) contra una placa simulada: los registros son memoria, y lo que hace el
//! hardware (bits que se borran solos, el DMA interno, el efuse, la radio por SIPI, el firmware
//! que arranca) lo hace el simulador. Así se prueba el orden y el formato de lo que el driver le
//! pide a la placa sin tenerla (QEMU no emula ninguna placa Wi-Fi).

use std::collections::HashMap;

use jarvis_drivers::rtw88::chip::{Link, Rtw8821c, rate_mask};
use jarvis_drivers::rtw88::desc::{self, TX_DESC_SIZE};
use jarvis_drivers::rtw88::phy::{self, Cond};
use jarvis_drivers::rtw88::{Bus, Error, Queue, efuse, fw, pwrseq, regs};

const MAC: [u8; 6] = [0x5c, 0xea, 0x1d, 0x12, 0x34, 0x56];

#[derive(Default)]
struct Placa {
    mem: HashMap<u32, u8>,
    rf: HashMap<u32, u32>,
    lte: HashMap<u16, u32>,
    efuse: Vec<u8>,
    tx: Vec<(Queue, Vec<u8>)>,
    /// (origen, destino, largo) de cada copia del DMA interno.
    ddma: Vec<(u32, u32, u32)>,
    /// (buzón, palabra 0, palabra 1) de cada H2C de 8 bytes.
    h2c: Vec<(usize, u32, u32)>,
    hci_setups: usize,
    power_offs: usize,
    fw_copied: bool,
    us: u64,
    /// La cola de beacons no avisa que llegó (para probar el error).
    bcn_broken: bool,
}

impl Placa {
    fn new() -> Placa {
        let mut p = Placa::default();
        p.put8(regs::CR, 0xea); // apagada
        p.put8(0x0006, 0x02); // la espera de la secuencia de encendido
        p.efuse = efuse_image();
        p
    }

    fn get8(&self, a: u32) -> u8 {
        self.mem.get(&a).copied().unwrap_or(0)
    }

    fn put8(&mut self, a: u32, v: u8) {
        self.mem.insert(a, v);
    }

    fn get32(&self, a: u32) -> u32 {
        u32::from_le_bytes([
            self.get8(a),
            self.get8(a + 1),
            self.get8(a + 2),
            self.get8(a + 3),
        ])
    }

    fn put32(&mut self, a: u32, v: u32) {
        for (i, b) in v.to_le_bytes().iter().enumerate() {
            self.put8(a + i as u32, *b);
        }
    }

    /// Lo que hace la placa cuando se escribe en `a` (rango `a..a+n`).
    fn after_write(&mut self, a: u32, n: u32) {
        let touches = |r: u32| a <= r && r < a + n;
        // Secuencia de encendido: el bit 0 de 0x05 se borra solo (y la placa queda activa); el
        // bit 1, al apagar.
        if touches(0x0005) {
            let v = self.get8(0x0005);
            if v & 1 != 0 {
                self.put8(0x0005, v & !1);
                self.put8(regs::CR, 0);
            }
            if v & 2 != 0 {
                self.put8(0x0005, v & !2);
                self.put8(regs::CR, 0xea);
                self.power_offs += 1;
            }
        }
        // Efuse: pedir un byte lo deja listo enseguida.
        if touches(regs::EFUSE_CTRL) {
            let v = self.get32(regs::EFUSE_CTRL);
            let addr = ((v >> 8) & 0x3ff) as usize;
            let data = self.efuse.get(addr).copied().unwrap_or(0xff);
            self.put32(
                regs::EFUSE_CTRL,
                (v & !0xff) | u32::from(data) | regs::EF_FLAG,
            );
        }
        // BCN_VALID se borra escribiendo 1 (la placa lo pone cuando llega la página).
        if touches(regs::FIFOPAGE_CTRL_2 + 1) && self.get8(regs::FIFOPAGE_CTRL_2 + 1) & 0x80 != 0 {
            let v = self.get8(regs::FIFOPAGE_CTRL_2 + 1);
            self.put8(regs::FIFOPAGE_CTRL_2 + 1, v & 0x7f);
        }
        // El DMA interno: copia y termina.
        if touches(regs::DDMA_CH0CTRL + 3) {
            let ctrl = self.get32(regs::DDMA_CH0CTRL);
            if ctrl & regs::DDMACH0_OWN != 0 {
                self.ddma.push((
                    self.get32(regs::DDMA_CH0SA),
                    self.get32(regs::DDMA_CH0DA),
                    ctrl & regs::DDMACH0_DLEN,
                ));
                self.put32(regs::DDMA_CH0CTRL, ctrl & !regs::DDMACH0_OWN);
                self.fw_copied = true;
            }
        }
        // La lista de páginas se arma sola.
        if touches(regs::AUTO_LLT_V1) {
            let v = self.get8(regs::AUTO_LLT_V1);
            self.put8(regs::AUTO_LLT_V1, v & !1);
        }
        // El procesador de la placa arranca si le copiaron el firmware.
        if touches(regs::SYS_FUNC_EN + 1)
            && self.get8(regs::SYS_FUNC_EN + 1) & regs::FEN_CPUEN != 0
            && self.fw_copied
        {
            let v = self.get8(regs::MCUFW_CTRL + 1);
            self.put8(regs::MCUFW_CTRL + 1, v | 0x80); // FW_INIT_RDY
        }
        // El coexistidor: registros indirectos.
        if touches(regs::LTECOEX_CTRL + 3) {
            let c = self.get32(regs::LTECOEX_CTRL);
            let off = c as u16;
            if c >> 28 == 0xc {
                let v = self.get32(regs::LTECOEX_WDATA);
                self.lte.insert(off, v);
            } else if c >> 28 == 0x8 {
                let v = self.lte.get(&off).copied().unwrap_or(0);
                self.put32(regs::LTECOEX_RDATA, v);
            }
        }
        // Los buzones H2C.
        for b in 0..4 {
            if touches(regs::HMEBOX[b] + 3) {
                let w0 = self.get32(regs::HMEBOX[b]);
                let w1 = self.get32(regs::HMEBOX_EX[b]);
                self.h2c.push((b, w0, w1));
            }
        }
        // La radio por SIPI.
        if touches(regs::RF_SIPI_A + 3) {
            let v = self.get32(regs::RF_SIPI_A);
            self.rf.insert((v >> 20) & 0xff, v & 0xfffff);
        }
    }
}

impl Bus for Placa {
    fn read8(&mut self, a: u32) -> u8 {
        self.read32(a & !3).to_le_bytes()[(a & 3) as usize]
    }
    fn read16(&mut self, a: u32) -> u16 {
        u16::from_le_bytes([self.read8(a), self.read8(a + 1)])
    }
    fn read32(&mut self, a: u32) -> u32 {
        if (regs::RF_BASE_A..regs::RF_BASE_A + 0x400).contains(&a) {
            return self
                .rf
                .get(&((a - regs::RF_BASE_A) >> 2))
                .copied()
                .unwrap_or(0);
        }
        if a == regs::LTECOEX_CTRL {
            return self.get32(a) | regs::LTECOEX_READY;
        }
        self.get32(a)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.put8(a, v);
        self.after_write(a, 1);
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.put8(a, v as u8);
        self.put8(a + 1, (v >> 8) as u8);
        self.after_write(a, 2);
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.put32(a, v);
        self.after_write(a, 4);
    }
    fn delay_us(&mut self, us: u32) {
        self.us += u64::from(us);
    }
    fn tx(&mut self, q: Queue, packet: &[u8]) -> bool {
        self.tx.push((q, packet.to_vec()));
        if q == Queue::Bcn && !self.bcn_broken {
            let v = self.get8(regs::FIFOPAGE_CTRL_2 + 1);
            self.put8(regs::FIFOPAGE_CTRL_2 + 1, v | 0x80);
        }
        // Un IQK: la radio avisa que terminó.
        if q == Queue::H2c && packet[TX_DESC_SIZE + 2] == 0x0e {
            self.rf.insert(regs::RF_DTXLOK, 0xabcde);
        }
        true
    }
    fn hci_setup(&mut self) {
        self.hci_setups += 1;
    }
}

/// Un efuse físico: el bloque 1 (bytes lógicos 0x08–0x0F) con encabezado de un byte, y la MAC
/// (0xD0, bloque 26) y el tipo de antena (0xCA, bloque 25) con encabezado de dos.
fn efuse_image() -> Vec<u8> {
    let mut e = Vec::new();
    // Bloque 1, solo la palabra 1 (bytes 0x0A, 0x0B): word_en = 1101.
    e.extend_from_slice(&[0x1d, 0xaa, 0xbb]);
    // Potencia 2,4 GHz: bloque 2 (0x10–0x17) entero.
    e.extend_from_slice(&[0x20, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f, 0x30, 0x31]);
    // Bloque 25 (0xC8–0xCF), palabra 1 (0xCA, 0xCB): rfe 2, país 'A'.
    e.extend_from_slice(&[(1 << 5) | 0x0f, (3 << 4) | 0x0d, 0x02, b'A']);
    // Bloque 26 (0xD0–0xD7), palabras 0–2: la MAC.
    e.extend_from_slice(&[(2 << 5) | 0x0f, (3 << 4) | 0x08]);
    e.extend_from_slice(&MAC);
    e.resize(512, 0xff);
    e
}

/// Un firmware de mentira con la cabecera de verdad: DMEM de 5000 bytes (dos pedazos), IMEM de
/// 3000 y sin EMEM.
fn fake_fw() -> Vec<u8> {
    let mut f = vec![0u8; fw::HEADER];
    f[0..2].copy_from_slice(&0x8821u16.to_le_bytes());
    f[4..6].copy_from_slice(&24u16.to_le_bytes());
    f[6] = 11;
    f[0x20..0x24].copy_from_slice(&0x8020_0000u32.to_le_bytes());
    f[0x24..0x28].copy_from_slice(&5000u32.to_le_bytes());
    f[0x30..0x34].copy_from_slice(&3000u32.to_le_bytes());
    f[0x3c..0x40].copy_from_slice(&0x8003_0000u32.to_le_bytes());
    f.extend((0..5008 + 3008).map(|i| (i % 251) as u8));
    f
}

#[test]
fn el_efuse_logico_y_lo_que_dice() {
    let log = efuse::logical_map(&efuse_image()).unwrap();
    assert_eq!(&log[0x08..0x0c], &[0xff, 0xff, 0xaa, 0xbb]);
    assert_eq!(log[0x10], 0x2a);
    assert_eq!(&log[0xd0..0xd6], &MAC);
    let e = efuse::parse(&log);
    assert_eq!(e.mac, MAC);
    assert!(e.mac_valid());
    assert_eq!(e.rfe_option, 2);
    assert_eq!(e.country[0], b'A');
    assert_eq!(e.power_2g.cck_base[0], 0x2a);
    assert_eq!(e.crystal_cap, 0, "sin escribir (0xFF) queda en 0");
    // Un bloque que se sale del mapa es un efuse roto.
    let mut bad = vec![0x0fu8, 0xf0]; // bloque 0x78 (> 63)
    bad.extend_from_slice(&[1, 2]);
    bad.resize(512, 0xff);
    assert_eq!(efuse::logical_map(&bad), Err(Error::Efuse));
}

#[test]
fn la_cabecera_del_firmware() {
    let f = fake_fw();
    let h = fw::parse(&f).unwrap();
    assert_eq!((h.version, h.sub_version), (24, 11));
    assert_eq!((h.dmem_addr, h.dmem_size), (0x0020_0000, 5008));
    assert_eq!(
        (h.imem_addr, h.imem_size, h.emem_size),
        (0x0003_0000, 3008, 0)
    );
    assert!(fw::parse(&f[..f.len() - 1]).is_err(), "cortado");
    let mut otro = f.clone();
    otro[0] = 0x22;
    assert!(fw::parse(&otro).is_err(), "de otro chip");
}

#[test]
fn las_condiciones_de_las_tablas() {
    let cond = Cond {
        rfe: 2,
        intf: 1,
        pkg: 15,
        cut: 15,
    };
    // if rfe == 1 { A } elif rfe == 2 { B } else { C } endif, D
    let table = [
        0x8000_0001,
        0,
        0x4000_0000,
        0,
        0x100,
        0xa, //
        0x9000_0002,
        0,
        0x4000_0000,
        0,
        0x100,
        0xb, //
        0xa000_0000,
        0,
        0x100,
        0xc, //
        0xb000_0000,
        0,
        0x200,
        0xd,
    ];
    let mut out = Vec::new();
    phy::load_table(&table, &cond, |a, d| out.push((a, d)));
    assert_eq!(out, [(0x100, 0xb), (0x200, 0xd)]);
    let otra = Cond { rfe: 7, ..cond };
    out.clear();
    phy::load_table(&table, &otra, |a, d| out.push((a, d)));
    assert_eq!(out, [(0x100, 0xc), (0x200, 0xd)]);
}

#[test]
fn arranca_carga_el_firmware_y_configura_todo() {
    let mut p = Placa::new();
    let chip = Rtw8821c::start(&mut p, &fake_fw()).unwrap();
    assert_eq!(chip.mac, MAC);
    assert_eq!(chip.fw_version, (24, 11, 0));
    assert_eq!(p.power_offs, 0, "estaba apagada: no hizo falta apagarla");
    // El firmware: DMEM en dos pedazos (4096 + 912) y la IMEM en uno, cada uno copiado desde la
    // página reservada (salteando el descriptor) a su lugar.
    let src = regs::OCP_TXBUF + TX_DESC_SIZE as u32;
    assert_eq!(
        p.ddma,
        [
            (src, 0x0020_0000, 4096),
            (src, 0x0020_1000, 912),
            (src, 0x0003_0000, 3008),
        ]
    );
    let pages: Vec<_> = p.tx.iter().filter(|(q, _)| *q == Queue::Bcn).collect();
    assert_eq!(pages.len(), 3);
    let first = &pages[0].1;
    assert_eq!(first[5] & 0x1f, desc::QSEL_BEACON, "cola de beacons");
    assert_eq!(
        &first[TX_DESC_SIZE..],
        &fake_fw()[fw::HEADER..fw::HEADER + 4096]
    );
    assert!(
        p.hci_setups >= 2,
        "los anillos se rearman después del firmware"
    );
    // Lo primero que se le dice al firmware: recuperar el Bluetooth (0xD1).
    assert_eq!(p.h2c[0].1 & 0xff, 0xd1);
    // Los paquetes H2C: información general (0x0D) y de la PHY (0x11), en secuencia.
    let pkts: Vec<_> = p.tx.iter().filter(|(q, _)| *q == Queue::H2c).collect();
    assert_eq!(pkts[0].1[TX_DESC_SIZE + 2], 0x0d);
    assert_eq!(
        pkts[0].1[TX_DESC_SIZE + 10],
        48,
        "buffer de TX del firmware"
    );
    assert_eq!(pkts[1].1[TX_DESC_SIZE + 2], 0x11);
    assert_eq!(pkts[1].1[TX_DESC_SIZE + 6], 1, "segundo paquete");
    // La MAC quedó en el puerto 0 y la tabla de la MAC se aplicó (0x428 = 0x0A).
    assert_eq!(
        (0..6).map(|i| p.get8(regs::MACID + i)).collect::<Vec<_>>(),
        MAC
    );
    assert_eq!(p.get8(0x428), 0x0a);
    // El Bluetooth fuera: GNT_BT bajo (1) y GNT_WL alto (3) por software.
    assert_eq!(p.lte[&0x38] & 0xff00, 0x7700);
    // Arrancó en el canal 1 de 2,4 GHz: la radio en 20 MHz y canal 1.
    assert_eq!(chip.channel(), 1);
    assert_eq!(p.rf[&regs::RF_CHANNEL] & 0xff, 1);
    assert_eq!(p.rf[&regs::RF_CHANNEL] & (1 << 16), 0);
    // Filtro de recepción: solo los beacons de "nuestra" red, más el estado de la PHY.
    let rcr = p.get32(regs::RCR);
    assert_ne!(rcr & regs::RCR_CBSSID_BCN, 0);
    assert_ne!(rcr & regs::RCR_APP_PHYSTS, 0);
}

#[test]
fn si_estaba_encendida_la_apaga_primero() {
    let mut p = Placa::new();
    p.put8(regs::CR, 0); // un arranque anterior la dejó encendida
    pwrseq::power_on(&mut p).unwrap();
    assert_eq!(p.power_offs, 1);
    assert!(pwrseq::powered(&mut p));
}

#[test]
fn sin_respuesta_de_la_cola_de_beacons_el_firmware_falla() {
    let mut p = Placa::new();
    p.bcn_broken = true;
    assert!(matches!(
        Rtw8821c::start(&mut p, &fake_fw()),
        Err(Error::Firmware(_))
    ));
}

#[test]
fn canales_conexion_y_tramas() {
    let mut p = Placa::new();
    let mut chip = Rtw8821c::start(&mut p, &fake_fw()).unwrap();
    // 5 GHz, canal 36.
    chip.set_scanning(&mut p, true);
    assert_eq!(
        p.get32(regs::RCR) & regs::RCR_CBSSID_BCN,
        0,
        "buscando: todos los beacons"
    );
    chip.set_channel(&mut p, 36);
    let rf18 = p.rf[&regs::RF_CHANNEL];
    assert_eq!(rf18 & 0xff, 36);
    assert_ne!(rf18 & (1 << 16), 0, "banda de 5 GHz");
    // Una trama de gestión: cola de gestión, 6 Mb/s.
    let probe = [0x40u8, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    assert!(chip.send(&mut p, &probe));
    let (q, pkt) = p.tx.last().unwrap();
    assert_eq!(*q, Queue::Mgmt);
    assert_eq!(pkt[5] & 0x1f, desc::QSEL_MGMT);
    assert_eq!(pkt[16] & 0x7f, desc::RATE_6M);
    assert_ne!(pkt[3] & 1, 0, "difusión");
    // Calibrar y asociarse.
    assert!(chip.calibrate(&mut p));
    chip.set_bssid(&mut p, &[2, 0xaa, 0xbb, 0xcc, 0xdd, 1]);
    let link = Link {
        legacy: 0xff0,
        ht: true,
        vht: true,
    };
    chip.set_associated(&mut p, Some(3), &link);
    assert_eq!(p.get32(regs::CR) >> 16 & 3, 2, "conectada");
    assert_eq!(p.get32(regs::AID) & 0x7ff, 3);
    let last = p.h2c.len();
    let (_, media, _) = p.h2c[last - 2];
    let (_, ra, mask) = p.h2c[last - 1];
    assert_eq!(media & 0x1ff, 0x101, "conectado");
    assert_eq!(ra & 0xff, 0x40);
    assert_eq!(ra >> 16 & 0x1f, 10, "802.11ac en 5 GHz");
    assert_eq!(mask, 0x3ff000 | 0x10);
    // Los datos ya los elige el firmware.
    let data = [0x08u8, 0x01, 0, 0, 2, 0xaa, 0xbb, 0xcc, 0xdd, 1];
    assert!(chip.send(&mut p, &data));
    let (q, pkt) = p.tx.last().unwrap();
    assert_eq!(*q, Queue::Be);
    assert_eq!(pkt[13] & 1, 0, "sin velocidad fija");
    chip.set_associated(&mut p, None, &link);
    assert_eq!(p.get32(regs::CR) >> 16 & 3, 0);
}

#[test]
fn la_recepcion() {
    let mut p = Placa::new();
    let chip = Rtw8821c::start(&mut p, &fake_fw()).unwrap();
    // Descriptor + estado de la PHY (página 1: -40 dBm) + trama de 30 bytes + CRC.
    let mut buf = vec![0u8; desc::RX_DESC_SIZE + 32 + 34];
    let w0: u32 = 34 | (4 << 16) | (1 << 26);
    buf[0..4].copy_from_slice(&w0.to_le_bytes());
    buf[24] = 1; // página 1
    buf[25] = 70; // PWDB: 70 - 110 = -40
    buf[56] = 0x80; // un beacon
    let (frame, rssi) = chip.receive(&buf).unwrap();
    assert_eq!(frame.len(), 30);
    assert_eq!(frame[0], 0x80);
    assert_eq!(rssi, Some(-40));
    // Con error de CRC se descarta; un mensaje del firmware no es una trama.
    let mut bad = buf.clone();
    bad[1] |= 0x40;
    assert!(chip.receive(&bad).is_none());
    let mut c2h = buf.clone();
    c2h[11] |= 0x10;
    assert!(chip.receive(&c2h).is_none());
}

#[test]
fn las_velocidades_segun_la_red() {
    let b = Link {
        legacy: 0x00f,
        ..Link::default()
    };
    assert_eq!(rate_mask(&b, false), (8, 0x00f), "802.11b");
    let g = Link {
        legacy: 0xfff,
        ..Link::default()
    };
    assert_eq!(rate_mask(&g, false), (6, 0xff5), "802.11g");
    let n = Link {
        legacy: 0xfff,
        ht: true,
        vht: false,
    };
    assert_eq!(rate_mask(&n, false), (3, 0xff015), "802.11n en 2,4 GHz");
    assert_eq!(rate_mask(&n, true), (5, 0xff030), "802.11n en 5 GHz");
}

#[test]
fn la_potencia_sale_de_la_calibracion() {
    let mut log = efuse::logical_map(&efuse_image()).unwrap();
    log[0x10 + 6] = 0x28; // base de 40 MHz del grupo 0
    log[0x10 + 11] = 0xfe; // OFDM -2, 20 MHz -1
    let e = efuse::parse(&log);
    assert_eq!(phy::power_index(&e, 1, desc::RATE_1M), 0x2a, "CCK: la base");
    assert_eq!(
        phy::power_index(&e, 1, desc::RATE_6M),
        0x26,
        "OFDM: base - 2"
    );
    assert_eq!(phy::power_index(&e, 1, 0x0c), 0x27, "HT: base - 1");
    // Sin calibrar (0xFF) en 5 GHz: un valor medio.
    assert_eq!(phy::power_index(&e, 36, desc::RATE_6M), 0x20);
}

#[test]
fn los_buffer_descriptors_de_la_pci() {
    let bd = desc::tx_bd(0x1234_0000, TX_DESC_SIZE + 300, false);
    assert_eq!(u16::from_le_bytes([bd[0], bd[1]]), 48);
    assert_eq!(
        u16::from_le_bytes([bd[2], bd[3]]),
        3,
        "348 bytes = 3 bloques de 128"
    );
    assert_eq!(
        u32::from_le_bytes([bd[4], bd[5], bd[6], bd[7]]),
        0x1234_0000
    );
    assert_eq!(u16::from_le_bytes([bd[8], bd[9]]), 300);
    assert_eq!(
        u32::from_le_bytes([bd[12], bd[13], bd[14], bd[15]]),
        0x1234_0030
    );
    let bcn = desc::tx_bd(0x1000, TX_DESC_SIZE + 10, true);
    assert_ne!(bcn[3] & 0x80, 0, "OWN en la de beacons");
    let rx = desc::rx_bd(0x8000);
    assert_eq!(
        usize::from(u16::from_le_bytes([rx[0], rx[1]])),
        desc::RX_BUF_SIZE
    );
}
