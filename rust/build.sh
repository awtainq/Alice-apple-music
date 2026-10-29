#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
TASK=build
if [ "${1:-}" = --zig ]; then TASK=zigbuild; shift; fi
if [ "$#" -ne 0 ]; then echo 'Usage: bash rust/build.sh [--zig]' >&2; exit 2; fi
remap() {
  local from=$1
  if [ -d "$from" ]; then from=$(cd "$from" && pwd -P); fi
  export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$from=$2"
  export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$from=$2"
}
remap "$HOME" /build/home
remap "$PWD" /build/workspace
remap "${CARGO_HOME:-$HOME/.cargo}" /build/cargo
remap "${RUSTUP_HOME:-$HOME/.rustup}" /build/rustup
remap "$ROOT" /build/app
exec cargo "$TASK" --locked --release --package alice-apple-music --package luna-import --target aarch64-unknown-linux-musl --manifest-path "$ROOT/rust/Cargo.toml"
