//! The pi session sheet's body: the transcript, the pending interactive cards,
//! and the composer.
//!
//! Rendering only reads [`AcpStore`] — the fold lives in `pi::book`, the
//! transport in `pi::runtime`. Entry semantics (tool cards, reasoning, asks)
//! reuse `ai-elements`; this module owns the pi layout, not the conversation
//! vocabulary.

use std::collections::HashMap;

use ai_elements::{
    AskBlock, AskOption, AskState, AskUserQuestion, Reasoning, ReasoningState, Tool, ToolState,
    ToolStatus as ElementToolStatus,
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, IntoElement, Render, SharedString, Subscription,
    Window, div, px,
};
use gpui_component::ActiveTheme;
use gpui_component::input::{Input, InputEvent, InputState};
use serde_json::Value;
use steer_acp::RequestId;

use crate::i18n::{t, t_str};
use crate::pi::store::fallback_title;
use crate::pi::{
    AcpStore, Entry, PendingRequest, PiSessionState, ToolEntry, ToolStatus, transcript::DiffBlock,
};

/// One pi session's sheet body.
pub struct PiSessionView {
    store: Entity<AcpStore>,
    session_id: String,
    input: Entity<InputState>,
    reasoning: HashMap<String, Entity<ReasoningState>>,
    tools: HashMap<String, Entity<ToolState>>,
    asks: HashMap<String, Entity<AskState>>,
    /// Elicitation form choices, keyed by request id then schema property.
    elicitation_choices: HashMap<String, HashMap<String, String>>,
    _store_sub: Subscription,
    _input_sub: Subscription,
}

impl PiSessionView {
    pub fn new(
        store: Entity<AcpStore>,
        session_id: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session_id = session_id.into();
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("pi-composer-placeholder")));
        let input_sub =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.submit(window, cx);
                }
            });
        let store_sub = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            store,
            session_id,
            input,
            reasoning: HashMap::new(),
            tools: HashMap::new(),
            asks: HashMap::new(),
            elicitation_choices: HashMap::new(),
            _store_sub: store_sub,
            _input_sub: input_sub,
        }
    }

    /// Sends the composer text (empty input is a no-op).
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        let store = self.store.clone();
        let session_id = self.session_id.clone();
        store.update(cx, |store, cx| store.prompt(&session_id, text, cx));
    }
}

impl Render for PiSessionView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (title, error, rows, state, loading, streaming, queued, model, mode, usage, version) = {
            let store = self.store.read(cx);
            let view = store.book().view(&self.session_id);
            let title = fallback_title(store, &self.session_id);
            let error = store.error().map(SharedString::from).or_else(|| {
                view.and_then(|view| view.error.clone())
                    .map(SharedString::from)
            });
            let rows = store.book().rows(&self.session_id);
            let (state, loading, streaming, queued, model, mode, usage, version) = view
                .map(|view| {
                    (
                        view.state,
                        view.loading,
                        view.streaming,
                        view.queued,
                        view.model
                            .as_ref()
                            .map(|model| model.name.clone().unwrap_or_else(|| model.id.clone())),
                        view.current_mode.clone(),
                        view.usage,
                        view.banner.as_ref().map(|banner| banner.version.clone()),
                    )
                })
                .unwrap_or((
                    PiSessionState::Starting,
                    true,
                    false,
                    0,
                    None,
                    None,
                    None,
                    None,
                ));
            (
                title, error, rows, state, loading, streaming, queued, model, mode, usage, version,
            )
        };

        let rows = rows
            .into_iter()
            .map(|entry| render_entry(entry, cx, &theme))
            .collect::<Vec<_>>();
        let cards = self.pending_cards(cx, &theme);
        let percent = usage.and_then(|usage| usage.percent());
        let store = self.store.clone();
        let session_id = self.session_id.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_sm()
                            .child(title)
                            .children(model.map(|model| muted_text(model, &theme)))
                            .children(mode.map(|mode| muted_text(mode, &theme)))
                            .children(percent.map(|pct| {
                                muted_text(
                                    t_str("pi-context-used", &[("pct", &pct.to_string())]),
                                    &theme,
                                )
                            })),
                    )
                    .children(version.map(|version| {
                        muted_text(t_str("pi-banner-version", &[("version", &version)]), &theme)
                    })),
            )
            .child(
                div()
                    .id("pi-transcript")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(loading && rows.is_empty(), |list| {
                        list.child(muted_text(t("pi-loading"), &theme))
                    })
                    .when(!loading && rows.is_empty(), |list| {
                        list.child(muted_text(t("pi-empty"), &theme))
                    })
                    .children(rows)
                    .when(streaming, |list| {
                        list.child(muted_text(t("pi-streaming"), &theme))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme.border)
                    .children(cards)
                    .children(
                        error.map(|error| div().text_xs().text_color(theme.danger).child(error)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.input)))
                            .when(streaming, |row| {
                                let store = store.clone();
                                let session_id = session_id.clone();
                                row.child(button(
                                    SharedString::from(format!("pi-stop:{session_id}")),
                                    t("pi-stop"),
                                    theme.danger,
                                    move |_, _, cx| {
                                        store.update(cx, |store, cx| {
                                            store.cancel(&session_id, cx);
                                        });
                                    },
                                ))
                            })
                            .child(button(
                                SharedString::from("pi-send"),
                                t("pi-send"),
                                theme.primary,
                                {
                                    let input = self.input.clone();
                                    let store = store.clone();
                                    let session_id = session_id.clone();
                                    move |_, window, cx| {
                                        let text = input.read(cx).value().to_string();
                                        if text.trim().is_empty() {
                                            return;
                                        }
                                        input.update(cx, |state, cx| {
                                            state.set_value("", window, cx)
                                        });
                                        store.update(cx, |store, cx| {
                                            store.prompt(&session_id, text, cx);
                                        });
                                    }
                                },
                            ))
                            .children((queued > 0).then(|| {
                                muted_text(
                                    t_str("pi-queued", &[("count", &queued.to_string())]),
                                    &theme,
                                )
                            })),
                    )
                    .when(state == PiSessionState::Dead, |column| {
                        column.child(muted_text(t("pi-session-dead"), &theme))
                    }),
            )
    }
}

impl PiSessionView {
    /// Every pending interactive card: approvals, elicitations, asks. The
    /// pending lists are cloned out of the store first — building a card needs
    /// `&mut Context` (its state entity), which conflicts with a live read.
    fn pending_cards(
        &mut self,
        cx: &mut Context<Self>,
        theme: &gpui_component::Theme,
    ) -> Vec<AnyElement> {
        let (permissions, elicitations, asks) = {
            let store = self.store.read(cx);
            let Some(session) = store.book().view(&self.session_id) else {
                return Vec::new();
            };
            (
                session.pending_permissions.clone(),
                session.pending_elicitations.clone(),
                session.pending_asks.clone(),
            )
        };
        let mut cards = Vec::new();
        for request in permissions {
            cards.push(self.permission_card(&request, theme));
        }
        for request in elicitations {
            cards.push(self.elicitation_card(&request, theme, cx));
        }
        for request in asks {
            cards.push(self.ask_card(&request, cx));
        }
        cards
    }

    fn permission_card(
        &self,
        request: &PendingRequest,
        theme: &gpui_component::Theme,
    ) -> AnyElement {
        let title = request
            .params
            .get("toolCall")
            .and_then(|tool| tool.get("title"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| t("pi-permission-title").to_string());
        let mut buttons = div().flex().gap_2();
        for option in request
            .params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let Some(option_id) = option.get("optionId").and_then(Value::as_str) else {
                continue;
            };
            let name = option
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(option_id)
                .to_owned();
            let reject = option
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.starts_with("reject") || kind.starts_with("deny"));
            buttons = buttons.child(self.answer_button(
                SharedString::from(format!("pi-perm:{}:{option_id}", request_key(&request.id))),
                SharedString::from(name),
                if reject { theme.danger } else { theme.primary },
                request.id.clone(),
                serde_json::json!({
                    "outcome": { "outcome": "selected", "optionId": option_id },
                }),
            ));
        }
        div()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child(SharedString::from(title)))
            .child(buttons)
            .into_any_element()
    }

    fn elicitation_card(
        &mut self,
        request: &PendingRequest,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = request_key(&request.id);
        let message = request
            .params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let properties = request
            .params
            .get("requestedSchema")
            .and_then(|schema| schema.get("properties"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut card = div()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child(SharedString::from(message)));
        let chosen = self.elicitation_choices.remove(&key).unwrap_or_default();
        let mut complete = true;
        for (name, schema) in &properties {
            let values = schema
                .get("enum")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if values.is_empty() {
                continue;
            }
            if !chosen.contains_key(name) {
                complete = false;
            }
            let title = schema
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_owned();
            let mut row = div()
                .flex()
                .items_center()
                .gap_2()
                .child(muted_text(title, theme));
            for value in values {
                let Some(value) = value.as_str() else {
                    continue;
                };
                let selected = chosen.get(name).is_some_and(|current| current == value);
                let this = cx.entity();
                let property = name.clone();
                let picked = value.to_owned();
                let choice_key = key.clone();
                row = row.child(choice_button(
                    SharedString::from(format!("pi-choice:{key}:{name}:{value}")),
                    SharedString::from(value.to_owned()),
                    if selected {
                        theme.primary
                    } else {
                        theme.border
                    },
                    move |_, _, cx| {
                        this.update(cx, |view, cx| {
                            view.elicitation_choices
                                .entry(choice_key.clone())
                                .or_default()
                                .insert(property.clone(), picked.clone());
                            cx.notify();
                        });
                    },
                ));
            }
            card = card.child(row);
        }
        self.elicitation_choices.insert(key, chosen.clone());
        let mut actions = div().flex().gap_2().child(self.answer_button(
            SharedString::from(format!("pi-elicit-decline:{}", request_key(&request.id))),
            t("pi-decline"),
            theme.border,
            request.id.clone(),
            serde_json::json!({ "action": "decline" }),
        ));
        if complete {
            let content: serde_json::Map<String, Value> = chosen
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect();
            actions = actions.child(self.answer_button(
                SharedString::from(format!("pi-elicit-accept:{}", request_key(&request.id))),
                t("pi-accept"),
                theme.primary,
                request.id.clone(),
                serde_json::json!({ "action": "accept", "content": content }),
            ));
        }
        card.child(actions).into_any_element()
    }

    fn ask_card(&mut self, request: &PendingRequest, cx: &mut Context<Self>) -> AnyElement {
        let tool_call_id = request.tool_call_id().unwrap_or_default().to_owned();
        let questions = request
            .params
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let multi = questions
            .iter()
            .map(|question| {
                question
                    .get("multiSelect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        let state = self
            .asks
            .entry(tool_call_id.clone())
            .or_insert_with(|| cx.new(|cx| AskState::new(multi, t("pi-ask-note-placeholder"), cx)))
            .clone();
        let mut blocks = Vec::new();
        for question in &questions {
            let mut block = AskBlock::new(
                question
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            );
            if let Some(header) = question.get("header").and_then(Value::as_str) {
                block = block.header(header.to_owned());
            }
            for option in question
                .get("options")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let mut choice = AskOption::new(
                    option
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                );
                if let Some(description) = option.get("description").and_then(Value::as_str) {
                    choice = choice.description(description.to_owned());
                }
                block = block.option(choice);
            }
            blocks.push(block);
        }

        let id = request.id.clone();
        let submit_store = self.store.clone();
        let close_store = self.store.clone();
        let session_id = self.session_id.clone();
        let close_session = self.session_id.clone();
        let submit_state = state.clone();
        let submit_questions = questions;
        let close_id = id.clone();

        AskUserQuestion::new(tool_call_id, &state)
            .name(t("pi-ask-title"))
            .blocks(blocks)
            .on_submit(move |_, cx| {
                let answers = submit_state.read(cx).answers(cx);
                let response = ask_response(&submit_questions, &answers);
                submit_store.update(cx, |store, cx| {
                    let _ = store.answer(&session_id, &id, response, cx);
                });
            })
            .on_close(move |_, cx| {
                close_store.update(cx, |store, cx| {
                    let _ = store.answer(
                        &close_session,
                        &close_id,
                        serde_json::json!({ "action": "cancel" }),
                        cx,
                    );
                });
            })
            .into_any_element()
    }

    fn answer_button(
        &self,
        id: SharedString,
        label: impl Into<SharedString>,
        color: gpui::Hsla,
        request: RequestId,
        result: Value,
    ) -> AnyElement {
        let store = self.store.clone();
        let session_id = self.session_id.clone();
        button(id, label, color, move |_, _, cx| {
            store.update(cx, |store, cx| {
                let _ = store.answer(&session_id, &request, result.clone(), cx);
            });
        })
    }
}

/// One transcript row. Tool and reasoning cards keep an entity per entry id, so
/// their folds and status chips survive re-renders.
fn render_entry(
    entry: Entry,
    cx: &mut Context<PiSessionView>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    match entry {
        Entry::User(user) => div()
            .flex()
            .justify_end()
            .child(
                div()
                    .max_w(px(640.))
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .px_3()
                    .py_2()
                    .text_sm()
                    .child(SharedString::from(user.text)),
            )
            .into_any_element(),
        Entry::Assistant(reply) => {
            let mut column = div().flex().flex_col().gap_1().text_sm();
            if let Some(thoughts) = reply
                .thoughts
                .clone()
                .filter(|text| !text.trim().is_empty())
            {
                let state = ensure_reasoning(cx, &reply.id);
                column = column.child(
                    Reasoning::new(
                        SharedString::from(format!("pi-reasoning:{}", reply.id)),
                        &state,
                    )
                    .content(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(thoughts),
                    ),
                );
            }
            if !reply.text.is_empty() {
                column = column.child(SharedString::from(reply.text.clone()));
            }
            if let Some(error) = reply
                .pi
                .as_ref()
                .and_then(|pi| pi.error.clone())
                .filter(|error| !error.is_empty())
            {
                column = column.child(div().text_xs().text_color(theme.danger).child(error));
            }
            column.into_any_element()
        }
        Entry::Tool(tool) => {
            let state = ensure_tool(cx, &tool.tool_call_id);
            state.update(cx, |state, cx| {
                state.set_status(element_status(tool.status), cx);
            });
            let mut card = Tool::new(SharedString::from(format!("pi-tool:{}", tool.id)), &state)
                .name(SharedString::from(
                    tool.name.clone().unwrap_or_else(|| tool.title.clone()),
                ));
            if let Some(input) = summarize_input(&tool) {
                card = card.args(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(input),
                );
            }
            if let Some(body) = tool_body(&tool, theme) {
                card = card.content(body);
            }
            card.into_any_element()
        }
        Entry::Note(note) => div()
            .text_xs()
            .text_color(match note.severity.as_deref() {
                Some("error") => theme.danger,
                Some("warning") => theme.warning,
                _ => theme.muted_foreground,
            })
            .child(SharedString::from(note.text))
            .into_any_element(),
    }
}

fn tool_body(tool: &ToolEntry, theme: &gpui_component::Theme) -> Option<AnyElement> {
    let mut body = div().flex().flex_col().gap_1();
    let mut empty = true;
    if !tool.output.is_empty() {
        empty = false;
        body = body.child(
            div()
                .id(SharedString::from(format!("pi-tool-output:{}", tool.id)))
                .max_h(px(220.))
                .overflow_y_scroll()
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(SharedString::from(tool.output.clone())),
        );
    }
    for diff in &tool.diffs {
        empty = false;
        body = body.child(diff_block(diff, theme));
    }
    if let Some(meta) = &tool.meta {
        let mut facts = Vec::new();
        if let Some(code) = meta.exit_code {
            facts.push(format!("exit {code}"));
        }
        if meta.truncated == Some(true) {
            facts.push(format!(
                "truncated ({} lines)",
                meta.total_lines.unwrap_or(0)
            ));
        }
        if !facts.is_empty() {
            empty = false;
            body = body.child(muted_text(SharedString::from(facts.join(" · ")), theme));
        }
    }
    (!empty).then(|| body.into_any_element())
}

fn diff_block(diff: &DiffBlock, theme: &gpui_component::Theme) -> AnyElement {
    let mut block = div().flex().flex_col().text_xs();
    if let Some(path) = &diff.path {
        block = block.child(muted_text(SharedString::from(path.clone()), theme));
    }
    for line in diff
        .old_text
        .as_deref()
        .unwrap_or_default()
        .lines()
        .map(|line| format!("-{line}"))
        .chain(diff.new_text.lines().map(|line| format!("+{line}")))
    {
        let removed = line.starts_with('-');
        block = block.child(
            div()
                .font_family(theme.mono_font_family.clone())
                .text_color(if removed { theme.danger } else { theme.success })
                .child(SharedString::from(line)),
        );
    }
    block.into_any_element()
}

fn summarize_input(tool: &ToolEntry) -> Option<SharedString> {
    let input = tool.raw_input.as_ref()?;
    for key in ["command", "path", "file_path", "pattern", "query", "url"] {
        if let Some(value) = input.get(key).and_then(Value::as_str) {
            let first = value.lines().next().unwrap_or_default();
            let text = if first.chars().count() > 80 {
                format!("{}…", first.chars().take(79).collect::<String>())
            } else {
                first.to_owned()
            };
            return Some(SharedString::from(text));
        }
    }
    None
}

fn element_status(status: ToolStatus) -> ElementToolStatus {
    match status {
        ToolStatus::Pending => ElementToolStatus::Pending,
        ToolStatus::InProgress => ElementToolStatus::Running,
        ToolStatus::Completed => ElementToolStatus::Succeeded,
        ToolStatus::Failed => ElementToolStatus::Failed,
    }
}

fn ensure_reasoning(cx: &mut Context<PiSessionView>, id: &str) -> Entity<ReasoningState> {
    if let Some(entity) = cx.entity().read(cx).reasoning.get(id).cloned() {
        return entity;
    }
    let entity = cx.new(ReasoningState::new);
    cx.entity().update(cx, |view, _| {
        view.reasoning.insert(id.to_owned(), entity.clone());
    });
    entity
}

fn ensure_tool(cx: &mut Context<PiSessionView>, id: &str) -> Entity<ToolState> {
    if let Some(entity) = cx.entity().read(cx).tools.get(id).cloned() {
        return entity;
    }
    let entity = cx.new(|cx| ToolState::new(cx).no_auto_fold());
    cx.entity().update(cx, |view, _| {
        view.tools.insert(id.to_owned(), entity.clone());
    });
    entity
}

/// The ask card's answers as the harness's `AskResponse` envelope.
fn ask_response(questions: &[Value], answers: &[(Vec<usize>, String)]) -> Value {
    let mut envelope = Vec::new();
    for (index, (selected, note)) in answers.iter().enumerate() {
        let labels: Vec<String> = selected
            .iter()
            .filter_map(|option| {
                questions
                    .get(index)?
                    .get("options")?
                    .get(*option)?
                    .get("label")?
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
        let note = note.trim();
        if labels.is_empty() {
            if note.is_empty() {
                envelope.push(serde_json::json!({ "kind": "option", "labels": [] }));
            } else {
                envelope.push(serde_json::json!({ "kind": "custom", "text": note }));
            }
        } else if note.is_empty() {
            envelope.push(serde_json::json!({ "kind": "option", "labels": labels }));
        } else {
            envelope.push(serde_json::json!({ "kind": "option", "labels": labels, "text": note }));
        }
    }
    serde_json::json!({ "action": "accept", "answers": envelope })
}

fn request_key(id: &RequestId) -> String {
    match id {
        RequestId::Number(number) => format!("n:{number}"),
        RequestId::String(text) => format!("s:{text}"),
    }
}

fn muted_text(text: impl Into<SharedString>, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(text.into())
        .into_any_element()
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    color: gpui::Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(px(6.))
        .bg(color)
        .text_sm()
        .cursor_pointer()
        .child(label.into())
        .on_click(on_click)
        .into_any_element()
}

fn choice_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    color: gpui::Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded(px(4.))
        .border_1()
        .border_color(color)
        .text_xs()
        .cursor_pointer()
        .child(label.into())
        .on_click(on_click)
        .into_any_element()
}
