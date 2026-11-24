"""Conservative local-clock certification of observed inference overlap."""

from __future__ import annotations

import time

# Allow 5 ms of wall-clock step/jitter plus 100 ppm drift over the sampled span.
# Clock-read uncertainty is added separately for each paired stamp.
_MAX_CLOCK_STEP_US = 5_000
_MAX_DRIFT_PPM = 100


def stamp():
    """Return wall and monotonic microseconds with a bracketing uncertainty."""
    before = time.monotonic_ns()
    wall = time.time_ns()
    after = time.monotonic_ns()
    return {
        "unix_us": wall // 1_000,
        "mono_us": (before + after) // 2_000,
        "error_us": (after - before + 1_999) // 2_000 + 1,
    }


def _valid_stamp(value):
    return (
        isinstance(value, dict)
        and all(isinstance(value.get(key), int) and not isinstance(value.get(key), bool)
                for key in ("unix_us", "mono_us", "error_us"))
        and value["unix_us"] > 0
        and value["mono_us"] >= 0
        and value["error_us"] >= 0
    )


def _refusal(reason):
    return {"certified": False, "reason": reason}


def certify_overlap(origin, generation, progress, epoch_before, epoch_after):
    """Return JSON-compatible evidence or a reason the evidence is insufficient.

    Indexed values are responses to serial, non-overlapping polling requests.
    Trace times are wall-clock microseconds; paired stamps bound conversion into
    the monotonic domain. A valid result establishes observed overlap, subject to
    the caller's independent guarantee that indexing uses serial inference and
    that each published increment represents a completed passage.
    """
    if not isinstance(origin, dict) or not isinstance(origin.get("path"), str) or not origin["path"]:
        return _refusal("invalid_origin")
    start, finish = origin.get("started"), origin.get("finished")
    if not _valid_stamp(start) or not _valid_stamp(finish) or finish["mono_us"] <= start["mono_us"]:
        return _refusal("invalid_origin_interval")
    if (not isinstance(generation, dict) or generation.get("name") != "answer_generation"
            or generation.get("level") != "ok"):
        return _refusal("invalid_generation_trace")
    trace_start, duration = generation.get("startedAtUnixMicros"), generation.get("value")
    if (not isinstance(trace_start, int) or isinstance(trace_start, bool) or trace_start <= 0
            or not isinstance(duration, int) or isinstance(duration, bool) or duration <= 0):
        return _refusal("invalid_generation_interval")
    if (not isinstance(epoch_before, int) or isinstance(epoch_before, bool)
            or not isinstance(epoch_after, int) or isinstance(epoch_after, bool)
            or epoch_before != epoch_after):
        return _refusal("index_epoch_changed")
    if not isinstance(progress, list) or len(progress) < 3:
        return _refusal("insufficient_progress_samples")
    stamps = [start, finish]
    previous = None
    total = None
    for poll in progress:
        if not isinstance(poll, dict):
            return _refusal("invalid_progress")
        pstart, pend = poll.get("started"), poll.get("finished")
        indexed, count = poll.get("indexed"), poll.get("total")
        if (not _valid_stamp(pstart) or not _valid_stamp(pend)
                or pend["mono_us"] < pstart["mono_us"]
                or not isinstance(indexed, int) or isinstance(indexed, bool)
                or not isinstance(count, int) or isinstance(count, bool)
                or count < 0 or indexed < 0 or indexed > count):
            return _refusal("invalid_progress")
        if total is None:
            total = count
        if count != total:
            return _refusal("corpus_total_changed")
        if previous is not None:
            if pstart["mono_us"] < previous["finished"]["mono_us"]:
                return _refusal("overlapping_progress_polls")
            if indexed < previous["indexed"]:
                return _refusal("progress_regressed")
        stamps.extend((pstart, pend))
        previous = poll
    # All paired stamps must agree on wall/monotonic displacement. An observed
    # offset larger than the bounded jitter/drift allowance indicates a clock jump.
    base_offset = start["unix_us"] - start["mono_us"]
    max_margin = start["error_us"]
    for item in stamps:
        elapsed = abs(item["mono_us"] - start["mono_us"])
        observed = abs((item["unix_us"] - item["mono_us"]) - base_offset)
        error = start["error_us"] + item["error_us"]
        allowance = _MAX_CLOCK_STEP_US + elapsed * _MAX_DRIFT_PPM // 1_000_000 + error
        if observed > allowance:
            return _refusal("clock_discontinuity")
        max_margin = max(max_margin, observed + error)
    trace_start_mono = trace_start - base_offset
    trace_end_mono = trace_start_mono + duration
    window_start = max(start["mono_us"], trace_start_mono)
    window_end = min(finish["mono_us"], trace_end_mono)
    if window_start + max_margin >= window_end - max_margin:
        return _refusal("no_provable_common_window")
    enclosed = []
    for poll in progress:
        pstart, pend = poll["started"], poll["finished"]
        if (pstart["mono_us"] - pstart["error_us"] > window_start + max_margin
                and pend["mono_us"] + pend["error_us"] < window_end - max_margin):
            enclosed.append(poll)
    increments = []
    # Use only consecutive original polls: skipping a poll could hide a reset.
    for left, right in zip(progress, progress[1:]):
        if left in enclosed and right in enclosed and right["indexed"] > left["indexed"]:
            increments.append({
                "before_indexed": left["indexed"], "after_indexed": right["indexed"],
                "before_poll": {"started": left["started"], "finished": left["finished"]},
                "after_poll": {"started": right["started"], "finished": right["finished"]},
            })
    if len(increments) < 2:
        return _refusal("insufficient_enclosed_increments")
    return {
        "certified": True,
        "origin_path": origin["path"],
        "index_epoch": epoch_before,
        "total": total,
        "clock_margin_us": max_margin,
        "common_window_mono_us": {"started": window_start, "finished": window_end},
        "enclosed_poll_count": len(enclosed),
        "increment_count": len(increments),
        "observed_delta": sum(item["after_indexed"] - item["before_indexed"] for item in increments),
        "increments": increments,
    }
