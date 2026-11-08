# ADR 0020: usuarios, `sudo` y permisos de archivos

- **Estado:** aceptada
- **Fecha:** 2026-11-08

## Contexto

Roman quiere instalar programas como en Ubuntu: `sudo apt install`, `sudo snap install`,
`chmod +x`, `chown`. Hasta ahora `sudo` solo ejecutaba lo que seguía ("en JARVIS-OS sos el
dueño de todo") y no había permisos: FAT32 no tiene dueños ni modos (`rwxr-xr-x`), solo un bit
de "solo lectura". Los programas de Linux (K11) recibían siempre `0644`/`0755` y uid 1000.

Referencias: `sudo-rs` (la reescritura de sudo en Rust que usa Ubuntu desde 25.10: contraseña,
`timestamp_timeout` de 15 minutos, 3 intentos), los metadatos de WSL para discos de Windows
(DrvFs guarda uid/gid/modo aparte del sistema de archivos) y `coreutils` (`chmod` simbólico).

## Decisión

1. **Dos usuarios**: `root` (uid 0) y el del asistente de instalación (uid 1000, grupos
   `roman` y `sudo`). La contraseña es la del asistente (el mismo hash PBKDF2 de `pin.rs`).
2. **Permisos fuera del FAT32**: `/Sistema/permisos.db` guarda `modo uid gid ruta` solo de lo que
   difiere del valor por defecto (`desktop/src/perms.rs`). El FAT32 no cambia (Windows lo sigue
   leyendo) y no hace falta tocar `jarvis-fs` (regla 11). Por defecto:
   - las carpetas del sistema (`/Sistema`, `/Programas`, `/snap`, `/usr`, `/lib*`, `/bin`,
     `/etc`, `/opt`, `/var`, `/Cuarentena`) son de root, `0755`; sus archivos `0644`, y `0755`
     los de una carpeta `bin`, las bibliotecas `.so` y los `.exe`;
   - `/Sistema/config.ini` (tiene el hash de la contraseña) es `0600`, como `/etc/shadow`;
   - el resto es del usuario: `0755` las carpetas, `0644` los archivos.
3. **Las reglas de Linux**: root lee y escribe todo; ejecutar pide algún bit `x`. Los demás,
   según los bits del dueño, del grupo o de los otros. Crear, borrar o renombrar pide escribir
   en la carpeta. No se mira el permiso de "atravesar" las carpetas de arriba (`x` de cada
   carpeta): con un solo usuario no cambia nada y costaría un `stat` por nivel.
4. **`sudo`** pide la contraseña al empezar la línea (sin mostrar lo que se tipea), la recuerda
   15 minutos y se rinde a los 3 intentos. `-k`, `-v`, `-l`, `-i`/`-s`, `-u root`. Adentro de un
   script, sin contraseña reciente, falla (no hay dónde pedirla). Sin contraseña configurada
   (un disco viejo, QEMU), anda directo, como `NOPASSWD`; `passwd` la define.
5. **Lo que pide root**: `apt` (install, remove, update, upgrade…), `snap` (install, remove,
   refresh, revert), `winget` (install, uninstall, upgrade) y `ufw` (lo que cambia reglas), con
   los mensajes de Debian, snapd y ufw. Escribir en las carpetas del sistema.
6. **Programas de Linux**: corren con el uid de quien los lanzó (0 con `sudo`). `stat` devuelve
   el modo y el dueño reales; `access` los mira; `chmod`, `fchmod`, `fchmodat`, `chown`,
   `lchown`, `fchown`, `fchownat` y `umask` existen. El escritorio controla cada pedido de
   archivos (`procs.rs`): `EACCES`/`EPERM` si no corresponde.
7. **Archivos** (la app) no cambia carpetas del sistema: avisa que se use `sudo` en la Terminal.
   Configuración sí puede cambiar el firewall (es el sistema, como polkit en GNOME).
8. Un `.exe` no necesita el bit `x` (Windows no lo tiene): `vino` lo corre igual.

## Consecuencias

- (+) `sudo apt install`, `chmod +x script.sh && ./script.sh`, `sudo chown`, `su` y `passwd`
  funcionan como en Ubuntu, y los programas de Linux ven permisos de verdad.
- (+) Un programa (o un script bajado de internet) corrido sin `sudo` no puede tocar el sistema.
- (−) Los permisos viven en un archivo aparte: si alguien mueve archivos desde Windows, la tabla
  no se entera (vuelven al valor por defecto de su carpeta).
- (−) No hay más usuarios ni `/etc/passwd` real; `chown` acepta números para los demás.
- (−) Las apps del escritorio (Editor, Configuración) no pasan por los permisos: son el sistema.
