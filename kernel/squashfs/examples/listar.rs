//! `cargo run -p jarvis-squashfs --example listar -- archivo.snap [ruta]`: lista lo que hay
//! adentro de un squashfs (y muestra un archivo si se da su ruta). Para mirar snaps reales.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("uso: listar archivo.snap [ruta adentro]");
        std::process::exit(2);
    };
    let data = std::fs::read(path).expect("no pude leer el archivo");
    let fs = match jarvis_squashfs::SquashFs::open(&data) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let entries = fs.entries().expect("no pude leer el árbol");
    if let Some(inner) = args.get(2) {
        let e = fs.find(&entries, inner).expect("no existe");
        let bytes = fs.read(e).expect("no pude leerlo");
        print!("{}", String::from_utf8_lossy(&bytes));
        return;
    }
    println!(
        "compresión {} · bloque {} · {} entradas",
        fs.compression.name(),
        fs.block_size,
        entries.len()
    );
    for e in &entries {
        println!(
            "{:o} {:>5}:{:<5} {:>9} {:?} {}",
            e.mode, e.uid, e.gid, e.size, e.kind, e.path
        );
    }
}
