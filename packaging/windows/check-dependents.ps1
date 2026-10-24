# Sjekk at exe-ene bare avhenger av DLL-er som følger med Windows (fase 7).
#
# Kjører `dumpbin /DEPENDENTS` på hver fil og feiler hvis en vanlig eller forsinket
# (delay-load) avhengighet ikke står på lista under. Særlig skal VCRUNTIME140.dll,
# MSVCP140.dll, ucrtbase/api-ms-win-crt-* (dynamisk C-runtime), onnxruntime.dll og
# DirectML.dll ikke dukke opp.
#
# Bruk:  .\check-dependents.ps1 target\release\fotomaxxer.exe target\release\fotomaxxer-gui.exe

param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Files)

$ErrorActionPreference = 'Stop'

# Systemets DLL-er (små bokstaver, uten .dll). api-ms-win-core-* o.l. er API-sett som
# Windows løser selv; api-ms-win-crt-* er unntatt, for de betyr dynamisk UCRT.
$allowed = @(
    'kernel32', 'kernelbase', 'ntdll', 'user32', 'gdi32', 'advapi32', 'shell32', 'shlwapi',
    'ole32', 'oleaut32', 'combase', 'rpcrt4', 'ws2_32', 'bcrypt', 'bcryptprimitives',
    'crypt32', 'secur32', 'ncrypt', 'userenv', 'dbghelp', 'powrprof', 'propsys',
    'dwmapi', 'uxtheme', 'comctl32', 'comdlg32', 'imm32', 'winmm', 'version', 'setupapi',
    'cfgmgr32', 'iphlpapi', 'psapi', 'shcore', 'dcomp', 'windowscodecs', 'msimg32',
    'uiautomationcore', 'wintrust', 'normaliz', 'mswsock', 'dnsapi', 'netapi32',
    'opengl32', 'dxgi', 'd3d11', 'd3d12', 'dxcore', 'd2d1', 'dwrite', 'winhttp', 'wininet',
    'oleacc', 'urlmon', 'credui', 'pdh', 'wtsapi32',
    # WebView2-lasteren lenkes som regel statisk av webview2-com; tillatt om den ikke gjør det.
    'webview2loader'
)

$dumpbin = Get-ChildItem "${env:ProgramFiles}\Microsoft Visual Studio" -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match 'Hostx64\\x64' } | Select-Object -First 1
if (-not $dumpbin) { throw 'fant ikke dumpbin.exe' }

$bad = @()
foreach ($file in $Files) {
    if (-not (Test-Path $file)) { throw "finnes ikke: $file" }
    $out = & $dumpbin.FullName /NOLOGO /DEPENDENTS $file
    if ($LASTEXITCODE -ne 0) { throw "dumpbin feilet for $file" }
    $dlls = $out | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '\.dll$' }
    $size = '{0:N1} MB' -f ((Get-Item $file).Length / 1MB)
    Write-Host "== $file ($size)"
    foreach ($dll in $dlls) {
        $name = [IO.Path]::GetFileNameWithoutExtension($dll).ToLowerInvariant()
        $ok = ($allowed -contains $name) -or ($name -like 'api-ms-win-*' -and $name -notlike 'api-ms-win-crt-*')
        Write-Host ("   {0} {1}" -f ($(if ($ok) { 'ok  ' } else { 'FEIL' })), $dll)
        if (-not $ok) { $bad += "$file -> $dll" }
    }
}
if ($bad.Count -gt 0) {
    Write-Host ''
    Write-Host 'Avhengigheter som ikke følger med Windows:'
    $bad | ForEach-Object { Write-Host "  $_" }
    exit 1
}
Write-Host 'Alle avhengigheter er systemets DLL-er.'
