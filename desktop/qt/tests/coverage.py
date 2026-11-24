"""Run the Qt suites against Rust and enforce first-party C++ line coverage."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from run_window_scenarios import run as run_window_scenarios


MINIMUM_LINE_PERCENT = 90
IMPLEMENTATIONS = {".cpp", ".cc", ".cxx"}
HEADERS = {".h", ".hh", ".hpp"}


def run_tool(command):
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode:
        raise SystemExit(f"{Path(command[0]).name} failed coverage qualification:\n{result.stderr}")
    return result


def check_diagnostics(cov, inputs, diagnostics, data, log):
    if not diagnostics.strip():
        log.write_text("", encoding="utf-8")
        return
    details = run_tool([cov, "report", "--dump", "--num-threads=1", *inputs]).stderr
    log.write_text(details, encoding="utf-8")
    lines = details.strip().splitlines()
    headline = re.fullmatch(r"warning: (\d+) functions have mismatched data", diagnostics.strip())
    if not headline or int(headline[1]) != len(lines) - 1 or lines[0] != headline[0]:
        raise SystemExit(f"Unexpected LLVM coverage diagnostics:\n{details or diagnostics}")

    # LLVM emits a zero-hash placeholder when an inline function is unused in
    # one translation unit. Keep the executable mapping from the other unit.
    executed = {function["name"] for function in data["functions"] if function["count"] > 0}
    for line in lines[1:]:
        mismatch = re.fullmatch(
            r"hash-mismatch: No profile record found for '([^']+)' with hash = 0x0", line
        )
        if not mismatch:
            raise SystemExit(f"Unverified LLVM coverage mapping: {line}")
        # Q_OBJECT expands tr() from the Qt SDK outside the measured filenames.
        qt_translation_wrapper = re.fullmatch(r"_ZN6ctxfab[\d\w]+2trEPKcS2_i", mismatch[1])
        if mismatch[1] not in executed and not qt_translation_wrapper:
            raise SystemExit(f"No executed mapping for LLVM placeholder: {mismatch[1]}")
    print(f"LLVM reported {len(lines) - 1} zero-hash placeholders. Details: {log}")


def collect(ctest, build, profdata, cov, binaries, sources):
    app, window_test, *_ = binaries
    backend = Path(os.environ.get("CTXFAB_TEST_BACKEND_EXECUTABLE", ""))
    if not (backend.is_absolute() and backend.is_file() and os.access(backend, os.X_OK)):
        raise SystemExit("Set CTXFAB_TEST_BACKEND_EXECUTABLE to the absolute path of ctxfab-server.")

    with tempfile.TemporaryDirectory(prefix="profiles-", dir=build) as temporary:
        profile_dir = Path(temporary)
        subprocess.run(
            [ctest, "--test-dir", build, "--output-on-failure",
             "--no-tests=error", "--output-junit", build / "tests.xml"],
            env=dict(os.environ, LLVM_PROFILE_FILE=str(profile_dir / "%m-%p.profraw")),
            check=True,
        )
        run_window_scenarios(
            Path(window_test), backend, build / "window-scenarios", offscreen=True,
            profile_dir=profile_dir,
        )
        profiles = sorted(profile_dir.glob("*.profraw"))
        if not profiles:
            raise SystemExit("CTest produced no LLVM profiles.")
        profile = profile_dir / "qt.profdata"
        merge = run_tool([profdata, "merge", "-sparse", *profiles, "-o", profile])
        if merge.stderr.strip():
            raise SystemExit(f"Unexpected LLVM profile merge diagnostics:\n{merge.stderr}")

        inputs = [app, *(f"-object={binary}" for binary in binaries[1:]),
                  f"-instr-profile={profile}", "--sources", *sources]
        summary = run_tool([cov, "report", *inputs])
        report = run_tool([cov, "export", *inputs])
        data = json.loads(report.stdout)["data"][0]
        if summary.stderr != report.stderr:
            raise SystemExit(f"LLVM report and export disagree:\n{summary.stderr}\n{report.stderr}")
        check_diagnostics(cov, inputs, report.stderr, data, build / "coverage-diagnostics.log")
    (build / "coverage.txt").write_text(summary.stdout, encoding="utf-8")
    (build / "coverage.json").write_text(report.stdout, encoding="utf-8")
    return data


def check_coverage(data, sources):
    measured = {
        Path(file["filename"]).resolve()
        for file in data["files"]
        if file["summary"]["lines"]["count"] > 0
    }
    missing = {source.resolve() for source in sources if source.suffix in IMPLEMENTATIONS} - measured
    if missing:
        raise SystemExit("Missing coverage mappings for: " + ", ".join(map(str, sorted(missing))))
    unexpected = measured - {source.resolve() for source in sources}
    if unexpected:
        raise SystemExit("Unexpected files in coverage report: " + ", ".join(map(str, sorted(unexpected))))
    lines = data["totals"]["lines"]
    covered, total = lines["covered"], lines["count"]
    if not 0 < total or not 0 <= covered <= total:
        raise SystemExit("Invalid line counts in the LLVM coverage report.")
    print(f"Qt C++ lines: {covered}/{total} ({covered / total:.2%}), required: {MINIMUM_LINE_PERCENT}%.")
    if covered * 100 < MINIMUM_LINE_PERCENT * total:
        raise SystemExit(1)


def main():
    ctest, build, source_root, profdata, cov, *binaries = sys.argv[1:]
    build = Path(build)
    # Clang records the source path it was given, which may include a symlink.
    source_root = Path(source_root).absolute()
    sources = sorted(
        path for path in source_root.rglob("*")
        if path.suffix in IMPLEMENTATIONS | HEADERS and path != source_root / "app" / "main.cpp"
    )
    for report in ("tests.xml", "coverage.txt", "coverage.json", "coverage-diagnostics.log"):
        (build / report).unlink(missing_ok=True)
    check_coverage(collect(ctest, build, profdata, cov, binaries, sources), sources)


if __name__ == "__main__":
    main()
