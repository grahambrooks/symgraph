#!/usr/bin/env bash
set -euo pipefail

# Test that symgraph is installed and executable
if ! command -v symgraph &>/dev/null; then
    echo "FAIL: symgraph not found on PATH"
    exit 1
fi
echo "PASS: symgraph is installed at $(command -v symgraph)"

# Test that it responds to --help
if symgraph --help &>/dev/null; then
    echo "PASS: symgraph --help exits successfully"
else
    echo "FAIL: symgraph --help failed"
    exit 1
fi

# Test that manifest.json is installed
if [ -f /usr/local/lib/symgraph/manifest.json ]; then
    echo "PASS: manifest.json installed"
else
    echo "FAIL: manifest.json not found at /usr/local/lib/symgraph/manifest.json"
    exit 1
fi

# Test that Claude Code MCP config was written
SETTINGS_FILE="/home/${_REMOTE_USER:-vscode}/.claude/settings.json"
if [ -f "${SETTINGS_FILE}" ]; then
    if grep -q '"symgraph"' "${SETTINGS_FILE}"; then
        echo "PASS: symgraph MCP server configured in ${SETTINGS_FILE}"
    else
        echo "FAIL: symgraph not found in ${SETTINGS_FILE}"
        exit 1
    fi
else
    echo "FAIL: Claude Code settings not found at ${SETTINGS_FILE}"
    exit 1
fi
