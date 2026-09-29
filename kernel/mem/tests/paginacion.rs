//! La paginación propia sobre una RAM de mentira: marcos, mapear/traducir/recorrer, páginas
//! grandes, W^X y los segmentos de un ELF.

use std::collections::HashMap;

use jarvis_mem::PAGE;
use jarvis_mem::elf::{Segment, load_segments, tighten};
use jarvis_mem::frames::FrameAllocator;
use jarvis_mem::table::{MapError, PageTable, PhysMem, Size, canonical, flags, index};

/// Solo guarda las entradas que alguien escribió (el resto vale 0, como una tabla recién borrada).
#[derive(Default)]
struct Ram(HashMap<u64, u64>);

impl PhysMem for Ram {
    fn read(&self, phys: u64) -> u64 {
        assert_eq!(phys % 8, 0, "entrada desalineada");
        self.0.get(&phys).copied().unwrap_or(0)
    }
    fn write(&mut self, phys: u64, value: u64) {
        self.0.insert(phys, value);
    }
}

const MIB: u64 = 1024 * 1024;

fn frames() -> FrameAllocator {
    let mut f = FrameAllocator::new(64 * MIB);
    f.add_free(MIB, 64 * MIB);
    f
}

#[test]
fn marcos_solo_de_la_ram_usable() {
    let mut f = FrameAllocator::new(16 * MIB);
    assert_eq!(f.alloc(), None, "empieza todo ocupado");
    // Una región que no empieza alineada pierde el pedazo del principio.
    f.add_free(0x1000 + 10, 0x5000);
    assert_eq!(f.free_frames(), 3);
    f.reserve(0x3000, 0x3001);
    assert_eq!(f.free_frames(), 2);
    let a = f.alloc().unwrap();
    let b = f.alloc().unwrap();
    assert_eq!((a, b), (0x2000, 0x4000));
    assert_eq!(f.alloc(), None);
    f.free(a);
    assert!(f.is_free(a));
    assert_eq!(f.alloc(), Some(a), "lo liberado se vuelve a usar");
    f.free(0x2001); // desalineado: se ignora
    assert_eq!(f.free_frames(), 0);
}

#[test]
fn marcos_contiguos() {
    let mut f = FrameAllocator::new(MIB);
    f.add_free(0, 0x3000);
    f.add_free(0x5000, 0x9000);
    assert_eq!(f.alloc_contiguous(4), Some(0x5000));
    assert_eq!(f.alloc_contiguous(4), None);
    assert_eq!(f.alloc_contiguous(3), Some(0));
}

#[test]
fn indices_y_forma_canonica() {
    let v = 0xFFFF_8000_0020_3000u64;
    assert_eq!(index(v, 4), 256);
    assert_eq!(index(v, 3), 0);
    assert_eq!(index(v, 2), 1);
    assert_eq!(index(v, 1), 3);
    assert_eq!(canonical(0x0000_8000_0000_0000), 0xFFFF_8000_0000_0000);
    assert_eq!(canonical(0x0000_7FFF_FFFF_F000), 0x0000_7FFF_FFFF_F000);
}

#[test]
fn mapear_traducir_y_desmapear() {
    let mut ram = Ram::default();
    let mut f = frames();
    let t = PageTable::new(&mut ram, &mut f).unwrap();
    let v = 0xFFFF_8000_1234_5000;
    t.map(
        &mut ram,
        &mut f,
        v,
        0x70_0000,
        Size::Small,
        flags::WRITABLE | flags::NO_EXECUTE,
    )
    .unwrap();
    assert_eq!(t.translate(&ram, v + 0x123), Some(0x70_0123));
    assert_eq!(t.translate(&ram, v + PAGE), None);
    let leaf = t.leaf(&ram, v).unwrap();
    assert_eq!(
        leaf.flags,
        flags::PRESENT | flags::WRITABLE | flags::NO_EXECUTE
    );
    assert_eq!(t.tables(&ram), 4, "PML4 + PDPT + PD + PT");
    assert_eq!(
        t.map(&mut ram, &mut f, v, 0x80_0000, Size::Small, 0),
        Err(MapError::AlreadyMapped)
    );
    assert_eq!(
        t.map(&mut ram, &mut f, v + 1, 0x80_0000, Size::Small, 0),
        Err(MapError::Unaligned)
    );
    assert_eq!(t.unmap(&mut ram, v).map(|l| l.phys), Some(0x70_0000));
    assert_eq!(t.translate(&ram, v), None);
}

#[test]
fn paginas_grandes_y_recorrido() {
    let mut ram = Ram::default();
    let mut f = frames();
    let t = PageTable::new(&mut ram, &mut f).unwrap();
    let off = 0xFFFF_9000_0000_0000u64;
    // El mapeo de 4 GiB de RAM con páginas de 1 GiB: una sola tabla además de la PML4.
    for g in 0..4 {
        let gib = Size::Huge.bytes();
        t.map(
            &mut ram,
            &mut f,
            off + g * gib,
            g * gib,
            Size::Huge,
            flags::WRITABLE,
        )
        .unwrap();
    }
    assert_eq!(t.tables(&ram), 2);
    assert_eq!(t.translate(&ram, off + 0xA345_6789), Some(0xA345_6789));
    t.map(&mut ram, &mut f, 0x20_0000, 0x40_0000, Size::Large, 0)
        .unwrap();
    assert_eq!(
        t.map(&mut ram, &mut f, 0x20_1000, 0, Size::Small, 0),
        Err(MapError::AlreadyMapped),
        "una página de 2 MiB ya cubre esa dirección"
    );
    let mut leaves = Vec::new();
    t.walk(&ram, &mut |l| leaves.push((l.virt, l.size)));
    assert_eq!(leaves.len(), 5);
    assert_eq!(leaves[0], (0x20_0000, Size::Large));
    assert_eq!(
        leaves[1],
        (off, Size::Huge),
        "las direcciones altas salen canónicas"
    );
}

#[test]
fn copiar_una_jerarquia_da_las_mismas_traducciones() {
    // Lo que hace el kernel al arrancar: recorre las tablas del bootloader y las rehace.
    let mut ram = Ram::default();
    let mut f = frames();
    let old = PageTable::new(&mut ram, &mut f).unwrap();
    for i in 0..40u64 {
        old.map(
            &mut ram,
            &mut f,
            0xFFFF_8000_0000_0000 + i * PAGE,
            0x100_0000 + i * PAGE,
            Size::Small,
            flags::WRITABLE,
        )
        .unwrap();
    }
    old.map(&mut ram, &mut f, 0x4000_0000, 0x4000_0000, Size::Huge, 0)
        .unwrap();
    let new = PageTable::new(&mut ram, &mut f).unwrap();
    let mut leaves = Vec::new();
    old.walk(&ram, &mut |l| leaves.push(l));
    for l in &leaves {
        new.map(&mut ram, &mut f, l.virt, l.phys, l.size, l.flags)
            .unwrap();
    }
    for v in [
        0xFFFF_8000_0000_0000u64,
        0xFFFF_8000_0002_7ABC,
        0x4000_0000 + 12345,
    ] {
        assert_eq!(new.translate(&ram, v), old.translate(&ram, v));
    }
    assert_ne!(new.root, old.root);
}

#[test]
fn w_xor_x_para_el_kernel() {
    let code = Segment {
        start: 0x1000,
        end: 0x3000,
        writable: false,
        executable: true,
    };
    let data = Segment {
        start: 0x3000,
        end: 0x3800,
        writable: true,
        executable: false,
    };
    let rodata = Segment {
        start: 0x5000,
        end: 0x6000,
        writable: false,
        executable: false,
    };
    let segs = [code, data, rodata];
    let old = flags::PRESENT | flags::WRITABLE;
    assert_eq!(
        tighten(old, 0x1000, PAGE, &segs),
        flags::PRESENT,
        "código: R-X"
    );
    assert_eq!(
        tighten(old, 0x3000, PAGE, &segs),
        flags::PRESENT | flags::WRITABLE | flags::NO_EXECUTE,
        "datos: RW-"
    );
    assert_eq!(
        tighten(old, 0x5000, PAGE, &segs),
        flags::PRESENT | flags::NO_EXECUTE,
        "constantes: R--"
    );
    assert_eq!(
        tighten(old, 0x9000, PAGE, &segs),
        old,
        "fuera del kernel no se toca"
    );
}

/// Un ELF64 mínimo con dos encabezados de programa.
fn tiny_elf() -> Vec<u8> {
    let mut e = vec![0u8; 64 + 2 * 56];
    e[..4].copy_from_slice(b"\x7fELF");
    e[4] = 2; // 64 bits
    e[5] = 1; // little-endian
    e[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
    e[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
    e[0x38..0x3A].copy_from_slice(&2u16.to_le_bytes());
    let ph = |e: &mut Vec<u8>, at: usize, flags: u32, vaddr: u64, memsz: u64| {
        e[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        e[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
        e[at + 0x10..at + 0x18].copy_from_slice(&vaddr.to_le_bytes());
        e[at + 0x28..at + 0x30].copy_from_slice(&memsz.to_le_bytes());
    };
    ph(&mut e, 64, 5, 0x1000, 0x2000); // R-X
    ph(&mut e, 64 + 56, 6, 0x3000, 0x800); // RW-
    e
}

#[test]
fn segmentos_del_elf() {
    let segs = load_segments(&tiny_elf(), 0xFFFF_8000_0000_0000).unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].start, 0xFFFF_8000_0000_1000);
    assert!(segs[0].executable && !segs[0].writable);
    assert!(segs[1].writable && !segs[1].executable);
    assert!(segs[1].contains(0xFFFF_8000_0000_37FF));
    assert_eq!(load_segments(b"no es un elf", 0), None);
    let mut cut = tiny_elf();
    cut.truncate(100);
    assert_eq!(load_segments(&cut, 0), None, "encabezados cortados");
}
