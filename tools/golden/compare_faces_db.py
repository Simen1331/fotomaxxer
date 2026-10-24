#!/usr/bin/env python3
"""Sammenlign ansiktsresultatet i to databaser (Python vs Rust) etter
``faces detect`` + ``faces cluster`` på de samme 1024-derivatene.

Embeddingene kan ikke bli bit-like (JPEG-dekoder og ONNX-runtime er forskjellige,
se fase 0 i docs/rust-port-plan.md), så sammenligningen er tolerant og måler det som
betyr noe — om personene blir de samme:

1. Antall ansikter per bilde, og hvilke bilder som avviker.
2. Ansikter matches per bilde på bbox-overlapp (IoU ≥ 0,5): bbox-avvik i
   originalkoordinater (piksler) og cosinus mellom embeddingene.
3. Klyngene slik de ble lagret i hver database: antall personer, ARI over matchede
   ansikter (støy = −1) og hvor mange som havner hos «samme» person (flertallsmatch).
4. Inkrementell tilordning: Python-personenes sentroider holdes fast (som eksisterende
   personer i en ekte database), og hvert matchet ansikt tilordnes nærmeste sentroide
   med terskel 0,5 — andel som lander hos samme person for Python- og Rust-embedding.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/compare_faces_db.py python.db rust.db
"""

from __future__ import annotations

import json
import sqlite3
import sys
from collections import Counter, defaultdict

import numpy as np
from sklearn.metrics import adjusted_rand_score

from fotomaxxer.faces import _centroid


def load(path: str):
    conn = sqlite3.connect(path)
    faces = defaultdict(list)
    for fid, image_id, bbox, emb, cid in conn.execute(
        "SELECT id, image_id, bbox, embedding, cluster_id FROM faces ORDER BY id"
    ):
        faces[image_id].append({
            "id": fid,
            "bbox": json.loads(bbox),
            "emb": np.frombuffer(emb, dtype="<f4"),
            "cluster": -1 if cid is None else cid,
        })
    states = dict(conn.execute(
        "SELECT status, COUNT(*) FROM processing_state WHERE stage = 'faces' GROUP BY status"
    ).fetchall())
    images = [r[0] for r in conn.execute("SELECT id FROM images ORDER BY id")]
    persons = conn.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
    conn.close()
    return faces, states, images, persons


def iou(a, b) -> float:
    ax2, ay2, bx2, by2 = a[0] + a[2], a[1] + a[3], b[0] + b[2], b[1] + b[3]
    iw = max(0, min(ax2, bx2) - max(a[0], b[0]))
    ih = max(0, min(ay2, by2) - max(a[1], b[1]))
    inter = iw * ih
    union = a[2] * a[3] + b[2] * b[3] - inter
    return inter / union if union > 0 else 0.0


def pct(values, p):
    return float(np.percentile(values, p)) if len(values) else float("nan")


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    py, py_states, images, py_persons = load(sys.argv[1])
    rs, rs_states, _, rs_persons = load(sys.argv[2])
    n_py = sum(len(v) for v in py.values())
    n_rs = sum(len(v) for v in rs.values())
    print(f"processing_state faces: python={py_states} rust={rs_states}")
    print(f"ansikter: python={n_py} rust={n_rs}  personer: python={py_persons} rust={rs_persons}")

    count_diff = [(i, len(py[i]), len(rs[i])) for i in images if len(py[i]) != len(rs[i])]
    print(f"bilder med ulikt antall ansikter: {len(count_diff)} av {len(images)}")
    for image_id, a, b in count_diff[:10]:
        print(f"  bilde {image_id}: python={a} rust={b}")

    matched = []  # (py_face, rs_face)
    unmatched_py = unmatched_rs = 0
    for image_id in images:
        used = set()
        for pf in py[image_id]:
            best = max(
                ((j, iou(pf["bbox"], rf["bbox"])) for j, rf in enumerate(rs[image_id]) if j not in used),
                key=lambda t: t[1],
                default=(None, 0.0),
            )
            if best[0] is not None and best[1] >= 0.5:
                used.add(best[0])
                matched.append((pf, rs[image_id][best[0]]))
            else:
                unmatched_py += 1
        unmatched_rs += len(rs[image_id]) - len(used)
    print(f"matchet={len(matched)} kun python={unmatched_py} kun rust={unmatched_rs}")
    if not matched:
        return 1

    bbox_diff = np.array([max(abs(a - b) for a, b in zip(p["bbox"], r["bbox"])) for p, r in matched])
    cos = np.array([float(p["emb"] @ r["emb"]) for p, r in matched])
    print(
        f"bbox-avvik (originalpiksler): maks={bbox_diff.max():.0f} p99={pct(bbox_diff, 99):.0f} "
        f"median={np.median(bbox_diff):.0f} likt={int((bbox_diff == 0).sum())}"
    )
    print(
        f"cosinus python↔rust: min={cos.min():.4f} p1={pct(cos, 1):.4f} p5={pct(cos, 5):.4f} "
        f"median={np.median(cos):.4f} under 0,99: {int((cos < 0.99).sum())}"
    )

    # Klyngene slik de ble lagret.
    lp = np.array([p["cluster"] for p, _ in matched])
    lr = np.array([r["cluster"] for _, r in matched])
    print(
        f"lagrede klynger (matchede ansikter): python={len(set(lp) - {-1})} rust={len(set(lr) - {-1})} "
        f"støy python={int((lp == -1).sum())} rust={int((lr == -1).sum())} "
        f"ARI={adjusted_rand_score(lp, lr):.4f}"
    )
    # Flertallsmatch: hver Python-person ↔ den Rust-personen flest av ansiktene havnet hos.
    agree = 0
    for cid in set(lp) - {-1}:
        targets = Counter(lr[lp == cid])
        agree += targets.most_common(1)[0][1] if targets else 0
    clustered = int((lp != -1).sum())
    print(f"  ansikter i Python-personer som havner samlet hos samme Rust-person: {agree}/{clustered}")

    # Inkrementell tilordning mot faste Python-sentroider.
    all_py = [f for faces in py.values() for f in faces]
    cids = sorted({f["cluster"] for f in all_py} - {-1})
    if cids:
        C = np.stack([_centroid(np.stack([f["emb"] for f in all_py if f["cluster"] == c])) for c in cids])
        P = np.stack([p["emb"] for p, _ in matched])
        R = np.stack([r["emb"] for _, r in matched])

        def assign(X):
            sims = X @ C.T
            return np.where(sims.max(axis=1) >= 0.5, np.array(cids)[sims.argmax(axis=1)], -1)

        ap, ar = assign(P), assign(R)
        same = int((ap == ar).sum())
        print(f"inkrementell tilordning mot python-sentroider: samme person {same}/{len(matched)} "
              f"({100 * same / len(matched):.1f} %)")
        for k in np.nonzero(ap != ar)[0][:10]:
            print(f"  ulik: ansikt py#{matched[k][0]['id']} python→{ap[k]} rust→{ar[k]} cos={cos[k]:.4f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
