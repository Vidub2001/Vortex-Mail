# Construit la version portable Windows de Vortex Mail et la livre en ZIP.
# Usage (PowerShell, à la racine du projet) :  .\scripts\portable.ps1
# Résultat : target\VortexMail-portable-<version>.zip, sans aucune donnée personnelle.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)

$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$stage = "target\VortexMail-portable"
$zip = "target\VortexMail-portable-$version.zip"

npm run tauri build -- --no-bundle
if ($LASTEXITCODE -ne 0) { throw "Compilation impossible." }

# Dossier propre : l'exécutable et le fichier qui active le mode portable, pas de dossier data.
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\vortexmail.exe $stage\
Set-Content -Encoding UTF8 $stage\portable.txt @"
Vortex Mail en mode portable : les données (comptes, courrier, réglages) sont dans le dossier data,
à côté de vortexmail.exe. Supprimer ce fichier fait fonctionner Vortex Mail comme une installation normale.
"@

Compress-Archive -Force -Path $stage -DestinationPath $zip
Write-Host "Version portable prête : $zip"
