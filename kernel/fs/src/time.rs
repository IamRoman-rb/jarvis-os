//! Fechas en formato FAT: dos enteros de 16 bits empaquetados.
//!
//! ```text
//! fecha: aaaaaaa mmmm ddddd   (año desde 1980, mes, día)
//! hora:  hhhhh mmmmmm sssss   (hora, minutos, segundos / 2)
//! ```

/// Fecha y hora local.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Default for Timestamp {
    fn default() -> Self {
        Timestamp::EPOCH
    }
}

impl Timestamp {
    /// La fecha más antigua que FAT puede guardar.
    pub const EPOCH: Timestamp = Timestamp {
        year: 1980,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    };

    /// `(fecha, hora)` en formato FAT. Los segundos se guardan de a 2.
    pub fn to_fat(&self) -> (u16, u16) {
        let year = self.year.clamp(1980, 2107) - 1980;
        let date =
            (year << 9) | ((self.month.clamp(1, 12) as u16) << 5) | self.day.clamp(1, 31) as u16;
        let time = ((self.hour.min(23) as u16) << 11)
            | ((self.minute.min(59) as u16) << 5)
            | (self.second.min(59) / 2) as u16;
        (date, time)
    }

    pub fn from_fat(date: u16, time: u16) -> Timestamp {
        if date == 0 {
            return Timestamp::EPOCH;
        }
        Timestamp {
            year: 1980 + (date >> 9),
            month: ((date >> 5) & 0x0F) as u8,
            day: (date & 0x1F) as u8,
            hour: (time >> 11) as u8,
            minute: ((time >> 5) & 0x3F) as u8,
            second: ((time & 0x1F) * 2) as u8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_y_vuelta() {
        let t = Timestamp {
            year: 2026,
            month: 9,
            day: 23,
            hour: 22,
            minute: 41,
            second: 36,
        };
        let (d, h) = t.to_fat();
        assert_eq!(Timestamp::from_fat(d, h), t);
        // Los segundos impares se redondean hacia abajo (FAT guarda de a 2).
        let impar = Timestamp { second: 37, ..t };
        let (d, h) = impar.to_fat();
        assert_eq!(Timestamp::from_fat(d, h).second, 36);
        assert_eq!(Timestamp::from_fat(0, 0), Timestamp::EPOCH);
    }
}
