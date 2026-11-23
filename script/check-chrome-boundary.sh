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

# 3. The shell must not grow a local selection mirror again (any visibility:
#    a private one drifts just as silently, it only stops aborting): the
#    highlight is the host foreground read through HostHooks::active_row.
#    NOTE: this script is a tripwire, not the defence. The structural guard is
#    `shell_handle()`'s module privacy: nothing outside this file can name a
#    shell entity, so every chrome write must pass through the two functions
#    above.
hits=$(grep -rnE '^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?active[[:space:]]*:' \
    crates/steer-agent-chrome-ui/src/shell.rs || true)
if [ -n "$hits" ]; then
    echo "violation: the shell owns a selection field again:"
    echo "$hits"
    status=1
fi

# 4. A view must not reach itself through its own entity handle. Inside
#    render, a cx.listener callback, or a cx.defer_in closure the view is
#    already the one outstanding lease ("one entity lease at a time"), so
#    cx.entity().read(cx) / cx.entity().update(cx) re-borrow the leased
#    entity and panic on the spot; mutate through &mut self instead.
#    Capturing the handle for later use (cx.entity(), .clone(),
#    .downgrade()) is fine — the lease is only violated by read/update.
hits=$(grep -rnF -e 'cx.entity().read(cx' -e 'cx.entity().update(cx' \
    crates/*/src --include='*.rs' || true)
if [ -n "$hits" ]; then
    echo "violation: view re-enters its own entity handle while leased (use &mut self):"
    echo "$hits"
    status=1
fi

if [ "$status" -eq 0 ]; then
    echo "chrome boundary clean (projection pump + deferred commands + single entity lease)"
fi
exit "$status"
