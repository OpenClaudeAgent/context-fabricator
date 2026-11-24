"""Deterministic request sequences against the disposable Rust service."""

import os
import sys
import time

import requests
import schemathesis
from schemathesis.checks import (
    content_type_conformance,
    response_schema_conformance,
    status_code_conformance,
)


def examples(schema, session):
    base_url = os.environ["CTXFAB_TEST_URL"]
    headers = {"Authorization": f"Bearer {os.environ['CTXFAB_TEST_TOKEN']}"}
    checks = [status_code_conformance, content_type_conformance, response_schema_conformance]

    def check(method, path, expected, *, body=None, report=True, **parameters):
        if body is not None:
            parameters.update(body=body, media_type="application/json")
        response = schema[path][method].Case(**parameters).call_and_validate(
            base_url=base_url,
            headers=headers,
            session=session,
            checks=checks,
            timeout=10,
        )
        assert response.status_code == expected, (
            f"{method} {path}: expected HTTP {expected}, got {response.status_code}"
        )
        if report:
            print(f"Schemathesis validated HTTP {expected} for {method} {path}")
        return response.json()

    settings = {"revision": 0, "answerLanguage": "auto", "diagnosticRetentionHours": 24}
    saved = check("PUT", "/v1/settings", 200, body=settings)
    assert saved["diagnosticRetentionHours"] == 24
    invalid = check("PUT", "/v1/settings", 400, body={**settings, "diagnosticRetentionHours": 0})
    assert invalid["code"] == "invalid_settings"

    storage = "/v1/github/storage"
    budget = check("GET", storage, 200)
    limit = 2 if budget["limitGb"] != 2 else 3
    change = {"revision": budget["revision"], "limitGb": limit}
    updated = check("PUT", storage, 200, body=change)
    assert updated == {"revision": budget["revision"] + 1, "limitGb": limit, "bytes": 0}
    stale = check("PUT", storage, 409, body={**change, "limitGb": 5})
    assert stale["code"] == "github_revision_conflict"
    overflow = check("PUT", storage, 400, body={**change, "revision": 2**32})
    assert overflow["code"] == "github_invalid_storage_limit"
    current = check("GET", storage, 200)
    assert current == updated

    github = check("GET", "/v1/github", 200)
    assert github["credential"] == "unconfigured"
    token = "github_pat_fixture_primary"
    connected = check("PUT", "/v1/github/credential", 200, body={"revision": github["revision"], "token": token})
    assert (connected["credential"], connected["login"]) == ("connected", "fixture-user")
    assert token not in repr(connected)
    repository = check("PUT", "/v1/github/repositories", 200, body={
        "revision": connected["revision"], "repository": "https://github.com/fixture/alpha.git",
        "expectedId": 41, "intervalMinutes": 60,
        "features": {"currentCode": True, "history": False, "actions": False, "codeGraph": True},
    })
    saved_repo, = repository["repositories"]
    assert saved_repo["repository"]["id"] == 41 and saved_repo["branch"] == "main" and saved_repo["verified"]
    graph_route = "/v1/github/repositories/{id}/graph"
    graph = check("GET", graph_route, 200, path_parameters={"id": 41}, query={"branch": "main"})
    assert graph == {"repositoryId": 41, "branch": "main", "access": "available", "snapshot": None}
    invalid = check(
        "GET", graph_route, 400, path_parameters={"id": 41}, query={"branch": "main", "symbol": "main"},
    )
    assert invalid["code"] == "github_invalid_selection"

    invalid = check("POST", "/v1/runtime/install", 400, body={"archivePath": "relative.zip"})
    assert (invalid["code"], invalid["field"]) == ("invalid_runtime_archive", "archivePath")
    invalid = check(
        "POST", "/v1/inference/setup", 400,
        body={
            "selection": {"embedding": False, "runtimeSource": "hosted"},
            "fingerprint": "A" * 64,
            "operationId": "5d6c0600-5609-42d0-9435-8f240807fe08",
        },
    )
    assert (invalid["code"], invalid["field"]) == ("invalid_inference_setup", "fingerprint")

    catalog = check("GET", "/v1/model-catalog", 200, query={"query": "catalog"})
    assert len(catalog) == 1, "Expected the loopback catalog repository"
    model, = catalog
    assert model["repository"] == "fixture/catalog-qwen"
    selection = {"repository": model["repository"], "revision": model["revision"]}
    invalid = check(
        "GET", "/v1/model-catalog/variants", 400,
        query={**selection, "repository": "invalid..owner/name"},
    )
    assert invalid["code"] == "invalid_model_discovery"
    variants = check("GET", "/v1/model-catalog/variants", 200, query=selection)
    assert len(variants) == 1, "Expected the loopback GGUF variant"
    variant, = variants
    assert variant["quantization"] == "Q4_K_M"
    downloads = "/v1/model-downloads"
    selected = check("POST", "/v1/model-catalog/downloads", 202, body={**selection, "file": variant["file"]})
    profile = selected["profile"]
    assert profile.startswith("hf-")
    accepted = check("POST", downloads, 202, body=selected)
    assert accepted == selected

    deadline = time.monotonic() + 10
    while True:
        models = check("GET", downloads, 200, report=False)
        installed = next(model for model in models if model["profile"] == profile)
        if installed["state"] == "installed":
            break
        assert installed["state"] == "installing" and time.monotonic() < deadline, installed["state"]
        time.sleep(0.1)
    assert installed["downloadedBytes"] == installed["totalBytes"] == variant["bytes"]
    print("Schemathesis validated the installed model and exact downloaded size")

    plan = check("GET", downloads + "/{profile}/removal-plan", 200, path_parameters=selected)
    assert plan["profile"] == profile and plan["bytes"] == variant["bytes"] and plan["identity"]
    active = check("PUT", "/v1/generation/models", 200, body=selected)
    assert active["activeProfile"] == profile
    assert any(model["id"] == profile for model in active["profiles"])
    current = check("GET", "/v1/generation/models", 200)
    assert current == active


with requests.Session() as session:
    session.trust_env = False
    examples(schemathesis.openapi.from_path(sys.argv[1]), session)
