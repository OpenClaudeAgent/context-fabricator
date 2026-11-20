#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"

if [[ ${CTXFAB_REFERENCE_COMPARE_STRUCTURE:-0} == 1 ]]; then
    : "${CTXFAB_REFERENCE_EMBEDDING_ASSETS:?Structural comparison requires the pinned embedding runtime}"
    : "${CTXFAB_REFERENCE_ANSWER_ASSETS:?Structural comparison requires the pinned generation runtime}"
fi

if [[ -n ${CTXFAB_REFERENCE_ANSWER_ASSETS:-} ]]; then
    source "$quality_root/quality/m09/profile.sh"
    (
        cd "$CTXFAB_REFERENCE_ANSWER_ASSETS"
        printf '%s  %s\n' "$m09_sha256" "$m09_model" | shasum -a 256 -c
        ./llama-server --version
    )
fi
if [[ -n ${CTXFAB_REFERENCE_EMBEDDING_ASSETS:-} ]]; then
    source "$quality_root/quality/m08/profile.sh"
    (
        cd "$CTXFAB_REFERENCE_EMBEDDING_ASSETS"
        printf '%s  %s\n' "$m08_sha256" "$m08_model" | shasum -a 256 -c
        runtime=$(./llama-server --version 2>&1)
        printf '%s\n' "$runtime"
        grep -Fq "build $m08_build, commit $m08_runtime_commit" <<< "$runtime"
    )
fi

work=$(mktemp -d "${TMPDIR:-/tmp}/ctxfab-reference-corpus.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
manifest="$quality_root/quality/m07/reference-corpora.json"
jq -er '.[] | [.repository, .commit, .path, .file, .sha256, .bytes, .blob] | @tsv' "$manifest" > "$work/files"
while IFS=$'\t' read -r repository commit path file sha256 bytes blob; do
    curl --noproxy '*' --fail --silent --show-error --max-time 20 --max-filesize "$bytes" \
        "https://raw.githubusercontent.com/$repository/$commit/$path" -o "$work/$file"
    (cd "$work" && printf '%s  %s\n' "$sha256" "$file" | shasum -a 256 -c)
    [[ $(git hash-object --no-filters "$work/$file") == "$blob" ]]
done < "$work/files"

if [[ ${CTXFAB_REFERENCE_COMPARE_STRUCTURE:-0} == 1 ]]; then
    export CTXFAB_REFERENCE_STRUCTURAL_BOUNDARIES="$work/structural.json"
    uv run --isolated --python 3.12 \
        --with tree-sitter==0.26.1 --with tree-sitter-python==0.25.0 --with tree-sitter-rust==0.24.0 \
        python "$quality_root/quality/m07/structural-boundaries.py" \
        "$work" "$manifest" "$CTXFAB_REFERENCE_STRUCTURAL_BOUNDARIES"
fi

cargo build --locked --manifest-path "$quality_root/cli/Cargo.toml"
CTXFAB_REFERENCE_CORPUS="$work" CTXFAB_REFERENCE_CLI="$quality_root/cli/target/debug/ctxfab" \
    cargo test --locked --manifest-path "$quality_root/backend/Cargo.toml" -p ctxfab-server \
    --test source_passages --test source_snapshot_cli -- --include-ignored --nocapture
