; FotoMaxxer – installasjonsprogram for Windows (NSIS 3, fase 7).
;
; Installerer per bruker (ingen administrator) til %LOCALAPPDATA%\Programs\FotoMaxxer
; med snarvei i Start-menyen og avinstallering under Innstillinger → Apper.
;
; Avinstalleringen fjerner BARE filene dette programmet la inn, snarveien og
; registeroppføringen. Datamappa (%LOCALAPPDATA%\FotoMaxxer), pekerfila
; (%USERPROFILE%\.config\fotomaxxer\config.json) og alt annet brukeren har laget
; blir liggende. Det brukes ingen RMDir /r: hver fil slettes for seg, og mappene
; fjernes bare hvis de er tomme.
;
; Bygges av .github/workflows/windows-release.yml:
;   makensis /DVERSION=0.1.0 /DNUMVERSION=0.1.0.0 /DSRCDIR=<pakkemappe> /DOUTFILE=<Setup.exe> fotomaxxer.nsi
; <pakkemappe> skal inneholde FotoMaxxer.exe, FotoMaxxer-cli.exe, fotomaxxer.ico,
; README.txt, models\det_10g.onnx, models\w600k_r50.onnx og licenses\*.txt.
;
; Stille modus: «FotoMaxxer-Setup-<versjon>.exe /S» og «uninstall.exe /S».

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !error "VERSION mangler (/DVERSION=…)"
!endif
!ifndef NUMVERSION
  !define NUMVERSION "0.0.0.0"
!endif
!ifndef SRCDIR
  !error "SRCDIR mangler (/DSRCDIR=…)"
!endif
!ifndef OUTFILE
  !define OUTFILE "FotoMaxxer-Setup-${VERSION}.exe"
!endif

!define APPNAME "FotoMaxxer"
!define PUBLISHER "FotoMaxxer"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\FotoMaxxer"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

Name "${APPNAME}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\FotoMaxxer"
InstallDirRegKey HKCU "${UNINSTKEY}" "InstallLocation"
BrandingText "${APPNAME} ${VERSION}"

VIProductVersion "${NUMVERSION}"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "FileDescription" "Installasjonsprogram for ${APPNAME}"
VIAddVersionKey "LegalCopyright" "${PUBLISHER}"

!define MUI_ICON "fotomaxxer.ico"
!define MUI_UNICON "fotomaxxer.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\FotoMaxxer.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start FotoMaxxer"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Norwegian"

; Avvis hvis FotoMaxxer kjører fra denne mappa: en kjørende exe kan ikke åpnes for
; skriving. Interaktivt kan brukeren lukke programmet og prøve igjen; i stille modus
; avbrytes det med feilkode 2.
!macro SJEKK_KJORER UN
Function ${UN}SjekkKjorer
  StrCpy $1 "$INSTDIR\FotoMaxxer.exe"
  Call ${UN}SjekkFil
  StrCpy $1 "$INSTDIR\FotoMaxxer-cli.exe"
  Call ${UN}SjekkFil
FunctionEnd

Function ${UN}SjekkFil
  igjen:
  ${IfNot} ${FileExists} "$1"
    Return
  ${EndIf}
  ClearErrors
  FileOpen $0 "$1" a
  ${IfNot} ${Errors}
    FileClose $0
    Return
  ${EndIf}
  IfSilent 0 +3
    SetErrorLevel 2
    Quit
  MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "FotoMaxxer kjører. Lukk programmet (og eventuelle FotoMaxxer-vinduer i terminalen) og trykk Prøv igjen." /SD IDCANCEL IDRETRY igjen
  SetErrorLevel 2
  Quit
FunctionEnd
!macroend
!insertmacro SJEKK_KJORER ""
!insertmacro SJEKK_KJORER "un."

Function .onInit
  ; Programmappa må aldri være datamappa (avinstalleringen skal aldri røre den).
  ${If} $INSTDIR == "$LOCALAPPDATA\FotoMaxxer"
  ${OrIf} ${FileExists} "$INSTDIR\fotomaxxer.db"
    MessageBox MB_OK|MB_ICONSTOP "Kan ikke installere i $INSTDIR: mappa er en FotoMaxxer-datamappe." /SD IDOK
    SetErrorLevel 2
    Quit
  ${EndIf}
  Call SjekkKjorer
FunctionEnd

Section "FotoMaxxer" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\FotoMaxxer.exe"
  File "${SRCDIR}\FotoMaxxer-cli.exe"
  File "${SRCDIR}\fotomaxxer.ico"
  File "${SRCDIR}\README.txt"
  SetOutPath "$INSTDIR\models"
  File "${SRCDIR}\models\det_10g.onnx"
  File "${SRCDIR}\models\w600k_r50.onnx"
  SetOutPath "$INSTDIR\licenses"
  File "${SRCDIR}\licenses\*.txt"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\FotoMaxxer.lnk" "$INSTDIR\FotoMaxxer.exe" "" "$INSTDIR\fotomaxxer.ico" 0

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\fotomaxxer.ico"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" "$0"
SectionEnd

Function un.onInit
  Call un.SjekkKjorer
FunctionEnd

Section "Uninstall"
  ; Bare det installasjonsprogrammet la inn – aldri datamappa eller brukerdata.
  Delete "$INSTDIR\FotoMaxxer.exe"
  Delete "$INSTDIR\FotoMaxxer-cli.exe"
  Delete "$INSTDIR\fotomaxxer.ico"
  Delete "$INSTDIR\README.txt"
  Delete "$INSTDIR\models\det_10g.onnx"
  Delete "$INSTDIR\models\w600k_r50.onnx"
  Delete "$INSTDIR\licenses\*.txt"
  RMDir "$INSTDIR\models"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\FotoMaxxer.lnk"
  DeleteRegKey HKCU "${UNINSTKEY}"
SectionEnd
