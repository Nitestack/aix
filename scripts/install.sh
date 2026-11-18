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

# ── download ───────────────────────────────────────────────────────────────────
printf 'Downloading aix (%s)...\n' "${TARGET}"
mkdir -p "${INSTALL_DIR}"

TMP="$(mktemp)"
SUMS="$(mktemp)"
trap 'rm -f "${TMP}" "${SUMS}"' EXIT INT TERM

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

if fetch "${BASE_URL}/SHA256SUMS" "${SUMS}" 2>/dev/null; then
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

chmod +x "${TMP}"
mv "${TMP}" "${INSTALL_DIR}/${BIN}"
printf 'Installed to %s/%s\n' "${INSTALL_DIR}" "${BIN}"

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
