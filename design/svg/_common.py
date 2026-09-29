"""Shared SVG helpers for the manox GUI prototypes.

Geometry and colour values are lifted from the ahp-client branch:
  crates/manox-agent-chrome-ui/src/theme/palette.rs   (2026-Light tokens)
  crates/manox-agent-chrome-ui/src/{shell,titlebar,session_list,right_pane,divider}.rs
  crates/manox-agent-chrome-ui/src/lib.rs             (window bounds / traffic lights)
  crates/manox-agent-chat-ui/src/views/context_rail.rs (rail geometry)
"""

# ── 2026-Light palette (palette.rs, value for value) ────────────────────────
SHELL_BG = "#FAFAFD"
CARD_BG = "#FFFFFF"
PANEL_BG = "#FAFAFD"
TABBAR_BG = "#EAEAEA"
GRADIENT_TINT = "#0069CC"

BORDER = "#E3E3E6"
CARD_BORDER = "rgba(32,32,32,0.15)"

FG = "#202020"
FG_STRONG = "#202020"
FG_DIM = "#606060"
FG_FAINT = "#8A8A8E"

ACCENT = "#0069CC"
ACCENT_HOVER = "#005AAE"
LIST_HOVER = "rgba(0,0,0,0.08)"
LIST_ACTIVE = "rgba(0,0,0,0.145)"
TOOLBAR_HOVER = "rgba(0,0,0,0.08)"
INPUT_BG = "#FFFFFF"
ICON_ON_BG = "rgba(0,105,204,0.10)"
SURFACE_TERTIARY = "#F0F0F2"
SURFACE_ACTIVE = "#E0E0E3"

OK_GREEN = "#1A7F37"
WARN_ORANGE = "#9A6700"
ERR_RED = "#CF222E"
SPINNER_BLUE = "#0069CC"

BADGE_BLUE_BG = "#0069CC"
BADGE_BLUE_FG = "#FFFFFF"

TRAFFIC_RED = "#FF5F57"
TRAFFIC_YELLOW = "#FEBC2E"
TRAFFIC_GREEN = "#28C840"

CARD_RADIUS = 8
FLOAT_GAP = 8

# ── Chrome geometry ────────────────────────────────────────────────────────
WINDOW_W = 1440.0
WINDOW_H = 900.0

TITLEBAR_H = 38.0
TRAFFIC_SLOT = 70.0          # titlebar.rs: content start
SIDEBAR_W = 224.0            # divider.rs SIDEBAR_DEFAULT
PANE_GAP = 6.0               # shell.rs sidebar|main seam
HANDLE_W = 6.0               # divider.rs HANDLE_WIDTH

RIGHT_W = 460.0              # right_pane.rs default width
RIGHT_TABBAR_H = 36.0
ROW_H = 46.0                 # session_list.rs ROW_H

DOCK_H = 220.0               # shell.rs render_panel HEIGHT
DOCK_TABBAR_H = 32.0

RAIL_W = 260.0               # context_rail.rs ENV_CARD_WIDTH
RAIL_INSET = 16.0
TITLE_BAR_HEIGHT = 34.0      # gpui_component TITLE_BAR_HEIGHT

FONT_UI = ("-apple-system, BlinkMacSystemFont, 'SF Pro Text', "
           "'PingFang SC', 'Helvetica Neue', Arial, sans-serif")
FONT_MONO = "'SF Mono', 'SFMono-Regular', Menlo, Consolas, monospace"


def esc(s):
    return (s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;"))


# ── text measurement (mirrors the platform UI face: CJK full-width) ────────
import re as _re
_CJK = _re.compile(r'[\u2e80-\u9fff\u3000-\u303f\uff00-\uffef]')


def _char_w(ch, size):
    if _CJK.match(ch):
        return size * 1.0
    if ch == ' ':
        return size * 0.28
    if ch.isdigit():
        return size * 0.56
    if ch.isupper():
        return size * 0.63
    if ch in '.,:;!|\'`iljI[]()':
        return size * 0.30
    return size * 0.53


def measure(s, size, mono=False):
    """Advance width in px. `mono` uses the fixed 0.6em advance of SF Mono,
    which is wider than my proportional estimate — measuring mono text with
    the proportional table under-sizes chips and clips their labels."""
    if mono:
        return sum(size * (1.0 if _CJK.match(c) else 0.6) for c in s)
    return sum(_char_w(c, size) for c in s)


def ellipsize(s, size, avail):
    """Single-line truncation with '…' — the `truncate()` behaviour every
    sidebar/tab label uses."""
    if measure(s, size) <= avail:
        return s
    out = ""
    for ch in s:
        if measure(out + ch + "…", size) > avail:
            break
        out += ch
    return out + "…"


def text(x, y, s, size=12.5, fill=FG, weight="400", anchor="start",
         family=None, opacity=None, letter_spacing=None):
    fam = family or FONT_UI
    extra = ""
    if opacity is not None:
        extra += f' opacity="{opacity}"'
    if letter_spacing is not None:
        extra += f' letter-spacing="{letter_spacing}"'
    return (f'<text x="{x:g}" y="{y:g}" font-family="{fam}" font-size="{size:g}" '
            f'fill="{fill}" font-weight="{weight}" text-anchor="{anchor}"{extra}>'
            f'{esc(s)}</text>')


def rect(x, y, w, h, fill="none", stroke=None, sw=1, rx=0, opacity=None):
    s = f'<rect x="{x:g}" y="{y:g}" width="{w:g}" height="{h:g}" fill="{fill}"'
    if rx:
        s += f' rx="{rx:g}"'
    if stroke:
        s += f' stroke="{stroke}" stroke-width="{sw:g}"'
    if opacity is not None:
        s += f' opacity="{opacity}"'
    return s + "/>"


def line(x1, y1, x2, y2, stroke=BORDER, sw=1, dash=None, cap=None):
    s = (f'<line x1="{x1:g}" y1="{y1:g}" x2="{x2:g}" y2="{y2:g}" '
         f'stroke="{stroke}" stroke-width="{sw:g}"')
    if dash:
        s += f' stroke-dasharray="{dash}"'
    if cap:
        s += f' stroke-linecap="{cap}"'
    return s + "/>"


def circle(cx, cy, r, fill="none", stroke=None, sw=1):
    s = f'<circle cx="{cx:g}" cy="{cy:g}" r="{r:g}" fill="{fill}"'
    if stroke:
        s += f' stroke="{stroke}" stroke-width="{sw:g}"'
    return s + "/>"


def path(d, fill="none", stroke=None, sw=1, cap="round", join="round"):
    s = f'<path d="{d}" fill="{fill}"'
    if stroke:
        s += (f' stroke="{stroke}" stroke-width="{sw:g}" '
              f'stroke-linecap="{cap}" stroke-linejoin="{join}"')
    return s + "/>"


def window_shadow_defs():
    return """<defs>
    <filter id="win-shadow" x="-20%" y="-20%" width="140%" height="150%">
      <feDropShadow dx="0" dy="10" stdDeviation="22" flood-color="#000" flood-opacity="0.18"/>
    </filter>
    <filter id="card-shadow" x="-30%" y="-30%" width="160%" height="180%">
      <feDropShadow dx="-3" dy="6" stdDeviation="5" flood-color="#000" flood-opacity="0.22"/>
    </filter>
    <filter id="soft-shadow" x="-20%" y="-20%" width="140%" height="160%">
      <feDropShadow dx="0" dy="4" stdDeviation="8" flood-color="#000" flood-opacity="0.12"/>
    </filter>
    <clipPath id="win-clip">
      <rect x="0" y="0" width="{w}" height="{h}" rx="10"/>
    </clipPath>
  </defs>""".replace("{w}", str(WINDOW_W)).replace("{h}", str(WINDOW_H))


def header(title, subtitle, height=64):
    """Figure title block above the window."""
    return (text(0, 26, title, size=20, fill="#111", weight="600")
            + text(0, 46, subtitle, size=12.5, fill="#666"))


def doc_open(w, h, title):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{w:g}" height="{h:g}" '
            f'viewBox="0 0 {w:g} {h:g}" role="img" aria-label="{esc(title)}">')


# ── interaction layer (pure SVG: CSS + SMIL, no JS) ────────────────────────
#
# Every rule below mirrors a real `hover(...)` / `with_animation(...)` in the
# chrome crate — see INTERACTION-SPEC for the file:line of each. Colour values
# come from palette.rs, so a hover wash here is the same wash the app paints:
#   LIST_HOVER        rgba(0,0,0,0.08)   session rows, tabs, menu rows
#   TOOLBAR_HOVER     rgba(0,0,0,0.08)   small icon buttons
#   SURFACE_TERTIARY  #F0F0F2            flat icon buttons (whole-box hover)
#   ACCENT_HOVER      #005AAE            accent pills
#   ICON_ON_BG        rgba(0,105,204,.10) active icon underlay

INTERACTION_CSS = """
    .hov-row       { transition: fill .12s ease; }
    .hov-row:hover { fill: rgba(0,0,0,0.08); }

    .hov-icon      { transition: fill .12s ease; }
    .hov-icon:hover .ibox { fill: #F0F0F2; }

    .hov-icon:hover .iunder { fill: rgba(0,0,0,0.06); }
    .hov-icon .iunder { fill: transparent; transition: fill .12s ease; }

    /* The pill's own <rect> carries fill="#0069CC", and a presentation
       attribute beats an inherited value — so the hover must target the rect
       itself, not the wrapping group. */
    .hov-pill rect:first-child { transition: fill .12s ease; }
    .hov-pill:hover rect:first-child { fill: #005AAE; }

    .hov-accent    { transition: fill .12s ease; }
    .hov-accent:hover { fill: rgba(0,0,0,0.05); }

    /* Rows reveal their trailing actions on hover, exactly as
       `session_list.rs` mounts pin/archive/menu only on the selected or
       hovered row. */
    .row-actions   { opacity: 0; transition: opacity .12s ease; }
    .hoverable:hover .row-actions { opacity: 1; }

    /* Tab pills: the inactive pill washes on hover; the active one is fixed. */
    .tab-pill      { transition: fill .12s ease; }
    .tab-pill:hover { fill: rgba(0,0,0,0.08); }

    /* Group headers toggle their row list. `:target` swaps which state is
       shown, so a click is a real state change with no scripting. */
    .grp-all       { display: inline; }
    .grp-none      { display: none; }
    :target .grp-all  { display: none; }
    :target .grp-none { display: inline; }

    .cursor-ptr { cursor: pointer; }

    /* ── underline tab strip (design proposal) ──────────────────────────
       A tab is an <a href="#anchor">, so the label is clickable and keyboard
       focusable, and the anchor id is what :target matches. */
    .tab-btn { text-decoration: none; cursor: pointer; }
    .tab-hoverbg { transition: fill .12s ease; }
    .tab-btn:hover .tab-hoverbg { fill: rgba(0,0,0,0.06); }

    /* The close affordance shows on hover or keyboard focus, mirroring the
       reference's `.tab-btn:hover .close-btn { opacity: 1 }`. */
    .tab-close { opacity: 0; transition: opacity .12s ease; }
    .tab-btn:hover .tab-close,
    .tab-btn:focus .tab-close { opacity: 1; }

    .ind { transition: all .25s cubic-bezier(.4,0,.2,1); }

    /* Tab state switching, CSS-only. The anchors must be SIBLINGS of the
       switching groups (a `:target` nested in a wrapper cannot reach out of
       it), so the generator emits the anchors immediately before them.
       Only the INDICATOR and the BODY switch — the strip is drawn once above,
       so every tab link stays clickable in every state. Duplicating the strip
       per state made the hidden copies render as extra "active" labels and
       left the visible one unreachable after a switch. */
    .u-state { display: none; }
    .u-default { display: inline; }
    #u-terminal:target ~ .u-default,
    #u-browser:target  ~ .u-default { display: none; }
    #u-terminal:target ~ .u-terminal { display: inline; }
    #u-browser:target  ~ .u-browser  { display: inline; }

    /* The active tab's tint follows the same anchors. A presentation
       attribute (`fill="#606060"`) is weaker than any CSS declaration, so
       these rules win without needing !important. */
    #u-terminal:target ~ .u-strip .t-browser text,
    #u-browser:target  ~ .u-strip .t-terminal text { fill: #606060; font-weight: 400; }
    #u-terminal:target ~ .u-strip .t-terminal text,
    #u-browser:target  ~ .u-strip .t-browser text { fill: #0069CC; font-weight: 600; }
    #u-terminal:target ~ .u-strip .t-browser use,
    #u-browser:target  ~ .u-strip .t-terminal use { fill: #606060; }
    #u-terminal:target ~ .u-strip .t-terminal use,
    #u-browser:target  ~ .u-strip .t-browser use { fill: #0069CC; }
"""


def interaction_defs():
    """Shared <style> block for the interactive sheets."""
    return f'<style type="text/css"><![CDATA[{INTERACTION_CSS}]]></style>'


def pulse_dot(cx, cy, r=3.5, color=BADGE_BLUE_BG, dur=1.0):
    """`attention_pulse` (session_list.rs): a dot breathing on a 1s loop,
    alpha 0.35 → 1.0. SMIL stands in for the gpui animation."""
    return (f'<circle cx="{cx:g}" cy="{cy:g}" r="{r:g}" fill="{color}">'
            f'<animate attributeName="opacity" values="1;0.35;1" '
            f'dur="{dur:g}s" repeatCount="indefinite"/></circle>')


def running_blocks(cx, cy, color=ACCENT, dur=0.54):
    """`running_blocks` (session_list.rs): three 5px squares light top-down,
    brighter the closer to the current block, 540ms cycle."""
    out = []
    for i in range(3):
        y = cy - 8 + i * 6
        # stagger each block by a third of the cycle
        begin = -(2 - i) * (dur / 3.0)
        out.append(
            f'<rect x="{cx-2.5:g}" y="{y:g}" width="5" height="5" rx="1.5" '
            f'fill="{color}" opacity="0.35">'
            f'<animate attributeName="opacity" values="1;0.35;0.2;0.35;1" '
            f'dur="{dur:g}s" begin="{begin:.3f}s" repeatCount="indefinite"/>'
            f'</rect>')
    return "".join(out)


def braille_spinner(cx, cy, size=12, color=ACCENT):
    """`BrailleSpinner`: the 8-frame braille cycle the app shows while a turn
    or tool is running.

    SMIL cannot animate `textContent`, so all eight frames are stacked and
    their `visibility` cycles on one shared 0.8s clock. Each frame's `values`
    list has exactly `n` equal time-slices with `calcMode="discrete"`, so
    precisely one frame is visible in every slice and the cycle never blanks.
    (A list of the wrong length distributes over the period differently and
    leaves a moment with no glyph — the bug this replaced.)
    """
    frames = "⠋⠙⠹⠸⠼⠴⠦⠧"
    n = len(frames)
    out = []
    for i, ch in enumerate(frames):
        vals = ";".join("visible" if j == i else "hidden" for j in range(n))
        out.append(
            f'<text x="{cx:g}" y="{cy:g}" font-size="{size:g}" fill="{color}" '
            f'font-family="{FONT_MONO}" text-anchor="middle" '
            f'visibility="{"visible" if i == 0 else "hidden"}">{ch}'
            f'<animate attributeName="visibility" values="{vals}" '
            f'dur="0.8s" begin="0s" repeatCount="indefinite" calcMode="discrete"/>'
            f'</text>')
    return "".join(out)


def save(name, body, w, h, title, style=None):
    """Write an SVG. `style` injects the interaction <style> block."""
    defs = style or ""
    out = doc_open(w, h, title) + "\n" + defs + body + "\n</svg>\n"
    with open(name, "w") as f:
        f.write(out)
    print(f"wrote {name} ({len(out)} bytes)")
