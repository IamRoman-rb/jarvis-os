//! GPT, AHCI y NVMe sin hardware. La GPT se prueba **cruzada** contra la crate `gpt`: lo que
//! escribe una lo lee la otra.

use std::io::Cursor;

use jarvis_drivers::ahci::{
    ATA_READ_DMA_EXT, command_header, command_table_size, h2d_fis, parse_identify, prdt_entry,
};
use jarvis_drivers::gpt::{
    EFI_SYSTEM, Guid, JARVIS_DATA, NewPartition, PartitionDevice, crc32, create, is_blank,
    parse_header, read,
};
use jarvis_drivers::nvme::{
    Command, IO_READ, doorbell, parse_cap, parse_completion, parse_controller, parse_namespace,
};
use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};

/// Un disco en memoria (y sus bytes, para pasárselos a `gpt`).
struct VecDisk(Vec<u8>);

impl BlockDevice for VecDisk {
    fn sector_count(&self) -> u64 {
        (self.0.len() / SECTOR_SIZE) as u64
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let at = lba as usize * SECTOR_SIZE;
        buf.copy_from_slice(self.0.get(at..at + buf.len()).ok_or(IoError)?);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let at = lba as usize * SECTOR_SIZE;
        self.0
            .get_mut(at..at + buf.len())
            .ok_or(IoError)?
            .copy_from_slice(buf);
        Ok(())
    }
}

const MIB: u64 = 1024 * 1024 / SECTOR_SIZE as u64;

fn guid(n: u8) -> Guid {
    Guid([n; 16])
}

/// Un disco de 64 MiB con una ESP de 16 MiB y el resto para JARVIS.
fn jarvis_disk() -> VecDisk {
    let mut d = VecDisk(vec![0u8; 64 << 20]);
    let layout = create(
        d.sector_count(),
        &[
            NewPartition {
                kind: EFI_SYSTEM,
                sectors: 16 * MIB,
                name: "EFI",
            },
            NewPartition {
                kind: JARVIS_DATA,
                sectors: 0,
                name: "JARVIS-OS ñ",
            },
        ],
        &[guid(1), guid(2), guid(3)],
    )
    .unwrap();
    for (lba, bytes) in layout.writes {
        d.write(lba, &bytes).unwrap();
    }
    d
}

#[test]
fn crc32_conocido() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn guid_en_el_orden_del_disco() {
    // C12A7328-F81F-11D2-BA4B-00A0C93EC93B se guarda como 28 73 2A C1 1F F8 D2 11 BA 4B ...
    assert_eq!(
        EFI_SYSTEM.0,
        [
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B
        ]
    );
}

#[test]
fn nuestra_gpt_la_lee_la_crate_gpt() {
    let d = jarvis_disk();
    assert_eq!(&d.0[510..512], &[0x55, 0xAA], "MBR protector");
    assert_eq!(d.0[446 + 4], 0xEE);
    let total = d.sector_count();
    let disk = gpt::GptConfig::new()
        .writable(false)
        .logical_block_size(gpt::disk::LogicalBlockSize::Lb512)
        .open_from_device(Box::new(Cursor::new(d.0)))
        .expect("la crate gpt no aceptó la tabla");
    let parts: Vec<_> = disk.partitions().values().collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0].part_type_guid.guid,
        "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
    );
    assert_eq!(parts[0].first_lba, 2048);
    assert_eq!(parts[0].last_lba, 2048 + 16 * MIB - 1);
    assert_eq!(parts[0].name, "EFI");
    assert_eq!(parts[0].part_guid.to_bytes_le(), [2; 16]);
    assert_eq!(parts[1].first_lba, 2048 + 16 * MIB);
    assert_eq!(parts[1].last_lba, total - 34);
    assert_eq!(parts[1].name, "JARVIS-OS ñ");
    assert_eq!(disk.guid().to_bytes_le(), [1; 16]);
}

#[test]
fn la_gpt_de_la_crate_la_leemos_nosotros() {
    let path = std::env::temp_dir().join(format!("jarvis-gpt-{}.img", std::process::id()));
    std::fs::write(&path, vec![0u8; 32 << 20]).unwrap();
    let mut disk = gpt::GptConfig::new()
        .writable(true)
        .initialized(false)
        .logical_block_size(gpt::disk::LogicalBlockSize::Lb512)
        .open(&path)
        .unwrap();
    disk.update_partitions(Default::default()).unwrap();
    disk.add_partition("ESP", 8 << 20, gpt::partition_types::EFI, 0, Some(2048))
        .unwrap();
    disk.add_partition("datos", 4 << 20, gpt::partition_types::BASIC, 0, Some(2048))
        .unwrap();
    disk.write().unwrap();
    let mut d = VecDisk(std::fs::read(&path).unwrap());
    let _ = std::fs::remove_file(&path);
    let parts = read(&mut d).expect("no leímos la GPT de la crate");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].kind, EFI_SYSTEM);
    assert_eq!(parts[0].name, "ESP");
    assert_eq!(parts[0].sectors(), 8 * MIB);
    assert_eq!(parts[1].name, "datos");
    assert_eq!(parts[1].sectors(), 4 * MIB);
}

#[test]
fn copia_del_final_si_se_rompe_el_principio() {
    let mut d = jarvis_disk();
    // Se pisa la cabecera principal: queda la copia del final.
    d.write(1, &[0u8; SECTOR_SIZE]).unwrap();
    let parts = read(&mut d).unwrap();
    assert_eq!(parts[1].kind, JARVIS_DATA);
    // Y con las entradas corruptas en las dos copias, no hay tabla.
    let mut d = jarvis_disk();
    let total = d.sector_count();
    for lba in [2, total - 33] {
        let mut s = vec![0u8; SECTOR_SIZE];
        d.read(lba, &mut s).unwrap();
        s[0] ^= 1;
        d.write(lba, &s).unwrap();
    }
    assert!(read(&mut d).is_none());
}

#[test]
fn cabecera_con_crc_roto() {
    let d = jarvis_disk();
    let mut h = d.0[512..1024].to_vec();
    assert!(parse_header(&h).is_some());
    h[40] ^= 1;
    assert!(parse_header(&h).is_none());
}

#[test]
fn disco_vacio_o_no() {
    let mut empty = VecDisk(vec![0u8; 8 << 20]);
    assert_eq!(is_blank(&mut empty), Ok(true));
    let mut gpt = jarvis_disk();
    assert_eq!(is_blank(&mut gpt), Ok(false));
    // Un MBR de Windows (solo la firma) tampoco se toca.
    let mut mbr = VecDisk(vec![0u8; 8 << 20]);
    mbr.0[510] = 0x55;
    mbr.0[511] = 0xAA;
    assert_eq!(is_blank(&mut mbr), Ok(false));
    // Ni uno al que solo le quedó la GPT de respaldo.
    let mut backup = jarvis_disk();
    backup.write(0, &[0u8; SECTOR_SIZE]).unwrap();
    backup.write(1, &[0u8; SECTOR_SIZE]).unwrap();
    assert_eq!(is_blank(&mut backup), Ok(false));
}

#[test]
fn particion_como_disco_no_se_sale() {
    let d = jarvis_disk();
    let mut d2 = VecDisk(d.0);
    let parts = read(&mut d2).unwrap();
    let data = &parts[1];
    let mut p = PartitionDevice::new(&mut d2, data.first, data.sectors());
    assert_eq!(p.sector_count(), data.sectors());
    p.write(0, &[7u8; SECTOR_SIZE]).unwrap();
    let last = p.sector_count() - 1;
    p.write(last, &[9u8; SECTOR_SIZE]).unwrap();
    assert_eq!(p.write(last, &[0u8; 2 * SECTOR_SIZE]), Err(IoError));
    assert_eq!(p.read(last + 1, &mut [0u8; SECTOR_SIZE]), Err(IoError));
    let first = data.first as usize * SECTOR_SIZE;
    assert_eq!(d2.0[first], 7);
    // La GPT de respaldo sigue sana: la partición no la pisó.
    assert!(read(&mut d2).is_some());
}

#[test]
fn no_entra_en_el_disco() {
    let parts = [NewPartition {
        kind: JARVIS_DATA,
        sectors: 100 * MIB,
        name: "grande",
    }];
    assert!(create(64 * MIB, &parts, &[guid(1), guid(2)]).is_none());
}

#[test]
fn comandos_ahci() {
    let f = h2d_fis(ATA_READ_DMA_EXT, 0x0123_4567_89AB, 8);
    assert_eq!(
        f[..14],
        [
            0x27, 0x80, 0x25, 0, 0xAB, 0x89, 0x67, 0x40, 0x45, 0x23, 0x01, 0, 8, 0
        ]
    );
    let h = command_header(5, true, 1, 0x1234_5000);
    assert_eq!(h[0], 5 | 0x40);
    assert_eq!(&h[2..4], &[1, 0]);
    assert_eq!(&h[8..16], &0x1234_5000u64.to_le_bytes());
    let e = prdt_entry(0x2000, 4096);
    assert_eq!(
        u32::from_le_bytes(e[12..16].try_into().unwrap()),
        4095 | 1 << 31
    );
    assert_eq!(command_table_size(1), 0x90);
}

#[test]
fn identify_de_un_disco_sata() {
    let mut d = vec![0u8; 512];
    // Modelo "WD Green" con los bytes de cada palabra dados vuelta, como lo manda el disco.
    let model = b"WD Green 2.5 480GB";
    let mut padded = [b' '; 40];
    padded[..model.len()].copy_from_slice(model);
    for (i, pair) in padded.chunks(2).enumerate() {
        d[54 + i * 2] = pair[1];
        d[54 + i * 2 + 1] = pair[0];
    }
    d[83 * 2 + 1] = 1 << 2; // palabra 83 bit 10: LBA48
    d[200..208].copy_from_slice(&937_703_088u64.to_le_bytes());
    let id = parse_identify(&d).unwrap();
    assert_eq!(id.model, "WD Green 2.5 480GB");
    assert!(id.lba48);
    assert_eq!(id.sectors, 937_703_088);
    assert_eq!(id.sector_size, 512);
}

#[test]
fn comandos_y_respuestas_nvme() {
    let c = Command::rw(IO_READ, 7, 1, 0x1_0000_0002, 8, [0x5000, 0]).to_bytes();
    assert_eq!(c[0], IO_READ);
    assert_eq!(&c[2..4], &7u16.to_le_bytes());
    assert_eq!(&c[4..8], &1u32.to_le_bytes());
    assert_eq!(&c[24..32], &0x5000u64.to_le_bytes());
    assert_eq!(&c[40..44], &2u32.to_le_bytes());
    assert_eq!(&c[44..48], &1u32.to_le_bytes());
    assert_eq!(&c[48..52], &7u32.to_le_bytes());

    let mut e = [0u8; 16];
    e[8..10].copy_from_slice(&3u16.to_le_bytes());
    e[12..14].copy_from_slice(&7u16.to_le_bytes());
    e[14..16].copy_from_slice(&((0x02 << 1) | 1u16).to_le_bytes());
    let r = parse_completion(&e);
    assert_eq!((r.sq_head, r.id, r.phase, r.status), (3, 7, true, 2));

    // CAP de QEMU: MQES 2047, DSTRD 0, TO 15 (7,5 s), MPSMIN 0.
    let cap = parse_cap(0x0000_0020_0F01_07FF);
    assert_eq!(cap.max_queue_entries, 2048);
    assert_eq!(cap.doorbell_stride, 4);
    assert_eq!(cap.timeout_ms, 7500);
    assert_eq!(cap.min_page, 4096);
    assert_eq!(doorbell(0, false, 4), 0x1000);
    assert_eq!(doorbell(1, true, 4), 0x100C);

    let mut ctrl = vec![0u8; 4096];
    ctrl[4..12].copy_from_slice(b"JARVIS01");
    ctrl[24..34].copy_from_slice(b"QEMU NVMe ");
    ctrl[77] = 5;
    ctrl[516] = 1;
    let c = parse_controller(&ctrl).unwrap();
    assert_eq!(
        (c.serial.as_str(), c.model.as_str()),
        ("JARVIS01", "QEMU NVMe")
    );
    assert_eq!((c.mdts, c.namespaces), (5, 1));

    let mut ns = vec![0u8; 4096];
    ns[..8].copy_from_slice(&262_144u64.to_le_bytes());
    ns[26] = 1; // formato 1
    ns[128 + 4 + 2] = 12; // 4 KiB por bloque
    let n = parse_namespace(&ns).unwrap();
    assert_eq!((n.blocks, n.block_size), (262_144, 4096));
    assert!(parse_namespace(&vec![0u8; 4096]).is_none());
}
