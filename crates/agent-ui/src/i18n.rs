//! GPUI-aware i18n helpers — thin wrappers over `steer_i18n` that return
//! `gpui::SharedString` for use in gpui element/component APIs.
//!
//! This is app chrome only. Values handed over by the manox runtime (tool
//! titles/summaries, slash-command descriptions, plan prose, model output) are
//! rendered verbatim and never resolved here.

use gpui::SharedString;
pub use steer_i18n::{Language, current, set_ui_language};

/// Resolve `key` with no arguments as a `SharedString`.
pub fn t(key: &str) -> SharedString {
    steer_i18n::t(key).into()
}

/// Resolve `key` with string arguments as a `SharedString`.
pub fn t_str(key: &str, args: &[(&str, &str)]) -> SharedString {
    steer_i18n::t_str(key, args).into()
}

/// Resolve `key` with a numeric `$count` argument as a `SharedString`.
pub fn t_count(key: &str, count: i64) -> SharedString {
    steer_i18n::t_count(key, count).into()
}

/// Resolve `key` with string arguments plus a numeric `$count` as a `SharedString`.
pub fn t_str_count(key: &str, args: &[(&str, &str)], count: i64) -> SharedString {
    steer_i18n::t_str_count(key, args, count).into()
}

/// A duration in seconds as localized h/m/s with leading zero units dropped:
/// `h>0` renders all three, `h=0,m>0` minute plus second, and a sub-minute
/// duration seconds alone (the second is always shown). Negative input clamps
/// to zero.
pub fn t_duration(secs: i64) -> SharedString {
    let secs = secs.max(0);
    let h = (secs / 3600).to_string();
    let m = ((secs % 3600) / 60).to_string();
    let s = (secs % 60).to_string();
    let key = if h != "0" {
        "duration-hms"
    } else if m != "0" {
        "duration-ms"
    } else {
        "duration-s"
    };
    t_str(key, &[("h", &h), ("m", &m), ("s", &s)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `t_duration` resolves through the process-global locale, so tests that
    /// flip it hold this lock for their whole body.
    static TEST_LANG_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn duration_drops_leading_zero_units() {
        let _g = TEST_LANG_LOCK.lock().unwrap();

        set_ui_language(Language::ZhCn);
        assert_eq!(t_duration(0).to_string(), "0秒");
        assert_eq!(t_duration(45).to_string(), "45秒");
        assert_eq!(t_duration(125).to_string(), "2分5秒");
        assert_eq!(t_duration(7385).to_string(), "2小时3分5秒");
        assert_eq!(t_duration(-5).to_string(), "0秒");

        set_ui_language(Language::En);
        assert_eq!(t_duration(0).to_string(), "0s");
        assert_eq!(t_duration(45).to_string(), "45s");
        assert_eq!(t_duration(125).to_string(), "2m 5s");
        assert_eq!(t_duration(7385).to_string(), "2h 3m 5s");

        set_ui_language(Language::ZhCn);
    }
}
