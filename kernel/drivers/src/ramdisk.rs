//! El *ramdisk* del arranque (K14): archivos que el bootloader carga en memoria junto con el
//! kernel, antes de que haya disco.
//!
//! Sirve para lo que tiene que estar aunque no haya disco de JARVIS (modo en vivo) o antes de
//! montarlo: el firmware de la placa Wi-Fi. Va en la partición de arranque, así que la ISO, el
//! pendrive y el instalador lo llevan solos. Lo arma `cargo xtask` con [`build`].
//!
//! Formato: `JRD1` y, por cada archivo, el largo del nombre (u16), el nombre (UTF-8), el largo de
//! los datos (u32) y los datos. Todo en little endian.

use alloc::vec::Vec;

const MAGIC: &[u8; 4] = b"JRD1";

/// Arma un ramdisk con estos archivos (nombre, datos).
pub fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    for (name, data) in files {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

/// Los archivos del ramdisk. `None` si no tiene el formato (o está cortado).
pub fn files(rd: &[u8]) -> Option<Vec<(&str, &[u8])>> {
    if rd.get(..4)? != MAGIC {
        return None;
    }
    let mut out = Vec::new();
    let mut at = 4;
    while at < rd.len() {
        let n = u16::from_le_bytes(rd.get(at..at + 2)?.try_into().ok()?) as usize;
        let name = core::str::from_utf8(rd.get(at + 2..at + 2 + n)?).ok()?;
        at += 2 + n;
        let len = u32::from_le_bytes(rd.get(at..at + 4)?.try_into().ok()?) as usize;
        let data = rd.get(at + 4..at + 4 + len)?;
        at += 4 + len;
        out.push((name, data));
    }
    Some(out)
}

/// Un archivo del ramdisk por su nombre.
pub fn find<'a>(rd: &'a [u8], name: &str) -> Option<&'a [u8]> {
    files(rd)?
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        let rd = build(&[("rtw88/rtw8821c_fw.bin", &[1, 2, 3]), ("vacio", &[])]);
        assert_eq!(find(&rd, "rtw88/rtw8821c_fw.bin"), Some(&[1u8, 2, 3][..]));
        assert_eq!(find(&rd, "vacio"), Some(&[][..]));
        assert_eq!(find(&rd, "otro"), None);
        assert_eq!(files(&rd).unwrap().len(), 2);
        assert!(files(&rd[..rd.len() - 1]).is_none(), "cortado");
        assert!(files(b"JRD2").is_none(), "otro formato");
        assert_eq!(files(b"JRD1").unwrap().len(), 0);
    }
}
