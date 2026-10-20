//! Desenrollar la pila en x86-64 (SEH): deshacer una llamada para volver a la de más afuera.
//!
//! En x64 no hay "marcos" encadenados por un registro: para saber cómo deshacer una función hay
//! que leer una tabla que arma el compilador. Cada función que toca la pila tiene una
//! `RUNTIME_FUNCTION` en `.pdata` (inicio, fin y dónde están sus datos de desenrollado) que apunta
//! a un `UNWIND_INFO` en `.xdata` con la lista de lo que hizo el prólogo: cuánto bajó `rsp`, qué
//! registros guardó y dónde. Deshacer la función es rehacer eso al revés sobre una copia de los
//! registros (`Context`), hasta recuperar los de quien la llamó (incluido a dónde vuelve, en la
//! dirección de retorno que quedó en la pila).
//!
//! Esto es lo que hacen `RtlLookupFunctionEntry`, `RtlVirtualUnwind` y, encima, `RtlUnwindEx` y el
//! despachador de excepciones. Todo lee memoria del propio proceso (la imagen cargada y la pila),
//! sin hardware: se prueba en el anfitrión armando tablas y una pila a mano.
//!
//! Referencia: "x64 exception handling" de Microsoft y el formato `UNWIND_INFO`/`UNWIND_CODE`.

/// Los registros que se van deshaciendo. El orden es el de `CONTEXT` de Windows x64 para los 16
/// enteros; acá solo están los que el desenrollado toca, más `rip` y `rsp`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Regs {
    pub gpr: [u64; 16],
    pub rip: u64,
}

/// Los nombres de los 16 registros enteros, en el orden de `CONTEXT`/`UNWIND_CODE`.
pub mod reg {
    pub const RAX: usize = 0;
    pub const RCX: usize = 1;
    pub const RDX: usize = 2;
    pub const RBX: usize = 3;
    pub const RSP: usize = 4;
    pub const RBP: usize = 5;
    pub const RSI: usize = 6;
    pub const RDI: usize = 7;
}

impl Regs {
    pub fn rsp(&self) -> u64 {
        self.gpr[reg::RSP]
    }
    pub fn set_rsp(&mut self, v: u64) {
        self.gpr[reg::RSP] = v;
    }
}

/// De dónde se leen la imagen (`.pdata`/`.xdata`) y la pila. En `vino` es la memoria del proceso;
/// en los tests, un buffer.
pub trait Mem {
    fn u8(&self, addr: u64) -> Option<u8>;
    fn u16(&self, addr: u64) -> Option<u16> {
        Some(u16::from_le_bytes([self.u8(addr)?, self.u8(addr + 1)?]))
    }
    fn u32(&self, addr: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        for (i, x) in b.iter_mut().enumerate() {
            *x = self.u8(addr + i as u64)?;
        }
        Some(u32::from_le_bytes(b))
    }
    fn u64(&self, addr: u64) -> Option<u64> {
        Some((self.u32(addr)? as u64) | (self.u32(addr + 4)? as u64) << 32)
    }
}

/// Una `RUNTIME_FUNCTION` encontrada: el rango de la función (RVA) y su `UNWIND_INFO` (RVA).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    pub begin: u32,
    pub end: u32,
    pub info: u32,
}

/// Busca en la tabla `.pdata` (en `table`, `count` entradas de 12 bytes) la función que contiene
/// `rva` (binaria: la tabla está ordenada). Como `RtlLookupFunctionEntry`.
pub fn lookup(mem: &dyn Mem, table: u64, count: usize, rva: u32) -> Option<Function> {
    let (mut lo, mut hi) = (0, count);
    while lo < hi {
        let mid = (lo + hi) / 2;
        let e = table + mid as u64 * 12;
        let begin = mem.u32(e)?;
        let end = mem.u32(e + 4)?;
        if rva < begin {
            hi = mid;
        } else if rva >= end {
            lo = mid + 1;
        } else {
            return Some(Function {
                begin,
                end,
                info: mem.u32(e + 8)?,
            });
        }
    }
    None
}

const UWOP_PUSH_NONVOL: u8 = 0;
const UWOP_ALLOC_LARGE: u8 = 1;
const UWOP_ALLOC_SMALL: u8 = 2;
const UWOP_SET_FPREG: u8 = 3;
const UWOP_SAVE_NONVOL: u8 = 4;
const UWOP_SAVE_NONVOL_FAR: u8 = 5;
const UWOP_SAVE_XMM128: u8 = 8;
const UWOP_SAVE_XMM128_FAR: u8 = 9;
const UWOP_PUSH_MACHFRAME: u8 = 10;
const UNW_FLAG_CHAININFO: u8 = 0x4;

/// El manejador de una función (si tiene) y cómo llamarlo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Handler {
    /// RVA de la rutina de manejo (`__C_specific_handler`, `__CxxFrameHandler*`…).
    pub routine: u32,
    /// RVA de los datos que le pasa (`scope table` o la info de C++).
    pub data: u32,
    /// Bits de `UNWIND_INFO`: bit 0 EHANDLER (se consulta al buscar), bit 1 UHANDLER (al deshacer).
    pub flags: u8,
}

/// Deshace una función sobre `regs` (los pone como los de quien la llamó). `image_base` es la base
/// del módulo; `func` la encontró [`lookup`]. Devuelve el manejador de esta función, si tiene.
/// Como `RtlVirtualUnwind` (modo "unwind", sin llamar al manejador).
pub fn unwind_function(
    mem: &dyn Mem,
    image_base: u64,
    func: Function,
    regs: &mut Regs,
) -> Option<Handler> {
    let rva = (regs.rip - image_base) as u32;
    // Cuántos bytes del prólogo ya se ejecutaron: las operaciones que vienen después no hay que
    // deshacerlas (la función todavía no las hizo).
    let offset = rva.wrapping_sub(func.begin);
    unwind_info(mem, image_base, func.info, offset, regs, 0)
}

fn unwind_info(
    mem: &dyn Mem,
    image_base: u64,
    info_rva: u32,
    offset_in_func: u32,
    regs: &mut Regs,
    depth: u32,
) -> Option<Handler> {
    if depth > 32 {
        return None;
    }
    let info = image_base + info_rva as u64;
    let first = mem.u8(info)?;
    let version = first & 7;
    if version != 1 && version != 2 {
        return None;
    }
    let flags = first >> 3;
    let prolog = mem.u8(info + 1)?;
    let count = mem.u8(info + 2)? as u64;
    let frame = mem.u8(info + 3)?;
    let frame_reg = (frame & 0xf) as usize;
    let frame_off = (frame >> 4) as u64 * 16;

    // Si estamos todavía adentro del prólogo (en `offset_in_func` bytes), solo se deshacen las
    // operaciones cuyo `offset` ya pasó.
    let in_prolog = offset_in_func < prolog as u32;

    let codes = info + 4;
    let mut i = 0u64;
    while i < count {
        let code = codes + i * 2;
        let code_off = mem.u8(code)?;
        let op = mem.u8(code + 1)?;
        let unwind_op = op & 0xf;
        let op_info = op >> 4;
        // Dentro del prólogo: saltear lo que todavía no se ejecutó.
        let skip = in_prolog && (offset_in_func as u8) < code_off;
        let advance = match unwind_op {
            UWOP_PUSH_NONVOL => {
                if !skip {
                    let sp = regs.rsp();
                    regs.gpr[op_info as usize] = mem.u64(sp)?;
                    regs.set_rsp(sp + 8);
                }
                1
            }
            UWOP_ALLOC_LARGE => {
                let (size, adv) = if op_info == 0 {
                    (mem.u16(code + 2)? as u64 * 8, 2)
                } else {
                    (mem.u32(code + 2)? as u64, 3)
                };
                if !skip {
                    regs.set_rsp(regs.rsp() + size);
                }
                adv
            }
            UWOP_ALLOC_SMALL => {
                if !skip {
                    regs.set_rsp(regs.rsp() + (op_info as u64 * 8 + 8));
                }
                1
            }
            UWOP_SET_FPREG => {
                if !skip {
                    regs.set_rsp(regs.gpr[frame_reg] - frame_off);
                }
                1
            }
            UWOP_SAVE_NONVOL => {
                let off = mem.u16(code + 2)? as u64 * 8;
                if !skip {
                    regs.gpr[op_info as usize] =
                        mem.u64(frame_base(regs, frame_reg, frame_off) + off)?;
                }
                2
            }
            UWOP_SAVE_NONVOL_FAR => {
                let off = mem.u32(code + 2)? as u64;
                if !skip {
                    regs.gpr[op_info as usize] =
                        mem.u64(frame_base(regs, frame_reg, frame_off) + off)?;
                }
                3
            }
            UWOP_SAVE_XMM128 => 2, // los XMM no se restauran acá (no se usan en el unwind)
            UWOP_SAVE_XMM128_FAR => 3,
            UWOP_PUSH_MACHFRAME => {
                // Una interrupción/excepción de hardware dejó un "machine frame": rip y rsp están
                // en la pila. op_info = 1 si hay un código de error arriba.
                if !skip {
                    let sp = regs.rsp() + if op_info != 0 { 8 } else { 0 };
                    regs.rip = mem.u64(sp)?;
                    regs.set_rsp(mem.u64(sp + 24)?);
                }
                1
            }
            _ => return None,
        };
        i += advance;
    }

    // ¿Esta `UNWIND_INFO` encadena a otra (una función partida en trozos)?
    if flags & UNW_FLAG_CHAININFO != 0 {
        let chain = codes + align2(count) * 2;
        let begin = mem.u32(chain)?;
        let next_info = mem.u32(chain + 8)?;
        // La cadena ya no está "en el prólogo": se deshace entera.
        let _ = begin;
        return unwind_info(mem, image_base, next_info, u32::MAX, regs, depth + 1);
    }

    // El manejador va después de los códigos (alineado a 4 bytes), si el flag lo dice.
    if flags & 0x3 != 0 {
        let after = codes + align2(count) * 2;
        return Some(Handler {
            routine: mem.u32(after)?,
            data: after as u32 + 4 - image_base as u32,
            flags,
        });
    }
    Some(Handler::default())
}

fn frame_base(regs: &Regs, frame_reg: usize, frame_off: u64) -> u64 {
    if frame_reg == 0 {
        regs.rsp()
    } else {
        regs.gpr[frame_reg] - frame_off
    }
}

/// La cantidad de "slots" de 2 bytes, redondeada a par (los códigos se alinean a 4 bytes).
fn align2(count: u64) -> u64 {
    count.div_ceil(2) * 2
}

/// Deshace **un** nivel: encuentra la función de `regs.rip`, la deshace y saca la dirección de
/// retorno. Devuelve su manejador (si tiene). `None` cuando no hay tabla (función hoja: deshacer
/// es solo `pop rip`) — el que llama decide (en una hoja, `rip = [rsp]`, `rsp += 8`).
pub fn step(
    mem: &dyn Mem,
    image_base: u64,
    table: u64,
    count: usize,
    regs: &mut Regs,
) -> Option<Handler> {
    let rva = (regs.rip - image_base) as u32;
    match lookup(mem, table, count, rva) {
        Some(func) => {
            let h = unwind_function(mem, image_base, func, regs)?;
            // Después de deshacer el cuerpo, la dirección de retorno está en el tope.
            let sp = regs.rsp();
            regs.rip = mem.u64(sp)?;
            regs.set_rsp(sp + 8);
            Some(h)
        }
        None => {
            // Función hoja: no tocó la pila salvo el `call`.
            let sp = regs.rsp();
            regs.rip = mem.u64(sp)?;
            regs.set_rsp(sp + 8);
            Some(Handler::default())
        }
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Memoria de mentira: un mapa de dirección → byte.
    #[derive(Default)]
    struct FakeMem(HashMap<u64, u8>);
    impl FakeMem {
        fn put(&mut self, addr: u64, bytes: &[u8]) {
            for (i, b) in bytes.iter().enumerate() {
                self.0.insert(addr + i as u64, *b);
            }
        }
        fn put32(&mut self, addr: u64, v: u32) {
            self.put(addr, &v.to_le_bytes());
        }
        fn put64(&mut self, addr: u64, v: u64) {
            self.put(addr, &v.to_le_bytes());
        }
    }
    impl Mem for FakeMem {
        fn u8(&self, addr: u64) -> Option<u8> {
            self.0.get(&addr).copied()
        }
    }

    #[test]
    fn busca_la_funcion_en_pdata() {
        let mut m = FakeMem::default();
        let table = 0x1000;
        // Tres funciones: [0x1000,0x1050), [0x1050,0x1100), [0x1100,0x1200).
        for (i, (b, e, info)) in [
            (0x1000, 0x1050, 0x5000),
            (0x1050, 0x1100, 0x5010),
            (0x1100, 0x1200, 0x5020),
        ]
        .iter()
        .enumerate()
        {
            let at = table + i as u64 * 12;
            m.put32(at, *b);
            m.put32(at + 4, *e);
            m.put32(at + 8, *info);
        }
        assert_eq!(lookup(&m, table, 3, 0x1080).unwrap().info, 0x5010);
        assert_eq!(lookup(&m, table, 3, 0x1000).unwrap().info, 0x5000);
        assert_eq!(lookup(&m, table, 3, 0x11ff).unwrap().info, 0x5020);
        assert!(lookup(&m, table, 3, 0x1200).is_none());
        assert!(lookup(&m, table, 3, 0x900).is_none());
    }

    /// Un prólogo típico: `push rbp; push rsi; sub rsp, 0x40`. `UNWIND_INFO` al revés: primero el
    /// código del final del prólogo.
    #[test]
    fn deshace_un_prologo() {
        let base = 0x1_4000_0000;
        let mut m = FakeMem::default();
        let info: u32 = 0x8000;
        // version 1, sin flags; prólogo de 9 bytes; 3 códigos; sin frame reg.
        m.put(base + info as u64, &[1, 9, 3, 0]);
        // Códigos (de último a primero): alloc small 0x40 (off 9), push rsi (off 5), push rbp (4).
        // ALLOC_SMALL con op_info 7 = (7+1)*8 = 64.
        m.put(base + info as u64 + 4, &[9, (7 << 4) | UWOP_ALLOC_SMALL]);
        m.put(
            base + info as u64 + 6,
            &[5, (reg::RSI as u8) << 4 | UWOP_PUSH_NONVOL],
        );
        m.put(
            base + info as u64 + 8,
            &[4, (reg::RBP as u8) << 4 | UWOP_PUSH_NONVOL],
        );
        // La pila: en rsp están los 0x40 del sub, después rsi guardado, después rbp, después la
        // dirección de retorno.
        let sp = 0x9000;
        m.put64(sp + 0x40, 0xBBBB); // rsi del que llamó
        m.put64(sp + 0x48, 0xCCCC); // rbp del que llamó
        m.put64(sp + 0x50, base + 0x2222); // dirección de retorno
        let func = Function {
            begin: 0x1000,
            end: 0x1100,
            info,
        };
        let mut regs = Regs::default();
        regs.rip = base + 0x1040; // pasado el prólogo
        regs.set_rsp(sp);
        let h = unwind_function(&m, base, func, &mut regs).unwrap();
        assert_eq!(h, Handler::default());
        assert_eq!(regs.gpr[reg::RSI], 0xBBBB);
        assert_eq!(regs.gpr[reg::RBP], 0xCCCC);
        assert_eq!(
            regs.rsp(),
            sp + 0x50,
            "rsp quedó sobre la dirección de retorno"
        );
        // `step` completo: saca también rip.
        let mut regs2 = Regs::default();
        regs2.rip = base + 0x1040;
        regs2.set_rsp(sp);
        let table = 0x1000;
        m.put32(table, func.begin);
        m.put32(table + 4, func.end);
        m.put32(table + 8, func.info);
        step(&m, base, table, 1, &mut regs2).unwrap();
        assert_eq!(regs2.rip, base + 0x2222);
        assert_eq!(regs2.rsp(), sp + 0x58);
    }

    #[test]
    fn el_manejador_esta_despues_de_los_codigos() {
        let base = 0x1_4000_0000;
        let mut m = FakeMem::default();
        let info: u32 = 0x8000;
        // flag EHANDLER (1), 1 código (alloc small), después el manejador.
        m.put(base + info as u64, &[(1 << 3) | 1, 4, 1, 0]);
        m.put(base + info as u64 + 4, &[4, UWOP_ALLOC_SMALL]); // sub rsp, 8
        // 1 código → align2(1) = 2 slots → el manejador va en info+4+4.
        m.put32(base + info as u64 + 8, 0x7777); // routine RVA
        m.put32(base + info as u64 + 12, 0x8888); // data
        let func = Function {
            begin: 0x1000,
            end: 0x1100,
            info,
        };
        let mut regs = Regs::default();
        regs.rip = base + 0x1050;
        regs.set_rsp(0x9000);
        m.put64(0x9008, base + 0x3333);
        let h = unwind_function(&m, base, func, &mut regs).unwrap();
        assert_eq!(h.routine, 0x7777);
        assert_eq!(h.flags & 1, 1);
    }

    #[test]
    fn en_el_prologo_no_se_deshace_lo_que_falta() {
        // Si rip está justo después del primer `push`, solo ese push se deshace.
        let base = 0x1_4000_0000;
        let mut m = FakeMem::default();
        let info: u32 = 0x8000;
        m.put(base + info as u64, &[1, 9, 2, 0]);
        m.put(
            base + info as u64 + 4,
            &[5, (reg::RSI as u8) << 4 | UWOP_PUSH_NONVOL],
        ); // off 5
        m.put(
            base + info as u64 + 6,
            &[1, (reg::RBP as u8) << 4 | UWOP_PUSH_NONVOL],
        ); // off 1
        let sp = 0x9000;
        m.put64(sp, 0xCCCC); // rbp (el único push hecho)
        let func = Function {
            begin: 0x1000,
            end: 0x1100,
            info,
        };
        let mut regs = Regs::default();
        regs.rip = base + 0x1001; // solo se ejecutó `push rbp` (off 1)
        regs.set_rsp(sp);
        unwind_function(&m, base, func, &mut regs).unwrap();
        assert_eq!(regs.gpr[reg::RBP], 0xCCCC);
        assert_eq!(regs.gpr[reg::RSI], 0, "el segundo push todavía no pasó");
        assert_eq!(regs.rsp(), sp + 8);
    }
}
