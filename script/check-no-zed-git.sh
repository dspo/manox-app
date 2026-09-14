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

if grep -rn "zed-industries" --include="Cargo.toml" . | grep -v "^./target"; then
  echo "error: Cargo.toml files must not reference zed-industries (use gpui-pre* via crates.io)" >&2
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
