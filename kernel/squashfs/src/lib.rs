//! Lector de **squashfs 4.0**, el sistema de archivos comprimido y de solo lectura de los
//! paquetes `.snap` de Ubuntu (y de las AppImage y muchos firmwares). ADR 0021.
//!
//! Cómo está armado un squashfs (documentado en `squashfs-tools` y en el kernel de Linux,
//! `Documentation/filesystems/squashfs.rst`):
//!
//! - **Superbloque** (96 bytes al principio): el tamaño de bloque, la compresión y dónde
//!   empiezan las tablas.
//! - **Bloques de metadatos**: las tablas de inodos y de directorios se guardan en pedazos de
//!   hasta 8 KiB, cada uno con 2 bytes de cabecera (el bit 15 dice si está sin comprimir; el resto,
//!   cuánto mide). Son un "flujo" continuo: un inodo puede empezar en un bloque y seguir en el otro.
//! - **Referencia a un inodo**: 64 bits; los de arriba dicen en qué bloque de metadatos empieza
//!   (relativo a la tabla de inodos) y los 16 de abajo, en qué byte de ese bloque descomprimido.
//! - **Inodos**: carpeta (dónde está su lista en la tabla de directorios), archivo (dónde
//!   empiezan sus bloques de datos, cuánto mide cada uno y si su final está en un "fragmento") y
//!   enlace simbólico (el destino).
//! - **Fragmentos**: los finales chicos de varios archivos juntos en un mismo bloque (así no se
//!   desperdicia un bloque de 128 KiB por un final de 3 bytes).
//! - **Tabla de ids**: los uid/gid de verdad (los inodos guardan solo un índice).
//!
//! Esta crate lee la imagen **entera en memoria** (un snap que se baja mide pocos MB) y no
//! confía en ella: cada posición se verifica y todo lo descomprimido tiene un tope.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

const MAGIC: u32 = 0x7371_7368; // "hsqs"
const METADATA_MAX: usize = 8192;
/// Lo más grande que se acepta descomprimir de un archivo (un snap normal es mucho menos).
pub const MAX_FILE: u64 = 256 * 1024 * 1024;
/// Carpetas anidadas como mucho (una imagen hecha para colgar no puede recorrerse para siempre).
const MAX_DEPTH: usize = 64;
/// Entradas en total como mucho.
const MAX_ENTRIES: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Gzip,
    Lzma,
    Lzo,
    Xz,
    Lz4,
    Zstd,
}

impl Compression {
    fn from_id(id: u16) -> Result<Compression, Error> {
        Ok(match id {
            1 => Compression::Gzip,
            2 => Compression::Lzma,
            3 => Compression::Lzo,
            4 => Compression::Xz,
            5 => Compression::Lz4,
            6 => Compression::Zstd,
            other => return Err(Error::Bad(format!("compresión desconocida ({other})"))),
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Compression::Gzip => "gzip",
            Compression::Lzma => "lzma",
            Compression::Lzo => "lzo",
            Compression::Xz => "xz",
            Compression::Lz4 => "lz4",
            Compression::Zstd => "zstd",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotSquashfs,
    /// Otra versión del formato (solo se lee la 4.0, la de los snaps desde 2009).
    Version(u16, u16),
    /// Una compresión que esta crate no sabe descomprimir (lzo, zstd, lzma viejo).
    Unsupported(Compression),
    Truncated,
    TooBig,
    Bad(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NotSquashfs => f.write_str("no es un squashfs"),
            Error::Version(a, b) => write!(f, "squashfs {a}.{b} (solo se lee el 4.0)"),
            Error::Unsupported(c) => write!(f, "compresión {} no soportada", c.name()),
            Error::Truncated => f.write_str("la imagen está cortada"),
            Error::TooBig => f.write_str("demasiado grande al descomprimir"),
            Error::Bad(why) => write!(f, "imagen dañada: {why}"),
        }
    }
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, Error> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated)
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, Error> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated)
}

fn u64_at(d: &[u8], at: usize) -> Result<u64, Error> {
    d.get(at..at + 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
        .ok_or(Error::Truncated)
}

/// Descomprime un bloque (de datos o de metadatos) con la compresión de la imagen.
pub fn decompress(c: Compression, data: &[u8], max: usize) -> Result<Vec<u8>, Error> {
    match c {
        Compression::Gzip => jarvis_image::inflate::zlib_decompress(data, max)
            .map_err(|_| Error::Bad("bloque gzip dañado".into())),
        Compression::Xz => xz(data, max),
        Compression::Lz4 => lz4_flex::block::decompress(data, max)
            .map_err(|_| Error::Bad("bloque lz4 dañado".into())),
        other => Err(Error::Unsupported(other)),
    }
}

fn xz(data: &[u8], max: usize) -> Result<Vec<u8>, Error> {
    let mut dec: Box<xz4rust::XzDecoder<'static>> =
        xz4rust::XzDecoder::in_heap_with_alloc_dict_size(xz4rust::DICT_SIZE_MIN, 64 << 20);
    let mut out = Vec::new();
    let mut buf = alloc::vec![0u8; 64 * 1024];
    let mut at = 0usize;
    loop {
        match dec.decode(&data[at..], &mut buf) {
            Ok(xz4rust::XzNextBlockResult::NeedMoreData(used, made)) => {
                at += used;
                out.extend_from_slice(&buf[..made]);
                if used == 0 && made == 0 {
                    return Err(Error::Bad("bloque xz cortado".into()));
                }
            }
            Ok(xz4rust::XzNextBlockResult::EndOfStream(_, made)) => {
                out.extend_from_slice(&buf[..made]);
                return if out.len() > max {
                    Err(Error::TooBig)
                } else {
                    Ok(out)
                };
            }
            Err(_) => return Err(Error::Bad("bloque xz dañado".into())),
        }
        if out.len() > max {
            return Err(Error::TooBig);
        }
    }
}

/// Qué es una entrada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Symlink(String),
    /// Dispositivo, tubería o socket: en un snap no deberían estar; se informan y se saltean.
    Other,
}

/// Una entrada del árbol, con su ruta (`/bin/hola`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    inode: Inode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Inode {
    Dir {
        block: u32,
        offset: u16,
        size: u32,
    },
    File {
        start: u64,
        size: u64,
        fragment: u32,
        frag_offset: u32,
        blocks: Vec<u32>,
    },
    Symlink(String),
    Other,
}

/// Una imagen squashfs abierta.
pub struct SquashFs<'a> {
    data: &'a [u8],
    pub compression: Compression,
    pub block_size: u32,
    flags: u16,
    root: u64,
    inode_table: u64,
    dir_table: u64,
    fragments: Vec<(u64, u32)>,
    ids: Vec<u32>,
}

const NO_FRAGMENT: u32 = 0xFFFF_FFFF;
const FLAG_COMPRESSOR_OPTIONS: u16 = 0x0400;

impl<'a> SquashFs<'a> {
    pub fn open(data: &'a [u8]) -> Result<SquashFs<'a>, Error> {
        if data.len() < 96 || u32_at(data, 0)? != MAGIC {
            return Err(Error::NotSquashfs);
        }
        let (major, minor) = (u16_at(data, 28)?, u16_at(data, 30)?);
        if major != 4 {
            return Err(Error::Version(major, minor));
        }
        let block_size = u32_at(data, 12)?;
        if !(4096..=1 << 20).contains(&block_size) || !block_size.is_power_of_two() {
            return Err(Error::Bad(format!("tamaño de bloque {block_size}")));
        }
        let compression = Compression::from_id(u16_at(data, 20)?)?;
        let mut fs = SquashFs {
            data,
            compression,
            block_size,
            flags: u16_at(data, 24)?,
            root: u64_at(data, 32)?,
            inode_table: u64_at(data, 64)?,
            dir_table: u64_at(data, 72)?,
            fragments: Vec::new(),
            ids: Vec::new(),
        };
        // El algoritmo de compresión se prueba ahora (si no se sabe, se dice antes de empezar).
        if !matches!(
            compression,
            Compression::Gzip | Compression::Xz | Compression::Lz4
        ) {
            return Err(Error::Unsupported(compression));
        }
        let _ = fs.flags & FLAG_COMPRESSOR_OPTIONS; // las opciones no hacen falta para leer
        let id_count = usize::from(u16_at(data, 26)?);
        fs.ids = fs
            .lookup_table(u64_at(data, 48)?, id_count, 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        let frag_count = u32_at(data, 16)? as usize;
        let frag_table = u64_at(data, 80)?;
        if frag_count > 0 && frag_table != u64::MAX {
            let raw = fs.lookup_table(frag_table, frag_count, 16)?;
            fs.fragments = raw
                .as_chunks::<16>()
                .0
                .iter()
                .map(|c| {
                    (
                        u64::from_le_bytes(c[0..8].try_into().unwrap_or([0; 8])),
                        u32::from_le_bytes([c[8], c[9], c[10], c[11]]),
                    )
                })
                .collect();
        }
        Ok(fs)
    }

    /// Una tabla "indexada" (ids, fragmentos): un arreglo de punteros (u64) a bloques de
    /// metadatos con `count` elementos de `size` bytes.
    fn lookup_table(&self, start: u64, count: usize, size: usize) -> Result<Vec<u8>, Error> {
        let total = count.checked_mul(size).ok_or(Error::TooBig)?;
        if total > 64 * 1024 * 1024 {
            return Err(Error::TooBig);
        }
        let blocks = total.div_ceil(METADATA_MAX);
        let mut out = Vec::with_capacity(total);
        for i in 0..blocks {
            let ptr = u64_at(
                self.data,
                usize::try_from(start).map_err(|_| Error::Truncated)? + i * 8,
            )?;
            let (block, _) = self.metadata_block(ptr)?;
            out.extend_from_slice(&block);
        }
        if out.len() < total {
            return Err(Error::Truncated);
        }
        out.truncate(total);
        Ok(out)
    }

    /// Un bloque de metadatos en la posición absoluta `at`: (descomprimido, cuánto ocupa).
    fn metadata_block(&self, at: u64) -> Result<(Vec<u8>, usize), Error> {
        let at = usize::try_from(at).map_err(|_| Error::Truncated)?;
        let header = u16_at(self.data, at)?;
        let len = usize::from(header & 0x7FFF);
        let raw = self
            .data
            .get(at + 2..at + 2 + len)
            .ok_or(Error::Truncated)?;
        let block = if header & 0x8000 != 0 {
            raw.to_vec()
        } else {
            decompress(self.compression, raw, METADATA_MAX)?
        };
        Ok((block, len + 2))
    }

    /// `len` bytes del flujo de metadatos que empieza en el bloque `table + block`, desde el
    /// byte `offset` de ese bloque (puede seguir en los bloques siguientes).
    fn read_metadata(
        &self,
        table: u64,
        block: u64,
        offset: usize,
        len: usize,
    ) -> Result<Vec<u8>, Error> {
        if len > 16 * 1024 * 1024 {
            return Err(Error::TooBig);
        }
        let mut pos = table.checked_add(block).ok_or(Error::Truncated)?;
        let mut out = Vec::with_capacity(len);
        let mut skip = offset;
        while out.len() < len {
            let (data, used) = self.metadata_block(pos)?;
            if skip > data.len() {
                return Err(Error::Bad("posición de metadatos fuera del bloque".into()));
            }
            let take = (len - out.len()).min(data.len() - skip);
            out.extend_from_slice(&data[skip..skip + take]);
            skip = 0;
            pos += used as u64;
            if data.is_empty() {
                return Err(Error::Truncated);
            }
        }
        Ok(out)
    }

    fn id(&self, idx: u16) -> u32 {
        self.ids.get(usize::from(idx)).copied().unwrap_or(0)
    }

    /// Lee el inodo de una referencia.
    fn inode(&self, reference: u64) -> Result<(Inode, u16, u32, u32, u64), Error> {
        let block = reference >> 16;
        let offset = (reference & 0xFFFF) as usize;
        // El inodo más grande (un archivo extendido con su lista de bloques) se lee en dos
        // pasos: primero la parte fija y después la lista.
        let head = self
            .read_metadata(self.inode_table, block, offset, 64)
            .or_else(|_| self.read_metadata(self.inode_table, block, offset, 32))?;
        let kind = u16_at(&head, 0)?;
        let mode = u16_at(&head, 2)?;
        let uid = self.id(u16_at(&head, 4)?);
        let gid = self.id(u16_at(&head, 6)?);
        let b = 16; // después de la cabecera común
        let blocks_for = |size: u64, fragment: u32| -> Result<usize, Error> {
            let bs = u64::from(self.block_size);
            let n = if fragment == NO_FRAGMENT {
                size.div_ceil(bs)
            } else {
                size / bs
            };
            usize::try_from(n).map_err(|_| Error::TooBig)
        };
        Ok(match kind {
            1 => {
                let block = u32_at(&head, b)?;
                let size = u32::from(u16_at(&head, b + 8)?);
                let off = u16_at(&head, b + 10)?;
                (
                    Inode::Dir {
                        block,
                        offset: off,
                        size,
                    },
                    mode,
                    uid,
                    gid,
                    0,
                )
            }
            8 => {
                let size = u32_at(&head, b + 4)?;
                let block = u32_at(&head, b + 8)?;
                let off = u16_at(&head, b + 18)?;
                (
                    Inode::Dir {
                        block,
                        offset: off,
                        size,
                    },
                    mode,
                    uid,
                    gid,
                    0,
                )
            }
            2 | 9 => {
                let (start, size, fragment, frag_offset, fixed) = if kind == 2 {
                    (
                        u64::from(u32_at(&head, b)?),
                        u64::from(u32_at(&head, b + 12)?),
                        u32_at(&head, b + 4)?,
                        u32_at(&head, b + 8)?,
                        b + 16,
                    )
                } else {
                    (
                        u64_at(&head, b)?,
                        u64_at(&head, b + 8)?,
                        u32_at(&head, b + 28)?,
                        u32_at(&head, b + 32)?,
                        b + 40,
                    )
                };
                if size > MAX_FILE {
                    return Err(Error::TooBig);
                }
                let n = blocks_for(size, fragment)?;
                let raw = self.read_metadata(self.inode_table, block, offset, fixed + n * 4)?;
                let blocks = raw[fixed..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes(*c))
                    .collect();
                (
                    Inode::File {
                        start,
                        size,
                        fragment,
                        frag_offset,
                        blocks,
                    },
                    mode,
                    uid,
                    gid,
                    size,
                )
            }
            3 | 10 => {
                let len = u32_at(&head, b + 4)? as usize;
                if len > 4096 {
                    return Err(Error::Bad("enlace demasiado largo".into()));
                }
                let raw = self.read_metadata(self.inode_table, block, offset, b + 8 + len)?;
                let target = String::from_utf8_lossy(&raw[b + 8..]).to_string();
                let size = len as u64;
                (Inode::Symlink(target), mode, uid, gid, size)
            }
            4..=7 | 11..=14 => (Inode::Other, mode, uid, gid, 0),
            other => return Err(Error::Bad(format!("tipo de inodo {other}"))),
        })
    }

    /// Lo que hay en una carpeta: (nombre, referencia al inodo).
    fn list(&self, block: u32, offset: u16, size: u32) -> Result<Vec<(String, u64)>, Error> {
        // `size` cuenta 3 bytes de más (por "." y "..", que no están guardados).
        let len = size.saturating_sub(3) as usize;
        if len == 0 {
            return Ok(Vec::new());
        }
        let raw = self.read_metadata(self.dir_table, u64::from(block), usize::from(offset), len)?;
        let mut out = Vec::new();
        let mut at = 0usize;
        while at + 12 <= raw.len() {
            let count = u32_at(&raw, at)? as usize + 1;
            let start = u32_at(&raw, at + 4)?;
            at += 12;
            if count > 256 {
                return Err(Error::Bad("cabecera de directorio inválida".into()));
            }
            for _ in 0..count {
                let off = u16_at(&raw, at)?;
                let name_len = usize::from(u16_at(&raw, at + 6)?) + 1;
                let name = raw.get(at + 8..at + 8 + name_len).ok_or(Error::Truncated)?;
                let name = String::from_utf8_lossy(name).to_string();
                at += 8 + name_len;
                if name.is_empty() || name == "." || name == ".." || name.contains('/') {
                    return Err(Error::Bad(format!("nombre inválido «{name}»")));
                }
                out.push((name, (u64::from(start) << 16) | u64::from(off)));
            }
        }
        Ok(out)
    }

    /// Todo el árbol, en orden (las carpetas antes que su contenido).
    pub fn entries(&self) -> Result<Vec<Entry>, Error> {
        let mut out = Vec::new();
        let mut stack: Vec<(String, u64, usize)> = alloc::vec![(String::new(), self.root, 0)];
        while let Some((path, reference, depth)) = stack.pop() {
            if depth > MAX_DEPTH || out.len() > MAX_ENTRIES {
                return Err(Error::Bad("árbol demasiado grande o con ciclos".into()));
            }
            let (inode, mode, uid, gid, size) = self.inode(reference)?;
            let kind = match &inode {
                Inode::Dir { .. } => Kind::Dir,
                Inode::File { .. } => Kind::File,
                Inode::Symlink(t) => Kind::Symlink(t.clone()),
                Inode::Other => Kind::Other,
            };
            if let Inode::Dir {
                block,
                offset,
                size,
            } = &inode
            {
                let mut children = self.list(*block, *offset, *size)?;
                // Al revés, así salen en orden alfabético de la pila.
                children.reverse();
                for (name, r) in children {
                    stack.push((format!("{path}/{name}"), r, depth + 1));
                }
            }
            out.push(Entry {
                path: if path.is_empty() { "/".into() } else { path },
                kind,
                mode: mode & 0o7777,
                uid,
                gid,
                size,
                inode,
            });
        }
        Ok(out)
    }

    /// El contenido de un archivo.
    pub fn read(&self, e: &Entry) -> Result<Vec<u8>, Error> {
        let Inode::File {
            start,
            size,
            fragment,
            frag_offset,
            blocks,
        } = &e.inode
        else {
            return Err(Error::Bad(format!("{} no es un archivo", e.path)));
        };
        let size = usize::try_from(*size).map_err(|_| Error::TooBig)?;
        let bs = self.block_size as usize;
        let mut out = Vec::with_capacity(size);
        let mut pos = usize::try_from(*start).map_err(|_| Error::Truncated)?;
        for &b in blocks {
            let len = (b & 0x00FF_FFFF) as usize;
            if len == 0 {
                // Bloque "ralo": todos ceros.
                let n = bs.min(size - out.len());
                out.resize(out.len() + n, 0);
                continue;
            }
            let raw = self.data.get(pos..pos + len).ok_or(Error::Truncated)?;
            pos += len;
            if b & (1 << 24) != 0 {
                out.extend_from_slice(raw);
            } else {
                out.extend_from_slice(&decompress(self.compression, raw, bs)?);
            }
            if out.len() > size {
                return Err(Error::Bad(format!("{}: bloques de más", e.path)));
            }
        }
        if *fragment != NO_FRAGMENT && out.len() < size {
            let &(frag_start, frag_size) = self
                .fragments
                .get(*fragment as usize)
                .ok_or_else(|| Error::Bad("fragmento inexistente".into()))?;
            let at = usize::try_from(frag_start).map_err(|_| Error::Truncated)?;
            let len = (frag_size & 0x00FF_FFFF) as usize;
            let raw = self.data.get(at..at + len).ok_or(Error::Truncated)?;
            let block = if frag_size & (1 << 24) != 0 {
                raw.to_vec()
            } else {
                decompress(self.compression, raw, bs)?
            };
            let from = *frag_offset as usize;
            let need = size - out.len();
            let piece = block.get(from..from + need).ok_or(Error::Truncated)?;
            out.extend_from_slice(piece);
        }
        if out.len() != size {
            return Err(Error::Bad(format!(
                "{}: mide {} y no {size}",
                e.path,
                out.len()
            )));
        }
        Ok(out)
    }

    /// La entrada de una ruta (`/meta/snap.yaml`).
    pub fn find(&self, entries: &'a [Entry], path: &str) -> Option<&'a Entry> {
        entries.iter().find(|e| e.path == path)
    }
}

/// Resuelve un enlace simbólico relativo a la carpeta del enlace (`/usr/bin/x` → `../lib/y` =
/// `/usr/lib/y`). `None` si sale de la imagen.
pub fn resolve_link(link_path: &str, target: &str) -> Option<String> {
    let base = if target.starts_with('/') {
        String::new()
    } else {
        link_path
            .rsplit_once('/')
            .map_or("", |(d, _)| d)
            .to_string()
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in base.split('/').chain(target.split('/')) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    let mut out = String::new();
    for p in parts {
        out.push('/');
        out.push_str(p);
    }
    Some(if out.is_empty() { "/".into() } else { out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enlaces() {
        assert_eq!(
            resolve_link("/usr/bin/x", "../lib/y").as_deref(),
            Some("/usr/lib/y")
        );
        assert_eq!(resolve_link("/a/b", "c").as_deref(), Some("/a/c"));
        assert_eq!(resolve_link("/a/b", "/etc/z").as_deref(), Some("/etc/z"));
        assert_eq!(resolve_link("/a", "../../x"), None);
    }

    #[test]
    fn basura() {
        assert_eq!(SquashFs::open(b"no").err(), Some(Error::NotSquashfs));
        let mut sb = [0u8; 96];
        sb[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        sb[28] = 3;
        assert_eq!(SquashFs::open(&sb).err(), Some(Error::Version(3, 0)));
    }
}
