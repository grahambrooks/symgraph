#!/usr/bin/env bash
set -euo pipefail

VERSION="${VERSION:-latest}"
REPO="grahambrooks/codemap"
INSTALL_DIR="/usr/local/bin"

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

install -d "${INSTALL_DIR}"
install -m 755 "${TMP_DIR}/codemap" "${INSTALL_DIR}/codemap"

echo "codemap ${VERSION} installed to ${INSTALL_DIR}/codemap"
