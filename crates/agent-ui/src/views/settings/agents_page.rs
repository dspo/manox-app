//! The Agents page — the read-only mirror of ZCode's SubagentsSection
//! (`packages/ui/src/settings/SubagentsSection.tsx`). A flat list of
//! fixed-height rows, pixel-matched to ZCode's `AgentListRow` (36px avatar,
//! name + model/tools badges, two-line clamped description); clicking a row
//! drills down to the form-style detail, every write control present but
//! DISABLED (read-only end to end).

use gpui::{
    AnyElement, Context, Pixels, StatefulInteractiveElement as _, div, prelude::*, px, rgb,
};
use gpui_component::{ActiveTheme as _, Disableable as _, Icon, Theme, checkbox::Checkbox, v_flex};
use gpui_kit_assets::IconName as AssetIcon;

use crate::i18n;
use crate::views::settings::{AgentRow, SettingsView};

/// Row metrics, ZCode-flavored.
const ROW_H: Pixels = px(78.);
const AVATAR: Pixels = px(36.);
const BADGE_H: Pixels = px(20.);
const FIELD_W: Pixels = px(320.);

/// The ZCode agent color-tag palette (display only; no def carries one).
const COLOR_DOTS: [&str; 8] = [
    "#FACC15", "#FB7185", "#FB923C", "#4ADE80", "#22D3EE", "#60A5FA", "#A78BFA", "#F472B6",
];

pub(crate) fn render(view: &mut SettingsView, cx: &mut Context<SettingsView>) -> AnyElement {
    let theme = cx.theme().clone();
    let selected = view.selected_agent.clone();
    match selected {
        Some(name) => detail(view, &name, &theme, cx),
        None => list(view, &theme, cx),
    }
}

// ── list ─────────────────────────────────────────────────────────────────

fn list(view: &SettingsView, theme: &Theme, cx: &mut Context<SettingsView>) -> AnyElement {
    // The lead first, then the registry order (built-ins, users, plugins).
    let captain = AgentRow {
        name: "Captain".into(),
        description: i18n::t("agents-captain-desc").to_string(),
        origin: "settings-origin-builtin",
        model: None,
        tools: Vec::new(),
        all_tools: true,
        system_prompt: view.captain_prompt.clone(),
    };
    let mut card = v_flex().overflow_hidden();
    let mut items: Vec<AgentRow> = Vec::with_capacity(view.agents.len() + 1);
    items.push(captain);
    items.extend(view.agents.iter().cloned());
    let entity = cx.entity();
    for (ix, row) in items.iter().enumerate() {
        if ix > 0 {
            card = card.child(div().h(px(1.)).w_full().bg(theme.border.opacity(0.5)));
        }
        let row = row.clone();
        let click_entity = entity.clone();
        let name = row.name.clone();
        card = card.child(
            div()
                .id(("agents-row", ix))
                .h(ROW_H)
                .w_full()
                .overflow_hidden()
                .px_4()
                .flex()
                .items_center()
                .gap_3()
                .hover(|this| this.bg(theme.secondary))
                .on_click(move |_ev, _window, cx: &mut gpui::App| {
                    click_entity.update(cx, |view, cx| {
                        view.selected_agent = Some(name.clone());
                        cx.notify();
                    });
                })
                .child(row_face(theme, &row)),
        );
    }
    v_flex()
        .w_full()
        .child(
            v_flex()
                .overflow_hidden()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .child(card),
        )
        .into_any_element()
}

/// The row face: avatar · (name + badges) / clamped description — the
/// ZCode `AgentListRow` left/mid columns. Write controls are omitted on the
/// row (nothing here is editable; the detail states that per field).
fn row_face(theme: &Theme, row: &AgentRow) -> gpui::Div {
    let model = row
        .model
        .clone()
        .unwrap_or_else(|| i18n::t("agents-model-inherit").to_string());
    let tools = if row.all_tools {
        i18n::t("agents-all-tools").to_string()
    } else {
        format!("{} {}", row.tools.len(), i18n::t("agents-tools-count"))
    };
    div()
        .flex()
        .items_center()
        .gap_3()
        .w_full()
        .child(avatar(theme))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .overflow_hidden()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child(row.name.clone()),
                        )
                        .child(badge(theme, model))
                        .child(badge(theme, tools)),
                )
                .child(
                    div()
                        .h(px(34.))
                        .overflow_hidden()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(row.description.clone()),
                ),
        )
}

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
            Icon::new(AssetIcon::Bot)
                .size_4()
                .text_color(theme.muted_foreground),
        )
}

fn badge(theme: &Theme, text: String) -> gpui::Div {
    div()
        .h(BADGE_H)
        .px_1p5()
        .flex_none()
        .flex()
        .items_center()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .text_size(px(11.))
        .text_color(theme.muted_foreground)
        .child(text)
}

// ── detail (drill-down; every write control DISABLED) ────────────────────

fn detail(
    view: &SettingsView,
    name: &str,
    theme: &Theme,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let row = view
        .agents
        .iter()
        .find(|row| row.name == name)
        .cloned()
        .unwrap_or_else(|| AgentRow {
            name: "Captain".into(),
            description: i18n::t("agents-captain-desc").to_string(),
            origin: "settings-origin-builtin",
            model: None,
            tools: Vec::new(),
            all_tools: true,
            system_prompt: view.captain_prompt.clone(),
        });
    let is_lead = row.name == "Captain";
    let body = if is_lead {
        view.captain_prompt.clone()
    } else {
        row.system_prompt.clone()
    };
    let model = row
        .model
        .clone()
        .unwrap_or_else(|| i18n::t("agents-model-inherit").to_string());

    // Field scaffolding: label over a disabled control.
    let label = |theme: &Theme, text: &'static str| {
        div()
            .text_size(px(13.))
            .text_color(theme.foreground)
            .child(i18n::t(text))
    };
    let input_box = |theme: &Theme, text: String| {
        div()
            .h(px(36.))
            .w(FIELD_W)
            .max_w_full()
            .px_3()
            .flex()
            .items_center()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_size(px(13.))
            .text_color(theme.foreground)
            .child(text)
    };
    let select_box = |theme: &Theme, text: String, wide: bool| {
        div()
            .h(px(36.))
            .when(wide, |this| this.w(FIELD_W))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .justify_between()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_size(px(13.))
            .text_color(theme.foreground)
            .child(text)
            .child(
                Icon::new(AssetIcon::ChevronDown)
                    .size_4()
                    .text_color(theme.muted_foreground),
            )
    };

    // 颜色标记: the palette dots, none selected (no def carries a color).
    let mut dots = div().flex().gap_2().items_center();
    for hex in COLOR_DOTS {
        dots = dots.child(
            div()
                .size_4()
                .rounded_full()
                .bg(rgb(u32::from_str_radix(&hex[1..], 16).unwrap_or(0x888888))),
        );
    }

    // 可用工具: select + hint; the 3-column checked grid only for custom sets.
    let mut tools_section = v_flex().gap_2().child(select_box(
        theme,
        if row.all_tools {
            i18n::t("agents-tools-all-label").to_string()
        } else {
            i18n::t("agents-tools-custom").to_string()
        },
        false,
    ));
    if !row.all_tools {
        tools_section = tools_section.child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(i18n::t("agents-tools-hint")),
        );
        let mut grid = v_flex()
            .gap_2()
            .p_3()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border);
        for chunk in row.tools.chunks(3) {
            let mut line = div().flex().gap(px(48.));
            for tool in chunk {
                line = line.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .w(px(150.))
                        .child(
                            Checkbox::new(format!("agents-tool-{tool}"))
                                .checked(true)
                                .disabled(true),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme.foreground)
                                .child(tool.clone()),
                        ),
                );
            }
            grid = grid.child(line);
        }
        tools_section = tools_section.child(grid);
    }

    // Footer: the write actions, all disabled — 取消 is the way back.
    let back_entity = cx.entity();
    let mut footer = div().flex().items_center().justify_between().pt_2();
    let mut right = div().flex().items_center().gap_2();
    if !is_lead {
        footer = footer.child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_size(px(13.))
                .text_color(theme.danger.opacity(0.5))
                .child(Icon::new(AssetIcon::Trash).size_4())
                .child(i18n::t("agents-delete")),
        );
    } else {
        footer = footer.child(div());
    }
    right = right
        .child(
            div()
                .h(px(32.))
                .px_3()
                .flex()
                .items_center()
                .rounded(theme.radius)
                .bg(theme.primary.opacity(0.5))
                .text_size(px(13.))
                .text_color(theme.primary_foreground.opacity(0.7))
                .child(i18n::t("agents-save")),
        )
        .child(
            div()
                .id("agents-back")
                .h(px(32.))
                .px_3()
                .flex()
                .items_center()
                .rounded(theme.radius)
                .text_size(px(13.))
                .text_color(theme.foreground)
                .hover(|this| this.bg(theme.secondary))
                .on_click(move |_ev, _window, cx: &mut gpui::App| {
                    back_entity.update(cx, |view, cx| {
                        view.selected_agent = None;
                        cx.notify();
                    });
                })
                .child(i18n::t("agents-cancel")),
        );
    footer = footer.child(right);

    v_flex()
        .id("agents-detail")
        .w_full()
        .overflow_y_scroll()
        .child(
            // The scope face (作用域 · 用户), top right — display only.
            div().flex().justify_end().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child(i18n::t("agents-detail-scope")),
                    )
                    .child(select_box(
                        theme,
                        i18n::t("agents-detail-scope-user").to_string(),
                        false,
                    )),
            ),
        )
        .child(
            v_flex()
                .gap_5()
                .pt_4()
                .pb_6()
                .child(
                    v_flex()
                        .gap_2()
                        .child(label(theme, "agents-field-name"))
                        .child(input_box(theme, row.name.clone())),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(label(theme, "agents-field-color"))
                        .child(dots),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(label(theme, "agents-field-model"))
                        .child(select_box(theme, model, false)),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(label(theme, "agents-field-desc"))
                        .child(
                            div()
                                .min_h(px(64.))
                                .max_w_full()
                                .p_3()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.background)
                                .text_size(px(13.))
                                .text_color(theme.foreground)
                                .child(row.description.clone()),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(label(theme, "agents-field-tools"))
                        .child(tools_section),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(label(theme, "agents-field-prompt"))
                        .child(
                            div()
                                .id("agents-prompt-box")
                                .h(px(220.))
                                .max_w_full()
                                .overflow_y_scroll()
                                .p_3()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.background)
                                .text_size(px(13.))
                                .text_color(theme.foreground)
                                .child(body),
                        ),
                )
                .child(footer),
        )
        .into_any_element()
}
