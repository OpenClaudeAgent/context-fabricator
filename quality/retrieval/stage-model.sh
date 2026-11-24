#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain/environment.sh"
source "$(dirname "$0")/../models/stage-weights.sh"
source "$(dirname "$0")/profile.sh"

mkdir -p "$quality_cache_dir/m08-assets"
stage_model "$quality_cache_dir/m08-assets/$m08_model" "$m08_profile" "$m08_sha256"
