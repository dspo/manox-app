#!/usr/bin/env bash
# v3 protocol gate: the app is a pure AHP client, so the v2 wire vocabulary
# must not reappear in production code. Any hit here means a v2 seam crept
# back in (or a new protocol path bypassed the AhpStore).
set -euo pipefail

cd "$(dirname "$0")/.."

banned=('manox_protocol' 'FromClient' 'FromServer' 'ClientCall' 'ClientNote' 'ServerCall' 'ServerNote' 'PROTOCOL_EPOCH' 'StreamFrame')
status=0
for word in "${banned[@]}"; do
    # A hit is a CODE line: `path:line:content` where the content does not
    # start with a comment. (The old two-stage filter was a no-op — grep
    # output lines start with the path, so `^\s*//` never matched.)
    hits=$(grep -rn --include='*.rs' "$word" crates/ | grep -cvE ':[0-9]+: *//' || true)
    if [ "$hits" -gt 0 ]; then
        echo "violation: $word appears $hits time(s) in production code:"
        grep -rn --include='*.rs' "$word" crates/ | grep -vE ':[0-9]+: *//' | head -5
        status=1
    fi
done
if [ "$status" -eq 0 ]; then
    echo "protocol gate clean (zero v2 vocabulary)"
fi
exit "$status"
