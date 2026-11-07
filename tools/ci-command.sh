#!/bin/sh
# Surface failure details as public Actions annotations, without requiring log-download credentials.
set -eu
CI_COMMAND_LOG=$(mktemp)
trap 'rm -f "$CI_COMMAND_LOG"' EXIT HUP INT TERM
if "$@" > "$CI_COMMAND_LOG" 2>&1; then
    cat "$CI_COMMAND_LOG"
else
    cat "$CI_COMMAND_LOG"
    python3 - "$CI_COMMAND_LOG" <<'PY'
from pathlib import Path
import sys
message = '\n'.join(Path(sys.argv[1]).read_text(errors='replace').splitlines()[-70:])
message = message.replace('%', '%25').replace('\r', '%0D').replace('\n', '%0A')
print('::error title=Build or test failed::' + message)
PY
    exit 1
fi
