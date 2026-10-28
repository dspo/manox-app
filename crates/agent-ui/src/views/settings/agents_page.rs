//! The Agents page — the read-only adaptation of ZCode's SubagentsSection
//! (`packages/ui/src/settings/SubagentsSection.tsx`): resource-group headers
//! with counts above rounded list cards; each row a 36px rounded-square
//! avatar, a name + badge line (model · tools), and a muted description.
//! The lead agent gets its own group whose card carries the REAL system
//! prompt (rendered by the runtime's own builder) instead of a summary.
//! Read-only end to end: no toggles, no edit affordances.

use gpui::{AnyElement, Context, Pixels, div, prelude::*, px};
use gpui_component::{ActiveTheme as _, Icon, Theme, h_flex, v_flex};
use gpui_kit_assets::IconName;

use crate::i18n;
use crate::views::settings::{AgentRow, SettingsView};

/// Row metrics, ZCode-flavored: 36px avatar, 20px badges, 14px group headers.
const AVATAR: Pixels = px(36.);
const BADGE_H: Pixels = px(20.);

pub(crate) fn render(view: &mut SettingsView, cx: &mut Context<SettingsView>) -> AnyElement {
    let theme = cx.theme().clone();

    let mut page = v_flex().w_full().gap_4();
    // The lead group: Captain's row + its real prompt preview in one card.
    page = page
        .child(group_header(
            &theme,
            i18n::t("agents-group-lead").to_string(),
            1,
        ))
        .child(captain_card(view, &theme));

    // Subagent groups by origin; empty discovery groups stay hidden.
    let mut builtin = Vec::new();
    let mut user = Vec::new();
    let mut plugin = Vec::new();
    for row in &view.agents {
        match row.origin {
            "settings-origin-builtin" => builtin.push(row),
            "settings-origin-plugin" => plugin.push(row),
            _ => user.push(row),
        }
    }
    for (title_key, rows) in [
        ("settings-origin-builtin", builtin),
        ("settings-origin-user", user),
        ("settings-origin-plugin", plugin),
    ] {
        if rows.is_empty() && title_key != "settings-origin-builtin" {
            continue;
        }
        let mut card = v_flex().overflow_hidden();
        for (ix, row) in rows.iter().enumerate() {
            if ix > 0 {
                card = card.child(div().h(px(1.)).w_full().bg(theme.border.opacity(0.5)));
            }
            card = card.child(agent_row(&theme, row));
        }
        page = page
            .child(group_header(
                &theme,
                i18n::t(title_key).to_string(),
                rows.len(),
            ))
            .child(list_card(&theme, card));
    }

    page = page.child(
        div()
            .pt_1()
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .child(i18n::t("agents-readonly-note")),
    );
    page.into_any_element()
}

/// A ZCode `SettingsResourceGroupHeader`: medium title + subtle count.
fn group_header(theme: &Theme, title: String, count: usize) -> gpui::Div {
    h_flex()
        .h(px(28.))
        .items_center()
        .gap_1p5()
        .child(
            div()
                .text_size(px(14.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(count.to_string()),
        )
}

/// The list card: rounded, bordered, on the surface tint.
fn list_card(theme: &Theme, rows: gpui::Div) -> gpui::Div {
    v_flex()
        .overflow_hidden()
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .child(rows)
}

/// A ZCode `AgentBadge`: a small ringed pill (model · tools).
fn badge(theme: &Theme, text: String) -> gpui::Div {
    h_flex()
        .h(BADGE_H)
        .px_1p5()
        .flex_none()
        .items_center()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_size(px(11.))
        .text_color(theme.muted_foreground)
        .child(text)
}

/// The 36px rounded-square avatar with the Bot glyph.
fn avatar(theme: &Theme) -> gpui::Div {
    div()
        .size(AVATAR)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius_lg)
        .bg(theme.background)
        .child(
            Icon::new(IconName::Bot)
                .size_4()
                .text_color(theme.muted_foreground),
        )
}

/// One subagent row: avatar · (name + badges) / description.
fn agent_row(theme: &Theme, row: &AgentRow) -> gpui::Div {
    let model = row
        .model
        .clone()
        .unwrap_or_else(|| i18n::t("agents-model-inherit").to_string());
    let tools = if row.all_tools {
        i18n::t("agents-all-tools").to_string()
    } else {
        format!("{} {}", row.tools, i18n::t("agents-tools-count"))
    };
    h_flex()
        .w_full()
        .px_4()
        .py_3()
        .gap_3()
        .items_start()
        .child(avatar(theme))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(px(2.))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child(row.name.clone()),
                        )
                        .child(badge(theme, model))
                        .child(badge(theme, tools)),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(row.description.clone()),
                ),
        )
}

/// The lead's card: the Captain row above its REAL system prompt — the
/// runtime-rendered stable base template in a scrollable mono block.
fn captain_card(view: &SettingsView, theme: &Theme) -> gpui::Div {
    let captain = AgentRow {
        name: "Captain".into(),
        description: i18n::t("agents-captain-desc").to_string(),
        origin: "settings-origin-builtin",
        model: None,
        tools: 0,
        all_tools: true,
    };
    let mut preview = v_flex()
        .id("captain-prompt-preview")
        .max_h(px(280.))
        .overflow_y_scroll()
        .p_3()
        .rounded(theme.radius)
        .bg(theme.background)
        .text_size(px(12.));
    for line in view.captain_prompt.lines() {
        preview = preview.child(
            div()
                .font_family("monospace")
                .text_color(theme.muted_foreground)
                .child(line.to_string()),
        );
    }
    v_flex()
        .px_4()
        .py_3()
        .gap_3()
        .child(agent_row(theme, &captain))
        .child(
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.muted_foreground)
                        .child(i18n::t("agents-prompt-preview")),
                )
                .child(preview),
        )
}
