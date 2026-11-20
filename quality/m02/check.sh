#!/bin/bash
set -euo pipefail
source "$(dirname "$0")/../toolchain.sh"

require_mac
mkdir -p "$quality_dir/reports"
reports=$(mktemp -d "$quality_dir/reports/m02.XXXXXX")
echo "M02 tool versions and telemetry reports in $reports"
{
    sw_vers
    xcodebuild -version
    xcrun swift --version
    rustc --version
    k6 version
    toxiproxy-server -version
    oasdiff --version
    java -jar "$tools_dir/openapi/generator.jar" version
    uvx --from schemathesis==4.33.4 schemathesis --version
} 2>&1 | tee "$reports/versions.log"

(
    cd "$quality_root/backend"
    cargo test --locked -p ctxfab-server --test telemetry -- --nocapture
) 2>&1 | tee "$reports/telemetry.log"

cd "$quality_root"
/bin/bash quality/m02/ui.sh
/bin/bash quality/m02/runtime.sh
echo "M02 additional gates passed. Reports in $reports"
