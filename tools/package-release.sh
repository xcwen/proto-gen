#!/bin/sh
set -eu
PACKAGE="proto-gen-${RELEASE_TAG:?}-${ASSET:?}"
mkdir -p "dist/$PACKAGE/schema/common"
install -m 0755 "target/${TARGET:?}/release/proto-gen" "dist/$PACKAGE/proto-gen"
install -m 0755 toolchain/bin/protoc "dist/$PACKAGE/protoc"
install -m 0755 dist-tools/protoc-gen-go "dist/$PACKAGE/protoc-gen-go"
cp schema/common/*.proto "dist/$PACKAGE/schema/common/"
cp README.md include/LICENSE.protobuf "dist/$PACKAGE/"
printf '%s\n' "$RELEASE_TAG" > "dist/$PACKAGE/version.txt"
"dist/$PACKAGE/proto-gen" --version
"dist/$PACKAGE/protoc" --version
"dist/$PACKAGE/protoc-gen-go" --version
tools/smoke-release.sh "$PWD/dist/$PACKAGE"
tar -czf "dist/$PACKAGE.tar.gz" -C dist "$PACKAGE"
