//! Errores del sistema de archivos, con mensajes en castellano para mostrar al usuario.

use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    /// El disco no respondió.
    Io,
    /// El disco no tiene un FAT32 (puede ser FAT12/16, NTFS o estar vacío).
    NotFat32,
    /// Una estructura en disco es imposible (cadena rota, tamaño que no coincide…).
    Corrupt(&'static str),
    NotFound,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    InvalidName,
    NoSpace,
    /// La raíz no se puede borrar, mover ni renombrar.
    RootNotAllowed,
    /// Mover una carpeta adentro de sí misma.
    MoveIntoItself,
}

pub type Result<T> = core::result::Result<T, FsError>;

impl From<crate::IoError> for FsError {
    fn from(_: crate::IoError) -> Self {
        FsError::Io
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            FsError::Io => "error de lectura o escritura en el disco",
            FsError::NotFat32 => "el disco no tiene un sistema de archivos FAT32",
            FsError::Corrupt(detalle) => return write!(f, "el disco está dañado: {detalle}"),
            FsError::NotFound => "no existe",
            FsError::AlreadyExists => "ya existe un elemento con ese nombre",
            FsError::NotADirectory => "no es una carpeta",
            FsError::IsADirectory => "es una carpeta",
            FsError::InvalidName => "nombre inválido",
            FsError::NoSpace => "no queda espacio en el disco",
            FsError::RootNotAllowed => "la carpeta raíz no se puede modificar",
            FsError::MoveIntoItself => "no se puede mover una carpeta adentro de sí misma",
        };
        f.write_str(msg)
    }
}
