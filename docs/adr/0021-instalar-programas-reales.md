# ADR 0021: instalar programas reales con winget y snap

- **Estado:** aceptada
- **Fecha:** 2026-11-08
- **Amplía:** ADR 0005 (paquetes), 0006 (snap/winget) y 0018 (vino)

## Contexto

`apt` ya instala paquetes de Debian de verdad (glibc incluida). Pero `winget install` y
`snap install` (de Snapcraft) solo **bajaban** el archivo: en 0005/0006 no había espacio de
usuario ni forma de correr un programa de Windows. Desde K11 (Linux en el anillo 3) y K15
(`vino`) eso cambió, y Roman pidió instalar programas de Windows y de Linux como en sus sistemas.

Referencias: winget-cli y winget-pkgs (el esquema de manifiestos: `InstallerType`,
`NestedInstallerType`, `NestedInstallerFiles`, `PortableCommandAlias`, `Commands`,
`InstallerSha256`), snapd (cómo arma `/snap/<nombre>/<revisión>` y las variables `SNAP*`),
squashfs-tools y `backhand` (el formato squashfs 4.0), Wine (`binfmt_misc` para correr `.exe`).

## Decisión

### winget

1. Se instalan los paquetes que `vino` puede correr: **`portable`** (un `.exe` suelto, como jq) y
   **`zip` con `NestedInstallerType: portable`** (como ripgrep, fd, bat). Van a
   `/Programas/windows/<Id>/` y queda un comando en `/Programas/bin` (el alias del manifiesto)
   que la terminal reconoce y corre con `vino`, como hace `binfmt_misc` con Wine.
2. **SHA-256 obligatorio**: si el manifiesto trae `InstallerSha256` y lo bajado no coincide, no
   se instala nada (como winget en Windows). Y el antivirus revisa cada archivo.
3. Los instaladores (`exe`, `msi`, `nullsoft`, `inno`, `wix`, `burn`) se siguen bajando a
   `/Descargas`: son programas con ventanas, casi siempre de 32 bits, que `vino` no corre todavía.
4. `winget install --manifest <ruta|URL>` instala desde un manifiesto propio (lo usan las pruebas
   sin internet). `list`, `uninstall` (a la Papelera) y `upgrade`.
5. ZIP propio (`desktop/src/term/zip.rs`, con el inflate de `jarvis-image`), probado contra la
   crate `zip`; rechaza nombres con `..` o absolutos y tiene topes de tamaño.
6. Lo que les faltaba a ripgrep y jq en `vino` se agregó (`vino/src/dll/compat.rs`): `setjmp`/
   `longjmp`, funciones de Bessel, `userenv`, `shlwapi`, y "no soportado" para la E/S asíncrona y
   las tuberías con nombre (que la biblioteca de Rust solo usa para lanzar procesos).

### snap

7. **squashfs propio** (`kernel/squashfs/`, `no_std`): superbloque, metadatos, inodos,
   directorios, fragmentos e ids; compresión xz, gzip y lz4 (lzo y zstd: error claro). Probado
   contra `backhand`.
8. `snap install <x>` de Snapcraft: API v2 → descarga → antivirus → squashfs →
   `/snap/<x>/<rev>/` (los enlaces simbólicos se copian, como con los `.deb`) → `meta/snap.yaml`
   → un comando por app en `/snap/bin` con `SNAP`, `SNAP_NAME`, `SNAP_REVISION`, `SNAP_DATA`,
   `SNAP_USER_DATA`.
9. **La base** (core22, core24: un Ubuntu mínimo con glibc) no se baja (pesa más que el tope de
   descarga del kernel): la cubre `libc6` de Debian, que `apt` instala. La glibc de Debian estable
   es más nueva que la de core22, así que los programas la aceptan.
10. **Sin confinamiento**: snapd encierra cada snap con AppArmor, seccomp y espacios de nombres;
    JARVIS-OS no tiene nada de eso todavía. Por eso los permisos de ADR 0020 (el snap corre como
    el usuario) y el antivirus son la defensa; `snap info` lo dice. Los snaps `classic` piden
    `--classic`, como snapd.

## Consecuencias

- (+) Programas reales de Windows (ripgrep, jq) y de Linux (snaps chicos) se instalan y corren.
- (+) Lo bajado se verifica (SHA-256 de winget, antivirus para todo).
- (−) Ni los instaladores gráficos de Windows ni los programas con ventanas: hace falta un
  servidor gráfico (K11) y `user32`/`gdi32` en `vino`.
- (−) Los snaps grandes (más de 32 MB) y los que usan lzo o zstd no se instalan todavía.
- (−) Sin confinamiento, un snap puede leer lo mismo que el usuario.
