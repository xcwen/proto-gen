#!/bin/sh
set -eu
BUNDLE=${1:?bundle directory required}
SMOKE_DIR=$(mktemp -d)
trap 'rm -rf "$SMOKE_DIR"' EXIT HUP INT TERM
mkdir -p "$SMOKE_DIR/proto/src/common" "$SMOKE_DIR/proto/src/root"
cp "$BUNDLE"/schema/common/*.proto "$SMOKE_DIR/proto/src/common/"
cat > "$SMOKE_DIR/proto/src/root/test__ping.proto" <<'PROTO'
syntax = "proto3";
import "common/api_options.proto";
import "common/field_options.proto";
option (common.api) = { title: "release smoke" auth: PUBLIC };
message in { optional int32 id = 1 [(common.field)={default:"10" examples:"10"}]; optional string title=2 [(common.field)={default:"" examples:"release smoke"}]; }
message out { string result = 1; }
PROTO
cat > "$SMOKE_DIR/proto/generator.toml" <<'CONFIG'
version = 1
target = "php"
source = "src"
output = "../gen/Proto"
controllers = "../controllers"
CONFIG
"$BUNDLE/proto-gen" generate --config "$SMOKE_DIR/proto/generator.toml"
"$BUNDLE/proto-gen" generate --config "$SMOKE_DIR/proto/generator.toml" --check

"$BUNDLE/proto-gen" openapi --config "$SMOKE_DIR/proto/generator.toml" --out "$SMOKE_DIR/openapi.json"
