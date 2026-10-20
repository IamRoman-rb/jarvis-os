# ADR 0016: programas de Windows en el mismo kernel

- **Estado:** reemplazada por el [ADR 0018](0018-linux-puro-y-vino.md) (2026-10-20): el kernel
  habla solo Linux y los programas de Windows los corre `vino`, un programa de Linux
- **Fecha:** 2026-10-05

## Contexto

Roman quiere bajar un programa de Windows desde Brave y uno de Linux desde la terminal, y que
los dos corran **dentro del mismo kernel**. Los de Linux ya corren (ADR 0010: procesos aislados
en el anillo 3, con la ABI de Linux). Los de Windows se bajan (Brave → `/Descargas`, `winget`)
y se inspeccionan (`file`), pero no se ejecutan.

Un programa de Windows no le habla al kernel directo: llama a funciones de DLL (`kernel32.dll`,
el CRT en `ucrtbase.dll` o `msvcrt.dll`, `ntdll.dll`…), y son esas DLL las que hablan con el
kernel de Windows. Así trabaja Wine en Linux.

## Decisión

1. **Los dos tipos de programa son procesos del mismo kernel.** Mismo planificador, misma
   memoria, mismo disco, mismo firewall. Lo que cambia es el formato del ejecutable (ELF o PE) y
   a quién llama el programa.
2. **Las DLL de Windows las escribimos nosotros, en Rust,** como DLL de Windows normales
   (`no_std`, `cdylib` para `x86_64-pc-windows-msvc`), en un workspace aparte
   (`kernel/windows/`). Las compila `cargo xtask` con el toolchain de siempre y van al disco
   (`/Sistema/windows/`). Por dentro usan las **mismas llamadas al sistema de Linux** que ya
   tiene el kernel (`read`, `write`, `openat`, `mmap`, `clock_gettime`, `getrandom`…): la capa
   de Windows vive casi entera en el espacio de usuario, como Wine.
3. **El kernel solo carga imágenes PE** que hizo un compilador: el programa y sus DLL, en su
   lugar, con las relocalizaciones y los imports resueltos contra los exports (incluidos los
   reenvíos y los "api sets" `api-ms-win-*`). No genera código. Además arma lo que un programa
   de Windows espera encontrar: el TEB en `GS` (con el PEB, la pila y el TLS) y los datos de
   `__declspec(thread)`.
4. **El arranque lo hace `ntdll.dll`** (una función exportada): llama los callbacks de TLS y
   los `DllMain`, y después el punto de entrada del programa.
5. **Si falta algo, se dice.** Un import que ninguna DLL nuestra ofrece se resuelve a una
   función que termina el programa con "falta KERNEL32!Foo" (en la Terminal y en el log), en
   vez de colgarse o romper en otro lado.
6. **Cómo se ejecutan**: en la Terminal, `./programa.exe` o `wine programa.exe`; en Archivos,
   doble clic (la app predeterminada de `.exe`). La salida va a la Terminal, como los de Linux.

## Qué no entra todavía

Programas de 32 bits, de .NET, con ventanas (GDI/USER: un `MessageBox` se muestra en la
Terminal), con varios hilos y los instaladores. Cada uno se agrega cuando haga falta, como en
ADR 0010.

## Consecuencias

- (+) Un solo kernel para los dos: lo que se mejora para uno (archivos, red, memoria) le sirve
  al otro.
- (+) Las DLL son código Rust normal, con tests en el anfitrión, y se cambian sin tocar el
  kernel.
- (−) La API de Windows es enorme: se implementa lo que piden los programas que corremos
  (empezando por los de consola compilados con Visual Studio y con Rust).
