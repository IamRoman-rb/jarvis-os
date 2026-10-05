//! Driver de discos y lectoras IDE por PIO (ATA y ATAPI).
//!
//! Hardware: la controladora IDE PCI (clase 01/01) en modo compatible —los puertos de siempre,
//! 0x1F0/0x3F6 el canal primario y 0x170/0x376 el secundario— o nativo (los puertos en los BAR
//! 0–3). Es la que VirtualBox pone por defecto (PIIX3/PIIX4). Cada disco ATA se convierte en un
//! `IdeDisk` y cada lectora ATAPI en un `IdeCd` (un `BlockDevice` de solo lectura con sectores
//! de 512, para que el instalador pueda copiar la imagen de arranque de la ISO).
//!
//! Sin DMA ni interrupciones (nIEN = 1): se espera revisando el estado y los datos pasan por el
//! puerto de datos con `rep insw`/`rep outsw`, que las máquinas virtuales atienden de a bloques.
//! La lógica que no toca puertos (firmas, paquetes SCSI, LBA48) está en `jarvis_drivers::ide`.
//! Referencias: ATA/ATAPI-6 §8–9, <https://wiki.osdev.org/ATA_PIO_Mode>,
//! <https://wiki.osdev.org/ATAPI>.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use jarvis_drivers::ahci::parse_identify;
use jarvis_drivers::ide::{
    ATA_FLUSH_EXT, ATA_IDENTIFY, ATA_IDENTIFY_PACKET, ATA_PACKET, ATA_READ_EXT, ATA_WRITE_EXT,
    Kind, MAX_SECTORS, READ_CAPACITY, ST_BSY, ST_DF, ST_DRQ, ST_ERR, START_STOP_EJECT,
    lba48_passes, parse_capacity, read10, signature,
};
use jarvis_fs::{BlockDevice, IoError, SECTOR_SIZE};
use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::{pci, serial_println, time};

// Registros (desde la base del canal).
const DATA: u16 = 0;
const FEATURES: u16 = 1;
const COUNT: u16 = 2;
const LBA_LO: u16 = 3;
const LBA_MID: u16 = 4;
const LBA_HI: u16 = 5;
const DRIVE: u16 = 6;
const COMMAND: u16 = 7;
/// Control del dispositivo (en el puerto de control): nIEN, sin interrupciones.
const CTRL_NIEN: u8 = 1 << 1;

const TIMEOUT_MS: u64 = 5000;
/// Una lectora puede tardar en arrancar el disco.
const CD_TIMEOUT_MS: u64 = 15_000;
/// Bloques de CD por pedido (64 KiB).
const CD_CHUNK: usize = 32;

/// Un solo comando a la vez en cada canal (maestro y esclavo comparten los registros). Más simple:
/// uno a la vez en todos.
static BUS: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct Channel {
    base: u16,
    ctrl: u16,
}

impl Channel {
    fn r8(&self, reg: u16) -> u8 {
        // SAFETY: `base` es el bloque de registros de un canal IDE que encontró `probe`; leerlos
        // no toca memoria.
        unsafe { Port::<u8>::new(self.base + reg).read() }
    }

    fn w8(&self, reg: u16, v: u8) {
        // SAFETY: ídem `r8`; son los registros de comando del canal.
        unsafe { Port::<u8>::new(self.base + reg).write(v) }
    }

    /// El estado alternativo: el mismo que STATUS, sin borrar la interrupción pendiente.
    fn alt(&self) -> u8 {
        // SAFETY: el registro de control/estado alternativo del canal.
        unsafe { Port::<u8>::new(self.ctrl).read() }
    }

    fn set_ctrl(&self, v: u8) {
        // SAFETY: ídem `alt`; solo se escribe nIEN.
        unsafe { Port::<u8>::new(self.ctrl).write(v) }
    }

    /// 400 ns: lo que hay que esperar después de elegir unidad o mandar un comando.
    fn delay(&self) {
        for _ in 0..4 {
            self.alt();
        }
    }

    fn wait_idle(&self, timeout: u64) -> Result<u8, IoError> {
        let deadline = time::millis() + timeout;
        loop {
            let s = self.alt();
            if s & ST_BSY == 0 {
                return Ok(s);
            }
            if time::millis() > deadline {
                return Err(IoError);
            }
            core::hint::spin_loop();
        }
    }

    /// Espera que haya datos para pasar (DRQ) o un error.
    fn wait_drq(&self, timeout: u64) -> Result<(), IoError> {
        let deadline = time::millis() + timeout;
        loop {
            let s = self.alt();
            if s & ST_BSY == 0 {
                if s & (ST_ERR | ST_DF) != 0 {
                    return Err(IoError);
                }
                if s & ST_DRQ != 0 {
                    return Ok(());
                }
            }
            if time::millis() > deadline {
                return Err(IoError);
            }
            core::hint::spin_loop();
        }
    }

    fn select(&self, slave: bool, bits: u8) {
        self.w8(DRIVE, 0xA0 | bits | (u8::from(slave) << 4));
        self.delay();
    }

    fn read_data(&self, buf: &mut [u8]) {
        let words = buf.len() / 2;
        // SAFETY: `rep insw` escribe `words` palabras en `buf`, que mide al menos eso. El
        // indicador de dirección está en 0 (lo garantiza el ABI de Rust).
        unsafe {
            core::arch::asm!(
                "rep insw",
                in("dx") self.base + DATA,
                inout("rdi") buf.as_mut_ptr() => _,
                inout("rcx") words => _,
                options(nostack, preserves_flags)
            );
        }
    }

    fn write_data(&self, buf: &[u8]) {
        let words = buf.len() / 2;
        // SAFETY: `rep outsw` lee `words` palabras de `buf`; ídem `read_data`.
        unsafe {
            core::arch::asm!(
                "rep outsw",
                in("dx") self.base + DATA,
                inout("rsi") buf.as_ptr() => _,
                inout("rcx") words => _,
                options(nostack, preserves_flags, readonly)
            );
        }
    }

    /// Descarta `n` bytes del puerto de datos (lo que no entra en el buffer).
    fn skip_data(&self, n: usize) {
        for _ in 0..n / 2 {
            // SAFETY: ídem `r8`; leer el puerto de datos solo consume la palabra.
            unsafe { Port::<u16>::new(self.base + DATA).read() };
        }
    }

    /// IDENTIFY (o IDENTIFY PACKET) de una posición: qué hay y sus 512 bytes.
    fn identify(&self, slave: bool) -> Option<(Kind, Vec<u8>)> {
        self.select(slave, 0);
        if self.alt() == 0xFF {
            return None; // bus flotante: no hay nada
        }
        for reg in [COUNT, LBA_LO, LBA_MID, LBA_HI] {
            self.w8(reg, 0);
        }
        self.w8(COMMAND, ATA_IDENTIFY);
        self.delay();
        if self.alt() == 0 {
            return None;
        }
        // Una lectora rechaza IDENTIFY y deja su firma en LBA medio/alto.
        let _ = self.wait_idle(1000).ok()?;
        let kind = signature(self.r8(LBA_MID), self.r8(LBA_HI))?;
        if kind == Kind::Atapi {
            self.w8(COMMAND, ATA_IDENTIFY_PACKET);
            self.delay();
        }
        self.wait_drq(1000).ok()?;
        let mut id = vec![0u8; 512];
        self.read_data(&mut id);
        Some((kind, id))
    }
}

/// Los canales de las controladoras IDE: (base, control) de cada uno.
fn channels() -> Vec<Channel> {
    let mut out: Vec<Channel> = Vec::new();
    for dev in pci::devices() {
        let (class, sub, prog_if) = dev.class();
        if (class, sub) != (0x01, 0x01) {
            continue;
        }
        // Bit 0 / 2 de prog_if: el canal primario / secundario está en modo nativo (BAR).
        let native = |bit: u8, bar: u8, legacy: (u16, u16)| {
            if prog_if & (1 << bit) != 0 {
                let base = (dev.bar(bar) & !3) as u16;
                let ctrl = (dev.bar(bar + 1) & !3) as u16 + 2;
                (base != 0).then_some(Channel { base, ctrl })
            } else {
                Some(Channel {
                    base: legacy.0,
                    ctrl: legacy.1,
                })
            }
        };
        for ch in [native(0, 0, (0x1F0, 0x3F6)), native(2, 2, (0x170, 0x376))]
            .into_iter()
            .flatten()
        {
            if !out.iter().any(|c| c.base == ch.base) {
                out.push(ch);
            }
        }
    }
    out
}

pub struct IdeDisk {
    ch: Channel,
    slave: bool,
    sectors: u64,
    lba48: bool,
    pub model: String,
    number: usize,
}

pub struct IdeCd {
    ch: Channel,
    slave: bool,
    blocks: u64,
    pub model: String,
    number: usize,
}

/// Las lectoras que se encontraron (para expulsar el disco después de instalar).
static CDS: Mutex<Vec<(Channel, bool)>> = Mutex::new(Vec::new());

/// Expulsa el disco de todas las lectoras (START STOP UNIT con LoEj): después de instalar desde
/// la ISO, así el próximo arranque es desde el disco y no otra vez desde el CD. En una máquina
/// virtual saca la ISO de la unidad. `true` si alguna lectora la expulsó.
pub fn eject_all() -> bool {
    let _bus = BUS.lock();
    let mut any = false;
    for &(ch, slave) in CDS.lock().iter() {
        let cd = IdeCd {
            ch,
            slave,
            blocks: 0,
            model: String::new(),
            number: 0,
        };
        let ok = cd.packet(START_STOP_EJECT, &mut []).is_ok();
        serial_println!("CD_EXPULSADO {}", if ok { "sí" } else { "no" });
        any |= ok;
    }
    any
}

/// Lo que hay en las controladoras IDE.
pub fn probe() -> (Vec<IdeDisk>, Vec<IdeCd>) {
    let _bus = BUS.lock();
    let (mut disks, mut cds) = (Vec::new(), Vec::new());
    for (n, ch) in channels().into_iter().enumerate() {
        ch.set_ctrl(CTRL_NIEN);
        for slave in [false, true] {
            let number = n * 2 + usize::from(slave);
            match ch.identify(slave) {
                Some((Kind::Ata, id)) => {
                    let Some(info) = parse_identify(&id) else {
                        continue;
                    };
                    if info.sector_size != 512 || info.sectors == 0 {
                        continue;
                    }
                    serial_println!(
                        "IDE {number}: disco {} ({} MiB)",
                        info.model,
                        info.sectors / 2048
                    );
                    disks.push(IdeDisk {
                        ch,
                        slave,
                        sectors: info.sectors,
                        lba48: info.lba48,
                        model: info.model,
                        number,
                    });
                }
                Some((Kind::Atapi, id)) => {
                    let model = jarvis_drivers::ahci::parse_identify(&id)
                        .map_or_else(|| String::from("CD/DVD"), |i| i.model);
                    let mut cd = IdeCd {
                        ch,
                        slave,
                        blocks: 0,
                        model,
                        number,
                    };
                    // La primera orden después de cambiar el disco responde "atención de
                    // unidad": se reintenta.
                    let mut cap = [0u8; 8];
                    for _ in 0..3 {
                        if cd.packet(READ_CAPACITY, &mut cap).is_ok()
                            && let Some((blocks, 2048)) = parse_capacity(&cap)
                        {
                            cd.blocks = blocks;
                            break;
                        }
                    }
                    serial_println!(
                        "IDE {number}: lectora {} ({} MiB)",
                        cd.model,
                        cd.blocks / 512
                    );
                    CDS.lock().push((ch, slave));
                    if cd.blocks > 0 {
                        cds.push(cd);
                    }
                }
                None => {}
            }
        }
    }
    (disks, cds)
}

impl IdeDisk {
    pub fn name(&self) -> String {
        format!("IDE {}: {}", self.number, self.model)
    }

    /// Elige la unidad y escribe la dirección y el conteo de un comando de lectura/escritura.
    fn setup(&self, lba: u64, count: usize) -> Result<(), IoError> {
        let ch = self.ch;
        if self.lba48 {
            ch.select(self.slave, 0x40);
            ch.wait_idle(TIMEOUT_MS)?;
            for p in lba48_passes(lba, count as u16) {
                ch.w8(COUNT, p[0]);
                ch.w8(LBA_LO, p[1]);
                ch.w8(LBA_MID, p[2]);
                ch.w8(LBA_HI, p[3]);
            }
        } else {
            if lba + count as u64 > 1 << 28 {
                return Err(IoError);
            }
            ch.select(self.slave, 0x40 | ((lba >> 24) as u8 & 0x0F));
            ch.wait_idle(TIMEOUT_MS)?;
            ch.w8(COUNT, count as u8);
            ch.w8(LBA_LO, lba as u8);
            ch.w8(LBA_MID, (lba >> 8) as u8);
            ch.w8(LBA_HI, (lba >> 16) as u8);
        }
        Ok(())
    }

    fn command(&self, ext: u8, short: u8) {
        self.ch.w8(COMMAND, if self.lba48 { ext } else { short });
        self.ch.delay();
    }
}

impl BlockDevice for IdeDisk {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let _bus = BUS.lock();
        for (i, chunk) in buf.chunks_mut(MAX_SECTORS * SECTOR_SIZE).enumerate() {
            let count = chunk.len() / SECTOR_SIZE;
            self.setup(lba + (i * MAX_SECTORS) as u64, count)?;
            self.command(ATA_READ_EXT, 0x20);
            for sector in chunk.chunks_mut(SECTOR_SIZE) {
                self.ch.wait_drq(TIMEOUT_MS)?;
                self.ch.read_data(sector);
            }
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        let _bus = BUS.lock();
        for (i, chunk) in buf.chunks(MAX_SECTORS * SECTOR_SIZE).enumerate() {
            let count = chunk.len() / SECTOR_SIZE;
            self.setup(lba + (i * MAX_SECTORS) as u64, count)?;
            self.command(ATA_WRITE_EXT, 0x30);
            for sector in chunk.chunks(SECTOR_SIZE) {
                self.ch.wait_drq(TIMEOUT_MS)?;
                self.ch.write_data(sector);
            }
            if self.ch.wait_idle(TIMEOUT_MS)? & (ST_ERR | ST_DF) != 0 {
                return Err(IoError);
            }
        }
        // Que quede en el disco y no en su caché.
        self.command(ATA_FLUSH_EXT, 0xE7);
        if self.ch.wait_idle(30_000)? & ST_ERR != 0 {
            return Err(IoError);
        }
        Ok(())
    }
}

impl IdeCd {
    pub fn name(&self) -> String {
        format!("CD {}: {}", self.number, self.model)
    }

    /// Manda un paquete SCSI y lee la respuesta en `out` (lo que sobre se descarta).
    fn packet(&self, pkt: [u8; 12], out: &mut [u8]) -> Result<(), IoError> {
        let ch = self.ch;
        ch.select(self.slave, 0);
        ch.wait_idle(CD_TIMEOUT_MS)?;
        ch.w8(FEATURES, 0); // PIO
        // Bytes por tanda que aceptamos: 64 KiB menos 2 (el máximo par).
        ch.w8(LBA_MID, 0xFE);
        ch.w8(LBA_HI, 0xFF);
        ch.w8(COMMAND, ATA_PACKET);
        ch.delay();
        ch.wait_drq(TIMEOUT_MS)?;
        ch.write_data(&pkt);
        let mut at = 0;
        loop {
            ch.delay();
            let s = ch.wait_idle(CD_TIMEOUT_MS)?;
            if s & (ST_ERR | ST_DF) != 0 {
                return Err(IoError);
            }
            if s & ST_DRQ == 0 {
                break;
            }
            let n = usize::from(ch.r8(LBA_MID)) | usize::from(ch.r8(LBA_HI)) << 8;
            let fits = n.min(out.len() - at) & !1;
            ch.read_data(&mut out[at..at + fits]);
            ch.skip_data(n - fits);
            at += fits;
        }
        if at < out.len() {
            return Err(IoError);
        }
        Ok(())
    }

    /// Lee bloques de 2048 bytes.
    pub fn read_blocks(&self, block: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let _bus = BUS.lock();
        for (i, chunk) in buf.chunks_mut(CD_CHUNK * 2048).enumerate() {
            let n = chunk.len() / 2048;
            self.packet(read10(block + (i * CD_CHUNK) as u32, n as u16), chunk)?;
        }
        Ok(())
    }
}

/// El CD visto como un disco de sectores de 512 (solo lectura).
impl BlockDevice for IdeCd {
    fn sector_count(&self) -> u64 {
        self.blocks * 4
    }

    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let start = lba * 512;
        let end = start + buf.len() as u64;
        let first = start / 2048;
        let last = end.div_ceil(2048);
        if last > self.blocks {
            return Err(IoError);
        }
        let mut tmp = vec![0u8; ((last - first) * 2048) as usize];
        self.read_blocks(first as u32, &mut tmp)?;
        let off = (start - first * 2048) as usize;
        buf.copy_from_slice(&tmp[off..off + buf.len()]);
        Ok(())
    }

    fn write(&mut self, _lba: u64, _buf: &[u8]) -> Result<(), IoError> {
        Err(IoError)
    }
}
