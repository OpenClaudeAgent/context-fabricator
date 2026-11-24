"""Measure natural Answers questions with pinned code, diffs, graph and Actions evidence."""

import argparse
import hashlib
import json
import os
import subprocess
import tempfile
import time
from collections import defaultdict
from pathlib import Path
from statistics import median
from urllib.parse import urlencode

import feature_graph
from dogfood import (
    BASE,
    TIP,
    prepare_answers,
    record_trace,
    verify_quote_link,
    wait_until,
)
from features import (
    HERE,
    REFERENCE,
    backend,
    inspect_git,
    origin,
    save,
    wait_for_commit,
)
from git_origin import FIXTURE_TOKEN, REPOSITORY, REPOSITORY_ID, GitHistory
from review import sheet
from run import request_answer

GOLD = HERE / "family-answers-gold.json"
QWEN = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"


def source_text(git, revision, path):
    oid = git.files[revision][path]["sha"]
    return oid, git.blob(oid)


def workflow_fields(run, workflow):
    reported_head = run.get("head_repository") or {}
    fields = {
        f"{label}: {value}"
        for label, value in (
            ("Workflow ID", run["workflow_id"]),
            ("Run ID", run["id"]),
            ("Attempt", run["run_attempt"]),
            ("Event", run["event"]),
            ("Status", run["status"]),
            ("Conclusion", run["conclusion"]),
            ("Created", run["created_at"]),
            ("Updated", run["updated_at"]),
            ("Reported head commit", run["head_sha"]),
            ("Head provenance", "verifiedRepositoryCommit"),
            ("Observed workflow name", workflow["name"]),
            ("Observed workflow path", workflow["path"]),
        )
    }
    if run.get("head_branch") is not None:
        fields.add(f"Reported head branch: {run['head_branch']}")
    if reported_head.get("id") is not None:
        fields.add(f"Reported head repository ID: {reported_head['id']}")
    return fields


def check_citation(citation, git, repository, repository_id, workflows):
    evidence, quote = citation["evidence"], citation["quote"]
    if evidence["kind"] == "workflow":
        identity = citation["workflowRun"]
        key = (identity["runId"], identity["attempt"])
        run = workflows[key]
        assert identity["repositoryId"] == repository_id
        assert identity["workflowId"] == run["workflow_id"]
        assert identity["reportedHead"] == run["head_sha"]
        assert (
            identity["runUrl"]
            == f"https://github.com/{repository}/actions/runs/{key[0]}/attempts/{key[1]}"
        )
        assert set(quote.splitlines()) <= workflow_fields(run, workflows["workflow"])
        return {"kind": "workflow", "runId": key[0], "attempt": key[1], "quote": quote}

    loc = evidence["location"]
    path, commit = loc["path"], evidence["revision"]
    if evidence["kind"] == "history":
        comparison = citation["comparison"]
        tip, parent = git.commits[:2]
        assert (comparison["parent"], comparison["child"]) == (parent, tip)
        side = comparison["side"]
        assert commit == {"before": parent, "after": tip}[side]
    else:
        assert evidence["kind"] == "current" and commit == git.tip
        side = None
    oid, raw = source_text(git, commit, path)
    assert loc["blob"] == oid
    assert loc["fileUrl"] == f"https://github.com/{repository}/blob/{commit}/{path}"
    start, end = loc["startByte"], loc["endByte"]
    assert 0 <= start < end <= len(raw)
    excerpt = raw[start:end]
    assert loc["startLine"] == 1 + raw.count(b"\n", 0, start)
    assert loc["endLine"] == raw.count(b"\n", 0, end) + (not excerpt.endswith(b"\n"))
    verify_quote_link(citation, excerpt.decode())
    return {"kind": evidence["kind"], "path": path, "side": side, "quote": quote}


def validate_anchors(cases, git, workflows):
    for case in cases:
        assert set(case["questions"]) == {"en", "fr"}
        assert bool(case["facts"]) != bool(case.get("insufficientReason"))
        for fact in case["facts"]:
            assert fact["anchors"] and fact["expected"]
            for anchor in fact["anchors"]:
                literal = anchor["contains"]
                if anchor["kind"] == "workflow":
                    run = workflows[anchor["runId"], anchor["attempt"]]
                    assert literal in workflow_fields(run, workflows["workflow"])
                    continue
                revision = git.commits[1] if anchor.get("side") == "before" else git.tip
                assert literal.encode() in source_text(git, revision, anchor["path"])[1]


def covers(citation, anchor):
    return anchor["contains"] in citation["quote"] and all(
        citation.get(key) == value for key, value in anchor.items() if key != "contains"
    )


def configure(api, git, repository, repository_id, features, report, *, external=False):
    api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
    api(
        "PUT",
        "/v1/github/repositories",
        {
            "revision": 1,
            "repository": repository,
            "expectedId": repository_id,
            "branch": "main",
            "intervalMinutes": 1440,
            "features": features,
        },
    )
    prefix = f"/v1/github/repositories/{repository_id}"
    if features["currentCode"] or (external and features["codeGraph"]):
        wait_for_commit(api, git.tip, timeout=900)
    failures = {}
    if features["history"]:

        def published_lineage():
            return api("GET", prefix + "/history")[0].get("snapshot")

        def published_comparison():
            return api("GET", f"{prefix}/history/evidence/{git.tip}")[0]

        def history_ready():
            state = api("GET", prefix + "/history")[0]
            if state.get("snapshot"):
                return state
            schedule = state.get("schedule") or {}
            return state if schedule.get("lastError") else None

        try:
            if external:
                state = wait_until(45, history_ready)
                save(report, "history-state.json", state)
                lineage = state.get("snapshot")
                if not lineage:
                    raise RuntimeError("First-parent lineage collection failed.")
            else:
                lineage = wait_until(120, published_lineage)
            assert (
                lineage["tip"] == git.tip
                and lineage["commits"][0]["parents"][0] == git.commits[1]
            )
            prepared = wait_until(45 if external else 120, published_comparison)
            assert (prepared["parent"], prepared["child"]) == (git.commits[1], git.tip)
            save(report, "history.json", prepared)
        except (RuntimeError, TimeoutError) as error:
            if not external:
                raise
            failures["history"] = str(error)
            save(report, "history-state.json", api("GET", prefix + "/history")[0])
    if features["actions"]:

        def published_actions():
            return api("GET", prefix + "/actions")[0].get("snapshot")

        actions = wait_until(120, published_actions)
        assert (
            actions["definitionCommit"] == git.tip
            and actions["executedDefinition"] == "unknown"
        )
        save(report, "actions.json", actions)
    return prefix, failures


def required_features(case, exact):
    kinds = {
        anchor["kind"]
        for anchor in [
            *(anchor for fact in case["facts"] for anchor in fact["anchors"]),
            *exact.get("scopedControls", []),
        ]
    }
    return {
        "currentCode": "current" in kinds or "graph" in exact,
        "history": "history" in kinds,
        "actions": "workflow" in kinds,
        "codeGraph": "graph" in exact,
    }


def verify_graph(api, prefix, git, repository, cases, anchors, report):
    observations = {}
    failures = {}
    for case in cases:
        expected = anchors[case["id"]].get("graph")
        if not expected:
            continue
        symbol = expected.get("qualifiedName", expected.get("symbol"))
        params = urlencode(
            {"branch": "main", "root": expected["root"], "symbol": symbol}
        )
        state = api("GET", prefix + "/graph?" + params)[0]
        observations[case["id"]] = state
        snapshot = state.get("snapshot") or {}
        hits = [
            hit
            for hit in snapshot.get("hits", [])
            if hit["role"] == expected["requiredRole"]
            and hit["resolution"] == expected.get("resolution", "definition")
            and hit.get("qualifiedName") == expected.get("qualifiedName")
            and hit["name"] == symbol.split("::")[-1]
        ]
        if (
            snapshot.get("revision") != git.tip
            or snapshot.get("coverage") != "published"
            or not hits
        ):
            failures[case["id"]] = "Expected pinned Rust graph occurrence unavailable."
            continue
        for hit in hits:
            site = hit["site"]
            path = site["path"]
            blob, source = source_text(git, git.tip, path)
            start, end = site["startByte"], site["endByte"]
            assert site["blob"] == blob
            assert (
                site["fileUrl"]
                == f"https://github.com/{repository}/blob/{git.tip}/{path}"
            )
            assert source[start:end] == hit["name"].encode()
            assert (
                site["startLine"] == site["endLine"] == source[:start].count(b"\n") + 1
            )
            if expected.get("resolution") == "unresolved":
                assert "target" not in hit
    save(report, "graph-prerequisites.json", observations)
    return failures


def evaluate(
    api,
    git,
    repository,
    repository_id,
    workflows,
    cases,
    report,
    *,
    external=False,
    blocked=None,
):
    validate_anchors(cases, git, workflows)
    rows = []
    for case in cases:
        for language, question in case["questions"].items():
            if blocked and case["id"] in blocked:
                rows.append(
                    {
                        "id": case["id"],
                        "family": case["family"],
                        "language": language,
                        "runtimeEligible": False,
                        "prerequisiteErrors": blocked[case["id"]],
                    }
                )
                save(report, "rows.json", rows)
                continue
            started = time.monotonic()
            query = {"query": question, "includeModelContext": True}
            if external:
                query.update(repositoryId=repository_id, branch="main")
            answer, trace, problem = request_answer(api, query)
            elapsed = round((time.monotonic() - started) * 1000, 2)
            if problem:
                answer = {
                    "mode": "bm25",
                    "outcome": "requestError",
                    "citations": [],
                    "evidenceReview": {"reason": problem["problem"]["code"]},
                }
            detail = (
                record_trace(
                    api,
                    trace,
                    "api_answer",
                    report,
                    f"{case['id']}-{language}.trace.json",
                )
                if trace
                else {"spans": []}
            )
            citations = [
                check_citation(c, git, repository, repository_id, workflows)
                for c in answer["citations"]
            ]
            rows.append(
                {
                    "id": case["id"],
                    "family": case["family"],
                    "language": language,
                    "mode": answer["mode"],
                    "answerMs": elapsed,
                    "answer": answer,
                    "requestError": problem,
                    "trace": trace,
                    "requiredFactsQuoted": [
                        all(
                            any(covers(c, anchor) for c in citations)
                            for anchor in fact["anchors"]
                        )
                        for fact in case["facts"]
                    ],
                    "graphUsed": any(
                        event["name"] == "Rust graph passages selected"
                        for span in detail["spans"]
                        for event in span.get("events", [])
                    ),
                }
            )
            if external:
                assert answer["mode"] == "bm25", answer["mode"]
                rows[-1]["runtimeEligible"] = True
            save(report, "rows.json", rows)
    return rows


def run_suite(
    bundle,
    assets,
    work,
    report,
    git,
    repository,
    repository_id,
    captured,
    cases,
    removed=(),
    *,
    exact_anchors=None,
):
    report.mkdir()
    data = work / report.name
    data.mkdir(mode=0o700)
    inference = data / "inference"
    inference.mkdir(mode=0o700)
    subprocess.run(
        ["/bin/cp", "-c", (assets / QWEN).resolve(strict=True), inference / QWEN],
        check=True,
    )
    workflows = (
        {(run["id"], run["run_attempt"]): run for run in captured["runs"]}
        if captured
        else {}
    )
    if captured:
        workflows["workflow"] = captured["workflow"]
    external = exact_anchors is not None
    needed = (
        {
            case["id"]: required_features(case, exact_anchors[case["id"]])
            for case in cases
        }
        if external
        else {}
    )
    features = (
        {
            feature: any(requirements[feature] for requirements in needed.values())
            for feature in ("currentCode", "history", "actions", "codeGraph")
        }
        if external
        else {
            "currentCode": repository != "astral-sh/uv",
            "history": repository == REPOSITORY,
            "actions": captured is not None,
            "codeGraph": bool(removed),
        }
    )
    assert not features["actions"] or captured is not None
    with (
        origin(
            git,
            repository=repository,
            repository_id=repository_id,
            actions_reference=captured,
        ) as (port, requests),
        backend(
            bundle, data, port, report, hub_port=port, preview_runtime=False
        ) as api,
    ):
        prepare_answers(api, report, expect_download=False)
        prefix, failures = configure(
            api, git, repository, repository_id, features, report, external=external
        )
        blocked = {
            case_id: {
                feature: error for feature, error in failures.items() if flags[feature]
            }
            for case_id, flags in needed.items()
        }
        if external and features["codeGraph"]:
            for case_id, error in verify_graph(
                api, prefix, git, repository, cases, exact_anchors, report
            ).items():
                blocked[case_id]["codeGraph"] = error
        blocked = {case_id: errors for case_id, errors in blocked.items() if errors}
        if external:
            save(
                report,
                "feature-prerequisites.json",
                {"configured": features, "blocked": blocked},
            )
        before = len(requests)
        rows = evaluate(
            api,
            git,
            repository,
            repository_id,
            workflows,
            cases,
            report,
            external=external,
            blocked=blocked,
        )
        assert (
            api("GET", "/v1/status")[0]["outboundAttempts"] == len(requests) == before
        )
        if removed:
            original = git.tip
            git.tip = git.commits[0]
            assert git.tip != original
            refreshed, _ = api("POST", prefix + "/refresh")
            assert refreshed == {"commit": git.tip, "changed": True}
            assert feature_graph.TARGET not in git.files[git.tip]
            before = len(requests)
            removed_report = report / "after-removal"
            removed_report.mkdir()
            rows.extend(
                evaluate(
                    api,
                    git,
                    repository,
                    repository_id,
                    workflows,
                    removed,
                    removed_report,
                )
            )
            assert (
                api("GET", "/v1/status")[0]["outboundAttempts"]
                == len(requests)
                == before
            )
    return rows


def external_suites(args, manifest, uv, ref, actions, groups):
    assert set(groups) <= {"uv", "context-fabricator"}
    assert (
        manifest["repository"],
        manifest["repositoryId"],
        manifest["commit"],
        manifest["firstParent"],
    ) == (ref["repository"], ref["repositoryId"], uv.tip, uv.commits[1])
    if groups["uv"]:
        lineage = uv.git("rev-list", "--first-parent", "--max-count=16", uv.tip)
        commits = lineage.decode().splitlines()
        assert commits[:2] == uv.commits[:2]
        history = manifest["historyReference"]
        assert history["firstParentCommits"] == commits
        assert history["tree"] == uv.roots[uv.tip]
        published = GitHistory(args.uv, commits)
        assert published.parents[commits[-1]][0] == history["firstExcludedParent"]
        assert history["firstExcludedParent"] not in published.commits
        yield (
            "uv",
            published,
            ref["repository"],
            ref["repositoryId"],
            actions,
        )
    if groups["context-fabricator"]:
        graph = manifest["graphReference"]
        project = GitHistory(args.project, [graph["commit"]])
        assert project.files[project.tip]["LICENSE"]["sha"] == graph["licenseBlob"]
        assert (graph["repository"], graph["repositoryId"]) == (
            REPOSITORY,
            REPOSITORY_ID,
        )
        yield "context-fabricator", project, REPOSITORY, REPOSITORY_ID, None


def run(args):
    dataset = (args.gold or GOLD).read_bytes()
    cases = json.loads(dataset)["cases"]
    assert len({case["id"] for case in cases}) == len(cases)
    manifest = (
        json.loads(args.exact_anchors.read_bytes()) if args.exact_anchors else None
    )
    anchors = {case["id"]: case for case in manifest["cases"]} if manifest else {}
    if manifest:
        assert manifest["version"] == 1 and set(anchors) == {
            case["id"] for case in cases
        }
    selected_cases = [
        case for case in cases if args.corpus is None or case["corpus"] == args.corpus
    ]
    if not selected_cases:
        raise ValueError("The selected corpus has no questions in the frozen dataset.")
    args.report.mkdir(parents=True, exist_ok=False)
    if args.exact_anchors:
        save(
            args.report,
            "run-identity.json",
            {
                "goldSha256": hashlib.sha256(dataset).hexdigest(),
                "exactAnchorsSha256": hashlib.sha256(
                    args.exact_anchors.read_bytes()
                ).hexdigest(),
                "binaries": {
                    name: hashlib.sha256(
                        (args.bundle / "Contents" / "MacOS" / name).read_bytes()
                    ).hexdigest()
                    for name in ("ctxfab", "ctxfab-server")
                },
                "retrievalMode": "bm25",
                "corpusFilter": args.corpus,
            },
        )
    save(args.report, "questions.json", json.loads(dataset))
    ref = json.loads(REFERENCE.read_text())
    uv, _ = inspect_git(args.uv, ref)
    capture = (HERE / "captured-attempt.json").read_bytes()
    run_fixture = json.loads(capture)
    assert run_fixture["run"] == ref["run"]
    actions = {"workflow": ref["workflow"], "runs": [run_fixture["run"]]}
    if manifest:
        assert manifest["capturedAttemptSha256"] == hashlib.sha256(capture).hexdigest()
    groups = defaultdict(list)
    for case in selected_cases:
        groups[case["corpus"]].append(case)
    rows = []
    with tempfile.TemporaryDirectory(prefix="ctxfab-family-answers-") as directory:
        work = Path(directory)
        suites = (
            external_suites(args, manifest, uv, ref, actions, groups)
            if manifest
            else legacy_suites(args, work, uv, ref, actions)
        )
        for name, git, repository, rid, captured in suites:
            rows.extend(
                run_suite(
                    args.bundle,
                    args.assets,
                    work,
                    args.report / name,
                    git,
                    repository,
                    rid,
                    captured,
                    groups[name],
                    groups["rust-graph-after-removal"] if name == "rust-graph" else (),
                    exact_anchors=anchors if manifest else None,
                )
            )
            save(args.report, "rows.json", rows)
    assert len(rows) == 2 * len(selected_cases)
    if any("answer" in row for row in rows):
        save(args.report, "review.json", sheet(rows, cases))
    by_id = {case["id"]: case for case in cases}
    groups.clear()
    for row in rows:
        groups[row["family"]].append(row)
    save(
        args.report,
        "summary.json",
        {
            "goldSha256": hashlib.sha256(dataset).hexdigest(),
            "answerModel": QWEN,
            "questions": len(rows),
            "families": {
                kind: summarize_family(items, by_id, external=bool(manifest))
                for kind, items in groups.items()
            },
        },
    )


def legacy_suites(args, work, uv, ref, actions):
    lineage = subprocess.check_output(
        [
            "git",
            "-C",
            args.project,
            "rev-list",
            "--first-parent",
            "--max-count=16",
            TIP,
        ],
        text=True,
    ).splitlines()
    assert lineage[:2] == [TIP, BASE]
    project = GitHistory(args.project, lineage)
    assert project.parents[TIP] == [BASE]
    project_actions = json.loads((HERE.parent / "actions-reference.json").read_text())
    graph = feature_graph.fixture_history(work / "graph-source")
    graph.tip = graph.commits[1]
    yield "context-fabricator", project, REPOSITORY, REPOSITORY_ID, project_actions
    yield "uv", uv, ref["repository"], ref["repositoryId"], actions
    yield (
        "rust-graph",
        graph,
        feature_graph.REPOSITORY,
        feature_graph.REPOSITORY_ID,
        None,
    )


def summarize_family(items, by_id, *, external):
    evaluated = [row for row in items if row.get("runtimeEligible", True)]
    result = {
        "questions": len(items),
        "answerable": sum(bool(by_id[row["id"]]["facts"]) for row in evaluated),
        "completeAnchorCoverage": sum(
            bool(row["requiredFactsQuoted"]) and all(row["requiredFactsQuoted"])
            for row in evaluated
        ),
        "correctAbstentions": sum(
            not by_id[row["id"]]["facts"]
            and row["answer"]["outcome"] == "insufficientEvidence"
            and row["answer"].get("evidenceReview", {}).get("reason")
            in {"noCandidates", "noUsableCandidates", "noCitations"}
            for row in evaluated
        ),
        "requestErrors": sum(row["requestError"] is not None for row in evaluated),
        "graphUsed": sum(row["graphUsed"] for row in evaluated),
        "medianAnswerMs": median(row["answerMs"] for row in evaluated)
        if evaluated
        else None,
    }
    if external:
        result.update(
            evaluated=len(evaluated), prerequisiteBlocked=len(items) - len(evaluated)
        )
    return result


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("project", "uv", "bundle", "assets", "report"):
        parser.add_argument(name, type=Path)
    parser.add_argument("--gold", type=Path)
    parser.add_argument("--exact-anchors", type=Path)
    parser.add_argument("--corpus", choices=("uv", "context-fabricator"))
    args = parser.parse_args()
    if bool(args.gold) != bool(args.exact_anchors):
        parser.error("Supply --gold and --exact-anchors together.")
    if args.corpus and not args.gold:
        parser.error("--corpus applies to an external frozen dataset.")
    run(args)
