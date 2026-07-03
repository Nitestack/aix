#!/usr/bin/env sh
# install.sh — download and install aix to ~/.local/bin (or $AIX_INSTALL_DIR)
set -eu

REPO="Nitestack/aix"
BIN="aix"
INSTALL_DIR="${AIX_INSTALL_DIR:-${HOME}/.local/bin}"

# ── helpers ────────────────────────────────────────────────────────────────────
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# ── platform detection ─────────────────────────────────────────────────────────
case "$(uname -s)" in
    Linux)  _os="unknown-linux-gnu" ;;
    Darwin) _os="apple-darwin" ;;
    *)      die "unsupported OS: $(uname -s)" ;;
esac

case "$(uname -m)" in
    x86_64)          _arch="x86_64" ;;
    aarch64 | arm64) _arch="aarch64" ;;
    *)               die "unsupported architecture: $(uname -m)" ;;
esac

TARGET="${_arch}-${_os}"
URL="https://github.com/${REPO}/releases/latest/download/${BIN}-${TARGET}"

# ── download ───────────────────────────────────────────────────────────────────
printf 'Downloading aix (%s)...\n' "${TARGET}"
mkdir -p "${INSTALL_DIR}"

TMP="$(mktemp)"
trap 'rm -f "${TMP}"' EXIT INT TERM

if command -v curl >/dev/null 2>&1; then
    curl -fSL "${URL}" -o "${TMP}"
elif command -v wget >/dev/null 2>&1; then
    wget -qO "${TMP}" "${URL}"
else
    die "curl or wget is required"
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
