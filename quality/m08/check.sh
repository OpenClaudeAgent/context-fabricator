#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"
source "$(dirname "$0")/profile.sh"

require_mac
worker=${CTXFAB_LLAMA_SERVER:-llama-server}
model=${CTXFAB_M08_MODEL:-"$quality_dir/m08-assets/$m08_model"}
app=${1:-"$quality_dir/foundation-xcode/Build/Products/Debug/Context Fabricator.app"}
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/m08.XXXXXX")
echo "M08 retrieval reports in $reports"
cargo test --manifest-path "$quality_root/backend/Cargo.toml" --locked \
    -p ctxfab-server --test model_download 2>&1 | tee "$reports/model-download.log"
{
    "$worker" --version
    "$worker" --list-devices
    sw_vers
    sysctl -n machdep.cpu.brand_string
    sysctl -n hw.memsize
    printf 'Profile %s\nRevision %s\n' "$m08_profile" "$m08_revision"
    printf '%s  %s\n' "$m08_sha256" "$model" | shasum -a 256 -c
} 2>&1 | tee "$reports/profile.log"
grep -Fq "build $m08_build, commit $m08_runtime_commit" "$reports/profile.log"
grep -Eq '^ *MTL0:' "$reports/profile.log"
/usr/bin/codesign --verify --deep --strict "$app"

private=$(mktemp -d "$quality_dir/m08-session.XXXXXX")
worker_pid=
sampler_pid=
cleanup() {
    if [[ -n $sampler_pid ]]; then kill "$sampler_pid" 2>/dev/null || true; wait "$sampler_pid" 2>/dev/null || true; fi
    if [[ -n $worker_pid ]]; then kill "$worker_pid" 2>/dev/null || true; wait "$worker_pid" 2>/dev/null || true; fi
    if [[ -f "$private/key" ]]; then rm -- "$private/key"; fi
    if [[ -f "$private/authorization" ]]; then rm -- "$private/authorization"; fi
    rmdir "$private"
}
trap cleanup EXIT
openssl rand -hex 24 > "$private/key"
printf 'Authorization: Bearer %s\n' "$(cat "$private/key")" > "$private/authorization"
port=$(python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)
"$worker" --offline --host 127.0.0.1 --port "$port" --api-key-file "$private/key" \
    --model "$model" --alias "$m08_profile" --embedding --pooling last --embd-normalize 2 \
    --device MTL0 --gpu-layers all --ctx-size 4096 --batch-size 4096 --ubatch-size 4096 --parallel 1 \
    --threads 4 --no-ui > "$reports/worker.log" 2>&1 &
worker_pid=$!
for ((attempt = 0; attempt < 180; attempt++)); do
    if curl --noproxy '*' -fsS --max-time 2 -H @"$private/authorization" \
        "http://127.0.0.1:$port/health" > "$reports/worker-ready.json" 2>/dev/null; then
        break
    fi
    kill -0 "$worker_pid"
    sleep 0.2
done
jq -e '.status == "ok"' "$reports/worker-ready.json" >/dev/null
(
    while kill -0 "$worker_pid" 2>/dev/null; do
        ps -o rss= -p "$worker_pid" || true
        sleep 0.1
    done
) > "$reports/worker-rss-kib.txt" &
sampler_pid=$!

cd "$quality_root/backend"
CTXFAB_TEST_APP_BUNDLE="$app" CTXFAB_EMBEDDING_ADDRESS="127.0.0.1:$port" \
    CTXFAB_EMBEDDING_KEY_FILE="$private/key" \
    cargo test --locked -p ctxfab-server --test retrieval \
    packaged_multilingual_corpus_qualifies_qwen_and_bm25 -- --ignored --exact --nocapture \
    2>&1 | tee "$reports/packaged.log"
sed -n 's/^M08_RESULT //p' "$reports/packaged.log" > "$reports/results.json"
jq -e '.documents > 0 and (.cases | length > 0)' "$reports/results.json" >/dev/null
python3 "$quality_root/quality/m08/summarize.py" "$reports/results.json" "$reports/worker-rss-kib.txt" \
    | tee "$reports/summary.json"
