# ADR 0022: la cámara (app Cámara, "bloquear al irme" y driver UVC)

- **Estado:** aceptada (el driver UVC, a medias: ver Consecuencias)
- **Fecha:** 2026-11-08
- Amplía la [0015](0015-gestos-con-la-camara.md) (gestos).

## Contexto

Roman pidió analizar qué hacer con la webcam y eligió los dos caminos: usar la cámara del
**anfitrión** (lo que anda hoy en QEMU) y un **driver propio** para cuando JARVIS-OS corra en una
PC real o en VirtualBox con la webcam conectada. La de su PC es una UVC (5843:7884).

Qué se puede hacer con una webcam, y qué conviene:

| Idea | ¿Se hace? | Por qué |
|---|---|---|
| Ver la cámara, fotos y video | Sí | Lo básico de cualquier sistema |
| Gestos de la mano | Ya está (0015) | |
| Presencia: bloquear al irse | Sí, apagado de fábrica | Útil y barato; solo viaja "hay / no hay" |
| Leer códigos QR | Más adelante | Necesita un decodificador (zbar/quirc) |
| **Desbloquear con la cara** | **No** | Con una webcam RGB común (sin infrarrojo ni profundidad) una foto alcanza para engañarla: lo advierten Howdy y Visage, los equivalentes de Linux. Nunca para `sudo` ni para el nivel 3 |

## Decisión

1. **App Cámara** (`desktop/src/apps/camera.rs`): vista en vivo con un indicador rojo "EN VIVO"
   (y "REC" al grabar), Espacio saca una foto → `/Imágenes/foto-AAAAMMDD-HHMMSS.jpg` (el JPEG tal
   cual llega, sin recomprimir) y V graba un video → `/Videos/video-….avi` (AVI con MJPEG, el
   escritor de `jarvis-audio`; tope de 1800 cuadros). La cámara está prendida **solo mientras la
   ventana está abierta**.
2. **Fuente "anfitrión"** (`src/jarvis/camera.py`): con la app abierta el kernel manda
   `camara_vista{activo}` y el cerebro lee la webcam con OpenCV, achica a 320×240, comprime en
   JPEG y manda `camara_cuadro{jpeg}` (base64) a ~10 por segundo; el kernel los decodifica con
   zune-jpeg (ADR 0009). En el anfitrión no se guarda nada. Si no hay cámara (o la tienen los
   gestos), llega `camara{estado}` y la app lo muestra. El cerebro simulado usa una foto fija
   del repo, así `cargo xtask test` prueba el camino entero.
3. **"Bloquear al irme"** (Configuración → Mouse y teclado, junto a los gestos;
   `bloquear_al_irme`, apagado de fábrica): el kernel manda `presencia_activa{activo}` (y viaja
   en `hola`); el anfitrión busca una cara cada medio segundo con el clasificador Haar que trae
   OpenCV (sin bajar modelos) y, si no hay nadie durante 60 s, avisa `presencia{hay: false}` → el escritorio se bloquea. Al kernel solo
   le llega el sí o el no. **Nunca desbloquea.**
4. **Driver UVC propio**, en dos mitades como el resto (ADR 0011):
   - `drivers/src/uvc.rs` (puro, con tests): descriptores de la interfaz de streaming (formatos
     MJPEG y YUY2, tamaños, intervalos, alternates isócronos con su ancho de banda), elegir
     tamaño, probe/commit, `SET_INTERFACE`, el armado de cuadros con los bits FID/EOF/ERR de la
     cabecera de cada paquete, YUY2 → RGB y las tablas de Huffman estándar que muchas webcams no
     mandan en sus cuadros MJPEG (probado contra la crate `image`).
   - `kernel/src/xhci.rs` ya **reconoce** una webcam al conectarla y anota sus formatos
     (`USB_WEBCAM …`). Lo que falta son las **transferencias isócronas** de xHCI (anillo de
     transferencia isócrono, TRB Isoch con su marco) y el probe/commit con datos de salida.

## Consecuencias

- (+) La Cámara anda hoy, en QEMU, con la webcam de la PC, sin tocar el kernel cuando cambie la
  fuente: la app recibe JPEG venga de donde venga.
- (+) La presencia no expone imágenes: ni al kernel ni al disco.
- (−) La fuente del anfitrión depende de OpenCV (`uv sync --extra gestures`) y no puede usarse a
  la vez que los gestos en Windows (DirectShow no comparte la cámara entre dos lectores).
- (−) **El driver UVC no recibe imagen todavía.** Las transferencias isócronas no se pueden
  probar en QEMU (no pasa webcams) ni en VirtualBox sin el Extension Pack
  (`VBoxManage controlvm JARVISOS-prueba webcam attach .1`), que la máquina de Roman todavía no
  tiene. Queda para cuando se pueda probar: es el próximo paso de este ADR.
- (−) El detector Haar se equivoca más que uno con redes (YuNet): por eso son 60 s seguidos sin
  cara, y por eso solo bloquea.
