//! `jarvis-relay [--publico] [puerto]` (8120): el relé de la sincronización (ver lib.rs).

fn main() {
    let mut public = false;
    let mut port = 8120u16;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--publico" => public = true,
            p => match p.parse() {
                Ok(n) => port = n,
                Err(_) => {
                    eprintln!("uso: jarvis-relay [--publico] [puerto]");
                    std::process::exit(2);
                }
            },
        }
    }
    match jarvis_relay::bind(public, port) {
        Ok(l) => {
            println!(
                "relé: escuchando en {}",
                l.local_addr().map(|a| a.to_string()).unwrap_or_default()
            );
            jarvis_relay::serve(l);
        }
        Err(e) => {
            eprintln!("relé: no pude escuchar en el puerto {port}: {e}");
            std::process::exit(1);
        }
    }
}
