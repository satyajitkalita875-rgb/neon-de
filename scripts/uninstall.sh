#!/usr/bin/env bash
set -euo pipefail

# Cleanly removes NEON DE installed binaries, session entries, portals, and systemd units.
# Never touches any other desktop environment or user ~/.config/neon-de/config.toml.

PREFIX="/usr/local"
USER_ONLY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)
            PREFIX="$2"
            shift 2
            ;;
        --user)
            USER_ONLY=true
            PREFIX="${HOME}/.local"
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

BINARIES=(neon-compositor neon-shell neon-settings neon-lockscreen neon-session)

if [[ "${USER_ONLY}" == true ]]; then
    for bin in "${BINARIES[@]}"; do
        rm -f "${PREFIX}/bin/${bin}"
    done
    rm -f "${HOME}/.local/share/applications/org.neonde.settings.desktop" \
          "${HOME}/.local/share/applications/neon-de-nested.desktop" \
          "${HOME}/.config/xdg-desktop-portal/neon-de-portals.conf" \
          "${HOME}/.config/systemd/user/neon-session.target" \
          "${HOME}/.config/systemd/user/neon-shell.service"
    echo "==> Uninstalled user-only NEON DE installation from ${PREFIX}/bin"
else
    SUDO=""
    if [[ "${EUID}" -ne 0 ]]; then
        SUDO="sudo"
    fi
    for bin in "${BINARIES[@]}"; do
        ${SUDO} rm -f "${PREFIX}/bin/${bin}"
    done
    ${SUDO} rm -f \
        "/usr/share/wayland-sessions/neon-de.desktop" \
        "/usr/share/wayland-sessions/neon-de-safe.desktop" \
        "/usr/share/applications/org.neonde.settings.desktop" \
        "/usr/share/applications/neon-de-nested.desktop" \
        "/usr/share/xdg-desktop-portal/neon-de-portals.conf" \
        "/usr/lib/systemd/user/neon-session.target" \
        "/usr/lib/systemd/user/neon-shell.service"
    echo "==> Uninstalled system-wide NEON DE installation from ${PREFIX}/bin"
fi
