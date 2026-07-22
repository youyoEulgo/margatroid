#!/usr/bin/env bash

set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to check the Cargo metadata dependency graph" >&2
    exit 127
fi

metadata="$(cargo metadata --no-deps --format-version 1)"
violations="$({
    jq -r '
        .workspace_root as $root
        | ($root + "/legacy/") as $legacy
        | (
            .packages[]
            | select(.manifest_path | startswith($legacy))
            | "workspace package \(.name) is located under legacy: \(.manifest_path)"
          ), (
            .packages[] as $package
            | $package.dependencies[]
            | select(.path != null and (.path | startswith($legacy)))
            | "package \($package.name) depends on legacy package \(.name): \(.path)"
          )
    ' <<<"$metadata"
})"

if [[ -n "$violations" ]]; then
    echo "formal workspace must not depend on legacy code:" >&2
    echo "$violations" >&2
    exit 1
fi

echo "legacy dependency boundary: ok"
