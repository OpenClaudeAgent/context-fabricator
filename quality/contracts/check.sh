#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"

require_mac
for tool in java jq uv uvx k6 python3; do
    command -v "$tool" > /dev/null || { echo "Contract checks require $tool." >&2; exit 1; }
done
mkdir -p "$quality_dir/reports"
quality_begin_report reports contract
work=$(quality_workspace contract-work)
echo "Contract reports in $reports"
backend_pid=
proxy_pid=
hub_pid=
github_pid=
cleanup() {
    if [[ -n $proxy_pid ]]; then kill "$proxy_pid" 2>/dev/null || true; wait "$proxy_pid" 2>/dev/null || true; fi
    if [[ -n $backend_pid ]]; then kill "$backend_pid" 2>/dev/null || true; wait "$backend_pid" 2>/dev/null || true; fi
    if [[ -n $hub_pid ]]; then kill "$hub_pid" 2>/dev/null || true; wait "$hub_pid" 2>/dev/null || true; fi
    if [[ -n $github_pid ]]; then kill "$github_pid" 2>/dev/null || true; wait "$github_pid" 2>/dev/null || true; fi
    rm -rf -- "$work"
}
quality_on_exit cleanup
contract="$quality_root/contracts/openapi.yaml"
java -jar "$tools_dir/openapi/generator.jar" validate -i "$contract" | tee "$reports/schema.log"
baseline="$quality_root/contracts/baselines/v1.yaml"
oasdiff breaking "$baseline" "$contract" --fail-on ERR > "$reports/compatible.log"
sed 's@/v1/diagnostics:@/v1/removed-diagnostics:@' "$contract" > "$work/broken.yaml"
if oasdiff breaking "$contract" "$work/broken.yaml" --fail-on ERR > "$reports/breaking.log"; then
    echo "oasdiff accepted the removal of an API path." >&2
    exit 1
fi
grep -F '[api-path-removed-without-deprecation]' "$reports/breaking.log"
grep -Fi 'GET /v1/diagnostics' "$reports/breaking.log"

cargo build --manifest-path "$quality_root/backend/Cargo.toml" --locked -p ctxfab-server
cargo build --manifest-path "$quality_root/cli/Cargo.toml" --locked
mkdir "$work/hugging-face" "$work/github"
mkdir -m 700 "$work/data"
mkdir "$work/data/inference"
install -m 755 "$quality_root/quality/answers/mock_worker.py" "$work/data/inference/llama-server"
python3 "$quality_root/quality/models/hugging_face_fixture.py" "$work/hugging-face" \
    > "$reports/hugging-face.log" 2>&1 &
hub_pid=$!
python3 "$quality_root/quality/github/github_fixture.py" "$work/github" \
    > "$reports/github.log" 2>&1 &
github_pid=$!
wait_for_fixture_port() {
    local port_file=$1 pid=$2 attempt
    for (( attempt=0; attempt < 100; attempt++ )); do
        [[ -s $port_file ]] && return 0
        kill -0 "$pid" || break
        sleep 0.05
    done
    echo "Fixture did not publish a port at $port_file." >&2
    return 1
}
wait_for_fixture_port "$work/hugging-face/port" "$hub_pid"
wait_for_fixture_port "$work/github/port" "$github_pid"
CTXFAB_INFERENCE_DIRECTORY="$work/data/inference" \
    CTXFAB_TEST_HUGGING_FACE_PORT="$(< "$work/hugging-face/port")" \
    "$quality_root/backend/target/debug/ctxfab-server" "$work/data" \
    --github-fixture "$(< "$work/github/port")" > "$reports/backend.log" 2>&1 &
backend_pid=$!
for (( attempt=0; attempt < 60; attempt++ )); do
    [[ -f "$work/data/endpoint.json" ]] && break
    kill -0 "$backend_pid"
    sleep 0.1
done
endpoint="$work/data/endpoint.json"
CTXFAB_TEST_URL=$(jq -er .url "$endpoint")
CTXFAB_TEST_TOKEN=$(jq -er .token "$endpoint")
export CTXFAB_TEST_URL CTXFAB_TEST_TOKEN
request() {
    curl --noproxy '*' --silent --show-error --max-time 3 --header "Authorization: Bearer $CTXFAB_TEST_TOKEN" "$@"
}
status=$(curl --noproxy '*' --silent --show-error --max-time 3 --output "$reports/unauthorized.json"     --write-out '%{http_code}' "$CTXFAB_TEST_URL/v1/status")
[[ $status == 401 ]]
jq -e '.code == "unauthorized"' "$reports/unauthorized.json" > /dev/null
request --fail "$CTXFAB_TEST_URL/v1/status" | jq -e '.status == "ready"' > /dev/null
CTXFAB_DATA_DIRECTORY="$work/data" "$quality_root/cli/target/debug/ctxfab" --trace status \
    2> "$reports/cli-trace.log" |
    tee "$reports/cli.json" | jq -e '.status == "ready" and .connectedSources == 0' > /dev/null
cli_trace=$(sed -n 's/^ctxfab: trace \([0-9a-f]\{32\}\)$/\1/p' "$reports/cli-trace.log")
[[ -n "$cli_trace" ]]
cli_trace_found=false
for (( attempt=0; attempt < 40; attempt++ )); do
    if request --fail "$CTXFAB_TEST_URL/v1/telemetry/traces/$cli_trace" |
        jq -e --arg id "$cli_trace" '.spans | any(.traceId == $id and .httpMethod == "GET" and .httpRoute == "/v1/status" and .httpStatus == 200 and .clientKind == "cli")' \
            > /dev/null; then
        cli_trace_found=true
        break
    fi
    sleep 0.05
done
[[ $cli_trace_found == true ]]
uv run --isolated --with schemathesis==4.33.4 python     "$quality_root/quality/contracts/contract-examples.py" "$contract" |
    tee "$reports/contract-examples.log"
NO_PROXY=127.0.0.1,localhost uvx --from schemathesis==4.33.4 schemathesis run "$contract" --url "$CTXFAB_TEST_URL"     --header "Authorization: Bearer $CTXFAB_TEST_TOKEN" --phases fuzzing --max-examples 8     --checks status_code_conformance,content_type_conformance,response_schema_conformance     > "$reports/conformance.log" 2>&1 || { cat "$reports/conformance.log"; exit 1; }
cat "$reports/conformance.log"

k6 run --quiet "$quality_root/quality/contracts/load.js" | tee "$reports/load.log"
load_exit=0
CTXFAB_IMPOSSIBLE=1 k6 run --quiet "$quality_root/quality/contracts/load.js"     > "$reports/load-red.log" 2>&1 || load_exit=$?
if [[ $load_exit != 99 ]] || ! grep -F "thresholds on metrics 'http_req_duration'" "$reports/load-red.log"; then
    echo "k6 did not reject the impossible duration threshold (exit $load_exit)." >&2
    exit 1
fi

# Reserve independent loopback addresses for the fault fixture.
read -r control_port proxy_port < <(python3 - <<'PY'
import socket
with socket.socket() as control, socket.socket() as proxy:
    control.bind(("127.0.0.1", 0))
    proxy.bind(("127.0.0.1", 0))
    print(control.getsockname()[1], proxy.getsockname()[1])
PY
)
toxiproxy-server -host 127.0.0.1 -port "$control_port" > "$reports/toxiproxy.log" 2>&1 &
proxy_pid=$!
control="http://127.0.0.1:$control_port"
for (( attempt=0; attempt < 30; attempt++ )); do
    if curl --noproxy '*' -fsS "$control/version" > "$reports/toxiproxy-version.txt" 2>/dev/null; then break; fi
    kill -0 "$proxy_pid"
    sleep 0.1
done
jq -nc --arg listen "127.0.0.1:$proxy_port" --arg upstream "${CTXFAB_TEST_URL#http://}" \
    '{name:"local-api", listen:$listen, upstream:$upstream, enabled:true}' |
    curl --noproxy '*' -fsS -H 'Content-Type: application/json' -d @- "$control/proxies" > /dev/null
probe="http://127.0.0.1:$proxy_port/v1/status"
request --fail "$probe" | jq -e '.status == "ready"' > "$reports/fault-healthy.txt"
curl --noproxy '*' -fsS -H 'Content-Type: application/json' -d '{"enabled":false}' \
    "$control/proxies/local-api" > /dev/null
outage_exit=0
request --fail "$probe" > "$reports/fault-outage.log" 2>&1 || outage_exit=$?
if [[ $outage_exit != 7 ]]; then
    echo "The disabled proxy did not refuse the connection (curl exit $outage_exit)." >&2
    exit 1
fi
curl --noproxy '*' -fsS -H 'Content-Type: application/json' -d '{"enabled":true}' \
    "$control/proxies/local-api" > /dev/null
request --fail "$probe" | jq -e '.status == "ready"' > "$reports/fault-recovered.txt"

echo "Integration checks passed. Reports in $reports"
