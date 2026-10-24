"""AI-steg: ett vision-kall per bilde via OpenRouter på 1024-derivatet.

Leverandøren ligger bak ett lite grensesnitt (``VisionProvider``) — kun
``OpenRouterProvider`` finnes nå, en lokal Ollama-variant slotter inn senere
uten å røre kall-stedet. Provider velges av ``settings`` (nøkkel + modell),
aldri ``.env``. Nettverksbundet → tråd-pool (ikke prosess-pool som ``derive``),
eksponentiell backoff på 429/5xx. Idempotent og myk per bilde: én råtten
respons stopper ikke batchen — samme jernregel som ``derive``.

Til forskjell fra alt annet i FotoMaxxer treffer dette steget et betalt API.
``tag`` teller pending og krever bekreftelse over en terskel med mindre
``assume_yes`` er satt.
"""

from __future__ import annotations

import base64
import json
import sqlite3
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol, TypedDict

import requests

from . import settings as settings_mod
from .storage import Storage

DEFAULT_WORKERS = 6
# Batcher over dette antallet krever eksplisitt bekreftelse (--yes hopper over).
CONFIRM_THRESHOLD = 200

_OPENROUTER_URL = "https://openrouter.ai/api/v1/chat/completions"
_MAX_ATTEMPTS = 5
_TEMPERATURE = 0.2

_PROMPT = (
    "Du analyserer ett privat familiebilde for et fotoalbum-arkiv. Svar KUN med JSON,\n"
    "ingen forklaring, på dette formatet:\n"
    "{\n"
    '  "scene": "<kort scene, f.eks. strand, skog, kjøkken>",\n'
    '  "event_type": "<hverdag|bursdag|jul|ferie|bryllup|tur|fest|annet>",\n'
    '  "season": "<vinter|vår|sommer|høst|null>",\n'
    '  "indoor_outdoor": "<ute|inne|null>",\n'
    '  "person_count": <heltall, antall synlige personer>,\n'
    '  "description": "<1–2 setninger på norsk om hva bildet viser>",\n'
    '  "tags": ["<3–8 frie stikkord på norsk>"]\n'
    "}\n"
    "Bruk null der du er usikker. Ikke gjett på navn eller identitet."
)


_TAGS_LINE = '  "tags": ["<3–8 frie stikkord på norsk>"]\n'


def _build_prompt(focus: str | None, categories: list[str] | None = None) -> str:
    """Bygg vision-prompten ut fra en tagging-profil. Tre stier:

    1. Tom fokus → eksakt dagens prompt (bit-for-bit) — standardstien bevart.
    2. Fokus, ingen kategorier → berik flate tags + description, hev tak 3–8 → 3–12.
    3. Fokus + kategorier → be modellen bøtte stikkord per kategori (``tags`` blir et
       objekt nøklet på kategoriene), grunnform/entall, 0–4 per kategori.

    ``event_type``-lista og resten av JSON-skjemaet er uendret i alle stier (Akse B):
    kun frie tags + description berikes.
    """
    if focus is None or not focus.strip():
        return _PROMPT
    focus = focus.strip()
    cats = [c.strip() for c in (categories or []) if c and c.strip()]

    if not cats:
        directive = (
            f"Fokusområde: {focus}.\n"
            "Når motiver innen dette området er i bildet, gi flere og mer spesifikke stikkord\n"
            "(materiale, teknikk, stil, periode der relevant) og en rikere description. Er fokusområdet\n"
            "ikke representert i bildet, merk helt som vanlig.\n\n"
        )
        body = _PROMPT.replace('"<3–8 frie stikkord på norsk>"', '"<3–12 frie stikkord på norsk>"')
        return directive + body

    cat_list = ", ".join(cats)
    cat_json = ", ".join(f'"{c}": ["<grunnform>"]' for c in cats)
    directive = (
        f"Fokusområde: {focus}.\n"
        f"Når motiver innen dette området er i bildet, gi spesifikke stikkord gruppert i disse "
        f"kategoriene: {cat_list}.\n"
        "Bruk grunnform (entall, ikke bøyning), 0–4 stikkord per kategori, kun der det er dekning i\n"
        "bildet. Er fokusområdet ikke representert i bildet, merk helt som vanlig.\n\n"
    )
    body = _PROMPT.replace(_TAGS_LINE, f'  "tags": {{ {cat_json} }}\n')
    return directive + body


class VisionResult(TypedDict):
    scene: str | None
    event_type: str | None
    season: str | None
    indoor_outdoor: str | None
    person_count: int | None
    description: str | None
    tags: list[str]                      # flate navn (dedup, rekkefølge bevart)
    tag_categories: dict[str, str]       # navn → kategori (kun de kategoriserte)
    raw: dict


class VisionProvider(Protocol):
    name: str

    def tag(
        self, image_path: Path, focus: str | None = None, categories: list[str] | None = None
    ) -> VisionResult: ...


class TaggingError(Exception):
    """Konfig-/oppsettsfeil som skal stoppe hele kjøringen (ikke per-bilde)."""


@dataclass
class TagStats:
    pending: int = 0
    done: int = 0
    errors: int = 0
    skipped: bool = False  # True hvis bruker avbrøt på kostnadsgate

    def summary(self) -> str:
        if self.skipped:
            return f"avbrutt (pending={self.pending}, ingen kall gjort)"
        return f"pending={self.pending} done={self.done} feil={self.errors}"


# --- Leverandør ------------------------------------------------------------


class OpenRouterProvider:
    """Vision-tagging via OpenRouter chat-completions med bilde-input."""

    def __init__(self, api_key: str, model: str):
        self._api_key = api_key
        self.model = model
        self.name = f"openrouter:{model}"

    def tag(
        self, image_path: Path, focus: str | None = None, categories: list[str] | None = None
    ) -> VisionResult:
        data_url = _encode_image(image_path)
        payload = {
            "model": self.model,
            "temperature": _TEMPERATURE,
            "response_format": {"type": "json_object"},
            "usage": {"include": True},  # be OpenRouter returnere faktisk kost i usage.cost
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": _build_prompt(focus, categories)},
                        {"type": "image_url", "image_url": {"url": data_url}},
                    ],
                }
            ],
        }
        headers = {
            "Authorization": f"Bearer {self._api_key}",
            "Content-Type": "application/json",
            "X-Title": "FotoMaxxer",
        }
        raw = _post_with_backoff(payload, headers)
        content = raw["choices"][0]["message"]["content"]
        parsed = _parse_json(content)
        return _to_result(parsed, raw)


def _encode_image(image_path: Path) -> str:
    with open(image_path, "rb") as fh:
        b64 = base64.b64encode(fh.read()).decode("ascii")
    return f"data:image/jpeg;base64,{b64}"


def _post_with_backoff(payload: dict, headers: dict) -> dict:
    """POST med eksponentiell backoff på 429/5xx og nettverksfeil."""
    delay = 1.0
    last_exc: Exception | None = None
    for attempt in range(1, _MAX_ATTEMPTS + 1):
        try:
            resp = requests.post(_OPENROUTER_URL, json=payload, headers=headers, timeout=90)
            if resp.status_code == 429 or resp.status_code >= 500:
                last_exc = TaggingError(f"HTTP {resp.status_code}: {resp.text[:200]}")
                if attempt < _MAX_ATTEMPTS:
                    time.sleep(delay)
                    delay *= 2
                    continue
                raise last_exc
            resp.raise_for_status()
            return resp.json()
        except requests.RequestException as e:
            last_exc = e
            if attempt < _MAX_ATTEMPTS:
                time.sleep(delay)
                delay *= 2
                continue
            raise
    # Uoppnåelig, men holder typecheckeren glad.
    raise last_exc if last_exc else TaggingError("ukjent feil")


def _parse_json(content: str) -> dict:
    """Tolerant JSON-parse: strip ev. kode-gjerder, finn ytre {...}."""
    text = content.strip()
    if text.startswith("```"):
        # ```json … ``` eller ``` … ```
        text = text.split("```", 2)[1] if text.count("```") >= 2 else text.strip("`")
        if text.lstrip().lower().startswith("json"):
            text = text.lstrip()[4:]
    text = text.strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        start, end = text.find("{"), text.rfind("}")
        if start != -1 and end != -1 and end > start:
            return json.loads(text[start : end + 1])
        raise


def _to_result(parsed: dict, raw: dict) -> VisionResult:
    """Map en (muligens ufullstendig) JSON-dict til VisionResult. Manglende → null."""

    def _str(v: object) -> str | None:
        if v is None:
            return None
        s = str(v).strip()
        return s if s and s.lower() != "null" else None

    pc = parsed.get("person_count")
    try:
        person_count = int(pc) if pc is not None and str(pc).strip() != "" else None
    except (ValueError, TypeError):
        person_count = None

    tags, tag_categories = _parse_tags(parsed.get("tags"))

    return VisionResult(
        scene=_str(parsed.get("scene")),
        event_type=_str(parsed.get("event_type")),
        season=_str(parsed.get("season")),
        indoor_outdoor=_str(parsed.get("indoor_outdoor")),
        person_count=person_count,
        description=_str(parsed.get("description")),
        tags=tags,
        tag_categories=tag_categories,
        raw=raw,
    )


def _parse_tags(raw_tags: object) -> tuple[list[str], dict[str, str]]:
    """Normaliser ``tags`` til (flate navn, navn→kategori). Tåler begge formene:

    - flat liste ``["eik", "dreid"]`` (ingen/eldre profil) → ingen kategorier.
    - kategori-objekt ``{"materiale": ["eik"], "teknikk": ["dreid"]}`` → kategori per tag.

    Navn lowercases (som før); kategori strippes men beholder profil-casing. Dedup
    på navn, første forekomst (og dens kategori) vinner."""
    pairs: list[tuple[str, str | None]] = []
    if isinstance(raw_tags, dict):
        for cat, names in raw_tags.items():
            category = str(cat).strip() or None
            if not isinstance(names, list):
                names = [names]
            for t in names:
                if isinstance(t, (str, int)) and str(t).strip():
                    pairs.append((str(t).strip().lower(), category))
    elif isinstance(raw_tags, list):
        for t in raw_tags:
            if isinstance(t, (str, int)) and str(t).strip():
                pairs.append((str(t).strip().lower(), None))

    seen: dict[str, str | None] = {}
    for name, cat in pairs:
        if name not in seen:
            seen[name] = cat
    tags = list(seen.keys())
    tag_categories = {n: c for n, c in seen.items() if c}
    return tags, tag_categories


def make_provider(conn: sqlite3.Connection) -> VisionProvider:
    """Bygg den aktive provideren fra settings. Feiler tydelig hvis nøkkel mangler."""
    api_key = settings_mod.get_setting(conn, "openrouter_api_key")
    if not api_key:
        raise TaggingError(
            "Mangler OpenRouter-nøkkel. Kjør: fotomaxxer settings set openrouter_api_key <key>"
        )
    model = settings_mod.get_setting(conn, "ai_model") or settings_mod.DEFAULTS["ai_model"]
    return OpenRouterProvider(api_key=api_key, model=model)


# --- Steg-kjøring -----------------------------------------------------------


def _candidate_rows(
    conn: sqlite3.Connection, limit: int | None, source_id: int | None = None
) -> list[sqlite3.Row]:
    # Effektiv profil per bilde: kildens profile_id (dedup: ikke-tom, ellers laveste
    # source_id) → globalt default_profile_id → ingen. Gjenbrukt for focus + categories.
    eff_pid = """COALESCE(
                   (SELECT s.profile_id FROM files f
                      JOIN sources s ON s.id = f.source_id
                     WHERE f.image_id = i.id AND s.profile_id IS NOT NULL
                     ORDER BY f.source_id ASC LIMIT 1),
                   (SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'default_profile_id')
               )"""
    sql = f"""
        SELECT i.id AS image_id, i.content_hash AS content_hash,
               (SELECT p.focus      FROM tagging_profiles p WHERE p.id = {eff_pid}) AS focus,
               (SELECT p.categories FROM tagging_profiles p WHERE p.id = {eff_pid}) AS categories
        FROM images i
        JOIN processing_state pa ON pa.image_id = i.id AND pa.stage = 'ai'     AND pa.status = 'pending'
        JOIN processing_state pd ON pd.image_id = i.id AND pd.stage = 'derive' AND pd.status = 'done'
        JOIN derived d           ON d.image_id  = i.id AND d.size = '1024'
    """
    params: list[object] = []
    if source_id is not None:
        sql += "        AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)\n"
        params.append(source_id)
    sql += "        ORDER BY i.id"
    if limit:  # 0 / None = alle pending (ikke ubegrenset-token-felle fra DEE-CLI)
        sql += "\n        LIMIT ?"
        params.append(limit)
    return conn.execute(sql, params).fetchall()


def _reset_for_retag(conn: sqlite3.Connection, source_id: int | None = None) -> None:
    """Nullstill ai-state til pending for bilder med ferdig derive (re-tagging).

    ``source_id`` begrenser nullstillingen til den kilden, så en per-mappe retag
    ikke nullstiller andre mappers ai-state."""
    sql = """
        UPDATE processing_state SET status = 'pending', error = NULL,
               updated_at = datetime('now')
        WHERE stage = 'ai' AND status IN ('done', 'error')
          AND image_id IN (
              SELECT image_id FROM processing_state WHERE stage = 'derive' AND status = 'done'
          )
    """
    params: list[object] = []
    if source_id is not None:
        sql += " AND image_id IN (SELECT image_id FROM files WHERE source_id = ?)"
        params.append(source_id)
    conn.execute(sql, params)
    conn.commit()


def tag(
    conn: sqlite3.Connection,
    storage: Storage,
    limit: int | None = None,
    workers: int | None = None,
    retag: bool = False,
    assume_yes: bool = False,
    source_id: int | None = None,
) -> TagStats:
    """Merk pending bilder via OpenRouter → ai_tags + tags/image_tags.

    Idempotent: ``done``-bilder hoppes over. ``retag`` nullstiller ai-state
    først. Per-bilde feil → ``status='error'``, jobben fortsetter. ``source_id``
    begrenser til bilder med en fil i den kilden (per-mappe-kjøring).
    """
    provider = make_provider(conn)  # feiler tidlig hvis nøkkel mangler

    if retag:
        _reset_for_retag(conn, source_id)

    rows = _candidate_rows(conn, limit, source_id)
    stats = TagStats(pending=len(rows))
    if not rows:
        return stats

    # Kostnadsgate: dette treffer et betalt API.
    if len(rows) > CONFIRM_THRESHOLD and not assume_yes:
        print(
            f"Skal merke {len(rows)} bilder via {provider.name} — dette treffer et "
            f"betalt API og koster ekte penger (per-bilde-pris varierer med modell)."
        )
        answer = input("Fortsett? [j/N] ").strip().lower()
        if answer not in ("j", "ja", "y", "yes"):
            stats.skipped = True
            return stats

    # Per-bilde: arbeidskopi-sti + resolvert profil (fokus + kategorier), så en global
    # «Oppdater alt» over flere kilder respekterer hvert bildes profil.
    jobs = {
        r["image_id"]: (
            storage.derived_path(r["content_hash"], "1024"),
            r["focus"],
            _parse_categories(r["categories"]),
        )
        for r in rows
    }
    max_workers = workers or DEFAULT_WORKERS

    with ThreadPoolExecutor(max_workers=max_workers) as pool:
        futures = {
            pool.submit(_tag_one, image_id, path, focus, cats, provider): image_id
            for image_id, (path, focus, cats) in jobs.items()
        }
        for fut in as_completed(futures):
            image_id = futures[fut]
            try:
                result = fut.result()
            except Exception as e:  # nettverk gir opp etter retry, korrupt fil, parse-feil
                _set_state(conn, image_id, "error", f"{type(e).__name__}: {e}")
                conn.commit()
                stats.errors += 1
                continue
            _persist(conn, image_id, provider.name, result)
            conn.commit()
            stats.done += 1

    return stats


def _parse_categories(raw: object) -> list[str]:
    """Profilens ``categories``-JSON → liste. Tom/ugyldig → []."""
    if not raw:
        return []
    try:
        cats = json.loads(raw) if isinstance(raw, str) else raw
    except (json.JSONDecodeError, TypeError):
        return []
    return [str(c).strip() for c in cats if str(c).strip()] if isinstance(cats, list) else []


def _tag_one(
    image_id: int,
    path: Path,
    focus: str | None,
    categories: list[str],
    provider: VisionProvider,
) -> VisionResult:
    """Tråd-arbeider: ren av DB. Kaller provideren på 1024-derivatet med resolvert profil."""
    if not path.exists():
        raise TaggingError(f"mangler 1024-derivat: {path.name}")
    return provider.tag(path, focus, categories)


def _extract_cost(raw: object) -> float | None:
    """Plukk faktisk kost (USD) fra OpenRouter-svarets ``usage.cost``. NULL hvis ukjent."""
    usage = raw.get("usage") if isinstance(raw, dict) else None
    cost = usage.get("cost") if isinstance(usage, dict) else None
    try:
        return float(cost) if cost is not None else None
    except (TypeError, ValueError):
        return None


def _persist(conn: sqlite3.Connection, image_id: int, provider_name: str, result: VisionResult) -> None:
    """Skriv ai_tags + tags/image_tags + sett ai-state done. Erstatter ev. eldre rad."""
    conn.execute("DELETE FROM ai_tags WHERE image_id = ?", (image_id,))
    conn.execute("DELETE FROM image_tags WHERE image_id = ?", (image_id,))
    conn.execute(
        "INSERT INTO ai_tags (image_id, provider, scene, event_type, season_ai, "
        "indoor_outdoor, person_count, description, raw_json, cost) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            image_id,
            provider_name,
            result["scene"],
            result["event_type"],
            result["season"],
            result["indoor_outdoor"],
            result["person_count"],
            result["description"],
            json.dumps(result["raw"], ensure_ascii=False),
            _extract_cost(result["raw"]),
        ),
    )
    cats = result["tag_categories"]
    for name in dict.fromkeys(result["tags"]):  # dedup, bevar rekkefølge
        # Kategori er global per tag, sist ikke-null vinner (COALESCE) — en senere
        # flat merking (kategori=NULL) viser ikke ut en allerede satt kategori.
        conn.execute(
            "INSERT INTO tags (name, category) VALUES (?, ?) "
            "ON CONFLICT(name) DO UPDATE SET category = COALESCE(excluded.category, tags.category)",
            (name, cats.get(name)),
        )
        tag_row = conn.execute("SELECT id FROM tags WHERE name = ?", (name,)).fetchone()
        conn.execute(
            "INSERT INTO image_tags (image_id, tag_id) VALUES (?, ?) "
            "ON CONFLICT(image_id, tag_id) DO NOTHING",
            (image_id, tag_row["id"]),
        )
    _set_state(conn, image_id, "done", None)


def _set_state(conn: sqlite3.Connection, image_id: int, status: str, error: str | None) -> None:
    conn.execute(
        "UPDATE processing_state SET status = ?, error = ?, updated_at = datetime('now') "
        "WHERE image_id = ? AND stage = 'ai'",
        (status, error, image_id),
    )
