# Bygg ONNX Runtime som ett statisk bibliotek (onnxruntime.lib) med statisk C-runtime
# (/MT) og bare CPU. Brukes av CI (se .github/actions/onnxruntime-static) og kan kjøres
# lokalt i en «Developer PowerShell for VS 2022» med CMake, Git og Python på PATH.
#
# Hvorfor: de prebygde bibliotekene fra pyke (som `ort` laster ned) er bygd med /MD og
# alltid med DirectML. Med `+crt-static` (.cargo/config.toml) feiler lenkingen mot dem
# (RuntimeLibrary MD_DynamicRelease mot MT_StaticRelease), og exe-en ville ellers trengt
# VCRUNTIME140.dll/MSVCP140.dll og DirectML.dll. Oppskriften følger pykes egen
# (github.com/pykeio/ort-artifacts, src/build.ts med --static) med tre forskjeller:
# MultiThreaded i stedet for MultiThreadedDLL, ingen DirectML, og ingen /arch:AVX2
# (MLAS velger AVX2-kjerner ved kjøring, så eldre/svake CPU-er uten AVX2 virker også).
#
# Bruk:  .\build-onnxruntime.ps1 -OutDir C:\ort-static
# Resultat: <OutDir>\lib\onnxruntime.lib (+ LICENSE og ThirdPartyNotices.txt i <OutDir>).
# Sett så ORT_LIB_PATH=<OutDir>\lib før `cargo build`.

param(
    [Parameter(Mandatory = $true)][string]$OutDir,
    [string]$Version = '1.31.0',
    # pykes ort-artifacts: CMake-oppsettet som samler alt i én .lib, og patchene deres.
    [string]$PykeCommit = 'a756c60595a74b4dc286b89bcdf81a76f43df8fd',
    [string]$WorkDir = (Join-Path ([IO.Path]::GetTempPath()) 'ort-static-build')
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

New-Item -ItemType Directory -Force $WorkDir | Out-Null
$src = Join-Path $WorkDir 'onnxruntime'
$pyke = Join-Path $WorkDir 'ort-artifacts'
$build = Join-Path $WorkDir 'build'

if (-not (Test-Path $src)) {
    git -c advice.detachedHead=false clone --depth 1 --branch "v$Version" https://github.com/microsoft/onnxruntime $src
}
if (-not (Test-Path $pyke)) {
    git init -q $pyke
    git -C $pyke fetch -q --depth 1 https://github.com/pykeio/ort-artifacts $PykeCommit
    git -C $pyke -c advice.detachedHead=false checkout -q FETCH_HEAD
}

git -C $src reset -q --hard HEAD
git -C $src clean -q -fdx
foreach ($patch in Get-ChildItem (Join-Path $pyke 'src\patches\all') -Filter *.patch | Sort-Object Name) {
    git -C $src apply --ignore-whitespace --recount $patch.FullName
    Write-Host "patch: $($patch.Name)"
}

# GNU patch fra Git for Windows. CMake finner ellers Strawberry Perls eldre patch.exe
# først på runneren, og den feiler på ORTs onnx.patch (som trenger --ignore-whitespace).
$gitRoot = Split-Path (Split-Path (Get-Command git).Source)
$patchExe = Join-Path $gitRoot 'usr\bin\patch.exe'
if (-not (Test-Path $patchExe)) { throw "fant ikke GNU patch i $patchExe" }

$flags = '/MP -D_CRT_SECURE_NO_WARNINGS -D_DISABLE_CONSTEXPR_MUTEX_CONSTRUCTOR'
$cmakeArgs = @(
    '-S', (Join-Path $pyke 'src\static-build'),
    '-B', $build,
    '-G', 'Visual Studio 17 2022', '-A', 'x64',
    '-DCMAKE_BUILD_TYPE=Release',
    '-DCMAKE_CONFIGURATION_TYPES=Release',
    "-DCMAKE_INSTALL_PREFIX=$OutDir",
    "-DONNXRUNTIME_SOURCE_DIR=$src",
    '--compile-no-warning-as-error',
    "-DPatch_EXECUTABLE=$patchExe",
    # Statisk C-runtime overalt, også i protobuf, onnx og abseil (som build.py --enable_msvc_static_runtime).
    '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
    '-DONNX_USE_MSVC_STATIC_RUNTIME=ON',
    '-Dprotobuf_MSVC_STATIC_RUNTIME=ON',
    '-DABSL_MSVC_STATIC_RUNTIME=ON',
    '-Dgtest_force_shared_crt=OFF',
    '-Donnxruntime_BUILD_UNIT_TESTS=OFF',
    '-Donnxruntime_USE_KLEIDIAI=OFF',
    '-Donnxruntime_CLIENT_PACKAGE_BUILD=ON',
    "-DCMAKE_C_FLAGS=$flags",
    "-DCMAKE_CXX_FLAGS=$flags"
)
cmake @cmakeArgs
cmake --build $build --config Release --parallel ([Environment]::ProcessorCount)
cmake --install $build --config Release

Copy-Item (Join-Path $src 'LICENSE') (Join-Path $OutDir 'LICENSE') -Force
Copy-Item (Join-Path $src 'ThirdPartyNotices.txt') (Join-Path $OutDir 'ThirdPartyNotices.txt') -Force

$lib = Join-Path $OutDir 'lib\onnxruntime.lib'
if (-not (Test-Path $lib)) { throw "fant ikke $lib etter bygget" }
Get-Item $lib | Format-Table Name, Length
