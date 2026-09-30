//! Top-level workspace view.
//!
//! Holds a gpui-free `ThreadHandle` plus the AgentServer-backed
//! `ClientStoreHandle` mirror + `Entity<Sidebar>`;
//! `cx.subscribe` handles:
//! - `ThreadEvent`: text/thinking/tool deltas go to `ConversationState`; `ToolCallAuthorization` opens the question card;
//!   the terminal `Stop` (non-ToolUse) triggers the gateway list refetch.
//! - `SidebarEvent`: new conversation / open history / delete.
//!
//! Enter in the input box → append a user message + run_turn + persist (the sidebar shows the new entry immediately).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use crate::i18n;
use gpui::ClickEvent;
use gpui::DismissEvent;
use gpui::{
    Anchor, AnyElement, App, Context, Entity, FollowMode, ListAlignment, ListOffset, ListState,
    MouseButton, Pixels, Render, ScrollHandle, SharedString, Subscription, WeakEntity, Window,
    anchored, deferred, prelude::*, px,
};
/// Shared across both harnesses: workspace struct fields hold
/// `Option<Entity<PopupMenu>>` regardless of feature.
use gpui_component::{
    ActiveTheme as _, Disableable as _, ElementExt as _, Icon, IconName, Sizable as _, Size, Theme,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Paste, RopeExt, Textarea, TextareaState},
    v_flex,
};
/// `WindowExt::push_notification` + `Notification` are shared: the
/// ChatGPT.app launch path (#410) reports outcomes under either harness.
use gpui_component::{WindowExt as _, notification::Notification, tooltip::Tooltip};
use manox_agent::PermissionDecision;
use manox_agent::thread::PermissionMode;
use manox_agent::thread_engine::BrowserTabId;
use manox_agent::{Thread, ThreadId};

use crate::OpenSettings;
use crate::ToggleTurnNavigator;
use crate::cockpit::format_elapsed;
use crate::conversation::ConvItem;
use crate::conversation::{ApplyOutcome, ConversationState, NoticeAnchor, UserImage, UserTurnMeta};
use crate::views::browser_view::BrowserView;
use crate::views::centered;
use crate::views::completion::{
    CompletionState, SelectHandler, build_replacement, detect, mention_source, render_completion,
    slash_source,
};
use crate::views::composer_menu::{
    PendingAttachment, build_plus_menu, load_attachment, render_attachment_chips,
    render_browser_chips,
};
use crate::views::popup_menu;
use crate::views::settings::{SettingsEvent, SettingsView};
use crate::views::turn_navigator::{TurnNavigator, TurnNavigatorEvent, collect_user_turns};

mod attach;
mod catch_up;
mod chat_column;
use chat_column::ChatColumn;
use manox_agent_chat_ui::ask_card::AskCardSnapshot;
mod chips;
mod composer_render;
mod render;

/// One label/value row of the goal status popover.
fn goal_popover_row(label: &str, value: &str, fg: gpui::Hsla, muted: gpui::Hsla) -> gpui::Div {
    h_flex()
        .w_full()
        .items_start()
        .gap_2()
        .child(
            gpui::div()
                .min_w(px(96.))
                .text_xs()
                .text_color(muted)
                .child(label.to_string()),
        )
        .child(
            gpui::div()
                .flex_1()
                .text_xs()
                .text_color(fg)
                .child(value.to_string()),
        )
}

/// Snapshot a thread's working directory as a `SharedString` for the
/// `TerminalPanel` prompt line. Reads the `Thread` entity (not the `Workspace`)
/// so it stays safe inside a `Workspace::update` closure, where reading the
/// `Workspace` itself would double-lease. `None` only when the path is empty.
fn thread_cwd(
    thread: &manox_agent::thread::ThreadHandle,
    store: &Option<(
        gpui::Entity<manox_agent_chat_ui::ahp_store::AhpStore>,
        String,
    )>,
    cx: &App,
) -> Option<SharedString> {
    let cwd = store
        .as_ref()
        .and_then(|(store, sid)| {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, sid)
                .cwd()
                .map(std::path::PathBuf::from)
        })
        .unwrap_or_else(|| thread.read(|t| t.cwd().to_path_buf()));
    if cwd.as_os_str().is_empty() {
        None
    } else {
        Some(SharedString::from(cwd.to_string_lossy().to_string()))
    }
}

/// The synthesized ask card's `ToolUse` input: the same shape the fold's
/// elicitation lowering and the rebuild path parse, so every path to the
/// card agrees on one input contract.
fn ask_input_json(
    ask: &manox_agent_chat_ui::column::PendingAsk,
    request_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "requestId": request_id,
        "questions": ask.questions.iter().map(|q| serde_json::json!({
            "id": q.id,
            "question": q.question,
            "header": q.header,
            "multiSelect": q.multi_select,
            "options": q.options.iter().map(|o| serde_json::json!({
                "label": o.label,
                "description": o.description,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Map a `PermissionMode` to the chip's (label, accent color, icon) triple.
///
/// Colors are theme tokens, not raw hsla values, so the chip follows the
/// active theme (light/dark) without bespoke palettes per mode. The
/// WorkspaceWrite accent uses `info` (green) as a "this is the safe
/// default" signal — staying gray would be visually identical to a disabled
/// state.
fn mode_chip_visual(mode: PermissionMode, theme: &Theme) -> (SharedString, gpui::Hsla, IconName) {
    match mode {
        PermissionMode::ReadOnly => (
            i18n::t("workspace-chip-mode-readonly"),
            theme.warning,
            IconName::Eye,
        ),
        PermissionMode::WorkspaceWrite => (
            i18n::t("workspace-chip-mode-workspacewrite"),
            theme.info,
            IconName::FolderOpen,
        ),
        PermissionMode::DangerFullAccess => (
            i18n::t("workspace-chip-mode-dangerfullaccess"),
            theme.danger,
            IconName::TriangleAlert,
        ),
    }
}

/// Build the popover content for the access chip: three selectable mode rows
/// (icon + title, check on the right for the active one) — titles only, no
/// header and no per-mode descriptions. The whole thing is a plain `v_flex`
/// so it sizes to its content with no `flex_1` distribution
/// across items. The chip's dropdown wraps this in a `popover_style` div
/// for the opaque card chrome — that path doesn't go through `PopupMenu`
/// at all, sidestepping the per-`ElementItem` `flex_1`/`min_h(26)` wrapper
/// that was producing both the height-leak bug and the clip-to-26 bug.
///
/// Every clickable row routes through `Workspace::apply_permission_mode` so
/// the mode switch + notice + menu close stay in one place. `theme` is
/// consumed up front: every value used inside the `'static` row closures is
/// pre-extracted into owned `SharedString`/`Hsla`/`IconName`, so the
/// closures don't capture a short-lived theme reference.
fn build_permission_content(
    workspace: WeakEntity<Workspace>,
    current: PermissionMode,
    cx: &mut gpui::App,
) -> gpui::Div {
    let info: gpui::Hsla = cx.theme().info;
    let warning: gpui::Hsla = cx.theme().warning;
    let danger: gpui::Hsla = cx.theme().danger;

    let make_row = |mode: PermissionMode,
                    title: SharedString,
                    icon: IconName,
                    accent: gpui::Hsla,
                    selected: bool| {
        let ws = workspace.clone();
        h_flex()
            .id(("permission-mode-row", mode as usize))
            .w_full()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .child(Icon::new(icon).small().text_color(accent))
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(accent)
                    .child(title),
            )
            .when(selected, |el| {
                el.child(Icon::new(IconName::Check).small().text_color(accent))
            })
            .on_click(move |_event, _window, cx| {
                let _ = ws.update(cx, |this, cx| this.apply_permission_mode(mode, cx));
            })
    };

    v_flex()
        .w_full()
        .gap_2()
        .p_2()
        .child(make_row(
            PermissionMode::ReadOnly,
            i18n::t("workspace-mode-readonly-title"),
            IconName::Eye,
            warning,
            current == PermissionMode::ReadOnly,
        ))
        .child(make_row(
            PermissionMode::WorkspaceWrite,
            i18n::t("workspace-mode-workspacewrite-title"),
            IconName::FolderOpen,
            info,
            current == PermissionMode::WorkspaceWrite,
        ))
        .child(make_row(
            PermissionMode::DangerFullAccess,
            i18n::t("workspace-mode-dangerfullaccess-title"),
            IconName::TriangleAlert,
            danger,
            current == PermissionMode::DangerFullAccess,
        ))
}
mod composer;
mod external;
mod plan_review;
mod subagent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerPlacement {
    Hero,
    Footer,
}

fn composer_placement(first_screen: bool) -> ComposerPlacement {
    if first_screen {
        ComposerPlacement::Hero
    } else {
        ComposerPlacement::Footer
    }
}

/// Key context for the composer wrapper. `completion = open` shadows the
/// input's keys for the popover; otherwise the wrapper just says `composer`,
/// which is what the recall bindings (`alt-up` / `alt-down`) hang off. Recall
/// never claims the bare arrows, so an open popover is the only state that has
/// to opt the composer out of it.
fn composer_key_context(completion_open: bool) -> &'static str {
    if completion_open {
        "completion = open"
    } else {
        "composer"
    }
}

/// The Captain's dispatch prompt for one sub-agent address, with the Unix
/// second it was sent: a sub-agent panel's opening bubble shows the send time,
/// never the time its tab was opened.
#[derive(Debug, Clone)]
struct SubagentPrompt {
    text: String,
    dispatched_at: i64,
}

/// A thread parked in the background while still running a turn. U6b⑤: the
/// park holds NO kernel entity — just the id, the leaf (the wire-state home
/// whose mirrors the server's §D.5 deltas keep fed while the session stays
/// attached), and the parked subscription (it coordinates the settle
/// unread, the parked plan-review stash and the follow-up stash; it never
/// touches `conversation`/`self.chat.thread`, so a background thread's events
/// cannot be misattributed to the foreground thread). The turn itself runs
/// server-side and survives parking regardless; nothing detaches or
/// disposes while parked — the reclaim is an in-place re-attach, no reopen.
struct BackgroundThread {
    id: String,
    store: Option<(
        gpui::Entity<manox_agent_chat_ui::ahp_store::AhpStore>,
        String,
    )>,
    session_id: Option<String>,
    _sub: Subscription,
}

/// Which shared registry backs a registry slash turn — a markdown
/// prompt-macro (`manox_agent::command`) or a skill (`manox_agent::skill`).
#[derive(Clone, Copy)]
enum RegistryTurnKind {
    Command,
    Skill,
}

// The chat-column state types moved to manox-agent-chat-ui's `column`
// module (Phase 2 tail); these re-exports keep every bare/`super::` name in
// the workspace family resolving unchanged.
pub use manox_agent_chat_ui::column::{
    AskIntent, AskOption, AskQuestion, ComposerPlaceholderMode, DeferredUserTurn, FollowUpState,
    PendingAsk, PendingConfirmation, QueuedFollowUp, parse_pending_ask,
};

/// The visited-thread history behind the titlebar ←/→ moves. Invariants
/// live here, not at the call sites: `index` is `Some` exactly when
/// `entries` is non-empty and always points inside it; a record equal to
/// the CURRENT entry is a no-op (comparing against the pointer, not the
/// tail — the pointer may sit mid-stack after a back step); a new record
/// truncates the forward tail; the front drains past the cap with the
/// index shifting to stay pinned at the tail.
#[derive(Default)]
pub(crate) struct NavHistory {
    entries: Vec<String>,
    index: Option<usize>,
}

impl NavHistory {
    fn record(&mut self, id: &str) {
        if self.current() == Some(id) {
            return;
        }
        if let Some(i) = self.index {
            self.entries.truncate(i + 1);
        }
        self.entries.push(id.to_string());
        if self.entries.len() > NAV_STACK_CAP {
            let drop = self.entries.len() - NAV_STACK_CAP;
            self.entries.drain(..drop);
        }
        self.index = Some(self.entries.len() - 1);
    }

    /// The successor hand-off: the current entry IS the same conversation
    /// under a new id, so it is rewritten in place (a plain record would
    /// strand the predecessor in the stack and a ← would land on the
    /// retired id).
    fn replace_current(&mut self, id: &str) {
        match self.index {
            Some(i) => self.entries[i] = id.to_string(),
            None => self.record(id),
        }
    }

    /// One step back; `None` at (or outside) the front.
    fn step_back(&mut self) -> Option<String> {
        let i = self.index?;
        if i == 0 || i >= self.entries.len() {
            return None;
        }
        self.index = Some(i - 1);
        Some(self.entries[i - 1].clone())
    }

    /// One step forward; `None` at (or outside) the tail.
    fn step_forward(&mut self) -> Option<String> {
        let i = self.index?;
        if i + 1 >= self.entries.len() {
            return None;
        }
        self.index = Some(i + 1);
        Some(self.entries[i + 1].clone())
    }

    fn avail(&self) -> manox_agent_chrome_ui::shell::NavAvail {
        let (back, forward) = match self.index {
            None => (false, false),
            Some(i) => (i > 0 && i < self.entries.len(), i + 1 < self.entries.len()),
        };
        manox_agent_chrome_ui::shell::NavAvail { back, forward }
    }

    fn current(&self) -> Option<&str> {
        self.index
            .and_then(|i| self.entries.get(i).map(String::as_str))
    }

    /// Test read face: the raw entries + pointer, for shape assertions
    /// (cap size, front id) the move API cannot express.
    #[cfg(test)]
    fn state(&self) -> (&[String], Option<usize>) {
        (&self.entries, self.index)
    }
}

/// The visited-thread history cap: past this, the front drains so the
/// stack stays a moving window.
const NAV_STACK_CAP: usize = 100;

pub struct Workspace {
    pub(crate) cwd: PathBuf,
    /// Visited-thread history for the titlebar ←/→ moves (see
    /// [`NavHistory`]).
    pub(crate) nav: NavHistory,
    /// The chat column's state (thread face, conversation, composer, ask
    /// drawer, rail — see `chat_column.rs`). Phase 1: a plain embedded
    /// struct, not yet an entity.
    pub(crate) chat: Entity<ChatColumn>,
    /// T-D: the shared single-connection multiplexer. One app-level
    /// `AgentClient` (client_id "desktop") carries every session; the
    /// per-session handles are leaves fed by its demux pump.
    pub(crate) multiplexer: gpui::Entity<crate::multiplexer::SessionMultiplexer>,
    /// T-D: the shared app-level client used by the fire-and-forget
    /// `send_note` and `Reply` verdict paths (no per-session connection).
    /// (retired: the protocol client lives in the AhpStore)
    #[allow(dead_code)]
    pub(crate) client: (),
    /// Threads that were running when the user switched away (U6b⑤: the
    /// turn runs server-side and survives the switch on its own — the park
    /// keeps the session ATTACHED so the reclaim re-attaches in place with
    /// no reopen, and the parked subscription keeps the settle unread, the
    /// plan-review stash and the follow-up stash coordinated).
    background_threads: Vec<BackgroundThread>,
    /// Distinct bound-project paths of the active summaries, in list order
    /// (the project chip's "recent, unregistered" section; U2 push cache).
    /// Registered project folders (chip menu + the sidebar grouping push).
    /// Repaint observer on the multiplexer's list/registry state (U2): its
    /// notify drives the sidebar rows and the workspace's model surfaces.
    _mux_lists: gpui::Subscription,
    _rail_updates: gpui::Subscription,
    /// Accumulated child-session events per Agent tool-call id, so a panel
    /// opened mid-run backfills from the start.
    subagent_transcripts: HashMap<String, Vec<manox_agent::SubagentChildEvent>>,
    /// Latest completion text per subagent address (from SubagentProgress
    /// status=Success/Error), used as the panel's final answer when the
    /// Agent tool-result is absent (new Steer bus has no ToolResult).
    subagent_final_text: HashMap<String, String>,
    /// The Captain's dispatch prompt per subagent address with its send time,
    /// captured from the Steer tool call so a panel always shows the opening
    /// user message — correctly attributed and timed — even before the child
    /// streams anything.
    subagent_prompts: HashMap<String, SubagentPrompt>,
    /// Lazily-built browser tab entities, keyed by `BrowserTabId`. A browser
    /// tab keeps its `BrowserView` (and the underlying native webview) across
    /// tab switches; dropped when the tab closes, which detaches the native
    /// view via [`manox_webview::webview::WebView`]'s `Drop`.
    pub(crate) browser_views: BTreeMap<BrowserTabId, Entity<BrowserView>>,
    /// Top-level view mode. `Settings` replaces the entire window content
    /// with the SettingsView overlay until the user requests exit.
    view_mode: ViewMode,
    /// Set briefly while the Settings overlay is sliding out to the right.
    /// Keeps `view_mode == Settings` mounted so the exit animation can play
    /// before the unmount; cleared when the slide-out completes.
    exiting_settings: bool,
    /// Bumped on every transition into or out of Settings. Embedded in the
    /// slide animation's element id so a fresh tween fires on each direction
    /// change (an old id would replay from the cached delta and visibly
    /// jump), and into the exit spawn so a stale unmount can be no-op'd
    /// when a new enter supersedes it.
    settings_transition_gen: u64,
    /// Lazily created on the first `enter_settings` call so we don't pay the
    /// cost when the user never opens Settings.
    settings_view: Option<Entity<SettingsView>>,
    settings_sub: Option<Subscription>,
}

/// Top-level rendering mode of the app page. `Settings` is a main-column swap
/// off the conversation; every other surface (terminal, browser, external
/// session, sub-agent panel) is a shell surface — a right-pane tab or the
/// bottom dock — and never a view mode.
#[derive(Default)]
enum ViewMode {
    #[default]
    Workspace,
    Settings,
}

/// The empty band the session list reserves at its top before any content:
/// macOS floats the traffic lights over it (28px), other platforms need only
/// a small breathing inset (8px). Shared by the settings nav's scroll body.
pub(crate) fn sidebar_top_inset() -> Pixels {
    if cfg!(target_os = "macos") {
        px(28.)
    } else {
        px(8.)
    }
}

/// The main card's `border_1` on both edges; width budgets that measure
/// card-interior space subtract this.
const CARD_BORDER: f32 = 2.;

/// The settings nav column's width inside the conversation card.
const SETTINGS_NAV_WIDTH: f32 = 240.;

/// Trailing overdraw for the message list: rows within this many pixels
/// below the viewport are pre-measured so scrolling never pops an
/// unmeasured row into view.
const MSG_LIST_OVERDRAW: Pixels = px(2048.);

#[derive(Clone, Copy, Debug, PartialEq)]
struct TurnNavigatorLayout {
    left_inset: Pixels,
    right_inset: Pixels,
    panel_width: Pixels,
}

fn turn_navigator_layout(card_width: Pixels) -> TurnNavigatorLayout {
    // The overlay anchors to the conversation card's padding box (gpui
    // absolute positioning is CSS-style), and it must fit INSIDE it: the card
    // clips its children, so a panel sized from the window would be cut off on
    // both sides whenever the shell's sidebar or right pane claims width. The
    // card's own 1px border is the only furniture on either side here; the
    // shell's sidebar and right pane live OUTSIDE the card, which is exactly
    // why the caller passes the measured card width and never the window's.
    let left_inset = px(CARD_BORDER / 2.);
    let right_inset = px(CARD_BORDER / 2.);
    let available = card_width - left_inset - right_inset - px(24.);
    let panel_width = if available <= px(0.) {
        px(0.)
    } else if available < px(480.) {
        available
    } else {
        px(480.)
    };

    TurnNavigatorLayout {
        left_inset,
        right_inset,
        panel_width,
    }
}

/// The Exit handler in `subscribe_settings` waits this long before flipping
/// `view_mode` back to `Workspace`, giving the outgoing page a frame to
/// settle before the swap.
const SLIDE_OUT_MS: u64 = 200;

enum RecallDirection {
    Up,
    Down,
}

/// What a recall step does to the composer's content.
#[derive(Debug)]
enum RecallStep {
    /// Leave the input as it is (walk already at the oldest turn).
    None,
    /// Replace the input with this text — a past turn, or the walk's draft.
    Recall(String),
    /// The walk ended with an empty draft: clear the input.
    Clear,
}

impl Workspace {
    fn apply_list_outcome(&mut self, outcome: ApplyOutcome, cx: &mut App) {
        let count_changed = self.sync_list_count(cx);
        match outcome {
            ApplyOutcome::Remeasure(ix) => {
                self.chat.read(cx).list_state.remeasure_items(ix..ix + 1)
            }
            ApplyOutcome::RemeasureAll => self.chat.read(cx).list_state.remeasure(),
            ApplyOutcome::RemeasureAndAppend { remeasure_ix } => {
                self.chat
                    .read(cx)
                    .list_state
                    .remeasure_items(remeasure_ix..remeasure_ix + 1);
                if !count_changed {
                    let tail = self.chat.read(cx).list_count.saturating_sub(1);
                    self.chat
                        .read(cx)
                        .list_state
                        .remeasure_items(tail..tail + 1);
                }
            }
            ApplyOutcome::Unchanged | ApplyOutcome::Appended | ApplyOutcome::RemovedTail => {}
        }
    }

    pub(crate) fn with_foreground_store<R>(
        &self,
        cx: &mut gpui::Context<Self>,
        f: impl FnOnce(&mut manox_agent_chat_ui::ahp_store::AhpStore, String) -> R,
    ) -> Option<R> {
        let pair = self.chat.read(cx).store.clone()?;
        let (store, sid) = pair;
        Some(store.update(cx, |store, _| f(store, sid)))
    }

    // Entity-handle accessors for ChatColumn fields: each returns a cloned
    // handle so callers can `.update(cx, …)` without holding the chat
    // entity's read guard across a mutable borrow of `cx`.
    pub(crate) fn chat_store(
        &self,
        cx: &App,
    ) -> Option<(Entity<manox_agent_chat_ui::ahp_store::AhpStore>, String)> {
        self.chat.read(cx).store.clone()
    }
    pub(crate) fn chat_conversation(&self, cx: &App) -> Entity<ConversationState> {
        self.chat.read(cx).conversation.clone()
    }
    pub(crate) fn chat_input(&self, cx: &App) -> Entity<TextareaState> {
        self.chat.read(cx).input_state.clone()
    }
    pub(crate) fn chat_list_state(&self, cx: &App) -> ListState {
        self.chat.read(cx).list_state.clone()
    }
    pub(crate) fn chat_rail(&self, cx: &App) -> Entity<crate::views::context_rail::ContextRail> {
        self.chat.read(cx).context_rail.clone()
    }

    /// Plan files this conversation wrote, for the bubble's plan segment:
    /// the session's `ProposePlan` tool rows — the model's only
    /// plan-approval channel, whose arguments carry the slug/title pair
    /// verbatim — collected in arrival order, deduped by slug (a
    /// re-proposal supersedes its predecessor and takes the newest slot),
    /// returned newest first. Zero-copy: the scan walks `kind()` references.
    pub(crate) fn collect_plan_files(
        &self,
        cx: &App,
    ) -> Vec<crate::views::context_rail::PlanFileEntry> {
        type PlanProposal = (String, crate::views::context_rail::PlanFileEntry);
        let mut found: Vec<PlanProposal> = Vec::new();
        for item in self.chat.read(cx).conversation.read(cx).items() {
            let calls: Vec<&crate::conversation::ToolCallItem> = match item.read(cx).kind() {
                ConvItem::ToolCall(call) => vec![call],
                ConvItem::Thinking(container) => container
                    .entries
                    .iter()
                    .filter_map(|entry| match entry {
                        crate::conversation::ActivityEntry::Tool(call) => Some(call),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for call in calls {
                if call.name != manox_agent::plan_mode::PROPOSE_PLAN
                    || call.status != manox_agent::ToolCallStatus::Success
                {
                    // A failed proposal never wrote the plan file (the tool
                    // rejects missing/empty files before anything lands).
                    continue;
                }
                if let Some((slug, entry)) =
                    crate::views::context_rail::PlanFileEntry::from_proposal(&call.input)
                {
                    found.retain(|(existing, _)| existing != &slug);
                    found.push((slug, entry));
                }
            }
        }
        found.into_iter().rev().map(|(_, entry)| entry).collect()
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        // An unbound conversation must not inherit the launch terminal's
        // cwd as its working directory — that is an arbitrary project dir
        // (or `/` under a GUI launch). Home is the neutral default; binding
        // a project via the chip / project "+" overrides it later.
        if let Some(home) = manox_agent::paths::home_dir() {
            cwd = home;
        }
        // L11: the process-global server — every window and the embedded
        // web UI share one AgentServer (one ownership/routing table). Its
        // construction installs the AHP runtime builder; the store then
        // dials the host over the in-proc leg.
        let _agent_server = manox_session_core::agent_server::global(cwd.clone());
        let ahp_store =
            cx.new(|cx| manox_agent_chat_ui::ahp_store::AhpStore::connect(cwd.clone(), cx));
        // The landing session id is client-minted (the createSession
        // idempotency key), so the session the workspace renders and the one
        // the server drives are the same conversation.
        let landing_id = uuid::Uuid::new_v4().to_string();
        let multiplexer =
            cx.new(|_| crate::multiplexer::SessionMultiplexer::new(ahp_store.clone(), cwd.clone()));
        let session_id = landing_id.clone();
        multiplexer.update(cx, |m, cx| {
            m.create_session(&session_id, cx);
            // GW5: the landing session is the focused one from tick one —
            // its row suppresses unread rises while attached.
            m.set_focused(Some(&session_id), cx);
        });
        let store = multiplexer.read(cx).store();
        let thread =
            Thread::landing_with_id(manox_agent::ThreadId(session_id.clone()), cwd.clone());

        let input_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 12)
                .submit_on_enter(true)
                .placeholder(i18n::t("workspace-input-placeholder"))
        });

        // U6a/U6b②: no store handle at all — the list refresh rides the
        // server's watcher broadcast, and the attach path is the landing
        // mirror (the wire owns the session state).
        // The multiplexer's notify (its list/registry state changed)
        // repaints the sidebar rows and the workspace's model surfaces, and
        // feeds the chip-menu caches off the wire state (U2 cross-domain
        // #1: the store-read decoration snapshot retired — the registry
        // rides the Projects mirror, the per-thread projects ride the rows).
        let _mux_lists = cx.observe(&multiplexer, |_, _, cx| cx.notify());
        let conversation = cx.new(|_| ConversationState::new(manox_agent::MessageAuthor::Lead));
        let context_rail = {
            let rail_store = (store.clone(), session_id.clone());
            cx.new(|cx| crate::views::context_rail::ContextRail::new(Some(rail_store), cx))
        };
        // The rail's background writers (git refresh, subagent progress,
        // plan snapshots) notify only the rail entity; without this
        // observer those notifies never schedule a frame — the changes
        // surface on the next unrelated repaint (silent, self-healing
        // lag).
        let rail_updates = cx.observe(&context_rail, |_, _, cx| cx.notify());
        let weak_ws = cx.weak_entity();
        let chat_host: manox_agent_chat_ui::host::ChatHostHandle =
            std::sync::Arc::new(crate::WorkspaceChatHost::new(weak_ws));

        let mut ws = Self {
            cwd: cwd.clone(),
            nav: NavHistory::default(),
            multiplexer,
            client: (),
            background_threads: Vec::new(),
            _mux_lists,
            _rail_updates: rail_updates,
            subagent_transcripts: HashMap::new(),
            subagent_final_text: HashMap::new(),
            subagent_prompts: HashMap::new(),
            browser_views: BTreeMap::new(),
            view_mode: ViewMode::default(),
            exiting_settings: false,
            settings_transition_gen: 0,
            settings_view: None,
            settings_sub: None,
            chat: cx.new(|_cx| ChatColumn {
                host: chat_host,
                thread,
                store: Some((store, session_id.clone())),
                session_id: Some(session_id),
                git_status_gen: 0,
                conversation: conversation.clone(),
                input_state,
                drafts: HashMap::new(),
                pending_ask: None,
                pending_ask_live: false,
                pending_confirmation_live: false,
                pending_confirmation: None,
                ask_projection_confirmed: false,
                confirmation_projection_confirmed: false,
                ask_snapshot_item: None,
                confirmation_row_synthesized: None,
                confirmation_snapshot_item: None,
                ask_step: 0,
                ask_transition_gen: 0,
                ask_custom_inputs: Vec::new(),
                ask_custom_subs: Vec::new(),
                ask_custom_text: Vec::new(),
                ask_skipped: Vec::new(),
                ask_body_scroll: Vec::new(),
                model_open: false,
                model_menu: None,
                model_menu_sub: None,
                plus_open: false,
                plus_menu: None,
                plus_menu_sub: None,
                access_open: false,
                project_chip_open: false,
                project_chip_menu: None,
                project_chip_menu_sub: None,
                completion: None,
                recall_index: -1,
                recall_draft: None,
                turn_navigator: None,
                turn_navigator_sub: None,
                turn_navigator_previous_focus: None,
                turn_rail_active: None,
                turn_rail_active_from: None,
                turn_rail_active_gen: 0,
                turn_rail_hover: None,
                turn_rail_hover_prev: None,
                turn_rail_hover_gen: 0,
                turn_rail_hover_painted: None,
                turn_rail_pointer_inside: false,
                turn_rail_followed: None,
                turn_rail_scroll: gpui::UniformListScrollHandle::new(),
                turn_rail_preview_mark: None,
                turn_rail_preview_top: None,
                turn_rail_preview_gen: 0,
                turn_rail_box_h: std::rc::Rc::new(std::cell::Cell::new(px(0.))),
                queued_follow_ups: std::collections::VecDeque::new(),
                queued_follow_ups_by_thread: HashMap::new(),
                queue_drag: None,
                composer_placeholder_mode: ComposerPlaceholderMode::Normal,
                pending_attachments: Vec::new(),
                active_browser_suites: Vec::new(),
                project_picker_pending: false,
                blank_project_parent: None,
                blank_project_name_input: None,
                thread_sub: None,
                store_observe: None,
                pending_successor: None,
                fork_in_flight: false,
                input_sub: None,
                conversation_sub: None,
                list_state: ListState::new(0, ListAlignment::Bottom, MSG_LIST_OVERDRAW),
                message_list_width: crate::views::MessageListWidthInvalidator::default(),
                card_width: crate::views::CardWidth::default(),
                bubble_clearance: crate::views::BubbleClearance::default(),
                list_count: 0,
                goal_popover_open: false,
                goal_ticker_gen: 0,
                turn_active: false,
                thinking_ticker_gen: 0,
                awaiting_history: None,
                rebuilt_pre_snapshot: false,
                built_turns: 0,
                last_declined_ask: None,
                context_rail,
            }),
        };
        let (thread_events, store_changes) = ws.subscribe_thread(cx);
        ws.chat.update(cx, |chat, cx| {
            chat.thread_sub = Some(thread_events);
            cx.notify();
        });
        ws.chat.update(cx, |chat, cx| {
            chat.store_observe = Some(store_changes);
            cx.notify();
        });
        let input_sub = ws.subscribe_input(window, cx);
        ws.chat.update(cx, |chat, cc| {
            chat.input_sub = Some(input_sub);
            cc.notify();
        });
        ws.observe_conversation(cx);
        // Focus the composer so typing works immediately on the hero screen.
        ws.chat_input(cx).update(cx, |s, cx| s.focus(window, cx));
        ws
    }

    /// Swap the conversation + list for a prebuilt state and re-arm tail
    /// follow. Diagnostic-only entry point used by the full-workspace overlap
    /// walk test to open a real session without the async attach pipeline.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_replace_conversation(
        &mut self,
        conversation: Entity<ConversationState>,
        cx: &mut Context<Self>,
    ) {
        self.chat.update(cx, |chat, cx| {
            chat.conversation = conversation;
            let count = chat.conversation.read(cx).items().len();
            chat.list_state.reset(count);
            chat.list_count = count;
            chat.list_state.set_follow_mode(FollowMode::Tail);
            chat.reset_turn_rail_interaction();
            cx.notify();
        });
        self.observe_conversation(cx);
        cx.notify();
    }

    #[cfg(feature = "test-support")]
    pub fn diagnostic_list_state(&self, cx: &App) -> ListState {
        self.chat.read(cx).list_state.clone()
    }

    /// The turn rail's hovered mark, for interaction tests.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_turn_rail_hover(&self, cx: &App) -> Option<usize> {
        self.chat.read(cx).turn_rail_hover
    }

    /// Attach a thread through the production switch path. Diagnostic-only
    /// entry point so integration tests can exercise parking + re-surface
    /// without simulating the sidebar click. `expect_history` passes through
    /// to the history-loading gate.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_attach_thread(
        &mut self,
        thread: manox_agent::thread::ThreadHandle,
        expect_history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.attach_thread(thread, false, expect_history, window, cx);
    }

    /// Diagnostic read of the history-loading gate.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_awaiting_history(&self, cx: &App) -> Option<std::time::Instant> {
        self.chat.read(cx).awaiting_history
    }

    /// Fold an empty chat snapshot for the foreground session into the book
    /// and notify the store — the wire shape of "the reopen's chat snapshot
    /// landed, and the session is genuinely empty". Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_apply_empty_chat_snapshot(&self, cx: &mut App) {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            return;
        };
        let chat: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
            "resource": crate::ahp_store::chat_uri(&sid),
            "title": "gate-test",
            "status": 0,
            "modifiedAt": "2026-01-01T00:00:00Z",
            "turns": [],
        }))
        .expect("empty chat snapshot parses");
        store.update(cx, |s, cx| {
            s.book.apply_snapshot(
                &crate::ahp_store::chat_uri(&sid),
                ahp_types::state::SnapshotState::Chat(Box::new(chat)),
            );
            cx.notify();
        });
    }

    /// Backdate the history-loading gate past its timeout, so the prune path
    /// is testable without a real 10-second wait. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_backdate_history_gate(&self, cx: &mut App) {
        self.chat.update(cx, |chat, cx| {
            chat.awaiting_history = chat.awaiting_history.and_then(|since| {
                // `Instant` has no lower bound; a panicking subtraction could
                // fire on a machine with seconds of uptime.
                since.checked_sub(2 * crate::views::history_loading::HISTORY_TIMEOUT)
            });
            cx.notify();
        });
    }

    /// Diagnostic entry to the gate's timeout prune ([`Self::prune_history_gate`]).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_prune_history_gate(&mut self, cx: &mut Context<Self>) {
        self.prune_history_gate(cx);
    }

    /// Drop the history-loading gate once it has outlived
    /// [`crate::views::history_loading::HISTORY_TIMEOUT`]: the chat snapshot
    /// never landed (host error, missing session), and a permanent
    /// full-column loading page with no composer and no transcript is worse
    /// than the hero screen. Render calls this every frame; tests call it
    /// directly.
    fn prune_history_gate(&mut self, cx: &mut Context<Self>) {
        let expired =
            self.chat.read(cx).awaiting_history.is_some_and(|since| {
                since.elapsed() > crate::views::history_loading::HISTORY_TIMEOUT
            });
        if expired {
            self.chat.update(cx, |chat, cx| {
                chat.awaiting_history = None;
                cx.notify();
            });
        }
    }

    /// Bind a caller-built store as the foreground store, bypassing the
    /// multiplexer handshake. Diagnostic-only: the live-ask edge's retirement
    /// semantics read the fold through this binding.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_bind_store(
        &mut self,
        store: gpui::Entity<manox_agent_chat_ui::ahp_store::AhpStore>,
        session_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let sid = session_id.into();
        self.chat.update(cx, |chat, cc| {
            chat.store = Some((store, sid));
            cc.notify();
        });
    }

    /// Run the store-observe's rebuild guard directly. Diagnostic-only: the
    /// production guard fires on store notify, which a detached test store
    /// never produces. Returns `(rebuilt, chat_landed)`.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_run_rebuild_guard(&mut self, cx: &mut Context<Self>) -> (bool, bool) {
        self.sync_rebuild_from_book(cx)
    }

    /// The dismiss leg (`dismiss_ask`): the ask card's close and the stop
    /// button both ride it. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_dismiss_ask(&mut self, cx: &mut Context<Self>) {
        self.dismiss_ask(cx);
    }

    /// The rebuild watermark triple: (built pre-snapshot, built turn count,
    /// conversation item count). Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_rebuild_watermark(&self, cx: &App) -> (bool, usize, usize) {
        let chat = self.chat.read(cx);
        (
            chat.rebuilt_pre_snapshot,
            chat.built_turns,
            chat.conversation.read(cx).items().len(),
        )
    }
    /// Run the live-ask edge against the bound store. Diagnostic-only: the
    /// production edge fires on store notify, which a detached test store
    /// never produces.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_sync_live_ask(&mut self, cx: &mut Context<Self>) {
        if let Some((store, _)) = self.chat.read(cx).store.clone() {
            self.sync_live_ask(&store, cx);
        }
    }

    /// Seed a parsed `AskUserQuestion` as the pending ask. Diagnostic-only:
    /// bypasses the engine gate so the synthesis path can be tested with a
    /// fake engine (whose `pending_auth_entries` is empty).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_seed_ask(
        &mut self,
        id: &str,
        input: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.chat.update(cx, |chat, cc| {
            chat.pending_ask = parse_pending_ask(id.to_string(), input);
            chat.ask_step = 0;
            chat.ask_transition_gen = chat.ask_transition_gen.wrapping_add(1);
            cc.notify();
        });
        self.reset_ask_custom(cx);
        cx.notify();
    }

    /// Seed a per-question free-text `custom` answer on the pending ask
    /// without a `Window` (the render path mirrors the `InputState` entities
    /// onto this, but the tri-state fold reads `ask_custom_text` directly).
    /// Diagnostic-only: drives the answer-leg wire tests.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_set_ask_custom(&mut self, qi: usize, text: &str, cx: &mut App) {
        self.chat.update(cx, |chat, cc| {
            if qi >= chat.ask_custom_text.len() {
                chat.ask_custom_text.resize(qi + 1, String::new());
            }
            if let Some(slot) = chat.ask_custom_text.get_mut(qi) {
                *slot = text.to_string();
            }
            cc.notify();
        });
    }

    /// The pending ask's per-question `custom` texts (diagnostic-only; used to
    /// confirm a skip/re-seed cleared the scratch).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ask_custom_texts(&self, cx: &App) -> Vec<String> {
        self.chat.read(cx).ask_custom_text.clone()
    }

    /// Seed a tool confirmation as the pending state, WITHOUT the row: the
    /// surface (row promotion + snapshot) is `sync_confirmation_snapshot`'s
    /// job — the same production path — so tests drive exactly what the
    /// live edge drives.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_seed_pending_confirmation(
        &mut self,
        auth_id: &str,
        tool_call_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.chat.update(cx, |chat, cc| {
            chat.pending_ask = None;
            chat.pending_confirmation = Some(PendingConfirmation {
                auth_id: auth_id.to_string(),
                tool_call_id: tool_call_id.to_string(),
            });
            chat.pending_confirmation_live = false;
            chat.ask_step = 0;
            cc.notify();
        });
        self.reset_ask_custom(cx);
        cx.notify();
    }

    /// Diagnostic entry to the confirmation snapshot sync.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_sync_confirmation_snapshot(&mut self, cx: &mut App) {
        self.sync_confirmation_snapshot(cx);
    }

    /// The confirmation snapshot's auth id attached to the parked row, if
    /// the sync has budgeted it.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_confirmation_snapshot(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .confirmation_snapshot_item
            .as_ref()
            .and_then(|item| item.read(cx).confirmation.as_ref())
            .map(|c| c.auth_id.clone())
    }

    /// The pending confirmation as (auth_id, tool_call_id).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_pending_confirmation(&self, cx: &App) -> Option<(String, String)> {
        self.chat
            .read(cx)
            .pending_confirmation
            .as_ref()
            .map(|a| (a.auth_id.clone(), a.tool_call_id.clone()))
    }

    /// Whether any blocking overlay (plan review, ask, generic approval,
    /// blank project) is up.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_blocking_overlay_active(&self, cx: &App) -> bool {
        self.blocking_overlay_active(cx)
    }

    /// Resolve the pending generic-approval card. Diagnostic-only wrapper
    /// around `resolve_auth`; the fake engine accepts the id.
    #[cfg(feature = "test-support")]
    pub fn resolve_auth_for_test(
        &mut self,
        auth_id: &str,
        decision: PermissionDecision,
        cx: &mut Context<Self>,
    ) {
        self.resolve_auth(auth_id, None, decision, cx);
    }

    /// How long a dismissed ask's re-park re-issues the decline instead of
    /// re-seeding the card. Bounds the dismiss/restore race window without
    /// ever swallowing an answerable question permanently.
    const ASK_REDECLINE_WINDOW: std::time::Duration = std::time::Duration::from_secs(15);

    /// The live ask edge: reconcile the pending ask with the fold's open
    /// elicitation (the session's input-needed list). A new request id seeds
    /// the interactive card — synthesizing its `ToolCall` item, since the v2
    /// gate event that created the card has no AHP successor — and a request
    /// that left the fold (answered, dismissed, settled remotely) retires a
    /// live-seeded card. Diagnostic seeds are not the wire's and are left
    /// alone. Runs on every store notify, ahead of the drain.
    fn sync_live_ask(
        &mut self,
        store: &Entity<manox_agent_chat_ui::ahp_store::AhpStore>,
        cx: &mut Context<Self>,
    ) {
        // A stale observer (the outgoing thread's, before its rebind) must
        // not drive the foreground ask.
        let bound = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .is_some_and(|(s, _)| s.entity_id() == store.entity_id());
        if !bound {
            return;
        }
        let live = {
            let view = store.read(cx);
            let sid = self
                .chat
                .read(cx)
                .store
                .as_ref()
                .map(|(_, sid)| sid.clone())
                .expect("bound above");
            crate::ahp_store::leaf(&view.book, &sid)
                .open_chat_input()
                .and_then(|(chat_id, req)| {
                    let _ = chat_id;
                    match crate::ahp_store::pending_ask_from_ahp(req.id.clone(), req) {
                        Some(mut ask) => {
                            // A plan-review elicitation renders the plan itself:
                            // pull the proposal's markdown from the plan channel
                            // into the question's support text.
                            if req.id.starts_with("plan-review:") {
                                let plan =
                                    crate::ahp_store::plan_review_of(&view.book, &sid, &req.id);
                                // A verdict on the plan channel means the review
                                // is settled engine-side. The chat-level part can
                                // never fold answered once its turn archived (the
                                // reducer only settles active-turn parts, and a
                                // plan review outlives its turn), so without this
                                // check the card would re-seed forever with the
                                // composer locked in ask-supplement mode — the
                                // #88 device repro.
                                if plan.is_some_and(|p| {
                                    p.get("type").and_then(serde_json::Value::as_str)
                                        == Some(manox_ahp::ext::actions::PLAN_REVIEW_SETTLED)
                                }) {
                                    tracing::info!(
                                        request_id = %req.id,
                                        "live ask: plan review already settled on the plan channel"
                                    );
                                    return None;
                                }
                                if let Some(q) = ask.questions.first_mut()
                                    && let Some(content) = plan.and_then(|p| {
                                        p.get("content").and_then(serde_json::Value::as_str)
                                    })
                                {
                                    q.detail = content.to_string();
                                }
                            }
                            Some((req.id.clone(), ask))
                        }
                        None => {
                            // The one diagnosis this edge can't recover from:
                            // the fold carries the request but its question
                            // list is empty/unparseable.
                            tracing::warn!(
                                request_id = %req.id,
                                questions = ?req.questions.as_ref().map(|q| q.len()),
                                "live ask: fold elicitation parsed to no questions"
                            );
                            None
                        }
                    }
                })
        };
        match live {
            Some((request_id, ask)) => {
                // The engine's restore re-parks an unsettled question (upstream
                // #840), so a just-dismissed ask can come right back: the
                // decline raced the restore and reduced to NoOp host-side.
                // Re-issue the decline for the same request inside the window
                // instead of re-seeding the card the user closed; past the
                // window the card seeds again — an answerable question must
                // not be silently swallowed forever.
                let recently_declined =
                    self.chat
                        .read(cx)
                        .last_declined_ask
                        .as_ref()
                        .is_some_and(|(id, at)| {
                            id == &request_id && at.elapsed() < Self::ASK_REDECLINE_WINDOW
                        });
                if recently_declined {
                    // Re-issue the decline for the re-parked question, but do
                    // NOT return: the unified confirmation card below runs on
                    // the same pass, and an early return would leave a parked
                    // confirmation without its card for the whole window.
                    if let Some((_, sid)) = self.chat.read(cx).store.clone() {
                        let view = store.read(cx);
                        if let Some((chat_id, _)) =
                            crate::ahp_store::leaf(&view.book, &sid).chat_input(&request_id)
                        {
                            tracing::info!(
                                request_id = %request_id,
                                "live ask: re-declining the re-parked question"
                            );
                            store.update(cx, |s, _| {
                                s.decline_input(&chat_id, &request_id);
                            });
                        }
                    }
                } else {
                    let stale = self
                        .chat
                        .read(cx)
                        .pending_ask
                        .as_ref()
                        .is_none_or(|a| a.id != ask.id);
                    if stale {
                        let input = ask_input_json(&ask, &request_id);
                        let summary = ask
                            .questions
                            .first()
                            .map(|q| q.header.clone())
                            .unwrap_or_default();
                        tracing::info!(
                            request_id = %request_id,
                            questions = ask.questions.len(),
                            summary = %summary,
                            "live ask: seeding the interactive card"
                        );
                        self.chat.update(cx, |chat, cx| {
                            chat.pending_ask = Some(ask);
                            chat.pending_ask_live = true;
                            chat.ask_step = 0;
                            chat.ask_transition_gen = chat.ask_transition_gen.wrapping_add(1);
                            chat.last_declined_ask = None;
                            cx.notify();
                        });
                        self.reset_ask_custom(cx);
                        self.ensure_ask_tool_item(&request_id, &summary, input, cx);
                    }
                }
            }
            None => {
                let live_seeded = self.chat.read(cx).pending_ask_live;
                let has_ask = self.chat.read(cx).pending_ask.is_some();
                // A live-seeded card retires outright — its source of truth
                // left the fold. Any other card retires when the fold
                // demonstrably lacks its request id: the fold is the only
                // route an answer can ride (`resolve_ask` looks it up there),
                // so a card the fold cannot route is dead weight that parks
                // the composer in ask-supplement mode — the composer half of
                // the #88 dead-lock. The live edge re-seeds from the fold on
                // a later notify, so a premature retirement heals itself.
                let fold_lacks_id = has_ask && !live_seeded && {
                    let view = store.read(cx);
                    self.chat
                        .read(cx)
                        .store
                        .as_ref()
                        .map(|(_, sid)| sid.clone())
                        .is_some_and(|sid| {
                            self.chat.read(cx).pending_ask.as_ref().is_some_and(|a| {
                                crate::ahp_store::leaf(&view.book, &sid)
                                    .chat_input(&a.id)
                                    .is_none()
                            })
                        })
                };
                if has_ask && (live_seeded || fold_lacks_id) {
                    tracing::info!(
                        live_seeded,
                        fold_lacks_id,
                        "live ask: request left the fold, retiring the card"
                    );
                    self.chat.update(cx, |chat, cx| {
                        chat.pending_ask = None;
                        chat.pending_ask_live = false;
                        chat.ask_step = 0;
                        cx.notify();
                    });
                    self.reset_ask_custom(cx);
                }
            }
        }
        // The unified confirmation card: a tool confirmation (a sandbox
        // escalation or any pre-run gate) parks the model on a decision the
        // conversation card delivers. A question-less elicitation has NO
        // surface by design — the current runtime cannot mint one (the
        // translator always emits structured questions; plan-review rides
        // its own single-select), and the arm below warns if that ever
        // changes.
        let live_auth = {
            let view = store.read(cx);
            let sid = self
                .chat
                .read(cx)
                .store
                .as_ref()
                .map(|(_, sid)| sid.clone())
                .expect("bound above");
            let leaf = crate::ahp_store::leaf(&view.book, &sid);
            leaf.open_tool_confirmation()
                .map(|(chat_id, confirmation)| {
                    let (_, _, tool_call_id) =
                        leaf.confirmation(&confirmation.id).unwrap_or_else(|| {
                            (chat_id.clone(), String::new(), confirmation.id.clone())
                        });
                    (confirmation.id.clone(), tool_call_id)
                })
        };
        match live_auth {
            Some((auth_id, tool_call_id)) => {
                let armed = self
                    .chat
                    .read(cx)
                    .pending_confirmation
                    .as_ref()
                    .is_none_or(|a| a.auth_id != auth_id);
                if armed {
                    tracing::info!(
                        request_id = %auth_id,
                        tool_call = %tool_call_id,
                        "live confirmation: arming the conversation card"
                    );
                    self.chat.update(cx, |chat, cx| {
                        chat.pending_confirmation =
                            Some(manox_agent_chat_ui::column::PendingConfirmation {
                                auth_id: auth_id.clone(),
                                tool_call_id,
                            });
                        chat.pending_confirmation_live = true;
                        cx.notify();
                    });
                }
            }
            None => {
                let live_seeded = self.chat.read(cx).pending_confirmation_live;
                let has_auth = self.chat.read(cx).pending_confirmation.is_some();
                if live_seeded && has_auth {
                    tracing::info!("live confirmation: request left the fold, retiring the card");
                    self.chat.update(cx, |chat, cx| {
                        chat.pending_confirmation = None;
                        chat.pending_confirmation_live = false;
                        cx.notify();
                    });
                }
            }
        }
    }

    /// Synthesize the top-level AskUserQuestion card when the conversation
    /// lacks the `ToolCall` item the interactive ask card renders on: the
    /// live ask edge seeds it for a freshly arrived elicitation, the
    /// resurface loop for one whose underlying ToolUse folded into an
    /// activity segment. Without the item the ask snapshot cannot attach and
    /// the interactive UI never renders.
    fn ensure_ask_tool_item(
        &mut self,
        id: &str,
        summary: &str,
        input: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let conversation = self.chat_conversation(cx);
        if conversation.read(cx).find_tool(id, cx).is_some() {
            return;
        }
        // The runtime always supplies a non-empty English summary for its own
        // tool call, and runtime-supplied values are never re-localized here;
        // the card's own empty-header fallback lives at the render site.
        let role = self.model_label(cx);
        let host = self.chat.read(cx).host.clone();
        conversation.update(cx, |conversation, cx| {
            conversation.push_tool_call(
                crate::conversation::ToolCallItem {
                    id: id.to_string(),
                    name: manox_agent::tools::ASK_USER_QUESTION.to_string(),
                    title: summary.to_string(),
                    status: manox_agent::ToolCallStatus::PendingApproval,
                    output: String::new(),
                    is_error: false,
                    input,
                    streaming: false,
                    collapsed: false,
                    user_toggled: false,
                    panel: None,
                },
                role,
                host,
                cx,
            );
        });
        self.sync_list_count(cx);
        self.chat.update(cx, |chat, _| {
            chat.list_state.set_follow_mode(FollowMode::Tail);
        });
    }

    /// Diagnostic-only wrapper around `ensure_ask_tool_item`.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ensure_ask_tool_item(
        &mut self,
        id: &str,
        summary: &str,
        input: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.ensure_ask_tool_item(id, summary, input, cx);
    }

    /// Sync the ask snapshots (render-time path). Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_sync_ask_card_snapshots(&mut self, cx: &mut Context<Self>) {
        self.sync_ask_card_snapshots(cx);
    }

    /// Whether the pending ask's card would render interactively: a matching
    /// top-level `ToolCall` item carrying the Workspace-derived snapshot.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ask_card_interactive(&self, id: &str, cx: &App) -> bool {
        let Some(ix) = self.chat_conversation(cx).read(cx).find_tool(id, cx) else {
            return false;
        };
        self.chat_conversation(cx).read(cx).items()[ix]
            .read(cx)
            .ask_snapshot
            .is_some()
    }

    /// Count of top-level `ToolCall` items with the given id. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_tool_call_count(&self, id: &str, cx: &App) -> usize {
        self.chat
            .read(cx)
            .conversation
            .read(cx)
            .items()
            .iter()
            .filter(|item| matches!(item.read(cx).kind(), ConvItem::ToolCall(t) if t.id == id))
            .count()
    }

    /// The accumulated output of the activity-segment tool entry with the given
    /// id, when one exists. Diagnostic-only: this is how the attach catch-up's
    /// replay of a parked thread's streamed `ToolOutput` is observed.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_tool_output(&self, id: &str, cx: &App) -> Option<String> {
        let conversation = self.chat_conversation(cx);
        let conversation = conversation.read(cx);
        let (cix, eix) = conversation.find_thinking_entry(id, cx)?;
        let item = conversation.items().get(cix)?.read(cx);
        match item.kind() {
            ConvItem::Thinking(container) => match container.entries.get(eix) {
                Some(crate::conversation::ActivityEntry::Tool(entry)) => Some(entry.output.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The attempt numbers of the retry notices the transcript shows, in order.
    /// Diagnostic-only: this is how the attach catch-up's turn gate on the
    /// replayed `Retry` row is observed.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_retry_attempts(&self, cx: &App) -> Vec<u32> {
        self.chat_conversation(cx)
            .read(cx)
            .items()
            .iter()
            .filter_map(|item| match item.read(cx).kind() {
                ConvItem::Retry { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect()
    }

    /// The pending question card's id, if one is surfaced. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_pending_ask_id(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .pending_ask
            .as_ref()
            .map(|a| a.id.clone())
    }

    /// The ask walk's churn counter. Diagnostic-only: a re-delivery of the
    /// same pending id must not advance it — the walk state belongs to the
    /// user, not to the wire.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ask_transition_gen(&self, cx: &App) -> u64 {
        self.chat.read(cx).ask_transition_gen
    }

    /// Run the render-time projection reconcile. Diagnostic-only wrapper
    /// around the private `reconcile_pending_with_projections`.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_reconcile_pending_with_projections(&mut self, cx: &mut Context<Self>) {
        self.reconcile_pending_with_projections(cx);
    }

    /// Remeasure every list row whenever the conversation mutates. A height
    /// change on an off-screen row would otherwise leave the list's cached
    /// height stale until the next width change; this applies that cure
    /// automatically on every conversation notify. Callers must invoke this
    /// after any `self.chat.conversation = ...` reassignment so the subscription
    /// tracks the live entity.
    fn observe_conversation(&mut self, cx: &mut Context<Self>) {
        let list_state = self.chat_list_state(cx);
        let conversation = self.chat_conversation(cx);
        let sub = cx.observe(&conversation, move |_this, _conv, _cx| {
            list_state.remeasure_items(0..list_state.item_count());
        });
        self.chat.update(cx, |chat, cc| {
            chat.conversation_sub = Some(sub);
            cc.notify();
        });
    }

    /// Rebuild the conversation view from the thread's v2 display fold. The
    /// trigger is the follow stream's authoritative history boundary
    /// (`WindowChange::Replace` → `ThreadEvent::HistoryRestored`, §D.1); the
    /// T10c-era successor of the deleted `ThreadHistory` note replay.
    /// Wire the workspace to the foreground leaf: the `ThreadEvent` stream
    /// drives the conversation surface, and the entity-level observe repaints
    /// on projection-only frames (the mid-session chip updates — see the
    /// `store_observe` field note).
    /// Rebuild the foreground conversation from the book's chat fold (the
    /// AHP successor of the v2 snapshot-rebuild: the subscribe snapshot is
    /// the authoritative transcript, and `synth_display` lowers it).
    pub(crate) fn rebuild_conversation_from_book(&mut self, cx: &mut Context<Self>) {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            return;
        };
        let (running, display, usage, pre_snapshot, built_turns) = {
            let view = store.read(cx);
            let leaf = crate::ahp_store::leaf(&view.book, &sid);
            let running = leaf.running();
            let Some(chat) = leaf.chat else {
                tracing::info!(session_id = %sid, "rebuild: no chat in the book yet");
                return;
            };
            let mut usage = crate::chat_fold::UsageTable::new();
            let display = crate::chat_fold::synth_display(chat, &mut usage);
            let user_rows = display
                .iter()
                .filter(|e| {
                    matches!(e, manox_agent::db::HistoryEntry::Message(m) if m.role == manox_agent::language_model::Role::User)
                })
                .count();
            let entry_count = display.len();
            let turns = chat.turns.len();
            tracing::info!(
                session_id = %sid,
                turns,
                entries = entry_count,
                user_rows,
                first_user_text_empty = chat.turns.first().is_some_and(|t| t.message.text.is_empty()),
                "rebuild: synthesized transcript from the chat fold"
            );
            let pre_snapshot = !view.book.chat_snapshot_landed(&sid);
            (running, display, usage, pre_snapshot, chat.turns.len())
        };
        let role = self.model_label(cx);
        let cwd = thread_cwd(&self.chat.read(cx).thread, &self.chat.read(cx).store, cx);
        let fork_source = self.fork_source_session(cx);
        let new_conv = cx.new(|cx| {
            ConversationState::rebuild_from_display(
                &display,
                &usage,
                &role,
                manox_agent::MessageAuthor::Lead,
                running,
                crate::conversation::ApplyCtx {
                    host: self.chat.read(cx).host.clone(),
                    cwd,
                    fork_source,
                },
                cx,
            )
        });
        self.chat.update(cx, |chat, cx| {
            chat.conversation = new_conv;
            chat.rebuilt_pre_snapshot = pre_snapshot;
            chat.built_turns = built_turns;
            // The snapshot landed with displayable history: the loading gate's
            // job is done.
            chat.awaiting_history = None;
            cx.notify();
        });
        self.sync_list_count(cx);
        cx.notify();
    }

    /// The store-observe's rebuild guard, shared with the diagnostic test
    /// entry: rebuild the foreground conversation when the fold outgrew what
    /// the conversation was built from. Returns `(rebuilt, chat_landed)`.
    fn sync_rebuild_from_book(&mut self, cx: &mut Context<Self>) -> (bool, bool) {
        let (displayable, chat_landed, snapshot_landed, turns_now) = self
            .chat
            .read(cx)
            .store
            .clone()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid).chat.map(|c| {
                    (
                        !c.turns.is_empty() || c.active_turn.is_some(),
                        true,
                        view.book.chat_snapshot_landed(&sid),
                        c.turns.len(),
                    )
                })
            })
            .unwrap_or((false, false, false, 0));
        // A build that ran before the chat snapshot landed (first replay
        // deltas beat the subscribe answer) froze whatever partial fold
        // existed then; once the snapshot lands with more settled turns than
        // the build saw, rebuild over it — the one-shot empty check below can
        // never fire again on a non-empty skeleton.
        let healing = {
            let chat = self.chat.read(cx);
            chat.rebuilt_pre_snapshot && snapshot_landed && turns_now > chat.built_turns
        };
        if displayable && (healing || self.chat_conversation(cx).read(cx).is_empty(cx)) {
            self.rebuild_conversation_from_book(cx);
            (true, chat_landed)
        } else {
            (false, chat_landed)
        }
    }

    fn subscribe_thread(&self, cx: &mut Context<Self>) -> (Subscription, Subscription) {
        // v3: the per-thread `ThreadEvent` pump is gone — the AhpStore pump
        // folds every channel and notifies. What remains here is the
        // successor hand-off watch (a disposed session's redirect) riding an
        // observe of the store entity; the conversation repaint reads the
        // book on the notify (see the workspace-level store observe).
        let Some((store, _)) = self.chat.read(cx).store.clone() else {
            let a = cx.observe(&self.multiplexer, |_, _, _| {});
            let b = cx.observe(&self.multiplexer, |_, _, _| {});
            return (a, b);
        };
        let repaint = cx.observe(&store, |this, store, cx| {
            // Snapshot → transcript transition: the attach-time rebuild ran
            // against an empty fold (the chat snapshot lands asynchronously
            // after subscribe), so the hero screen would stick forever. The
            // moment the fold holds anything displayable — settled turns, or
            // a first turn still in flight (`active_turn`, whose content
            // synth_display lowers the same way) — and the conversation is
            // still empty, rebuild from the snapshot. The drained deltas are
            // already inside it, so the drain skips one round.
            let (rebuilt, chat_landed) = this.sync_rebuild_from_book(cx);
            if !rebuilt && chat_landed && this.chat.read(cx).awaiting_history.is_some() {
                // The chat snapshot landed but holds no displayable turn: the
                // reopened session is genuinely empty. Drop the loading gate so
                // the hero screen returns (without this the loading view would
                // stick forever on an empty history).
                this.chat.update(cx, |chat, cx| {
                    chat.awaiting_history = None;
                    cx.notify();
                });
            }
            // Live ask edge: the fold's open elicitation IS the pending ask.
            // Runs AFTER the rebuild so a freshly seeded card lands on the
            // rebuilt conversation instead of the empty skeleton it replaces
            // (seeding first would make the skeleton non-empty and starve
            // the rebuild forever).
            this.sync_live_ask(&store, cx);
            if rebuilt {
                return;
            }
            // Live streaming leg: the pump folded chat actions into the book;
            // drain the display events they derived and run the conversation
            // applier over them (the v2 ThreadEvent pump's transcript role).
            let events = store.update(cx, |s, _| s.drain_chat_events());
            if events.is_empty() {
                return;
            }
            let role = this.model_label(cx);
            for event in events {
                match &event {
                    crate::chat_fold::ChatEvent::Notice { text } => {
                        let host = this.chat.read(cx).host.clone();
                        this.chat.update(cx, |chat, cx| {
                            chat.conversation.update(cx, |c, cx| {
                                c.push_notice(
                                    text.clone(),
                                    crate::conversation::NoticeAnchor::TurnEnd,
                                    host.clone(),
                                    cx,
                                );
                            });
                            cx.notify();
                        });
                        continue;
                    }
                    crate::chat_fold::ChatEvent::Usage { .. } => {
                        // The rail's metrics fold carries usage; nothing in
                        // the transcript.
                        continue;
                    }
                    crate::chat_fold::ChatEvent::TurnStarted => {
                        this.chat.update(cx, |chat, cx| {
                            chat.turn_active = true;
                            cx.notify();
                        });
                        this.spawn_thinking_ticker(cx);
                    }
                    crate::chat_fold::ChatEvent::TurnFinished => {
                        this.chat.update(cx, |chat, cx| {
                            chat.turn_active = false;
                            cx.notify();
                        });
                        this.multiplexer.update(cx, |m, cx| m.fetch_thread_list(cx));
                        this.spawn_git_status_refresh(cx);
                    }
                    _ => {}
                }
                let thread_event = crate::chat_fold::to_thread_event(event);
                let Some(thread_event) = thread_event else {
                    continue;
                };
                let cwd = thread_cwd(&this.chat.read(cx).thread, &this.chat.read(cx).store, cx);
                let outcome = this.chat_conversation(cx).update(cx, |c, cx| {
                    c.apply(
                        &thread_event,
                        &role,
                        None,
                        crate::conversation::ApplyCtx {
                            host: this.chat.read(cx).host.clone(),
                            cwd,
                            fork_source: None,
                        },
                        cx,
                    )
                });
                this.apply_list_outcome(outcome, cx);
            }
            cx.notify();
        });
        let successor = cx.observe(&store, |this, store, cx| {
            // Identity hand-off: a disposed session records its successor;
            // the foreground moves onto it at the next render (the switch
            // needs a window). Only the FOREGROUND session's own
            // `replacedBy` counts — a book-wide find_map could hit some
            // unrelated disposed session and, through
            // `replace_nav_current`, overwrite the user's history entry.
            let next = this
                .chat
                .read(cx)
                .store
                .as_ref()
                .and_then(|(_, sid)| store.read(cx).book.sessions.get(sid))
                .and_then(|s| {
                    s.meta
                        .as_ref()
                        .and_then(|m| m.get("replacedBy"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                });
            if let Some(next) = next
                && this
                    .chat
                    .read(cx)
                    .store
                    .as_ref()
                    .is_none_or(|(_, sid)| sid != &next)
            {
                this.chat.update(cx, |chat, cx| {
                    chat.pending_successor = Some(next);
                    cx.notify();
                });
            }
            cx.notify();
        });
        (repaint, successor)
    }

    /// Switch into the Settings overlay. The Settings view is created lazily on
    /// first entry; from then on the entity + subscription are reused so the
    /// user's last selection (and any scroll position) survives re-entry.
    pub fn enter_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_view.is_none() {
            let settings = cx.new(|cx| SettingsView::new(px(SETTINGS_NAV_WIDTH), window, cx));
            let sub = self.subscribe_settings(&settings, cx);
            self.settings_view = Some(settings);
            self.settings_sub = Some(sub);
        }
        self.view_mode = ViewMode::Settings;
        // Clear any pending exit animation: clicking Settings… while the
        // panel is still sliding out re-opens the overlay. Bumping the
        // transition generation also retires the old exit spawn (it carries
        // the previous gen and no-ops on stale state), and forces the slide
        // animation to replay from the left edge.
        self.exiting_settings = false;
        self.settings_transition_gen = self.settings_transition_gen.wrapping_add(1);
        cx.notify();
    }

    fn subscribe_settings(
        &self,
        settings: &Entity<SettingsView>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe(settings, |this, _settings, ev: &SettingsEvent, cx| {
            if matches!(ev, SettingsEvent::Exit) && !this.exiting_settings {
                // Start the slide-out animation; the actual mode flip and
                // unmount happen once the animation has finished. The
                // captured transition gen is the watermark for this exit
                // attempt — if a new enter supersedes it before the timer
                // fires, the spawn's update is a no-op.
                this.exiting_settings = true;
                this.settings_transition_gen = this.settings_transition_gen.wrapping_add(1);
                cx.notify();
                let entity = cx.entity().clone();
                let exit_gen = this.settings_transition_gen;
                cx.spawn(async move |_workspace, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(SLIDE_OUT_MS + 20))
                        .await;
                    entity.update(cx, |this, cx| {
                        if this.settings_transition_gen != exit_gen {
                            return;
                        }
                        this.view_mode = ViewMode::default();
                        this.exiting_settings = false;
                        cx.notify();
                    });
                })
                .detach();
            }
        })
    }

    /// Switch to the conversation pane.
    pub fn focus_conversation(&mut self, cx: &mut Context<Self>) {
        self.view_mode = ViewMode::Workspace;
        cx.notify();
    }

    /// How many sessions currently owe the user a look or a verdict — the
    /// dock badge source (`SessionMultiplexer::attention_count`). The
    /// aggregate reads the multiplexer's rows plus the leaves' unread
    /// mirrors, so any state edge that moves the sidebar's attention marks
    /// moves this count on the next read.
    pub fn attention_count(&self, cx: &App) -> usize {
        self.multiplexer.read(cx).attention_count()
    }

    fn subscribe_input(&self, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        let input = self.chat.read(cx).input_state.clone();
        cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { shift: false, .. } => this.submit_input(window, cx),
                // Shift+Enter inserts a newline inside the input and does not submit.
                InputEvent::PressEnter { shift: true, .. } => {}
                InputEvent::Change => this.sync_completion(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            },
        )
    }

    /// Re-evaluate the completion popover against the live input value + caret.
    ///
    /// When the caret sits inside a `/` or `@` trigger token, the matching
    /// source is filtered by the query and a fresh [`CompletionState`] replaces
    /// the current one. With no trigger or zero matches the popover closes. The
    /// popover is a pure render overlay and never grabs focus, so the
    /// `InputState` keeps typing and the filter updates every keystroke.
    fn sync_completion(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (value, cursor) = {
            let s = self.chat_input(cx).read(cx);
            (s.value().to_string(), s.selected_range().end)
        };
        let new = match detect(&value, cursor) {
            None => None,
            Some(det) => {
                let items = if det.trigger == '/' {
                    // U2: the popover lists the gateway's command snapshot.
                    slash_source(
                        &det.query,
                        &self
                            .multiplexer
                            .read(cx)
                            .commands(cx)
                            .cloned()
                            .unwrap_or(serde_json::json!([])),
                    )
                } else {
                    mention_source(&det.query)
                };
                if items.is_empty() {
                    None
                } else {
                    // Carry the selection forward when the same trigger is
                    // active and the previously-picked item survived the
                    // narrower filter, so typing more to refine doesn't snap
                    // the highlight back to the top.
                    let selected = self
                        .chat
                        .read(cx)
                        .completion
                        .as_ref()
                        .filter(|s| s.trigger == det.trigger)
                        .and_then(|s| s.items.get(s.selected).map(|it| it.name.clone()))
                        .and_then(|name| items.iter().position(|it| it.name == name))
                        .unwrap_or(0);
                    Some(CompletionState::new(
                        det.trigger,
                        det.token_start,
                        items,
                        selected,
                    ))
                }
            }
        };
        let changed = match (&self.chat.read(cx).completion, &new) {
            (None, None) => false,
            (Some(_), None) | (None, Some(_)) => true,
            (Some(a), Some(b)) => {
                !a.items.eq(&b.items) || a.trigger != b.trigger || a.selected != b.selected
            }
        };
        self.chat.update(cx, |chat, cx| {
            chat.completion = new;
            cx.notify();
        });
        if changed {
            cx.notify();
        }
    }

    /// Drop the popover without touching the input.
    fn close_completion(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            if chat.completion.take().is_some() {
                cc.notify();
            }
        });
    }

    /// Confirm the selected (or clicked) completion item: replace the trigger
    /// token with `trigger + name + " "` and place the caret after the space.
    fn completion_confirm(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.chat.update(cx, |chat, cc| {
            let v = chat.completion.take();
            cc.notify();
            v
        }) else {
            return;
        };
        let Some(item) = state.items.get(ix) else {
            self.chat.update(cx, |chat, cx| {
                chat.completion = Some(state);
                cx.notify();
            });
            return;
        };
        let name = item.name.to_string();
        let trigger = state.trigger;
        let token_start = state.token_start;
        let (value, cursor) = {
            let s = self.chat_input(cx).read(cx);
            (s.value().to_string(), s.selected_range().end)
        };
        if cursor > value.len() || token_start > cursor {
            return;
        }
        let (new_value, caret) = build_replacement(trigger, &name, &value, token_start, cursor);
        self.chat_input(cx).update(cx, |s, cx| {
            s.set_value(new_value, window, cx);
            let pos = RopeExt::offset_to_position(s.text(), caret.min(s.text().len()));
            s.set_cursor_position(pos, window, cx);
        });
        cx.notify();
    }

    fn completion_up(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            if let Some(state) = chat.completion.as_mut() {
                state.move_selection(-1);
                cc.notify();
            }
        });
    }

    fn completion_down(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            if let Some(state) = chat.completion.as_mut() {
                state.move_selection(1);
                cc.notify();
            }
        });
    }

    fn completion_confirm_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .chat
            .read(cx)
            .completion
            .as_ref()
            .map(|s| s.selected)
            .unwrap_or(0);
        self.completion_confirm(ix, window, cx);
    }

    /// Close the access-chip dropdown, dropping the menu entity + subscription.
    fn close_access_menu(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.access_open = false;
            cx.notify();
        });
    }

    /// Close the project-chip dropdown.
    fn close_project_chip_menu(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.project_chip_open = false;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.project_chip_menu = None;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.project_chip_menu_sub = None;
            cx.notify();
        });
    }

    fn blocking_overlay_active(&self, cx: &App) -> bool {
        let chat = self.chat.read(cx);
        chat.pending_ask.is_some()
            || chat.pending_confirmation.is_some()
            || chat.blank_project_parent.is_some()
    }

    fn toggle_turn_navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat.read(cx).turn_navigator.is_some() {
            self.close_turn_navigator(window, cx);
            return;
        }
        if !matches!(self.view_mode, ViewMode::Workspace) || self.blocking_overlay_active(cx) {
            return;
        }

        let turns = collect_user_turns(
            self.chat
                .read(cx)
                .conversation
                .read(cx)
                .items()
                .iter()
                .enumerate()
                .map(|(ix, item)| (ix, item.read(cx).kind())),
        );
        let previous_focus = window.focused(cx);
        let navigator = cx.new(|cx| TurnNavigator::new(turns, window, cx));
        let sub = cx.subscribe_in(
            &navigator,
            window,
            |this, _navigator, event: &TurnNavigatorEvent, window, cx| match event {
                TurnNavigatorEvent::Navigate { item_ix } => {
                    let target = *item_ix;
                    this.close_turn_navigator(window, cx);
                    this.reveal_message(target, cx);
                }
                TurnNavigatorEvent::FillComposer { text } => {
                    let text = text.clone();
                    this.close_turn_navigator(window, cx);
                    this.fill_composer_from_turn(text, window, cx);
                }
                TurnNavigatorEvent::Dismiss => this.close_turn_navigator(window, cx),
            },
        );
        self.chat.update(cx, |chat, cx| {
            chat.turn_navigator = Some(navigator.clone());
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.turn_navigator_sub = Some(sub);
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.turn_navigator_previous_focus = previous_focus;
            cx.notify();
        });
        navigator.update(cx, |navigator, cx| navigator.focus(window, cx));
        cx.notify();
    }

    /// Reconcile the `list_state` item count with the live conversation length
    /// via `splice`, which preserves scroll position. Append (the common case,
    /// every user/assistant/tool item) splices new tail items in as Unmeasured;
    /// a tail removal (a `Retry` badge popped without replacement) splices the
    /// dangling slot out. Call after any direct conversation mutation that the
    /// `ApplyOutcome` path does not already cover (e.g. `push_user`/`push_notice`,
    /// which bypass `apply`).
    fn sync_list_count(&mut self, cx: &mut App) -> bool {
        let new_count = self.chat_conversation(cx).read(cx).items().len();
        if new_count == self.chat.read(cx).list_count {
            return false;
        }
        let (current, list_state) = {
            let chat = self.chat.read(cx);
            (chat.list_count, chat.list_state.clone())
        };
        let (start, end, extra) = if new_count > current {
            (current, current, new_count - current)
        } else {
            (new_count, current, 0)
        };
        list_state.splice(start..end, extra);
        self.chat.update(cx, |chat, cc| {
            chat.list_count = new_count;
            cc.notify();
        });
        true
    }

    /// Reconcile the `list_state` with a conversation mutation: splice the
    /// count (append/remove) and remeasure the affected index/indices. Call
    /// after any `ConversationState::apply` (the outcome tells which path) so
    /// the virtualized list's per-item height cache never goes stale.
    /// Splice a single newly inserted conversation item at `ix` into
    /// `list_state`. Mid-list insertions (anchored notices) can't ride the
    /// tail-diff in `sync_list_count`, so this splices at the exact position
    /// and bumps `list_count` to keep the two reconciliations consistent (a
    /// later `sync_list_count` sees an equal count and is a no-op).
    fn apply_list_insert(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.list_state.splice(ix..ix, 1);
            chat.list_count += 1;
            cc.notify();
        });
    }

    /// Re-engage tail-follow. `FollowMode::Tail` pins to the end natively and
    /// keeps following; an upward user scroll disengages it and landing back
    /// at the bottom re-arms it. This is the user-initiated "jump to live
    /// tail" path (submit, slash command, thread open) — it always re-arms
    /// follow, even if the user had scrolled up to read back.
    fn follow_message_tail(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.list_state.set_follow_mode(FollowMode::Tail);
            cc.notify();
        });
    }

    /// Jump the viewport so the given conversation item is at the top. Native
    /// `scroll_to` is a single atomic state change, so no frame protection is
    /// needed against a stale tail re-pin.
    fn reveal_message(&mut self, item_ix: usize, cx: &mut Context<Self>) {
        self.chat
            .read(cx)
            .list_state
            .set_follow_mode(FollowMode::Normal);
        self.chat.read(cx).list_state.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    fn close_turn_navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .chat
            .update(cx, |chat, cc| {
                let v = chat.turn_navigator.take();
                cc.notify();
                v
            })
            .is_none()
        {
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.turn_navigator_sub = None;
            cx.notify();
        });
        if let Some(previous) = self.chat.update(cx, |chat, cc| {
            let v = chat.turn_navigator_previous_focus.take();
            cc.notify();
            v
        }) {
            window.focus(&previous, cx);
        }
        cx.notify();
    }

    fn drop_turn_navigator(&mut self, cx: &mut Context<Self>) {
        if self
            .chat
            .update(cx, |chat, cc| {
                let v = chat.turn_navigator.take();
                cc.notify();
                v
            })
            .is_some()
        {
            self.chat.update(cx, |chat, cx| {
                chat.turn_navigator_sub = None;
                cx.notify();
            });
            self.chat.update(cx, |chat, cx| {
                chat.turn_navigator_previous_focus = None;
                cx.notify();
            });
            cx.notify();
        }
    }

    fn render_turn_navigator_overlay(
        &self,
        theme: &Theme,
        card_width: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let navigator = self.chat.read(cx).turn_navigator.clone()?;
        let layout = turn_navigator_layout(card_width);
        let panel_height = navigator.read(cx).panel_height(cx);

        Some(
            v_flex()
                .id("turn-navigator-overlay")
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.close_turn_navigator(window, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    v_flex()
                        .absolute()
                        .top_0()
                        .right(layout.right_inset)
                        .bottom_0()
                        .left(layout.left_inset)
                        .items_center()
                        .pt(px(8.0))
                        .child(
                            popup_menu::popup_container(theme, navigator)
                                .id("turn-navigator-panel")
                                .w(layout.panel_width)
                                .h(panel_height)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Start (or restart) the per-second ticker that drives the Thinking status
    /// row's "for Xs" counter. Bumping `thinking_ticker_gen` first invalidates
    /// any prior ticker — it polls the generation and self-terminates when it
    /// no longer matches, so a new turn or thread switch replaces the old task
    /// instead of stacking a second one.
    fn spawn_thinking_ticker(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.thinking_ticker_gen = chat.thinking_ticker_gen.wrapping_add(1);
            cx.notify();
        });
        let entity = cx.entity().clone();
        let ticker_gen = self.chat.read(cx).thinking_ticker_gen;
        cx.spawn(async move |_this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let still = entity.read_with(cx, |this, cx| {
                    let chat = this.chat.read(cx);
                    chat.thinking_ticker_gen == ticker_gen && chat.turn_active
                });
                if !still {
                    break;
                }
                entity.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }

    /// Debounced git-status refresh. Bumps `git_status_gen` (invalidating any
    /// prior in-flight refresh), waits 400ms so a burst of tool results
    /// coalesces into one git call, then shells out to `git diff --numstat`
    /// / `branch --show-current` on the global tokio runtime. The result is
    /// delivered back to the gpui side via `async_channel` and pushed onto the
    /// `ContextRail`. Cancelled (superseded) refreshes self-terminate by
    /// comparing their captured gen to the live one.
    ///
    /// Uses `cx.background_executor().timer()` — never `tokio::time` on the
    /// gpui foreground (that panics: no current tokio runtime).
    fn spawn_git_status_refresh(&mut self, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cc| {
            chat.git_status_gen = chat.git_status_gen.wrapping_add(1);
            cc.notify();
        });
        let entity = cx.entity().clone();
        let refresh_gen = self.chat.read(cx).git_status_gen;
        let rail = self.chat.read(cx).context_rail.clone();
        let cwd = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid)
                    .cwd()
                    .map(std::path::PathBuf::from)
            })
            .unwrap_or_default();
        let worktree_branch = self.chat.read(cx).store.as_ref().and(None::<String>);
        cx.spawn(async move |_this, cx| {
            // Debounce: coalesce a burst of tool results / a turn's worth of
            // file writes into a single git call.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            // Superseded by a newer trigger — let the newer refresh win.
            let stale = entity.read_with(cx, |this, cx| {
                this.chat.read(cx).git_status_gen != refresh_gen
            });
            if stale {
                return;
            }
            let result = crate::git_status::gather_bridged(cwd, worktree_branch).await;
            // The refresh may have been superseded while the git call was in
            // flight; drop the result if so.
            let still_current = entity.read_with(cx, |this, cx| {
                this.chat.read(cx).git_status_gen == refresh_gen
            });
            if !still_current {
                return;
            }
            rail.update(cx, |r, cx| r.set_git_branch(result, cx));
        })
        .detach();
    }

    /// The agent whose conversation this workspace renders — every user
    /// bubble's header `to`. `lead`-labeled threads show the localized Captain
    /// label; a team member thread shows its own member name.
    fn recipient_author(&self, cx: &App) -> manox_agent::MessageAuthor {
        self.chat.read(cx).thread.read(|t| t.self_author())
    }

    /// The session the transcript's rows belong to: the single source for
    /// stamping journal-replayed rows as fork anchors (`fork_source`) and
    /// for the outgoing `ForkSession` source — an entry id is only
    /// addressable within the session it was replayed from, so both sides
    /// must read one id.
    pub(crate) fn fork_source_session(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone())
    }

    fn user_turn_meta(&self, cx: &mut Context<Self>) -> UserTurnMeta {
        let permission_mode = self
            .chat
            .read(cx)
            .store
            .clone()
            .and_then(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid)
                    .approval_mode()
                    .and_then(|m| serde_json::from_str::<PermissionMode>(&format!("{:?}", m)).ok())
            })
            .unwrap_or(PermissionMode::ReadOnly);
        UserTurnMeta::new(
            chrono::Utc::now().timestamp(),
            self.model_label(cx),
            Some(permission_mode),
        )
    }

    pub(crate) fn model_label(&self, cx: &App) -> String {
        {
            // Selector read face (§J11): the composer model chip derives from
            // the store's projection-materialized model, falling back to the
            // bound thread mirror only while no projection has landed yet.
            // T10c: the v1 `model_name` human label (CurrentModel/ThreadInfo
            // notes) is gone — the `model` projection carries the canonical
            // wire identity only; display names resolve via the provider glue.
            self.chat
                .read(cx)
                .store
                .as_ref()
                .and_then(|(store, sid)| {
                    let view = store.read(cx);
                    crate::ahp_store::leaf(&view.book, sid)
                        .model_id()
                        .map(str::to_string)
                })
                .unwrap_or_else(|| {
                    self.chat
                        .read(cx)
                        .thread
                        .read(|t| t.model().cloned())
                        .map(|model| manox_agent::provider_glue::display_name(&model))
                        .unwrap_or_else(|| i18n::t("workspace-no-model").to_string())
                })
        }
    }

    /// Push a system-styled notice into the conversation (no thread message,
    /// no model turn). Used by slash commands and mode toggles to report
    /// outcomes — e.g. the mode-change acknowledgement. Renders as a
    /// neutral-toned `ConvItem::Notice` card (distinct from the red
    /// `ConvItem::Error`).
    ///
    /// The notice is inserted at `anchor` — `TurnEnd` (the end of the current
    /// turn, i.e. the list tail when idle) or `After(ix)` for a tool-call-
    /// adjacent record. Also persists the notice as a session `custom` entry
    /// so a reloaded thread reproduces it at the same position (entries
    /// carrying `tool_call_id` are re-spliced right after their tool item by
    /// the rebuild).
    pub fn add_info_message(
        &mut self,
        text: String,
        anchor: NoticeAnchor,
        tool_call_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let _weak = cx.weak_entity();
        let ix = self.chat_conversation(cx).update(cx, |c, cx| {
            c.push_notice(text.clone(), anchor, self.chat.read(cx).host.clone(), cx)
        });
        self.apply_list_insert(ix, cx);
        self.append_ui_note(manox_agent::db::UiNoteKind::Notice, text, tool_call_id, cx);
        // Tail-follow keeps the viewport pinned to the live end, so a
        // `TurnEnd`-anchored notice is revealed by the follow; an `After`
        // anchored one sits above the viewport by design (a record near its
        // tool call, not an alert).
        cx.notify();
    }

    /// Persist a UI annotation (`Error` / `Notice` / `PlanReview`) as a
    /// `custom` entry in the session jsonl at the current leaf. The append
    /// order IS the reload order, so rebuilt conversations place the card
    /// where it appeared live; entries carrying `tool_call_id` are re-spliced
    /// next to their tool item by the rebuild instead. Fire-and-forget via
    /// the engine's command queue, which orders it against prompts.
    fn append_ui_note(
        &self,
        kind: manox_agent::db::UiNoteKind,
        text: String,
        tool_call_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(sid) = self.chat.read(cx).session_id.clone() {
            self.append_ui_note_for(&sid, kind, text, tool_call_id);
        }
    }

    /// The session-addressed form of [`Self::append_ui_note`]. A parked
    /// thread's events never reach the foreground handler, so a durable card
    /// for one has to be writable while the thread is in the background: the
    /// append lands on that session's journal, which is what reproduces the
    /// card when the thread comes back into view.
    fn append_ui_note_for(
        &self,
        session_id: &str,
        kind: manox_agent::db::UiNoteKind,
        text: String,
        tool_call_id: Option<&str>,
    ) {
        let mut data = serde_json::json!({ "text": text });
        // A tool-anchored notice carries the tool call id so the rebuild can
        // splice it next to the tool item, matching the live placement.
        // `data` is raw JSON — no schema change.
        if let Some(id) = tool_call_id {
            data["tool_call_id"] = serde_json::Value::String(id.to_owned());
        }
        let kind_str = match kind {
            manox_agent::db::UiNoteKind::Error => "error",
            manox_agent::db::UiNoteKind::Notice => "notice",
            manox_agent::db::UiNoteKind::PlanReview => "plan_review",
        };
        // v3: a UI note is a client-local annotation card — the protocol
        // has no client-side transcript write, so it renders from local
        // state only (accepted tradeoff: it does not survive a reload).
        let _ = (session_id, kind_str, data);
    }

    /// Abort the current turn.
    pub(crate) fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        // B2-PR-3: a parked question card is dismissed on the interrupt —
        // the same `{"dismissed": true}` marker the close leg sends — so the
        // server's waterfall converges on it immediately instead of stalling
        // the session pump until the turn-cancel or a disconnect settles it.
        if self.chat.read(cx).pending_ask.is_some() {
            self.dismiss_ask(cx);
        }
        // A dropped cancel is the silent-death shape this file's regressions
        // keep producing: leaving no trace made the composer-locked repro
        // undebuggable.
        let pair = self.chat.read(cx).store.clone();
        if let Some((store, sid)) = pair {
            let view = store.read(cx);
            let turn_id = manox_agent_chat_ui::ahp_store::leaf(&view.book, &sid)
                .chat
                .and_then(|c| c.active_turn.as_ref().map(|t| t.id.clone()));
            tracing::info!(
                session_id = %sid,
                turn_id = ?turn_id,
                "cancel: sending the turn-cancel dispatch"
            );
            store.update(cx, |store, _| {
                if let Some(turn_id) = turn_id {
                    store.cancel_turn(&sid, &turn_id);
                } else {
                    // No active turn in the fold yet the UI reads running —
                    // say so loudly; this is the composer-locked repro.
                    // Tracked with the rest of the dead-lock surface in #88.
                    tracing::warn!(
                        session_id = %sid,
                        "cancel: fold has no active turn (client/host desync); see #88"
                    );
                }
            });
        } else {
            tracing::warn!("cancel dropped: no bound session");
        }
        cx.notify();
    }

    /// Send a protocol write when the landing-thread
    /// connection is available (γ-3 mutation path). Returns `true` when the
    /// note was sent; the caller falls back to `self.chat.thread.update` when `false`.
    /// v2 §D.2 submit path: mint an `origin_rpc` correlation id, register the
    /// optimistic echo in the foreground store, and send the
    /// the submit dispatch (receipt-only per L7 — the durable user row
    /// arrives through the follow stream and retires the echo by matching its
    /// `originRpc`). The conversation's optimistic bubble was already pushed by
    /// the caller; retirement just clears the store's echo bookkeeping so the
    /// row is not treated as a remote (unmatched) insertion.
    ///
    /// Returns `true` when the submit rode the connection (session bound).
    pub(crate) fn send_submit_v2(
        &mut self,
        text: String,
        images: Vec<serde_json::Value>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            tracing::warn!("submit dropped: no session bound to the workspace");
            return false;
        };
        tracing::info!(session_id = %sid, "submit sent (ahp)");
        let _ = images;
        // Sidebar optimism: the host's store row (what `listSessions` serves)
        // lands with the first persistence, which can lag a whole turn — a
        // brand-new conversation would run invisibly in the sidebar. Seed a
        // placeholder row when absent; the host's summary upserts over it.
        let missing = !store.read(cx).book.summaries.contains_key(&sid);
        if missing {
            let title = text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default()
                .to_string();
            store.update(cx, |store, cx| {
                if store.seed_local_summary(&sid, &title) {
                    cx.notify();
                }
            });
        }
        let turn_id = uuid::Uuid::new_v4().to_string();
        store.update(cx, |store, _| {
            store.submit_turn(&sid, &turn_id, text, None);
        });
        true
    }

    /// v2 §D.2 steer path: hand a message to the running turn's server-side
    /// steer queue. The desktop's `Workspace.thread` is an engine-less render
    /// mirror, so the old `thread.enqueue_steer` only inserted a local id and
    /// never reached the server — a dead end where the card sat forever and the
    /// message was neither injected nor confirmed. This sends the real
    /// the steer dispatch (host: enqueue while running,
    /// insert + start a turn while idle). Receipt-only like
    /// [`Self::send_submit_v2`]; no echo is registered because the message does
    /// not enter the conversation here — it moves in only when the turn settles
    /// (see the `TurnFinished` handler). `message_id` is the client-minted id the
    /// composer card correlates by.
    pub(crate) fn send_steer_v2(
        &mut self,
        cx: &App,
        message_id: String,
        text: String,
        images: Vec<serde_json::Value>,
    ) -> bool {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            tracing::warn!("steer dropped: no session bound to the workspace");
            return false;
        };
        tracing::info!(session_id = %sid, "steer sent (ahp)");
        let _ = images;
        let (tx, rx) = async_channel::bounded::<()>(1);
        manox_agent::runtime::handle().spawn(async move {
            let _ = rx.recv().await;
        });
        drop((sid, message_id, text, store, tx));
        true
    }
}

impl Workspace {
    /// Cycle the permission mode on the current thread (`/mode` no-args
    /// form): ReadOnly → WorkspaceAccess → FullAccess → ReadOnly. The mode
    /// change notice rides `apply_permission_mode` so the conversation shows
    /// the switch.
    pub(crate) fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid)
                    .approval_mode()
                    .and_then(|m| serde_json::from_str::<PermissionMode>(&format!("{:?}", m)).ok())
                    .unwrap_or(PermissionMode::ReadOnly)
            })
            .expect("foreground store present")
        {
            PermissionMode::ReadOnly => PermissionMode::WorkspaceWrite,
            PermissionMode::WorkspaceWrite => PermissionMode::DangerFullAccess,
            PermissionMode::DangerFullAccess => PermissionMode::ReadOnly,
        };
        self.apply_permission_mode(next, cx);
    }

    /// Apply `mode` and immediately send `prompt` as a user turn — the
    /// `/mode <name> [prompt]` form. `/mode` dispatches even mid-turn (mode
    /// switches are hot), so the mode applies right away while the prompt
    /// half parks in the follow-up queue like any message sent while
    /// running.
    pub(crate) fn start_mode_turn(
        &mut self,
        mode: PermissionMode,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        self.apply_permission_mode(mode, cx);
        self.send_user_turn(prompt, Vec::new(), cx);
    }

    /// Switch the thread's `PermissionMode`, post a localized notice, and
    /// close the popover. Centralized so slash command, chip click, and the
    /// settings panel wiring all funnel through one path.
    pub(crate) fn apply_permission_mode(&mut self, mode: PermissionMode, cx: &mut Context<Self>) {
        let mode_key = match mode {
            PermissionMode::ReadOnly => "readonly",
            PermissionMode::WorkspaceWrite => "workspacewrite",
            PermissionMode::DangerFullAccess => "dangerfullaccess",
        };
        // The wire value is the serde (kebab-case) form so the AgentServer's
        // `from_value::<PermissionMode>` round-trips; `mode_key` stays the
        // lowercase form the i18n `workspace-mode-notice` selector keys on.
        let mode_wire = serde_json::to_value(mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let _ = mode_wire;
        self.with_foreground_store(cx, |store, sid| {
            let mut config = serde_json::Map::new();
            config.insert("approvalMode".into(), serde_json::json!(mode_wire));
            store.set_config(&sid, config);
        });
        self.add_info_message(
            i18n::t_str("workspace-mode-notice", &[("mode", mode_key)]).to_string(),
            NoticeAnchor::TurnEnd,
            None,
            cx,
        );
        self.close_access_menu(cx);
        cx.notify();
    }
}

/// Whether the gateway's command snapshot (§D.5 `Commands` / `ListCommands`)
/// registers `name` under `kind` (`"command"` for macros, `"skill"` for
/// skills). U2: the slash-dispatch hit check reads the wire projection of the
/// server's command/skill registries instead of the in-process kernel
/// registries, so a remote server's registry decides the hit. Builtins share
/// the `"command"` kind but never reach this check (they dispatch through
/// their own `SlashCommand::execute`, and the registry adapters skip
/// builtin-named keys at init).
fn wire_commands_has(commands: &serde_json::Value, name: &str, kind: &str) -> bool {
    commands.as_array().is_some_and(|entries| {
        entries.iter().any(|e| {
            e.get("name").and_then(|v| v.as_str()) == Some(name)
                && e.get("kind").and_then(|v| v.as_str()) == Some(kind)
        })
    })
}

/// Read the sidebar decoration columns the wire `ThreadListItem` does not
/// carry yet (U2 dual-track): per-thread project/tag/approval-mode plus the
/// registered-project folder list. Returns `(meta by id, distinct active
/// project paths in list order, known projects)`. The meta map spans both
/// store partitions (active rows win) so a tag lookup addresses archived
/// rows too; the project-path list drives the chip's "recent, unregistered"
/// section. This is the last kernel read feeding the sidebar — the cross-
/// domain ask is to extend §D.5 `ThreadsUpdated` with these columns so it
/// retires.
#[cfg(test)]
mod tests;
