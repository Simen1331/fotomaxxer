#!/usr/bin/env python3
"""Sammenlign to ``derived/``-mapper (Python-fasit mot Rust) piksel for piksel.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/compare_derived.py \\
        PYTHON_DERIVED RUST_DERIVED [--min-psnr 40] [--worst 10] [--sources DB]

Krav som sjekkes:

1. Samme filnavn i begge mappene (``<hash>_<1024|300>.jpg``).
2. Nøyaktig samme dimensjoner for hver fil.
3. Pikselavvik etter dekoding med Pillow (samme dekoder for begge): snitt og maks
   absolutt avvik per kanal, andel ulike verdier og PSNR (RGB, 8 bit). Filer under
   ``--min-psnr`` rapporteres som avvik.

Alle tall skrives ut uansett. ``--sources`` (en ``fotomaxxer.db``) viser kildefil og
format for de dårligste filene. Exit 0 = navn og dimensjoner like og alle filer over
PSNR-kravet.
"""

from __future__ import annotations

import argparse
import math
import sqlite3
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def load(path: Path) -> np.ndarray:
    with Image.open(path) as im:
        return np.asarray(im.convert("RGB"), dtype=np.int16)


def psnr(mse: float) -> float:
    return math.inf if mse == 0 else 10 * math.log10(255.0**2 / mse)


def sources_by_hash(db: Path) -> dict[str, str]:
    conn = sqlite3.connect(db)
    try:
        rows = conn.execute(
            "SELECT i.content_hash, MIN(f.path) FROM images i JOIN files f ON f.image_id = i.id "
            "GROUP BY i.id"
        ).fetchall()
    finally:
        conn.close()
    return {h: p for h, p in rows}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("python_dir", type=Path)
    ap.add_argument("rust_dir", type=Path)
    ap.add_argument("--min-psnr", type=float, default=40.0)
    ap.add_argument("--worst", type=int, default=10)
    ap.add_argument("--sources", type=Path, help="fotomaxxer.db for å vise kildefilene")
    args = ap.parse_args()

    py_names = {p.name for p in args.python_dir.glob("*.jpg")}
    rs_names = {p.name for p in args.rust_dir.glob("*.jpg")}
    failures = 0
    only_py, only_rs = sorted(py_names - rs_names), sorted(rs_names - py_names)
    print(f"filer: python={len(py_names)} rust={len(rs_names)} felles={len(py_names & rs_names)}")
    for name in only_py:
        print(f"  [MANGLER i rust]   {name}")
    for name in only_rs:
        print(f"  [MANGLER i python] {name}")
    failures += len(only_py) + len(only_rs)

    results: list[tuple[float, str, float, int, float]] = []
    dim_mismatch = 0
    total_sq = total_abs = total_n = total_diff = 0
    for name in sorted(py_names & rs_names):
        a, b = load(args.python_dir / name), load(args.rust_dir / name)
        if a.shape != b.shape:
            dim_mismatch += 1
            print(f"  [DIMENSJON] {name}: python {a.shape[1]}x{a.shape[0]}, rust {b.shape[1]}x{b.shape[0]}")
            continue
        d = np.abs(a - b)
        sq = float(np.sum(d.astype(np.int64) ** 2))
        total_sq += sq
        total_abs += float(np.sum(d))
        total_n += d.size
        total_diff += int(np.count_nonzero(d))
        results.append((psnr(sq / d.size), name, float(d.mean()), int(d.max()), np.count_nonzero(d) / d.size))
    failures += dim_mismatch

    if not results:
        print("ingen sammenlignbare filer")
        return 1
    psnrs = np.array([r[0] for r in results])
    finite = psnrs[np.isfinite(psnrs)]
    below = [r for r in results if r[0] < args.min_psnr]
    failures += len(below)
    print(f"dimensjoner: {len(results)} like, {dim_mismatch} ulike")
    print(
        f"samlet: snitt |avvik| {total_abs / total_n:.4f}, maks {max(r[3] for r in results)}, "
        f"ulike verdier {100 * total_diff / total_n:.3f} %, PSNR {psnr(total_sq / total_n):.2f} dB"
    )
    print(
        f"PSNR per fil: min {psnrs.min():.2f}, median {np.median(psnrs):.2f} dB; "
        f"identiske filer (∞) {int(np.sum(~np.isfinite(psnrs)))}/{len(results)}"
        + (f"; endelige: snitt {finite.mean():.2f} dB" if finite.size else "")
    )
    for size in ("1024", "300"):
        sub = [r for r in results if r[1].endswith(f"_{size}.jpg")]
        if sub:
            p = np.array([r[0] for r in sub])
            print(
                f"  {size:>4}: {len(sub)} filer, PSNR min {p.min():.2f} median {np.median(p):.2f} dB, "
                f"maks avvik {max(r[3] for r in sub)}"
            )
    print(f"under {args.min_psnr:g} dB: {len(below)}")

    srcs = sources_by_hash(args.sources) if args.sources else {}
    print(f"dårligste {args.worst}:")
    for p, name, mean, mx, frac in sorted(results)[: args.worst]:
        src = srcs.get(name.rsplit("_", 1)[0], "")
        print(f"  {p:7.2f} dB  snitt {mean:.4f}  maks {mx:3d}  ulike {100 * frac:6.2f} %  {name}  {src}")

    print("OK" if failures == 0 else f"{failures} avvik")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
