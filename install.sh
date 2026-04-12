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

TARBALL="symgraph-${VERSION}-${OS}-${ARCH}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${TARBALL}"

echo "Installing symgraph ${VERSION} for ${OS}/${ARCH}..."

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${TARBALL}"
tar -xzf "${TMP_DIR}/${TARBALL}" -C "${TMP_DIR}"

# Install binary and manifest
mkdir -p "${INSTALL_DIR}/bin"
install -m 755 "${TMP_DIR}/symgraph" "${INSTALL_DIR}/bin/symgraph"

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
    MCP_ENTRY="{\"command\":\"${SYMGRAPH_BIN}\",\"args\":[\"serve\"]}"

    configure_json() {
        local file="$1"
        local label="$2"

        if [ ! -f "${file}" ]; then
            mkdir -p "$(dirname "${file}")"
            echo '{}' > "${file}"
        fi

        # Use python3 (available on macOS and most Linux) to safely merge JSON
        python3 -c "
import json, sys
with open('${file}') as f:
    config = json.load(f)
config.setdefault('mcpServers', {})
config['mcpServers']['symgraph'] = {'command': '${SYMGRAPH_BIN}', 'args': ['serve']}
with open('${file}', 'w') as f:
    json.dump(config, f, indent=2)
    f.write('\n')
"
        echo "  Configured ${label}: ${file}"
    }

    echo ""
    echo "Configuring MCP server..."

    # Claude Code: ~/.claude/settings.json
    configure_json "$HOME/.claude/settings.json" "Claude Code"

    # Claude Desktop: platform-specific config
    if [ "${OS}" = "darwin" ]; then
        DESKTOP_CONFIG="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
    else
        DESKTOP_CONFIG="$HOME/.config/Claude/claude_desktop_config.json"
    fi
    configure_json "${DESKTOP_CONFIG}" "Claude Desktop"

    echo ""
    echo "Restart Claude Code / Claude Desktop to pick up the new MCP server."
fi
