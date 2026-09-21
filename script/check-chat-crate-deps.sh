#!/usr/bin/env bash
# Dependency invariant for the chrome/chat split (PLAN-CHROME-CHAT-SPLIT §1):
# the chat crate never touches terminals, webviews, or external-agent wiring.
# Those live with the chrome crate and the assembly layer. Any hit here means
# the decoupling regressed.
set -euo pipefail

cd "$(dirname "$0")/.."

banned=("terminal-ui" "manox-webview" "manox-ext-agents")
status=0
for dep in "${banned[@]}"; do
    if cargo tree -p manox-agent-chat-ui -i "$dep" >/dev/null 2>&1; then
        echo "violation: manox-agent-chat-ui depends on $dep"
        cargo tree -p manox-agent-chat-ui -i "$dep" || true
        status=1
    fi
done
if [ "$status" -eq 0 ]; then
    echo "chat-crate deps clean (no ${banned[*]})"
fi
exit "$status"
