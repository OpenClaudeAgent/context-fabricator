import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from process_sampling import ProcessSampler


@unittest.skipUnless(sys.platform == "darwin", "requires macOS libproc")
class SamplerTest(unittest.TestCase):
    def test_samples_own_tree_and_write_delta(self):
        child_script = """
import os, sys, time
time.sleep(0.5)
until = time.process_time() + 0.05
while time.process_time() < until:
    pass
with open(sys.argv[1], 'wb', buffering=0) as out:
    out.write(b'x' * (4 * 1024 * 1024))
    os.fsync(out.fileno())
time.sleep(0.6)
"""
        parent_script = """
import subprocess, sys, time
child = subprocess.Popen([sys.executable, '-c', sys.argv[1], sys.argv[2]])
print(child.pid, flush=True)
child.wait()
time.sleep(0.2)
"""
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "writes")
            process = subprocess.Popen(
                [sys.executable, "-c", parent_script, child_script, path],
                stdout=subprocess.PIPE, text=True,
            )
            try:
                child_pid = int(process.stdout.readline().strip())
                with ProcessSampler(process.pid, interval=0.05) as sampler:
                    process.wait(timeout=5)
                report = sampler.report()
                by_pid = {row["pid"]: row for row in report["processes"]}
                self.assertIn(process.pid, by_pid)
                self.assertIn(child_pid, by_pid)
                self.assertGreaterEqual(report["sample_count"], 5)
                self.assertGreater(report["peak_simultaneous_footprint_bytes"], 0)
                self.assertGreater(by_pid[child_pid]["peak_rss_bytes"], 0)
                self.assertGreaterEqual(by_pid[child_pid]["write_delta_bytes"], 4 * 1024 * 1024)
                self.assertGreater(by_pid[child_pid]["cpu_time_delta_ns"], 25_000_000)
                self.assertLess(by_pid[child_pid]["cpu_time_delta_ns"], 500_000_000)
                self.assertGreater(by_pid[child_pid]["peak_sampled_cpu_percent"], 0)
                self.assertEqual(report["observed_exits"], 2)
                self.assertNotIn("process_info_unavailable", report["missing_counters"])
                self.assertEqual(os.path.getsize(path), 4 * 1024 * 1024)
                print(json.dumps(report, sort_keys=True))
            finally:
                if process.poll() is None:
                    process.wait(timeout=5)
                process.stdout.close()


if __name__ == "__main__":
    unittest.main()
