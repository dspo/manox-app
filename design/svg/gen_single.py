"""One consolidated interactive sheet — every interaction in a single file.

The earlier interactive/ set split each state across its own sheet
(a-static, b-hover-row, c-hover-toolbar, d-collapsed, e-running,
f-tab-browser, plus the g-/h- pair). That was needless: the styling is shared
and only the baked-in state differed. This sheet carries all of it at once,
driven by CSS and SMIL with no JavaScript:

  hover      session rows, icon buttons, sync pill, tabs, quick actions
  click      右栏 tab: Terminal / Browser
  click      sidebar group collapse: manox / chenzhongrun / 自定义
  animation  attention pulse, running blocks, braille spinner

Click behaviour uses one mechanism throughout: an `<a href="#anchor">` whose
anchor is a SIBLING of the `.state` elements it reveals (`#id:target ~ .state`
cannot reach out of a wrapper). The generator therefore emits, in order:

    strip (always visible)  →  anchors  →  state groups

so the tab/group links stay clickable in every state.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *
from codicon import codicon, brand, used_symbol_defs, used_symbols, reset_used
from validate import measure

M = 48.0
HELP_W = 330.0
SHEET_W = 1100.0
SHEET_H = 761.0
TOTAL_W = SHEET_W + M * 2 + HELP_W
TOTAL_H = SHEET_H + M * 2 + 96

TAB_H = 34.0
CUST_H = 26 + 30 * 2
RAIL_Y = 31.0 + TAB_H
CUST_Y = SHEET_H - CUST_H

GROUPS = [
    ("manox", "manox", [
        ("Review dspo/manox PR #824", "ba690af"),
        ("manox⇔manox-app 协议全…", "09afb8f"),
        ("在内置终端中使用 Claude Co…", "e102bdc"),
        ("一些 vs code extension 的 h…", "049a553", "running")]),
    ("chenzhongrun", "chen", [
        ("what model now ?", "5d6dea9"),
        ("what model now ?", "1bf6621"),
        ("what model now ?", "f64c637"),
        ("hi", "bb29468"),
        ("what cwd now ?", "7144964"),
        ("写一个暴风雪山庄模式的小故…", "9b3e2d8"),
        ("帮我清理磁盘：僵尸 worktree", "78ca56e", "pending")]),
]

# first row y of each group, resolved while laying out the sidebar
GROUP_ROW_Y = {}
GROUP_HEAD_Y = {}


def session_row(add, y, title, sid, status="unread"):
    add('<g class="hoverable cursor-ptr">')
    add(rect(10, y, 210, 46, fill="transparent", rx=4))
    if status == "pending":
        # SessionStatus::PendingAuth — the 1s breathing dot
        add(pulse_dot(32, y + 9, 4))
    elif status == "running":
        # SessionStatus::Running — the 540ms three-block indicator
        add(running_blocks(32, y + 9))
    else:
        add(circle(32, y + 9, 4, fill=BADGE_BLUE_BG))
    add(text(46, y + 13, title, size=12.5, fill=FG))
    add(text(32, y + 33, "· 未读", size=11.5, fill=FG_FAINT))
    chw = measure(sid, 10.5) + 12
    chx = 230 - 12 - chw
    add(rect(chx, y + 23, chw, 16, fill="none", stroke=BORDER, sw=1, rx=3))
    add(text(chx + chw / 2, y + 35, sid, size=10.5, fill=FG_FAINT, anchor="middle"))
    add('<g class="row-actions">')
    ax = 230 - 8
    for nm in ("more", "archive", "pin"):
        ax -= 16
        add(codicon(nm, ax + 6, y + 9, 11, FG_FAINT))
    add("</g>")
    add("</g>")


def emit_sidebar(add):
    """Sidebar body. Group headers/rows carry `data-group`-style classes so the
    collapse anchors can fold them; row positions are recorded for the
    collapsed repaint."""
    add(rect(0, 31, 230, SHEET_H - 31, fill=SHELL_BG))
    hy = 31
    add(text(16, hy + 26, "会话", size=13, fill=FG_STRONG, weight="600"))
    add('<g class="hov-accent cursor-ptr">')
    add(rect(92, hy + 13, 62, 22, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add("</g>")
    add(text(100, hy + 28, "新建", size=12, fill=FG))
    add(text(126, hy + 28, "⌘N", size=10, fill=FG_DIM))
    for gx, nm in ((183, "sort"), (226, "search")):
        add('<g class="hov-icon cursor-ptr">')
        add(f'<rect class="ibox" x="{gx-11:g}" y="{hy+13:g}" width="22" height="19" '
            f'rx="4" fill="transparent"/>')
        add(codicon(nm, gx, hy + 24, 14, FG_DIM))
        add("</g>")

    ry = hy + 40
    for lt, ic, badge in (("自动化", "calendar", "NEW"), ("对话", "comment", None)):
        add('<g class="hov-row cursor-ptr">')
        add(rect(0, ry - 4, 230, 34, fill="transparent"))
        add("</g>")
        add(codicon(ic, 22, ry + 11, 15, FG_DIM))
        add(text(40, ry + 15, lt, size=12.5, fill=FG))
        if badge:
            add(rect(136, ry + 4, 30, 15, fill=BADGE_BLUE_BG, rx=4))
            add(text(151, ry + 14.5, badge, size=9, fill=BADGE_BLUE_FG, anchor="middle"))
        ry += 36

    add(f'<clipPath id="sb-scroll"><rect x="0" y="{ry:g}" width="230" '
        f'height="{CUST_Y - 4 - ry:g}"/></clipPath>')
    add('<g clip-path="url(#sb-scroll)">')
    for gname, key, rows in GROUPS:
        GROUP_ROW_Y[key] = ry + 24
        GROUP_HEAD_Y[key] = ry
        add(f'<a href="#a-g-{key}" class="tab-btn">')
        add(f'<g class="grp-head g-{key} cursor-ptr">')
        add(rect(0, ry - 4, 230, 26, fill="transparent"))
        add(f'<g class="chev-open">')
        add(codicon("chevron-down", 20, ry + 10, 13, FG_DIM))
        add("</g>")
        add(f'<g class="chev-closed">')
        add(codicon("chevron-right", 20, ry + 10, 13, FG_DIM))
        add("</g>")
        add(codicon("folder", 38, ry + 10, 15, FG_DIM))
        add(text(54, ry + 14, gname, size=12.5, fill=FG, weight="600"))
        add("</g></a>")
        ry += 24
        add(f'<g class="rows-{key}">')
        for item in rows:
            title, sid = item[0], item[1]
            st = item[2] if len(item) > 2 else "unread"
            session_row(add, ry, title, sid, st)
            ry += 58
        add("</g>")
        ry += 6
    add("</g>")

    add(rect(0, CUST_Y - 4, 230, CUST_H + 4, fill=SHELL_BG))
    add('<a href="#a-g-cust" class="tab-btn">')
    add('<g class="grp-head g-cust cursor-ptr">')
    add(f'<g class="chev-open">')
    add(codicon("chevron-down", 20, CUST_Y + 10, 13, FG_DIM))
    add("</g>")
    add(f'<g class="chev-closed">')
    add(codicon("chevron-right", 20, CUST_Y + 10, 13, FG_DIM))
    add("</g>")
    add(text(36, CUST_Y + 14, "自定义", size=12.5, fill=FG, weight="600"))
    add("</g></a>")
    yy = CUST_Y + 26
    add('<g class="rows-cust">')
    for lt, ic in (("概览", "home"), ("MCP", "gear")):
        add('<g class="hov-row cursor-ptr">')
        add(rect(0, yy - 4, 230, 28, fill="transparent"))
        add("</g>")
        add(codicon(ic, 34, yy + 10, 13, FG_DIM))
        add(text(50, yy + 14, lt, size=12, fill=FG))
        yy += 30
    add("</g>")


def quick_actions(add, RX):
    items = [("codicon", "globe", "打开集成浏览器"),
             ("codicon", "terminal", "打开集成终端"),
             ("brand", "claude", "打开 Claude Code"),
             ("brand", "codex", "打开 Codex"),
             ("brand", "copilot", "打开 GitHub Copilot"),
             ("codicon", "symbol-file", "打开编辑器")]
    qy = 552.0
    for knd, ic, lt in items:
        add('<g class="hov-accent cursor-ptr">')
        add(rect(RX + 6, qy - 11, 306, 28, fill="transparent", rx=6))
        add("</g>")
        if knd == "brand":
            add(brand(ic, RX + 30 - 7.5, qy - 7.5, 15, FG))
        else:
            add(codicon(ic, RX + 30, qy, 15, FG))
        add(text(RX + 46, qy + 5, lt, size=13, fill=FG))
        qy += 34


def emit_right_strip(add, RX, RW, boxes, newx):
    """Tab strip + pane toolbar: drawn once, outside every state group, so the
    tab links remain clickable no matter which state is showing."""
    add(f'<rect x="{RX:g}" y="31" width="{RW:g}" height="{SHEET_H - 39:g}" '
        f'fill="{CARD_BG}" rx="{CARD_RADIUS}"/>')
    add(rect(RX, 31, RW, SHEET_H - 39, fill="none", stroke=CARD_BORDER, sw=1,
             rx=CARD_RADIUS))
    add(line(RX + 1, RAIL_Y, RX + RW - 1, RAIL_Y, stroke=BORDER))
    add('<g class="strip">')
    for bx, bw, ic, name in boxes:
        add(f'<a href="#a-tab-{name.lower()}" class="tab-btn t-{name.lower()}">')
        add(f'<rect x="{bx:g}" y="33" width="{bw:g}" height="{TAB_H-2:g}" rx="5" '
            f'fill="transparent" class="tab-hoverbg"/>')
        add(codicon(ic, bx + 19, 31 + TAB_H / 2, 15, FG_DIM))
        add(text(bx + 34, 31 + TAB_H / 2 + 4.2, name, size=12.5, fill=FG_DIM))
        add('<g class="tab-close">')
        add(codicon("close", bx + bw - 14, 31 + TAB_H / 2, 11, FG_FAINT))
        add("</g></a>")
    add("</g>")
    add('<g class="hov-accent cursor-ptr">')
    add(f'<rect x="{newx-4:g}" y="35" width="92" height="26" rx="5" fill="transparent"/>')
    add("</g>")
    add(codicon("add", newx + 8, 31 + TAB_H / 2, 13, FG_DIM))
    add(text(newx + 20, 31 + TAB_H / 2 + 4.2, "新标签页", size=12, fill=FG_DIM))
    by = RAIL_Y + 30
    for gx, nm in ((RX + 16, "add"), (RX + 74, "split"), (RX + 130, "external")):
        add('<g class="hov-icon cursor-ptr">')
        add(f'<rect class="ibox" x="{gx-11:g}" y="{by-9.5:g}" width="22" height="19" '
            f'rx="4" fill="transparent"/>')
        add(codicon(nm, gx, by, 15, FG))
        add("</g>")


def emit_body_terminal(add, RX):
    lines = [("~ ", FG_DIM), ("cargo run", FG), ("", FG),
             ("   Compiling agent-ui v0.1.0", FG_DIM),
             ("    Finished `dev` profile in 42.18s", OK_GREEN),
             ("", FG), ("~ ", FG_DIM), ("git status --short", FG),
             (" M AGENTS.md", WARN_ORANGE), ("?? design/svg/", FG_FAINT),
             ("", FG), ("~ ", FG_DIM)]
    ly = RAIL_Y + 105
    for txt, colr in lines:
        add(text(RX + 14, ly, txt, size=11.5, fill=colr, family=FONT_MONO))
        ly += 17
    add(braille_spinner(RX + 22, ly - 1, 11.5, FG))
    add(rect(RX + 34, ly - 10, 7, 13, fill=FG, opacity="0.7"))
    quick_actions(add, RX)


def emit_body_browser(add, RX):
    add(rect(RX + 14, RAIL_Y + 20, 240, 30, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add(text(RX + 26, RAIL_Y + 39, "https://example.com", size=12, fill=FG,
             family=FONT_MONO))
    add(rect(RX + 262, RAIL_Y + 20, 42, 30, fill=ACCENT, rx=5))
    add(text(RX + 283, RAIL_Y + 39, "Go", size=12, fill="#fff", anchor="middle",
             weight="600"))
    add(rect(RX + 14, RAIL_Y + 62, 290, 560, fill="#F7F7F9", stroke=BORDER, sw=1, rx=6))
    add(text(RX + 159, RAIL_Y + 340, "webview 子视图（wry）", size=12,
             fill=FG_FAINT, anchor="middle"))


def emit_indicator(add, boxes, dest):
    for bx, bw, ic, name in boxes:
        if name.lower() == dest:
            add(f'<rect class="ind" x="{bx:g}" y="{RAIL_Y-2:g}" width="{bw:g}" '
                f'height="2" rx="1" fill="{ACCENT}"/>')


def sheet():
    o = []
    def add(s): o.append(s)

    # ── titlebar ──────────────────────────────────────────────────────────
    add(rect(0, 0, SHEET_W, 31, fill=SHELL_BG))
    for i, c in enumerate((TRAFFIC_RED, TRAFFIC_YELLOW, TRAFFIC_GREEN)):
        add(circle(18 + i * 20, 19, 6, fill=c))
    cy = 16.5

    def ibtn(gx, name, size=15, color=FG_DIM, on=False):
        add('<g class="hov-icon cursor-ptr">')
        add(f'<rect class="ibox" x="{gx-11:g}" y="{cy-9.5:g}" width="22" height="19" '
            f'rx="4" fill="transparent"/>')
        if on:
            add(f'<rect x="{gx-7:g}" y="{cy-6:g}" width="14" height="12" rx="3" '
                f'fill="{ICON_ON_BG}"/>')
        add(codicon(name, gx, cy, size, ACCENT if on else color))
        add("</g>")

    ibtn(93, "sidebar-left", 15, FG_DIM, on=True)
    ibtn(140, "arrow-left", 14)
    ibtn(190, "arrow-right", 14)

    label = "Sync Changes"
    pillw = 12 + 13 + 6 + measure(label, 12) + 8 + 24 + 12
    cluster_w = (11 + 12) + (22 + 14) + (22 + 14) + (22 + 16) + pillw + \
                (14 + 9 + 18 + 22) + (44 + 44)
    px_, pw = 214.0, (SHEET_W - cluster_w - 16) - 214.0
    add(rect(px_, cy - 12, pw, 24, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add(codicon("folder-open", px_ + 14, cy, 13, FG_DIM))
    add(text(px_ + 27, cy + 4.4, "Manox", size=12.5, fill=FG))
    add(codicon("chevron-down", px_ + pw - 16, cy, 12, FG_DIM))

    ax = SHEET_W - 23
    add(circle(ax, cy, 11, fill="#7099D8"))
    add(codicon("account", ax, cy, 13, BADGE_BLUE_FG))
    x = ax - 47
    ibtn(x, "sidebar-right", 15); x -= 36
    ibtn(x, "panel", 15, FG_DIM, on=True); x -= 38
    pillx = x - pillw
    add('<g class="hov-pill cursor-ptr">')
    add(rect(pillx, cy - 12, pillw, 24, fill=ACCENT, rx=5))
    add(codicon("sync", pillx + 18, cy, 13, BADGE_BLUE_FG))
    add(text(pillx + 30, cy + 4.3, label, size=12, fill=BADGE_BLUE_FG))
    bx = pillx + pillw - 12 - 24
    add(rect(bx, cy - 8, 24, 16, fill="rgba(255,255,255,0.28)", rx=6))
    add(text(bx + 12, cy + 3.8, "1↑", size=10.5, fill=BADGE_BLUE_FG, anchor="middle"))
    add("</g>")
    x = pillx - 23
    add(rect(x - 9, cy - 9, 18, 18, fill=ACCENT, rx=4))
    add(codicon("code", x, cy, 12, BADGE_BLUE_FG))
    x -= 49
    ibtn(x, "split", 14); x -= 44
    ibtn(x, "play", 14)

    # ── sidebar ───────────────────────────────────────────────────────────
    emit_sidebar(add)

    # ── main card: empty MainSurface slot ─────────────────────────────────
    add(rect(231, 31, 534, SHEET_H - 39, fill=CARD_BG, stroke=CARD_BORDER,
             sw=1, rx=CARD_RADIUS))
    add(rect(231 + 267 - 96, 31 + 361 - 13, 192, 26, fill="rgba(0,0,0,0.028)", rx=6))
    add(text(231 + 267, 31 + 361 + 4.5, "MainSurface 槽（外壳不绘制内容）", size=11,
             fill=FG_FAINT, anchor="middle"))

    # ── right pane strip ──────────────────────────────────────────────────
    RX, RW = 773.0, 318.0
    tabs = [("terminal", "Terminal"), ("globe", "Browser")]
    tx = RX + 10
    boxes = []
    for ic, name in tabs:
        tw = 24 + 15 + 7 + measure(name, 12.5) + 21
        boxes.append((tx, tw, ic, name))
        tx += tw
    newx = tx + 6
    # ── anchors FIRST, then everything they skin ──────────────────────────
    # `#a:target ~ .x` only matches FOLLOWING siblings, so every anchor is
    # emitted before the strip and the state groups. The strip itself carries
    # no state so its links stay clickable in every state.
    for anc in ("a-tab-terminal", "a-tab-browser", "a-g-manox", "a-g-chen",
                "a-g-cust"):
        add(f'<a id="{anc}"></a>')

    emit_right_strip(add, RX, RW, boxes, newx)

    # ── state groups ──────────────────────────────────────────────────────
    # Terminal carries BOTH classes: it is the resting (un-targeted) state and
    # its own target. One element for both roles means returning to it cannot
    # render two copies — the earlier two-group form stacked them.
    add('<g class="state state-default" data-group="tab" data-name="terminal">')
    emit_indicator(add, boxes, "terminal")
    emit_body_terminal(add, RX)
    add("</g>")
    add('<g class="state" data-group="tab" data-name="browser">')
    emit_indicator(add, boxes, "browser")
    emit_body_browser(add, RX)
    add("</g>")

    # group-collapse states: each carries the folded variant of its group
    for gname, key, rows in GROUPS:
        add(f'<g class="state" data-group="grp" data-name="{key}">')
        # The header sits EARLIER in the document than these anchors, so no `~`
        # rule can hide it; instead this group repaints over the expanded
        # chevron with the shell colour and draws the collapsed one, then
        # blanks the group's rows the same way. A static SVG cannot reflow the
        # rows below upward, so the freed space stays empty — the fold is
        # legible without pretending the layout closed up.
        hy = GROUP_HEAD_Y[key]
        add(rect(0, hy - 4, 230, 26, fill=SHELL_BG))
        add(codicon("chevron-right", 20, hy + 10, 13, FG_DIM))
        add(codicon("folder", 38, hy + 10, 15, FG_DIM))
        add(text(54, hy + 14, gname, size=12.5, fill=FG, weight="600"))
        y0 = GROUP_ROW_Y[key]
        add(rect(0, y0, 230, len(rows) * 58, fill=SHELL_BG))
        add("</g>")
    add('<g class="state" data-group="grp" data-name="cust">')
    add(rect(0, CUST_Y + 22, 230, 60, fill=SHELL_BG))
    add(text(120, CUST_Y + 44, "（自定义已折叠）", size=11, fill=FG_FAINT,
             anchor="middle"))
    add("</g>")

    return "".join(o)


def help_panel():
    s = []
    ax = M + SHEET_W + 26
    s.append(text(ax, 30, "交互原型 · 单文件", size=13, fill=FG, weight="600"))
    s.append(text(ax, 50, "纯 SVG：CSS + SMIL，零 JavaScript", size=10.5, fill=FG_FAINT))
    y = 86
    s.append(text(ax, y, "可点击切换", size=11.5, fill=FG, weight="600")); y += 20
    for line in ("右栏页签：Terminal ⇄ Browser",
                 "侧栏分组头：折叠 / 展开"):
        s.append(circle(ax + 3, y - 4, 2.5, fill=ACCENT))
        s.append(text(ax + 14, y, line, size=11, fill=FG_DIM))
        y += 19
    y += 12
    s.append(text(ax, y, "悬停", size=11.5, fill=FG, weight="600")); y += 20
    for line in ("会话行：底色 + 浮动操作淡入", "图标按钮：整箱 #F0F0F2",
                 "Sync 药丸：转 #005AAE", "页签 / 快捷动作：底色"):
        s.append(text(ax + 8, y, "· " + line, size=10.5, fill=FG_DIM))
        y += 17
    y += 12
    s.append(text(ax, y, "自动播放", size=11.5, fill=FG, weight="600")); y += 20
    for line in ("待办呼吸点（1s）", "运行三格（540ms）", "braille spinner（8 帧）"):
        s.append(text(ax + 8, y, "· " + line, size=10.5, fill=FG_DIM))
        y += 17
    y += 14
    s.append(text(ax, y, "点击写入 URL hash，浏览器后退可回退。", size=10, fill=FG_FAINT))
    return "".join(s)


def build(fname, title):
    reset_used()
    o = ['<g transform="translate(%g,%g)">' % (M, M + 52),
         '<g filter="url(#win-shadow)">',
         rect(0, 0, SHEET_W, SHEET_H, fill=SHELL_BG, rx=10),
         "</g>", '<g clip-path="url(#win-clip)">',
         sheet(), "</g>",
         rect(0, 0, SHEET_W, SHEET_H, fill="none", stroke="rgba(0,0,0,0.18)", sw=1, rx=10),
         "</g>", help_panel()]
    defs = (used_symbol_defs(used_symbols()) + window_shadow_defs()
            .replace(f'width="{WINDOW_W}" height="{WINDOW_H}" rx="10"',
                     f'width="{SHEET_W}" height="{SHEET_H}" rx="10"')
            .replace(f'<rect x="0" y="0" width="{WINDOW_W}"',
                     f'<rect x="0" y="0" width="{SHEET_W}"')
            .replace(f'height="{WINDOW_H}" rx="10"/>\n    </clipPath>',
                     f'height="{SHEET_H}" rx="10"/>\n    </clipPath>'))
    here = os.path.dirname(os.path.abspath(__file__))
    save(os.path.join(here, "interactive", fname), defs + "".join(o),
         TOTAL_W, TOTAL_H, title, style=interaction_defs())


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    os.makedirs(os.path.join(here, "interactive"), exist_ok=True)
    build("i-interactive.svg", "manox 外壳 · 单文件交互原型")
