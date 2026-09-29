"""State variants — Hero empty screen, Ask card, PlanReview decision card,
Settings card, session picker.

Each figure reuses the chrome geometry from _common.py and renders just the
main card (plus surrounding chrome where the surface needs it), so the
variants can be compared side by side.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *
from codicon import codicon, brand, used_symbol_defs, used_symbols, reset_used

M = 40.0
CARD_W = 900.0
CARD_H = 640.0

o = []
def add(s): o.append(s)


def frame(title, subtitle, card_w=CARD_W, card_h=CARD_H):
    """Common page header + white main card outline."""
    add(header(title, subtitle, 64))
    add(f'<g transform="translate({M},{M + 56})">')
    add(rect(0, 0, card_w, card_h, fill=CARD_BG, stroke=CARD_BORDER, sw=1,
             rx=CARD_RADIUS))
    add(f'<g clip-path="url(#clip-{int(card_w)}x{int(card_h)})">')


def close(clip_w, clip_h):
    add("</g>")
    add(rect(0, 0, clip_w, clip_h, fill="none", stroke=CARD_BORDER, sw=1,
             rx=CARD_RADIUS))
    add("</g>")


def defs_for(w, h):
    return (used_symbol_defs(used_symbols()) + f'<defs><clipPath id="clip-{int(w)}x{int(h)}">'
            f'<rect x="0" y="0" width="{w:g}" height="{h:g}" rx="{CARD_RADIUS}"/>'
            f'</clipPath>'
            f'<filter id="card-shadow" x="-30%" y="-30%" width="160%" height="180%">'
            f'<feDropShadow dx="0" dy="4" stdDeviation="7" flood-color="#000" '
            f'flood-opacity="0.16"/></filter></defs>')


def composer(x, y, w, draft=None, chips=("claude-sonnet-4.5 · high", "工作区访问",
                                          "manox-app"), running=False,
             queue=None):
    """The composer block; returns its height."""
    s = []
    cy = y
    if queue:
        s.append(rect(x, cy, w, 26, fill="rgba(0,0,0,0.03)",
                      stroke="rgba(0,0,0,0.06)", sw=1, rx=5))
        s.append(codicon("more", x + 12, cy + 13, 11, FG_FAINT))
        s.append(text(x + 24, cy + 17, queue, size=11.5, fill=FG_DIM))
        s.append(text(x + w - 56, cy + 17, "立即", size=11, fill=ACCENT))
        s.append(codicon("close", x + w - 12, cy + 13, 10, FG_FAINT))
        cy += 34
    s.append(rect(x, cy, w, 44, fill=INPUT_BG, stroke=BORDER, sw=1, rx=7))
    if draft:
        s.append(text(x + 12, cy + 26, draft, size=12.5, fill=FG))
    else:
        s.append(text(x + 12, cy + 26, "输入消息，/ 唤起命令，@ 提及技能…", size=12.5,
                      fill=FG_FAINT))
    # send / stop button, 24px disc
    bx, by = x + w - 30, cy + 22
    if running:
        s.append(circle(bx, by, 12, fill="rgba(207,34,46,0.2)"))
        s.append(rect(bx - 4, by - 5, 3, 10, fill=ERR_RED, rx=1))
        s.append(rect(bx + 1, by - 5, 3, 10, fill=ERR_RED, rx=1))
    else:
        s.append(circle(bx, by, 12, fill="rgba(0,105,204,0.2)"))
        s.append(path(f"M {bx:g} {by+6:g} L {bx:g} {by-6:g}", stroke=ACCENT, sw=1.6))
        s.append(path(f"M {bx-4:g} {by-2:g} L {bx:g} {by-6.5:g} L {bx+4:g} {by-2:g}",
                      stroke=ACCENT, sw=1.6))
    cy += 52
    cx2 = x
    for label in chips:
        chw = 12 + 13 + 4 + len(label) * 6.6 + 10
        s.append(rect(cx2, cy, chw, 20, fill="rgba(0,0,0,0.03)", rx=10))
        s.append(codicon("sparkle" if label.startswith("claude") else
                       ("terminal" if "访问" in label else "folder"),
                       cx2 + 10, cy + 10, 11, FG_DIM))
        s.append(text(cx2 + 20, cy + 13.6, label, size=11, fill=FG_DIM))
        cx2 += chw + 6
    return "".join(s), cy + 20 - y


# ══ 02 · Hero (empty session) ══════════════════════════════════════════════
o.clear()
reset_used()
W, H = CARD_W, 560.0
frame("02 · Hero — 空会话首屏", "无实质消息时的垂直居中欢迎区 + 内联 composer",
      W, H)
# centered logo block
lx, ly = W / 2, 150
add(rect(lx - 26, ly - 26, 52, 52, fill=ACCENT, rx=14))
add(codicon("code", lx, ly, 30, "#FFFFFF"))
add(text(lx, ly + 76, "manox", size=30, fill=FG_STRONG, weight="600", anchor="middle"))
add(text(lx, ly + 100, "把任务交给 agent，在这里看着它做完", size=13, fill=FG_DIM,
         anchor="middle"))
add(text(lx, 390, "Loading conversation…", size=12, fill=FG_FAINT, anchor="middle"))
add(text(lx - 60, 390, "⠹", size=13, fill=ACCENT, family=FONT_MONO, anchor="middle"))
comp, _ch = composer(W / 2 - 320, 424, 640)
add(comp)
close(W, H)
here = os.path.dirname(os.path.abspath(__file__))
save(os.path.join(here, "02-hero-empty.svg"),
     defs_for(W, H) + "".join(o), W + M * 2, H + M * 2 + 56, "Hero 空会话")

# ══ 03 · Ask drawer (generic stepper) ══════════════════════════════════════
o.clear()
reset_used()
W, H = 820.0, 700.0
frame("03 · AskDrawer — 多步问答卡", "内联在转录里；footer 固定在下，body 上限 520px 内部滚动",
      W, H)
cx = 40
cw = W - 80
y = 28
# header: title + close
add(text(cx, y, "1 / 2", size=11, fill=FG_DIM))
add(text(cx + 40, y, "AskUserQuestion", size=12.5, fill=FG, weight="600"))
add(codicon("close", cx + cw - 8, y - 4, 12, FG_FAINT))
y += 10
add(line(cx, y, cx + cw, y, stroke=BORDER))
y += 22
# question
add(rect(cx, y - 12, 44, 18, fill="rgba(0,0,0,0.05)", rx=4))
add(text(cx + 8, y + 1, "范围", size=10.5, fill=FG_DIM))
add(text(cx + 54, y + 1, "这次重构要覆盖哪些 crate？", size=13.5, fill=FG, weight="600"))
y += 20
add(text(cx, y + 4, "勾选后会在同一个 PR 内改动；跨 crate 的改动需要同步更新 design/svg/ 原型。",
         size=11.5, fill=FG_DIM))
y += 26
# options: checkbox list with labels + descriptions
opts = [("manox-agent-chrome-ui", "壳与壳内组件（Shell / SessionList / RightPane）", False, False),
        ("manox-agent-chat-ui", "会话列状态机与消息管线", True, False),
        ("agent-ui", "状态与装配层（multiplexer / 侧栏投影）", False, False),
        ("terminal-ui", "终端渲染层（TerminalElement / TerminalView）", False, True)]
for label, desc, checked, disabled in opts:
    add(rect(cx, y, cw, 46, fill="rgba(0,0,0,0.02)" if checked else "none",
             stroke=BORDER if checked else "rgba(0,0,0,0.08)", sw=1, rx=6))
    bx, by = cx + 16, y + 16
    if checked:
        add(rect(bx - 7, by - 7, 14, 14, fill=ACCENT, rx=3))
        add(path(f"M {bx-3.5:g} {by:g} L {bx-1:g} {by+3:g} L {bx+4:g} {by-3.5:g}",
                 stroke="#fff", sw=1.8))
    else:
        add(rect(bx - 7, by - 7, 14, 14, fill=CARD_BG, stroke="rgba(0,0,0,0.28)",
                 sw=1, rx=3))
    add(text(cx + 34, y + 20, label, size=12.5,
             fill=FG_FAINT if disabled else FG, family=FONT_MONO))
    add(text(cx + 34, y + 36, desc, size=11, fill=FG_FAINT))
    y += 52
# per-question custom input
add(rect(cx, y + 4, cw, 32, fill=INPUT_BG, stroke=BORDER, sw=1, rx=6))
add(text(cx + 12, y + 24, "或输入自定义回答…", size=12, fill=FG_FAINT))
y += 46
# footer: pager left, skip + primary right
add(line(cx, y + 6, cx + cw, y + 6, stroke=BORDER))
fy = y + 30
add(text(cx, fy, "上一题", size=12, fill=FG_FAINT))
add(text(cx + 52, fy, "1 / 2", size=12, fill=FG_DIM))
add(text(cx + 96, fy, "下一题", size=12, fill=FG))
add(rect(cx + cw - 190, fy - 16, 76, 30, fill="none", stroke=BORDER, sw=1, rx=6))
add(text(cx + cw - 152, fy + 4, "跳过", size=12, fill=FG, anchor="middle"))
add(rect(cx + cw - 106, fy - 16, 106, 30, fill=ACCENT, rx=6))
add(text(cx + cw - 53, fy + 4, "下一步", size=12, fill="#fff", weight="600",
         anchor="middle"))
close(W, H)
save(os.path.join(here, "03-ask-drawer.svg"),
     defs_for(W, H) + "".join(o), W + M * 2, H + M * 2 + 56, "AskDrawer")

# ══ 04 · PlanReviewDecisionCard ════════════════════════════════════════════
o.clear()
reset_used()
W, H = 820.0, 660.0
frame("04 · PlanReviewDecisionCard — Plan 评审裁决卡",
      "warning 色条 + plan 正文内部滚动 + 底部一键裁决（点击即裁决）", W, H)
cx, cw = 40, W - 80
y = 26
# warning strip
add(rect(cx, y, cw, 30, fill="rgba(154,103,0,0.10)", rx=6))
add(circle(cx + 16, y + 15, 4.5, fill=WARN_ORANGE))
add(text(cx + 30, y + 19.5, "Plan 评审", size=12.5, fill=WARN_ORANGE, weight="600"))
add(text(cx + cw - 16, y + 19.5, "3 个选项", size=11, fill=FG_DIM, anchor="end"))
y += 42
# plan body (markdown), owns the internal scroll
add(text(cx, y + 4, "# 拆分计划：chrome 壳与会话列", size=14, fill=FG_STRONG,
         weight="600"))
y += 24
body = [
    "1. 从 shell.rs 抽出 PANE_GAP / FLOAT_GAP 常量到 divider.rs",
    "2. SessionList 改为 props 组件，交互经回调上抛",
    "3. RightPane 只保留页签机制，内容走 ToolTab trait",
    "4. chrome_assembly 把 Workspace 挂进 MainSurface 槽",
    "5. 删除双壳构建开关与旧 Workspace 全壳渲染",
]
for ln in body:
    add(text(cx + 8, y, ln, size=12, fill=FG))
    y += 20
y += 6
add(text(cx, y, "```rust", size=11.5, fill=FG_DIM, family=FONT_MONO))
y += 17
add(text(cx, y, "窗口 = chrome::Shell(MainSurface = ChatColumn,", size=11.5,
         fill=FG, family=FONT_MONO))
y += 17
add(text(cx, y, "                      ToolTab 注册表)", size=11.5, fill=FG,
         family=FONT_MONO))
y += 17
add(text(cx, y, "```", size=11.5, fill=FG_DIM, family=FONT_MONO))
y += 26
add(text(cx, y, "验证：每 PR 门禁 cargo clippy --all-targets -D warnings + 全量 test。",
         size=12, fill=FG_DIM))
# decision row (fixed bottom)
dy = H - 34
add(line(cx, dy - 26, cx + cw, dy - 26, stroke=BORDER))
add(rect(cx, dy - 16, 118, 32, fill=ACCENT, rx=6))
add(text(cx + 59, dy + 5, "Approve", size=12.5, fill="#fff", weight="600",
         anchor="middle"))
add(rect(cx + 128, dy - 16, 168, 32, fill="none", stroke=BORDER, sw=1, rx=6))
add(text(cx + 212, dy + 5, "Approve & compact", size=12.5, fill=FG, anchor="middle"))
add(rect(cx + 306, dy - 16, 150, 32, fill="none", stroke=BORDER, sw=1, rx=6))
add(text(cx + 381, dy + 5, "Request changes", size=12.5, fill=FG, anchor="middle"))
add(text(cx + cw, dy + 5, "继续讨论", size=12.5, fill=FG_DIM, anchor="end"))
close(W, H)
save(os.path.join(here, "04-plan-review.svg"),
     defs_for(W, H) + "".join(o), W + M * 2, H + M * 2 + 56, "PlanReview 裁决卡")

# ══ 05 · Settings (nav | panel inside the main card) ═══════════════════════
o.clear()
reset_used()
W, H = 940.0, 620.0
frame("05 · Settings — 主区卡内的 nav｜panel", "替换会话列直到 back 控件退出；两栏换位", W, H)
# title bar inside card
add(rect(0, 0, W, 44, fill=SHELL_BG))
add(codicon("arrow-left", 24, 22, 14, FG_DIM))
add(text(52, 26.5, "设置", size=13.5, fill=FG_STRONG, weight="600"))
add(line(0, 44, W, 44, stroke=BORDER))
# left nav 240px
NV = 240.0
add(rect(0, 45, NV, H - 45, fill=SHELL_BG))
add(rect(NV, 45, 1, H - 45, fill=BORDER))
# search
add(rect(12, 57, NV - 24, 30, fill=INPUT_BG, stroke=BORDER, sw=1, rx=6))
add(codicon("search", 26, 72, 13, FG_DIM))
add(text(40, 76.5, "搜索设置", size=12, fill=FG_FAINT))
ny = 104
sections = [("模型", ["Provider 与模型", "推理强度", "速率卡计价"]),
            ("会话", ["权限模式", "上下文与压缩", "计划与 Goal"]),
            ("工具", ["MCP servers", "Skills", "Plugins"]),
            ("外观", ["语言与区域", "字体", "快捷键"])]
active = "Provider 与模型"
for head, items in sections:
    add(text(14, ny, head, size=10.5, fill=FG_FAINT, weight="600"))
    ny += 18
    for it in items:
        if it == active:
            add(rect(8, ny - 13, NV - 16, 26, fill=LIST_HOVER, rx=5))
        add(text(22, ny + 4, it, size=12.5,
                 fill=FG if it == active else FG_DIM))
        ny += 26
    ny += 8
# right panel
px_, pw = NV + 28, W - NV - 56
py_ = 76
add(text(px_, py_, "Provider 与模型", size=17, fill=FG_STRONG, weight="600"))
py_ += 10
add(text(px_, py_ + 12, "决定新会话默认使用的端点；已存在的会话保持自己的选择。",
         size=12, fill=FG_DIM))
py_ += 40
# section card
add(rect(px_, py_, pw, 208, fill=CARD_BG, stroke=BORDER, sw=1, rx=8))
py_ += 18
add(text(px_ + 18, py_, "已配置端点", size=12.5, fill=FG, weight="600"))
py_ += 16
add(line(px_ + 18, py_, px_ + pw - 18, py_, stroke=BORDER))
py_ += 8
rows = [("anthropic", "claude-sonnet-4.5", "Responses", True),
        ("anthropic", "claude-opus-4.1", "Messages", False),
        ("openai", "gpt-5-codex", "Responses", True),
        ("google", "gemini-3-pro", "Messages", False)]
for prov, model, wire, on in rows:
    add(circle(px_ + 30, py_ + 16, 4, fill=OK_GREEN if on else "#C8C8CE"))
    add(text(px_ + 44, py_ + 20, prov, size=12.5, fill=FG, family=FONT_MONO))
    add(text(px_ + 150, py_ + 20, model, size=12.5, fill=FG, family=FONT_MONO))
    add(text(px_ + pw - 30, py_ + 20, wire, size=11, fill=FG_FAINT, anchor="end"))
    py_ += 34
    if py_ < py_ + 34:
        add(line(px_ + 18, py_ - 8, px_ + pw - 18, py_ - 8, stroke="rgba(0,0,0,0.05)"))
add(text(px_ + 18, py_ + 4, "连接状态由 provider registry 快照驱动。", size=11,
         fill=FG_FAINT))
close(W, H)
save(os.path.join(here, "05-settings.svg"),
     defs_for(W, H) + "".join(o), W + M * 2, H + M * 2 + 56, "Settings 设置页")

# ══ 06 · Session picker popover ════════════════════════════════════════════
o.clear()
reset_used()
W, H = 860.0, 560.0
# draw a titlebar slice + the anchored dropdown
add(header("06 · 会话选择器（工具栏下拉）",
           "点击工具栏中部的会话选择器展开：搜索框 + 最近 10 条；⊞ 锚定 TopLeft，宽 360",
           64))
add(f'<g transform="translate({M},{M + 56})">')
add(rect(0, 0, W, 46, fill=SHELL_BG))
add(rect(0, 0, W, 46, fill="none", stroke=BORDER, sw=1))
for i, c in enumerate((TRAFFIC_RED, TRAFFIC_YELLOW, TRAFFIC_GREEN)):
    add(circle(12 + 6 + i * 20, 13, 6, fill=c))
add(rect(70, 11, 300, 24, fill=CARD_BG, stroke=BORDER, sw=1, rx=5))
add(codicon("folder-open", 70 + 14, 23, 13, FG_DIM))
add(text(70 + 28, 27.4, "重构 chrome 壳的会话列布局", size=12.5, fill=FG))
add(codicon("chevron-down", 70 + 288, 23, 12, FG_DIM))
# dropdown at the picker anchor + offset(0,30)
dx, dy = 70, 41 + 8
dw = 360
add(f'<g filter="url(#card-shadow)">')
add(rect(dx, dy, dw, 392, fill=CARD_BG, stroke=BORDER, sw=1, rx=6))
add("</g>")
ix, iw = dx + 4, dw - 8
add(rect(ix, dy + 4, iw, 26, fill="none", stroke=BORDER, sw=1, rx=5))
add(codicon("search", ix + 12, dy + 17, 13, FG_DIM))
add(text(ix + 24, dy + 21.5, "搜索会话", size=12.5, fill=FG_FAINT))
ry = dy + 38
rows = [("重构 chrome 壳的会话列布局", "manox-app", True, True),
        ("AHP v3 通道折叠排查", "manox-app", True, False),
        ("cx web 网关超时", "manox-app", True, False),
        ("终端页签 PTY 泄漏", "manox-app", False, False),
        ("journal 折叠为 AHP 通道状态", "dspo-manox", False, False),
        ("plan-review 三选项裁决", "dspo-manox", False, False),
        ("写一个 SVG 原型", "Chats", False, False),
        ("暗色主题令牌对齐", "Chats", False, False)]
for title, ws, unread, sel in rows:
    if sel:
        add(rect(ix, ry, iw, 30, fill=LIST_HOVER, rx=4))
    add(circle(ix + 9, ry + 15, 3, fill=ACCENT if unread else "none"))
    add(text(ix + 18, ry + 19, title, size=12.5, fill=FG_STRONG))
    add(text(ix + iw - 6, ry + 19, ws, size=11, fill=FG_FAINT, anchor="end"))
    ry += 32
add(text(dx + dw / 2, ry + 16, "滚动查看更多（最多 10 条）", size=11, fill=FG_FAINT,
         anchor="middle"))
add("</g>")
save(os.path.join(here, "06-session-picker.svg"),
     defs_for(W, 500) + "".join(o), W + M * 2, H + M * 2 + 56, "会话选择器")
print("done")
