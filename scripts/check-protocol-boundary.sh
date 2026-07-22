#!/usr/bin/env bash

set -euo pipefail

if ! command -v jq >/dev/null 2>&1; then
    echo "jq is required to check the Cargo metadata dependency graph" >&2
    exit 127
fi

metadata="$(cargo metadata --no-deps --format-version 1)"

if ! jq -e '.packages | any(.name == "margatroid_protocol")' <<<"$metadata" >/dev/null; then
    echo "margatroid_protocol is not a workspace package" >&2
    exit 1
fi

unexpected_dependencies="$(
    jq -r '
        .packages[]
        | select(.name == "margatroid_protocol")
        | .dependencies[]
        | select(.name != "serde" and .name != "serde_json")
        | .name
    ' <<<"$metadata"
)"

if [[ -n "$unexpected_dependencies" ]]; then
    echo "margatroid_protocol has forbidden dependencies:" >&2
    echo "$unexpected_dependencies" >&2
    exit 1
fi

for consumer in cli margatroidd; do
    if ! jq -e --arg consumer "$consumer" '
        .packages[]
        | select(.name == $consumer)
        | any(.dependencies[]; .name == "margatroid_protocol")
    ' <<<"$metadata" >/dev/null; then
        echo "$consumer must depend on margatroid_protocol" >&2
        exit 1
    fi
done

echo "protocol dependency boundary: ok"
