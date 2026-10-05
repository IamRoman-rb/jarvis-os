# ADR 0017: antivirus propio

- **Estado:** aceptada
- **Fecha:** 2026-10-05

## Contexto

Roman pidió agregar Bitdefender. Bitdefender es un programa comercial y cerrado para Windows,
macOS y distribuciones de Linux comunes: no corre en JARVIS-OS (un kernel propio) y no se puede
redistribuir. Tampoco corresponde ponerle ese nombre a otra cosa.

Lo que sí tiene sentido es lo que hace un antivirus: revisar lo que entra al sistema y lo que se
va a ejecutar, contra firmas de malware conocido que se actualizan seguido.

## Decisión

1. **Antivirus propio** (`desktop/src/antivirus.rs`), no un programa de terceros.
2. **Firmas**: los SHA-256 que publica MalwareBazaar (abuse.ch), de uso libre y sin cuenta. La
   lista de las últimas 48 horas pesa unos 80 KB y se baja por HTTPS con el TLS del kernel; cada
   actualización **se suma** a la base local (`/Sistema/antivirus/firmas.bin`, hasta 500 000
   firmas). La lista completa (43 MB comprimidos) es demasiado para bajarla dentro del kernel.
   Además, la firma de prueba EICAR, para probar que el antivirus anda.
3. **Protección en tiempo real**: se revisa al guardarlo lo que llega por Brave, `curl`/`wget` y
   `winget`, los `.deb` de `apt` antes de desarmarlos, y cada programa antes de ejecutarlo.
4. **Cuarentena, no borrar** (regla 13): lo infectado se mueve a `/Cuarentena` (anotando de dónde
   vino) y desde ahí no se puede ejecutar. `antivirus restaurar` lo devuelve.
5. **Dónde se maneja**: el comando `antivirus` en la Terminal y la sección Antivirus de
   Configuración (protección en tiempo real, actualizar, escanear, cuarentena).

## Consecuencias

- (+) Bloquea el malware conocido que se baja, que es la forma más común de infectarse.
- (+) Sin dependencias ni cuentas: la base se arma con datos abiertos.
- (−) Solo detecta archivos **idénticos** a muestras conocidas (por hash): no hay heurística ni
  análisis de comportamiento, como en los antivirus comerciales.
- (−) La base crece de a 48 horas por actualización: lo más viejo que se ve depende de cuándo
  se empezó a actualizar.
