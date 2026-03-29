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
