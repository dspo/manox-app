//! The chat column's state, carved out of `Workspace`
//! (PLAN-CHROME-CHAT-SPLIT Phase 1). Everything the conversation column owns
//! lives here — the thread face + journal mirror, composer, ask drawer,
//! follow-up queue, completion, turn navigator, context rail, and the
//! message-list virtualization. The shell (sidebar / right pane / view
//! modes / external sessions) stays on `Workspace`; `Workspace` orchestrates
//! both sides, so this is a plain embedded struct for now — the entity
//! boundary and the `ChatHost` port arrive with the crate split.

use super::*;

pub(crate) struct ChatColumn {
    /// The port the moved chat views call through (see `host.rs` in
    /// manox-agent-chat-ui and agent-ui's `WorkspaceChatHost`).
    pub(crate) host: manox_agent_chat_ui::host::ChatHostHandle,

    pub(crate) thread: manox_agent::thread::ThreadHandle,
    /// The `AgentServer`-backed `ClientStoreHandle` — the v2 `SessionStore`
    /// (journal window + projection face + echo map) fed by the multiplexer's
    /// follow stream. `None` until the workspace creates the AgentServer
    /// connection (landing thread); views read the store mirror. Held on
    /// the workspace for the next wiring step (re-handling the store on
    /// thread switch) — written at landing, read there.
    pub(crate) store: Option<gpui::Entity<ClientStoreHandle>>,
    /// γ-3: the AgentServer session_id for the landing thread. Used as the
    /// `session_id` field in `FromClient` commands.
    pub(crate) session_id: Option<String>,
    /// Generation counter for git-status refreshes: bumping it means any
    /// prior in-flight refresh self-cancels instead of overwriting newer
    /// state. The refresh runs on the global tokio runtime and delivers its
    /// result back via `async_channel`, the same bridge the worktree tool uses.
    pub(crate) git_status_gen: u64,
    pub(crate) conversation: Entity<ConversationState>,
    pub(crate) input_state: Entity<TextareaState>,
    /// Per-thread unsent composer text, keyed by thread id. Saved when
    /// switching away and restored on return, so each thread keeps its own
    /// in-progress draft instead of a single shared input bleeding across.
    pub(crate) drafts: HashMap<String, String>,
    /// Composer history-recall position into the newest-first user-turn texts;
    /// -1 means the walk is not running. Only `alt-up` / `alt-down` move along
    /// it, so nothing about the text or the caret has to be watched to leave it.
    pub(crate) recall_index: i64,
    /// The walk's working line: what the composer held when the walk was
    /// entered, or the last recalled turn once the user has changed it. `Down`
    /// past the newest turn restores it and ends the walk.
    pub(crate) recall_draft: Option<String>,
    /// A pending `AskUserQuestion` card rendered inline in the message list.
    pub(crate) pending_ask: Option<PendingAsk>,
    pub(crate) pending_auth: Option<PendingAuth>,
    /// Whether the CURRENT pending interaction's id has been observed in the
    /// leaf store's `pending_auth` projection set. Arms the remote-settle
    /// reconcile (chips.rs) so the startup race — Request landing before the
    /// projection frame — can never clear a freshly surfaced card.
    pub(crate) pending_projection_confirmed: bool,
    /// Tool row currently carrying the Workspace-derived ask snapshot. This is
    /// synchronized before list construction; the row factory itself remains
    /// a read-only projection during measurement and prepaint.
    pub(crate) ask_snapshot_item: Option<Entity<MessageItem>>,
    /// Current question index in the ask drawer (0-based).
    pub(crate) ask_step: usize,
    /// Animation generation counter for the ask drawer slide, bumped on every
    /// open/close so a fresh tween fires rather than replaying a cached delta.
    pub(crate) ask_transition_gen: u64,
    /// Per-question free-text `custom` inputs for the pending ask card, one
    /// slot per question (index-aligned with `pending_ask.questions`). Created
    /// lazily at render time because an `InputState` needs a `Window`, which
    /// the park event handler lacks; reset whenever the ask is (re)seeded or
    /// retired. A tri-state answer is `{selected, custom}` — `custom` overrides
    /// a single-select and supplements a multi-select at the settle fold, and
    /// an empty selection with no `custom` is an explicit skip.
    pub(crate) ask_custom_inputs: Vec<Option<Entity<InputState>>>,
    /// Subscriptions keeping the card repainted as the custom inputs change.
    pub(crate) ask_custom_subs: Vec<Subscription>,
    /// Authoritative per-question custom text (index-aligned with
    /// `pending_ask.questions`), the value `resolve_ask` folds
    /// into each canonical `AskAnswer`. The `InputState` entities above mirror
    /// this for live editing; tests drive this directly. Reset with the ask.
    pub(crate) ask_custom_text: Vec<String>,
    /// Per-question explicit-skip markers (index-aligned with
    /// `pending_ask.questions`): set by the footer Skip, cleared with the ask.
    /// The submit completeness gate reads them — an untouched question is
    /// never silently folded to a skip, but an explicitly skipped one counts
    /// as completed (dsh `QuestionDraftAnswer.skipped` parity).
    pub(crate) ask_skipped: Vec<bool>,
    pub(crate) model_open: bool,
    /// PopupMenu entity for the open model selector; created on open, destroyed on close.
    pub(crate) model_menu: Option<Entity<PopupMenu>>,
    pub(crate) model_menu_sub: Option<Subscription>,
    pub(crate) plus_open: bool,
    pub(crate) plus_menu: Option<Entity<PopupMenu>>,
    pub(crate) plus_menu_sub: Option<Subscription>,
    /// Access-chip dropdown (permission modes). Mirrors the model selector pattern.
    pub(crate) access_open: bool,
    /// Project-chip dropdown (recent projects + new project submenu).
    pub(crate) project_chip_open: bool,
    pub(crate) project_chip_menu: Option<Entity<PopupMenu>>,
    pub(crate) project_chip_menu_sub: Option<Subscription>,
    /// Composer typeahead completion popover (`/` commands, `@` skills/agents).
    /// `None` when no trigger token is active at the caret. A pure render
    /// overlay — it never grabs focus, so the `InputState` keeps focus and the
    /// query filters live on every keystroke.
    pub(crate) completion: Option<CompletionState>,
    /// Searchable, newest-first snapshot of the active thread's user turns.
    pub(crate) turn_navigator: Option<Entity<TurnNavigator>>,
    pub(crate) turn_navigator_sub: Option<Subscription>,
    pub(crate) turn_navigator_previous_focus: Option<FocusHandle>,
    /// Follow-ups submitted while a turn is running. Steer items are injected
    /// into the running turn at the next safe join point; queue items flush as
    /// the next user turn at `TurnFinished`.
    pub(crate) queued_follow_ups: std::collections::VecDeque<QueuedFollowUp>,
    /// Session-only per-thread queue stash. Switching tasks moves the active
    /// deque here and restores it on return; no database persistence is used.
    pub(crate) queued_follow_ups_by_thread:
        HashMap<String, std::collections::VecDeque<QueuedFollowUp>>,
    /// In-flight queue-row drag (the composer queue's grip handle): the
    /// insertion marker cleared on commit / cancel / thread switch. Mirrors
    /// the sidebar's `drag_row` cue for the same gesture.
    pub(crate) queue_drag: Option<composer_render::QueueRowDrag>,
    /// Tracks which composer placeholder is installed, so render only mutates
    /// the input state on mode transitions.
    pub(crate) composer_placeholder_mode: ComposerPlaceholderMode,
    /// Files picked via the `+` menu, not yet sent. Cleared on submit.
    pub(crate) pending_attachments: Vec<PendingAttachment>,
    /// Opt-in browser tool suites activated via the `+` menu. Unlike file
    /// attachments these persist across submits (they track session-level tool
    /// activation); removing a chip deactivates the suite.
    pub(crate) active_browser_suites: Vec<manox_agent::engine::BrowserSuite>,
    /// True while a native directory picker is open from the "Choose project" row.
    /// Guards against the user submitting a message before the picker resolves
    /// (which would make `set_project` a silent no-op once `messages` is non-empty).
    pub(crate) project_picker_pending: bool,
    /// Parent directory selected for "Create blank project"; waiting for name input.
    pub(crate) blank_project_parent: Option<PathBuf>,
    /// Input state for the blank project folder name overlay.
    pub(crate) blank_project_name_input: Option<Entity<InputState>>,
    pub(crate) thread_sub: Option<Subscription>,
    /// Observes the foreground leaf store itself (beyond `thread_sub`'s
    /// events): a projection-only frame (mid-session `SetModel` →
    /// `Projections` delta) writes the chip fields and notifies the LEAF, but
    /// the entry event's re-render can land in an earlier tick — without this
    /// observe the workspace never repaints and the chip stays stale (the
    /// #765 "picks a model, nothing happens" repro: the journal had both
    /// changes, the render never showed them).
    pub(crate) store_observe: Option<Subscription>,
    /// A successor session the foreground must switch to at the next render
    /// (a bind's identity hand-off arrived while this workspace held the
    /// predecessor). Taken once, so the switch cannot re-trigger.
    pub(crate) pending_successor: Option<String>,
    /// A `ForkSession` round trip is outstanding. The fork control is a button
    /// on every forkable reply, and the verdict takes a round trip, so without
    /// this a double click mints two children for one intent. Cleared on both
    /// verdicts so a failed fork stays retryable.
    pub(crate) fork_in_flight: bool,
    pub(crate) input_sub: Option<Subscription>,
    /// Height-invalidation subscription: any `ConversationState` mutation may
    /// change a row's height (including off-screen rows whose height is cached
    /// in the list sum tree). Remeasure all rows on every conversation notify —
    /// the same cure a window resize applies — so a stale cached height can
    /// never survive to paint an overlapping row.
    pub(crate) conversation_sub: Option<Subscription>,
    /// Scroll/virtualization state for the message column, held natively by
    /// `gpui::ListState`. `ListAlignment::Bottom` gives chat-log semantics:
    /// short histories sit at the bottom, long ones scroll. `FollowMode::Tail`
    /// pins to the live end on each layout while following, disengages on an
    /// upward user scroll, and re-arms when a scroll lands back at the bottom.
    /// `MSG_LIST_OVERDRAW` rows below the viewport are pre-measured; a width
    /// change invalidates every cached height, and visible rows re-measure
    /// every frame (so a height change without an explicit signal self-
    /// corrects). Count changes are reconciled via `splice`, in-place
    /// mutations via `remeasure_items`, both driven by `ApplyOutcome`. Only
    /// the visible items render.
    pub(crate) list_state: ListState,
    /// Exact width of the list child from the previous prepaint. Official GPUI
    /// at the pinned revision does not invalidate off-screen row heights when
    /// this changes, so the application explicitly remeasures the cache.
    pub(crate) message_list_width: crate::views::MessageListWidthInvalidator,
    /// Cached `items().len()`; the event handler reconciles the list count via
    /// `splice` whenever the conversation grows or shrinks.
    pub(crate) list_count: usize,
    /// Whether the goal status popover is open (toggled by the `◎ /goal active`
    /// chip or the bare `/goal` command).
    pub(crate) goal_popover_open: bool,
    /// Generation counter for the goal elapsed-time ticker. Incremented when a
    /// goal is cleared or the active thread changes so the prior ticker
    /// self-terminates instead of notifying a stale chip. Mirrors
    /// `settings_transition_gen`.
    pub(crate) goal_ticker_gen: u64,
    /// True while the active thread has a turn in flight, so the Thinking
    /// status row's "for Xs" counter ticks every second. Set on `TurnStarted`,
    /// cleared on a terminal `Stop`/`Error`. The ticker task polls this and
    /// self-terminates when it goes false.
    pub(crate) turn_active: bool,
    /// Generation counter for the thinking elapsed-time ticker. Incremented
    /// on every `TurnStarted` and on thread switch so a prior ticker
    /// self-terminates instead of driving a stale container.
    pub(crate) thinking_ticker_gen: u64,
    /// Right-hand context rail. Owns the cockpit state (run phase, the model's
    /// plan snapshot, per-cell counter animation state) that used to live
    /// directly on `Workspace`, plus strong handles to the active thread and conversation
    /// it renders against. Writes flow through `self.context_rail.update`.
    pub(crate) context_rail: Entity<crate::views::context_rail::ContextRail>,
}
