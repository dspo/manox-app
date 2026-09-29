"""Underline-style right-pane tabs — an alternative to the shell's shipping
"top-corner pill" tabs (`right_pane.rs::tab_pill`).

This is a DESIGN PROPOSAL, not a description of the app: the shipping tabs are
the 31px rounded-top pill that joins the body under it. Here the tabs are flat
labels over a shared hairline, with a 3px accent indicator under the active
one — the pattern in the reference the user supplied.

The reference switched tabs with `<script>` + `onclick`. That cannot work here:
`<script>` is inert inside `<img>`, and this sheet family is deliberately
declarative. The same behaviour is expressed with `:target` instead — three
stacked indicator+panel pairs, each revealed by its own anchor hash — so the
switching is real, keyboard-reachable (the anchors are focusable links), and
still script-free.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *
from codicon import codicon, brand, used_symbol_defs, used_symbols, reset_used
from validate import measure

M = 48.0
HELP_W = 320.0

SHEET_W = 1100.0
SHEET_H = 761.0
TOTAL_W = SHEET_W + M * 2 + HELP_W
TOTAL_H = SHEET_H + M * 2 + 96

# underline palette: the accent is the app's ACCENT; the rail hairline is its
# BORDER. Sizes are the reference's proportions scaled to the app's 12-13px UI.
IND_H = 2.0            # indicator thickness (reference uses 3 at 14px text)
TAB_PAD = 12.0         # horizontal padding inside a tab
TAB_H = 34.0           # strip height


def sheet(kind):
    o = []
    def add(s): o.append(s)

    # ── titlebar ──────────────────────────────────────────────────────────
    add(rect(0, 0, SHEET_W, 31, fill=SHELL_BG))
    for i, c in enumerate((TRAFFIC_RED, TRAFFIC_YELLOW, TRAFFIC_GREEN)):
        add(circle(18 + i * 20, 19, 6, fill=c))
    cy = 16.5

    def ibtn(gx, name, size=15, color=FG_DIM, active=False):
        add('<g class="hov-icon cursor-ptr">')
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
    ibtn(x, "sidebar-right", 15); x -= 36
    ibtn(x, "panel", 15); x -= 38
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

    # ── sidebar (unchanged from the shell sheets) ─────────────────────────
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

    CUST_H = 26 + 30 * 2
    cust_y = SHEET_H - CUST_H
    add(f'<clipPath id="sb-scroll"><rect x="0" y="{ry:g}" width="230" '
        f'height="{cust_y - 4 - ry:g}"/></clipPath>')
    add('<g clip-path="url(#sb-scroll)">')
    groups = [
        ("manox", [("Review dspo/manox PR #824", "ba690af"),
                   ("manox⇔manox-app 协议全…", "09afb8f"),
                   ("在内置终端中使用 Claude Co…", "e102bdc"),
                   ("一些 vs code extension 的 h…", "049a553")]),
        ("chenzhongrun", [("what model now ?", "5d6dea9"),
                          ("what model now ?", "1bf6621"),
                          ("what model now ?", "f64c637"),
                          ("hi", "bb29468"),
                          ("what cwd now ?", "7144964"),
                          ("写一个暴风雪山庄模式的小故…", "9b3e2d8"),
                          ("帮我清理磁盘：僵尸 worktree", "78ca56e")]),
    ]
    for gname, rows in groups:
        add('<g class="hov-row cursor-ptr">')
        add(rect(0, ry - 4, 230, 26, fill="transparent"))
        add("</g>")
        add(codicon("chevron-down", 20, ry + 10, 13, FG_DIM))
        add(codicon("folder", 38, ry + 10, 15, FG_DIM))
        add(text(54, ry + 14, gname, size=12.5, fill=FG, weight="600"))
        ry += 24
        for title, sid in rows:
            add('<g class="hoverable cursor-ptr">')
            add(rect(10, ry, 210, 46, fill="transparent", rx=4))
            add(circle(32, ry + 9, 4, fill=BADGE_BLUE_BG))
            add(text(46, ry + 13, title, size=12.5, fill=FG))
            add(text(32, ry + 33, "· 未读", size=11.5, fill=FG_FAINT))
            chw = measure(sid, 10.5) + 12
            chx = 230 - 12 - chw
            add(rect(chx, ry + 23, chw, 16, fill="none", stroke=BORDER, sw=1, rx=3))
            add(text(chx + chw / 2, ry + 35, sid, size=10.5, fill=FG_FAINT, anchor="middle"))
            add('<g class="row-actions">')
            axx = 230 - 8
            for nm in ("more", "archive", "pin"):
                axx -= 16
                add(codicon(nm, axx + 6, ry + 9, 11, FG_FAINT))
            add("</g>")
            add("</g>")
            ry += 58
        ry += 6
    add("</g>")
    add(rect(0, cust_y - 4, 230, CUST_H + 4, fill=SHELL_BG))
    add(codicon("chevron-down", 20, cust_y + 10, 13, FG_DIM))
    add(text(36, cust_y + 14, "自定义", size=12.5, fill=FG, weight="600"))
    yy = cust_y + 26
    for lt, ic in (("概览", "home"), ("MCP", "gear")):
        add('<g class="hov-row cursor-ptr">')
        add(rect(0, yy - 4, 230, 28, fill="transparent"))
        add("</g>")
        add(codicon(ic, 34, yy + 10, 13, FG_DIM))
        add(text(50, yy + 14, lt, size=12, fill=FG))
        yy += 30

    # ── main card: empty MainSurface slot ─────────────────────────────────
    add(rect(231, 31, 534, SHEET_H - 39, fill=CARD_BG, stroke=CARD_BORDER,
             sw=1, rx=CARD_RADIUS))
    add(rect(231 + 267 - 96, 31 + 361 - 13, 192, 26, fill="rgba(0,0,0,0.028)", rx=6))
    add(text(231 + 267, 31 + 361 + 4.5, "MainSurface 槽（外壳不绘制内容）", size=11,
             fill=FG_FAINT, anchor="middle"))

    # ── right pane: UNDERLINE TABS ────────────────────────────────────────
    RX, RW = 773.0, 318.0
    add(rect(RX, 31, RW, SHEET_H - 39, fill=CARD_BG, stroke=CARD_BORDER,
             sw=1, rx=CARD_RADIUS))

    # anchors: one per tab state, so :target can drive indicator + panel
    for anc in ("u-terminal", "u-browser"):
        add(f'<a id="{anc}"></a>')

    tabs = [("terminal", "Terminal", "u-terminal"),
            ("globe", "Browser", "u-browser")]
    strip_y = 31.0
    rail_y = strip_y + TAB_H
    add(line(RX + 1, rail_y, RX + RW - 1, rail_y, stroke=BORDER))

    # the reference's hover wash + close reveal, minus the JS
    tx = RX + 10
    tab_boxes = []
    for ic, name, anc in tabs:
        tw = TAB_PAD * 2 + 15 + 7 + measure(name, 12.5) + 7 + 14
        tab_boxes.append((tx, tw, ic, name, anc))
        tx += tw
    # "+ 新标签页" stays at the strip's right end, as an action not a tab
    newx = tx + 6

    def paint_tab(bx, bw, ic, name, anc, active):
        add(f'<a href="#{anc}" class="tab-btn t-{name.lower()}">')
        add(f'<rect x="{bx:g}" y="{strip_y+2:g}" width="{bw:g}" '
            f'height="{TAB_H-2:g}" rx="5" fill="transparent" class="tab-hoverbg"/>')
        col = ACCENT if active else FG_DIM
        add(codicon(ic, bx + TAB_PAD + 7, strip_y + TAB_H / 2, 15, col))
        add(text(bx + TAB_PAD + 22, strip_y + TAB_H / 2 + 4.2, name, size=12.5,
                 fill=col, weight="600" if active else "400"))
        cx2 = bx + bw - 14
        add(f'<g class="tab-close">')
        add(codicon("close", cx2, strip_y + TAB_H / 2, 11, FG_FAINT))
        add("</g>")
        add("</a>")

    # Tab labels are static (both drawn, one tinted active per state group);
    # the INDICATOR and the BODY are what switch. The anchors must be siblings
    # of those state groups — a `:target` nested inside a wrapper cannot reach
    # out of it — so the whole strip+body is emitted as three stacked sibling
    # groups after the anchors, and CSS shows exactly one.
    default_state = "browser" if kind == "underline-browser" else "terminal"

    def indicator(bx, bw):
        add(f'<rect class="ind" x="{bx:g}" y="{rail_y-IND_H:g}" width="{bw:g}" '
            f'height="{IND_H:g}" rx="1" fill="{ACCENT}"/>')

    def indicator_for(active_name):
        """The underline under the named tab (one per state group)."""
        for (bx, bw, ic, name, anc) in tab_boxes:
            if name.lower() == active_name:
                indicator(bx, bw)

    def toolbar():
        by = rail_y + 30
        for gx, nm in ((RX + 16, "add"), (RX + 74, "split"), (RX + 130, "external")):
            add('<g class="hov-icon cursor-ptr">')
            add(f'<rect class="ibox" x="{gx-11:g}" y="{by-9.5:g}" width="22" '
                f'height="19" rx="4" fill="transparent"/>')
            add(codicon(nm, gx, by, 15, FG))
            add("</g>")

    def new_tab_btn():
        add('<g class="hov-accent cursor-ptr">')
        add(f'<rect x="{newx-4:g}" y="{strip_y+4:g}" width="92" height="{TAB_H-8:g}" '
            f'rx="5" fill="transparent"/>')
        add("</g>")
        add(codicon("add", newx + 8, strip_y + TAB_H / 2, 13, FG_DIM))
        add(text(newx + 20, strip_y + TAB_H / 2 + 4.2, "新标签页", size=12, fill=FG_DIM))

    def body_terminal():
        lines = [("~ ", FG_DIM), ("cargo run", FG), ("", FG),
                 ("   Compiling agent-ui v0.1.0", FG_DIM),
                 ("    Finished `dev` profile in 42.18s", OK_GREEN),
                 ("", FG), ("~ ", FG_DIM), ("git status --short", FG),
                 (" M AGENTS.md", WARN_ORANGE), ("?? design/svg/", FG_FAINT),
                 ("", FG), ("~ ", FG_DIM)]
        ly = rail_y + 105
        for txt, colr in lines:
            add(text(RX + 14, ly, txt, size=11.5, fill=colr, family=FONT_MONO))
            ly += 17
        add(rect(RX + 27, ly - 10, 7, 13, fill=FG, opacity="0.7"))
        quick_actions()

    def body_browser():
        add(rect(RX + 14, rail_y + 20, 240, 30, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
        add(text(RX + 26, rail_y + 39, "https://example.com", size=12, fill=FG,
                 family=FONT_MONO))
        add(rect(RX + 262, rail_y + 20, 42, 30, fill=ACCENT, rx=5))
        add(text(RX + 283, rail_y + 39, "Go", size=12, fill="#fff", anchor="middle",
                 weight="600"))
        add(rect(RX + 14, rail_y + 62, 290, 560, fill="#F7F7F9", stroke=BORDER,
                 sw=1, rx=6))
        add(text(RX + 159, rail_y + 340, "webview 子视图（wry）", size=12,
                 fill=FG_FAINT, anchor="middle"))

    def quick_actions():
        quick = [("codicon", "globe", "打开集成浏览器"),
                 ("codicon", "terminal", "打开集成终端"),
                 ("brand", "claude", "打开 Claude Code"),
                 ("brand", "codex", "打开 Codex"),
                 ("brand", "copilot", "打开 GitHub Copilot"),
                 ("codicon", "symbol-file", "打开编辑器")]
        qy = 552.0
        for knd, ic, lt in quick:
            add('<g class="hov-accent cursor-ptr">')
            add(rect(RX + 6, qy - 11, 306, 28, fill="transparent", rx=6))
            add("</g>")
            if knd == "brand":
                add(brand(ic, RX + 30 - 7.5, qy - 7.5, 15, FG))
            else:
                add(codicon(ic, RX + 30, qy, 15, FG))
            add(text(RX + 46, qy + 5, lt, size=13, fill=FG))
            qy += 34

    # ── the strip is drawn ONCE, above the state groups ──────────────────
    # Its links must stay reachable in every state, so it is not duplicated
    # per state; only the indicator and the body below it switch.
    add('<g class="u-strip">')
    for (bx, bw, ic, name, anc) in tab_boxes:
        paint_tab(bx, bw, ic, name, anc,
                  active=(name.lower() == default_state))
    add("</g>")
    new_tab_btn()
    toolbar()

    # ── switching layers: indicator + body, one sibling group per anchor ──
    add('<g class="u-state u-default">')
    indicator_for(default_state)
    body_browser() if default_state == "browser" else body_terminal()
    add("</g>")

    add('<g class="u-state u-terminal">')
    indicator_for("terminal")
    body_terminal()
    add("</g>")

    add('<g class="u-state u-browser">')
    indicator_for("browser")
    body_browser()
    add("</g>")
    return "".join(o)


def help_panel(title, desc, kind):
    s = []
    ax = M + SHEET_W + 26
    s.append(text(ax, 30, title, size=13, fill=FG, weight="600"))
    s.append(text(ax, 50, "下划线式页签（设计提案）", size=10.5, fill=FG_FAINT))
    y = 86
    for line in desc:
        s.append(circle(ax + 3, y - 4, 2.5, fill=ACCENT))
        s.append(text(ax + 14, y, line, size=11, fill=FG_DIM))
        y += 20
    y += 12
    s.append(text(ax, y, "与出厂样式的差异", size=11.5, fill=FG, weight="600"))
    y += 20
    for line in ("出厂：31px 顶角圆角药丸，激活页签并入内容",
                 "本提案：34px 扁平文字 + 2px 强调色下划线",
                 "激活态用 ACCENT + 600 字重；关闭键 hover 才显现",
                 "下划线宽度 = 页签宽度（随标签长度自适应）"):
        s.append(text(ax + 8, y, line, size=10.5, fill=FG_DIM))
        y += 17
    y += 14
    s.append(text(ax, y, "注意", size=11.5, fill=FG, weight="600"))
    y += 20
    for line in ("参考稿用 <script>+onclick，在 <img> 里无效；",
                 "这里改用 :target 切换，保持零脚本。",
                 "要真正切换请点页签（会写入 URL hash）。"):
        s.append(text(ax + 8, y, line, size=10.5, fill=FG_DIM))
        y += 17
    return "".join(s)


def build(fname, title, desc, kind):
    reset_used()
    o = []
    o.append(f'<g transform="translate({M},{M + 52})">')
    o.append('<g filter="url(#win-shadow)">')
    o.append(rect(0, 0, SHEET_W, SHEET_H, fill=SHELL_BG, rx=10))
    o.append("</g>")
    o.append('<g clip-path="url(#win-clip)">')
    o.append(sheet(kind))
    o.append("</g>")
    o.append(rect(0, 0, SHEET_W, SHEET_H, fill="none", stroke="rgba(0,0,0,0.18)",
                  sw=1, rx=10))
    o.append("</g>")
    o.append(help_panel(title, desc, kind))

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


FIGS = [
    ("g-underline-terminal.svg", "下划线页签 · Terminal 激活",
     ["扁平文字页签，激活项为强调色 + 下划线", "页签宽度随标签文本自适应"], "underline-terminal"),
    ("h-underline-browser.svg", "下划线页签 · Browser 激活",
     ["下划线移到 Browser 下方", "内容区随激活页签切换"], "underline-browser"),
]

if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    os.makedirs(os.path.join(here, "interactive"), exist_ok=True)
    for fname, title, desc, kind in FIGS:
        build(fname, title, desc, kind)
