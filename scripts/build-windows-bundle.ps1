#Requires -Version 5.1
<#
.SYNOPSIS
    Bygger FotoMaxxer Windows Portable Bundle.

.DESCRIPTION
    Lager en selvinneholdt Windows-pakke med Python 3.12 embeddable, alle
    Python-avhengigheter, frontend-bygget, exiftool.exe og oppstartsskript.
    Krever at web/dist/ er bygget foer scriptet kjoeres.

.PARAMETER ExiftoolPath
    Sti til exiftool.exe (standalone, omdoopt fra exiftool(-k).exe).
    Standard: exiftool.exe i gjeldende mappe eller PATH.

.EXAMPLE
    .\scripts\build-windows-bundle.ps1 -ExiftoolPath C:\tools\exiftool.exe
#>
param(
    [string]$ExiftoolPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path $PSScriptRoot -Parent
$Date = Get-Date -Format "yyyy-MM-dd"
$BundleDir = Join-Path $RepoRoot "FotoMaxxer"
$ZipName = "FotoMaxxer-$Date.zip"
$ZipPath = Join-Path $RepoRoot $ZipName

$PythonVersion = "3.12.10"
$PythonUrl = "https://www.python.org/ftp/python/$PythonVersion/python-$PythonVersion-embed-amd64.zip"
$GetPipUrl = "https://bootstrap.pypa.io/get-pip.py"

function Write-Step([string]$msg) {
    Write-Host "`n==> $msg" -ForegroundColor Cyan
}

function Assert-Exists([string]$path, [string]$label) {
    if (-not (Test-Path $path)) {
        Write-Error "$label ikke funnet: $path"
        exit 1
    }
}

# --- Forhandssjekk ---------------------------------------------------------

Write-Step "Sjekker forutsetninger"

$WebDist = Join-Path $RepoRoot "web\dist"
if (-not (Test-Path $WebDist)) {
    Write-Error @"
web\dist\ finnes ikke. Bygg frontend foerst:
  cd web
  npm ci
  npm run build
"@
    exit 1
}

# Finn exiftool.exe
if ($ExiftoolPath -eq "") {
    $found = Get-Command exiftool.exe -ErrorAction SilentlyContinue
    if ($found) {
        $ExiftoolPath = $found.Source
    }
}
if ($ExiftoolPath -eq "" -or -not (Test-Path $ExiftoolPath)) {
    Write-Error @"
exiftool.exe ikke funnet. Last ned fra https://exiftool.org (Windows zip),
doep om exiftool(-k).exe til exiftool.exe, og angi stien:
  .\scripts\build-windows-bundle.ps1 -ExiftoolPath C:\sti\til\exiftool.exe
"@
    exit 1
}

# --- Rydd opp gammel bundle -------------------------------------------------

Write-Step "Rydder opp eventuell gammel bundle"
if (Test-Path $BundleDir) { Remove-Item $BundleDir -Recurse -Force }
if (Test-Path $ZipPath)   { Remove-Item $ZipPath -Force }

New-Item -ItemType Directory -Path $BundleDir | Out-Null
$PythonDir = Join-Path $BundleDir "python"
New-Item -ItemType Directory -Path $PythonDir | Out-Null

# --- Last ned og pakk ut Python embeddable ----------------------------------

Write-Step "Laster ned Python $PythonVersion embeddable"
$EmbedZip = Join-Path $env:TEMP "python-embed-amd64.zip"
Invoke-WebRequest $PythonUrl -OutFile $EmbedZip -UseBasicParsing
Expand-Archive $EmbedZip -DestinationPath $PythonDir -Force
Remove-Item $EmbedZip

# Aktiver pip og site-packages i embedded Python
$PthFile = Join-Path $PythonDir "python312._pth"
$PthContent = @"
python312.zip
.
Lib\site-packages
..
import site
"@
Set-Content $PthFile $PthContent -Encoding UTF8

# --- Installer pip ----------------------------------------------------------

Write-Step "Installerer pip"
$GetPip = Join-Path $env:TEMP "get-pip.py"
Invoke-WebRequest $GetPipUrl -OutFile $GetPip -UseBasicParsing
$PythonExe = Join-Path $PythonDir "python.exe"
& $PythonExe $GetPip --no-warn-script-location
Remove-Item $GetPip

# --- Installer Python-avhengigheter -----------------------------------------

Write-Step "Installerer Python-avhengigheter (kan ta noen minutter)"
$SitePackages = Join-Path $PythonDir "Lib\site-packages"
New-Item -ItemType Directory -Path $SitePackages -Force | Out-Null

& $PythonExe -m pip install `
    Pillow pillow-heif pyexiftool requests `
    insightface onnxruntime numpy "scikit-learn>=1.3" `
    fastapi "uvicorn[standard]" python-multipart `
    --target $SitePackages `
    --no-warn-script-location `
    --quiet

# --- Kopier kildekode -------------------------------------------------------

Write-Step "Kopierer fotomaxxer-pakken"
Copy-Item -Recurse (Join-Path $RepoRoot "fotomaxxer") (Join-Path $SitePackages "fotomaxxer")
Copy-Item -Recurse (Join-Path $RepoRoot "api")        (Join-Path $SitePackages "api")

# --- Kopier frontend-bygg ---------------------------------------------------

Write-Step "Kopierer frontend (web/dist)"
$WebDir = Join-Path $BundleDir "web"
New-Item -ItemType Directory -Path $WebDir | Out-Null
Copy-Item -Recurse $WebDist (Join-Path $WebDir "dist")

# --- Kopier exiftool --------------------------------------------------------

Write-Step "Kopierer exiftool.exe"
Copy-Item $ExiftoolPath (Join-Path $BundleDir "exiftool.exe")

# --- Kopier oppstartsfiler --------------------------------------------------

Write-Step "Kopierer oppstartsskript og README"
Copy-Item (Join-Path $RepoRoot "setup.bat")              (Join-Path $BundleDir "setup.bat")
Copy-Item (Join-Path $RepoRoot "start-fotomaxxer.bat")   (Join-Path $BundleDir "start-fotomaxxer.bat")
Copy-Item (Join-Path $RepoRoot "README.txt")             (Join-Path $BundleDir "README.txt")

# --- Pakk bundle ------------------------------------------------------------

Write-Step "Pakker bundle: $ZipName"
Compress-Archive -Path $BundleDir -DestinationPath $ZipPath

$SizeMB = [math]::Round((Get-Item $ZipPath).Length / 1MB, 0)
Write-Host "`n*** Ferdig! ***" -ForegroundColor Green
Write-Host "Zip: $ZipPath ($SizeMB MB)"
Write-Host "Send $ZipName til Bjorn."
