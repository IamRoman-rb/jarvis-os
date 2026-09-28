//! La memoria de JARVIS-OS (hito K8): la lógica de la paginación propia, sin hardware.
//!
//! - [`frames`]: qué marcos de 4 KiB de la RAM física están libres (un mapa de bits).
//! - [`table`]: las tablas de páginas de x86_64 (4 niveles): mapear, traducir, recorrer.
//! - [`elf`]: los segmentos de la imagen del kernel, para darle a cada uno sus permisos
//!   (código: leer y ejecutar; datos: leer y escribir, nunca ejecutar).
//!
//! El binario del kernel solo le presta la memoria física (el trait [`table::PhysMem`]) y carga
//! la tabla nueva en CR3. Los tests corren la misma lógica sobre una "RAM" de mentira.
//!
//! Referencias: Intel SDM vol. 3A cap. 4 (paginación de 4 niveles),
//! <https://os.phil-opp.com/paging-introduction/> y la especificación de ELF64.

#![no_std]

extern crate alloc;

pub mod elf;
pub mod frames;
pub mod table;

/// Tamaño de una página (y de un marco) chica.
pub const PAGE: u64 = 4096;
