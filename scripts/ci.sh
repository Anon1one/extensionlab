#!/usr/bin/env bash
# Used by action.yml; installs nothing and never needs repository write permissions.
set -euo pipefail

binary=${EXTENSIONLAB_ACTION_BINARY:?compiled adapter binary is required}
adapter=${EXTENSIONLAB_ACTION_ADAPTER:?adapter is required}
matrix=${EXTENSIONLAB_ACTION_MATRIX:-default}
out=${EXTENSIONLAB_ACTION_OUT:-extensionlab-report}
baseline=${EXTENSIONLAB_ACTION_BASELINE:-}
fail_on=${EXTENSIONLAB_ACTION_FAIL_ON:-fail}
seed=${EXTENSIONLAB_ACTION_SEED:-0}
token_so=${EXTENSIONLAB_ACTION_TOKEN_SO:-}
token_hash=${EXTENSIONLAB_ACTION_TOKEN_HASH:-}

fail() { printf 'extensionlab: %s\n' "$*" >&2; exit 3; }
[[ -x "$binary" ]] || fail "adapter binary is not executable: $binary"
[[ "$fail_on" == fail || "$fail_on" == rejected ]] || fail 'fail-on must be fail or rejected'
[[ -z "$baseline" || -f "$baseline" ]] || fail "trusted baseline report not found: $baseline"
# Refuse reused output so a crashed build cannot leave a stale report looking current.
[[ ! -e "$out" ]] || fail "output directory already exists; choose a fresh out path: $out"
[[ "$out" != *$'\n'* && "$out" != *$'\r'* ]] || fail 'output path must be a single line'
args=(run --adapter "$adapter" --matrix "$matrix" --out "$out" --format both --fail-on "$fail_on" --seed "$seed")
if [[ -n "$token_so" ]]; then args+=(--token-2022-so "$token_so"); fi
if [[ -n "$token_hash" ]]; then args+=(--token-2022-sha256 "$token_hash"); fi

status=0
"$binary" "${args[@]}" || status=$?
[[ -f "$out/report.json" ]] || fail "run produced no report (exit $status)"
case "$status" in 0|1|3|4) ;; *) fail "unexpected runner exit code: $status" ;; esac
comparison=
if [[ -n "$baseline" && "$status" != 3 && "$status" != 4 ]]; then
    status=0
    "$binary" report compare "$baseline" "$out/report.json" --fail-on "$fail_on" --out "$out" || status=$?
    comparison="$out/comparison.json"
fi

if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    if [[ -f "$out/comparison.md" ]]; then cat "$out/comparison.md" >> "$GITHUB_STEP_SUMMARY"; fi
    cat "$out/report.md" >> "$GITHUB_STEP_SUMMARY"
fi
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'exit-code=%s\nreport-path=%s/report.json\ncomparison-path=%s\n' "$status" "$out" "$comparison" >> "$GITHUB_OUTPUT"
fi
exit "$status"
