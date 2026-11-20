#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"
source "$(dirname "$0")/../model-staging.sh"
source "$(dirname "$0")/../m08/profile.sh"
source "$(dirname "$0")/profile.sh"

worker=$(command -v "${CTXFAB_LLAMA_SERVER:-llama-server}")
assets="$quality_dir/m09-assets"
mkdir -p "$assets"
stage_model "$assets/$m09_model" "$m09_profile" "$m09_sha256"
stage_model "$assets/$m09_alternate" "$m09_alternate_profile" "$m09_alternate_sha256"
ln -sfn "$worker" "$assets/llama-server"
ln -sfn "$quality_dir/m08-assets/$m08_model" "$assets/$m08_model"
printf 'M09 assets staged in %s\n' "$assets"
