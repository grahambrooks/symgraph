#!/usr/bin/env bash
set -euo pipefail

VERSION="${VERSION:-latest}"
REPO="grahambrooks/codemap"
INSTALL_DIR="/usr/local/lib/codemap"
BIN_DIR="/usr/local/bin"

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
    echo "Resolved latest version: ${VERSION}"
fi

# Strip leading 'v' if present
VERSION="${VERSION#v}"

TARBALL="codemap-${VERSION}-${OS}-${ARCH}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${TARBALL}"

echo "Downloading codemap ${VERSION} for ${OS}/${ARCH}..."
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${TARBALL}"
tar -xzf "${TMP_DIR}/${TARBALL}" -C "${TMP_DIR}"

# Install binary and manifest
install -d "${INSTALL_DIR}/bin"
install -m 755 "${TMP_DIR}/codemap" "${INSTALL_DIR}/bin/codemap"

if [ -f "${TMP_DIR}/manifest.json" ]; then
    install -m 644 "${TMP_DIR}/manifest.json" "${INSTALL_DIR}/manifest.json"
fi

# Symlink binary to PATH
install -d "${BIN_DIR}"
ln -sf "${INSTALL_DIR}/bin/codemap" "${BIN_DIR}/codemap"

# Configure as MCP server for Claude Code
CLAUDE_CONFIG_DIR="/home/${_REMOTE_USER:-vscode}/.claude"
mkdir -p "${CLAUDE_CONFIG_DIR}"

SETTINGS_FILE="${CLAUDE_CONFIG_DIR}/settings.json"
if [ -f "${SETTINGS_FILE}" ]; then
    EXISTING=$(cat "${SETTINGS_FILE}")
else
    EXISTING='{}'
fi

# Add codemap to mcpServers in Claude Code settings
UPDATED=$(echo "${EXISTING}" | python3 -c "
import json, sys
settings = json.load(sys.stdin)
settings.setdefault('mcpServers', {})
settings['mcpServers']['codemap'] = {
    'command': '${INSTALL_DIR}/bin/codemap',
    'args': ['serve']
}
json.dump(settings, sys.stdout, indent=2)
")
echo "${UPDATED}" > "${SETTINGS_FILE}"
chown -R "${_REMOTE_USER:-vscode}:${_REMOTE_USER:-vscode}" "${CLAUDE_CONFIG_DIR}" 2>/dev/null || true

echo "codemap ${VERSION} installed to ${INSTALL_DIR}"
echo "MCP server configured in ${SETTINGS_FILE}"
