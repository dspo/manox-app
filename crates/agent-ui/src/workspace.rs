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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::i18n;
use crate::views::launcher::LauncherPick;
use gpui::DismissEvent;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Context, Entity, FollowMode,
    ListAlignment, ListOffset, ListState, MouseButton, Pixels, Render, ScrollHandle, SharedString,
    Subscription, WeakEntity, Window, anchored, deferred, ease_out_quint, prelude::*, px,
};
use gpui::{ClickEvent, CursorStyle, DragMoveEvent, MouseUpEvent};
/// Shared across both harnesses: workspace struct fields hold
/// `Option<Entity<PopupMenu>>` regardless of feature.
use gpui_component::menu::PopupMenu;
use gpui_component::{
    ActiveTheme as _, ColorName, Disableable as _, ElementExt as _, Icon, IconName, Sizable as _,
    Size, TITLE_BAR_HEIGHT, Theme, TitleBar,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{
        Editor, EditorState, Input, InputEvent, InputState, Paste, RopeExt, Textarea, TextareaState,
    },
    v_flex,
};
use gpui_component::{
    ThemeStyled as _,
    menu::PopupMenuItem,
    tab::{Tab, TabBar},
    tag::{Tag, TagVariant},
};
/// `WindowExt::push_notification` + `Notification` are shared: the
/// ChatGPT.app launch path (#410) reports outcomes under either harness.
use gpui_component::{WindowExt as _, notification::Notification, tooltip::Tooltip};
use manox_agent::PermissionDecision;
use manox_agent::language_model::StopReason;
use manox_agent::thread::PermissionMode;
use manox_agent::thread_engine::BrowserTabId;
use manox_agent::{Thread, ThreadEvent, ThreadId};
use manox_components::markdown::HeadingMode;
use manox_components::markdown::Markdown;
use serde::{Deserialize, Serialize};
use std::rc::Rc;

use crate::client_store_handle::ClientStoreHandle;
use crate::cockpit::{CockpitPhase, format_elapsed};
use crate::conversation::ConvItem;
use crate::conversation::{ApplyOutcome, ConversationState, NoticeAnchor, UserImage, UserTurnMeta};
use crate::external_session::{
    ExternalSession, ResumeSidecar, SessionKind, SessionPlacement, claude_cwd_from_file_head,
    claude_project_dir_for_cwd, claude_session_id_from_file_name, codex_session_id_from_rollout,
    codex_sessions_dir, list_nested_jsonl, list_sidecars, list_top_level_jsonl,
    merge_external_summaries, new_file_names, remove_sidecar, resume_args, write_sidecar,
};
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
use crate::views::sidebar::{Sidebar, SidebarEvent};
use crate::views::turn_navigator::{TurnNavigator, TurnNavigatorEvent, collect_user_turns};
use crate::{
    CloseBrowserTab, CloseTerminalTab, FocusTerminal, NewTerminalTab, OpenBrowserTab,
    ToggleTurnNavigator,
};
use crate::{FocusConversation, OpenSettings};
use manox_terminal::Terminal;
use terminal_ui::TerminalView;
use terminal_ui::terminal_proxy::TerminalProxy;

mod attach;
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
    store: &Option<gpui::Entity<ClientStoreHandle>>,
    cx: &App,
) -> Option<SharedString> {
    let cwd = store
        .as_ref()
        .map(|s| std::path::PathBuf::from(s.read(cx).store.cwd.clone()))
        .unwrap_or_else(|| thread.read(|t| t.cwd().to_path_buf()));
    if cwd.as_os_str().is_empty() {
        None
    } else {
        Some(SharedString::from(cwd.to_string_lossy().to_string()))
    }
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
mod right_pane;

/// A tab in the right observation pane. `Editor` is the markdown composer
/// (Write/Preview); `Launcher` is the empty-tab launcher offering the
/// built-in browser / terminal / CLI-agent views; `Browser(id)` is an
/// untrusted embedded webview (see [`BrowserView`]); `Session(id)` embeds an
/// [`ExternalSession`]'s terminal (plain PTY or CLI agent TUI).
#[derive(Clone, Debug)]
enum RightTab {
    Editor,
    Launcher,
    Browser(BrowserTabId),

    /// A pi sub-agent's observation panel, keyed by subagent address.
    Subagent(String),
    /// An embedded terminal/CLI-agent session, keyed by `ExternalSession.id`.
    Session(String),
}

/// Persisted shape of a thread's right-pane state — one row per thread in
/// `threads.db` (`thread_right_pane`). The UI layer owns this shape; the db
/// stores opaque TEXT. Subagent tabs are ephemeral by design and never
/// serialized.
#[derive(Serialize, Deserialize)]
struct PersistedRightPane {
    visible: bool,
    active: usize,
    tabs: Vec<PersistedRightTab>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PersistedRightTab {
    Editor,
    Launcher,
    Browser { url: String },

    Session { id: String },
}

/// In-session per-thread right-pane stash: the live tabs (browser views
/// keep their entities across switches), the active index, and visibility.
/// The persistent copy lives in `threads.db`.
struct RightPaneSnapshot {
    tabs: Vec<RightTab>,
    active: usize,
    visible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComposerPlacement {
    Hidden,
    Hero,
    Footer,
}

fn composer_placement(editor_open: bool, first_screen: bool) -> ComposerPlacement {
    if editor_open {
        ComposerPlacement::Hidden
    } else if first_screen {
        ComposerPlacement::Hero
    } else {
        ComposerPlacement::Footer
    }
}

fn editor_can_submit(
    history_loading: bool,
    running: bool,
    has_pending_ask: bool,
    text: &str,
) -> bool {
    !history_loading && !running && !has_pending_ask && !text.trim().is_empty()
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
    store: Option<gpui::Entity<ClientStoreHandle>>,
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
pub(crate) use manox_agent_chat_ui::column::parse_pending_ask;
pub use manox_agent_chat_ui::column::{
    AskIntent, AskOption, AskQuestion, ComposerPlaceholderMode, DeferredUserTurn, FollowUpState,
    PendingAsk, PendingAuth, QueuedFollowUp,
};

pub struct Workspace {
    pub(crate) cwd: PathBuf,
    /// Dual-shell embed (PLAN Phase 4 tranche 3): true when this workspace
    /// is mounted as the chrome shell's main surface — render then produces
    /// ONLY the conversation column (the chrome shell owns gutter, sidebar,
    /// card chrome, and right pane). Fixed at construction; the legacy
    /// full-shell path is the default.
    pub(crate) embedded: bool,
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
    pub(crate) client: std::sync::Arc<manox_session_core::agent_client::AgentClient>,
    /// Threads that were running when the user switched away (U6b⑤: the
    /// turn runs server-side and survives the switch on its own — the park
    /// keeps the session ATTACHED so the reclaim re-attaches in place with
    /// no reopen, and the parked subscription keeps the settle unread, the
    /// plan-review stash and the follow-up stash coordinated).
    background_threads: Vec<BackgroundThread>,
    pub(crate) sidebar: Entity<Sidebar>,
    /// Distinct bound-project paths of the active summaries, in list order
    /// (the project chip's "recent, unregistered" section; U2 push cache).
    /// Registered project folders (chip menu + the sidebar grouping push).
    /// Repaint observer on the multiplexer's list/registry state (U2): its
    /// notify drives the sidebar rows and the workspace's model surfaces.
    _mux_lists: gpui::Subscription,
    /// Per-thread right-side editor text, keyed by thread id. The editor pane
    /// is a right-side resource of the thread it was written for: switching
    /// away stashes the outgoing text, switching back restores it, so no
    /// thread ever sees another thread's draft and returning recovers the
    /// text. Mirrors `drafts` (the composer's per-thread stash).
    editor_drafts: HashMap<String, String>,
    /// Right-side markdown composer; opened via the `ToggleEditor` shortcut.
    /// Plain-text edit mode by default; `ToggleEditorPreview` switches to a
    /// rendered markdown preview (`Markdown`).
    editor_state: Entity<EditorState>,
    /// Whether the Editor tab is the active right-pane tab. Drives the inline
    /// composer hide (writing happens in the side panel) and the env/hero
    /// gates.
    editor_open: bool,
    editor_preview: bool,
    /// Stable markdown preview entity kept across renders so the source is
    /// only re-parsed when the draft changes (not every frame).
    editor_preview_md: Option<Entity<Markdown>>,
    /// Explicit pixel-anchored scroll state for the preview column. Mirrors the
    /// message-list pattern: an explicit handle (not entity-state scroll) keeps
    /// the offset stable and defaulting to the top, and a `flex_1`-sized (not
    /// `h_full`-percentage) scroll container reliably engages `overflow_y_scroll`
    /// instead of letting content overflow and clip.
    editor_preview_scroll: ScrollHandle,
    /// Peer right-pane tabs for the editor, launcher, browser, sub-agent
    /// observers, and embedded terminal/CLI sessions. `editor_open` tracks
    /// whether the Editor tab specifically is active.
    right_tabs: Vec<RightTab>,
    active_right_tab: usize,
    /// Right-pane visibility gate, orthogonal to the tab list: hiding the pane
    /// keeps every tab (and its state) alive for the next toggle. Closing the
    /// last tab hides the pane; the TitleBar toggle restores the tabs.
    right_pane_visible: bool,
    /// Per-thread right-pane stash for in-session round trips; the persistent
    /// copy lives in `threads.db` (`thread_right_pane`).
    right_pane_by_thread: HashMap<String, RightPaneSnapshot>,
    /// The tab currently under the mouse — the close `×` reveals on hover.
    hovered_right_tab: Option<usize>,
    /// Generation counter for the browser page-title ticker; bumped when the
    /// last browser tab closes so the prior ticker self-terminates.
    browser_title_ticker_gen: u64,
    /// Provider→model cascade opened from the Launcher's CLI-agent rows.
    /// Created on open, destroyed on close (the model-selector pattern).
    launcher_menu: Option<Entity<PopupMenu>>,
    launcher_menu_sub: Option<Subscription>,
    /// The CLI agent kind the open launcher cascade belongs to — anchors the
    /// popup under its launcher row.
    launcher_menu_kind: Option<SessionKind>,
    /// Live sub-agent observation panels keyed by Agent tool-call id.
    subagent_panels: HashMap<String, Entity<crate::views::subagent_panel::SubagentPanel>>,
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
    /// Editor pane width, driven by dragging the divider. In-memory only.
    editor_width: Pixels,
    /// Sidebar width, driven by dragging the divider on its right edge.
    /// In-memory only; never persisted so the user's drag state stays
    /// session-local.
    sidebar_width: Pixels,
    /// Sidebar collapse gate (the TitleBar's panel-left toggle): collapsed
    /// hides the sidebar slot and its resize handle so the main card takes
    /// the full width; the remembered `sidebar_width` survives the round
    /// trip. In-memory only, like the width.
    sidebar_visible: bool,
    sidebar_sub: Option<Subscription>,
    editor_sub: Option<Subscription>,
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
    /// The terminal tab's view, lazily created on the first `FocusTerminal` /
    /// `NewTerminalTab`. `None` until then. Dropped on `CloseTerminalTab`.
    terminal_view: Option<Entity<TerminalView>>,
    /// Live external agent CLI sessions (claude / codex / copilot) launched from
    /// the sidebar `+` menu. In-memory only — never persisted. Each owns its
    /// `TerminalView` plus a shared `Arc<SessionHandle>` so the close path can
    /// `kill` the agent explicitly.
    pub(crate) external_sessions: Vec<crate::external_session::ExternalSession>,
    /// Unclosed external sessions from previous runs, restored from their
    /// sidecars at startup. Rendered in the sidebar as resumable rows; clicking
    /// one re-spawns the CLI with its resume flag. Never auto-resumed.
    resumable_external: Vec<ResumeSidecar>,
    /// Ids of resumable rows whose CLI re-spawn is in flight; the sidebar
    /// shows a loading indicator on each such row. A set (not a single slot)
    /// so resuming two rows concurrently cannot steal each other's spinner.
    resuming_external: std::collections::HashSet<String>,
    /// Conversation file names already claimed by a live session's CLI-session
    /// watcher, keyed by watched directory — concurrent watchers on the same
    /// directory (two sessions in one cwd) can never claim the same file.
    cli_session_claims: std::collections::HashMap<PathBuf, std::collections::HashSet<String>>,
    /// The currently-displayed external session id when
    /// `view_mode == ExternalSession`. Mirrors `terminal_view`'s "one at a
    /// time" model; switching away parks the session (its terminal keeps
    /// running) rather than killing it.
    active_external: Option<String>,
}

/// Top-level rendering mode of the Workspace window. `Settings` and
/// `Terminal` are full-pane switches off the default `Workspace` (conversation)
/// mode; `ExternalSession` shows an external agent CLI's TUI terminal in place
/// of the conversation. Future overlays can extend this enum rather than
/// carrying parallel `bool` flags.
#[derive(Default)]
enum ViewMode {
    #[default]
    Workspace,
    Settings,
    Terminal,
    ExternalSession,
}

/// Right-side composer width. Wide enough for rendered markdown
/// (headings, lists, code blocks) alongside the 1100px window.
const EDITOR_PANEL_WIDTH: f32 = 640.;
const EDITOR_MIN_WIDTH: f32 = 320.;
const EDITOR_MAX_WIDTH: f32 = 960.;
/// Fixed width of every right-pane tab: long labels cap + ellipsis instead
/// of stretching the bar.
const RIGHT_TAB_WIDTH: f32 = 160.;
/// Character cap for right-pane tab labels; longer labels end in `…` and the
/// full text rides the tab's tooltip.
const RIGHT_TAB_LABEL_CAP: usize = 16;

/// Cap a right-pane tab label at [`RIGHT_TAB_LABEL_CAP`] chars + `…`.
fn cap_tab_label(label: &str) -> String {
    let mut chars = label.chars();
    let head: String = chars.by_ref().take(RIGHT_TAB_LABEL_CAP).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}
/// Width of the drag handle between the message column and the right side
/// view (the editor pane).
const EDITOR_DIVIDER_WIDTH: f32 = 6.;
// Mirrors `views/sidebar.rs` (`Sidebar` renders at `w(px(SIDEBAR_WIDTH))`).
// Kept here so the editor pane's resize clamp can reserve space for the
// sidebar + main column without depending on the sidebar's internals.
const SIDEBAR_WIDTH: f32 = 260.;
const SIDEBAR_MIN_WIDTH: f32 = 200.;
const SIDEBAR_MAX_WIDTH: f32 = 480.;
/// Width of the invisible sidebar resize hot zone. It overlays the
/// sidebar/card boundary as an absolute strip and claims no layout space —
/// the two panels sit flush against each other.
const SIDEBAR_DIVIDER_WIDTH: f32 = 6.;
/// Gutter between the window edge and the shell content (the sidebar slot
/// and the main card): wider on the left (the sidebar's seamless outer
/// edge), tighter on the top/bottom/right card sides.
const SHELL_PAD_LEFT: f32 = 10.;
const SHELL_PAD_EDGE: f32 = 4.;

/// The empty band the sidebar slot reserves at its top before any content:
/// macOS floats the traffic lights over it (28px), other platforms need only
/// a small breathing inset (8px). Shared by the sidebar/settings-nav scroll
/// bodies (`pt(top_inset)`) and the shell's sidebar window-drag zone, which
/// must cover exactly this band and never the interactive rows below it.
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
/// Floor for the message column width when the right side view (editor
/// pane) is dragged wide.
const MAIN_MIN_WIDTH: f32 = 160.;

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

fn turn_navigator_layout(
    window_width: Pixels,
    sidebar_width: Pixels,
    right_pane_width: Option<Pixels>,
    show_context_rail: bool,
) -> TurnNavigatorLayout {
    // The overlay anchors to the shell root's padding box (gpui absolute
    // positioning is CSS-style), so both insets carry the shell gutter plus
    // the card's 1px border on their side.
    let left_inset = px(SHELL_PAD_LEFT) + sidebar_width + px(CARD_BORDER / 2.);
    let right_pane_inset = right_pane_width
        .map(|width| width + px(EDITOR_DIVIDER_WIDTH))
        .unwrap_or(px(0.));
    let context_inset = if show_context_rail {
        px(crate::views::context_rail::ENV_CONTENT_INSET)
    } else {
        px(0.)
    };
    let right_inset = px(SHELL_PAD_EDGE) + px(CARD_BORDER / 2.) + right_pane_inset + context_inset;
    let available = window_width - left_inset - right_inset - px(24.);
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

/// Settings overlay slide duration. The enter animation glides the panel in
/// from the left edge, the exit animation glides it out to the right.
const SLIDE_MS: u64 = 180;
/// The Exit handler in `subscribe_settings` waits this long before flipping
/// `view_mode` back to `Workspace`, giving the exit animation time to play.
/// Set slightly above `SLIDE_MS` so the last frame is not popped mid-tween.
const SLIDE_OUT_MS: u64 = 200;

/// Drag payload for the editor pane divider. Doubles as the invisible drag
/// ghost view, mirroring the `DraggedDock` drag-ghost pattern.
struct DraggedEditorDivider;

impl Render for DraggedEditorDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Drag payload for the sidebar divider. Same shape as the editor divider's
/// payload; the two are distinguished by type so their drag-move handlers
/// can each run only on the matching payload.
struct DraggedSidebarDivider;

impl Render for DraggedSidebarDivider {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
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

/// The cascade selection backing [`Workspace::spawn_external_session`]; one
/// struct keeps the spawn entry point under clippy's argument cap.
pub(crate) struct ExternalSpawn {
    kind: SessionKind,
    provider_name: String,
    model_id: String,
    /// Cx wire key pinning the endpoint variant (`anthropic` /
    /// `responses` / `completions`); `None` = default derivation.
    wire_api: Option<String>,
    project_cwd: Option<PathBuf>,
}

impl Workspace {
    // Entity-handle accessors for ChatColumn fields: each returns a cloned
    // handle so callers can `.update(cx, …)` without holding the chat
    // entity's read guard across a mutable borrow of `cx`.
    pub(crate) fn chat_thread(&self, cx: &App) -> manox_agent::thread::ThreadHandle {
        self.chat.read(cx).thread.clone()
    }
    pub(crate) fn chat_store(&self, cx: &App) -> Option<Entity<ClientStoreHandle>> {
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

    /// The chrome-embed constructor: identical state machine and
    /// subscriptions, flagged to render column-only inside the chrome
    /// shell's main surface. [`Self::new`] stays the legacy full shell.
    pub fn new_embedded(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut ws = Self::new(window, cx);
        ws.embedded = true;
        ws
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
        // web UI share one AgentServer (one ownership/routing table).
        let agent_server = manox_session_core::agent_server::global(cwd.clone());
        // The landing thread id doubles as its AgentServer session id
        // (`CreateSession` uses the session id as the `ThreadId`), so the
        // thread the workspace renders and the thread the server drives are
        // the same conversation.
        let landing_id = uuid::Uuid::new_v4().to_string();
        let thread =
            Thread::landing_with_id(manox_agent::ThreadId(landing_id.clone()), cwd.clone());
        let client = std::sync::Arc::new(manox_session_core::agent_client::AgentClient::connect(
            &agent_server,
            "desktop",
            vec![
                manox_protocol::AnswerKind::Approve,
                manox_protocol::AnswerKind::AskUserQuestion,
            ],
            vec![],
        ));
        let multiplexer =
            cx.new(|cx| crate::multiplexer::SessionMultiplexer::with_client(client.clone(), cx));
        let (store, session_id) = {
            let session_id = landing_id.clone();
            let store = multiplexer.update(cx, |m, cx| {
                let handle =
                    m.open_or_create(&session_id, cwd.to_str().unwrap_or_default(), false, cx);
                // GW5: the landing session is the focused one from tick
                // one — its leaf suppresses unread rises while attached.
                m.set_focused(Some(&session_id), cx);
                handle
            });
            (store, session_id)
        };

        let input_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 12)
                .submit_on_enter(true)
                .placeholder(i18n::t("workspace-input-placeholder"))
        });

        let editor_state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(true)
                .folding(false)
                .soft_wrap(true)
                .submit_on_enter(false)
                .placeholder(i18n::t("workspace-composer-placeholder"))
        });

        let sidebar = cx.new(|cx| Sidebar::new(px(SIDEBAR_WIDTH), cx));
        // U2 list source + GW5 badge source: rows are the multiplexer's wire
        // list, and badges prefer the leaves' client-owned unread mirrors.
        sidebar.update(cx, |s, _| s.bind_multiplexer(multiplexer.clone()));
        // U6a/U6b②: no store handle at all — the list refresh rides the
        // server's watcher broadcast, and the attach path is the landing
        // mirror (the wire owns the session state).
        // The multiplexer's notify (its list/registry state changed)
        // repaints the sidebar rows and the workspace's model surfaces, and
        // feeds the chip-menu caches off the wire state (U2 cross-domain
        // #1: the store-read decoration snapshot retired — the registry
        // rides the Projects mirror, the per-thread projects ride the rows).
        let _mux_lists = cx.observe(&multiplexer, |_, _, cx| cx.notify());
        let recipient = thread.read(|t| t.self_author());
        let conversation = cx.new(|_| ConversationState::new(recipient));
        let context_rail =
            { cx.new(|_| crate::views::context_rail::ContextRail::new(Some(store.clone()))) };
        let weak_ws = cx.weak_entity();
        let chat_host: manox_agent_chat_ui::host::ChatHostHandle =
            std::sync::Arc::new(crate::WorkspaceChatHost::new(weak_ws));
        context_rail.update(cx, |r, _| r.set_host(chat_host.clone()));

        let mut ws = Self {
            embedded: false,
            cwd,
            multiplexer,
            client,
            background_threads: Vec::new(),
            sidebar,
            _mux_lists,
            editor_drafts: HashMap::new(),
            editor_state,
            editor_open: false,
            editor_preview: false,
            editor_preview_md: None,
            editor_preview_scroll: ScrollHandle::new(),
            right_tabs: Vec::new(),
            active_right_tab: 0,
            right_pane_visible: false,
            right_pane_by_thread: HashMap::new(),
            hovered_right_tab: None,
            browser_title_ticker_gen: 0,
            launcher_menu: None,
            launcher_menu_sub: None,
            launcher_menu_kind: None,
            subagent_panels: HashMap::new(),
            subagent_transcripts: HashMap::new(),
            subagent_final_text: HashMap::new(),
            subagent_prompts: HashMap::new(),
            browser_views: BTreeMap::new(),
            editor_width: px(EDITOR_PANEL_WIDTH),
            sidebar_width: px(SIDEBAR_WIDTH),
            sidebar_visible: true,
            sidebar_sub: None,
            editor_sub: None,
            view_mode: ViewMode::default(),
            exiting_settings: false,
            settings_transition_gen: 0,
            settings_view: None,
            settings_sub: None,
            terminal_view: None,
            external_sessions: Vec::new(),
            resumable_external: list_sidecars(),
            resuming_external: std::collections::HashSet::new(),
            cli_session_claims: std::collections::HashMap::new(),
            active_external: None,
            chat: cx.new(|_cx| ChatColumn {
                host: chat_host,
                thread,
                store: Some(store),
                session_id: Some(session_id),
                git_status_gen: 0,
                conversation: conversation.clone(),
                input_state,
                drafts: HashMap::new(),
                pending_ask: None,
                pending_auth: None,
                pending_projection_confirmed: false,
                ask_snapshot_item: None,
                ask_step: 0,
                ask_transition_gen: 0,
                ask_custom_inputs: Vec::new(),
                ask_custom_subs: Vec::new(),
                ask_custom_text: Vec::new(),
                ask_skipped: Vec::new(),
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
                list_count: 0,
                goal_popover_open: false,
                goal_ticker_gen: 0,
                turn_active: false,
                thinking_ticker_gen: 0,
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
        ws.sidebar_sub = Some(ws.subscribe_sidebar(window, cx));
        let input_sub = ws.subscribe_input(window, cx);
        ws.chat.update(cx, |chat, cc| {
            chat.input_sub = Some(input_sub);
            cc.notify();
        });
        ws.editor_sub = Some(ws.subscribe_editor(window, cx));
        ws.observe_conversation(cx);
        // The sidebar lists the restored resumable rows from the first frame;
        // nothing is resumed until the user clicks one.
        ws.sync_sidebar_external(cx);
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
            cx.notify();
        });
        self.observe_conversation(cx);
        cx.notify();
    }

    #[cfg(feature = "test-support")]
    pub fn diagnostic_list_state(&self, cx: &App) -> ListState {
        self.chat.read(cx).list_state.clone()
    }

    /// Attach a thread through the production switch path. Diagnostic-only
    /// entry point so integration tests can exercise parking + re-surface
    /// without simulating the sidebar click.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_attach_thread(
        &mut self,
        thread: manox_agent::thread::ThreadHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.attach_thread(thread, false, window, cx);
    }
    /// Emit a `ThreadEvent` on the store bound to `thread_id` — the foreground
    /// store when the id is the active thread, otherwise the parked
    /// background thread's store. Lets tests drive the workspace's subscription
    /// handler without a live AgentServer round-trip.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_emit_event(
        &self,
        thread_id: &str,
        event: ThreadEvent,
        cx: &mut Context<Self>,
    ) {
        let fg = self.chat.read(cx).store.clone();
        let store = if self
            .chat
            .read(cx)
            .thread
            .read(|t| t.id.0.as_str() == thread_id)
        {
            fg.as_ref()
        } else {
            self.background_threads
                .iter()
                .find(|b| b.id == thread_id)
                .and_then(|bg| bg.store.as_ref())
        };
        if let Some(store) = store {
            store.update(cx, |_, cx| cx.emit(event));
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
            // Only AskUserQuestion payloads reach this seeder (the live event
            // path is `ToolCallAuthorization`, which carries the real tool
            // name); the fallback exists solely for a malformed ask payload,
            // so the constant names the tool that actually fired.
            chat.pending_auth = chat.pending_ask.is_none().then(|| PendingAuth {
                id: id.to_string(),
                tool_name: "AskUserQuestion".to_string(),
                summary: String::new(),
            });
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

    /// Seed a non-question authorization (a sandbox escalation, or an ask
    /// whose payload failed to parse) as the pending generic card.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_seed_auth(
        &mut self,
        id: &str,
        tool_name: &str,
        summary: &str,
        cx: &mut Context<Self>,
    ) {
        self.chat.update(cx, |chat, cc| {
            chat.pending_ask = None;
            chat.pending_auth = Some(PendingAuth {
                id: id.to_string(),
                tool_name: tool_name.to_string(),
                summary: summary.to_string(),
            });
            chat.ask_step = 0;
            cc.notify();
        });
        self.reset_ask_custom(cx);
        cx.notify();
    }

    /// The pending generic-approval card as (id, tool_name, summary).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_pending_auth(&self, cx: &App) -> Option<(String, String, String)> {
        self.chat
            .read(cx)
            .pending_auth
            .as_ref()
            .map(|a| (a.id.clone(), a.tool_name.clone(), a.summary.clone()))
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
    pub fn resolve_auth_for_test(&mut self, decision: PermissionDecision, cx: &mut Context<Self>) {
        self.resolve_auth(decision, cx);
    }

    /// Run the missing-card synthesis. Diagnostic-only wrapper around the
    /// private `ensure_ask_tool_item`.
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

    /// The leaf store's pending-auth MsgId keys. Diagnostic-only.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_store_pending_auth_ids(&self, cx: &App) -> Vec<String> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.pending_auth.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Register an auth id → MsgId mapping in the bound leaf store, mirroring
    /// what the live `Request` frame does. Diagnostic-only: lets tests drive
    /// the reply-leg bookkeeping without a wire round-trip.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_seed_store_pending_auth(
        &mut self,
        auth_id: &str,
        msg_id: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(store) = self.chat.read(cx).store.clone() {
            store.update(cx, |h, cx| {
                h.store
                    .pending_auth
                    .insert(auth_id.to_string(), manox_protocol::MsgId::new(msg_id));
                cx.notify();
            });
        }
    }

    /// Merge a projection into the bound leaf store. Diagnostic-only: stands
    /// in for the gateway's `Projections` stream without a wire round-trip.
    #[cfg(feature = "test-support")]
    pub fn diagnostic_merge_projection(
        &mut self,
        key: &str,
        value: serde_json::Value,
        seq: u64,
        cx: &mut Context<Self>,
    ) {
        if let Some(store) = self.chat.read(cx).store.clone() {
            store.update(cx, |h, cx| {
                h.store.merge_projection(key, value, seq);
                cx.notify();
            });
        }
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
    pub(crate) fn rebuild_conversation_from_thread(&mut self, cx: &mut Context<Self>) {
        let display: Vec<manox_agent::db::HistoryEntry> = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.display.clone())
            .expect("foreground store present");
        let subagent_rows = manox_agent::subagent_restore::rebuild_from_messages(
            &self
                .chat
                .read(cx)
                .store
                .as_ref()
                .map(|s| s.read(cx).store.derived_messages())
                .expect("foreground store present"),
        );
        let usage = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| {
                s.read(cx)
                    .store
                    .per_request_usage
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            manox_agent::TokenUsage {
                                input_tokens: v.input,
                                output_tokens: v.output,
                                cache_creation_input_tokens: v.cache_creation,
                                cache_read_input_tokens: v.cache_read,
                            },
                        )
                    })
                    .collect()
            })
            .expect("foreground store present");
        let role = self.model_label(cx);
        let recipient = self.recipient_author(cx);
        let _weak = cx.weak_entity();
        let running = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.running)
            .expect("foreground store present");
        let cwd = thread_cwd(&self.chat.read(cx).thread, &self.chat.read(cx).store, cx);
        // A rebuild from the thread is that session's journal replayed, so its
        // rows may anchor forks.
        let fork_source = self.fork_source_session(cx);
        let new_conv = cx.new(|cx| {
            ConversationState::rebuild_from_display(
                &display,
                &usage,
                &role,
                recipient,
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
            cx.notify();
        });
        self.observe_conversation(cx);
        let count = self.chat_conversation(cx).read(cx).items().len();
        self.chat.update(cx, |chat, cx| {
            chat.list_state.reset(count);
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.list_count = count;
            cx.notify();
        });
        // `Tail` natively pins to the end and keeps following; an upward user
        // scroll disengages it and landing back at the bottom re-arms it.
        self.chat
            .read(cx)
            .list_state
            .set_follow_mode(FollowMode::Tail);
        // Recover the settled sub-agent observation rows alongside the
        // conversation: a restored transcript is the only record of runs that
        // finished (or were killed) before the restart / switch.
        self.apply_subagent_rows(subagent_rows, cx);
        cx.notify();
    }

    /// Wire the workspace to the foreground leaf: the `ThreadEvent` stream
    /// drives the conversation surface, and the entity-level observe repaints
    /// on projection-only frames (the mid-session chip updates — see the
    /// `store_observe` field note).
    fn subscribe_thread(&self, cx: &mut Context<Self>) -> (Subscription, Subscription) {
        let store = self
            .chat
            .read(cx)
            .store
            .clone()
            .expect("subscribe_thread requires the foreground store");
        let observe = cx.observe(&store, |this, store, cx| {
            // Identity hand-off: the predecessor's store records the
            // successor when the disposal arrives; the foreground moves onto
            // it at the next render (the switch needs a window).
            if let Some(next) = store.read(cx).store.replaced_by.clone()
                && this
                    .chat
                    .read(cx)
                    .store
                    .as_ref()
                    .is_none_or(|s| s.read(cx).session_id() != next)
            {
                // One-shot: the signal is consumed here, so re-attaching the
                // predecessor later cannot bounce the user back (review #39
                // [sugg] 4).
                store.update(cx, |handle, _| handle.store.replaced_by = None);
                this.chat.update(cx, |chat, cx| {
                    chat.pending_successor = Some(next);
                    cx.notify();
                });
            }
            cx.notify();
        });
        let events = cx.subscribe(&store, |this, _store, ev: &ThreadEvent, cx| {
            match ev {
                ThreadEvent::ToolCallAuthorization {
                    id,
                    tool_name,
                    summary,
                    input,
                } => {
                    // AskUserQuestion-shaped payloads surface as the question
                    // card; everything else (a sandbox_permissions escalation
                    // from Edit/Write, a malformed ask) surfaces as the
                    // generic approval card — a parked thread must never wait
                    // invisibly.
                    //
                    // Idempotent per (kind, id): the gateway re-delivers a
                    // parked question on re-own (§D.6 replay), and re-churning
                    // the card state here would wipe the walk a user is
                    // mid-way through. Only the card row is re-adoption-safe
                    // to (re-)ensure.
                    if this
                        .chat
                        .read(cx)
                        .pending_ask
                        .as_ref()
                        .is_some_and(|a| &a.id == id)
                        || this
                            .chat
                            .read(cx)
                            .pending_auth
                            .as_ref()
                            .is_some_and(|a| &a.id == id)
                    {
                        if this.chat.read(cx).pending_ask.is_some() {
                            this.ensure_ask_tool_item(id, summary, input.clone(), cx);
                        }
                        return;
                    }
                    this.chat.update(cx, |chat, cx| {
                        chat.pending_ask = parse_pending_ask(id.clone(), input.clone());
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cc| {
                        chat.pending_auth = chat.pending_ask.is_none().then(|| PendingAuth {
                            id: id.clone(),
                            tool_name: tool_name.clone(),
                            summary: summary.clone(),
                        });
                        cc.notify();
                    });
                    // Arming is per-card: a fresh adjudication has never been
                    // confirmed in the leaf's `pending_auth` projection, so it
                    // must not inherit the previous card's armed flag — mere
                    // absence right after a Request frame is exactly the race
                    // the arming guard exists for.
                    this.chat.update(cx, |chat, cx| {
                        chat.pending_projection_confirmed = false;
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cx| {
                        chat.ask_step = 0;
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cc| {
                        chat.ask_transition_gen = chat.ask_transition_gen.wrapping_add(1);
                        cc.notify();
                    });
                    this.reset_ask_custom(cx);
                    // Live synthesis: the question card appears in the
                    // transcript on the same edge as the pending state —
                    // never waiting on the journal's `tool_use` fold to
                    // arrive through the stream (when it lagged or lost the
                    // race against a thread switch, the ask was unrenderable
                    // and the turn deadlocked).
                    if this.chat.read(cx).pending_ask.is_some() {
                        this.ensure_ask_tool_item(id, summary, input.clone(), cx);
                    }
                    this.chat_rail(cx).update(cx, |r, cx| {
                        r.cockpit_phase = CockpitPhase::AwaitingApproval;
                        cx.notify();
                    });
                    // U3a: the pending-auth badge is the server pump's
                    // store write + SessionStatus delta (single writer,
                    // §F.2) — a parked thread blocked on this authorization
                    // keeps its badge through the pump, not through a
                    // desktop mirror write that only raced it.
                    cx.notify();
                }
                ThreadEvent::PlanModeChanged { .. } => {
                    // Refresh the plan chip.
                    cx.notify();
                }
                ThreadEvent::PlanUpdated { snapshot } => {
                    // Live plan progress: mirror onto the rail as the model
                    // publishes it (an empty snapshot clears the section).
                    let snapshot = snapshot.clone();
                    this.chat_rail(cx)
                        .update(cx, |r, cx| r.set_plan(snapshot, cx));
                }
                ThreadEvent::PermissionModeChanged { .. } => {
                    // Refresh the access chip; no conversation item.
                    cx.notify();
                }
                ThreadEvent::BrowserSuitesChanged { suites } => {
                    // The composer chips are derived state of the thread's
                    // suite mirror (survives thread switches and restores).
                    this.chat.update(cx, |chat, cx| {
                        chat.active_browser_suites = suites.clone();
                        cx.notify();
                    });
                    cx.notify();
                }
                // v2 (T10c, §D.1): the follow stream's authoritative history
                // boundary (`WindowChange::Replace` from the opening
                // Snapshot / a seamless re-open) re-arms the conversation
                // rebuild — the role the deleted `ThreadHistory` note's
                // `restored` flag used to play. Without this a reopened
                // thread strands on the loading screen; a window change that
                // rewrites the branch is corrected to the authoritative
                // active branch here.
                ThreadEvent::HistoryRestored => {
                    this.rebuild_conversation_from_thread(cx);
                    // Re-seed the rail's plan now that the authoritative
                    // transcript has landed: the attach-time seed ran against
                    // an empty transcript and an unfilled sidecar mirror
                    // (`Ready` is async), so a restarted session's plan would
                    // otherwise wait for a manual thread switch.
                    let messages = this
                        .chat
                        .read(cx)
                        .store
                        .as_ref()
                        .map(|s| s.read(cx).store.derived_messages())
                        .expect("foreground store present");
                    if let Some(snapshot) = manox_agent::plan::rebuild_from_messages(&messages)
                        .or_else(|| {
                            this.chat
                                .read(cx)
                                .store
                                .as_ref()
                                .and_then(|s| s.read(cx).store.persisted_plan.as_ref())
                                .and_then(|v| {
                                    serde_json::from_value::<manox_agent::plan::PlanSnapshot>(
                                        v.clone(),
                                    )
                                    .ok()
                                })
                        })
                    {
                        this.chat_rail(cx)
                            .update(cx, |r, cx| r.set_plan(snapshot, cx));
                    }
                }
                ThreadEvent::ModelChanged { from, to } => {
                    // Persist a model_change event to the thread's event stream.
                    // The conversation view itself stays unchanged (no item).
                    let _ = (from, to);
                    cx.notify();
                }
                ThreadEvent::ReasoningEffortChanged { .. } => {
                    // Persist effort change to the thread record immediately.
                    cx.notify();
                }
                ThreadEvent::TokenUsageUpdated(_) => {
                    cx.notify();
                }
                ThreadEvent::TurnStarted => {
                    // U3a: the running indicator is the server pump's store
                    // write + SessionStatus delta (single writer) — it still
                    // lights before the first streaming delta arrives (the
                    // pump sees TurnStarted off the same facade broadcast).
                    // Drive the Thinking status row's per-second "for Xs"
                    // counter while this turn is live. The ticker polls
                    // `turn_active` and self-terminates on the terminal stop.
                    this.chat.update(cx, |chat, cx| {
                        chat.turn_active = true;
                        cx.notify();
                    });
                    this.spawn_thinking_ticker(cx);
                }
                ThreadEvent::UserRowLanded { message_id } => {
                    // The steer's injected `user` row landed in the
                    // transcript: the model has consumed it (dsh's `claimed`
                    // instant). Retire the matching card NOW — a message the
                    // model already saw must not linger in the queue until
                    // the turn boundary. No-op for ordinary prompt rows
                    // (nothing carries their id), and the settle path below
                    // stays the fallback for a row that raced it.
                    this.retire_injected_steer(message_id, cx);
                }
                ThreadEvent::TurnFinished {
                    cancelled,
                    failed,
                    stranded_steer_ids,
                    ..
                } => {
                    // Seal the conversation's streaming state at the
                    // authoritative turn boundary: a turn that ended without
                    // a terminal `Stop` (provider error, stream closed without
                    // `MessageStop`) would otherwise leave its activity
                    // segment accepting entries — a perpetual spinner and the
                    // root condition for the next turn's thinking folding into
                    // a segment above the new user bubble.
                    let _weak = cx.weak_entity();
                    let role = this.model_label(cx);
                    let cwd = thread_cwd(&this.chat_thread(cx), &this.chat_store(cx), cx);
                    let outcome = this.chat_conversation(cx).update(cx, |c, cx| {
                        c.apply(
                            ev,
                            &role,
                            None,
                            crate::conversation::ApplyCtx {
                                host: this.chat.read(cx).host.clone(),
                                cwd,
                                fork_source: this.fork_source_session(cx),
                            },
                            cx,
                        )
                    });
                    this.apply_list_outcome(outcome, cx);
                    // The server's per-id verdict: only the not-yet-injected
                    // tail of the steer group is retracted (`stranded_steer_ids`,
                    // FIFO), the rest was injected and its rows are on disk. The
                    // claim path (`UserRowLanded`) usually retired those already;
                    // this settle is the fallback for a row that raced it — and
                    // it must NOT fail the injected subset (a retry of one would
                    // double-deliver). A normal settle carries zero stranded.
                    let stranded = if *cancelled || *failed {
                        stranded_steer_ids.len()
                    } else {
                        0
                    };
                    this.settle_steer_group(stranded, cx);
                    let thread_id = this
                        .chat
                        .read(cx)
                        .store
                        .as_ref()
                        .map(|s| s.read(cx).store.id.0.clone())
                        .expect("foreground store present");
                    // Cross-domain #5: the wire refetch replaces the kernel
                    // rescan trigger — the server self-holds the scan in its
                    // ListThreads answer.
                    this.multiplexer.update(cx, |m, _| m.fetch_thread_list());
                    // U3a: the settle flags (idle / pending-plan / errored)
                    // are the server pump's store writes — one writer, no
                    // race with this former mirror. The pump clears
                    // pending-plan unconditionally at settle; the review
                    // card's own demote below is UI state, not a store flag.
                    this.chat.update(cx, |chat, cx| {
                        chat.turn_active = false;
                        cx.notify();
                    });
                    this.background_threads.retain(|b| b.id != thread_id);
                    this.spawn_git_status_refresh(cx);
                    // Dispatch last: `run_turn` emits `TurnStarted`
                    // synchronously, so no terminal bookkeeping above may run
                    // afterward and overwrite the new turn's running state.
                    if !cancelled {
                        this.flush_queued_follow_ups(cx);
                    }
                    cx.notify();
                }
                ThreadEvent::Stop(reason) => {
                    let _weak = cx.weak_entity();
                    let role = this.model_label(cx);
                    let usage = this.chat.read(cx).store.as_ref().and_then(|s| {
                        s.read(cx).store.last_token_usage.as_ref().map(|u| {
                            manox_agent::TokenUsage {
                                input_tokens: u.input,
                                output_tokens: u.output,
                                cache_creation_input_tokens: u.cache_creation,
                                cache_read_input_tokens: u.cache_read,
                            }
                        })
                    });
                    let cwd = thread_cwd(&this.chat_thread(cx), &this.chat_store(cx), cx);
                    let outcome = this.chat_conversation(cx).update(cx, |c, cx| {
                        c.apply(
                            ev,
                            &role,
                            usage,
                            crate::conversation::ApplyCtx {
                                host: this.chat.read(cx).host.clone(),
                                cwd,
                                fork_source: this.fork_source_session(cx),
                            },
                            cx,
                        )
                    });
                    this.apply_list_outcome(outcome, cx);
                    // `Stop` flips streaming flags off, so finalized bodies switch
                    // to full `Markdown` layout and may grow a frame or two later;
                    // the list's Absolute scroll anchor holds the viewport steady
                    // across that growth, and `FollowMode::Tail` — if still
                    // engaged — re-pins to the end on the next layout.
                    // Persist on terminal state (not the ToolUse mid-state).
                    if !matches!(reason, StopReason::ToolUse) {
                        this.multiplexer.update(cx, |m, _| m.fetch_thread_list());
                        // `Stop` is a provider-round boundary. Queue draining,
                        // idle state, and git refresh wait for `TurnFinished`.
                    }
                    cx.notify();
                }
                ThreadEvent::PrefixStability { .. } => {
                    // Per-turn cache stability signal. The composer chip that
                    // used to render this was removed in #62; the event stays
                    // emitted for any future telemetry/debug subscriber.
                    cx.notify();
                }
                ThreadEvent::GoalChanged { goal } => {
                    // Bump the ticker generation so any prior ticker
                    // self-terminates; start a fresh ticker only on activation.
                    let active = goal
                        .as_ref()
                        .map(|g| !g.status.is_terminal())
                        .unwrap_or(false);
                    this.chat.update(cx, |chat, cc| {
                        chat.goal_ticker_gen = chat.goal_ticker_gen.wrapping_add(1);
                        cc.notify();
                        chat.goal_ticker_gen
                    });
                    if active {
                        let entity = cx.entity().clone();
                        let ticker_gen = this.chat.read(cx).goal_ticker_gen;
                        cx.spawn(async move |_this, cx| {
                            loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_secs(1))
                                    .await;
                                let still = entity.read_with(cx, |this, cx| {
                                    this.chat.read(cx).goal_ticker_gen == ticker_gen
                                        && this
                                            .chat
                                            .read(cx)
                                            .store
                                            .as_ref()
                                            .and_then(|s| s.read(cx).store.goal.clone())
                                            .and_then(|v| {
                                                serde_json::from_value::<
                                                    manox_agent::goal::ThreadGoal,
                                                >(v)
                                                .ok()
                                            })
                                            .is_some()
                                });
                                if !still {
                                    break;
                                }
                                entity.update(cx, |_, cx| cx.notify());
                            }
                        })
                        .detach();
                    }
                    cx.notify();
                }
                // The v2 link never delivers `ThreadEvent::SteerInjected`: the
                // facade emits it (manox thread.rs `BackendNotice::Settled` →
                // push per steered id) but the journal→wire translation drops
                // it, so the engine-less render mirror never sees it. Steer
                // outcomes are driven entirely by the `TurnFinished` settle
                // boundary above; any stray `SteerInjected` falls through to the
                // generic `apply` (a no-op).
                _ => {
                    // U3b: the background-work flag is the server pump's
                    // store write + delta (its BackgroundTaskUpdated arm
                    // computes the same thread_has_running_tasks); the
                    // event still falls through to the conversation's
                    // task-card dispatch below.
                    // U3b: the pending-auth badge drops at VERDICT time on
                    // the server (clear_pending_auth_if_settled at all four
                    // settle points + the §D.5 delta). The tool-traffic
                    // heuristic this replaces only ran in-proc, only for
                    // this client, and raced the actual verdict.
                    // `Error` is a terminal signal symmetric to a terminal
                    // `Stop`: the turn aborted, so this thread is no longer
                    // running. Pulled out of the catch-all rather than given a
                    // dedicated arm because the conversation still needs the
                    // generic `apply` below to render the error item.
                    if let ThreadEvent::Error(e) = ev {
                        let thread_id = this
                            .chat
                            .read(cx)
                            .store
                            .as_ref()
                            .map(|s| s.read(cx).store.id.0.clone())
                            .expect("foreground store present");
                        // U3b: the Error edge is the server pump's —
                        // mark_idle, the errored flag and the full badge
                        // clear ride its store write + the single delta
                        // carrying the whole set. GW5 kept: no unread rise
                        // for the FOREGROUND error — the user is watching
                        // it; the errored triangle is the signal
                        // (client-owned unread, §F.2).
                        this.chat.update(cx, |chat, cx| {
                            chat.turn_active = false;
                            cx.notify();
                        });
                        this.background_threads.retain(|b| b.id != thread_id);
                        // Persist the error card so a reloaded thread reproduces
                        // what went wrong at the failed turn's position. The
                        // append rides the actor queue behind the settling run;
                        // a crash before the actor drains it loses the card
                        // (accepted window for an annotation).
                        this.append_ui_note(
                            manox_agent::db::UiNoteKind::Error,
                            e.to_string(),
                            None,
                            cx,
                        );
                        // The run task emits `TurnFinished` after it has cleared
                        // `running_turn`; queue recovery and follow-up dispatch
                        // happen there.
                    }
                    // Cockpit phase tracking for the streaming/tool variants
                    // that flow through this generic arm. `Error` is handled
                    // above; `CompactionStarted` flips Summarizing, `Compaction`
                    // flips back to Streaming; a `Running` tool call caches its
                    // title and flips RunningTool; other tool statuses return to
                    // Streaming; text/thinking deltas mark Thinking/Streaming.
                    this.chat_rail(cx)
                        .update(cx, |r, cx| r.update_cockpit_phase(ev, cx));
                    // Sub-agent observation: the pi harness observes its
                    // ephemeral nested sessions through progress events on
                    // the rail (the retired manox harness tracked child
                    // threads in observation panels instead).
                    if let ThreadEvent::SubagentProgress {
                        id,
                        subagent_type,
                        latest_activity,
                        status,
                        health,
                        ..
                    } = ev
                    {
                        let id = id.clone();
                        let subagent_type = subagent_type.clone();
                        let latest_activity = latest_activity.clone();
                        let health = health.clone();
                        this.chat_rail(cx).update(cx, |r, cx| {
                            r.apply_subagent_progress(
                                &id,
                                &subagent_type,
                                latest_activity.as_deref(),
                                *status,
                                health.as_deref(),
                                cx,
                            );
                        });
                        // Record the completion text so a panel opened later
                        // (after the Agent tool-result is gone) can show it.
                        if matches!(
                            *status,
                            manox_agent::ToolCallStatus::Success
                                | manox_agent::ToolCallStatus::Error
                                | manox_agent::ToolCallStatus::Denied
                        ) && let Some(text) = &latest_activity
                        {
                            this.subagent_final_text.insert(id.clone(), text.clone());
                        }
                        if let Some(panel) = this.subagent_panels.get(&id) {
                            panel.update(cx, |p, cx| p.set_status(*status, cx));
                        }
                    }
                    if let ThreadEvent::SubagentChild { id, child } = ev {
                        this.subagent_transcripts
                            .entry(id.clone())
                            .or_default()
                            .push(child.clone());
                        if let Some(panel) = this.subagent_panels.get(id) {
                            panel.update(cx, |p, cx| p.push(child, cx));
                        }
                    }
                    // Capture the Captain's dispatch prompt from the Steer
                    // tool call so the subagent panel can show the opening
                    // user message even before the child streams anything.
                    if let ThreadEvent::ToolCall { name, input, .. } = ev
                        && name == "Steer"
                        && let Some(args) = input
                    {
                        // Only a Dispatch (to.spawn set) establishes the
                        // opening prompt; a later Inject must not overwrite the
                        // panel's first user bubble with a mid-run message.
                        let is_dispatch = args.get("to").and_then(|t| t.get("spawn")).is_some();
                        let addr = args
                            .get("to")
                            .and_then(|t| t.get("agent_address"))
                            .and_then(|v| v.as_str());
                        let prompt = args.get("prompt").and_then(|v| v.as_str());
                        if is_dispatch && let (Some(addr), Some(prompt)) = (addr, prompt) {
                            this.subagent_prompts.insert(
                                addr.to_string(),
                                SubagentPrompt {
                                    text: prompt.to_string(),
                                    dispatched_at: chrono::Utc::now().timestamp(),
                                },
                            );
                        }
                    }
                    let _weak = cx.weak_entity();
                    let role = this.model_label(cx);
                    let usage = this.chat.read(cx).store.as_ref().and_then(|s| {
                        s.read(cx).store.last_token_usage.as_ref().map(|u| {
                            manox_agent::TokenUsage {
                                input_tokens: u.input,
                                output_tokens: u.output,
                                cache_creation_input_tokens: u.cache_creation,
                                cache_read_input_tokens: u.cache_read,
                            }
                        })
                    });
                    let cwd = thread_cwd(&this.chat_thread(cx), &this.chat_store(cx), cx);
                    let outcome = this.chat_conversation(cx).update(cx, |c, cx| {
                        c.apply(
                            ev,
                            &role,
                            usage,
                            crate::conversation::ApplyCtx {
                                host: this.chat.read(cx).host.clone(),
                                cwd,
                                fork_source: this.fork_source_session(cx),
                            },
                            cx,
                        )
                    });
                    this.apply_list_outcome(outcome, cx);
                    cx.notify();
                }
            }
        });
        (events, observe)
    }

    fn subscribe_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        let sidebar = self.sidebar.clone();
        cx.subscribe_in(
            &sidebar,
            window,
            |this, _sidebar, ev: &SidebarEvent, window, cx| match ev {
                SidebarEvent::NewThread => this.start_new_thread(None, window, cx),
                SidebarEvent::NewThreadWithProject(dir) => {
                    this.start_new_thread(Some(dir.clone()), window, cx);
                }
                SidebarEvent::OpenThread(id) => this.open_thread(id.clone(), window, cx),
                SidebarEvent::SpawnExternalSession(kind, provider, model, wire, project) => {
                    this.spawn_external_session(
                        ExternalSpawn {
                            kind: *kind,
                            provider_name: provider.clone(),
                            model_id: model.clone(),
                            wire_api: wire.clone(),
                            project_cwd: project.clone(),
                        },
                        SessionPlacement::FullWindow,
                        window,
                        cx,
                    );
                }
                SidebarEvent::SpawnPlainSession(kind, project) => {
                    this.spawn_plain_session(
                        *kind,
                        project.clone(),
                        SessionPlacement::FullWindow,
                        window,
                        cx,
                    );
                }
                SidebarEvent::LaunchVSCode(project) => {
                    // VS Code opens the project directory the menu was launched
                    // from; from the Conversations header (no project) it
                    // falls back to the workspace cwd — the same directory a
                    // fresh session runs in. Injection targets come from the
                    // persisted `vscode_app:` settings (no launch-time choice).
                    let folder = project.clone().unwrap_or_else(|| this.cwd.clone());
                    this.launch_vscode_app(Some(folder), window, cx);
                }
                SidebarEvent::OpenExternalSession(id) => {
                    this.open_external_session(id, window, cx);
                }
                SidebarEvent::ArchiveExternalSession(id) => {
                    this.close_external_session(id, cx);
                }
                SidebarEvent::ArchiveThread(id, archived) => {
                    let is_current = this
                        .chat
                        .read(cx)
                        .store
                        .as_ref()
                        .map(|s| s.read(cx).store.id.0 == *id)
                        .expect("foreground store present");
                    let store = manox_agent::thread_store_global();
                    store.with_mut(|s| s.archive_thread(id, *archived));
                    // Sync the in-memory flag so the title-bar menu label stays
                    // fresh when the sidebar archives the currently active thread.
                    if is_current {
                        let _ =
                            this.send_note(cx, |sid| manox_protocol::ClientNote::ArchiveThread {
                                session_id: sid.into(),
                                archived: *archived,
                            });
                    }
                    // Archiving the active thread navigates away to a fresh
                    // empty thread (Hero view) so the user doesn't stare at a
                    // ghost conversation that just vanished from the sidebar.
                    if *archived && is_current {
                        this.start_new_thread(None, window, cx);
                    }
                }
                SidebarEvent::SetThreadTag(id, tag) => {
                    let store = manox_agent::thread_store_global();
                    store.with_mut(|s| s.set_thread_tag(id, tag.clone()));
                }
                // Sidebar order is the server's durable manual account, so a
                // move rides the gateway rather than an in-process store write
                // (the same-face rule the archive/tag migrations converged on).
                // These notes carry no session: they address a thread and a
                // folder, not the landing conversation.
                SidebarEvent::MoveThread { id, before_id } => {
                    // These notes address no session, so they go straight to the
                    // shared client rather than the session-scoped helper.
                    this.client
                        .send_note(manox_protocol::ClientNote::InsertThreadBefore {
                            thread_id: id.clone(),
                            before_thread_id: before_id.clone(),
                        });
                }
                SidebarEvent::MoveFolder { path, before_path } => {
                    this.client
                        .send_note(manox_protocol::ClientNote::InsertGroupBefore {
                            path: path.to_string_lossy().into_owned(),
                            before_path: before_path
                                .as_ref()
                                .map(|p| p.to_string_lossy().into_owned()),
                        });
                }
                SidebarEvent::RemoveProject(path) => {
                    // Unregister the folder; the sidebar drops the group and
                    // its threads fall back to the loose Conversations list.
                    // Conversation history is never touched.
                    let store = manox_agent::thread_store_global();
                    store.with_mut(|s| s.remove_project(&path.to_string_lossy()));
                }
            },
        )
    }

    /// Switch into the Settings overlay. The Settings view is created lazily on
    /// first entry; from then on the entity + subscription are reused so the
    /// user's last selection (and any scroll position) survives re-entry.
    pub fn enter_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_view.is_none() {
            let settings = cx.new(|cx| SettingsView::new(self.sidebar_width, window, cx));
            let sub = self.subscribe_settings(&settings, cx);
            self.settings_view = Some(settings);
            self.settings_sub = Some(sub);
        } else if let Some(settings) = self.settings_view.as_ref() {
            // Re-entry after a divider resize in the app page: the settings
            // nav follows the shared sidebar width.
            settings.update(cx, |s, cx| s.set_width(self.sidebar_width, cx));
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
        self.multiplexer.read(cx).attention_count(cx)
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

    /// Submit the right-side editor on Cmd/Ctrl-Enter (`InputEvent::PressEnter`
    /// with `secondary` set). Plain Enter inserts a newline (submit_on_enter
    /// is off for the panel editor).
    fn subscribe_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> Subscription {
        let editor = self.editor_state.clone();
        cx.subscribe_in(&editor, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary, shift } = ev
                && *secondary
                && !shift
            {
                this.submit_editor(window, cx);
            }
        })
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
                    slash_source(&det.query, self.multiplexer.read(cx).commands())
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
            || chat.pending_auth.is_some()
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
                    this.reveal_message(target, window, cx);
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
    fn apply_list_outcome(&mut self, outcome: ApplyOutcome, cx: &mut App) {
        let count_changed = self.sync_list_count(cx);
        match outcome {
            ApplyOutcome::Remeasure(ix) => {
                self.chat.read(cx).list_state.remeasure_items(ix..ix + 1)
            }
            ApplyOutcome::RemeasureAll => self.chat.read(cx).list_state.remeasure(),
            // Remeasure the just-mutated segment (e.g. an activity segment
            // closed for an incoming reply) in addition to the append splice
            // `sync_list_count` already performed. When the append was net-
            // neutralized by a trailing `Retry` pop (count unchanged → no
            // splice), the new assistant bubble occupies a reused `Measured`
            // tail slot whose cached height is the popped retry badge's, so
            // remeasure the tail too. (When `popped_retry` was false the push
            // grew the count by one, `count_changed` is true, and the splice
            // already inserted the new bubble as `Unmeasured` — so the tail
            // remeasure is skipped as redundant, not because the branch is
            // dead.)
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
            // `Unchanged` touched no item; `Appended`/`RemovedTail` only changed
            // the count, which `sync_list_count` already spliced.
            ApplyOutcome::Unchanged | ApplyOutcome::Appended | ApplyOutcome::RemovedTail => {}
        }
    }

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
    fn reveal_message(&mut self, item_ix: usize, _window: &mut Window, cx: &mut Context<Self>) {
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
        window: &mut Window,
        theme: &Theme,
        right_pane_open: bool,
        show_context_rail: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let navigator = self.chat.read(cx).turn_navigator.clone()?;
        let layout = turn_navigator_layout(
            window.bounds().size.width,
            self.effective_sidebar_width(),
            right_pane_open.then_some(self.editor_width),
            show_context_rail,
        );
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
                        // The card (and its title bar) starts below the shell
                        // gutter, so the panel clears them from the window top.
                        .pt(px(SHELL_PAD_EDGE) + TITLE_BAR_HEIGHT + px(8.0))
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
            .map(|s| std::path::PathBuf::from(s.read(cx).store.cwd.clone()))
            .expect("foreground store present");
        let worktree_branch = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .and_then(|s| s.read(cx).store.with(|st| st.branch.clone()));
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
            let (stats, display) = match result {
                Some(v) => (Some(v.0), Some(v.1)),
                None => (None, None),
            };
            rail.update(cx, |r, cx| r.set_git_status(stats, display, cx));
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
            .map(|s| s.read(cx).session_id().to_string())
    }

    fn user_turn_meta(&self, cx: &mut Context<Self>) -> UserTurnMeta {
        let permission_mode = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.permission_mode)
            .expect("foreground store present");
        UserTurnMeta::new(
            chrono::Utc::now().timestamp(),
            self.model_label(cx),
            Some(permission_mode),
        )
    }

    pub(crate) fn model_label(&self, cx: &mut Context<Self>) -> String {
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
                .and_then(|s| s.read(cx).store.with(|st| st.model_id.clone()))
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
        let _ = self.send_note(cx, |sid| manox_protocol::ClientNote::AppendUiNote {
            session_id: sid.into(),
            kind: kind_str.into(),
            data: data.clone(),
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
        if !self.send_note(cx, |sid| manox_protocol::ClientNote::CancelTurn {
            session_id: sid.into(),
        }) {
            tracing::warn!("CancelTurn dropped: the active leaf has no bound session");
        }
        cx.notify();
    }

    /// Send a `ClientNote` to the AgentServer when the landing-thread
    /// connection is available (γ-3 mutation path). Returns `true` when the
    /// note was sent; the caller falls back to `self.chat.thread.update` when `false`.
    pub(crate) fn send_note(
        &self,
        cx: &App,
        note_fn: impl FnOnce(&str) -> manox_protocol::ClientNote,
    ) -> bool {
        if let Some(sid) = &self.chat.read(cx).session_id {
            self.client.send_note(note_fn(sid));
            true
        } else {
            false
        }
    }

    /// v2 §D.2 submit path: mint an `origin_rpc` correlation id, register the
    /// optimistic echo in the foreground store, and send the
    /// [`ClientCall::Submit`] (receipt-only per L7 — the durable user row
    /// arrives through the follow stream and retires the echo by matching its
    /// `originRpc`). The conversation's optimistic bubble was already pushed by
    /// the caller; retirement just clears the store's echo bookkeeping so the
    /// row is not treated as a remote (unmatched) insertion.
    ///
    /// Returns `true` when the submit rode the connection (session bound).
    pub(crate) fn send_submit_v2(
        &mut self,
        text: String,
        images: Vec<manox_protocol::ImageAttachment>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(sid) = self.chat.read(cx).session_id.clone() else {
            tracing::warn!("submit dropped: no session bound to the workspace");
            return false;
        };
        tracing::info!(session_id = %sid, "submit v2 sent");
        let origin_rpc = uuid::Uuid::new_v4().to_string();
        if let Some(store) = self.chat_store(cx) {
            store.update(cx, |h, _| {
                h.store.push_echo(&origin_rpc, text.clone());
            });
        }
        self.client.send_call(manox_protocol::ClientCall::Submit {
            session_id: sid,
            text,
            images,
            origin_rpc: Some(origin_rpc),
        });
        true
    }

    /// v2 §D.2 steer path: hand a message to the running turn's server-side
    /// steer queue. The desktop's `Workspace.thread` is an engine-less render
    /// mirror, so the old `thread.enqueue_steer` only inserted a local id and
    /// never reached the server — a dead end where the card sat forever and the
    /// message was neither injected nor confirmed. This sends the real
    /// [`manox_protocol::ClientCall::Steer`] (server: enqueue while running,
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
        images: Vec<manox_protocol::ImageAttachment>,
    ) -> bool {
        let Some(sid) = self.chat.read(cx).session_id.clone() else {
            tracing::warn!("steer dropped: no session bound to the workspace");
            return false;
        };
        tracing::info!(session_id = %sid, "steer v2 sent");
        self.client.send_call(manox_protocol::ClientCall::Steer {
            session_id: sid,
            message_id,
            text,
            images,
            origin_rpc: None,
        });
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
            .map(|s| s.read(cx).store.permission_mode)
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
        let _ = self.send_note(cx, |sid| manox_protocol::ClientNote::SetApprovalMode {
            session_id: sid.into(),
            mode: mode_wire,
        });
        // Optimistic mirror: the chip reflects the click now; the journal
        // echo lands later (turn end at the latest) and confirms it.
        if let Some(store) = self.chat.read(cx).store.clone() {
            store.update(cx, |leaf, _| leaf.set_permission_mode_optimistic(mode));
        }
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
