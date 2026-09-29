"""Index + spec sheet for the interactive (pure-SVG) shell prototypes."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *

W, H = 1180.0, 900.0
o = []
def add(s): o.append(s)

add(header("manox 外壳原型 · 纯 SVG 交互版",
           "只用 CSS :hover/:target 与 SMIL 动画，无 JavaScript——"
           "因此仍是单文件，能当 <img> 用、能在 GitHub 直接预览", 64))

x0, y0 = 40.0, 96.0
add(rect(x0, y0, W - 80, H - y0 - 40, fill=CARD_BG, stroke=CARD_BORDER, sw=1, rx=8))
fx, fy = x0 + 32, y0 + 40

add(text(fx, fy, "图集（interactive/）", size=15, fill=FG_STRONG, weight="600"))
fy += 14
add(line(fx, fy, x0 + W - 80 - 32, fy, stroke=BORDER))
fy += 26

figs_ = [
    ("a-static.svg", "静止态", "交付基线：无 hover、无展开"),
    ("b-hover-row.svg", "会话行 hover", "行底色 + 右侧浮动 pin/archive/menu 淡入"),
    ("c-hover-toolbar.svg", "工具栏 hover", "图标整箱 #F0F0F2；Sync 药丸转 #005AAE"),
    ("d-collapsed.svg", "分组折叠", "manox 组收起（:target 状态）"),
    ("e-running.svg", "运行态动画", "呼吸点 + 三格跳动 + 8 帧 braille spinner"),
    ("f-tab-browser.svg", "页签切换", "出厂药丸页签：Browser 激活，终端转非激活"),
    ("g-underline-terminal.svg", "下划线页签 · Terminal", "设计提案：扁平文字 + 2px 强调下划线"),
    ("h-underline-browser.svg", "下划线页签 · Browser", "点页签可真切换（:target，零脚本）"),
]
for name, title, desc in figs_:
    add(rect(fx, fy - 13, 20, 18, fill=ICON_ON_BG, rx=4))
    add(text(fx + 10, fy, name[0], size=11, fill=ACCENT, weight="600", anchor="middle"))
    add(text(fx + 32, fy, title, size=12.5, fill=FG, weight="600"))
    add(text(fx + 150, fy, desc, size=11, fill=FG_DIM))
    add(text(fx + 150, fy + 15, name, size=10, fill=FG_FAINT, family=FONT_MONO))
    fy += 36

fy += 8
add(text(fx, fy, "技术边界（实测，非推断）", size=13, fill=FG_STRONG, weight="600"))
fy += 22
notes = [
    ("可以做", [
        "hover 底色 / 淡入淡出（CSS transition）",
        "点击切换状态（:target 换 hash）",
        "循环动画（SMIL <animate>）",
        "真实图标（codicon.ttf 抽出的 <symbol>）",
    ]),
    ("做不到", [
        "真正的滚动（无滚轮、无拖拽）",
        "悬停弹层维持（鼠标离开即收起）",
        "多步表单 / 输入框交互",
        "跨文件共享状态",
    ]),
]
colw = (W - 80 - 64) / 2
for ci, (head, items) in enumerate(notes):
    cx = fx + ci * (colw + 16)
    col = OK_GREEN if ci == 0 else WARN_ORANGE
    add(circle(cx + 4, fy - 4, 3.5, fill=col))
    add(text(cx + 16, fy, head, size=12, fill=FG, weight="600"))
    for j, it in enumerate(items):
        add(text(cx + 16, fy + 20 + j * 17, "· " + it, size=11, fill=FG_DIM))
    fy2 = fy + 20 + len(items) * 17
fy = fy2 + 20

add(line(fx, fy, x0 + W - 80 - 32, fy, stroke=BORDER))
fy += 24
add(text(fx, fy, "图标来源（两族，与应用一一对应）", size=13, fill=FG_STRONG, weight="600"))
fy += 22
for line in [
    "① codicon.ttf → chrome 层的全部图标（folder / terminal / play / sync …）",
    "   crates/manox-agent-chrome-ui/assets/fonts/，对应 theme::icon(NAME, size)",
    "② brand SVG → 右栏三个 CLI agent 的品牌标（claude / codex / githubcopilot）",
    "   crates/agent-ui/assets/icons/，对应 tool_tabs.rs::registry 的 icon 路径",
    "",
    "两族都内联为 <symbol> + <use>：SVG 的 <use> 无法跨文档引用外部 svg 片段，",
    "字体也无法在不内嵌的情况下被独立 svg 引用，所以只能内联（代价是文件偏大）。",
]:
    add(text(fx, fy, line, size=11, fill=FG_DIM,
             family=FONT_MONO if ("crates/" in line or "①" in line or "②" in line) else None))
    fy += 17

fy += 12
add(text(fx, fy, "校验", size=13, fill=FG_STRONG, weight="600"))
fy += 22
for line in [
    "verify_interactive.py 用 Playwright 驱动系统 Chrome，断言「计算样式」真的变化：",
    "hover 底色、opacity 淡入、SMIL 时钟推进、braille 帧循环、:target 行数差异。",
    "当前 29 项全通过。静态几何另由 validate.py / validate_regions.py 把关。",
]:
    add(text(fx, fy, line, size=11, fill=FG_DIM))
    fy += 17

here = os.path.dirname(os.path.abspath(__file__))
save(os.path.join(here, "interactive", "00-index.svg"),
     window_shadow_defs() + "".join(o), W, H, "纯 SVG 交互原型索引")
