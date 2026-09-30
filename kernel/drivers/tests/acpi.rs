//! Las tablas ACPI armadas a mano, byte por byte, como las deja un firmware (QEMU q35 y una PC
//! con AMD B550 tienen esta forma), leídas por el parser.

use jarvis_drivers::acpi::{
    Ecam, Fadt, Madt, PhysRead, Register, Route, Rsdp, Tables, checksum_ok, parse_hpet, parse_mcfg,
    pm_timer_delta,
};
use jarvis_drivers::apic::{
    MsiLayout, MsixTable, msi_address, msi_enable, msix_enable, msix_entry, redirection,
};

/// Una "RAM física" de 1 MiB.
struct Ram(Vec<u8>);

impl PhysRead for Ram {
    fn read(&self, phys: u64, buf: &mut [u8]) {
        let p = phys as usize;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.0.get(p + i).copied().unwrap_or(0);
        }
    }
}

impl Ram {
    fn put(&mut self, at: usize, bytes: &[u8]) {
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
    }
}

/// Completa el byte de suma de control para que todo sume 0.
fn fix_checksum(bytes: &mut [u8], at: usize) {
    bytes[at] = 0;
    let sum = bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    bytes[at] = 0u8.wrapping_sub(sum);
}

/// Una tabla con su cabecera de 36 bytes.
fn table(sig: &[u8; 4], revision: u8, body: &[u8]) -> Vec<u8> {
    let mut t = vec![0u8; 36];
    t[..4].copy_from_slice(sig);
    t[8] = revision;
    t[10..16].copy_from_slice(b"JARVIS");
    t.extend_from_slice(body);
    let len = t.len() as u32;
    t[4..8].copy_from_slice(&len.to_le_bytes());
    fix_checksum(&mut t, 9);
    t
}

fn gas(space: u8, bits: u8, addr: u64) -> [u8; 12] {
    let mut g = [0u8; 12];
    g[0] = space;
    g[1] = bits;
    g[3] = if bits == 32 { 3 } else { 2 };
    g[4..].copy_from_slice(&addr.to_le_bytes());
    g
}

fn rsdp(xsdt: u64) -> Vec<u8> {
    let mut r = vec![0u8; 36];
    r[..8].copy_from_slice(b"RSD PTR ");
    r[9..15].copy_from_slice(b"JARVIS");
    r[15] = 2;
    r[20..24].copy_from_slice(&36u32.to_le_bytes());
    r[24..32].copy_from_slice(&xsdt.to_le_bytes());
    fix_checksum(&mut r[..20], 8);
    fix_checksum(&mut r, 32);
    r
}

fn madt() -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&0xFEE0_0000u32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes()); // PCAT_COMPAT
    // Dos CPU (una apagada) y un x2APIC.
    b.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]);
    b.extend_from_slice(&[0, 8, 1, 2, 0, 0, 0, 0]);
    let mut x2 = vec![9, 16, 0, 0];
    x2.extend_from_slice(&300u32.to_le_bytes());
    x2.extend_from_slice(&1u32.to_le_bytes());
    x2.extend_from_slice(&7u32.to_le_bytes());
    b.extend_from_slice(&x2);
    // IOAPIC 0 en 0xFEC00000, GSI desde 0.
    b.extend_from_slice(&[1, 12, 0, 0]);
    b.extend_from_slice(&0xFEC0_0000u32.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    // IRQ 0 → GSI 2 (el timer, como en toda PC). IRQ 9 → GSI 9, nivel y activa en bajo (el SCI).
    b.extend_from_slice(&[2, 10, 0, 0]);
    b.extend_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    b.extend_from_slice(&[2, 10, 0, 9]);
    b.extend_from_slice(&9u32.to_le_bytes());
    b.extend_from_slice(&0b1111u16.to_le_bytes());
    // Una entrada de tipo desconocido (se saltea) y el NMI del APIC local.
    b.extend_from_slice(&[0x7F, 4, 0xAA, 0xBB]);
    b.extend_from_slice(&[4, 6, 0xFF, 5, 0, 1]);
    table(b"APIC", 4, &b)
}

fn fadt(dsdt: u64, facs: u64) -> Vec<u8> {
    let mut b = vec![0u8; 276 - 36];
    let at = |off: usize| off - 36;
    b[at(36)..at(40)].copy_from_slice(&(facs as u32).to_le_bytes());
    b[at(40)..at(44)].copy_from_slice(&(dsdt as u32).to_le_bytes());
    b[at(46)..at(48)].copy_from_slice(&9u16.to_le_bytes());
    b[at(48)..at(52)].copy_from_slice(&0xB2u32.to_le_bytes());
    b[at(52)] = 0xF1;
    b[at(56)..at(60)].copy_from_slice(&0x600u32.to_le_bytes());
    b[at(64)..at(68)].copy_from_slice(&0x604u32.to_le_bytes());
    b[at(76)..at(80)].copy_from_slice(&0x608u32.to_le_bytes());
    b[at(88)] = 4;
    b[at(108)] = 0x32;
    b[at(109)..at(111)].copy_from_slice(&0b10u16.to_le_bytes()); // hay 8042
    // Flags: TMR_VAL_EXT (8) y RESET_REG_SUP (10).
    b[at(112)..at(116)].copy_from_slice(&((1u32 << 8) | (1 << 10)).to_le_bytes());
    b[at(116)..at(128)].copy_from_slice(&gas(1, 8, 0xCF9));
    b[at(128)] = 0x06;
    // X_PM1a_CNT en memoria (como en algunas placas), pisa al puerto de 32 bits.
    b[at(172)..at(184)].copy_from_slice(&gas(0, 16, 0xFED8_0804));
    table(b"FACP", 6, &b)
}

fn mcfg() -> Vec<u8> {
    let mut b = vec![0u8; 8];
    b.extend_from_slice(&0xB000_0000u64.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0xFF, 0, 0, 0, 0]);
    table(b"MCFG", 1, &b)
}

fn hpet() -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend_from_slice(&gas(0, 64, 0xFED0_0000));
    b.extend_from_slice(&[0, 0, 0, 0]);
    table(b"HPET", 1, &b)
}

/// Arma la RAM: RSDP en 0xE0000 y las tablas arriba, como en una PC.
fn firmware() -> Ram {
    let mut ram = Ram(vec![0u8; 1 << 20]);
    let (madt_at, fadt_at, mcfg_at, hpet_at, dsdt_at, xsdt_at, broken_at) =
        (0x1000, 0x2000, 0x3000, 0x3800, 0x4000, 0x5000, 0x6000);
    ram.put(0xE0000, &rsdp(xsdt_at as u64));
    ram.put(madt_at, &madt());
    ram.put(fadt_at, &fadt(dsdt_at as u64, 0x7000));
    ram.put(mcfg_at, &mcfg());
    ram.put(hpet_at, &hpet());
    ram.put(dsdt_at, &table(b"DSDT", 2, &[0x10, 0x20, 0x30]));
    let mut broken = table(b"SSDT", 2, &[1, 2, 3]);
    broken[37] ^= 0xFF; // suma rota
    ram.put(broken_at, &broken);
    let mut ptrs = Vec::new();
    for p in [madt_at, fadt_at, mcfg_at, hpet_at, broken_at] {
        ptrs.extend_from_slice(&(p as u64).to_le_bytes());
    }
    ram.put(xsdt_at, &table(b"XSDT", 1, &ptrs));
    ram
}

#[test]
fn rsdp_valido_y_roto() {
    let r = rsdp(0x1234);
    assert_eq!(
        Rsdp::parse(&r),
        Ok(Rsdp {
            revision: 2,
            rsdt: 0,
            xsdt: Some(0x1234)
        })
    );
    let mut bad = r.clone();
    bad[30] ^= 1;
    assert!(Rsdp::parse(&bad).is_err(), "la suma extendida no da");
    let mut bad = r;
    bad[0] = b'X';
    assert!(Rsdp::parse(&bad).is_err());
    assert!(Rsdp::parse(&[0u8; 10]).is_err());
}

#[test]
fn indice_de_tablas_con_dsdt_y_sumas() {
    let ram = firmware();
    let t = Tables::read(&ram, 0xE0000).unwrap();
    let sigs: Vec<&[u8]> = t.entries.iter().map(|e| &e.signature[..]).collect();
    assert_eq!(
        sigs,
        [&b"APIC"[..], b"FACP", b"DSDT", b"MCFG", b"HPET", b"SSDT"]
    );
    assert!(t.find(b"SSDT").is_none(), "la de suma rota no se usa");
    assert_eq!(t.find(b"DSDT").unwrap().phys, 0x4000);
    assert_eq!(t.find(b"DSDT").unwrap().len, 39);
    for e in t.entries.iter().filter(|e| e.valid) {
        let mut bytes = vec![0u8; e.len as usize];
        ram.read(e.phys, &mut bytes);
        assert!(checksum_ok(&bytes));
    }
}

#[test]
fn rsdt_de_acpi_1() {
    let mut ram = firmware();
    let mut r = vec![0u8; 20];
    r[..8].copy_from_slice(b"RSD PTR ");
    r[16..20].copy_from_slice(&0x8000u32.to_le_bytes());
    fix_checksum(&mut r, 8);
    ram.put(0xE1000, &r);
    ram.put(0x8000, &table(b"RSDT", 1, &0x1000u32.to_le_bytes()));
    let t = Tables::read(&ram, 0xE1000).unwrap();
    assert_eq!(t.revision, 0);
    assert_eq!(t.entries.len(), 1);
    assert!(t.find(b"APIC").is_some());
}

#[test]
fn madt_cpus_ioapic_y_overrides() {
    let m = Madt::parse(&madt());
    assert_eq!(m.lapic_addr, 0xFEE0_0000);
    assert!(m.has_8259);
    assert_eq!(m.cpus.len(), 3);
    assert!(m.cpus[0].usable && !m.cpus[1].usable);
    assert_eq!((m.cpus[2].apic_id, m.cpus[2].uid), (300, 7));
    assert_eq!(m.ioapics.len(), 1);
    assert_eq!(m.ioapics[0].addr, 0xFEC0_0000);
    assert_eq!(
        m.isa_route(0),
        Route {
            gsi: 2,
            active_low: false,
            level: false
        }
    );
    assert_eq!(
        m.isa_route(9),
        Route {
            gsi: 9,
            active_low: true,
            level: true
        }
    );
    assert_eq!(m.isa_route(1).gsi, 1, "sin override, la misma entrada");
    assert_eq!(m.ioapic_for(2, |_| 24).map(|(_, n)| n), Some(2));
    assert_eq!(m.ioapic_for(30, |_| 24), None);
}

#[test]
fn madt_cortada_no_rompe() {
    let full = madt();
    for cut in 36..full.len() {
        let _ = Madt::parse(&full[..cut]);
    }
}

#[test]
fn fadt_registros() {
    let f = Fadt::parse(&fadt(0x4000, 0x7000));
    assert_eq!((f.dsdt, f.facs), (0x4000, 0x7000));
    assert_eq!(f.sci_irq, 9);
    assert_eq!((f.smi_cmd, f.acpi_enable), (0xB2, 0xF1));
    assert_eq!(f.pm1a_event, Some(Register::Io(0x600)));
    assert_eq!(f.pm1a_control, Some(Register::Memory(0xFED8_0804)));
    assert_eq!(f.pm1b_control, None);
    assert_eq!(f.pm_timer, Some(Register::Io(0x608)));
    assert!(f.pm_timer_32);
    assert!(f.has_8042);
    assert!(!f.hardware_reduced);
    assert_eq!(f.reset, Some(Register::Io(0xCF9)));
    assert_eq!(f.reset_value, 6);
    assert_eq!(f.century, 0x32);
    // Una FADT de ACPI 1.0 (84 bytes de cuerpo): sin campos extendidos, con 8042 supuesto.
    let short = &fadt(0x4000, 0x7000)[..116];
    let mut short = short.to_vec();
    short[8] = 1;
    let f = Fadt::parse(&short);
    assert_eq!(f.pm1a_control, Some(Register::Io(0x604)));
    assert!(f.has_8042);
    assert_eq!(f.reset, None);
}

#[test]
fn mcfg_y_hpet() {
    let e = parse_mcfg(&mcfg());
    assert_eq!(
        e,
        [Ecam {
            base: 0xB000_0000,
            segment: 0,
            bus_start: 0,
            bus_end: 0xFF
        }]
    );
    assert_eq!(e[0].address(0, 0x1F, 3, 0x40), Some(0xB00F_B040));
    assert_eq!(e[0].address(1, 0, 0, 0x100), Some(0xB010_0100));
    assert_eq!(e[0].address(0, 32, 0, 0), None);
    assert_eq!(parse_hpet(&hpet()), Some(0xFED0_0000));
}

#[test]
fn timer_pm_con_vuelta() {
    assert_eq!(pm_timer_delta(100, 400, false), 300);
    assert_eq!(pm_timer_delta(0x00FF_FFF0, 0x10, false), 0x20);
    assert_eq!(pm_timer_delta(0xFFFF_FFF0, 0x10, true), 0x20);
}

#[test]
fn ioapic_y_msi() {
    // Vector 0x21, nivel, activa en bajo, a la CPU 3.
    let r = redirection(0x21, true, true, false, 3);
    assert_eq!(r, 0x0300_0000_0000_A021);
    assert_eq!(redirection(0x30, false, false, true, 0), 0x1_0030);
    assert_eq!(msi_address(0), 0xFEE0_0000);
    assert_eq!(msi_address(2), 0xFEE0_2000);
    // MSI de 32 bits sin máscara; de 64 con máscara.
    assert_eq!(
        MsiLayout::new(0),
        MsiLayout {
            address_high: None,
            data: 8,
            mask: None
        }
    );
    assert_eq!(
        MsiLayout::new(0x180),
        MsiLayout {
            address_high: Some(8),
            data: 0xC,
            mask: Some(0x10)
        }
    );
    // Habilitar pide un solo vector aunque el dispositivo ofrezca 32.
    assert_eq!(msi_enable(0b0101_0000), 1);
    let t = MsixTable::new(0x0010, 0x2003);
    assert_eq!((t.bar, t.offset, t.entries), (3, 0x2000, 17));
    assert_eq!(msix_enable(0x4000), 0x8000);
    assert_eq!(msix_entry(1, 0x40), [0xFEE0_1000, 0, 0x40, 0]);
}
