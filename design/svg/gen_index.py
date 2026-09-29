"""Index sheet — the prototype set at a glance, plus the token/geometry spec
the drawings were derived from."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _common import *

W, H = 1200.0, 760.0
o = []
def add(s): o.append(s)

add(header("manox-app GUI 原型 · SVG",
           "基于 ahp-client 分支代码绘制：几何取自 shell.rs / titlebar.rs / "
           "session_list.rs / right_pane.rs / divider.rs，色值取自 theme/palette.rs", 64))

x0, y0 = 40.0, 96.0
add(rect(x0, y0, W - 80, H - y0 - 40, fill=CARD_BG, stroke=CARD_BORDER, sw=1, rx=8))

# ── figure list ───────────────────────────────────────────────────────────
fx, fy = x0 + 32, y0 + 40
add(text(fx, fy, "图集", size=16, fill=FG_STRONG, weight="600"))
fy += 14
add(line(fx, fy, x0 + W - 80 - 32, fy, stroke=BORDER))
fy += 26
figs = [
    ("01-main-window.svg", "应用外壳（app shell）",
     "工具栏 + 侧栏 + 主区卡（空壳 MainSurface 槽）+ 右栏；不含会话列内容"),
    ("02-hero-empty.svg", "Hero 空会话",
     "无实质消息时的居中欢迎区；composer 内联挂载，发送键在历史恢复前禁用"),
    ("03-ask-drawer.svg", "AskDrawer 多步问答",
     "内联卡片；选项列表 + 每题自定义输入；底部固定 footer（上一题/1 of 2/下一题/跳过/下一步）"),
    ("04-plan-review.svg", "PlanReviewDecisionCard",
     "plan-review 意图的单题 ask：warning 色条 + plan 正文 + 底部一键裁决（点击即裁决）"),
    ("05-settings.svg", "Settings 设置页",
     "主区卡内的 nav｜panel 两栏，替换会话列直到 back 控件退出"),
    ("06-session-picker.svg", "会话选择器",
     "工具栏中部选择器展开：搜索框 + 最近 10 条，锚定 TopLeft 偏移 (0, 30)"),
]
for name, title, desc in figs:
    add(rect(fx, fy - 14, 34, 22, fill=ICON_ON_BG, rx=4))
    add(text(fx + 17, fy + 1, name[:2], size=11.5, fill=ACCENT, weight="600",
             anchor="middle"))
    add(text(fx + 46, fy + 1, title, size=13, fill=FG, weight="600"))
    add(text(fx + 46, fy + 19, name, size=10.5, fill=FG_FAINT, family=FONT_MONO))
    add(text(fx + 190, fy + 19, desc, size=11, fill=FG_DIM))
    fy += 44
    add(line(fx, fy - 12, x0 + W - 80 - 32, fy - 12, stroke="rgba(0,0,0,0.05)"))

# ── spec columns ──────────────────────────────────────────────────────────
cy = fy + 16
col2 = x0 + (W - 80) / 2 + 10
add(text(fx, cy, "关键几何（代码常量）", size=13, fill=FG_STRONG, weight="600"))
add(text(col2, cy, "2026-Light 色板（palette.rs）", size=13, fill=FG_STRONG, weight="600"))
cy += 24

geo = [
    ("窗口", "代码默认 1280×820（最小 980×640）；图 01 按截图 1100×761 绘制"),
    ("工具栏", "38px；traffic-light (12,13)，内容起点 70px（图 01 按截图量得 31px）"),
    ("侧栏", "224px 默认，拖拽 180–460"),
    ("卡片缝 / 外边距", "PANE_GAP 6px / FLOAT_GAP 8px，圆角 8px"),
    ("会话行", "固定 46px（两行：标题 20 + meta 16）；图 01 按截图量得 58px"),
    ("主区卡", "flex-1；右栏 460px（320–900）"),
    ("底部 dock", "220px；页签条 32px（图 01 中为折叠态）"),
    ("ContextRail", "260px 浮动卡片；属会话列内容，不在外壳原型范围内"),
    ("右栏页签条", "36px；激活页签 31px 顶角并入内容"),
]
for k, v in geo:
    add(text(fx, cy, k, size=11.5, fill=FG))
    add(text(fx + 118, cy, v, size=11.5, fill=FG_DIM))
    cy += 19

ty = fy + 16 + 24
swatches = [
    ("SHELL_BG", SHELL_BG, "#FAFAFD"), ("CARD_BG", CARD_BG, "#FFFFFF"),
    ("TABBAR_BG", TABBAR_BG, "#EAEAEA"), ("BORDER", BORDER, "#E3E3E6"),
    ("FG", FG, "#202020"), ("FG_DIM", FG_DIM, "#606060"),
    ("FG_FAINT", FG_FAINT, "#8A8A8E"), ("ACCENT", ACCENT, "#0069CC"),
    ("OK_GREEN", OK_GREEN, "#1A7F37"), ("WARN_ORANGE", WARN_ORANGE, "#9A6700"),
    ("ERR_RED", ERR_RED, "#CF222E"), ("SURFACE_TERTIARY", SURFACE_TERTIARY, "#F0F0F2"),
]
for name, col, hexv in swatches:
    add(rect(col2, ty - 9, 14, 14, fill=col, stroke="rgba(0,0,0,0.12)", sw=1, rx=3))
    add(text(col2 + 22, ty + 2, name, size=11.5, fill=FG, family=FONT_MONO))
    add(text(col2 + 190, ty + 2, hexv, size=11.5, fill=FG_FAINT, family=FONT_MONO))
    ty += 21

fy2 = max(cy, ty) + 22
add(line(fx, fy2, x0 + W - 80 - 32, fy2, stroke=BORDER))
fy2 += 22
add(text(fx, fy2, "说明：SVG 为手工绘制的设计原型（非运行时截图）。文案为示意，"
                  "i18n 键见 crates/manox-i18n/locales/{zh-CN,en}.ftl；"
                  "字体为系统 UI 面（SF Pro / 苹方），未内嵌。",
         size=11, fill=FG_DIM))
add(text(fx, fy2 + 18, "生成脚本：gen_main.py（图 01）、gen_states.py（图 02–06）、"
                       "_common.py（几何与色板常量）；校验：validate.py / validate_regions.py。",
         size=11, fill=FG_DIM))

here = os.path.dirname(os.path.abspath(__file__))
save(os.path.join(here, "00-index.svg"),
     window_shadow_defs() + "".join(o), W, H, "manox GUI 原型索引")
