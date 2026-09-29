//! Fecha y hora: zona horaria fija y formato en castellano (o inglés o portugués, según
//! [`set_language`]), sin `std` ni reservas de memoria.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU8, Ordering};

/// Idioma de la fecha: 0 = castellano, 1 = inglés, 2 = portugués.
static LANGUAGE: AtomicU8 = AtomicU8::new(0);

pub fn set_language(lang: u8) {
    LANGUAGE.store(lang.min(2), Ordering::Relaxed);
}

/// Fecha y hora de pared (sin zona horaria).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

const WEEKDAYS: [&str; 7] = [
    "DOMINGO",
    "LUNES",
    "MARTES",
    "MIÉRCOLES",
    "JUEVES",
    "VIERNES",
    "SÁBADO",
];
const WEEKDAYS_EN: [&str; 7] = [
    "SUNDAY",
    "MONDAY",
    "TUESDAY",
    "WEDNESDAY",
    "THURSDAY",
    "FRIDAY",
    "SATURDAY",
];
const WEEKDAYS_PT: [&str; 7] = [
    "DOMINGO",
    "SEGUNDA-FEIRA",
    "TERÇA-FEIRA",
    "QUARTA-FEIRA",
    "QUINTA-FEIRA",
    "SEXTA-FEIRA",
    "SÁBADO",
];
const MONTHS_EN: [&str; 12] = [
    "JANUARY",
    "FEBRUARY",
    "MARCH",
    "APRIL",
    "MAY",
    "JUNE",
    "JULY",
    "AUGUST",
    "SEPTEMBER",
    "OCTOBER",
    "NOVEMBER",
    "DECEMBER",
];
const MONTHS_PT: [&str; 12] = [
    "JANEIRO",
    "FEVEREIRO",
    "MARÇO",
    "ABRIL",
    "MAIO",
    "JUNHO",
    "JULHO",
    "AGOSTO",
    "SETEMBRO",
    "OUTUBRO",
    "NOVEMBRO",
    "DEZEMBRO",
];
const MONTHS: [&str; 12] = [
    "ENERO",
    "FEBRERO",
    "MARZO",
    "ABRIL",
    "MAYO",
    "JUNIO",
    "JULIO",
    "AGOSTO",
    "SEPTIEMBRE",
    "OCTUBRE",
    "NOVIEMBRE",
    "DICIEMBRE",
];

pub const fn is_leap(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

pub const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl DateTime {
    /// Valida rangos (el reloj CMOS puede devolver basura si la pila está agotada).
    pub fn is_valid(&self) -> bool {
        (1..=12).contains(&self.month)
            && self.day >= 1
            && self.day <= days_in_month(self.year, self.month)
            && self.hour < 24
            && self.minute < 60
            && self.second < 60
    }

    /// Día de la semana, 0 = domingo (algoritmo de Sakamoto).
    pub fn weekday(&self) -> usize {
        const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let mut y = self.year as i32;
        if self.month < 3 {
            y -= 1;
        }
        ((y + y / 4 - y / 100 + y / 400 + T[self.month as usize - 1] + self.day as i32) % 7)
            as usize
    }

    /// Segundos desde el 1/1/1970 UTC (tiempo Unix); `None` antes de 1970. TLS lo usa para ver si
    /// un certificado está vigente. Cuenta los días con el algoritmo `days_from_civil` de Howard
    /// Hinnant: corre el año para que empiece en marzo, así el 29 de febrero queda al final.
    pub fn unix_seconds(&self) -> Option<u64> {
        if self.year < 1970 {
            return None;
        }
        let y = self.year as i64 - if self.month <= 2 { 1 } else { 0 };
        let era = y / 400;
        let yoe = y - era * 400;
        let m = self.month as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        let secs =
            days * 86_400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64;
        u64::try_from(secs).ok()
    }

    /// Corre la hora `hours` horas (puede cambiar el día, el mes y el año).
    /// Se usa para pasar de UTC (lo que guarda el reloj del hardware) a hora local.
    pub fn offset_hours(mut self, hours: i8) -> DateTime {
        let mut h = self.hour as i32 + hours as i32;
        while h < 0 {
            h += 24;
            self = self.prev_day();
        }
        while h >= 24 {
            h -= 24;
            self = self.next_day();
        }
        self.hour = h as u8;
        self
    }

    fn next_day(mut self) -> DateTime {
        if self.day < days_in_month(self.year, self.month) {
            self.day += 1;
        } else if self.month < 12 {
            self.day = 1;
            self.month += 1;
        } else {
            self.day = 1;
            self.month = 1;
            self.year += 1;
        }
        self
    }

    fn prev_day(mut self) -> DateTime {
        if self.day > 1 {
            self.day -= 1;
        } else if self.month > 1 {
            self.month -= 1;
            self.day = days_in_month(self.year, self.month);
        } else {
            self.year -= 1;
            self.month = 12;
            self.day = 31;
        }
        self
    }

    /// "MIÉRCOLES, 23 DE SEPTIEMBRE DE 2026" (o "WEDNESDAY, SEPTEMBER 23, 2026").
    pub fn write_date(&self, out: &mut impl Write) -> fmt::Result {
        let (wd, m) = (self.weekday(), self.month as usize - 1);
        match LANGUAGE.load(Ordering::Relaxed) {
            1 => write!(
                out,
                "{}, {} {}, {}",
                WEEKDAYS_EN[wd], MONTHS_EN[m], self.day, self.year
            ),
            2 => write!(
                out,
                "{}, {} DE {} DE {}",
                WEEKDAYS_PT[wd], self.day, MONTHS_PT[m], self.year
            ),
            _ => write!(
                out,
                "{}, {} DE {} DE {}",
                WEEKDAYS[wd], self.day, MONTHS[m], self.year
            ),
        }
    }

    /// "15:25"
    pub fn write_time(&self, out: &mut impl Write) -> fmt::Result {
        write!(out, "{:02}:{:02}", self.hour, self.minute)
    }
}

/// Buffer de texto de tamaño fijo en el stack: sirve para formatear sin un heap.
pub struct StrBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StrBuf<N> {
    pub const fn new() -> Self {
        StrBuf {
            buf: [0; N],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        // `write_str` solo copia `&str` completos, así que siempre es UTF-8 válido.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Default for StrBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Write for StrBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();
        if end > N {
            return Err(fmt::Error);
        }
        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(year: u16, month: u8, day: u8, hour: u8) -> DateTime {
        DateTime {
            year,
            month,
            day,
            hour,
            minute: 25,
            second: 0,
        }
    }

    fn date(d: DateTime) -> std::string::String {
        let mut s = StrBuf::<64>::new();
        d.write_date(&mut s).unwrap();
        s.as_str().into()
    }

    #[test]
    fn formato_en_castellano() {
        assert_eq!(
            date(dt(2026, 9, 23, 15)),
            "MIÉRCOLES, 23 DE SEPTIEMBRE DE 2026"
        );
        assert_eq!(date(dt(2026, 6, 20, 15)), "SÁBADO, 20 DE JUNIO DE 2026");
        let mut s = StrBuf::<8>::new();
        dt(2026, 9, 23, 9).write_time(&mut s).unwrap();
        assert_eq!(s.as_str(), "09:25");
    }

    #[test]
    fn utc_a_argentina_cruza_dias_meses_y_anios() {
        assert_eq!(dt(2026, 9, 23, 18).offset_hours(-3), dt(2026, 9, 23, 15));
        assert_eq!(dt(2026, 9, 23, 1).offset_hours(-3), dt(2026, 9, 22, 22));
        assert_eq!(dt(2026, 3, 1, 0).offset_hours(-3), dt(2026, 2, 28, 21));
        assert_eq!(dt(2028, 3, 1, 0).offset_hours(-3), dt(2028, 2, 29, 21)); // bisiesto
        assert_eq!(dt(2027, 1, 1, 2).offset_hours(-3), dt(2026, 12, 31, 23));
        assert_eq!(dt(2026, 12, 31, 23).offset_hours(3), dt(2027, 1, 1, 2));
    }

    #[test]
    fn bisiestos_y_validacion() {
        assert!(is_leap(2028) && is_leap(2000) && !is_leap(2100) && !is_leap(2026));
        assert!(dt(2028, 2, 29, 0).is_valid());
        assert!(!dt(2026, 2, 29, 0).is_valid());
        assert!(
            !DateTime {
                month: 13,
                ..dt(2026, 1, 1, 0)
            }
            .is_valid()
        );
    }

    #[test]
    fn tiempo_unix() {
        // `dt` pone el minuto en 25; acá hace falta en punto.
        let dt = |y, m, d, h| DateTime {
            minute: 0,
            ..dt(y, m, d, h)
        };
        assert_eq!(dt(1970, 1, 1, 0).unix_seconds(), Some(0));
        assert_eq!(dt(1969, 12, 31, 23).unix_seconds(), None);
        // Valores conocidos (date -u -d ... +%s).
        assert_eq!(dt(2000, 3, 1, 0).unix_seconds(), Some(951_868_800));
        assert_eq!(dt(2027, 1, 1, 0).unix_seconds(), Some(1_798_761_600));
        let t = DateTime {
            minute: 34,
            second: 56,
            ..dt(2028, 2, 29, 12)
        };
        assert_eq!(t.unix_seconds(), Some(1_835_440_496));
        // Cada día suma 86400, también al cruzar un 29 de febrero.
        for (a, b) in [
            (dt(2028, 2, 28, 0), dt(2028, 2, 29, 0)),
            (dt(2028, 2, 29, 0), dt(2028, 3, 1, 0)),
            (dt(2026, 12, 31, 0), dt(2027, 1, 1, 0)),
        ] {
            assert_eq!(
                b.unix_seconds().unwrap() - a.unix_seconds().unwrap(),
                86_400
            );
        }
    }

    #[test]
    fn strbuf_no_desborda() {
        let mut s = StrBuf::<4>::new();
        assert!(s.write_str("JARVIS").is_err());
        assert_eq!(s.as_str(), "");
    }
}
