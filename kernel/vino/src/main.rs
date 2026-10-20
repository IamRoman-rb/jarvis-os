//! `vino programa.exe [argumentos…]`: corre un programa de Windows en JARVIS-OS (ADR 0018).
//!
//! `vino --imports programa.exe` dice qué DLL y funciones pide el programa y cuáles le faltan,
//! sin correrlo.

use std::process::ExitCode;

use vino::{dll, pe};

fn usage() -> ExitCode {
    eprintln!("uso: vino programa.exe [argumentos...]");
    eprintln!("     vino --imports programa.exe   (qué pide de Windows y qué le falta)");
    ExitCode::from(2)
}

fn imports(path: &str, file: &[u8]) -> ExitCode {
    let img = match pe::parse(file) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("vino: {path}: {e}");
            return ExitCode::from(126);
        }
    };
    let mut total = 0;
    for imp in &img.imports {
        let known = dll::dll_of(&imp.dll).is_some();
        println!(
            "{}{}",
            imp.dll,
            if known { "" } else { "  (vino no la tiene)" }
        );
        for (_, sym) in &imp.entries {
            total += 1;
            dll::resolve(&imp.dll, sym);
        }
    }
    let missing = dll::missing::names();
    println!("{total} funciones, faltan {}", missing.len());
    for m in &missing {
        println!("  falta {m}");
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let list = args.first().is_some_and(|a| a == "--imports");
    if list {
        args.remove(0);
    }
    let Some(path) = args.first().cloned() else {
        return usage();
    };
    let file = match std::fs::read(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("vino: {path}: {e}");
            return ExitCode::from(1);
        }
    };
    if list {
        return imports(&path, &file);
    }
    let abs = std::fs::canonicalize(&path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or(path.clone());
    run(&abs, &file, &args)
}

#[cfg(target_os = "linux")]
fn run(path: &str, file: &[u8], args: &[String]) -> ExitCode {
    // `run` no vuelve si el programa arranca.
    match vino::run::run(path, file, args) {
        Err(e) => {
            eprintln!("vino: {path}: {e}");
            ExitCode::from(126)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run(_path: &str, _file: &[u8], _args: &[String]) -> ExitCode {
    eprintln!("vino corre programas en JARVIS-OS (Linux); acá solo sirve `vino --imports`");
    ExitCode::from(1)
}
