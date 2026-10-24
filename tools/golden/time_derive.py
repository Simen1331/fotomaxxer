#!/usr/bin/env python3
"""Mål tidsbruken for ``derive`` i Python- og Rust-versjonen på samme bibliotek.

    python3 tools/golden/time_derive.py \\
        --python ~/dev/fotomaxxer/.venv/bin/fotomaxxer \\
        --rust target/release/fotomaxxer \\
        --library ~/dev/fotomaxxer-testlib/scanlib [--runs 3] [--workers N]

Biblioteket skannes én gang (med Python-CLI-en, kildene er ``<library>/*``-mappene).
Deretter kjøres ``derive`` fra den samme skannede databasen i en fersk pakke per
kjøring, vekselvis Python og Rust, og veggtiden rapporteres. (CPU-tid lar seg ikke
sammenligne: Pythons arbeidsprosesser startes av en forkserver og telles ikke som barn.)
Alt skjer i en midlertidig mappe; biblioteket leses bare.
"""

from __future__ import annotations

import argparse
import os
import shutil
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def run(exe: str, home: Path, xdg: Path, *argv: str) -> str:
    env = {k: v for k, v in os.environ.items() if k != "FOTOMAXXER_HOME"}
    env["XDG_CONFIG_HOME"] = str(xdg)
    proc = subprocess.run(
        [exe, "--home", str(home), *argv], capture_output=True, text=True, env=env, check=True
    )
    return proc.stdout.strip()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True)
    ap.add_argument("--rust", required=True)
    ap.add_argument("--library", type=Path, required=True)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--workers", type=int)
    args = ap.parse_args()

    tmp = Path(tempfile.mkdtemp(prefix="fmx-time-")).resolve()
    try:
        xdg = tmp / "xdg"
        xdg.mkdir()
        seed = tmp / "seed"
        run(args.python, seed, xdg, "init")
        for src in sorted(p for p in args.library.iterdir() if p.is_dir()):
            run(args.python, seed, xdg, "source", "add", str(src))
        run(args.python, seed, xdg, "scan", "--all")

        extra = ["--workers", str(args.workers)] if args.workers else []
        times: dict[str, list[float]] = {"python": [], "rust": []}
        summary: dict[str, str] = {}
        for i in range(args.runs):
            for name, exe in (("python", args.python), ("rust", args.rust)):
                home = tmp / f"{name}-{i}"
                home.mkdir()
                shutil.copy2(seed / "fotomaxxer.db", home / "fotomaxxer.db")
                t0 = time.perf_counter()
                summary[name] = run(exe, home, xdg, "derive", *extra)
                times[name].append(time.perf_counter() - t0)
                shutil.rmtree(home)

        for name, walls in times.items():
            print(
                f"{name:>6}: vegg median {statistics.median(walls):6.2f} s "
                f"(min {min(walls):.2f})  —  {summary[name]}"
            )
        ratio = statistics.median(times["python"]) / statistics.median(times["rust"])
        print(f"Rust er {ratio:.2f}× så rask som Python (veggtid, median av {args.runs})")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
