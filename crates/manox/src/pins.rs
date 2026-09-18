//! Compile-time provenance of the stacks this binary links against, injected by
//! `build.rs`. An empty injection means the value could not be resolved for this
//! build (no git metadata, no lockfile) and renders as an absent About row.

/// Short form of a commit for display; an id too short to abbreviate is kept
/// whole rather than mangled.
pub fn short_commit(commit: &str) -> &str {
    commit.get(..SHORT_COMMIT_LEN).unwrap_or(commit)
}

const SHORT_COMMIT_LEN: usize = 8;

/// manox-app commit this binary was built from.
pub fn app_commit() -> Option<&'static str> {
    non_empty(option_env!("MANOX_APP_COMMIT"))
}

/// dspo/manox runtime commit compiled into this binary. The runtime reports the
/// source tree it was built from (the pinned rev under plain git dependencies,
/// the local checkout HEAD under `script/local-manox.sh on`); the lockfile rev
/// is the fallback for builds where that probe found no git metadata.
pub fn manox_commit() -> Option<&'static str> {
    non_empty(manox_agent::version::COMMIT_SHA)
        .or_else(|| non_empty(option_env!("MANOX_UPSTREAM_REV")))
}

/// The pinned `gpui-component` (GPUI Kit) dependency, as the About row shows it.
pub fn gpui_kit_pin() -> Option<String> {
    pin(
        option_env!("GPUI_COMPONENT_VERSION"),
        option_env!("GPUI_COMPONENT_SOURCE"),
    )
}

/// The pinned `gpui-pre` snapshot (the `gpui` package alias), as the About row
/// shows it.
pub fn gpui_pin() -> Option<String> {
    pin(option_env!("GPUI_VERSION"), option_env!("GPUI_SOURCE"))
}

/// A lockfile-resolved dependency as one display value: the version literal for
/// a registry release, so the row reads like the manifest entry it came from
/// (`gpui-component` / `"0.6.1"`); `owner/repo @ rev` when the dependency is a
/// git checkout, where the version alone would not say what was built.
fn pin(version: Option<&str>, source: Option<&str>) -> Option<String> {
    let version = non_empty(version)?;
    Some(match git_origin(source) {
        Some((repo, rev)) => format!("{repo} @ {rev}"),
        None => format!("\"{version}\""),
    })
}

/// `owner/repo` and the short rev of a `git+https://github.com/owner/repo…#rev`
/// lockfile source.
fn git_origin(source: Option<&str>) -> Option<(&str, &str)> {
    let rest = non_empty(source)?.strip_prefix("git+https://github.com/")?;
    let (repo, rev) = rest.rsplit_once('#')?;
    let repo = repo.split('?').next().unwrap_or(repo);
    Some((repo, short_commit(rev)))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lockfile is the only source of the pinned versions; a build script
    /// pointed at the wrong path or key would silently blank the About rows, so
    /// read it back here independently of the build script's own parsing.
    #[test]
    fn gpui_pins_match_lockfile() {
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .expect("workspace Cargo.lock must be readable");

        let locked_kit = locked_version(&lock, "gpui-component");
        let locked_gpui = locked_version(&lock, "gpui-pre");
        assert!(
            locked_kit.is_some() && locked_gpui.is_some(),
            "the lockfile must list both gpui crates"
        );
        assert_eq!(gpui_kit_pin(), locked_kit.map(|v| format!("\"{v}\"")));
        assert_eq!(gpui_pin(), locked_gpui.map(|v| format!("\"{v}\"")));
    }

    /// An empty injection means "unresolved" everywhere downstream, so no
    /// accessor may hand an empty string to the About rows.
    #[test]
    fn accessors_never_yield_an_empty_value() {
        assert!(app_commit() != Some(""));
        assert!(manox_commit() != Some(""));
        assert!(gpui_kit_pin() != Some(String::new()));
        assert!(gpui_pin() != Some(String::new()));
    }

    /// A git checkout has to name its origin: the version alone would not say
    /// which tree was built, and the rev would be unreadable in full.
    #[test]
    fn git_pins_carry_the_origin_repo() {
        assert_eq!(pin(None, None), None);
        assert_eq!(
            pin(Some(""), Some("registry+https://crates.io-index")),
            None
        );
        assert_eq!(
            pin(
                Some("0.6.1"),
                Some("registry+https://github.com/rust-lang/crates.io-index")
            ),
            Some("\"0.6.1\"".to_string())
        );
        assert_eq!(
            pin(
                Some("0.6.1"),
                Some(
                    "git+https://github.com/longbridge/gpui-kit?branch=main#6dc7649915ccd7bba9f841cfbb53616d9055ee91"
                )
            ),
            Some("longbridge/gpui-kit @ 6dc76499".to_string())
        );
        // A path-patched source records no origin, so the version stands alone.
        assert_eq!(pin(Some("0.6.1"), None), Some("\"0.6.1\"".to_string()));
    }

    #[test]
    fn short_commit_keeps_short_ids_whole() {
        assert_eq!(
            short_commit("6dc7649915ccd7bba9f841cfbb53616d9055ee91"),
            "6dc76499"
        );
        assert_eq!(short_commit("abc"), "abc");
    }

    /// The lockfile writes `version` on the line after `name` for every package.
    fn locked_version(lock: &str, name: &str) -> Option<String> {
        let needle = format!("name = \"{name}\"");
        let mut lines = lock.lines();
        while let Some(line) = lines.next() {
            if line.trim() == needle {
                return lines
                    .next()?
                    .trim()
                    .strip_prefix("version = \"")?
                    .strip_suffix('"')
                    .map(str::to_string);
            }
        }
        None
    }
}
