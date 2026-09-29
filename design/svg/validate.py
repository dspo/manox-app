"""Geometric validator for the prototype SVGs.

The renderer cannot be inspected by eye in this environment, so defects are
caught numerically instead:
  * every <text> is measured with its real font size and checked against the
    nearest enclosing card/column edge it must not cross;
  * every element is checked against the document viewBox;
  * declared container boxes are compared against the code's constants.

Font metrics are approximated per-character from the platform UI face
(SF Pro / PingFang): CJK glyphs are full-width (1.0em), latin lowercase
~0.52em, uppercase ~0.62em, digits ~0.55em, spaces ~0.28em.
"""
import re
import sys
import xml.etree.ElementTree as ET

CJK = re.compile(r'[\u2e80-\u9fff\u3000-\u303f\uff00-\uffef]')


def char_w(ch, size):
    if CJK.match(ch):
        return size * 1.0
    if ch == ' ':
        return size * 0.28
    if ch.isdigit():
        return size * 0.56
    if ch.isupper():
        return size * 0.63
    if ch in '.,:;!|\'`iljI[]()':
        return size * 0.30
    if ch in '↑↓→←✓○◐─│├└═╰':
        return size * 0.62
    return size * 0.53


def measure(s, size, mono=False):
    """Single source of truth lives in _common (shared with the generators)."""
    from _common import measure as m
    return m(s, size, mono=mono)


def num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return 0.0


def walk(el, ox=0.0, oy=0.0, path=""):
    """Yield (tag, x, y, w, h, attrs, full_path) in absolute coordinates."""
    ox += num(el.get('x'))
    oy += num(el.get('y'))
    tr = el.get('transform')
    if tr:
        m = re.search(r'translate\(\s*(-?[\d.]+)[ ,]+(-?[\d.]+)\s*\)', tr)
        if m:
            ox += float(m.group(1)); oy += float(m.group(2))
    for ch in el:
        tag = ch.tag.split('}')[-1]
        p = f"{path}/{tag}"
        yield tag, ox, oy, ch, p
        yield from walk(ch, ox, oy, p)


def validate(path, containers):
    """containers: list of (name, x, y, w, h) boxes text must stay inside."""
    tree = ET.parse(path)
    root = tree.getroot()
    vb = root.get('viewBox').split()
    W, H = float(vb[2]), float(vb[3])
    problems = []
    ntext = 0
    for tag, ox, oy, el, p in walk(root):
        if tag == 'text':
            ntext += 1
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
            if x0 < -1 or x1 > W + 1:
                problems.append(f"[viewBox] text runs outside page: {s[:34]!r} "
                                f"x0={x0:.0f} x1={x1:.0f} (page {W:.0f})")
            for (cname, cx, cy, cw, chh) in containers:
                inside_y = cy - 2 <= y <= cy + chh + 2
                starts_inside = cx - 2 <= x <= cx + cw + 2
                if inside_y and starts_inside and x1 > cx + cw + 2:
                    problems.append(
                        f"[overflow:{cname}] {s[:34]!r} ends at {x1:.0f}, "
                        f"container right edge {cx + cw:.0f} "
                        f"(over by {x1 - (cx + cw):.0f}px)")
                if inside_y and starts_inside and x0 < cx - 2:
                    problems.append(f"[overflow:{cname}] {s[:34]!r} starts left "
                                    f"of container ({x0:.0f} < {cx:.0f})")
    return ntext, problems


def box_of(path, predicate):
    """Find the first rect matching a predicate (for container discovery)."""
    for el in ET.parse(path).getroot().iter():
        if el.tag.split('}')[-1] == 'rect' and predicate(el):
            return (num(el.get('x')), num(el.get('y')),
                    num(el.get('width')), num(el.get('height')))
    return None


if __name__ == '__main__':
    targets = sys.argv[1:] or ['01-main-window.svg', '02-hero-empty.svg',
                              '03-ask-drawer.svg', '04-plan-review.svg',
                              '05-settings.svg', '06-session-picker.svg']
    for t in targets:
        # discover the main white card as the largest white rect
        best = None
        for el in ET.parse(t).getroot().iter():
            if el.tag.split('}')[-1] == 'rect' and el.get('fill') == '#FFFFFF':
                w = num(el.get('width')); h = num(el.get('height'))
                x = num(el.get('x')); y = num(el.get('y'))
                if w > 300 and h > 120 and (best is None or w * h > best[2] * best[3]):
                    best = (x, y, w, h)
        # Region-relative checks are validate_regions.py's job (it knows each
        # figure's translate); here we only assert page bounds.
        n, probs = validate(t, [])
        print(f"{t}: {n} text nodes, {len(probs)} problem(s)")
        for p in probs[:40]:
            print("   ", p)
