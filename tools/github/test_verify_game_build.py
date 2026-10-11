"""Regression coverage for cross-platform private native-build verification."""

import copy
from pathlib import Path
import tempfile
import unittest

from verify_game_build import check_report, select_archive


class NativeBuildGate(unittest.TestCase):
    def test_40420_never_uses_the_older_archive(self):
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            repo, old, current = (base / name for name in ("repo", "old", "current"))
            for path in (repo, old, current):
                path.mkdir()
            self.assertEqual(select_archive("tf3_build40420_steam_windows", str(old), str(current), repo), current)
            self.assertEqual(select_archive("tf3_build40408_steam_windows", str(old), str(current), repo), old)
            for selected, archive in (("unknown", str(current)), ("tf3_build40420_steam_windows", None),
                                      ("tf3_build40420_steam_windows", str(repo)),
                                      ("tf3_build40420_steam_windows", str(base / "missing"))):
                with self.assertRaises((ValueError, OSError)):
                    select_archive(selected, str(old), archive, repo)

    def test_only_exact_clean_successful_static_reports_pass(self):
        commit = "a" * 40
        selected = "tf3_build40420_steam_windows"
        report = {"source_commit": commit, "source_dirty": False, "review_required": False,
                  "runtime_verified": False, "kind": "selected_build_verification", "bundle": selected,
                  "targets": [{"matched": True}]}
        check_report(report, commit, selected)
        for key, value in (("source_commit", "b" * 40), ("source_dirty", True),
                           ("review_required", True), ("runtime_verified", True),
                           ("kind", "profile_verification"), ("bundle", "old"), ("targets", [])):
            altered = copy.deepcopy(report)
            altered[key] = value
            with self.assertRaises(ValueError, msg=key):
                check_report(altered, commit, selected)
        for key in ("source_commit", "source_dirty", "review_required", "runtime_verified", "targets"):
            altered = copy.deepcopy(report)
            del altered[key]
            with self.assertRaises(ValueError, msg=f"missing {key}"):
                check_report(altered, commit, selected)


if __name__ == "__main__":
    unittest.main()
