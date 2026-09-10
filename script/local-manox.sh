#!/usr/bin/env bash
# Toggle the local manox [patch] override for this repo's git dependencies.
#
# ON  (default): rewrite .cargo/config.toml so the six manox-agent/-harness/
#     -protocol/-session-core/-providers/-supervisor (+ lsp) git deps resolve
#     from a local manox checkout instead of GitHub — daily two-repo workflow:
#     edits in ../manox are picked up by the next cargo build, no push needed.
# OFF: remove the override (falls back to pure git deps pinned by Cargo.lock —
#     what CI uses). .cargo/config.toml is gitignored on purpose.
#
# Usage:
#   script/local-manox.sh on  [path-to-manox]   # default: ../manox
#   script/local-manox.sh off
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CFG_DIR="$ROOT/.cargo"
CFG="$CFG_DIR/config.toml"

case "${1:-}" in
    on)
        MANOX_DIR="${2:-$ROOT/../manox}"
        if [[ ! -d "$MANOX_DIR/crates/manox-agent" ]]; then
            echo "error: $MANOX_DIR is not a manox checkout (crates/manox-agent missing)" >&2
            exit 1
        fi
        MANOX_DIR="$(cd "$MANOX_DIR" && pwd)"
        mkdir -p "$CFG_DIR"
        cat > "$CFG" <<EOF
# 本地 manox 联动覆盖 — 由 script/local-manox.sh 生成，勿手编勿提交。
[patch."https://github.com/dspo/manox"]
manox-agent = { path = "$MANOX_DIR/crates/manox-agent" }
manox-harness = { path = "$MANOX_DIR/crates/manox-harness" }
manox-protocol = { path = "$MANOX_DIR/crates/manox-protocol" }
manox-session-core = { path = "$MANOX_DIR/crates/manox-session-core" }
manox-providers = { path = "$MANOX_DIR/crates/manox-providers" }
supervisor = { path = "$MANOX_DIR/crates/supervisor" }
lsp = { path = "$MANOX_DIR/crates/lsp" }
EOF
        echo "local override ON -> $MANOX_DIR"
        ;;
    off)
        rm -f "$CFG"
        echo "local override OFF (pure git deps)"
        ;;
    *)
        echo "usage: ${0##*/} on [path-to-manox] | off" >&2
        exit 1
        ;;
esac
