#!/usr/bin/env bash

set -Eeuo pipefail

readonly BOX_NAME="wis-dev"
PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly PROJECT_ROOT

if [[ "${CONTAINER_ID:-}" == "$BOX_NAME" ]]; then
  cd -- "$PROJECT_ROOT"
  exec env \
    PATH=/usr/local/bin:/usr/bin:/bin \
    RUSTC=/usr/bin/rustc \
    RUSTDOC=/usr/bin/rustdoc \
    /usr/bin/npm run dev:app
fi

if [[ -n "${CONTAINER_ID:-}" ]]; then
  printf 'wis is running inside Distrobox %s, but needs %s.\n' "$CONTAINER_ID" "$BOX_NAME" >&2
  printf 'Exit this container and run yarn dev from the host terminal.\n' >&2
  exit 1
fi

if ! command -v distrobox >/dev/null 2>&1; then
  printf 'Distrobox is unavailable. Run ./setup-bazzite.sh first.\n' >&2
  exit 1
fi

if ! distrobox list 2>/dev/null | awk -F '|' -v wanted="$BOX_NAME" '
  NR > 1 {
    name = $2
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
    if (name == wanted) found = 1
  }
  END { exit(found ? 0 : 1) }
'; then
  printf 'The %s Distrobox does not exist. Run ./setup-bazzite.sh first.\n' "$BOX_NAME" >&2
  exit 1
fi

cleanup() {
  distrobox enter --clean-path --name "$BOX_NAME" -- bash -lc '
    project_root="$1"
    pkill -TERM -f "node ${project_root}/node_modules/.bin/[t]auri dev" 2>/dev/null || true
    pkill -TERM -f "node ${project_root}/node_modules/.bin/[v]ite" 2>/dev/null || true
    pkill -TERM -f "${project_root}/target/debug/[w]is" 2>/dev/null || true
  ' bash "$PROJECT_ROOT" >/dev/null 2>&1 || true
}

trap cleanup EXIT
trap 'exit 130' INT TERM

# Recover automatically if an earlier interrupted Distrobox session left the
# development server or app running.
cleanup

distrobox enter --clean-path --name "$BOX_NAME" -- \
  bash -lc 'cd -- "$1" && exec env PATH=/usr/local/bin:/usr/bin:/bin RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc /usr/bin/npm run dev:app' bash "$PROJECT_ROOT"
