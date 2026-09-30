//! Lo que la ABI de Linux necesita del resto del sistema. El kernel lo implementa con las tablas
//! de páginas, el reloj y las colas hacia el escritorio (archivos, consola) y la red; los tests,
//! con memoria y un disco de mentira.

use alloc::string::String;
use alloc::vec::Vec;

use crate::mm::Pages;

/// Por qué no se pudo copiar de o hacia la memoria del proceso.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// La página (esa dirección, alineada) no tiene memoria todavía: si es de una zona válida,
    /// hay que ponerle una y reintentar.
    Missing(u64),
    /// Existe pero no se puede (del kernel, o de solo lectura al escribir).
    Denied,
}

/// Pedidos al "servidor de archivos" (la tarea del escritorio, dueña del FAT32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    Stat(String),
    Read(String),
    /// Crea o reemplaza el archivo entero.
    Write(String, Vec<u8>),
    List(String),
    Mkdir(String),
    /// Borrar un archivo: va a la Papelera (regla 13).
    Remove(String),
    Rmdir(String),
    Rename(String, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub is_dir: bool,
    pub size: u64,
    /// Segundos desde 1970 (UTC).
    pub mtime: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileReply {
    Meta(Meta),
    Data(Vec<u8>),
    List(Vec<DirEntry>),
    Done,
    /// Un `errno` (positivo).
    Error(i64),
}

/// Pedidos a la red (TCP sobre IPv4). Pasan por el firewall con el nombre del programa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetOp {
    Connect {
        ip: [u8; 4],
        port: u16,
    },
    Send {
        id: u32,
        data: Vec<u8>,
    },
    /// Espera hasta que llegue algo (o se cierre: vacío).
    Recv {
        id: u32,
        max: usize,
    },
    Close {
        id: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetReply {
    Connected(u32),
    Sent(usize),
    Data(Vec<u8>),
    Done,
    Error(i64),
}

pub trait System: Pages {
    /// Copia desde la memoria del proceso.
    fn read_user(&mut self, addr: u64, buf: &mut [u8]) -> Result<(), Fault>;
    /// Copia hacia la memoria del proceso. Con `force` se escribe aunque la página sea de solo
    /// lectura (lo usa el cargador para poner el código).
    fn write_user(&mut self, addr: u64, data: &[u8], force: bool) -> Result<(), Fault>;
    /// Pone una página en cero en `page` con esos permisos (`prot::*`). `false`: sin memoria.
    fn map_page(&mut self, page: u64, prot: u32) -> bool;
    /// La base del registro FS (el TLS del programa: `arch_prctl`).
    fn set_fs(&mut self, base: u64);
    /// Nanosegundos desde 1970 (UTC).
    fn realtime_ns(&mut self) -> u64;
    /// Nanosegundos desde que arrancó el sistema.
    fn monotonic_ns(&mut self) -> u64;
    fn random(&mut self, buf: &mut [u8]);
    fn sleep_ns(&mut self, ns: u64);
    fn file(&mut self, op: FileOp) -> FileReply;
    /// La salida del programa (la Terminal que lo lanzó).
    fn console_write(&mut self, data: &[u8]);
    /// Espera una línea de la Terminal (con su `\n`). Vacío: fin de la entrada (Ctrl+D).
    fn console_read(&mut self, max: usize) -> Vec<u8>;
    /// Filas y columnas de la Terminal.
    fn console_size(&mut self) -> (u16, u16);
    fn net(&mut self, op: NetOp) -> NetReply;
    /// Algo para el log del sistema (llamadas que no existen, errores del programa).
    fn log(&mut self, msg: &str);
}
