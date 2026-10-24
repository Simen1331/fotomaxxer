#!/usr/bin/env python3
"""Fasit for klyngingen i Rust (`crates/faces`): HDBSCAN-etiketter, numpy-argsort og
float32-sentroider/likheter, regnet med master-versjonens egen kode (sklearn/numpy).

To bruksmåter (kjøres med master-worktreens venv):

    # Syntetiske fiksturer som committes (ingen private data):
    ~/dev/fotomaxxer/.venv/bin/python tools/golden/hdbscan_golden.py fixtures \\
        crates/faces/tests/fixtures

    # Fasit for de private ansikts-embeddingene (skrives utenfor repoet):
    ~/dev/fotomaxxer/.venv/bin/python tools/golden/hdbscan_golden.py faces \\
        ~/dev/fotomaxxer-testlib/golden/faces.jsonl \\
        ~/dev/fotomaxxer-testlib/golden/hdbscan.json

Rust-siden sammenligner med `cargo test -p fotomaxxer-faces` (fiksturene) og
`cargo run --release -p fotomaxxer-faces --example hdbscan_golden -- <faces.jsonl> <hdbscan.json>`.
"""

from __future__ import annotations

import base64
import json
import sys
from pathlib import Path

import numpy as np

from fotomaxxer.faces import _centroid, _hdbscan


def f32_bits(v: np.ndarray) -> str:
    return base64.b64encode(np.asarray(v, dtype="<f4").tobytes()).decode("ascii")


def labels(X: np.ndarray, mcs: int) -> list[int]:
    return [int(v) for v in _hdbscan(X, mcs)]


def blobs(rng: np.random.Generator, n: int, dim: int, centers: int, spread: float, grid: int) -> np.ndarray:
    """Gaussiske klumper rundet til et 1/grid-gitter (eksakt i float32, mange like avstander)."""
    c = rng.uniform(-4, 4, size=(centers, dim))
    which = rng.integers(0, centers, n)
    X = c[which] + rng.normal(0, spread, size=(n, dim))
    return (np.round(X * grid) / grid).astype(np.float32)


def fixtures(out_dir: Path) -> None:
    rng = np.random.default_rng(2026)
    out_dir.mkdir(parents=True, exist_ok=True)

    # numpy-argsort med mange like verdier, også lengder over 256 (partisjonering).
    sorts = []
    for n in [2, 3, 4, 5, 7, 8, 9, 16, 31, 64, 100, 255, 256, 257, 258, 300, 511, 648, 1100]:
        for levels in sorted({1 + n // 40, 1 + n // 4} | ({n} if n <= 256 else set())):
            ints = rng.integers(0, levels, n)
            a = ints.astype(np.float64) * 0.37
            sorts.append({"ints": ints.tolist(), "argsort": np.argsort(a).tolist()})
    (out_dir / "npsort.json").write_text(json.dumps(sorts), encoding="utf-8")

    # HDBSCAN på syntetiske sett: grove gitre (mange like avstander) og finere data.
    sets = []
    specs = [
        (60, 2, 4, 0.6, 2),
        (150, 3, 6, 0.5, 4),
        (300, 2, 8, 0.7, 4),
        (420, 8, 10, 0.4, 16),
        (600, 4, 12, 0.6, 8),
    ]
    for n, dim, centers, spread, grid in specs:
        X = blobs(rng, n, dim, centers, spread, grid)
        sets.append({
            "dim": dim,
            "grid": grid,
            "points": (X * grid).round().astype(int).ravel().tolist(),
            "labels": {str(m): labels(X, m) for m in (2, 3, 4, 5, 8)},
        })
    (out_dir / "hdbscan.json").write_text(json.dumps(sets), encoding="utf-8")

    # float32-sentroider og likheter (OpenBLAS sdot) på normaliserte tilfeldige vektorer.
    vecs = rng.normal(size=(16, 512)).astype(np.float32)
    vecs /= np.linalg.norm(vecs, axis=1, keepdims=True)
    groups = [[0], [1, 2], list(range(3, 8)), list(range(4, 16))]
    cents = [_centroid(vecs[g]) for g in groups]
    linalg = {
        "vectors": f32_bits(vecs),
        "groups": groups,
        "centroids": [f32_bits(c) for c in cents],
        "sims": [[float(np.float32(v @ c)) for c in cents] for v in vecs],
    }
    (out_dir / "linalg.json").write_text(json.dumps(linalg), encoding="utf-8")
    print(f"skrev fiksturer til {out_dir}")


def faces(jsonl: Path, out_path: Path) -> None:
    embs = []
    for line in jsonl.open(encoding="utf-8"):
        for f in json.loads(line)["faces"]:
            embs.append(np.frombuffer(base64.b64decode(f["embedding"]), dtype="<f4"))
    E = np.stack(embs)
    rng = np.random.default_rng(7)
    cases = []
    for m in (2, 3, 4, 5, 6):
        cases.append({"indices": None, "mcs": m, "labels": labels(E, m)})
    # Delmengder og stokket rekkefølge: mange ulike konstellasjoner av like avstander.
    for t in range(24):
        size = int(rng.integers(20, len(E) + 1))
        idx = rng.permutation(len(E))[:size].tolist()
        m = int(rng.integers(2, 6))
        cases.append({"indices": idx, "mcs": m, "labels": labels(E[idx], m)})

    lab = np.array(labels(E, 3))
    cids = sorted(set(lab.tolist()) - {-1})
    cents = [_centroid(E[lab == c]) for c in cids]
    out = {
        "count": len(E),
        "cases": cases,
        "centroid_labels": lab.tolist(),
        "centroids": [f32_bits(c) for c in cents],
        "sims": [f32_bits(np.array([np.float32(e @ c) for c in cents])) for e in E],
    }
    out_path.write_text(json.dumps(out), encoding="utf-8")
    print(f"{len(E)} embeddings, {len(cases)} tilfeller → {out_path}")


def main() -> int:
    if len(sys.argv) >= 3 and sys.argv[1] == "fixtures":
        fixtures(Path(sys.argv[2]))
    elif len(sys.argv) >= 4 and sys.argv[1] == "faces":
        faces(Path(sys.argv[2]), Path(sys.argv[3]))
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
