#!/usr/bin/env bash
# Dependency invariant for the chrome/chat split: the chat crate never
# touches terminals, webviews, or external-agent wiring.
# Those live with the chrome crate and the assembly layer. Any hit here means
# the decoupling regressed.
#
# The canvas model has its own half of the invariant: its pure modules (ids,
# layout, state, persistence) must build without a GUI stack, and the model
# never depends on the chat crate — the arrow runs chrome/assembly → canvas.
set -euo pipefail

cd "$(dirname "$0")/.."

banned=("steer-webview" "steer-ext-agents")
status=0
for dep in "${banned[@]}"; do
    if cargo tree -p steer-agent-chat-ui -i "$dep" >/dev/null 2>&1; then
        echo "violation: steer-agent-chat-ui depends on $dep"
        cargo tree -p steer-agent-chat-ui -i "$dep" || true
        status=1
    fi
done

# gpui enters steer-canvas only through the `registry` feature (view
# factories); `--no-default-features` is the pure state model.
if cargo tree -p steer-canvas --no-default-features -e normal 2>/dev/null | grep -q 'gpui'; then
    echo "violation: steer-canvas pulls gpui without its registry feature"
    cargo tree -p steer-canvas --no-default-features -e normal || true
    status=1
fi

if cargo tree -p steer-canvas -e normal 2>/dev/null | grep -q 'steer-agent-chat-ui'; then
    echo "violation: steer-canvas depends on steer-agent-chat-ui"
    cargo tree -p steer-canvas -e normal || true
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "chat-crate deps clean (no ${banned[*]}); canvas model gpui-free and chat-free"
fi
exit "$status"
