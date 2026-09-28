# ADR 0009: TLS y decodificadores PNG/JPEG en el kernel

- **Estado:** propuesta
- **Fecha:** 2026-09-28

## Contexto

Desde K3, todo HTTPS pasa por el puente del anfitrión (ADR 0004, punto 4): el kernel le pide la
URL por HTTP a 127.0.0.1 y el puente hace el TLS. Desde K4 el puente también convierte imágenes
(PNG, JPEG, GIF, WebP, ICO → BMP, ADR 0005) y desde K5 dibuja los SVG (ADR 0006). Los dos ADR
dicen que eso es temporal: cuando haya TLS y decodificadores en el kernel, el puente se achica.

Sin el puente, JARVIS-OS no puede abrir casi ninguna página en una PC real (K13), ni hablarle a la
API de Anthropic directamente (ADR 0008).

## Decisión

1. **TLS con `rustls` en modo `no_std`**, en una crate nueva `kernel/tls` (testeable en el host),
   con las raíces de confianza de `webpki-roots` compiladas adentro. Soporta TLS 1.2 y 1.3 y
   valida la cadena de certificados X.509. La criptografía la pone un **proveedor propio**
   (`tls/src/provider/`) sobre las primitivas de RustCrypto: los de rustls (aws-lc-rs, ring)
   traen C y ensamblador que no compilan para el kernel, y `rustls-rustcrypto` sigue en alfa
   (0.0.2-alpha). Solo suites AEAD con ECDHE (nada de CBC ni de RSA sin
   ECDHE), X25519 y P-256, y firmas ECDSA P-256/P-384 y RSA (PKCS#1 v1.5 y PSS, 2048–8192 bits).
   - **Por qué no un TLS propio:** es el lugar donde un error chico (un MAC mal comparado, una
     extensión mal parseada) rompe la seguridad sin que se note. Lo que se aprende va en
     `docs/kernel.md`: el handshake, el intercambio de claves, X.509 y la cadena de confianza.
2. **Entropía propia.** Las claves de TLS necesitan números al azar impredecibles. El kernel
   junta entropía de RDSEED/RDRAND (si la CPU los tiene) y de la variación del TSC, y la usa como
   semilla de un generador ChaCha20 con *borrado rápido de la clave* (cada pedido rota la clave,
   así que robar el estado no revela lo ya generado). El generador vive en `kernel/tls` (sin
   hardware); las fuentes, en el binario del kernel.
3. **Hora real para los certificados**, del RTC (`rtc.rs`) más el TSC. Si el RTC está muy mal, la
   validación falla: es preferible a aceptar certificados vencidos.
4. **HTTPS directo** desde la pila de red (`kernel/net`) para el navegador, `apt`, `snap`,
   `winget` y `wget`. Sigue pasando por el `Outbox` y el firewall con el nombre de la app
   (regla 20 de CLAUDE.md). Si el sitio no se puede validar, la página de error lo dice; **no hay
   "continuar de todos modos"**.
5. **PNG propio** (inflate, filtros, paletas, transparencia), con tests cruzados contra la crate
   `png` como referencia. **JPEG con `zune-jpeg`** (`no_std`), porque en la web abundan los JPEG
   progresivos.
6. **El puente se achica, no desaparece.** Se queda con: el repositorio de paquetes
   (`http://paquetes.jarvis/`), los SVG, GIF, WebP e ICO (los pide el kernel explícitamente con
   `X-Jarvis-Imagen`), y el HTTPS de respaldo mientras dure la transición, con un interruptor en
   Configuración. La regla 15 de CLAUDE.md se actualiza cuando se acepte este ADR.

## Consecuencias

- (+) JARVIS-OS navega solo: el camino a hardware real (K13) y a la API de Anthropic sin el
  anfitrión queda abierto.
- (+) El anfitrión deja de ver en claro lo que se navega.
- (−) El kernel crece (criptografía, raíces de confianza, decodificadores): unos cientos de KiB.
- (−) El handshake cuesta CPU con punto flotante por software… pero la criptografía es aritmética
  entera, así que no le afecta. Sí hay que medir la pila de las tareas: el handshake usa bastante.
- (−) Dependemos de `rustls` y de RustCrypto en `no_std`; si una versión rompe el soporte
  `no_std`, se fija la versión.
