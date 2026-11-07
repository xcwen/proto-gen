#!/bin/sh
# Download and verify one fixed release. Stdout contains only the resolved binary path.
set -eu
VERSION=${1:?Usage: install.sh <vX.Y.Z> <cache-directory>}
CACHE=${2:?cache directory required}
case "$VERSION" in v[0-9]*.[0-9]*.[0-9]*) ;; *) echo 'Invalid proto-gen version' >&2; exit 1 ;; esac
case "$VERSION" in *[!A-Za-z0-9.-]*) echo 'Invalid proto-gen version' >&2; exit 1 ;; esac
case "$(uname -s)" in Darwin) OS=macos ;; Linux) OS=linux ;; *) echo 'Unsupported proto-gen OS' >&2; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) ARCH=aarch64 ;; x86_64|amd64) ARCH=x86_64 ;; *) echo 'Unsupported proto-gen architecture' >&2; exit 1 ;; esac
mkdir -p "$CACHE/$VERSION"
CACHE=$(CDPATH= cd -- "$CACHE" && pwd)
DEST="$CACHE/$VERSION/$OS-$ARCH"
if [ -x "$DEST/proto-gen" ] && [ -x "$DEST/protoc" ] && [ -x "$DEST/protoc-gen-go" ] && [ "$(cat "$DEST/version.txt" 2>/dev/null || true)" = "$VERSION" ]; then
    printf '%s\n' "$DEST/proto-gen"
    exit 0
fi
PACKAGE="proto-gen-$VERSION-$OS-$ARCH"
ASSET="$PACKAGE.tar.gz"
BASE=${PROTO_GEN_RELEASE_BASE_URL:-https://github.com/xcwen/proto-gen/releases/download}
DOWNLOAD_DIR=$(mktemp -d "$CACHE/$VERSION/.install-XXXXXX")
trap 'rm -rf "$DOWNLOAD_DIR"' EXIT HUP INT TERM
echo "Downloading proto-gen $VERSION ($OS-$ARCH)" >&2
curl -fsSL --retry 3 --connect-timeout 15 --max-time 180 "$BASE/$VERSION/$ASSET" -o "$DOWNLOAD_DIR/$ASSET"
curl -fsSL --retry 3 --connect-timeout 15 --max-time 60 "$BASE/$VERSION/SHA256SUMS" -o "$DOWNLOAD_DIR/SHA256SUMS"
EXPECTED=$(awk -v asset="$ASSET" '$2 == asset || $2 == "./" asset {print $1}' "$DOWNLOAD_DIR/SHA256SUMS")
if command -v sha256sum >/dev/null 2>&1; then
    ACTUAL=$(sha256sum "$DOWNLOAD_DIR/$ASSET" | awk '{print $1}')
else
    ACTUAL=$(shasum -a 256 "$DOWNLOAD_DIR/$ASSET" | awk '{print $1}')
fi
if [ -z "$EXPECTED" ] || [ "$EXPECTED" != "$ACTUAL" ]; then
    echo 'proto-gen checksum verification failed; no binary installed' >&2
    exit 1
fi
tar -xzf "$DOWNLOAD_DIR/$ASSET" -C "$DOWNLOAD_DIR" "$PACKAGE"
test "$(cat "$DOWNLOAD_DIR/$PACKAGE/version.txt")" = "$VERSION"
test -f "$DOWNLOAD_DIR/$PACKAGE/proto-gen"
test -f "$DOWNLOAD_DIR/$PACKAGE/protoc"
test -f "$DOWNLOAD_DIR/$PACKAGE/protoc-gen-go"
chmod 0755 "$DOWNLOAD_DIR/$PACKAGE/proto-gen" "$DOWNLOAD_DIR/$PACKAGE/protoc" "$DOWNLOAD_DIR/$PACKAGE/protoc-gen-go"
if [ -e "$DEST" ]; then mv "$DEST" "$DOWNLOAD_DIR/previous"; fi
if ! mv "$DOWNLOAD_DIR/$PACKAGE" "$DEST"; then
    if [ -d "$DOWNLOAD_DIR/previous" ]; then mv "$DOWNLOAD_DIR/previous" "$DEST"; fi
    exit 1
fi
printf '%s\n' "$DEST/proto-gen"
