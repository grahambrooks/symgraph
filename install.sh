#!/usr/bin/env bash
set -euo pipefail

REPO="grahambrooks/symgraph"
INSTALL_DIR="${SYMGRAPH_INSTALL_DIR:-$HOME/.symgraph}"
VERSION="${SYMGRAPH_VERSION:-latest}"
CONFIGURE_MCP=false

# Parse arguments
for arg in "$@"; do
    case "${arg}" in
        --mcp) CONFIGURE_MCP=true ;;
        --help|-h)
            echo "Usage: install.sh [OPTIONS]"
            echo ""
            echo "Install symgraph from GitHub releases."
            echo ""
            echo "Options:"
            echo "  --mcp    Configure symgraph as an MCP server for Claude Code and Claude Desktop"
            echo "  --help   Show this help message"
            echo ""
            echo "Environment variables:"
            echo "  SYMGRAPH_VERSION       Version to install (default: latest)"
            echo "  SYMGRAPH_INSTALL_DIR   Installation directory (default: ~/.symgraph)"
            exit 0
            ;;
    esac
done

# Detect architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64 | amd64) ARCH="x64" ;;
    aarch64 | arm64) ARCH="arm64" ;;
    *)
        echo "Error: unsupported architecture '${ARCH}'"
        exit 1
        ;;
esac

# Detect OS
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "${OS}" in
    linux) ;;
    darwin) ;;
    *)
        echo "Error: unsupported OS '${OS}'"
        exit 1
        ;;
esac

# Resolve version
if [ "${VERSION}" = "latest" ]; then
    RELEASE_URL="https://api.github.com/repos/${REPO}/releases/latest"
    VERSION="$(curl -fsSL "${RELEASE_URL}" | grep '"tag_name"' | sed -E 's/.*"v?([^"]+)".*/\1/')"
    if [ -z "${VERSION}" ]; then
        echo "Error: failed to resolve latest version"
        exit 1
    fi
    echo "Resolved latest version: ${VERSION}"
fi

# Strip leading 'v' if present
VERSION="${VERSION#v}"

# Release archives are named symgraph-v<version>-<rust target triple>.tar.gz
# (release-kit v2). Releases cut before that used symgraph-<version>-<os>-<arch>,
# which is tried as a fallback so pinned older versions still install.
case "${ARCH}" in
    x64) CPU="x86_64" ;;
    arm64) CPU="aarch64" ;;
esac
case "${OS}" in
    linux) TARGET="${CPU}-unknown-linux-gnu" ;;
    darwin) TARGET="${CPU}-apple-darwin" ;;
esac
TARBALL="symgraph-v${VERSION}-${TARGET}.tar.gz"
LEGACY_TARBALL="symgraph-${VERSION}-${OS}-${ARCH}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/v${VERSION}"
CHECKSUM_URL="${BASE_URL}/SHA256SUMS"

echo "Installing symgraph ${VERSION} for ${OS}/${ARCH}..."

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

# `ASSET` is the name the release actually carries, which is also how
# SHA256SUMS lists it. The two naming schemes must not be confused at
# verification time, or a legitimate download looks unlisted.
ASSET="${TARBALL}"
if ! curl -fsSL "${BASE_URL}/${TARBALL}" -o "${TMP_DIR}/${TARBALL}" 2>/dev/null; then
    ASSET="${LEGACY_TARBALL}"
    curl -fsSL "${BASE_URL}/${LEGACY_TARBALL}" -o "${TMP_DIR}/${TARBALL}"
fi

# Verify the download against the release's published checksums. This script
# is run as `curl | bash`, so the archive is executed on the machine moments
# later; taking it on trust from the transport alone is not good enough.
verify_checksum() {
    local expected actual
    if ! curl -fsSL "${CHECKSUM_URL}" -o "${TMP_DIR}/SHA256SUMS" 2>/dev/null; then
        # Releases before checksums were published have no manifest to check.
        echo "Warning: no SHA256SUMS published for v${VERSION}; skipping verification." >&2
        return 0
    fi

    expected="$(awk -v f="${ASSET}" '$2 == f || $2 == "*" f { print $1 }' "${TMP_DIR}/SHA256SUMS")"
    if [ -z "${expected}" ]; then
        echo "Error: ${ASSET} is not listed in SHA256SUMS." >&2
        exit 1
    fi

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "${TMP_DIR}/${TARBALL}" | cut -d' ' -f1)"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "${TMP_DIR}/${TARBALL}" | cut -d' ' -f1)"
    else
        echo "Error: neither sha256sum nor shasum found; cannot verify download." >&2
        exit 1
    fi

    if [ "${expected}" != "${actual}" ]; then
        echo "Error: checksum mismatch for ${ASSET}." >&2
        echo "  expected: ${expected}" >&2
        echo "  actual:   ${actual}" >&2
        echo "Refusing to install. Report this at https://github.com/${REPO}/issues" >&2
        exit 1
    fi
    echo "Checksum verified."
}
verify_checksum

tar -xzf "${TMP_DIR}/${TARBALL}" -C "${TMP_DIR}"

# Install binaries and manifest. The tarball ships both the full `symgraph`
# (CLI + MCP server) and the lean `symgraph-cli`; only the former used to be
# installed, leaving the other silently discarded.
mkdir -p "${INSTALL_DIR}/bin"
install -m 755 "${TMP_DIR}/symgraph" "${INSTALL_DIR}/bin/symgraph"
if [ -f "${TMP_DIR}/symgraph-cli" ]; then
    install -m 755 "${TMP_DIR}/symgraph-cli" "${INSTALL_DIR}/bin/symgraph-cli"
fi

if [ -f "${TMP_DIR}/manifest.json" ]; then
    install -m 644 "${TMP_DIR}/manifest.json" "${INSTALL_DIR}/manifest.json"
fi

# Add to PATH guidance
SHELL_NAME="$(basename "${SHELL:-/bin/bash}")"
case "${SHELL_NAME}" in
    zsh)  PROFILE="$HOME/.zshrc" ;;
    bash) PROFILE="$HOME/.bashrc" ;;
    fish) PROFILE="$HOME/.config/fish/config.fish" ;;
    *)    PROFILE="$HOME/.profile" ;;
esac

PATH_ENTRY="${INSTALL_DIR}/bin"
if ! echo "${PATH}" | tr ':' '\n' | grep -qx "${PATH_ENTRY}"; then
    echo ""
    echo "Add symgraph to your PATH by running:"
    echo ""
    if [ "${SHELL_NAME}" = "fish" ]; then
        echo "  fish_add_path ${PATH_ENTRY}"
    else
        echo "  echo 'export PATH=\"${PATH_ENTRY}:\$PATH\"' >> ${PROFILE}"
    fi
    echo ""
    echo "Then restart your shell or run:"
    echo "  export PATH=\"${PATH_ENTRY}:\$PATH\""
fi

echo ""
echo "symgraph ${VERSION} installed to ${INSTALL_DIR}/bin/symgraph"

# Configure as MCP server
if [ "${CONFIGURE_MCP}" = true ]; then
    SYMGRAPH_BIN="${INSTALL_DIR}/bin/symgraph"

    # Merge a server entry into a JSON config, without disturbing the rest of
    # the file. Paths are passed as arguments rather than spliced into the
    # program text: an install dir containing a quote or a backslash would
    # otherwise produce a syntax error at best, and execute as code at worst.
    configure_json() {
        local file="$1"
        local label="$2"

        if [ ! -f "${file}" ]; then
            mkdir -p "$(dirname "${file}")"
            echo '{}' > "${file}"
        fi

        python3 - "${file}" "${SYMGRAPH_BIN}" <<'PYEOF'
import json
import sys

config_path, binary = sys.argv[1], sys.argv[2]

try:
    with open(config_path) as handle:
        config = json.load(handle)
except (OSError, ValueError) as exc:
    raise SystemExit(f"could not read {config_path}: {exc}")

if not isinstance(config, dict):
    raise SystemExit(f"{config_path} is not a JSON object; leaving it alone")

servers = config.setdefault("mcpServers", {})
servers["symgraph"] = {"command": binary, "args": ["serve"]}

with open(config_path, "w") as handle:
    json.dump(config, handle, indent=2)
    handle.write("\n")
PYEOF
        echo "  Configured ${label}: ${file}"
    }

    echo ""
    echo "Configuring MCP server..."

    # Claude Code: prefer its own CLI, which owns the user-scope config and
    # will keep owning it if the file layout changes. Only fall back to
    # editing the config directly when the CLI is not installed.
    if command -v claude >/dev/null 2>&1; then
        if claude mcp add symgraph --scope user -- "${SYMGRAPH_BIN}" serve >/dev/null 2>&1; then
            echo "  Configured Claude Code (via 'claude mcp add --scope user')"
        else
            echo "  Claude Code: 'claude mcp add' failed — it may already be configured."
            echo "               Check with: claude mcp list"
        fi
    else
        configure_json "$HOME/.claude.json" "Claude Code"
    fi

    # Claude Desktop reads mcpServers from its own config file.
    if [ "${OS}" = "darwin" ]; then
        DESKTOP_CONFIG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
    else
        DESKTOP_CONFIG="$HOME/.config/Claude/claude_desktop_config.json"
    fi
    configure_json "${DESKTOP_CONFIG}" "Claude Desktop"

    echo ""
    echo "Restart Claude Code / Claude Desktop to pick up the new MCP server."
fi
