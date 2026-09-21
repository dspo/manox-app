//! codicon codepoint table (generated from VS Code `codiconsLibrary.ts`),
//! paired with the embedded `codicon.ttf`. `icon(NAME, size)` renders one
//! glyph; glyphs live in the font's private-use area and inherit the
//! surrounding text color.

use gpui::{IntoElement, ParentElement, Styled, div, px};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icon(pub &'static str);

pub const ACCOUNT: Icon = Icon("\u{eb99}");
pub const ACTIVATE_BREAKPOINTS: Icon = Icon("\u{ea97}");
pub const ADD: Icon = Icon("\u{ea60}");
pub const ARCHIVE: Icon = Icon("\u{ea98}");
pub const ARROW_LEFT: Icon = Icon("\u{ea9b}");
pub const ARROW_RIGHT: Icon = Icon("\u{ea9c}");
pub const ARROW_SMALL_RIGHT: Icon = Icon("\u{ea9f}");
pub const ARROW_UP: Icon = Icon("\u{eaa1}");
pub const BEAKER: Icon = Icon("\u{ea79}");
pub const BELL: Icon = Icon("\u{eaa2}");
pub const BOLD: Icon = Icon("\u{eaa3}");
pub const BOOK: Icon = Icon("\u{eaa4}");
pub const BROADCAST: Icon = Icon("\u{eaad}");
pub const BROWSER: Icon = Icon("\u{eaae}");
pub const BUG: Icon = Icon("\u{eaaf}");
pub const CALENDAR: Icon = Icon("\u{eab0}");
pub const CASE_SENSITIVE: Icon = Icon("\u{eab1}");
pub const CHAT_SPARKLE: Icon = Icon("\u{ec4f}");
pub const CHECK: Icon = Icon("\u{eab2}");
pub const CHECK_ALL: Icon = Icon("\u{ebb1}");
pub const CHECKLIST: Icon = Icon("\u{eab3}");
pub const CHEVRON_DOWN: Icon = Icon("\u{eab4}");
pub const CHEVRON_LEFT: Icon = Icon("\u{eab5}");
pub const CHEVRON_RIGHT: Icon = Icon("\u{eab6}");
pub const CHEVRON_UP: Icon = Icon("\u{eab7}");
pub const CHROME_CLOSE: Icon = Icon("\u{eab8}");
pub const CHROME_MAXIMIZE: Icon = Icon("\u{eab9}");
pub const CHROME_MINIMIZE: Icon = Icon("\u{eaba}");
pub const CHROME_RESTORE: Icon = Icon("\u{eabb}");
pub const CIRCLE_FILLED: Icon = Icon("\u{ea71}");
pub const CIRCLE_OUTLINE: Icon = Icon("\u{eabc}");
pub const CIRCLE_SLASH: Icon = Icon("\u{eabd}");
pub const CLEAR_ALL: Icon = Icon("\u{eabf}");
pub const CLOCK: Icon = Icon("\u{ea82}");
pub const CLOSE: Icon = Icon("\u{ea76}");
pub const CLOUD: Icon = Icon("\u{ebaa}");
pub const CODE: Icon = Icon("\u{eac4}");
pub const COLLAPSE_ALL: Icon = Icon("\u{eac5}");
pub const COMBINE: Icon = Icon("\u{ebb6}");
pub const COMMENT_DISCUSSION: Icon = Icon("\u{eac7}");
pub const COPY: Icon = Icon("\u{ebcc}");
pub const DEBUG_BREAKPOINT_CONDITIONAL_UNVERIFIED: Icon = Icon("\u{eaa6}");
pub const DEBUG_PAUSE: Icon = Icon("\u{ead1}");
pub const DEBUG_START: Icon = Icon("\u{ead3}");
pub const DESKTOP_DOWNLOAD: Icon = Icon("\u{ea78}");
pub const DIFF: Icon = Icon("\u{eae1}");
pub const DISCARD: Icon = Icon("\u{eae2}");
pub const EDIT: Icon = Icon("\u{ea73}");
pub const ELLIPSIS: Icon = Icon("\u{ea7c}");
pub const ERROR: Icon = Icon("\u{ea87}");
pub const EXPAND_ALL: Icon = Icon("\u{eb95}");
pub const EXTENSIONS: Icon = Icon("\u{eae6}");
pub const EYE: Icon = Icon("\u{ea70}");
pub const EYE_CLOSED: Icon = Icon("\u{eae7}");
pub const FEEDBACK: Icon = Icon("\u{eb96}");
pub const FILE: Icon = Icon("\u{ea7b}");
pub const FILES: Icon = Icon("\u{eaf0}");
pub const FILTER: Icon = Icon("\u{eaf1}");
pub const FOLDER: Icon = Icon("\u{ea83}");
pub const FOLDER_OPENED: Icon = Icon("\u{eaf7}");
pub const GEAR: Icon = Icon("\u{eaf8}");
pub const GIT_BRANCH: Icon = Icon("\u{ec6f}");
pub const GIT_COMMIT: Icon = Icon("\u{eafc}");
pub const GLOBE: Icon = Icon("\u{eb01}");
pub const GO_TO_FILE: Icon = Icon("\u{ea94}");
pub const GRAPH: Icon = Icon("\u{eb03}");
pub const GROUP_BY_REF_TYPE: Icon = Icon("\u{eb97}");
pub const HISTORY: Icon = Icon("\u{ea82}");
pub const HOME: Icon = Icon("\u{eb06}");
pub const INBOX: Icon = Icon("\u{eb09}");
pub const INFO: Icon = Icon("\u{ea74}");
pub const ITALIC: Icon = Icon("\u{eb0d}");
pub const KEBAB_VERTICAL: Icon = Icon("\u{eb10}");
pub const LAYERS: Icon = Icon("\u{ebd2}");
pub const LAYOUT: Icon = Icon("\u{ebeb}");
pub const LAYOUT_CENTERED: Icon = Icon("\u{ebf7}");
pub const LAYOUT_PANEL: Icon = Icon("\u{ebf2}");
pub const LAYOUT_SIDEBAR_LEFT: Icon = Icon("\u{ebf3}");
pub const LAYOUT_SIDEBAR_RIGHT: Icon = Icon("\u{ebf4}");
pub const LIBRARY: Icon = Icon("\u{eb9c}");
pub const LIGHTBULB: Icon = Icon("\u{ea61}");
pub const LINK_EXTERNAL: Icon = Icon("\u{eb14}");
pub const LIST_TREE: Icon = Icon("\u{eb86}");
pub const LOADING: Icon = Icon("\u{eb19}");
pub const LOCATION: Icon = Icon("\u{eb1a}");
pub const MAIL: Icon = Icon("\u{eb1c}");
pub const MAP: Icon = Icon("\u{ec05}");
pub const MENU: Icon = Icon("\u{eb94}");
pub const MIC: Icon = Icon("\u{ec12}");
pub const MIC_FILLED: Icon = Icon("\u{ec1c}");
pub const MILESTONE: Icon = Icon("\u{eb20}");
pub const MORE: Icon = Icon("\u{ea7c}");
pub const NEW_FILE: Icon = Icon("\u{ea7f}");
pub const NEW_FOLDER: Icon = Icon("\u{ea80}");
pub const OPEN_PREVIEW: Icon = Icon("\u{eb28}");
pub const OUTPUT: Icon = Icon("\u{eb9d}");
pub const PIN: Icon = Icon("\u{eb2b}");
pub const PLAY: Icon = Icon("\u{eb2c}");
pub const PLUG: Icon = Icon("\u{eb2d}");
pub const PREVIEW: Icon = Icon("\u{eb2f}");
pub const PRIMITIVE_SQUARE: Icon = Icon("\u{ea72}");
pub const PULSE: Icon = Icon("\u{eb31}");
pub const RECORD: Icon = Icon("\u{eba7}");
pub const REDO: Icon = Icon("\u{ebb0}");
pub const REGEX: Icon = Icon("\u{eb38}");
pub const REMOTE: Icon = Icon("\u{eb3a}");
pub const REMOTE_EXPLORER: Icon = Icon("\u{eb39}");
pub const REPLACE: Icon = Icon("\u{eb3d}");
pub const REPO_CLONE: Icon = Icon("\u{eb3e}");
pub const ROBOT: Icon = Icon("\u{ec20}");
pub const ROCKET: Icon = Icon("\u{eb44}");
pub const SCREEN_FULL: Icon = Icon("\u{eb4c}");
pub const SCREEN_NORMAL: Icon = Icon("\u{eb4d}");
pub const SEARCH: Icon = Icon("\u{ea6d}");
pub const SEND: Icon = Icon("\u{ec0f}");
pub const SORT_PRECEDENCE: Icon = Icon("\u{eb55}");
pub const SOURCE_CONTROL: Icon = Icon("\u{ea68}");
pub const SPARKLE: Icon = Icon("\u{ec10}");
pub const SPLIT_HORIZONTAL: Icon = Icon("\u{eb56}");
pub const STAR_FULL: Icon = Icon("\u{eb59}");
pub const STOP_CIRCLE: Icon = Icon("\u{eba5}");
pub const SYMBOL_FILE: Icon = Icon("\u{eb60}");
pub const SYMBOL_METHOD: Icon = Icon("\u{ea8c}");
pub const SYNC: Icon = Icon("\u{ea77}");
pub const TERMINAL: Icon = Icon("\u{ea85}");
pub const THREE_BARS: Icon = Icon("\u{eb6a}");
pub const THUMBSDOWN: Icon = Icon("\u{eb6b}");
pub const THUMBSUP: Icon = Icon("\u{eb6c}");
pub const TOOLS: Icon = Icon("\u{eb6d}");
pub const TRASH: Icon = Icon("\u{ea81}");
pub const UNGROUP_BY_REF_TYPE: Icon = Icon("\u{eb98}");
pub const VM_ACTIVE: Icon = Icon("\u{eb79}");
pub const WARNING: Icon = Icon("\u{ea6c}");
pub const WAND: Icon = Icon("\u{ebcf}");
pub const WAND2: Icon = Icon("\u{ebcf}");
pub const WATCH: Icon = Icon("\u{eb7c}");
pub const WORD_WRAP: Icon = Icon("\u{eb80}");
pub const SETTINGS_GEAR: Icon = Icon("\u{eb51}");

/// Render one codicon glyph. The color inherits the surrounding text color
/// (set `.text_color(...)` on an ancestor); the glyph box is square because
/// codicon glyphs are designed on a square em box.
pub fn icon(glyph: Icon, size: f32) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .size(px(size))
        .items_center()
        .justify_center()
        .font_family(crate::theme::FONT_ICON)
        .text_size(px(size))
        .line_height(px(size))
        .child(glyph.0.to_string())
}
