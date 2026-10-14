//! History-loading view: the main column's face while a reopened thread's
//! chat snapshot is still in flight (the fold holds no chat channel yet).
//! The sprite itself — the pixel meerkat, its grids and its clock — lives in
//! `ai_elements::Meerkat`; this page is the heading, the thread id, and the
//! 10-second self-clearing gate's timeout constant.

use std::time::Duration;

use ai_elements::Meerkat;
use gpui::prelude::*;
use gpui::{AnyElement, FontWeight, div};
use gpui_component::{Theme, v_flex};

use crate::i18n;

/// A reopened thread whose chat snapshot never lands (host error, missing
/// session) must not pin the loading page forever: after this long the gate
/// self-clears and the hero screen returns.
pub(crate) const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);

/// The full loading page: the animated meerkat, the heading, and the thread
/// id. No composer — the thread is not ready.
pub(crate) fn render_history_loading(theme: &Theme, thread_id: &str) -> AnyElement {
    v_flex()
        .flex_1()
        .w_full()
        .relative()
        .overflow_hidden()
        .debug_selector(|| "history-loading".into())
        .bg(theme.background)
        .child(
            v_flex()
                .absolute()
                .inset_0()
                .justify_center()
                .items_center()
                .gap_4()
                .child(Meerkat::new())
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(i18n::t("workspace-history-loading-heading")),
                )
                .child(
                    div()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(thread_id.to_string()),
                ),
        )
        .into_any_element()
}
