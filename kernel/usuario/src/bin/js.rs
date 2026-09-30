//! `js`: un intérprete de JavaScript que corre como programa de Linux en JARVIS-OS (K11).
//!
//! El motor es Boa (escrito en Rust); acá solo está la parte de "programa de consola", como
//! `node`:
//!
//! - `js` sin argumentos: la consola interactiva (REPL). Cada línea se evalúa y se muestra el
//!   resultado. `.exit` o Ctrl+D salen.
//! - `js archivo.js`: corre el archivo.
//! - `js -e 'código'`: corre el código y muestra el resultado.
//!
//! `console.log` (y `setTimeout`, `atob`, `TextEncoder`…) los pone `boa_runtime`.

use std::io::{BufRead, Write};

use boa_engine::{Context, JsValue, Source};
use boa_runtime::extensions::ConsoleExtension;

fn context() -> Context {
    let mut ctx = Context::default();
    if let Err(e) = boa_runtime::register(ConsoleExtension::default(), None, &mut ctx) {
        eprintln!("js: no se pudo preparar la consola: {e}");
    }
    ctx
}

/// Evalúa y corre lo pendiente (promesas, `setTimeout`). `Err` con el mensaje del error.
fn eval(ctx: &mut Context, code: &str) -> Result<JsValue, String> {
    let r = ctx
        .eval(Source::from_bytes(code))
        .map_err(|e| format!("Uncaught {e}"));
    if let Err(e) = ctx.run_jobs() {
        return Err(format!("Uncaught {e}"));
    }
    r
}

fn repl(ctx: &mut Context) {
    println!("JavaScript (motor Boa) en JARVIS-OS. `.exit` o Ctrl+D para salir.");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    loop {
        print!("> ");
        let _ = std::io::stdout().flush();
        let Some(Ok(line)) = lines.next() else {
            println!();
            break;
        };
        let line = line.trim();
        if line == ".exit" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        match eval(ctx, line) {
            Ok(v) => println!("{}", v.display()),
            Err(e) => println!("{e}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut ctx = context();
    match args.get(1).map(String::as_str) {
        None => repl(&mut ctx),
        Some("-e") => {
            let code = args.get(2).map_or("", |s| s.as_str());
            match eval(&mut ctx, code) {
                Ok(v) if v.is_undefined() => {}
                // Un texto sale tal cual (como `node -p`); lo demás, como en la consola.
                Ok(v) => match v.as_string() {
                    Some(s) => println!("{}", s.to_std_string_escaped()),
                    None => println!("{}", v.display()),
                },
                Err(e) => {
                    println!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some(path) => {
            let code = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("js: {path}: {e}");
                    std::process::exit(2);
                }
            };
            if let Err(e) = eval(&mut ctx, &code) {
                println!("{e}");
                std::process::exit(1);
            }
        }
    }
}
