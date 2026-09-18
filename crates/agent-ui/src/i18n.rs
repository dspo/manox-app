//! GPUI-aware i18n helpers — thin wrappers over `manox_i18n` that return
//! `gpui::SharedString` for use in gpui element/component APIs.
//!
//! This is app chrome only. Values handed over by the manox runtime (tool
//! titles/summaries, slash-command descriptions, plan prose, model output) are
//! rendered verbatim and never resolved here.

use gpui::SharedString;
pub use manox_i18n::{Language, current, set_ui_language};

/// Resolve `key` with no arguments as a `SharedString`.
pub fn t(key: &str) -> SharedString {
    manox_i18n::t(key).into()
}

/// Resolve `key` with string arguments as a `SharedString`.
pub fn t_str(key: &str, args: &[(&str, &str)]) -> SharedString {
    manox_i18n::t_str(key, args).into()
}

/// Resolve `key` with a numeric `$count` argument as a `SharedString`.
pub fn t_count(key: &str, count: i64) -> SharedString {
    manox_i18n::t_count(key, count).into()
}

/// Resolve `key` with string arguments plus a numeric `$count` as a `SharedString`.
pub fn t_str_count(key: &str, args: &[(&str, &str)], count: i64) -> SharedString {
    manox_i18n::t_str_count(key, args, count).into()
}
