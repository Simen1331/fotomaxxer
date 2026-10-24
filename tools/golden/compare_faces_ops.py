#!/usr/bin/env python3
"""Kjør samme ansikts-/personoperasjoner (klynging + kuratering) mot Python og Rust og
sammenlign svarene og hele databasen etterpå.

Kurateringsfunksjonene i ``fotomaxxer/faces.py`` har ingen CLI (de brukes av API-et),
så Python-siden kalles direkte her, og Rust-siden via eksempelprogrammet
``crates/core/examples/faces_ops.rs``. Operasjonsformatet er beskrevet der.

    cargo build --release -p fotomaxxer-core --example faces_ops
    ~/dev/fotomaxxer/.venv/bin/python tools/golden/compare_faces_ops.py \\
        --rust target/release/examples/faces_ops \\
        --seed-db ~/dev/fotomaxxer-testlib/py-golden/fotomaxxer.db \\
        --prep-sql "UPDATE faces SET cluster_id=NULL; DELETE FROM persons;" \\
        --ops tools/golden/scenarios/faces-kuratering.json

Exit 0 = identisk.
"""

from __future__ import annotations

import argparse
import difflib
import json
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

from fotomaxxer import db as fdb
from fotomaxxer import faces

sys.path.insert(0, str(Path(__file__).resolve().parent))
from compare_cli import dump  # noqa: E402


def run_python(db_path: Path, ops: list[list]) -> list[str]:
    conn = fdb.connect(db_path)
    fdb.init_db(conn)
    out = []
    for op in ops:
        name, args = op[0], op[1:]
        try:
            if name == "cluster":
                value = faces.cluster(conn, full=bool(args[0]), threshold=args[1]).summary()
            elif name == "reassign":
                value = faces.reassign_face(conn, args[0], args[1])
            elif name == "lock":
                value = faces.set_face_locked(conn, args[0], bool(args[1]))
            elif name == "create":
                value = faces.create_person_from_faces(conn, args[0], args[1])
            elif name == "merge":
                value = faces.merge_persons(conn, args[0], args[1])
            elif name == "delete":
                value = faces.delete_person(conn, args[0])
            elif name == "ignore":
                value = faces.ignore_person(conn, args[0])
            elif name == "rename":
                value = faces.set_person_name(conn, args[0], args[1])
            elif name == "sql":
                conn.executescript(args[0])
                value = None
            else:
                raise SystemExit(f"ukjent operasjon {name}")
            out.append("ok" if value is None or value == "" else f"ok {value}")
        except ValueError as e:
            out.append(f"feil: {e}")
    conn.close()
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust", required=True, help="target/release/examples/faces_ops")
    ap.add_argument("--seed-db", type=Path, required=True)
    ap.add_argument("--prep-sql")
    ap.add_argument("--ops", type=Path, required=True)
    args = ap.parse_args()
    ops = json.loads(args.ops.read_text(encoding="utf-8"))

    tmp = Path(tempfile.mkdtemp(prefix="fmx-faces-ops-")).resolve()
    dbs = {name: tmp / f"{name}.db" for name in ("python", "rust")}
    for path in dbs.values():
        shutil.copy2(args.seed_db, path)
        if args.prep_sql:
            with sqlite3.connect(path) as pconn:
                pconn.executescript(args.prep_sql)

    py_out = run_python(dbs["python"], ops)
    proc = subprocess.run(
        [args.rust, str(dbs["rust"]), str(args.ops)], capture_output=True, text=True, check=False
    )
    if proc.returncode != 0:
        print(proc.stderr)
        return 1
    rs_out = proc.stdout.splitlines()

    failures = 0
    for op, p, r in zip(ops, py_out, rs_out, strict=True):
        label = json.dumps(op, ensure_ascii=False)
        if p == r:
            print(f"  [LIK]   {label} → {p}")
        else:
            failures += 1
            print(f"  [ULIK]  {label}\n      python: {p}\n      rust:   {r}")

    dumps = {name: dump(path, tmp, tmp) for name, path in dbs.items()}
    if dumps["python"] == dumps["rust"]:
        print(f"  [LIK]   database ({len(dumps['rust'])} linjer)")
    else:
        failures += 1
        print("  [ULIK]  database")
        for line in difflib.unified_diff(
            dumps["python"], dumps["rust"], "python", "rust", lineterm="", n=0
        ):
            print(f"      {line}")
    shutil.rmtree(tmp, ignore_errors=True)
    print("IDENTISK" if failures == 0 else f"{failures} avvik")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
