# ADR 0014: Sin navegador propio; aplicaciones predeterminadas

- **Estado:** aceptada (reemplaza la parte del navegador de los ADR 0004 y 0006)
- **Fecha:** 2026-10-05

## Contexto

K3–K6 hicieron un navegador propio dentro del kernel: HTML, CSS con la cascada, maquetación en
cajas (flex, grid, tablas), una fuente proporcional, imágenes y adaptadores para algunos sitios.
Sirvió para aprender, pero sin JavaScript la web de hoy se ve incompleta, y desde el ADR 0007
está Brave (en el anfitrión, mostrado por JARVIS-OS) para navegar de verdad. Roman pidió sacarlo
por completo.

## Decisión

1. Se borran `apps/browser.rs` y del motor web todo lo que solo usaba el navegador: `web/css.rs`,
   `dom.rs`, `html.rs`, `layout.rs`, `style.rs`, `sites.rs`, la fuente de las páginas
   (`gfx/webfont.rs`, las DejaVu de `gfx/fonts/`) y las dependencias `fontdue` y `hashbrown`.
   Quedan `web/url.rs`, `web/http.rs` (los usan la red, apt, curl, el firewall) y `web/json.rs`
   (el cerebro, snap, winget).
2. `Launch::Browse` abre Brave: una dirección, o una búsqueda (`? texto`) con el buscador
   elegido. F1 abre la ayuda en la terminal. La app de firewall `navegador` deja de existir.
3. **Aplicaciones predeterminadas** (`desktop/src/defaults.rs`, Configuración → Aplicaciones
   predeterminadas): con qué app se abre cada tipo (carpetas, texto, código, páginas .html,
   imágenes, videos, audio), entre las que de verdad lo pueden abrir. Es el único lugar que lo
   decide: Archivos, `abrir_archivo` y `open` lo usan. Brave no abre archivos del disco de
   JARVIS-OS (corre en el anfitrión): los .html van al editor o a la terminal.

## Consecuencias

- (+) ~12 000 líneas y 1,7 MB de fuentes menos en el kernel; nada que mantener contra la web.
- (+) Una sola regla para abrir archivos (antes había tres que no coincidían).
- (−) Sin el anfitrión (Brave) no hay navegador: en una PC real sin puente de Brave, JARVIS-OS
  baja cosas con `curl`/`wget`, pero no muestra páginas.
- Las secciones de historia de `docs/kernel.md` (K3–K6) siguen contando cómo se hizo.
