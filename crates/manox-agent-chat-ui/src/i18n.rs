//! GPUI-aware i18n helpers for the chat crate — the same thin wrappers
//! agent-ui has over `manox_i18n`, returning `gpui::SharedString` for the
//! component APIs. App chrome only; runtime-provided values never resolve
//! through here.

use gpui::SharedString;

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
