//! Conversation history sidebar.
//!
//! A standalone gpui Entity that lists past threads from the gateway (U2):
//! the rows are the multiplexer's wire `ThreadListItem`s (§D.5
//! `ThreadsUpdated` mirrors / `ListThreads` responses with the
//! `SessionStatus` deltas merged in) — the former kernel `StoreHandle` event
//! pump is retired. The decoration columns (project grouping, tag chip,
//! approval wash) ride the wire rows and the grouping registry rides the
//! `HostEvent::Projects` mirror (U2 cross-domain #1). Clicking a
//! conversation entry emits
//! `OpenThread(id)`; the "Conversations" section header's "+" opens the
//! flat new-session menu and each project folder header's ellipsis button opens the project
//! action menu (new session / terminal / VS Code / remove project). Workspace subscribes to
//! these events.
//!
//! Threads bound to a project (chosen on the first screen) are grouped under a collapsible folder
//! in the "Projects" section, keyed by project path; the rest fall under "Conversations". The top
//! menu and bottom account footer are static decoration.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use crate::i18n;
use crate::sidebar_view::{self, OrderBy};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClipboardItem, Context, DismissEvent,
    DragMoveEvent, Entity, EventEmitter, Pixels, Render, ScrollHandle, SharedString, Subscription,
    Transformation, WeakEntity, Window, deferred, ease_in_out, linear, percentage, prelude::*, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Theme,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    tag::{Tag, TagVariant},
    tooltip::Tooltip,
    v_flex,
};
use manox_agent::thread::PermissionMode;
use manox_protocol::ThreadListItem;

/// How far the row wash translates (in pixels, clipped to the row) during the
/// selection-slide. The two adjacent rows animate in opposite directions so
/// the wash reads as moving from the old row to the new one.
const SELECT_SLIDE_PX: f32 = 28.;

/// Vertical direction from the previously-selected row to the newly-selected
/// one, used to angle the slide. `None` (e.g. the two rows are in different
/// sections, or one is off-screen) falls back to a plain fade.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SlideDir {
    Up,
    Down,
    None,
}

/// Per-frame snapshot of the selection transition, shared with every row so
/// each can decide whether it is the incoming or outgoing end of the slide.
#[derive(Clone)]
struct SlideCtx {
    selecting_id: Option<String>,
    deselecting_id: Option<String>,
    dir: SlideDir,
    gen_id: u64,
}

/// Which section header the sticky overlay above the scroll body shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PinnedSection {
    Projects,
    Conversations,
}

/// Section-header pinning rule for the sticky overlay: the Projects header
/// stays pinned while the viewport is inside the projects section; once the
/// projects content has scrolled fully past the top (`scroll_top >=
/// projects_height`) the Conversations header takes over. `projects_height`
/// is `None` before the scroll container has been laid out — the overlay only
/// appears once `scroll_top > 0`, which cannot happen before the first
/// layout, so this branch is just a safety fallback.
fn pinned_section(
    projects_present: bool,
    scroll_top: Pixels,
    projects_height: Option<Pixels>,
) -> PinnedSection {
    if !projects_present {
        return PinnedSection::Conversations;
    }
    match projects_height {
        None => PinnedSection::Projects,
        Some(h) if scroll_top < h => PinnedSection::Projects,
        _ => PinnedSection::Conversations,
    }
}

/// Which end of the slide a row is playing this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimRole {
    /// The newly-selected row: wash fades in, settling toward its resting spot.
    Selecting,
    /// The previously-selected row: wash fades out, drifting toward the new row.
    Deselecting,
    /// Neither — no wash overlay (hover handles non-selected feedback).
    None,
}

/// A row in the Conversations list — either a manox thread (the gateway's
/// wire `ThreadListItem`, U2) or an external agent CLI session, unified so the
/// two render through one row factory in one band sequence. Both row kinds share
/// the selection-slide: their ids join one `flat_ids` ordering and
/// `render_thread_item` applies the same `SlideCtx` wash to either.
#[derive(Clone)]
enum SidebarRow {
    Thread(ThreadListItem),
    External(crate::external_session::ExternalSessionSummary),
}

/// The render environment every partition row shares: selection, badges, the
/// slide animation and the theme.
struct PartitionEnv<'a> {
    selected: Option<&'a str>,
    unread_map: &'a HashMap<String, bool>,
    slide: &'a SlideCtx,
    theme: &'a Theme,
    /// Left inset of a top-level row: a project folder indents its rows, the
    /// loose partition does not.
    indent_base: Pixels,
}

/// Which edge of a row the insertion line hugs: the boundary is between this
/// row and its neighbour, so the anchor resolves at commit time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragEdge {
    Top,
    Bottom,
}

/// In-flight row drag: the row being dragged, the row whose edge carries the
/// insertion line, and which edge that is.
#[derive(Debug, Clone, PartialEq)]
struct RowDrag {
    dragged: String,
    line_on: String,
    edge: DragEdge,
}

/// A committed folder drop: a no-op (emit nothing) or the move to forward to
/// the server, `None` meaning append.
enum FolderDrop {
    Noop,
    Move(Option<PathBuf>),
}

/// Drag payload for a thread row. The id is all the gesture needs: the
/// partition resolves from the wire row at commit time.
#[derive(Clone, PartialEq)]
struct DraggedThreadRow {
    id: String,
}

impl Render for DraggedThreadRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // No visible ghost: the row itself is the thing being moved.
        gpui::div()
    }
}

/// Drag payload for a project folder header.
#[derive(Clone, PartialEq)]
struct DraggedFolderRow {
    path: String,
}

impl Render for DraggedFolderRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

impl SidebarRow {
    fn id(&self) -> &str {
        match self {
            Self::Thread(s) => s.id.as_str(),
            Self::External(s) => s.id.as_str(),
        }
    }
}

/// Row geometry + team nesting metadata for [`SidebarThreadItem::from_wire`]:
/// `indent` offsets members under their leader, `nested` draws the left guide
/// rail.
#[derive(Clone, Copy)]
struct RowNesting {
    indent: gpui::Pixels,
    team_leader: bool,
    team_collapsed: bool,
    /// Nested row (team member or fork child): draws the left guide rail.
    nested: bool,
}

/// One ordered sidebar row plus its team nesting metadata: `indent` offsets
/// members under their leader, `team_leader` marks a row that can collapse
/// its member group, `team_collapsed` the fold state.
#[derive(Clone)]
struct ThreadRender {
    row: SidebarRow,
    indent: f32,
    team_leader: bool,
    team_collapsed: bool,
}

/// Events the sidebar emits to the Workspace.
#[derive(Debug, Clone)]
pub enum SidebarEvent {
    OpenThread(String),
    NewThread,
    /// New thread bound to a specific project path.
    NewThreadWithProject(PathBuf),
    /// User clicked archive/unarchive. The bool is the new archived state.
    ArchiveThread(String, bool),
    /// User set or cleared a thread row's tag (overflow menu / chip clear
    /// button). `None` removes the tag.
    SetThreadTag(String, Option<String>),
    /// Launch an external agent CLI session with a user-picked provider +
    /// model (the cascade wizard's terminal action). The kind identifies the
    /// agent (`claude` / `codex` / `copilot`); the strings are provider name +
    /// model id; the optional wire key pins the endpoint variant of the model
    /// (`anthropic` / `responses` / `completions`); the optional PathBuf is
    /// the project path to use as the CLI's cwd (when launched from a
    /// project folder's menu).
    SpawnExternalSession(
        crate::external_session::SessionKind,
        String,
        String,
        Option<String>,
        Option<PathBuf>,
    ),
    /// Launch a plain PTY session with no cx provider injection — the user's
    /// shell (`Terminal`). The optional PathBuf is the project path to use
    /// as the session's cwd (when launched from a project folder's menu);
    /// `None` falls back to the workspace cwd.
    SpawnPlainSession(crate::external_session::SessionKind, Option<PathBuf>),
    /// Launch VS Code with injection resolved from the persisted
    /// `vscode_app:` settings (single entry — no provider/model choice at
    /// launch time). The optional PathBuf is the project path the menu was
    /// opened from — VS Code opens that directory (falls back to the
    /// workspace cwd in the handler).
    LaunchVSCode(Option<PathBuf>),
    /// Switch the main area to an already-running external session.
    OpenExternalSession(String),
    /// Archive an external session from the sidebar row's hover action (the
    /// unified "Inbox" button threads also use): kill the agent and drop it
    /// from the sidebar — the same path as closing the tab.
    ArchiveExternalSession(String),
    /// User dragged one thread row to a new position in its partition. The
    /// anchor is the row the dragged one lands in front of; `None` appends to
    /// the end of the partition.
    MoveThread {
        id: String,
        before_id: Option<String>,
    },
    /// User dragged one project folder to a new position in the Projects
    /// section, with the same anchor semantics as [`SidebarEvent::MoveThread`].
    MoveFolder {
        path: PathBuf,
        before_path: Option<PathBuf>,
    },
    /// User removed a project folder from the sidebar (project action menu).
    /// The store unregisters the path: the folder disappears and its threads
    /// fall back to the loose Conversations list. Conversation history is
    /// never touched.
    RemoveProject(PathBuf),
}

/// Project one partition's rows into render order. **The caller hands rows in
/// display order already** (the client's view account, seeded from the server's
/// durable manual account) — this function never sorts, because a sort on any
/// timestamp key is what made rows drift.
///
/// Each partition renders three bands: pinned threads, then the live external
/// CLI/terminal sessions, then the remaining threads. Externals own no account
/// (the server never sees them) and their spawn time never changes, so their
/// band is stable without ever being compared against a thread timestamp.
///
/// Every leader is followed by its indented member subtree in the same incoming
/// order; orphans and cycles stay top-level (`depth` is zeroed for them at the
/// store); a collapsed leader hides its subtree.
fn team_forest(
    team_collapsed: &HashSet<String>,
    threads: &[ThreadListItem],
    externals: &[crate::external_session::ExternalSessionSummary],
) -> Vec<ThreadRender> {
    let mut members: HashMap<&str, Vec<&ThreadListItem>> = HashMap::new();
    let ids: HashSet<&str> = threads.iter().map(|s| s.id.as_str()).collect();
    for s in threads {
        if s.depth > 0
            && let Some(parent) = s.parent_id.as_deref()
        {
            members.entry(parent).or_default().push(s);
        }
    }
    let top: Vec<&ThreadListItem> = threads
        .iter()
        .filter(|s| {
            // A member whose leader lives in another partition (e.g. an
            // archived leader) cannot nest here: emit it top-level like the
            // webview forest, so archiving a leader never hides its members.
            s.depth == 0 || !s.parent_id.as_deref().is_some_and(|p| ids.contains(p))
        })
        .collect();
    // A stable re-band: the server's snapshot already leads with pinned rows,
    // but the client's promotion can lift an unpinned row above one, so the
    // band boundary is re-applied here (in one place, never per call site).
    let (pinned, rest): (Vec<&ThreadListItem>, Vec<&ThreadListItem>) =
        top.into_iter().partition(|s| s.pinned);
    let mut externals: Vec<crate::external_session::ExternalSessionSummary> = externals.to_vec();
    externals.sort_by_key(|s| std::cmp::Reverse(s.created_at));

    let mut out = Vec::new();
    for row in pinned
        .into_iter()
        .map(|s| SidebarRow::Thread((*s).clone()))
        .chain(externals.into_iter().map(SidebarRow::External))
        .chain(rest.into_iter().map(|s| SidebarRow::Thread((*s).clone())))
    {
        let id = row.id().to_string();
        // Only threads can lead a team; external CLI sessions never do.
        let team_leader = match &row {
            SidebarRow::Thread(s) => members.contains_key(s.id.as_str()),
            SidebarRow::External(_) => false,
        };
        out.push(ThreadRender {
            row,
            indent: 0.0,
            team_leader,
            team_collapsed: team_collapsed.contains(&id),
        });
        if team_leader && !team_collapsed.contains(&id) {
            let kids = members.get(id.as_str()).cloned().unwrap_or_default();
            for kid in kids {
                push_member(team_collapsed, &mut out, kid, 1.0, &members);
            }
        }
    }
    out
}

/// Render-side nesting cap, mirroring the store's `MAX_TEAM_DEPTH`. The
/// store zeroes cycle/orphan depths, so a deep tree here means corrupt wire
/// data; the cap keeps the recursion from stacking forever as a last line of
/// defense.
const MAX_TEAM_RENDER_DEPTH: f32 = 8.0;

/// Append a member row and its subtree (recursively) at the given indent.
fn push_member(
    team_collapsed: &HashSet<String>,
    out: &mut Vec<ThreadRender>,
    s: &ThreadListItem,
    depth: f32,
    members: &HashMap<&str, Vec<&ThreadListItem>>,
) {
    let has_kids = members.contains_key(s.id.as_str());
    out.push(ThreadRender {
        row: SidebarRow::Thread(s.clone()),
        indent: depth * 14.0,
        team_leader: has_kids,
        team_collapsed: team_collapsed.contains(&s.id),
    });
    if has_kids && !team_collapsed.contains(&s.id) && depth < MAX_TEAM_RENDER_DEPTH {
        // Members follow their leader in the incoming display order; no sort.
        let kids = members.get(s.id.as_str()).cloned().unwrap_or_default();
        for kid in kids {
            push_member(team_collapsed, out, kid, depth + 1.0, members);
        }
    }
}

/// Whether an external session projects into the loose Conversations list
/// instead of a folder group: unbound sessions, and sessions bound to a
/// path that is not a registered project (a removed folder, or a session
/// cwd never bound as a project). Mirrors the thread-side partitioning
/// rule in [`Sidebar::render`].
fn external_session_is_loose(project: Option<&std::path::Path>, known_projects: &[String]) -> bool {
    project.is_none_or(|p| {
        !known_projects
            .iter()
            .any(|kp| kp.as_str() == p.to_string_lossy().as_ref())
    })
}

pub struct Sidebar {
    /// The gateway client (U2 list source + GW5 badge source): rows are its
    /// wire `ThreadListItem`s, and their unread badges prefer the leaves'
    /// client-owned mirrors. Bound by the workspace after construction; an
    /// unbound sidebar (tests) renders no thread rows.
    mux: Option<gpui::Entity<crate::multiplexer::SessionMultiplexer>>,
    selected: Option<String>,
    /// The thread that was selected immediately before `selected`; its row
    /// plays a fade-out wash while the new row's wash fades in, so selection
    /// reads as the wash sliding from the old row to the new one.
    prev_selected: Option<String>,
    /// Bumped on every selection change. Embedded in each row's animation id
    /// so gpui treats it as a fresh animation and replays 0→1 (its element
    /// state is keyed by id).
    select_gen: u64,
    /// Project paths whose folder group is collapsed; absent means expanded.
    /// Mirrored into [`Sidebar::view`] so the sidebar reopens as it was left.
    collapsed: HashSet<String>,
    /// Team leader ids whose member group is collapsed; absent means
    /// expanded. Mirrors `collapsed` but per leader row instead of folder, and
    /// persisted alongside it.
    team_collapsed: HashSet<String>,
    /// The client's view state: ordering mode, per-partition display order,
    /// observed interaction stamps, and the two collapse sets.
    view: crate::sidebar_view::SidebarView,
    /// The mode as of the previous paint — the edge that triggers the one
    /// complete recency sort when the user switches into `Last updated`.
    prev_order_by: crate::sidebar_view::OrderBy,
    /// list. Merged into the Conversations list by recency (an `external:` id
    /// in `selected` highlights the active one).
    external_sessions: Vec<crate::external_session::ExternalSessionSummary>,
    /// Whether the new-session `PopupMenu` (Manox / Claude Code / Codex /
    /// GitHub Copilot) is open.
    new_session_open: bool,
    new_session_menu: Option<Entity<PopupMenu>>,
    new_session_menu_sub: Option<Subscription>,
    /// The project path the menu was opened from. `None` when opened from
    /// the Conversations header; `Some` when opened from a project folder's
    /// ellipsis button. The menu closures read this to decide whether to
    /// emit `NewThread` vs `NewThreadWithProject`, to pass the project path
    /// as the CWD for external CLI sessions, and to identify the folder the
    /// Remove-project row unregisters.
    new_session_project: Option<PathBuf>,
    /// The single inline tag edit in flight (one row at a time); `None`
    /// when no row is editing its tag.
    tag_edit: Option<TagEdit>,
    /// The thread row whose three-dot overflow menu is open (one at a
    /// time); mirrors the new-session menu's open flag.
    row_menu_open: Option<String>,
    row_menu: Option<Entity<PopupMenu>>,
    row_menu_sub: Option<Subscription>,
    /// The view-options popup (session ordering mode) and its dismissal
    /// subscription; one at a time, mirroring the row menu.
    view_menu_open: bool,
    view_menu: Option<Entity<PopupMenu>>,
    view_menu_sub: Option<Subscription>,
    /// The thread row currently under a drag, with the insertion line's host.
    drag_row: Option<RowDrag>,
    /// The project folder currently under a drag, same shape.
    drag_folder: Option<RowDrag>,
    /// The ids each partition last rendered, in display order: a drop resolves
    /// its anchor against what the user actually sees.
    displayed: HashMap<String, Vec<String>>,
    /// The folder paths as last rendered, in order — a folder drop resolves its
    /// anchor against what the user sees.
    folder_order: Vec<String>,
    /// Partitions whose overflow the user revealed for this mount. Transient by
    /// design: closing a folder clears it, so a reopened folder returns to the
    /// bounded projection.
    revealed: HashSet<String>,
    /// Live width driven by dragging the divider on the right edge. Updated
    /// from the owning `Workspace` on every drag-move tick.
    width: Pixels,
    /// Scroll offset + child-bounds reader for the sidebar scroll body; the
    /// sticky section-header overlay reads it to decide which header to show.
    scroll_handle: ScrollHandle,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    /// Bind the gateway client (U2 list source, GW5 badge source): rows are
    /// the multiplexer's wire `ThreadListItem`s, and their unread badges
    /// prefer the leaves' client-owned mirrors over the (deprecated,
    /// constant-false) row flag.
    pub fn bind_multiplexer(&mut self, mux: gpui::Entity<crate::multiplexer::SessionMultiplexer>) {
        self.mux = Some(mux);
    }

    pub fn new(width: Pixels, _cx: &mut Context<Self>) -> Self {
        // The view file carries the ordering mode, the display accounts and the
        // two collapse sets; a missing or corrupt file is the empty default.
        let view = sidebar_view::load();
        Self {
            mux: None,
            selected: None,
            collapsed: view.collapsed_folders.iter().cloned().collect(),
            team_collapsed: view.collapsed_teams.iter().cloned().collect(),
            prev_selected: None,
            select_gen: 0,
            view: view.clone(),
            // Seeded equal to the loaded mode: the first paint is not a switch
            // into `Last updated`, so it must not force a complete re-sort.
            prev_order_by: view.order_by,
            external_sessions: Vec::new(),
            new_session_open: false,
            new_session_menu: None,
            new_session_menu_sub: None,
            new_session_project: None,
            tag_edit: None,
            row_menu_open: None,
            row_menu: None,
            row_menu_sub: None,
            view_menu_open: false,
            view_menu: None,
            view_menu_sub: None,
            drag_row: None,
            drag_folder: None,
            displayed: HashMap::new(),
            folder_order: Vec::new(),
            revealed: HashSet::new(),
            width,
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// Whether the view-options popup is mounted open.
    pub fn view_menu_is_open(&self) -> bool {
        self.view_menu_open
    }

    /// Open the view-options popup: the session ordering mode. The mode is
    /// client view state (never a kernel write) — it decides whether activity may
    /// promote a row, nothing else.
    fn open_view_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_view_menu();
        self.close_new_session_menu();
        let sidebar = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            // The active mode carries the check mark; the popup is the mode's
            // only editor, and choosing a mode never touches the server.
            let build = |menu: PopupMenu, order_by: OrderBy, label: &'static str| {
                let target = sidebar.clone();
                let active = sidebar
                    .upgrade()
                    .is_some_and(|entity| entity.read(_cx).view.order_by == order_by);
                menu.item(PopupMenuItem::new(i18n::t(label)).checked(active).on_click(
                    move |_, _, cx| {
                        let _ = target.update(cx, |this, cx| {
                            this.close_view_menu();
                            this.set_order_by(order_by, cx);
                        });
                    },
                ))
            };
            let menu = build(
                menu.max_w(gpui::px(240.)),
                OrderBy::Manual,
                "sidebar-order-manual",
            );
            build(menu, OrderBy::Updated, "sidebar-order-updated")
        });
        self.view_menu_sub = Some(cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.view_menu_open = false;
            this.view_menu = None;
            this.view_menu_sub = None;
            cx.notify();
        }));
        self.view_menu_open = true;
        self.view_menu = Some(menu);
        cx.notify();
    }

    fn close_view_menu(&mut self) {
        self.view_menu_open = false;
        self.view_menu = None;
        self.view_menu_sub = None;
    }

    /// Switch the ordering mode. Entering `Last updated` is the edge that runs
    /// the one complete recency sort; leaving it keeps every current position
    /// and only stops further promotion. Landing back in `Manual` additionally
    /// reconciles the server account to those kept positions, so the order the
    /// user is now looking at is the order a `Manual` drag edits against.
    fn set_order_by(&mut self, order_by: OrderBy, cx: &mut Context<Self>) {
        if self.view.order_by == order_by {
            return;
        }
        self.view.order_by = order_by;
        self.save_view(cx);
        if order_by == OrderBy::Manual {
            self.reconcile_manual_account(cx);
        }
        cx.notify();
    }

    /// Replay the drift between the client's view account and the server's
    /// committed order as one minimal batch of `MoveThread` moves.
    ///
    /// The `Last updated` account drifts by design — promotions at the head,
    /// view-local drags — and landing in `Manual` keeps those positions. The
    /// server has to catch up before the user's first `Manual` drag: its
    /// anchors are resolved against the visible order, so a move committed
    /// over the drift would land the row somewhere the user is not looking
    /// at. `Manual` drags themselves emit and apply the same move locally, so
    /// the accounts stay in step — this switch edge is the only place drift
    /// can exist. Rows the wire list no longer knows are skipped by
    /// [`sidebar_view::reconcile_moves`]; a replay whose emits are lost
    /// (gateway down at switch time) leaves the drift until the next switch,
    /// while the client keeps showing the user's order either way.
    fn reconcile_manual_account(&mut self, cx: &mut Context<Self>) {
        let Some(mux) = self.mux.as_ref() else {
            return;
        };
        let items = mux.read(cx).thread_list().to_vec();
        let known = mux.read(cx).known_projects().to_vec();
        for (partition, server) in wire_partition_orders(&items, &known) {
            let Some(target) = self.view.account.get(&partition) else {
                continue;
            };
            for (id, before) in sidebar_view::reconcile_moves(&server, target) {
                cx.emit(SidebarEvent::MoveThread {
                    id,
                    before_id: before,
                });
            }
        }
    }

    /// Persist the two collapse sets into the view file. Called after a folder
    /// or leader toggle, so a folded sidebar survives a restart.
    fn persist_collapse(&mut self, cx: &mut Context<Self>) {
        self.view.collapsed_folders = self.collapsed.iter().cloned().collect();
        self.view.collapsed_teams = self.team_collapsed.iter().cloned().collect();
        self.save_view(cx);
    }

    /// Replace the external-session projection. Called by the Workspace
    /// whenever the canonical set changes (spawn / close). The sidebar never
    /// owns the live sessions — it only renders this snapshot.
    pub fn set_external_sessions(
        &mut self,
        sessions: Vec<crate::external_session::ExternalSessionSummary>,
        cx: &mut Context<Self>,
    ) {
        self.external_sessions = sessions;
        cx.notify();
    }

    /// Update the rendered width. Called by the owning `Workspace` on every
    /// divider drag-move tick; the new value takes effect on the next render.
    pub fn set_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if self.width == width {
            return;
        }
        self.width = width;
        cx.notify();
    }

    /// The Conversations section header with its `+` new-session button.
    /// `id_prefix` disambiguates element ids when the header exists twice in
    /// one tree (in-flow copy + sticky overlay), and `dropdown` gates the
    /// deferred new-session menu so only the visible copy anchors it.
    fn conversations_section_header(
        &self,
        theme: &Theme,
        id_prefix: &str,
        dropdown: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Both header actions sit in one element: the new-session `+` and the
        // ordering-mode trigger. Each mounts its own popup only on the in-flow
        // copy (`dropdown`), so the sticky overlay never duplicates a menu.
        let actions = gpui::div()
            .relative()
            .flex()
            .items_center()
            .gap_0p5()
            .child(
                gpui::div()
                    .relative()
                    .child(
                        Button::new(format!("{id_prefix}-conv-plus"))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .tooltip(i18n::t("sidebar-new-chat"))
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                if this.new_session_open {
                                    this.close_new_session_menu();
                                } else {
                                    this.open_new_session_menu(None, window, cx);
                                }
                                cx.notify();
                            })),
                    )
                    // Deferred inside this relative wrapper so it paints after
                    // sibling rows (z-order) while staying positioned just
                    // below the button (`top_full()` is 100% of this wrapper's
                    // height).
                    .children(
                        (dropdown && self.new_session_open && self.new_session_project.is_none())
                            .then(|| {
                                self.render_new_session_dropdown(
                                    format!("{id_prefix}-new-session-dropdown").into(),
                                )
                            })
                            .flatten(),
                    ),
            )
            .child(
                gpui::div()
                    .relative()
                    .child(
                        Button::new(format!("{id_prefix}-view-options"))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Menu)
                            .tooltip(i18n::t("sidebar-view-options"))
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                if this.view_menu_open {
                                    this.close_view_menu();
                                } else {
                                    this.open_view_menu(window, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .children(
                        (dropdown && self.view_menu_open)
                            .then(|| self.render_view_menu_dropdown())
                            .flatten(),
                    ),
            );
        section_header(
            i18n::t("sidebar-section-conversations"),
            theme,
            Some(actions.into_any_element()),
        )
    }

    /// Open the session-menu popup. `project` is `None` when opened from the
    /// Conversations header — the flat new-session menu — and `Some(path)`
    /// when opened from a project folder's ellipsis button — the project
    /// action menu (new-session submenu / Terminal / VS Code / Remove
    /// Project). The path determines the CWD for external CLI sessions and
    /// whether the new Manox session binds to the project.
    fn open_new_session_menu(
        &mut self,
        project: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_session_project = project.clone();
        let theme = cx.theme().clone();
        let sidebar = cx.entity().downgrade();
        // The menu-build closures run EAGERLY inside this Sidebar update,
        // so the cascade builders get the mux handle up front — reading
        // the Sidebar entity from inside them double-leases it (the
        // acceptance-run crash).
        let mux = self.mux.clone();
        let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
            if project.is_some() {
                return build_project_menu(menu, &sidebar, &mux, &theme, window, cx);
            }
            let mut menu = menu
                .max_w(gpui::px(280.))
                .label(i18n::t("sidebar-new-session-label"));
            let sidebar_new = sidebar.clone();
            menu = menu.item(
                PopupMenuItem::new(i18n::t("sidebar-new-session-manox"))
                    .icon(
                        Icon::default()
                            .path("icons/manox.svg")
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(move |_, _window, cx| {
                        let _ = sidebar_new.update(cx, |this, cx| {
                            let project = this.new_session_project.take();
                            this.close_new_session_menu();
                            if let Some(p) = project {
                                cx.emit(SidebarEvent::NewThreadWithProject(p));
                            } else {
                                cx.emit(SidebarEvent::NewThread);
                            }
                            cx.notify();
                        });
                    }),
            );
            // Terminal: a plain shell session — no provider/model cascade,
            // the click spawns immediately in the workspace cwd. Shares the
            // project menu's `sidebar-new-terminal` label — both menus create
            // a fresh terminal.
            {
                let kind = crate::external_session::SessionKind::Terminal;
                let sidebar_plain = sidebar.clone();
                menu = menu.item(
                    PopupMenuItem::new(i18n::t("sidebar-new-terminal"))
                        .icon(
                            Icon::default()
                                .path(kind.icon_asset())
                                .small()
                                .text_color(theme.muted_foreground),
                        )
                        .on_click(move |_, _window, cx| {
                            let _ = sidebar_plain.update(cx, |this, cx| {
                                let project = this.new_session_project.take();
                                this.close_new_session_menu();
                                cx.emit(SidebarEvent::SpawnPlainSession(kind, project));
                                cx.notify();
                            });
                        }),
                );
            }
            for kind in [
                crate::external_session::SessionKind::ClaudeCode,
                crate::external_session::SessionKind::Codex,
                crate::external_session::SessionKind::GithubCopilot,
            ] {
                let sidebar = sidebar.clone();
                let theme = theme.clone();
                let mux_agent = mux.clone();
                let label = kind.label();
                let agent_id = kind.agent_id();
                menu = menu.submenu_with_icon(
                    Some(
                        Icon::default()
                            .path(kind.icon_asset())
                            .small()
                            .text_color(theme.muted_foreground),
                    ),
                    label,
                    window,
                    cx,
                    move |submenu, window, cx| {
                        build_agent_model_cascade(
                            submenu, kind, agent_id, &sidebar, &mux_agent, window, cx,
                        )
                    },
                );
            }
            // VS Code: single entry — injection resolves from the persisted
            // `vscode_app:` settings (Settings → 外部工具 → Visual Studio
            // Code.app); no provider/model cascade. Launches the VS Code
            // desktop app opening the project directory the menu was opened
            // from. Disabled outright when VS Code is not installed (parity
            // with the 工具 → VS Code system menu).
            {
                let sidebar = sidebar.clone();
                let icon = Icon::default()
                    .path("icons/vscode.svg")
                    .small()
                    .text_color(theme.muted_foreground);
                menu = menu.item(
                    PopupMenuItem::new("VS Code")
                        .icon(icon)
                        .disabled(!manox_ext_agents::vscode_app_installed())
                        .on_click(move |_, _window, cx| {
                            let _ = sidebar.update(cx, |this, cx| {
                                let project = this.new_session_project.clone();
                                cx.emit(SidebarEvent::LaunchVSCode(project));
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        });
        let sub = cx.subscribe(&menu, |this, _menu, _: &DismissEvent, cx| {
            this.close_new_session_menu();
            cx.notify();
        });
        self.new_session_open = true;
        self.new_session_menu = Some(menu);
        self.new_session_menu_sub = Some(sub);
    }

    fn close_new_session_menu(&mut self) {
        self.new_session_open = false;
        self.new_session_menu = None;
        self.new_session_menu_sub = None;
        self.new_session_project = None;
    }

    /// Open a thread row's three-dot overflow menu (Archive + Tag), anchored
    /// below its trigger button. One row menu at a time — opening one closes
    /// the previous.
    fn open_row_menu(
        &mut self,
        id: String,
        archived: bool,
        has_tag: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_row_menu();
        let theme = cx.theme().clone();
        let sidebar = cx.entity().downgrade();
        let open_id = id.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            let archive_label = if archived {
                i18n::t("sidebar-unarchive")
            } else {
                i18n::t("sidebar-archive")
            };
            let sidebar_archive = sidebar.clone();
            let id_archive = id.clone();
            let menu = menu.item(
                PopupMenuItem::new(archive_label)
                    .icon(
                        Icon::new(IconName::Inbox)
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(move |_, _, cx| {
                        let _ = sidebar_archive.update(cx, |this, cx| {
                            this.close_row_menu();
                            cx.emit(SidebarEvent::ArchiveThread(id_archive.clone(), !archived));
                            cx.notify();
                        });
                    }),
            );
            let tag_label = if has_tag {
                i18n::t("sidebar-thread-tag-rename")
            } else {
                i18n::t("sidebar-thread-tag-add")
            };
            let mode = if has_tag {
                TagEditMode::Renaming
            } else {
                TagEditMode::Adding
            };
            let sidebar_tag = sidebar.clone();
            let id_tag = id.clone();
            menu.item(
                PopupMenuItem::new(tag_label)
                    .icon(
                        Icon::default()
                            .path("icons/tag.svg")
                            .small()
                            .text_color(theme.muted_foreground),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = sidebar_tag.update(cx, |this, cx| {
                            this.close_row_menu();
                            this.begin_tag_edit(id_tag.clone(), mode, window, cx);
                        });
                    }),
            )
        });
        let sub = cx.subscribe(&menu, |this, _menu, _: &DismissEvent, cx| {
            this.close_row_menu();
            cx.notify();
        });
        self.row_menu_open = Some(open_id);
        self.row_menu = Some(menu);
        self.row_menu_sub = Some(sub);
    }

    fn close_row_menu(&mut self) {
        self.row_menu_open = None;
        self.row_menu = None;
        self.row_menu_sub = None;
    }

    /// Anchor the open row menu below its trigger button. Deferred so it
    /// paints above sibling rows and escapes the row's `overflow_hidden`;
    /// `top_full()` + `right_0()` hang it just under the button's wrapper.
    /// The view-options popup, hung under its header trigger the same deferred
    /// way the row menu is, so it escapes the scroll clip.
    fn render_view_menu_dropdown(&self) -> Option<AnyElement> {
        if !self.view_menu_open {
            return None;
        }
        let menu = self.view_menu.clone()?;
        Some(
            deferred(
                gpui::div()
                    .id("sidebar-view-options-dropdown")
                    .absolute()
                    .top_full()
                    .right_0()
                    .occlude()
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_row_menu_dropdown(&self, id: &str) -> Option<AnyElement> {
        if self.row_menu_open.as_deref() != Some(id) {
            return None;
        }
        let menu = self.row_menu.clone()?;
        Some(
            deferred(
                gpui::div()
                    .id(SharedString::from(format!("thread-menu-dropdown-{id}")))
                    .absolute()
                    .top_full()
                    .right_0()
                    .occlude()
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// Mount the inline tag input on a row — at most one edit in flight;
    /// starting a new one replaces the previous. Rename mode prefills the
    /// current tag. The input is focused immediately and clamped to
    /// [`MAX_THREAD_TAG_CHARS`] on every edit.
    fn begin_tag_edit(
        &mut self,
        id: String,
        mode: TagEditMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefill = (mode == TagEditMode::Renaming)
            .then(|| self.summary_tag(&id, cx))
            .flatten();
        let placeholder = i18n::t("sidebar-thread-tag-placeholder");
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder);
            if let Some(value) = prefill {
                state.set_value(value, window, cx);
            }
            state
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                // The pinned gpui-component input has no max-length
                // support; clamp every edit down to the tag ceiling.
                InputEvent::Change => {
                    input.update(cx, |state, cx| {
                        let value = state.value();
                        if value.chars().count() > MAX_THREAD_TAG_CHARS {
                            let truncated: String =
                                value.chars().take(MAX_THREAD_TAG_CHARS).collect();
                            state.set_value(truncated, window, cx);
                        }
                    });
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    this.commit_tag_edit(cx);
                }
                _ => {}
            },
        );
        self.tag_edit = Some(TagEdit {
            id,
            input: input.clone(),
            _sub: sub,
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    /// Commit the in-flight tag edit: a non-empty value emits
    /// `SetThreadTag`, an empty one discards silently. Either way the input
    /// unmounts. Idempotent — a blur may race in after an Enter commit.
    fn commit_tag_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.tag_edit.take() else {
            return;
        };
        let value = edit.input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            cx.emit(SidebarEvent::SetThreadTag(edit.id, Some(value)));
        }
        cx.notify();
    }

    fn cancel_tag_edit(&mut self, cx: &mut Context<Self>) {
        if self.tag_edit.take().is_some() {
            cx.notify();
        }
    }

    /// The persisted tag for a row (U2 cross-domain #1: it rides the wire
    /// row — the wire list spans both store partitions, so archived rows'
    /// tags resolve too).
    fn summary_tag(&self, id: &str, cx: &App) -> Option<String> {
        self.mux.as_ref().and_then(|m| {
            m.read(cx)
                .thread_list()
                .iter()
                .find(|i| i.id == id)
                .and_then(|i| i.tag.clone())
        })
    }

    /// Build the session-menu dropdown anchored below the trigger button
    /// that opened it. Deferred so it paints above sibling rows; `top_full()` is
    /// 100% of the wrapping `.relative()` div, so the menu sits just under the
    /// button rather than at the sidebar's bottom edge.
    fn render_new_session_dropdown(&self, id: SharedString) -> Option<AnyElement> {
        self.new_session_menu.clone().map(|menu| {
            deferred(
                gpui::div()
                    .id(id)
                    .absolute()
                    .top_full()
                    .right_0()
                    .occlude()
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element()
        })
    }

    /// Mark the currently selected thread id (back-filled by Workspace on switch/new, for highlight).
    pub fn set_selected(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.selected == id {
            return;
        }
        self.prev_selected = self.selected.take();
        self.selected = id;
        self.select_gen = self.select_gen.wrapping_add(1);
        cx.notify();
    }

    /// The currently selected thread id (the highlight source).
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Project one partition's rows into render order (no sorting here: the
    /// incoming order is the display order, and the three bands are assembled by
    /// [`team_forest`]).
    fn order_rows(
        &self,
        threads: &[ThreadListItem],
        externals: &[crate::external_session::ExternalSessionSummary],
    ) -> Vec<ThreadRender> {
        team_forest(&self.team_collapsed, threads, externals)
    }

    /// Re-order each partition's thread rows by the client's view account before
    /// anything is projected.
    ///
    /// This is where the reference's browser-local order lives: `Manual` renders
    /// the server's durable manual account verbatim, `Last updated` keeps a
    /// display order of its own that a real user interaction moves exactly one
    /// row through. A partition whose account changed marks the view file for
    /// writing; a partition that merely re-reads the same order writes nothing.
    fn apply_view_order(
        &mut self,
        projects: &mut [(String, Vec<ThreadListItem>)],
        loose: &mut Vec<ThreadListItem>,
        cx: &mut Context<Self>,
    ) {
        let switched =
            self.view.order_by == OrderBy::Updated && self.prev_order_by != OrderBy::Updated;
        self.prev_order_by = self.view.order_by;
        let mut changed = false;
        for (partition, rows) in projects
            .iter_mut()
            .map(|(path, items)| (path.as_str(), items))
            .chain(std::iter::once((crate::sidebar_view::LOOSE, loose)))
        {
            // Owned (id, stamp) pairs first: `rows` must stay free to reorder
            // while the account is being computed from this snapshot.
            let stamps: Vec<(String, i64)> = rows
                .iter()
                .map(|s| (s.id.clone(), s.updated_at as i64))
                .collect();
            let input: Vec<sidebar_view::Row<'_>> = stamps
                .iter()
                .map(|(id, stamp)| sidebar_view::Row {
                    id: id.as_str(),
                    stamp: *stamp,
                })
                .collect();
            let stored = self.view.account.get(partition).cloned();
            let observed = self
                .view
                .observed
                .get(partition)
                .cloned()
                .unwrap_or_default();
            if let Some(order) = sidebar_view::next_account(
                &input,
                stored.as_deref(),
                &observed,
                self.view.order_by,
                switched || stored.is_none(),
            ) {
                let rank: HashMap<&str, usize> = order
                    .iter()
                    .enumerate()
                    .map(|(i, id)| (id.as_str(), i))
                    .collect();
                rows.sort_by_key(|s| rank.get(s.id.as_str()).copied().unwrap_or(usize::MAX));
                self.view.account.insert(partition.to_string(), order);
                changed = true;
            }
            sidebar_view::observe(&mut self.view, partition, &input);
            // A drop resolves its anchor against the partition's live order,
            // recorded here rather than at paint time: the fold hides rows, the
            // account they belong to does not.
            let order = self
                .view
                .account
                .get(partition)
                .cloned()
                .unwrap_or_default();
            let live: Vec<String> = input.iter().map(|r| r.id.to_string()).collect();
            let merged = sidebar_view::reconciled(
                &live
                    .iter()
                    .map(|id| sidebar_view::Row {
                        id: id.as_str(),
                        stamp: 0,
                    })
                    .collect::<Vec<_>>(),
                Some(&order),
            );
            self.displayed.insert(partition.to_string(), merged);
        }
        if changed {
            self.save_view(cx);
        }
    }

    /// Persist the view state. A failed write is logged and dropped: this file is
    /// presentation state, and the in-memory order stays correct for the run.
    fn save_view(&mut self, _cx: &mut Context<Self>) {
        if let Err(error) = sidebar_view::save(&self.view) {
            tracing::warn!(error = %error, "failed to persist the sidebar view state");
        }
    }

    /// Resolve the pointer's boundary on one row: the insertion line hugs the
    /// half of the row the pointer is in, so a top-half hit lands the drag in
    /// front of this row and a bottom-half hit lands it behind. Outside the
    /// row's own bounds there is no boundary — `on_drag_move` fires for every
    /// move of a live drag, not only the hovered element.
    fn drag_boundary(bounds_top: Pixels, height: Pixels, pointer_y: Pixels) -> Option<DragEdge> {
        let bottom = bounds_top + height;
        if pointer_y < bounds_top || pointer_y > bottom {
            return None;
        }
        Some(if pointer_y < bounds_top + height / 2.0 {
            DragEdge::Top
        } else {
            DragEdge::Bottom
        })
    }

    /// The id the dragged row lands in front of, resolved against the order the
    /// user is looking at: the row after the line's host, or `None` when the
    /// line sits at the end of the partition.
    fn anchor_after(drag: &RowDrag, shown: &[String]) -> Option<String> {
        let at = shown.iter().position(|id| *id == drag.line_on)?;
        match drag.edge {
            DragEdge::Top => shown.get(at).cloned(),
            DragEdge::Bottom => shown.get(at + 1).cloned(),
        }
    }

    /// Commit a thread drop: reorder the partition's display order locally, so
    /// the row lands immediately, and hand the same move to the server's manual
    /// account. A redundant move (self-anchored, already in place) emits
    /// nothing and writes nothing. The server leg runs only in `Manual` — see
    /// the emit gate below.
    fn commit_row_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag_row.take() else {
            return;
        };
        if drag.dragged == drag.line_on {
            cx.notify();
            return;
        }
        let Some(partition) = self.partition_of_row(&drag.dragged, cx) else {
            cx.notify();
            return;
        };
        let mut shown = self.displayed.get(&partition).cloned().unwrap_or_default();
        let anchor = Self::resolve_row_anchor(&drag, &shown);
        let Some(next) = sidebar_view::move_in_account(&shown, &drag.dragged, anchor.as_deref())
        else {
            cx.notify();
            return;
        };
        shown = next;
        self.displayed.insert(partition.clone(), shown.clone());
        self.view.account.insert(partition, shown);
        self.save_view(cx);
        // `Manual` is the server account's single writer. In `Last updated`
        // the head promotions have already split the view order from the
        // server's account, so the visible anchor can resolve to a move that
        // is a server-side no-op — the drag stays pure view state (this
        // client account persists, so the reorder survives a restart) and the
        // durable account only ever records moves the user laid out in
        // `Manual`.
        if self.view.order_by == OrderBy::Manual {
            cx.emit(SidebarEvent::MoveThread {
                id: drag.dragged,
                before_id: anchor,
            });
        }
        cx.notify();
    }

    /// The row drop's anchor: a line under the last row means "append"
    /// (`None`); anywhere else it means "in front of whatever follows the
    /// line's host".
    fn resolve_row_anchor(drag: &RowDrag, shown: &[String]) -> Option<String> {
        let at_end = matches!(drag.edge, DragEdge::Bottom)
            && shown.last().is_some_and(|id| *id == drag.line_on);
        if at_end {
            None
        } else {
            Self::anchor_after(drag, shown)
        }
    }

    /// Commit a folder drop. Folder order is server-owned state with no client
    /// account, so this only forwards the move.
    fn commit_folder_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag_folder.take() else {
            return;
        };
        if drag.dragged == drag.line_on {
            cx.notify();
            return;
        }
        match Self::resolve_folder_drop(&drag, &self.folder_order) {
            FolderDrop::Noop => cx.notify(),
            FolderDrop::Move(at) => {
                cx.emit(SidebarEvent::MoveFolder {
                    path: PathBuf::from(&drag.dragged),
                    before_path: at,
                });
                cx.notify();
            }
        }
    }

    /// Resolve a folder drop against the rendered folder order, mirroring the
    /// row path's no-op contract: only the line on the bottom edge of the
    /// last folder is a true append, and everything the row commit rejects
    /// through `move_in_account` is a no-op here too — a drop back onto the
    /// dragged folder's own boundary, a drop that would re-insert the folder
    /// where it already sits, and a dragged folder that left the rendered
    /// order mid-drag. None of these may decay into an append: upstream
    /// treats `before = None` as a real move to the tail.
    fn resolve_folder_drop(drag: &RowDrag, order: &[String]) -> FolderDrop {
        if !order.iter().any(|p| p == &drag.line_on) {
            return FolderDrop::Noop;
        }
        let anchor = Self::anchor_after(drag, order);
        match sidebar_view::move_in_account(order, &drag.dragged, anchor.as_deref()) {
            Some(_) => FolderDrop::Move(anchor.map(PathBuf::from)),
            None => FolderDrop::Noop,
        }
    }

    /// The partition a row belongs to: its registered project, else the loose
    /// Conversations account. Same rule the server partitions by.
    fn partition_of_row(&self, id: &str, cx: &mut App) -> Option<String> {
        let mux = self.mux.as_ref()?;
        let list = mux.read(cx).thread_list().to_vec();
        let known = mux.read(cx).known_projects().to_vec();
        let row = list.iter().find(|r| r.id == id)?;
        let project = row.project.as_deref().unwrap_or_default();
        Some(
            if !project.is_empty() && known.iter().any(|k| k == project) {
                project.to_string()
            } else {
                crate::sidebar_view::LOOSE.to_string()
            },
        )
    }

    /// Render one partition's projected rows, folded to the quota with an
    /// explicit reveal affordance for the remainder.
    fn render_partition_rows(
        &self,
        account: &str,
        rows: Vec<ThreadRender>,
        env: &PartitionEnv<'_>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let indent_base = env.indent_base;
        let theme = env.theme;
        let slide = env.slide;
        let unread_map = env.unread_map;
        let selected = env.selected;
        let revealed = self.revealed.contains(account);
        let (shown, hidden) = folded_rows(&rows, revealed);
        let mut render = |tr: ThreadRender| {
            let is_selected = selected == Some(tr.row.id());
            match tr.row {
                SidebarRow::Thread(s) => render_thread_item(
                    &SidebarThreadItem::from_wire(
                        &s,
                        is_selected,
                        unread_map.get(&s.id).copied(),
                        RowNesting {
                            indent: indent_base + px(tr.indent),
                            team_leader: tr.team_leader,
                            team_collapsed: tr.team_collapsed,
                            nested: tr.indent > 0.0,
                        },
                        theme,
                    ),
                    slide,
                    self,
                    theme,
                    cx,
                ),
                SidebarRow::External(s) => render_thread_item(
                    &SidebarThreadItem::from_external(&s, is_selected, indent_base, theme),
                    slide,
                    self,
                    theme,
                    cx,
                ),
            }
        };
        let account = account.to_string();
        v_flex()
            .w_full()
            .gap_0p5()
            .children(shown.into_iter().map(&mut render))
            .children((hidden > 0).then(|| {
                Button::new(format!("show-more-{account}"))
                    .ghost()
                    .xsmall()
                    .label(i18n::t_count("sidebar-show-more", hidden as i64))
                    .text_color(theme.muted_foreground)
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        this.revealed.insert(account.clone());
                        cx.notify();
                    }))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// A collapsible project folder: a clickable header (chevron + folder icon +
    /// basename) over its indented conversation rows when expanded. The
    /// trailing ellipsis button opens the project action menu with the
    /// project path so the workspace can set the CWD for external CLI
    /// sessions.
    fn render_project_group(
        &self,
        path: &str,
        group: &[ThreadListItem],
        selected: Option<&str>,
        slide: &SlideCtx,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        // GW5 badge source: the leaves' client-owned unread mirrors.
        let unread_map = self
            .mux
            .as_ref()
            .map(|m| m.read(cx).unread_map(cx))
            .unwrap_or_default();
        let expanded = !self.collapsed.contains(path);
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string();
        let key: SharedString = path.to_string().into();
        // External sessions bound to this project folder — pulled from the
        // sidebar's projection rather than threaded through as an arg, so the
        // signature stays under clippy's argument limit.
        let externals: Vec<crate::external_session::ExternalSessionSummary> = self
            .external_sessions
            .iter()
            .filter(|s| s.project.as_deref() == Some(std::path::Path::new(path)))
            .cloned()
            .collect();

        let drag_path = path.to_string();
        let folder_drag_line = self
            .drag_folder
            .as_ref()
            .filter(|d| d.line_on == path && d.dragged != d.line_on)
            .map(|d| d.edge);
        let header = h_flex()
            .id(key.clone())
            .w_full()
            .px_2()
            .py_1p5()
            .gap_1()
            .items_center()
            .rounded(theme.radius)
            .hover(|s| s.bg(theme.accent.opacity(0.08)))
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(theme.muted_foreground),
            )
            .child(
                Icon::new(IconName::Folder)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child(name),
            )
            .child(
                gpui::div()
                    .relative()
                    .child(
                        Button::new(format!("project-menu-{key}"))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .tooltip(i18n::t("sidebar-project-menu"))
                            .on_click(cx.listener({
                                let path = path.to_string();
                                move |this, _ev, window, cx| {
                                    cx.stop_propagation();
                                    if this.new_session_open {
                                        this.close_new_session_menu();
                                    } else {
                                        this.open_new_session_menu(
                                            Some(PathBuf::from(path.clone())),
                                            window,
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                }
                            })),
                    )
                    // Only render the dropdown here when the menu was opened
                    // from *this* project folder's ellipsis button, so the
                    // menu anchors below the clicked button instead of the
                    // Conversations header's `+`.
                    .children(
                        (self.new_session_open
                            && self.new_session_project.as_deref()
                                == Some(std::path::Path::new(path)))
                        .then(|| {
                            self.render_new_session_dropdown(
                                format!("new-session-dropdown-{path}").into(),
                            )
                        })
                        .flatten(),
                    ),
            )
            .on_click(cx.listener({
                let path = path.to_string();
                move |this, _ev, _window, cx| {
                    let folding = this.collapsed.remove(&path);
                    if !folding {
                        this.collapsed.insert(path.clone());
                    }
                    // Closing a folder resets its transient reveal, so reopening
                    // it returns to the bounded projection.
                    if folding {
                        this.revealed.remove(&path);
                    }
                    // Which folders are folded survives a restart; which are
                    // revealed does not.
                    this.persist_collapse(cx);
                    cx.notify();
                }
            }))
            .relative()
            .children(folder_drag_line.map(|edge| {
                gpui::div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .h(px(2.))
                    .rounded_full()
                    .bg(theme.accent)
                    .when(edge == DragEdge::Top, |this| this.top(px(-1.)))
                    .when(edge == DragEdge::Bottom, |this| this.bottom(px(-1.)))
                    .into_any_element()
            }))
            .on_drag(
                DraggedFolderRow {
                    path: drag_path.clone(),
                },
                {
                    let payload = DraggedFolderRow {
                        path: drag_path.clone(),
                    };
                    move |_, _, _, cx| cx.new(|_| payload.clone())
                },
            )
            .on_drag_move::<DraggedFolderRow>(cx.listener(
                move |this, e: &DragMoveEvent<DraggedFolderRow>, _window, cx| {
                    let top = e.bounds.origin.y;
                    let Some(edge) =
                        Sidebar::drag_boundary(top, e.bounds.size.height, e.event.position.y)
                    else {
                        return;
                    };
                    let marker = RowDrag {
                        dragged: e.drag(cx).path.clone(),
                        line_on: drag_path.clone(),
                        edge,
                    };
                    if this.drag_folder.as_ref() != Some(&marker) {
                        this.drag_folder = Some(marker);
                        cx.notify();
                    }
                },
            ))
            .on_drop::<DraggedFolderRow>(cx.listener(
                move |this, folder: &DraggedFolderRow, _window, cx| {
                    if this
                        .drag_folder
                        .as_ref()
                        .is_some_and(|d| d.dragged == folder.path)
                    {
                        this.commit_folder_drag(cx);
                    }
                },
            ));

        let rows = expanded.then(|| {
            // The folder's threads (in view-account order) with this project's
            // external sessions as a fixed band between the pinned and unpinned
            // thread bands; team members nest indented under their leader.
            self.render_partition_rows(
                path,
                self.order_rows(group, &externals),
                &PartitionEnv {
                    selected,
                    unread_map: &unread_map,
                    slide,
                    theme: &theme,
                    indent_base: px(16.),
                },
                cx,
            )
        });

        v_flex()
            .w_full()
            .gap_0p5()
            .child(header)
            .children(rows)
            .into_any_element()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // gpui cancels a drag on any mouse-up that doesn't land inside a drop
        // target carrying the payload's type — release below the list, over a
        // folder header while dragging a thread, or outside the sidebar. No
        // `on_drop` runs in those cases, so the commit paths never fire and
        // the markers would survive the gesture, pinning the insertion line
        // and the ghosted source row until some later drag overwrote them.
        // The cancel always paints (gpui refreshes the window right after
        // clearing its `active_drag`), so pruning here cleans up in the same
        // frame the gesture dies — no repaint request needed.
        if !cx.has_active_drag() && (self.drag_row.is_some() || self.drag_folder.is_some()) {
            self.drag_row = None;
            self.drag_folder = None;
        }
        let theme = cx.theme().clone();
        // U2: the rows are the gateway's wire list (the multiplexer's
        // `ThreadListItem`s — §D.5 mirrors / `ListThreads` responses with
        // the `SessionStatus` deltas merged), never a kernel store read.
        let items: Vec<ThreadListItem> = self
            .mux
            .as_ref()
            .map(|m| m.read(cx).thread_list().to_vec())
            .unwrap_or_default();
        // U2 cross-domain #1: the grouping registry rides the wire (the
        // `HostEvent::Projects` mirror), not a workspace push.
        let known_projects = self
            .mux
            .as_ref()
            .map(|m| m.read(cx).known_projects().to_vec())
            .unwrap_or_default();
        let selected = self.selected.clone();
        // GW5 badge source: the leaves' client-owned unread mirrors.
        let unread_map = self
            .mux
            .as_ref()
            .map(|m| m.read(cx).unread_map(cx))
            .unwrap_or_default();

        let mut projects: Vec<(String, Vec<ThreadListItem>)> = Vec::new();
        let mut loose: Vec<ThreadListItem> = Vec::new();
        for s in &items {
            // Only REGISTERED projects become folder groups; a session cwd
            // that was never bound as a project (e.g. the default home dir)
            // stays in the loose Conversations list. U2 cross-domain #1:
            // the binding rides the wire row's project column.
            let project = s.project.as_deref().unwrap_or_default();
            if project.is_empty() || !known_projects.iter().any(|kp| kp == project) {
                loose.push(s.clone());
            } else if let Some(entry) = projects.iter_mut().find(|(p, _)| p == project) {
                entry.1.push(s.clone());
            } else {
                projects.push((project.to_string(), vec![s.clone()]));
            }
        }
        // Merge registered projects that have no active threads — they still
        // appear as empty folders so the user can start a new conversation
        // in the project without losing the folder reference. The folder
        // sequence itself is the server's committed order (`known_projects`
        // arrives in it), so no sorting happens here.
        for kp in &known_projects {
            if !projects.iter().any(|(p, _)| p == kp) {
                projects.push((kp.clone(), Vec::new()));
            }
        }
        // The client's view account is the last word on row order: `Manual` keeps
        // the server's account order, `Last updated` promotes the single row whose
        // human-interaction stamp advanced. Runs before any banding or folding.
        self.apply_view_order(&mut projects, &mut loose, cx);
        // External sessions not bound to a *registered* project stay in the
        // loose Conversations list — that covers unbound sessions, sessions
        // whose folder was removed, and sessions bound to a cwd never
        // registered as a project. Bound ones are pulled into their folder
        // group inside `render_project_group` (filtered by project path
        // there), so a removed folder never strands its live sessions.
        let loose_externals: Vec<crate::external_session::ExternalSessionSummary> = self
            .external_sessions
            .iter()
            .filter(|s| external_session_is_loose(s.project.as_deref(), &known_projects))
            .cloned()
            .collect();

        let mut flat_ids: Vec<String> = Vec::new();
        for (path, group) in &projects {
            if !self.collapsed.contains(path.as_str()) {
                flat_ids.extend(group.iter().map(|s| s.id.clone()));
                // Include external sessions bound to this project folder so
                // they participate in the selection-slide alongside the
                // folder's threads (merged + recency-sorted at render time).
                flat_ids.extend(
                    self.external_sessions
                        .iter()
                        .filter(|s| s.project.as_deref() == Some(std::path::Path::new(path)))
                        .map(|s| s.id.clone()),
                );
            }
        }
        flat_ids.extend(loose.iter().map(|s| s.id.clone()));
        flat_ids.extend(loose_externals.iter().map(|s| s.id.clone()));
        let dir = match (&self.prev_selected, &self.selected) {
            (Some(prev), Some(cur)) => match (
                flat_ids.iter().position(|id| id == prev),
                flat_ids.iter().position(|id| id == cur),
            ) {
                (Some(p), Some(c)) => match c.cmp(&p) {
                    std::cmp::Ordering::Greater => SlideDir::Down,
                    std::cmp::Ordering::Less => SlideDir::Up,
                    std::cmp::Ordering::Equal => SlideDir::None,
                },
                _ => SlideDir::None,
            },
            _ => SlideDir::None,
        };
        let slide = SlideCtx {
            selecting_id: self.selected.clone(),
            deselecting_id: self.prev_selected.clone(),
            dir,
            gen_id: self.select_gen,
        };

        let top_inset = crate::workspace::sidebar_top_inset();

        // Sticky overlay pinned above the scroll body: while the current
        // section's header would scroll out of view, an overlay copy takes its
        // place at the header's resting position and the rows scroll
        // underneath. The in-flow headers stay in the content (parent-child
        // grouping — each header sits directly above its own section), and
        // the overlay only appears after the first scroll, by which point the
        // scroll handle's offset/bounds have been measured.
        let projects_present = !projects.is_empty();
        let scroll_top = -self.scroll_handle.offset().y;
        let projects_height = self.scroll_handle.bounds_for_item(0).map(|b| b.size.height);
        let pinned = pinned_section(projects_present, scroll_top, projects_height);
        let overlay_visible = scroll_top > px(0.);
        // When the overlay carries the Conversations header it also anchors
        // the new-session dropdown; the in-flow copy must then drop its own
        // copy to avoid two menus.
        let overlay_shows_conversations = overlay_visible && pinned == PinnedSection::Conversations;
        let overlay: Option<AnyElement> = overlay_visible.then(|| {
            let header = match pinned {
                PinnedSection::Projects => {
                    section_header(i18n::t("sidebar-section-projects"), &theme, None)
                }
                PinnedSection::Conversations => {
                    self.conversations_section_header(&theme, "sticky", true, cx)
                }
            };
            gpui::div()
                .id("sidebar-sticky-header")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .px_2()
                .pt(top_inset)
                .bg(theme.background)
                .border_b_1()
                .border_color(theme.border)
                .child(header)
                .into_any_element()
        });
        self.folder_order = projects.iter().map(|(path, _)| path.clone()).collect();
        let projects_el: Vec<AnyElement> = projects
            .into_iter()
            .map(|(path, group)| {
                self.render_project_group(&path, &group, selected.as_deref(), &slide, cx)
            })
            .collect();

        // Seamless slot: no own background or border — the shell's gutter
        // background shows through and the main card's left border is the
        // only visible boundary.
        v_flex()
            .h_full()
            .w(self.width)
            .flex_shrink_0()
            .relative()
            .child(
                v_flex()
                    .id("sidebar-body")
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pt(top_inset)
                    .pb_2()
                    .track_scroll(&self.scroll_handle)
                    // child 0: the Projects section — its header sits directly
                    // above the folder groups (parent-child grouping),
                    // rendered even when no registered projects exist (a
                    // zero-height slot) so `bounds_for_item(0)` keeps
                    // measuring the projects↔conversations boundary for the
                    // sticky overlay.
                    .child(
                        v_flex()
                            .w_full()
                            .children(projects_present.then(|| {
                                section_header(i18n::t("sidebar-section-projects"), &theme, None)
                            }))
                            .children(projects_el),
                    )
                    // child 1: the Conversations section — header + loose
                    // (non-project) threads and external sessions, merged by
                    // recency so an external CLI session sits among manox
                    // threads instead of a separate section. External rows
                    // sort by spawn time (manox cannot observe in-TUI
                    // interaction); threads by last interaction.
                    .child(
                        v_flex()
                            .w_full()
                            .child(self.conversations_section_header(
                                &theme,
                                "inflow",
                                !overlay_shows_conversations,
                                cx,
                            ))
                            .child(self.render_partition_rows(
                                crate::sidebar_view::LOOSE,
                                self.order_rows(&loose, &loose_externals),
                                &PartitionEnv {
                                    selected: selected.as_deref(),
                                    unread_map: &unread_map,
                                    slide: &slide,
                                    theme: &theme,
                                    indent_base: px(0.),
                                },
                                cx,
                            )),
                    ),
            )
            .children(overlay)
    }
}

/// The fold quota: an open folder shows this many rows, then offers the
/// remainder through an explicit reveal.
const COLLAPSED_ROWS: usize = 5;

/// The wire list partitioned exactly as the render partitions it — a row
/// bound to a *registered* project joins that folder's partition, everything
/// else joins the loose Conversations account — preserving the server's
/// committed order within each partition. This is the server-side surface
/// `reconcile_manual_account` replays against: the wire list arrives in the
/// durable account's order, so it is what a `Manual` landing diffs the view
/// account into. Partitions with no wire rows never appear.
fn wire_partition_orders(
    items: &[ThreadListItem],
    known_projects: &[String],
) -> Vec<(String, Vec<String>)> {
    let mut orders: Vec<(String, Vec<String>)> = Vec::new();
    for s in items {
        let project = s.project.as_deref().unwrap_or_default();
        let key = if !project.is_empty() && known_projects.iter().any(|k| k == project) {
            project.to_string()
        } else {
            crate::sidebar_view::LOOSE.to_string()
        };
        match orders.iter_mut().find(|(p, _)| p == &key) {
            Some((_, ids)) => ids.push(s.id.clone()),
            None => orders.push((key, vec![s.id.clone()])),
        }
    }
    orders
}

/// Fold a projected row list to the quota and report how many rows it hides.
///
/// A leader and its member subtree are one unit: the cut lands between units,
/// so a member can never be shown without the leader that owns its indent guide
/// and never stranded above a hidden leader. Revealing is a per-mount
/// inspection, never persisted — a folder reopened much later returns to the
/// bounded projection.
fn folded_rows(rows: &[ThreadRender], revealed: bool) -> (Vec<ThreadRender>, usize) {
    if revealed || rows.len() <= COLLAPSED_ROWS {
        return (rows.to_vec(), 0);
    }
    // Walk whole units (a top-level row plus its contiguous subtree) until the
    // next unit would cross the quota.
    let mut keep = 0;
    while keep < rows.len() {
        let mut end = keep + 1;
        while end < rows.len() && rows[end].indent > 0.0 {
            end += 1;
        }
        if end > COLLAPSED_ROWS {
            break;
        }
        keep = end;
    }
    // A first unit longer than the quota still shows: hiding everything while
    // rows remain hidden would leave the reveal affordance pointing at nothing.
    let keep = keep.max(1).min(rows.len());
    (rows[..keep].to_vec(), rows.len() - keep)
}

fn section_header(label: SharedString, theme: &Theme, action: Option<AnyElement>) -> AnyElement {
    let mut row = h_flex()
        .px_2()
        .pt_3()
        .pb_1()
        .items_center()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .child(gpui::div().flex_1().child(label));

    if let Some(el) = action {
        row = row.child(el);
    }

    row.into_any_element()
}

/// Cascade for the external-agent CLI submenus (Claude Code / Codex / GitHub
/// Copilot) in the new-session menu: builds the shared provider→model cascade
/// and emits `SpawnExternalSession(kind, provider, model, wire, project)` on
/// pick — the workspace spawns the agent CLI in the project's directory and
/// mounts its TUI in the main area. The project path (if any) is read from
/// the sidebar's `new_session_project` field so the handler can set the CWD /
/// open directory.
fn build_agent_model_cascade(
    menu: PopupMenu,
    kind: crate::external_session::SessionKind,
    agent_id: &'static str,
    sidebar: &WeakEntity<Sidebar>,
    mux: &Option<gpui::Entity<crate::multiplexer::SessionMultiplexer>>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let sidebar = sidebar.clone();
    // U2 cross-domain #4: the cascade projects the multiplexer's wire
    // models (the provider_glue direct read retired). The mux handle is
    // passed IN: this builder runs eagerly inside the Sidebar's own
    // update listener, so upgrading + reading the Sidebar entity here
    // would double-lease it — GPUI panics ("cannot read Sidebar while
    // it is already being updated"), and in the real app the objc
    // callback frame turns the panic into a non-unwinding abort.
    let models: Vec<manox_protocol::ModelInfo> = mux
        .as_ref()
        .map(|m| m.read(cx).models().to_vec())
        .unwrap_or_default();
    crate::views::model_cascade::build_model_cascade(
        menu,
        agent_id,
        &models,
        window,
        cx,
        move |provider, model, wire, _window, cx| {
            let _ = sidebar.update(cx, |this, cx| {
                let project = this.new_session_project.clone();
                cx.emit(SidebarEvent::SpawnExternalSession(
                    kind, provider, model, wire, project,
                ));
                cx.notify();
            });
        },
    )
}

/// A project folder's action menu (its ellipsis button): a new-session
/// submenu (Manox + the external-agent model cascades), a plain terminal,
/// VS Code, and a destructive Remove-project row. The session rows share
/// the flat new-session menu's semantics, scoped to the sidebar's
/// `new_session_project`.
fn build_project_menu(
    menu: PopupMenu,
    sidebar: &WeakEntity<Sidebar>,
    mux: &Option<gpui::Entity<crate::multiplexer::SessionMultiplexer>>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let theme = theme.clone();
    let sidebar = sidebar.clone();
    // Owned clone: the submenu closures are `move` and must be 'static
    // (deferred builds), so they cannot capture the borrowed parameter.
    let mux = mux.clone();
    let mut menu = menu.max_w(gpui::px(280.));
    // New-session submenu: Manox flat row + one provider→model cascade per
    // external agent kind.
    let sidebar_new = sidebar.clone();
    let theme_new = theme.clone();
    menu = menu.submenu_with_icon(
        Some(
            Icon::new(IconName::Plus)
                .small()
                .text_color(theme.muted_foreground),
        ),
        i18n::t("sidebar-new-session-label"),
        window,
        cx,
        move |submenu, window, cx| {
            let mut submenu = submenu;
            let sidebar_manox = sidebar_new.clone();
            submenu = submenu.item(
                PopupMenuItem::new(i18n::t("sidebar-new-session-manox"))
                    .icon(
                        Icon::default()
                            .path("icons/manox.svg")
                            .small()
                            .text_color(theme_new.muted_foreground),
                    )
                    .on_click(move |_, _window, cx| {
                        let _ = sidebar_manox.update(cx, |this, cx| {
                            let project = this.new_session_project.take();
                            this.close_new_session_menu();
                            if let Some(p) = project {
                                cx.emit(SidebarEvent::NewThreadWithProject(p));
                            } else {
                                cx.emit(SidebarEvent::NewThread);
                            }
                            cx.notify();
                        });
                    }),
            );
            for kind in [
                crate::external_session::SessionKind::ClaudeCode,
                crate::external_session::SessionKind::Codex,
                crate::external_session::SessionKind::GithubCopilot,
            ] {
                let sidebar_agent = sidebar_new.clone();
                let mux_agent = mux.clone();
                let label = kind.label();
                let agent_id = kind.agent_id();
                submenu = submenu.submenu_with_icon(
                    Some(
                        Icon::default()
                            .path(kind.icon_asset())
                            .small()
                            .text_color(theme_new.muted_foreground),
                    ),
                    label,
                    window,
                    cx,
                    move |submenu, window, cx| {
                        build_agent_model_cascade(
                            submenu,
                            kind,
                            agent_id,
                            &sidebar_agent,
                            &mux_agent,
                            window,
                            cx,
                        )
                    },
                );
            }
            submenu
        },
    );
    // Terminal: a plain shell session — no provider/model cascade, the
    // click spawns immediately in the project directory.
    {
        let kind = crate::external_session::SessionKind::Terminal;
        let sidebar_plain = sidebar.clone();
        menu = menu.item(
            PopupMenuItem::new(i18n::t("sidebar-new-terminal"))
                .icon(
                    Icon::default()
                        .path(kind.icon_asset())
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .on_click(move |_, _window, cx| {
                    let _ = sidebar_plain.update(cx, |this, cx| {
                        let project = this.new_session_project.take();
                        this.close_new_session_menu();
                        cx.emit(SidebarEvent::SpawnPlainSession(kind, project));
                        cx.notify();
                    });
                }),
        );
    }
    // VS Code: single entry — injection resolves from the persisted
    // `vscode_app:` settings; no provider/model cascade. Disabled outright
    // when VS Code is not installed (parity with the flat menu).
    {
        let sidebar_vscode = sidebar.clone();
        let icon = Icon::default()
            .path("icons/vscode.svg")
            .small()
            .text_color(theme.muted_foreground);
        menu = menu.item(
            PopupMenuItem::new("VS Code")
                .icon(icon)
                .disabled(!manox_ext_agents::vscode_app_installed())
                .on_click(move |_, _window, cx| {
                    let _ = sidebar_vscode.update(cx, |this, cx| {
                        let project = this.new_session_project.clone();
                        cx.emit(SidebarEvent::LaunchVSCode(project));
                        cx.notify();
                    });
                }),
        );
    }
    menu = menu.separator();
    // Remove project: unregister the folder; the threads fall back to the
    // loose Conversations list and the history stays on disk.
    let sidebar_rm = sidebar.clone();
    menu = menu.item(
        PopupMenuItem::new(i18n::t("sidebar-remove-project"))
            .icon(
                Icon::default()
                    .path("icons/trash-2.svg")
                    .small()
                    .text_color(theme.danger),
            )
            .on_click(move |_, _window, cx| {
                let _ = sidebar_rm.update(cx, |this, cx| {
                    let project = this.new_session_project.take();
                    this.close_new_session_menu();
                    if let Some(p) = project {
                        cx.emit(SidebarEvent::RemoveProject(p));
                    }
                    cx.notify();
                });
            }),
    );
    menu
}

/// Leading icon for a unified sidebar row. Manox threads carry the brand
/// mark; external agent sessions use their own brand SVG (resolved by
/// `ExtrasAssetSource`, tinted via `text_color`).
#[derive(Clone)]
enum RowIcon {
    Thread,
    External(&'static str),
}

/// What the row's trailing action emits — a thread row's overflow menu
/// toggles its archived flag, an external session's hover button tears it
/// down (kill + drop).
#[derive(Clone)]
enum RowKind {
    Thread { archived: bool },
    External,
}

/// Maximum length of a user thread tag (enforced on every input edit).
const MAX_THREAD_TAG_CHARS: usize = 10;

/// Which flavor of inline tag edit a row is running.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagEditMode {
    /// The row has no tag yet; committing stores the typed value.
    Adding,
    /// The row's existing tag is being replaced; the input is prefilled.
    Renaming,
}

/// The one inline tag edit in flight (at most one row edits at a time).
/// The add/rename mode is consumed at mount time (prefill decision) and
/// never needed again.
struct TagEdit {
    id: String,
    input: Entity<InputState>,
    /// Input-event subscription (length clamp + commit); dropped with the
    /// edit so a stale input can never fire into the sidebar.
    _sub: Subscription,
}

/// A UI-layer sidebar row projected from either the gateway's wire
/// `ThreadListItem` (its decoration columns ride the row) or an
/// `ExternalSessionSummary`, so the two render through one layout with a shared
/// selection-slide animation, id tag, and hover archive action. Only display +
/// identity fields live here — the sidebar never holds PTY handles.
#[derive(Clone)]
struct SidebarThreadItem {
    id: String,
    /// 8-char tag label. Threads use the thread UUID prefix; external sessions
    /// use the cx session id prefix (traceable to `~/.manox/sessions/<id>.sock`).
    short_id: String,
    /// Clipboard payload for the id-tag click. Threads copy the thread id;
    /// external sessions copy the full cx session id (or socket path).
    copy_value: String,
    title: String,
    updated: String,
    pinned: bool,
    /// User-assigned tag rendered as a chip beside the id tag; `None` when
    /// the thread carries no tag. Threads only — external rows never tag.
    tag: Option<String>,
    has_unread: bool,
    errored: bool,
    selected: bool,
    indent: gpui::Pixels,
    /// A team leader whose member rows nest under this row; renders a
    /// collapse chevron before the status icon.
    team_leader: bool,
    /// The leader's member group is folded; the chevron shows the fold.
    team_collapsed: bool,
    /// Nested row (team member or fork child): draws the left guide rail
    /// tying the row to its parent.
    nested: bool,
    icon: RowIcon,
    /// A plan-review verdict is due; the icon stays blue static, not spinning.
    pending_plan: bool,
    /// Live monitors / background bash: the loop can still self-advance even
    /// with no turn in flight, so the icon keeps spinning.
    background_work: bool,
    /// A tool authorization is parked waiting for the user's verdict (the
    /// thread's card is only visible when it is the active thread).
    pending_auth: bool,
    running: bool,
    /// A sidecar-restored row with no live process; clicking it resumes the
    /// CLI. Rendered dimmed with a resume hover action.
    resumable: bool,
    /// A resume is in flight for this row; rendered with a loading indicator.
    resuming: bool,
    /// Selection-wash color: threads tint by approval mode, external rows use
    /// the theme accent.
    wash: gpui::Hsla,
    kind: RowKind,
}

impl SidebarThreadItem {
    /// Project a gateway wire row (U2): the live flags AND the decoration
    /// columns (tag chip, approval wash — U2 cross-domain #1) ride the row
    /// itself (the server's list projection with the §D.5 `SessionStatus`
    /// deltas merged by the multiplexer).
    fn from_wire(
        item: &ThreadListItem,
        selected: bool,
        // GW5: the leaf's client-owned unread mirror, when the session has
        // a leaf; `None` falls back to the row's flag (the deprecated wire
        // field the multiplexer's monotonic mirror keeps client-owned).
        unread_override: Option<bool>,
        nesting: RowNesting,
        theme: &Theme,
    ) -> Self {
        let title = if item.title.is_empty() {
            i18n::t("sidebar-empty-summary").to_string()
        } else {
            truncate(&item.title, 24)
        };
        Self {
            short_id: item.id.chars().take(8).collect(),
            copy_value: item.id.clone(),
            id: item.id.clone(),
            title,
            updated: format_relative(item.updated_at as i64),
            pinned: item.pinned,
            tag: item.tag.clone(),
            has_unread: unread_override.unwrap_or(item.unread),
            errored: item.errored,
            running: item.running,
            pending_auth: item.pending_auth,
            pending_plan: item.pending_plan,
            background_work: item.background_work,
            resumable: false,
            resuming: false,
            selected,
            indent: nesting.indent,
            team_leader: nesting.team_leader,
            team_collapsed: nesting.team_collapsed,
            nested: nesting.nested,
            icon: RowIcon::Thread,
            wash: approval_mode_color(
                item.approval_mode
                    .unwrap_or_else(|| PermissionMode::default().as_i64()),
                theme,
            ),
            kind: RowKind::Thread {
                archived: item.archived,
            },
        }
    }

    fn from_external(
        summary: &crate::external_session::ExternalSessionSummary,
        selected: bool,
        indent: gpui::Pixels,
        theme: &Theme,
    ) -> Self {
        let display = summary.display_title();
        let title = if display.is_empty() {
            i18n::t("sidebar-empty-summary").to_string()
        } else {
            truncate(&display, 24)
        };
        // Tag the row with the cx session id prefix (tracks the .sock file);
        // fall back to the manox-internal id prefix when the cx id was not
        // recoverable (IPC bind failed).
        let short_id: String = if !summary.cx_session_id.is_empty() {
            summary.cx_session_id.chars().take(8).collect()
        } else {
            // No cx id — plain PTY sessions never have one, and an agent
            // session lands here when its IPC bind failed. Show the trailing
            // uuid segment of the manox-internal id; the `external:` prefix
            // would render the same "external" tag on every such row.
            summary
                .id
                .rsplit(':')
                .next()
                .unwrap_or(summary.id.as_str())
                .chars()
                .take(8)
                .collect()
        };
        Self {
            id: summary.id.clone(),
            short_id,
            copy_value: summary.copy_identity(),
            title,
            updated: format_relative(summary.created_at),
            pinned: false,
            has_unread: false,
            tag: None,
            errored: false,
            running: false,
            pending_auth: false,
            pending_plan: false,
            background_work: false,
            resumable: summary.resumable,
            resuming: summary.resuming,
            selected,
            indent,
            // External CLI sessions never lead a team; no collapse affordance.
            team_leader: false,
            team_collapsed: false,
            nested: false,
            icon: RowIcon::External(summary.kind.icon_asset()),
            // Same wash as Workspace Access threads: `theme.accent` resolves to
            // `neutral-100` (near-white) in the forced Light theme, which made
            // the external row's hover/active/selected wash invisible on the
            // white sidebar. `theme.info` (cyan) is the visible default tint.
            wash: theme.info,
            kind: RowKind::External,
        }
    }
}

/// Render one unified sidebar row — threads and external agent sessions
/// share the layout (leading icon → title → id tag + tag chip + updated
/// time + trailing action), the selection-wash slide animation, and the
/// hover-reveal affordance. Only the emitted event, the trailing action
/// (overflow menu vs close button), and the leading icon differ by `kind`.
/// `sidebar` carries the per-sidebar overlay state (tag edit, open row
/// menu) — `Context<Sidebar>` derefs to `App`, not the entity.
fn render_thread_item(
    item: &SidebarThreadItem,
    slide: &SlideCtx,
    sidebar: &Sidebar,
    theme: &Theme,
    cx: &mut Context<Sidebar>,
) -> AnyElement {
    let id = item.id.clone();
    let id_open = id.clone();
    let id_archive = id.clone();
    let id_team = id.clone();
    let id_copy = item.copy_value.clone();
    let short_id = item.short_id.clone();
    let title = item.title.clone();
    let updated = item.updated.clone();
    let title_color = if item.resumable {
        // A sidecar-restored row reads as parked: dimmed until resumed.
        theme.muted_foreground
    } else if item.errored {
        theme.danger
    } else {
        theme.foreground
    };
    let wash = item.wash;
    let icon = item.icon.clone();
    let open_kind = item.kind.clone();
    let group = gpui::SharedString::from(format!("thread-row-{id}"));
    // Only thread rows carry order: an external session has no server-side
    // account, so it is never a drag source, a drop target or a line host.
    let is_thread_row = matches!(item.kind, RowKind::Thread { .. });
    let drag_id = id.clone();
    let drag_line = sidebar
        .drag_row
        .as_ref()
        .filter(|d| d.line_on == id.as_str() && d.dragged != d.line_on)
        .map(|d| d.edge);
    let drag_source = sidebar.drag_row.as_ref().is_some_and(|d| d.dragged == id);
    // The loop can still self-advance (turn in flight, monitors / background
    // bash alive): the row spins and the id tag stays highlighted.
    let autonomous = item.running || item.background_work;
    let tag_variant = if item.selected || autonomous {
        TagVariant::Primary
    } else {
        TagVariant::Secondary
    };
    let role = if slide.selecting_id.as_deref() == Some(id.as_str()) {
        AnimRole::Selecting
    } else if slide.deselecting_id.as_deref() == Some(id.as_str()) {
        AnimRole::Deselecting
    } else {
        AnimRole::None
    };
    let dir_sign: f32 = match slide.dir {
        SlideDir::Down => 1.0,
        SlideDir::Up => -1.0,
        SlideDir::None => 0.0,
    };
    let slide_gen = slide.gen_id;
    let wash_overlay: Option<AnyElement> = if role != AnimRole::None {
        let anim_role = role;
        let anim_id = format!("thread-sel-wash-{id}-{slide_gen}");
        Some(
            gpui::div()
                .absolute()
                .left_0()
                .right_0()
                .rounded(theme.radius)
                .with_animation(
                    anim_id,
                    Animation::new(Duration::from_millis(160)).with_easing(ease_in_out),
                    move |el, t| {
                        let (opacity, ty) = if anim_role == AnimRole::Selecting {
                            (t, -dir_sign * SELECT_SLIDE_PX * (1.0 - t))
                        } else {
                            (1.0 - t, dir_sign * SELECT_SLIDE_PX * t)
                        };
                        el.bg(wash.opacity(0.18 * opacity))
                            .top(px(ty))
                            .bottom(px(-ty))
                    },
                )
                .into_any_element(),
        )
    } else {
        None
    };

    let tag_button = Button::new(format!("thread-id-tag-{id}"))
        .ghost()
        .xsmall()
        .compact()
        .tooltip(i18n::t("sidebar-copy-thread-id"))
        .cursor_pointer()
        .child(
            Tag::new()
                .with_variant(tag_variant)
                .outline()
                .small()
                .child(short_id),
        )
        .on_click(move |_ev, _window, cx: &mut App| {
            cx.stop_propagation();
            cx.write_to_clipboard(ClipboardItem::new_string(id_copy.clone()));
        });

    let tag_wrapper = gpui::div().relative().overflow_hidden().child(tag_button);
    let tag_element: AnyElement = if autonomous {
        let accent = theme.accent;
        tag_wrapper
            .with_animation(
                format!("thread-running-shimmer-{id}"),
                Animation::new(Duration::from_millis(1400))
                    .repeat()
                    .with_easing(ease_in_out),
                move |el, delta| {
                    el.child(
                        gpui::div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .w(px(12.))
                            .bg(accent.opacity(0.55))
                            .left(px(-20. + 120. * delta)),
                    )
                },
            )
            .into_any_element()
    } else {
        tag_wrapper.into_any_element()
    };

    let leading_icon = if item.errored {
        // Abnormal stop: the danger triangle replaces the ship-wheel.
        Icon::new(IconName::TriangleAlert)
            .xsmall()
            .text_color(theme.danger)
            .into_any_element()
    } else if item.resuming {
        // A resume is in flight: the loading indicator replaces the idle icon.
        crate::views::braille_spinner::BrailleSpinner::new()
            .xsmall()
            .color(theme.muted_foreground)
            .into_any_element()
    } else {
        match icon {
            RowIcon::Thread => {
                // ship-wheel state machine: green and spinning while the loop
                // can self-advance, blue static while it waits on the user
                // (approval / plan verdict / unread finished turn), gray for a
                // read pause or a never-run thread.
                let waiting_user = item.pending_auth || item.pending_plan;
                let (color, spin) = if waiting_user {
                    (theme.info, false)
                } else if autonomous {
                    (theme.success, true)
                } else if item.has_unread {
                    (theme.info, false)
                } else {
                    (theme.foreground, false)
                };
                let icon = gpui::svg()
                    .path("icons/ship-wheel.svg")
                    .size(px(16.))
                    .text_color(color);
                if spin {
                    icon.with_animation(
                        format!("thread-running-spin-{id}"),
                        Animation::new(Duration::from_millis(1600))
                            .repeat()
                            .with_easing(linear),
                        |el, delta| {
                            el.with_transformation(Transformation::rotate(percentage(delta)))
                        },
                    )
                    .into_any_element()
                } else {
                    icon.into_any_element()
                }
            }
            RowIcon::External(path) => gpui::svg()
                .path(path)
                .size(px(16.))
                .text_color(theme.muted_foreground)
                .into_any_element(),
        }
    };

    h_flex()
        .id(format!("thread-item-{id}"))
        .group(group.clone())
        .w_full()
        .relative()
        .overflow_hidden()
        // Left guide rail for nested rows ties a member/fork row to its
        // parent, making the leader→member hierarchy visually explicit.
        .when(item.nested, |this| {
            this.child(
                gpui::div()
                    .absolute()
                    .left(px(12.))
                    .top(px(0.))
                    .bottom(px(0.))
                    .w(px(1.))
                    .bg(theme.border),
            )
        })
        .pl(px(8.) + item.indent)
        .pr_2()
        .py_1()
        .gap_2()
        .items_start()
        .rounded(theme.radius)
        .when(!item.selected, |this| {
            this.hover(move |s| s.bg(wash.opacity(0.08)))
                .active(move |s| s.bg(wash.opacity(0.18)))
        })
        // Open click branches on `kind`: a thread opens the conversation, an
        // external session switches the main area to its running TUI.
        .on_click(cx.listener(move |_this, _ev, _window, cx| match open_kind {
            RowKind::Thread { .. } => cx.emit(SidebarEvent::OpenThread(id_open.clone())),
            RowKind::External => cx.emit(SidebarEvent::OpenExternalSession(id_open.clone())),
        }))
        .when_some(wash_overlay, |this, overlay| this.child(overlay))
        .children(drag_line.map(|edge| {
            gpui::div()
                .absolute()
                .left_0()
                .right_0()
                .h(px(2.))
                .rounded_full()
                .bg(theme.accent)
                .when(edge == DragEdge::Top, |this| this.top(px(-1.)))
                .when(edge == DragEdge::Bottom, |this| this.bottom(px(-1.)))
                .into_any_element()
        }))
        .when(drag_source, |this| this.opacity(0.4))
        .when(is_thread_row, |this| {
            let payload = DraggedThreadRow {
                id: drag_id.clone(),
            };
            let ghost = payload.clone();
            this.on_drag(payload, move |_, _, _, cx| cx.new(|_| ghost.clone()))
                .on_drag_move::<DraggedThreadRow>(cx.listener(
                    move |this, e: &DragMoveEvent<DraggedThreadRow>, _window, cx| {
                        let top = e.bounds.origin.y;
                        let Some(edge) =
                            Sidebar::drag_boundary(top, e.bounds.size.height, e.event.position.y)
                        else {
                            return;
                        };
                        let marker = RowDrag {
                            dragged: e.drag(cx).id.clone(),
                            line_on: drag_id.clone(),
                            edge,
                        };
                        if this.drag_row.as_ref() != Some(&marker) {
                            this.drag_row = Some(marker);
                            cx.notify();
                        }
                    },
                ))
                .on_drop::<DraggedThreadRow>(cx.listener(
                    move |this, row: &DraggedThreadRow, _window, cx| {
                        // The drop is authoritative only for the row being dragged.
                        if this.drag_row.as_ref().is_some_and(|d| d.dragged == row.id) {
                            this.commit_row_drag(cx);
                        }
                    },
                ))
        })
        .when(item.team_leader, |this| {
            // Team collapse toggle: folds/unfolds the member rows nested
            // under this leader without opening the conversation.
            let id_team = id_team.clone();
            this.child(
                gpui::div()
                    .id(format!("thread-team-chevron-{id}"))
                    .cursor_pointer()
                    .child(
                        Icon::new(if item.team_collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall()
                        .text_color(theme.muted_foreground),
                    )
                    .on_click(cx.listener(move |this, _ev, _window, cx| {
                        cx.stop_propagation();
                        if !this.team_collapsed.remove(&id_team) {
                            this.team_collapsed.insert(id_team.clone());
                        }
                        this.persist_collapse(cx);
                        cx.notify();
                    })),
            )
        })
        .child(leading_icon)
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .items_center()
                        .min_w_0()
                        .when(item.pinned, |this| {
                            this.child(
                                Icon::new(IconName::Star)
                                    .xsmall()
                                    .text_color(theme.accent_foreground),
                            )
                        })
                        .when(item.pending_auth, |this| {
                            this.child(
                                gpui::div()
                                    .id(format!("thread-pending-auth-{id}"))
                                    .child(
                                        Icon::new(IconName::LoaderCircle)
                                            .xsmall()
                                            .text_color(theme.accent),
                                    )
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(i18n::t("sidebar-pending-auth"))
                                            .build(window, cx)
                                    }),
                            )
                        })
                        .child(
                            gpui::div()
                                .flex_1()
                                .min_w_0()
                                // Single-line invariant: the row height must
                                // stay bounded for any title (LLM-generated
                                // thread titles included) at any width.
                                .truncate()
                                .text_sm()
                                .text_color(title_color)
                                .child(title),
                        ),
                )
                .child({
                    // Tag slot: the inline input while an edit is in flight,
                    // else the persisted chip; external rows render neither.
                    let editing_input = sidebar
                        .tag_edit
                        .as_ref()
                        .filter(|e| e.id == id)
                        .map(|e| e.input.clone());
                    let tag_slot: Option<AnyElement> = if let Some(input) = editing_input {
                        Some(render_tag_edit(&id, &input, cx))
                    } else {
                        item.tag
                            .as_ref()
                            .map(|tag| render_tag_chip(&id, tag, &group, cx))
                    };
                    let menu_open = sidebar.row_menu_open.as_deref() == Some(id.as_str());
                    h_flex()
                        .w_full()
                        .gap_1()
                        .items_center()
                        .child(tag_element)
                        .children(tag_slot)
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .group_hover(group.clone(), |s| s.invisible())
                                .child(gpui::div().child(updated)),
                        )
                        .child(
                            h_flex()
                                .gap_0p5()
                                // The trigger stays visible while its menu is
                                // open so the anchor doesn't vanish
                                // mid-interaction.
                                .when(!menu_open, |this| {
                                    this.invisible().group_hover(group.clone(), |s| s.visible())
                                })
                                .child(match &item.kind {
                                    // Thread rows overflow into the three-dot
                                    // menu (Archive + Tag); external sessions
                                    // keep the single hover close button.
                                    RowKind::Thread { .. } => {
                                        render_thread_menu_trigger(item, sidebar, cx)
                                    }
                                    RowKind::External => render_hover_action(
                                        id_archive.clone(),
                                        item.kind.clone(),
                                        item.resumable,
                                        cx,
                                    ),
                                }),
                        )
                }),
        )
        .into_any_element()
}

/// The inline tag input of the row currently editing its tag. The wrapper
/// swallows clicks (the row's open-thread click must not fire while the
/// user works the input) and catches the input's propagated `Escape` as a
/// cancel; Enter/blur commits flow through the input's own events.
fn render_tag_edit(id: &str, input: &Entity<InputState>, cx: &mut Context<Sidebar>) -> AnyElement {
    let id = id.to_string();
    gpui::div()
        .id(SharedString::from(format!("thread-tag-edit-{id}")))
        .w(px(90.))
        .on_click(|_ev, _window, cx| cx.stop_propagation())
        .on_action(cx.listener(
            move |this, _: &gpui_component::input::Escape, _window, cx| {
                cx.stop_propagation();
                this.cancel_tag_edit(cx);
            },
        ))
        .child(Input::new(input).appearance(false).xsmall())
        .into_any_element()
}

/// The persisted user tag: a chip beside the id tag with an ✕ to clear it;
/// double-clicking it renames inline. Clicks stop propagation so chip
/// interaction never trips the row's open-thread click.
fn render_tag_chip(
    id: &str,
    tag: &str,
    row_group: &SharedString,
    cx: &mut Context<Sidebar>,
) -> AnyElement {
    let id = id.to_string();
    let id_rename = id.clone();
    let id_clear = id.clone();
    let tag_text = tag.to_string();
    let row_group = row_group.clone();
    // Same visual language as the thread-id tag; the clear affordance rides
    // the row hover like every other row action (space reserved, so the row
    // never reflows mid-hover).
    h_flex()
        .items_center()
        .id(SharedString::from(format!("thread-tag-chip-{id}")))
        .cursor_pointer()
        .tooltip(move |window, cx| Tooltip::new(i18n::t("sidebar-thread-tag")).build(window, cx))
        .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
            cx.stop_propagation();
            if ev.click_count() >= 2 {
                this.begin_tag_edit(id_rename.clone(), TagEditMode::Renaming, window, cx);
            }
        }))
        .child(
            Tag::new()
                .with_variant(TagVariant::Secondary)
                .outline()
                .small()
                .child(tag_text),
        )
        .child(
            Button::new(format!("thread-tag-clear-{id}"))
                .ghost()
                .xsmall()
                .icon(IconName::Close)
                .tooltip(i18n::t("sidebar-thread-tag-clear"))
                .invisible()
                .group_hover(row_group, |s| s.visible())
                .on_click(cx.listener(move |_this, _ev, _window, cx| {
                    cx.stop_propagation();
                    cx.emit(SidebarEvent::SetThreadTag(id_clear.clone(), None));
                })),
        )
        .into_any_element()
}

/// The thread row's overflow trigger: a three-dot button opening the
/// Archive + Tag popup. Clicking it again toggles the menu closed; the
/// dropdown anchors below the button (deferred, so it escapes the row's
/// `overflow_hidden` and paints above sibling rows).
fn render_thread_menu_trigger(
    item: &SidebarThreadItem,
    sidebar: &Sidebar,
    cx: &mut Context<Sidebar>,
) -> AnyElement {
    let id = item.id.clone();
    let id_toggle = id.clone();
    let archived = matches!(item.kind, RowKind::Thread { archived: true });
    let has_tag = item.tag.is_some();
    let button = Button::new(format!("thread-menu-{id}"))
        .ghost()
        .xsmall()
        .icon(IconName::Ellipsis)
        .tooltip(i18n::t("sidebar-thread-menu"))
        .on_click(cx.listener(move |this, _ev, window, cx| {
            cx.stop_propagation();
            if this.row_menu_open.as_deref() == Some(id_toggle.as_str()) {
                this.close_row_menu();
            } else {
                this.open_row_menu(id_toggle.clone(), archived, has_tag, window, cx);
            }
            cx.notify();
        }));
    let dropdown = sidebar.render_row_menu_dropdown(&id);
    gpui::div()
        .relative()
        .child(button)
        .children(dropdown)
        .into_any_element()
}

/// The hover action on an external session row's right edge: the Inbox
/// close button while live, a Play resume button when resumable (the row
/// click already emits the same `OpenExternalSession` event, so the
/// affordance is just a visible hint). Thread rows use the overflow menu
/// instead. Uses `cx.listener` so the click emits on the sidebar's own
/// context (where `EventEmitter<SidebarEvent>` lives) rather than the bare
/// `App` the standalone `on_click` receives.
fn render_hover_action(
    id: String,
    kind: RowKind,
    resumable: bool,
    cx: &mut Context<Sidebar>,
) -> AnyElement {
    if resumable {
        let id_open = id.clone();
        return Button::new(format!("resume-external-{id}"))
            .ghost()
            .xsmall()
            .icon(IconName::Play)
            .tooltip(i18n::t("sidebar-resume-external"))
            .on_click(cx.listener(move |_this, _ev, _window, cx| {
                cx.stop_propagation();
                cx.emit(SidebarEvent::OpenExternalSession(id_open.clone()));
            }))
            .into_any_element();
    }
    let id = id.clone();
    Button::new(format!("archive-thread-{id}"))
        .ghost()
        .xsmall()
        .icon(IconName::Inbox)
        .tooltip(match &kind {
            RowKind::Thread { .. } => i18n::t("sidebar-archive"),
            RowKind::External => i18n::t("sidebar-close-external"),
        })
        .on_click(cx.listener(move |_this, _ev, _window, cx| {
            cx.stop_propagation();
            match kind {
                RowKind::Thread { archived } => {
                    cx.emit(SidebarEvent::ArchiveThread(id.clone(), !archived));
                }
                RowKind::External => {
                    cx.emit(SidebarEvent::ArchiveExternalSession(id.clone()));
                }
            }
        }))
        .into_any_element()
}

fn approval_mode_color(mode: i64, theme: &Theme) -> gpui::Hsla {
    match PermissionMode::from_i64(mode) {
        PermissionMode::ReadOnly => theme.warning,
        PermissionMode::WorkspaceWrite => theme.info,
        PermissionMode::DangerFullAccess => theme.danger,
    }
}

fn format_relative(epoch: i64) -> String {
    let now = chrono::Local::now().timestamp();
    let diff = (now - epoch).max(0);
    if diff < 60 {
        i18n::t("sidebar-time-just-now").to_string()
    } else if diff < 3600 {
        i18n::t_count("sidebar-time-minutes", diff / 60).to_string()
    } else if diff < 86_400 {
        i18n::t_count("sidebar-time-hours", diff / 3600).to_string()
    } else if diff < 604_800 {
        i18n::t_count("sidebar-time-days", diff / 86_400).to_string()
    } else {
        i18n::t_count("sidebar-time-weeks", diff / 604_800).to_string()
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    let one_line = s.replace('\n', " ");
    if one_line.chars().count() > max_chars {
        let t: String = one_line.chars().take(max_chars).collect();
        format!("{t}…")
    } else {
        one_line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_component::theme::ThemeColor;

    /// A real (non-transparent) theme — `Theme::default()` derives all-zero
    /// colors, so it cannot validate wash visibility.
    fn real_theme() -> Theme {
        Theme::from(&*ThemeColor::light())
    }

    /// The U2 wire row sample: the server projects the display title into
    /// `title` and the recency column into `updated_at` (unix seconds).
    fn sample_item() -> ThreadListItem {
        ThreadListItem {
            id: "thread-abcdef12".into(),
            title: "Summarize the diff".into(),
            updated_at: 0,
            running: false,
            // GW5: the wire unread field is the deprecated constant-false;
            // badges come from the leaf mirrors (or the multiplexer's
            // client-owned row mirror).
            unread: false,
            errored: false,
            pending_auth: false,
            pending_plan: false,
            background_work: false,
            model_id: "claude".into(),
            pinned: false,
            archived: false,
            parent_id: None,
            depth: 0,
            project: None,
            tag: None,
            approval_mode: None,
        }
    }

    fn sample_external() -> crate::external_session::ExternalSessionSummary {
        crate::external_session::ExternalSessionSummary {
            id: "external:claude:deadbeef".into(),
            kind: crate::external_session::SessionKind::ClaudeCode,
            created_at: 0,
            project: None,
            title: None,
            cx_session_id: "deadbeef".into(),
            socket_path: None,
            resumable: false,
            resuming: false,
        }
    }

    /// A member whose leader is not in the current partition (e.g. an
    /// archived leader) must not disappear: the row degrades to top-level,
    /// matching the webview forest.
    #[test]
    fn team_forest_keeps_members_when_leader_is_in_another_partition() {
        let thread = |id: &str, parent: Option<&str>, depth: i32, at: i64| {
            let mut s = sample_item();
            s.id = id.into();
            s.parent_id = parent.map(str::to_string);
            s.depth = depth;
            s.updated_at = at as i32;
            s
        };
        // Only the active partition is handed to the forest; the leader sits
        // in the archived partition, so its depth-1 member must surface as a
        // top-level row instead of vanishing.
        let threads = vec![thread("member", Some("leader"), 1, 100)];
        let rows = team_forest(&HashSet::new(), &threads, &[]);
        let ids: Vec<&str> = rows.iter().map(|r| r.row.id()).collect();
        assert_eq!(ids, vec!["member"]);
        assert_eq!(rows[0].indent, 0.0);
        assert!(!rows[0].team_leader);
    }

    /// The insertion boundary is the row's own half the pointer sits in, and a
    /// pointer outside the row's bounds claims no boundary at all (every row
    /// sees every move event of a live drag).
    #[test]
    fn drag_boundary_reports_the_half_the_pointer_is_in() {
        use gpui::px;
        assert_eq!(
            Sidebar::drag_boundary(px(10.), px(20.), px(12.)),
            Some(DragEdge::Top)
        );
        assert_eq!(
            Sidebar::drag_boundary(px(10.), px(20.), px(25.)),
            Some(DragEdge::Bottom)
        );
        assert_eq!(
            Sidebar::drag_boundary(px(10.), px(20.), px(9.)),
            None,
            "above the row"
        );
        assert_eq!(
            Sidebar::drag_boundary(px(10.), px(20.), px(31.)),
            None,
            "below the row"
        );
    }

    /// A drop in front of a row anchors that row; behind it anchors the next;
    /// behind the last row anchors nothing (append).
    #[test]
    fn the_drag_anchor_resolves_against_the_displayed_order() {
        let shown: Vec<String> = ["t1", "t2", "t3"].iter().map(|s| s.to_string()).collect();
        let at = |line_on: &str, edge| {
            Sidebar::anchor_after(
                &RowDrag {
                    dragged: "t3".into(),
                    line_on: line_on.into(),
                    edge,
                },
                &shown,
            )
        };
        assert_eq!(at("t2", DragEdge::Top).as_deref(), Some("t2"));
        assert_eq!(at("t1", DragEdge::Bottom).as_deref(), Some("t2"));
        assert_eq!(at("t3", DragEdge::Bottom), None);
    }

    /// The row commit's anchor: a line under the last row is the append
    /// gesture (`None`), the same line on any other row anchors its follower.
    #[test]
    fn a_line_under_the_last_row_is_the_row_append_gesture() {
        let shown: Vec<String> = ["t1", "t2", "t3"].iter().map(|s| s.to_string()).collect();
        let anchor = |line_on: &str, edge| {
            Sidebar::resolve_row_anchor(
                &RowDrag {
                    dragged: "t1".into(),
                    line_on: line_on.into(),
                    edge,
                },
                &shown,
            )
        };
        assert_eq!(anchor("t3", DragEdge::Bottom), None);
        assert_eq!(anchor("t3", DragEdge::Top).as_deref(), Some("t3"));
        assert_eq!(anchor("t2", DragEdge::Bottom).as_deref(), Some("t3"));
    }

    /// A folder drop resolved against the rendered order. The case that
    /// motivated the guard: releasing on the boundary immediately above the
    /// dragged folder's own position (`anchor_after` returns the dragged
    /// folder) is a no-op — it must never decay into an append, which is what
    /// upstream `before = None` means. The mirrored in-place drop (the top
    /// edge of the folder that already follows the dragged one) is a no-op
    /// for the same reason `move_in_account` rejects it on the row path.
    #[test]
    fn a_folder_drop_back_onto_its_own_boundary_is_a_noop() {
        let order: Vec<String> = ["f1", "f2", "f3"].iter().map(|s| s.to_string()).collect();
        let drop = |dragged: &str, line_on: &str, edge| {
            Sidebar::resolve_folder_drop(
                &RowDrag {
                    dragged: dragged.into(),
                    line_on: line_on.into(),
                    edge,
                },
                &order,
            )
        };
        assert!(matches!(
            drop("f2", "f1", DragEdge::Bottom),
            FolderDrop::Noop
        ));
        // In-place mirror: f3 already follows f2, so "f2 in front of f3"
        // re-inserts the folder where it sits — the same already-in-place
        // rejection the row commit gets from `move_in_account`.
        assert!(matches!(drop("f2", "f3", DragEdge::Top), FolderDrop::Noop));
    }

    /// The true append still works: the line on the bottom edge of the last
    /// folder lands the dragged folder at the tail, and an ordinary drop in
    /// front of an unrelated folder keeps that folder as the anchor.
    #[test]
    fn a_folder_drop_below_the_last_folder_is_a_true_append() {
        let order: Vec<String> = ["f1", "f2", "f3"].iter().map(|s| s.to_string()).collect();
        let drop = |dragged: &str, line_on: &str, edge| {
            Sidebar::resolve_folder_drop(
                &RowDrag {
                    dragged: dragged.into(),
                    line_on: line_on.into(),
                    edge,
                },
                &order,
            )
        };
        match drop("f1", "f3", DragEdge::Bottom) {
            FolderDrop::Move(at) => assert_eq!(at, None),
            FolderDrop::Noop => panic!("the bottom edge of the last folder appends"),
        }
        match drop("f1", "f3", DragEdge::Top) {
            FolderDrop::Move(at) => {
                assert_eq!(at.as_deref(), Some(std::path::Path::new("f3")))
            }
            FolderDrop::Noop => panic!("a front-of-anchor drop keeps the anchor"),
        }
    }

    /// A line whose host left the rendered order mid-drag (folder removed, or
    /// the order otherwise rewritten under the gesture) claims no move — it
    /// must not decay into an append either.
    #[test]
    fn a_folder_line_whose_host_vanished_is_a_noop() {
        let order: Vec<String> = ["f1", "f3"].iter().map(|s| s.to_string()).collect();
        assert!(matches!(
            Sidebar::resolve_folder_drop(
                &RowDrag {
                    dragged: "f1".into(),
                    line_on: "f2".into(),
                    edge: DragEdge::Bottom,
                },
                &order,
            ),
            FolderDrop::Noop
        ));
    }

    /// The reconcile's server-side surface partitions the wire list by the
    /// same rule render does — registered project folder, else the loose
    /// account — and keeps the wire's committed order inside each partition.
    #[test]
    fn the_wire_list_partitions_in_committed_order() {
        let item = |id: &str, project: Option<&str>| ThreadListItem {
            id: id.into(),
            project: project.map(str::to_string),
            ..sample_item()
        };
        let items = vec![
            item("a", Some("/p1")),
            item("b", None),
            item("c", Some("/p1")),
            item("d", Some("/ghost")),
            item("e", Some("/p2")),
        ];
        let known = vec!["/p1".to_string(), "/p2".to_string()];
        let orders = wire_partition_orders(&items, &known);
        assert_eq!(
            orders,
            vec![
                ("/p1".to_string(), vec!["a".to_string(), "c".to_string()]),
                (
                    crate::sidebar_view::LOOSE.to_string(),
                    vec!["b".to_string(), "d".to_string()]
                ),
                ("/p2".to_string(), vec!["e".to_string()]),
            ]
        );
    }

    /// A projected row for the fold tests: identity, and the parent/depth pair
    /// that decides whether the row hangs under a leader.
    fn rendered(id: &str, parent: Option<&str>, depth: i32) -> ThreadRender {
        let mut item = sample_item();
        item.id = id.into();
        item.parent_id = parent.map(str::to_string);
        item.depth = depth;
        ThreadRender {
            row: SidebarRow::Thread(item),
            indent: if depth > 0 { 14.0 } else { 0.0 },
            team_leader: false,
            team_collapsed: false,
        }
    }

    /// An open partition shows the quota; the remainder waits behind an explicit
    /// reveal, and revealing reports nothing left to hide.
    #[test]
    fn folding_bounds_an_open_partition_at_the_quota() {
        let rows: Vec<ThreadRender> = (0..8)
            .map(|i| rendered(&format!("t{i}"), None, 0))
            .collect();
        let (shown, hidden) = folded_rows(&rows, false);
        assert_eq!(hidden, 3);
        let names: Vec<&str> = shown.iter().map(|r| r.row.id()).collect();
        assert_eq!(names, vec!["t0", "t1", "t2", "t3", "t4"]);
        let (all, hidden) = folded_rows(&rows, true);
        assert_eq!(all.len(), 8);
        assert_eq!(hidden, 0);
    }

    /// A subtree longer than the quota does not stretch the quota: the boundary
    /// falls back to the leader alone, so no member is ever shown without the
    /// leader it hangs off and the projection never exceeds the bound.
    #[test]
    fn a_quota_landing_mid_subtree_cuts_back_to_the_leader() {
        let rows = vec![
            rendered("l1", None, 0),
            rendered("m1", Some("l1"), 1),
            rendered("m2", Some("l1"), 1),
            rendered("m3", Some("l1"), 1),
            rendered("m4", Some("l1"), 1),
            rendered("m5", Some("l1"), 1),
            rendered("l2", None, 0),
        ];
        let (shown, hidden) = folded_rows(&rows, false);
        let names: Vec<&str> = shown.iter().map(|r| r.row.id()).collect();
        assert_eq!(names, vec!["l1"]);
        assert_eq!(hidden, 6);
    }

    /// A partition within the quota never folds, so no reveal appears.
    #[test]
    fn a_partition_within_the_quota_never_folds() {
        let rows = vec![rendered("a", None, 0), rendered("b", None, 0)];
        let (shown, hidden) = folded_rows(&rows, false);
        assert_eq!(shown.len(), 2);
        assert_eq!(hidden, 0);
    }

    /// The cut never lands inside a leader's subtree: no member can be shown
    /// without the leader that owns its indent guide.
    #[test]
    fn folding_never_splits_a_leader_from_its_members() {
        let rows = vec![
            rendered("l1", None, 0),
            rendered("m1", Some("l1"), 1),
            rendered("m2", Some("l1"), 1),
            rendered("l2", None, 0),
            rendered("m3", Some("l2"), 1),
            rendered("l3", None, 0),
        ];
        let (shown, hidden) = folded_rows(&rows, false);
        let names: Vec<&str> = shown.iter().map(|r| r.row.id()).collect();
        // The quota of 5 covers l1's subtree and l2's whole subtree exactly; the
        // next unit (l3) is what folds away.
        assert_eq!(names, vec!["l1", "m1", "m2", "l2", "m3"]);
        assert_eq!(hidden, 1);
        for r in &shown {
            if let SidebarRow::Thread(s) = &r.row
                && let Some(parent) = s.parent_id.as_deref().filter(|_| r.indent > 0.0)
            {
                assert!(
                    names.contains(&parent),
                    "{} shown without its leader {parent}",
                    s.id
                );
            }
        }
    }

    /// The team forest is a pure projection: top-level rows keep the order they
    /// arrive in (the caller's view account, ultimately the server's manual
    /// account), members nest indented right after their leader, a collapsed
    /// leader hides its subtree, and an orphan stays top-level.
    #[test]
    fn team_forest_nests_members_and_honors_collapse() {
        let thread = |id: &str, parent: Option<&str>, depth: i32, at: i64| {
            let mut s = sample_item();
            s.id = id.into();
            s.parent_id = parent.map(str::to_string);
            s.depth = depth;
            s.updated_at = at as i32;
            s
        };
        // Deliberately NOT stamp-ordered: the stamps must not move anything.
        let threads = vec![
            thread("leader", None, 0, 100),
            thread("member-old", Some("leader"), 1, 50),
            thread("member-new", Some("leader"), 1, 200),
            thread("orphan", Some("gone"), 0, 300),
        ];

        let collapsed = HashSet::new();
        let rows = team_forest(&collapsed, &threads, &[]);
        let ids: Vec<&str> = rows.iter().map(|r| r.row.id()).collect();
        // Incoming order, with each member pulled under its leader.
        assert_eq!(ids, vec!["leader", "member-old", "member-new", "orphan"]);
        assert!(rows[0].team_leader);
        assert_eq!(rows[1].indent, 14.0);
        assert_eq!(rows[2].indent, 14.0);
        assert!(!rows[3].team_leader);

        let collapsed = HashSet::from(["leader".to_string()]);
        let rows = team_forest(&collapsed, &threads, &[]);
        let ids: Vec<&str> = rows.iter().map(|r| r.row.id()).collect();
        assert_eq!(ids, vec!["leader", "orphan"]);
        assert!(rows[0].team_collapsed);
    }

    /// The forest never re-sorts: shuffling the incoming order shuffles the
    /// output identically, even though every stamp stayed where it was. This is
    /// the regression lock for rows that used to drift on background activity.
    #[test]
    fn team_forest_preserves_incoming_order_regardless_of_stamps() {
        let thread = |id: &str, at: i64| {
            let mut s = sample_item();
            s.id = id.into();
            s.updated_at = at as i32;
            s
        };
        let one = vec![thread("a", 300), thread("b", 100), thread("c", 200)];
        let ids: Vec<String> = team_forest(&HashSet::new(), &one, &[])
            .iter()
            .map(|r| r.row.id().to_string())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);

        let other = vec![thread("c", 200), thread("a", 300), thread("b", 100)];
        let ids: Vec<String> = team_forest(&HashSet::new(), &other, &[])
            .iter()
            .map(|r| r.row.id().to_string())
            .collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    /// The unified row projection: a selected external session carries the
    /// same `selected` flag, id-namespace, and a non-transparent wash as a
    /// selected manox thread, so `set_selected(external_id)` highlights its
    /// row through the same `render_thread_item` wash the threads use.
    #[test]
    fn external_selection_mirrors_thread_selection() {
        let theme = real_theme();

        let thread = SidebarThreadItem::from_wire(
            &sample_item(),
            true,
            None,
            RowNesting {
                indent: px(0.),
                team_leader: false,
                team_collapsed: false,
                nested: false,
            },
            &theme,
        );
        assert!(thread.selected);
        assert_eq!(thread.id, "thread-abcdef12");
        assert_eq!(
            thread.wash,
            approval_mode_color(PermissionMode::default().as_i64(), &theme)
        );
        assert!(thread.wash.a > 0.0);
        assert!(matches!(thread.kind, RowKind::Thread { archived: false }));

        let external = SidebarThreadItem::from_external(&sample_external(), true, px(0.), &theme);
        assert!(external.selected);
        // The row's id is the same string the click emits (`OpenExternalSession`)
        // and the workspace feeds to `set_selected`, so the selection lands on
        // this row's `is_selected` check.
        assert_eq!(external.id, "external:claude:deadbeef");
        // Fully unified: the external row's wash is the same visible tint a
        // default (Workspace Access) manox thread carries — never the near-white
        // `theme.accent`, which disappears against the light sidebar.
        assert_eq!(external.wash, thread.wash);
        assert_eq!(external.wash, theme.info);
        assert!(external.wash.a > 0.0);
        assert!(matches!(external.kind, RowKind::External));
    }

    /// Without a cx session id (plain PTY sessions, or an IPC bind failure)
    /// the id tag shows the trailing uuid segment of the manox-internal id —
    /// never the `external` prefix every such row would otherwise share.
    #[test]
    fn external_short_id_falls_back_to_uuid_segment() {
        let theme = real_theme();
        let mut summary = sample_external();
        summary.kind = crate::external_session::SessionKind::Terminal;
        summary.cx_session_id = String::new();
        summary.id = "external:terminal:0123abcd-uuid".into();
        let item = SidebarThreadItem::from_external(&summary, false, px(0.), &theme);
        assert_eq!(item.short_id, "0123abcd");
    }

    /// Deselected rows stay deselected for both kinds (hover-only feedback),
    /// so the `selected` flag alone drives the persistent wash.
    #[test]
    fn external_and_thread_rows_share_deselected_state() {
        let theme = real_theme();
        let thread = SidebarThreadItem::from_wire(
            &sample_item(),
            false,
            None,
            RowNesting {
                indent: px(0.),
                team_leader: false,
                team_collapsed: false,
                nested: false,
            },
            &theme,
        );
        let external = SidebarThreadItem::from_external(&sample_external(), false, px(0.), &theme);
        assert!(!thread.selected);
        assert!(!external.selected);
    }

    /// U2 row projection (cross-domain #1 adoption): the live flags AND the
    /// decoration columns (tag chip, approval wash) ride the wire row, and
    /// the leaf's client-owned unread mirror wins over the deprecated row
    /// flag.
    #[test]
    fn from_wire_projects_the_wire_columns_and_leaf_unread() {
        let theme = real_theme();
        let mut item = sample_item();
        item.running = true;
        item.pending_plan = true;
        item.background_work = true;
        item.tag = Some("chip".into());
        item.approval_mode = Some(PermissionMode::default().as_i64());
        let nesting = RowNesting {
            indent: px(0.),
            team_leader: false,
            team_collapsed: false,
            nested: false,
        };
        // The leaf mirror (GW5 badge source) wins over the row's deprecated
        // constant-false unread.
        let with_override = SidebarThreadItem::from_wire(&item, false, Some(true), nesting, &theme);
        assert!(with_override.has_unread, "the leaf mirror wins");
        assert!(
            with_override.running && with_override.pending_plan && with_override.background_work,
            "the live flags ride the wire row"
        );
        assert_eq!(
            with_override.tag.as_deref(),
            Some("chip"),
            "the tag chip rides the wire row"
        );
        assert_eq!(
            with_override.wash,
            approval_mode_color(PermissionMode::default().as_i64(), &theme),
            "the wash rides the wire row's approval column"
        );
        // No leaf: fall back to the row value (the client-owned mirror the
        // multiplexer keeps — false on a fresh snapshot row).
        let no_override = SidebarThreadItem::from_wire(&item, false, None, nesting, &theme);
        assert!(!no_override.has_unread);
        // A row without decoration columns: default wash, no tag — it still
        // renders (the optional columns' absence is legal).
        let mut bare_item = sample_item();
        bare_item.tag = None;
        bare_item.approval_mode = None;
        let bare = SidebarThreadItem::from_wire(&bare_item, false, None, nesting, &theme);
        assert_eq!(bare.tag, None);
        assert_eq!(
            bare.wash,
            approval_mode_color(PermissionMode::default().as_i64(), &theme)
        );
    }

    /// The pinned slot shows the Conversations header when no registered
    /// projects exist — the conversations section is the only section, so its
    /// header is pinned from the top regardless of scroll.
    #[test]
    fn pinned_section_without_projects_is_conversations() {
        assert_eq!(
            pinned_section(false, px(0.), None),
            PinnedSection::Conversations
        );
        assert_eq!(
            pinned_section(false, px(120.), Some(px(100.))),
            PinnedSection::Conversations
        );
    }

    /// Inside the projects section the Projects header stays pinned; once the
    /// projects content has fully scrolled past the top the Conversations
    /// header takes over.
    #[test]
    fn pinned_section_tracks_projects_boundary() {
        // First frame (no measurement yet) stays on Projects.
        assert_eq!(pinned_section(true, px(50.), None), PinnedSection::Projects);
        assert_eq!(
            pinned_section(true, px(0.), Some(px(200.))),
            PinnedSection::Projects
        );
        assert_eq!(
            pinned_section(true, px(199.), Some(px(200.))),
            PinnedSection::Projects
        );
        assert_eq!(
            pinned_section(true, px(200.), Some(px(200.))),
            PinnedSection::Conversations
        );
        assert_eq!(
            pinned_section(true, px(500.), Some(px(200.))),
            PinnedSection::Conversations
        );
    }

    /// The loose/folder partition for external sessions: unbound rows and
    /// rows bound to an unregistered path (removed folder, or a cwd never
    /// bound as a project) stay loose; only a registered project pulls its
    /// sessions into the folder group.
    #[test]
    fn external_session_is_loose_partitions_by_registered_projects() {
        let known = vec!["/p/a".to_string()];
        assert!(external_session_is_loose(None, &known));
        assert!(!external_session_is_loose(
            Some(std::path::Path::new("/p/a")),
            &known
        ));
        assert!(external_session_is_loose(
            Some(std::path::Path::new("/p/b")),
            &known
        ));
        assert!(external_session_is_loose(
            Some(std::path::Path::new("/home/user")),
            &[]
        ));
    }

    /// A sidecar-restored row carries its resumable/resuming states into the
    /// unified row item, so the renderer can dim it and swap in the loading
    /// indicator without touching the live-row path.
    #[test]
    fn resumable_row_propagates_resume_states() {
        let theme = real_theme();
        let mut summary = sample_external();
        summary.resumable = true;
        let item = SidebarThreadItem::from_external(&summary, false, px(0.), &theme);
        assert!(item.resumable);
        assert!(!item.resuming);
        summary.resuming = true;
        let item = SidebarThreadItem::from_external(&summary, false, px(0.), &theme);
        assert!(item.resuming);
    }

    /// The acceptance-run crash: opening a project group's new-session
    /// menu builds the provider→model cascades EAGERLY inside the
    /// Sidebar's own update listener, and the cascade builder's
    /// `sidebar.upgrade().read(cx)` double-leased the entity — GPUI
    /// panics "cannot read Sidebar while it is already being updated"
    /// (and the objc callback frame turns the panic into an abort in
    /// the real app). The cascade builders now receive the mux handle
    /// up front; opening either menu shape inside an update must build
    /// cleanly.
    struct MenuHost;
    impl Render for MenuHost {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::Empty
        }
    }

    #[gpui::test]
    fn new_session_menu_builds_inside_sidebar_update(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let window = cx.open_window(
            gpui::size(gpui::px(960.), gpui::px(640.)),
            move |window, cx| {
                let host = cx.new(|_| MenuHost);
                gpui_component::Root::new(host, window, cx)
            },
        );
        cx.run_until_parked();
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        let sidebar = cx.new(|cx| Sidebar::new(gpui::px(240.), cx));
        // The project-group shape: the crash path (build_project_menu →
        // the nested per-agent cascade submenus).
        visual.update(|window, cx| {
            sidebar.update(cx, |s, cx| {
                s.open_new_session_menu(Some(std::path::PathBuf::from("/tmp/p")), window, cx);
            });
        });
        // The flat shape: the cascade submenus built directly.
        visual.update(|window, cx| {
            sidebar.update(cx, |s, cx| {
                s.open_new_session_menu(None, window, cx);
            });
        });
    }
}
