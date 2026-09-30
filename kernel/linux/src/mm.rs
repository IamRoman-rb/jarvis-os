//! El mapa de memoria de un proceso: qué zonas de su espacio de direcciones existen y con qué
//! permisos (las "VMA" de Linux).
//!
//! Que una zona exista no quiere decir que tenga páginas: la memoria se asigna **al primer
//! uso**. Reservar 8 MiB de pila o un `mmap` de 1 GiB cuesta nada hasta que el programa toca una
//! página; ahí la CPU da un fallo de página, el kernel mira acá si la dirección es de una zona
//! válida y, si lo es, pone una página en cero y el programa sigue sin enterarse. Si no es de
//! ninguna zona (o se escribe donde no se puede), es una violación de segmento.
//!
//! El espacio del proceso son los primeros 512 GiB (la entrada 0 de la PML4, ADR 0010):
//!
//! ```text
//! 0x0000_0000_0000 ── nada (un puntero nulo tiene que fallar)
//! 0x0000_0040_0000 ── el programa (ELF) y, justo después, el heap (brk) que crece hacia arriba
//!        ...
//! MMAP_TOP         ── los mmap, que se asignan de arriba hacia abajo
//! STACK_TOP-8 MiB  ── la pila (crece hacia abajo)
//! USER_END         ── fin
//! ```

use alloc::vec::Vec;

use crate::abi::prot;

pub const PAGE: u64 = 4096;
/// Fin del espacio del proceso: la entrada 0 de la PML4.
pub const USER_END: u64 = 0x80_0000_0000;
pub const STACK_TOP: u64 = USER_END - PAGE;
pub const STACK_SIZE: u64 = 8 * 1024 * 1024;
/// Los `mmap` sin dirección se buscan de acá para abajo (dejando lugar libre bajo la pila).
pub const MMAP_TOP: u64 = STACK_TOP - STACK_SIZE - 0x1000_0000;
/// El heap no puede crecer más que esto (así no alcanza a los mmap).
pub const BRK_MAX: u64 = 0x10_0000_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Una parte del programa (ELF).
    Program,
    Heap,
    Stack,
    Anon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vma {
    pub start: u64,
    pub end: u64,
    pub prot: u32,
    pub kind: Kind,
}

/// Lo que el mapa le pide al sistema cuando cambia (las páginas reales las maneja el kernel).
pub trait Pages {
    /// Liberar las páginas que haya en `[start, end)`.
    fn unmap(&mut self, start: u64, end: u64);
    /// Cambiar los permisos de las páginas que haya en `[start, end)`.
    fn protect(&mut self, start: u64, end: u64, prot: u32);
}

pub fn page_down(a: u64) -> u64 {
    a & !(PAGE - 1)
}

pub fn page_up(a: u64) -> Option<u64> {
    a.checked_add(PAGE - 1).map(page_down)
}

#[derive(Clone, Debug, Default)]
pub struct Mm {
    /// Ordenadas por dirección y sin superponerse.
    vmas: Vec<Vma>,
    brk_start: u64,
    brk: u64,
}

impl Mm {
    pub fn new() -> Mm {
        Mm::default()
    }

    pub fn vmas(&self) -> &[Vma] {
        &self.vmas
    }

    /// La zona que contiene `addr`.
    pub fn find(&self, addr: u64) -> Option<&Vma> {
        let i = self.vmas.partition_point(|v| v.end <= addr);
        self.vmas.get(i).filter(|v| v.start <= addr)
    }

    /// ¿Un acceso a `addr` es válido? (`write`: para escribir; `exec`: para ejecutar).
    pub fn allows(&self, addr: u64, write: bool, exec: bool) -> Option<u32> {
        let v = self.find(addr)?;
        let ok = if write {
            v.prot & prot::WRITE != 0
        } else if exec {
            v.prot & prot::EXEC != 0
        } else {
            v.prot != prot::NONE
        };
        ok.then_some(v.prot)
    }

    /// Agrega una zona sin mirar si pisa otra (para el cargador, que ya lo verificó).
    fn insert(&mut self, v: Vma) {
        let i = self.vmas.partition_point(|x| x.start < v.start);
        self.vmas.insert(i, v);
    }

    /// Una zona del programa (el cargador ya comprobó que no se superponen; si dos comparten
    /// una página, quedan las dos: los permisos de esa página son la unión).
    pub fn add_program(&mut self, start: u64, end: u64, prot: u32) {
        self.insert(Vma {
            start,
            end,
            prot,
            kind: Kind::Program,
        });
    }

    /// La pila y el comienzo del heap, después de cargar el programa (que termina en `end`).
    pub fn setup(&mut self, program_end: u64) {
        self.insert(Vma {
            start: STACK_TOP - STACK_SIZE,
            end: STACK_TOP,
            prot: prot::READ | prot::WRITE,
            kind: Kind::Stack,
        });
        // Una página de separación entre el programa y el heap.
        let start = page_up(program_end).unwrap_or(program_end) + PAGE;
        self.brk_start = start;
        self.brk = start;
    }

    /// ¿Hay algo en `[start, end)`?
    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.vmas.iter().any(|v| v.start < end && start < v.end)
    }

    /// Saca `[start, end)` de las zonas (partiendo las que queden a medias) y libera sus páginas.
    pub fn unmap(&mut self, start: u64, end: u64, pages: &mut dyn Pages) {
        if start >= end {
            return;
        }
        let mut out = Vec::with_capacity(self.vmas.len() + 1);
        for v in self.vmas.drain(..) {
            if v.end <= start || end <= v.start {
                out.push(v);
                continue;
            }
            if v.start < start {
                out.push(Vma { end: start, ..v });
            }
            if end < v.end {
                out.push(Vma { start: end, ..v });
            }
        }
        self.vmas = out;
        pages.unmap(start, end);
    }

    /// Cambia los permisos de `[start, end)`. Falla (`None`) si hay huecos: Linux da `ENOMEM`.
    pub fn protect(&mut self, start: u64, end: u64, p: u32, pages: &mut dyn Pages) -> Option<()> {
        // Todo el rango tiene que estar cubierto.
        let mut at = start;
        for v in &self.vmas {
            if v.end <= at || v.start >= end {
                continue;
            }
            if v.start > at {
                return None;
            }
            at = v.end;
            if at >= end {
                break;
            }
        }
        if at < end {
            return None;
        }
        let mut out = Vec::with_capacity(self.vmas.len() + 2);
        for v in self.vmas.drain(..) {
            if v.end <= start || end <= v.start {
                out.push(v);
                continue;
            }
            if v.start < start {
                out.push(Vma { end: start, ..v });
            }
            out.push(Vma {
                start: v.start.max(start),
                end: v.end.min(end),
                prot: p,
                kind: v.kind,
            });
            if end < v.end {
                out.push(Vma { start: end, ..v });
            }
        }
        self.vmas = out;
        pages.protect(start, end, p);
        Some(())
    }

    /// `mmap` anónimo. `hint` con `fixed` es obligatorio (pisa lo que haya); sin `fixed`, se
    /// busca un hueco de arriba hacia abajo. Devuelve la dirección.
    pub fn map(
        &mut self,
        hint: u64,
        len: u64,
        p: u32,
        fixed: bool,
        pages: &mut dyn Pages,
    ) -> Option<u64> {
        let len = page_up(len).filter(|&l| l > 0)?;
        let start = if fixed {
            if !hint.is_multiple_of(PAGE) || hint < PAGE || hint.checked_add(len)? > USER_END {
                return None;
            }
            self.unmap(hint, hint + len, pages);
            hint
        } else {
            self.find_gap(len)?
        };
        self.insert(Vma {
            start,
            end: start + len,
            prot: p,
            kind: Kind::Anon,
        });
        Some(start)
    }

    /// El hueco más alto de `len` bytes debajo de [`MMAP_TOP`] y arriba del heap.
    fn find_gap(&self, len: u64) -> Option<u64> {
        let floor = BRK_MAX;
        let mut top = MMAP_TOP;
        // De la zona más alta a la más baja: el hueco entre cada una y `top`.
        for v in self.vmas.iter().rev() {
            if v.start >= top {
                continue;
            }
            if v.end <= top && top - v.end >= len {
                return Some(top - len).filter(|&s| s >= floor);
            }
            top = top.min(v.start);
        }
        top.checked_sub(len).filter(|&s| s >= floor)
    }

    /// `brk`: con 0 (o algo inválido) devuelve el final actual del heap; si no, lo mueve.
    ///
    /// Solo se agrega o se saca el pedazo entre el final viejo y el nuevo: el programa puede
    /// haber puesto otra cosa sobre su heap (musl pone una página de guarda con `mmap` fijo al
    /// principio) y eso se respeta.
    pub fn brk(&mut self, want: u64, pages: &mut dyn Pages) -> u64 {
        if want < self.brk_start || want > BRK_MAX {
            return self.brk;
        }
        let old_end = page_up(self.brk).unwrap_or(self.brk);
        let Some(new_end) = page_up(want) else {
            return self.brk;
        };
        if new_end > old_end {
            if self.overlaps(old_end, new_end) {
                return self.brk;
            }
            // Si la zona de abajo es el heap, se la estira (así no se juntan miles de zonas).
            match self.vmas.iter_mut().find(|v| v.end == old_end) {
                Some(v) if v.kind == Kind::Heap && v.prot == prot::READ | prot::WRITE => {
                    v.end = new_end;
                }
                _ => self.insert(Vma {
                    start: old_end,
                    end: new_end,
                    prot: prot::READ | prot::WRITE,
                    kind: Kind::Heap,
                }),
            }
        } else if new_end < old_end {
            self.unmap(new_end, old_end, pages);
        }
        self.brk = want;
        want
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[derive(Default)]
    struct Log(Vec<(char, u64, u64)>);
    impl Pages for Log {
        fn unmap(&mut self, s: u64, e: u64) {
            self.0.push(('u', s, e));
        }
        fn protect(&mut self, s: u64, e: u64, _p: u32) {
            self.0.push(('p', s, e));
        }
    }

    const RW: u32 = prot::READ | prot::WRITE;

    #[test]
    fn brk_crece_y_se_achica() {
        let mut mm = Mm::new();
        mm.add_program(0x40_0000, 0x40_3000, prot::READ | prot::EXEC);
        mm.setup(0x40_2800);
        let mut log = Log::default();
        let start = mm.brk(0, &mut log);
        assert_eq!(start, 0x40_4000);
        assert!(mm.find(start).is_none(), "el heap vacío no tiene zona");
        assert_eq!(mm.brk(start + 0x2345, &mut log), start + 0x2345);
        assert_eq!(mm.allows(start + 0x2000, true, false), Some(RW));
        assert!(mm.allows(start + 0x3000, false, false).is_none());
        // Achicar libera las páginas que sobran.
        assert_eq!(mm.brk(start + 0x10, &mut log), start + 0x10);
        assert_eq!(log.0, vec![('u', start + 0x1000, start + 0x3000)]);
        // Por debajo del principio: no cambia.
        assert_eq!(mm.brk(0x1000, &mut log), start + 0x10);
    }

    #[test]
    fn brk_respeta_lo_que_el_programa_puso_encima() {
        // Lo que hace el malloc de musl: agranda el heap, pone una página de guarda (PROT_NONE)
        // al principio con un mmap fijo y sigue agrandando.
        let mut mm = Mm::new();
        mm.setup(0x40_2000);
        let mut log = Log::default();
        let start = mm.brk(0, &mut log);
        assert_eq!(mm.brk(start + PAGE, &mut log), start + PAGE);
        mm.map(start, PAGE, prot::NONE, true, &mut log).unwrap();
        for i in 2..50 {
            assert_eq!(mm.brk(start + i * PAGE, &mut log), start + i * PAGE);
        }
        assert!(mm.allows(start, false, false).is_none(), "la guarda sigue");
        assert_eq!(mm.allows(start + 40 * PAGE, true, false), Some(RW));
        // Las zonas no se superponen y el heap nuevo es una sola.
        for w in mm.vmas().windows(2) {
            assert!(w[0].end <= w[1].start, "{:?}", mm.vmas());
        }
        assert_eq!(mm.vmas().iter().filter(|v| v.kind == Kind::Heap).count(), 1);
    }

    #[test]
    fn mmap_busca_huecos_de_arriba_hacia_abajo() {
        let mut mm = Mm::new();
        mm.setup(0x40_0000);
        let mut log = Log::default();
        let a = mm.map(0, 0x3000, RW, false, &mut log).unwrap();
        assert_eq!(a, MMAP_TOP - 0x3000);
        let b = mm.map(0, 100, RW, false, &mut log).unwrap();
        assert_eq!(b, a - PAGE);
        // Liberar la de arriba deja su hueco, que se reusa.
        mm.unmap(a, a + 0x3000, &mut log);
        assert_eq!(
            mm.map(0, 0x2000, RW, false, &mut log),
            Some(MMAP_TOP - 0x2000)
        );
        // Fijo: pisa lo que haya.
        let c = mm.map(b, PAGE, prot::READ, true, &mut log).unwrap();
        assert_eq!(c, b);
        assert_eq!(mm.allows(b, true, false), None);
        assert_eq!(mm.map(0x123, PAGE, RW, true, &mut log), None, "desalineado");
    }

    #[test]
    fn mprotect_y_munmap_parten_zonas() {
        let mut mm = Mm::new();
        let mut log = Log::default();
        let a = mm.map(0, 0x5000, RW, false, &mut log).unwrap();
        mm.protect(a + 0x1000, a + 0x2000, prot::NONE, &mut log)
            .unwrap();
        assert_eq!(mm.vmas().len(), 3);
        assert!(
            mm.allows(a + 0x1000, false, false).is_none(),
            "página de guarda"
        );
        assert!(mm.allows(a + 0x2000, true, false).is_some());
        mm.unmap(a + 0x3000, a + 0x4000, &mut log);
        assert_eq!(mm.vmas().len(), 4);
        assert!(mm.find(a + 0x3000).is_none());
        // mprotect sobre un hueco: error.
        assert!(mm.protect(a, a + 0x5000, RW, &mut log).is_none());
    }

    #[test]
    fn la_pila_existe_sin_paginas() {
        let mut mm = Mm::new();
        mm.setup(0x40_0000);
        assert_eq!(mm.find(STACK_TOP - 8).map(|v| v.kind), Some(Kind::Stack));
        assert!(mm.find(STACK_TOP - STACK_SIZE - 1).is_none());
        assert!(mm.find(0).is_none());
    }
}
