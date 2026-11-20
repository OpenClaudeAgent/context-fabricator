"""Deterministic success and typed rejection against the disposable Rust service."""

import os
import sys

import requests
import schemathesis
from schemathesis.checks import (
    content_type_conformance,
    response_schema_conformance,
    status_code_conformance,
)

schema = schemathesis.openapi.from_path(sys.argv[1])
operation = schema["/v1/settings"]["PUT"]
checks = [status_code_conformance, content_type_conformance, response_schema_conformance]
base_url = os.environ["CTXFAB_TEST_URL"]
headers = {"Authorization": f"Bearer {os.environ['CTXFAB_TEST_TOKEN']}"}

with requests.Session() as session:
    session.trust_env = False
    for hours, expected_status in [(24, 200), (0, 400)]:
        case = operation.Case(
            body={"revision": 0, "answerLanguage": "auto", "diagnosticRetentionHours": hours},
            media_type="application/json",
        )
        response = case.call_and_validate(
            base_url=base_url,
            headers=headers,
            session=session,
            checks=checks,
            timeout=3,
        )
        body = response.json()
        if response.status_code != expected_status:
            raise ValueError(f"Expected HTTP {expected_status}, got {response.status_code}")
        if expected_status == 200 and body.get("diagnosticRetentionHours") != hours:
            raise ValueError("The live service did not return the saved settings")
        if expected_status == 400 and body.get("code") != "invalid_settings":
            raise ValueError("The live service did not return a structured settings rejection")
        print(f"Schemathesis validated HTTP {expected_status} for PUT /v1/settings")
