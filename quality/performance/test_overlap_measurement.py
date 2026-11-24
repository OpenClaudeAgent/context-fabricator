import unittest

from overlap_measurement import certify_overlap, stamp


BASE_MONO = 10_000_000
BASE_WALL = 1_700_000_000_000_000


def mark(offset, wall_jump=0):
    return {"mono_us": BASE_MONO + offset, "unix_us": BASE_WALL + offset + wall_jump, "error_us": 5}


def poll(at, indexed, total=20, jump=0):
    return {"started": mark(at, jump), "finished": mark(at + 1_000, jump), "indexed": indexed, "total": total}


def fixture():
    origin = {"path": "/v1/answers", "started": mark(0), "finished": mark(1_000_000)}
    trace = {"name": "answer_generation", "level": "ok", "startedAtUnixMicros": BASE_WALL + 100_000, "value": 800_000}
    progress = [poll(200_000, 2), poll(400_000, 3), poll(600_000, 4)]
    return origin, trace, progress


class OverlapTest(unittest.TestCase):
    def test_paired_stamp_has_bounded_interval(self):
        first = stamp()
        second = stamp()
        self.assertGreater(first["unix_us"], 0)
        self.assertGreaterEqual(first["error_us"], 0)
        self.assertGreaterEqual(second["mono_us"], first["mono_us"])

    def test_two_separately_observed_increments_inside_both_intervals(self):
        origin, trace, progress = fixture()
        result = certify_overlap(origin, trace, progress, 4, 4)
        self.assertTrue(result["certified"])
        self.assertEqual(result["observed_delta"], 2)
        self.assertEqual(result["increment_count"], 2)
        self.assertEqual(result["total"], 20)
        self.assertEqual(len(result["increments"]), 2)

    def test_one_increment_or_partial_overlap_is_rejected(self):
        origin, trace, progress = fixture()
        progress[-1]["indexed"] = 3
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "insufficient_enclosed_increments")
        origin, trace, progress = fixture()
        trace["value"] = 350_000
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "insufficient_enclosed_increments")

    def test_wall_clock_step_and_index_changes_are_rejected(self):
        origin, trace, progress = fixture()
        progress[1] = poll(400_000, 3, jump=2_000_000)
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "clock_discontinuity")
        origin, trace, progress = fixture()
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 5)["reason"], "index_epoch_changed")
        progress[1]["total"] = 21
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "corpus_total_changed")
        progress[1]["total"] = 20
        progress[-1]["indexed"] = 1
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "progress_regressed")

    def test_edge_crossing_and_wrong_trace_are_rejected(self):
        origin, trace, progress = fixture()
        progress[0] = poll(99_999, 2)
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "insufficient_enclosed_increments")
        trace["level"] = "error"
        self.assertEqual(certify_overlap(origin, trace, progress, 4, 4)["reason"], "invalid_generation_trace")


if __name__ == "__main__":
    unittest.main()
