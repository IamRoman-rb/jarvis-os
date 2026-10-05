# ADR 0013: JARVIS modifica JARVIS-OS desde adentro

- **Estado:** aceptada
- **Fecha:** 2026-10-04

## Contexto

Roman quiere pedirle a JARVIS cambios al propio sistema ("modificá tal interfaz", "agregá
estos drivers", cambios más grandes) y que los haga, sin salir de JARVIS-OS. El código del
sistema (kernel en Rust y cerebro en Python) está en el anfitrión, en el repo de `jarvis-os`, y
JARVIS-OS corre en QEMU desde una imagen que arma `cargo xtask run`. Ya existía `abrir_proyecto`
(ADR 0008): un agente de Claude Code trabajando en una carpeta, con cada edición y comando
confirmados en la pantalla de JARVIS.

## Decisión

1. **`modificar_sistema(pedido)`** (nivel 2) arranca `OsProject`: el agente de proyectos sobre
   el repo de JARVIS-OS, con instrucciones propias (`projects.py`, `OS_PROMPT`): ubicar el
   código con el grafo de graphify antes de leer, seguir CLAUDE.md y los ADR, verificar con
   tests, clippy y fmt, no commitear ni tocar `target/`. Su avance se ve en la ventana Proyecto.
2. **Permisos del agente**: los de cualquier proyecto (leer: 1; editar: 2; lo demás: 3), salvo
   comandos sueltos sin metacaracteres de shell: los que solo leen (`graphify query/path/
   explain`, `git status/diff/log/show`) van en 1 y los que verifican (`cargo test/check/
   clippy/build/fmt`, `uv run pytest/ruff/mypy`) en 2. Así se pueden correr los tests sin un
   clic obligatorio cada vez, y nada encadenado pasa de largo (ver `docs/permisos.md`).
3. **`aplicar_cambios_sistema`** (nivel 3), cuando el agente terminó y Roman quiere verlo:
   - el cerebro **verifica antes de apagar**: compila el kernel (el mismo `cargo build` que usa
     `xtask`; con QEMU corriendo se puede, porque QEMU usa la imagen y no el binario) e importa
     el cerebro nuevo. Si falla, no reinicia y el error vuelve a JARVIS para corregirlo;
   - si compila, deja la marca `target/jarvis-reiniciar` y le manda `reiniciar` al kernel, que
     avisa y se **apaga** a los 4 segundos;
   - `cargo xtask run` ve la marca al terminar QEMU, vuelve a armar la imagen y los programas de
     Linux, levanta el cerebro de nuevo (con su código nuevo) y arranca QEMU otra vez. Si no
     compila, arranca la imagen anterior;
   - deja cómo le fue en `target/jarvis-actualizacion.txt`, y el saludo del arranque lo cuenta.
4. El cerebro sabe dónde está todo por variables de entorno que pone `xtask`: `JARVIS_REPO`,
   `JARVIS_REINICIO` y `JARVIS_ACTUALIZACION`. Sin `xtask` (por ejemplo `jarvis serve` a mano)
   se puede modificar el sistema, pero aplicar pide reiniciar a mano.

## Consecuencias

- (+) Cualquier cambio que se pueda hacer en el repo (interfaz, apps, drivers, kernel, cerebro)
  se puede pedir hablando, con el mismo control que el resto: Roman aprueba cada edición.
- (+) Nunca se apaga hacia algo que no compila; si igual algo falla al armar la imagen, vuelve
  la versión anterior.
- (−) "Compila" no es "anda": un cambio puede compilar y romper algo en tiempo de ejecución. El
  agente corre los tests antes, pero los de punta a punta (`cargo xtask test`) no se corren
  solos porque tardan; se pueden pedir.
- (−) Los cambios quedan sin commitear en el repo: deshacerlos es pedirlo (`git checkout`, con
  confirmación de nivel 3) o hacerlo a mano.
- (−) En una PC real sin anfitrión (instalado desde la ISO) no hay repo ni `xtask`: modificar el
  sistema solo funciona en el modo de desarrollo, con QEMU.
