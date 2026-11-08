# ADR 0024: firewall con listas y preguntas; antivirus con reglas y heurística

- **Estado:** aceptada
- **Fecha:** 2026-11-08
- **Amplía:** ADR 0006 (firewall) y ADR 0017 (antivirus)

## Contexto

Roman pidió mejorar el firewall y el antivirus mirando proyectos de GitHub. El firewall tenía
reglas por sitio, puerto y app (como `ufw`); el antivirus, solo hashes de MalwareBazaar y EICAR:
detecta un archivo **idéntico** a una muestra conocida y nada más.

Referencias estudiadas (no se copió código): **OpenSnitch** (preguntar por cada conexión nueva),
**Portmaster** (monitor de conexiones por app), **pi-hole** y **StevenBlack/hosts** (listas de
dominios), **URLhaus** de abuse.ch (dominios que reparten malware), **YARA**/**yara-x** de
VirusTotal (reglas), **capa** de Mandiant (capacidades de un ejecutable por sus imports) y
**ClamAV** (formato de firmas).

## Decisión

### Firewall

1. **Listas de bloqueo** (`desktop/src/blocklist.rs`): URLhaus y StevenBlack. Se guardan como
   hashes FNV-1a de 64 bits ordenados (8 bytes por dominio, búsqueda binaria); bloquean el
   dominio y sus subdominios. Se miran **después** de las reglas: una regla `allow` explícita
   gana (la lista blanca de pi-hole). `sudo ufw listas actualizar|activar|desactivar`.
2. **Política "preguntar"** (`ufw default ask outgoing`): la conexión nueva espera y aparece un
   diálogo: permitir en esta sesión, permitir siempre (regla) o denegar siempre (regla). Sin
   respuesta en 60 s, se deniega. Esc deniega por la sesión.
3. **Registro de conexiones**: app, sitio, puerto, resultado y cuántas veces (las últimas 200).
   `ufw show conexiones` y Configuración → Firewall.
4. **Reglas por programa de Linux**: `app programas:curl` (y `app programas`, todos).

### Antivirus

5. **Motor de reglas tipo YARA propio** (`kernel/reglas/`, `no_std`): cadenas de texto
   (`nocase`, `wide`, `ascii`), hexadecimales con comodines y saltos, y las condiciones comunes
   (`and/or/not`, `any/all/N of`, `#a`, `$a at`, `$a in`, `filesize`, `uint16/32`). Todas las
   cadenas se buscan en una sola pasada con un autómata de Aho-Corasick propio. Lo que no
   soporta (módulos como `pe`, alternativas en hex) da un error claro al compilar.
6. **Reglas propias** (`kernel/paquetes/antivirus/reglas.yar`): mineros, volcadores de
   credenciales, webshells, notas de rescate, PowerShell y scripts que bajan y ejecutan, rootkits
   de `ld.so.preload`. Vienen adentro del sistema y se actualizan con `antivirus actualizar`
   (solo se guardan si compilan). Severidad **alta** → cuarentena; **media** → aviso.
7. **Heurística** (`reglas/src/heuristica.rs`): imports de PE combinados (inyección en otros
   procesos, keylogger, cifrado masivo con borrado, bajar y ejecutar), UPX, entropía de las
   secciones; en ELF, pools de minería y `ld.so.preload`. Solo **avisa**: un indicio no es una
   prueba (un depurador también lee la memoria de otros procesos).
8. **Firmas de ClamAV** `.hsb` (SHA-256): `antivirus importar`. Las `.hdb` (MD5) no.
9. `antivirus analizar ARCHIVO` muestra todo lo que se sabe de un archivo.

## Consecuencias

- (+) El firewall bloquea miles de sitios de malware y publicidad sin escribir reglas, y Roman
  puede decidir conexión por conexión.
- (+) El antivirus reconoce familias (no solo archivos idénticos) y ejecutables con
  comportamiento peligroso.
- (−) Las reglas propias son pocas: las públicas (Yara-Rules, signature-base) tienen otras
  licencias y muchas usan módulos que el motor no tiene.
- (−) La heurística puede marcar programas legítimos (por eso solo avisa).
- (−) El modo "preguntar" no está probado todavía de punta a punta en QEMU.
