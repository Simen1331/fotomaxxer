@echo off
cd /d "%~dp0"
set PATH=%~dp0;%PATH%
set FOTOMAXXER_HOME=%USERPROFILE%\Documents\FotoMaxxer

echo Starter FotoMaxxer...
echo (Minimer dette vinduet -- det ma sta apent mens appen er i bruk)
echo.

start "" http://127.0.0.1:8000
"%~dp0python\python.exe" -m fotomaxxer serve
