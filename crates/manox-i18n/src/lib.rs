//! manox-app's own UI-chrome localization.
//!
//! This crate owns **app chrome text only**. Simplified Chinese is the primary
//! language; English is the fallback, consulted whenever the primary bundle
//! lacks a key. Anything the manox runtime hands this app — tool
//! titles/summaries, slash-command descriptions, plan prose, model output — is
//! rendered verbatim and never resolved through these bundles, so the runtime
//! stays free of localization concerns entirely.
//!
//! The UI locale is runtime-switchable: [`set_ui_language`] swaps the global
//! and invalidates each thread's cached bundle, so a user picking a language in
//! settings sees chrome re-localize on the next render. Already-produced
//! content and user text are never retroactively rewritten.

mod settings;

use std::cell::RefCell;
use std::sync::RwLock;

use anyhow::Result;
use fluent::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

pub use settings::{load_ui_language, persist_ui_language};

const EN_FTL: &str = include_str!("../locales/en.ftl");
const ZH_CN_FTL: &str = include_str!("../locales/zh-CN.ftl");

/// The languages manox-app chrome speaks. `Copy` so it threads through render
/// call sites by value with no lifetime plumbing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Language {
    /// Primary language: Simplified Chinese.
    #[default]
    ZhCn,
    /// Fallback language.
    En,
}

impl Language {
    /// The canonical token written to `settings.toml` (`"zh-CN"` / `"en"`).
    /// Stable across builds; changing a token is a breaking config change.
    pub const fn token(self) -> &'static str {
        match self {
            Language::ZhCn => "zh-CN",
            Language::En => "en",
        }
    }

    /// Parse a config token. Accepts only the canonical [`Self::token`] forms;
    /// any other string is `None` so the caller can warn and fall back, rather
    /// than silently re-mapping an unrelated tag.
    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim() {
            "zh-CN" => Some(Language::ZhCn),
            "en" => Some(Language::En),
            _ => None,
        }
    }

    /// Endonym shown verbatim in the settings dropdown. Fixed per language and
    /// never re-localized, so the picker reads `简体中文` / `English` regardless
    /// of which language the UI is currently in.
    pub const fn endonym(self) -> &'static str {
        match self {
            Language::ZhCn => "简体中文",
            Language::En => "English",
        }
    }

    /// The other selectable language, for populating the picker.
    pub const fn alternate(self) -> Self {
        match self {
            Language::ZhCn => Language::En,
            Language::En => Language::ZhCn,
        }
    }

    /// BCP47 langid for Fluent bundle construction.
    fn langid(self) -> LanguageIdentifier {
        match self {
            Language::ZhCn => "zh-CN".parse().expect("zh-CN is a valid BCP47 langid"),
            Language::En => "en".parse().expect("en is a valid BCP47 langid"),
        }
    }
}

struct L10n {
    bundle: FluentBundle<FluentResource>,
    /// English fallback consulted only when `bundle` lacks the key. Fluent
    /// rejects duplicate message ids within a single bundle, so fallback is a
    /// separate bundle rather than a second resource in the primary one.
    fallback: Option<FluentBundle<FluentResource>>,
}

/// Current UI locale. `RwLock` (not `OnceLock`) so [`set_ui_language`] can swap
/// it at runtime; `FluentBundle` itself is `!Send` (its intl-memoizer holds a
/// `RefCell`), so each thread's bundle lives in a thread-local and is rebuilt
/// lazily against whatever locale this global reports when the thread next calls
/// [`t`].
static LANG: RwLock<Language> = RwLock::new(Language::ZhCn);

thread_local! {
    static L10N: RefCell<Option<L10n>> = const { RefCell::new(None) };
}

/// Read `~/.manox/settings.toml`, resolve the UI locale, and build the bundle
/// on the calling (startup) thread. Called once before any UI render. Any
/// failure is non-fatal: warn and fall back to Simplified Chinese so a
/// malformed config never blocks startup.
pub fn init() {
    let lang = load_ui_language();
    *LANG.write().expect("i18n LANG lock poisoned") = lang;
    if init_with(lang).is_err() {
        tracing::warn!("i18n bundle build failed for {lang:?}; falling back to zh-CN");
        *LANG.write().expect("i18n LANG lock poisoned") = Language::ZhCn;
        let _ = init_with(Language::ZhCn);
    }
}

/// Swap the UI locale in memory: update the global, drop every thread's
/// cached bundle so the next [`t`] rebuilds against the new locale.
/// Callers must refresh windows and rebuild native menus after this call
/// (both are gpui-dependent and live in `agent-ui` / the app binary).
pub fn set_ui_language(lang: Language) {
    *LANG.write().expect("i18n LANG lock poisoned") = lang;
    // Drop this thread's bundle; other threads' bundles rebuild lazily on
    // their next `t()` call, reading the new locale from `LANG`.
    L10N.with(|cell| *cell.borrow_mut() = None);
}

fn primary_resource(lang: Language) -> &'static str {
    match lang {
        Language::ZhCn => ZH_CN_FTL,
        Language::En => EN_FTL,
    }
}

fn init_with(lang: Language) -> Result<()> {
    let primary = FluentResource::try_new(primary_resource(lang).to_string())
        .map_err(|(_, errs)| anyhow::anyhow!("primary locale resource parse failed: {errs:?}"))?;
    let mut bundle = FluentBundle::new(vec![lang.langid()]);
    // This is a code editor (LTR); the bidi isolate marks fluent wraps
    // around variables would leak U+2068/2069 into rendered strings and break
    // substring matching in tests / copy-paste.
    bundle.set_use_isolating(false);
    bundle
        .add_resource(primary)
        .map_err(|errs| anyhow::anyhow!("primary resource add errors: {errs:?}"))?;
    // English fallback as a separate bundle — fluent rejects duplicate
    // message ids within one bundle, so a partial translation defers to en
    // via a lookup chain rather than a second resource in the primary bundle.
    let fallback = if lang != Language::En {
        let res = FluentResource::try_new(EN_FTL.to_string())
            .map_err(|(_, errs)| anyhow::anyhow!("en fallback resource parse failed: {errs:?}"))?;
        let mut fb = FluentBundle::new(vec![Language::En.langid()]);
        fb.set_use_isolating(false);
        fb.add_resource(res)
            .map_err(|errs| anyhow::anyhow!("fallback resource add errors: {errs:?}"))?;
        Some(fb)
    } else {
        None
    };
    let l10n = L10n { bundle, fallback };
    L10N.with(|cell| *cell.borrow_mut() = Some(l10n));
    Ok(())
}

/// Build the thread-local bundle for the current thread if not already built,
/// or rebuild it if the cached locale drifts from the live global (e.g. after
/// [`set_ui_language`] on another thread). Returns the resolved locale.
fn ensure_init() -> Language {
    let lang = read_lang();
    L10N.with(|cell| {
        let needs_rebuild = cell
            .borrow()
            .as_ref()
            .is_none_or(|l| l.bundle.locales.first() != Some(&lang.langid()));
        if needs_rebuild && let Err(e) = init_with(lang) {
            // A corrupt .ftl would otherwise fall through to key-verbatim
            // rendering in `format` with no trace; surface it here so the
            // failure is diagnosable in the field.
            tracing::warn!(error = %e, ?lang, "i18n bundle rebuild failed");
        }
    });
    lang
}

/// Current UI language. Returns [`Language::default`] (Simplified Chinese)
/// before `init` or on lock poisoning, so callers during early startup still
/// get a sane answer.
pub fn current() -> Language {
    read_lang()
}

/// Read the live UI locale, falling back to Simplified Chinese on a poisoned
/// lock (a panicking lock holder is unrecoverable, so defaulting keeps startup
/// robust rather than propagating the poisoning).
fn read_lang() -> Language {
    LANG.read().map(|g| *g).unwrap_or(Language::ZhCn)
}

/// Resolve `key` with no arguments. Missing keys render as the key itself so
/// leaks surface during development rather than silently empty strings.
pub fn t(key: &str) -> String {
    format(key, None)
}

/// Resolve `key` with string arguments (e.g. `workspace-unknown-command`'s
/// `$name`). Arguments borrow the caller's slices for the duration of the call.
pub fn t_str(key: &str, args: &[(&str, &str)]) -> String {
    let mut fa = FluentArgs::new();
    for (k, v) in args {
        fa.set(*k, FluentValue::String(std::borrow::Cow::Borrowed(*v)));
    }
    format(key, Some(&fa))
}

/// Resolve `key` with a numeric `$count` argument, used for plural-aware
/// strings like relative time formatting.
pub fn t_count(key: &str, count: i64) -> String {
    let mut fa = FluentArgs::new();
    fa.set("count", FluentValue::from(count));
    format(key, Some(&fa))
}

/// Resolve `key` with string arguments plus a numeric `$count`, for
/// plural-aware messages that also carry named string args.
pub fn t_str_count(key: &str, args: &[(&str, &str)], count: i64) -> String {
    let mut fa = FluentArgs::new();
    for (k, v) in args {
        fa.set(*k, FluentValue::String(std::borrow::Cow::Borrowed(*v)));
    }
    fa.set("count", FluentValue::from(count));
    format(key, Some(&fa))
}

fn format(key: &str, args: Option<&FluentArgs>) -> String {
    ensure_init();
    L10N.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(l10n) = guard.as_mut() else {
            return key.to_string();
        };
        if let Some(s) = format_in(&mut l10n.bundle, key, args) {
            return s;
        }
        if let Some(fb) = l10n.fallback.as_mut()
            && let Some(s) = format_in(fb, key, args)
        {
            return s;
        }
        key.to_string()
    })
}

/// Format `key` against a single bundle. Returns `None` if the key is absent
/// or the message has no value (so the caller can try the fallback bundle).
fn format_in(
    bundle: &mut FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs>,
) -> Option<String> {
    let msg = bundle.get_message(key)?;
    let value = msg.value()?;
    let mut errors = vec![];
    let formatted = bundle.format_pattern(value, args, &mut errors);
    if !errors.is_empty() {
        tracing::warn!(key, ?errors, "fluent format errors");
    }
    Some(formatted.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `t` / `t_count` read the process-global `LANG`, so any test that flips
    /// it must hold this lock for its whole body — otherwise a parallel sibling
    /// reassigning `LANG` mid-call makes `ensure_init` rebuild against the
    /// wrong locale and the assertion sees the other language's string.
    static TEST_LANG_LOCK: Mutex<()> = Mutex::new(());

    fn set_lang(lang: Language) {
        *LANG.write().expect("i18n LANG lock poisoned") = lang;
        init_with(lang).expect("init_with should succeed for tests");
    }

    #[test]
    fn missing_key_returns_key() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::ZhCn);
        assert_eq!(t("does-not-exist-xyz").as_str(), "does-not-exist-xyz");
    }

    #[test]
    fn default_language_is_simplified_chinese() {
        assert_eq!(Language::default(), Language::ZhCn);
        assert_eq!(Language::default().token(), "zh-CN");
    }

    #[test]
    fn token_round_trip() {
        for lang in [Language::ZhCn, Language::En] {
            assert_eq!(Language::from_token(lang.token()), Some(lang));
        }
    }

    #[test]
    fn from_token_rejects_non_canonical() {
        // Lenient variants (`zh`, `zh-Hans`, `en-US`) are intentionally not
        // accepted — the config token is fixed to the canonical form so a typo
        // surfaces at load rather than coercing silently.
        assert_eq!(Language::from_token("zh"), None);
        assert_eq!(Language::from_token("zh-Hans"), None);
        assert_eq!(Language::from_token("en-US"), None);
        assert_eq!(Language::from_token(""), None);
    }

    #[test]
    fn endonym_is_fixed_per_language() {
        assert_eq!(Language::ZhCn.endonym(), "简体中文");
        assert_eq!(Language::En.endonym(), "English");
    }

    #[test]
    fn current_reflects_set_language() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::En);
        assert_eq!(current(), Language::En);
        set_lang(Language::ZhCn);
        assert_eq!(current(), Language::ZhCn);
    }

    /// Chrome strings must resolve to actual prose in both locales — this is
    /// the regression gate for "a key was dropped during the move out of the
    /// manox runtime", which would otherwise render the raw key in the UI.
    #[test]
    fn chrome_keys_resolve_in_both_locales() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        let keys = [
            "sidebar-new-chat",
            "workspace-input-placeholder",
            "menu-quit",
            "settings-row-ui-language",
            "workspace-rename-confirm",
            "message-fork-here",
            "message-fork-unavailable-not-landed",
            "message-fork-unavailable-mid-turn",
            "message-fork-unavailable-not-replayed",
        ];
        for key in keys {
            set_lang(Language::ZhCn);
            let zh = t(key);
            assert_ne!(zh.as_str(), key, "missing zh-CN copy for {key}");
            assert!(
                zh.chars().any(|c| c as u32 > 0x2E80),
                "{key} is not Chinese in the primary locale: {zh}"
            );

            set_lang(Language::En);
            let en = t(key);
            assert_ne!(en.as_str(), key, "missing en copy for {key}");
            assert!(
                en.is_ascii(),
                "{key} is not English in the fallback locale: {en}"
            );
        }
    }

    /// The two bundles must carry the same key set. Historically this invariant
    /// was maintained by hand; a key added to one file only would silently
    /// render the raw key in the other locale, so it is pinned here.
    #[test]
    fn locale_key_sets_are_identical() {
        fn keys(src: &str) -> std::collections::BTreeSet<&str> {
            src.lines()
                .filter_map(|l| {
                    let (k, _) = l.split_once('=')?;
                    let k = k.trim();
                    (!k.is_empty() && k.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
                        .then_some(k)
                })
                .collect()
        }
        let zh = keys(ZH_CN_FTL);
        let en = keys(EN_FTL);
        let only_zh: Vec<_> = zh.difference(&en).collect();
        let only_en: Vec<_> = en.difference(&zh).collect();
        assert!(
            only_zh.is_empty() && only_en.is_empty(),
            "locale key sets drifted — missing in en: {only_zh:?}; missing in zh-CN: {only_en:?}"
        );
        assert!(!zh.is_empty(), "no keys parsed out of the zh-CN bundle");
    }

    #[test]
    fn en_plural_minutes() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::En);
        assert_eq!(t_count("sidebar-time-minutes", 1).as_str(), "1 minute ago");
        assert_eq!(t_count("sidebar-time-minutes", 5).as_str(), "5 minutes ago");
    }

    #[test]
    fn zh_cn_no_plural_minutes() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::ZhCn);
        assert_eq!(t_count("sidebar-time-minutes", 1).as_str(), "1 分钟前");
        assert_eq!(t_count("sidebar-time-minutes", 5).as_str(), "5 分钟前");
    }

    #[test]
    fn string_arg_interpolation() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::En);
        let v = t_str("workspace-unknown-command", &[("name", "foo")]);
        assert!(v.contains("/foo"), "got: {v}");
        assert!(v.contains("Unknown command"), "got: {v}");
    }

    /// The permission-mode notice must resolve all three variants in both
    /// locales: the workspace posts it on every mode switch, and a missing
    /// variant renders the raw fluent term. Pin the shapes so a rename or a
    /// dropped translation fails loudly here.
    #[test]
    fn permission_mode_notice_localized() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::En);
        let readonly = t_str("workspace-mode-notice", &[("mode", "readonly")]);
        assert!(readonly.contains("Read Only"), "got: {readonly}");
        let workspace = t_str("workspace-mode-notice", &[("mode", "workspacewrite")]);
        assert!(workspace.contains("Workspace Access"), "got: {workspace}");
        let full = t_str("workspace-mode-notice", &[("mode", "dangerfullaccess")]);
        assert!(full.contains("Full Access"), "got: {full}");
        set_lang(Language::ZhCn);
        let readonly = t_str("workspace-mode-notice", &[("mode", "readonly")]);
        assert!(readonly.contains("只读"), "got: {readonly}");
        let workspace = t_str("workspace-mode-notice", &[("mode", "workspacewrite")]);
        assert!(workspace.contains("工作区访问"), "got: {workspace}");
        let full = t_str("workspace-mode-notice", &[("mode", "dangerfullaccess")]);
        assert!(full.contains("完全访问"), "got: {full}");
    }

    /// Both directions of the plan chip's pending state must resolve in both
    /// bundles. `plan_mode_pending` is directionless (`enabled != committed`),
    /// so a queued switch-on and a queued exit need separate copy — one label
    /// cannot describe both. Missing from `en.ftl` a term renders the raw key;
    /// missing from `zh-CN.ftl` it renders the English fallback.
    #[test]
    fn plan_chip_pending_localized() {
        let _g = TEST_LANG_LOCK.lock().unwrap();
        set_lang(Language::En);
        assert_eq!(
            t("plan-chip-pending-enter-label").as_str(),
            "Plan mode pending"
        );
        assert!(t("plan-chip-pending-enter-tooltip").contains("next turn"));
        assert!(t("plan-chip-pending-exit-label").contains("exiting"));
        assert!(t("plan-chip-pending-exit-tooltip").contains("stay blocked"));
        assert!(t("plan-mode-cancel-notice").contains("stays writable"));
        set_lang(Language::ZhCn);
        assert!(t("plan-chip-pending-enter-label").contains("待生效"));
        assert!(t("plan-chip-pending-enter-tooltip").contains("下一轮"));
        assert!(t("plan-chip-pending-exit-label").contains("待退出"));
        assert!(t("plan-chip-pending-exit-tooltip").contains("写权限"));
        assert!(t("plan-mode-cancel-notice").contains("保持可写"));
    }
}
