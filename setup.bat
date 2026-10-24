@echo off
setlocal
cd /d "%~dp0"

echo =============================================
echo  FotoMaxxer -- forste gangs oppsett
echo =============================================
echo.

set FM_PYTHON=%~dp0python\python.exe
set FM_DATA=%USERPROFILE%\Documents\FotoMaxxer

if not exist "%FM_PYTHON%" (
    echo FEIL: python\python.exe ikke funnet.
    echo Pakk ut zip-filen pa nytt og prov igjen.
    pause
    exit /b 1
)

echo Oppretter datamappe: %FM_DATA%
mkdir "%FM_DATA%" 2>nul

echo Initialiserer database...
set PATH=%~dp0;%PATH%
"%FM_PYTHON%" -m fotomaxxer --home "%FM_DATA%" init
if errorlevel 1 (
    echo.
    echo FEIL: Klarte ikke initialisere databasen. Se feilmelding over.
    pause
    exit /b 1
)

echo.
echo =============================================
echo  Oppsett ferdig!
echo  Start FotoMaxxer med: start-fotomaxxer.bat
echo =============================================
pause
