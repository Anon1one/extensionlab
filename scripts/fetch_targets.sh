#!/bin/sh
# Fetch the unmodified public target, including its upstream MIT license.
set -eu

if [ "$#" -gt 1 ] || [ "${1:-program-examples}" != "program-examples" ]; then
    echo "usage: scripts/fetch_targets.sh [program-examples]" >&2
    exit 2
fi

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_root=${EXTENSIONLAB_TARGETS_DIR:-"$repo_root/targets"}
destination="$target_root/program-examples"
repository=https://github.com/solana-foundation/program-examples.git
revision=de090195e4de698e150572c4f886c333323a64a5
subdirectory=tokens/escrow/anchor

if [ -e "$destination" ]; then
    if [ ! -d "$destination/.git" ] ||
       [ "$(git -C "$destination" remote get-url origin)" != "$repository" ] ||
       [ "$(git -C "$destination" rev-parse HEAD)" != "$revision" ]; then
        echo "refusing to replace an existing directory or checkout at $destination" >&2
        exit 1
    fi
    if [ -n "$(git -C "$destination" status --porcelain)" ]; then
        echo "refusing to use a modified target checkout at $destination" >&2
        exit 1
    fi
else
    mkdir -p "$target_root"
    git clone --filter=blob:none --no-checkout "$repository" "$destination"
    git -C "$destination" sparse-checkout init --cone
    git -C "$destination" sparse-checkout set "$subdirectory"
    git -C "$destination" checkout --detach "$revision"
fi

lockfile="$repo_root/targets/locks/program-examples-escrow.Cargo.lock"
target_lockfile="$destination/$subdirectory/Cargo.lock"
if [ -e "$target_lockfile" ] && ! cmp -s "$lockfile" "$target_lockfile"; then
    echo "refusing to replace a different dependency lockfile at $target_lockfile" >&2
    exit 1
fi
cp "$lockfile" "$target_lockfile"

printf 'Target: %s\nRevision: %s\nLicense: %s/LICENSE\n' "$destination/$subdirectory" "$revision" "$destination"
printf 'Build using the installed host toolchain (the upstream root requests Rust 1.92):\n'
printf 'cd "%s/%s"\n' "$destination" "$subdirectory"
printf 'RUSTUP_TOOLCHAIN=stable CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf --tools-version v1.52 -- --locked\n'
