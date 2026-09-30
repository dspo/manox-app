//! The conversation info bubble: the active thread's cockpit information
//! (run status, subagents, branch/worktree, written plan files, todos,
//! per-model token usage, sources) rendered as a popover above the
//! composer's context-usage ring.
//!
//! The rail entity is the state store the workspace feeds from
//! `ThreadEvent`s and the Q-face info fetches; the bubble is its only
//! render face. The floating top-right card it replaces was the same
//! state drawn as a permanent overlay — the bubble trades the reserved
//! column inset and the width gate for a click-open surface (toggle on
//! the ring, outside click / Escape / thread switch to close; the fold
//! state resets on close).
//!
//! Layout contract (design/conversation-info-bubble): no headings — six
//! segments separated only by 1px hairlines, width
//! `clamp(natural, 260, 360)`, height = content capped by the space above
//! the ring with internal scrolling past that.

use crate::ahp_store::{AhpStore, leaf as leaf_of};
use crate::i18n;
use gpui::{AnyElement, App, Context, Entity, Pixels, SharedString, prelude::*, px};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, Theme, h_flex, v_flex};
use manox_agent::ThreadEvent;
use std::collections::HashMap;
use std::path::PathBuf;

use manox_agent::{PlanSnapshot, PlanStep, PlanStepStatus};

use crate::cockpit::{
    CockpitPhase, cache_read_ratio, context_budget_pct, format_cache_hit, format_tokens_pi,
};
use crate::git_status::GitBranchDisplay;
use crate::views::subagents::{SubagentInfo, status_indicator, task_display_title};

// ── Bubble geometry ──────────────────────────────────────────────────────

/// Bubble width floor: the old card's width, so the information density
/// never regresses below what the permanent overlay showed.
pub const BUBBLE_MIN_W: f32 = 260.;
/// Bubble width ceiling: one overlong branch name or todo must not stretch
/// the bubble across the window.
pub const BUBBLE_MAX_W: f32 = 360.;

/// Occupancy at which warning coloring kicks in — shared with the
/// composer ring (agent-ui imports this; the old agent-ui-private copy
/// forked the threshold).
pub const CONTEXT_NEAR_FULL_PCT: f64 = 90.0;
/// Fold caps — rows shown before a section collapses the rest into `+N`.
const SUBAGENTS_CAP: usize = 5;
const PLANS_CAP: usize = 5;
const TODOS_CAP: usize = 8;
const MODELS_CAP: usize = 5;

/// The main agent's literal label. App chrome is localized, but this one
/// name is a product term rendered verbatim — unlike the message-signature
/// Captain (`views/message.rs`), which keeps its localized key.
const CAPTAIN_LABEL: &str = "Captain";

// ── Fold helpers ─────────────────────────────────────────────────────────

/// A capped list section's fold: the visible row count plus the fold size
/// (what collapsing would hide — the `+N` row's count). `hidden` is
/// non-zero whenever the collapsed view would hide rows, expanded or not:
/// the `+N` toggle must stay reachable to fold back down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldPlan {
    pub visible: usize,
    pub hidden: usize,
}

/// Prefix fold for the plainly-capped sections (plans / todos / models):
/// expanded shows everything; collapsed shows the first `cap` rows. The
/// fold size is expansion-independent.
pub fn fold_window(len: usize, cap: usize, expanded: bool) -> FoldPlan {
    let hidden = len.saturating_sub(cap);
    FoldPlan {
        visible: if expanded { len } else { len - hidden },
        hidden,
    }
}

/// Lifecycle activity, per the `ToolCallStatus` — NOT the watchdog's
/// `health` line, which describes liveness-with-possible-stalls and misses
/// rows that finished before their first health verdict landed.
pub fn subagent_is_active(status: manox_agent::ToolCallStatus) -> bool {
    matches!(
        status,
        manox_agent::ToolCallStatus::Running | manox_agent::ToolCallStatus::PendingApproval
    )
}

/// The subagent section's folded view: the rows to render (see
/// [`subagent_display_order`] for the order) plus the fold size — what
/// collapsing would hide, expansion-independent so the `+N` toggle stays
/// reachable to fold back down.
pub struct SubagentFold<'a> {
    pub rows: Vec<&'a SubagentInfo>,
    pub hidden: usize,
}

/// Folded view of the bubble's subagent section: active rows first and
/// finished rows appended in their first-seen order. Collapsed, the
/// finished rows fold into `hidden` (plus any active overflow past the
/// cap); expanded, everything shows. Both halves keep their original
/// order — the list must not shuffle on every store notify.
pub fn subagent_display_order(agents: &[SubagentInfo], expanded: bool) -> SubagentFold<'_> {
    let mut active: Vec<usize> = Vec::new();
    let mut finished: Vec<usize> = Vec::new();
    for (i, info) in agents.iter().enumerate() {
        if subagent_is_active(info.status) {
            active.push(i);
        } else {
            finished.push(i);
        }
    }
    let hidden = agents.len() - active.len().min(SUBAGENTS_CAP);
    if expanded {
        active.extend(finished);
    } else {
        active.truncate(SUBAGENTS_CAP);
    }
    SubagentFold {
        rows: active.into_iter().map(|i| &agents[i]).collect(),
        hidden,
    }
}

/// Stable priority order for the todo list: InProgress → Pending →
/// Completed, original order preserved within each group (`sort_by_key` is
/// stable — an unstable sort would visibly reshuffle rows on every refresh).
pub fn sort_todo_steps(steps: &mut [PlanStep]) {
    steps.sort_by_key(|step| plan_sort_key(step.status));
}

/// Sort priority of a todo status: in-progress work first, then pending,
/// completed last.
pub fn plan_sort_key(status: PlanStepStatus) -> u8 {
    match status {
        PlanStepStatus::InProgress => 0,
        PlanStepStatus::Pending => 1,
        PlanStepStatus::Completed => 2,
    }
}

// ── Todo status visuals ───────────────────────────────────────────────────

/// One todo status glyph: a single-diameter ring carrying the state — the
/// sidebar's "blue dot = awaiting human" vocabulary, not three mismatched
/// typefaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TodoGlyph {
    pub ring_color: gpui::Hsla,
    /// `InProgress` only: the accent dot inside the ring.
    pub inner_dot: bool,
    /// `Completed` only: the small check nested in the (muted) ring.
    pub check: bool,
}

/// Text treatment that rides the glyph — the completed state is carried
/// mostly by the text (muted + struck through), not by the mark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TodoTextStyle {
    pub color: gpui::Hsla,
    pub line_through: bool,
    pub semibold: bool,
}

/// Status → glyph + text mapping.
pub fn todo_visual(status: PlanStepStatus, theme: &Theme) -> (TodoGlyph, TodoTextStyle) {
    match status {
        PlanStepStatus::Pending => (
            TodoGlyph {
                ring_color: theme.muted_foreground,
                inner_dot: false,
                check: false,
            },
            TodoTextStyle {
                color: theme.foreground,
                line_through: false,
                semibold: false,
            },
        ),
        PlanStepStatus::InProgress => (
            TodoGlyph {
                ring_color: theme.accent,
                inner_dot: true,
                check: false,
            },
            TodoTextStyle {
                color: theme.foreground,
                line_through: false,
                semibold: true,
            },
        ),
        PlanStepStatus::Completed => (
            TodoGlyph {
                ring_color: theme.muted_foreground,
                inner_dot: false,
                check: true,
            },
            TodoTextStyle {
                color: theme.muted_foreground,
                line_through: true,
                semibold: false,
            },
        ),
    }
}

// ── Plan files ───────────────────────────────────────────────────────────

/// One plan file this conversation wrote, resolved at bubble-open time by
/// the workspace from the session's `ProposePlan` rows — the model's only
/// plan-approval channel, whose args carry the slug/title pair verbatim.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanFileEntry {
    /// Display title: the proposal's supplied title, else the slug (the
    /// file stem — `<slug>-plan.md` is the on-disk name).
    pub title: String,
}

impl PlanFileEntry {
    /// Parse a `ProposePlan` tool call's arguments into an entry, paired
    /// with its dedupe key. `None` when the slug is missing or blank —
    /// the caller drops the row (a proposal without a slug never named a
    /// plan file).
    pub fn from_proposal(input: &serde_json::Value) -> Option<(String, Self)> {
        let slug = input
            .get("slug")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let title = input
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map_or_else(|| slug.to_string(), str::to_string);
        Some((slug.to_string(), Self { title }))
    }
}

// ── Fold sections ────────────────────────────────────────────────────────

/// Which foldable list section a `+N` row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleSection {
    Subagents,
    Plans,
    Todos,
    Models,
}

/// Per-section fold state. Bubble-local UI state only: reset when the
/// bubble closes, never persisted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BubbleExpanded {
    subagents: bool,
    plans: bool,
    todos: bool,
    models: bool,
}

impl BubbleExpanded {
    fn flip(&mut self, section: BubbleSection) {
        match section {
            BubbleSection::Subagents => self.subagents = !self.subagents,
            BubbleSection::Plans => self.plans = !self.plans,
            BubbleSection::Todos => self.todos = !self.todos,
            BubbleSection::Models => self.models = !self.models,
        }
    }
}

// ── ContextRail state ─────────────────────────────────────────────────────

/// Cockpit state store for the active thread plus the conversation info
/// bubble's open/fold state. Writes flow through `Workspace` →
/// `self.context_rail.update(cx, |r, cx| …)`.
pub struct ContextRail {
    /// The AHP store plus the attached session id (the rail's only read
    /// face — per-model usage, project, cwd and title all derive from the
    /// book's channel state). `None` only before the workspace connects.
    store: Option<(gpui::Entity<AhpStore>, String)>,
    /// Coarse run phase. Derived from `ThreadEvent`s routed here by
    /// `Workspace`; drives the Captain row's status indicator.
    pub cockpit_phase: CockpitPhase,
    /// The model's current todo list, published via `UpdatePlan` and
    /// recovered from history on reload. `None` until the model publishes
    /// one (or after it clears its list). The bubble renders the snapshot's
    /// own step statuses verbatim — nothing here infers progress.
    pub plan: Option<PlanSnapshot>,
    agents: Vec<SubagentInfo>,
    pub side_calls: Vec<manox_agent::SideCallMetric>,
    pub main_call: Option<manox_agent::SideCallMetric>,
    /// Latest resolved branch display for the thread's cwd. `None` until
    /// the first refresh completes; the bubble's branch pair renders its
    /// placeholders until then.
    pub git_branch_display: Option<GitBranchDisplay>,
    /// Whether the conversation info bubble is open. Toggled by the
    /// context-usage ring; any close path (toggle, outside click, Escape,
    /// thread switch) also resets the fold state.
    pub bubble_open: bool,
    bubble_expanded: BubbleExpanded,
    /// Keyboard escape hatch for the open bubble: the surface takes focus
    /// when it opens, so `Escape` lands on this handle's key context.
    pub bubble_focus: gpui::FocusHandle,
}

/// Wire api → tag color + display label: THE wire-api vocabulary, shared
/// with the model menu (whose rows render it as a `Tag` variant). One
/// mapping, so every surface that names a wire api reads the same color.
pub fn pi_wire_tag(api: &str) -> Option<(gpui_component::ColorName, &'static str)> {
    match api {
        "anthropic" => Some((gpui_component::ColorName::Blue, "Anthropic")),
        "openai_responses" => Some((gpui_component::ColorName::Cyan, "Responses")),
        "openai_completions" => Some((gpui_component::ColorName::Amber, "Completions")),
        _ => None,
    }
}

/// Wire-api text tint for the bubble's model rows: the tag color at the
/// menu tag's exact scale (600 light / 300 dark — the `Tag` outline fg
/// formula), muted for unknown/absent apis.
pub fn pi_wire_text_color(api: &str, theme: &gpui_component::Theme) -> gpui::Hsla {
    match pi_wire_tag(api) {
        Some((color, _)) => color.scale(if theme.is_dark() { 300 } else { 600 }),
        None => theme.muted_foreground,
    }
}

impl ContextRail {
    pub fn new(store: Option<(gpui::Entity<AhpStore>, String)>, cx: &mut App) -> Self {
        Self {
            store,
            cockpit_phase: CockpitPhase::Idle,
            plan: None,
            agents: Vec::new(),
            side_calls: Vec::new(),
            main_call: None,
            git_branch_display: None,
            bubble_open: false,
            bubble_expanded: BubbleExpanded::default(),
            bubble_focus: cx.focus_handle(),
        }
    }

    /// Re-bind the bubble's read face to the newly attached thread's leaf.
    /// The store is the ONLY data source (U7b): the SessionStatus deltas
    /// and the Q-face info-fetch responses only reach the leaf of the
    /// ATTACHED session, so a rail left bound to a previous leaf renders a
    /// permanently frozen usage face (the rail-freeze regression from the
    /// visual-acceptance run).
    pub fn bind_store(
        &mut self,
        store: Option<(gpui::Entity<AhpStore>, String)>,
        cx: &mut Context<Self>,
    ) {
        self.store = store;
        cx.notify();
    }

    /// Diagnostic: the entity id of the bound store leaf (the rail-freeze
    /// regression asserts the attach-time re-bind).
    pub fn diagnostic_store_id(&self) -> Option<gpui::EntityId> {
        self.store.as_ref().map(|(store, _)| store.entity_id())
    }

    /// Reset per-thread cockpit state on thread switch: the outgoing
    /// thread's plan and per-model counter state do not apply to the
    /// incoming one. The bubble closes too — its contents are the outgoing
    /// thread's, already stale.
    pub fn reset_for_thread_switch(&mut self, running: bool, cx: &mut Context<Self>) {
        self.side_calls.clear();
        self.main_call = None;
        self.agents.clear();
        let new_phase = if running {
            CockpitPhase::Streaming
        } else {
            CockpitPhase::Idle
        };
        self.cockpit_phase = new_phase;
        // The incoming thread's plan is seeded separately from its history by
        // `set_plan`; clear here so a thread with no plan starts empty rather
        // than inheriting the outgoing thread's list.
        self.plan = None;
        self.git_branch_display = None;
        self.set_bubble_open(false, cx);
        cx.notify();
    }

    /// Update `cockpit_phase` for the streaming/tool variants that flow through
    /// the generic catch-all arm. `Error`, `Stop`, `TurnStarted`, and
    /// `ToolCallAuthorization` are handled in their dedicated arms on `Workspace`;
    /// this only covers the residual transitions routed here from the workspace
    /// event handler.
    pub fn update_cockpit_phase(&mut self, ev: &ThreadEvent, cx: &mut Context<Self>) {
        match ev {
            ThreadEvent::AgentText(_) => {
                self.cockpit_phase = CockpitPhase::Streaming;
            }
            ThreadEvent::AgentThinking(_) => {
                self.cockpit_phase = CockpitPhase::Thinking;
            }
            ThreadEvent::ToolCall { status, .. } => match status {
                manox_agent::thread::ToolCallStatus::Running => {
                    self.cockpit_phase = CockpitPhase::RunningTool;
                }
                // A non-running terminal/intermediate status means the model
                // is back to streaming the next assistant segment.
                _ => {
                    self.cockpit_phase = CockpitPhase::Streaming;
                }
            },
            ThreadEvent::CompactionStarted { .. } => {
                self.cockpit_phase = CockpitPhase::Summarizing;
            }
            ThreadEvent::Compaction { .. } => {
                // The summary landed; the turn resumes streaming.
                self.cockpit_phase = CockpitPhase::Streaming;
            }
            _ => {}
        }
        cx.notify();
    }

    /// Upsert one sub-agent observation row from a `SubagentProgress` event
    /// (the pi harness emits these around its ephemeral nested sessions).
    /// `self.agents` is first-seen order by construction ("update in place,
    /// else push") — the bubble's subagent section relies on that and never
    /// re-sorts. `health` carries the watchdog's one-line verdict while the
    /// run is live; `None` leaves the stored verdict untouched.
    pub fn apply_subagent_progress(
        &mut self,
        id: &str,
        subagent_type: &str,
        description: Option<&str>,
        status: manox_agent::ToolCallStatus,
        health: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if let Some(info) = self.agents.iter_mut().find(|info| info.id == id) {
            info.status = status;
            if !subagent_type.is_empty() {
                info.subagent_type = subagent_type.to_string();
            }
            if let Some(description) = description
                && info.description.is_empty()
            {
                info.description = description.to_string();
            }
            if let Some(health) = health {
                info.health = Some(health.to_string());
            }
        } else {
            self.agents.push(SubagentInfo {
                id: id.to_string(),
                subagent_type: subagent_type.to_string(),
                description: description.unwrap_or_default().to_string(),
                status,
                health: health.map(str::to_string),
            });
        }
        cx.notify();
    }

    /// Adopt a todo snapshot published by the model (or recovered from
    /// history). An empty snapshot clears the list.
    pub fn set_plan(&mut self, snapshot: PlanSnapshot, cx: &mut Context<Self>) {
        if snapshot.is_empty() {
            self.plan = None;
            cx.notify();
            return;
        }
        self.plan = Some(snapshot);
        cx.notify();
    }

    /// Replace the cached branch display. Called by `Workspace` after a
    /// debounced background `git_status::gather_branch` resolves.
    pub fn set_git_branch(&mut self, display: Option<GitBranchDisplay>, cx: &mut Context<Self>) {
        self.git_branch_display = display;
        cx.notify();
    }

    // ── Bubble open/fold state ────────────────────────────────────────────

    /// Open/close the bubble. Every close path resets the fold state — the
    /// expansion is bubble-local UI, not durable state.
    pub fn set_bubble_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.bubble_open == open {
            return;
        }
        self.bubble_open = open;
        if !open {
            self.bubble_expanded = BubbleExpanded::default();
        }
        cx.notify();
    }

    pub fn toggle_bubble(&mut self, cx: &mut Context<Self>) {
        self.set_bubble_open(!self.bubble_open, cx);
    }

    fn toggle_section_fold(&mut self, section: BubbleSection, cx: &mut Context<Self>) {
        self.bubble_expanded.flip(section);
        cx.notify();
    }

    // ── Bubble rendering ──────────────────────────────────────────────────

    /// The conversation info bubble's content: six segments separated only
    /// by hairlines (no headings), each empty segment — divider included —
    /// skipped. `plan_files` arrives newest-first (the workspace collects
    /// the session's `ProposePlan` rows and reverses); `max_w`/`max_h` are
    /// the window-derived clamps the caller refines onto the popover
    /// surface.
    pub fn render_bubble(
        this: &Entity<Self>,
        plan_files: &[PlanFileEntry],
        max_w: Pixels,
        max_h: Pixels,
        cx: &App,
    ) -> AnyElement {
        let rail = this.read(cx);
        let theme = cx.theme().clone();
        let expanded = rail.bubble_expanded;
        let weak = this.downgrade();

        let mut sections = Vec::new();
        push_section(
            &mut sections,
            &theme,
            rail.render_agents_section(&theme, &weak, cx),
        );
        push_section(
            &mut sections,
            &theme,
            rail.render_branch_pair_section(&theme, cx),
        );
        push_section(
            &mut sections,
            &theme,
            render_plan_files_section(plan_files, expanded.plans, &weak, &theme),
        );
        push_section(
            &mut sections,
            &theme,
            rail.render_todos_section(expanded.todos, &weak, &theme),
        );
        push_section(
            &mut sections,
            &theme,
            rail.render_models_section(expanded.models, &weak, &theme, cx),
        );
        push_section(&mut sections, &theme, render_sources_section(&theme));

        v_flex()
            .id("conversation-info-bubble")
            .w_full()
            .min_w(px(BUBBLE_MIN_W))
            .max_w(max_w)
            .max_h(max_h)
            .overflow_y_scroll()
            .gap_2()
            .children(sections)
            .into_any_element()
    }

    /// Captain + subagents. The Captain row always renders; subagent rows
    /// show `{type} · {topic}` on the SAME left baseline (no indent, no
    /// tree glyphs — the subordination is singular and certain). Only
    /// unfinished rows show by default; finished rows fold into `+N` (a
    /// quiet history keeps the Captain row and a bare `+N`).
    fn render_agents_section(
        &self,
        theme: &Theme,
        weak: &gpui::WeakEntity<Self>,
        cx: &App,
    ) -> Option<AnyElement> {
        let running = self
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                leaf_of(&view.book, sid).running()
            })
            .unwrap_or(false);
        let main_status = if self.cockpit_phase == CockpitPhase::Failed {
            manox_agent::ToolCallStatus::Error
        } else if running {
            manox_agent::ToolCallStatus::Running
        } else {
            manox_agent::ToolCallStatus::Success
        };
        let mut rows = vec![
            h_flex()
                .w_full()
                .min_w_0()
                .py_0p5()
                .gap_1p5()
                .items_center()
                .child(Self::captain_status_indicator(main_status, theme))
                .child(
                    gpui::div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(CAPTAIN_LABEL),
                )
                .into_any_element(),
        ];

        let fold = subagent_display_order(&self.agents, self.bubble_expanded.subagents);
        for info in fold.rows {
            // `{type} · {topic}` with graceful one-sided fallbacks.
            let title = task_display_title(&info.subagent_type, &info.description)
                .unwrap_or_else(|| info.id.clone());
            rows.push(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .py_0p5()
                    .gap_1p5()
                    .items_center()
                    .child(status_indicator(info.status, theme))
                    .child(
                        gpui::div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(theme.foreground)
                            .child(SharedString::from(title)),
                    )
                    .into_any_element(),
            );
        }
        if fold.hidden > 0 {
            rows.push(plus_n_row(
                BubbleSection::Subagents,
                fold.hidden,
                self.bubble_expanded.subagents,
                weak,
                theme,
            ));
        }
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .children(rows)
                .into_any_element(),
        )
    }

    /// The branch / worktree pair: `┌ {branch}` over `└ {worktree basename}`,
    /// one pair per checkout (the thread's single cwd today). Same data
    /// sources the old card's branch block read; the change ±counts and the
    /// click-to-copy affordances retired with the card.
    fn render_branch_pair_section(&self, theme: &Theme, cx: &App) -> Option<AnyElement> {
        let cwd_path = self.store.as_ref().and_then(|(store, sid)| {
            let view = store.read(cx);
            leaf_of(&view.book, sid).cwd().map(PathBuf::from)
        });
        let basename = cwd_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string);
        let Some(basename) = basename else {
            // No project bound: nothing to pair — hide the segment outright.
            return None;
        };
        let mono = theme.mono_font_family.clone();
        let display = self.git_branch_display.clone();
        let branch_row: SharedString = match &display {
            Some(d) if d.is_no_repo() => i18n::t("workspace-env-git-not-a-repo"),
            Some(d) => {
                let mut s = d
                    .branch
                    .clone()
                    .or_else(|| d.detached_sha.clone())
                    .map(SharedString::from)
                    .unwrap_or_else(|| i18n::t("workspace-env-git-unavailable"));
                if d.branch.is_none() && d.detached_sha.is_some() {
                    s = SharedString::from(format!(
                        "{} {}",
                        s,
                        i18n::t("workspace-env-git-detached")
                    ));
                }
                s
            }
            // The refresh is still in flight; `--` keeps the pair's shape.
            None => SharedString::from("--"),
        };
        let pair = v_flex()
            .w_full()
            .min_w_0()
            .font_family(mono)
            .child(pair_row(
                "┌",
                branch_row,
                theme.muted_foreground,
                theme.foreground,
            ))
            .child(pair_row(
                "└",
                SharedString::from(basename),
                theme.muted_foreground,
                theme.muted_foreground,
            ));
        Some(pair.into_any_element())
    }

    /// The model's todo list: stable InProgress → Pending → Completed
    /// order, every status visible (completed rows are part of the
    /// progress story), unified-diameter ring glyphs.
    fn render_todos_section(
        &self,
        expanded: bool,
        weak: &gpui::WeakEntity<Self>,
        theme: &Theme,
    ) -> Option<AnyElement> {
        let plan = self.plan.as_ref()?;
        let mut steps: Vec<PlanStep> = plan.steps.clone();
        sort_todo_steps(&mut steps);
        let fold = fold_window(steps.len(), TODOS_CAP, expanded);
        let mut rows: Vec<AnyElement> = steps[..fold.visible]
            .iter()
            .map(|step| todo_row(step, theme))
            .collect();
        if fold.hidden > 0 {
            rows.push(plus_n_row(
                BubbleSection::Todos,
                fold.hidden,
                expanded,
                weak,
                theme,
            ));
        }
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .children(rows)
                .into_any_element(),
        )
    }

    /// Per-model token usage, total-token descending. The model row is
    /// `{provider}/{model}` with the model segment tinted by wire api; the
    /// `├ Context` line is the LAST-REQUEST occupancy over the window —
    /// only resolvable for the foreground model (that is the ring's own
    /// measure), so every other model shows just the `└` token line.
    fn render_models_section(
        &self,
        expanded: bool,
        weak: &gpui::WeakEntity<Self>,
        theme: &Theme,
        cx: &App,
    ) -> Option<AnyElement> {
        let (store, sid) = self.store.as_ref()?;
        let leaf = leaf_of(&store.read(cx).book, sid);
        let per_model: HashMap<String, manox_agent::language_model::TokenUsage> = leaf
            .metrics
            .map(|m| {
                m.per_model_usage
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_tokens()))
                    .collect()
            })
            .unwrap_or_default();
        if per_model.is_empty() {
            return None;
        }
        // The foreground model's identity — the one model whose last-request
        // occupancy the ring already measures; the measure itself is the
        // in-flight turn's usage report while streaming, else the last
        // completed turn's (cache-write tokens are not modeled on the AHP
        // turn usage, so the occupancy floor is input + cache-read).
        let fg_key: Option<String> = leaf.model_id().map(str::to_string);
        let fg_last = leaf.last_usage().map(|u| crate::ahp_store::UsageSnapshot {
            input: u.input_tokens.unwrap_or(0).max(0) as u64,
            output: u.output_tokens.unwrap_or(0).max(0) as u64,
            cache_creation: 0,
            cache_read: u.cache_read_tokens.unwrap_or(0).max(0) as u64,
        });
        let muted = theme.muted_foreground;
        let warn = theme.warning;

        let mut models: Vec<(&String, &manox_agent::language_model::TokenUsage)> =
            per_model.iter().collect();
        // Total tokens desc; ties break by model key so rows cannot
        // reshuffle between frames (the per-frame HashMap re-collect has no
        // stable iteration order of its own).
        models.sort_by(|a, b| {
            b.1.total_tokens()
                .cmp(&a.1.total_tokens())
                .then_with(|| a.0.cmp(b.0))
        });
        let fold = fold_window(models.len(), MODELS_CAP, expanded);
        let mut blocks: Vec<AnyElement> = Vec::new();
        for (model_name, usage) in models[..fold.visible].iter() {
            let block = v_flex().w_full().min_w_0().gap_0p5();
            // Model row: three separately-styled segments (provider muted,
            // `/` muted, model tinted by wire api) so no width arithmetic
            // ever overlaps them.
            let model_row: AnyElement =
                match model_name.split_once('/').and_then(|(provider, id)| {
                    manox_agent::provider_glue::global().resolve_model(provider, id)
                }) {
                    Some(m) => h_flex()
                        .min_w_0()
                        .text_sm()
                        .child(
                            gpui::div()
                                .flex_none()
                                .text_color(muted)
                                .child(manox_agent::provider_glue::display_provider_name(&m)),
                        )
                        .child(gpui::div().flex_none().text_color(muted).child("/"))
                        .child(
                            gpui::div()
                                .min_w_0()
                                .truncate()
                                .text_color(pi_wire_text_color(&m.api, theme))
                                .child(manox_agent::provider_glue::display_name(&m)),
                        )
                        .into_any_element(),
                    None => gpui::div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(SharedString::from((*model_name).clone()))
                        .into_any_element(),
                };
            let mut block = block.child(model_row);

            // `├ Context` — last-request occupancy, foreground model only,
            // ≥90% in the warning color.
            if Some(*model_name) == fg_key.as_ref()
                && let Some(last) = fg_last
                && let Some(window) = model_window_tokens(model_name)
            {
                let active = last
                    .input
                    .saturating_add(last.cache_creation)
                    .saturating_add(last.cache_read);
                if let Some(budget) = context_budget_pct(window, active) {
                    let color = if budget.used_pct >= CONTEXT_NEAR_FULL_PCT {
                        warn
                    } else {
                        muted
                    };
                    block = block.child(
                        gpui::div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(color)
                            .child(SharedString::from(format!(
                                "├ Context {:.0}% {} / {}",
                                budget.used_pct,
                                format_tokens_pi(budget.active_tokens),
                                format_tokens_pi(budget.cap_tokens),
                            ))),
                    );
                }
            }

            // `└` token line: `↑input ↓output Rcache_read CHhit%` (`--`
            // when there is no input to measure).
            let cache_hit = cache_read_ratio(**usage)
                .map(|r| format_cache_hit(r, 1))
                .unwrap_or_else(|| "--".into());
            block = block.child(
                gpui::div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(format!(
                        "└ ↑{} ↓{} R{} CH{}",
                        format_tokens_pi(usage.input_tokens),
                        format_tokens_pi(usage.output_tokens),
                        format_tokens_pi(usage.cache_read_input_tokens),
                        cache_hit,
                    ))),
            );
            blocks.push(block.into_any_element());
        }
        if fold.hidden > 0 {
            blocks.push(plus_n_row(
                BubbleSection::Models,
                fold.hidden,
                expanded,
                weak,
                theme,
            ));
        }
        Some(
            v_flex()
                .w_full()
                .min_w_0()
                .children(blocks)
                .into_any_element(),
        )
    }

    /// Status indicator for the Captain (main agent) row. The Captain uses
    /// `ship-wheel` for the completed state, distinguishing it from
    /// sub-agents that use `circle-check-big`.
    fn captain_status_indicator(status: manox_agent::ToolCallStatus, theme: &Theme) -> AnyElement {
        use manox_agent::ToolCallStatus;
        match status {
            ToolCallStatus::PendingApproval | ToolCallStatus::Running => {
                ai_elements::BrailleSpinner::new()
                    .xsmall()
                    .color(theme.accent_foreground)
                    .into_any_element()
            }
            ToolCallStatus::Success | ToolCallStatus::Continued => Icon::default()
                .path("icons/ship-wheel.svg")
                .xsmall()
                .text_color(theme.success)
                .into_any_element(),
            ToolCallStatus::Error | ToolCallStatus::Denied => Icon::new(IconName::CircleX)
                .xsmall()
                .text_color(theme.danger)
                .into_any_element(),
            ToolCallStatus::Cancelled => Icon::new(IconName::Minus)
                .xsmall()
                .text_color(theme.muted_foreground)
                .into_any_element(),
        }
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────

/// Append a section with a preceding divider only when earlier sections
/// exist — computing "has content" BEFORE drawing lines means an empty
/// section leaves neither an orphan divider nor two adjacent ones.
fn push_section(out: &mut Vec<AnyElement>, theme: &Theme, section: Option<AnyElement>) {
    if let Some(content) = section {
        if !out.is_empty() {
            out.push(
                gpui::div()
                    .h(px(1.))
                    .w_full()
                    .bg(theme.border)
                    .flex_none()
                    .into_any_element(),
            );
        }
        out.push(content);
    }
}

/// One `┌`/`└` row of the branch pair: tree glyph muted, text truncating.
fn pair_row(
    glyph: &str,
    text: SharedString,
    glyph_color: gpui::Hsla,
    color: gpui::Hsla,
) -> AnyElement {
    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap_1()
        .child(
            gpui::div()
                .flex_none()
                .text_sm()
                .text_color(glyph_color)
                .child(SharedString::from(glyph)),
        )
        .child(
            gpui::div()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(color)
                .child(text),
        )
        .into_any_element()
}

/// One todo row: the unified ring glyph + the text treatment the status
/// maps to (completed = muted + struck through).
fn todo_row(step: &PlanStep, theme: &Theme) -> AnyElement {
    let (glyph, style) = todo_visual(step.status, theme);
    let title = SharedString::from(step.step.clone());
    h_flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap_1p5()
        .py_0p5()
        .child(todo_glyph(glyph))
        .child(
            gpui::div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .text_color(style.color)
                .when(style.line_through, |d| d.line_through())
                .when(style.semibold, |d| {
                    d.font_weight(gpui::FontWeight::SEMIBOLD)
                })
                .child(title),
        )
        .into_any_element()
}

/// The status ring: fixed 10px diameter across all three states; the inner
/// dot and the check ride inside it.
fn todo_glyph(glyph: TodoGlyph) -> AnyElement {
    gpui::div()
        .relative()
        .flex_none()
        .size(px(10.))
        .child(
            gpui::div()
                .absolute()
                .inset_0()
                .rounded_full()
                .border_1()
                .border_color(glyph.ring_color),
        )
        .when(glyph.inner_dot, |ring| {
            ring.child(
                gpui::div()
                    .absolute()
                    .top(px(2.))
                    .left(px(2.))
                    .size(px(4.))
                    .rounded_full()
                    .bg(glyph.ring_color),
            )
        })
        .when(glyph.check, |ring| {
            ring.child(
                gpui::div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Check)
                            .with_size(gpui_component::Size::Size(px(7.)))
                            .text_color(glyph.ring_color),
                    ),
            )
        })
        .into_any_element()
}

/// The `+N` fold toggle: chevron + count on the section's shared left
/// baseline, rendered only while the section hides rows. Clicking expands
/// in place (the bubble grows with its content); clicking again collapses.
/// The handler rides the rail entity — the bubble's fold state is
/// bubble-local and dies with the close.
fn plus_n_row(
    section: BubbleSection,
    hidden: usize,
    expanded: bool,
    rail: &gpui::WeakEntity<ContextRail>,
    theme: &Theme,
) -> AnyElement {
    let id = SharedString::from(format!("bubble-fold-{section:?}"));
    let rail = rail.clone();
    h_flex()
        .id(id)
        .w_full()
        .min_w_0()
        .items_center()
        .gap_1()
        .py_0p5()
        .cursor_pointer()
        .on_click(move |_, _, cx| {
            let _ = rail.update(cx, |rail, cx| rail.toggle_section_fold(section, cx));
        })
        .child(
            Icon::new(if expanded {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            })
            .xsmall()
            .text_color(theme.muted_foreground),
        )
        .child(
            gpui::div()
                .min_w_0()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(format!("+ {hidden}"))),
        )
        .into_any_element()
}

/// The plan-file list: one document row per written plan, newest first.
fn render_plan_files_section(
    plan_files: &[PlanFileEntry],
    expanded: bool,
    rail: &gpui::WeakEntity<ContextRail>,
    theme: &Theme,
) -> Option<AnyElement> {
    if plan_files.is_empty() {
        return None;
    }
    let fold = fold_window(plan_files.len(), PLANS_CAP, expanded);
    let mut rows: Vec<AnyElement> = plan_files[..fold.visible]
        .iter()
        .map(|entry| {
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap_1p5()
                .py_0p5()
                .child(
                    Icon::new(IconName::FileText)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .child(
                    gpui::div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(theme.foreground)
                        .child(SharedString::from(entry.title.clone())),
                )
                .into_any_element()
        })
        .collect();
    if fold.hidden > 0 {
        rows.push(plus_n_row(
            BubbleSection::Plans,
            fold.hidden,
            expanded,
            rail,
            theme,
        ));
    }
    Some(
        v_flex()
            .w_full()
            .min_w_0()
            .children(rows)
            .into_any_element(),
    )
}

/// The sources segment: the old card carried only the placeholder — it
/// renders the same muted line, heading gone.
fn render_sources_section(theme: &Theme) -> Option<AnyElement> {
    Some(
        gpui::div()
            .w_full()
            .min_w_0()
            .truncate()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(i18n::t("workspace-env-no-sources"))
            .into_any_element(),
    )
}

/// Context window for a model name: the shared pi provider registry, by
/// wire id then display name (the same probe the usage face has always
/// used).
fn model_window_tokens(model_name: &str) -> Option<u64> {
    let registry = manox_agent::provider_glue::global();
    // per_model keys are composite "{provider}/{model_id}"; resolve O(1).
    // Bare ids (legacy keys) fall through to the scan below.
    if let Some((provider, id)) = model_name.split_once('/')
        && let Some(m) = registry.resolve_model(provider, id)
    {
        return Some(m.context_window as u64);
    }
    registry
        .models()
        .iter()
        .find(|m| {
            m.id == model_name
                || m.metadata
                    .get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|name| name == model_name)
        })
        .map(|m| m.context_window as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    fn step(step: &str, status: PlanStepStatus) -> PlanStep {
        PlanStep {
            step: step.to_string(),
            status,
        }
    }

    // ── fold_window ──────────────────────────────────────────────────────

    #[test]
    fn fold_window_shows_plus_n_only_past_the_cap() {
        // At or under the cap: no fold, no hidden count.
        assert_eq!(
            fold_window(8, TODOS_CAP, false),
            FoldPlan {
                visible: 8,
                hidden: 0
            }
        );
        // One past the cap: cap shown, the tail hidden.
        assert_eq!(
            fold_window(9, TODOS_CAP, false),
            FoldPlan {
                visible: 8,
                hidden: 1
            }
        );
        // Expanded shows everything, but the fold size stays: the `+N`
        // toggle must remain rendered (chevron flipped) so the section can
        // fold back down.
        assert_eq!(
            fold_window(30, TODOS_CAP, true),
            FoldPlan {
                visible: 30,
                hidden: 22
            }
        );
        assert_eq!(
            fold_window(9, TODOS_CAP, true),
            FoldPlan {
                visible: 9,
                hidden: 1
            }
        );
    }

    // ── subagent fold ────────────────────────────────────────────────────

    fn agent(id: &str, status: manox_agent::ToolCallStatus) -> SubagentInfo {
        SubagentInfo {
            id: id.to_string(),
            subagent_type: format!("type-{id}"),
            description: String::new(),
            status,
            health: None,
        }
    }

    #[test]
    fn subagent_fold_keeps_first_seen_order_and_hides_finished() {
        use manox_agent::ToolCallStatus as S;
        // Interleaved arrival: finished rows keep their own order, appended
        // after the actives on expand.
        let agents = vec![
            agent("a1", S::Success),
            agent("a2", S::Running),
            agent("a3", S::Error),
            agent("a4", S::Running),
            agent("a5", S::Cancelled),
            agent("a6", S::PendingApproval),
        ];
        let collapsed = subagent_display_order(&agents, false);
        assert_eq!(
            collapsed
                .rows
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a2", "a4", "a6"],
            "only unfinished rows show by default, in arrival order"
        );
        assert_eq!(collapsed.hidden, 3, "the three finished rows fold into +N");

        let expanded = subagent_display_order(&agents, true);
        assert_eq!(
            expanded
                .rows
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a2", "a4", "a6", "a1", "a3", "a5"]
        );
        // The fold size survives expansion: the `+N` toggle stays rendered
        // so the section can fold back down.
        assert_eq!(expanded.hidden, 3);
    }

    #[test]
    fn subagent_fold_with_no_active_rows_still_reports_hidden() {
        use manox_agent::ToolCallStatus as S;
        let agents = vec![agent("a1", S::Success), agent("a2", S::Continued)];
        let fold = subagent_display_order(&agents, false);
        assert!(
            fold.rows.is_empty(),
            "no active rows: the section shows only +N"
        );
        assert_eq!(fold.hidden, 2);
    }

    // ── todo sort ────────────────────────────────────────────────────────

    #[test]
    fn todo_sort_is_stable_within_priority_groups() {
        use PlanStepStatus::{Completed, InProgress, Pending};
        let mut steps = vec![
            step("t1", Completed),
            step("t2", InProgress),
            step("t3", Pending),
            step("t4", InProgress),
            step("t5", Completed),
            step("t6", Pending),
        ];
        sort_todo_steps(&mut steps);
        let names: Vec<&str> = steps.iter().map(|s| s.step.as_str()).collect();
        assert_eq!(
            names,
            vec!["t2", "t4", "t3", "t6", "t1", "t5"],
            "in-progress first, then pending, completed last — arrival order within groups"
        );
    }

    // ── todo visuals ─────────────────────────────────────────────────────

    #[test]
    fn todo_status_maps_to_glyph_and_text_treatment() {
        let t = theme();
        let (glyph, text) = todo_visual(PlanStepStatus::Pending, &t);
        assert!(!glyph.inner_dot && !glyph.check);
        assert!(!text.line_through && !text.semibold);

        let (glyph, text) = todo_visual(PlanStepStatus::InProgress, &t);
        assert!(glyph.inner_dot && !glyph.check);
        assert_eq!(glyph.ring_color, t.accent);
        assert!(text.semibold && !text.line_through);

        let (glyph, text) = todo_visual(PlanStepStatus::Completed, &t);
        assert!(glyph.check && !glyph.inner_dot);
        assert!(text.line_through && !text.semibold);
        assert_eq!(text.color, t.muted_foreground);
    }

    // ── wire api coloring ────────────────────────────────────────────────

    #[test]
    fn wire_api_color_matches_the_menu_tag_palette() {
        let t = theme();
        // The three routed apis carry their menu tag color at the tag's own
        // scale; the REAL wire keys are `openai_responses` /
        // `openai_completions` (the old `responses`/`completions` guesses
        // never matched a single model and fell through to muted).
        assert_eq!(
            pi_wire_text_color("anthropic", &t),
            gpui_component::ColorName::Blue.scale(600)
        );
        assert_eq!(
            pi_wire_text_color("openai_responses", &t),
            gpui_component::ColorName::Cyan.scale(600)
        );
        assert_eq!(
            pi_wire_text_color("openai_completions", &t),
            gpui_component::ColorName::Amber.scale(600)
        );
        assert_eq!(pi_wire_text_color("responses", &t), t.muted_foreground);
        assert_eq!(pi_wire_text_color("completions", &t), t.muted_foreground);
        assert_eq!(pi_wire_text_color("whatever-comes", &t), t.muted_foreground);
        // Every mapped api keeps its menu label.
        assert_eq!(pi_wire_tag("anthropic").unwrap().1, "Anthropic");
        assert_eq!(pi_wire_tag("openai_responses").unwrap().1, "Responses");
        assert_eq!(pi_wire_tag("openai_completions").unwrap().1, "Completions");
    }

    // ── open/close state machine ─────────────────────────────────────────

    #[gpui::test]
    fn closing_the_bubble_resets_fold_state(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let rail = cx.new(|cx| ContextRail::new(None, cx));
            rail.update(cx, |rail, cx| {
                rail.toggle_bubble(cx);
                rail.toggle_section_fold(BubbleSection::Todos, cx);
                rail.toggle_section_fold(BubbleSection::Models, cx);
                assert!(rail.bubble_open);
                assert!(rail.bubble_expanded.todos && rail.bubble_expanded.models);
                // Closing — by toggle, outside click or Escape, all funneling
                // through `set_bubble_open` — wipes the per-section folds.
                rail.set_bubble_open(false, cx);
                assert!(!rail.bubble_open);
                assert_eq!(rail.bubble_expanded, BubbleExpanded::default());
            });
        });
    }

    #[gpui::test]
    fn thread_switch_closes_the_bubble(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let rail = cx.new(|cx| ContextRail::new(None, cx));
            rail.update(cx, |rail, cx| {
                rail.toggle_bubble(cx);
                assert!(rail.bubble_open);
                // The bubble's contents are the outgoing thread's — already
                // stale; the switch must not leave it open.
                rail.reset_for_thread_switch(false, cx);
                assert!(!rail.bubble_open);
                assert_eq!(rail.bubble_expanded, BubbleExpanded::default());
            });
        });
    }

    // ── section assembly ─────────────────────────────────────────────────

    /// The divider rule: dividers only BETWEEN present sections. Driven
    /// through `push_section` directly since the bubble's builder needs a
    /// live app context.
    #[test]
    fn empty_sections_never_produce_double_dividers() {
        let t = theme();
        let mut out: Vec<AnyElement> = Vec::new();
        let some = || Some(gpui::div().into_any_element());
        push_section(&mut out, &t, None);
        push_section(&mut out, &t, some());
        push_section(&mut out, &t, None);
        push_section(&mut out, &t, some());
        push_section(&mut out, &t, None);
        // Two present sections, one divider between them, nothing else.
        assert_eq!(out.len(), 3);
    }
}
