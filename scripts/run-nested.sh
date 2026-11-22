#!/usr/bin/env bash
set -euo pipefail

# Safely launches NEON DE in a nested window on the existing Wayland/X11 session.
# Never modifies host display settings, login managers, or existing user sessions.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TERMINAL="${NEON_TERMINAL:-}"
if [[ -z "$TERMINAL" ]]; then
  for candidate in weston-terminal alacritty kitty foot; do
    if command -v "$candidate" >/dev/null 2>&1; then
      TERMINAL="$candidate"
      break
    fi
  done
fi

cd "$REPO_ROOT"

if [[ -n "$TERMINAL" ]]; then
  echo "[NEON DE] Launching nested compositor with startup client: $TERMINAL"
  exec cargo run -p neon-compositor -- --command "$TERMINAL" "$@"
else
  echo "[NEON DE] Launching nested compositor (no default terminal detected)"
  exec cargo run -p neon-compositor -- "$@"
fi
