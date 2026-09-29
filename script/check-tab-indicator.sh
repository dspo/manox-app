#!/usr/bin/env bash
# Alignment guard for the right pane's sliding tab indicator.
#
# The indicator's left edge must line up with its tab's left edge. That is a
# POSITION property, so asserting its width or its y cannot catch a drift: an
# earlier version was exactly the right width and height while sitting 6 logical
# px to the right of its tab, and every existing check passed.
#
# Reference used: the strip's bottom rail. A correct indicator starts at the
# strip's content-left, i.e. rail + the strip's own left padding (4 logical
# = 8 device px). The buggy version started 20 device px in, because the tab
# bounds came from the padded content box and were never rebased.
#
# Requires: ImageMagick (`magick`) and a macOS host for the Metal readback.
# Set SKIP_VISUAL=1 to no-op.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "${SKIP_VISUAL:-}" = "1" ]; then
  echo "check-tab-indicator: SKIP_VISUAL=1, skipping"
  exit 0
fi

if [ "$(uname -s)" != "Darwin" ]; then
  echo "check-tab-indicator: the chrome visual harness is macOS-only, skipping"
  exit 0
fi

command -v magick >/dev/null || {
  echo "check-tab-indicator: ImageMagick (magick) not found, skipping"
  exit 0
}

SHOT="${CHROME_SHOT:-$(mktemp -t manox-tab-indicator).png}"
CHROME_SHOT="$SHOT" CHROME_RIGHT=1 \
  cargo test -p manox-agent-chrome-ui --test visual >/dev/null 2>&1

[ -f "$SHOT" ] || {
  echo "check-tab-indicator: no screenshot at $SHOT" >&2
  exit 1
}

python3 - "$SHOT" <<'PYEOF'
import subprocess
import sys

shot = sys.argv[1]

# The capture is @2x of a 1280x820 window, so device px are logical * 2.
# Everything below is in device px; logical is reported by halving.
PANE_X0, PANE_X1 = 1620, 2540


def columns(pred, y0, height):
    out = subprocess.run(
        ["magick", shot, "-crop",
         "%dx%d+%d+%d" % (PANE_X1 - PANE_X0, height, PANE_X0, y0),
         "-depth", "8", "txt:-"],
        capture_output=True, text=True, check=True).stdout
    cols = set()
    for line in out.splitlines()[1:]:
        try:
            pos, rest = line.split(":", 1)
            x, _y = pos.split(",")
            colour = rest.split()[1]
            if pred(colour):
                cols.add(int(x) + PANE_X0)
        except Exception:
            pass
    return sorted(cols)


def is_accent(c):
    return c[:7] == "#0069CC"


def is_rail(c):
    return c[:7] == "#E3E3E6"


# The rail is the strip's bottom hairline: the longest border-coloured run.
rail = None
for y in range(100, 300):
    cols = columns(is_rail, y, 1)
    if cols and (rail is None or cols[-1] - cols[0] > rail[2]):
        rail = (cols[0], cols[-1], cols[-1] - cols[0])

if rail is None or rail[2] < 200:
    print("check-tab-indicator: FAIL - no strip rail found", file=sys.stderr)
    sys.exit(1)

# The indicator is a SOLID accent bar, so its row is almost entirely accent
# across its span. The active label's text is also accent but broken into
# glyphs, so a density test separates them without hardcoding a row. The scan
# starts below the titlebar so the toolbar's accent pill cannot match.
indicator = None
indicator_y = 0
for y in range(100, 300):
    cols = columns(is_accent, y, 1)
    if len(cols) < 100:
        continue
    span = cols[-1] - cols[0] + 1
    if span and len(cols) / span > 0.95:
        indicator = (cols[0], cols[-1])
        indicator_y = y
        break

if indicator is None:
    print("check-tab-indicator: FAIL - no indicator found in the strip", file=sys.stderr)
    sys.exit(1)

# MEASURE THE LEFT EDGE, not the width: the bug this guards against had exactly
# the right width and height.
EXPECTED = 8
TOLERANCE = 2
delta = indicator[0] - rail[0]

print("check-tab-indicator: rail left %d, indicator x=%d..%d (width %d device) at y=%d"
      % (rail[0], indicator[0], indicator[1], indicator[1] - indicator[0] + 1, indicator_y))
print("check-tab-indicator: indicator left is %d device (%.1f logical) right of the rail; "
      "expected %d (%.1f logical)"
      % (delta, delta / 2.0, EXPECTED, EXPECTED / 2.0))

if abs(delta - EXPECTED) > TOLERANCE:
    print(
        "check-tab-indicator: FAIL - the indicator's left edge is off by %.1f logical px. "
        "Check that tab bounds are rebased for the content-box origin (TAB_PL) and that "
        "the indicator's containing block is the tab row, not the padded strip."
        % ((delta - EXPECTED) / 2.0),
        file=sys.stderr,
    )
    sys.exit(1)

print("check-tab-indicator: OK")
PYEOF
