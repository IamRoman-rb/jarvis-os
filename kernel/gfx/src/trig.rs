//! Trigonometría en punto fijo, con tabla.
//!
//! El kernel no tiene punto flotante por hardware (el target `x86_64-unknown-none` compila los
//! `f32` como llamadas a rutinas de software). Para animar 22.000 partículas por frame se usa
//! punto fijo **Q14**: un entero donde `ONE = 16384` representa 1,0. Multiplicar dos Q14 da un
//! Q28, y con `>> 14` se vuelve a Q14. Todo son operaciones enteras nativas de la CPU.
//!
//! Los ángulos se miden en "unidades de vuelta": `FULL_TURN` (65536) es una vuelta completa, así
//! que sumar ángulos con `wrapping_add` da la vuelta sola, sin `%`.

/// 1,0 en Q14.
pub const ONE: i32 = 1 << 14;
/// Una vuelta completa (360°).
pub const FULL_TURN: u32 = 1 << 16;

const TABLE_BITS: u32 = 10;
const TABLE_SIZE: usize = 1 << TABLE_BITS;

/// Seno por serie de Taylor, evaluado en tiempo de compilación (x en [-π, π]).
const fn sin_taylor(x: f64) -> f64 {
    let x2 = x * x;
    let mut term = x;
    let mut sum = x;
    let mut n = 1;
    while n < 14 {
        term = -term * x2 / ((2 * n) as f64 * (2 * n + 1) as f64);
        sum += term;
        n += 1;
    }
    sum
}

const fn build_table() -> [i16; TABLE_SIZE] {
    const PI: f64 = core::f64::consts::PI;
    let mut table = [0i16; TABLE_SIZE];
    let mut i = 0;
    while i < TABLE_SIZE {
        let mut a = 2.0 * PI * i as f64 / TABLE_SIZE as f64;
        if a > PI {
            a -= 2.0 * PI;
        }
        let v = sin_taylor(a) * ONE as f64;
        table[i] = if v >= 0.0 {
            (v + 0.5) as i16
        } else {
            (v - 0.5) as i16
        };
        i += 1;
    }
    table
}

/// La tabla se calcula al compilar: en el binario ya está lista, no cuesta nada al arrancar.
static SIN_TABLE: [i16; TABLE_SIZE] = build_table();

/// Seno en Q14 (−16384..=16384) de un ángulo en unidades de vuelta.
#[inline]
pub fn sin(angle: u32) -> i32 {
    SIN_TABLE[((angle >> (16 - TABLE_BITS)) as usize) & (TABLE_SIZE - 1)] as i32
}

/// Coseno en Q14: el seno corrido un cuarto de vuelta.
#[inline]
pub fn cos(angle: u32) -> i32 {
    sin(angle.wrapping_add(FULL_TURN / 4))
}

/// Multiplica dos Q14.
#[inline]
pub const fn mul(a: i32, b: i32) -> i32 {
    (a * b) >> 14
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coincide_con_libm() {
        for step in 0..4096u32 {
            let angle = step * (FULL_TURN / 4096);
            let rad = angle as f32 / FULL_TURN as f32 * core::f32::consts::TAU;
            let esperado_sin = libm::sinf(rad);
            let esperado_cos = libm::cosf(rad);
            // La tabla tiene 1024 pasos: el error por no interpolar es < 2π/1024 ≈ 0,006.
            assert!(
                (sin(angle) as f32 / ONE as f32 - esperado_sin).abs() < 0.01,
                "sin {angle}"
            );
            assert!(
                (cos(angle) as f32 / ONE as f32 - esperado_cos).abs() < 0.01,
                "cos {angle}"
            );
        }
    }

    #[test]
    fn valores_exactos_y_vuelta_completa() {
        assert_eq!(sin(0), 0);
        assert_eq!(sin(FULL_TURN / 4), ONE);
        assert_eq!(sin(3 * FULL_TURN / 4), -ONE);
        assert_eq!(cos(0), ONE);
        assert_eq!(sin(FULL_TURN + 1000), sin(1000)); // pasar de una vuelta no rompe nada
        assert_eq!(mul(ONE, ONE), ONE);
        assert_eq!(mul(ONE / 2, ONE / 2), ONE / 4);
    }
}
