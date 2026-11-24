"""Exercise retention without touching workspace artifacts."""

import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "report_directory", Path(__file__).with_name("report-directory.py")
)
reports = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reports)


class RetentionTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name) / ".quality/reports"
        self.pid = os.getpid()
        self.ci = patch.dict(os.environ, {"CI": ""})
        self.ci.start()
        self.addCleanup(self.ci.stop)

    def test_leaves_live_and_preexisting_reports_alone(self):
        legacy = self.root / "analysis.legacy"
        legacy.mkdir(parents=True)
        first = reports.allocate(self.root, "analysis", self.pid)
        second = reports.allocate(self.root, "analysis", self.pid)
        self.assertNotEqual(first.parent, self.root)
        self.assertTrue(first.exists() and second.exists())
        self.assertEqual(list(self.root.iterdir()), [legacy])
        reports.finish(first, self.pid, self.root)
        reports.finish(second, self.pid, self.root)
        third = reports.allocate(self.root, "analysis", self.pid)
        reports.finish(third, self.pid, self.root)
        self.assertFalse((self.root / first.name).exists())
        self.assertTrue((self.root / second.name).exists())
        self.assertTrue((self.root / third.name).exists() and legacy.exists())

    def test_budget_accounts_for_files_outside_reports(self):
        previous = reports.allocate(self.root, "telemetry", self.pid)
        reports.finish(previous, self.pid, self.root)
        manual = self.root.parent / "legacy-build.log"
        manual.write_bytes(os.urandom(64 * 1024))
        current = reports.allocate(self.root, "contract", self.pid)
        output = current / "log"
        output.write_bytes(os.urandom(64 * 1024))
        used = reports.size(self.root.parent) - reports.size(self.root / previous.name)
        with patch.object(reports, "BUDGET_BYTES", reports.METADATA_RESERVE + used + reports.size(current)):
            reports.finish(current, self.pid, self.root)
        self.assertFalse((self.root / previous.name).exists())
        self.assertFalse(current.exists())
        published = self.root / current.name
        self.assertTrue(published.exists() and manual.exists())
        self.assertTrue(json.loads((published / reports.MARKER).read_text())["finished"])

    def test_oversized_active_report_never_enters_quality_directory(self):
        current = reports.allocate(self.root, "app", self.pid)
        (current / "log").write_bytes(os.urandom(64 * 1024))
        with patch.object(reports, "BUDGET_BYTES", reports.METADATA_RESERVE):
            reports.finish(current, self.pid, self.root)
        self.assertFalse(current.exists())
        self.assertFalse((self.root / current.name).exists())

    def test_reports_fail_if_unmanaged_entries_already_exceed_budget(self):
        current = reports.allocate(self.root, "app", self.pid)
        legacy = self.root.parent / "legacy.log"
        legacy.parent.mkdir()
        legacy.write_bytes(os.urandom(64 * 1024))
        with patch.object(reports, "BUDGET_BYTES", 32 * 1024):
            with self.assertRaisesRegex(RuntimeError, "budget is already exceeded"):
                reports.finish(current, self.pid, self.root)
        self.assertTrue(legacy.exists())
        self.assertFalse(current.exists())

    def test_allocation_fails_before_work_if_unmanaged_data_exceeds_budget(self):
        legacy = self.root.parent / "legacy.log"
        legacy.parent.mkdir()
        legacy.write_bytes(os.urandom(64 * 1024))
        with patch.object(reports, "BUDGET_BYTES", 32 * 1024):
            with self.assertRaisesRegex(RuntimeError, "budget is already exceeded"):
                reports.allocate(self.root, "app", self.pid)
        self.assertTrue(legacy.exists())
        self.assertFalse(self.root.exists())

    def test_ci_preserves_reports_for_upload(self):
        with patch.dict(os.environ, {"CI": "true"}):
            for _ in range(4):
                path = reports.allocate(self.root, "app", self.pid)
                reports.finish(path, self.pid, self.root)
        self.assertEqual(len(list(self.root.glob("app.*"))), 4)


if __name__ == "__main__":
    unittest.main()
