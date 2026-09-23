<#
.SYNOPSIS
    Crea la VM de desarrollo de JARVIS-OS en VirtualBox (Debian 13 + XFCE).

.DESCRIPTION
    Idempotente: si la VM ya existe, no la toca. No instala el sistema: deja el ISO en la
    lectora y (con -Start) arranca el instalador para que lo sigas con docs/entorno-vm.md.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\dev-vm\crear-vm.ps1 -Start
#>
param(
    [string]$Name = "JARVIS-OS-dev",
    [string]$IsoPath = "$env:USERPROFILE\Downloads\debian-13.7.0-amd64-netinst.iso",
    [int]$MemoryMB = 8192,
    [int]$Cpus = 4,
    [int]$DiskGB = 60,
    [switch]$Start
)

$ErrorActionPreference = "Stop"
$VBox = "C:\Program Files\Oracle\VirtualBox\VBoxManage.exe"

function Invoke-VBox {
    & $VBox @args
    if ($LASTEXITCODE -ne 0) { throw "VBoxManage $($args -join ' ') falló (código $LASTEXITCODE)." }
}

if (-not (Test-Path $VBox)) { throw "No encontré VirtualBox en $VBox." }
if (-not (Test-Path $IsoPath)) { throw "No encontré el ISO en $IsoPath. Ver docs/entorno-vm.md, paso 1." }

$existing = & $VBox list vms
if ($existing -match "^`"$([regex]::Escape($Name))`" ") {
    Write-Host "La VM '$Name' ya existe: no la modifico."
} else {
    $machineFolder = ((& $VBox list systemproperties) -match "^Default machine folder:") -replace "^Default machine folder:\s*", ""
    $disk = Join-Path $machineFolder "$Name\$Name.vdi"

    Write-Host "Creando '$Name' ($MemoryMB MB, $Cpus CPUs, disco de $DiskGB GB)..."
    Invoke-VBox createvm --name $Name --ostype Debian13_64 --register
    Invoke-VBox modifyvm $Name `
        --firmware efi --memory $MemoryMB --cpus $Cpus --ioapic on --rtc-use-utc on `
        --graphicscontroller vmsvga --vram 128 `
        --nic1 nat `
        --audio-enabled on --audio-driver default --audio-controller hda --audio-in on --audio-out on `
        --clipboard-mode bidirectional --drag-and-drop bidirectional `
        --boot1 disk --boot2 dvd --boot3 none --boot4 none  # disco vacío → arranca del ISO
    Invoke-VBox createmedium disk --filename $disk --size ($DiskGB * 1024) --format VDI --variant Standard
    Invoke-VBox storagectl $Name --name SATA --add sata --controller IntelAhci --portcount 2
    Invoke-VBox storageattach $Name --storagectl SATA --port 0 --device 0 --type hdd --medium $disk
    Invoke-VBox storageattach $Name --storagectl SATA --port 1 --device 0 --type dvddrive --medium $IsoPath
    Write-Host "Listo. Disco: $disk"
}

if ($Start) {
    Invoke-VBox startvm $Name --type gui
    Write-Host "Se abrió la ventana del instalador. Seguí docs/entorno-vm.md, paso 2."
}
