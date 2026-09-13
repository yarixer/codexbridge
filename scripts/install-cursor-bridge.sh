#!/usr/bin/env sh
set -eu

VERSION="${1:-1.0.31}"
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)
    TARGET="linux-x64"
    EXPECTED_SHA256="527cbebdc6aad4ea7d3026f49b4879e3e7f3d6e907c0598241863802b022c838"
    ;;
  aarch64|arm64)
    TARGET="linux-arm64"
    EXPECTED_SHA256="c5b3dce52ba01f60b152861f008e8d2c931c0ba9fbf79a110ddea375ae40717b"
    ;;
  *)
    echo "Unsupported Linux architecture: $ARCH" >&2
    exit 1
    ;;
esac

ARCHIVE="cursor-sdk-bridge-standalone-$TARGET.tar.gz"
PROJECT_ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
INSTALL_ROOT="$PROJECT_ROOT/.tools/cursor-sdk-bridge"
TEMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEMP_ROOT"' EXIT

curl --fail --location --silent --show-error \
  "https://github.com/cursor/sdk-bridge/releases/download/v$VERSION/$ARCHIVE" \
  --output "$TEMP_ROOT/$ARCHIVE"
echo "$EXPECTED_SHA256  $TEMP_ROOT/$ARCHIVE" | sha256sum --check --status
mkdir -p "$INSTALL_ROOT"
tar -xzf "$TEMP_ROOT/$ARCHIVE" -C "$INSTALL_ROOT"
chmod +x "$INSTALL_ROOT/bin/cursor-sdk-bridge"
"$INSTALL_ROOT/bin/cursor-sdk-bridge" --help >/dev/null
echo "Installed: $INSTALL_ROOT/bin/cursor-sdk-bridge"

