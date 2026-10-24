#!/usr/bin/env python3
"""Fasit for ansikter: kjør insightface (master-versjonen) på 1024-derivatene.

Leser en Python-bygd datamappe (``fotomaxxer.db`` + ``derived/``) og skriver én
JSON-linje per bilde med rå deteksjoner i derivatkoordinater: bbox, 5 nøkkelpunkter,
score og L2-normalisert embedding (base64 float32 LE). Rust-spiken sammenligner mot
denne fila. Kjøres med master-worktreens venv:

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/faces_golden.py \\
        ~/dev/fotomaxxer-testlib/py-golden ~/dev/fotomaxxer-testlib/golden/faces.jsonl
"""

from __future__ import annotations

import base64
import json
import sqlite3
import sys
from pathlib import Path

import cv2
import numpy as np
from insightface.app import FaceAnalysis


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    root, out_path = Path(sys.argv[1]), Path(sys.argv[2])
    conn = sqlite3.connect(root / "fotomaxxer.db")
    rows = conn.execute(
        "SELECT i.id, i.content_hash, i.width, i.height FROM images i "
        "JOIN derived d ON d.image_id = i.id AND d.size = '1024' ORDER BY i.id"
    ).fetchall()

    # Samme oppsett som fotomaxxer.faces, men kun modellene koden faktisk bruker.
    app = FaceAnalysis(
        name="buffalo_l",
        providers=["CPUExecutionProvider"],
        allowed_modules=["detection", "recognition"],
    )
    app.prepare(ctx_id=-1, det_size=(640, 640))

    out_path.parent.mkdir(parents=True, exist_ok=True)
    total = 0
    with out_path.open("w", encoding="utf-8") as out:
        for image_id, content_hash, width, height in rows:
            derived = root / "derived" / f"{content_hash}_1024.jpg"
            img = cv2.imdecode(np.fromfile(derived, dtype=np.uint8), cv2.IMREAD_COLOR)
            if img is None:
                continue
            faces = []
            for face in app.get(img):
                emb = np.asarray(face.normed_embedding, dtype="<f4")
                faces.append({
                    "bbox": [float(v) for v in face.bbox],
                    "kps": [[float(x), float(y)] for x, y in face.kps],
                    "score": float(face.det_score),
                    "embedding": base64.b64encode(emb.tobytes()).decode("ascii"),
                })
            total += len(faces)
            out.write(json.dumps({
                "image_id": image_id,
                "hash": content_hash,
                "width": width,
                "height": height,
                "thumb": [int(img.shape[1]), int(img.shape[0])],
                "faces": faces,
            }) + "\n")
    print(f"{len(rows)} bilder, {total} ansikter → {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
