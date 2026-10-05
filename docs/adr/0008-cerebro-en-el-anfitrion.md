# ADR 0008: El cerebro de JARVIS en el anfitrión, por una conexión con token

- **Estado:** aceptada
- **Fecha:** 2026-09-27

## Contexto

K7 conecta el kernel con el cerebro: Claude, por el Agent SDK de Python (ADR 0003, punto 5;
`docs/investigacion.md` §12). El kernel no tiene TLS (llega en K10), así que no puede hablarle a la
API de Anthropic. Además, el SDK usa el login de Claude Code de la PC de Roman: el cerebro tiene
que correr en el anfitrión.

Roman quiere hablarle a JARVIS en lenguaje natural para que responda, actúe sobre JARVIS-OS
("abrí el navegador y buscá X") y retome proyectos ("abrí jarvis-os y seguí"), también con la voz.

## Decisión

1. **`jarvis serve`** (Python, `src/jarvis/service/server.py`) escucha **solo en 127.0.0.1**.
   QEMU lo ve como 10.0.2.2. Lo levanta `cargo xtask run` y se cierra con él.
2. **Un token por sesión.** `xtask` genera 32 caracteres al azar y se los pasa:
   - al servicio, por una variable de entorno (la línea de comandos la ven los demás procesos);
   - al kernel, por **fw_cfg** (`opt/jarvis/cerebro` = "puerto token"), el mismo canal de la
     resolución de pantalla.

   La primera línea de cada conexión es `hola{token}`. Si no coincide (comparación en tiempo
   constante), se corta. Así otro programa del anfitrión que se conecte al puerto no puede usar el
   login de Claude de Roman.
3. **Protocolo: un objeto JSON por renglón** sobre una conexión larga (las de ADR 0007). El
   kernel ya tiene un parser de JSON (`desktop/src/web/json.rs`). Cada mensaje tiene `t`:
   - kernel → cerebro: `hola`, `pedido{id, texto, origen}`, `cancelar{id}`,
     `resultado{llamada, ok, datos}`, `confirmacion{llamada, ok}`;
   - cerebro → kernel: `listo`, `texto{id, delta}` (la respuesta de a pedazos), `fin{id}`,
     `error{id, msg}`, y en las etapas siguientes `accion`, `confirmar`, `oido`, `voz`,
     `proyecto`.
4. **La consola decide primero.** Las órdenes locales de siempre ("abrir monitor", "ls") se
   resuelven en el kernel, sin red. Lo demás va al cerebro. Sin cerebro, la consola lo dice.
5. **Las acciones las ejecuta el kernel.** Las tools del cerebro (etapa 7.2) no tocan nada: mandan
   `accion` y esperan el `resultado`. Las que cambian algo piden confirmación **en la pantalla de
   JARVIS**, con los 3 niveles de `docs/permisos.md`. Nunca `bypassPermissions` ni `acceptEdits`.
6. **La voz, en el anfitrión** (etapa 7.4): micrófono y parlantes de la PC, con wake word, STT y
   TTS locales. El texto va al kernel como si se hubiera escrito. El audio dentro del kernel sigue
   en K12.

## Consecuencias

- (+) Claude con el login de Roman, sin claves en el repo ni en el disco virtual.
- (+) El kernel no aprende de Claude ni del SDK: solo de un protocolo de texto chico.
- (+) Los tests no gastan: `jarvis serve --simulado` responde con un guion (`cargo xtask test`).
- (−) El cerebro necesita el anfitrión: en una PC real sin él, JARVIS responde solo con las
  órdenes locales. Hablarle a la API directo desde el kernel depende de TLS (K10).
- (−) La puerta es local: si el anfitrión está comprometido, el token no protege. Es el mismo
  límite que el de Claude Code en esa PC.

## Actualización (2026-10-04): varios agentes como cerebro

Además de Claude, Roman puede vincular **Gemini, ChatGPT y DeepSeek** desde Configuración →
Asistente, de dos maneras:

- **Con Google**: el anfitrión lanza el programa oficial del proveedor, que abre el navegador
  (Gemini CLI con la cuenta de Google; Codex CLI con la de ChatGPT, que ofrece "Continuar con
  Google"). DeepSeek no tiene un programa así: se abre su página de claves para entrar con Google
  y crear una. Las credenciales las guarda el programa del proveedor; JARVIS no las ve.
- **Con el formulario**: una clave de API. Viaja por la conexión con token al anfitrión, que la
  valida contra el proveedor y la guarda en `agentes.json` (carpeta de configuración, modo 600).
  En el disco de JARVIS-OS no queda; al kernel solo le vuelven los últimos 4 caracteres.

Todos juntos forman el cerebro (`agent/council.py`): Roman elige el **agente principal** (el que
contesta y actúa, con las mismas tools y los mismos niveles de `docs/permisos.md`) y si quiere el
**consejo** (los demás vinculados opinan en paralelo y el principal decide, tratando las opiniones
como información, no como órdenes). Sin consejo, el principal puede pedir una segunda opinión con
la tool `consultar_agente` (nivel 1). Si el principal falla, contesta el siguiente que funcione.
Con Google (sin clave) un agente solo contesta texto: el programa del proveedor no expone tools.
