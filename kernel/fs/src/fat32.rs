//! Montaje y operaciones de FAT32.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::device::{BlockDevice, SECTOR_SIZE};
use crate::dirent::{
    ATTR_ARCHIVE, ATTR_DIRECTORY, ATTR_HIDDEN, ATTR_LFN, ATTR_READ_ONLY, ATTR_VOLUME_ID, DELETED,
    END, ENTRY_SIZE, RawEntry, encode_lfn, exact_short, lfn_checksum, make_short, parse_lfn,
    same_name, short_display, validate_name,
};
use crate::error::{FsError, Result};
use crate::time::Timestamp;

/// Las entradas de la FAT usan 28 bits; los 4 de arriba están reservados y se preservan.
const FAT_MASK: u32 = 0x0FFF_FFFF;
/// "Fin de cadena": último cluster de un archivo.
const END_OF_CHAIN: u32 = 0x0FFF_FFFF;
const END_OF_CHAIN_MIN: u32 = 0x0FFF_FFF8;
/// Mínimo de clusters para que un volumen sea FAT32 (por debajo es FAT16).
const MIN_FAT32_CLUSTERS: u32 = 65_525;
const FSINFO_LEAD_SIG: u32 = 0x4161_5252;
const FSINFO_STRUCT_SIG: u32 = 0x6141_7272;

/// Un archivo o carpeta, tal como lo ve quien usa el sistema de archivos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// Nombre completo (largo si lo tiene).
    pub name: String,
    /// Alias 8.3 ("NOTASD~1.TXT"), útil para el inspector.
    pub short_name: String,
    pub is_dir: bool,
    pub size: u32,
    /// Primer cluster de datos (0 si el archivo está vacío).
    pub first_cluster: u32,
    pub created: Timestamp,
    pub modified: Timestamp,
    pub read_only: bool,
    pub hidden: bool,
}

/// Entrada encontrada al recorrer un directorio, con su ubicación (para modificarla).
struct Found {
    entry: DirEntry,
    raw: RawEntry,
    /// Primera entrada que ocupa (la primera LFN, o la corta si no tiene nombre largo).
    first_slot: usize,
    /// Entrada corta.
    short_slot: usize,
}

/// Un directorio cargado entero en memoria (los directorios son chicos).
struct Dir {
    clusters: Vec<u32>,
    data: Vec<u8>,
}

impl Dir {
    fn slots(&self) -> usize {
        self.data.len() / ENTRY_SIZE
    }

    fn slot(&self, i: usize) -> &[u8] {
        &self.data[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE]
    }

    fn slot_mut(&mut self, i: usize) -> &mut [u8] {
        &mut self.data[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE]
    }

    fn mark_deleted(&mut self, f: &Found) {
        for i in f.first_slot..=f.short_slot {
            self.slot_mut(i)[0] = DELETED;
        }
    }

    /// Primer lugar con `n` entradas libres seguidas.
    fn free_run(&self, n: usize) -> Option<usize> {
        let mut run = 0;
        for i in 0..self.slots() {
            let b0 = self.data[i * ENTRY_SIZE];
            if b0 == END || b0 == DELETED {
                run += 1;
                if run == n {
                    return Some(i + 1 - n);
                }
            } else {
                run = 0;
            }
        }
        None
    }
}

/// Nombre largo en construcción mientras se leen sus entradas LFN.
struct LfnAcc {
    start: usize,
    checksum: u8,
    /// Número de la última parte leída (van en orden descendente hasta 1).
    last: u8,
    units: Vec<u16>,
}

fn scan(dir: &Dir) -> Vec<Found> {
    let mut out = Vec::new();
    let mut lfn: Option<LfnAcc> = None;
    for i in 0..dir.slots() {
        let b = dir.slot(i);
        match b[0] {
            END => break,
            DELETED => {
                lfn = None;
                continue;
            }
            _ => {}
        }
        if b[11] == ATTR_LFN {
            let (ord, checksum, chars) = parse_lfn(b);
            let part = ord & 0x1F;
            if ord & 0x40 != 0 && part > 0 {
                let mut units = vec![0u16; part as usize * 13];
                units[(part as usize - 1) * 13..part as usize * 13].copy_from_slice(&chars);
                lfn = Some(LfnAcc {
                    start: i,
                    checksum,
                    last: part,
                    units,
                });
            } else if let Some(acc) = lfn.as_mut()
                && acc.checksum == checksum
                && part + 1 == acc.last
                && part > 0
            {
                acc.units[(part as usize - 1) * 13..part as usize * 13].copy_from_slice(&chars);
                acc.last = part;
            } else {
                lfn = None; // LFN huérfana o desordenada: se ignora
            }
            continue;
        }
        let raw = RawEntry::parse(b);
        if raw.attr & ATTR_VOLUME_ID != 0 {
            lfn = None;
            continue;
        }
        let (name, first_slot) = match lfn.take() {
            Some(acc) if acc.last == 1 && acc.checksum == lfn_checksum(&raw.name) => {
                let len = acc.units.iter().position(|&u| u == 0 || u == 0xFFFF);
                let units = &acc.units[..len.unwrap_or(acc.units.len())];
                (String::from_utf16_lossy(units), acc.start)
            }
            _ => (short_display(&raw), i),
        };
        out.push(Found {
            entry: DirEntry {
                name,
                short_name: short_display(&raw),
                is_dir: raw.is_dir(),
                size: raw.size,
                first_cluster: raw.cluster,
                created: Timestamp::from_fat(raw.crt_date, raw.crt_time),
                modified: Timestamp::from_fat(raw.wrt_date, raw.wrt_time),
                read_only: raw.attr & ATTR_READ_ONLY != 0,
                hidden: raw.attr & ATTR_HIDDEN != 0,
            },
            raw,
            first_slot,
            short_slot: i,
        });
    }
    out
}

fn find<'a>(found: &'a [Found], name: &str) -> Option<&'a Found> {
    found
        .iter()
        .find(|f| !f.raw.is_dot() && same_name(&f.entry.name, name))
}

fn components(path: &str) -> Result<Vec<&str>> {
    let parts: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    if parts.iter().any(|&c| c == "." || c == "..") {
        return Err(FsError::InvalidName);
    }
    Ok(parts)
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

/// Sectores de FAT que se leen por pedido al contar el espacio libre.
const FAT_READ_SECTORS: u32 = 64;
/// Tope de clusters por escritura de datos (el driver igual los parte en bloques de 64 KiB).
const MAX_RUN_CLUSTERS: usize = 4096;

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn new_raw(attr: u8, cluster: u32, size: u32, now: Timestamp) -> RawEntry {
    let (date, time) = now.to_fat();
    RawEntry {
        name: [b' '; 11],
        attr,
        ntres: 0,
        crt_tenth: 0,
        crt_time: time,
        crt_date: date,
        acc_date: date,
        cluster,
        wrt_time: time,
        wrt_date: date,
        size,
    }
}

pub struct FileSystem<D: BlockDevice> {
    dev: D,
    sectors_per_cluster: u32,
    reserved_sectors: u32,
    fat_count: u32,
    fat_sectors: u32,
    root_cluster: u32,
    first_data_sector: u32,
    /// Clusters de datos: los válidos van de 2 a `cluster_count + 1`.
    cluster_count: u32,
    fsinfo_sector: u32,
    free_clusters: u32,
    next_free: u32,
    label: String,
    /// Último sector de la FAT leído (casi todo el acceso a la FAT es secuencial).
    fat_cache: Option<(u32, [u8; SECTOR_SIZE])>,
}

impl<D: BlockDevice> FileSystem<D> {
    /// Lee el sector de arranque, valida que sea FAT32 y cuenta el espacio libre.
    pub fn mount(mut dev: D) -> Result<Self> {
        let mut bs = [0u8; SECTOR_SIZE];
        dev.read(0, &mut bs)?;
        if bs[510] != 0x55 || bs[511] != 0xAA {
            return Err(FsError::NotFat32);
        }
        let bytes_per_sector = u16_at(&bs, 11) as usize;
        let sectors_per_cluster = bs[13] as u32;
        let reserved_sectors = u16_at(&bs, 14) as u32;
        let fat_count = bs[16] as u32;
        let root_entries = u16_at(&bs, 17);
        let total16 = u16_at(&bs, 19) as u32;
        let fat16_size = u16_at(&bs, 22);
        let total32 = u32_at(&bs, 32);
        let fat_sectors = u32_at(&bs, 36);
        let root_cluster = u32_at(&bs, 44);
        let fsinfo_sector = u16_at(&bs, 48) as u32;

        if bytes_per_sector != SECTOR_SIZE
            || !sectors_per_cluster.is_power_of_two()
            || fat_count == 0
            || root_entries != 0
            || fat16_size != 0
            || fat_sectors == 0
        {
            return Err(FsError::NotFat32);
        }
        let total = if total16 != 0 { total16 } else { total32 };
        let first_data_sector = reserved_sectors + fat_count * fat_sectors;
        if total <= first_data_sector || total as u64 > dev.sector_count() {
            return Err(FsError::Corrupt("tamaño de volumen inválido"));
        }
        let cluster_count = (total - first_data_sector) / sectors_per_cluster;
        if cluster_count < MIN_FAT32_CLUSTERS {
            return Err(FsError::NotFat32);
        }
        if fat_sectors as u64 * (SECTOR_SIZE as u64 / 4) < cluster_count as u64 + 2 {
            return Err(FsError::Corrupt("la FAT es más chica que el volumen"));
        }
        let label = if bs[66] == 0x29 {
            String::from(String::from_utf8_lossy(&bs[71..82]).trim_end())
        } else {
            String::new()
        };

        let mut fs = FileSystem {
            dev,
            sectors_per_cluster,
            reserved_sectors,
            fat_count,
            fat_sectors,
            root_cluster,
            first_data_sector,
            cluster_count,
            fsinfo_sector,
            free_clusters: 0,
            next_free: 2,
            label,
            fat_cache: None,
        };
        if !fs.valid_cluster(root_cluster) {
            return Err(FsError::Corrupt("cluster raíz inválido"));
        }
        fs.free_clusters = fs.count_free()?;
        Ok(fs)
    }

    pub fn into_device(self) -> D {
        self.dev
    }

    /// Etiqueta del volumen ("JARVIS").
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn cluster_size(&self) -> u32 {
        self.sectors_per_cluster * SECTOR_SIZE as u32
    }

    pub fn total_bytes(&self) -> u64 {
        self.cluster_count as u64 * self.cluster_size() as u64
    }

    pub fn free_bytes(&self) -> u64 {
        self.free_clusters as u64 * self.cluster_size() as u64
    }

    // --- tabla FAT ----------------------------------------------------------------------------

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.cluster_count + 2
    }

    fn count_free(&mut self) -> Result<u32> {
        let mut free = 0;
        let per_sector = (SECTOR_SIZE / 4) as u32;
        // De a 64 sectores (32 KiB) por pedido: un disco de 256 MiB con clusters de 512 bytes
        // tiene 4096 sectores de FAT, y leerlos de a uno demoraba el arranque.
        let mut buf = vec![0u8; FAT_READ_SECTORS as usize * SECTOR_SIZE];
        let used = (self.cluster_count + 2)
            .div_ceil(per_sector)
            .min(self.fat_sectors);
        let mut s = 0;
        while s < used {
            let n = FAT_READ_SECTORS.min(used - s);
            let chunk = &mut buf[..n as usize * SECTOR_SIZE];
            self.dev.read((self.reserved_sectors + s) as u64, chunk)?;
            for i in 0..n * per_sector {
                let c = s * per_sector + i;
                if self.valid_cluster(c) && u32_at(chunk, i as usize * 4) & FAT_MASK == 0 {
                    free += 1;
                }
            }
            s += n;
        }
        Ok(free)
    }

    fn load_fat_sector(&mut self, sector: u32) -> Result<()> {
        if self.fat_cache.as_ref().map(|(s, _)| *s) != Some(sector) {
            let mut buf = [0u8; SECTOR_SIZE];
            self.dev
                .read((self.reserved_sectors + sector) as u64, &mut buf)?;
            self.fat_cache = Some((sector, buf));
        }
        Ok(())
    }

    fn fat_get(&mut self, c: u32) -> Result<u32> {
        let (sector, offset) = (c * 4 / SECTOR_SIZE as u32, (c * 4) as usize % SECTOR_SIZE);
        self.load_fat_sector(sector)?;
        let Some((_, buf)) = &self.fat_cache else {
            return Err(FsError::Io);
        };
        Ok(u32_at(buf, offset) & FAT_MASK)
    }

    /// Cambia varias entradas de la FAT a la vez: cada sector que cambia se lee una vez y se
    /// escribe una vez en cada copia. Guardar un archivo de 3 MB pasó de 12.000 escrituras de la
    /// FAT a menos de 100.
    fn fat_set_many(&mut self, entries: &[(u32, u32)]) -> Result<()> {
        let mut sorted: Vec<(u32, u32)> = entries.to_vec();
        sorted.sort_unstable_by_key(|e| e.0);
        let per_sector = (SECTOR_SIZE / 4) as u32;
        let mut i = 0;
        while i < sorted.len() {
            let sector = sorted[i].0 / per_sector;
            self.load_fat_sector(sector)?;
            let Some((_, buf)) = self.fat_cache.as_mut() else {
                return Err(FsError::Io);
            };
            while i < sorted.len() && sorted[i].0 / per_sector == sector {
                let (c, value) = sorted[i];
                let offset = (c * 4) as usize % SECTOR_SIZE;
                let old = u32_at(buf, offset);
                let new = (old & !FAT_MASK) | (value & FAT_MASK);
                buf[offset..offset + 4].copy_from_slice(&new.to_le_bytes());
                i += 1;
            }
            let copy = *buf;
            for fat in 0..self.fat_count {
                let lba = self.reserved_sectors + fat * self.fat_sectors + sector;
                self.dev.write(lba as u64, &copy)?;
            }
        }
        Ok(())
    }

    /// Escribe una entrada de la FAT en **todas** las copias.
    fn fat_set(&mut self, c: u32, value: u32) -> Result<()> {
        let (sector, offset) = (c * 4 / SECTOR_SIZE as u32, (c * 4) as usize % SECTOR_SIZE);
        self.load_fat_sector(sector)?;
        let Some((_, buf)) = self.fat_cache.as_mut() else {
            return Err(FsError::Io);
        };
        let old = u32_at(buf, offset);
        let new = (old & !FAT_MASK) | (value & FAT_MASK);
        buf[offset..offset + 4].copy_from_slice(&new.to_le_bytes());
        let copy = *buf;
        for fat in 0..self.fat_count {
            let lba = self.reserved_sectors + fat * self.fat_sectors + sector;
            self.dev.write(lba as u64, &copy)?;
        }
        Ok(())
    }

    /// Clusters de un archivo, siguiendo la lista enlazada de la FAT.
    fn chain(&mut self, first: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        let mut c = first;
        if c == 0 {
            return Ok(out);
        }
        loop {
            if !self.valid_cluster(c) || out.len() as u32 > self.cluster_count {
                return Err(FsError::Corrupt("cadena de clusters rota"));
            }
            out.push(c);
            let next = self.fat_get(c)?;
            if next >= END_OF_CHAIN_MIN {
                return Ok(out);
            }
            c = next;
        }
    }

    /// Reserva `n` clusters libres, los enlaza entre sí y devuelve la lista.
    fn alloc_clusters(&mut self, n: u32) -> Result<Vec<u32>> {
        if n > self.free_clusters {
            return Err(FsError::NoSpace);
        }
        let mut found = Vec::with_capacity(n as usize);
        let mut c = self.next_free;
        for _ in 0..self.cluster_count {
            if found.len() as u32 == n {
                break;
            }
            if !self.valid_cluster(c) {
                c = 2;
            }
            if self.fat_get(c)? == 0 {
                found.push(c);
            }
            c += 1;
        }
        if (found.len() as u32) < n {
            return Err(FsError::Corrupt(
                "el contador de espacio libre no coincide con la FAT",
            ));
        }
        let links: Vec<(u32, u32)> = found
            .iter()
            .enumerate()
            .map(|(i, &cluster)| (cluster, found.get(i + 1).copied().unwrap_or(END_OF_CHAIN)))
            .collect();
        self.fat_set_many(&links)?;
        self.free_clusters -= n;
        self.next_free = c;
        Ok(found)
    }

    fn free_chain(&mut self, first: u32) -> Result<()> {
        let chain = self.chain(first)?;
        let zeros: Vec<(u32, u32)> = chain.iter().map(|&c| (c, 0)).collect();
        self.fat_set_many(&zeros)?;
        self.free_clusters += chain.len() as u32;
        Ok(())
    }

    fn clusters_for(&self, len: usize) -> u32 {
        (len as u64).div_ceil(self.cluster_size() as u64) as u32
    }

    // --- datos --------------------------------------------------------------------------------

    fn cluster_lba(&self, c: u32) -> u64 {
        self.first_data_sector as u64 + (c - 2) as u64 * self.sectors_per_cluster as u64
    }

    fn read_cluster(&mut self, c: u32, buf: &mut [u8]) -> Result<()> {
        let lba = self.cluster_lba(c);
        Ok(self.dev.read(lba, buf)?)
    }

    fn write_cluster(&mut self, c: u32, buf: &[u8]) -> Result<()> {
        let lba = self.cluster_lba(c);
        Ok(self.dev.write(lba, buf)?)
    }

    /// Guarda `data` en clusters nuevos. Devuelve el primero (0 si `data` está vacío).
    fn alloc_and_write(&mut self, data: &[u8]) -> Result<u32> {
        if data.is_empty() {
            return Ok(0);
        }
        let cs = self.cluster_size() as usize;
        let chain = self.alloc_clusters(self.clusters_for(data.len()))?;
        // Los clusters consecutivos (lo normal en un disco poco fragmentado) van en un solo
        // pedido: el driver los parte en bloques de 64 KiB en vez de mandar uno por cluster.
        let mut i = 0;
        while i < chain.len() {
            let mut j = i + 1;
            while j < chain.len() && chain[j] == chain[j - 1] + 1 && j - i < MAX_RUN_CLUSTERS {
                j += 1;
            }
            let (start, end) = (i * cs, (j * cs).min(data.len()));
            let lba = self.cluster_lba(chain[i]);
            if end - start == (j - i) * cs {
                self.dev.write(lba, &data[start..end])?;
            } else {
                // El último cluster, incompleto: se completa con ceros.
                let mut buf = vec![0u8; (j - i) * cs];
                buf[..end - start].copy_from_slice(&data[start..end]);
                self.dev.write(lba, &buf)?;
            }
            i = j;
        }
        Ok(chain[0])
    }

    // --- directorios --------------------------------------------------------------------------

    fn load_dir(&mut self, first: u32) -> Result<Dir> {
        let clusters = self.chain(first)?;
        let cs = self.cluster_size() as usize;
        let mut data = vec![0u8; clusters.len() * cs];
        for (i, &c) in clusters.iter().enumerate() {
            self.read_cluster(c, &mut data[i * cs..(i + 1) * cs])?;
        }
        Ok(Dir { clusters, data })
    }

    fn store_dir(&mut self, dir: &Dir) -> Result<()> {
        let cs = self.cluster_size() as usize;
        for (i, &c) in dir.clusters.iter().enumerate() {
            self.write_cluster(c, &dir.data[i * cs..(i + 1) * cs])?;
        }
        Ok(())
    }

    /// Agrega un cluster vacío al final de un directorio.
    fn extend_dir(&mut self, dir: &mut Dir) -> Result<()> {
        let new = self.alloc_clusters(1)?[0];
        let cs = self.cluster_size() as usize;
        self.write_cluster(new, &vec![0u8; cs])?;
        if let Some(&last) = dir.clusters.last() {
            self.fat_set(last, new)?;
        }
        dir.clusters.push(new);
        dir.data.resize(dir.data.len() + cs, 0);
        Ok(())
    }

    fn dir_cluster(&self, raw_cluster: u32) -> u32 {
        // En las entradas "..", el cluster 0 significa "la raíz".
        if raw_cluster == 0 {
            self.root_cluster
        } else {
            raw_cluster
        }
    }

    /// Cluster de la carpeta a la que lleva `parts`.
    fn resolve_dir(&mut self, parts: &[&str]) -> Result<u32> {
        let mut current = self.root_cluster;
        for part in parts {
            let dir = self.load_dir(current)?;
            let found = scan(&dir);
            let f = find(&found, part).ok_or(FsError::NotFound)?;
            if !f.raw.is_dir() {
                return Err(FsError::NotADirectory);
            }
            current = self.dir_cluster(f.raw.cluster);
        }
        Ok(current)
    }

    /// Carpeta contenedora y entrada de `path`.
    fn lookup(&mut self, path: &str) -> Result<(u32, Found)> {
        let parts = components(path)?;
        let (name, parents) = parts.split_last().ok_or(FsError::RootNotAllowed)?;
        let parent = self.resolve_dir(parents)?;
        let dir = self.load_dir(parent)?;
        let found = scan(&dir);
        let i = found
            .iter()
            .position(|f| !f.raw.is_dot() && same_name(&f.entry.name, name))
            .ok_or(FsError::NotFound)?;
        Ok((parent, found.into_iter().nth(i).ok_or(FsError::NotFound)?))
    }

    /// Crea las entradas (LFN + corta) de `name` en la carpeta `parent`, copiando el resto de
    /// los datos de `template`.
    fn create_entry(&mut self, parent: u32, name: &str, template: RawEntry) -> Result<()> {
        validate_name(name)?;
        let mut dir = self.load_dir(parent)?;
        let found = scan(&dir);
        if find(&found, name).is_some() {
            return Err(FsError::AlreadyExists);
        }
        let existing: Vec<[u8; 11]> = found.iter().map(|f| f.raw.name).collect();
        let short = make_short(name, &existing)?;
        let mut entries = if exact_short(name) == Some(short) {
            Vec::new()
        } else {
            encode_lfn(name, &short)
        };
        let mut raw = template;
        raw.name = short;
        raw.ntres = 0;
        let mut b = [0u8; ENTRY_SIZE];
        raw.write(&mut b);
        entries.push(b);

        let slot = loop {
            if let Some(s) = dir.free_run(entries.len()) {
                break s;
            }
            self.extend_dir(&mut dir)?;
        };
        for (k, e) in entries.iter().enumerate() {
            dir.slot_mut(slot + k).copy_from_slice(e);
        }
        self.store_dir(&dir)
    }

    /// Libera los clusters de una carpeta y de todo su contenido.
    fn remove_tree(&mut self, cluster: u32) -> Result<()> {
        let dir = self.load_dir(cluster)?;
        for f in scan(&dir).iter().filter(|f| !f.raw.is_dot()) {
            if f.raw.is_dir() {
                self.remove_tree(f.raw.cluster)?;
            } else {
                self.free_chain(f.raw.cluster)?;
            }
        }
        self.free_chain(cluster)
    }

    /// Actualiza el sector FSInfo (espacio libre), que leen Windows y otros sistemas.
    fn sync_fsinfo(&mut self) -> Result<()> {
        if self.fsinfo_sector == 0 || self.fsinfo_sector >= self.reserved_sectors {
            return Ok(());
        }
        let mut buf = [0u8; SECTOR_SIZE];
        self.dev.read(self.fsinfo_sector as u64, &mut buf)?;
        if u32_at(&buf, 0) != FSINFO_LEAD_SIG || u32_at(&buf, 484) != FSINFO_STRUCT_SIG {
            return Ok(());
        }
        buf[488..492].copy_from_slice(&self.free_clusters.to_le_bytes());
        buf[492..496].copy_from_slice(&self.next_free.to_le_bytes());
        Ok(self.dev.write(self.fsinfo_sector as u64, &buf)?)
    }

    // --- API pública --------------------------------------------------------------------------

    /// Contenido de una carpeta (sin "." ni "..").
    pub fn list(&mut self, path: &str) -> Result<Vec<DirEntry>> {
        let parts = components(path)?;
        let cluster = self.resolve_dir(&parts)?;
        let dir = self.load_dir(cluster)?;
        Ok(scan(&dir)
            .into_iter()
            .filter(|f| !f.raw.is_dot())
            .map(|f| f.entry)
            .collect())
    }

    pub fn stat(&mut self, path: &str) -> Result<DirEntry> {
        if components(path)?.is_empty() {
            return Ok(DirEntry {
                name: String::from("/"),
                short_name: String::new(),
                is_dir: true,
                size: 0,
                first_cluster: self.root_cluster,
                created: Timestamp::EPOCH,
                modified: Timestamp::EPOCH,
                read_only: false,
                hidden: false,
            });
        }
        Ok(self.lookup(path)?.1.entry)
    }

    pub fn exists(&mut self, path: &str) -> bool {
        self.stat(path).is_ok()
    }

    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        self.read_prefix(path, usize::MAX)
    }

    /// Lee hasta `max` bytes del principio del archivo (para vistas previas).
    pub fn read_prefix(&mut self, path: &str, max: usize) -> Result<Vec<u8>> {
        let (_, f) = self.lookup(path)?;
        if f.raw.is_dir() {
            return Err(FsError::IsADirectory);
        }
        let len = (f.raw.size as usize).min(max);
        let cs = self.cluster_size() as usize;
        let chain = self.chain(f.raw.cluster)?;
        if (chain.len() * cs) < f.raw.size as usize {
            return Err(FsError::Corrupt(
                "el archivo tiene menos clusters que su tamaño",
            ));
        }
        let mut data = vec![0u8; len.div_ceil(cs) * cs];
        for (i, &c) in chain.iter().take(len.div_ceil(cs)).enumerate() {
            self.read_cluster(c, &mut data[i * cs..(i + 1) * cs])?;
        }
        data.truncate(len);
        Ok(data)
    }

    /// Crea un archivo nuevo con `data`. Falla si ya existe.
    pub fn create_file(&mut self, path: &str, data: &[u8], now: Timestamp) -> Result<()> {
        if self.exists(path) {
            return Err(FsError::AlreadyExists);
        }
        self.write_file(path, data, now)
    }

    /// Crea el archivo o reemplaza su contenido.
    pub fn write_file(&mut self, path: &str, data: &[u8], now: Timestamp) -> Result<()> {
        let parts = components(path)?;
        let (name, parents) = parts.split_last().ok_or(FsError::RootNotAllowed)?;
        validate_name(name)?;
        let parent = self.resolve_dir(parents)?;
        let mut dir = self.load_dir(parent)?;
        let found = scan(&dir);
        match find(&found, name) {
            Some(f) if f.raw.is_dir() => Err(FsError::IsADirectory),
            Some(f) => {
                // Reemplazo: primero se verifica que entre, contando lo que libera el viejo.
                let old = self.chain(f.raw.cluster)?.len() as u32;
                if self.clusters_for(data.len()) > self.free_clusters + old {
                    return Err(FsError::NoSpace);
                }
                self.free_chain(f.raw.cluster)?;
                let first = self.alloc_and_write(data)?;
                let (date, time) = now.to_fat();
                let mut raw = f.raw;
                raw.cluster = first;
                raw.size = data.len() as u32;
                raw.wrt_date = date;
                raw.wrt_time = time;
                raw.acc_date = date;
                raw.attr |= ATTR_ARCHIVE;
                raw.write(dir.slot_mut(f.short_slot));
                self.store_dir(&dir)?;
                self.sync_fsinfo()
            }
            None => {
                let first = self.alloc_and_write(data)?;
                let raw = new_raw(ATTR_ARCHIVE, first, data.len() as u32, now);
                if let Err(e) = self.create_entry(parent, name, raw) {
                    self.free_chain(first)?;
                    return Err(e);
                }
                self.sync_fsinfo()
            }
        }
    }

    /// Crea una carpeta vacía (con sus entradas "." y "..").
    pub fn mkdir(&mut self, path: &str, now: Timestamp) -> Result<()> {
        let parts = components(path)?;
        let (name, parents) = parts.split_last().ok_or(FsError::RootNotAllowed)?;
        validate_name(name)?;
        let parent = self.resolve_dir(parents)?;
        let cluster = self.alloc_clusters(1)?[0];
        let mut buf = vec![0u8; self.cluster_size() as usize];
        let mut dot = new_raw(ATTR_DIRECTORY, cluster, 0, now);
        dot.name = *b".          ";
        dot.write(&mut buf[0..ENTRY_SIZE]);
        let parent_ref = if parent == self.root_cluster {
            0
        } else {
            parent
        };
        let mut dotdot = new_raw(ATTR_DIRECTORY, parent_ref, 0, now);
        dotdot.name = *b"..         ";
        dotdot.write(&mut buf[ENTRY_SIZE..2 * ENTRY_SIZE]);
        self.write_cluster(cluster, &buf)?;
        if let Err(e) = self.create_entry(parent, name, new_raw(ATTR_DIRECTORY, cluster, 0, now)) {
            self.free_chain(cluster)?;
            return Err(e);
        }
        self.sync_fsinfo()
    }

    /// Cambia el nombre dentro de la misma carpeta. Conserva fechas y contenido.
    pub fn rename(&mut self, path: &str, new_name: &str) -> Result<()> {
        validate_name(new_name)?;
        let (parent, f) = self.lookup(path)?;
        if f.entry.name == new_name {
            return Ok(());
        }
        let mut dir = self.load_dir(parent)?;
        let found = scan(&dir);
        if found.iter().any(|o| {
            o.short_slot != f.short_slot && !o.raw.is_dot() && same_name(&o.entry.name, new_name)
        }) {
            return Err(FsError::AlreadyExists);
        }
        let saved = dir.data[f.first_slot * ENTRY_SIZE..(f.short_slot + 1) * ENTRY_SIZE].to_vec();
        dir.mark_deleted(&f);
        self.store_dir(&dir)?;
        if let Err(e) = self.create_entry(parent, new_name, f.raw) {
            // No entró el nombre nuevo (sin espacio para agrandar la carpeta): se restaura el viejo.
            let mut dir = self.load_dir(parent)?;
            dir.data[f.first_slot * ENTRY_SIZE..(f.short_slot + 1) * ENTRY_SIZE]
                .copy_from_slice(&saved);
            self.store_dir(&dir)?;
            return Err(e);
        }
        self.sync_fsinfo()
    }

    /// Mueve un archivo o carpeta a otra carpeta, con el mismo nombre.
    pub fn move_to(&mut self, path: &str, dest_dir: &str) -> Result<()> {
        let src_parts = components(path)?;
        let dest_parts = components(dest_dir)?;
        let (parent, f) = self.lookup(path)?;
        if f.raw.is_dir()
            && dest_parts.len() >= src_parts.len()
            && src_parts
                .iter()
                .zip(&dest_parts)
                .all(|(a, b)| same_name(a, b))
        {
            return Err(FsError::MoveIntoItself);
        }
        let dest = self.resolve_dir(&dest_parts)?;
        if dest == parent {
            return Ok(());
        }
        // Primero se crea en el destino y después se borra del origen: si algo falla a la mitad,
        // queda duplicado en vez de perdido.
        self.create_entry(dest, &f.entry.name, f.raw)?;
        let mut dir = self.load_dir(parent)?;
        dir.mark_deleted(&f);
        self.store_dir(&dir)?;
        if f.raw.is_dir() {
            let mut moved = self.load_dir(f.raw.cluster)?;
            let dest_ref = if dest == self.root_cluster { 0 } else { dest };
            if let Some(dd) = scan(&moved).iter().find(|e| e.raw.name == *b"..         ") {
                let mut raw = dd.raw;
                raw.cluster = dest_ref;
                raw.write(moved.slot_mut(dd.short_slot));
                self.store_dir(&moved)?;
            }
        }
        self.sync_fsinfo()
    }

    /// Borra un archivo, o una carpeta con todo su contenido. Es definitivo: la papelera la
    /// maneja quien usa el sistema de archivos (moviendo a /Papelera).
    pub fn remove(&mut self, path: &str) -> Result<()> {
        let (parent, f) = self.lookup(path)?;
        if f.raw.is_dir() {
            self.remove_tree(f.raw.cluster)?;
        } else {
            self.free_chain(f.raw.cluster)?;
        }
        let mut dir = self.load_dir(parent)?;
        dir.mark_deleted(&f);
        self.store_dir(&dir)?;
        self.sync_fsinfo()
    }
    /// Copia un archivo, o una carpeta con todo su contenido, a `dest` (la ruta nueva completa,
    /// que no tiene que existir). Si algo falla a la mitad (por ejemplo, se llena el disco), se
    /// borra lo que se llegó a copiar: nunca queda una copia a medias.
    pub fn copy(&mut self, src: &str, dest: &str, now: Timestamp) -> Result<()> {
        let src_parts = components(src)?;
        let dest_parts = components(dest)?;
        if src_parts.is_empty() || dest_parts.is_empty() {
            return Err(FsError::RootNotAllowed);
        }
        let entry = self.stat(src)?;
        if entry.is_dir
            && dest_parts.len() >= src_parts.len()
            && src_parts
                .iter()
                .zip(&dest_parts)
                .all(|(a, b)| same_name(a, b))
        {
            return Err(FsError::MoveIntoItself);
        }
        if self.exists(dest) {
            return Err(FsError::AlreadyExists);
        }
        let result = self.copy_tree(src, dest, &entry, now);
        if result.is_err() && self.exists(dest) {
            let _ = self.remove(dest);
        }
        result
    }

    fn copy_tree(&mut self, src: &str, dest: &str, entry: &DirEntry, now: Timestamp) -> Result<()> {
        if !entry.is_dir {
            let data = self.read_file(src)?;
            return self.create_file(dest, &data, now);
        }
        self.mkdir(dest, now)?;
        for child in self.list(src)? {
            let from = alloc::format!("{}/{}", src.trim_end_matches('/'), child.name);
            let to = alloc::format!("{}/{}", dest.trim_end_matches('/'), child.name);
            self.copy_tree(&from, &to, &child, now)?;
        }
        Ok(())
    }
}
