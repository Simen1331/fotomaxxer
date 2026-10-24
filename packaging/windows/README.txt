FotoMaxxer
==========

Lokal foto-kuratering for trykte album. Alt kjører på denne maskina; internett trengs
bare til AI-merking (OpenRouter) og adresseoppslag (Kartverket).

Start
-----
Start FotoMaxxer fra Start-menyen. Første gang Windows viser «Windows beskyttet PC-en
din» (SmartScreen), fordi programmet ikke er kodesignert: klikk «Flere detaljer» og
så «Kjør likevel».

Hvor ligger hva
---------------
Programmet:  %LOCALAPPDATA%\Programs\FotoMaxxer
             (FotoMaxxer.exe, FotoMaxxer-cli.exe, models\ med ansiktsmodellene)
Biblioteket: %LOCALAPPDATA%\FotoMaxxer   (database, derived\, logs\)
             Ta backup av denne mappa. Den kan også flyttes med --home <mappe>.

Avinstallering
--------------
Innstillinger → Apper → FotoMaxxer → Avinstaller. Bare programfilene, snarveien og
oppføringen under Apper fjernes. Biblioteket (%LOCALAPPDATA%\FotoMaxxer) og bildene
dine blir liggende; slett mappa selv hvis du vil fjerne alt.

Kommandolinje
-------------
FotoMaxxer-cli.exe er samme program for terminalen, f.eks.:
  "%LOCALAPPDATA%\Programs\FotoMaxxer\FotoMaxxer-cli.exe" --help

Lisenser for tredjepartskomponenter ligger i mappa licenses\.
