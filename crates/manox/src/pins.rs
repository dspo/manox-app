//! Compile-time provenance of the stacks this binary links against, injected by
//! `build.rs`. An empty injection means the value could not be resolved for this
//! build (no git metadata, no lockfile) and renders as an absent About row.

/// manox-app commit this binary was built from.
pub fn app_commit() -> Option<&'static str> {
    non_empty(option_env!("MANOX_APP_COMMIT"))
}

/// dspo/manox runtime commit compiled into this binary. The runtime reports the
/// source tree it was built from (the pinned rev under plain git dependencies,
/// the local checkout HEAD under `script/local-manox.sh on`); the lockfile rev
/// is the fallback for builds where that probe found no git metadata.
pub fn manox_commit() -> Option<&'static str> {
    manox_agent::version::COMMIT_SHA.or_else(|| non_empty(option_env!("MANOX_UPSTREAM_REV")))
}

/// Pinned `gpui-component` (GPUI Kit) version.
pub fn gpui_kit_version() -> Option<&'static str> {
    non_empty(option_env!("GPUI_COMPONENT_VERSION"))
}

/// Pinned `gpui-pre` snapshot version (the `gpui` package alias).
pub fn gpui_version() -> Option<&'static str> {
    non_empty(option_env!("GPUI_VERSION"))
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
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

        assert_eq!(
            gpui_kit_version(),
            locked_version(&lock, "gpui-component").as_deref()
        );
        assert_eq!(gpui_version(), locked_version(&lock, "gpui-pre").as_deref());
        assert!(
            gpui_kit_version().is_some(),
            "the gpui-component pin must resolve from the workspace lockfile"
        );
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
