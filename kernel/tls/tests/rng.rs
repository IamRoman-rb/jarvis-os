use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use jarvis_tls::rng::{Pool, Rng, SEED_BITS, jitter_credit};

fn seeded(tag: &[u8]) -> Rng {
    let mut p = Pool::new();
    p.add(tag, SEED_BITS);
    Rng::from_seed(p.seed())
}

#[test]
fn el_pool_cuenta_los_bits_acreditados() {
    let mut p = Pool::new();
    assert!(!p.ready());
    p.add(b"mac 52:54:00:12:34:56", 0);
    p.add(&[1; 8], 200);
    assert_eq!(p.credited(), 200);
    assert!(!p.ready());
    p.add(&[2; 8], 56);
    assert!(p.ready());
    p.add(&[3; 8], u32::MAX);
    assert_eq!(p.credited(), u32::MAX);
}

#[test]
fn el_largo_separa_las_entradas() {
    let mut a = Pool::new();
    a.add(b"ab", 0);
    a.add(b"c", 0);
    let mut b = Pool::new();
    b.add(b"a", 0);
    b.add(b"bc", 0);
    assert_ne!(a.seed(), b.seed());
}

#[test]
fn misma_semilla_misma_salida_y_distinta_semilla_otra() {
    let (mut a, mut b, mut c) = (seeded(b"uno"), seeded(b"uno"), seeded(b"dos"));
    let (mut x, mut y, mut z) = ([0u8; 64], [0u8; 64], [0u8; 64]);
    a.fill(&mut x);
    b.fill(&mut y);
    c.fill(&mut z);
    assert_eq!(x, y);
    assert_ne!(x, z);
}

#[test]
fn es_chacha20_con_borrado_de_clave() {
    // Referencia a mano: la salida es el flujo de ChaCha20 salteando los primeros 32 bytes, y la
    // clave siguiente son esos 32 bytes.
    let key = [7u8; 32];
    let mut rng = Rng::from_seed(key);
    let mut out = [0u8; 40];
    rng.fill(&mut out);

    let mut flujo = [0u8; 72];
    ChaCha20::new(&key.into(), &[0u8; 12].into()).apply_keystream(&mut flujo);
    assert_eq!(out[..], flujo[32..]);

    let mut siguiente = [0u8; 8];
    rng.fill(&mut siguiente);
    let nueva: [u8; 32] = flujo[..32].try_into().unwrap();
    let mut flujo2 = [0u8; 40];
    ChaCha20::new(&nueva.into(), &[0u8; 12].into()).apply_keystream(&mut flujo2);
    assert_eq!(siguiente[..], flujo2[32..]);
}

#[test]
fn pedidos_seguidos_no_se_repiten() {
    let mut r = seeded(b"x");
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    r.fill(&mut a);
    r.fill(&mut b);
    assert_ne!(a, b);
    assert_ne!(r.next_u64(), r.next_u64());
}

#[test]
fn pedidos_largos_y_vacios() {
    let mut r = seeded(b"largo");
    r.fill(&mut []);
    let mut big = vec![0u8; 10_000];
    r.fill(&mut big);
    // Ningún trozo de 224 bytes (el tamaño de cada vuelta) queda en cero.
    assert!(big.chunks(224).all(|c| c.iter().any(|&b| b != 0)));
}

#[test]
fn reseed_cambia_la_salida() {
    let (mut a, mut b) = (seeded(b"s"), seeded(b"s"));
    b.reseed(&[1, 2, 3]);
    assert_ne!(a.next_u64(), b.next_u64());
}

#[test]
fn los_bytes_salen_parejos() {
    // Chequeo grueso de distribución: en 1 MiB, cada valor de byte aparece ~4096 veces.
    let mut r = seeded(b"estadistica");
    let mut buf = vec![0u8; 1 << 20];
    r.fill(&mut buf);
    let mut counts = [0u32; 256];
    for &b in &buf {
        counts[b as usize] += 1;
    }
    // Chi-cuadrado con 255 grados de libertad: el 99,99 % cae por debajo de ~370.
    let expected = (buf.len() / 256) as f64;
    let chi: f64 = counts
        .iter()
        .map(|&c| (c as f64 - expected).powi(2) / expected)
        .sum();
    assert!(chi < 370.0, "chi-cuadrado {chi}");
}

#[test]
fn el_jitter_constante_o_lineal_no_acredita() {
    assert_eq!(jitter_credit(&[]), 0);
    assert_eq!(jitter_credit(&[100; 64]), 0);
    // Crece de a 3 ciclos: predecible.
    let lineal: Vec<u64> = (0..64).map(|i| 100 + 3 * i).collect();
    assert_eq!(jitter_credit(&lineal), 0);
}

#[test]
fn el_jitter_que_varia_acredita_un_bit_por_muestra_como_mucho() {
    let ruido = [100, 131, 97, 180, 102, 99, 250, 101];
    assert_eq!(jitter_credit(&ruido), 6);
    // Repetir una muestra no suma.
    assert_eq!(jitter_credit(&[100, 131, 131, 131]), 0);
}
