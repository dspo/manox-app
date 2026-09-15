#!/usr/bin/env bash
# Guard against reintroducing a direct dependency on the zed repo.
#
# The GPUI stack runs on the longbridge/gpui-kit track: gpui itself is the
# `gpui-pre*` snapshot crates (published weekly from zed main by Longbridge,
# each carrying a [package.metadata] zed-rev mapping), and the component layer
# is gpui-component 0.6 from crates.io. manox-app must never depend on
# github.com/zed-industries/zed directly again (see the gpui stack block in the
# root Cargo.toml for the full rationale).
#
# Usage:
#   script/check-no-zed-git.sh          # CI check, non-zero exit on violation
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail=0

# Non-comment `git = "...zed-industries..."` declarations are the violation;
# comments may mention the name to document the policy.
if grep -rEn '^[[:space:]]*[^#]*git[[:space:]]*=[[:space:]]*"[^"]*zed-industries' \
    --include="Cargo.toml" . | grep -v "^./target"; then
  echo "error: Cargo.toml files must not declare zed-industries git deps (use gpui-pre* via crates.io)" >&2
  fail=1
fi

# `[patch]`/`[replace]` entries whose source is a zed git URL are caught here
# (Cargo.toml) or by the Cargo.lock source check below. Known limit: a
# `path`-form patch (e.g. vendoring zed sources into the tree) has no zed
# string to match and no git source in the lock — it is out of scope for this
# guard and is covered by code review instead.
# This precise check also covers a committed .cargo/config.toml override
# (dspo/manox local-dev patches are legitimate — only zed sources are banned).
if [ -f .cargo/config.toml ] \
    && grep -qE 'git[[:space:]]*=[[:space:]]*"[^"]*zed-industries' .cargo/config.toml; then
  echo "error: .cargo/config.toml must not patch/replace with a zed-industries git source" >&2
  fail=1
fi

if grep -q "zed-industries/zed" Cargo.lock; then
  echo "error: Cargo.lock still pins zed-industries/zed packages" >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "ok: no direct zed-industries/zed dependency (gpui-kit track intact)"
