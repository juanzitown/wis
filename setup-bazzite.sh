#!/usr/bin/env bash

set -Eeuo pipefail

readonly BOX_NAME="wis-dev"
readonly BOX_IMAGE="registry.fedoraproject.org/fedora:45"
PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly PROJECT_ROOT

log() {
  printf '\n\033[1;32m[wis-setup]\033[0m %s\n' "$*"
}

fail() {
  printf '\n\033[1;31m[wis-setup]\033[0m %s\n' "$*" >&2
  exit 1
}

if [[ "$(uname -s)" != "Linux" ]]; then
  fail "This setup script currently supports Linux only."
fi

command -v distrobox >/dev/null 2>&1 || fail "Distrobox is required. On Bazzite it should already be installed."
command -v npm >/dev/null 2>&1 || fail "npm is required on the host."

if ! command -v yarn >/dev/null 2>&1; then
  log "Installing the yarn launcher for your host user"
  npm install --global yarn
fi

box_exists() {
  distrobox list 2>/dev/null | awk -F '|' -v wanted="$BOX_NAME" '
    NR > 1 {
      name = $2
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
      if (name == wanted) found = 1
    }
    END { exit(found ? 0 : 1) }
  '
}

if box_exists; then
  log "Reusing Distrobox ${BOX_NAME}"
else
  log "Creating Distrobox ${BOX_NAME} from ${BOX_IMAGE}"
  distrobox create --yes --name "$BOX_NAME" --image "$BOX_IMAGE"
fi

log "Installing the native Tauri, PipeWire, OpenCV, and Tesseract build dependencies"
distrobox enter --clean-path --name "$BOX_NAME" -- bash -lc '
  set -Eeuo pipefail

  # Distrobox imports the host home directory. Pin the Fedora toolchain so a
  # host Rustup installation cannot replace cargo/rustc during the build.
  export PATH=/usr/local/bin:/usr/bin:/bin
  export RUSTC=/usr/bin/rustc
  export RUSTDOC=/usr/bin/rustdoc

  sudo dnf install -y \
    clang \
    curl \
    dbus-devel \
    file \
    gcc \
    gcc-c++ \
    gtk3-devel \
    libappindicator-gtk3-devel \
    librsvg2-devel \
    libxdo-devel \
    make \
    nodejs \
    npm \
    opencv-devel \
    openssl-devel \
    leptonica-devel \
    pipewire-devel \
    pkgconf-pkg-config \
    rust \
    cargo \
    tesseract-devel \
    tesseract-langpack-eng \
    webkit2gtk4.1-devel \
    wget

  if ! command -v yarn >/dev/null 2>&1; then
    sudo npm install --global yarn
  fi

  cd -- "$1"
  /usr/bin/npm install
  env RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc /usr/bin/cargo fetch
  env RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc /usr/bin/cargo test -p wis-core
' bash "$PROJECT_ROOT"

log "Setup complete"
printf '\nFrom this project directory, start wis with:\n\n  yarn dev\n\n'
