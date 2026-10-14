#!/usr/bin/env bash
if [ -z "${BASH_VERSION:-}" ]; then
  exec bash "$0" "$@"
fi
set -euo pipefail

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
  cat <<'USAGE'
Usage: ./build.sh [--build-only]

Builds Swift Design as documented: first the Dioxus WASM studio in crates/ui,
then the release server package. --build-only is accepted for callers that use a
common interface; it has the same local behavior as running without arguments.

Commands:
  (cd crates/ui && dx build --release)
  cargo build --release -p server
USAGE
}

while (($#)); do
  case "$1" in
    --help|-h) usage; exit 0 ;;
    --build-only) shift ;;
    *) echo "error: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

need() {
  command -v "$1" >/dev/null 2>&1 || { echo "error: required tool not found: $1" >&2; exit 127; }
}

need cargo
need dx
need rustup
rustup target list --installed | grep -qx 'wasm32-unknown-unknown' || {
  echo "error: required Rust target not installed: wasm32-unknown-unknown" >&2
  echo "hint: rustup target add wasm32-unknown-unknown" >&2
  exit 1
}

echo '+ (cd crates/ui && dx build --release)'
(
  cd crates/ui
  dx build --release
)

echo '+ cargo build --release -p server'
cargo build --release -p server
