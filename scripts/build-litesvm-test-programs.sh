#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
registry_src="${CARGO_HOME:-$HOME/.cargo}/registry/src"

anchor build

token_manifest="$(find "$registry_src" -path '*/spl-token-2022-10.0.0/Cargo.toml' -print -quit)"
if [[ -z "$token_manifest" ]]; then
    cargo info spl-token-2022@10.0.0 >/dev/null
    token_manifest="$(find "$registry_src" -path '*/spl-token-2022-10.0.0/Cargo.toml' -print -quit)"
fi
if [[ -z "$token_manifest" ]]; then
    echo "Unable to locate the spl-token-2022 10.0.0 source manifest" >&2
    exit 1
fi

cargo-build-sbf \
    --manifest-path "$token_manifest" \
    --features zk-ops \
    --sbf-out-dir "$repo_root/target/test-programs" \
    --skip-tools-install
