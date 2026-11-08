#!/bin/sh
set -eu
BUNDLE=${1:?bundle directory required}
SMOKE_DIR=$(mktemp -d)
WEB_PID=""
cleanup() { if [ -n "$WEB_PID" ]; then kill "$WEB_PID" 2>/dev/null || true; wait "$WEB_PID" 2>/dev/null || true; fi; rm -rf "$SMOKE_DIR"; }
trap cleanup EXIT HUP INT TERM
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

"$BUNDLE/proto-gen" serve --config "$SMOKE_DIR/proto/generator.toml" --listen 127.0.0.1:0 > "$SMOKE_DIR/web.log" 2>&1 &
WEB_PID=$!
python3 - "$SMOKE_DIR/web.log" <<'PYTEST'
import json,pathlib,re,sys,time,urllib.request
log=pathlib.Path(sys.argv[1])
for attempt in range(50):
    match=re.search(r"ProtoGen Web: (http://[^\s]+)",log.read_text())
    if match:
        base=match[1]
        with urllib.request.urlopen(base+"/api/projects/0/protocols") as response:
            data=json.load(response)
        assert data["protocols"][0]["title"] == "release smoke"
        with urllib.request.urlopen(base) as response:
            assert "ProtoGen" in response.read().decode()
        print("embedded Web smoke passed")
        break
    time.sleep(.1)
else:
    raise RuntimeError("Web server did not start: "+log.read_text())
PYTEST
