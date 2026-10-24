#!/usr/bin/env python3
"""Fasit for EXIF: kjør master-versjonens ``ExifReader`` (ExifTool) på bildefiler.

Skriver én JSON-linje per fil (``path`` + normaliserte felter fra ``exif._parse``).
Argumenter er mapper (gås rekursivt, samme whitelist som scan) eller enkeltfiler;
``-`` leser én filsti per linje fra stdin.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/exif_golden.py ut.jsonl MAPPE_ELLER_FIL...
"""

from __future__ import annotations

import json
import os
import sys

from fotomaxxer.exif import ExifReader
from fotomaxxer.scan import WHITELIST


def files(args: list[str]):
    for arg in args:
        if arg == "-":
            yield from (line.rstrip("\n") for line in sys.stdin if line.strip())
            continue
        if os.path.isfile(arg):
            yield arg
            continue
        for dirpath, _dirs, names in os.walk(arg):
            for name in names:
                if os.path.splitext(name)[1].lower() in WHITELIST:
                    yield os.path.join(dirpath, name)


def main() -> int:
    out_path, targets = sys.argv[1], sys.argv[2:]
    n = 0
    with ExifReader() as exif, open(out_path, "w", encoding="utf-8") as out:
        for path in files(targets):
            out.write(json.dumps({"path": path, "meta": exif.extract(path)}, ensure_ascii=False) + "\n")
            n += 1
    print(f"{n} filer → {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
