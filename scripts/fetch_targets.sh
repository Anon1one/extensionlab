#!/bin/sh
# Fetch public program source unchanged, retaining each target's own license.
set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: scripts/fetch_targets.sh [program-examples|solsab|train]" >&2
    exit 2
fi

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_root=${EXTENSIONLAB_TARGETS_DIR:-"$repo_root/targets"}
target=${1:-program-examples}
case "$target" in
    program-examples)
        repository=https://github.com/solana-foundation/program-examples.git
        revision=de090195e4de698e150572c4f886c333323a64a5
        sparse=tokens/escrow/anchor
        subdirectory=tokens/escrow/anchor
        manifest=Cargo.toml
        license=LICENSE
        ;;
    solsab)
        repository=https://github.com/sablier-labs/solsab.git
        revision=e060b26a6c736a3104a0b24ac069fd53633b3ceb
        sparse=programs
        subdirectory=.
        manifest=programs/merkle_instant/Cargo.toml
        license=LICENSE.md
        ;;
    train)
        repository=https://github.com/TrainProtocol/contracts.git
        revision=3dbda6949e35e9095493333a6fff7dfe4ccde1aa
        sparse=chains/solana
        subdirectory=chains/solana
        manifest=programs/train-htlc/Cargo.toml
        license=LICENSE
        ;;
    *) echo "usage: scripts/fetch_targets.sh [program-examples|solsab|train]" >&2; exit 2 ;;
esac
destination="$target_root/$target"

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
    git -C "$destination" sparse-checkout set "$sparse"
    git -C "$destination" checkout --detach "$revision"
fi

if [ "$target" = program-examples ]; then
    lockfile="$repo_root/targets/locks/program-examples-escrow.Cargo.lock"
    target_lockfile="$destination/$subdirectory/Cargo.lock"
    if [ -e "$target_lockfile" ] && ! cmp -s "$lockfile" "$target_lockfile"; then
        echo "refusing to replace a different dependency lockfile at $target_lockfile" >&2
        exit 1
    fi
    cp "$lockfile" "$target_lockfile"
fi

printf 'Target: %s\nRevision: %s\nLicense: %s/%s\n' "$destination/$subdirectory" "$revision" "$destination" "$license"
printf 'Build using the installed host toolchain and the pinned dependency lock:\n'
printf 'cd "%s/%s"\n' "$destination" "$subdirectory"
printf 'RUSTUP_TOOLCHAIN=stable CARGO_BUILD_JOBS=4 nice -n 15 cargo build-sbf --manifest-path %s --tools-version v1.52 -- --locked\n' "$manifest"
