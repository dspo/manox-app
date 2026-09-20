//! Rendering of a single conversation message.
//!
//! - Text blocks render via `Markdown` with per-block copy buttons (cross-block
//!   selection + Cmd+C copy lands in a follow-up).
//! - User: a full-width bordered turn block with a muted metadata header
//!   (role · model · project) — the Claude Code TUI turn-block look, not a
//!   chat bubble.
//! - Assistant: a full-width block with a role label + markdown body.
//! - Reasoning: a collapsible block, indented secondary text with a left border.
//! - ToolCall: a card with title + status icon + monospace output.
//!
//! Streaming assistant / reasoning bodies render formatted markdown throughout
//! the stream via `Markdown::blocks` (blocks from an `IncrementalParser`), with
//! a trailing cursor on the last block. `Stop` finalizes the parser (one full
//! parse for consistency) and flips the streaming flag off — no reflow jump.
//! Streaming tool output is stricter: while lines are still arriving we paint
//! a plain monospace run and only mount the syntax-highlighted `Markdown` once
//! the final `ToolResult` lands.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use crate::conversation::{
    ActivityEntry, AgentTaskItem, BackgroundTaskItem, ConvItem, ThinkingContainer, ToolCallItem,
    UserImage, UserTurnMeta, next_reasoning_entry_id,
};
use crate::i18n;
use ai_elements::{BrailleSpinner, ChainOfThought, ChainOfThoughtHeader, ChainOfThoughtStep};
use base64::Engine as _;
use chrono::{Datelike as _, Local, TimeZone as _};
use gpui::prelude::*;
use gpui::{Animation, AnimationExt as _, CursorStyle, ease_out_quint};
use gpui::{App, ClipboardItem, Entity, Render, SharedString, WeakEntity, px};
use gpui_component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, Theme,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    tooltip::Tooltip,
    v_flex,
};
use gpui_component::{
    Disableable as _,
    tag::{Tag, TagVariant},
};
use manox_agent::language_model::{LanguageModelToolResult, MessageContent, Role};
use manox_agent::thread::PermissionMode;
use manox_agent::{Message, TokenUsage, ToolCallStatus};
use manox_components::markdown::ast::LinkKind;
use manox_components::markdown::terminal_panel::GitSummary;
use manox_components::markdown::{HeadingMode, Markdown, PanelKind, TerminalPanel, hashline_text};

/// Message body type size: one step below the chrome `text_base` (14px) so
/// dense conversation text reads lighter. Composer + Editor tab pin the same
/// value so input text stays interchangeable with message content.
pub const MESSAGE_BODY_SIZE: gpui::Pixels = gpui::px(13.);
use manox_components::turn_frame::TurnFrame;
use std::path::{Path, PathBuf};

use crate::Workspace;
use crate::views::centered;
use crate::workspace::{AskCardQuestion, AskCardSnapshot};

/// Render-time context for sub-agent task rows. `None` when the owning
/// workspace has been dropped; the row remains visible but clicks become a
/// no-op.
#[derive(Clone)]
pub struct AgentTaskCtx {
    pub weak: WeakEntity<Workspace>,
}

/// Render-time context for plain tool-call cards. Carries a weak handle to the
/// owning `Workspace` so the card's header can toggle its own `collapsed` flag
/// (the flag lives on the `ToolCallItem` so the user's choice survives scroll-
/// driven remounts). `None` after the Workspace drops — clicks no-op and the
/// card stays in whatever state it last rendered.
#[derive(Clone)]
pub struct ToolCallCtx {
    pub weak: WeakEntity<Workspace>,
    pub(crate) ask: Option<AskCardSnapshot>,
}

/// Markdown renderer with theme-aware syntax highlighting.
///
/// `scrollable = true` mounts an internal vertical scrollbar; the renderer
/// sizes to its parent's box, so the parent must carry a fixed height (use
/// `h(...)` rather than `max_h(...)`).
///
/// `scrollable = false` clips overflow horizontally — the renderer itself is a
/// `w_full` + `min_w_0` column, so a long unbreakable run cannot push past the
/// env-card gutter.
///
/// Mounts a fresh `Entity<Markdown>` per frame — used for static chrome and
/// defensive fallbacks (sub-message bodies, tool-output / reasoning fallbacks)
/// where cross-block selection is not required (code blocks carry their own
/// hover copy button). Text bodies use the `MessageItem`'s persistent
/// `Entity<Markdown>` directly (see `ensure_markdown`).
fn markdown_tv(
    id: impl Into<gpui::ElementId>,
    text: impl Into<gpui::SharedString>,
    theme: &Theme,
    scrollable: bool,
    cx: &mut App,
) -> gpui::AnyElement {
    cx.new(|_cx| {
        Markdown::new(id, text)
            .theme(theme)
            .scrollable(scrollable)
            .heading_mode(HeadingMode::Uniform)
            .body_size(MESSAGE_BODY_SIZE)
    })
    .into_any_element()
}

/// Mount the item's persistent text body when present; otherwise fall back to a
/// per-frame `markdown_tv` mount so the same renderer output appears while
/// selection degrades to per-frame. Every text-body render shares this path.
fn body_or_static(
    body: Option<Entity<Markdown>>,
    id: impl Into<gpui::ElementId>,
    text: String,
    theme: &Theme,
    cx: &mut App,
) -> gpui::AnyElement {
    body.map(|md| md.into_any_element())
        .unwrap_or_else(|| markdown_tv(id, text, theme, false, cx))
}

/// The persistent text body for a conversation item, if it has one: the
/// streaming flag (true only for a mid-stream `Assistant`) plus the body's
/// current source. `None` for items without a text body — their content
/// renders via `TerminalPanel`, activity-entry markdown, or static chrome.
fn text_body_of(kind: &ConvItem) -> Option<(bool, String)> {
    match kind {
        ConvItem::Assistant {
            text, streaming, ..
        } => Some((*streaming, text.clone())),
        ConvItem::User { text, .. } => Some((false, text.clone())),
        ConvItem::Error(msg) => Some((false, msg.clone())),
        // `Notice` owns a paginated `TerminalPanel` body (`notice_panel`),
        // not a markdown document.
        ConvItem::Recap { summary, .. } => Some((false, summary.clone())),
        ConvItem::Retry {
            detail: Some(detail),
            ..
        } => Some((false, detail.clone())),
        _ => None,
    }
}

/// One renderable conversation item, owned by its own gpui `Entity` so a
/// streaming delta notifies (and re-renders) only this item rather than the
/// whole workspace. `id` is the item's stable list index (the conversation
/// only ever appends, so the index never shifts); it keys element ids within
/// the entity's own namespace. `role` is the model display name captured at
/// creation time so a finished bubble keeps its model label after the user
/// switches models.
///
/// `markdown` holds the owned `Entity<Markdown>` for items with a text body
/// (Assistant, User, Error, Recap, Retry): a
/// stateful document carrying parse-once incremental parsing + document-level
/// selection, so a streaming delta re-parses only the tail and a cross-block
/// drag selects one continuous range with Cmd/Ctrl+C copy. `None` for other
/// items (Thinking, ToolCall, AgentTask, Notice, …): reasoning and tool-call
/// bodies own their own persistent entities, a `Notice` body is a persistent
/// `TerminalPanel` (`notice_panel`, paginating long notices like tool
/// output), and the remaining static chrome mounts a fresh `Entity<Markdown>`
/// per frame via `markdown_tv` (no persistent selection; code blocks carry
/// their own hover copy button).
pub struct MessageItem {
    kind: ConvItem,
    role: String,
    id: usize,
    /// Weak handle to the owning `Workspace`, used by interactive message
    /// rows such as `AgentTask` to open their peer right-pane view.
    weak_workspace: WeakEntity<Workspace>,
    markdown: Option<Entity<Markdown>>,
    /// Persistent `Entity<TerminalPanel>` for a `ConvItem::Notice` body: the
    /// same paginated surface as tool output (default `PAGE_SIZE` lines, `+N`
    /// to reveal more) so a long notice — e.g. an escalated approval reason —
    /// folds by default instead of pushing the transcript off-screen. The
    /// pagination cursor and selection survive across frames. `None` for
    /// non-notice items.
    notice_panel: Option<Entity<TerminalPanel>>,
    /// Ask-card snapshot budgeted by `Workspace` before the native list starts
    /// measuring rows, so `render` never reads the owning `Workspace` and the
    /// list callback remains read-only.
    pub(crate) ask_snapshot: Option<AskCardSnapshot>,
}

impl MessageItem {
    pub fn new(kind: ConvItem, role: String, id: usize, weak: WeakEntity<Workspace>) -> Self {
        Self {
            kind,
            role,
            id,
            weak_workspace: weak,
            markdown: None,
            notice_panel: None,
            ask_snapshot: None,
        }
    }

    pub fn kind(&self) -> &ConvItem {
        &self.kind
    }

    pub fn kind_mut(&mut self) -> &mut ConvItem {
        &mut self.kind
    }

    /// Diagnostic handle for the overlap registry; the row factory maps its
    /// list index to this id so row/body bounds can be paired.
    pub fn diagnostic_id(&self) -> usize {
        self.id
    }

    /// Lazily create (or re-sync) the owned `Entity<Markdown>` for an item with
    /// a text body, seeded with the kind's streaming flag (true mid-stream,
    /// false for finalized/historical bodies) and the body's current source.
    /// Returns the entity handle so the caller can mount it. `None` for items
    /// without a text body (they have no owned markdown body).
    fn ensure_markdown(&mut self, cx: &mut gpui::Context<Self>) -> Option<Entity<Markdown>> {
        let (streaming, current) = text_body_of(&self.kind)?;
        match &self.markdown {
            None => {
                let id = self.id;
                let cwd = std::env::current_dir().ok();
                self.markdown = Some(cx.new(|cx| {
                    Markdown::new(("md", id), current)
                        .theme(cx.theme())
                        .heading_mode(HeadingMode::Uniform)
                        .body_size(MESSAGE_BODY_SIZE)
                        .streaming(streaming)
                        .on_open_link(Some(Arc::new(move |url, kind| match kind {
                            LinkKind::Url => {
                                let _ = open::that(&url);
                            }
                            LinkKind::FilePath => open_file_in_vscode(&url, cwd.as_deref()),
                        })))
                }));
            }
            // A streaming body is synced incrementally by `update_text`; a
            // finalized body is re-synced here because it can be rewritten in
            // place (e.g. `Retry` coalescing a new detail) — the mounted entity
            // must never lag the item's current text.
            Some(md) if !streaming => {
                md.update(cx, |m, cx| {
                    if m.source() != current {
                        m.replace(current.clone(), cx);
                    }
                });
            }
            Some(_) => {}
        }
        self.markdown.clone()
    }

    /// Feed a full text snapshot to the owned markdown document. `replace` runs
    /// the incremental parser's append-only fast path (re-parse only the tail),
    /// so a streaming delta pays proportional to the delta, not the full body.
    pub fn update_text(&mut self, full_text: &str, cx: &mut gpui::Context<Self>) {
        if let Some(md) = self.ensure_markdown(cx) {
            md.update(cx, |m, cx| m.replace(full_text, cx));
        }
    }

    /// Run the parser's final full parse so the frozen prefix + tail match a
    /// one-shot parse. Used by `rebuild_from_display` for non-streaming text
    /// items loaded from history.
    pub fn finalize_parser(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(md) = &self.markdown {
            md.update(cx, |m, cx| m.finalize(cx));
        }
    }
    /// Mount (lazily) the persistent `TerminalPanel` body for a `Notice` item,
    /// fed with the notice text. Renders the body with the same paginated
    /// surface as tool output (default `PAGE_SIZE` lines, `+N` to reveal
    /// more), keeping the pagination cursor and selection across frames.
    /// `None` for non-notice items.
    pub fn ensure_notice_panel(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let ConvItem::Notice(text) = &self.kind else {
            return None;
        };
        if self.notice_panel.is_none() {
            let panel = cx.new(|cx| TerminalPanel::new(PanelKind::Plain, None, None, cx.theme()));
            let text = text.clone();
            panel.update(cx, |p, cx| p.set_output(text, cx));
            self.notice_panel = Some(panel);
        }
        self.notice_panel.clone()
    }

    /// Ensure the `eix`-th activity entry's persistent `Entity<Markdown>` exists
    /// and feed it the entry's current text. Mirrors `update_text` for the
    /// activity-tree reasoning rounds: a streaming delta drives the incremental
    /// parser's append-only fast path while document-level selection + focus
    /// survive across frames (drag + Cmd/Ctrl+C), so a reasoning round selects
    /// and copies just like the top-level body.
    pub fn sync_reasoning_entry(&mut self, eix: usize, cx: &mut gpui::Context<Self>) {
        let ConvItem::Thinking(t) = &mut self.kind else {
            return;
        };
        let Some(ActivityEntry::Reasoning {
            text,
            streaming,
            markdown,
            ..
        }) = t.entries.get_mut(eix)
        else {
            return;
        };
        if markdown.is_none() {
            let streaming = *streaming;
            let id = eix;
            *markdown = Some(cx.new(|cx| {
                Markdown::new(("reasoning-md", id), "")
                    .theme(cx.theme())
                    .heading_mode(HeadingMode::Uniform)
                    .body_size(MESSAGE_BODY_SIZE)
                    .streaming(streaming)
            }));
        }
        if let Some(md) = markdown.as_ref().cloned() {
            let text = text.clone();
            md.update(cx, |m, cx| m.replace(&text, cx));
        }
    }

    /// For a rebuilt (historical) `Thinking` container, mount + finalize the
    /// persistent markdown for every reasoning round so document-level
    /// selection works on reloaded history (not just live-streamed turns).
    /// Mirrors `update_text` + `finalize_parser` for the top-level text bodies.
    pub fn rebuild_activity_reasoning(&mut self, cx: &mut gpui::Context<Self>) {
        let eixs: Vec<usize> = {
            let ConvItem::Thinking(t) = &mut self.kind else {
                return;
            };
            t.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(e, ActivityEntry::Reasoning { .. }))
                .map(|(i, _)| i)
                .collect()
        };
        for eix in eixs {
            self.sync_reasoning_entry(eix, cx);
            if let ConvItem::Thinking(t) = &mut self.kind
                && let Some(ActivityEntry::Reasoning { markdown, .. }) = t.entries.get_mut(eix)
                && let Some(md) = markdown.as_ref()
            {
                md.update(cx, |m, cx| m.finalize(cx));
            }
        }
    }

    /// Ensure the `eix`-th activity entry's persistent `Entity<TerminalPanel>`
    /// exists and feed it the entry's current display output. Mirrors
    /// `sync_reasoning_entry` for tool calls: a streaming delta or a finalized
    /// result drives the panel's `set_output` while document-level selection +
    /// focus survive across frames (drag + Cmd/Ctrl+C), so tool output selects
    /// and copies like the assistant body — and renders as a terminal-styled
    /// shell (cwd / toolchain / `❯` command + ANSI-colored output) rather than
    /// a fenced code block.
    pub fn sync_tool_entry_panel(
        &mut self,
        eix: usize,
        cwd: Option<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) {
        let ConvItem::Thinking(t) = &mut self.kind else {
            return;
        };
        let Some(ActivityEntry::Tool(entry)) = t.entries.get_mut(eix) else {
            return;
        };
        Self::ensure_tool_panel(entry, cwd, cx);
    }

    /// Top-level `ConvItem::ToolCall` variant of the panel sync, used by the
    /// `AskUserQuestion` answered-state card and the orphan `ToolResult` card.
    pub fn sync_tool_call_panel(
        &mut self,
        cwd: Option<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) {
        let ConvItem::ToolCall(entry) = &mut self.kind else {
            return;
        };
        Self::ensure_tool_panel(entry, cwd, cx);
    }

    /// Mount (lazily) and refresh the tool-call's `TerminalPanel` from its
    /// current display output. `cwd` is the live thread working directory
    /// gathered by the caller from the workspace. Only `bash` earns a prompt
    /// block (cwd + git + `❯ command`); see the gate below — internal tools
    /// and MCP tools render the body only, so `command`/`cwd` stay `None`
    /// for them even though some carry a `command` input field.
    fn ensure_tool_panel(
        entry: &mut ToolCallItem,
        cwd: Option<SharedString>,
        cx: &mut gpui::Context<MessageItem>,
    ) {
        let (kind, body) = tool_panel_body(entry);
        if entry.panel.is_none() {
            // Only the `bash` tool runs a real shell command a human would type in
            // a terminal, so only it earns the prompt block (cwd + git + `❯
            // command`). Internal tools (grep / read_file / edit_file / glob /
            // list_directory / …) and MCP tools are manox abstractions, not
            // terminal commands — they render the body only, without the cwd
            // preamble that would imply "run this in a shell".
            let is_terminal_command = entry.name.as_str() == manox_agent::tools::BASH;
            let command = if is_terminal_command {
                entry
                    .input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .map(SharedString::from)
            } else {
                None
            };
            let cwd_for_panel = if is_terminal_command {
                cwd.clone()
            } else {
                None
            };
            let panel = cx.new(|cx| TerminalPanel::new(kind, command, cwd_for_panel, cx.theme()));
            // Probe the workdir's git state once per panel — a snapshot at the
            // moment the command ran, like a real shell prompt. Runs on the
            // background executor so the two `git` subprocess spawns never block
            // the UI; `set_git` re-renders the prompt line when it lands.
            if is_terminal_command && let Some(cwd_s) = cwd.as_ref() {
                let cwd_path = PathBuf::from(cwd_s.as_ref());
                let panel = panel.clone();
                cx.spawn(async move |_, cx| {
                    let git = cx
                        .background_spawn(async move { detect_git(&cwd_path) })
                        .await;
                    panel.update(cx, |p, cx| p.set_git(git, cx));
                })
                .detach();
            }
            entry.panel = Some(panel);
        }
        if let Some(panel) = entry.panel.as_ref().cloned() {
            let streaming = entry.streaming;
            panel.update(cx, |p, cx| {
                p.set_kind(kind, cx);
                p.set_streaming(streaming, cx);
                p.set_output(body, cx);
            });
        }
    }

    /// On history reload, mount + refresh the persistent panel for every tool
    /// entry across all activity segments (and each top-level `ToolCall`),
    /// so selection works on reloaded history and finalized output renders as a
    /// terminal panel rather than a per-frame fallback.
    pub fn rebuild_tool_panels(&mut self, cwd: Option<SharedString>, cx: &mut gpui::Context<Self>) {
        // Gather entry indices through an immutable borrow first, then drive the
        // mutable `sync_tool_*_panel` calls — otherwise `&mut self.kind` and
        // `&mut self` (via the sync method) collide.
        let tool_eixs: Vec<usize> = match &self.kind {
            ConvItem::Thinking(t) => t
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(e, ActivityEntry::Tool(_)))
                .map(|(i, _)| i)
                .collect(),
            ConvItem::ToolCall(_) => return self.sync_tool_call_panel(cwd, cx),
            _ => return,
        };
        for eix in tool_eixs {
            self.sync_tool_entry_panel(eix, cwd.clone(), cx);
        }
    }

    /// Flip streaming flags off on a `Stop`. Called once per stop, so the
    /// O(items) walk is harmless. `terminal` distinguishes a turn-ending stop
    /// (`EndTurn`/`MaxTokens`/`Refusal`/cancel/error) from a mid-turn
    /// `StopReason::ToolUse`: a terminal stop freezes the activity segment
    /// (pins elapsed, schedules the entries' delayed auto-collapse) and the
    /// tool-call cards; a ToolUse stop only finalizes the assistant/reasoning
    /// text streaming so the next model response's tool calls fold into the
    /// same segment. The markdown document
    /// always gets a final pass so the frozen prefix + tail match a one-shot
    /// full parse exactly.
    pub fn finalize_streaming(&mut self, terminal: bool, cx: &mut gpui::Context<Self>) {
        match &mut self.kind {
            ConvItem::Assistant { streaming, .. } => *streaming = false,
            ConvItem::Thinking(t) if terminal => {
                // Turn ended: freeze the segment, pin elapsed, and schedule
                // the entries' delayed auto-collapse (the stream plays out,
                // then folds ~1s later). `finalize_segment` is idempotent
                // with `recompute_streaming`'s pinning.
                t.finalize_segment();
                for (eix, entry) in t.entries.iter_mut().enumerate() {
                    match entry {
                        ActivityEntry::Reasoning {
                            streaming,
                            collapsed,
                            user_toggled,
                            markdown,
                            ..
                        } => {
                            *streaming = false;
                            if !*user_toggled && !*collapsed {
                                schedule_auto_collapse(AutoCollapseTarget::ReasoningEntry(eix), cx);
                            }
                            if let Some(md) = markdown.as_ref() {
                                md.update(cx, |m, cx| m.finalize_streaming(cx));
                            }
                        }
                        ActivityEntry::Tool(tool) => {
                            tool.streaming = false;
                            if matches!(
                                tool.status,
                                ToolCallStatus::Success
                                    | ToolCallStatus::Error
                                    | ToolCallStatus::Denied
                            ) && !tool.user_toggled
                                && !tool.collapsed
                            {
                                schedule_auto_collapse(
                                    AutoCollapseTarget::Tool(tool.id.clone()),
                                    cx,
                                );
                            }
                        }
                    }
                }
                t.collapsed = !t.user_toggled;
            }
            // ToolUse stop (`!terminal`): the segment stays open so the next
            // model response's tool calls still fold into it, but this
            // reasoning round is done — flip its `streaming` flag off (and
            // finalize its markdown) so the next `AgentThinking` opens a
            // fresh round instead of appending to the previous one.
            ConvItem::Thinking(t) => {
                t.finalize_reasoning_rounds();
                for (eix, entry) in t.entries.iter_mut().enumerate() {
                    if let ActivityEntry::Reasoning {
                        streaming: false,
                        collapsed,
                        user_toggled,
                        markdown,
                        ..
                    } = entry
                    {
                        if !*user_toggled && !*collapsed {
                            schedule_auto_collapse(AutoCollapseTarget::ReasoningEntry(eix), cx);
                        }
                        if let Some(md) = markdown.as_ref() {
                            md.update(cx, |m, cx| m.finalize_streaming(cx));
                        }
                    }
                }
            }
            ConvItem::ToolCall(t) => {
                t.streaming = false;
                if terminal
                    && matches!(
                        t.status,
                        ToolCallStatus::Success
                            | ToolCallStatus::Continued
                            | ToolCallStatus::Error
                            | ToolCallStatus::Denied
                    )
                    && !t.user_toggled
                    && !t.collapsed
                {
                    schedule_auto_collapse(AutoCollapseTarget::Tool(t.id.clone()), cx);
                }
            }
            ConvItem::AgentTask(_) => {}
            _ => {}
        }
        if let Some(md) = &self.markdown {
            md.update(cx, |m, cx| m.finalize_streaming(cx));
        }
    }
    /// Close the activity segment when assistant text arrives mid-turn —
    /// mirrors `build_items`'s `close_segment` on `MessageContent::Text` so
    /// the live streaming path matches the historical rebuild path. Without
    /// this, `AgentThinking` arriving after the answer text folds into the
    /// pre-answer segment (temporal inversion, issue #216).
    pub fn close_segment_for_text(&mut self, cx: &mut gpui::Context<Self>) {
        let ConvItem::Thinking(t) = &mut self.kind else {
            return;
        };
        t.close_for_text();
        // A text boundary settles the segment: fold to the cover unless the
        // user pinned it open — mirrors `close_segment` on the rebuild path.
        t.collapsed = !t.user_toggled;
        // Finalize reasoning markdown so the streaming cursor stops and the
        // final parse matches a one-shot parse; schedule the rounds' delayed
        // auto-collapse so a finished round folds ~1s after it stops
        // streaming.
        for (eix, entry) in t.entries.iter_mut().enumerate() {
            if let ActivityEntry::Reasoning {
                streaming: false,
                collapsed,
                user_toggled,
                markdown,
                ..
            } = entry
            {
                if !*user_toggled && !*collapsed {
                    schedule_auto_collapse(AutoCollapseTarget::ReasoningEntry(eix), cx);
                }
                if let Some(md) = markdown.as_ref() {
                    md.update(cx, |m, cx| m.finalize_streaming(cx));
                }
            }
        }
    }
}

/// One-shot auto-collapse delay after an entry stops streaming / reaches a
/// terminal status — mirrors the vscode extension's `AUTO_CLOSE_DELAY`.
const ENTRY_AUTO_COLLAPSE_DELAY: Duration = Duration::from_millis(1000);

/// Target of a delayed auto-collapse.
pub(crate) enum AutoCollapseTarget {
    /// A reasoning round inside an activity segment, by entry index (entries
    /// are append-only, so the index stays valid).
    ReasoningEntry(usize),
    /// A tool node inside an activity segment or a top-level `ToolCall` card,
    /// by tool id (ids are unique across cards and segment entries).
    Tool(String),
}

/// Schedule a one-shot auto-collapse for an activity entry or top-level tool
/// card: `ENTRY_AUTO_COLLAPSE_DELAY` after the entry stopped streaming /
/// reached a terminal status it folds to its header, so the live stream plays
/// out before the block closes (vscode-extension parity). The fire is skipped
/// when the user toggled the entry (either before scheduling or within the
/// delay window) — afterwards the entry is fully user-driven. A missing entry
/// (thread switch, cleared conversation) makes the fire a no-op.
pub(crate) fn schedule_auto_collapse(
    target: AutoCollapseTarget,
    cx: &mut gpui::Context<MessageItem>,
) {
    cx.spawn(async move |weak, cx| {
        cx.background_executor()
            .timer(ENTRY_AUTO_COLLAPSE_DELAY)
            .await;
        let Some(item) = weak.upgrade() else {
            return;
        };
        item.update(cx, |item, cx| {
            let collapsed = match &target {
                AutoCollapseTarget::ReasoningEntry(eix) => {
                    let ConvItem::Thinking(t) = item.kind_mut() else {
                        return;
                    };
                    let Some(ActivityEntry::Reasoning {
                        collapsed,
                        user_toggled,
                        ..
                    }) = t.entries.get_mut(*eix)
                    else {
                        return;
                    };
                    if *user_toggled || *collapsed {
                        return;
                    }
                    *collapsed = true;
                    true
                }
                AutoCollapseTarget::Tool(id) => match item.kind_mut() {
                    ConvItem::Thinking(t) => {
                        let Some(entry) = t.get_tool_entry_mut(id) else {
                            return;
                        };
                        if entry.user_toggled || entry.collapsed {
                            return;
                        }
                        entry.collapsed = true;
                        true
                    }
                    ConvItem::ToolCall(t) if t.id == *id => {
                        if t.user_toggled || t.collapsed {
                            return;
                        }
                        t.collapsed = true;
                        true
                    }
                    _ => return,
                },
            };
            if collapsed {
                cx.notify();
            }
        });
    })
    .detach();
}

impl Render for MessageItem {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let agent_ctx = self.weak_workspace.upgrade().map(|ws| AgentTaskCtx {
            weak: ws.downgrade(),
        });
        let tool_ctx = self.weak_workspace.upgrade().map(|ws| ToolCallCtx {
            weak: ws.downgrade(),
            ask: self.ask_snapshot.clone(),
        });
        // The owned markdown document for text-bearing items (persistent across
        // frames → selection + streaming state survive). `None` for non-text
        // items; their chrome mounts a fresh `Entity<Markdown>` per frame.
        let body = self.ensure_markdown(cx);
        let diag_id = self.id;
        let diag_enabled = crate::overlap_diag::enabled();
        // The owned paginated `TerminalPanel` for a `Notice` body (`None` for
        // every other kind).
        let notice_panel = self.ensure_notice_panel(cx);
        centered(render_item(
            &self.kind,
            self.id,
            &self.role,
            &theme,
            agent_ctx.as_ref(),
            tool_ctx.as_ref(),
            body,
            notice_panel,
            Some(self.weak_workspace.clone()),
            cx,
        ))
        .debug_selector(|| format!("message-item-body-{}", self.id))
        .when(diag_enabled, |this| {
            this.on_prepaint(move |bounds, _window, _cx| {
                crate::overlap_diag::record_body(diag_id, bounds);
            })
        })
    }
}

/// Render a `ConvItem` as an element. `ix` is the entry index (stable key for
/// collapsibles and text-block element ids). `agent_ctx` supplies expansion
/// state for `AgentTask` cards; `tool_ctx` carries the workspace weak handle
/// for `ToolCall` cards to flip their own collapse flag. `None` renders them
/// in a static state with no-op clicks (used when the owning Workspace is gone).
///
/// `body` is the owned `Entity<Markdown>` for a top-level text body
/// (Assistant / Reasoning); `notice_panel` is the owned paginated
/// `TerminalPanel` for a `Notice` body. The recursive `render_item` calls for
/// embedded sub-messages pass `None` for both, falling back to a per-frame
/// `Entity<Markdown>`.
//
// Each arg is a distinct render input; the function is a leaf dispatch, not a
// public API. Bundling would only forward the same values through an
// intermediate struct without reducing complexity.
#[allow(clippy::too_many_arguments)]
pub fn render_item(
    item: &ConvItem,
    ix: usize,
    role: &str,
    theme: &Theme,
    agent_ctx: Option<&AgentTaskCtx>,
    tool_ctx: Option<&ToolCallCtx>,
    body: Option<Entity<Markdown>>,
    notice_panel: Option<Entity<TerminalPanel>>,
    weak: Option<WeakEntity<Workspace>>,
    cx: &mut App,
) -> gpui::AnyElement {
    match item {
        ConvItem::User { text, images, meta } => render_user(
            UserRenderContent {
                text,
                images,
                meta: meta.as_ref(),
            },
            ix,
            role,
            theme,
            body,
            cx,
        ),
        ConvItem::Assistant {
            text,
            streaming: _,
            token_usage: _,
            activity_header,
            entry_id,
            fork_unavailable,
        } => render_assistant(
            AssistantRenderContent {
                text,
                role,
                activity_header: *activity_header,
            },
            ix,
            theme,
            body,
            AssistantActions {
                entry_id: entry_id.clone(),
                weak,
                unavailable: *fork_unavailable,
            },
            cx,
        ),
        ConvItem::Thinking(t) => render_thinking(t, ix, role, theme, tool_ctx, cx),
        ConvItem::ToolCall(t) => {
            if t.name == manox_agent::tools::ASK_USER_QUESTION {
                render_ask_user_card(t, ix, theme, tool_ctx, cx)
            } else {
                // Ordinary tool calls fold into `Thinking`; a top-level
                // ToolCall here is the answered-state fallback for an
                // `AskUserQuestion` whose interactive snapshot is gone, or a
                // defensive orphan — render it as a plain card.
                render_tool_call(t, ix, theme, tool_ctx, cx)
            }
        }
        ConvItem::AgentTask(t) => render_agent_task(t, ix, theme, agent_ctx, tool_ctx, cx),
        ConvItem::Error(msg) => render_error(msg, ix, theme, body, cx),
        ConvItem::Notice(msg) => render_notice(msg, ix, theme, notice_panel, cx),
        ConvItem::Recap {
            summary,
            collapsed,
            user_toggled: _,
        } => render_recap(summary, *collapsed, ix, theme, tool_ctx, body, cx),
        ConvItem::Retry {
            attempt,
            max_attempts,
            delay_secs,
            reason,
            detail,
            collapsed,
            user_toggled: _,
        } => render_retry(
            *attempt,
            *max_attempts,
            *delay_secs,
            reason,
            detail.as_deref(),
            *collapsed,
            ix,
            theme,
            tool_ctx,
            body,
            cx,
        ),
        ConvItem::BackgroundTask(bt) => render_background_task(bt, ix, theme, tool_ctx, cx),
        ConvItem::CacheMiss { reprocessed_tokens } => {
            render_cache_miss(*reprocessed_tokens, ix, theme, cx)
        }
    }
}

/// Copy button: writes `text` to the clipboard on click.
fn copy_button(ix: usize, prefix: &'static str, text: String) -> Button {
    Button::new((prefix, ix))
        .ghost()
        .xsmall()
        .icon(IconName::Copy)
        .on_click(move |_, _, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        })
}

/// Copy button visible only when the parent element (group) is hovered.
/// The caller must attach `.group(name)` to the enclosing wrapper.
fn copy_button_hoverable(
    ix: usize,
    prefix: &'static str,
    group: impl Into<gpui::SharedString>,
    text: String,
) -> gpui::Div {
    let group = group.into();
    gpui::div()
        .opacity(0.0)
        .group_hover(group, |s| s.opacity(1.0))
        .child(copy_button(ix, prefix, text))
}

/// Display name of an agent in a turn header: the main agent uses the
/// localized Captain label (shared with the context rail), the host harness
/// keeps its own name, and named agents keep their manifest / member name
/// verbatim.
pub fn author_display(author: &manox_agent::MessageAuthor) -> String {
    match author {
        manox_agent::MessageAuthor::Lead => i18n::t("context-agents-captain").to_string(),
        manox_agent::MessageAuthor::Harness => i18n::t("message-harness-role").to_string(),
        manox_agent::MessageAuthor::Agent(name) => name.clone(),
    }
}

/// The user-turn header: `{from} > {to}·{model}·{time}`. `from` is the
/// message's real author (human input is unattributed); `to` is the agent
/// whose conversation renders the turn. Empty segments drop, and the `>`
/// clause disappears when nothing follows `from`.
fn user_turn_header(from: &str, to: &str, model: &str, time: &str) -> String {
    let tail = [to, model, time]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("·");
    if tail.is_empty() {
        from.to_string()
    } else {
        format!("{from} > {tail}")
    }
}

/// Render a user message as one full-width turn frame. The frame itself owns
/// the accent border; the bottom edge keeps only the two corners so the center
/// stays open. `min_w_0` end to end keeps long CJK / unbreakable runs from
/// collapsing the block to min-content (the failure mode of the old bubble).
struct UserRenderContent<'a> {
    text: &'a str,
    images: &'a [UserImage],
    meta: Option<&'a UserTurnMeta>,
}

fn render_user(
    content: UserRenderContent<'_>,
    ix: usize,
    model: &str,
    theme: &Theme,
    body: Option<Entity<Markdown>>,
    cx: &mut App,
) -> gpui::AnyElement {
    let UserRenderContent { text, images, meta } = content;
    let model_id = meta
        .map(|m| m.model_id.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(model);
    let sender = meta
        .and_then(|m| m.author.as_ref())
        .map(author_display)
        .unwrap_or_else(|| i18n::t("message-user-role").to_string());
    let recipient = meta
        .and_then(|m| m.recipient.as_ref())
        .map(author_display)
        .unwrap_or_default();
    let time = meta
        .map(|m| format_user_turn_time(m.timestamp))
        .unwrap_or_default();
    let header = user_turn_header(&sender, &recipient, model_id, &time);
    let group = format!("user-{ix}");
    // An inbound peer delivery keeps the hue the team banner used, so agent
    // chatter reads apart from the human's own turns.
    let accent = if meta.is_some_and(|m| m.peer) {
        theme.primary
    } else {
        meta.and_then(|m| m.approval_mode)
            .map(|mode| approval_mode_color(mode, theme))
            .unwrap_or(theme.accent)
    };

    // A persistent "steered" marker for user messages that entered the list
    // via the steer-queue drain (mid-turn injection) rather than starting a
    // fresh turn. Survives reload because it is read back from
    // `MessageUiMetadata::steered` in `from_message`. A steer still in flight
    // never reaches the list (it parks in the composer queue until settle), so
    // there is no live "pending" variant here.
    let steer_badge = meta.filter(|m| m.steered).map(|_| {
        gpui::div()
            .px_1()
            .py_0p5()
            .rounded(theme.radius)
            .bg(accent.opacity(0.15))
            .text_sm()
            .text_color(theme.accent_foreground)
            .child(i18n::t("message-steered-badge"))
    });

    let body_el = body_or_static(body, ("user-text", ix), text.to_string(), theme, cx);
    let mut header_el = h_flex()
        .items_center()
        .gap_1()
        .child(SharedString::from(header));
    if let Some(badge) = steer_badge {
        header_el = header_el.child(badge);
    }

    TurnFrame::new(theme)
        .group(group.clone())
        .accent(accent)
        .header(
            gpui::div()
                .text_color(theme.muted_foreground)
                .child(header_el),
        )
        .trailing(copy_button_hoverable(
            ix,
            "copy-user",
            group,
            text.to_string(),
        ))
        .child(
            v_flex()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .gap_2()
                .text_base()
                .text_color(theme.foreground)
                .children(images.iter().map(|ui| {
                    gpui::img(ui.0.clone())
                        .max_w(px(280.))
                        .max_h(px(280.))
                        .rounded(theme.radius)
                        .object_fit(gpui::ObjectFit::ScaleDown)
                }))
                .child(body_el),
        )
        .into_any_element()
}

fn approval_mode_color(mode: PermissionMode, theme: &Theme) -> gpui::Hsla {
    match mode {
        PermissionMode::ReadOnly => theme.warning,
        PermissionMode::WorkspaceWrite => theme.info,
        PermissionMode::DangerFullAccess => theme.danger,
    }
}

fn format_user_turn_time(timestamp: i64) -> String {
    let Some(sent) = Local.timestamp_opt(timestamp, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if sent.date_naive() == now.date_naive() {
        sent.format("%H:%M").to_string()
    } else if sent.year() == now.year() {
        sent.format("%m-%d %H:%M").to_string()
    } else {
        sent.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// The two things the assistant action row needs beyond the reply body: the
/// reply's durable journal entry id (the fork anchor) and the owning workspace
/// that can act on it. `entry_id` is `None` while a reply is still streaming
/// (no durable row exists yet), which withholds the branch control.
#[derive(Clone, Default)]
pub(crate) struct AssistantActions {
    pub entry_id: Option<String>,
    pub weak: Option<WeakEntity<Workspace>>,
    pub unavailable: Option<ForkUnavailable>,
}

/// Why a reply cannot be forked, in the user's terms — or `None` when it can.
/// The reason is resolved while building the item (where the journal context is
/// known), so the render layer only decides how to draw it: a disabled control
/// that cannot say what it wants is a dead end, so each reason carries copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ForkUnavailable {
    /// The reply has no durable row to anchor on yet — it is still streaming,
    /// or the turn stopped but the authoritative rebuild that carries the
    /// anchor has not landed. Either way the fork opens when the row does.
    NotLanded,
    /// The reply closes a step but not the turn: it carries a tool call whose
    /// result rides a later entry, so a prefix ending here would be replanted
    /// with a synthetic "No result provided" outcome.
    MidTurn,
    /// The row is not a faithful replay of a session journal (a sub-agent
    /// panel's answer backfill, a compaction retained tail), so its id names
    /// no entry of the session it would be forked from.
    NotReplayed,
}

impl ForkUnavailable {
    fn notice_key(self) -> &'static str {
        match self {
            Self::NotLanded => "message-fork-unavailable-not-landed",
            Self::MidTurn => "message-fork-unavailable-mid-turn",
            Self::NotReplayed => "message-fork-unavailable-not-replayed",
        }
    }
}

/// The assistant reply's own render inputs, grouped so the render entry point
/// keeps a readable signature (`render_user`'s `UserRenderContent` is the
/// precedent).
pub(crate) struct AssistantRenderContent<'a> {
    text: &'a str,
    role: &'a str,
    activity_header: bool,
}

/// Render an assistant message. A reply that follows an activity segment
/// (`activity_header`) suppresses its own model row — the segment's header
/// already carries the model name. The action row (copy / fork) always renders
/// beneath the body, never overlaid on it.
pub(crate) fn render_assistant(
    content: AssistantRenderContent<'_>,
    ix: usize,
    theme: &Theme,
    body: Option<Entity<Markdown>>,
    actions: AssistantActions,
    cx: &mut App,
) -> gpui::AnyElement {
    let AssistantRenderContent {
        text,
        role,
        activity_header,
    } = content;
    // Owned `Entity<Markdown>` (persistent → selection + streaming survive);
    // fall back to a per-frame mount for embedded sub-message bodies.
    let body_el = match body {
        Some(md) => md.into_any_element(),
        None => markdown_tv(("assistant", ix), text.to_string(), theme, false, cx),
    };
    let group = format!("assistant-{ix}");
    let mut col = v_flex().group(group.clone()).w_full().min_w_0().gap_1();
    if !activity_header {
        col = col.child(
            h_flex().w_full().min_w_0().gap_1().items_center().child(
                gpui::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(role.to_string()),
            ),
        );
    }
    // The action row sits under the body (never overlaid on it), matching the
    // reference web layout: copy / fork / usage / elapsed / clock.
    col.child(
        gpui::div()
            .relative()
            .w_full()
            .min_w_0()
            .overflow_x_hidden()
            .child(body_el),
    )
    .child(assistant_action_row(ix, &group, text, actions))
    .into_any_element()
}

/// The assistant reply's action row: copy then fork, rendered beneath the body
/// so no control ever covers prose. The whole row is hover-revealed like every
/// other transcript affordance.
///
/// The fork control is always present so the row keeps a stable shape: when the
/// reply cannot be forked it stays visible and disabled, its tooltip naming the
/// reason. A control that disappears cannot teach the rule it enforces.
fn assistant_action_row(
    ix: usize,
    group: &str,
    text: &str,
    actions: AssistantActions,
) -> gpui::Div {
    let mut row = h_flex()
        .w_full()
        .min_w_0()
        .gap_1()
        .items_center()
        .opacity(0.0)
        .group_hover(group.to_string(), |s| s.opacity(1.0))
        .child(copy_button(ix, "copy-assistant", text.to_string()));
    // Fork additionally needs a live owner to route the call through; a
    // dropped workspace leaves copy as the row's only actionable control.
    if let Some(weak) = actions.weak {
        row = row.child(fork_button(ix, weak, actions.entry_id, actions.unavailable));
    }
    row
}

/// Fork this session at the reply this button belongs to, or — when the reply
/// is not a forkable anchor — the same control, disabled, explaining why.
///
/// A fork runs through the owning `Workspace` (which owns the outgoing call and
/// the child bind), so the row carries only the durable anchor id.
fn fork_button(
    ix: usize,
    weak: WeakEntity<Workspace>,
    entry_id: Option<String>,
    unavailable: Option<ForkUnavailable>,
) -> Button {
    let Some(entry_id) = entry_id.filter(|_| unavailable.is_none()) else {
        let reason = unavailable.unwrap_or(ForkUnavailable::NotReplayed);
        return Button::new(("fork-assistant", ix))
            .ghost()
            .xsmall()
            .icon(gpui_kit_assets::IconName::GitBranch)
            .tooltip(i18n::t(reason.notice_key()))
            .disabled(true);
    };
    Button::new(("fork-assistant", ix))
        .ghost()
        .xsmall()
        .icon(gpui_kit_assets::IconName::GitBranch)
        .tooltip(i18n::t("message-fork-here"))
        .debug_selector(move || format!("fork-assistant-{ix}"))
        .on_click(move |_, _window, cx: &mut App| {
            let _ = weak.update(cx, |workspace, cx| {
                workspace.fork_session_at(&entry_id, cx);
            });
        })
}

/// Optional collapsible slot for `render_banner`: turns the label row into a
/// click-toggle that shows/hides the body. Used by the recap card; `None` for
/// the always-open banners (error / notice / team message / retry).
struct CollapsibleBanner {
    collapsed: bool,
    on_click: Box<dyn Fn(&mut App) + 'static>,
}

/// Unified banner card: a label row (accent-colored label + optional icon,
/// hover-revealed copy button, optional collapse chevron) over a foreground-
/// tinted body. All five non-card banners (error / notice / team message /
/// recap / retry) share this shape so only accent, label, icon, body, and
/// collapse differ between them.
// The parameter list is intentionally rich: each param maps to one slot the
// five call sites need to differentiate (accent / label / icon / group /
// copy / body / fold). Grouping them into a config struct would obscure the
// per-call-site differences the unification is meant to make visible.
#[allow(clippy::too_many_arguments)]
fn render_banner(
    accent: gpui::Hsla,
    label: SharedString,
    icon: Option<gpui::AnyElement>,
    group: impl Into<SharedString>,
    ix: usize,
    copy_prefix: &'static str,
    copy_text: String,
    body: gpui::AnyElement,
    theme: &Theme,
    collapsible: Option<CollapsibleBanner>,
) -> gpui::AnyElement {
    let group = group.into();
    let mut left = h_flex()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .items_center()
        .gap_1()
        .text_sm()
        .text_color(accent);
    if let Some(c) = &collapsible {
        let chevron = if c.collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };
        left = left.child(Icon::new(chevron).xsmall());
    }
    left = left.when_some(icon, |row, el| row.child(el));
    left = left.child(label);

    let label_row = h_flex()
        .w_full()
        .min_w_0()
        .justify_between()
        .items_center()
        .child(left)
        .child(copy_button_hoverable(
            ix,
            copy_prefix,
            group.clone(),
            copy_text,
        ));
    // `.id()` turns `Div` into `Stateful<Div>`, so erase to `AnyElement` to
    // keep the collapsible and non-collapsible branches one type. Read the
    // collapsed flag before `on_click` moves the closure out of `collapsible`.
    let show_body = match &collapsible {
        Some(c) => !c.collapsed,
        None => true,
    };
    let label_row: gpui::AnyElement = match collapsible {
        Some(c) => label_row
            .id(("banner-header", ix))
            .cursor_pointer()
            .on_click(move |_, _window, cx: &mut App| (c.on_click)(cx))
            .into_any_element(),
        None => label_row.into_any_element(),
    };

    let mut card = v_flex()
        .group(group)
        .w_full()
        .min_w_0()
        .gap_1()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .bg(accent.opacity(0.10))
        .child(label_row);
    if show_body {
        card = card.child(
            gpui::div()
                .w_full()
                .min_w_0()
                .text_base()
                .text_color(theme.foreground)
                .child(body),
        );
    }
    card.into_any_element()
}

/// Render an error message + copy button.
pub fn render_error(
    msg: &str,
    ix: usize,
    theme: &Theme,
    body: Option<Entity<Markdown>>,
    cx: &mut App,
) -> gpui::AnyElement {
    render_banner(
        theme.danger,
        i18n::t("message-error"),
        None,
        format!("error-{ix}"),
        ix,
        "copy-error",
        msg.to_string(),
        body_or_static(body, ("error", ix), msg.to_string(), theme, cx),
        theme,
        None,
    )
}
/// Render an ephemeral system notice — status toggles, slash-command acks.
/// Neutral tones so positive state changes (e.g. a mode switch) do not
/// read as a runtime error. The body is the persistent paginated
/// `TerminalPanel` (`notice_panel`) — the same folded surface as tool output,
/// defaulting to `PAGE_SIZE` lines with a `+N` load-more row. The defensive
/// fallback (the panel is mounted synchronously for every `Notice` item)
/// renders plain text so the notice body never falls back to markdown
/// interpretation.
pub fn render_notice(
    msg: &str,
    ix: usize,
    theme: &Theme,
    notice_panel: Option<Entity<TerminalPanel>>,
    _cx: &mut App,
) -> gpui::AnyElement {
    render_banner(
        theme.muted_foreground,
        i18n::t("message-notice"),
        None,
        format!("notice-{ix}"),
        ix,
        "copy-notice",
        msg.to_string(),
        notice_panel
            .map(|p| p.into_any_element())
            .unwrap_or_else(|| gpui::div().child(msg.to_string()).into_any_element()),
        theme,
        None,
    )
}

/// Render a compaction Recap card: a collapsible summary of the history that
/// was folded into a handoff note. Collapsed by default; the summary body is
/// model-generated markdown (not localized), only the title is. Toggling
/// follows the same `user_toggled`-stamped pattern as reasoning blocks.
pub fn render_recap(
    summary: &str,
    collapsed: bool,
    ix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    body: Option<Entity<Markdown>>,
    cx: &mut App,
) -> gpui::AnyElement {
    let weak_workspace = tool_ctx.map(|c| c.weak.clone());
    let on_click = Box::new(move |_cx: &mut App| {
        let Some(weak) = weak_workspace.clone() else {
            return;
        };
        let ix_click = ix;
        let _ = weak.update(_cx, |w, cx| {
            let conv = w.conversation.clone();
            conv.update(cx, |c, cx| {
                if let Some(item) = c.items().get(ix_click) {
                    item.update(cx, |item, cx| {
                        if let ConvItem::Recap {
                            collapsed,
                            user_toggled,
                            ..
                        } = item.kind_mut()
                        {
                            *collapsed = !*collapsed;
                            *user_toggled = true;
                        }
                        cx.notify();
                    });
                }
            });
            cx.notify();
        });
    }) as Box<dyn Fn(&mut App) + 'static>;
    render_banner(
        theme.muted_foreground,
        i18n::t("recap-card-title"),
        Some(Icon::new(IconName::BookOpen).xsmall().into_any_element()),
        format!("recap-{ix}"),
        ix,
        "copy-recap",
        summary.to_string(),
        body_or_static(body, ("recap", ix), summary.to_string(), theme, cx),
        theme,
        Some(CollapsibleBanner {
            collapsed,
            on_click,
        }),
    )
}

/// Transient retry badge shown while the provider backs off after a 429 / 5xx
/// / network error. Replaced in place by the first real content or terminal
/// error event. Amber-toned to read as "waiting, not failed". The badge line
/// carries a short `reason` (HTTP status phrase or network error class); when a
/// provider response body is available it lands in an expandable detail slot
/// below, toggled by the same `user_toggled`-stamped pattern as recap cards.
// Mirrors render_banner: each param maps to one banner slot (attempt/max/secs
// for the badge, reason/detail for the body, collapsed/tool_ctx for the fold).
#[allow(clippy::too_many_arguments)]
pub fn render_retry(
    attempt: u32,
    max_attempts: u32,
    delay_secs: u64,
    reason: &str,
    detail: Option<&str>,
    collapsed: bool,
    ix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    body: Option<Entity<Markdown>>,
    cx: &mut App,
) -> gpui::AnyElement {
    let badge: SharedString = i18n::t_str(
        "retry-badge",
        &[
            ("attempt", &attempt.to_string()),
            ("max", &max_attempts.to_string()),
            ("secs", &delay_secs.to_string()),
            ("reason", reason),
        ],
    );
    let copy_text = badge.to_string();
    let weak_workspace = tool_ctx.map(|c| c.weak.clone());
    let on_click = Box::new(move |_cx: &mut App| {
        let Some(weak) = weak_workspace.clone() else {
            return;
        };
        let _ = weak.update(_cx, |w, cx| {
            let conv = w.conversation.clone();
            conv.update(cx, |c, cx| {
                if let Some(item) = c.items().get(ix) {
                    item.update(cx, |item, cx| {
                        if let ConvItem::Retry {
                            collapsed,
                            user_toggled,
                            ..
                        } = item.kind_mut()
                        {
                            *collapsed = !*collapsed;
                            *user_toggled = true;
                        }
                        cx.notify();
                    });
                }
            });
            cx.notify();
        });
    }) as Box<dyn Fn(&mut App) + 'static>;
    let body_el = detail
        .map(|d| body_or_static(body, ("retry", ix), d.to_string(), theme, cx))
        .unwrap_or_else(|| gpui::div().into_any_element());
    let collapsible = if detail.is_some() {
        Some(CollapsibleBanner {
            collapsed,
            on_click,
        })
    } else {
        None
    };
    render_banner(
        theme.warning,
        badge,
        Some(
            BrailleSpinner::new()
                .xsmall()
                .color(theme.warning)
                .into_any_element(),
        ),
        format!("retry-{ix}"),
        ix,
        "copy-retry",
        copy_text,
        body_el,
        theme,
        collapsible,
    )
}

/// Heuristic: map a tool name to a markdown code-block language tag so that
/// syntax highlighting can colour the output.
fn lang_hint_for_tool(name: &str) -> Option<&'static str> {
    match name {
        x if x == manox_agent::tools::BASH => Some("bash"),
        "python" => Some("python"),
        _ => None,
    }
}
/// Aggregated per-kind counts rendered on a segment's cover row.
struct SegmentStats {
    thinking_rounds: usize,
    /// Tool call counts in first-appearance order.
    tools: Vec<(String, usize)>,
    failed: usize,
    pending_approval: usize,
}

fn segment_stats(t: &ThinkingContainer) -> SegmentStats {
    let mut stats = SegmentStats {
        thinking_rounds: 0,
        tools: Vec::new(),
        failed: 0,
        pending_approval: 0,
    };
    for e in &t.entries {
        match e {
            ActivityEntry::Reasoning { .. } => stats.thinking_rounds += 1,
            ActivityEntry::Tool(tool) => {
                match stats.tools.iter_mut().find(|(name, _)| *name == tool.name) {
                    Some((_, count)) => *count += 1,
                    None => stats.tools.push((tool.name.clone(), 1)),
                }
                match tool.status {
                    ToolCallStatus::Error | ToolCallStatus::Denied => stats.failed += 1,
                    ToolCallStatus::PendingApproval => stats.pending_approval += 1,
                    _ => {}
                }
            }
        }
    }
    stats
}

/// The model display name on a segment header row. The assistant reply that
/// follows a segment renders no model row of its own, so the header is the
/// single place the turn's model shows.
fn segment_header_model(role: &str, theme: &Theme) -> gpui::Div {
    gpui::div()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(role.to_string())
}

/// Which entries of a segment render below its cover row, plus the cover's
/// aggregated counts (computed once per render).
struct SegmentLayout {
    /// False ⇒ too small to fold: entries render as a flat stack, no cover.
    cover: bool,
    /// True ⇒ every entry renders (user expanded, or an approval is pending).
    expanded: bool,
    visible: Vec<usize>,
    stats: SegmentStats,
}

fn segment_layout(t: &ThinkingContainer) -> SegmentLayout {
    let stats = segment_stats(t);
    if t.entries.len() < 2 {
        return SegmentLayout {
            cover: false,
            expanded: true,
            visible: (0..t.entries.len()).collect(),
            stats,
        };
    }
    // Awaiting approval is user-facing interaction: the segment stays open
    // even after auto-collapse so the pending row is never hidden (fail-closed
    // visibility at the UI layer).
    if !t.collapsed || stats.pending_approval > 0 {
        return SegmentLayout {
            cover: true,
            expanded: true,
            visible: (0..t.entries.len()).collect(),
            stats,
        };
    }
    // Collapsed hides every entry regardless of liveness — the header's
    // spinner, counts, and elapsed carry the live signal.
    SegmentLayout {
        cover: true,
        expanded: false,
        visible: Vec::new(),
        stats,
    }
}

/// Render an activity segment as a chain of thought: a header row carrying the
/// model display name plus the segment's cover (per-kind counts, elapsed,
/// failure and approval badges) over one step per entry. Collapsed shows the
/// header alone (live or settled); expanded lists every entry under it, each
/// step drawing the connector rail. Segments with fewer than two entries render
/// flat under a model-name-only header — folding a single row would only add a
/// click.
///
/// The block is controlled: `layout.expanded` (which already folds in the
/// pending-approval force-open) is the open state, and the click writes the
/// container's own `collapsed` / `user_toggled`. None of the segment's policy
/// moved into the component.
pub fn render_thinking(
    t: &ThinkingContainer,
    ix: usize,
    role: &str,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    cx: &mut App,
) -> gpui::AnyElement {
    if t.entries.is_empty() {
        // A freshly-created container before any entry has arrived renders
        // nothing — the first reasoning delta or tool call lands next frame.
        return gpui::div().into_any_element();
    }
    let layout = segment_layout(t);
    // The list caches each row's height, so the segment's fold stays instant:
    // a reveal that changes height frame by frame would go stale under it. The
    // chevron, the content fade, and each step's entrance are layout-neutral
    // and stay on.
    let mut chain = ChainOfThought::new(("activity-tree", ix))
        .open(layout.expanded)
        .animated(false);
    for &eix in &layout.visible {
        chain = chain.step(render_activity_entry(
            &t.entries[eix],
            eix,
            ix,
            theme,
            tool_ctx,
            cx,
        ));
    }

    if !layout.cover {
        // Too small to fold: the steps sit under a model-name-only row, with no
        // cover to click.
        return v_flex()
            .w_full()
            .min_w_0()
            .gap_0p5()
            .debug_selector(|| format!("message-overflow-activity-tree-{ix}"))
            .child(segment_header_model(role, theme))
            .child(chain)
            .into_any_element();
    }

    let stats = &layout.stats;
    let secs = if t.streaming {
        Some(t.started_at.elapsed().as_secs())
    } else {
        t.frozen_secs
    };
    let weak_workspace = tool_ctx.map(|c| c.weak.clone());
    let interactive = tool_ctx.is_some();

    // The header's label is the model display name — the segment is the single
    // place a turn's model shows, so it stays the row's identity rather than a
    // generic "Chain of Thought". Everything the cover used to carry rides the
    // meta slots, in the order it was shown.
    let mut header = ChainOfThoughtHeader::new(("activity-cover", ix)).label(role.to_string());
    if t.streaming {
        header = header.meta(
            BrailleSpinner::new()
                .xsmall()
                .color(theme.muted_foreground)
                .into_any_element(),
        );
    }
    if stats.thinking_rounds > 0 {
        header = header.meta(
            gpui::div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "{}×{}",
                    i18n::t("message-reasoning"),
                    stats.thinking_rounds
                )),
        );
    }
    for (name, count) in &stats.tools {
        header = header.meta(
            gpui::div()
                .text_sm()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(format!("{name}×{count}")),
        );
    }
    if let Some(secs) = secs
        && secs > 0
    {
        header = header.meta(
            gpui::div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(i18n::t_count("thinking-duration", secs as i64).to_string()),
        );
    }
    if stats.failed > 0 {
        header = header.meta(
            gpui::div()
                .text_sm()
                .text_color(theme.danger)
                .child(i18n::t_count("activity-failed", stats.failed as i64).to_string()),
        );
    }
    if stats.pending_approval > 0 {
        header = header.meta(gpui::div().text_sm().text_color(theme.warning).child(
            i18n::t_count("activity-awaiting-approval", stats.pending_approval as i64).to_string(),
        ));
    }
    chain = chain.header(header);

    if interactive {
        chain = chain.on_toggle(move |_, _window, cx: &mut App| {
            let Some(weak) = weak_workspace.clone() else {
                return;
            };
            let _ = weak.update(cx, |w, cx| {
                let conv = w.conversation.clone();
                conv.update(cx, |c, cx| {
                    if let Some(item) = c.items().get(ix) {
                        item.update(cx, |item, cx| {
                            if let ConvItem::Thinking(t) = item.kind_mut() {
                                t.collapsed = !t.collapsed;
                                t.user_toggled = true;
                            }
                            cx.notify();
                        });
                    }
                });
                cx.notify();
            });
        });
    }

    v_flex()
        .w_full()
        .min_w_0()
        .gap_0p5()
        .debug_selector(|| format!("message-overflow-activity-tree-{ix}"))
        .child(chain)
        .into_any_element()
}

/// Render one activity entry (a reasoning round or a tool node) as a step: the
/// entry's status marker in the step's marker column, its clickable row as the
/// step's label, and its body as the step's content. The step's id is the
/// selector the overflow test measures, so a step's painted bounds are what that
/// test compares.
fn render_activity_entry(
    e: &ActivityEntry,
    eix: usize,
    cix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    cx: &mut App,
) -> ChainOfThoughtStep {
    let id = format!("message-overflow-activity-entry-{cix}-{eix}");
    match e {
        ActivityEntry::Reasoning {
            id: entry_id,
            text,
            streaming,
            collapsed,
            user_toggled: _,
            markdown,
        } => reasoning_step(
            id,
            entry_id,
            text,
            *streaming,
            *collapsed,
            markdown.clone(),
            eix,
            cix,
            theme,
            tool_ctx,
            cx,
        ),
        ActivityEntry::Tool(tool) => tool_step(id, tool, eix, cix, theme, tool_ctx, cx),
    }
}

/// A reasoning round's step: the marker spins while the round streams, the body
/// is the persistent markdown document the streaming path keeps in sync.
// One arg per slot the step is assembled from (selector identity, fold data,
// body document, render positions for the debug selectors, host context);
// a config struct would only rename the same values.
#[allow(clippy::too_many_arguments)]
fn reasoning_step(
    id: String,
    entry_id: &str,
    text: &str,
    streaming: bool,
    collapsed: bool,
    markdown: Option<Entity<Markdown>>,
    eix: usize,
    cix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    cx: &mut App,
) -> ChainOfThoughtStep {
    let weak_workspace = tool_ctx.map(|c| c.weak.clone());
    let toggle_id = entry_id.to_string();
    let marker: gpui::AnyElement = if streaming {
        BrailleSpinner::new()
            .xsmall()
            .color(theme.muted_foreground)
            .into_any_element()
    } else {
        Icon::new(IconName::BookOpen)
            .xsmall()
            .text_color(theme.muted_foreground)
            .into_any_element()
    };

    // The row's structure — chevron, hover, click, one-line clipped title — is
    // the step's; this layer supplies the data and what a click means.
    let mut step = ChainOfThoughtStep::new(id)
        .icon(marker)
        .title(i18n::t("message-reasoning"))
        .disclosed(!collapsed, move |_, _window, cx: &mut App| {
            let Some(weak) = weak_workspace.clone() else {
                return;
            };
            let _ = weak.update(cx, |w, cx| {
                let id = toggle_id.clone();
                let conv = w.conversation.clone();
                conv.update(cx, |c, cx| {
                    // Address the round by its stable id, exactly like the
                    // tool step: the (container, entry) indices captured at
                    // render time can point at the wrong container by the
                    // time the click fires (a notice inserts above the
                    // segment; the toggle must never land on a stranger).
                    if let Some((cix, eix)) = c.find_thinking_entry(&id, &*cx)
                        && let Some(item) = c.items().get(cix)
                    {
                        item.update(cx, |item, cx| {
                            if let ConvItem::Thinking(t) = item.kind_mut()
                                && let Some(ActivityEntry::Reasoning {
                                    collapsed,
                                    user_toggled,
                                    ..
                                }) = t.entries.get_mut(eix)
                            {
                                *collapsed = !*collapsed;
                                *user_toggled = true;
                            }
                            cx.notify();
                        });
                    }
                });
                cx.notify();
            });
        });
    if !collapsed && !text.is_empty() {
        // The persistent `Entity<Markdown>` (synced by the streaming/rebuild
        // path) carries parse-once incremental parsing + document-level
        // selection; fall back to a per-frame mount only before the first sync.
        let body = match markdown {
            Some(md) => md.into_any_element(),
            None => markdown_tv(("reasoning-entry-body", eix), text, theme, false, cx),
        };
        step = step.content(
            gpui::div()
                .id(("reasoning-body", eix))
                .w_full()
                .min_w_0()
                .overflow_x_hidden()
                .py_1()
                .italic()
                .text_color(theme.muted_foreground)
                .debug_selector(move || format!("message-overflow-activity-entry-body-{cix}-{eix}"))
                .child(body),
        );
    }
    step
}

/// A tool call's step: the marker carries the call's status, the label is its
/// command summary, and the body is the terminal-styled output panel (which
/// keeps its own frame — the single bordered box is the panel's, not the row's).
fn tool_step(
    id: String,
    e: &ToolCallItem,
    eix: usize,
    cix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    cx: &mut App,
) -> ChainOfThoughtStep {
    use manox_agent::ToolCallStatus;
    let is_active = matches!(
        e.status,
        ToolCallStatus::PendingApproval | ToolCallStatus::Running
    );
    let status_color = match e.status {
        ToolCallStatus::PendingApproval | ToolCallStatus::Running => theme.muted_foreground,
        ToolCallStatus::Success | ToolCallStatus::Continued => theme.success,
        ToolCallStatus::Error | ToolCallStatus::Denied => theme.danger,
        ToolCallStatus::Cancelled => theme.muted_foreground,
    };
    let marker: gpui::AnyElement = if is_active {
        BrailleSpinner::new()
            .xsmall()
            .color(status_color)
            .into_any_element()
    } else {
        let icon = match e.status {
            ToolCallStatus::Success | ToolCallStatus::Continued => {
                Icon::default().path("icons/circle-check-big.svg")
            }
            ToolCallStatus::Error | ToolCallStatus::Denied => Icon::new(IconName::CircleX),
            ToolCallStatus::Cancelled => Icon::new(IconName::Minus),
            _ => unreachable!(),
        };
        icon.xsmall().text_color(status_color).into_any_element()
    };
    // Live tools play open; the delayed auto-collapse folds the output once
    // the result lands. The status marker still spins while running.
    let show_output = !e.collapsed;
    let title = if !e.title.is_empty() {
        e.title.clone()
    } else if !e.name.is_empty() {
        e.name.clone()
    } else {
        i18n::t("thinking-tool-result").to_string()
    };
    let id_for_toggle = e.id.clone();
    let weak_workspace = tool_ctx.map(|c| c.weak.clone());

    let mut step = ChainOfThoughtStep::new(id)
        .icon(marker)
        .title(title)
        .disclosed(!e.collapsed, move |_, _window, cx: &mut App| {
            let Some(weak) = weak_workspace.clone() else {
                return;
            };
            let _ = weak.update(cx, |w, cx| {
                let id = id_for_toggle.clone();
                let conv = w.conversation.clone();
                conv.update(cx, |c, cx| {
                    if let Some((cix, eix)) = c.find_thinking_entry(&id, &*cx)
                        && let Some(item) = c.items().get(cix)
                    {
                        item.update(cx, |item, cx| {
                            if let ConvItem::Thinking(t) = item.kind_mut()
                                && let Some(ActivityEntry::Tool(entry)) = t.entries.get_mut(eix)
                            {
                                entry.collapsed = !entry.collapsed;
                                entry.user_toggled = true;
                            }
                            cx.notify();
                        });
                    }
                });
                cx.notify();
            });
        });
    if show_output && !e.output.is_empty() {
        step = step.framed(true).content(
            v_flex()
                .w_full()
                .min_w_0()
                .debug_selector(move || format!("message-overflow-activity-entry-body-{cix}-{eix}"))
                .child(render_tool_output(e, eix, theme, cx)),
        );
    }
    step
}

/// Open a file path in VS Code. Strips `:line` and `:line-end` suffixes
/// from the path string and passes them as `--goto file:line` arguments.
/// Falls back to `open -a "Visual Studio Code"` if the `code` CLI is not
/// available.
fn open_file_in_vscode(raw: &str, cwd: Option<&Path>) {
    // Strip line-number suffix: `:42` or `:42-100`.
    let (file, line) = if let Some(colon) = raw.rfind(':') {
        let after = &raw[colon + 1..];
        if after.chars().all(|c| c.is_ascii_digit() || c == '-') {
            (PathBuf::from(&raw[..colon]), Some(after.to_string()))
        } else {
            (PathBuf::from(raw), None)
        }
    } else {
        (PathBuf::from(raw), None)
    };

    let resolved = if file.is_absolute() {
        file
    } else if let Some(cwd) = cwd {
        cwd.join(&file)
    } else {
        file
    };

    // Build --goto argument if we have a resolved path with an optional line.
    let goto = if let Some(ln) = &line {
        format!("{}:{}", resolved.display(), ln)
    } else {
        resolved.display().to_string()
    };

    // Try `code --goto <path>:<line>` first.
    let result = std::process::Command::new("code")
        .arg("--goto")
        .arg(&goto)
        .spawn();
    if result.is_err() {
        // Fall back to `open -a "Visual Studio Code"` on macOS.
        let _ = std::process::Command::new("open")
            .arg("-a")
            .arg("Visual Studio Code")
            .arg(&goto)
            .spawn();
    }
}

/// The ask card's render entry and presentation router. `pub(crate)` so the
/// test-support diagnostic probe (`Workspace::diagnostic_ask_card_element`)
/// can render the card outside the conversation list.
pub(crate) fn render_ask_user_card(
    item: &ToolCallItem,
    ix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    cx: &mut App,
) -> gpui::AnyElement {
    let Some(ctx) = tool_ctx else {
        return render_tool_call(item, ix, theme, tool_ctx, cx);
    };
    let Some(snapshot) = ctx.ask.clone() else {
        return render_tool_call(item, ix, theme, tool_ctx, cx);
    };
    if item.status != ToolCallStatus::PendingApproval {
        return render_tool_call(item, ix, theme, tool_ctx, cx);
    }

    // One takeover, two presentations: a plan review is one decision over one
    // plan, so it takes the decision card; every other ask takes the generic
    // stepper flow. The routing lives at the one entry that renders the card,
    // so neither presentation can claim an ask the other is already showing.
    if snapshot.total == 1
        && let Some(approve_ix) = plan_review_approve_index(&snapshot.question)
    {
        return render_plan_review_card(ix, theme, ctx.weak.clone(), &snapshot, approve_ix, cx);
    }
    render_question_card(ix, theme, ctx.weak.clone(), snapshot, cx)
}

/// The plan-review decision presentation's approve option: the ask must carry
/// a `plan-review` intent whose `approve` label names one of the question's
/// own options, and the question must be single-select — `decide_ask_option`
/// clears siblings, which would destroy answers on a multi-select. `None`
/// renders the generic question card. The label match runs against the
/// DISPLAY label — the server's `(Recommended)` suffix was already stripped
/// at parse — while the server validated the raw label, so a model-minted
/// intent can miss and degrade to the generic card. That asymmetry is the
/// intended safe fallback, not a bug to align away. (dsh's `planReviewOf`
/// also caps options at two, shaping its two-button panel; our footer is N
/// buttons over the gateway's three minted options, so that guard MUST NOT
/// be copied.)
fn plan_review_approve_index(question: &AskCardQuestion) -> Option<usize> {
    let intent = question.intent.as_ref()?;
    if intent.kind != "plan-review" || intent.approve.is_empty() || question.multi_select {
        return None;
    }
    question
        .options
        .iter()
        .position(|opt| opt.label == intent.approve)
}

/// The plan-review decision card: a tinted strip, the plan as the body that
/// owns the scroll (so the strip and the decision row stay reachable on a
/// long plan), and a fixed decision row at the BOTTOM — the approve option as
/// the primary button, the remaining options as outline buttons, and a quiet
/// discuss action carrying the dismissal leg. A click IS the verdict
/// (`decide_ask_option`): no selection-then-confirm two-step.
fn render_plan_review_card(
    ix: usize,
    theme: &Theme,
    weak: WeakEntity<Workspace>,
    snapshot: &AskCardSnapshot,
    approve_ix: usize,
    cx: &mut App,
) -> gpui::AnyElement {
    let weak_esc = weak.clone();
    let weak_discuss = weak.clone();

    let strip = h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .bg(theme.warning.opacity(0.12))
        .child(gpui::div().size(px(8.)).rounded_full().bg(theme.warning))
        .child(
            gpui::div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_sm()
                .text_color(theme.warning)
                .child(i18n::t("workspace-ask-plan-review-label")),
        );

    let lead = gpui::div()
        .w_full()
        .min_w_0()
        .text_base()
        .text_color(theme.foreground)
        .child(snapshot.question.question.clone());
    let plan_body = (!snapshot.question.detail.trim().is_empty()).then(|| {
        markdown_tv(
            format!("plan-review-detail-{ix}"),
            snapshot.question.detail.clone(),
            theme,
            false,
            cx,
        )
    });
    let body = gpui::div()
        .id(format!("plan-review-body-{ix}"))
        .debug_selector(move || format!("plan-review-body-{ix}"))
        .w_full()
        .min_w_0()
        .max_h(px(520.))
        .overflow_y_scroll()
        .px_3()
        .pt_2p5()
        .pb_1()
        .child(lead)
        .children(plan_body);

    let mut footer = h_flex()
        .debug_selector(move || format!("plan-review-footer-{ix}"))
        .w_full()
        .min_w_0()
        .items_center()
        .justify_end()
        .flex_wrap()
        .gap_2()
        .px_3()
        .pt_1p5()
        .pb_1()
        .child(
            Button::new(format!("plan-review-discuss-{ix}"))
                .ghost()
                .small()
                .icon(gpui_kit_assets::IconName::SquarePen)
                .label(i18n::t("workspace-ask-discuss"))
                .on_click(move |_, _, cx: &mut App| {
                    let _ = weak_discuss.update(cx, |w, cx| w.dismiss_ask(cx));
                }),
        );
    for (oi, opt) in snapshot.question.options.iter().enumerate() {
        if oi == approve_ix {
            continue;
        }
        let weak_decide = weak.clone();
        let mut button = Button::new(format!("plan-review-decide-{ix}-{oi}"))
            .outline()
            .small()
            .label(opt.label.clone())
            .on_click(move |_, _, cx: &mut App| {
                let _ = weak_decide.update(cx, |w, cx| w.decide_ask_option(0, oi, cx));
            });
        if !opt.description.trim().is_empty() {
            button = button.tooltip(opt.description.clone());
        }
        footer = footer.child(button);
    }
    let weak_approve = weak.clone();
    let approve = &snapshot.question.options[approve_ix];
    let mut approve_button = Button::new(format!("plan-review-approve-{ix}"))
        .primary()
        .small()
        .label(approve.label.clone())
        .on_click(move |_, _, cx: &mut App| {
            let _ = weak_approve.update(cx, |w, cx| w.decide_ask_option(0, approve_ix, cx));
        });
    if !approve.description.trim().is_empty() {
        approve_button = approve_button.tooltip(approve.description.clone());
    }
    footer = footer.child(approve_button);

    v_flex()
        .id(format!(
            "plan-review-card-{}-{}",
            snapshot.id, snapshot.transition_gen
        ))
        .key_context("AskDrawer")
        // The discuss action is the decision card's only exit. The AskDrawer
        // Esc binding is context-scoped and only lands while focus sits
        // INSIDE the card, and nothing on this card takes focus (gpui
        // buttons avoid focus on mouse-down) — a deliberate button-only
        // surface, same as dsh's PlanReviewPanel.
        .on_action(move |_: &crate::AskCancel, _window, cx: &mut App| {
            let _ = weak_esc.update(cx, |w, cx| w.dismiss_ask(cx));
        })
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .rounded(px(18.))
        .border_1()
        .border_color(theme.warning.opacity(0.45))
        .bg(theme.background)
        .shadow_lg()
        // The extra bottom padding plus negative margin lets the composer
        // cover the card tail, the same contract as the generic ask card.
        .pb_5()
        .mb(px(-10.))
        .child(strip)
        .child(body)
        .child(footer)
        .with_animation(
            format!("plan-review-slide-{}", snapshot.transition_gen),
            Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
            |el, delta| el.mt(px(8. * (1. - delta))).opacity(delta),
        )
        .into_any_element()
}

/// The generic multi-step question card: title + close in the header; the
/// question, its detail, the options and the custom supplement in a capped
/// scrollable body; and a fixed footer with the pager on the left and the
/// skip + next/submit actions on the right — the decision actions sit where
/// the reading finishes, never pinned above the content they settle.
fn render_question_card(
    ix: usize,
    theme: &Theme,
    weak: WeakEntity<Workspace>,
    snapshot: AskCardSnapshot,
    cx: &mut App,
) -> gpui::AnyElement {
    let weak_close = weak.clone();
    let weak_esc = weak.clone();
    let step = snapshot.step;
    let total = snapshot.total;
    let can_prev = step > 0;
    let can_next = step + 1 < total;
    // dsh parity: the primary is gated on the CURRENT question being answered
    // (a pick or typed custom). Without the gate, "submit an unanswered card"
    // hits the composer's silent submit gate and dies as a no-op the user
    // reads as a broken button; the footer Skip is the always-available way
    // through (it advances, or settles on the last question).
    let answered_current =
        snapshot.selections.iter().any(|s| *s) || !snapshot.custom.trim().is_empty();

    let title = question_card_title(&snapshot.question.header);

    let header = h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .justify_between()
        .child(
            gpui::div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .text_base()
                .text_color(theme.foreground)
                .child(title),
        )
        .child(
            Button::new(("ask-card-cancel", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .on_click(move |_, _, cx: &mut App| {
                    // Close (not deny): the dismissal marker, never
                    // the allow/deny exit the approval card owns.
                    let _ = weak_close.update(cx, |w, cx| w.dismiss_ask(cx));
                }),
        );

    let question_row = gpui::div()
        .w_full()
        .min_w_0()
        .text_base()
        .text_color(theme.foreground)
        .child(snapshot.question.question.clone());

    // B2-PR-1 L1: `detail` is optional markdown support text beneath the
    // question (the plan-review body rides here). Static, so a per-frame
    // `markdown_tv` mount is fine — no persistent selection across frames.
    let detail_block = (!snapshot.question.detail.trim().is_empty()).then(|| {
        markdown_tv(
            format!("ask-card-detail-{ix}-{step}"),
            snapshot.question.detail.clone(),
            theme,
            false,
            cx,
        )
    });

    let mut options_block = v_flex().w_full().min_w_0().gap_1p5();
    for (oi, opt) in snapshot.question.options.iter().enumerate() {
        let selected = snapshot.selections.get(oi).copied().unwrap_or(false);
        let indicator_size = px(15.);
        let indicator = if snapshot.question.multi_select {
            if selected {
                h_flex()
                    .size(indicator_size)
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.primary)
                    .bg(theme.primary.opacity(0.08))
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Check)
                            .xsmall()
                            .text_color(theme.primary),
                    )
            } else {
                h_flex()
                    .size(indicator_size)
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.border)
            }
        } else if selected {
            h_flex()
                .size(indicator_size)
                .rounded_full()
                .border_1()
                .border_color(theme.primary)
                .items_center()
                .justify_center()
                .child(gpui::div().size(px(8.)).rounded_full().bg(theme.primary))
        } else {
            h_flex()
                .size(indicator_size)
                .rounded_full()
                .border_1()
                .border_color(theme.border)
        };
        let weak_for_option = weak.clone();
        // B2-PR-5: a plan-review ask (single question, `intent.kind ==
        // "plan-review"`) highlights the affirmative option (`intent.approve`,
        // "Approve") so the verdict card reads at a glance. `intent.approve`
        // always names one of this question's own option labels (server
        // validated), so the match is exact.
        let is_approve_option = snapshot.question.intent.as_ref().is_some_and(|i| {
            i.kind == "plan-review" && !i.approve.is_empty() && i.approve == opt.label
        });
        let option_row = h_flex()
            .w_full()
            .min_w_0()
            .gap_2p5()
            .items_start()
            .px_2()
            .py_1p5()
            .rounded(px(10.))
            .when(selected, |row| row.bg(theme.accent.opacity(0.08)))
            .when(is_approve_option && !selected, |row| {
                row.bg(theme.primary.opacity(0.08))
                    .border_1()
                    .border_color(theme.primary.opacity(0.45))
            })
            .hover(|row| row.bg(theme.accent.opacity(0.06)))
            .id(gpui::SharedString::from(format!(
                "ask-card-opt-{ix}-{step}-{oi}"
            )))
            .cursor(CursorStyle::PointingHand)
            .on_click(move |_, _, cx: &mut App| {
                let _ = weak_for_option.update(cx, |w, cx| {
                    w.toggle_ask_option(step, oi, cx);
                });
            })
            .child(indicator)
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(
                                gpui::div()
                                    .flex_shrink_0()
                                    .text_base()
                                    .text_color(theme.foreground)
                                    .child(format!("{}.", oi + 1)),
                            )
                            .child(
                                gpui::div()
                                    .min_w_0()
                                    .overflow_x_hidden()
                                    .text_base()
                                    .text_color(theme.foreground)
                                    .child(opt.label.clone()),
                            )
                            .when(opt.recommended, |row| {
                                row.child(
                                    Tag::new()
                                        .with_variant(TagVariant::Secondary)
                                        .small()
                                        .child(i18n::t("workspace-ask-recommended")),
                                )
                            }),
                    )
                    .when(!opt.description.trim().is_empty(), |col| {
                        col.child(
                            gpui::div()
                                .mt_0p5()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(opt.description.clone()),
                        )
                    }),
            );
        options_block = options_block.child(option_row);
    }

    // B2-PR-1 L1 tri-state: a per-question free-text `custom` input (single
    // select — it overrides the selection at the settle fold; multi select —
    // it supplements it) plus an explicit per-question skip button (clears the
    // selection and the custom, settling `{selected: [], no custom}` — a skip,
    // distinct from closing the whole card). The `custom` entity is allocated on
    // the render path (see `ensure_ask_custom_inputs`) because an `InputState`
    // needs a `Window`; if it isn't present yet the row simply carries the skip.
    let custom_state: Option<Entity<InputState>> = weak
        .upgrade()
        .and_then(|ws| ws.read(cx).ask_custom_state(step));
    let custom_row = custom_state.map(|state| {
        h_flex().w_full().min_w_0().mt_0p5().child(
            Input::new(&state).appearance(false).prefix(
                gpui::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(i18n::t("workspace-ask-supplement-label")),
            ),
        )
    });

    // The body is the card's scrollport: a long detail (a plan body rides a
    // generic ask when the intent fallback fires) caps here instead of
    // pushing the footer's decision actions out of reach.
    let body = gpui::div()
        .id(format!("ask-card-body-{ix}-{step}"))
        .debug_selector(move || format!("ask-card-body-{ix}-{step}"))
        .w_full()
        .min_w_0()
        .max_h(px(520.))
        .overflow_y_scroll()
        .child(question_row)
        .children(detail_block)
        .child(options_block)
        .children(custom_row);

    let weak_prev = weak.clone();
    let weak_next = weak.clone();
    let weak_advance = weak.clone();
    let weak_submit = weak.clone();
    let weak_skip = weak.clone();
    let pager = h_flex()
        .items_center()
        .gap_1()
        .child(
            Button::new(("ask-card-prev", ix))
                .ghost()
                .xsmall()
                .icon(IconName::ChevronLeft)
                .disabled(!can_prev)
                .on_click(move |_, _, cx: &mut App| {
                    let _ = weak_prev.update(cx, |w, cx| w.ask_prev(cx));
                }),
        )
        .child(
            gpui::div()
                .min_w(px(44.))
                .text_center()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(format!("{} of {total}", step + 1)),
        )
        .child(
            Button::new(("ask-card-next", ix))
                .ghost()
                .xsmall()
                .icon(IconName::ChevronRight)
                .disabled(!can_next)
                .on_click(move |_, _, cx: &mut App| {
                    let _ = weak_advance.update(cx, |w, cx| w.ask_next(cx));
                }),
        );
    let primary_action = if can_next {
        Button::new(format!("ask-card-advance-{ix}-{step}"))
            .primary()
            .small()
            .disabled(!answered_current)
            .label(i18n::t("workspace-ask-next"))
            .on_click(move |_, _, cx: &mut App| {
                let _ = weak_next.update(cx, |w, cx| w.ask_next(cx));
            })
    } else {
        Button::new(format!("ask-card-submit-{ix}-{step}"))
            .primary()
            .small()
            .disabled(!answered_current)
            .label(i18n::t("workspace-ask-submit"))
            .on_click(move |_, window, cx: &mut App| {
                let _ = weak_submit.update(cx, |w, cx| w.submit_input(window, cx));
            })
    };
    let footer = h_flex()
        .debug_selector(move || format!("ask-card-footer-{ix}-{step}"))
        .w_full()
        .min_w_0()
        .items_center()
        .justify_between()
        .gap_2()
        .child(pager)
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new(format!("ask-card-skip-{ix}-{step}"))
                        .outline()
                        .small()
                        .label(i18n::t("workspace-ask-skip"))
                        .on_click(move |_, window, cx: &mut App| {
                            let _ =
                                weak_skip.update(cx, |w, cx| w.skip_ask_question(step, window, cx));
                        }),
                )
                .child(primary_action),
        );

    v_flex()
        .id(format!(
            "ask-card-{}-{}",
            snapshot.id, snapshot.transition_gen
        ))
        .key_context("AskDrawer")
        // Esc inside the drawer closes it unanswered — the same dismissal
        // leg as the X button, never the approval card's deny.
        .on_action(move |_: &crate::AskCancel, _window, cx: &mut App| {
            let _ = weak_esc.update(cx, |w, cx| w.dismiss_ask(cx));
        })
        .w_full()
        .min_w_0()
        .gap_2p5()
        .px_3()
        .pt_3()
        // The extra bottom padding plus negative margin lets the composer cover
        // the drawer tail, so the card reads as emerging from beneath it.
        .pb_5()
        .mb(px(-10.))
        .rounded(px(18.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .shadow_lg()
        .child(header)
        .child(body)
        .child(footer)
        .with_animation(
            format!("ask-card-slide-{}", snapshot.transition_gen),
            Animation::new(Duration::from_millis(180)).with_easing(ease_out_quint()),
            |el, delta| el.mt(px(8. * (1. - delta))).opacity(delta),
        )
        .into_any_element()
}

/// Render a plain tool-call card: title + status icon + copy button + (collapsible)
/// monospace output. Used only as the answered-state fallback for an
/// `AskUserQuestion` whose interactive snapshot is gone (and the defensive
/// orphan in `render_item`'s ToolCall dispatch). Ordinary tool calls no longer
/// reach this path — they fold into a `Thinking` batch via `render_thinking`.
pub fn render_tool_call(
    item: &ToolCallItem,
    ix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    cx: &mut App,
) -> gpui::AnyElement {
    use manox_agent::ToolCallStatus;
    let (status_color, status_label): (gpui::Hsla, SharedString) = match item.status {
        ToolCallStatus::PendingApproval => (theme.muted_foreground, i18n::t("status-pending")),
        ToolCallStatus::Running => (theme.muted_foreground, i18n::t("status-running")),
        ToolCallStatus::Success => (theme.success, i18n::t("status-success")),
        ToolCallStatus::Continued => (theme.muted_foreground, i18n::t("status-continued")),
        ToolCallStatus::Error => (theme.danger, i18n::t("status-error")),
        ToolCallStatus::Denied => (theme.danger, i18n::t("status-denied")),
        ToolCallStatus::Cancelled => (theme.muted_foreground, i18n::t("status-cancelled")),
    };

    let title = if item.title.is_empty() {
        item.name.clone()
    } else {
        item.title.clone()
    };

    let show_body = item.streaming || !item.collapsed;
    let chevron = if item.collapsed {
        IconName::ChevronRight
    } else {
        IconName::ChevronDown
    };

    let id_for_toggle = item.id.clone();
    let weak_workspace = tool_ctx.map(|c| c.weak.clone());

    // Tool-call chrome and output render as Lilex italic to set them apart
    // from upright body text (#140). This card is now only the AskUserQuestion
    // answered-state fallback + the defensive orphan; ordinary tools fold into
    // `render_activity_entry`, which carries the same italic. The header is the
    // terminal titlebar (command summary + status + copy + chevron), clicking it
    // toggles the body; the pair shares one bordered frame.
    let mut card = v_flex()
        .group(format!("tool-{ix}"))
        .w_full()
        .min_w_0()
        .italic()
        .border_1()
        .border_color(theme.border)
        .rounded(theme.radius)
        .overflow_hidden()
        .child(
            h_flex()
                .id(("tool-header", ix))
                .w_full()
                .min_w_0()
                .px_2()
                .py_1()
                .gap_1p5()
                .items_center()
                .when(show_body, |h| h.border_b_1().border_color(theme.border))
                .cursor_pointer()
                .hover(|s| s.bg(theme.secondary.opacity(0.5)))
                .on_click(move |_, _window, cx: &mut App| {
                    let Some(weak) = weak_workspace.clone() else {
                        return;
                    };
                    let _ = weak.update(cx, |w, cx| {
                        let id = id_for_toggle.clone();
                        let conv = w.conversation.clone();
                        conv.update(cx, |c, cx| {
                            if let Some(ix) = c.find_tool(&id, &*cx)
                                && let Some(item) = c.items().get(ix)
                            {
                                item.update(cx, |item, cx| {
                                    if let ConvItem::ToolCall(t) = item.kind_mut() {
                                        t.collapsed = !t.collapsed;
                                        t.user_toggled = true;
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        cx.notify();
                    });
                })
                .child(
                    Icon::new(chevron)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    gpui::div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.muted_foreground)
                        .child(truncate(&title, 80)),
                )
                .child(copy_button_hoverable(
                    ix,
                    "copy-tool",
                    format!("tool-{ix}"),
                    item.output.clone(),
                ))
                .child(
                    gpui::div()
                        .text_sm()
                        .text_color(status_color)
                        .child(status_label),
                ),
        );

    if show_body && !item.output.is_empty() {
        card = card.child(render_tool_output(item, ix, theme, cx));
    }
    card.into_any_element()
}

/// Fixed-height container with the tool's output. While streaming we paint a
/// plain monospace run (no markdown re-parse per chunk); once the final
/// `ToolResult` lands we mount the syntax-highlighted, scrollable `Markdown`.
/// The container keeps a deterministic height either way so the parent card
/// (and the list) reports a stable layout.
fn render_tool_output(
    item: &ToolCallItem,
    ix: usize,
    theme: &Theme,
    cx: &mut App,
) -> gpui::AnyElement {
    // B2-PR-2: once the server's model-facing ask result is canonical JSON
    // (`{"answers":[{"id","selected","custom"?}]}`), the answered-state card
    // body reads as raw JSON. Fold it back into compact human Q/A rows — the
    // question text is recovered from the call's own `input` by id. A parse
    // miss (a legacy prose result, an error, or a non-canonical payload) falls
    // through to the raw output path, so the display degrades gracefully. This
    // sits above the terminal-panel branch because the ask result is a small
    // structured payload, not shell text.
    if item.name == manox_agent::tools::ASK_USER_QUESTION
        && let Some(rows) = ask_result_qa_rows(&item.input, &item.output)
    {
        return render_ask_result_body(rows, ix, theme);
    }
    // Persistent terminal panel: the conversation handler mounts it at every
    // live output chunk, finalized result, and reloaded-history entry, so the
    // common path renders the `Entity<TerminalPanel>` directly — giving tool
    // output the same document-level selection + Cmd/Ctrl+C copy as message
    // bodies (drag/copy survive across frames) and rendering the body as a
    // terminal-styled shell (cwd / toolchain / `❯` command + ANSI-colored
    // output) rather than a per-frame fenced code block.
    if let Some(panel) = item.panel.clone() {
        return panel.into_any_element();
    }
    // Defensive fallback (panel not yet mounted): render the display output as
    // a fenced code block so the body still appears while selection degrades to
    // per-frame. The persistent panel is the supported path; this only fires
    // for paths the conversation handler doesn't sync (e.g. a freshly built
    // entry before the first `ToolOutput`).
    let display = if item.streaming {
        live_tail(&item.output)
    } else {
        item.output.clone()
    };
    // The panel gets the body verbatim: a `read_file` result is the model-facing
    // hashline shape, and `PanelKind::Numbered` is what presents it — the file's
    // own line numbers, gap markers unnumbered. This fallback paints no panel, so
    // it asks `manox-components` for the same parse's display text rather than
    // reading the envelope a second time here; either way the user never sees
    // `[path#TAG]` or an `N:` gutter.
    let display = if item.name == manox_agent::tools::READ {
        hashline_text(&display)
    } else {
        display
    };
    let lang = lang_hint_for_tool(&item.name);
    let code = if let Some(l) = lang {
        format!("```{l}\n{display}\n```")
    } else {
        format!("```\n{display}\n```")
    };
    let container = gpui::div()
        .id(("tool-output", ix))
        .w_full()
        .min_w_0()
        .debug_selector(|| format!("message-overflow-tool-output-{ix}"))
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(theme.border)
        .text_color(theme.muted_foreground);
    container
        .child(markdown_tv(
            ("tool-output-text", ix),
            code,
            theme,
            false,
            cx,
        ))
        .into_any_element()
}

/// Parse an ask tool's canonical result JSON (`{"answers":[{"id","selected",
/// "custom"?}]}`) into `(question, answer)` display rows. The question text is
/// recovered from the call's own `input` by matching `id`; an id the input
/// doesn't name falls back to the id itself (never a wrong question). Tri-state
/// fold mirrors the server: an explicit skip (empty `selected`, no `custom`)
/// shows a marked non-answer, `custom` alone is the whole answer, `selected` +
/// `custom` shows the picks with the note appended. Returns `None` for any
/// non-canonical payload (prose result, error text, malformed JSON) so the
/// caller falls back to the raw output.
fn ask_result_qa_rows(input: &serde_json::Value, output: &str) -> Option<Vec<(String, String)>> {
    use serde_json::Value;
    let trimmed = output.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let answers = value.get("answers")?.as_array()?;
    if answers.is_empty() {
        return None;
    }
    let questions = input.get("questions").and_then(|q| q.as_array());
    let question_text = |id: &str| -> String {
        questions
            .and_then(|arr| {
                arr.iter()
                    .find(|q| q.get("id").and_then(Value::as_str) == Some(id))
                    .and_then(|q| {
                        q.get("question")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
            })
            .unwrap_or_else(|| id.to_string())
    };
    let mut rows = Vec::with_capacity(answers.len());
    for answer in answers {
        let id = answer.get("id").and_then(Value::as_str)?;
        let selected: Vec<&str> = answer
            .get("selected")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let custom = answer
            .get("custom")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|c| !c.is_empty());
        let rendered = match (selected.is_empty(), custom) {
            // An explicit skip is a language-neutral em dash — no locale key,
            // and visually distinct from a real (possibly empty-ish) answer.
            (true, None) => "—".to_string(),
            (true, Some(c)) => c.to_string(),
            (false, None) => selected.join(", "),
            (false, Some(c)) => format!("{} · {c}", selected.join(", ")),
        };
        rows.push((question_text(id), rendered));
    }
    Some(rows)
}

/// Render the folded ask Q/A rows as a bordered card body (a compact two-
/// column question / answer table), replacing the raw JSON the model now sees.
fn render_ask_result_body(
    rows: Vec<(String, String)>,
    ix: usize,
    theme: &Theme,
) -> gpui::AnyElement {
    let container = v_flex()
        .id(format!("ask-result-{ix}"))
        .w_full()
        .min_w_0()
        .gap_1p5()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(theme.border);
    let body = rows.into_iter().fold(container, |col, (question, answer)| {
        col.child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .items_start()
                .child(
                    gpui::div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(question),
                )
                .child(
                    gpui::div()
                        .max_w(px(300.))
                        .text_right()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(answer),
                ),
        )
    });
    body.into_any_element()
}

fn agent_terminal_icon(status: ToolCallStatus) -> Icon {
    use manox_agent::ToolCallStatus;
    match status {
        ToolCallStatus::Success | ToolCallStatus::Continued => {
            Icon::default().path("icons/circle-check-big.svg")
        }
        ToolCallStatus::Error | ToolCallStatus::Denied => Icon::new(IconName::CircleX),
        ToolCallStatus::Cancelled => Icon::new(IconName::Minus),
        ToolCallStatus::Running | ToolCallStatus::PendingApproval => {
            unreachable!("running statuses use BrailleSpinner, not an icon")
        }
    }
}

/// Status indicator for a sub-agent task row. Running/Pending get a braille
/// spinner; terminal states get a static icon.
fn agent_status_indicator(status: ToolCallStatus, theme: &Theme) -> gpui::AnyElement {
    use manox_agent::ToolCallStatus;
    let color = match status {
        ToolCallStatus::Running | ToolCallStatus::PendingApproval => theme.accent_foreground,
        ToolCallStatus::Success | ToolCallStatus::Continued => theme.success,
        ToolCallStatus::Error | ToolCallStatus::Denied => theme.danger,
        ToolCallStatus::Cancelled => theme.muted_foreground,
    };
    let is_active = matches!(
        status,
        ToolCallStatus::Running | ToolCallStatus::PendingApproval
    );
    if is_active {
        BrailleSpinner::new()
            .xsmall()
            .color(color)
            .into_any_element()
    } else {
        agent_terminal_icon(status)
            .xsmall()
            .text_color(color)
            .into_any_element()
    }
}

/// Render a sub-agent task as a single-line clickable item:
/// `[status_icon] {subagent_type} · {description}`.
/// No expand/collapse, no inline body, no metrics chip. Clicking opens the
/// sub-agent's observation panel in the right pane (live transcript while
/// running, final answer after a reload).
pub fn render_agent_task(
    item: &AgentTaskItem,
    ix: usize,
    theme: &Theme,
    agent_ctx: Option<&AgentTaskCtx>,
    _tool_ctx: Option<&ToolCallCtx>,
    _cx: &mut App,
) -> gpui::AnyElement {
    let icon_el = agent_status_indicator(item.status, theme);

    // "Type · topic"; the call id is the last resort when both are empty.
    let display_title =
        crate::views::subagents::task_display_title(&item.subagent_type, &item.description)
            .unwrap_or_else(|| item.id.clone());

    let tooltip_text = display_title.clone();
    let open_target = agent_ctx.map(|ctx| {
        (
            ctx.weak.clone(),
            item.id.clone(),
            item.subagent_type.clone(),
            item.description.clone(),
            item.status,
        )
    });
    let row = h_flex()
        .id(("agent-row", ix))
        .debug_selector(move || format!("message-overflow-agent-row-{ix}"))
        .w_full()
        .min_w_0()
        .px_2()
        .py_1()
        .gap_1p5()
        .items_center()
        .rounded(theme.radius)
        .cursor_pointer()
        .hover(|s| s.bg(theme.secondary.opacity(0.5)))
        .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
        .on_click(move |_, _window, cx| {
            let Some((weak, id, subagent_type, topic, status)) = &open_target else {
                return;
            };
            if let Some(ws) = weak.upgrade() {
                ws.update(cx, |ws, cx| {
                    ws.open_subagent_tab(id, subagent_type, topic, *status, cx);
                });
            }
        });

    row.child(icon_el)
        .child(
            gpui::div()
                .debug_selector(move || format!("message-overflow-agent-title-{ix}"))
                .flex_1()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .text_sm()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(display_title),
        )
        .into_any_element()
}

/// Render a background task status card. Shows the task kind icon,
/// description, task ID, status badge, event/byte counts, and a Stop
/// button while running.
fn render_background_task(
    bt: &BackgroundTaskItem,
    ix: usize,
    theme: &Theme,
    tool_ctx: Option<&ToolCallCtx>,
    _cx: &mut App,
) -> gpui::AnyElement {
    use manox_agent::background_task::{TaskKind, TaskStatus};

    let is_running = matches!(bt.status, TaskStatus::Running | TaskStatus::Stopping);
    let kind_str = match bt.kind {
        TaskKind::MonitorCommand => i18n::t("background-task-kind-command"),
        TaskKind::MonitorWebSocket => i18n::t("background-task-kind-websocket"),
        TaskKind::BackgroundBash => i18n::t("background-task-kind-bash"),
        TaskKind::Subagent => i18n::t("background-task-kind-subagent"),
    };
    let status_str = match bt.status {
        TaskStatus::Running => i18n::t("background-task-status-running"),
        TaskStatus::Stopping => i18n::t("background-task-status-stopping"),
        TaskStatus::Completed => i18n::t("background-task-status-completed"),
        TaskStatus::Failed => i18n::t("background-task-status-failed"),
        TaskStatus::TimedOut => i18n::t("background-task-status-timed-out"),
        TaskStatus::Stopped => i18n::t("background-task-status-stopped"),
        TaskStatus::SessionEnded => i18n::t("background-task-status-session-ended"),
    };
    let icon_color = match bt.status {
        TaskStatus::Running | TaskStatus::Stopping => theme.accent_foreground,
        TaskStatus::Completed => theme.success,
        TaskStatus::Failed | TaskStatus::TimedOut => theme.danger,
        TaskStatus::Stopped | TaskStatus::SessionEnded => theme.muted_foreground,
    };
    // A background bash description is the full command, heredoc body
    // included — the title keeps only its first line; the complete text is
    // one hover away via the title's tooltip.
    let first_line = bt.description.lines().next().unwrap_or("").trim();
    let title = if first_line.is_empty() {
        kind_str.to_string()
    } else {
        format!("{kind_str} · {first_line}")
    };
    let title_tooltip = (!bt.description.trim().is_empty()).then(|| bt.description.clone());
    let mut status_text = format!(
        "{status_str} · {task_id} · {events} events · {bytes} bytes",
        task_id = bt.task_id,
        events = bt.event_count,
        bytes = bt.total_bytes,
    );
    if let Some(code) = bt.exit_code {
        status_text.push_str(&format!(" · exit {code}"));
    }
    let detail = bt.failure_summary.clone().or_else(|| {
        bt.recent_events
            .last()
            .map(|event| event.trim().to_string())
            .filter(|event| !event.is_empty())
    });
    let _ = tool_ctx;
    let task_id_for_stop = bt.task_id.clone();

    let row = h_flex()
        .id(("bg-task", ix))
        .debug_selector(move || format!("message-overflow-bg-task-row-{ix}"))
        .w_full()
        .min_w_0()
        .px_2()
        .py_1p5()
        .gap_1p5()
        .items_center()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .when(is_running, |s| {
            // The card row centers its children; pin the status indicator to
            // the title's first line, not the two-line column's middle.
            s.child(
                gpui::div()
                    .self_flex_start()
                    .child(BrailleSpinner::new().xsmall().color(icon_color)),
            )
        })
        .when(!is_running, |s| {
            let icon: Icon = match bt.status {
                TaskStatus::Completed => Icon::default().path("icons/circle-check-big.svg"),
                TaskStatus::Failed | TaskStatus::TimedOut => Icon::new(IconName::CircleX),
                TaskStatus::Stopped | TaskStatus::SessionEnded => Icon::new(IconName::Minus),
                _ => unreachable!(),
            };
            s.child(
                gpui::div()
                    .self_flex_start()
                    .child(icon.xsmall().text_color(icon_color)),
            )
        })
        .child(
            // flex_1 + min_w_0 give the text divs a definite width so
            // text_ellipsis actually truncates; without them a nowrap line
            // sizes to its content and paints past the card border.
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    gpui::div()
                        .id(("bg-task-title", ix))
                        .debug_selector(move || format!("message-overflow-bg-task-title-{ix}"))
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_family(theme.mono_font_family.clone())
                        .text_color(theme.foreground)
                        .child(title)
                        .when_some(title_tooltip, |d, text| {
                            d.tooltip(move |window, cx| {
                                Tooltip::new(text.clone()).build(window, cx)
                            })
                        }),
                )
                .child(
                    gpui::div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .whitespace_nowrap()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(status_text),
                )
                // The failure summary is the only UI surface for a task's
                // error text, so the detail wraps in full (bounded to 2048
                // chars at the source) instead of truncating to one line.
                .when_some(detail, |column, detail| {
                    column.child(
                        gpui::div()
                            .debug_selector(move || format!("message-overflow-bg-task-detail-{ix}"))
                            .w_full()
                            .min_w_0()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(detail),
                    )
                }),
        );

    // Stop button while running.
    if is_running {
        row.child(
            Button::new(("stop-bg-task", ix))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .label(i18n::t("background-task-stop"))
                .on_click({
                    let task_id = task_id_for_stop.clone();
                    move |_, _window, _cx: &mut App| {
                        let task_id = task_id.clone();
                        manox_agent::runtime::handle().spawn(async move {
                            let _ = manox_agent::background_task::stop(&task_id).await;
                        });
                    }
                }),
        )
        .into_any_element()
    } else {
        row.into_any_element()
    }
}

/// Slim cache-miss divider matching oh-my-pi's `CacheInvalidationMarkerComponent`:
///
/// ```text
/// ────────── ⊘ cache miss · 50.9k tokens
/// ```
fn render_cache_miss(
    reprocessed_tokens: u64,
    _ix: usize,
    theme: &Theme,
    _cx: &mut App,
) -> gpui::AnyElement {
    let rule_width = 10;
    let tokens_str = crate::cockpit::format_tokens(reprocessed_tokens);
    let label = i18n::t_str("cache-miss-label", &[("tokens", &tokens_str)]);
    let rule = "\u{2500}".repeat(rule_width);
    let line = format!("{rule} {label}");
    gpui::div()
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(line)
        .into_any_element()
}

/// Map a tool call to its panel rendering kind and the body text the panel
/// renders. `read_file` → `Numbered` (the panel owns the model-facing hashline
/// shape, so the file's own line numbers survive; see
/// `manox_components::markdown::PanelKind`). `write_file` → `File`, fed the
/// written content from the tool input. `edit_file` → `Diff` (the panel
/// classifies the `+`/`-`/`@@` lines). Anything else → `Plain` (ANSI-parsed
/// command output). Streaming bodies take the live tail so the most recent
/// lines are in view as they stream in.
fn tool_panel_body(entry: &ToolCallItem) -> (PanelKind, String) {
    let raw = if entry.streaming {
        live_tail(&entry.output)
    } else {
        entry.output.clone()
    };
    match entry.name.as_str() {
        x if x == manox_agent::tools::READ => (PanelKind::Numbered, raw),
        // write_file's `output` is a one-line confirmation ("Wrote N bytes"), not
        // the file content; the content lives in the tool input. Show the written
        // content with a line-number gutter on success. On failure (`is_error`)
        // `output` carries the error — surface that as plain text via the default
        // arm so the user sees what went wrong, not just what was attempted.
        x if x == manox_agent::tools::WRITE && !entry.is_error => {
            let content = entry
                .input
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (PanelKind::File, content)
        }
        x if x == manox_agent::tools::EDIT => (PanelKind::Diff, raw),
        _ => (PanelKind::Plain, raw),
    }
}

/// Snapshot the workdir's git state for the `TerminalPanel` prompt line:
/// branch name (`HEAD` when detached) + counts of modified / deleted / conflict
/// / untracked paths. Shells out to `git` (two short subprocess spawns) on the
/// background executor; returns `None` outside a git repo so the panel omits
/// the `git:…` segment entirely.
fn detect_git(cwd: &Path) -> Option<GitSummary> {
    use std::process::Command;
    // `rev-parse --abbrev-ref HEAD` succeeds inside any repo (yields the branch
    // name, or `HEAD` when detached) and fails outside one — so a missing branch
    // is a reliable not-a-repo signal.
    let branch = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let branch = branch?;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    let mut conflict = 0usize;
    let mut untracked = 0usize;
    let porcelain = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    for line in porcelain.lines() {
        let b = line.as_bytes();
        if b.len() < 2 {
            continue;
        }
        let x = b[0] as char;
        let y = b[1] as char;
        if x == '?' && y == '?' {
            untracked += 1;
        } else if [x, y] == ['U', 'U']
            || [x, y] == ['A', 'A']
            || [x, y] == ['D', 'D']
            || [x, y] == ['A', 'U']
            || [x, y] == ['U', 'A']
            || [x, y] == ['D', 'U']
            || [x, y] == ['U', 'D']
        {
            conflict += 1;
        } else if x == 'D' || y == 'D' {
            deleted += 1;
        } else {
            // M (modified/staged), A (added), R (renamed), C (copied) — surface
            // as modified so the marker stays meaningful without a per-code table.
            modified += 1;
        }
    }
    Some(GitSummary {
        branch: Some(branch),
        modified,
        deleted,
        conflict,
        untracked,
    })
}

/// Trailing slice of live output: keep the last ~12 KiB so the most recent
/// lines are in view as they stream in. Whole-buffer lines are preserved once
/// the final result arrives.
fn live_tail(output: &str) -> String {
    const TAIL_BYTES: usize = 12 * 1024;
    if output.len() <= TAIL_BYTES {
        return output.to_string();
    }
    let cut = output.len() - TAIL_BYTES;
    // `cut` is a byte offset; round it down to a UTF-8 char boundary so the
    // slices below stay valid when the tail split lands inside a multi-byte
    // glyph (e.g. CJK output). Without this, `output[cut..]` panics.
    let cut = output.floor_char_boundary(cut);
    // Start at the next line boundary so we don't slice mid-line.
    let start = output[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
    let mut s = format!("{}\n", i18n::t("message-omitted-prefix"));
    s.push_str(&output[start..]);
    s
}

/// Collapse `s` to a single line, clipped to `max_chars` with an ellipsis.
///
/// `ai-elements`' `chain_of_thought.rs` carries a line-for-line twin,
/// `one_line` — the crate boundary keeps the two apart, so change one and
/// change the other.
fn truncate(s: &str, max_chars: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() > max_chars {
        let t: String = one_line.chars().take(max_chars).collect();
        format!("{t}…")
    } else {
        one_line
    }
}

/// Build a flat `ConvItem` list from a `Thread`'s canonical message list.
/// Used by the nested sub-agent panel and the tests; the top-level rebuild
/// (`ConversationState::rebuild_from_display`) drives `ItemBuilder` directly
/// so persisted UI notes can interleave between message batches.
///
/// Tool calls pair ToolUse with ToolResult by `tool_use_id`; an unpaired side
/// becomes its own item. Ordinary tool uses within one user turn aggregate into
/// a single activity segment (mirroring the live `apply` path, where
/// `StopReason::ToolUse` does not close the segment). A user prompt (text-
/// bearing user message) is the turn boundary; a user-role ToolResult is not.
/// Close the active activity segment: freezes and auto-collapses it. Shared
/// by the one-shot `build_items` and the incremental `ItemBuilder::finish`.
///
/// `frozen_secs` is pinned to `Some(0)` so historical containers rebuilt
/// from persisted messages do not fall back to a live `started_at.elapsed()`
/// — every container is created with a fresh `Instant::now()`, so an
/// unfrozen timer would tick forever from zero on a reloaded thread.
fn close_segment(items: &mut [ConvItem], seg_ix: Option<usize>) {
    if let Some(ix) = seg_ix
        && let Some(ConvItem::Thinking(t)) = items.get_mut(ix)
    {
        t.accepting_entries = false;
        t.streaming = false;
        t.collapsed = !t.user_toggled;
        if t.frozen_secs.is_none() {
            t.frozen_secs = Some(0);
        }
    }
}

/// Stateful item builder: appends `ConvItem`s from a message sequence while
/// carrying the turn/segment state (last user id, open activity segment)
/// across calls. `build_items` uses it for one-shot rebuilds; the streaming
/// history preview (`ConversationState::append_history_messages`) uses it to
/// append batches without re-processing the prefix, so a tool loop spanning a
/// batch boundary folds into one activity segment exactly as a one-shot build
/// would.
#[derive(Debug, Default)]
pub struct ItemBuilder {
    last_user_id: Option<String>,
    active_segment_ix: Option<usize>,
    /// The agent whose conversation the built items render in — every user
    /// bubble's header `to`. `None` omits the segment (a bare rebuild with no
    /// owning view).
    recipient: Option<manox_agent::MessageAuthor>,
    /// The session these messages were replayed from, when they are a faithful
    /// journal replay. Fork anchors are attached only then: a synthetic row's
    /// id does not name an entry the session has, so offering a fork on it
    /// would build a button that can only fail.
    fork_source: Option<String>,
}

impl ItemBuilder {
    pub fn new(recipient: Option<manox_agent::MessageAuthor>) -> Self {
        Self {
            recipient,
            ..Default::default()
        }
    }

    /// Mark these rows as a faithful replay of `session_id`, so their assistant
    /// replies can carry fork anchors. Without this the builder is building
    /// display-only content (synthetic backfill, borrowed compaction rows).
    pub fn with_fork_source(mut self, session_id: Option<String>) -> Self {
        self.fork_source = session_id;
        self
    }

    /// Append items for `messages` to `items`. A trailing open activity
    /// segment is left accepting entries so the next `extend` can fold into
    /// it; call `finish` on the last batch to close it.
    pub fn extend(
        &mut self,
        messages: &[Message],
        usage: &HashMap<String, TokenUsage>,
        items: &mut Vec<ConvItem>,
    ) {
        for m in messages {
            match m.role {
                Role::User => {
                    let external_event =
                        m.ui.as_ref()
                            .and_then(|ui| ui.external_event)
                            .unwrap_or(false);
                    let has_prompt_text = !external_event
                        && m.content.iter().any(|c| match c {
                            MessageContent::Text(t) | MessageContent::Thinking { text: t, .. } => {
                                !t.is_empty()
                            }
                            _ => false,
                        })
                        || m.content
                            .iter()
                            .any(|c| matches!(c, MessageContent::Image { .. }));
                    if has_prompt_text {
                        // A new user prompt closes the current turn's activity
                        // segment so the next turn opens a fresh one. A pure-
                        // tool-result user message has no prompt text and is NOT
                        // a turn boundary.
                        close_segment(items, self.active_segment_ix);
                        self.active_segment_ix = None;
                    }
                    if !external_event {
                        self.last_user_id = Some(m.id.clone());
                    }
                    // Text becomes a user bubble; ToolResult blocks pair back to the
                    // ToolCall item emitted from the preceding assistant ToolUse.
                    // ToolResults live in user messages per the Anthropic wire contract.
                    let text: String =
                        m.content
                            .iter()
                            .filter_map(|c| match c {
                                MessageContent::Text(t)
                                | MessageContent::Thinking { text: t, .. } => Some(t.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("");
                    // A registry slash turn persists a compact display form
                    // (`/name args`) alongside the expanded model-facing body;
                    // the bubble shows the display form so a reloaded thread
                    // matches the live send-time view.
                    let text =
                        m.ui.as_ref()
                            .and_then(|ui| ui.display_text.clone())
                            .filter(|t| !t.is_empty())
                            .unwrap_or(text);
                    let images: Vec<UserImage> = m
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            MessageContent::Image { data, mime_type } => {
                                let bytes = base64::engine::general_purpose::STANDARD
                                    .decode(data.as_bytes())
                                    .ok()?;
                                let fmt = gpui::ImageFormat::from_mime_type(mime_type.as_str())?;
                                Some(UserImage(Arc::new(gpui::Image::from_bytes(fmt, bytes))))
                            }
                            _ => None,
                        })
                        .collect();
                    if !external_event && (!text.is_empty() || !images.is_empty()) {
                        let meta = crate::conversation::UserTurnMeta::from_message(
                            m,
                            self.recipient.clone(),
                        );
                        items.push(ConvItem::User {
                            text,
                            images,
                            meta: Some(meta),
                        });
                    }
                    for c in &m.content {
                        match c {
                            MessageContent::ToolResult(tr) => {
                                pair_tool_result(items, tr);
                            }
                            MessageContent::Compaction(summary) => {
                                // A compaction message is role User but carries no
                                // prompt text — render it as a Recap card instead of
                                // an empty user bubble.
                                items.push(ConvItem::Recap {
                                    summary: summary.clone(),
                                    collapsed: true,
                                    user_toggled: false,
                                });
                            }
                            _ => {}
                        }
                    }
                }
                Role::Assistant => {
                    // A fork may only anchor on a *completed turn tail*: a
                    // prefix that ends mid-turn would carry an unresolved tool
                    // call, which the runtime repairs with a synthetic
                    // "No result provided" outcome — the child would start
                    // from a fabricated history. An entry that carries a
                    // `ToolUse` is by construction not a turn tail: its result
                    // rides the *next* entry, and the loop continues. Only
                    // entries without one can close a turn.
                    let closes_turn = !m
                        .content
                        .iter()
                        .any(|c| matches!(c, MessageContent::ToolUse(_)));
                    for c in &m.content {
                        match c {
                            MessageContent::Text(t) => {
                                // Assistant prose interrupts the activity segment:
                                // reasoning after it starts a fresh round. The
                                // closed segment's header carries the model name,
                                // so the reply suppresses its own model row.
                                close_segment(items, self.active_segment_ix);
                                let activity_header = self.active_segment_ix.is_some();
                                self.active_segment_ix = None;
                                items.push(ConvItem::Assistant {
                                    text: t.clone(),
                                    streaming: false,
                                    token_usage: self
                                        .last_user_id
                                        .as_deref()
                                        .and_then(|id| usage.get(id).copied()),
                                    activity_header,
                                    // Anchor eligibility is resolved here,
                                    // where the journal context is known: only
                                    // a faithful replay makes `m.id` name an
                                    // entry of the session, and only a reply
                                    // with no tool call closes its turn.
                                    entry_id: (closes_turn && self.fork_source.is_some())
                                        .then(|| m.id.clone()),
                                    fork_unavailable: if self.fork_source.is_none() {
                                        Some(ForkUnavailable::NotReplayed)
                                    } else if !closes_turn {
                                        Some(ForkUnavailable::MidTurn)
                                    } else {
                                        None
                                    },
                                });
                            }
                            MessageContent::Thinking { text, .. } => {
                                // Fold reasoning into the active activity segment.
                                // A reasoning block before any tool calls opens the
                                // segment; subsequent reasoning and tools share it.
                                let entry = ActivityEntry::Reasoning {
                                    id: next_reasoning_entry_id(),
                                    text: text.clone(),
                                    streaming: false,
                                    collapsed: true,
                                    user_toggled: false,
                                    markdown: None,
                                };
                                match self.active_segment_ix {
                                    Some(ix) => {
                                        if let Some(ConvItem::Thinking(t)) = items.get_mut(ix) {
                                            t.entries.push(entry);
                                        }
                                    }
                                    None => {
                                        self.active_segment_ix = Some(items.len());
                                        items.push(ConvItem::Thinking(ThinkingContainer {
                                            entries: vec![entry],
                                            accepting_entries: true,
                                            streaming: false,
                                            collapsed: true,
                                            user_toggled: false,
                                            started_at: Instant::now(),
                                            frozen_secs: None,
                                        }));
                                    }
                                }
                            }
                            MessageContent::ToolUse(tu) => {
                                if crate::conversation::is_agent_task_call(Some(&tu.input)) {
                                    // Sub-agent tasks stay as standalone compact
                                    // rows; their full conversation lives in a
                                    // read-only right-pane tab.
                                    close_segment(items, self.active_segment_ix);
                                    self.active_segment_ix = None;
                                    let (subagent_type, description) =
                                        crate::conversation::agent_task_labels(
                                            tu.name.as_ref(),
                                            &tu.input,
                                        );
                                    items.push(ConvItem::AgentTask(AgentTaskItem {
                                        id: tu.id.clone(),
                                        subagent_type,
                                        description,
                                        status: ToolCallStatus::Success,
                                        is_error: false,
                                    }));
                                } else if tu.name.as_ref() == manox_agent::tools::ASK_USER_QUESTION
                                {
                                    // An inline clarify card: stays a top-level
                                    // ToolCall so `render_ask_user_card` can drive
                                    // its interactive snapshot while pending; the
                                    // paired ToolResult stamps the user's answer
                                    // into `output`. Never folded into a segment.
                                    close_segment(items, self.active_segment_ix);
                                    self.active_segment_ix = None;
                                    items.push(ConvItem::ToolCall(ToolCallItem {
                                        id: tu.id.clone(),
                                        name: tu.name.to_string(),
                                        title: manox_agent::thread::tool_title(
                                            tu.name.as_ref(),
                                            &tu.input,
                                            None,
                                        ),
                                        status: ToolCallStatus::PendingApproval,
                                        output: String::new(),
                                        is_error: false,
                                        input: tu.input.clone(),
                                        streaming: false,
                                        collapsed: false,
                                        user_toggled: false,
                                        panel: None,
                                    }));
                                } else {
                                    // Ordinary tool call: fold into the active
                                    // activity segment. The segment is created at
                                    // the first ordinary tool call's position and
                                    // stays there; subsequent tool calls (across
                                    // assistant messages and tool-result user
                                    // messages within the same turn) append to it.
                                    let entry = ActivityEntry::Tool(ToolCallItem {
                                        id: tu.id.clone(),
                                        name: tu.name.to_string(),
                                        title: manox_agent::thread::tool_title(
                                            tu.name.as_ref(),
                                            &tu.input,
                                            None,
                                        ),
                                        status: ToolCallStatus::Success,
                                        output: String::new(),
                                        is_error: false,
                                        input: tu.input.clone(),
                                        streaming: false,
                                        collapsed: true,
                                        user_toggled: false,
                                        panel: None,
                                    });
                                    match self.active_segment_ix {
                                        Some(ix) => {
                                            if let Some(ConvItem::Thinking(t)) = items.get_mut(ix) {
                                                t.entries.push(entry);
                                            }
                                        }
                                        None => {
                                            self.active_segment_ix = Some(items.len());
                                            items.push(ConvItem::Thinking(ThinkingContainer {
                                                entries: vec![entry],
                                                accepting_entries: true,
                                                streaming: false,
                                                collapsed: true,
                                                user_toggled: false,
                                                started_at: Instant::now(),
                                                frozen_secs: None,
                                            }));
                                        }
                                    }
                                }
                            }
                            MessageContent::ToolResult(tr) => {
                                // Defensive: tool results normally live in user messages,
                                // but pair them here too if they ever appear in an assistant turn.
                                pair_tool_result(items, tr);
                            }
                            MessageContent::Image { .. } => {}
                            // Compaction messages are `Role::User` by construction;
                            // they cannot appear in an assistant turn. Reachable
                            // here only if a future caller mis-assigns the role.
                            MessageContent::Compaction(summary) => {
                                close_segment(items, self.active_segment_ix);
                                self.active_segment_ix = None;
                                items.push(ConvItem::Recap {
                                    summary: summary.clone(),
                                    collapsed: true,
                                    user_toggled: false,
                                });
                            }
                        }
                    }
                }
                Role::System => {}
            }
        }
    }

    /// Close any open activity segment and apply the trailing-streaming
    /// postlude (a still-running thread's tail must read as live so resumed
    /// `AgentText`/`AgentThinking` deltas append to it instead of spawning a
    /// second bubble). Consumes the builder: `finish` is terminal, so a
    /// caller cannot forget the mandatory close.
    pub fn finish(self, items: &mut [ConvItem], trailing_streaming: bool) {
        close_segment(items, self.active_segment_ix);
        if trailing_streaming && let Some(last) = items.last_mut() {
            match last {
                ConvItem::Assistant { streaming, .. } => {
                    *streaming = true;
                }
                ConvItem::Thinking(t) => {
                    // A resumed turn may still be mid-segment: mark the
                    // container live so later-arriving `ToolCall`/`ToolOutput`
                    // deltas fold into it instead of opening a fresh one.
                    // Unfreeze the timer too — `close_segment` just pinned it,
                    // but a live container must tick from `started_at` (now).
                    t.accepting_entries = true;
                    t.streaming = true;
                    t.frozen_secs = None;
                }
                _ => {}
            }
        }
    }
}

/// One-shot item build over a complete message list (historical rebuild).
pub fn build_items(
    messages: &[Message],
    usage: &HashMap<String, TokenUsage>,
    trailing_streaming: bool,
    recipient: Option<manox_agent::MessageAuthor>,
) -> Vec<ConvItem> {
    let mut builder = ItemBuilder::new(recipient);
    let mut items = Vec::new();
    builder.extend(messages, usage, &mut items);
    builder.finish(&mut items, trailing_streaming);
    items
}
/// Attach a tool_result to its matching item by id. Sub-agent results only
/// stamp their compact row's terminal state; ordinary tool results stamp the
/// entry inside the owning `ThinkingContainer`. A result with no matching
/// ToolUse becomes a standalone single-entry `ThinkingContainer` so an orphan
/// result still renders as a `⎿`.
fn pair_tool_result(items: &mut Vec<ConvItem>, tr: &LanguageModelToolResult) {
    let status = if tr.is_error {
        ToolCallStatus::Error
    } else {
        ToolCallStatus::Success
    };
    // Locate the owning item: an AgentTask or a Thinking-container entry.
    // Remember the entry index for the Thinking path so we can stamp the
    // right `⎿` inside its batch.
    let mut thinking_eix: Option<usize> = None;
    let ix = items.iter().position(|i| match i {
        ConvItem::AgentTask(t) => t.id == tr.tool_use_id,
        ConvItem::ToolCall(t) => t.id == tr.tool_use_id,
        ConvItem::Thinking(t) => match t.find_entry_index(&tr.tool_use_id) {
            Some(eix) => {
                thinking_eix = Some(eix);
                true
            }
            None => false,
        },
        _ => false,
    });
    let Some(ix) = ix else {
        items.push(ConvItem::Thinking(ThinkingContainer {
            entries: vec![ActivityEntry::Tool(ToolCallItem {
                id: tr.tool_use_id.clone(),
                name: tr.tool_name.to_string(),
                title: tr.tool_name.to_string(),
                status,
                output: tr.content.clone(),
                is_error: tr.is_error,
                input: serde_json::Value::Null,
                streaming: false,
                collapsed: !matches!(
                    status,
                    ToolCallStatus::Running | ToolCallStatus::PendingApproval
                ),
                user_toggled: false,
                panel: None,
            })],
            accepting_entries: false,
            streaming: false,
            collapsed: false,
            user_toggled: false,
            started_at: Instant::now(),
            frozen_secs: None,
        }));
        return;
    };
    match &mut items[ix] {
        ConvItem::AgentTask(t) => {
            t.is_error = tr.is_error;
            t.status = status;
        }
        ConvItem::ToolCall(t) => {
            t.output = tr.content.clone();
            t.is_error = tr.is_error;
            t.status = status;
            if t.name.is_empty() {
                t.name = tr.tool_name.to_string();
            }
        }
        ConvItem::Thinking(t) => {
            if let Some(eix) = thinking_eix
                && let Some(ActivityEntry::Tool(entry)) = t.entries.get_mut(eix)
            {
                entry.output = tr.content.clone();
                entry.is_error = tr.is_error;
                entry.status = status;
                entry.streaming = false;
                entry.collapsed = !entry.user_toggled;
            }
            t.recompute_streaming();
            if !t.streaming {
                t.collapsed = !t.user_toggled;
            }
        }
        _ => {}
    }
}

/// Title for a `AskUserQuestion` card.
///
/// A header comes from the model/runtime and is shown as-is. The runtime does
/// not require it (`parse_pending_ask` tolerates a missing field), so an empty
/// header falls back to this app's own chrome label rather than rendering a
/// blank card.
fn question_card_title(header: &str) -> SharedString {
    if header.trim().is_empty() {
        i18n::t("workspace-clarify-title")
    } else {
        header.into()
    }
}

#[cfg(test)]
mod tests {

    /// An empty question header is reachable (the runtime's validation does not
    /// require the field), so the card must fall back to app chrome rather
    /// than render a blank title. A present header is shown verbatim.
    #[test]
    fn question_card_title_falls_back_for_an_empty_header() {
        let fallback = i18n::t("workspace-clarify-title");
        assert_ne!(
            fallback.as_ref(),
            "workspace-clarify-title",
            "the fallback key is missing from the bundles"
        );
        for empty in ["", "   ", "\n"] {
            assert_eq!(question_card_title(empty), fallback, "header: {empty:?}");
        }
        assert_eq!(question_card_title("Pick a database"), "Pick a database");
        // Runtime-supplied text is never re-localized, even when it is Chinese.
        assert_eq!(question_card_title("选一个数据库"), "选一个数据库");
    }

    /// The plan-review routing is exact: the intent must name `plan-review`
    /// AND its `approve` label must match one of the question's own option
    /// labels; anything else renders the generic question card.
    #[test]
    fn plan_review_approve_index_requires_a_matching_option() {
        use crate::workspace::{AskCardIntent, AskCardOption, AskCardQuestion};

        let question = |intent: Option<AskCardIntent>, multi_select: bool| AskCardQuestion {
            question: String::new(),
            header: String::new(),
            detail: String::new(),
            intent,
            multi_select,
            options: vec![
                AskCardOption {
                    label: "Approve".into(),
                    description: String::new(),
                    recommended: false,
                },
                AskCardOption {
                    label: "Request changes".into(),
                    description: String::new(),
                    recommended: false,
                },
            ],
        };
        let plan_review = Some(AskCardIntent {
            kind: "plan-review".into(),
            approve: "Approve".into(),
        });
        assert_eq!(
            plan_review_approve_index(&question(plan_review.clone(), false)),
            Some(0),
            "the approve label resolves to its option's index"
        );
        for (case, (intent, multi_select)) in [
            ("no intent", (None, false)),
            (
                "empty approve label",
                (
                    Some(AskCardIntent {
                        kind: "plan-review".into(),
                        approve: String::new(),
                    }),
                    false,
                ),
            ),
            (
                "approve label matches no option",
                (
                    Some(AskCardIntent {
                        kind: "plan-review".into(),
                        approve: "Nope".into(),
                    }),
                    false,
                ),
            ),
            (
                "foreign intent kind",
                (
                    Some(AskCardIntent {
                        kind: "clarify".into(),
                        approve: "Approve".into(),
                    }),
                    false,
                ),
            ),
            ("multi-select question", (plan_review, true)),
        ] {
            assert_eq!(
                plan_review_approve_index(&question(intent, multi_select)),
                None,
                "case: {case}"
            );
        }
    }
    use super::*;
    use gpui::{
        AnyWindowHandle, Bounds, Pixels, Render, TestAppContext, VisualTestContext, Window, size,
    };
    use manox_agent::language_model::{LanguageModelToolResult, LanguageModelToolUse};

    #[test]
    fn live_tail_short_output_unchanged() {
        let s = "line\nline2\n";
        assert_eq!(live_tail(s), s);
    }

    /// B2-PR-2: the ask tool result row folds a canonical answers payload into
    /// `(question, answer)` rows (question text recovered by id from the call's
    /// own input). This is the shared render contract BOTH arrival paths satisfy
    /// — the live `ToolResult` event and the rebuild journal translate each
    /// deposit the identical JSON string into `ToolCallItem.output`, so a
    /// fixture per path feeds this one function.
    #[test]
    fn ask_result_qa_rows_fold_the_canonical_single_select_live_payload() {
        let input = serde_json::json!({
            "questions": [{"id": "a1", "question": "Which color?", "options": [
                {"label": "Red"}, {"label": "Blue"}]}]
        });
        // The live path's `ToolResult.output` once the server emits canonical JSON.
        let output = r#"{"answers":[{"id":"a1","selected":["Blue"]}]}"#;
        let rows = ask_result_qa_rows(&input, output).expect("canonical payload folds");
        assert_eq!(rows, vec![("Which color?".to_string(), "Blue".to_string())]);
    }

    /// Rebuild-path fixture: a multi-select supplement plus an explicit skip
    /// across two questions, arriving verbatim from the journal tool row.
    #[test]
    fn ask_result_qa_rows_fold_the_canonical_multi_and_skip_rebuild_payload() {
        let input = serde_json::json!({
            "questions": [
                {"id": "m", "question": "Pick flavors", "multiSelect": true},
                {"id": "s", "question": "Skip me"}
            ]
        });
        let output = r#"{"answers":[
            {"id":"m","selected":["vanilla","salt"],"custom":"and pistachio"},
            {"id":"s","selected":[]}
        ]}"#;
        let rows = ask_result_qa_rows(&input, output).expect("canonical payload folds");
        assert_eq!(
            rows,
            vec![
                (
                    "Pick flavors".to_string(),
                    "vanilla, salt · and pistachio".to_string()
                ),
                ("Skip me".to_string(), "—".to_string()),
            ],
            "multi-select lists picks with the supplement; the skip renders an em dash"
        );
    }

    /// An unknown id (not in the call's input) falls back to the id itself —
    /// never a mislabelled question.
    #[test]
    fn ask_result_qa_rows_fall_back_to_the_id_for_an_unknown_question() {
        let input = serde_json::json!({ "questions": [] });
        let output = r#"{"answers":[{"id":"ghost","selected":["x"]}]}"#;
        let rows = ask_result_qa_rows(&input, output).expect("still folds");
        assert_eq!(rows, vec![("ghost".to_string(), "x".to_string())]);
    }

    /// A non-canonical result (the transitional prose render, an error string,
    /// or malformed JSON) yields `None` so the display falls back to the raw
    /// output verbatim.
    #[test]
    fn ask_result_qa_rows_reject_non_canonical_payloads() {
        let input = serde_json::json!({ "questions": [{"id":"a","question":"Q"}] });
        assert!(
            ask_result_qa_rows(&input, "Question: Q\nAnswer: Blue").is_none(),
            "the legacy prose result is not canonical JSON"
        );
        assert!(ask_result_qa_rows(&input, "boom: not json").is_none());
        assert!(
            ask_result_qa_rows(&input, r#"{"answers":[]}"#).is_none(),
            "empty is not a fold"
        );
        assert!(
            ask_result_qa_rows(&input, r#"{"answers":[{"id":"a","custom":"x"}]}"#).is_none(),
            "a row without `selected` is not canonical"
        );
    }

    #[test]
    fn live_tail_long_ascii_splits_on_newline() {
        let s = "a".repeat(13 * 1024);
        let out = live_tail(&s);
        assert!(out.starts_with(&i18n::t("message-omitted-prefix").to_string()));
        // No newline in the input -> fall back to the byte cut; still valid.
        assert!(out.ends_with('a'));
    }

    /// Regression: a byte cut landing inside a multi-byte CJK glyph used to panic
    /// `output[cut..]` with a slice-out-of-bounds. The tail must split on a char
    /// boundary instead.
    #[test]
    fn live_tail_multibyte_cut_does_not_panic() {
        // Each line is a CJK char repeated so the tail boundary lands mid-glyph.
        let line = "中".repeat(64);
        let mut s = String::new();
        for _ in 0..(13 * 1024 / line.len() + 1) {
            s.push_str(&line);
            s.push('\n');
        }
        let out = live_tail(&s);
        assert!(out.starts_with(&i18n::t("message-omitted-prefix").to_string()));
        // The retained tail must be valid UTF-8 (would have panicked before).
        assert!(out.contains('中'));
    }

    #[test]
    fn agent_terminal_statuses_use_the_expected_icons() {
        // Verify that terminal states produce icons without panicking.
        // Running/Pending are excluded — they use BrailleSpinner, not icons.
        let _ = agent_terminal_icon(ToolCallStatus::Success);
        let _ = agent_terminal_icon(ToolCallStatus::Continued);
        let _ = agent_terminal_icon(ToolCallStatus::Error);
        let _ = agent_terminal_icon(ToolCallStatus::Denied);
        let _ = agent_terminal_icon(ToolCallStatus::Cancelled);
    }

    struct MessageOverflowProbe;

    impl Render for MessageOverflowProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let mut thinking = ThinkingContainer::new();
            thinking.collapsed = false;
            thinking.streaming = false;
            thinking.entries.push(ActivityEntry::Reasoning {
                id: next_reasoning_entry_id(),
                text: "reasoning ".repeat(300),
                streaming: false,
                collapsed: false,
                user_toggled: true,
                markdown: None,
            });
            thinking.entries.push(ActivityEntry::Tool(ToolCallItem {
                id: "tool-long-final-output".into(),
                name: "Bash".into(),
                title: "bash: a very long final command title that should never force the message list wider than its host".into(),
                status: ToolCallStatus::Success,
                output: format!("{}\n{}", "x".repeat(2048), "y".repeat(2048)),
                is_error: false,
                input: serde_json::json!({"command": "printf"}),
                streaming: false,
                collapsed: false,
                user_toggled: true,
                panel: None,
            }));
            thinking.entries.push(ActivityEntry::Tool(ToolCallItem {
                id: "tool-long-streaming-output".into(),
                name: "Bash".into(),
                title: "bash: a very long streaming command title that should keep horizontal scroll local".into(),
                status: ToolCallStatus::Running,
                output: format!("{}\n{}", "z".repeat(2048), "w".repeat(2048)),
                is_error: false,
                input: serde_json::json!({"command": "printf"}),
                streaming: true,
                collapsed: false,
                user_toggled: true,
                panel: None,
            }));

            let thinking_item = ConvItem::Thinking(thinking);
            let agent_item = ConvItem::AgentTask(AgentTaskItem {
                id: "agent-long-title".into(),
                subagent_type: "Explore".into(),
                description: "检查一段非常长的中英文混合标题 and verify that it remains a single truncated line without rendering metrics or child output".into(),
                status: ToolCallStatus::Success,
                is_error: false,
            });
            // The description carries a heredoc body and the failure summary
            // holds overlong single lines — the exact shape that used to paint
            // past the card border.
            let bg_task_item = ConvItem::BackgroundTask(BackgroundTaskItem {
                task_id: "bash_overflow".into(),
                kind: manox_agent::background_task::TaskKind::BackgroundBash,
                description: format!(
                    "cd /some/project && run <<'EOF'\n{{\n  \"prompt\": \"{}\"\n}}\nEOF",
                    "x".repeat(512)
                ),
                status: manox_agent::background_task::TaskStatus::Failed,
                event_count: 3,
                total_bytes: 2048,
                exit_code: Some(65),
                failure_summary: Some(format!(
                    "sandbox-exec: host must be * or localhost in network address {}",
                    "y".repeat(512)
                )),
                created_at: None,
                recent_events: Vec::new(),
            });
            let theme = cx.theme().clone();
            gpui::div()
                .id("message-overflow-probe")
                .w(px(260.))
                .min_w_0()
                .overflow_x_hidden()
                .debug_selector(|| "message-overflow-host".into())
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_2()
                        .debug_selector(|| "message-overflow-item".into())
                        .child(render_item(
                            &thinking_item,
                            0,
                            "test-model",
                            &theme,
                            None,
                            None,
                            None,
                            None,
                            None,
                            cx,
                        ))
                        .child(render_item(
                            &agent_item,
                            1,
                            "test-model",
                            &theme,
                            None,
                            None,
                            None,
                            None,
                            None,
                            cx,
                        ))
                        .child(render_item(
                            &bg_task_item,
                            2,
                            "test-model",
                            &theme,
                            None,
                            None,
                            None,
                            None,
                            None,
                            cx,
                        )),
                )
        }
    }

    fn assert_width_within(bounds: Bounds<Pixels>, max_width: Pixels, label: &str) {
        assert!(
            bounds.size.width <= max_width,
            "{label} should stay within the narrow host, got {:?}",
            bounds.size.width
        );
    }

    #[gpui::test]
    fn message_overflow_activity_tree_stays_within_narrow_width(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let window = cx.open_window(size(px(320.), px(520.)), move |_, _| MessageOverflowProbe);
        cx.run_until_parked();
        let any: AnyWindowHandle = window.into();
        let mut cx = VisualTestContext::from_window(any, cx);
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        for selector in [
            "message-overflow-host",
            "message-overflow-item",
            "message-overflow-activity-tree-0",
            "message-overflow-activity-entry-0-0",
            "message-overflow-activity-entry-body-0-0",
            "message-overflow-activity-entry-0-1",
            "message-overflow-activity-entry-body-0-1",
            "message-overflow-tool-output-1",
            "message-overflow-activity-entry-0-2",
            "message-overflow-activity-entry-body-0-2",
            "message-overflow-tool-output-2",
            "message-overflow-agent-row-1",
            "message-overflow-agent-title-1",
            "message-overflow-bg-task-row-2",
            "message-overflow-bg-task-title-2",
            "message-overflow-bg-task-detail-2",
        ] {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("missing debug bounds for {selector}"));
            assert_width_within(bounds, px(260.), selector);
        }

        let row = cx
            .debug_bounds("message-overflow-agent-row-1")
            .expect("agent row bounds");
        assert!(
            row.size.height <= px(40.),
            "agent task must remain a compact single row, got {:?}",
            row.size.height
        );
    }

    /// Helper: build a `MessageContent::ToolUse` for a tool name + JSON input.
    fn tu(id: &str, name: &str, input: serde_json::Value) -> MessageContent {
        MessageContent::ToolUse(LanguageModelToolUse {
            id: id.to_string(),
            name: Arc::from(name),
            raw_input: String::new(),
            input,
            is_input_complete: true,
            thought_signature: None,
        })
    }

    /// Helper: build a `MessageContent::ToolResult` for a tool id + name.
    fn tr(id: &str, name: &str, content: &str) -> MessageContent {
        MessageContent::ToolResult(LanguageModelToolResult {
            tool_use_id: id.to_string(),
            tool_name: Arc::from(name),
            is_error: false,
            content: content.to_string(),
        })
    }

    /// A multi-step user turn — read → edit → bash, each in its own assistant
    /// message with a tool-result user message between — must rebuild as ONE
    /// activity segment holding all three entries, not one segment per tool.
    /// This is the historical-rebuild mirror of the live `Stop(ToolUse)` does-
    /// not-freeze behavior.
    #[test]
    fn build_items_aggregates_tool_loop_into_one_segment() {
        let messages = vec![
            Message::user("do the task".to_string()),
            Message::assistant(vec![tu(
                "tu_1",
                "Read",
                serde_json::json!({"path": "a.rs"}),
            )]),
            Message::user_with_content(vec![tr("tu_1", "Read", "a contents")]),
            Message::assistant(vec![tu(
                "tu_2",
                "Edit",
                serde_json::json!({"patch": "[a.rs#T1]\nINS x"}),
            )]),
            Message::user_with_content(vec![tr("tu_2", "Edit", "ok")]),
            Message::assistant(vec![tu(
                "tu_3",
                "Bash",
                serde_json::json!({"command": "cargo build"}),
            )]),
            Message::user_with_content(vec![tr("tu_3", "Bash", "Built.")]),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        let segments: Vec<&ThinkingContainer> = items
            .iter()
            .filter_map(|i| match i {
                ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 1, "one turn → one activity segment");
        assert_eq!(
            segments[0].entries.len(),
            3,
            "all three tools in the segment"
        );
        assert!(!segments[0].streaming, "historical segment is frozen");
        assert!(
            !segments[0].accepting_entries,
            "historical segment is closed"
        );
        // Entries in arrival order — all tool entries.
        let tool_ids: Vec<&str> = segments[0]
            .entries
            .iter()
            .filter_map(|e| match e {
                ActivityEntry::Tool(t) => Some(t.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(tool_ids, vec!["tu_1", "tu_2", "tu_3"]);
    }

    /// The streaming history preview appends messages batch-by-batch through
    /// `ItemBuilder::extend`, carrying the turn/segment state across calls. A
    /// tool loop spanning a batch boundary must fold into the same activity
    /// segment as a one-shot `build_items` — not spawn a second segment.
    #[test]
    fn item_builder_incremental_matches_one_shot_across_batch_boundary() {
        let messages = vec![
            Message::user("turn one".to_string()),
            Message::assistant(vec![
                MessageContent::Thinking {
                    text: "reasoning".to_string(),
                    signature: None,
                },
                tu("tu_1", "Read", serde_json::json!({"path": "a.rs"})),
            ]),
            Message::user_with_content(vec![tr("tu_1", "Read", "a contents")]),
            Message::assistant(vec![MessageContent::Text("done".to_string())]),
        ];
        let usage = HashMap::new();

        // One-shot build (the authoritative rebuild path).
        let one_shot = build_items(&messages, &usage, false, None);

        fn segment_entry_counts(items: &[ConvItem]) -> Vec<usize> {
            items
                .iter()
                .filter_map(|it| match it {
                    ConvItem::Thinking(t) => Some(t.entries.len()),
                    _ => None,
                })
                .collect()
        }

        // Every candidate split point must reproduce the one-shot build —
        // the carried turn/segment state makes the batch boundary invisible.
        for split in 1..messages.len() {
            let mut builder = ItemBuilder::new(None);
            let mut incremental: Vec<ConvItem> = Vec::new();
            builder.extend(&messages[..split], &usage, &mut incremental);
            builder.extend(&messages[split..], &usage, &mut incremental);
            builder.finish(&mut incremental, false);
            assert_eq!(
                segment_entry_counts(&incremental),
                segment_entry_counts(&one_shot),
                "split at {split}: the tool loop folds into one segment"
            );
            assert_eq!(
                incremental.len(),
                one_shot.len(),
                "split at {split}: same item count as the one-shot build"
            );
        }

        // A three-batch chain (per-message batches) must also match.
        let mut builder = ItemBuilder::new(None);
        let mut incremental: Vec<ConvItem> = Vec::new();
        for message in &messages {
            builder.extend(std::slice::from_ref(message), &usage, &mut incremental);
        }
        builder.finish(&mut incremental, false);
        assert_eq!(
            segment_entry_counts(&incremental),
            segment_entry_counts(&one_shot),
            "per-message batches fold identically"
        );
        assert_eq!(incremental.len(), one_shot.len());

        // The result stamped into the segment's tool entry, and the assistant
        // reply follows as a fresh bubble — no second segment opened.
        assert_eq!(segment_entry_counts(&one_shot), vec![2]);
        let last = one_shot.last().unwrap();
        assert!(
            matches!(last, ConvItem::Assistant { text, .. } if text == "done"),
            "assistant reply closes the segment and stands alone"
        );
    }

    /// `ConversationState::append_history_messages` calls `extend` with a
    /// fresh scratch vec per batch, so a segment index carried over from a
    /// previous batch points outside the current vec. `extend` must tolerate
    /// the stale index instead of panicking (regression: bounds-check abort
    /// when a fresh batch opens with assistant text).
    #[test]
    fn item_builder_extend_survives_stale_segment_index_on_fresh_vec() {
        let usage = HashMap::new();
        let mut builder = ItemBuilder::new(None);

        // Batch 1 leaves an open activity segment (index into `first`).
        let mut first = Vec::new();
        builder.extend(
            &[
                Message::user("turn one".to_string()),
                Message::assistant(vec![tu(
                    "tu_1",
                    "Read",
                    serde_json::json!({"path": "a.rs"}),
                )]),
            ],
            &usage,
            &mut first,
        );

        // Batch 2 opens with assistant text in a brand-new empty vec: the
        // carried segment index is out of bounds for it.
        let mut second = Vec::new();
        builder.extend(
            &[Message::assistant(vec![MessageContent::Text(
                "done".to_string(),
            )])],
            &usage,
            &mut second,
        );

        assert_eq!(second.len(), 1);
        assert!(
            matches!(
                &second[0],
                ConvItem::Assistant {
                    activity_header: true,
                    ..
                }
            ),
            "the reply follows the previous batch's segment, so its header carries the model"
        );
    }

    /// Historical `ThinkingContainer`s rebuilt from persisted messages must
    /// have `frozen_secs` pinned so the header's elapsed label reports a
    /// fixed duration (not a live `started_at.elapsed()`) — otherwise the
    /// timer ticks forever from zero on a reloaded thread (regression: the
    /// old "计时一直进行" bug).
    #[test]
    fn build_items_freezes_historical_segment_elapsed() {
        let messages = vec![
            Message::user("do it".to_string()),
            Message::assistant(vec![tu(
                "tu_1",
                "Read",
                serde_json::json!({"path": "a.rs"}),
            )]),
            Message::user_with_content(vec![tr("tu_1", "Read", "a contents")]),
            // Second user prompt closes the first turn's segment.
            Message::user("next".to_string()),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        let seg = items.iter().find_map(|i| match i {
            ConvItem::Thinking(t) => Some(t),
            _ => None,
        });
        let seg = seg.expect("segment present");
        assert!(
            seg.frozen_secs.is_some(),
            "historical segment must pin elapsed to stop the timer"
        );
    }

    #[test]
    fn build_items_prefers_persisted_display_text_for_user_bubble() {
        // A registry slash turn persists the expanded macro body as the
        // model-facing text plus a compact `display_text`; the rebuilt bubble
        // must show the compact form (parity with the live send-time view).
        let mut message = Message::user("EXPANDED MACRO BODY".to_string());
        message.ui = Some(manox_agent::MessageUiMetadata {
            display_text: Some("/gitwork:deliver fast".to_string()),
            ..Default::default()
        });
        let items = build_items(&[message], &HashMap::new(), false, None);
        let texts: Vec<&String> = items
            .iter()
            .filter_map(|i| match i {
                ConvItem::User { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["/gitwork:deliver fast"]);
    }

    #[test]
    fn build_items_user_bubble_keeps_plain_text_without_display_text() {
        let message = Message::user("plain turn".to_string());
        let items = build_items(&[message], &HashMap::new(), false, None);
        assert!(matches!(
            items.first(),
            Some(ConvItem::User { text, .. }) if text == "plain turn"
        ));
    }

    #[test]
    fn build_items_user_prompt_is_turn_boundary_tool_result_is_not() {
        let messages = vec![
            Message::user("turn one".to_string()),
            Message::assistant(vec![tu(
                "tu_1",
                "Read",
                serde_json::json!({"path": "a.rs"}),
            )]),
            // tool-result user message — NOT a turn boundary.
            Message::user_with_content(vec![tr("tu_1", "Read", "a")]),
            Message::assistant(vec![tu(
                "tu_2",
                "Bash",
                serde_json::json!({"command": "ls"}),
            )]),
            Message::user_with_content(vec![tr("tu_2", "Bash", "files")]),
            // New user prompt — IS a turn boundary.
            Message::user("turn two".to_string()),
            Message::assistant(vec![tu(
                "tu_3",
                "Read",
                serde_json::json!({"path": "b.rs"}),
            )]),
            Message::user_with_content(vec![tr("tu_3", "Read", "b")]),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        let segments: Vec<&ThinkingContainer> = items
            .iter()
            .filter_map(|i| match i {
                ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(segments.len(), 2, "two turns → two segments");
        assert_eq!(segments[0].entries.len(), 2, "turn one has 2 tools");
        assert_eq!(segments[1].entries.len(), 1, "turn two has 1 tool");
    }

    /// `agent` and `AskUserQuestion` must stay standalone top-level cards — they
    /// must not be swallowed into the activity segment, even when they appear in
    /// the same assistant message as ordinary tools.
    #[test]
    fn build_items_keeps_special_tools_standalone() {
        let messages = vec![
            Message::user("go".to_string()),
            Message::assistant(vec![
                tu("tu_1", "Read", serde_json::json!({"path": "a.rs"})),
                tu(
                    "tu_agent",
                    "Agent",
                    serde_json::json!({
                        "subagent_type": "r",
                        "description": "inspect p",
                        "prompt": "p"
                    }),
                ),
                tu(
                    "tu_ask",
                    "AskUserQuestion",
                    serde_json::json!({"questions": [{"question": "q", "header": "h", "options": [{"text": "a", "value": "a"}], "multi_select": false}]}),
                ),
            ]),
            Message::user_with_content(vec![
                tr("tu_1", "Read", "a"),
                tr("tu_agent", "Agent", "{\"final\":\"done\"}"),
                tr("tu_ask", "AskUserQuestion", "answered"),
            ]),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        // The ordinary tool folds into a segment; the two special tools are
        // standalone top-level cards.
        let agent = items.iter().find_map(|i| match i {
            ConvItem::AgentTask(t) if t.id == "tu_agent" => Some(t),
            _ => None,
        });
        let ask = items.iter().find_map(|i| match i {
            ConvItem::ToolCall(t) if t.name == manox_agent::tools::ASK_USER_QUESTION => Some(t),
            _ => None,
        });
        let seg = items.iter().find_map(|i| match i {
            ConvItem::Thinking(t) => Some(t),
            _ => None,
        });
        assert!(agent.is_some(), "agent task is standalone");
        assert!(ask.is_some(), "AskUserQuestion is standalone");
        let seg = seg.expect("ordinary tool folded into a segment");
        assert_eq!(
            seg.entries.len(),
            1,
            "only the ordinary tool is in the segment"
        );
        match &seg.entries[0] {
            ActivityEntry::Tool(t) => assert_eq!(t.id, "tu_1"),
            _ => panic!("expected tool entry"),
        }
    }

    /// `segment_stats` counts tool calls per name in first-appearance order
    /// and flags failed / approval-pending entries for the cover badges.
    #[test]
    fn segment_stats_counts_tools_and_flags() {
        let tool = |id: &str, name: &str, status: ToolCallStatus| {
            ActivityEntry::Tool(ToolCallItem {
                id: id.into(),
                name: name.into(),
                title: String::new(),
                status,
                output: String::new(),
                is_error: false,
                input: serde_json::Value::Null,
                streaming: false,
                collapsed: false,
                user_toggled: false,
                panel: None,
            })
        };
        let mut t = ThinkingContainer::new();
        t.entries.push(ActivityEntry::Reasoning {
            id: next_reasoning_entry_id(),
            text: "hmm".into(),
            streaming: false,
            collapsed: true,
            user_toggled: false,
            markdown: None,
        });
        t.entries.push(tool("1", "Read", ToolCallStatus::Success));
        t.entries.push(tool("2", "Edit", ToolCallStatus::Error));
        t.entries.push(tool("3", "Read", ToolCallStatus::Success));
        t.entries
            .push(tool("4", "Bash", ToolCallStatus::PendingApproval));
        let stats = segment_stats(&t);
        assert_eq!(stats.thinking_rounds, 1);
        assert_eq!(
            stats.tools,
            vec![
                ("Read".to_string(), 2),
                ("Edit".to_string(), 1),
                ("Bash".to_string(), 1)
            ]
        );
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.pending_approval, 1);
    }

    /// The segment shell's visibility rules: segments with fewer than two
    /// entries render flat (no cover); collapsed shows the cover alone
    /// whether live or settled; expanded — or collapsed with an approval
    /// pending — shows every entry.
    #[test]
    fn render_thinking_collapsed_visibility_rules() {
        let tool = |id: &str, status: ToolCallStatus| {
            ActivityEntry::Tool(ToolCallItem {
                id: id.into(),
                name: "Read".into(),
                title: String::new(),
                status,
                output: String::new(),
                is_error: false,
                input: serde_json::Value::Null,
                streaming: false,
                collapsed: true,
                user_toggled: false,
                panel: None,
            })
        };
        let mut t = ThinkingContainer::new();
        t.accepting_entries = false;
        t.streaming = false;
        t.collapsed = true;
        t.entries.push(tool("1", ToolCallStatus::Success));

        // Single entry: flat, no cover.
        let layout = segment_layout(&t);
        assert!(!layout.cover);
        assert_eq!(layout.visible, vec![0]);

        t.entries.push(tool("2", ToolCallStatus::Success));

        // Settled + collapsed: cover only, no entries.
        let layout = segment_layout(&t);
        assert!(layout.cover && !layout.expanded);
        assert!(
            layout.visible.is_empty(),
            "settled + collapsed shows no entries"
        );

        // Live + collapsed: still the cover alone.
        t.streaming = true;
        if let ActivityEntry::Tool(e) = &mut t.entries[0] {
            e.streaming = true;
            e.status = ToolCallStatus::Running;
        }
        let layout = segment_layout(&t);
        assert!(layout.cover && !layout.expanded);
        assert!(
            layout.visible.is_empty(),
            "live + collapsed shows no entries"
        );

        // Expanded: every entry renders.
        t.collapsed = false;
        let layout = segment_layout(&t);
        assert!(layout.cover && layout.expanded);
        assert_eq!(layout.visible, vec![0, 1]);

        // Collapsed again but an approval is pending: forced open.
        t.collapsed = true;
        if let ActivityEntry::Tool(e) = &mut t.entries[1] {
            e.status = ToolCallStatus::PendingApproval;
        }
        let layout = segment_layout(&t);
        assert!(layout.expanded, "pending approval forces the segment open");
        assert_eq!(layout.visible, vec![0, 1]);
    }

    /// `MessageContent::Thinking` folds into the same activity segment as
    /// tool calls — one `ThinkingContainer` holds both reasoning rounds and
    /// tool entries. This mirrors the live `apply()` behavior where
    /// `AgentThinking` deltas fold into the active segment.
    #[test]
    fn build_items_folds_thinking_into_activity_segment() {
        let messages = vec![
            Message::user("go".to_string()),
            Message::assistant(vec![
                MessageContent::Thinking {
                    text: "let me think about this".to_string(),
                    signature: None,
                },
                MessageContent::ToolUse(LanguageModelToolUse {
                    id: "tu_1".to_string(),
                    name: Arc::from("Read"),
                    raw_input: String::new(),
                    input: serde_json::json!({"path": "a.rs"}),
                    is_input_complete: true,
                    thought_signature: None,
                }),
            ]),
            Message::user_with_content(vec![MessageContent::ToolResult(LanguageModelToolResult {
                tool_use_id: "tu_1".to_string(),
                tool_name: Arc::from("Read"),
                is_error: false,
                content: "file contents".to_string(),
            })]),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        let containers: Vec<&ThinkingContainer> = items
            .iter()
            .filter_map(|i| match i {
                ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(containers.len(), 1, "one container for thinking + tool");
        let t = containers[0];
        assert_eq!(t.entries.len(), 2, "one reasoning + one tool");
        assert!(
            matches!(&t.entries[0], ActivityEntry::Reasoning { text, .. } if text == "let me think about this"),
            "first entry is reasoning"
        );
        assert!(
            matches!(&t.entries[1], ActivityEntry::Tool(tool) if tool.id == "tu_1"),
            "second entry is tool"
        );
    }

    /// Multiple `Thinking` blocks within the same turn produce multiple
    /// reasoning entries within the same activity segment.
    #[test]
    fn build_items_multiple_thinking_rounds_in_one_segment() {
        let messages = vec![
            Message::user("go".to_string()),
            // First assistant response: thinking + tool
            Message::assistant(vec![
                MessageContent::Thinking {
                    text: "round 1".to_string(),
                    signature: None,
                },
                MessageContent::ToolUse(LanguageModelToolUse {
                    id: "tu_1".to_string(),
                    name: Arc::from("Read"),
                    raw_input: String::new(),
                    input: serde_json::Value::Null,
                    is_input_complete: true,
                    thought_signature: None,
                }),
            ]),
            Message::user_with_content(vec![MessageContent::ToolResult(LanguageModelToolResult {
                tool_use_id: "tu_1".to_string(),
                tool_name: Arc::from("Read"),
                is_error: false,
                content: "done".to_string(),
            })]),
            // Second assistant response: more thinking + text.
            // The thinking folds into the same segment since the text comes
            // after it (text closes the segment, but thinking is already in).
            Message::assistant(vec![
                MessageContent::Thinking {
                    text: "round 2".to_string(),
                    signature: None,
                },
                MessageContent::Text("here is the answer".to_string()),
            ]),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        let containers: Vec<&ThinkingContainer> = items
            .iter()
            .filter_map(|i| match i {
                ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .collect();
        // Both thinking rounds and the tool share one segment (the text
        // closes it after the second thinking is already added).
        assert_eq!(containers.len(), 1, "one segment for the whole turn");
        let t = containers[0];
        assert_eq!(t.entries.len(), 3, "2 reasoning + 1 tool");
        // Verify entry types and order.
        assert!(matches!(
            &t.entries[0],
            ActivityEntry::Reasoning { text, .. } if text == "round 1"
        ));
        assert!(matches!(&t.entries[1], ActivityEntry::Tool(tool) if tool.id == "tu_1"));
        assert!(matches!(
            &t.entries[2],
            ActivityEntry::Reasoning { text, .. } if text == "round 2"
        ));
    }

    /// Historical rebuild of the issue #216 scenario: text THEN thinking within
    /// one assistant message must produce two activity segments, not fold the
    /// post-text thinking into the pre-text segment. The `Text` arm calls
    /// `close_segment` so the subsequent `Thinking` opens a fresh container.
    /// This test confirms the rebuild path is correct (the live `apply` path
    /// is fixed by `close_for_text` to match this behavior).
    #[test]
    fn build_items_text_before_thinking_opens_new_segment() {
        // Round 1: thinking + tool → tool result
        // Round 2: Text THEN Thinking (the issue's temporal-inversion scenario)
        let messages = vec![
            Message::user("go".to_string()),
            Message::assistant(vec![
                MessageContent::Thinking {
                    text: "round 1".to_string(),
                    signature: None,
                },
                tu("tu_1", "Read", serde_json::json!({"path": "a.rs"})),
            ]),
            Message::user_with_content(vec![tr("tu_1", "Read", "a")]),
            Message::assistant(vec![
                MessageContent::Text("the answer".to_string()),
                MessageContent::Thinking {
                    text: "round 2".to_string(),
                    signature: None,
                },
            ]),
        ];
        let items = build_items(&messages, &HashMap::new(), false, None);
        let containers: Vec<&ThinkingContainer> = items
            .iter()
            .filter_map(|i| match i {
                ConvItem::Thinking(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(
            containers.len(),
            2,
            "text closes old segment, thinking opens new"
        );
        assert_eq!(containers[0].entries.len(), 2, "round 1: thinking + tool");
        assert_eq!(containers[1].entries.len(), 1, "round 2: thinking only");
        assert_eq!(
            items
                .iter()
                .filter(|i| matches!(i, ConvItem::Assistant { .. }))
                .count(),
            1,
            "one assistant text between the two segments"
        );
    }

    #[test]
    fn build_items_does_not_render_external_event_as_user_bubble() {
        let mut external = Message::user("CI pipeline completed".into());
        external.ui = Some(manox_agent::MessageUiMetadata {
            external_event: Some(true),
            ..Default::default()
        });
        let items = build_items(
            &[Message::user("wait for CI".into()), external],
            &HashMap::new(),
            false,
            None,
        );
        assert_eq!(
            items
                .iter()
                .filter(|item| matches!(item, ConvItem::User { .. }))
                .count(),
            1,
            "machine-generated events must not be attributed to the user"
        );
    }

    /// A reloaded peer delivery is a user-role turn like any other: the same
    /// bubble, carrying the sender as `from`, the owning agent as `to`, and the
    /// unwrapped body the sender actually wrote.
    #[test]
    fn build_items_rebuilds_peer_deliveries_as_attributed_user_turns() {
        let mut peer = Message::user("[from alice]: report".into());
        peer.ui = Some(manox_agent::MessageUiMetadata {
            author: Some(manox_agent::MessageAuthor::Agent("alice".into())),
            peer: true,
            display_text: Some("report".into()),
            ..Default::default()
        });
        let items = build_items(
            &[peer],
            &HashMap::new(),
            false,
            Some(manox_agent::MessageAuthor::Lead),
        );
        let Some(ConvItem::User { text, meta, .. }) = items.first() else {
            panic!(
                "a peer delivery renders as a user turn, got {:?}",
                items.first()
            );
        };
        let meta = meta.as_ref().expect("a peer turn carries its chrome");
        assert_eq!(
            text, "report",
            "the wrapped model-facing form stays out of view"
        );
        assert_eq!(
            meta.author,
            Some(manox_agent::MessageAuthor::Agent("alice".into())),
            "the sender is the header's `from`"
        );
        assert_eq!(meta.recipient, Some(manox_agent::MessageAuthor::Lead));
        assert!(meta.peer, "the inbound-peer marker survives the reload");
    }

    /// The header's segment order and joiners: `{from} > {to}·{model}·{time}`,
    /// with empty segments dropped and no `>` clause when nothing follows.
    #[test]
    fn user_turn_header_orders_from_then_recipient_model_time() {
        assert_eq!(
            user_turn_header("你", "船长", "glm-5.2", "17:55"),
            "你 > 船长·glm-5.2·17:55"
        );
        assert_eq!(
            user_turn_header("You", "Sailor", "", "17:55"),
            "You > Sailor·17:55"
        );
        assert_eq!(user_turn_header("You", "", "", ""), "You");
    }

    #[test]
    fn build_items_keeps_author_on_user_bubble() {
        let mut seed = Message::user("implement the approved plan".into());
        seed.ui = Some(manox_agent::MessageUiMetadata {
            author: Some(manox_agent::MessageAuthor::Lead),
            ..Default::default()
        });
        let items = build_items(&[seed], &HashMap::new(), false, None);
        let Some(ConvItem::User { meta, .. }) = items.first() else {
            panic!("agent-authored seed stays a user bubble");
        };
        assert_eq!(
            meta.as_ref().unwrap().author,
            Some(manox_agent::MessageAuthor::Lead)
        );
    }

    /// Regression: user messages (and every other text-bearing kind) must feed
    /// a persistent `Entity<Markdown>`. A per-frame `markdown_tv` mount would
    /// reset document selection each frame, so the body could not be selected
    /// or copied; `text_body_of` decides which kinds own a persistent body.
    #[test]
    fn text_body_of_identifies_all_selectable_bodies() {
        let user = ConvItem::User {
            text: "hello".into(),
            images: Vec::new(),
            meta: None,
        };
        assert_eq!(text_body_of(&user), Some((false, "hello".into())));

        let assistant = ConvItem::Assistant {
            text: "hi".into(),
            streaming: true,
            token_usage: None,
            activity_header: false,
            entry_id: None,
            fork_unavailable: None,
        };
        assert_eq!(text_body_of(&assistant), Some((true, "hi".into())));

        let error = ConvItem::Error("boom".into());
        assert_eq!(text_body_of(&error), Some((false, "boom".into())));

        // `Notice` owns a paginated `TerminalPanel` body, not a markdown
        // document — `text_body_of` reports `None` for it.
        let notice = ConvItem::Notice("n".into());
        assert_eq!(text_body_of(&notice), None);

        let recap = ConvItem::Recap {
            summary: "sum".into(),
            collapsed: true,
            user_toggled: false,
        };
        assert_eq!(text_body_of(&recap), Some((false, "sum".into())));

        let retry = ConvItem::Retry {
            attempt: 1,
            max_attempts: 3,
            delay_secs: 1,
            reason: "429".into(),
            detail: Some("provider body".into()),
            collapsed: true,
            user_toggled: false,
        };
        assert_eq!(text_body_of(&retry), Some((false, "provider body".into())));
        let retry_no_detail = ConvItem::Retry {
            attempt: 1,
            max_attempts: 3,
            delay_secs: 1,
            reason: "429".into(),
            detail: None,
            collapsed: true,
            user_toggled: false,
        };
        assert_eq!(text_body_of(&retry_no_detail), None);

        // Non-text kinds own their persistence elsewhere (activity entries /
        // `TerminalPanel`) and must not mount a body markdown entity.
        let thinking = ConvItem::Thinking(ThinkingContainer::new());
        assert_eq!(text_body_of(&thinking), None);
    }

    /// A fork anchor is only attached where a fork is actually valid, and the
    /// control explains every case where it is not. Three gates: the rows must
    /// be a faithful journal replay, the reply must close its turn (no tool
    /// call, whose result rides a later entry), and the reply must have landed
    /// (a streamed delta has no durable row).
    #[test]
    fn fork_anchor_eligibility_follows_replay_and_turn_tail() {
        let usage = HashMap::new();

        let build = |messages: &[Message], replayed: bool| -> Vec<ConvItem> {
            let mut items = Vec::new();
            let source = replayed.then(|| "session-1".to_string());
            let mut builder = ItemBuilder::new(None).with_fork_source(source);
            builder.extend(messages, &usage, &mut items);
            items
        };

        let assistant_conv = |items: &[ConvItem]| -> (Option<String>, Option<ForkUnavailable>) {
            items
                .iter()
                .find_map(|item| match item {
                    ConvItem::Assistant {
                        entry_id,
                        fork_unavailable,
                        ..
                    } => Some((entry_id.clone(), *fork_unavailable)),
                    _ => None,
                })
                .expect("the batch produces an assistant item")
        };

        // A plain text reply that closes its turn, replayed: forkable.
        let mut reply = Message::assistant(vec![MessageContent::Text("done".to_string())]);
        reply.id = "entry-1".to_string();
        let (anchor, gate) = assistant_conv(&build(&[reply.clone()], true));
        assert_eq!(anchor.as_deref(), Some("entry-1"));
        assert_eq!(gate, None, "a completed turn tail must be forkable");

        // The same reply in a display-only rebuild (sub-agent panel, synthetic
        // backfill): its id names no entry of the session, so no anchor.
        let (anchor, gate) = assistant_conv(&build(&[reply.clone()], false));
        assert_eq!(anchor, None);
        assert_eq!(
            gate,
            Some(ForkUnavailable::NotReplayed),
            "a row outside the session record must say so rather than offer a doomed call"
        );

        // A reply that carries a tool call does not close its turn: its result
        // rides the next entry, so a prefix ending here would be repaired with
        // a synthetic result and the child would start from false history.
        let mut mid_turn = Message::assistant(vec![
            MessageContent::Text("let me check".to_string()),
            tu("tu_1", "Read", serde_json::json!({"path": "a.rs"})),
        ]);
        mid_turn.id = "entry-2".to_string();
        let (anchor, gate) = assistant_conv(&build(&[mid_turn], true));
        assert_eq!(anchor, None);
        assert_eq!(gate, Some(ForkUnavailable::MidTurn));
    }
}
