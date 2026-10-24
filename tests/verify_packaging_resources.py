"""Ressursoppslag må aldri falle tilbake til maskinas verktøy i frosset app."""

import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from fotomaxxer import resources


class ResourcesTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="fmx-resources-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.enterContext(patch.object(sys, "frozen", True, create=True))
        self.enterContext(patch.object(sys, "_MEIPASS", str(self.root), create=True))

    def test_frontend_uses_bundle_instead_of_data_or_working_directory(self):
        with patch("pathlib.Path.cwd", return_value=self.root / "other"), patch.dict(
            "os.environ", {"FOTOMAXXER_HOME": str(self.root / "data")},
        ):
            self.assertEqual(resources.frontend_dist(), self.root / "web" / "dist")

    def test_missing_exiftool_never_uses_path(self):
        with patch("shutil.which") as which:
            with self.assertRaises(FileNotFoundError):
                resources.exiftool_executable()
            which.assert_not_called()

    def test_windows_and_linux_exiftool_layout(self):
        tools = self.root / "tools" / "exiftool"
        tools.mkdir(parents=True)
        for platform, name in (("win32", "exiftool.exe"), ("linux", "exiftool")):
            (tools / name).touch()
            with patch.object(sys, "platform", platform):
                self.assertEqual(resources.exiftool_executable(), str(tools / name))

    def test_missing_or_incomplete_models_fail_locally(self):
        with self.assertRaises(FileNotFoundError):
            resources.face_model_root()
        models = self.root / "insightface" / "models" / "buffalo_l"
        models.mkdir(parents=True)
        for name in resources.BUFFALO_FILES[:-1]:
            (models / name).touch()
        with self.assertRaises(FileNotFoundError):
            resources.face_model_root()
        (models / resources.BUFFALO_FILES[-1]).touch()
        self.assertEqual(resources.face_model_root(), self.root / "insightface")

    def test_development_keeps_default_model_cache(self):
        with patch.object(sys, "frozen", False):
            self.assertIsNone(resources.face_model_root())
            self.assertEqual(resources.resource_root(), Path(__file__).resolve().parent.parent)


if __name__ == "__main__":
    unittest.main()
