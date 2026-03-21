#!/bin/sh
set -eu

REPO="ryugou/vibepod"
INSTALL_DIR="/usr/local/bin"

# Get latest version from GitHub API
VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | sed 's/.*"v\(.*\)".*/\1/')

if [ -z "$VERSION" ]; then
    echo "Error: Could not determine latest version" >&2
    exit 1
fi

# Detect platform
OS=$(uname -s)
ARCH=$(uname -m)

case "${OS}" in
    Darwin) PLATFORM="apple-darwin" ;;
    Linux)  PLATFORM="unknown-linux-gnu" ;;
    *)      echo "Error: Unsupported OS: ${OS}" >&2; exit 1 ;;
esac

case "${ARCH}" in
    x86_64)         TARGET="${ARCH}-${PLATFORM}" ;;
    arm64|aarch64)  TARGET="aarch64-${PLATFORM}" ;;
    *)              echo "Error: Unsupported architecture: ${ARCH}" >&2; exit 1 ;;
esac

ARCHIVE="vibepod-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ARCHIVE}"
CHECKSUM_URL="${URL}.sha256"

echo "Installing vibepod v${VERSION} (${TARGET})..."

# Create temp directory
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Download archive and checksum
curl -fsSL -o "${TMP}/${ARCHIVE}" "${URL}"
curl -fsSL -o "${TMP}/${ARCHIVE}.sha256" "${CHECKSUM_URL}"

# Verify checksum
cd "$TMP"
if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "${ARCHIVE}.sha256"
elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "${ARCHIVE}.sha256"
else
    echo "Warning: Could not verify checksum (no sha256sum or shasum found)" >&2
fi

# Extract
tar xzf "${ARCHIVE}"

# Install
if [ -w "${INSTALL_DIR}" ]; then
    mv vibepod "${INSTALL_DIR}/vibepod"
    ln -sf "${INSTALL_DIR}/vibepod" "${INSTALL_DIR}/vp"
else
    echo "Installing to ${INSTALL_DIR} (requires sudo)..."
    sudo mv vibepod "${INSTALL_DIR}/vibepod"
    sudo ln -sf "${INSTALL_DIR}/vibepod" "${INSTALL_DIR}/vp"
fi

echo "vibepod v${VERSION} installed to ${INSTALL_DIR}/vibepod"
echo "vp alias installed to ${INSTALL_DIR}/vp"
