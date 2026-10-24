"""EXIF-uttrekk via exiftool/pyexiftool.

Åpne én ``ExifReader`` for hele scannen (exiftool-prosessen er dyr å starte per
fil). Henter ``taken_at``/``year``/``season``, kamera, dimensjoner og GPS i
desimalgrader. EXIF-feil blokkerer aldri ingest — da returneres et tomt dict.
"""

from __future__ import annotations

from datetime import datetime
from pathlib import Path
from typing import Any

import exiftool

from .resources import exiftool_executable

# Måned → årstid (nordlig halvkule), per TB.
_SEASON = {
    12: "vinter", 1: "vinter", 2: "vinter",
    3: "vår", 4: "vår", 5: "vår",
    6: "sommer", 7: "sommer", 8: "sommer",
    9: "høst", 10: "høst", 11: "høst",
}


class ExifReader:
    """Kontekst-styrt wrapper rundt én langtlevende exiftool-prosess."""

    def __init__(self) -> None:
        self._helper: exiftool.ExifToolHelper | None = None

    def __enter__(self) -> "ExifReader":
        # ExifToolHelper bruker som standard common_args ["-G", "-n"] →
        # gruppe-prefiks på tag-navn + numeriske verdier (GPS i desimalgrader).
        self._helper = exiftool.ExifToolHelper(executable=exiftool_executable())
        self._helper.run()
        return self

    def __exit__(self, *exc: object) -> None:
        if self._helper is not None:
            self._helper.terminate()
            self._helper = None

    def extract(self, path: str | Path) -> dict[str, Any]:
        """Returnér normaliserte EXIF-felter, eller ``{}`` ved feil."""
        assert self._helper is not None, "ExifReader brukt utenfor with-blokk"
        try:
            meta = self._helper.get_metadata([str(path)])[0]
        except Exception:
            return {}
        return _parse(meta)


def _parse(m: dict[str, Any]) -> dict[str, Any]:
    taken_at = _parse_datetime(
        m.get("EXIF:DateTimeOriginal")
        or m.get("EXIF:CreateDate")
        or m.get("QuickTime:CreateDate")
    )
    year: int | None = None
    season: str | None = None
    if taken_at:
        dt = datetime.fromisoformat(taken_at)
        year = dt.year
        season = _SEASON.get(dt.month)

    return {
        "taken_at": taken_at,
        "year": year,
        "season": season,
        "width": _first_int(m, "EXIF:ImageWidth", "File:ImageWidth",
                            "EXIF:ExifImageWidth", "PNG:ImageWidth"),
        "height": _first_int(m, "EXIF:ImageHeight", "File:ImageHeight",
                             "EXIF:ExifImageHeight", "PNG:ImageHeight"),
        "camera_make": _clean_str(m.get("EXIF:Make")),
        "camera_model": _clean_str(m.get("EXIF:Model")),
        "gps_lat": _to_float(m.get("Composite:GPSLatitude") or m.get("EXIF:GPSLatitude")),
        "gps_lon": _to_float(m.get("Composite:GPSLongitude") or m.get("EXIF:GPSLongitude")),
    }


def _parse_datetime(value: Any) -> str | None:
    """``"2020:06:15 14:30:00"`` → ISO8601. Ugyldig/tom → None."""
    if not value or not isinstance(value, str):
        return None
    head = value.strip()[:19]
    try:
        return datetime.strptime(head, "%Y:%m:%d %H:%M:%S").isoformat(sep="T")
    except ValueError:
        return None


def _first_int(m: dict[str, Any], *keys: str) -> int | None:
    for k in keys:
        v = m.get(k)
        if v is not None:
            try:
                return int(v)
            except (TypeError, ValueError):
                continue
    return None


def _to_float(value: Any) -> float | None:
    if value is None:
        return None
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _clean_str(value: Any) -> str | None:
    if value is None:
        return None
    s = str(value).strip()
    return s or None
