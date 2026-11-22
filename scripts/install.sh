#!/usr/bin/env bash
set -euo pipefail

# NEON DE Installer (System or User Prefix)
# Installs compiled release binaries, Wayland session entries, XDG portal config,
# desktop launchers, and systemd user units WITHOUT modifying existing desktop
# sessions or display-manager default configurations.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="/usr/local"
BUILD_PROFILE="release"
USER_ONLY=false

usage() {
    cat <<EOF
Usage: $0 [--prefix <dir>] [--user] [--debug]

Options:
  --prefix <dir>   Binary installation prefix (default: /usr/local)
  --user           Install strictly under ~/.local (no root/sudo required)
  --debug          Install debug binaries instead of building --release
EOF
}

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
        --debug)
            BUILD_PROFILE="debug"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

cd "${REPO_ROOT}"

echo "==> Building NEON DE workspace (${BUILD_PROFILE})..."
if [[ "${BUILD_PROFILE}" == "release" ]]; then
    cargo build --workspace --release
    TARGET_DIR="${REPO_ROOT}/target/release"
else
    cargo build --workspace
    TARGET_DIR="${REPO_ROOT}/target/debug"
fi

BINARIES=(
    neon-compositor
    neon-shell
    neon-settings
    neon-lockscreen
    neon-session
)

BIN_DIR="${PREFIX}/bin"
if [[ "${USER_ONLY}" == true ]]; then
    APP_DIR="${HOME}/.local/share/applications"
    SYSTEMD_DIR="${HOME}/.config/systemd/user"
    PORTAL_DIR="${HOME}/.config/xdg-desktop-portal"

    install -d "${BIN_DIR}" "${APP_DIR}" "${SYSTEMD_DIR}" "${PORTAL_DIR}"
    for bin in "${BINARIES[@]}"; do
        install -m 0755 "${TARGET_DIR}/${bin}" "${BIN_DIR}/${bin}"
    done

    sed "s|/usr/local/bin|${BIN_DIR}|g" "${REPO_ROOT}/data/applications/org.neonde.settings.desktop" \
        > "${APP_DIR}/org.neonde.settings.desktop"
    sed "s|/usr/local/bin|${BIN_DIR}|g" "${REPO_ROOT}/data/applications/neon-de-nested.desktop" \
        > "${APP_DIR}/neon-de-nested.desktop"
    install -m 0644 "${REPO_ROOT}/data/xdg-desktop-portal/neon-de-portals.conf" \
        "${PORTAL_DIR}/neon-de-portals.conf"
    sed "s|/usr/local/bin|${BIN_DIR}|g" "${REPO_ROOT}/data/systemd/neon-shell.service" \
        > "${SYSTEMD_DIR}/neon-shell.service"
    install -m 0644 "${REPO_ROOT}/data/systemd/neon-session.target" \
        "${SYSTEMD_DIR}/neon-session.target"

    echo "==> Installed NEON DE (user-only) to ${BIN_DIR}"
else
    SUDO=""
    if [[ "${EUID}" -ne 0 ]]; then
        SUDO="sudo"
    fi

    ${SUDO} install -d \
        "${BIN_DIR}" \
        "/usr/share/wayland-sessions" \
        "/usr/share/applications" \
        "/usr/share/xdg-desktop-portal" \
        "/usr/lib/systemd/user"

    for bin in "${BINARIES[@]}"; do
        ${SUDO} install -m 0755 "${TARGET_DIR}/${bin}" "${BIN_DIR}/${bin}"
    done

    for file in neon-de.desktop neon-de-safe.desktop; do
        sed "s|/usr/local/bin|${BIN_DIR}|g" "${REPO_ROOT}/data/wayland-sessions/${file}" \
            | ${SUDO} tee "/usr/share/wayland-sessions/${file}" >/dev/null
        ${SUDO} chmod 0644 "/usr/share/wayland-sessions/${file}"
    done

    for file in org.neonde.settings.desktop neon-de-nested.desktop; do
        sed "s|/usr/local/bin|${BIN_DIR}|g" "${REPO_ROOT}/data/applications/${file}" \
            | ${SUDO} tee "/usr/share/applications/${file}" >/dev/null
        ${SUDO} chmod 0644 "/usr/share/applications/${file}"
    done

    ${SUDO} install -m 0644 "${REPO_ROOT}/data/xdg-desktop-portal/neon-de-portals.conf" \
        "/usr/share/xdg-desktop-portal/neon-de-portals.conf"
    ${SUDO} install -m 0644 "${REPO_ROOT}/data/systemd/neon-session.target" \
        "/usr/lib/systemd/user/neon-session.target"
    sed "s|/usr/local/bin|${BIN_DIR}|g" "${REPO_ROOT}/data/systemd/neon-shell.service" \
        | ${SUDO} tee "/usr/lib/systemd/user/neon-shell.service" >/dev/null
    ${SUDO} chmod 0644 "/usr/lib/systemd/user/neon-shell.service"

    echo "==> Installed NEON DE system-wide to ${BIN_DIR}"
    echo "    Wayland sessions added (existing sessions untouched):"
    echo "      - /usr/share/wayland-sessions/neon-de.desktop"
    echo "      - /usr/share/wayland-sessions/neon-de-safe.desktop"
fi

echo "==> Run 'neon-session --diagnostics' to verify installation health."
echo "==> Run 'neon-session --nested' to launch NEON DE safely inside your current desktop."
