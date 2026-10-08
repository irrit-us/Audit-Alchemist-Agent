"""Guard the publishing boundary without running a compiler or remote service."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "scripts/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_version_tag_mismatch_is_rejected(self):
        with patch.dict("os.environ", {"GITHUB_REF_TYPE": "tag", "GITHUB_REF_NAME": "v999.0.0"}):
            with self.assertRaisesRegex(ValueError, "release tag"):
                release.metadata()

    def test_publication_requires_all_targets_and_valid_checksums(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(release, "DIST", Path(directory)), patch.object(release, "metadata", return_value=("0.1.0", "a" * 40)):
            archives = []
            for target in release.TARGETS:
                archive = Path(directory) / release.archive_name("0.1.0", target)
                archive.write_bytes(target.encode())
                archive.with_name(archive.name + ".sha256").write_text(f"{release.digest(archive)}  {archive.name}\n", encoding="ascii")
                archives.append(archive)
            release.verify()
            archives[0].write_bytes(b"corrupted")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                release.verify()
            archives[0].unlink()
            with self.assertRaisesRegex(ValueError, "exactly three"):
                release.verify()


if __name__ == "__main__":
    unittest.main()
