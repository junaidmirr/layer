#!/usr/bin/env bash
# Layer macOS & Linux Installer
# Usage: curl -fsSL https://raw.githubusercontent.com/junaidmirr/layer/master/install.sh | sh

set -euo pipefail

REPO="junaidmirr/layer"
INSTALL_DIR="${HOME}/.local/bin"
EXE_PATH="${INSTALL_DIR}/layer"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
    Darwin)
        if [ "${ARCH}" = "arm64" ]; then
            TARGET="aarch64-apple-darwin"
        else
            TARGET="x86_64-apple-darwin"
        fi
        ;;
    Linux)
        if [ "${ARCH}" = "x86_64" ]; then
            TARGET="x86_64-unknown-linux-gnu"
        else
            echo "Unsupported Linux architecture: ${ARCH}" >&2
            exit 1
        fi
        ;;
    *)
        echo "Unsupported OS: ${OS}" >&2
        exit 1
        ;;
esac

echo "Fetching latest release for ${TARGET}..."
RELEASE_JSON=$(curl -fsSL -H "User-Agent: layer-installer" "https://api.github.com/repos/${REPO}/releases")
TAG=$(echo "${RELEASE_JSON}" | grep -m1 '"tag_name":' | cut -d '"' -f 4)

if [ -z "${TAG}" ]; then
    echo "Could not find a release for ${REPO}." >&2
    exit 1
fi

echo "Selected version: ${TAG}"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${TAG}/layer-${TARGET}.tar.gz"

TMP_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "Downloading ${DOWNLOAD_URL}..."
curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/layer.tar.gz"

mkdir -p "${INSTALL_DIR}"
tar -xzf "${TMP_DIR}/layer.tar.gz" -C "${TMP_DIR}"
cp "${TMP_DIR}/layer" "${EXE_PATH}"
chmod +x "${EXE_PATH}"

echo ""
echo "Layer (${TAG}) installed successfully to ${EXE_PATH}!"

# Check if INSTALL_DIR is in PATH
case ":${PATH}:" in
    *:"${INSTALL_DIR}":*) ;;
    *)
        echo ""
        echo "Note: ${INSTALL_DIR} is not in your PATH."
        echo "Add it by adding this to your ~/.bashrc or ~/.zshrc:"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac

echo ""
echo "Run 'layer --help' or 'layer mcp' to get started."
