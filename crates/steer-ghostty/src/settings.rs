//! The `[terminal]` slice of the manox `settings.toml` this surface
//! consumes: the shell override and the user's env pairs. Fonts / cursor /
//! bell / scrollback stay with the view's fixed theme (accepted migration
//! gaps).
//!
//! The file is shared with `manox_agent::settings` (which ignores the
//! `[terminal]` table — no `deny_unknown_fields`), so the two readers
//! coexist; the path honors `MANOX_HOME` the same way `manox_agent::paths`
//! does. Absent file / section / parse failure is non-fatal: defaults are
//! returned.

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalSettings {
    /// `[terminal].shell` override (`"/bin/zsh"`, `"vim"`…). `None` = the
    /// default user shell, spawned as a login shell.
    pub shell: Option<String>,
    /// `[terminal].env` pairs, layered OVER the container's creation
    /// snapshot and UNDER the process-inherited world.
    pub env: Vec<(String, String)>,
}

pub fn load() -> TerminalSettings {
    let Some(path) = settings_path() else {
        return TerminalSettings::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return TerminalSettings::default();
    };
    parse(&text)
}

/// `MANOX_HOME/settings.toml`, falling back to `$HOME/.manox/settings.toml`
/// — the same resolution `manox_agent::paths` performs, so embedders that
/// redirect the state root get the redirected settings too.
fn settings_path() -> Option<PathBuf> {
    let root = match std::env::var_os("MANOX_HOME") {
        Some(home) => PathBuf::from(home),
        None => std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".manox"))?,
    };
    Some(root.join("settings.toml"))
}

fn parse(text: &str) -> TerminalSettings {
    #[derive(serde::Deserialize)]
    struct Section {
        shell: Option<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
    }
    #[derive(serde::Deserialize)]
    struct File {
        terminal: Option<Section>,
    }
    let Ok(file) = toml::from_str::<File>(text) else {
        return TerminalSettings::default();
    };
    match file.terminal {
        Some(section) => TerminalSettings {
            shell: section.shell,
            env: section.env,
        },
        None => TerminalSettings::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn absent_section_yields_defaults() {
        let settings = parse("ui_language = \"zh-CN\"\n");
        assert_eq!(settings.shell, None);
        assert!(settings.env.is_empty());
    }

    #[test]
    fn shell_and_env_parse_from_the_table() {
        let settings = parse(
            "[terminal]\nshell = \"/bin/bash\"\nenv = [[\"RUST_LOG\", \"debug\"], [\"FOO\", \"bar\"]]\n",
        );
        assert_eq!(settings.shell.as_deref(), Some("/bin/bash"));
        assert_eq!(
            settings.env,
            vec![
                ("RUST_LOG".to_string(), "debug".to_string()),
                ("FOO".to_string(), "bar".to_string()),
            ]
        );
    }

    #[test]
    fn malformed_text_is_non_fatal() {
        assert_eq!(super::parse("not [valid toml !!!"), Default::default());
    }
}
