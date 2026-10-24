"""Reverse-geocoding via Geonorge/Kartverket adresse-punktsøk.

Gratis, ingen nøkkel. Dekker Norge — utenlandske koordinater gir tomt
``place_name``. Cache på avrundede koordinater (3 desimaler) for å unngå
tusenvis av identiske kall. Feilende geocoding blokkerer aldri ingest: ved feil
eller treff utenfor Norge blir ``place_name`` bare NULL.
"""

from __future__ import annotations

import requests

PUNKTSOK_URL = "https://ws.geonorge.no/adresser/v1/punktsok"


class Geocoder:
    def __init__(self, radius: int = 500, precision: int = 3, timeout: float = 5.0):
        self.radius = radius
        self.precision = precision
        self.timeout = timeout
        self.cache: dict[tuple[float, float], str | None] = {}
        self._session = requests.Session()

    def lookup(self, lat: float | None, lon: float | None) -> str | None:
        if lat is None or lon is None:
            return None
        key = (round(lat, self.precision), round(lon, self.precision))
        if key in self.cache:
            return self.cache[key]
        place = self._fetch(lat, lon)
        self.cache[key] = place
        return place

    def _fetch(self, lat: float, lon: float) -> str | None:
        try:
            resp = self._session.get(
                PUNKTSOK_URL,
                params={
                    "lat": lat,
                    "lon": lon,
                    "radius": self.radius,
                    "treffPerSide": 1,
                    "side": 0,
                },
                timeout=self.timeout,
            )
            resp.raise_for_status()
            addresses = resp.json().get("adresser") or []
        except Exception:
            return None  # feiler mykt
        if not addresses:
            return None
        nearest = addresses[0]
        poststed = (nearest.get("poststed") or "").strip()
        kommune = (nearest.get("kommunenavn") or "").strip()
        return _format_place(poststed, kommune)


def _format_place(poststed: str, kommune: str) -> str | None:
    poststed = poststed.title() if poststed else ""
    kommune = kommune.title() if kommune else ""
    if poststed and kommune and poststed.lower() != kommune.lower():
        return f"{poststed}, {kommune}"
    return poststed or kommune or None
