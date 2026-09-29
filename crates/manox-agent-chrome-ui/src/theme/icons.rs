//! Chrome icon table. Each constant is an SVG asset path (`icons/…`) served
//! by `gpui-kit-assets`' `AllAssets` bundle; `icon(glyph, size)` renders one
//! as a `gpui_component::Icon` (the same SVG pipeline that
//! `PopupMenuItem::icon` / `Button::icon` consume). SVGs paint with the
//! surrounding text color (set `.text_color(...)` on an ancestor), matching
//! the color-inheritance semantics of the retired codicon glyph table.
//!
//! The guard test at the bottom pins every constant to the embedded bundle so
//! an upstream rename fails a test instead of rendering blank.

use gpui::{Styled as _, px};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icon(pub &'static str);

pub const ACCOUNT: Icon = Icon("icons/circle-user.svg");
pub const ADD: Icon = Icon("icons/plus.svg");
pub const ARCHIVE: Icon = Icon("icons/archive.svg");
pub const ARROW_LEFT: Icon = Icon("icons/arrow-left.svg");
pub const ARROW_RIGHT: Icon = Icon("icons/arrow-right.svg");
pub const CALENDAR: Icon = Icon("icons/calendar.svg");
pub const CHEVRON_DOWN: Icon = Icon("icons/chevron-down.svg");
pub const CHEVRON_RIGHT: Icon = Icon("icons/chevron-right.svg");
pub const CLOSE: Icon = Icon("icons/close.svg");
pub const CODE: Icon = Icon("icons/code.svg");
pub const COPY: Icon = Icon("icons/copy.svg");
pub const COMMENT_DISCUSSION: Icon = Icon("icons/messages-square.svg");
pub const EXTENSIONS: Icon = Icon("icons/puzzle.svg");
pub const FOLDER: Icon = Icon("icons/folder.svg");
pub const FOLDER_OPENED: Icon = Icon("icons/folder-open.svg");
pub const GLOBE: Icon = Icon("icons/globe.svg");
pub const HOME: Icon = Icon("icons/house.svg");
pub const LAYOUT_PANEL: Icon = Icon("icons/panel-bottom.svg");
pub const LAYOUT_SIDEBAR_LEFT: Icon = Icon("icons/panel-left.svg");
pub const LAYOUT_SIDEBAR_RIGHT: Icon = Icon("icons/panel-right.svg");
pub const LINK_EXTERNAL: Icon = Icon("icons/external-link.svg");
pub const MORE: Icon = Icon("icons/ellipsis.svg");
pub const PIN: Icon = Icon("icons/pin.svg");
pub const PLAY: Icon = Icon("icons/play.svg");
pub const ROBOT: Icon = Icon("icons/bot.svg");
pub const SEARCH: Icon = Icon("icons/search.svg");
pub const SETTINGS_GEAR: Icon = Icon("icons/settings.svg");
pub const SORT_PRECEDENCE: Icon = Icon("icons/arrow-down-wide-narrow.svg");
pub const SPLIT_HORIZONTAL: Icon = Icon("icons/columns-2.svg");
pub const SYMBOL_FILE: Icon = Icon("icons/file-text.svg");
pub const SYNC: Icon = Icon("icons/refresh-cw.svg");
pub const TERMINAL: Icon = Icon("icons/terminal.svg");
pub const TOOLS: Icon = Icon("icons/wrench.svg");
pub const TRASH: Icon = Icon("icons/trash.svg");
pub const WAND: Icon = Icon("icons/wand-sparkles.svg");
pub const WARNING: Icon = Icon("icons/triangle-alert.svg");

/// Render one chrome icon at an explicit square size. Color inherits the
/// surrounding text color, like any text element.
pub fn icon(glyph: Icon, size: f32) -> gpui_component::Icon {
    gpui_component::Icon::default().path(glyph.0).size(px(size))
}

/// Render one chrome icon at the consumer's default size — the shape expected
/// by menu/button icon slots (`PopupMenuItem::icon`, `Button::icon`).
pub fn menu_icon(glyph: Icon) -> gpui_component::Icon {
    gpui_component::Icon::default().path(glyph.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_constant_resolves_in_the_embedded_bundle() {
        // AllAssets embeds the full Lucide/GPUI Kit catalog; a miss here means
        // an upstream rename drifted the path and the icon would render blank
        // at runtime.
        for glyph in [
            ACCOUNT,
            ADD,
            ARCHIVE,
            ARROW_LEFT,
            ARROW_RIGHT,
            CALENDAR,
            CHEVRON_DOWN,
            CHEVRON_RIGHT,
            CLOSE,
            CODE,
            COMMENT_DISCUSSION,
            COPY,
            EXTENSIONS,
            FOLDER,
            FOLDER_OPENED,
            GLOBE,
            HOME,
            LAYOUT_PANEL,
            LAYOUT_SIDEBAR_LEFT,
            LAYOUT_SIDEBAR_RIGHT,
            LINK_EXTERNAL,
            MORE,
            PIN,
            PLAY,
            ROBOT,
            SEARCH,
            SETTINGS_GEAR,
            SORT_PRECEDENCE,
            SPLIT_HORIZONTAL,
            SYMBOL_FILE,
            SYNC,
            TERMINAL,
            TOOLS,
            TRASH,
            WAND,
            WARNING,
        ] {
            assert!(
                gpui_kit_assets::AllAssets::get(glyph.0).is_some(),
                "icon asset missing from the embedded bundle: {}",
                glyph.0
            );
        }
    }
}
