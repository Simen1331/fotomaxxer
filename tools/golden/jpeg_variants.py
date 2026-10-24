#!/usr/bin/env python3
"""Sjekk at Rust-dekodingen av JPEG gir nøyaktig samme piksler som Pillow.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/jpeg_variants.py \\
        --dump target/release/examples/decode_rgb [bilde ...]

Fra hvert kildebilde (standard: ``tests/fixtures/astronaut.png``) lages JPEG-varianter
med Pillow i en midlertidig mappe: alle subsamplinger (4:4:4, 4:2:2, 4:2:0, 4:1:1),
gråtone, progressiv, optimaliserte Huffman-tabeller, restart-markører, odde størrelser
og EXIF-orienteringer. Hver fil dekodes med Pillow (``exif_transpose`` + ``convert("RGB")``,
som i derive) og med Rust (``imageio::open_oriented`` via eksempelet ``decode_rgb``), og
pikslene sammenlignes bit for bit. Exit 0 = alle like.
"""

from __future__ import annotations

import argparse
import itertools
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageOps

ROOT = Path(__file__).resolve().parents[2]


def with_orientation(jpeg: Path, orientation: int) -> None:
    """Sett EXIF-orientering ved å skrive fila på nytt med samme kodede data."""
    with Image.open(jpeg) as im:
        exif = im.getexif()
        exif[0x0112] = orientation
        data = jpeg.read_bytes()
    # Sett inn APP1 rett etter SOI (Pillows lagring ville kodet bildet på nytt).
    raw = exif.tobytes()
    payload = raw if raw.startswith(b"Exif\x00\x00") else b"Exif\x00\x00" + raw
    app1 = b"\xff\xe1" + (len(payload) + 2).to_bytes(2, "big") + payload
    jpeg.write_bytes(data[:2] + app1 + data[2:])


def variants(src: Image.Image, out: Path) -> list[Path]:
    files = []
    sizes = [src.size, (src.width - 3, src.height - 5), (17, 9), (1, 1)]
    combos = itertools.product(
        sizes,
        ("4:4:4", "4:2:2", "4:2:0", "4:1:1", "L"),
        (False, True),  # progressiv
        (False, True),  # optimaliserte Huffman-tabeller
        (0, 3),  # restart-intervall i MCU-blokker
    )
    for (w, h), sub, prog, opt, rst in combos:
        im = src.convert("L" if sub == "L" else "RGB").resize((w, h), Image.LANCZOS)
        kw: dict = {"quality": 90, "progressive": prog, "optimize": opt}
        if sub != "L":
            kw["subsampling"] = sub
        if rst:
            kw["restart_marker_blocks"] = rst
        name = f"v_{w}x{h}_{sub.replace(':', '')}_{'p' if prog else 'b'}{'o' if opt else ''}_r{rst}.jpg"
        path = out / name
        im.save(path, "JPEG", **kw)
        files.append(path)
    for o in range(1, 9):
        path = out / f"orient{o}.jpg"
        src.convert("RGB").resize((61, 37)).save(path, "JPEG", quality=85)
        with_orientation(path, o)
        files.append(path)
    return files


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dump", required=True, help="target/release/examples/decode_rgb")
    ap.add_argument("images", nargs="*", type=Path)
    args = ap.parse_args()
    sources = args.images or [ROOT / "tests/fixtures/astronaut.png"]

    failures = total = 0
    with tempfile.TemporaryDirectory(prefix="fmx-jpeg-") as tmp:
        tmp = Path(tmp)
        files: list[Path] = []
        for i, s in enumerate(sources):
            d = tmp / f"src{i}"
            d.mkdir()
            with Image.open(s) as im:
                im.load()
                if s.suffix.lower() in (".jpg", ".jpeg"):
                    files.append(s)  # ekte fil i tillegg til variantene
                files += variants(im, d)
        for f in files:
            total += 1
            with Image.open(f) as im:
                im.load()
                want = np.asarray(ImageOps.exif_transpose(im).convert("RGB"))
            out = tmp / "rust.png"
            proc = subprocess.run([args.dump, str(f), str(out)], capture_output=True, text=True)
            if proc.returncode != 0:
                failures += 1
                print(f"  [FEIL]  {f.name}: {proc.stderr.strip()}")
                continue
            got = np.asarray(Image.open(out).convert("RGB"))
            if got.shape != want.shape:
                failures += 1
                print(f"  [ULIK]  {f.name}: {got.shape} mot {want.shape}")
            elif not np.array_equal(got, want):
                failures += 1
                d = np.abs(got.astype(int) - want.astype(int))
                print(f"  [ULIK]  {f.name}: maks {d.max()}, {np.count_nonzero(d)} verdier")
    print(f"{total - failures}/{total} like" + ("" if failures == 0 else f", {failures} avvik"))
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
