"""Icon sources for the shell prototypes, taken from the app's own assets.

Two families, because the app genuinely has two:

1. **codicon.ttf** (`crates/manox-agent-chrome-ui/assets/fonts/`) — the chrome
   crate's whole icon vocabulary, rendered by `theme::icon(NAME, size)`
   (`theme/icons.rs`). Glyphs are outlines on a 300-unit em with y-up.

2. **Brand SVGs** (`crates/agent-ui/assets/icons/*.svg`) — the per-tool marks
   `tool_tabs.rs` hands to `ToolTabFactory::icon`, e.g. `icons/claude.svg`.
   These are already 24x24 single-path outlines.

Neither family may be referenced from another file: an SVG `<use>` cannot pull
a fragment out of an external SVG document (browsers block cross-document
`use`, and `file://` makes it worse), and a font cannot be referenced from a
standalone SVG without embedding it. So both are *inlined* once as `<symbol>`s
in each sheet's `<defs>` and instantiated with `<use>` — which is also why the
final files carry a shared symbol catalogue.
"""
import os
import re

from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.misc.transform import Transform

_HERE = os.path.dirname(os.path.abspath(__file__))
_REPO = os.path.normpath(os.path.join(_HERE, "..", ".."))

FONT_PATH = os.path.join(
    _REPO, "crates", "manox-agent-chrome-ui", "assets", "fonts", "codicon.ttf")
ASSET_ICONS = os.path.join(_REPO, "crates", "agent-ui", "assets", "icons")

# ── 1. codicon font glyphs ─────────────────────────────────────────────────

# codepoint -> our short name (names follow theme/icons.rs constants)
CODEPOINTS = {
    0xeb99: "account", 0xea60: "add", 0xea98: "archive",
    0xea9b: "arrow-left", 0xea9c: "arrow-right",
    0xeaa4: "book", 0xeab0: "calendar", 0xeab2: "check",
    0xeab4: "chevron-down", 0xeab6: "chevron-right", 0xeab5: "chevron-left",
    0xeab7: "chevron-up", 0xea76: "close", 0xeac4: "code",
    0xeac7: "comment", 0xebcc: "copy", 0xeae6: "extensions",
    0xea83: "folder", 0xeaf7: "folder-open", 0xeaf8: "gear",
    0xec6f: "branch", 0xeb01: "globe", 0xeb06: "home",
    0xebf2: "panel", 0xebf3: "sidebar-left", 0xebf4: "sidebar-right",
    0xeb14: "external", 0xea7c: "more", 0xeb2b: "pin", 0xeb2c: "play",
    0xec20: "robot", 0xea6d: "search", 0xeb55: "sort", 0xec10: "sparkle",
    0xeb56: "split", 0xea77: "sync", 0xea85: "terminal", 0xea81: "trash",
    0xea6c: "warning", 0xebcf: "wand", 0xeb51: "settings-gear",
    0xea86: "thumbsup", 0xea95: "circle-slash", 0xeb15: "link",
    0xeb60: "symbol-file", 0xec4f: "chat-sparkle",
}

_cache = None

# Names drawn since the last reset, so a generator can emit only the <symbol>s
# its sheet actually references. Populated by codicon()/brand().
USED = set()


def reset_used():
    USED.clear()


def used_symbols():
    return set(USED)


def _load():
    global _cache
    if _cache is not None:
        return _cache
    font = TTFont(FONT_PATH)
    cmap = font.getBestCmap()
    glyphset = font.getGlyphSet()
    upm = font["head"].unitsPerEm
    bounds = {}
    for cp, name in CODEPOINTS.items():
        gname = cmap.get(cp)
        if gname is None:
            continue
        pen = SVGPathPen(glyphset)
        # flip y and normalise to a 1x1 box (viewBox 0 0 1 1), so callers can
        # scale with a single transform.
        t = Transform(1.0 / upm, 0, 0, -1.0 / upm, 0, 1.0)
        glyphset[gname].draw(TransformPen(pen, t))
        bounds[name] = pen.getCommands()
    _cache = bounds
    return bounds


def icon_path(name):
    return _load().get(name)


# ── 2. brand SVGs from the app's asset folder ──────────────────────────────
#
# `tool_tabs.rs` passes these paths to `ToolTabFactory::icon`; a brand mark is
# a solid 24x24 silhouette and must NOT be flattened to `currentColor`-only
# codicon styling, so each keeps its own viewBox and fill-rule.

# our name -> (file, the label tool_tabs.rs gives the factory)
ASSET_FILES = {
    "claude": "claude.svg",
    "codex": "codex.svg",
    "copilot": "githubcopilot.svg",
    "vscode": "vscode.svg",
    "chatgpt": "chatgpt.svg",
    "manox": "manox.svg",
}

_asset_cache = None


def _load_assets():
    """Pull the single <path> out of each brand SVG, keeping its own viewBox."""
    global _asset_cache
    if _asset_cache is not None:
        return _asset_cache
    out = {}
    for name, fname in ASSET_FILES.items():
        p = os.path.join(ASSET_ICONS, fname)
        if not os.path.exists(p):
            continue
        src = open(p, encoding="utf-8").read()
        vb = re.search(r'viewBox="([^"]+)"', src)
        paths = re.findall(r'<path\b([^>]*?)/?>', src, re.S)
        drawn = []
        for attrs in paths:
            d = re.search(r'\bd="([^"]+)"', attrs)
            if not d:
                continue
            rule = re.search(r'fill-rule="([^"]+)"', attrs)
            clip = re.search(r'clip-rule="([^"]+)"', attrs)
            drawn.append((d.group(1),
                          rule.group(1) if rule else None,
                          clip.group(1) if clip else None))
        if drawn:
            out[name] = (vb.group(1) if vb else "0 0 24 24", drawn)
    _asset_cache = out
    return out


def asset_symbol_defs():
    """<symbol>s for the brand marks, each in its source viewBox."""
    out = []
    for name, (vb, drawn) in sorted(_load_assets().items()):
        body = "".join(
            f'<path d="{d}"'
            + (f' fill-rule="{r}"' if r else "")
            + (f' clip-rule="{c}"' if c else "")
            + "/>" for d, r, c in drawn)
        out.append(f'<symbol id="brand-{name}" viewBox="{vb}" '
                   f'fill="currentColor">{body}</symbol>')
    return "".join(out)


def brand(name, x, y, size, color, opacity=None):
    """Place a brand mark with its top-left at (x, y).

    Unlike a codicon (a 300em font glyph centred in a square), a brand mark
    fills its whole 24x24 box, so callers position by top-left.
    """
    if name not in _load_assets():
        return ""
    USED.add("brand:" + name)
    op = f' opacity="{opacity}"' if opacity is not None else ""
    return (f'<use href="#brand-{name}" x="{x:g}" y="{y:g}" '
            f'width="{size:g}" height="{size:g}" fill="{color}"{op}/>')


def all_symbol_defs():
    """Every inlinable icon (codicon + brand) for a sheet's <defs>."""
    return "<defs>" + "".join(
        s for s in _symbol_defs_raw()) + "</defs>"


def used_symbol_defs(names):
    """Only the named symbols, as a <defs> block.

    Every sheet previously carried the whole catalogue (~120KB) regardless of
    what it drew, which dominated the file size: a figure using 12 glyphs paid
    for 50. `names` is an iterable of codicon names and/or `brand:<name>`
    entries; the generator collects them as it draws.
    """
    cod, brand_names = set(), set()
    for n in names:
        if n.startswith("brand:"):
            brand_names.add(n.split(":", 1)[1])
        else:
            cod.add(n)
    out = []
    glyphs = _load()
    for name in sorted(cod):
        if name in glyphs:
            out.append(f'<symbol id="ic-{name}" viewBox="0 0 1 1" '
                       f'overflow="visible"><path d="{glyphs[name]}" '
                       f'fill="currentColor"/></symbol>')
    assets = _load_assets()
    for name in sorted(brand_names):
        if name not in assets:
            continue
        vb, drawn = assets[name]
        body = "".join(
            f'<path d="{d}"'
            + (f' fill-rule="{r}"' if r else "")
            + (f' clip-rule="{c}"' if c else "")
            + "/>" for d, r, c in drawn)
        out.append(f'<symbol id="brand-{name}" viewBox="{vb}" '
                   f'fill="currentColor">{body}</symbol>')
    return "<defs>" + "".join(out) + "</defs>"


def _symbol_defs_raw():
    for name, d in sorted(_load().items()):
        yield (f'<symbol id="ic-{name}" viewBox="0 0 1 1" '
               f'overflow="visible"><path d="{d}" fill="currentColor"/></symbol>')
    for name, (vb, drawn) in sorted(_load_assets().items()):
        body = "".join(
            f'<path d="{d}"'
            + (f' fill-rule="{r}"' if r else "")
            + (f' clip-rule="{c}"' if c else "")
            + "/>" for d, r, c in drawn)
        yield (f'<symbol id="brand-{name}" viewBox="{vb}" '
               f'fill="currentColor">{body}</symbol>')


def symbol_defs():
    """All glyphs once, as <symbol>s — callers then emit compact <use> refs.

    Inlining the same 2-6KB path for every icon instance pushed a sheet past
    180KB; defining each glyph once and referencing it keeps a whole figure
    roughly the size of one glyph set.
    """
    out = []
    for name, d in sorted(_load().items()):
        out.append(f'<symbol id="ic-{name}" viewBox="0 0 1 1" '
                   f'overflow="visible"><path d="{d}" fill="currentColor"/></symbol>')
    return "<defs>" + "".join(out) + "</defs>"


def codicon(name, cx, cy, size, color, opacity=None):
    """One codicon glyph centred on (cx, cy), as a <use> of its <symbol>.

    Mirrors `theme::icon(NAME, size)`. Requires the matching <symbol> in the
    same document (see `used_symbol_defs`); `currentColor` carries the tint so
    an ancestor's `color` (or the `fill` given here) drives it.
    """
    if icon_path(name) is None:
        return ""
    USED.add(name)
    inner = size * 0.9
    tx = cx - inner / 2.0
    ty = cy - inner / 2.0
    op = f' opacity="{opacity}"' if opacity is not None else ""
    return (f'<use href="#ic-{name}" x="0" y="0" width="1" height="1" '
            f'fill="{color}"{op} '
            f'transform="translate({tx:.3f} {ty:.3f}) scale({inner:.4f})"/>')


if __name__ == "__main__":
    got = _load()
    print(f"{len(got)} glyphs extracted from codicon.ttf")
    for n in sorted(got):
        print(f"  {n:16} {len(got[n]):5d} chars")
