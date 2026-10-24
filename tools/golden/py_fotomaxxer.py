#!/usr/bin/env python3
"""Master-versjonens CLI med OpenRouter-endepunktet overstyrt fra miljøet.

Python-koden endres ikke: skriptet setter ``fotomaxxer.tagging._OPENROUTER_URL`` fra
``FOTOMAXXER_OPENROUTER_URL`` (samme testkrok som Rust-versjonen har innebygd) og kaller
``fotomaxxer.cli.main()``. Brukes som ``--python`` i ``compare_cli.py``/``compare_tagging.py``
så begge implementasjonene snakker med den samme falske tjeneren.

``FMX_WEB_DIST`` overstyrer ``fotomaxxer.resources.frontend_dist()`` før ``serve`` importerer
``api.app``, så Python-tjeneren serverer den samme ``web/dist`` som er bakt inn i
Rust-binæren (``compare_api.py``).

Finnes ikke ``fotomaxxer`` i den Python-en som kjører skriptet, startes det på nytt med
``$FMX_PYTHON`` (standard ``~/dev/fotomaxxer/.venv/bin/python``).
"""

from __future__ import annotations

import os
import sys

try:
    from fotomaxxer import cli, tagging
except ImportError:
    py = os.environ.get("FMX_PYTHON", os.path.expanduser("~/dev/fotomaxxer/.venv/bin/python"))
    if os.environ.get("_FMX_REEXEC"):
        raise
    os.environ["_FMX_REEXEC"] = "1"
    os.execv(py, [py, os.path.abspath(__file__), *sys.argv[1:]])

url = os.environ.get("FOTOMAXXER_OPENROUTER_URL")
if url:
    tagging._OPENROUTER_URL = url
dist = os.environ.get("FMX_WEB_DIST")
if dist:
    from pathlib import Path

    from fotomaxxer import resources

    resources.frontend_dist = lambda: Path(dist)

# Vakten trengs: Python 3.14 bruker «forkserver» for prosesspooler (derive, faces), og
# barneprosessene importerer hovedmodulen på nytt. Uten vakt ville de startet CLI-en igjen.
if __name__ == "__main__":
    sys.exit(cli.main())
