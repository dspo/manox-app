# UI design prototypes

Hand-built SVG mockups of the manox desktop shell, used as a shared reference
when discussing chrome layout. Nothing here is code: no crate, no build step,
and nothing in `crates/` depends on it.

Open any file directly in a browser. `01-main-window.svg` and the
`interactive/` sheets are the main entry points.

## Contents

### `svg/` — static figures

| File | What it shows |
| --- | --- |
| `00-index.svg` | Index: figure list, geometry constants, 2026-Light palette |
| `01-main-window.svg` | The app shell: toolbar + sidebar + empty `MainSurface` card + right pane |
| `02-hero-empty.svg` | Hero empty-session screen |
| `03-ask-drawer.svg` | `AskDrawer` multi-step question card |
| `04-plan-review.svg` | `PlanReviewDecisionCard` |
| `05-settings.svg` | Settings (`nav｜panel` inside the main card) |
| `06-session-picker.svg` | Toolbar session-picker dropdown |

### `svg/interactive/` — interaction sheets (CSS + SMIL, no JavaScript)

| File | Interaction |
| --- | --- |
| `00-index.svg` | Index + the measured capability boundary |
| `a-static.svg` | Baseline: no hover, nothing expanded |
| `b-hover-row.svg` | Session-row hover: wash + revealed pin/archive/menu |
| `c-hover-toolbar.svg` | Icon-button and `Sync Changes` pill hover |
| `d-collapsed.svg` | Sidebar group collapse (`:target`) |
| `e-running.svg` | Attention pulse, running blocks, braille spinner |
| `f-tab-browser.svg` | Shipping pill tabs, Browser active |
| `g-underline-terminal.svg` | **Proposal**: underline tabs, Terminal active |
| `h-underline-browser.svg` | **Proposal**: underline tabs, Browser active |

The `f-` sheet draws the tabs as shipped (`right_pane.rs::tab_pill`: 31px
rounded-top pill). The `g-`/`h-` pair is a **design proposal**, not a
description of the app — flat labels over a hairline with a 2px accent
indicator. Clicking a tab in those sheets really switches state.

## Derived from the code, not drawn by eye

Geometry and colour are taken from the sources, so a figure can be re-checked
against them:

- `crates/manox-agent-chrome-ui/src/theme/palette.rs` — the 2026-Light tokens
- `.../src/{shell,titlebar,session_list,right_pane,divider}.rs` — layout sizes
- `crates/manox-agent-chat-ui/src/views/context_rail.rs` — rail geometry
- `UI-MAP.md` — component names and containment

Icons are the app's own assets, inlined as `<symbol>`s and referenced with
`<use>`:

- `crates/manox-agent-chrome-ui/assets/fonts/codicon.ttf` — the chrome crate's
  icon vocabulary, outline-extracted via fontTools (`theme::icon(NAME, size)`)
- `crates/agent-ui/assets/icons/*.svg` — the brand marks `tool_tabs.rs::registry`
  hands to the CLI-agent tab factories

An SVG `<use>` cannot reference a fragment of an external SVG document (and a
font cannot be referenced without embedding it), so both families are inlined.
Each sheet carries only the symbols it actually draws.

## Working on these

```bash
cd design/svg
python3 gen_main.py            # 01
python3 gen_states.py          # 02-06
python3 gen_index.py           # svg/00-index
python3 gen_interactive.py     # interactive/a-f
python3 gen_underline_tabs.py  # interactive/g-h
python3 gen_interactive_index.py

python3 validate.py            # page bounds + text collision
python3 validate_regions.py    # per-card overflow
python3 verify_interactive.py  # hover/animation/:target in real Chrome
```

`validate*.py` need only the Python stdlib. `verify_interactive.py` drives
system Chrome through Playwright and asserts **computed styles actually
change** — a static checker cannot tell whether `:hover` or `:target` applies.
It currently reports 52 passing checks and is the reason several real defects
were caught (a `.row-actions` class that was never emitted, a tab strip drawn
over the toolbar, a braille cycle that blanked for part of every period).

`codicon.py` needs `fonttools`; everything else is stdlib.

## Scope

The main card is drawn as an empty `MainSurface` slot on purpose: the message
column and composer belong to `agent-ui`'s `Workspace`, not to the chrome, so
they are outside a shell prototype. Figures 02–06 are kept as reference for
those flows.
