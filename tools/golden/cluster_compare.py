#!/usr/bin/env python3
"""Klynger fasit-embeddings (Python) og Rust-embeddings med master-versjonens egen
klyngekode, og rapporterer om personene blir de samme.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/cluster_compare.py \\
        golden/faces.jsonl rust-embeddings.jsonl [min_cluster_size] [threshold]

1. Full klynging (HDBSCAN som ``fotomaxxer.faces._hdbscan``) på begge sett → ARI og
   antall ansikter som havner i en annen klynge.
2. Inkrementell tilordning: Python-klyngenes sentroider holdes fast (som eksisterende
   personer i en ekte database); hvert Rust-ansikt tilordnes nærmeste sentroide med
   samme terskel som ``faces.cluster`` → andel som lander hos samme person som før.
"""

from __future__ import annotations

import base64
import json
import sys

import numpy as np
from sklearn.metrics import adjusted_rand_score

from fotomaxxer.faces import _centroid, _hdbscan


def emb(b64: str) -> np.ndarray:
    return np.frombuffer(base64.b64decode(b64), dtype="<f4")


def main() -> int:
    golden_path, rust_path = sys.argv[1], sys.argv[2]
    min_cluster_size = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    threshold = float(sys.argv[4]) if len(sys.argv) > 4 else 0.5

    py: dict[tuple[str, int], np.ndarray] = {}
    for line in open(golden_path, encoding="utf-8"):
        g = json.loads(line)
        for i, f in enumerate(g["faces"]):
            py[(g["hash"], i)] = emb(f["embedding"])
    rs = {}
    for line in open(rust_path, encoding="utf-8"):
        r = json.loads(line)
        rs[(r["hash"], r["index"])] = emb(r["embedding"])

    keys = sorted(k for k in py if k in rs)
    P = np.stack([py[k] for k in keys])
    R = np.stack([rs[k] for k in keys])
    print(f"ansikter i begge sett: {len(keys)} (kun python: {len(py) - len(keys)})")

    lp = _hdbscan(P, min_cluster_size)
    lr = _hdbscan(R, min_cluster_size)
    print(
        f"full klynging: klynger python={len(set(lp) - {-1})} rust={len(set(lr) - {-1})} "
        f"støy python={int((lp == -1).sum())} rust={int((lr == -1).sum())} "
        f"ARI={adjusted_rand_score(lp, lr):.4f}"
    )

    # Inkrementell: Python-klyngene er «eksisterende personer».
    cids = sorted(set(lp) - {-1})
    C = np.stack([_centroid(P[lp == c]) for c in cids])

    def assign(X: np.ndarray) -> np.ndarray:
        sims = X @ C.T
        best = sims.argmax(axis=1)
        return np.where(sims.max(axis=1) >= threshold, np.array(cids)[best], -1)

    ap, ar = assign(P), assign(R)
    same = int((ap == ar).sum())
    print(f"inkrementell tilordning mot python-sentroider: samme person {same}/{len(keys)}")
    for k, a, b in zip(keys, ap, ar):
        if a != b:
            i = keys.index(k)
            print(f"  ulik: {k[0][:12]}#{k[1]} python→{a} rust→{b} cos={float(P[i] @ R[i]):.4f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
