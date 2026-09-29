"""Region-aware overflow check for the prototype SVGs.

`validate.py` catches text running off the page. This script is stricter: it
declares the real card/column rectangles for each figure (taken from the same
constants the generators use) and reports any text that crosses the right edge
of the region it belongs to.

Usage: python3 validate_regions.py
"""
import re
import sys
from validate import walk, measure, num

# Region rects per file: (name, x, y, w, h). Absolute SVG coordinates.
# Regions are declared in *window-local* coordinates; TX/TY shift them into
# the page space the walker reports (the figure's outer translate).
TX, TY = 48.0, 100.0
REGIONS = {
    '00-index.svg': [('card', 40, 96, 1120, 624)],
    '01-main-window.svg': [
        ('titlebar', TX + 0, TY + 0, 1100, 31),
        ('sidebar', TX + 0, TY + 31, 230, 730),
        ('main-card', TX + 231, TY + 31, 534, 722),
        ('right-pane', TX + 773, TY + 31, 318, 722),
        ('gutter', TX + 1100 + 26, 0, 320, 1000),
    ],
    '02-hero-empty.svg': [('card', 40, 96, 900, 560)],
    '03-ask-drawer.svg': [('card', 40, 96, 820, 700)],
    '04-plan-review.svg': [('card', 40, 96, 820, 660)],
    '05-settings.svg': [('card', 40, 96, 940, 620)],
    '06-session-picker.svg': [('page', 40, 96, 860, 500)],
}

# Text that is intentionally wider than a region (labels spanning columns).
ALLOW = {
    ('01-main-window.svg', 'gutter'),   # annotation prose is page-bound only
}


def main():
    files = sys.argv[1:] or list(REGIONS)
    bad = 0
    for f in files:
        regions = REGIONS[f]
        issues = []
        for tag, ox, oy, el, p in walk(__import__('xml.etree.ElementTree',
                                                  fromlist=['x']).parse(f).getroot()):
            if tag != 'text':
                continue
            s = "".join(el.itertext())
            if not s.strip():
                continue
            size = num(el.get('font-size')) or 12.5
            x = num(el.get('x')) + ox
            y = num(el.get('y')) + oy
            anchor = el.get('text-anchor', 'start')
            tw = measure(s, size)
            x0 = x - tw / 2 if anchor == 'middle' else (x - tw if anchor == 'end' else x)
            x1 = x0 + tw
            # find the smallest region containing the text's start point
            cands = [r for r in regions
                     if r[1] - 4 <= x <= r[1] + r[3] + 4 and r[2] - 6 <= y <= r[2] + r[4] + 6]
            if not cands:
                continue
            name, rx, ry, rw, rh = min(cands, key=lambda r: r[3] * r[4])
            if (f, name) in ALLOW:
                continue
            over = x1 - (rx + rw)
            if over > 2:
                issues.append(f"[{name}] {s[:44]!r} overflows right edge by "
                              f"{over:.0f}px (ends {x1:.0f}, edge {rx+rw:.0f})")
            under = rx - x0
            if under > 2:
                issues.append(f"[{name}] {s[:44]!r} starts {-under:.0f}px left of edge")
        print(f"{f}: {len(issues)} region overflow(s)")
        for i in issues:
            print("   ", i)
        bad += len(issues)
    print(f"\ntotal region overflows: {bad}")
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
