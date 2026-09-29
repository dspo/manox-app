"""Main window — corrected against a real screenshot of the running app.

The screenshot is a 2x capture of a 1100x761 logical window. All values below
are logical px; measured screenshot edges (÷2) are noted next to the constant
so the drawing can be re-verified against the code's own constants.

Measured from the capture:
  sidebar seam      x=460/2 = 230   (divider SIDEBAR_DEFAULT 224 + 6 pane gap)
  main card         x=462..1530 -> 231..765
  right pane        x=1546..2182 -> 773..1091
  titlebar          0..62/2   -> 0..31
  main card top     y=76/2  = 38     (titlebar height ✓)
  user turn frame   border #E9B306 (amber/warning), stroke ~2px logical
  composer sep      y=1222/2 = 611
  window bottom     y=1504/2 = 752
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *
from codicon import codicon, brand, used_symbol_defs, used_symbols, reset_used
from validate import measure

# ── window (measured 1100x761 logical) ────────────────────────────────────
WIN_W = 1100.0
WIN_H = 761.0

M = 48.0
LABEL_W = 320.0
TOTAL_W = WIN_W + M * 2 + LABEL_W
TOTAL_H = WIN_H + M * 2 + 64

TITLEBAR_H = 31.0            # screenshot: content band 16..60 /2
SB_W = 230.0                 # screenshot seam 460/2
MAIN_X = SB_W
CARD_X = MAIN_X + 1.0
CARD_W = 765.0 - CARD_X      # main card right edge 765
RIGHT_X = 773.0
RIGHT_W = 1091.0 - RIGHT_X
FLOAT_GAP = 8.0
CARD_Y = TITLEBAR_H

# The dock is collapsed in the capture (no dock present) — the main card runs
# to the bottom of the window.
CARD_H = WIN_H - CARD_Y - FLOAT_GAP
RIGHT_H = CARD_H

o = []
def add(s): o.append(s)


def titlebar():
    add(rect(0, 0, WIN_W, TITLEBAR_H, fill=SHELL_BG))
    for i, c in enumerate((TRAFFIC_RED, TRAFFIC_YELLOW, TRAFFIC_GREEN)):
        add(circle(12 + 6 + i * 20, 13 + 6, 6, fill=c))
    cy = TITLEBAR_H / 2 + 1
    # sidebar toggle sits in an accent-tinted active box right after lights;
    # then back/forward (dim, disabled-looking)
    def icon_btn(gx, name, size=15, color=FG_DIM, box=False):
        if box:
            add(rect(gx - 11, cy - 9.5, 22, 19, fill=ICON_ON_BG, rx=4))
        add(codicon(name, gx, cy, size, color))
    icon_btn(93, "sidebar-left", 15, ACCENT, box=True)
    icon_btn(140, "arrow-left", 15, FG_DIM)
    icon_btn(190, "arrow-right", 15, FG_DIM)
    # Right cluster is laid out right-to-left from the window edge, exactly as
    # the real toolbar does (avatar, right-pane toggle, panel toggle, sync
    # pill, VS logo, split, play) — laying it out left-to-right overflowed the
    # window and collided the pill's label with its own `1↑` badge. Its left
    # extent is computed FIRST so the picker can fill what remains.
    label = "Sync Changes"
    pillw = 12 + 13 + 6 + measure(label, 12) + 8 + 24 + 12
    cluster_w = (11 + 12) + (22 + 14) + (22 + 14) + (22 + 16) + pillw + \
                (14 + 9 + 18 + 22) + (44 + 44)
    cluster_left = WIN_W - cluster_w

    # session picker fills everything between the nav buttons and the cluster
    # (the real one is `flex_1`).
    px_ = 214.0
    pw = (cluster_left - 16) - px_
    add(rect(px_, cy - 12, pw, 24, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add(codicon("folder-open", px_ + 14, cy, 13, FG_DIM))
    add(text(px_ + 27, cy + 4.4, "Manox", size=12.5, fill=FG))
    add(codicon("chevron-down", px_ + pw - 16, cy, 12, FG_DIM))

    ax = WIN_W - 12 - 11          # avatar centre (11px radius, 12px inset)
    add(circle(ax, cy, 11, fill="#7099D8"))
    add(codicon("account", ax, cy, 13, BADGE_BLUE_FG))
    x = ax - 11 - 22 - 14         # right-pane toggle centre
    icon_btn(x, "sidebar-right", 15)
    x -= 22 + 14
    icon_btn(x, "panel", 15, FG_DIM, box=True)
    x -= 22 + 16
    pillx = x - pillw
    add(rect(pillx, cy - 12, pillw, 24, fill=ACCENT, rx=5))
    add(codicon("sync", pillx + 18, cy, 13, BADGE_BLUE_FG))
    add(text(pillx + 30, cy + 4.3, label, size=12, fill=BADGE_BLUE_FG))
    bx = pillx + pillw - 12 - 24
    add(rect(bx, cy - 8, 24, 16, fill="rgba(255,255,255,0.28)", rx=6))
    add(text(bx + 12, cy + 3.8, "1↑", size=10.5, fill=BADGE_BLUE_FG, anchor="middle"))
    x = pillx - 14 - 9            # VS logo block (18px)
    add(rect(x - 9, cy - 9, 18, 18, fill=ACCENT, rx=4))
    add(codicon("code", x, cy, 12, BADGE_BLUE_FG))
    x -= 9 + 18 + 22
    icon_btn(x, "split", 15); x -= 44
    icon_btn(x, "play", 15)


def sidebar():
    add(rect(0, CARD_Y, SB_W, WIN_H - CARD_Y, fill=SHELL_BG))
    pad = 10.0
    x = pad
    w = SB_W - pad * 2
    hy = CARD_Y
    # header row: 会话 | 新建 ⌘N | sort | search  (measured y 76..105 /2)
    add(text(x + 6, hy + 26, "会话", size=13, fill=FG_STRONG, weight="600"))
    bx = 92.0
    add(rect(bx, hy + 13, 62, 22, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add(text(bx + 8, hy + 28, "新建", size=12, fill=FG))
    add(text(bx + 34, hy + 28, "⌘N", size=10, fill=FG_DIM))
    add(codicon("sort", 183, hy + 24, 14, FG_DIM))
    add(codicon("search", 226, hy + 24, 14, FG_DIM))

    ry = hy + 40
    # fixed rows: 自动化 (NEW badge) / 对话   — measured 36px tall each
    for label, ic, badge in (("自动化", "calendar", "NEW"), ("对话", "comment", None)):
        add(codicon(ic, x + 12, ry + 11, 15, FG_DIM))
        add(text(x + 30, ry + 15, label, size=12.5, fill=FG))
        if badge:
            add(rect(x + 30 + 66, ry + 4, 30, 15, fill=BADGE_BLUE_BG, rx=4))
            add(text(x + 30 + 66 + 15, ry + 14.5, badge, size=9,
                     fill=BADGE_BLUE_FG, anchor="middle"))
        ry += 36

    # Customizations is a FIXED footer block in the shell (SessionList renders
    # it outside the `sessions-scroll` area), so it is pinned to the bottom and
    # the group list scrolls behind it. The scroll area is clipped: the list is
    # longer than the viewport, exactly as in the running app.
    CUST_H = 26 + 30 * 2
    cust_y = WIN_H - CUST_H
    scroll_top = ry
    scroll_bot = cust_y - 4
    add(f'<clipPath id="sb-scroll"><rect x="0" y="{scroll_top:g}" width="{SB_W:g}" '
        f'height="{max(0.0, scroll_bot - scroll_top):g}"/></clipPath>')
    add(f'<g clip-path="url(#sb-scroll)">')

    # groups and rows (row height measured 58/2 = 29? -> use 58px: see below).
    # From the capture: rows are single-line title + a `· 未读` meta line,
    # ~58 logical px apart, with a blue unread dot at the left and a short-id
    # chip at the right edge.
    groups = [
        ("manox", [
            ("Review dspo/manox PR #824", "ba690af"),
            ("manox⇔manox-app 协议全…", "09afb8f"),
            ("在内置终端中使用 Claude Co…", "e102bdc"),
            ("一些 vs code extension 的 h…", "049a553"),
        ]),
        ("chenzhongrun", [
            ("what model now ?", "5d6dea9"),
            ("what model now ?", "1bf6621"),
            ("what model now ?", "f64c637"),
            ("hi", "bb29468"),
            ("what cwd now ?", "7144964"),
            ("写一个暴风雪山庄模式的小故…", "9b3e2d8"),
            ("帮我清理磁盘：僵尸 worktree", "78ca56e"),
        ]),
    ]
    for gname, rows in groups:
        add(codicon("chevron-down", x + 10, ry + 10, 13, FG_DIM))
        add(codicon("folder", x + 28, ry + 10, 15, FG_DIM))
        add(text(x + 44, ry + 14, gname, size=12.5, fill=FG, weight="600"))
        ry += 24
        for title, sid in rows:
            row(x, ry, w, title, sid)
            ry += 58
        ry += 6
    add("</g>")

    ry = cust_y
    add(rect(0, cust_y - 4, SB_W, CUST_H + 4, fill=SHELL_BG))
    add(codicon("chevron-down", x + 10, ry + 10, 13, FG_DIM))
    add(text(x + 26, ry + 14, "自定义", size=12.5, fill=FG, weight="600"))
    ry += 26
    for label, ic in (("概览", "home"), ("MCP", "gear")):
        add(codicon(ic, 34, ry + 10, 13, FG_DIM))
        add(text(50, ry + 14, label, size=12, fill=FG))
        ry += 30


def row(x, y, w, title, sid):
    """Unread session row: blue dot + title, then `· 未读` + short-id chip."""
    add(circle(x + 22, y + 9, 4, fill=BADGE_BLUE_BG))
    tw = measure(title, 12.5)
    add(text(x + 36, y + 13, title, size=12.5, fill=FG))
    add(text(x + 22, y + 33, "· 未读", size=11.5, fill=FG_FAINT))
    # short-id chip pinned to the row's right edge
    chw = measure(sid, 10.5) + 12
    chx = SB_W - 12 - chw
    add(rect(chx, y + 23, chw, 16, fill="none", stroke=BORDER, sw=1, rx=3))
    add(text(chx + chw / 2, y + 35, sid, size=10.5, fill=FG_FAINT, anchor="middle"))
    add(circle(chx - 14, y + 31, 3, fill=BADGE_BLUE_BG))


def main_card():
    """The shell's main-area card — the frame the `MainSurface` slot injects
    into, with NO conversation content. The chat column (message list, user
    turn frame, composer) belongs to agent-ui's Workspace and is intentionally
    out of scope for a chrome-only prototype: the card is drawn as the empty
    surface the shell hands over, labelled so the omission reads as deliberate.
    """
    add(rect(CARD_X, CARD_Y, CARD_W, CARD_H, fill=CARD_BG,
             stroke=CARD_BORDER, sw=1, rx=CARD_RADIUS))
    mx, my = CARD_X + CARD_W / 2, CARD_Y + CARD_H / 2
    add(rect(mx - 96, my - 13, 192, 26, fill="rgba(0,0,0,0.028)", rx=6))
    add(text(mx, my + 4.5, "MainSurface 槽（外壳不绘制内容）", size=11,
             fill=FG_FAINT, anchor="middle"))


def right_pane():
    add(rect(RIGHT_X, CARD_Y, RIGHT_W, RIGHT_H, fill=CARD_BG,
             stroke=CARD_BORDER, sw=1, rx=CARD_RADIUS))
    # tab strip: "+ 新标签页" pill joins the body (measured y 76..138 /2)
    tab_h = 31.0
    tx = RIGHT_X + 2
    tw = 150.0
    ty = CARD_Y + 36.0 - tab_h
    add(path(f"M {tx+7:g} {ty:g} L {tx+tw-7:g} {ty:g} Q {tx+tw:g} {ty:g} "
             f"{tx+tw:g} {ty+7:g} L {tx+tw:g} {ty+tab_h:g} L {tx:g} {ty+tab_h:g} "
             f"L {tx:g} {ty+7:g} Q {tx:g} {ty:g} {tx+7:g} {ty:g} Z", fill=CARD_BG))
    add(line(tx, ty, tx, ty + tab_h, stroke=BORDER, sw=1))
    add(line(tx, ty, tx + tw, ty, stroke=BORDER, sw=1))
    add(line(tx + tw, ty, tx + tw, ty + tab_h, stroke=BORDER, sw=1))
    add(line(tx, ty + tab_h, tx + tw, ty + tab_h, stroke=CARD_BG, sw=1.5))
    add(codicon("add", tx + 21, ty + tab_h / 2, 14, FG))
    add(text(tx + 36, ty + tab_h / 2 + 4.4, "新标签页", size=13, fill=FG))
    # the strip's own bottom border across the pane
    add(line(RIGHT_X + 1, CARD_Y + 36, RIGHT_X + RIGHT_W - 1, CARD_Y + 36,
             stroke=BORDER))
    # toolbar row inside the body: + / split / external (measured y~175/2)
    by = CARD_Y + 36 + 30
    add(codicon("add", RIGHT_X + 16, by, 15, FG))
    add(codicon("split", RIGHT_X + 74, by, 15, FG))
    add(codicon("external", RIGHT_X + 130, by, 15, FG))
    # Quick actions: the new-tab page's registry-driven list, bottom-anchored
    # and right-aligned in the pane (measured centres ≈ 556/590/619/652/686/724
    # logical, ~34px apart — the last row must stay inside the card).
    # Icons follow `tool_tabs.rs::registry`: browser/terminal are codicons, the
    # three CLI agents use their brand marks from crates/agent-ui/assets/icons/,
    # the editor uses SYMBOL_FILE. Hand-drawn stand-ins were simply wrong here.
    items = [("codicon", "globe", "打开集成浏览器"),
             ("codicon", "terminal", "打开集成终端"),
             ("brand", "claude", "打开 Claude Code"),
             ("brand", "codex", "打开 Codex"),
             ("brand", "copilot", "打开 GitHub Copilot"),
             ("codicon", "symbol-file", "打开编辑器")]
    qy = 552.0
    for kind, ic, label in items:
        if kind == "brand":
            add(brand(ic, RIGHT_X + 30 - 7.5, qy - 7.5, 15, FG))
        else:
            add(codicon(ic, RIGHT_X + 30, qy, 15, FG))
        add(text(RIGHT_X + 46, qy + 5, label, size=13, fill=FG))
        qy += 34


def annotations():
    s = []
    ax = M + WIN_W + 26
    items = [
        ("工具栏 31px", "traffic-light (12,13)；侧栏开关高亮；选择器撑满中部（231–702）"),
        ("侧栏 230px", "会话/新建⌘N/排序/搜索；自动化 NEW + 对话；分组 manox、chenzhongrun"),
        ("会话行", "未读蓝点 + 标题一行 + `· 未读`；short-id chip 贴右缘（可点击复制）"),
        ("主区卡", "壳的 MainSurface 槽：卡片框架与留白；会话列内容不在本原型范围内"),
        ("右栏", "「+ 新标签页」页签；+ / 分屏 / 外开；快捷动作列表（浏览器/终端/CLI/编辑器）"),
        ("底部 dock", "截图中为折叠态，故未绘制"),
    ]
    maxw = LABEL_W - 34
    y = M + 34
    for i, (t, d) in enumerate(items, 1):
        s.append(circle(ax + 8, y - 4, 8, fill=ACCENT))
        s.append(text(ax + 8, y - 1, str(i), size=10, fill="#fff", anchor="middle"))
        s.append(text(ax + 24, y, t, size=12.5, fill=FG, weight="600"))
        lines, line = [], ""
        for seg in d.split("；"):
            cand = (line + "；" + seg) if line else seg
            if measure(cand, 11) > maxw:
                if line:
                    lines.append(line)
                line = seg
            else:
                line = cand
        if line:
            lines.append(line)
        for j, ln in enumerate(lines):
            s.append(text(ax + 24, y + 16 + j * 15, ln, size=11, fill=FG_DIM))
        y += 16 + len(lines) * 15 + 16
    return "".join(s)


reset_used()
add(f'<g transform="translate({M},{M + 52})">')
add(f'<g filter="url(#win-shadow)">')
add(rect(0, 0, WIN_W, WIN_H, fill=SHELL_BG, rx=10))
add("</g>")
add(f'<g clip-path="url(#win-clip)">')
titlebar()
sidebar()
main_card()
right_pane()
add("</g>")
add(rect(0, 0, WIN_W, WIN_H, fill="none", stroke="rgba(0,0,0,0.18)", sw=1, rx=10))
add("</g>")
add(annotations())

# window_shadow_defs uses the module-level WINDOW_W/H; rebuild for this size.
defs = (window_shadow_defs()
        .replace(f'width="{WINDOW_W}" height="{WINDOW_H}" rx="10"',
                 f'width="{WIN_W}" height="{WIN_H}" rx="10"')
        .replace(f'<rect x="0" y="0" width="{WINDOW_W}"', f'<rect x="0" y="0" width="{WIN_W}"')
        .replace(f'height="{WINDOW_H}" rx="10"/>\n    </clipPath>',
                 f'height="{WIN_H}" rx="10"/>\n    </clipPath>'))
body = used_symbol_defs(used_symbols()) + defs + "".join(o)
here = os.path.dirname(os.path.abspath(__file__))
save(os.path.join(here, "01-main-window.svg"), body, TOTAL_W, TOTAL_H,
     "manox 应用外壳（不含会话列内容）")
print(f"card {CARD_X:.0f},{CARD_Y:.0f} {CARD_W:.0f}x{CARD_H:.0f}; "
      f"right {RIGHT_X:.0f} w{RIGHT_W:.0f}; win {WIN_W}x{WIN_H}")
