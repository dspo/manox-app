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

use std::collections::HashMap;
// Backs the platform-gated `browser_views` map.
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::i18n;
use gpui::ClickEvent;
use gpui::DismissEvent;
use gpui::{
    AnyElement, App, Context, Entity, FollowMode, ListAlignment, ListOffset, ListState,
    MouseButton, Pixels, Render, SharedString, Subscription, WeakEntity, Window, anchored,
    deferred, prelude::*, px,
};
/// Shared across both harnesses: workspace struct fields hold
/// `Option<Entity<PopupMenu>>` regardless of feature.
use gpui_component::{
    ActiveTheme as _, ElementExt as _, Icon, IconName, Sizable as _, Theme,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, RopeExt, Textarea, TextareaState},
    speech::{SpeechButton, SpeechEvent, SpeechState},
    tab::{Tab, TabBar},
    v_flex,
};
/// `WindowExt::push_notification` + `Notification` are shared: the
/// ChatGPT.app launch path (#410) reports outcomes under either harness.
use gpui_component::{WindowExt as _, notification::Notification, tooltip::Tooltip};
use manox_agent::PermissionDecision;
use manox_agent::thread::AccessMode;
use manox_agent::{Thread, ThreadId};

use crate::OpenSettings;
use crate::ToggleTurnNavigator;
use crate::cockpit::format_elapsed;
use crate::conversation::{ConvItem, ConversationState, UserImage, UserTurnMeta};
use crate::views::centered;
use crate::views::completion::{
    CompletionState, SelectHandler, build_replacement, build_replacement_content,
    caret_anchor_origin, detect, mention_source, render_completion, slash_source,
};
use crate::views::composer_menu::{
    PendingAttachment, load_attachment, render_attachment_chips, render_browser_chips,
};
use crate::views::popup_menu;
use crate::views::settings::{SettingsEvent, SettingsView};
use crate::views::turn_navigator::{TurnNavigator, TurnNavigatorEvent, collect_user_turns};
use gpui_component::input::InputContent;

pub use composer::SubmitBehavior;

mod attach;

mod catch_up;
mod chat_column;
/// The send-mode gallery (test-support only): a driver strip over the real
/// composer, driven against a detached store. See `examples/send_mode_gallery.rs`.
#[cfg(feature = "test-support")]
pub mod gallery;
use chat_column::ChatColumn;
use steer_agent_chat_ui::ask_card::AskCardSnapshot;
mod chips;
mod composer_render;
mod exit;
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
        gpui::Entity<steer_agent_chat_ui::ahp_store::AhpStore>,
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

/// Build the composer footer's inline access-mode switch: a two-segment
/// segmented [`TabBar`] (`access-mode-sandbox-title` / `access-mode-full-access-title`
/// as plain-text labels) on the component's muted track, its sliding
/// indicator pill animating between segments — one click reaches either tier,
/// no popover in between. The selected segment IS the current [`AccessMode`]:
/// the caller reads it from the fold (§J11) and mirrors it into
/// `selected_index`, so the control is a pure projection with no local open
/// state. A click on the inactive segment routes through
/// `Workspace::apply_access_mode` (write-ahead config write + object-row
/// mirror + notice); clicking the already-active segment is a no-op — the
/// notice announces a mode transition, it is not an echo. The no-op guard
/// reads the mode from the fold AT CLICK TIME: the optimistic flip repaints a
/// frame later, so a second click landing in that window would otherwise
/// re-apply the same mode and announce it twice. The bar hugs its intrinsic
/// width and refuses to shrink (`flex_shrink_0`), so the footer's left chip
/// group can never squash it.
fn build_access_switch(workspace: WeakEntity<Workspace>, current: AccessMode) -> gpui::Div {
    let selected_ix = usize::from(current == AccessMode::FullAccess);
    h_flex()
        .flex_shrink_0()
        .debug_selector(|| "access-switch".to_string())
        .child(
            TabBar::new("access-switch")
                .segmented()
                .small()
                .selected_index(selected_ix)
                .on_click(move |ix, _window, cx| {
                    let mode = if *ix == 1 {
                        AccessMode::FullAccess
                    } else {
                        AccessMode::Sandbox
                    };
                    let _ = workspace.update(cx, |this, cx| {
                        if this.foreground_access_mode(cx) == mode {
                            return;
                        }
                        this.apply_access_mode(mode, cx);
                    });
                })
                .child(
                    Tab::new()
                        .label(i18n::t("access-mode-sandbox-title"))
                        .debug_selector(|| "access-switch-sandbox".to_string()),
                )
                .child(
                    Tab::new()
                        .label(i18n::t("access-mode-full-access-title"))
                        .debug_selector(|| "access-switch-full-access".to_string()),
                ),
        )
}

impl Workspace {
    /// The foreground session's effective [`AccessMode`] read from the fold
    /// (§J11): the seated value, the host-declared schema default, or the enum
    /// default — never a local guess. `Sandbox` when no store is bound.
    fn foreground_access_mode(&self, cx: &gpui::App) -> AccessMode {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                crate::ahp_store::leaf(&store.read(cx).book, sid).access_mode_or_default()
            })
            .unwrap_or_default()
    }
}
mod composer;
pub mod container;
mod external;
pub mod external_sessions;
mod right_diff;
mod subagent;
mod subagent_stream;

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

/// The open sub-agent stream page: the run it renders, the page entity (its
/// own store observer is the live leg — one subscribe on open, no polling),
/// and the page's back subscription.
struct SubagentStreamPage {
    id: String,
    view: Entity<crate::views::subagent_stream::SubagentStreamView>,
    _sub: Subscription,
}

/// A thread parked in the background while still running a turn. U6b⑤: the
/// park holds NO kernel entity — just the id, the leaf (the wire-state home
/// whose mirrors the server's §D.5 deltas keep fed while the session stays
/// attached), and the parked subscription (it coordinates the settle
/// unread and the follow-up stash; it never
/// touches `conversation`/`self.chat.thread`, so a background thread's events
/// cannot be misattributed to the foreground thread). The turn itself runs
/// server-side and survives parking regardless; nothing detaches or
/// disposes while parked — the reclaim is an in-place re-attach, no reopen.
struct BackgroundThread {
    id: String,
    store: Option<(
        gpui::Entity<steer_agent_chat_ui::ahp_store::AhpStore>,
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

// The chat-column state types moved to steer-agent-chat-ui's `column`
// module (Phase 2 tail); these re-exports keep every bare/`super::` name in
// the workspace family resolving unchanged.
pub use steer_agent_chat_ui::column::{
    AskOption, AskQuestion, ComposerPlaceholderMode, DeferredUserTurn, FollowUpState, PendingAsk,
    PendingConfirmation, QueuedFollowUp, drain_queued_follow_ups, parse_pending_ask,
    steer_group_insert_index,
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

    fn avail(&self) -> steer_agent_chrome_ui::shell::NavAvail {
        let (back, forward) = match self.index {
            None => (false, false),
            Some(i) => (i > 0 && i < self.entries.len(), i + 1 < self.entries.len()),
        };
        steer_agent_chrome_ui::shell::NavAvail { back, forward }
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
    /// no reopen, and the parked subscription keeps the settle unread and the
    /// follow-up stash coordinated).
    background_threads: Vec<BackgroundThread>,
    /// Distinct bound-project paths of the active summaries, in list order
    /// (the project chip's "recent, unregistered" section; U2 push cache).
    /// Registered project folders (chip menu + the sidebar grouping push).
    /// Repaint observer on the multiplexer's list/registry state (U2): its
    /// notify drives the sidebar rows and the workspace's model surfaces.
    _mux_lists: gpui::Subscription,
    _rail_updates: gpui::Subscription,
    /// Accumulated child-session events per Agent tool-call id, so a panel
    /// Latest completion text per subagent address (from SubagentProgress
    /// status=Success/Error), used as the panel's final answer when the
    /// Agent tool-result is absent (new Steer bus has no ToolResult).
    /// The Captain's dispatch prompt per subagent address with its send time,
    /// captured from the Steer tool call so a panel always shows the opening
    /// user message — correctly attributed and timed — even before the child
    /// streams anything.
    /// Lazily-built browser tab entities, keyed by `BrowserTabId`. A browser
    /// tab keeps its `BrowserView` (and the underlying native webview) across
    /// tab switches; dropped when the tab closes, which detaches the native
    /// view via [`steer_webview::webview::WebView`]'s `Drop`.
    /// Compiled out on Linux/FreeBSD with the integrated browser.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) browser_views: BTreeMap<
        manox_agent::thread_engine::BrowserTabId,
        Entity<crate::views::browser_view::BrowserView>,
    >,
    /// Top-level view mode. `Settings` swaps the main column for the
    /// settings detail and the chrome sidebar for the settings nav (the
    /// injected surface mounts exactly while the mode is `Settings`).
    view_mode: ViewMode,
    /// Live external sessions (project-menu launches: CLI agent TUIs and
    /// plain terminals) and the id of the one occupying the main column
    /// while `view_mode == ExternalSession`. In-memory only — the PTY dies
    /// with the process, nothing persists (the legacy shell's
    /// `external_sessions` + `active_external` pair).
    pub(crate) externals: Vec<external_sessions::ExternalSessionRecord>,
    active_external: Option<String>,
    /// The open sub-agent stream page (the Captain status block's sub-agent
    /// line opens it; `view_mode == SubagentStream` while it owns the main
    /// column). One page at a time — the block lists the live runs, and the
    /// page renders the one that was clicked.
    subagent_stream: Option<SubagentStreamPage>,
    /// The right pane's diff tab content (the edit family's full-diff face —
    /// see `right_diff.rs`): ONE tab per pane, its content replaced by every
    /// open-diff click. Session-lived, retired on thread switch.
    pub(crate) right_diff: Option<right_diff::RightDiff>,
    /// Lazily created on the first `enter_settings` call so we don't pay the
    /// cost when the user never opens Settings.
    settings_view: Option<Entity<SettingsView>>,
    settings_sub: Option<Subscription>,
    /// The selected settings page (the split layout's nav selection; the
    /// chrome sidebar's injected nav writes it, the main column reads it).
    /// Index into `views::settings::nav_leaves()`'s flat leaf order.
    pub(crate) settings_nav: usize,
    /// Composer dictation: the speech session behind the mic button. Created
    /// unconditionally — where the platform has no system recognizer (Linux)
    /// or the process lacks the Info.plist usage keys (test binaries; the
    /// steer binary embeds them, see steer-app `build.rs`), `SpeechButton`
    /// renders nothing, so the entry only exists where dictation can actually
    /// run. The final transcript lands at the composer's caret (see
    /// [`Self::land_dictation`]).
    pub(crate) speech: Entity<SpeechState>,
    _speech_events: Subscription,
}

/// Top-level rendering mode of the app page. `Settings` and the two
/// conversation-replacing pages are main-column swaps: `ExternalSession`
/// (a launched TUI / plain terminal) and `SubagentStream` (one running
/// sub-agent's own transcript). Every other surface (browser, sub-agent
/// panel, dock) is a shell surface — a right-pane tab or the bottom dock —
/// and never a view mode.
#[derive(Default)]
enum ViewMode {
    #[default]
    Workspace,
    /// The settings detail owns the main column while the chrome sidebar
    /// renders the injected settings nav. Entering still parks any external
    /// session / sub-agent page that owned the column — Settings is the
    /// page while it is up.
    Settings,
    /// A live external session (a launched CLI agent TUI / plain terminal)
    /// renders IN PLACE OF the conversation column — the legacy shell's
    /// full-pane external-session mode. Switching away parks the session
    /// (its terminal keeps running); closing kills it.
    ExternalSession,
    /// One running sub-agent's own transcript renders in place of the
    /// conversation column (the Captain status block's sub-agent line opens
    /// it). Switching away drops the page: it is a live view of a child
    /// chat, and reopening re-subscribes that chat's channel.
    SubagentStream,
}

/// The main card's `border_1` on both edges; width budgets that measure
/// card-interior space subtract this.
const CARD_BORDER: f32 = 2.;

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
    /// Reconcile the message list with one projection pass: splice on a
    /// count change, remeasure exactly the rows whose content mutated, and
    /// remeasure everything when the shape moved beyond a tail append.
    fn apply_projection(&mut self, change: crate::conversation::ProjectionChange, cx: &mut App) {
        self.sync_list_count(cx);
        let list_state = self.chat.read(cx).list_state.clone();
        for ix in &change.changed {
            list_state.remeasure_items(*ix..*ix + 1);
        }
        if change.structural {
            list_state.remeasure();
        }
    }

    /// The single transcript sync — the one funnel every trigger converges
    /// on: the store observe on any fold change, a thread attach, an
    /// optimistic submit. Projects the foreground session's chat fold into
    /// the conversation (the same keyed diff whether the fold just grew one
    /// delta or just arrived whole), reconciles the list, and drives the
    /// turn-lifecycle edges the state transitions imply.
    pub(crate) fn sync_conversation(
        &mut self,
        store: &gpui::Entity<steer_agent_chat_ui::ahp_store::AhpStore>,
        cx: &mut gpui::Context<Self>,
    ) {
        // A stale observer (the outgoing thread's, before its rebind) must
        // not drive the foreground transcript.
        let bound = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .is_some_and(|(s, _)| s.entity_id() == store.entity_id());
        if !bound {
            return;
        }
        let sid = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone())
            .expect("bound above");
        let running = {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, &sid).running()
        };
        let cancelled = {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, &sid)
                .chat
                .is_none_or(|chat| {
                    chat.active_turn.is_none()
                        && chat.turns.last().is_some_and(|turn| {
                            turn.state == ahp_types::state::TurnState::Cancelled
                        })
                })
        };
        let was_running = self.chat.read(cx).turn_active;

        // Steer-consumption receipts retire their cards first, so the badge
        // they stamp (through a turn-id-bearing bubble the adoption consumes)
        // and the entrance ride they name land in the same projection pass.
        let receipts = store.update(cx, |s, _| s.drain_steer_receipts(&sid));
        let steered_turns = store.update(cx, |s, _| s.steered_turns(&sid));
        let mut entering = Vec::<String>::new();
        let mut steer_turns = Vec::<(String, String)>::new();
        for receipt in receipts {
            let turn_id = receipt.turn_id.as_deref();
            match receipt.wake_text {
                Some(text) => {
                    self.retire_woken_steer(Some(&receipt.message_id), Some(&text), turn_id, cx);
                }
                None => {
                    self.retire_steer_card(&receipt.message_id, turn_id, cx);
                }
            }
            // The entrance ride follows the RECEIPT's turn, not the card:
            // the removal precedes the partition on the wire, so the card is
            // often already retired when the row's turn names itself.
            if let Some(turn) = receipt.turn_id.as_ref() {
                if !entering.contains(turn) {
                    entering.push(turn.clone());
                }
                steer_turns.push((receipt.message_id.clone(), turn.clone()));
            }
        }
        // A window bubble minted by the removal (no turn id) is upgraded to
        // the identity the partition names, so the projection adopts it even
        // when both receipts drained in THIS sync — a parked re-attach, or a
        // backlog that coalesced the two envelopes. Applied after the loop,
        // so a turn-start processed before its removal still upgrades the
        // bubble that removal mints later in the same drain.
        if !steer_turns.is_empty() {
            let conversation = self.chat_conversation(cx);
            conversation.update(cx, |conv, _| {
                for (steer_id, turn_id) in &steer_turns {
                    conv.attach_steer_turn(steer_id, turn_id);
                }
            });
        }
        // A mid-run steer's partition lands as TWO envelopes — the boundary's
        // `turnCompleted` and the steer turn's `turnStarted` — with the
        // not-running state between them observable. That off state is not a
        // settle: consume the boundary flag and hold the running mirror up
        // until the steer turn's own start (the next envelope) restores it.
        // A flag with no steer turn behind it (the run aborted in the gap)
        // self-heals: the flag is spent on this sync, the next falling edge
        // settles normally.
        let steer_boundary = store.update(cx, |s, _| s.take_steer_boundary(&sid));

        // The projection itself: one keyed diff over the fold. The fold is
        // read inside the conversation update — the diff walks the state,
        // it never copies it.
        let role = self.model_label(cx);
        let recipient = self.recipient_author(cx);
        let host = self.chat.read(cx).host.clone();
        let cwd = thread_cwd(&self.chat.read(cx).thread, &self.chat.read(cx).store, cx);
        let fork_source = self.fork_source_session(cx);
        let promoted = self
            .chat
            .read(cx)
            .pending_confirmation
            .as_ref()
            .map(|parked| parked.tool_call_id.clone());
        let conversation = self.chat_conversation(cx);
        let env = steer_agent_chat_ui::transcript::ProjectEnv {
            role: &role,
            recipient: &recipient,
            host: &host,
            cwd: cwd.as_deref(),
            fork_source: fork_source.is_some(),
            promoted_confirmation: promoted.as_deref(),
            entering_turns: &entering,
            steered_turns: &steered_turns,
        };
        let change = conversation.update(cx, |conv, cx| {
            let refusals = store.update(cx, |s, _| s.drain_refusals(&sid));
            conv.absorb_refusals(
                refusals
                    .into_iter()
                    .map(|r| steer_agent_chat_ui::transcript::RefusalNotice {
                        seq: r.seq,
                        text: r.text,
                        turn_id: r.turn_id,
                    })
                    .collect(),
            );
            // Two phases: the fold borrow lives only across the stage.
            let nodes = {
                let view = store.read(cx);
                let leaf = crate::ahp_store::leaf(&view.book, &sid);
                conv.stage(leaf.chat, &env)
            };
            conv.reconcile(nodes, &env, cx)
        });
        self.apply_projection(change, cx);

        // The chat's presence is the loading gate's whole question now: a
        // landed fold carries whatever history it has (possibly none — the
        // hero screen is the empty projection), and the projection keeps the
        // transcript identical to it from here on.
        let chat_landed = {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, &sid).chat.is_some()
        };
        if chat_landed && self.chat.read(cx).awaiting_history.is_some() {
            self.chat.update(cx, |chat, cx| {
                chat.awaiting_history = None;
                cx.notify();
            });
        }
        // The plan-channel subscription follows the default-chat pointer
        // (the two diverge on session continuation).
        self.sync_plan_channel(cx);

        // Turn-lifecycle edges: the state's running bit against the last
        // sync's mirror. The rising edge arms the elapsed tickers; the
        // falling edge settles the follow-up queue (a cancelled settle
        // strands the parked steers) and refreshes the sidebar/git views.
        // A steer-boundary completion is the one falling edge that is not a
        // settle: the partition's `turnCompleted` fired between envelopes,
        // and the steer turn's `turnStarted` restores the bit next envelope.
        if running != was_running && !(steer_boundary && !running) {
            self.chat.update(cx, |chat, cx| {
                chat.turn_active = running;
                cx.notify();
            });
            if running {
                self.spawn_thinking_ticker(cx);
            } else {
                self.multiplexer.update(cx, |m, cx| m.fetch_thread_list(cx));
                self.spawn_git_status_refresh(cx);
                self.settle_follow_ups(cancelled, cx);
            }
        }
        cx.notify();
    }

    /// Submit a user turn optimistically: the bubble renders this frame and
    /// is adopted by the turn `turn_id` mints — identity, not text, so a
    /// `/skill`'s compact bubble still pairs with the journal's expanded
    /// body. Re-arms tail-follow, the same discipline the old direct push
    /// had.
    pub(super) fn push_optimistic_bubble(
        &mut self,
        text: String,
        images: Vec<UserImage>,
        meta: UserTurnMeta,
        turn_id: Option<String>,
        steer_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some((store, sid)) = self.chat_store(cx) else {
            return;
        };
        let conversation = self.chat_conversation(cx);
        let role = self.model_label(cx);
        let recipient = self.recipient_author(cx);
        let host = self.chat.read(cx).host.clone();
        let cwd = thread_cwd(&self.chat.read(cx).thread, &self.chat.read(cx).store, cx);
        let fork_source = self.fork_source_session(cx);
        let promoted = self
            .chat
            .read(cx)
            .pending_confirmation
            .as_ref()
            .map(|parked| parked.tool_call_id.clone());
        let steered_turns = store.update(cx, |s, _| s.steered_turns(&sid));
        let env = steer_agent_chat_ui::transcript::ProjectEnv {
            role: &role,
            recipient: &recipient,
            host: &host,
            cwd: cwd.as_deref(),
            fork_source: fork_source.is_some(),
            promoted_confirmation: promoted.as_deref(),
            entering_turns: &[],
            steered_turns: &steered_turns,
        };
        conversation.update(cx, |conv, cx| {
            let chat = {
                let view = store.read(cx);
                let leaf = crate::ahp_store::leaf(&view.book, &sid);
                leaf.chat.cloned()
            };
            let bubble = steer_agent_chat_ui::transcript::OverlayBubbleSeed {
                text,
                images,
                meta,
                turn_id,
                steer_id,
            };
            conv.push_optimistic_user(chat.as_ref(), bubble, &env, cx);
        });
        self.sync_list_count(cx);
        self.follow_message_tail(cx);
    }

    pub(crate) fn with_foreground_store<R>(
        &self,
        cx: &mut gpui::Context<Self>,
        f: impl FnOnce(&mut steer_agent_chat_ui::ahp_store::AhpStore, String) -> R,
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
    ) -> Option<(Entity<steer_agent_chat_ui::ahp_store::AhpStore>, String)> {
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

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The launch directory is the conversation's anchor: a terminal
        // launch from a project dir (e.g. `cargo run` inside the repo)
        // starts a conversation that belongs to that project — the claim's
        // working directory decides the core bucket the sidebar groups by.
        // `/` is the GUI-launch signature on macOS: an unbound conversation
        // must not anchor there, so home is the neutral default; binding a
        // project via the chip / project "+" overrides it later.
        let mut cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        if (!cwd.is_absolute() || cwd.as_path() == "/")
            && let Some(home) = manox_agent::paths::home_dir()
        {
            cwd = home;
        }
        // L11: the process-global server — every window and the embedded
        // web UI share one AgentServer (one ownership/routing table). Its
        // construction installs the AHP runtime builder; the store then
        // dials the host over the in-proc leg.
        let _agent_server = manox_session_core::agent_server::global(cwd.clone());
        // The server's constructor claims the process-wide embedder-tool
        // slot (last-wins), so the Preview tool's chain-install must come
        // after it — earlier wiring would be clobbered (verified on the
        // machine: the model saw no preview tool until this ordering was
        // fixed).
        crate::preview_host::install();
        let ahp_store =
            cx.new(|cx| steer_agent_chat_ui::ahp_store::AhpStore::connect(cwd.clone(), cx));
        // The landing session id is client-minted (the createSession
        // idempotency key), so the session the workspace renders and the one
        // the server drives are the same conversation.
        let landing_id = uuid::Uuid::new_v4().to_string();
        let multiplexer =
            cx.new(|_| crate::multiplexer::SessionMultiplexer::new(ahp_store.clone(), cwd.clone()));
        let session_id = landing_id.clone();
        // The landing conversation is a project-less object row at boot —
        // one sqlite write, no journal, no directory, until the user speaks.
        if let Some(objects) = manox_agent::object_store::ObjectStore::try_global() {
            objects.create_conversation(&session_id, None);
        }
        multiplexer.update(cx, |m, cx| {
            m.create_session(&session_id, cx);
            // GW5: the landing session is the focused one from tick one —
            // its row suppresses unread rises while attached.
            m.set_focused(Some(&session_id), cx);
            // The catalogue channels (commands/skills for the composer's
            // completion popover + the registry-turn hit check, workspaces for
            // the project chip) ride the same pending-write replay as the
            // session subscribes — without this baseline pull the popover's
            // source is permanently empty and every `/skill` submit misses.
            m.fetch_catalogues(cx);
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

        let weak_ws = cx.weak_entity();
        let chat_host: steer_agent_chat_ui::host::ChatHostHandle =
            std::sync::Arc::new(crate::WorkspaceChatHost::new(weak_ws));
        // The rail's background writers (git refresh, subagent progress,
        // plan snapshots) notify only the rail entity; without this
        // observer those notifies never schedule a frame — the changes
        // surface on the next unrelated repaint (silent, self-healing
        // lag). The rail's agents rows drill into the child sessions
        // through the same host the conversation cards use.
        let context_rail = {
            let rail_store = (store.clone(), session_id.clone());
            cx.new(|cx| {
                let mut rail = crate::views::context_rail::ContextRail::new(Some(rail_store), cx);
                rail.host = Some(chat_host.clone());
                rail
            })
        };
        let rail_updates = cx.observe(&context_rail, |_, _, cx| cx.notify());

        // Composer dictation: one speech session state per composer. The mic
        // button self-hides wherever the platform cannot recognize; the
        // subscription lands the final transcript and surfaces failures.
        let speech = cx.new(SpeechState::new);
        let speech_events = cx.subscribe_in(
            &speech,
            window,
            |this, _, event: &SpeechEvent, window, cx| match event {
                SpeechEvent::Final(text) => this.land_dictation(text.clone(), window, cx),
                SpeechEvent::Error(error) => {
                    tracing::warn!(%error, "composer dictation ended with an error");
                }
                _ => {}
            },
        );

        let mut ws = Self {
            cwd: cwd.clone(),
            nav: NavHistory::default(),
            multiplexer,
            client: (),
            background_threads: Vec::new(),
            _mux_lists,
            _rail_updates: rail_updates,
            speech,
            _speech_events: speech_events,
            #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
            browser_views: BTreeMap::new(),
            view_mode: ViewMode::default(),
            externals: Vec::new(),
            active_external: None,
            subagent_stream: None,
            right_diff: None,
            settings_view: None,
            settings_sub: None,
            settings_nav: 0,
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
                ask_state: None,
                model_open: false,
                model_menu: None,
                model_menu_sub: None,
                send_menu: None,
                send_menu_sub: None,
                project_chip_open: false,
                project_chip_menu: None,
                project_chip_menu_sub: None,
                completion: None,
                mention_hover: None,
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
                interrupt_outbox: Vec::new(),
                interrupt_outbox_by_thread: HashMap::new(),
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
                list_count: 0,
                goal_popover_open: false,
                goal_ticker_gen: 0,
                turn_active: false,
                thinking_ticker_gen: 0,
                awaiting_history: None,
                rebuilt_pre_snapshot: false,
                built_turns: 0,
                last_declined_ask: None,
                plan_chat_subscribed: None,
                context_rail,
                transcript_press: Default::default(),
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

    /// The composer input entity, for probes that drive the real textarea and
    /// read back its prepaint caret geometry (the completion-anchor probe).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_chat_input(&self, cx: &App) -> Entity<gpui_component::input::TextareaState> {
        self.chat_input(cx)
    }

    /// The composer's staged pending attachments, for the paste-intercept
    /// probe (a clipboard image must land here, not in the input rope).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_pending_attachments(&self, cx: &App) -> Vec<PendingAttachment> {
        self.chat.read(cx).pending_attachments.clone()
    }

    /// The completion popover's trigger + item names, for probes that pin the
    /// re-sync (an empty list means the popover closed).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_completion(&self, cx: &App) -> Option<(char, Vec<String>)> {
        self.chat.read(cx).completion.as_ref().map(|state| {
            (
                state.trigger,
                state
                    .items
                    .iter()
                    .map(|item| item.name.to_string())
                    .collect(),
            )
        })
    }

    /// Plant a live completion snapshot, as a populated skill/command source
    /// would have produced. Diagnostic-only: the popover sources are empty in
    /// tests, so the paste probe pins the re-sync by whether this planted
    /// snapshot gets replaced (no trigger after the paste → closed).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_seed_completion(&mut self, cx: &mut Context<Self>) {
        use crate::views::completion::{CompletionItem, CompletionKind};
        self.chat.update(cx, |chat, cc| {
            chat.completion = Some(CompletionState::new(
                '@',
                0,
                vec![CompletionItem {
                    name: "planted-skill".into(),
                    description: "planted".into(),
                    kind: CompletionKind::Skill,
                }],
                0,
            ));
            cc.notify();
        });
    }

    /// Drive the dictation landing directly with a finished transcript, for
    /// probes that pin where the text lands without a live recognizer.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_land_dictation(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.land_dictation(SharedString::from(text.to_string()), window, cx);
    }

    /// The composer's hovered `@`-mention chip (registry key + window-space
    /// chip rect), for the hover-tooltip probe.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_mention_hover(
        &self,
        cx: &App,
    ) -> Option<(gpui::SharedString, gpui::Bounds<gpui::Pixels>)> {
        self.chat.read(cx).mention_hover.clone()
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

    /// The foreground session id (the store pair's id half) — the
    /// regression read for a new-thread landing.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_foreground_session(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|(_, sid)| sid.clone())
    }

    /// The foreground leaf's effective working directory — the composer
    /// project chip's read face, and the new-thread inheritance assertion.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_foreground_cwd(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, sid)| crate::ahp_store::leaf(&store.read(cx).book, sid).cwd())
    }

    /// The foreground leaf's canonical model selection (`{provider}/{model}`)
    /// — the Captain strip's model identity source.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_foreground_model(&self, cx: &App) -> Option<String> {
        self.chat.read(cx).store.as_ref().and_then(|(store, sid)| {
            crate::ahp_store::leaf(&store.read(cx).book, sid)
                .model_id()
                .map(str::to_string)
        })
    }

    /// The label the Captain strip resolves its model identity from
    /// (`Workspace::model_label`): canonical `{provider}/{model}` when the
    /// session channel carries a model, the registry default otherwise.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_model_label(&self, cx: &App) -> String {
        self.model_label(cx)
    }

    /// The live external sessions' row ids — the sidebar's routing keys (and
    /// what `open_external_session` resolves). Test-support diagnostic.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_external_session_ids(&self) -> Vec<String> {
        self.externals.iter().map(|s| s.id.clone()).collect()
    }

    /// One external container's A1 face — the alive bit, the 0 号-decided
    /// kind, and the attrs snapshot's cwd. Test-support diagnostic.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_external_container(
        &self,
        id: &str,
    ) -> Option<(bool, Option<container::ContainerKind>, std::path::PathBuf)> {
        self.externals
            .iter()
            .find(|s| s.id == id)
            .map(|s| (s.alive, s.kind, s.attrs.cwd.clone()))
    }

    /// The foreground chat's running sub-agents (the Captain block's sub-agent
    /// lines). Diagnostic-only read face.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_running_subagents(
        &self,
        cx: &App,
    ) -> Vec<steer_agent_chat_ui::ahp_store::SubagentWorkRow> {
        self.running_subagents(cx)
    }

    /// Open one run's sub-agent page by id — the line click's own path.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_open_subagent_stream(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(row) = self
            .running_subagents(cx)
            .into_iter()
            .find(|row| row.id == id)
        {
            self.open_subagent_stream(&row, cx);
        }
    }

    /// The run id whose page owns the main column, if any.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_subagent_stream(&self, _cx: &App) -> Option<String> {
        self.subagent_stream.as_ref().map(|page| page.id.clone())
    }

    /// The open page's transcript texts — the CHILD chat's fold, never the
    /// foreground conversation's.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_subagent_stream_texts(&self, cx: &App) -> Vec<String> {
        self.subagent_stream
            .as_ref()
            .map(|page| page.view.read(cx).transcript_texts(cx))
            .unwrap_or_default()
    }

    /// The open page's row kinds in order — the projection tests read them:
    /// a child fold's reasoning and tool parts must land as the Captain's
    /// thinking segment (`thinking(reasoning+tool)`), not a second shape.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_subagent_stream_item_kinds(&self, cx: &App) -> Vec<String> {
        self.subagent_stream
            .as_ref()
            .map(|page| page.view.read(cx).item_kinds(cx))
            .unwrap_or_default()
    }

    /// Fire the page's back control (its real event path).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_subagent_stream_back(&self, cx: &mut Context<Self>) {
        if let Some(page) = self.subagent_stream.as_ref() {
            page.view.update(cx, |_, cx| {
                cx.emit(crate::views::subagent_stream::BackToConversation)
            });
        }
    }

    /// Whether the AHP client leg finished its handshake — the gate the
    /// host-wired regression binaries wait on before driving the workspace.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ahp_connected(&self, cx: &App) -> bool {
        self.multiplexer.read(cx).store().read(cx).is_connected()
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
        store: gpui::Entity<steer_agent_chat_ui::ahp_store::AhpStore>,
        session_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let sid = session_id.into();
        self.chat.update(cx, |chat, cx| {
            chat.store = Some((store, sid));
            cx.notify();
        });
        // Re-aim the store observe at the bound entity: production never
        // swaps the store half (the multiplexer holds one for its lifetime),
        // so `subscribe_thread` ran once at construction against it — a test
        // binding a different entity must re-install the observe or the
        // production legs (rebuild guard, live-ask edge, event drain) never
        // fire for it. BOTH subscriptions re-seat: a dropped Subscription is
        // a cancelled observer.
        let (thread_events, store_changes) = self.subscribe_thread(cx);
        self.chat.update(cx, |chat, cx| {
            chat.thread_sub = Some(thread_events);
            chat.store_observe = Some(store_changes);
            cx.notify();
        });
    }

    /// Run the transcript sync directly. Diagnostic-only: the production
    /// sync fires on store notify, which a detached test store never
    /// produces.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_sync_conversation(&mut self, cx: &mut Context<Self>) {
        if let Some((store, _)) = self.chat.read(cx).store.clone() {
            self.sync_conversation(&store, cx);
        }
        self.sync_plan_channel(cx);
    }

    /// The dismiss leg (`dismiss_ask`): the ask card's close and the stop
    /// button both ride it. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_dismiss_ask(&mut self, cx: &mut Context<Self>) {
        self.dismiss_ask(cx);
    }

    /// The transcript's assistant texts, in order — the prose a replayed
    /// delta would duplicate (a merge-append grows a row's text without
    /// adding a row, so item counts cannot see it). Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_transcript_texts(&self, cx: &App) -> Vec<String> {
        self.chat
            .read(cx)
            .conversation
            .read(cx)
            .items()
            .iter()
            .filter_map(|item| match item.read(cx).kind() {
                ConvItem::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
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
            cc.notify();
        });
        self.reset_ask_state(cx);
        cx.notify();
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
            cc.notify();
        });
        self.reset_ask_state(cx);
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

    /// Whether any blocking overlay (ask, generic approval, blank project)
    /// is up.
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
        store: &Entity<steer_agent_chat_ui::ahp_store::AhpStore>,
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
                        Some(ask) => {
                            // The ask's support markdown (a plan review's plan
                            // body included) rides the request-level `message`
                            // — `pending_ask_from_ahp` folds it into the first
                            // question's detail.
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
                        tracing::info!(
                            request_id = %request_id,
                            questions = ask.questions.len(),
                            "live ask: arming the interactive card"
                        );
                        self.chat.update(cx, |chat, cx| {
                            chat.pending_ask = Some(ask);
                            chat.pending_ask_live = true;
                            chat.last_declined_ask = None;
                            cx.notify();
                        });
                        self.reset_ask_state(cx);
                        // The ask card's row is the projection's: the fold's
                        // open elicitation is an `ir:` part, and the sync
                        // renders it as the interactive card the snapshot
                        // attaches to. No synthesis leg here anymore.
                    }
                }
            }
            None => {
                let live_seeded = self.chat.read(cx).pending_ask_live;
                let has_ask = self.chat.read(cx).pending_ask.is_some();
                // A live-seeded card retires outright — its source of truth
                // left the fold. Any other card retires when the fold
                // demonstrably lacks its request id: the fold is the only
                // route an answer can ride (`reply_ask` looks it up there),
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
                        cx.notify();
                    });
                    self.reset_ask_state(cx);
                }
            }
        }
        // The unified confirmation card: a tool confirmation (a sandbox
        // escalation or any pre-run gate) parks the model on a decision the
        // conversation card delivers. A question-less elicitation has NO
        // surface by design — the current runtime cannot mint one (the
        // translator always emits structured questions), and the arm below
        // warns if that ever changes.
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
                            Some(steer_agent_chat_ui::column::PendingConfirmation {
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

    /// Sync the ask snapshots (render-time path). Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_sync_ask_card_snapshots(&mut self, cx: &mut Context<Self>) {
        self.sync_ask_card_snapshots(cx);
    }

    /// Whether the pending ask's card would render interactively: a matching
    /// top-level `ToolCall` item carrying the Workspace-derived snapshot and
    /// the card's interactive state.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ask_card_interactive(&self, id: &str, cx: &App) -> bool {
        let Some(ix) = self.chat_conversation(cx).read(cx).find_tool(id, cx) else {
            return false;
        };
        let item = self.chat_conversation(cx).read(cx).items()[ix].read(cx);
        item.ask_snapshot.is_some() && item.ask_state.is_some()
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
    /// The pending question card's id, if one is surfaced. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_pending_ask_id(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .pending_ask
            .as_ref()
            .map(|a| a.id.clone())
    }

    /// The foreground store's delivery failure face. Test-support
    /// diagnostic: the strip's render test reads what the composer reads.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_delivery_failure(
        &self,
        cx: &App,
    ) -> Option<steer_agent_chat_ui::ahp_store::DeliveryFailure> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, _)| store.read(cx).delivery_failure())
    }

    /// The delivery-error strip as a standalone element. Test-support
    /// diagnostic: a geometry probe re-pulls it every frame (an AnyElement
    /// is consumed by its first paint).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_delivery_error_element(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let failure = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|(store, _)| store.read(cx).delivery_failure())?;
        Some(self.render_delivery_error_strip(&failure, cx))
    }

    /// The pending ask's interactive card state (diagnostic-only): the page
    /// and the answers the card is showing.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_ask_state(&self, cx: &App) -> Option<gpui::Entity<ai_elements::AskState>> {
        self.chat
            .read(cx)
            .ask_state
            .as_ref()
            .map(|(_, s)| s.clone())
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

    /// Align the plan-channel subscription with the default-chat pointer:
    /// plan rows are emitted on `x-manox-plan:/<active chat id>`, and the
    /// attach-time subscription rides the session id until the pointer lands
    /// (the two diverge on session continuation). One re-issue per pointer
    /// value; the key comes from `plan_channel_for` — the same authority
    /// every read face uses, so they cannot fork.
    fn sync_plan_channel(&mut self, cx: &mut Context<Self>) {
        if let Some((store, sid)) = self.chat.read(cx).store.clone() {
            let plan_channel = {
                let view = store.read(cx);
                crate::ahp_store::plan_channel_for(&view.book, &sid)
            };
            let chat_id = crate::ahp_store::id_of(&plan_channel).to_string();
            // The session-id form is already subscribed at attach; a pointer
            // that lands on the same id needs no second subscription.
            if chat_id != sid && self.chat.read(cx).plan_chat_subscribed.as_ref() != Some(&chat_id)
            {
                self.chat.update(cx, |chat, _| {
                    chat.plan_chat_subscribed = Some(chat_id.clone());
                });
                store.update(cx, |s, cx| {
                    s.subscribe(plan_channel, cx);
                });
            }
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
            // Live ask edge first: the fold's open elicitation and the open
            // tool confirmation arm the pending state the projection reads —
            // `promoted_confirmation` (the parked call's top-level card) is
            // assembled in THIS pass's environment, so arming must precede
            // projecting or the card waits a notify.
            this.sync_live_ask(&store, cx);
            // The one transcript sync — the projection reads whatever the
            // fold now holds, so a first snapshot, a replayed journal and a
            // single streaming delta all take the same keyed diff.
            this.sync_conversation(&store, cx);
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

    /// Switch into the Settings split layout. The Settings view is created
    /// lazily on first entry; from then on the entity + subscription are
    /// reused so the user's last selection (and any scroll position)
    /// survives re-entry.
    pub fn enter_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.settings_view {
            None => {
                let settings = cx.new(|cx| SettingsView::new(window, cx));
                let sub = self.subscribe_settings(&settings, window, cx);
                self.settings_view = Some(settings);
                self.settings_sub = Some(sub);
            }
            // A reused view re-reads the rosters: switching projects between
            // visits must not keep the last foreground project's agent
            // layer.
            Some(settings) => settings.update(cx, |settings, _| settings.refresh_rosters()),
        }
        // Leaving the external session for Settings drops its ownership:
        // the invariant is `view_mode == ExternalSession` ⟺
        // `active_external` names a live session — a mode flip that keeps
        // the id leaves the title face reading a session not on screen. The
        // sub-agent page obeys the same rule: a mode flip that kept it would
        // leave a page nobody sees still polling its child chat.
        self.active_external = None;
        // Dropping the page drops its refresh poll with it.
        self.subagent_stream = None;
        self.view_mode = ViewMode::Settings;
        // The split layout's other half: the chrome sidebar swaps its session
        // list for the settings nav (leave_settings restores it).
        crate::chrome_assembly::enter_settings_layout(cx);
        cx.notify();
    }

    /// Leave the Settings split layout: back to the conversation column and
    /// the session-list sidebar (the nav's back control routes here).
    pub fn leave_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.view_mode = ViewMode::default();
        crate::chrome_assembly::leave_settings_layout(cx);
        cx.notify();
    }

    /// Selects the settings page shown in the main column (the split
    /// layout's nav selection). Out-of-range indices clamp to the last page:
    /// this is a public setter, so a bad index degrades to a wrong-ish page
    /// instead of panicking a render path.
    pub fn select_settings_page(&mut self, ix: usize, cx: &mut Context<Self>) {
        let last = crate::views::settings::nav_leaves().len().saturating_sub(1);
        self.settings_nav = ix.min(last);
        cx.notify();
    }

    /// Flips the Agents detail's prompt display to the source editor or the
    /// rendered preview.
    pub fn set_agent_prompt_edit(&mut self, edit: bool, cx: &mut Context<Self>) {
        if let Some(settings) = self.settings_view.as_ref() {
            settings.update(cx, |settings, cx| {
                settings.set_prompt_edit(edit);
                cx.notify();
            });
        }
    }

    /// Drills the Models page into the provider at list position `ix`.
    pub fn select_model_provider(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(settings) = self.settings_view.as_ref() {
            settings.update(cx, |settings, cx| {
                settings.select_model_provider(ix);
                cx.notify();
            });
        }
    }

    /// Drills the Agents page into one agent's detail (`None` = the list).
    pub fn select_agent(
        &mut self,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(settings) = self.settings_view.as_ref() {
            settings.update(cx, |settings, cx| {
                match name {
                    Some(name) => settings.open_agent(name, window, cx),
                    None => settings.close_agent(),
                }
                cx.notify();
            });
        }
    }

    fn subscribe_settings(
        &self,
        settings: &Entity<SettingsView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        cx.subscribe_in(
            settings,
            window,
            |_this, _settings, ev: &SettingsEvent, window, cx| {
                let SettingsEvent::SaveFailed(msg) = ev;
                window.push_notification(
                    Notification::error(msg.clone()).title(i18n::t("settings-save-failed-title")),
                    cx,
                );
            },
        )
    }

    /// Switch to the conversation pane. Drops external ownership with the
    /// mode flip (same invariant as the Settings arm), and the sub-agent page
    /// with it — and restores the session-list sidebar if the settings nav
    /// was still mounted.
    pub fn focus_conversation(&mut self, cx: &mut Context<Self>) {
        self.active_external = None;
        self.subagent_stream = None;
        self.view_mode = ViewMode::Workspace;
        crate::chrome_assembly::leave_settings_layout(cx);
        cx.notify();
    }

    /// How many sessions currently owe the user a look or a verdict — the
    /// dock badge source (`SessionMultiplexer::attention_count`). The
    /// aggregate reads the multiplexer's rows plus the leaves' unread
    /// mirrors, so any state edge that moves the sidebar's attention marks
    /// moves this count on the next read.
    pub fn attention_count(&self, cx: &App) -> usize {
        self.multiplexer.read(cx).attention_count(cx)
    }

    /// Land a finished dictation transcript at the composer's caret. The
    /// insert rides the public edit API so the input's `Change` event fires
    /// and the completion popover re-syncs; the composer takes focus back so
    /// the next keystroke (or Enter-to-send) lands in it. An empty transcript
    /// (the recognizer heard nothing) is a no-op that doesn't steal focus.
    fn land_dictation(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.chat_input(cx).update(cx, |state, cx| {
            state.insert(text, window, cx);
            state.focus(window, cx);
        });
        cx.notify();
    }

    fn subscribe_input(&self, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        let input = self.chat.read(cx).input_state.clone();
        cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                // The thinkrail key scheme: Enter sends (steers while
                // running), ⌘↩ queues, ⇧⌘↩ interrupts — three actions on one
                // key, each an explicit gesture, never sticky mode state.
                // ⇧⌘↩ rides the dedicated ComposerSubmitInterrupt action
                // (see render.rs), not an Enter variant: the input's Enter
                // path newlines on shift before emitting.
                InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } => this.submit_input(window, cx),
                InputEvent::PressEnter {
                    secondary: true,
                    shift: false,
                } => this.submit_steer(window, cx),
                // Shift+Enter inserts a newline inside the input and does not submit.
                // ⇧⌘↩ never lands here — the dedicated
                // ComposerSubmitInterrupt action owns the chord (see
                // render.rs); this arm stays for exhaustiveness only.
                InputEvent::PressEnter {
                    secondary: true,
                    shift: true,
                } => {}
                // Shift+Enter inserts a newline inside the input and does not submit.
                InputEvent::PressEnter {
                    secondary: false,
                    shift: true,
                } => {}
                InputEvent::Change => {
                    // Content changes can delete the hovered chip outright
                    // (atomic backspace, submit clear) — the token's element
                    // vanishes with it, so no exit event ever retires the
                    // hover state. Clear it here, before it can outlive the
                    // content it described.
                    if this.chat.read(cx).mention_hover.is_some() {
                        this.chat.update(cx, |chat, cx| {
                            chat.mention_hover = None;
                            cx.notify();
                        });
                    }
                    this.sync_completion(window, cx);
                }
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
        // The inserted name rides as an atomic inline token (chip render, whole
        // unit delete); a rejected token op degrades to the plain-text insert.
        let (new_value, caret): (InputContent, usize) =
            match build_replacement_content(trigger, &name, &name, &value, token_start, cursor) {
                Ok((content, caret)) => (content, caret),
                Err(_) => {
                    let (text, caret) =
                        build_replacement(trigger, &name, &value, token_start, cursor);
                    (InputContent::from(text), caret)
                }
            };
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
        let access_mode = self
            .chat
            .read(cx)
            .store
            .clone()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, &sid).access_mode_or_default()
            })
            .unwrap_or_default();
        UserTurnMeta::new(
            chrono::Utc::now().timestamp(),
            self.model_label(cx),
            Some(access_mode),
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
                        .map(|model| Self::model_wire_id(&model))
                        // Same rule as the chip's identity read: a fresh
                        // session's engine already runs the registry default,
                        // so show it — "no model" only when nothing is
                        // registered at all.
                        .or_else(|| {
                            manox_agent::provider_glue::default_model()
                                .map(|m| Self::model_wire_id(&m))
                        })
                        .unwrap_or_else(|| i18n::t("workspace-no-model").to_string())
                })
        }
    }

    /// The canonical `{registration}/{model-id}` wire identity of a
    /// registered model — the form every display surface resolves through
    /// [`steer_agent_chat_ui::model_tag::ModelTag`]; an unresolvable id has no
    /// provider segment and no wire tint, so the label never hands it a
    /// display name. `provider_glue::display_name` belongs to the surfaces
    /// that RENDER a name (the model catalog, the context rail); the label's
    /// own last resort is the localized "no model" string.
    fn model_wire_id(model: &manox_harness::types::Model) -> String {
        format!("{}/{}", model.provider, model.id)
    }

    /// Report a system outcome into the conversation (no thread message, no
    /// model turn). Used by slash commands and mode toggles — e.g. the
    /// mode-change acknowledgement. The card's single live display is the
    /// chat-face echo of its own `annotation_set` row (dspo/manox#889): the
    /// dispatch lands the row, the projection answers with a
    /// `systemNotification` part, and the live pump renders it — the same
    /// source the rebuild folds, so the card can never double-display.
    pub fn add_info_message(&mut self, text: String, cx: &mut Context<Self>) {
        let _weak = cx.weak_entity();
        self.append_ui_note(manox_agent::db::UiNoteKind::Notice, text, cx);
        cx.notify();
    }

    /// Persist a UI annotation (`Error` / `Notice`) as an
    /// `annotation_set` row on the session journal. Fire-and-forget; the
    /// chat-face echo displays it live and the rebuild folds it back in place.
    fn append_ui_note(
        &self,
        kind: manox_agent::db::UiNoteKind,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(sid) = self.chat.read(cx).session_id.clone() {
            self.append_ui_note_for(&sid, kind, text, cx);
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
        cx: &mut Context<Self>,
    ) {
        let kind_str = match kind {
            manox_agent::db::UiNoteKind::Error => "error",
            manox_agent::db::UiNoteKind::Notice => "notice",
        };
        // The annotations channel (dspo/manox#884) is the durable face: the
        // dispatch lands an `annotation_set` journal row host-side, so the
        // card survives reloads and reaches every client. The card's kind
        // rides `_meta["x-manox"]`, the protocol's extension slot.
        let Some((store, _)) = self.chat_store(cx) else {
            return;
        };
        let note_id = format!("note-{}", uuid::Uuid::new_v4().simple());
        let mut xmanox = serde_json::Map::new();
        xmanox.insert("uiNoteKind".to_string(), serde_json::json!(kind_str));
        let session = steer_agent_chat_ui::ahp_store::session_uri(session_id);
        let annotation = ahp_types::state::Annotation {
            id: note_id.clone(),
            origin: ahp_types::state::AnnotationOrigin {
                session: session.clone(),
                chat: None,
                turn_id: None,
            },
            // A UI note has no file to anchor to — the session itself is the
            // anchor, and `_meta` carries the render payload.
            resource: session,
            range: None,
            resolved: false,
            entries: vec![ahp_types::state::AnnotationEntry {
                id: format!("{note_id}-e1"),
                text: ahp_types::common::StringOrMarkdown::Plain(text),
                meta: None,
            }],
            meta: Some({
                let mut meta = ahp_types::common::JsonObject::new();
                meta.insert("x-manox".to_string(), serde_json::Value::Object(xmanox));
                meta
            }),
        };
        store.update(cx, |store, _| {
            store.dispatch(
                manox_ahp::channels::annotations::uri(session_id),
                ahp_types::actions::StateAction::AnnotationsSet(
                    ahp_types::actions::AnnotationsSetAction { annotation },
                ),
            );
        });
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
            let turn_id = steer_agent_chat_ui::ahp_store::leaf(&view.book, &sid)
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
    /// Returns the turn id the submit minted, or `None` when it never rode
    /// the wire (no session bound).
    pub(crate) fn send_submit_v2(
        &mut self,
        text: String,
        images: Vec<serde_json::Value>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            tracing::warn!("submit dropped: no session bound to the workspace");
            return None;
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
        Some(turn_id)
    }

    /// v2 §D.2 steer path: hand a message to the server's steer queue via
    /// `chat/pendingMessageSet{kind: steering}`. The backend parks it while
    /// idle and injects it into the running turn otherwise — one intent
    /// covers both; the pending id IS the steer id the journal row, the
    /// removal echo and the composer card all correlate by. Receipt-only
    /// like [`Self::send_submit_v2`]; the card retires when the wire
    /// reports the consumption (see `retire_steer_card`). `message_id` is
    /// the client-minted id the composer card correlates by.
    pub(crate) fn send_steer_v2(
        &mut self,
        cx: &mut Context<Self>,
        message_id: String,
        text: String,
        images: Vec<serde_json::Value>,
    ) -> bool {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            tracing::warn!("steer dropped: no session bound to the workspace");
            return false;
        };
        tracing::info!(session_id = %sid, "steer sent (ahp)");
        // The pending-message wire shape is a text message; a steer carrying
        // attachments has no lossless ride — callers downgrade those to the
        // queue (text-only too; see `flush_batch`).
        let _ = images;
        store.update(cx, |store, _| {
            store.set_pending_message(
                &sid,
                &message_id,
                ahp_types::state::PendingMessageKind::Steering,
                text,
            );
        });
        true
    }
}

impl Workspace {
    /// Toggle the access mode on the current thread (`/mode` no-args
    /// form): Sandbox ↔ FullAccess. The mode change notice rides
    /// `apply_access_mode` so the conversation shows the switch.
    pub(crate) fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let next = match self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).access_mode_or_default()
            })
            .expect("foreground store present")
        {
            AccessMode::Sandbox => AccessMode::FullAccess,
            AccessMode::FullAccess => AccessMode::Sandbox,
        };
        self.apply_access_mode(next, cx);
    }

    /// Apply `mode` and immediately send `prompt` as a user turn — the
    /// `/mode <name> [prompt]` form. `/mode` dispatches even mid-turn (mode
    /// switches are hot), so the mode applies right away while the prompt
    /// half parks in the follow-up queue like any message sent while
    /// running.
    pub(crate) fn start_mode_turn(
        &mut self,
        mode: AccessMode,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        self.apply_access_mode(mode, cx);
        let behavior = if self.foreground_running(cx) {
            SubmitBehavior::Steer
        } else {
            SubmitBehavior::Send
        };
        self.send_user_turn(prompt, Vec::new(), behavior, cx);
    }

    /// Switch the thread's `AccessMode` and post a localized notice.
    /// Centralized so slash command, the footer switch's click, and the
    /// settings panel wiring all funnel through one path. The write is
    /// write-ahead: the switch flips optimistically (the model picker's
    /// semantics) and the transcript notice rides the SAME optimistic
    /// value, so the two never disagree; the host's echo confirms
    /// silently, and a refusal always lands a corrective notice that
    /// names the reason and rolls both back.
    pub(crate) fn apply_access_mode(&mut self, mode: AccessMode, cx: &mut Context<Self>) {
        let mode_key = match mode {
            AccessMode::Sandbox => "sandbox",
            AccessMode::FullAccess => "fullaccess",
        };
        // The wire value is the serde (kebab-case) form so the AgentServer's
        // `from_value::<AccessMode>` round-trips; `mode_key` stays the
        // flattened form the i18n `workspace-mode-notice` selector keys on.
        let mode_wire = serde_json::to_value(mode)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        self.with_foreground_store(cx, |store, sid| {
            let mut config = serde_json::Map::new();
            config.insert("accessMode".into(), serde_json::json!(mode_wire));
            store.optimistic_config(&sid, &config);
            store.set_config(&sid, config);
            // The durable pick lands in the object row at once; the engine's
            // request-boundary handshake applies it (with a notice) from
            // there on the next message.
            if let Some(objects) = manox_agent::object_store::ObjectStore::try_global() {
                objects.set_access_mode(&sid, Some(&mode_wire));
            }
        });
        self.add_info_message(
            i18n::t_str("workspace-mode-notice", &[("mode", mode_key)]).to_string(),
            cx,
        );
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
    // The catalogue channel's baseline is the object `{"commands": [...]}`
    // (see `slash_source`); a bare array was the retired `ListCommands` shape.
    commands
        .get("commands")
        .and_then(|v| v.as_array())
        .is_some_and(|entries| {
            entries.iter().any(|e| {
                e.get("name").and_then(|v| v.as_str()) == Some(name)
                    && e.get("kind").and_then(|v| v.as_str()) == Some(kind)
            })
        })
}

/// Read the sidebar decoration columns the wire `ThreadListItem` does not
/// carry yet (U2 dual-track): per-thread project/tag/access-mode plus the
/// registered-project folder list. Returns `(meta by id, distinct active
/// project paths in list order, known projects)`. The meta map spans both
/// store partitions (active rows win) so a tag lookup addresses archived
/// rows too; the project-path list drives the chip's "recent, unregistered"
/// section. This is the last kernel read feeding the sidebar — the cross-
/// domain ask is to extend §D.5 `ThreadsUpdated` with these columns so it
/// retires.
#[cfg(test)]
mod tests;
