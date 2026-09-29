# ADR 0007: Conexiones largas, Brave remoto, sincronización entre máquinas e ISO

- **Estado:** aceptada
- **Fecha:** 2026-09-24

## Contexto

Después de K5, Roman pidió:

1. **Dejar el navegador propio y usar Brave.**
2. **Sincronizar una carpeta entre dos máquinas**, que no tienen por qué estar en la misma red.
3. **Instalar el sistema desde una ISO.**

Hasta ahora la red del kernel hacía una sola cosa: pedidos HTTP GET que terminan (`fetch`). El
puente del anfitrión (ADR 0004/0005/0006) escucha solo en 127.0.0.1 y solo acepta GET.

Brave es un programa de Windows/Linux de decenas de millones de líneas. Para correrlo adentro de
JARVIS-OS hace falta todo el camino de K11: espacio de usuario, ABI de Linux, bibliotecas
dinámicas, hilos y un servidor gráfico. En la PC de Roman no hay WSL ni Docker, así que tampoco
hay una VM de Linux donde correrlo al lado.

## Decisión

### 1. Conexiones TCP largas en `jarvis-net`

`Net::stream(StreamOp)` abre, escribe y cierra conexiones TCP que duran lo que la app quiera.
`take_stream_events()` devuelve `Connected`, `Data` y `Closed(motivo)`.

- Las apps las piden por el `Outbox` (`connect`, `send`, `close_stream`), y el **firewall** revisa
  cada `connect` con el nombre de la app (regla 20).
- `brave` y `sync` son apps nuevas del firewall.
- Buffers de 1 MiB para recibir y 64 KiB para mandar. Lo que la app manda de más queda en una
  cola, con un tope de 4 MiB: si el otro lado no lee, es un error y no se come la memoria.
- Un socket cerrado sigue en la pila hasta mandar su FIN, así el otro lado ve un cierre normal y
  no un corte.

### 2. Brave remoto: el motor de Brave, la ventana de JARVIS

Brave corre en el anfitrión **sin ventana** (`--headless=new`) y un puente nuevo (`xtask`, módulo
`brave.rs`) lo maneja con el **protocolo de DevTools** (CDP, el mismo que usan Puppeteer y
Playwright):

- `Page.startScreencast` manda cada cuadro de la página;
- `Input.dispatch*` mueve el mouse y escribe;
- `Page.navigate` y `Target.*` manejan las pestañas.

El puente parte cada cuadro en **mosaicos de 64×64** y manda al kernel solo los que cambiaron,
comprimidos con LZ4 (`lz4_flex` descomprime sin `std`). Así un cursor que parpadea cuesta un
mosaico, no la pantalla entera.

JARVIS dibuja las pestañas, la barra de dirección y atrás/adelante; la página es de Brave. Es el
modelo de los "navegadores remotos" (y, en el fondo, de VNC y RDP).

- **Por qué no VNC con un Brave con ventana:** en Windows, un servidor VNC comparte el escritorio
  entero, no una ventana; y para aislar a Brave en un Linux hace falta WSL o Docker, que no hay.
- **Por qué no esperar a K11:** Brave nativo es la meta de ese camino, pero faltan varios hitos.
  El protocolo de este puente no cambia cuando exista.

**Red del puente de Brave:**
- escucha en `127.0.0.1:8119`: QEMU lo ve como `10.0.2.2:8119`;
- es un protocolo binario propio, con mensajes `[largo u32][tipo u8][datos]`; no es HTTP, así que
  **no** amplía el puente HTTPS, que sigue siendo solo GET;
- con `--red` escucha en todas las interfaces, para que un JARVIS instalado en otra máquina use
  el Brave de esta PC. En ese modo exige un **token** que se configura en los dos lados, y viene
  apagado por defecto.

### 3. Sincronización: relé + cifrado de punta a punta

Dos máquinas en redes distintas, cada una detrás de su NAT (Wi-Fi de casa, datos del celular),
no pueden conectarse directo: ninguna acepta conexiones entrantes. Es el mismo problema que
resuelven Syncthing y Tailscale con sus relés. Las dos **salen** hacia un **relé** con IP pública
(`kernel/relay/`, un binario `std` chico, que también corre con `cargo xtask relay`). El relé
reenvía mensajes entre las máquinas del mismo grupo.

- **Emparejado:** un código de 20 caracteres que se genera en una máquina y se escribe en la
  otra. Con HKDF-SHA256 salen de él el **id de grupo**, que ve el relé, y la clave de
  **ChaCha20-Poly1305**, que no la ve nadie más. El relé solo ve bytes cifrados: ni nombres, ni
  contenido, ni cuántos archivos hay.
- **Por qué ChaCha20-Poly1305 y no TLS:** TLS en el kernel es K10 (certificados, X.509, varios
  cifrados). Acá las dos puntas comparten un secreto, así que alcanza con un cifrado autenticado
  (AEAD) y un contador por mensaje. Las crates de RustCrypto son `no_std` y rápidas por software.
- **Qué se sincroniza:** la carpeta `/Sincronizado`. El estado va en `/Sistema/sync.db`: por
  archivo, un hash, un reloj de Lamport, la máquina que lo cambió y el hash de la última versión
  común.
- **Conflictos:** si las dos máquinas cambiaron el mismo archivo desde la última versión común,
  gana el reloj más alto (y, si empatan, el id de máquina). La otra versión se guarda como
  `nombre (conflicto de PC2).ext`: no se pierde nada.
- **Borrados remotos:** van a `/Papelera`, igual que los locales (regla 13).

### 4. ISO

`cargo xtask iso` arma una ISO 9660 con El Torito, que apunta a una imagen FAT con el cargador
UEFI (la que ya genera `bootloader`). El escritor es propio y mínimo (unas 200 líneas), para no
depender de `xorriso`.

Hasta que haya drivers de disco reales (AHCI/NVMe, K13), en una PC real el sistema arranca en
**modo en vivo**: un FAT32 en RAM, sembrado con el rootfs.

## Consecuencias

- El puente HTTPS no cambia: sigue en 127.0.0.1 y solo GET.
- Hay dos servicios nuevos del anfitrión, cada uno con su puerto:
  - el puente de Brave (8119, solo local salvo `--red` con token);
  - el relé (8120; se puede correr en cualquier máquina con IP pública).
- La regla 15 de CLAUDE.md se amplía para nombrarlos.
- La interfaz de Brave (Shields, extensiones, sincronización de Brave) no se ve: solo la página.
  Las descargas de Brave quedan en el anfitrión.
- El Wi-Fi sigue pendiente: necesita drivers reales, firmware y WPA2. Viene después de las placas
  Ethernet de K13 (el Wi-Fi es K14), y el protocolo de sincronización no cambia cuando llegue.
- Si el relé se cae, cada máquina sigue funcionando sola. Al reconectar, se comparan los
  manifiestos y se pone al día lo que cambió mientras tanto.
