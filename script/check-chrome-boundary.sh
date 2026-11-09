#!/usr/bin/env bash
# Chrome/host boundary invariant (see AGENTS.md): chrome state is written by
# the projection pump, and host code reaches chrome entities only through
# chrome_assembly's two channels (`project` and `defer_shell`).
#
# Why a script instead of review: a stray `shell.update` compiles, passes every
# test, and only fails later — as a double-lease abort inside a listener, or as
# a silently dropped command. The bug class is invisible until a gesture hits
# it, so the boundary needs a compile-independent check.
set -euo pipefail

cd "$(dirname "$0")/.."

status=0

# 1. No source file outside the assembly may name a chrome entity handle.
#    The chrome crate itself (self.*) and the test/example fixtures are out of
#    scope: they have no host.
hits=$(grep -rnE '\.(shell)\.(update|read)\(' crates \
    --include='*.rs' \
    | grep -v '^crates/agent-ui/src/chrome_assembly.rs:' \
    | grep -v '^crates/steer-agent-chrome-ui/' || true)
if [ -n "$hits" ]; then
    echo "violation: chrome entity touched outside chrome_assembly.rs:"
    echo "$hits"
    status=1
fi

# 2. Inside the assembly, a direct shell write may only appear in the two
#    boundary functions. Everything else goes through defer_shell.
hits=$(awk '
    /^(pub(\([a-z]+\))? )?fn / { fn = $0 }
    /shell\.update\(/ && fn !~ /fn (project|defer_shell)\(/ {
        printf "%s:%d: %s\n", FILENAME, FNR, fn
    }
' crates/agent-ui/src/chrome_assembly.rs || true)
if [ -n "$hits" ]; then
    echo "violation: shell.update outside the boundary channels (project / defer_shell):"
    echo "$hits"
    status=1
fi

# 3. The shell must not grow a local selection mirror again: the highlight is
#    the host foreground read through HostHooks::active_row.
hits=$(grep -rnE '^[[:space:]]*pub active[[:space:]]*:' crates/steer-agent-chrome-ui/src/shell.rs || true)
if [ -n "$hits" ]; then
    echo "violation: the shell owns a selection field again:"
    echo "$hits"
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "chrome boundary clean (projection pump + deferred commands)"
fi
exit "$status"
