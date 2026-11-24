import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from generation_recovery import verify_runtime_workers


class RuntimeOwnershipTest(unittest.TestCase):
    def test_both_workers_must_execute_the_bundled_binary(self):
        parents = {10: 1, 11: 10, 12: 11, 13: 10, 14: 13}
        commands = {
            10: "ctxfab-server",
            11: "ctxfab-server --guard-inference-worker generation",
            12: "llama-server",
            13: "ctxfab-server --guard-inference-worker embedding",
            14: "llama-server --embedding",
        }

        def process(pid, parent):
            self.assertEqual(parent, parents[pid])
            return {"pid": pid, "parent": parent, "started": "fixture", "command": commands[pid]}

        with TemporaryDirectory() as directory:
            root = Path(directory)
            bundled = root / "app" / "llama-server"
            preview = root / "preview" / "llama-server"
            for binary in (bundled, preview):
                binary.parent.mkdir()
                binary.touch()
            helper = str(root / "ctxfab-server")
            executables = {10: helper, 11: helper, 12: str(bundled), 13: helper, 14: str(bundled)}
            with (
                patch("generation_recovery._processes", return_value=parents),
                patch("generation_recovery._fixture_process", side_effect=process),
                patch("generation_recovery._executable", side_effect=executables.__getitem__),
            ):
                verified = verify_runtime_workers(10, bundled, embedding=True)
                self.assertEqual(verified["generation"]["worker"]["pid"], 12)
                self.assertEqual(verified["embedding"][0]["worker"]["pid"], 14)
                for pid in (12, 14):
                    with self.subTest(worker=pid):
                        executables[pid] = str(preview)
                        with self.assertRaisesRegex(AssertionError, "bundled inference engine"):
                            verify_runtime_workers(10, bundled, embedding=True)
                        executables[pid] = str(bundled)


if __name__ == "__main__":
    unittest.main()
