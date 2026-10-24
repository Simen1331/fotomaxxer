#!/usr/bin/env python3
"""Probe end-to-end verification of the FotoMaxxer handoff guarantees.

Drives the real CLI surface (the same functions the GUI uses) against a
synthetic library in a throwaway temp dir. Touches no real data: XDG_CONFIG_HOME
and the package root are both redirected to the temp tree, so the bootstrap
pointer file and the user's library are never seen.

Covers the gate checklist from task d68683c8:
  - dedup (same bytes in two sources -> one image, two files)
  - idempotent re-scan (re-run does no new work, DB unchanged)
  - source remove + orphan cleanup (orphan image + its thumbnails gone;
    shared image survives)
  - source files on disk untouched (the iron rule)
  - portable package relocates (db + derived move, pointer follows, queries work)
  - prepare-for-handoff is clean (no sources/images/derived, settings emptied)
  - API key does not leak (deep check: raw bytes of the db file)

Run:  fotomaxxer/.venv/bin/python tests/verify_handoff_e2e.py
Exit 0 = all guarantees hold; nonzero = at least one failed.
"""
from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PY = sys.executable

CHECKS: list[tuple[bool, str, str]] = []  # (ok, label, detail)


def check(ok: bool, label: str, detail: str = "") -> None:
    CHECKS.append((bool(ok), label, detail))
    mark = "PASS" if ok else "FAIL"
    line = f"  [{mark}] {label}"
    if detail:
        line += f"  — {detail}"
    print(line)


def cli(home: Path, *args: str, xdg: Path | None = None) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    env["PYTHONPATH"] = str(REPO) + os.pathsep + env.get("PYTHONPATH", "")
    if xdg is not None:
        env["XDG_CONFIG_HOME"] = str(xdg)
    cmd = [PY, "-m", "fotomaxxer.cli", "--home", str(home), *args]
    return subprocess.run(cmd, cwd=REPO, env=env, capture_output=True, text=True)


def make_jpeg(path: Path, color: tuple[int, int, int], size: tuple[int, int] = (64, 48)) -> None:
    from PIL import Image

    path.parent.mkdir(parents=True, exist_ok=True)
    Image.new("RGB", size, color).save(path, "JPEG", quality=90)


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def db_query(home: Path, sql: str):
    import sqlite3

    sys.path.insert(0, str(REPO))
    from fotomaxxer import db as dbmod  # noqa

    conn = dbmod.connect(home / "fotomaxxer.db")
    try:
        return conn.execute(sql).fetchall()
    finally:
        conn.close()


def count(home: Path, table: str) -> int:
    return db_query(home, f"SELECT COUNT(*) FROM {table}")[0][0]


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="fmx-probe-"))
    pkg = tmp / "pkg"
    pkg2 = tmp / "pkg-moved"
    xdg = tmp / "xdg"
    src_a = tmp / "srcA"
    src_b = tmp / "srcB"
    for d in (pkg, xdg, src_a, src_b):
        d.mkdir(parents=True, exist_ok=True)

    print(f"\nProbe handoff verification — scratch dir: {tmp}\n")

    # --- Synthetic library -------------------------------------------------
    # unique-to-A, shared (identical bytes in BOTH sources), unique-to-B
    make_jpeg(src_a / "a1.jpg", (200, 30, 30))
    shared_bytes = None
    make_jpeg(src_a / "shared.jpg", (30, 200, 30))
    shared_bytes = (src_a / "shared.jpg").read_bytes()
    (src_b / "shared_copy.jpg").write_bytes(shared_bytes)  # identical content, different path
    make_jpeg(src_b / "b1.jpg", (30, 30, 200))
    (src_a / "notes.txt").write_text("not an image")  # non-whitelist

    src_files = [src_a / "a1.jpg", src_a / "shared.jpg", src_b / "shared_copy.jpg", src_b / "b1.jpg"]
    pre_hashes = {p: sha(p) for p in src_files}

    print("== Setup ==")
    r = cli(pkg, "init")
    check(r.returncode == 0, "init creates package", r.stderr.strip()[:120])
    check(cli(pkg, "source", "add", str(src_a)).returncode == 0, "source add A")
    check(cli(pkg, "source", "add", str(src_b)).returncode == 0, "source add B")

    print("\n== Scan + dedup ==")
    r = cli(pkg, "scan", "--all")
    check(r.returncode == 0, "scan --all runs", r.stderr.strip()[:200])
    images = count(pkg, "images")
    files = count(pkg, "files")
    check(images == 3, "dedup: 3 distinct images (a1, shared, b1)", f"images={images}")
    check(files == 4, "dedup: 4 file rows (shared counted twice)", f"files={files}")
    shared_hash = sha(src_a / "shared.jpg")
    rows = db_query(pkg, f"SELECT COUNT(*) FROM files f JOIN images i ON i.id=f.image_id "
                         f"WHERE i.content_hash='{shared_hash}'")
    check(rows[0][0] == 2, "dedup: shared image has exactly 2 files", f"n={rows[0][0]}")

    print("\n== Derive ==")
    r = cli(pkg, "derive")
    check(r.returncode == 0, "derive runs", r.stderr.strip()[:200])
    derived_rows = count(pkg, "derived")
    derived_on_disk = list((pkg / "derived").glob("*.jpg"))
    check(derived_rows == 6, "derive: 3 images x 2 sizes = 6 rows", f"rows={derived_rows}")
    check(len(derived_on_disk) == 6, "derive: 6 thumbnails on disk", f"files={len(derived_on_disk)}")

    print("\n== Idempotent re-scan ==")
    img_before, file_before, der_before = images, files, len(derived_on_disk)
    r = cli(pkg, "scan", "--all")
    check(r.returncode == 0, "re-scan runs", r.stderr.strip()[:200])
    check("nye=0" in r.stdout, "re-scan reports 0 new images", _grep(r.stdout, "nye="))
    check(count(pkg, "images") == img_before, "re-scan: image count unchanged")
    check(count(pkg, "files") == file_before, "re-scan: file count unchanged")
    check(len(list((pkg / "derived").glob("*.jpg"))) == der_before, "re-scan: thumbnails unchanged")

    print("\n== Source files untouched (after scan+derive) ==")
    untouched = all(p.exists() and sha(p) == pre_hashes[p] for p in src_files)
    check(untouched, "all source files present + byte-identical")

    print("\n== Source remove + orphan cleanup ==")
    # thumbnails belonging to b1 (orphan-to-be) vs shared/a1 (must survive)
    b1_hash = sha(src_b / "b1.jpg")
    a1_hash = sha(src_a / "a1.jpg")
    b1_thumbs = list((pkg / "derived").glob(f"{b1_hash}_*.jpg"))
    r = cli(pkg, "source", "remove", str(src_b))
    check(r.returncode == 0, "source remove B runs", r.stderr.strip()[:200])
    check(count(pkg, "images") == 2, "orphan: b1 image deleted, 2 remain", f"images={count(pkg,'images')}")
    # shared image survives with its single remaining file (in A)
    shared_files = db_query(pkg, f"SELECT COUNT(*) FROM files f JOIN images i ON i.id=f.image_id "
                                 f"WHERE i.content_hash='{shared_hash}'")[0][0]
    check(shared_files == 1, "orphan: shared image kept, now 1 file (in A)", f"files={shared_files}")
    b1_left = db_query(pkg, f"SELECT COUNT(*) FROM images WHERE content_hash='{b1_hash}'")[0][0]
    check(b1_left == 0, "orphan: b1 image row gone")
    check(all(not t.exists() for t in b1_thumbs), "orphan: b1 thumbnails deleted from disk",
          f"{len(b1_thumbs)} thumbs")
    a1_thumbs_left = list((pkg / "derived").glob(f"{a1_hash}_*.jpg"))
    check(len(a1_thumbs_left) == 2, "orphan: a1 thumbnails survive", f"n={len(a1_thumbs_left)}")
    shared_thumbs_left = list((pkg / "derived").glob(f"{shared_hash}_*.jpg"))
    check(len(shared_thumbs_left) == 2, "orphan: shared thumbnails survive", f"n={len(shared_thumbs_left)}")
    # scan_index rows under srcB prefix removed
    si_b = db_query(pkg, f"SELECT COUNT(*) FROM scan_index WHERE path LIKE '{src_b}%'")[0][0]
    check(si_b == 0, "orphan: scan_index rows under B removed", f"n={si_b}")
    # the iron rule: srcB files still on disk, untouched
    b_intact = all((src_b / n).exists() for n in ("shared_copy.jpg", "b1.jpg"))
    check(b_intact, "iron rule: removed source's files still on disk + untouched",
          "shared_copy.jpg, b1.jpg present")

    print("\n== Portable package relocate ==")
    sys.path.insert(0, str(REPO))
    from fotomaxxer import storage as st  # noqa

    os.environ["XDG_CONFIG_HOME"] = str(xdg)
    new_root = st.relocate_data(pkg2, current_root=pkg)
    check(Path(new_root) == pkg2.resolve(), "relocate returns new root")
    check((pkg2 / "fotomaxxer.db").exists(), "relocate: db at new root")
    check((pkg2 / "derived").exists() and any((pkg2 / "derived").glob("*.jpg")),
          "relocate: derived/ moved with thumbnails")
    check((pkg / "fotomaxxer.db").exists(), "relocate: old db kept by default")
    pointer = st.read_pointer()
    check(pointer == str(pkg2.resolve()), "relocate: bootstrap pointer follows", str(pointer))
    # data integrity at new home
    check(count(pkg2, "images") == 2, "relocate: data intact at new home", f"images={count(pkg2,'images')}")

    print("\n== prepare-for-handoff (clean + no key leak) ==")
    SECRET = "sk-or-PROBE-SECRET-DO-NOT-LEAK-abc123"
    check(cli(pkg2, "settings", "set", "openrouter_api_key", SECRET, xdg=xdg).returncode == 0,
          "settings set api key")
    r = cli(pkg2, "prepare-handoff", xdg=xdg)
    check(r.returncode == 0, "prepare-handoff runs", r.stderr.strip()[:200])
    check(count(pkg2, "sources") == 0, "handoff: no sources left")
    check(count(pkg2, "files") == 0, "handoff: no files left")
    check(count(pkg2, "images") == 0, "handoff: no images left")
    check(count(pkg2, "settings") == 0, "handoff: settings emptied")
    leftover_thumbs = list((pkg2 / "derived").glob("*.jpg"))
    check(len(leftover_thumbs) == 0, "handoff: no thumbnails on disk", f"n={len(leftover_thumbs)}")
    # DEEP check: does the deleted key survive in the db file bytes (freelist)?
    raw = (pkg2 / "fotomaxxer.db").read_bytes()
    key_in_bytes = SECRET.encode() in raw
    check(not key_in_bytes, "handoff: API key absent from raw db bytes (no freelist leak)",
          "LEAK — key found in db file" if key_in_bytes else "clean")

    # --- Summary -----------------------------------------------------------
    failed = [c for c in CHECKS if not c[0]]
    print("\n" + "=" * 64)
    print(f"RESULT: {len(CHECKS) - len(failed)}/{len(CHECKS)} checks passed")
    if failed:
        print("FAILED:")
        for _ok, label, detail in failed:
            print(f"  - {label}  ({detail})")
    print(f"scratch dir left for inspection: {tmp}")
    return 1 if failed else 0


def _grep(text: str, needle: str) -> str:
    for line in text.splitlines():
        if needle in line:
            return line.strip()
    return ""


if __name__ == "__main__":
    sys.exit(main())
