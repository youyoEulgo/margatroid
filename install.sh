#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
SKIP_BUILD=0
INSTALL_FLAGS=()
while (($#)); do
    case "$1" in
        --skip-build) SKIP_BUILD=1 ;;
        --skip-landstrip|--update-resources) INSTALL_FLAGS+=("$1") ;;
        *) printf 'Unknown option: %s\n' "$1" >&2; exit 2 ;;
    esac
    shift
done
EXE_SUFFIX=''
case "${OSTYPE:-}" in
    msys*|cygwin*|win32*) EXE_SUFFIX='.exe' ;;
esac
if (( ! SKIP_BUILD )); then
    command -v cargo >/dev/null || { printf 'Install the Rust toolchain first.\n' >&2; exit 1; }
    printf 'Building Margatroid...\n'
    (cd -- "$PROJECT_ROOT" && cargo build --locked --release -p margatroid -p tool_plugin -p shell-client -p mcp-client --bins)
fi
[[ -x "$PROJECT_ROOT/target/release/margatroid-install${EXE_SUFFIX}" ]] || { printf 'Build the margatroid-install helper first, or omit --skip-build.\n' >&2; exit 1; }
exec "$PROJECT_ROOT/target/release/margatroid-install${EXE_SUFFIX}" "$PROJECT_ROOT" "${INSTALL_FLAGS[@]}"
