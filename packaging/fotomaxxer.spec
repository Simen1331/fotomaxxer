"""Mappemodus, bygget på mål-OS via scripts/build-desktop-probe.py."""

import os
from pathlib import Path

from PyInstaller.utils.hooks import collect_all, collect_submodules, copy_metadata

repo = Path(SPECPATH).parent
resources = Path(os.environ["FOTOMAXXER_BUILD_RESOURCES"])
datas = []
binaries = []
hiddenimports = collect_submodules("api")
for package in ("insightface", "onnxruntime", "pillow_heif"):
    data, native, hidden = collect_all(package)
    datas += data
    binaries += native
    hiddenimports += [name for name in hidden if not name.startswith("insightface.gui")]
datas += copy_metadata("fotomaxxer", recursive=True)

for path in resources.rglob("*"):
    if not path.is_file():
        continue
    destination = str(path.parent.relative_to(resources))
    native = path.name == "perl" or path.suffix in (".exe", ".dll", ".so") or ".so." in path.name
    (binaries if native else datas).append((str(path), destination))

a = Analysis(
    [str(repo / "scripts" / "desktop_entry.py"), str(repo / "scripts" / "frozen_entry.py")],
    pathex=[str(repo)], binaries=binaries, datas=datas,
    hiddenimports=hiddenimports, hookspath=[], runtime_hooks=[],
    excludes=["tkinter", "matplotlib", "IPython", "pytest"],
    noarchive=False,
)
pyz = PYZ(a.pure)
exe = EXE(
    pyz, [s for s in a.scripts if s[0] != "frozen_entry"], [], exclude_binaries=True, name="FotoMaxxer",
    debug=False, strip=False, upx=False, console=False,
)
cli = EXE(
    pyz, [s for s in a.scripts if s[0] != "desktop_entry"], [], exclude_binaries=True, name="FotoMaxxer-cli",
    debug=False, strip=False, upx=False, console=True,
)
coll = COLLECT(exe, cli, a.binaries, a.datas, strip=False, upx=False, name="FotoMaxxer")
