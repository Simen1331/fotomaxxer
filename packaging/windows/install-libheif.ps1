# Installer libheif statisk (x64-windows-static) via vcpkg, bare med HEVC-dekoding
# (libde265). Standardfunksjonen «hevc» drar inn x265-koderen, som er GPL og ikke
# trengs: FotoMaxxer leser HEIC, men skriver det aldri.
#
# Runner-avbildningene har ulike vcpkg-versjoner. I eldre utgaver av porten er libde265
# en fast avhengighet; fra 1.23.6 er den en egen funksjon (standard sammen med hevc).
# Setter VCPKG_ROOT og VCPKGRS_TRIPLET i $GITHUB_ENV når den kjører i Actions.

$ErrorActionPreference = 'Stop'
$root = $env:VCPKG_INSTALLATION_ROOT
$port = Get-Content (Join-Path $root 'ports\libheif\vcpkg.json') -Raw | ConvertFrom-Json
$features = if ($port.features.PSObject.Properties.Name -contains 'libde265') { 'core,libde265' } else { 'core' }
$spec = "libheif[$features]:x64-windows-static"
Write-Host "vcpkg-port libheif $($port.version): installerer $spec"
& (Join-Path $root 'vcpkg.exe') install $spec
if ($LASTEXITCODE -ne 0) { throw "vcpkg install feilet ($LASTEXITCODE)" }
if ($env:GITHUB_ENV) {
    "VCPKG_ROOT=$root" | Out-File -FilePath $env:GITHUB_ENV -Append
    'VCPKGRS_TRIPLET=x64-windows-static' | Out-File -FilePath $env:GITHUB_ENV -Append
}
