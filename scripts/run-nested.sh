#!/usr/bin/env bash
set -euo pipefail

# Safely launches NEON DE (compositor + GTK4 desktop shell) in a nested window
# on your existing Wayland/X11 session. Never modifies the host desktop or login manager.

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

echo "[NEON DE] Building neon-compositor and neon-shell..."
cargo build -p neon-compositor -p neon-shell

if [[ -n "$TERMINAL" ]]; then
  echo "[NEON DE] Launching nested NEON DE session (shell + $TERMINAL)"
  exec ./target/debug/neon-compositor --shell --command "$TERMINAL" "$@"
else
  echo "[NEON DE] Launching nested NEON DE session (shell only)"
  exec ./target/debug/neon-compositor --shell "$@"
fi
