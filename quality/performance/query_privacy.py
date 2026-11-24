"""Qualify opt-in history and diagnostic redaction on an optimized app bundle."""

import json
import shutil
import sqlite3
import sys
from contextlib import closing, contextmanager
from dataclasses import dataclass
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener
from uuid import uuid4

quality = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(quality / name) for name in ("github", "release")]
from dogfood import NoRedirect
from release import read_package
from workload import (
    BASE,
    FEATURES,
    INDEX,
    QUESTION,
    REPOSITORY,
    REPOSITORY_ID,
    WEIGHTS,
    GitHistory,
    answer_verified,
    backend,
    origin,
    prepare_answers,
    read_manifest,
    save,
    wait_for_commit,
    wait_until,
)

HISTORY = "/v1/answer-history"
SETTINGS = HISTORY + "/settings"
PRIMARY_DB = "context.sqlite3"
PRIMARY_FILES = {
    PRIMARY_DB,
    PRIMARY_DB + "-wal",
    PRIMARY_DB + "-shm",
    PRIMARY_DB + "-journal",
}
ANSWER_WEIGHT = WEIGHTS[0]


def require_redacted(data, markers, source):
    for name, value in markers.items():
        if value in data:
            raise AssertionError(f"{name} leaked into {source}")


def json_bytes(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def inspect_files(
    data, log, secrets, history_content, *, live=False, history_enabled=False
):
    candidates = [
        path for path in data.rglob("*") if not path.is_symlink() and path.is_file()
    ]
    inspected = []
    for file in [*candidates, log]:
        if live and file == data / "endpoint.json":
            # This private discovery record is the designated location of the current bearer.
            continue
        if file == data / "inference" / ANSWER_WEIGHT:
            # The managed runtime independently verifies these pinned model weights.
            continue
        try:
            contents = file.read_bytes()
        except FileNotFoundError:
            # SQLite can close a sidecar between directory listing and read.
            continue
        label = str(file.relative_to(data)) if file.is_relative_to(data) else file.name
        require_redacted(contents, secrets, label)
        if not history_enabled or label not in PRIMARY_FILES:
            require_redacted(contents, history_content, label)
        inspected.append(label)
    assert {PRIMARY_DB, log.name} <= set(inspected)
    return sorted(inspected)


def telemetry_payload(api, path, markers):
    value, _ = api("GET", path)
    serialized = json_bytes(value)
    require_redacted(serialized, markers, path)
    assert b'"modelContext"' not in serialized, path
    return value


def verify_credentials(api, markers):
    path = "/v1/github/credential"
    accepted, _ = api(
        "PUT", path, {"revision": 0, "token": markers["provider credential"].decode()}
    )
    assert accepted["credential"] == "connected"
    require_redacted(json_bytes(accepted), markers, path)
    invalid = markers["rejected provider credential"].decode()
    try:
        api("PUT", path, {"revision": accepted["revision"], "token": invalid})
    except HTTPError as error:
        with error:
            problem = error.read()
        assert (
            error.code == 400 and json.loads(problem)["code"] == "github_invalid_token"
        )
        require_redacted(problem, markers, path)
    else:
        raise AssertionError("The provider accepted an invalid fixture credential.")
    state = telemetry_payload(api, "/v1/github", markers)
    assert (
        state["revision"] == accepted["revision"] and state["credential"] == "connected"
    )
    return state["revision"]


@contextmanager
def private_provider_session(api, markers):
    api("PUT", INDEX, {"enabled": False})
    debug, _ = api("PUT", "/v1/telemetry", {"command": "enableDebug"})
    assert debug["health"]["debugUntil"] > 0
    try:
        yield verify_credentials(api, markers)
    finally:
        state, _ = api("GET", "/v1/github")
        if state["credential"] != "unconfigured":
            path = "/v1/github/credential"
            disconnected, _ = api("DELETE", f"{path}?revision={state['revision']}")
            assert disconnected["credential"] == "unconfigured"
            require_redacted(json_bytes(disconnected), markers, path)


@dataclass(frozen=True)
class Probe:
    question: str
    trace: str
    answer: dict
    label: bytes

    def content_markers(self):
        return {
            "question label": self.label,
            "answer text": self.answer["text"].encode(),
            "encoded answer": json_bytes(self.answer["text"])[1:-1],
        }


def answer_probe(api, git, secrets):
    assert telemetry_payload(api, "/v1/telemetry", secrets)["health"]["debugUntil"] > 0
    label = "ctxfab-query-privacy-" + uuid4().hex
    question = QUESTION + f" This request has the reference label {label}."
    answer, trace = api(
        "POST",
        "/v1/answers",
        {
            "query": question,
            "repositoryId": REPOSITORY_ID,
            "branch": "main",
            "includeModelContext": True,
        },
    )
    try:
        answer_verified(git, answer, BASE)
    except (AssertionError, KeyError, TypeError, ValueError):
        raise AssertionError(
            "The fixture answer did not carry the required pinned citation."
        ) from None
    assert trace and len(answer["text"].encode()) >= 32
    require_redacted(json_bytes(answer), secrets, "live answer")
    context = answer.pop("modelContext")
    assert context["question"] == question and context["sources"], (
        "Expected live model context"
    )
    return Probe(question, trace, answer, label.encode())


def verify_diagnostics(api, probes, markers):
    for probe in probes:
        path = f"/v1/telemetry/traces/{probe.trace}"

        def generation_retained(trace_path=path):
            try:
                detail = telemetry_payload(api, trace_path, markers)
            except HTTPError as error:
                if error.code != 404:
                    raise
                error.close()
                return None
            return (
                True
                if any(
                    span["name"] == "answer_generation" and span.get("level") == "ok"
                    for span in detail["spans"]
                )
                else None
            )

        wait_until(8, generation_retained)
    for path in ["/v1/telemetry", "/v1/telemetry/logs", "/v1/telemetry/answer-usage"]:
        telemetry_payload(api, path, markers)


def history_request(
    endpoint, path, *, method="GET", expected=200, credential=True, browser=False
):
    headers = {}
    if credential:
        headers["Authorization"] = "Bearer " + endpoint["token"]
    if browser:
        headers["Origin"] = "https://fixture.invalid"
    action = Request(endpoint["url"] + path, method=method, headers=headers)
    opener = build_opener(ProxyHandler({}), NoRedirect())
    try:
        response = opener.open(action, timeout=8)
    except HTTPError as error:
        response = error
    with response:
        raw = response.read()
        assert response.code == expected, (path, response.code, expected)
        if credential and not browser:
            assert response.headers.get("Cache-Control") == "no-store", path
        return json.loads(raw) if raw else None


def verify_saved_history(endpoint, probe, markers):
    entries = history_request(endpoint, HISTORY)["entries"]
    assert len(entries) == 1 and entries[0]["question"] == probe.question
    entry_id = entries[0]["id"]
    trace_path = f"{HISTORY}/by-trace/{probe.trace}"
    id_path = f"{HISTORY}/{entry_id}"
    by_id = history_request(endpoint, id_path)
    by_trace = history_request(endpoint, trace_path)
    assert by_id == by_trace, "Saved detail and trace lookup diverged"
    assert by_id["question"] == probe.question and by_id["traceId"] == probe.trace
    assert by_id["answer"] == probe.answer, (
        "Saved answer diverged from the published answer"
    )
    assert by_id["scope"] == {"repositoryId": REPOSITORY_ID, "branch": "main"}
    for path in [HISTORY, id_path, trace_path]:
        rejected = history_request(endpoint, path, expected=401, credential=False)
        require_redacted(json_bytes(rejected), markers, path)
    rejected = history_request(endpoint, trace_path, expected=401, browser=True)
    require_redacted(json_bytes(rejected), markers, trace_path)
    return entry_id


def inspect_history_rows(data, probe, markers):
    """Inspect logical TEXT/BLOB values, including rows split across SQLite pages."""
    with closing(
        sqlite3.connect((data / PRIMARY_DB).as_uri() + "?mode=ro", uri=True)
    ) as db:
        saved = db.execute(
            "SELECT question, answer_json FROM answer_history_entries"
        ).fetchall()
        if probe is None:
            assert not saved, "History has rows before opt-in"
        else:
            assert len(saved) == 1 and saved[0][0] == probe.question
            assert json.loads(saved[0][1]) == probe.answer, (
                "SQLite retained an unexpected answer field"
            )
        tables = db.execute(
            "SELECT name FROM pragma_table_list WHERE schema = 'main' AND type IN ('table', 'shadow')"
        ).fetchall()
        for (table,) in tables:
            quoted = '"' + table.replace('"', '""') + '"'
            columns = db.execute(f"SELECT * FROM {quoted}")
            names = [description[0] for description in columns.description]
            for row in columns:
                for name, value in zip(names, row, strict=True):
                    if table == "answer_history_entries" and name in (
                        "question",
                        "answer_json",
                    ):
                        # Exact equality above confines the expected content to these two fields.
                        continue
                    if isinstance(value, str):
                        value = value.encode()
                    if isinstance(value, bytes):
                        require_redacted(value, markers, f"{table}.{name}")
    return len(tables)


def install_weights(bundle, data, port, assets, report):
    inference = data / "inference"
    inference.mkdir(mode=0o700)
    shutil.copyfile(assets / ANSWER_WEIGHT, inference / ANSWER_WEIGHT)
    setup = report / "setup"
    setup.mkdir()
    with backend(
        bundle, data, port, setup, hub_port=port, preview_runtime=False
    ) as api:
        prepare_answers(api, setup, expect_download=False)


def clear_history(endpoint, data, probe, entry_id):
    history_request(endpoint, HISTORY, method="DELETE", expected=204)
    assert history_request(endpoint, HISTORY)["entries"] == []
    history_request(endpoint, f"{HISTORY}/{entry_id}", expected=404)
    history_request(endpoint, f"{HISTORY}/by-trace/{probe.trace}", expected=404)
    with closing(
        sqlite3.connect((data / PRIMARY_DB).as_uri() + "?mode=ro", uri=True)
    ) as db:
        assert db.execute("SELECT COUNT(*) FROM answer_history_entries").fetchone() == (
            0,
        )


def verify_release_answer(api, endpoint, data, log, git, secrets, earlier):
    attempts = api("GET", "/v1/status")[0]["outboundAttempts"]
    debug, _ = api("PUT", "/v1/telemetry", {"command": "enableDebug"})
    assert debug["health"]["debugUntil"] > 0
    probe = answer_probe(api, git, secrets)
    content = {
        **earlier,
        **{f"release {key}": value for key, value in probe.content_markers().items()},
    }
    redacted = {**secrets, **content}
    entry_id = verify_saved_history(endpoint, probe, redacted)
    verify_diagnostics(api, [probe], redacted)
    inspected = inspect_history_rows(data, probe, redacted)
    files = inspect_files(data, log, secrets, content, live=True, history_enabled=True)
    assert api("GET", "/v1/status")[0]["outboundAttempts"] == attempts
    clear_history(endpoint, data, probe, entry_id)
    return {
        "traceId": probe.trace,
        "providerAttemptsDuringAnswer": 0,
        "inspectedLogicalTables": inspected,
        "inspectedFiles": files,
    }


def run(bundle, repo, assets, report, release_package=None):
    if not __debug__:
        raise RuntimeError("Privacy qualification requires Python assertions.")
    git = GitHistory(repo, [BASE])
    git.tip = BASE
    token = "github_pat_fixture_privacy_" + uuid4().hex
    with (
        TemporaryDirectory(prefix="ctxfab-query-privacy-") as root,
        origin(git, token=token) as (port, _),
    ):
        data = Path(root) / "data"
        data.mkdir(mode=0o700)
        install_weights(bundle, data, port, assets, report)
        log = report / "backend.log"
        secrets = {
            "provider credential": token.encode(),
            "rejected provider credential": (
                "github_pat_fixture_rejected_" + uuid4().hex
            ).encode(),
        }
        with backend(
            bundle, data, port, report, hub_port=port, preview_runtime=False
        ) as api:
            endpoint = json.loads((data / "endpoint.json").read_text())
            secrets["original session bearer"] = endpoint["token"].encode()
            with private_provider_session(api, secrets) as revision:
                api(
                    "PUT",
                    "/v1/github/repositories",
                    {
                        "revision": revision,
                        "repository": REPOSITORY,
                        "expectedId": REPOSITORY_ID,
                        "branch": "main",
                        "intervalMinutes": 60,
                        "features": FEATURES,
                    },
                )
                wait_for_commit(api, BASE)
                settings, _ = api("GET", SETTINGS)
                assert settings["enabled"] is False
                default = answer_probe(api, git, secrets)
                history_request(
                    endpoint, f"{HISTORY}/by-trace/{default.trace}", expected=404
                )
                assert history_request(endpoint, HISTORY)["entries"] == []
                verify_diagnostics(
                    api, [default], {**secrets, **default.content_markers()}
                )
                table_count = inspect_history_rows(
                    data, None, {**secrets, **default.content_markers()}
                )
                default_files = inspect_files(
                    data, log, secrets, default.content_markers(), live=True
                )

                enabled, _ = api("PUT", SETTINGS, {**settings, "enabled": True})
                assert enabled["enabled"]
                saved = answer_probe(api, git, secrets)
                # Both genuine answers can be identical. The default-off proof above
                # checked its answer before a legitimate opted-in copy could exist.
                content = {
                    **{
                        f"default {key}": value
                        for key, value in default.content_markers().items()
                    },
                    **saved.content_markers(),
                }
                redacted = {**secrets, **content}
                entry = verify_saved_history(endpoint, saved, redacted)
                verify_diagnostics(api, [default, saved], redacted)
                assert inspect_history_rows(data, saved, redacted) == table_count
                enabled_files = inspect_files(
                    data, log, secrets, content, live=True, history_enabled=True
                )

        assert not (data / "endpoint.json").exists()
        stopped_files = inspect_files(data, log, secrets, content, history_enabled=True)
        log.rename(report / "backend-first.log")
        reopened_app, reopened_port, release_manifest = bundle, port, None
        if release_package is not None:
            reopened_app, release_manifest = read_package(
                release_package, Path(root) / "release", None
            )
            reopened_port = None
        with backend(
            reopened_app,
            data,
            reopened_port,
            report,
            hub_port=reopened_port,
            preview_runtime=False,
        ) as api:
            reopened = json.loads((data / "endpoint.json").read_text())
            assert reopened["token"] != endpoint["token"]
            secrets["reopened session bearer"] = reopened["token"].encode()
            state = telemetry_payload(api, "/v1/github", secrets)
            assert state["credential"] == "unconfigured"
            read_manifest(data, git, BASE)
            redacted = {**secrets, **content}
            assert verify_saved_history(reopened, saved, redacted) == entry
            verify_diagnostics(api, [default, saved], redacted)
            assert inspect_history_rows(data, saved, redacted) == table_count
            reopened_files = inspect_files(
                data, log, secrets, content, live=True, history_enabled=True
            )
            clear_history(reopened, data, saved, entry)
            release_answer = None
            if release_manifest is not None:
                release_answer = verify_release_answer(
                    api, reopened, data, log, git, secrets, content
                )
        save(
            report,
            "query-privacy.json",
            {
                "revision": BASE,
                "verifiedCitedAnswers": 2 + (release_answer is not None),
                "usedActualModelContext": True,
                "debugEnabledAtAnswer": True,
                "defaultOffTraceId": default.trace,
                "enabledTraceId": saved.trace,
                "defaultOffFiles": default_files,
                "enabledFiles": enabled_files,
                "inspectedAfterShutdown": stopped_files,
                "inspectedAfterReopen": reopened_files,
                "historyLogicalTablesInspected": table_count,
                "storedOnlyFinalAnswer": True,
                "authenticatedNoStoreHistoryReads": True,
                "browserOriginRejected": True,
                "historyClearedFromActiveStore": True,
                "sessionBearerRotated": True,
                "diagnosticExportPayloadInspected": True,
                "providerCredentialRejectedAndRemoved": True,
                "indexedSnapshotRetained": True,
                "releasePackage": release_manifest,
                "releaseAnswer": release_answer,
            },
        )


if __name__ == "__main__":
    run(*map(Path, sys.argv[1:]))
