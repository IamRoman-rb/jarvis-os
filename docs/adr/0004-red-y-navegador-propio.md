# ADR 0004 — Red propia y navegador de texto (en vez de Brave), con un puente HTTPS temporal

- **Estado:** aceptada
- **Fecha:** 2026-09-24

## Contexto

Roman pidió "instalar un navegador, en lo posible Brave, y los drivers de la placa de red para
poder navegar".

**Brave no se puede instalar en JARVIS-OS.** Brave es Chromium: decenas de millones de líneas
que dan por sentado un sistema operativo completo debajo (Linux, Windows o macOS):

- procesos, hilos, memoria virtual por proceso y llamadas al sistema POSIX o Win32;
- un sistema de archivos con permisos, sockets del sistema, un cargador de programas (ELF/PE) y
  bibliotecas compartidas (libc, fuentes, certificados);
- drivers de GPU (OpenGL/Vulkan), sonido, un servidor gráfico o compositor.

JARVIS-OS hoy es un kernel de un solo hilo, sin espacio de usuario (ring 3), sin llamadas al
sistema ni cargador de programas. Portar Chromium exigiría primero construir todo eso: años de
trabajo, y es el hito K8 del roadmap (espacio de usuario) y mucho más.

Lo que sí se puede hacer ahora, y que además es el mismo camino que siguieron los sistemas de
hobby (SerenityOS, ToaruOS), es construir la red y un navegador propios.

## Decisión

1. **Driver de placa de red propio: virtio-net** (`kernel/kernel/src/virtio_net.rs`), la placa
   virtual de QEMU/KVM, con el mismo mecanismo de virtqueues que el disco. Las placas reales
   (Intel e1000, Realtek) quedan para el hito de hardware real.
2. **Pila TCP/IP: smoltcp** (crate `jarvis-net`). Escribir TCP desde cero es un proyecto en sí
   mismo (retransmisiones, ventanas, control de congestión); smoltcp está hecho para kernels
   `no_std` y se puede leer y estudiar. DHCP y DNS vienen con ella.
3. **Navegador de texto propio** (`desktop/src/apps/browser.rs`, `desktop/src/web/`): HTTP/1.1
   propio (con redirecciones y "chunked"), HTML convertido a texto con títulos, listas, citas y
   enlaces, "modo lectura" que saltea menús y pies de página, historial, buscador (DuckDuckGo en
   su versión HTML) y páginas locales (`file://`). Sin JavaScript ni CSS, como Lynx o w3m.
4. **HTTPS por un puente en el anfitrión, temporal.** Casi toda la web exige TLS. Un TLS en el
   kernel necesita criptografía, números aleatorios de calidad y verificación de certificados;
   hacerlo bien es un hito entero. Mientras tanto, `cargo xtask run` levanta un puente
   (`xtask/src/puente.rs`) en `127.0.0.1:8118` que recibe `GET https://…` (como un proxy HTTP),
   hace la conexión HTTPS verificando el certificado y devuelve la respuesta en claro. Las páginas
   `http://` van directo desde el kernel (DNS y TCP propios); si el DNS falla, se usa el puente.

## Consecuencias

- Se puede navegar desde JARVIS-OS hoy, y todo el camino hasta el cable (driver, TCP/IP, HTTP,
  HTML) es código del proyecto o una biblioteca `no_std` embebida en el kernel.
- Las páginas que dependen de JavaScript para mostrar su contenido no se ven bien.
- **Seguridad del puente**: solo escucha en 127.0.0.1 (desde otra máquina no se puede usar) y solo
  acepta GET. Mientras corre, cualquier programa del anfitrión podría usarlo como proxy; por eso
  vive solo lo que dura `cargo xtask run`. Agregarle otros métodos (POST) o exponerlo en la red
  requiere un ADR nuevo.
- Cuando haya TLS en el kernel (roadmap), el puente se elimina. Cuando haya espacio de usuario,
  se puede evaluar portar un navegador existente más completo (por ejemplo, NetSurf, que es
  mucho más chico que Chromium).
