#!/usr/bin/env bash
# Prove a real exported refund regression is red before fix A and green after it.
# Both program binaries must already be built; program source is never edited.
set -euo pipefail
repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
original=${EXTENSIONLAB_ORIGINAL_ESCROW_SO:-"$repo_root/targets/escrow/target/deploy/escrowq32026.so"}
patched=${EXTENSIONLAB_PATCHED_ESCROW_SO:-"$repo_root/research/fixcheck/escrow_patched/target/deploy/escrowq32026.so"}
[[ -f "$original" && -f "$patched" ]] || { echo 'Build the original and patched escrow binaries first.' >&2; exit 3; }
scratch=$(mktemp -d "${TMPDIR:-/tmp}/extensionlab-regression.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export EXTENSIONLAB_ESCROW_SO="$original"
status=0
cargo run --locked --manifest-path "$repo_root/Cargo.toml" -p extensionlab-adapter-escrow -- \
    run --adapter escrow --matrix default --out "$scratch/report" || status=$?
[[ "$status" == 1 ]] || { echo "Expected compatibility findings; got exit $status" >&2; exit 3; }
mkdir -p "$scratch/tests"
# JSON quoting is also valid for these TOML string paths, including spaces.
python3 - "$repo_root" "$scratch/Cargo.toml" <<'PY'
import json
import sys
from pathlib import Path
repo, manifest = map(Path, sys.argv[1:])
manifest.write_text('[package]\nname = "extensionlab-export-check"\nversion = "0.0.0"\nedition = "2021"\n'
    + '[workspace]\n[dev-dependencies]\nserde_json = "1.0"\n'
    + 'extensionlab-core = { path = ' + json.dumps(str(repo / 'crates/extensionlab-core')) + ' }\n'
    + 'extensionlab-adapter-escrow = { path = ' + json.dumps(str(repo / 'adapters/escrow')) + ' }\n')
PY
cargo run --locked --manifest-path "$repo_root/Cargo.toml" -p extensionlab-adapter-escrow -- \
    regress "$scratch/report/report.json" --run token_2022/make_refund \
    --adapter-expr 'extensionlab_adapter_escrow::boxed()' --out "$scratch/tests/refund.rs"
# Resolve once from the cache, then prove the red result is an assertion failure,
# rather than a missing binary, a compiler error or a download problem.
cp "$repo_root/Cargo.lock" "$scratch/Cargo.lock"
# Cargo adds the temporary package to this copy, retaining the workspace's
# dependency versions. Later executions are locked and need no downloads.
cargo test --offline --manifest-path "$scratch/Cargo.toml" --test refund --no-run
status=0
cargo test --locked --offline --manifest-path "$scratch/Cargo.toml" --test refund > "$scratch/red.log" 2>&1 || status=$?
[[ "$status" == 101 ]] || { cat "$scratch/red.log"; echo 'Original program did not fail the regression.' >&2; exit 3; }
python3 - "$scratch/red.log" <<'PY'
import re
import sys
from pathlib import Path
log = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', Path(sys.argv[1]).read_text())
if ('left: Fail' not in log or 'right: Pass' not in log
        or 'harness error:' in log or 'could not compile' in log):
    print(log)
    raise SystemExit('Expected a compatibility status assertion, not a harness or compiler failure.')
PY
EXTENSIONLAB_ESCROW_SO="$patched" cargo test --locked --offline --manifest-path "$scratch/Cargo.toml" --test refund
echo 'Verified: the exported refund test fails on the original program and passes with fix A.'
