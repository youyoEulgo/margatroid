#!/usr/bin/env bash
# Build the web interface into ui/dist, which ui_plugin embeds at compile time.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/ui"

if ! command -v bun >/dev/null 2>&1; then
  echo "bun is required to build the interface" >&2
  exit 1
fi

bun install --frozen-lockfile
bun run build
