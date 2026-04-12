#!/usr/bin/env bash
#
# Devcontainer feature install for codemap.
#
# Delegates to the canonical install.sh from the repo. This thin wrapper
# exists only to handle devcontainer-specific concerns:
#
#   - This script runs as root, so HOME must be overridden to the remote
#     user's home directory; otherwise MCP config lands in /root/.claude
#     instead of /home/<user>/.claude.
#   - Installs to system-wide paths (/usr/local/lib/codemap, /usr/local/bin)
#     rather than the per-user ~/.codemap default.
#   - Fixes file ownership after install since root created the files but
#     the remote user needs to own their Claude config.
#   - The canonical install.sh requires python3 for JSON config merging.
#     Most devcontainer base images include it; if yours doesn't, add the
#     ghcr.io/devcontainers/features/common-utils feature (listed in
#     installsAfter in devcontainer-feature.json).
#
set -euo pipefail

REPO="grahambrooks/codemap"
INSTALL_DIR="/usr/local/lib/codemap"
BIN_DIR="/usr/local/bin"
REMOTE_USER="${_REMOTE_USER:-vscode}"
VERSION="${VERSION:-latest}"

# Run the canonical install script with system-wide paths and MCP config.
# Override HOME so MCP config is written to the remote user's home, not root's.
export CODEMAP_VERSION="${VERSION}"
export CODEMAP_INSTALL_DIR="${INSTALL_DIR}"
export HOME="/home/${REMOTE_USER}"

curl -fsSL "https://raw.githubusercontent.com/${REPO}/main/install.sh" | bash -s -- --mcp

# Symlink into system PATH
ln -sf "${INSTALL_DIR}/bin/codemap" "${BIN_DIR}/codemap"

# Fix ownership — install script ran as root
chown -R "${REMOTE_USER}:${REMOTE_USER}" "${HOME}/.claude" 2>/dev/null || true
