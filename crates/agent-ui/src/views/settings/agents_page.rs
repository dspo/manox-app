//! The Agents page — the read-only mirror of ZCode's SubagentsSection
//! (`packages/ui/src/settings/SubagentsSection.tsx`). A vertical stack of
//! fixed-height item cards (one per agent, thin color-coded border, no card
//! fill); clicking an item drills down to the form-style detail, every write
//! control present but DISABLED (read-only end to end). The detail's top row
//! carries the back control; the system prompt flips between its rendered
//! markdown and the raw source.

use gpui::{
    AnyElement, Context, Hsla, Pixels, StatefulInteractiveElement as _, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, Theme, checkbox::Checkbox, input::Editor, v_flex,
};
use gpui_kit_assets::IconName as AssetIcon;
use steer_components::markdown::static_document;

use crate::i18n;
use crate::views::settings::{
    ACCENT_NAMES, AgentRow, PromptView, SettingsView, accent_by_name, accent_color, tile_avatar,
    tile_badge,
};

/// Row metrics, ZCode-flavored.
const ROW_H: Pixels = px(78.);
const ROW_GAP: Pixels = px(10.);
const FIELD_W: Pixels = px(320.);
const PROMPT_H: Pixels = px(280.);
/// One tool column is wide enough for the longest preset name
/// (`WebExploreScreenshot`).
const TOOL_COL_W: Pixels = px(190.);

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
        color: None,
        system_prompt: view.captain_prompt.clone(),
    };
    let mut items: Vec<AgentRow> = Vec::with_capacity(view.agents.len() + 1);
    items.push(captain);
    items.extend(view.agents.iter().cloned());
    let entity = cx.entity();
    let mut column = v_flex().w_full().gap(ROW_GAP);
    for (ix, row) in items.iter().enumerate() {
        // A definition that declares a color wears it; the rest cycle the
        // palette by position.
        let accent = row
            .color
            .as_deref()
            .and_then(accent_by_name)
            .unwrap_or_else(|| accent_color(ix));
        let row = row.clone();
        let click_entity = entity.clone();
        let name = row.name.clone();
        column = column.child(
            div()
                .id(("agents-row", ix))
                .min_h(ROW_H)
                .w_full()
                .overflow_hidden()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(accent.opacity(0.45))
                .px_4()
                .py_3()
                .flex()
                .items_center()
                .gap_3()
                .hover(move |this| this.border_color(accent))
                .on_click(move |_ev, window, cx: &mut gpui::App| {
                    click_entity.update(cx, |view, cx| {
                        view.open_agent(name.clone(), window, cx);
                        cx.notify();
                    });
                })
                .child(row_face(theme, &row, accent)),
        );
    }
    v_flex().w_full().child(column).into_any_element()
}

/// The item face: avatar · (name + badges) / clamped description — the
/// ZCode `AgentListRow` left/mid columns. Write controls are omitted on the
/// row (nothing here is editable; the detail states that per field).
fn row_face(theme: &Theme, row: &AgentRow, accent: Hsla) -> gpui::Div {
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
        .child(tile_avatar(theme, accent, Icon::new(AssetIcon::Bot)))
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
                        .child(tile_badge(theme, model.into()))
                        .child(tile_badge(theme, tools.into())),
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
            color: None,
            system_prompt: view.captain_prompt.clone(),
        });
    let is_lead = row.name == "Captain";
    let body = if is_lead {
        view.captain_prompt.clone()
    } else {
        row.system_prompt.clone()
    };
    // The Captain preview stays empty until a foreground thread gives the
    // builder a cwd — say so instead of painting a blank box.
    let prompt_empty = body.is_empty();
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

    // 颜色标记: the palette dots; the definition's declared color (if any)
    // is the one ringed. Display only — the page is read-only.
    let declared = row.color.as_deref().map(str::to_ascii_lowercase);
    let mut dots = div().flex().gap_2().items_center();
    for name in ACCENT_NAMES {
        let dot = accent_by_name(name).expect("palette names resolve");
        let selected = declared.as_deref() == Some(name);
        dots = dots.child(
            div()
                .size_5()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .when(selected, |this| {
                    this.border_1().border_color(theme.foreground)
                })
                .child(div().size_3().rounded_full().bg(dot)),
        );
    }

    // 可用工具: the full preset catalog, every box checked, grouped by the
    // job the tools do. The page is read-only, so this mirrors what the
    // runtime mounts rather than a per-agent subset; MCP, skill and plugin
    // tools are a different surface.
    let mut catalog = v_flex()
        .gap_4()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border);
    for (ix, (label_key, tools)) in preset_tool_groups().into_iter().enumerate() {
        if ix > 0 {
            catalog = catalog.child(div().h(px(1.)).w_full().bg(theme.border.opacity(0.6)));
        }
        let mut grid = v_flex().gap_2();
        for chunk in tools.chunks(3) {
            let mut line = div().flex().gap(px(24.));
            for tool in chunk {
                line = line.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .w(TOOL_COL_W)
                        .child(
                            Checkbox::new(format!("agents-tool-{tool}"))
                                .checked(true)
                                .disabled(true),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme.foreground)
                                .child(*tool),
                        ),
                );
            }
            grid = grid.child(line);
        }
        catalog = catalog.child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child(i18n::t(label_key)),
                )
                .child(grid),
        );
    }
    let tools_section = v_flex()
        .gap_2()
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(i18n::t("agents-tools-hint")),
        )
        .child(catalog)
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(i18n::t("agents-tools-preset-note")),
        );

    // 系统提示词 source/rendered toggle — a view choice, not a write.
    let prompt_toggle = prompt_toggle(view, theme, cx);

    // Back to the list: the top-left control; the footer 取消 mirrors it.
    let back_entity = cx.entity();
    let back = div()
        .id("agents-back")
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded(theme.radius)
        .text_size(px(13.))
        .text_color(theme.foreground)
        .hover(|this| this.bg(theme.secondary))
        .on_click(move |_ev, _window, cx: &mut gpui::App| {
            back_entity.update(cx, |view, cx| {
                view.close_agent();
                cx.notify();
            });
        })
        .child(Icon::new(AssetIcon::ArrowLeft).size_4())
        .child(i18n::t("agents-back"));

    // Footer: the write actions, all disabled — the top control (and 取消)
    // is the way back.
    let cancel_entity = cx.entity();
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
                .id("agents-cancel")
                .h(px(32.))
                .px_3()
                .flex()
                .items_center()
                .rounded(theme.radius)
                .text_size(px(13.))
                .text_color(theme.foreground)
                .hover(|this| this.bg(theme.secondary))
                .on_click(move |_ev, _window, cx: &mut gpui::App| {
                    cancel_entity.update(cx, |view, cx| {
                        view.close_agent();
                        cx.notify();
                    });
                })
                .child(i18n::t("agents-cancel")),
        );
    footer = footer.child(right);

    // The prompt body: rendered markdown by default, the read-only source
    // editor on demand. The editor owns its own scrolling; the preview sits
    // in the same fixed-height box so switching never jumps the page.
    let prompt_body = if prompt_empty {
        return_prompt_placeholder(theme)
    } else {
        match view.prompt_view {
            PromptView::Preview => {
                let id = gpui::ElementId::Name(format!("agents-prompt-{name}").into());
                div()
                    .id("agents-prompt-preview")
                    .h_full()
                    .w_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(static_document(id, body, theme, cx))
                    .into_any_element()
            }
            PromptView::Edit => div()
                .w_full()
                .overflow_hidden()
                .child(
                    Editor::new(&view.prompt_editor)
                        .h(PROMPT_H)
                        .appearance(false)
                        .bordered(false)
                        .readonly(true),
                )
                .into_any_element(),
        }
    };

    v_flex()
        .id("agents-detail")
        .w_full()
        .overflow_y_scroll()
        .child(
            // Top row: back to the list on the left, the scope face (作用域 ·
            // 用户) on the right — both display only.
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(back)
                .child(
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
                        // The def's own origin group (内置 / 用户 / 插件) —
                        // display only, so a pill rather than a select face.
                        .child(tile_badge(theme, i18n::t(row.origin))),
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
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(label(theme, "agents-field-prompt"))
                                .child(prompt_toggle),
                        )
                        .child(
                            div()
                                .id("agents-prompt-box")
                                .h(PROMPT_H)
                                .w_full()
                                .max_w_full()
                                .overflow_hidden()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.background)
                                .child(prompt_body),
                        ),
                )
                .child(footer),
        )
        .into_any_element()
}

/// The muted stand-in for a Captain prompt that cannot be built yet (no
/// foreground thread — the builder would claim a working directory no
/// session has).
fn return_prompt_placeholder(theme: &Theme) -> AnyElement {
    div()
        .h_full()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .p_3()
        .child(
            div()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(i18n::t("agents-prompt-empty")),
        )
        .into_any_element()
}

/// The 编辑 / 预览 segmented toggle for the system prompt.
fn prompt_toggle(view: &SettingsView, theme: &Theme, cx: &mut Context<SettingsView>) -> gpui::Div {
    let mut segment = div()
        .flex()
        .items_center()
        .h(px(26.))
        .p(px(2.))
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border);
    for (mode, key) in [
        (PromptView::Edit, "agents-prompt-edit"),
        (PromptView::Preview, "agents-prompt-preview"),
    ] {
        let active = view.prompt_view == mode;
        let entity = cx.entity();
        segment = segment.child(
            div()
                .id(("agents-prompt-view", mode as usize))
                .h_full()
                .px_2()
                .flex()
                .items_center()
                .rounded(theme.radius)
                .text_size(px(12.))
                .when(active, |this| {
                    this.bg(theme.secondary).text_color(theme.foreground)
                })
                .when(!active, |this| {
                    this.text_color(theme.muted_foreground)
                        .hover(|this| this.bg(theme.secondary))
                })
                .on_click(move |_ev, _window, cx: &mut gpui::App| {
                    entity.update(cx, |view, cx| {
                        if view.prompt_view != mode {
                            view.prompt_view = mode;
                            cx.notify();
                        }
                    });
                })
                .child(i18n::t(key)),
        );
    }
    segment
}

/// The preset (built-in) tool catalog, grouped by the job the tools do.
/// Covered: everything the runtime mounts in a main session. Excluded: MCP
/// servers, skills, embedder/plugin contributions and the per-agent
/// delegation tools (those are agents, not tools).
fn preset_tool_groups() -> Vec<(&'static str, Vec<&'static str>)> {
    use manox_agent::tools::{
        ASK_USER_QUESTION, BASH, BASH_OUTPUT, CREATE_GOAL, EDIT, GET_GOAL, GLOB, GREP, PY, READ,
        TASK_STOP, UPDATE_GOAL, UPDATE_PLAN, WEB_FETCH, WRITE,
    };

    let mut web = vec![WEB_FETCH];
    web.extend(manox_agent::engine::BrowserSuite::WebExplore.tool_names());
    vec![
        (
            "agents-tools-group-files",
            vec![READ, WRITE, EDIT, GREP, GLOB, "Ls"],
        ),
        ("agents-tools-group-shell", vec![BASH, BASH_OUTPUT, PY]),
        ("agents-tools-group-web", web),
        (
            "agents-tools-group-interaction",
            vec![ASK_USER_QUESTION, UPDATE_PLAN],
        ),
        (
            "agents-tools-group-tasks",
            // The names without upstream constants stay as literals: exported
            // from `manox_agent::tools` in dspo/manox#984 (the module's own
            // single-source rule) and this list follows the next tag.
            vec!["TaskList", TASK_STOP, GET_GOAL, CREATE_GOAL, UPDATE_GOAL],
        ),
        (
            "agents-tools-group-host",
            vec![
                "ClipboardRead",
                "Open",
                "Steer",
                manox_agent::subagent::ListAgentsTool::NAME,
                manox_agent::subagent::InterruptAgentTool::NAME,
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::preset_tool_groups;

    /// The grid claims to list the whole preset set once; a duplicate, a
    /// dropped core tool or an empty group would silently misstate what the
    /// agent may call.
    #[test]
    fn preset_tool_catalog_is_complete_and_deduplicated() {
        let groups = preset_tool_groups();
        let tools: Vec<&str> = groups.iter().flat_map(|(_, tools)| tools.clone()).collect();
        let unique: HashSet<&str> = tools.iter().copied().collect();
        assert_eq!(tools.len(), unique.len(), "preset tools must not repeat");
        for (label, tools) in &groups {
            assert!(!tools.is_empty(), "tool group {label} is empty");
        }
        for name in [
            "Read",
            "Write",
            "Edit",
            "Grep",
            "Glob",
            "Ls",
            "Bash",
            "WebFetch",
            "AskUserQuestion",
            "UpdatePlan",
            "ListAgents",
        ] {
            assert!(unique.contains(name), "preset catalog is missing {name}");
        }
        assert!(
            !unique.contains("Skill"),
            "skills are not preset tools (they belong to the skill fold)"
        );
    }
}
