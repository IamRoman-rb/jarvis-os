// Reglas del antivirus de JARVIS-OS (ADR 0024), escritas para este proyecto. Sintaxis de YARA
// (un subconjunto: ver kernel/reglas). `severity = "alta"`: a cuarentena; "media": se avisa.
// Se actualizan con `antivirus actualizar` (el puente las sirve en
// http://paquetes.jarvis/antivirus/reglas.yar); también vienen adentro del sistema.

rule Minero_de_criptomonedas : minero {
    meta:
        description = "Minero de criptomonedas (XMRig y parecidos)"
        severity = "alta"
    strings:
        $pool1 = "stratum+tcp://" nocase
        $pool2 = "stratum+ssl://" nocase
        $x1 = "xmrig" nocase wide ascii
        $x2 = "cryptonight" nocase
        $x3 = "--donate-level" nocase
        $x4 = "randomx" nocase
    condition:
        any of ($pool*) and any of ($x*)
}

rule Volcador_de_credenciales : credenciales {
    meta:
        description = "Herramienta que roba contraseñas de la memoria (Mimikatz)"
        severity = "alta"
    strings:
        $a = "sekurlsa::logonpasswords" nocase wide ascii
        $b = "mimikatz" nocase wide ascii
        $c = "lsadump::sam" nocase wide ascii
        $d = "gentilkiwi" nocase wide ascii
    condition:
        2 of them
}

rule Webshell_PHP : webshell {
    meta:
        description = "Webshell de PHP (ejecuta lo que le mandan por la web)"
        severity = "alta"
    strings:
        $php = "<?php" nocase
        $e1 = "eval($_POST" nocase
        $e2 = "eval($_REQUEST" nocase
        $e3 = "system($_GET" nocase
        $e4 = "eval(base64_decode($_" nocase
        $e5 = "assert($_POST" nocase
    condition:
        $php and any of ($e*)
}

rule Nota_de_rescate : ransomware {
    meta:
        description = "Nota de rescate de un ransomware"
        severity = "alta"
    strings:
        $r1 = "your files have been encrypted" nocase wide ascii
        $r2 = "tus archivos han sido cifrados" nocase wide ascii
        $r3 = "to decrypt your files" nocase wide ascii
        $r4 = "para recuperar tus archivos" nocase wide ascii
        $pay1 = "bitcoin" nocase wide ascii
        $pay2 = ".onion" nocase wide ascii
    condition:
        any of ($r*) and all of ($pay*) and filesize < 200KB
}

rule PowerShell_que_baja_y_ejecuta : descargador {
    meta:
        description = "PowerShell que baja código de internet y lo ejecuta"
        severity = "alta"
    strings:
        $iex = "IEX" nocase wide ascii
        $iex2 = "Invoke-Expression" nocase wide ascii
        $dl1 = "DownloadString" nocase wide ascii
        $dl2 = "Net.WebClient" nocase wide ascii
        $hidden = "-WindowStyle Hidden" nocase wide ascii
    condition:
        any of ($iex*) and any of ($dl*) or ($hidden and $dl1)
}

rule Script_que_baja_y_ejecuta : descargador {
    meta:
        description = "Script que baja algo de internet y lo ejecuta (curl | sh)"
        severity = "media"
    strings:
        $c1 = "curl " nocase
        $c2 = "wget " nocase
        $p1 = "| sh" nocase
        $p2 = "| bash" nocase
        $p3 = "|sh" nocase
        $p4 = "|bash" nocase
        $tmp = "/tmp/" nocase
        $x = "chmod +x" nocase
    condition:
        any of ($c*) and (any of ($p*) or ($tmp and $x)) and filesize < 100KB
}

rule Rootkit_ld_preload : rootkit {
    meta:
        description = "Programa de Linux que se instala en /etc/ld.so.preload (rootkit)"
        severity = "media"
    strings:
        $a = "/etc/ld.so.preload"
    condition:
        uint32be(0) == 0x7F454C46 and $a
}
