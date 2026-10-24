#!/usr/bin/env python3
"""Regression checks for the portable-package storage flow.

Exercises storage.resolve_root + relocate_data against throwaway temp dirs.
Touches no real data: XDG_CONFIG_HOME is redirected into the temp tree, so the
real bootstrap pointer (~/.config/fotomaxxer/config.json) and the user's library
are never seen.

Covers:
  - resolve_root precedence: explicit > FOTOMAXXER_HOME > pointer > cwd
  - relocate_data copies db + derived and points the pointer at the new root
  - relocate_data KEEPS the old folder by default (the safety invariant)
  - relocate_data(delete_source=True) removes the old db + derived
  - dst == src is a no-op (no data destroyed)
  - failed copy rolls back and leaves pointer + source untouched

Run:  .venv/bin/python tests/verify_storage_relocate.py
Exit 0 = all invariants hold; nonzero = at least one failed.
"""
from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

CHECKS: list[tuple[bool, str, str]] = []  # (ok, label, detail)


def check(ok: bool, label: str, detail: str = "") -> None:
    CHECKS.append((bool(ok), label, detail))
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f"  ({detail})" if detail and not ok else ""))


def make_root(path: Path, marker: str) -> Path:
    """Create a package root with a db + one derived thumbnail."""
    from fotomaxxer import storage

    path.mkdir(parents=True, exist_ok=True)
    (path / storage.DB_FILENAME).write_text(marker, encoding="utf-8")
    (path / storage.DERIVED_DIRNAME).mkdir(exist_ok=True)
    (path / storage.DERIVED_DIRNAME / "h_300.jpg").write_text("thumb", encoding="utf-8")
    return path


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="fmx-storage-"))
    # Isolate: own XDG_CONFIG_HOME so the real pointer file is never touched.
    os.environ["XDG_CONFIG_HOME"] = str(tmp / "cfg")
    os.environ.pop("FOTOMAXXER_HOME", None)

    from fotomaxxer import storage

    check(str(tmp) in str(storage.config_path()), "pointer file is isolated to temp tree")

    # --- resolve_root precedence ------------------------------------------
    src = make_root(tmp / "data-a", "DB-A")
    os.chdir(src)
    check(storage.resolve_root() == src.resolve(), "resolve_root falls back to cwd")

    # --- relocate keeps source by default ---------------------------------
    dst = tmp / "data-b"
    res = storage.relocate_data(dst, current_root=src)
    check(res == dst.resolve(), "relocate returns the new root")
    check((dst / storage.DB_FILENAME).read_text() == "DB-A", "db copied to new root")
    check((dst / storage.DERIVED_DIRNAME / "h_300.jpg").exists(), "derived copied to new root")
    check((src / storage.DB_FILENAME).exists(), "old db KEPT by default (safety invariant)")
    check((src / storage.DERIVED_DIRNAME).exists(), "old derived KEPT by default")
    check(storage.read_pointer() == str(dst.resolve()), "pointer updated to new root")
    check(storage.resolve_root() == dst.resolve(), "resolve_root follows the pointer")

    # --- precedence: explicit > env > pointer -----------------------------
    check(storage.resolve_root(src) == src.resolve(), "explicit arg wins over pointer")
    os.environ["FOTOMAXXER_HOME"] = str(src)
    check(storage.resolve_root() == src.resolve(), "FOTOMAXXER_HOME wins over pointer")
    os.environ.pop("FOTOMAXXER_HOME")

    # --- relocate with delete_source=True ---------------------------------
    dst2 = tmp / "data-c"
    storage.relocate_data(dst2, current_root=dst, delete_source=True)
    check(not (dst / storage.DB_FILENAME).exists(), "old db removed when delete_source=True")
    check(not (dst / storage.DERIVED_DIRNAME).exists(), "old derived removed when delete_source=True")
    check((dst2 / storage.DB_FILENAME).read_text() == "DB-A", "data intact after delete-move")

    # --- no-op when dst == src --------------------------------------------
    same = storage.relocate_data(dst2, current_root=dst2, delete_source=True)
    check(same == dst2.resolve() and (dst2 / storage.DB_FILENAME).exists(),
          "dst == src is a no-op (delete_source ignored)")

    # --- failed copy rolls back, leaves pointer + source untouched --------
    blocker = tmp / "blocked"
    blocker.write_text("not a dir", encoding="utf-8")  # mkdir(new_dir) under a file -> fails
    before = storage.read_pointer()
    failed = False
    try:
        storage.relocate_data(blocker / "child", current_root=dst2, delete_source=True)
    except OSError:
        failed = True
    check(failed, "relocate raises on copy failure")
    check(storage.read_pointer() == before, "pointer untouched after failed relocate")
    check((dst2 / storage.DB_FILENAME).exists(), "source intact after failed relocate")

    # --- summary -----------------------------------------------------------
    os.chdir("/")
    bad = [c for c in CHECKS if not c[0]]
    print("\n" + "=" * 64)
    print(f"RESULT: {len(CHECKS) - len(bad)}/{len(CHECKS)} checks passed")
    if bad:
        for _ok, label, detail in bad:
            print(f"  - {label}  ({detail})")
    print(f"scratch dir left for inspection: {tmp}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
