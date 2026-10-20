"""Signing setup must fail before generating keys when trust already exists."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("setup-update-key.sh").resolve()


class SetupRefusal(unittest.TestCase):
    def test_existing_remote_trust_and_failed_query_make_no_local_changes(self):
        for reply in ("printf '1\\n'", "exit 9"):
            with self.subTest(reply=reply), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                gh = root / "gh"
                gh.write_text("#!/bin/sh\n" + reply + "\n", encoding="utf-8")
                gh.chmod(0o700)
                keys = root / "new-private-directory"
                env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"])
                result = subprocess.run(
                    ["bash", str(SCRIPT), "ReedME/example", str(keys)],
                    env=env, capture_output=True, text=True, check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(keys.exists(), "refusal must precede key generation and directory changes")


if __name__ == "__main__":
    unittest.main()
