"""Qualify the pinned public corpus through the packaged GitHub connector."""

import argparse
import getpass
import json
import re
import shutil
import sqlite3
import sys
import time
import warnings
from pathlib import Path
from tempfile import TemporaryDirectory, mkdtemp
from urllib.request import ProxyHandler, Request, build_opener

from dogfood import (
    BASE,
    FEATURES,
    TIP,
    GitHistory,
    NoRedirect,
    backend,
    judge_actions,
    judge_answers,
    judge_graph,
    judge_history,
    prepare_answers,
    read_manifest,
    record_trace,
    save,
    wait_for_commit,
    wait_until,
)
from git_origin import REPOSITORY, REPOSITORY_ID

GITHUB_STATE = "/v1/github"
CREDENTIAL = GITHUB_STATE + "/credential"
COLLECTION = f"{GITHUB_STATE}/repositories/{REPOSITORY_ID}"
RUN_ID = 44723025441
WORKFLOW_ID = 424872934


class ReferenceChanged(RuntimeError):
    """The upstream evidence changed before the qualification finished."""


class PublicGithub:
    def __init__(self, repository=REPOSITORY):
        self.origin = f"https://api.github.com/repos/{repository}"
        self.opener = build_opener(ProxyHandler({}), NoRedirect())
        self.requests = []
        self.previous_call = 0

    def __call__(self, route):
        time.sleep(max(0, self.previous_call + 1.1 - time.monotonic()))
        self.previous_call = time.monotonic()
        request = Request(self.origin + route, headers={
            "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2026-03-10",
            "User-Agent": "Context-Fabricator-M10-qualification", "Cache-Control": "no-cache",
        })
        with self.opener.open(request, timeout=15) as response:
            self.requests.append({"route": route or "/", "date": response.headers.get("Date"),
                                  "remaining": response.headers.get("X-RateLimit-Remaining")})
            return json.load(response)


def check_branch(public):
    branch = public("/branches/main")
    if branch["name"] != "main" or branch["commit"]["sha"] != TIP:
        raise ReferenceChanged("The public main branch moved during qualification.")


def collect_reference(repo, public):
    repository = public("")
    if (repository["id"], repository["full_name"], repository["default_branch"], repository["private"]) != (
        REPOSITORY_ID, REPOSITORY, "main", False,
    ):
        raise ReferenceChanged("The pinned public repository identity changed.")
    check_branch(public)
    git = GitHistory(repo, [BASE, TIP])
    commit = public(f"/git/commits/{TIP}")
    parents = [item["sha"] for item in commit["parents"]]
    if (commit["sha"], commit["tree"]["sha"], parents) != (TIP, git.roots[TIP], [BASE]):
        raise ReferenceChanged("The hosted Git objects disagree with the pinned local history.")
    attempt = public(f"/actions/runs/{RUN_ID}/attempts/1")
    if (attempt["id"], attempt["run_attempt"], attempt["workflow_id"], attempt["head_sha"],
        attempt["head_branch"], attempt["head_repository"]["id"], attempt["status"],
        attempt["conclusion"]) != (
        RUN_ID, 1, WORKFLOW_ID, TIP, "main", REPOSITORY_ID, "completed", "cancelled",
    ):
        raise ReferenceChanged("The selected hosted Actions observation changed.")
    workflow_path = ".github/workflows/quality.yml"
    workflow = git.files[TIP][workflow_path]
    return git, {
        "repositoryId": REPOSITORY_ID, "repository": REPOSITORY, "branch": "main",
        "base": BASE, "tip": TIP, "tree": git.roots[TIP],
        "attempt": {"id": RUN_ID, "attempt": 1, "workflowId": WORKFLOW_ID, "head": TIP,
                    "status": attempt["status"], "conclusion": attempt["conclusion"]},
        "workflowDefinition": {"path": workflow_path, "blob": workflow["sha"], "revision": TIP},
        "requests": public.requests,
    }


def verify_current(bundle, data, git, api, report, revision, timeout):
    api("PUT", GITHUB_STATE + "/repositories", {
        "revision": revision, "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
        "branch": "main", "intervalMinutes": 60, "features": FEATURES,
    })
    wait_for_commit(api, TIP, timeout=timeout)
    before = read_manifest(data, git, TIP)
    starting_attempts = api("GET", "/v1/status")[0]["outboundAttempts"]
    reply, trace = api("POST", COLLECTION + "/refresh")
    assert reply == {"commit": TIP, "changed": False} and trace
    refresh_attempts = api("GET", "/v1/status")[0]["outboundAttempts"] - starting_attempts
    assert refresh_attempts >= 2
    after = read_manifest(data, git, TIP)
    assert before == after
    save(report, "current-snapshot.json", after)
    record_trace(api, trace, "github_current_code", report, "unchanged-refresh-trace.json")
    judge_answers(bundle, data, git, report)
    return trace, refresh_attempts


def verify_journey(bundle, data, git, api, report, revision, *, timeout=900, model_attempts=0):
    api("PUT", "/v1/retrieval/index", {"enabled": False})
    trace, refresh_attempts = verify_current(bundle, data, git, api, report, revision, timeout)
    judge_history(bundle, data, git, api, report, timeout=timeout)
    judge_actions(bundle, data, git, api, report, timeout=timeout)
    judge_graph(bundle, data, git, api, report, timeout=timeout)
    state, _ = api("GET", GITHUB_STATE)
    status, _ = api("GET", "/v1/status")
    traffic = state["traffic"]
    assert traffic["attempts"] > 0
    assert traffic["attempts"] + model_attempts == status["outboundAttempts"]
    assert traffic["uncertain"] == 0 and traffic["concurrency"] == 1
    return {"unchangedRefreshTrace": trace, "unchangedRefreshAttempts": refresh_attempts,
            "traffic": traffic, "modelDownloadAttempts": model_attempts}


def use_existing_binding(profile, data):
    source = (profile / "context.sqlite3").resolve(strict=True)
    with sqlite3.connect(source.as_uri() + "?mode=ro", uri=True) as read:
        binding = read.execute(
            "SELECT credential_ref, login FROM github_connection "
            "WHERE singleton=1 AND reconnect=0 AND credential_ref IS NOT NULL",
        ).fetchone()
    if binding is None:
        raise RuntimeError("The existing GitHub binding needs a connected credential.")
    with sqlite3.connect(data / "context.sqlite3") as isolated:
        changed = isolated.execute(
            "UPDATE github_connection SET credential_ref=?, login=? "
            "WHERE singleton=1 AND credential_ref IS NULL", binding,
        ).rowcount
        if changed != 1:
            raise RuntimeError("The qualification database must start unconfigured.")


def published_workflow(api):
    state, _ = api("GET", COLLECTION + "/actions")
    if state.get("schedule", {}).get("lastError") or state["availability"] != "ready":
        raise RuntimeError(f"GitHub Actions collection failed: {state['availability']}, "
                           f"{state.get('schedule', {}).get('lastError')}.")
    return state.get("snapshot")


def first_collection_trace(api, report):
    def available():
        telemetry, _ = api("GET", "/v1/telemetry")
        for span in telemetry["traces"]:
            if span["name"] == "github_current_collection" and span["level"] == "ok":
                return span["traceId"]
        return None

    identifier = wait_until(10, available)
    detail = record_trace(api, identifier, "github_current_collection", report, "first-fetch-trace.json")
    provider = [(span.get("httpMethod"), span.get("httpRoute")) for span in detail["spans"]
                if span["name"] == "github_provider_request"]
    assert ("GET", "/{owner}/{repo}.git/info/refs") in provider, provider
    assert ("POST", "/{owner}/{repo}.git/git-upload-pack") in provider, provider
    assert all(not any(part in (route or "") for part in ("/git/blobs/", "/git/trees/"))
               for _, route in provider), provider
    return len(provider)


def qualify_connector(bundle, git, public, report, profile):
    with TemporaryDirectory(prefix="ctxfab-public-connector-") as scratch:
        data = Path(scratch) / "data"
        with backend(bundle, data, None, report):
            use_existing_binding(profile, data)
        with backend(bundle, data, None, report) as api:
            state, _ = api("GET", GITHUB_STATE)
            assert state["credential"] == "connected" and not state["repositories"]
            assert state["traffic"]["concurrency"] == 1
            api("PUT", GITHUB_STATE + "/repositories", {
                "revision": state["revision"], "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
                "branch": "main", "intervalMinutes": 60, "features": FEATURES,
            })
            wait_for_commit(api, TIP)
            snapshot = read_manifest(data, git, TIP)
            save(report, "connector-snapshot.json", snapshot)
            first_requests = first_collection_trace(api, report)
            starting_attempts = api("GET", GITHUB_STATE)[0]["traffic"]["attempts"]
            reply, trace = api("POST", COLLECTION + "/refresh")
            assert reply == {"commit": TIP, "changed": False} and trace
            refresh_requests = api("GET", GITHUB_STATE)[0]["traffic"]["attempts"] - starting_attempts
            detail = record_trace(api, trace, "github_current_code", report, "unchanged-refresh-trace.json")
            assert not any(".git/" in span.get("httpRoute", "") for span in detail["spans"])
            state, _ = api("GET", GITHUB_STATE)
            api("PUT", GITHUB_STATE + "/repositories", {
                "revision": state["revision"], "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
                "branch": "main", "intervalMinutes": 60, "features": {**FEATURES, "actions": True},
            })
            actions = wait_until(120, lambda: published_workflow(api))
            workflow = next(item for item in actions["workflows"] if item["id"] == WORKFLOW_ID)
            run = next(item for item in actions["runs"] if (item["id"], item["attempt"]) == (RUN_ID, 1))
            assert actions["definitionCommit"] == TIP
            entry = git.files[TIP][workflow["path"]]
            assert workflow["definitionBlob"] == entry["sha"]
            assert workflow["definition"].encode() == git.blob(entry["sha"])
            assert (run["headSha"], run["status"], run["conclusion"], run["commitProvenance"]) == (
                TIP, "completed", "cancelled", "verifiedRepositoryCommit",
            )
            save(report, "connector-actions.json", actions)
            traffic = api("GET", GITHUB_STATE)[0]["traffic"]
            assert traffic["uncertain"] == 0 and traffic["concurrency"] == 1
    check_branch(public)
    save(report, "connector-journey.json", {
        "status": "passed", "repository": REPOSITORY, "tip": TIP, "firstFetchRequests": first_requests,
        "unchangedRefreshRequests": refresh_requests, "actionsAttempt": [RUN_ID, 1], "traffic": traffic,
    })


def read_token():
    if not sys.stdin.isatty():
        raise ValueError("Use a terminal for masked entry of the qualification token.")
    with warnings.catch_warnings():
        warnings.simplefilter("error", getpass.GetPassWarning)
        token = getpass.getpass("Dedicated read-only GitHub fine-grained token: ").strip()
    if re.fullmatch(r"github_pat_[A-Za-z0-9_]+", token) is None or len(token) > 512:
        raise ValueError("A GitHub fine-grained token is required.")
    return token


def qualify(bundle, git, public, report):
    token = read_token()
    data = Path(mkdtemp(prefix="private-session-", dir=report))
    disconnected = True
    try:
        with backend(bundle, data, None, report) as api:
            model_attempts = prepare_answers(api, report)
            state, _ = api("GET", GITHUB_STATE)
            assert state["credential"] == "unconfigured" and not state["repositories"]
            assert state["traffic"]["concurrency"] == 1
            assert 0 < state["traffic"]["effectiveRequestsPerMinute"] <= 60
            try:
                disconnected = False
                state, _ = api("PUT", CREDENTIAL, {"revision": state["revision"], "token": token})
                assert state["credential"] == "connected"
                result = verify_journey(
                    bundle, data, git, api, report, state["revision"], model_attempts=model_attempts,
                )
                assert 0 < result["traffic"]["effectiveRequestsPerMinute"] <= 60
                check_branch(public)
            finally:
                state, _ = api("GET", GITHUB_STATE)
                if state["credential"] != "unconfigured":
                    state, _ = api("DELETE", f"{CREDENTIAL}?revision={state['revision']}")
                disconnected = state["credential"] == "unconfigured"
        if not disconnected or cleanup_warning(report):
            raise RuntimeError("The qualification credential cleanup needs attention in the isolated account.")
    finally:
        if disconnected and not cleanup_warning(report):
            shutil.rmtree(data)
        else:
            print(f"Private qualification directory retained for cleanup at {data}.", file=sys.stderr)
    save(report, "hosted-journey.json", {"status": "passed", "tip": TIP,
         "publicReferenceChecks": public.requests, **result})


def cleanup_warning(report):
    log = report / "backend.log"
    return log.is_file() and "github_keychain_cleanup" in log.read_text()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--bundle", type=Path, help="Run the hosted qualification with this app bundle.")
    parser.add_argument("--existing-profile", type=Path,
                        help="Run connector-only qualification with that profile's Keychain binding.")
    args = parser.parse_args()
    if args.existing_profile and not args.bundle:
        parser.error("--existing-profile requires --bundle")
    if any((args.report / name).exists() for name in ("hosted-reference.json", "hosted-journey.json")):
        parser.error("Select a fresh hosted report directory.")
    public = PublicGithub()
    try:
        git, reference = collect_reference(args.repo, public)
        args.report.mkdir(mode=0o700, parents=True, exist_ok=True)
        save(args.report, "hosted-reference.json", reference)
        if args.existing_profile:
            qualify_connector(args.bundle.resolve(strict=True), git, public, args.report.resolve(),
                              args.existing_profile.resolve(strict=True))
        elif args.bundle:
            qualify(args.bundle.resolve(strict=True), git, public, args.report.resolve())
    except ReferenceChanged as error:
        parser.exit(2, f"{error}\n")
