#!/usr/bin/env python3
"""Fasit for AI-merking: kjør master-versjonens rene hjelpefunksjoner i ``tagging.py``.

Skriver én JSON-fil med prompt per profilsti (``_build_prompt``), tolerant parsing
(``_parse_json`` + ``_to_result``), tag-normalisering (``_parse_tags``),
kategori-parsing (``_parse_categories``), kostnadsuttrekk (``_extract_cost``) og
``json.dumps`` av flyttall. Rust-testene i ``crates/core/src/tagging.rs`` leser fila
og krever samme resultat. Ingen nettverkskall.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/tagging_golden.py \\
        crates/core/tests/fixtures/tagging_golden.json
"""

from __future__ import annotations

import json
import sys

from fotomaxxer import tagging as t

PROMPT_CASES: list[tuple[str | None, list[str] | None]] = [
    (None, None),
    ("", []),
    ("   ", ["materiale"]),
    ("Design og håndverk", []),
    ("  Design og håndverk  ", None),
    ("Design og håndverk", ["materiale", "teknikk", "stil"]),
    ("Fugler", ["  art ", "", "atferd"]),
    ("Fokus med \"anførsel\" og {klammer}", ["kategori med \"quote\"", "{x}"]),
    ("Linje en\nlinje to", ["æøå"]),
]

# Innhold slik modellen kan svare (``choices[0].message.content``).
CONTENT_CASES: list[str] = [
    '{"scene": "strand", "event_type": "ferie", "season": "sommer", "indoor_outdoor": "ute",'
    ' "person_count": 3, "description": "Tre personer på stranda.", "tags": ["Strand", "hav", "strand"]}',
    '```json\n{"scene": "skog", "tags": ["tre"]}\n```',
    '```JSON\n{"scene": "skog"}\n```',
    '```\n{"scene": "kjøkken"}\n```',
    '```{"scene": "x"}',
    '``` json {"scene": "y"} ``` etter',
    'Her er svaret: {"scene": "hage", "tags": ["blomst"]} Håper det hjelper!',
    'Beklager, jeg kan ikke analysere dette bildet.',
    '',
    '   ',
    '[1, 2, 3]',
    '"bare en streng"',
    '42',
    'null',
    '{"scene": null, "event_type": "null", "season": "NULL", "indoor_outdoor": "  ",'
    ' "person_count": null, "description": " Nullus "}',
    '{"person_count": " 4 "}',
    '{"person_count": "2.5"}',
    '{"person_count": 2.7}',
    '{"person_count": -2.7}',
    '{"person_count": true}',
    '{"person_count": "+7"}',
    '{"person_count": "1_000"}',
    '{"person_count": ""}',
    '{"person_count": []}',
    '{"person_count": "tre"}',
    '{"person_count": "007"}',
    '{"person_count": 1e3}',
    '{"scene": 12, "event_type": 1.5, "season": true, "indoor_outdoor": false,'
    ' "description": ["a", "b\'c", "d\\"e"]}',
    '{"description": {"k": "vår", "n": null, "l": [1, 2.0, 1e-05]}}',
    '{"scene": "ok"} trailing {"x": 1}',
    'tekst {ikke json} mer tekst',
    '{"scene": "uavsluttet"',
    '{"tags": {"materiale": ["Eik", "bjørk"], "teknikk": "dreid", " stil ": ["eik", "Skandinavisk"], "": ["uten"]}}',
    '{"tags": {"materiale": [1, true, 2.5, null, "  ", ["nested"], "Ok"]}}',
    '{"tags": ["A", " a ", 5, false, null, 1.5, "", "ÆØÅ", "Σ", {"x": 1}]}',
    '{"tags": "bare en streng"}',
    '{"tags": null}',
    '{"tags": {}}',
    '{"tags": []}',
    '{"scene": "dobbel", "scene": "sist"}',
    '{"description": "Linje\\nny \\u00e6 \\ud83d\\ude00 emoji"}',
    '\n\n  {"scene": "innrykk"}  \n',
    '{"scene": "x", "tags": ["a"]}\n```',
    'json {"scene": "uten gjerde"}',
]

TAG_CASES: list[object] = [
    ["Eik", "eik", " EIK ", "bjørk"],
    {"materiale": ["eik"], "teknikk": ["eik", "dreid"]},
    {"materiale": "eik"},
    {" ": ["x"]},
    {"a": None},
    {"a": [None, 1, True]},
    "streng",
    7,
    None,
    [],
    {},
]

CATEGORY_CASES: list[str | None] = [
    None,
    "",
    "[]",
    '["materiale", "teknikk"]',
    '[" materiale ", "", "  ", "stil"]',
    '[1, 2.5, true, null, "x"]',
    '["a", ["b"], {"c": 1}]',
    "ikke json",
    '{"a": 1}',
    '"streng"',
    "null",
]

COST_CASES: list[object] = [
    {"usage": {"cost": 0.000123}},
    {"usage": {"cost": 1.2e-05}},
    {"usage": {"cost": 0}},
    {"usage": {"cost": "0.0005"}},
    {"usage": {"cost": " 1e-4 "}},
    {"usage": {"cost": "gratis"}},
    {"usage": {"cost": None}},
    {"usage": {"cost": True}},
    {"usage": {"cost": [1]}},
    {"usage": {}},
    {"usage": None},
    {"usage": [1]},
    {},
    [1, 2],
]

# Hele OpenRouter-svar: ``raw["choices"][0]["message"]["content"]`` + parsing.
RESPONSE_CASES: list[object] = [
    {"choices": [{"message": {"content": '{"scene": "x"}'}}], "usage": {"cost": 1e-05}},
    {},
    {"choices": []},
    {"choices": {}},
    {"choices": "abc"},
    {"choices": ""},
    {"choices": [{"message": {"content": None}}]},
    {"choices": [{"message": {"content": ["a"]}}]},
    {"choices": [{"message": {"content": {"scene": "x"}}}]},
    {"choices": [{"message": {"content": 5}}]},
    {"choices": [{"message": {}}]},
    {"choices": [None]},
    {"choices": [5]},
    {"choices": [2.5]},
    {"choices": [True]},
    {"choices": [[1]]},
    {"choices": [{"message": "abc"}]},
    {"choices": [{"message": ""}]},
    [1],
    [],
    "x",
    5,
    None,
    True,
    1.5,
]

# Ugyldig JSON → ``json.loads``-feilmeldingen (C-skanneren i CPython).
JSON_ERROR_CASES: list[str] = [
    "", " ", "x", "{", "[", "]", "}", '{"a"', '{"a":', '{"a": 1', '{"a": 1,', '{"a": 1,}',
    '{"a": 1 , }', "[1,]", "[1 , ]", "[1 2]", '{"a" 1}', "{1: 2}", '{"a": 1 "b": 2}', '"abc',
    '"a\\qb"', '"\\u12"', '"\\u12G4"', '"\\u1234', '"\\u123', '"\\', '"a\x01b"',
    '"a\nb"', '"\\ud83d\\ude0"', '"\\ud83d\\uZZZZ"', "tru", "nul", "fals", "-", "-x",
    "01", "1.", "1e", "1.5e+", "1.x", "[1, 2", '{"a": [1, {"b": }]}', "\ufeff{}", '["a" , ]',
    "{} x", "{}\n\n  x", "æøå", '{"æ": "ø" "å"}', "\n\n{", "[\n1,\n2\n", "[-]", "[--1]",
    "Infinityx", "-Inf", "[NaN", "{\"a\": tru}", "[true false]", "1 2", "[1]]", '{"a": 1}}',
    "\t\r\n x", "[\u00a0]", '{"a":\u00a01}', "{'a': 1}", "[.5]", "[+1]", "[0x10]",
    '"æøå', '["ok", "æ\x02"]',
]

FLOATS: list[float] = [
    0.0, -0.0, 1.0, 0.2, 0.1 + 0.2, 1.5, 100.0, 1e-4, 1e-5, 9.5e-05, 1.2345e-05,
    0.000123, 0.0001234, 1e15, 1e16, 1.5e16, 123456789012345678.0, 1e22, 1e-7,
    2.5e-10, 1.7976931348623157e308, 5e-324, 3.14159265358979, 1 / 3, 2 / 3,
    0.30000000000000004, 1234567.0, 0.5, 4.35e-06,
]


def _content_case(content: str) -> dict:
    raw = {"id": "gen-1", "choices": [{"message": {"content": content}}]}
    try:
        parsed = t._parse_json(content)
        res = t._to_result(parsed, raw)
    except Exception as e:  # samme form som tag(): «Type: melding»
        return {"content": content, "error": f"{type(e).__name__}: {e}"}
    out = {k: v for k, v in res.items() if k != "raw"}
    return {"content": content, "result": out}


def _response_case(raw: object) -> dict:
    try:
        content = raw["choices"][0]["message"]["content"]  # type: ignore[index]
        res = t._to_result(t._parse_json(content), raw)  # type: ignore[arg-type]
    except Exception as e:
        return {"raw": raw, "error": f"{type(e).__name__}: {e}"}
    return {"raw": raw, "result": {k: v for k, v in res.items() if k != "raw"}}


def _json_error(text: str) -> dict:
    try:
        json.loads(text)
    except json.JSONDecodeError as e:
        return {"text": text, "error": str(e)}
    raise SystemExit(f"forventet JSON-feil for {text!r}")


def main() -> int:
    out_path = sys.argv[1]
    golden = {
        "prompts": [
            {"focus": f, "categories": c, "prompt": t._build_prompt(f, c)} for f, c in PROMPT_CASES
        ],
        "contents": [_content_case(c) for c in CONTENT_CASES],
        "responses": [_response_case(r) for r in RESPONSE_CASES],
        "json_errors": [_json_error(x) for x in JSON_ERROR_CASES],
        "tags": [
            dict(zip(("tags", "tag_categories"), t._parse_tags(raw)), input=raw) for raw in TAG_CASES
        ],
        "categories": [{"input": raw, "output": t._parse_categories(raw)} for raw in CATEGORY_CASES],
        "costs": [{"input": raw, "output": t._extract_cost(raw)} for raw in COST_CASES],
        "floats": [{"value": v, "repr": repr(v), "dumps": json.dumps({"v": [v]})} for v in FLOATS],
    }
    with open(out_path, "w", encoding="utf-8") as fh:
        json.dump(golden, fh, ensure_ascii=False, indent=1)
        fh.write("\n")
    print(
        f"Skrev {out_path}: {len(golden['prompts'])} prompter, {len(golden['contents'])} svar, "
        f"{len(golden['tags'])} tag-tilfeller"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
