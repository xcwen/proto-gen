#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=35.0
case "$(uname -s):$(uname -m)" in
 Darwin:*) PLATFORM=osx-universal_binary ;;
 Linux:x86_64) PLATFORM=linux-x86_64 ;;
 Linux:aarch64|Linux:arm64) PLATFORM=linux-aarch_64 ;;
 *) echo 'Unsupported platform; set PROTOC to a compatible compiler.' >&2; exit 1 ;;
esac
PROTO_DOWNLOAD_DIR=$(mktemp -d)
trap 'rm -rf "$PROTO_DOWNLOAD_DIR"' EXIT HUP INT TERM
curl -fL "https://github.com/protocolbuffers/protobuf/releases/download/v$VERSION/protoc-$VERSION-$PLATFORM.zip" -o "$PROTO_DOWNLOAD_DIR/protoc.zip"
unzip -q "$PROTO_DOWNLOAD_DIR/protoc.zip" bin/protoc -d "$PROTO_DOWNLOAD_DIR"
mkdir -p "$ROOT/toolchain/bin"
install -m 0755 "$PROTO_DOWNLOAD_DIR/bin/protoc" "$ROOT/toolchain/bin/protoc"
"$ROOT/toolchain/bin/protoc" --version
