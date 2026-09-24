# ADR 0006 — Motor de maquetación propio, firewall, tiendas de programas e idiomas

- **Estado:** aceptada
- **Fecha:** 2026-09-24

## Contexto

Roman pidió, después de K4:

1. que las páginas **se vean como en un navegador** (Wikipedia salía como una lista de enlaces
   uno debajo del otro; YouTube, en blanco);
2. **snap** y otras herramientas para bajar programas;
3. **cambiar el idioma** desde la Configuración;
4. un **firewall**.

El navegador de K4 entendía CSS, pero no tenía maquetación: convertía la página en bloques de
texto uno debajo del otro. Sin cajas no hay columnas, márgenes, centrado ni menús en fila, que es
casi todo lo que hace que una página "se vea bien". Además, la única fuente del sistema era
monoespaciada y en cuatro tamaños (16, 20, 24 y 32 px).

## Decisión

### 1. Un motor de maquetación propio (`desktop/src/web/`)

La página pasa por las mismas etapas que en un navegador de verdad, cada una en su módulo
`no_std`, testeable en el host:

| Etapa | Módulo | Qué hace |
|---|---|---|
| HTML → árbol | `dom.rs` | cierres implícitos, `<html>` siempre presente |
| Cascada | `css.rs`, `style.rs` | selectores completos, `@media` con el ancho real, `calc()`, variables heredadas, estilos del navegador |
| Maquetación | `layout.rs` | flujo con márgenes colapsados, flotantes, flex, grid (áreas, `auto-fill`), tablas, posiciones, `overflow` |
| Dibujo | `apps/browser.rs` | fondos, bordes, texto, imágenes, íconos con máscara, campos de formulario |

Por qué propio y no portar uno (Servo, Blitz, Ladybird): esos motores dependen de `std`, de hilos
y de cientos de miles de líneas; portarlos es un proyecto en sí mismo. Uno propio de unas 10.000
líneas (con sus tests) cubre lo que usan la mayoría de las páginas y sirve para aprender cómo
funciona la web por dentro (el objetivo del proyecto).

**Fuente proporcional**: DejaVu Sans Condensed (anchos parecidos a Arial) y DejaVu Sans Mono, con
licencia libre, rasterizadas en el momento con `fontdue` (`no_std`) al tamaño que pide la página.
Cada letra se rasteriza una vez y queda en caché. Los archivos están en `kernel/gfx/fonts/`.

**Imágenes**: el puente del anfitrión ya convertía PNG/JPEG a BMP. Ahora también **dibuja los SVG**
(con `resvg`) y, si la imagen tiene partes transparentes, manda un BMP de 32 bits con canal alfa
(el kernel lo mezcla con el fondo). Los íconos hechos con `mask-image` se pintan con la forma de la
máscara.

**JavaScript: no.** Un intérprete de JavaScript es un hito en sí mismo (QuickJS está en C; Boa
necesita `std`), y sin él las páginas que se arman con JavaScript (YouTube, Instagram, Gmail)
llegan vacías. Por ahora:
- el navegador avisa cuando una página "necesita JavaScript" (casi sin texto y con scripts);
- hay **adaptadores por sitio** (`web/sites.rs`): YouTube trae la lista de videos como JSON dentro
  de la página; se lee y se arma una versión en HTML simple (búsquedas y videos, con miniaturas).
  Los videos no se reproducen: faltan decodificadores de video y audio.

### 2. El firewall controla el `Outbox`, no los paquetes (`desktop/src/firewall.rs`)

En JARVIS-OS ninguna app abre conexiones: le piden al kernel que baje una dirección, y todo eso
pasa por el `Outbox` del escritorio. Ahí se revisa cada pedido **antes** de que llegue a la red
(ni siquiera sale la consulta DNS). Cada pedido lleva el nombre de la app que lo hizo, así las
reglas pueden ser por sitio, puerto **o app** (`navegador`, `terminal`, `apt`, `snap`, `winget`…).
Las reglas se evalúan en orden y gana la primera, como en `ufw` de Ubuntu, y el comando `ufw` de
la terminal usa su sintaxis. Lo bloqueado se anota en `/Sistema/firewall.log`.

**Lo que entra**: el kernel no escucha en ningún puerto, así que la pila TCP/IP rechaza cualquier
conexión que no haya abierto JARVIS-OS. La política de entrada existe y se guarda, para cuando
haya servicios.

Cuando haya espacio de usuario y sockets (K10), los programas podrán abrir conexiones por su
cuenta: el firewall tendrá que bajar a la pila de red (`jarvis-net`), con las mismas reglas.

### 3. `snap` y `winget`

- **snap** (`term/snap.rs`): una tienda propia (`kernel/paquetes/snaps/`, la sirve el puente) con
  programas que corren en JARVIS-OS, con **canales** (`stable`, `beta`), **revisiones** que se
  guardan aparte (`/snap/<nombre>/<revisión>`), `snap refresh` y `snap revert`. Además, `snap find`
  e `info` consultan **la tienda real de Snapcraft** y `snap download` baja un `.snap` de Linux
  (se inspecciona; no se ejecuta, igual que los `.exe`, ADR 0005).
- **winget** (`term/winget.rs`): busca en una lista de programas conocidos
  (`kernel/paquetes/winget.txt`) y resuelve cualquier Id contra el **repositorio oficial de
  Microsoft** en GitHub (`microsoft/winget-pkgs`): lee los manifiestos, elige el instalador de 64
  bits y lo baja a `/Descargas`. No se ejecuta (ADR 0005).

**Cambio en el puente** (regla 15 de CLAUDE.md): la API de Snapcraft exige la cabecera
`Snap-Device-Series`. El puente pasa esa y `Snap-Device-Architecture`, y ninguna otra. Sigue
escuchando solo en 127.0.0.1 y aceptando solo GET.

### 4. Idiomas (`desktop/src/i18n.rs`)

Los textos se siguen escribiendo en castellano en el código. `tr("Papelera")` busca la traducción
del idioma elegido en tablas ordenadas (inglés y portugués); `trf` completa los textos con datos.
Así no hay que inventar identificadores para cada texto, y un texto sin traducir se ve en
castellano en vez de romperse. Solo Latin-1 (la fuente de la interfaz no tiene otros caracteres).

Quedan en castellano, a propósito: la terminal (sus mensajes imitan a los de Linux en castellano)
y lo que sale por el puerto serie, que es lo que leen los tests de punta a punta.

## Consecuencias

- (+) Las páginas comunes (Wikipedia, Google, diarios, documentación) se ven parecidas a como se
  ven en Chrome o Firefox: columnas, menús, colores, imágenes e íconos.
- (+) Todo lo nuevo es `no_std` y se prueba en el host; la vista previa
  (`tests/vista_previa.rs`) arma una página real sin QEMU, con sus imágenes.
- (−) Sin JavaScript, las aplicaciones web siguen sin funcionar; los adaptadores por sitio son un
  parche y hay que mantenerlos si el sitio cambia.
- (−) El kernel crece ~1,7 MB por las fuentes, y la maquetación usa `f32` por software en algunas
  cuentas (`calc()`, porcentajes): se hace una vez por página, no por frame.
- (−) El firewall cubre todo lo que existe hoy, pero no es un filtro de paquetes: eso llega con
  los sockets de espacio de usuario.
