#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"
source "$(dirname "$0")/../model-staging.sh"
source "$(dirname "$0")/profile.sh"

mkdir -p "$quality_dir/m08-assets"
stage_model "$quality_dir/m08-assets/$m08_model" "$m08_profile" "$m08_sha256"
