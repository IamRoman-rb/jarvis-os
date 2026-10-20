# ADR 0018: JARVIS-OS habla solo Linux; los programas de Windows corren con `vino`

- **Estado:** aceptada
- **Fecha:** 2026-10-20
- **Reemplaza:** [ADR 0016](0016-programas-de-windows.md)

## Contexto

El ADR 0016 ponía los programas de Windows **en el kernel**: el kernel cargaba las imágenes PE,
armaba el TEB y resolvía los imports contra DLL de Windows escritas en Rust y compiladas como PE.
Se había hecho el parser de PE (`linux/src/pe.rs`) y guardar/restaurar GS por tarea; ningún
programa de Windows corría todavía.

Roman pidió un giro: que JARVIS-OS sea **100% Linux** en lo que ven los programas, respetando el
kernel propio, y que los programas de Windows corran con una aplicación parecida a Wine. "100%
Linux" se definió como **ABI de Linux pura**: el kernel solo entiende la ABI de Linux x86-64;
nada de Windows vive adentro. El escritorio y las apps siguen como están.

## Decisión

1. **El kernel solo habla Linux.** Sale el parser de PE del kernel (se muda a `vino`). Lo único
   que se agrega al kernel es Linux de verdad: `arch_prctl(ARCH_SET_GS / ARCH_GET_GS)` (y la base
   de GS se guarda por tarea) y `MAP_FIXED_NOREPLACE` con su semántica real (`EEXIST` si pisa
   algo).
2. **`vino` es un programa de Linux más** (`kernel/vino/`, Rust, estático con musl, anillo 3):
   se instala con `apt install vino`, pasa por el antivirus y el firewall como cualquier
   programa, y no tiene permisos especiales. Hace lo que hace Wine en Linux:
   - carga el `.exe` con `mmap` (en su `ImageBase` si está libre; si no, relocaliza), pone los
     permisos de cada sección con `mprotect` y llena la IAT;
   - arma el TEB, el PEB y los parámetros del proceso, y pone el TEB en GS con `arch_prctl`;
   - prepara el TLS del programa (`__declspec(thread)`), llama a sus callbacks de TLS y salta al
     punto de entrada en una pila propia.
3. **Las DLL son "builtin": funciones Rust con la convención de Windows** (`extern "win64"`)
   adentro de `vino`, no archivos PE (cambio respecto del ADR 0016). Por dentro usan la
   biblioteca estándar de Rust, o sea, llamadas de Linux. Así no hace falta un segundo
   toolchain, se prueban en el anfitrión con `cargo test` y una función nueva es una función más.
   Hay `kernel32`, `ntdll`, el CRT de Microsoft (`ucrtbase` con sus "api sets"
   `api-ms-win-crt-*`, `msvcrt` y `vcruntime140`), `bcryptprimitives`/`advapi32` (azar) y
   `user32` (`MessageBox` a la Terminal). `printf` de `msvcrt` (argumentos variables) se arma
   con funciones `naked` que guardan los registros en la sombra de 32 bytes y saltan a `vprintf`.
4. **Si falta algo, se dice** (se conserva del ADR 0016). Cada import que no existe apunta a una
   función propia (generada por el compilador con genéricos, no en tiempo de ejecución) que dice
   `vino: falta KERNEL32!Foo` y termina con código 3. `vino --imports programa.exe` lista lo que
   pide y lo que falta sin correrlo.
5. **Cómo se usa.** `vino programa.exe`, `wine programa.exe` (alias) o `./programa.exe` (la
   Terminal le pasa el `.exe` a `vino`, como `binfmt_misc`). En Archivos, doble clic. La Terminal
   es la consola: lo que escribe el programa aparece ahí y lo que se tipea es su entrada.
6. **Decisiones de compatibilidad**: el texto "ANSI" es UTF-8 (`GetACP` = 65001, como Windows con
   UTF-8 en todo el sistema); todas las unidades (`C:`, `D:`…) son la raíz del disco; la salida a
   la Terminal pierde los `\r` de `\r\n`; los archivos se escriben sin `\r` (quedan como los de
   Linux); `GetConsoleMode` falla (la consola se ve como una tubería, así los programas usan
   `WriteFile`).
7. **Los `.exe` de prueba van compilados en el repositorio** (`kernel/usuario-win/bin/`, ~650 KB),
   como los `.deb` de los tests de `apt`: la CI corre en Linux y no tiene el compilador de
   Microsoft. Se regeneran con `cargo xtask windows` en una PC con Visual Studio. Son cuatro
   (Rust y C, con el CRT en DLL y adentro) más uno que usa una función que falta.

## Qué no entra todavía

Lo mismo que en el ADR 0016: 32 bits (WoW64), .NET, ventanas (GDI/USER), varios hilos
(`CreateThread` devuelve error hasta que el kernel tenga `clone`), instaladores, excepciones (SEH
y C++: el programa termina con un aviso) y DLL de terceros (las que vienen al lado del `.exe`; el
parser ya lee exports y reenvíos). `scanf`, el registro de Windows y `system()` tampoco.

## Consecuencias

- (+) El kernel queda más chico y con una sola ABI: todo lo que se mejora para los programas de
  Linux (memoria, archivos, red) le sirve a `vino` sin tocarlo.
- (+) Un bug de `vino` no tumba el sistema: es un proceso aislado en el anillo 3. Y se puede
  desinstalar (`apt remove vino`).
- (+) La API de Windows se prueba en el anfitrión: 30 tests de `vino` corren en Windows y en Linux
  (las funciones `win64` se llaman de verdad).
- (−) Cada llamada de un programa de Windows pasa por dos capas (la DLL builtin y la llamada de
  Linux); para programas de consola no se nota.
- (−) Los `.exe` de prueba son binarios en el repositorio: hay que regenerarlos si cambian sus
  fuentes.
- (−) La API de Windows es enorme: se implementa lo que piden los programas que se prueban.
