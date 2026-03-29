#!/usr/bin/env bash
set -euo pipefail

# Test that codemap is installed and executable
if ! command -v codemap &>/dev/null; then
    echo "FAIL: codemap not found on PATH"
    exit 1
fi
echo "PASS: codemap is installed at $(command -v codemap)"

# Test that it responds to --help
if codemap --help &>/dev/null; then
    echo "PASS: codemap --help exits successfully"
else
    echo "FAIL: codemap --help failed"
    exit 1
fi

# Test that manifest.json is installed
if [ -f /usr/local/lib/codemap/manifest.json ]; then
    echo "PASS: manifest.json installed"
else
    echo "FAIL: manifest.json not found at /usr/local/lib/codemap/manifest.json"
    exit 1
fi

# Test that Claude Code MCP config was written
SETTINGS_FILE="/home/${_REMOTE_USER:-vscode}/.claude/settings.json"
if [ -f "${SETTINGS_FILE}" ]; then
    if grep -q '"codemap"' "${SETTINGS_FILE}"; then
        echo "PASS: codemap MCP server configured in ${SETTINGS_FILE}"
    else
        echo "FAIL: codemap not found in ${SETTINGS_FILE}"
        exit 1
    fi
else
    echo "FAIL: Claude Code settings not found at ${SETTINGS_FILE}"
    exit 1
fi
