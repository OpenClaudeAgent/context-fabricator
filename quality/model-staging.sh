# Sourced by qualification commands after loading the pinned toolchain.
# Keep partial downloads and the outbound ledger in one isolated backend session directory.
stage_model() (
    local target=$1 profile=$2 expected=$3
    local staging="$quality_dir/model-staging"
    local staged="$staging/inference/$(basename "$target")"

    if [[ -f "$target" ]]; then
        printf '%s  %s\n' "$expected" "$target" | shasum -a 256 -c
        exit 0
    fi

    cargo build --locked --manifest-path "$quality_root/backend/Cargo.toml" -p ctxfab-server
    cargo build --locked --manifest-path "$quality_root/cli/Cargo.toml"
    local backend="$quality_root/backend/target/debug/ctxfab-server"
    local cli="$quality_root/cli/target/debug/ctxfab"
    mkdir -p "$quality_dir/reports"
    local report
    report=$(mktemp "$quality_dir/reports/model-staging.XXXXXX")
    "$backend" "$staging" > "$report" 2>&1 &
    local backend_pid=$!
    trap 'kill "$backend_pid" 2>/dev/null || true; wait "$backend_pid" 2>/dev/null || true' EXIT

    for ((attempt = 0; attempt < 100; attempt++)); do
        if jq -e --argjson pid "$backend_pid" '.processId == $pid' "$staging/endpoint.json" >/dev/null 2>&1; then
            break
        fi
        kill -0 "$backend_pid" 2>/dev/null || { cat "$report" >&2; exit 1; }
        sleep 0.1
    done
    jq -e --argjson pid "$backend_pid" '.processId == $pid' "$staging/endpoint.json" >/dev/null
    export CTXFAB_DATA_DIRECTORY="$staging"
    "$cli" install-model "$profile" >/dev/null

    for ((attempt = 0; attempt < 1800; attempt++)); do
        local snapshot state
        snapshot=$("$cli" model-downloads | jq -ec --arg profile "$profile" '.[] | select(.profile == $profile)')
        state=$(jq -er '.state' <<< "$snapshot")
        case "$state" in
            installed)
                printf '%s  %s\n' "$expected" "$staged" | shasum -a 256 -c
                if [[ ! -f "$target" ]]; then ln -- "$staged" "$target"; fi
                printf '%s  %s\n' "$expected" "$target" | shasum -a 256 -c
                exit 0
                ;;
            failed) printf '%s\n' "$snapshot" >&2; exit 1 ;;
        esac
        sleep 1
    done
    echo "Model staging timed out. The isolated session retains download progress." >&2
    exit 1
)
