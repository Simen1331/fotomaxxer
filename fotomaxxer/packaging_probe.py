"""Reell scan/derive/faces mot et generert bibliotek, aldri brukerens database."""

from __future__ import annotations

import hashlib
import json
import platform
import shutil
import sys
import time
import traceback
from dataclasses import asdict
from pathlib import Path

import numpy as np
import pillow_heif
from PIL import Image, ImageEnhance

from . import db, sources
from .derive import derive
from .exif import ExifReader
from .faces import cluster, detect
from .resources import resource_root
from .scan import scan_source
from .storage import Storage


class _OfflineGeocoder:
    def lookup(self, lat: float | None, lon: float | None) -> None:
        return None


def _sha(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _fixtures(root: Path, portrait: Path) -> dict[str, tuple[int, int] | None]:
    root.mkdir()
    expected: dict[str, tuple[int, int] | None] = {}
    exif = Image.Exif()
    exif[274] = 6
    exif[36867] = "2020:06:15 14:30:00"
    Image.new("RGB", (1200, 600), "red").save(root / "rotert.jpg", exif=exif)
    expected["rotert.jpg"] = (600, 1200)
    shutil.copy2(root / "rotert.jpg", root / "duplikat.jpg")
    Image.new("RGB", (1600, 800), "blue").save(root / "bred.png")
    expected["bred.png"] = (1600, 800)
    for name, size in (("test.heic", (900, 450)), ("test.heif", (450, 900))):
        pillow_heif.from_pillow(Image.new("RGB", size, "green")).save(root / name)
        expected[name] = size
    (root / "korrupt.jpg").write_bytes(b"FotoMaxxer: dette er ikke et bilde\n")
    expected["korrupt.jpg"] = None
    with Image.open(portrait) as image:
        image = image.convert("RGB")
        for i, brightness in enumerate((0.8, 0.9, 1.0, 1.1)):
            name = f"ansikt-{i}.png"
            ImageEnhance.Brightness(image).enhance(brightness).save(root / name)
            expected[name] = image.size
    return expected


def run(output: str, face_image: str, workers: int) -> int:
    root = Path(output).expanduser().resolve()
    portrait = Path(face_image).expanduser().resolve()
    if not portrait.is_file() or workers < 1:
        print("Feil: angi et lesbart ansiktsbilde og minst én arbeider.", file=sys.stderr)
        return 2
    # Avvis også eksisterende mapper: dette skal aldri åpne et gammelt bibliotek.
    try:
        root.mkdir(parents=True, exist_ok=False)
    except FileExistsError:
        print(f"Feil: testmappa finnes allerede: {root}", file=sys.stderr)
        return 2
    report: dict = {
        "frozen": bool(getattr(sys, "frozen", False)),
        "python": platform.python_version(), "platform": platform.platform(),
        "face_fixture_sha256": _sha(portrait), "checks": [], "stages": {},
    }
    started = time.monotonic()
    conn = None

    def check(ok: bool, label: str) -> None:
        report["checks"].append({"ok": bool(ok), "label": label})
        print(f"[{'PASS' if ok else 'FAIL'}] {label}", flush=True)

    try:
        manifest_path = resource_root() / "resource-manifest.json"
        if report["frozen"]:
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            check(all(_sha(resource_root() / name) == info["sha256"]
                      for name, info in manifest["files"].items()), "Pakkede ressursers sjekksummer")
            report["build"] = {k: v for k, v in manifest.items() if k != "files"}

        src = root / "kilder med æøå"
        expected = _fixtures(src, portrait)
        before = {p.name: _sha(p) for p in src.iterdir()}
        store = Storage(root / "bibliotek med æøå")
        store.ensure_dirs()
        conn = db.connect(store.db_path)
        db.init_db(conn)
        source = sources.add(conn, str(src))
        with ExifReader() as exif:
            report["exiftool_version"] = exif._helper.version
            first = scan_source(conn, source, exif, _OfflineGeocoder())
            again = scan_source(conn, source, exif, _OfflineGeocoder())
        report["stages"]["scan"] = asdict(first)
        check(first.new_images == len(expected) and first.duplicates == 1 and first.errors == 0,
              "Scan og deduplisering")
        check(again.new_images == 0 and again.hashed == 0 and again.fast_path == len(before),
              "Idempotent scan med stat-cache")
        row = conn.execute("SELECT year, season FROM images WHERE content_hash = ?",
                           (before["rotert.jpg"],)).fetchone()
        check(row["year"] == 2020 and row["season"] == "sommer", "EXIF fra pakket ExifTool")

        derived = derive(conn, store, workers=workers)
        report["stages"]["derive"] = asdict(derived)
        check(derived.done == len(expected) - 1 and derived.errors == 1,
              "Prosesspool: JPEG, PNG, HEIC/HEIF og isolert korrupt fil")
        thumbs_ok = True
        for name, dimensions in expected.items():
            if dimensions is None:
                continue
            for size in (1024, 300):
                path = store.derived_path(before[name], str(size))
                if not path.is_file():
                    thumbs_ok = False
                    continue
                with Image.open(path) as thumb:
                    reference = Image.new("RGB", dimensions)
                    reference.thumbnail((size, size))
                    thumbs_ok &= thumb.size == reference.size and thumb.format == "JPEG"
        check(thumbs_ok, "Thumbnail-størrelser og EXIF-rotasjon")
        check(derive(conn, store, workers=workers).pending == 0, "Idempotent derive")

        detected = detect(conn, store, workers=workers)
        report["stages"]["detect"] = asdict(detected)
        check(detected.done == len(expected) - 1 and detected.errors == 0,
              "Offline InsightFace/ONNX/OpenCV i arbeidsprosesser")
        portraits_ok = all(conn.execute(
            "SELECT COUNT(*) FROM faces f JOIN images i ON i.id=f.image_id WHERE i.content_hash=?",
            (before[f"ansikt-{i}.png"],),
        ).fetchone()[0] >= 1 for i in range(4))
        check(portraits_ok, "Ansikt funnet i alle fire testportretter")
        faces = conn.execute("SELECT embedding FROM faces").fetchall()
        check(bool(faces) and all(
            len(r["embedding"]) == 512 * 4 and
            np.isclose(np.linalg.norm(np.frombuffer(r["embedding"], dtype=np.float32)), 1.0, atol=1e-4)
            for r in faces), "512-dimensjonale normaliserte embeddings")
        clustered = cluster(conn)
        report["stages"]["cluster"] = asdict(clustered)
        check(clustered.faces_total >= 4, "scikit-learn HDBSCAN på detekterte ansikter")
        check(detect(conn, store, workers=workers).pending == 0, "Idempotent ansiktsdeteksjon")
        check({p.name: _sha(p) for p in src.iterdir()} == before, "Originalfiler er uendret")
        check(conn.execute("PRAGMA integrity_check").fetchone()[0] == "ok", "SQLite-integritet")
    except Exception:
        report["error"] = traceback.format_exc()
        print(report["error"], file=sys.stderr)
    finally:
        if conn is not None:
            conn.close()
        report["seconds"] = round(time.monotonic() - started, 2)
        report["ok"] = "error" not in report and all(c["ok"] for c in report["checks"])
        path = root / "report.json"
        path.write_text(json.dumps(report, indent=2, ensure_ascii=False), encoding="utf-8")
        print(f"Rapport: {path}", flush=True)
    return 0 if report["ok"] else 1
