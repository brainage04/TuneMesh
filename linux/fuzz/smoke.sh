#!/usr/bin/env bash
# Runs every fuzz target for SECONDS (default 60), JOBS at a time (default:
# the number of CPUs), starting from the committed corpus in corpus/<target>
# (which holds the regression inputs of every crash found so far). New
# inputs go to a scratch directory, so the committed corpus stays as it is.
# Fails if any target finds a crash; the input is in artifacts/<target>/.
#
#   nix develop .#nightly -c linux/fuzz/smoke.sh [SECONDS] [TARGET...]
set -euo pipefail
cd "$(dirname "$0")"
secs=${1:-60}
shift || true
jobs=${JOBS:-$(nproc)}
if [ $# -gt 0 ]; then
    targets=("$@")
else
    mapfile -t targets < <(cargo fuzz list)
fi
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

cargo fuzz build -O

run() {
    local t=$1 dict=()
    [ -f "dicts/$t.dict" ] && dict=("-dict=dicts/$t.dict")
    mkdir -p "$scratch/$t" "corpus/$t"
    if cargo fuzz run -O "$t" "$scratch/$t" "corpus/$t" -- \
        -max_total_time="$secs" -rss_limit_mb=3072 -timeout=30 "${dict[@]}" \
        >"$scratch/$t.log" 2>&1; then
        echo "ok      $t"
    else
        echo "FAILED  $t"
        grep -E -A8 'panicked at|ERROR: ' "$scratch/$t.log" | head -40
        return 1
    fi
}
export -f run
export secs scratch

printf '%s\n' "${targets[@]}" | xargs -P "$jobs" -I{} bash -c 'run {}'
