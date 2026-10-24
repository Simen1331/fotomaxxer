#!/usr/bin/env python3
"""Kjør samme kommandorekke mot Python- og Rust-CLI-en og sammenlign resultatet.

Hver implementasjon får sin egen pakke-rot og sin egen ``XDG_CONFIG_HOME`` i en
midlertidig mappe (ekte data røres aldri). For hvert steg sammenlignes exit-kode,
stdout og stderr; til slutt sammenlignes hele databasen (``iterdump``). Stier og
tidsstempler normaliseres før sammenligning.

    python3 tools/golden/compare_cli.py \\
        --python ~/dev/fotomaxxer/.venv/bin/fotomaxxer \\
        --rust target/release/fotomaxxer [--seed-db sti/til/fotomaxxer.db] [--scenario navn]

``--seed-db`` kopierer en eksisterende database inn i begge pakkene først (tester
migrasjon på ekte data); ``--seed-dir`` kopierer en hel datamappe (``fotomaxxer.db``
+ ``derived/``). ``--prep-sql`` kjøres på begge databasene før første steg.
``--scenario-file`` leser stegene fra en JSON-liste med argv-lister (samme
plassholdere som de innebygde scenarioene). ``--env KEY=VAL`` settes for begge
implementasjonene; ``--mask-errors`` sammenligner bare om ``processing_state.error``
er satt, ikke selve feilteksten (Python-unntakstekster kan ikke gjenskapes ordrett).
Exit 0 = identisk.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

# {tmp} = felles testmappe, {src_a}/{src_b} = tomme kilde-mapper, {lib} = --library
# (et ekte testbibliotek som begge implementasjonene leser, men aldri endrer).
SCENARIOS: dict[str, list[list[str]]] = {
    "fase1": [
        ["init"],
        ["settings", "list"],
        ["settings", "set", "ai_model", "x/y"],
        ["settings", "set", "openrouter_api_key", "sk-or-v1-abcdefghijkl"],
        ["settings", "set", "face_match_threshold", "0.55"],
        ["settings", "list"],
        ["source", "add", "{src_a}"],
        ["source", "add", "{src_a}"],
        ["source", "add", "{tmp}/finnes-ikke"],
        ["source", "add", "{src_a}/bilde.jpg"],
        ["source", "list"],
        ["source", "add", "{src_b}"],
        ["source", "remove", "1"],
        ["source", "remove", "99"],
        ["source", "remove", "{src_b}"],
        ["source", "list"],
        ["source", "add", "{src_a}"],
        ["prepare-handoff"],
        ["settings", "list"],
        ["source", "list"],
    ],
    "scan": [
        ["init"],
        ["source", "add", "{lib}/a"],
        ["source", "add", "{lib}/b"],
        ["scan"],
        ["scan", "--source", "{lib}/finnes-ikke"],
        ["scan", "--all"],
        ["source", "list"],
        ["scan", "--all"],
        ["scan", "--source", "{lib}/b"],
        ["source", "add", "{lib}/a/Sommer på hytta"],
        ["scan", "--all"],
        ["source", "list"],
        ["source", "remove", "{lib}/b"],
        ["source", "list"],
    ],
    "migrering": [
        ["init"],
        ["settings", "list"],
        ["source", "list"],
    ],
}

TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}")


def normalize(text: str, tmp: Path, home: Path) -> str:
    text = text.replace(str(home), "<HOME>").replace(str(tmp), "<TMP>")
    return TIMESTAMP.sub("<TS>", text)


def dump(db: Path, tmp: Path, home: Path, mask_errors: bool = False) -> list[str]:
    if not db.exists():
        return ["<ingen database>"]
    if mask_errors:
        masked = db.with_name(db.name + ".maskert")
        shutil.copy2(db, masked)
        with sqlite3.connect(masked) as mconn:
            mconn.execute("UPDATE processing_state SET error = '<feil>' WHERE error IS NOT NULL")
        db = masked
    conn = sqlite3.connect(db)
    try:
        version = conn.execute("PRAGMA user_version").fetchone()[0]
        return [f"PRAGMA user_version = {version}"] + [
            normalize(line, tmp, home) for line in conn.iterdump()
        ]
    finally:
        conn.close()


def run_impl(
    exe: str, home: Path, xdg: Path, argv: list[str], extra_env: dict[str, str]
) -> tuple[int, str, str]:
    env = {k: v for k, v in os.environ.items() if k != "FOTOMAXXER_HOME"}
    env.update(extra_env)
    env["XDG_CONFIG_HOME"] = str(xdg)
    env["PYTHONIOENCODING"] = "utf-8"
    proc = subprocess.run(
        [exe, "--home", str(home), *argv], capture_output=True, text=True, env=env, timeout=3600
    )
    return proc.returncode, proc.stdout, proc.stderr


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True)
    ap.add_argument("--rust", required=True)
    ap.add_argument("--seed-db", type=Path)
    ap.add_argument("--seed-dir", type=Path)
    ap.add_argument("--prep-sql")
    ap.add_argument("--scenario", default="fase1", choices=sorted(SCENARIOS))
    ap.add_argument("--scenario-file", type=Path)
    ap.add_argument("--library", type=Path, default=Path("/finnes-ikke"))
    ap.add_argument("--env", action="append", default=[], metavar="KEY=VAL")
    ap.add_argument("--mask-errors", action="store_true")
    args = ap.parse_args()
    steps = (
        json.loads(args.scenario_file.read_text(encoding="utf-8"))
        if args.scenario_file
        else SCENARIOS[args.scenario]
    )
    extra_env = dict(kv.split("=", 1) for kv in args.env)

    tmp = Path(tempfile.mkdtemp(prefix="fmx-cli-")).resolve()
    src_a, src_b = tmp / "kilde æøå", tmp / "kilde b"
    for d in (src_a, src_b):
        d.mkdir()
    (src_a / "bilde.jpg").write_bytes(b"ikke et ekte bilde")

    impls = {"python": args.python, "rust": args.rust}
    homes = {name: tmp / f"pakke-{name}" for name in impls}
    for name, home in homes.items():
        home.mkdir()
        (tmp / f"xdg-{name}").mkdir()
        if args.seed_db:
            shutil.copy2(args.seed_db, home / "fotomaxxer.db")
        if args.seed_dir:
            shutil.copy2(args.seed_dir / "fotomaxxer.db", home / "fotomaxxer.db")
            if (args.seed_dir / "derived").is_dir():
                shutil.copytree(args.seed_dir / "derived", home / "derived")
        if args.prep_sql:
            with sqlite3.connect(home / "fotomaxxer.db") as pconn:
                pconn.executescript(args.prep_sql)

    failures = 0
    for step in steps:
        argv = [a.format(tmp=tmp, src_a=src_a, src_b=src_b, lib=args.library) for a in step]
        results = {
            name: run_impl(exe, homes[name], tmp / f"xdg-{name}", argv, extra_env)
            for name, exe in impls.items()
        }
        norm = {
            name: tuple(normalize(str(x), tmp, homes[name]) for x in res)
            for name, res in results.items()
        }
        label = " ".join(step)
        if norm["python"] == norm["rust"]:
            print(f"  [LIK]   {label}")
            continue
        failures += 1
        print(f"  [ULIK]  {label}")
        for part, (p, r) in zip(("exit", "stdout", "stderr"), zip(norm["python"], norm["rust"])):
            if p != r:
                print(f"    {part}:")
                for line in difflib.unified_diff(
                    p.splitlines(), r.splitlines(), "python", "rust", lineterm="", n=1
                ):
                    print(f"      {line}")

    dumps = {
        name: dump(home / "fotomaxxer.db", tmp, home, args.mask_errors) for name, home in homes.items()
    }
    if dumps["python"] == dumps["rust"]:
        print(f"  [LIK]   database ({len(dumps['rust'])} linjer)")
    else:
        failures += 1
        print("  [ULIK]  database")
        for line in difflib.unified_diff(dumps["python"], dumps["rust"], "python", "rust", lineterm="", n=0):
            print(f"      {line}")

    if os.environ.get("FMX_KEEP_TMP"):
        print(f"  (beholdt {tmp})")
    else:
        shutil.rmtree(tmp, ignore_errors=True)
    print("IDENTISK" if failures == 0 else f"{failures} avvik")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
