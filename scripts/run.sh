#!/usr/bin/env bash
set -euo pipefail
PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
exec "${PYTHON_BIN:-$PROJECT_DIR/.venv/bin/python}" "$PROJECT_DIR/service.py" \
  --private "${ALICE_PRIVATE_DIR:-$PROJECT_DIR/private}" \
  --cache "${ALICE_CACHE_DIR:-$PROJECT_DIR/cache}"
