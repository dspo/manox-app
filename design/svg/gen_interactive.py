"""Interactive shell — pure SVG (CSS + SMIL, zero JavaScript).

Everything here is declarative SVG: `:hover` for the hover washes the app
paints, `:target` for the one click-toggle (sidebar group collapse), and SMIL
`<animate>` for the two animations the chrome crate really runs. No <script>,
so the file stays usable as an <img> and in GitHub's preview.

Interaction inventory, each traced to its source:
  sidebar row hover       session_list.rs:654  .hover(LIST_HOVER)
  sidebar group hover     session_list.rs:352  .hover(LIST_HOVER)
  new/sort/search hover   session_list.rs:227, primitives.rs:56 (SURFACE_TERTIARY)
  toolbar icon hover      primitives.rs:56     whole-box SURFACE_TERTIARY
  sync pill hover         titlebar.rs          ACCENT -> ACCENT_HOVER
  right tab pill hover    right_pane.rs:638    .hover(LIST_HOVER)
  quick action hover      right_pane.rs:708    .hover(LIST_HOVER)
  picker row hover        titlebar.rs:359      .hover(LIST_HOVER)
  attention pulse         session_list.rs:689  1s loop, alpha .35 -> 1
  running blocks          session_list.rs:712  540ms loop, 3 squares
  group collapse          session_list.rs:347  toggle_group (click)
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *
from codicon import codicon, brand, used_symbol_defs, used_symbols, reset_used
from validate import measure

M = 48.0
HELP_W = 300.0
STEP = 190.0

FIGS = [
    ("a-static.svg", "① 静止态（默认）", "无 hover、无展开：交付基线"),
    ("b-hover-row.svg", "② 会话行 hover", "行底色 LIST_HOVER + 浮动 pin/archive/menu 显现"),
    ("c-hover-toolbar.svg", "③ 工具栏 hover", "图标按钮整箱 SURFACE_TERTIARY；Sync 药丸转 ACCENT_HOVER"),
    ("d-collapsed.svg", "④ 分组折叠（:target）", "点 manox 组头 → 该组行收起（真实 toggle_group 语义）"),
    ("e-running.svg", "⑤ 运行态动画", "呼吸圆点 + 三格跳动 + braille spinner"),
    ("f-tab-browser.svg", "⑥ 页签切换", "Browser 页签激活，终端页签转非激活"),
]

SHEET_W = 1100.0
SHEET_H = 761.0
TOTAL_W = SHEET_W + M * 2 + HELP_W
TOTAL_H = SHEET_H + M * 2 + 96


def sheet(kind):
    """Draw the shell, varying the state according to `kind`."""
    o = []
    def add(s): o.append(s)

    # ── titlebar ──────────────────────────────────────────────────────────
    add(rect(0, 0, SHEET_W, 31, fill=SHELL_BG))
    for i, c in enumerate((TRAFFIC_RED, TRAFFIC_YELLOW, TRAFFIC_GREEN)):
        add(circle(18 + i * 20, 19, 6, fill=c))
    cy = 16.5

    def ibtn(gx, name, size=15, color=FG_DIM, active=False):
        """icon_button: outer box hover SURFACE_TERTIARY, inner underlay when on."""
        add(f'<g class="hov-icon cursor-ptr">')
        add(f'<rect class="ibox" x="{gx-11:g}" y="{cy-9.5:g}" width="22" height="19" '
            f'rx="4" fill="transparent"/>')
        if active:
            add(f'<rect x="{gx-7:g}" y="{cy-6:g}" width="14" height="12" rx="3" '
                f'fill="{ICON_ON_BG}"/>')
        add(codicon(name, gx, cy, size, ACCENT if active else color))
        add("</g>")

    ibtn(93, "sidebar-left", 15, FG_DIM, active=True)
    ibtn(140, "arrow-left", 14)
    ibtn(190, "arrow-right", 14)

    # right cluster (right-to-left), so the picker can fill the rest
    label = "Sync Changes"
    pillw = 12 + 13 + 6 + measure(label, 12) + 8 + 24 + 12
    cluster_w = (11 + 12) + (22 + 14) + (22 + 14) + (22 + 16) + pillw + \
                (14 + 9 + 18 + 22) + (44 + 44)
    cluster_left = SHEET_W - cluster_w

    px_ = 214.0
    pw = (cluster_left - 16) - px_
    add(rect(px_, cy - 12, pw, 24, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add(codicon("folder-open", px_ + 14, cy, 13, FG_DIM))
    add(text(px_ + 27, cy + 4.4, "Manox", size=12.5, fill=FG))
    add(codicon("chevron-down", px_ + pw - 16, cy, 12, FG_DIM))

    ax = SHEET_W - 23
    add(circle(ax, cy, 11, fill="#7099D8"))
    add(codicon("account", ax, cy, 13, BADGE_BLUE_FG))
    x = ax - 47
    ibtn(x, "sidebar-right", 15)
    x -= 36
    ibtn(x, "panel", 15)
    x -= 38
    pillx = x - pillw
    add(f'<g class="hov-pill cursor-ptr">')
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
    add(rect(0, 31, 230, SHEET_H - 31, fill=SHELL_BG))
    hy = 31
    add(text(16, hy + 26, "会话", size=13, fill=FG_STRONG, weight="600"))
    add(f'<g class="hov-accent cursor-ptr">')
    add(rect(92, hy + 13, 62, 22, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
    add("</g>")
    add(text(100, hy + 28, "新建", size=12, fill=FG))
    add(text(126, hy + 28, "⌘N", size=10, fill=FG_DIM))
    add(f'<g class="hov-icon cursor-ptr">')
    add(f'<rect class="ibox" x="172" y="{hy+13:g}" width="22" height="19" rx="4" '
        f'fill="transparent"/>')
    add(codicon("sort", 183, hy + 24, 14, FG_DIM))
    add("</g>")
    add(f'<g class="hov-icon cursor-ptr">')
    add(f'<rect class="ibox" x="215" y="{hy+13:g}" width="22" height="19" rx="4" '
        f'fill="transparent"/>')
    add(codicon("search", 226, hy + 24, 14, FG_DIM))
    add("</g>")

    ry = hy + 40
    for lt, ic, badge in (("自动化", "calendar", "NEW"), ("对话", "comment", None)):
        add(f'<g class="hov-row cursor-ptr">')
        add(rect(0, ry - 4, 230, 34, fill="transparent"))
        add("</g>")
        add(codicon(ic, 22, ry + 11, 15, FG_DIM))
        add(text(40, ry + 15, lt, size=12.5, fill=FG))
        if badge:
            add(rect(136, ry + 4, 30, 15, fill=BADGE_BLUE_BG, rx=4))
            add(text(151, ry + 14.5, badge, size=9, fill=BADGE_BLUE_FG, anchor="middle"))
        ry += 36

    CUST_H = 26 + 30 * 2
    cust_y = SHEET_H - CUST_H
    scroll_top, scroll_bot = ry, cust_y - 4
    add(f'<clipPath id="sb-scroll"><rect x="0" y="{scroll_top:g}" width="230" '
        f'height="{scroll_bot - scroll_top:g}"/></clipPath>')
    add('<g clip-path="url(#sb-scroll)">')

    groups = [
        ("manox", "g-manox", True, [
            ("Review dspo/manox PR #824", "ba690af", "unread"),
            ("manox⇔manox-app 协议全…", "09afb8f", "unread"),
            ("在内置终端中使用 Claude Co…", "e102bdc", "unread"),
            ("一些 vs code extension 的 h…", "049a553", "unread"),
        ]),
        ("chenzhongrun", "g-chen", True, [
            ("what model now ?", "5d6dea9", "unread"),
            ("what model now ?", "1bf6621", "unread"),
            ("what model now ?", "f64c637", "unread"),
            ("hi", "bb29468", "unread"),
            ("what cwd now ?", "7144964", "unread"),
            ("写一个暴风雪山庄模式的小故…", "9b3e2d8",
             "running" if kind == "running" else "unread"),
            # A parked ask/plan raises SessionStatus::PendingAuth, whose marker
            # is the 1s breathing dot (`attention_pulse`).
            ("帮我清理磁盘：僵尸 worktree", "78ca56e",
             "pending" if kind == "running" else "unread"),
        ]),
    ]
    for gname, gid, expanded, rows in groups:
        collapsed = (kind == "collapsed" and gname == "manox")
        add(f'<g class="hov-row cursor-ptr">')
        add(rect(0, ry - 4, 230, 26, fill="transparent"))
        add("</g>")
        add(codicon("chevron-right" if collapsed else "chevron-down", 20, ry + 10, 13, FG_DIM))
        add(codicon("folder", 38, ry + 10, 15, FG_DIM))
        add(text(54, ry + 14, gname, size=12.5, fill=FG, weight="600"))
        ry += 24
        if not collapsed:
            for title, sid, status in rows:
                row(add, ry, title, sid, status)
                ry += 58
        ry += 6
    add("</g>")

    # pinned Customizations block
    add(rect(0, cust_y - 4, 230, CUST_H + 4, fill=SHELL_BG))
    add(codicon("chevron-down", 20, cust_y + 10, 13, FG_DIM))
    add(text(36, cust_y + 14, "自定义", size=12.5, fill=FG, weight="600"))
    yy = cust_y + 26
    for lt, ic in (("概览", "home"), ("MCP", "gear")):
        add(f'<g class="hov-row cursor-ptr">')
        add(rect(0, yy - 4, 230, 28, fill="transparent"))
        add("</g>")
        add(codicon(ic, 34, yy + 10, 13, FG_DIM))
        add(text(50, yy + 14, lt, size=12, fill=FG))
        yy += 30

    # ── main card: the empty MainSurface slot (no conversation content) ────
    add(rect(231, 31, 534, SHEET_H - 39, fill=CARD_BG, stroke=CARD_BORDER,
             sw=1, rx=CARD_RADIUS))
    mx, my = 231 + 267, 31 + 361
    add(rect(mx - 96, my - 13, 192, 26, fill="rgba(0,0,0,0.028)", rx=6))
    add(text(mx, my + 4.5, "MainSurface 槽（外壳不绘制内容）", size=11,
             fill=FG_FAINT, anchor="middle"))

    # ── right pane ────────────────────────────────────────────────────────
    add(rect(773, 31, 318, SHEET_H - 39, fill=CARD_BG, stroke=CARD_BORDER,
             sw=1, rx=CARD_RADIUS))
    tab_h = 31.0
    tabs = [("terminal", "Terminal"), ("globe", "Browser")]
    tx = 775.0
    active_tab = 1 if kind == "tab-browser" else 0
    for i, (ic, name) in enumerate(tabs):
        tw = 10 + 15 + 6 + measure(name, 12) + 8 + 14 + 10
        act = (i == active_tab)
        # The strip's floor is CARD_Y + RIGHT_TABBAR_H; a tab's top is that
        # minus its own height (31 active / 28 inactive). Omitting CARD_Y drew
        # the active tab over the titlebar, where it swallowed the pill hover.
        floor = 31.0 + 36.0
        ty = floor - (tab_h if act else 28)
        if act:
            add(path(f"M {tx+7:g} {ty:g} L {tx+tw-7:g} {ty:g} Q {tx+tw:g} {ty:g} "
                     f"{tx+tw:g} {ty+7:g} L {tx+tw:g} {ty+tab_h:g} L {tx:g} "
                     f"{ty+tab_h:g} L {tx:g} {ty+7:g} Q {tx:g} {ty:g} {tx+7:g} {ty:g} Z",
                     fill=CARD_BG))
            add(line(tx, ty, tx, ty + tab_h, stroke=BORDER))
            add(line(tx, ty, tx + tw, ty, stroke=BORDER))
            add(line(tx + tw, ty, tx + tw, ty + tab_h, stroke=BORDER))
            add(line(tx, ty + tab_h, tx + tw, ty + tab_h, stroke=CARD_BG, sw=1.5))
        else:
            add(f'<g class="tab-pill cursor-ptr">')
            add(rect(tx, ty, tw, 28, fill="transparent", rx=5))
            add("</g>")
        add(codicon(ic, tx + 17, ty + (tab_h if act else 28) / 2, 15,
                  FG_STRONG if act else FG_DIM))
        add(text(tx + 31, ty + (tab_h if act else 28) / 2 + 4.2, name, size=12,
                 fill=FG_STRONG if act else FG_DIM))
        add(codicon("close", tx + tw - 14, ty + (tab_h if act else 28) / 2, 10, FG_FAINT))
        tx += tw + 2
    # new-tab pill (same floor as the tabs)
    add(codicon("add", tx + 16, 31 + 21, 12, FG_DIM))
    add(text(tx + 28, 31 + 25, "New Tab", size=12, fill=FG_DIM))
    add(line(774, 67, 1090, 67, stroke=BORDER))

    by = 31 + 66
    for gx, nm in ((789, "add"), (847, "split"), (903, "external")):
        add(f'<g class="hov-icon cursor-ptr">')
        add(f'<rect class="ibox" x="{gx-11:g}" y="{by-9.5:g}" width="22" height="19" '
            f'rx="4" fill="transparent"/>')
        add(codicon(nm, gx, by, 15, FG))
        add("</g>")

    if active_tab == 0:
        # terminal tab body
        lines = [
            ("~ ", FG_DIM), ("cargo run", FG), ("", FG),
            ("   Compiling agent-ui v0.1.0", FG_DIM),
            ("    Finished `dev` profile in 42.18s", OK_GREEN),
            ("", FG), ("~ ", FG_DIM), ("git status --short", FG),
            (" M AGENTS.md", WARN_ORANGE), ("?? design/svg/", FG_FAINT),
            ("", FG), ("~ ", FG_DIM),
        ]
        ly = 140.0
        for txt, col in lines:
            add(text(787, ly, txt, size=11.5, fill=col, family=FONT_MONO))
            ly += 17
        # live cursor + a running tool -> braille spinner (real animation)
        if kind == "running":
            add(text(787, ly, "~ ", size=11.5, fill=FG_DIM, family=FONT_MONO))
            add(braille_spinner(800, ly, 11.5, FG))
            add(rect(811, ly - 10, 7, 13, fill=FG, opacity="0.7"))
        else:
            add(text(787, ly, "~ ", size=11.5, fill=FG_DIM, family=FONT_MONO))
            add(rect(800, ly - 10, 7, 13, fill=FG, opacity="0.7"))
        qy = 552.0
        # Registry order and icons exactly as `tool_tabs.rs::registry`: browser
        # and terminal use codicon glyphs, the three CLI agents use their brand
        # marks from `crates/agent-ui/assets/icons/`, the editor uses
        # SYMBOL_FILE. The quick-action label is the factory's `quick_action()`.
        quick = [
            ("codicon", "globe", "打开集成浏览器"),
            ("codicon", "terminal", "打开集成终端"),
            ("brand", "claude", "打开 Claude Code"),
            ("brand", "codex", "打开 Codex"),
            ("brand", "copilot", "打开 GitHub Copilot"),
            ("codicon", "symbol-file", "打开编辑器"),
        ]
        for kind, ic, lt in quick:
            add(f'<g class="hov-accent cursor-ptr">')
            add(rect(779, qy - 11, 308, 28, fill="transparent", rx=6))
            add("</g>")
            if kind == "brand":
                # brand marks fill a square box, so they sit on a glyph centre
                add(brand(ic, 803 - 7.5, qy - 7.5, 15, FG))
            else:
                add(codicon(ic, 803, qy, 15, FG))
            add(text(819, qy + 5, lt, size=13, fill=FG))
            qy += 34
    else:
        # browser tab body: address bar + page placeholder
        add(rect(783, 85, 240, 30, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
        add(text(795, 104, "https://example.com", size=12, fill=FG, family=FONT_MONO))
        add(rect(1031, 85, 48, 30, fill=ACCENT, rx=5))
        add(text(1055, 104, "Go", size=12, fill="#fff", anchor="middle", weight="600"))
        add(rect(783, 125, 296, 610, fill="#F7F7F9", stroke=BORDER, sw=1, rx=6))
        add(text(931, 430, "webview 子视图（wry）", size=12, fill=FG_FAINT, anchor="middle"))
    return "".join(o)


def row(add, y, title, sid, status):
    """One session row with hover reveal of the trailing actions."""
    add(f'<g class="hoverable cursor-ptr">')
    add(rect(10, y, 210, 46, fill="transparent", rx=4))
    if status == "running":
        add(running_blocks(32, y + 9))
    else:
        add(pulse_dot(32, y + 9, 4) if status == "pending" else
            circle(32, y + 9, 4, fill=BADGE_BLUE_BG))
    add(text(46, y + 13, title, size=12.5, fill=FG))
    add(text(32, y + 33, "· 未读", size=11.5, fill=FG_FAINT))
    chw = measure(sid, 10.5) + 12
    chx = 230 - 12 - chw
    add(rect(chx, y + 23, chw, 16, fill="none", stroke=BORDER, sw=1, rx=3))
    add(text(chx + chw / 2, y + 35, sid, size=10.5, fill=FG_FAINT, anchor="middle"))
    # revealed on hover: pin / archive / menu (the selected-row action cluster)
    add('<g class="row-actions">')
    ax = 230 - 8
    for nm in ("more", "archive", "pin"):
        ax -= 16
        add(circle(ax + 6, y + 9, 8, fill="transparent"))
        add(codicon(nm, ax + 6, y + 9, 11, FG_FAINT))
    add("</g>")
    add("</g>")


def help_panel(kind, title, desc):
    s = []
    ax = M + SHEET_W + 26
    s.append(text(ax, 30, title, size=13, fill=FG, weight="600"))
    s.append(text(ax, 50, "纯 SVG：CSS + SMIL，无 JavaScript", size=10.5, fill=FG_FAINT))
    y = 86
    for line in desc:
        s.append(circle(ax + 3, y - 4, 2.5, fill=ACCENT))
        s.append(text(ax + 14, y, line, size=11, fill=FG_DIM))
        y += 20
    y += 10
    s.append(text(ax, y, "可交互项", size=11.5, fill=FG, weight="600"))
    y += 20
    for line in ("鼠标悬停会话行 → 底色 + 浮动操作",
                 "悬停工具栏图标 → 整箱高亮",
                 "悬停 Sync 药丸 → 深色 ACCENT_HOVER",
                 "悬停页签 / 快捷动作 → 底色",
                 "动画：呼吸点 / 三格 / braille 自动播放"):
        s.append(text(ax + 8, y, line, size=10.5, fill=FG_DIM))
        y += 17
    y += 12
    s.append(text(ax, y, "注：SVG 内 <script> 会被 <img> 禁用，", size=10, fill=FG_FAINT))
    s.append(text(ax, y + 14, "故本页一律用 CSS/SMIL 声明式实现。", size=10, fill=FG_FAINT))
    return "".join(s)


def build(fname, title, desc, kind):
    reset_used()
    o = []
    o.append(f'<g transform="translate({M},{M + 52})">')
    o.append(f'<g filter="url(#win-shadow)">')
    o.append(rect(0, 0, SHEET_W, SHEET_H, fill=SHELL_BG, rx=10))
    o.append("</g>")
    o.append('<g clip-path="url(#win-clip)">')
    o.append(sheet(kind))
    o.append("</g>")
    o.append(rect(0, 0, SHEET_W, SHEET_H, fill="none", stroke="rgba(0,0,0,0.18)",
                  sw=1, rx=10))
    o.append("</g>")
    o.append(help_panel(kind, title, desc))

    defs = (used_symbol_defs(used_symbols()) + window_shadow_defs()
            .replace(f'width="{WINDOW_W}" height="{WINDOW_H}" rx="10"',
                     f'width="{SHEET_W}" height="{SHEET_H}" rx="10"')
            .replace(f'<rect x="0" y="0" width="{WINDOW_W}"',
                     f'<rect x="0" y="0" width="{SHEET_W}"')
            .replace(f'height="{WINDOW_H}" rx="10"/>\n    </clipPath>',
                     f'height="{SHEET_H}" rx="10"/>\n    </clipPath>'))

    # `:target` needs an anchor shared by the page: the collapse demo points at
    # #g-manox so the same file can show both states.
    anchors = ''
    if kind == "collapsed":
        anchors = '<a id="g-manox-collapsed"></a>'
    body = defs + anchors + "".join(o)
    here = os.path.dirname(os.path.abspath(__file__))
    save(os.path.join(here, "interactive", fname), body, TOTAL_W, TOTAL_H,
         title, style=interaction_defs())


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    os.makedirs(os.path.join(here, "interactive"), exist_ok=True)
    for fname, title, desc in FIGS:
        kind = {
            "a-static.svg": "static",
            "b-hover-row.svg": "static",
            "c-hover-toolbar.svg": "static",
            "d-collapsed.svg": "collapsed",
            "e-running.svg": "running",
            "f-tab-browser.svg": "tab-browser",
        }[fname]
        build(fname, title, [desc], kind)
