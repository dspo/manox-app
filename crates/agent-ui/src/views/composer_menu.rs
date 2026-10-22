//! Composer pending-attachment + browser-suite chip support.
//!
//! A [`PendingAttachment`] is a file the user picked but has not yet sent. On submit the workspace
//! turns each into message content: images become base64 [`MessageContent::Image`] blocks, text
//! files are inlined into the message text wrapped in `<file>` tags. The browser-suite chips (the
//! persisted opt-in face) render here too. Slash-command typeahead (`/`) and `@` mentions
//! live in [`crate::views::completion`], not here.

use std::path::Path;

use crate::i18n;
use base64::Engine as _;
use gpui::{SharedString, prelude::*};
use gpui_component::{Icon, IconName, Sizable as _, Theme, h_flex, v_flex};
use manox_agent::language_model::MessageContent;
pub use steer_agent_chat_ui::column::PendingAttachment;

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// Read one on-disk attachment into a [`MessageContent`]. Images become base64
/// image blocks; text files are inlined into `text_out` wrapped in `<file>`
/// tags. Clipboard images are handled separately by the caller (resize) and
/// yield `None` here. Blocking file IO — call from a background executor.
pub fn load_attachment(att: &PendingAttachment, text_out: &mut String) -> Option<MessageContent> {
    let PendingAttachment::File { path, is_image } = att else {
        return None;
    };
    if *is_image {
        let bytes = std::fs::read(path).ok()?;
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Some(MessageContent::Image {
            data,
            mime_type: mime_for(path).to_string(),
        })
    } else {
        let content = std::fs::read_to_string(path).ok()?;
        text_out.push_str(&format!(
            "\n<file path=\"{}\">\n{content}\n</file>",
            path.display()
        ));
        None
    }
}

/// Render the pending-attachment chips shown above the composer. `on_remove(ix)` removes chip `ix`.
pub fn render_attachment_chips(
    attachments: &[PendingAttachment],
    theme: &Theme,
    on_remove: impl Fn(usize, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    let on_remove = std::rc::Rc::new(on_remove);
    let mut row = h_flex().w_full().flex_wrap().gap_1();
    for (ix, att) in attachments.iter().enumerate() {
        let on_remove = on_remove.clone();
        let icon = if att.is_image() {
            IconName::Palette
        } else {
            IconName::File
        };
        let name: SharedString = att.file_name().into();
        row = row.child(
            h_flex()
                .id(("attachment-chip", ix))
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .bg(theme.secondary)
                .border_1()
                .border_color(theme.border)
                .child(
                    Icon::new(icon.clone())
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    gpui::div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(name),
                )
                .child(
                    gpui::div()
                        .id(("attachment-remove", ix))
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        )
                        .on_click(move |_, window, cx| on_remove(ix, window, cx)),
                ),
        );
    }
    v_flex().w_full().child(row).into_any_element()
}

/// Display metadata for an active browser tool suite chip.
fn browser_chip_meta(suite: manox_agent::engine::BrowserSuite) -> (IconName, &'static str) {
    match suite {
        manox_agent::engine::BrowserSuite::WebExplore => {
            (IconName::Frame, "composer-add-internal-browser")
        }
    }
}

/// Render the active browser-tool-suite chips shown above the composer. These
/// persist across submits (unlike file attachments); `on_remove(ix)` removes
/// chip `ix` and deactivates its suite.
pub fn render_browser_chips(
    suites: &[manox_agent::engine::BrowserSuite],
    theme: &Theme,
    on_remove: impl Fn(usize, &mut gpui::Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    let on_remove = std::rc::Rc::new(on_remove);
    let mut row = h_flex().w_full().flex_wrap().gap_1();
    for (ix, suite) in suites.iter().enumerate() {
        let on_remove = on_remove.clone();
        let (icon, label_key) = browser_chip_meta(*suite);
        let name: SharedString = i18n::t(label_key);
        row = row.child(
            h_flex()
                .id(("browser-chip", ix))
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .bg(theme.secondary)
                .border_1()
                .border_color(theme.border)
                .child(
                    Icon::new(icon.clone())
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    gpui::div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(name),
                )
                .child(
                    gpui::div()
                        .id(("browser-chip-remove", ix))
                        .cursor_pointer()
                        .child(
                            Icon::new(IconName::Close)
                                .xsmall()
                                .text_color(theme.muted_foreground),
                        )
                        .on_click(move |_, window, cx| on_remove(ix, window, cx)),
                ),
        );
    }
    v_flex().w_full().child(row).into_any_element()
}
