#!/usr/bin/env python3
"""Sammenlign JPEG-dekoderne bit for bit mot ``cv2.imdecode`` — dekoderen Python sin
``faces detect`` bruker på 1024-derivatene.

    ~/dev/fotomaxxer/.venv/bin/python tools/golden/compare_decode.py \\
        ~/dev/fotomaxxer-testlib/py-golden/derived target/release/examples/decode_stream

Referansen er ``cv2.imdecode(…, cv2.IMREAD_COLOR)`` konvertert BGR → RGB, akkurat som
i ``fotomaxxer/faces.py``. Mot den sammenlignes Rust-dekoderne fra eksemplet
``decode_stream`` (``jpegdec``, ``image`` og ``open_rgb``) og Pillow
(``Image.open(…).convert("RGB")``, slik derive leser). Per dekoder skrives antall
bit-like filer, maks og gjennomsnittlig absolutt avvik per kanalverdi og andelen ulike
verdier. Bare filer som slutter på ``_1024.jpg`` tas med. Exit 0 = alle dekoderne
leverte et bilde for hver fil.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, features

RUST_MODES = ["jpegdec", "image", "open_rgb"]


class Stats:
    def __init__(self) -> None:
        self.files = self.identical = self.failed = 0
        self.max = 0
        self.abs_sum = 0
        self.values = self.differing = 0
        self.worst: list[tuple[int, str]] = []

    def add(self, name: str, ref: np.ndarray, got: np.ndarray | None) -> None:
        self.files += 1
        if got is None or got.shape != ref.shape:
            self.failed += 1
            return
        d = np.abs(ref.astype(np.int16) - got.astype(np.int16))
        m = int(d.max())
        self.max = max(self.max, m)
        self.abs_sum += int(d.sum())
        self.values += d.size
        self.differing += int(np.count_nonzero(d))
        if m == 0:
            self.identical += 1
        else:
            self.worst.append((m, name))

    def line(self, label: str) -> str:
        mean = self.abs_sum / self.values if self.values else float("nan")
        share = 100 * self.differing / self.values if self.values else float("nan")
        return (
            f"{label:<9} bit-like {self.identical}/{self.files}  feilet {self.failed}  "
            f"maks {self.max}  snitt {mean:.5f}  ulike verdier {share:.3f} %"
        )


def read_frame(stream) -> np.ndarray | None:
    head = stream.read(8)
    if len(head) != 8:
        raise RuntimeError("decode_stream sluttet for tidlig")
    w, h = int.from_bytes(head[:4], "little"), int.from_bytes(head[4:], "little")
    if w == 0 or h == 0:
        return None
    data = stream.read(w * h * 3)
    return np.frombuffer(data, dtype=np.uint8).reshape(h, w, 3)


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    derived, tool = Path(sys.argv[1]), sys.argv[2]
    paths = sorted(derived.glob("*_1024.jpg"))
    listing = "".join(f"{p}\n" for p in paths).encode()

    procs = {}
    for mode in RUST_MODES:
        proc = subprocess.Popen([tool, mode], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        # decode_stream leser hele lista før den skriver, så dette kan ikke låse seg.
        proc.stdin.write(listing)
        proc.stdin.close()
        procs[mode] = proc

    stats = {name: Stats() for name in [*RUST_MODES, "pillow"]}
    with_exif = 0
    for path in paths:
        data = np.fromfile(path, dtype=np.uint8)
        if b"Exif\x00\x00" in data[:65536].tobytes():
            with_exif += 1  # cv2.imdecode ville anvendt EXIF-orienteringen
        bgr = cv2.imdecode(data, cv2.IMREAD_COLOR)
        ref = cv2.cvtColor(bgr, cv2.COLOR_BGR2RGB)
        for mode, proc in procs.items():
            stats[mode].add(path.name, ref, read_frame(proc.stdout))
        with Image.open(path) as im:
            stats["pillow"].add(path.name, ref, np.asarray(im.convert("RGB")))

    failed = False
    for proc in procs.values():
        failed |= proc.wait() != 0

    print(
        f"{len(paths)} filer i {derived} (med EXIF: {with_exif})  "
        f"referanse: cv2.imdecode {cv2.__version__} BGR→RGB, "
        f"Pillow {Image.__version__} (libjpeg-turbo {features.version('libjpeg_turbo')})"
    )
    for name, s in stats.items():
        print(s.line(name))
        failed |= s.failed > 0
    for name, s in stats.items():
        if s.worst:
            s.worst.sort(reverse=True)
            hist = np.bincount([m for m, _ in s.worst])
            per_max = {i: int(c) for i, c in enumerate(hist) if c}
            print(f"{name}: filer per maks avvik {per_max}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
