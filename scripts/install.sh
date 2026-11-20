#!/usr/bin/env sh
# install.sh — download and install aix to ~/.local/bin (or $AIX_INSTALL_DIR)
set -eu

REPO="get-aix/aix"
BIN="aix"
INSTALL_DIR="${AIX_INSTALL_DIR:-${HOME}/.local/bin}"

# ── helpers ────────────────────────────────────────────────────────────────────
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# ── platform detection ─────────────────────────────────────────────────────────
case "$(uname -s)" in
    Linux)  _os="unknown-linux-musl" ;;  # static, works on any distro
    Darwin) _os="apple-darwin" ;;
    *)      die "unsupported OS: $(uname -s)" ;;
esac

case "$(uname -m)" in
    x86_64)          _arch="x86_64" ;;
    aarch64 | arm64) _arch="aarch64" ;;
    *)               die "unsupported architecture: $(uname -m)" ;;
esac

TARGET="${_arch}-${_os}"
BASE_URL="https://github.com/${REPO}/releases/latest/download"
ASSET="${BIN}-${TARGET}"
case "${BASE_URL}" in
    */releases/download/nightly) RELEASE_CHANNEL="nightly" ;;
    *)                           RELEASE_CHANNEL="stable" ;;
esac

# ── download ───────────────────────────────────────────────────────────────────
printf 'Downloading aix (%s)...\n' "${TARGET}"
mkdir -p "${INSTALL_DIR}"

TMP="$(mktemp)"
SUMS="$(mktemp)"
VERSION_TMP="$(mktemp)"
STATE_TMP=""
cleanup() {
    rm -f "${TMP}" "${SUMS}" "${VERSION_TMP}"
    [ -z "${STATE_TMP}" ] || rm -f "${STATE_TMP}"
}
trap cleanup EXIT INT TERM

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        die "curl or wget is required"
    fi
}

fetch "${BASE_URL}/${ASSET}" "${TMP}"

# ── checksum verification ──────────────────────────────────────────────────────
if command -v sha256sum >/dev/null 2>&1; then
    _sha() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    _sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    _sha() { :; }
fi

SUMS_PRESENT=0
if fetch "${BASE_URL}/SHA256SUMS" "${SUMS}" 2>/dev/null; then
    SUMS_PRESENT=1
    expected="$(awk -v f="${ASSET}" '$2 == f || $2 == "*" f { print $1 }' "${SUMS}")"
    actual="$(_sha "${TMP}")"
    if [ -z "${expected}" ]; then
        die "no checksum for ${ASSET} in SHA256SUMS"
    elif [ -z "${actual}" ]; then
        printf 'warning: sha256sum/shasum not found; skipping checksum verification\n' >&2
    elif [ "${expected}" != "${actual}" ]; then
        die "checksum mismatch for ${ASSET} (expected ${expected}, got ${actual})"
    else
        printf 'Checksum verified.\n'
    fi
else
    printf 'warning: SHA256SUMS not published for this release; skipping verification\n' >&2
fi

if [ "${RELEASE_CHANNEL}" = "nightly" ]; then
    fetch "${BASE_URL}/NIGHTLY_VERSION" "${VERSION_TMP}"
    if [ "${SUMS_PRESENT}" -eq 1 ]; then
        expected="$(awk '$2 == "NIGHTLY_VERSION" || $2 == "*NIGHTLY_VERSION" { print $1 }' "${SUMS}")"
        actual="$(_sha "${VERSION_TMP}")"
        if [ -z "${expected}" ]; then
            die "no checksum for NIGHTLY_VERSION in SHA256SUMS"
        elif [ -z "${actual}" ]; then
            printf 'warning: sha256sum/shasum not found; skipping NIGHTLY_VERSION verification\n' >&2
        elif [ "${expected}" != "${actual}" ]; then
            die "checksum mismatch for NIGHTLY_VERSION (expected ${expected}, got ${actual})"
        fi
    fi
    BUILD_ID="$(tr -d '\r\n' < "${VERSION_TMP}")"
    if [ "${#BUILD_ID}" -ne 40 ] || ! printf '%s\n' "${BUILD_ID}" | grep -Eq '^[0-9a-fA-F]{40}$'; then
        die "NIGHTLY_VERSION does not contain a 40-character commit ID"
    fi
    UPDATE_TARGET="nightly"
    INSTALLED_BUILD="nightly:${BUILD_ID}"
else
    UPDATE_TARGET="stable"
    INSTALLED_BUILD="stable"
fi

chmod +x "${TMP}"
mv "${TMP}" "${INSTALL_DIR}/${BIN}"
printf 'Installed to %s/%s\n' "${INSTALL_DIR}" "${BIN}"

STATE_TMP="$(mktemp "${INSTALL_DIR}/.aix-update-state.XXXXXX")"
printf 'target=%s\ninstalled=%s\n' "${UPDATE_TARGET}" "${INSTALLED_BUILD}" > "${STATE_TMP}"
chmod 600 "${STATE_TMP}"
mv -f "${STATE_TMP}" "${INSTALL_DIR}/.aix-update-state"
STATE_TMP=""
printf 'Update target: %s\n' "${UPDATE_TARGET}"

# ── PATH wiring ────────────────────────────────────────────────────────────────
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
        printf '%s is already in PATH\n' "${INSTALL_DIR}"
        ;;
    *)
        SHELL_NAME="$(basename "${SHELL:-sh}")"
        case "${SHELL_NAME}" in
            zsh)  SHELL_RC="${ZDOTDIR:-${HOME}}/.zshrc" ;;
            bash) SHELL_RC="${HOME}/.bashrc" ;;
            fish) SHELL_RC="${XDG_CONFIG_HOME:-${HOME}/.config}/fish/config.fish" ;;
            *)    SHELL_RC="${HOME}/.profile" ;;
        esac

        if [ "${SHELL_NAME}" = "fish" ]; then
            PATH_LINE="fish_add_path ${INSTALL_DIR}"
        else
            PATH_LINE="export PATH=\"${INSTALL_DIR}:\${PATH}\""
        fi

        if grep -qF "${INSTALL_DIR}" "${SHELL_RC}" 2>/dev/null; then
            : # already wired
        else
            printf '\n# Added by aix installer\n%s\n' "${PATH_LINE}" >> "${SHELL_RC}"
            printf 'Added %s to PATH in %s\n' "${INSTALL_DIR}" "${SHELL_RC}"
        fi
        printf 'Restart your shell or run: . %s\n' "${SHELL_RC}"
        ;;
esac
