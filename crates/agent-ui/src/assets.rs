//! Asset source overlaying manox-specific SVG icons on top of
//! `gpui-kit-assets`（原 `gpui-component-assets`，0.6 更名）。
//!
//! `gpui-kit-assets` ships the icon set `IconName` resolves to, but it
//! cannot carry manox's own brand icons (the Manox / Claude / Codex / GitHub Copilot
//! marks used in the sidebar and the new-session menu). `ExtrasAssetSource`
//! layers those on top: a `rust-embed` lookup of `assets/icons/**` wins, then
//! it falls through to `gpui-kit-assets` for everything else. The fallback is
//! `AllAssets` — the full Lucide catalog — not the default 101-icon subset,
//! so every `IconName` variant resolves at runtime (the chrome icon table
//! relies on names outside the subset). The manox
//! bin registers it via `with_assets`, so any `gpui::svg().path("icons/…")`
//! call site resolves through here.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use gpui_kit_assets::AllAssets as ComponentAssets;
use rust_embed::RustEmbed;

/// Embedded manox-local SVG assets (brand icons not in gpui-component).
#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct LocalAssets;

/// Hybrid asset source: manox-local SVGs first, then `gpui-kit-assets`
/// for the shared icon set. Mirrors the gpui-manos-assets pattern.
pub struct ExtrasAssetSource;

impl ExtrasAssetSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ExtrasAssetSource {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetSource for ExtrasAssetSource {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = LocalAssets::get(path) {
            return Ok(Some(file.data));
        }
        ComponentAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let path = path.trim_matches('/');
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };

        let mut children: Vec<SharedString> = Vec::new();

        for asset_path in LocalAssets::iter() {
            if !asset_path.starts_with(&prefix) {
                continue;
            }
            let rest = &asset_path[prefix.len()..];
            let name = rest.split('/').next().unwrap_or(rest);
            if !children.iter().any(|item| item.as_ref() == name) {
                children.push(SharedString::from(name.to_string()));
            }
        }

        if let Ok(component_children) = ComponentAssets.list(path) {
            for child in component_children {
                if !children.iter().any(|item| item.as_ref() == child.as_ref()) {
                    children.push(child);
                }
            }
        }

        Ok(children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_every_new_session_brand_icon() {
        for path in [
            "icons/manox.svg",
            "icons/claude.svg",
            "icons/codex.svg",
            "icons/githubcopilot.svg",
            "icons/terminal.svg",
        ] {
            assert!(LocalAssets::get(path).is_some(), "missing {path}");
        }
    }

    #[test]
    fn embeds_context_rail_branch_and_worktree_glyphs() {
        // Rail glyphs resolved via `ExtrasAssetSource`. Local files shadow the
        // kit fallback: since the fallback is `AllAssets` (full catalog), a
        // name that exists in both silently renders the kit drawing here, so
        // keeping a local copy is only meaningful for drawings we own.
        for path in ["icons/git-branch.svg", "icons/workflow.svg"] {
            assert!(LocalAssets::get(path).is_some(), "missing {path}");
        }
    }

    #[test]
    fn embeds_custom_icon_overrides() {
        // Local copies for names we render via `Icon::default().path(…)`.
        // Most of these also exist in the kit's full catalog; the local file
        // shadows it, so a removal here silently switches the drawing to the
        // upstream Lucide one (and a name unique to this bundle would render
        // blank when missing).
        for path in [
            "icons/circle-check-big.svg",
            "icons/check-check.svg",
            "icons/download.svg",
            "icons/ship-wheel.svg",
            "icons/corner-right-up.svg",
            "icons/zodiac-scorpio.svg",
            "icons/blocks.svg",
            "icons/panel-right-dashed.svg",
            "icons/grip-vertical.svg",
            "icons/image.svg",
            "icons/pencil.svg",
            // The row-menu glyphs: not in the gpui-kit-assets default
            // bundle, so the local layer is the only provider.
            "icons/pin.svg",
            "icons/archive.svg",
            "icons/archive-restore.svg",
            "icons/tag.svg",
            "icons/trash-2.svg",
            "icons/context-bubble-tail.svg",
        ] {
            assert!(LocalAssets::get(path).is_some(), "missing {path}");
        }
    }

    #[test]
    fn extras_load_resolves_chrome_table_and_known_literals() {
        // The production `load` path (local first, then the kit bundle) must
        // resolve every chrome icon and the repo's other literal icon paths —
        // falling back to the 101-icon `Assets` subset instead of `AllAssets`
        // would blank these out while every test that bypasses `load` stays
        // green.
        let extras = ExtrasAssetSource::new();
        let mut paths: Vec<&str> = manox_agent_chrome_ui::theme::icons::ALL
            .iter()
            .map(|glyph| glyph.0)
            .collect();
        paths.push("icons/square-pen.svg");
        for path in paths {
            match extras.load(path) {
                Ok(Some(_)) => {}
                other => panic!("{path} does not resolve through ExtrasAssetSource: {other:?}"),
            }
        }
    }
}
