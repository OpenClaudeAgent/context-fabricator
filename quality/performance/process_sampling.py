"""Bounded, best-effort macOS process-tree resource sampling.

Uses the macOS SDK's proc_bsdinfo and rusage_info_v2 ABI. It never reads process
command lines. Peak simultaneous sums include only processes observed in the same
sample and are estimates, not a measurement of deduplicated physical memory.
"""

from __future__ import annotations

import ctypes
import errno
import os
import threading
import time
from collections import Counter


class _BsdInfo(ctypes.Structure):
    _fields_ = [
        *[(name, ctypes.c_uint32) for name in (
            "flags", "status", "xstatus", "pid", "ppid", "uid", "gid", "ruid",
            "rgid", "svuid", "svgid", "reserved",
        )],
        ("comm", ctypes.c_char * 16),
        ("name", ctypes.c_char * 32),
        *[(name, ctypes.c_uint32) for name in ("nfiles", "pgid", "pjobc", "tdev", "tpgid")],
        ("nice", ctypes.c_int32),
        ("start_sec", ctypes.c_uint64),
        ("start_usec", ctypes.c_uint64),
    ]


class _RusageV2(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [
        (name, ctypes.c_uint64) for name in (
            "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups", "pageins",
            "wired_size", "resident_size", "phys_footprint", "proc_start_abstime",
            "proc_exit_abstime", "child_user_time", "child_system_time", "child_pkg_idle_wkups",
            "child_interrupt_wkups", "child_pageins", "child_elapsed_abstime",
            "diskio_bytesread", "diskio_byteswritten",
        )
    ]


class _MachTimebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


_LIB = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True) if os.uname().sysname == "Darwin" else None
if _LIB is not None:
    _LIB.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
    _LIB.proc_pidinfo.restype = ctypes.c_int
    _LIB.proc_listchildpids.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
    _LIB.proc_listchildpids.restype = ctypes.c_int
    _LIB.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    _LIB.proc_pid_rusage.restype = ctypes.c_int
    _mach_timebase_info = ctypes.CDLL("/usr/lib/libSystem.B.dylib").mach_timebase_info
    _mach_timebase_info.argtypes = [ctypes.POINTER(_MachTimebase)]
    _mach_timebase_info.restype = ctypes.c_int
    _TIMEBASE = _MachTimebase()
    if _mach_timebase_info(ctypes.byref(_TIMEBASE)) != 0 or not _TIMEBASE.denom:
        raise RuntimeError("macOS timebase is unavailable")


class ProcessSampler:
    """Sample a root PID and discovered descendants without signalling any process.

    Write deltas start at first observation and end at last observation; writes
    before discovery and after the final sample are not counted. Short-lived or
    reparented processes may be missed. Descendants already discovered remain
    tracked after reparenting, provided their start identity still matches.
    """

    def __init__(self, root_pid: int, interval: float = 0.1, max_processes: int = 4096):
        if _LIB is None:
            raise RuntimeError("ProcessSampler requires macOS libproc")
        if root_pid <= 0 or interval <= 0 or not 1 <= max_processes <= 4096:
            raise ValueError("root_pid and interval must be positive; max_processes must be 1 to 4096")
        self._root_pid = root_pid
        self._interval = interval
        self._limit = max_processes
        self._lock = threading.Lock()
        self._stop = threading.Event()
        self._thread = None
        self._started = None
        self._ended = None
        self._known = {}
        self._root_checked = False
        self._records = {}
        self._current = []
        self._missing = Counter()
        self._exits = 0
        self._samples = 0
        self._max_footprint = 0
        self._max_rss = 0

    def __enter__(self):
        if self._thread is not None:
            raise RuntimeError("sampler cannot be started twice")
        self._started = time.monotonic()
        self._sample()
        self._thread = threading.Thread(target=self._run, name="process-sampler", daemon=True)
        self._thread.start()
        return self

    def __exit__(self, _type, _value, _traceback):
        self._stop.set()
        if self._thread is not None:
            self._thread.join()
        self._sample()
        self._ended = time.monotonic()

    def _run(self):
        while not self._stop.wait(self._interval):
            try:
                self._sample()
            except Exception as error:
                with self._lock:
                    self._missing[f"sampler_error_{type(error).__name__}"] += 1
                return

    @staticmethod
    def _info(pid):
        info = _BsdInfo()
        ctypes.set_errno(0)
        result = _LIB.proc_pidinfo(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
        return info if result == ctypes.sizeof(info) else None

    def _children(self, pid):
        buffer = (ctypes.c_int * self._limit)()
        count = _LIB.proc_listchildpids(pid, buffer, ctypes.sizeof(buffer))
        if count < 0:
            self._missing["child_enumeration_failed"] += 1
            return []
        if count >= self._limit:
            self._missing["child_list_limit_reached"] += 1
        return [child for child in buffer[: min(count, self._limit)] if child > 0]

    def _sample(self):
        with self._lock:
            self._samples += 1
            if not self._root_checked:
                self._root_checked = True
                root = self._info(self._root_pid)
                if root is None:
                    self._missing["root_unavailable"] += 1
                    return
                self._known[self._root_pid] = (root.start_sec, root.start_usec)
            if not self._known:
                return
            queue = list(self._known)
            visited = set()
            current = []
            while queue:
                pid = queue.pop()
                if pid in visited:
                    continue
                visited.add(pid)
                info = self._info(pid)
                if info is None:
                    if ctypes.get_errno() == errno.ESRCH:
                        del self._known[pid]
                        self._exits += 1
                    else:
                        self._missing["process_info_unavailable"] += 1
                    continue
                identity = (info.start_sec, info.start_usec)
                if self._known.get(pid) != identity:
                    self._missing["pid_identity_changed"] += 1
                    continue
                usage = _RusageV2()
                if _LIB.proc_pid_rusage(pid, 2, ctypes.byref(usage)) != 0:
                    self._missing["rusage_unavailable"] += 1
                else:
                    after = self._info(pid)
                    if after is None or (after.start_sec, after.start_usec) != identity:
                        self._missing["identity_changed_during_sample"] += 1
                    else:
                        key = (pid, *identity)
                        sampled_ns = time.monotonic_ns()
                        cpu_ns = (usage.user_time + usage.system_time) * _TIMEBASE.numer // _TIMEBASE.denom
                        record = self._records.setdefault(key, {
                            "pid": pid, "start_seconds": identity[0], "start_microseconds": identity[1],
                            "peak_footprint_bytes": 0, "peak_rss_bytes": 0,
                            "first_write_bytes": usage.diskio_byteswritten,
                            "last_write_bytes": usage.diskio_byteswritten, "samples": 0,
                            "first_cpu_ns": cpu_ns, "last_cpu_ns": cpu_ns,
                            "last_sample_ns": sampled_ns, "peak_sampled_cpu_percent": 0.0,
                        })
                        elapsed_ns = sampled_ns - record["last_sample_ns"]
                        if elapsed_ns >= self._interval * 500_000_000:
                            rate = 100 * (cpu_ns - record["last_cpu_ns"]) / elapsed_ns
                            record["peak_sampled_cpu_percent"] = max(record["peak_sampled_cpu_percent"], rate)
                        record["last_sample_ns"] = sampled_ns
                        record["last_cpu_ns"] = max(record["last_cpu_ns"], cpu_ns)
                        record["peak_footprint_bytes"] = max(record["peak_footprint_bytes"], usage.phys_footprint)
                        record["peak_rss_bytes"] = max(record["peak_rss_bytes"], usage.resident_size)
                        record["last_write_bytes"] = max(record["last_write_bytes"], usage.diskio_byteswritten)
                        record["samples"] += 1
                        current.append({"pid": pid, "footprint_bytes": usage.phys_footprint,
                                        "rss_bytes": usage.resident_size})
                for child in self._children(pid):
                    child_info = self._info(child)
                    if child_info is None:
                        self._missing["child_disappeared"] += 1
                        continue
                    if child_info.ppid != pid:
                        self._missing["child_parent_changed"] += 1
                        continue
                    child_identity = (child_info.start_sec, child_info.start_usec)
                    if child in self._known and self._known[child] != child_identity:
                        self._missing["pid_identity_changed"] += 1
                        continue
                    if child not in self._known:
                        if len(self._known) >= self._limit:
                            self._missing["process_limit_reached"] += 1
                            continue
                        self._known[child] = child_identity
                    queue.append(child)
            self._current = current
            self._max_footprint = max(self._max_footprint, sum(p["footprint_bytes"] for p in current))
            self._max_rss = max(self._max_rss, sum(p["rss_bytes"] for p in current))

    def processes(self):
        with self._lock:
            return [dict(item) for item in self._current]

    def report(self):
        with self._lock:
            records = []
            for record in self._records.values():
                item = dict(record)
                item["write_delta_bytes"] = max(0, item.pop("last_write_bytes") - item.pop("first_write_bytes"))
                item["cpu_time_delta_ns"] = item.pop("last_cpu_ns") - item.pop("first_cpu_ns")
                item.pop("last_sample_ns")
                records.append(item)
            elapsed = 0.0 if self._started is None else (self._ended or time.monotonic()) - self._started
            return {
                "root_pid": self._root_pid,
                "sample_count": self._samples,
                "elapsed_seconds": elapsed,
                "peak_simultaneous_footprint_bytes": self._max_footprint,
                "peak_simultaneous_rss_bytes": self._max_rss,
                "processes": sorted(records, key=lambda item: (item["pid"], item["start_seconds"], item["start_microseconds"])),
                "observed_exits": self._exits,
                "missing_counters": dict(self._missing),
                "limitations": [
                    "Short-lived or reparented-before-discovery descendants can be missed.",
                    "Write deltas exclude activity before first and after last observation; they are not SQLite file growth.",
                    "Simultaneous sums are sampled estimates and can double-count shared memory.",
                    "CPU rates are sampled utilization as a percentage of one logical core.",
                ],
            }
