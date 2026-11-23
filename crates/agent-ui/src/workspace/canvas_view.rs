//! The canvas renderer: the container's dock regions, its layout tree, the
//! tab strips and the sheet bodies placed on them.
//!
//! Layout geometry comes from `steer_canvas::projection` (pure); this module
//! only draws what that reports and turns pointer gestures into [`CanvasHost`]
//! commands. Every gesture commits through the host's single write path, so a
//! stale reply from a canvas that is no longer shown is dropped by the state,
//! not by the renderer remembering to check.
//!
//! The four interaction disciplines (PLAN-ABSORB-08):
//!
//! - **the layout has one writer**: drags, sizes, dock toggles and zoom all
//!   commit through the host; the renderer reads back whatever landed;
//! - **placement is a pure function**: `projection::drop_target` resolves a
//!   pointer against the projected frame — no per-element hover state;
//! - **the canvas never reads the workspace**: titles/cwd arrive injected;
//! - **a move-drag writes nothing until the drop**; size drags commit a
//!   ticket per move (real rubber-band geometry) and let the save debounce
//!   converge.
//!
//! Bodies: a sheet's content is produced by the entity that owns it — the
//! conversation column's own view for `session:` sheets, the launcher's for
//! `home:`. The canvas never builds another entity's page, so it can never
//! take a second lease on the workspace while the shell is rendering.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;
use gpui_base::InteractiveElementExt as _;
use gpui_component::ActiveTheme;
use gpui_component::{ElementExt as _, Icon, Sizable as _, h_flex, v_flex};

use steer_canvas::id::{home_sheet_id, sheet_kind, terminal_resource, terminal_sheet_id};
use steer_canvas::layout::{DockSide, DropTarget, SplitEdge};
use steer_canvas::projection::{self, GroupFrame, Indicator, Point, SplitHandle, TabBarBox};

use crate::CanvasToggleZoom;
use crate::Workspace;
use crate::views::centered;
use crate::workspace::canvas_host::{CanvasAction, CanvasHost};
use crate::workspace::chat_column::ChatColumn;
use crate::workspace::column_view::ChatColumnView;
use steer_agent_chat_ui::pi::AcpStore;
use steer_agent_chat_ui::views::pi::PiSessionView;
use steer_canvas::persist;

/// The tab strip's height (the sheet frames start below it).
pub const TAB_STRIP_H: f32 = projection::TAB_STRIP_H;

/// The switcher's travel: the gallery's `motion.move` spring, expressed as
/// the duration + easing the pane's indicator already uses.
const SLIDE_DURATION: std::time::Duration = std::time::Duration::from_millis(504);

/// The ghost card a tab drag carries (the tab's shape, DESIGN §3.7).
const GHOST_WIDTH: f32 = 240.;
const GHOST_OPACITY: f32 = 0.92;
/// How faint the source tab sits while its ghost follows the pointer.
const SOURCE_FADE: f32 = 0.35;

/// A center split's minimum slot: a drag never squeezes a group below this.
const SPLIT_MIN: f32 = 80.;

/// The tab-drag payload: the dragged sheet's identity and its label/glyph,
/// so the ghost needs nothing from the canvas while it follows the pointer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DraggedSheet {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) glyph: &'static str,
}

impl Render for DraggedSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .w(px(GHOST_WIDTH))
            .h(px(TAB_STRIP_H))
            .px_2()
            .gap_1()
            .items_center()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .shadow_md()
            .opacity(GHOST_OPACITY)
            .child(Icon::from(glyph_icon(self.glyph)).small())
            .child(gpui::div().text_sm().truncate().child(self.label.clone()))
    }
}

/// A center split's resize payload (the path names the split; the drag move
/// resolves it against the current frame).
#[derive(Clone, PartialEq)]
pub(crate) struct DraggedSplit {
    pub(crate) path: Vec<usize>,
}

impl Render for DraggedSplit {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// A dock edge's resize payload.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DraggedDock {
    pub(crate) side: DockSide,
}

impl Render for DraggedDock {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// The canvas surface of one container.
pub struct CanvasView {
    pub(crate) host: CanvasHost,
    /// The conversation column, when the shell has one: the `session:` sheets'
    /// content.
    pub(crate) column: Option<Entity<ChatColumnView>>,
    /// The terminal sheets' PTY-backed views, keyed by sheet id. The entity is
    /// the resource: dropping it ends the shell, so the map is pruned against
    /// the sheets any canvas still holds.
    terminals: HashMap<String, Entity<steer_ghostty::GhosttyView>>,
    /// The pi sheet bodies, keyed by sheet id. One entity per sheet; the
    /// store behind it owns the process (a closed sheet closes its session).
    pi_views: HashMap<String, Entity<PiSessionView>>,
    /// The pi harness's session store (owned by the workspace, shown here).
    pi_store: Entity<AcpStore>,
    /// The right dock's tool kinds (webview/editor/diff/agent/subagent):
    /// the per-sheet state the registry's body factories mint-or-reuse.
    pub(crate) panes: Entity<crate::canvas_kinds::CanvasPaneStore>,
    /// The shown container's working directory: a terminal sheet spawns there.
    /// Injected by the workspace when the canvas opens a container (the canvas
    /// never reads the workspace itself — that would take a second lease while
    /// the shell is updating it).
    cwd: std::path::PathBuf,
    workspace: WeakEntity<Workspace>,
    /// The card's interior size, measured at prepaint (the first frame of a
    /// fresh window falls back to the window's).
    size: Rc<std::cell::Cell<Option<Size<Pixels>>>>,
    /// The live drag's target + indicator, from `projection::drop_target`.
    /// `None` when the pointer is over nothing droppable.
    drop: Option<projection::DropPreview>,
    /// The sheet being dragged (its tab fades while the ghost flies).
    dragging: Option<String>,
    /// Each group's last displayed sheet, keyed by group key: the slide
    /// animation's "from" place (only a one-place turn slides).
    last_active: HashMap<String, String>,
    /// The open tab context menu (sheet id, menu entity) and its dismissal
    /// subscription.
    tab_menu: Option<(String, Entity<gpui_component::menu::PopupMenu>)>,
    /// The menu's anchor, in window coordinates.
    tab_menu_anchor: Option<gpui::Point<Pixels>>,
    _tab_menu_sub: Option<gpui::Subscription>,
}

/// Resolves a sheet's display title from the host's data face.
///
/// Injected at assembly because the canvas paints inside the workspace's own
/// update — reading the workspace here would take a second lease. The
/// conversation entity is a sibling of the canvas, so titles come through it
/// instead (session titles are the fold's, the same string the sidebar shows).
pub(crate) type SheetTitles =
    std::sync::Arc<dyn Fn(&str, &App) -> Option<SharedString> + Send + Sync>;

/// The v1 title source: a session sheet is titled by its session's fold title
/// (live state first, then the catalogue summary); every other kind is named
/// statically by its registry entry.
pub(crate) fn sheet_titles(chat: Entity<ChatColumn>) -> SheetTitles {
    std::sync::Arc::new(move |sheet: &str, cx: &App| {
        let session = steer_canvas::id::session_of_sheet(sheet)?;
        let chat = chat.read(cx);
        let (store, _) = chat.store.as_ref()?;
        let store = store.read(cx);
        let book = &store.book;
        if let Some(title) = crate::ahp_store::leaf(book, session)
            .display_title()
            .filter(|title| !title.is_empty())
        {
            return Some(SharedString::from(title));
        }
        book.summaries
            .get(session)
            .map(|summary| summary.title.as_str())
            .filter(|title| !title.is_empty())
            .map(SharedString::from)
    })
}

/// A kind's static title (the home page and terminals have no data-face name
/// of their own in v1).
fn static_title(key: &'static str) -> steer_canvas::registry::TitleFn {
    std::sync::Arc::new(move |_, _| crate::i18n::t(key))
}

/// The v1 sheet kinds. `home` is the container's start page; `session` is a
/// conversation, named by its tab title and never by the container (the
/// container's own name is the projection over the earliest surviving sheet
/// that *does* name it).
///
/// Every `body` is `None`: the canvas paints its sheets from the entities it
/// already holds, so a kind owns existence/title/closability and the last mile
/// of painting is the host's. A kind whose own store builds the view (the pi
/// sheet) registers a factory instead.
fn v1_registry(
    titles: SheetTitles,
    pi_store: Entity<AcpStore>,
    panes: &Entity<crate::canvas_kinds::CanvasPaneStore>,
) -> steer_canvas::SheetRegistry {
    use std::sync::Arc;
    use steer_canvas::{ParsedSheet, SheetKindSpec};

    let kind = |kind: &'static str,
                glyph: &'static str,
                names_container: bool,
                title: steer_canvas::registry::TitleFn,
                tab_title: Option<steer_canvas::registry::TabTitleFn>,
                parse: Option<steer_canvas::registry::ParseFn>|
     -> SheetKindSpec {
        SheetKindSpec {
            kind,
            title,
            tab_title,
            glyph,
            closable: true,
            names_container,
            restore: None,
            parse,
            // The canvas renders these bodies inline from the entities it
            // holds; the kind still owns existence, title and closability.
            body: None,
        }
    };
    let mut registry = steer_canvas::SheetRegistry::new().with(pi_kind(pi_store));
    for spec in crate::canvas_kinds::pane_kinds(panes.clone()) {
        registry = registry.with(spec);
    }
    registry
        .with(kind(
            "home",
            "home",
            false,
            static_title("canvas-tab-home"),
            None,
            None,
        ))
        .with(kind(
            "terminal",
            "terminal",
            false,
            static_title("canvas-tab-terminal"),
            None,
            Some(Arc::new(|id| {
                let resource = terminal_resource(id);
                ParsedSheet {
                    container_id: resource.as_ref().map(|r| r.container_id.clone()),
                    instance: resource.map(|r| r.index.to_string()).unwrap_or_default(),
                }
            })),
        ))
        .with(kind(
            "session",
            "chat",
            true,
            Arc::new(move |id, cx| {
                titles(id, cx).unwrap_or_else(|| crate::i18n::t("canvas-tab-new-chat"))
            }),
            // The tab-only projection (PLAN-ABSORB-02): a program's OSC title
            // renames its tab, never its container.
            Some(Arc::new(|id, _| {
                let session = steer_canvas::id::session_of_sheet(id)?;
                crate::title_store::TitleStore::global()
                    .program_title(session)
                    .map(SharedString::from)
            })),
            Some(Arc::new(|id| ParsedSheet::workspace_wide(id))),
        ))
}

/// The tab glyph for a registered glyph name; unknown names keep the
/// document fallback.
fn glyph_icon(glyph: &str) -> gpui_kit_assets::IconName {
    use gpui_kit_assets::IconName;
    match glyph {
        "home" => IconName::House,
        "terminal" => IconName::SquareTerminal,
        "chat" => IconName::MessageCircle,
        "pi" => IconName::Sparkles,
        "frame" => IconName::Frame,
        "code" => IconName::Code,
        "bot" => IconName::Bot,
        _ => IconName::File,
    }
}

impl CanvasView {
    pub(crate) fn new(
        host: CanvasHost,
        workspace: WeakEntity<Workspace>,
        pi_store: Entity<AcpStore>,
        panes: Entity<crate::canvas_kinds::CanvasPaneStore>,
    ) -> Self {
        Self {
            host,
            column: None,
            terminals: HashMap::new(),
            pi_views: HashMap::new(),
            pi_store,
            panes,
            cwd: std::env::current_dir().unwrap_or_default(),
            workspace,
            size: Rc::new(std::cell::Cell::new(None)),
            drop: None,
            dragging: None,
            last_active: HashMap::new(),
            tab_menu: None,
            tab_menu_anchor: None,
            _tab_menu_sub: None,
        }
    }

    pub(crate) fn set_column(&mut self, column: Entity<ChatColumnView>, cx: &mut Context<Self>) {
        self.column = Some(column);
        cx.notify();
    }

    /// Shows `container_id`'s canvas, seeding its start page and — when the
    /// container is a session — that session's sheet.
    pub(crate) fn open_container(
        &mut self,
        container_id: &str,
        session_id: Option<&str>,
        cwd: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.cwd = cwd;
        let saved = persist::read_canvas(container_id).map(|file| file.snapshot());
        let alive = |_sheet: &str| true;
        self.host.switch_container(container_id, saved, alive);
        self.host.open_sheet(&home_sheet_id(container_id), None);
        if let Some(session_id) = session_id {
            let sheet = steer_canvas::id::session_sheet_id(session_id);
            self.host.open_sheet(&sheet, None);
        }
        cx.notify();
    }

    /// Opens the container's next terminal sheet (its PTY spawns on first
    /// show, so a restored layout re-attaches to a fresh shell).
    pub(crate) fn open_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(container) = self.host.active_container().map(str::to_string) else {
            return;
        };
        let sheet = terminal_sheet_id(&container, self.next_terminal_index(&container));
        self.ensure_terminal(&sheet, cx);
        if self.host.open_sheet(&sheet, None) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// Opens (or focuses) `sheet` in the right dock: the tool kinds' open
    /// channel (the diff card, the agent's browser page, a CLI sheet). A
    /// sheet the canvas already holds only moves there; the dock region
    /// opens with it (the drop is the explicit placement surface).
    pub(crate) fn open_in_right_dock(&mut self, sheet: &str, cx: &mut Context<Self>) {
        let target = DropTarget::Dock {
            side: DockSide::Right,
        };
        if self.host.drop_sheet(sheet, target) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// Closes every shown sheet of a kind prefix (the `/exit` retire face;
    /// the per-container canvas scopes ephemeral sheets by construction).
    pub(crate) fn close_sheets_of_prefix(&mut self, prefix: &str, cx: &mut Context<Self>) {
        let doomed: Vec<String> = steer_canvas::layout::panel_ids(self.host.layout())
            .into_iter()
            .filter(|sheet| sheet.starts_with(prefix))
            .map(str::to_string)
            .collect();
        for sheet in doomed {
            self.panes.update(cx, |panes, cx| panes.reclaim(&sheet, cx));
            if self.host.close_sheet(&sheet) == CanvasAction::Applied {
                self.after_edit(cx);
            }
        }
        cx.notify();
    }

    /// The pane store prunes against the same union the terminal PTYs
    /// prune against — a closed sheet's state ends, a parked canvas's
    /// survives.
    pub(crate) fn prune_panes(&mut self, cx: &mut Context<Self>) {
        let kept: std::collections::HashSet<String> =
            steer_canvas::projection::open_sheet_ids(self.host.layout(), self.host.held())
                .into_iter()
                .collect();
        self.panes.update(cx, |panes, cx| panes.prune(&kept, cx));
    }

    /// The container's next unused instance index for a minting kind.
    pub(crate) fn next_kind_index(&self, container: &str, prefix: &str) -> u32 {
        let mut index = 1;
        let taken: std::collections::HashSet<u32> =
            steer_canvas::projection::open_sheet_ids(self.host.layout(), self.host.held())
                .into_iter()
                .filter_map(|sheet| {
                    let rest = sheet.strip_prefix(prefix)?.strip_prefix(':')?;
                    let rest = rest.strip_prefix(container)?.strip_prefix(':')?;
                    rest.parse::<u32>().ok()
                })
                .collect();
        while taken.contains(&index) {
            index += 1;
        }
        index
    }

    /// Opens a CLI agent sheet in the right dock: the body is the picker
    /// (agent segment → model segment → the TUI), so one kind carries every
    /// CLI and the spawn's kill handle rides the sheet.
    pub(crate) fn open_agent_sheet(&mut self, cx: &mut Context<Self>) {
        let Some(container) = self.host.active_container().map(str::to_string) else {
            return;
        };
        let index = self.next_kind_index(&container, "agent");
        let sheet = steer_canvas::id::agent_sheet_id(&container, index);
        self.open_in_right_dock(&sheet, cx);
    }

    /// Opens the container's next terminal sheet straight into the bottom
    /// dock — opening the dock is asking for a terminal there.
    pub(crate) fn open_terminal_into_dock(&mut self, cx: &mut Context<Self>) {
        let Some(container) = self.host.active_container().map(str::to_string) else {
            return;
        };
        let sheet = terminal_sheet_id(&container, self.next_terminal_index(&container));
        self.ensure_terminal(&sheet, cx);
        let target = DropTarget::Dock {
            side: DockSide::Bottom,
        };
        if self.host.drop_sheet(&sheet, target) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// Opens a fresh pi conversation as a sheet of the shown container. The
    /// process starts immediately; the body appears on the sheet's first show.
    pub(crate) fn open_pi_session(&mut self, cwd: std::path::PathBuf, cx: &mut Context<Self>) {
        let session_id = steer_agent_chat_ui::pi::mint_session_id();
        let sheet = format!("pi-session:{session_id}");
        self.pi_store
            .update(cx, |store, cx| store.open_new(&session_id, cwd, cx));
        if self.host.open_sheet(&sheet, None) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    fn next_terminal_index(&self, container: &str) -> u32 {
        let mut index = 1;
        while self
            .terminals
            .contains_key(&terminal_sheet_id(container, index))
            || steer_canvas::layout::has_sheet(
                self.host.layout(),
                &terminal_sheet_id(container, index),
            )
        {
            index += 1;
        }
        index
    }

    /// Makes sure a terminal sheet's PTY view exists (spawned once per sheet).
    fn ensure_terminal(&mut self, sheet: &str, cx: &mut Context<Self>) {
        if self.terminals.contains_key(sheet) {
            return;
        }
        if terminal_resource(sheet).is_none() {
            return;
        }
        let cwd = self.cwd.clone();
        match steer_ghostty::spawn_ghostty_view(&cwd, steer_ghostty::Theme::LIGHT, cx) {
            Ok(view) => {
                self.terminals.insert(sheet.to_string(), view);
            }
            Err(error) => {
                tracing::warn!(%error, "canvas terminal spawn failed");
            }
        }
    }

    /// Whether the canvas holds a sheet of `prefix` for `container_id`
    /// (shown or parked — the `/exit` probe reads the union).
    pub(crate) fn holds_kind_for(&self, container_id: &str, prefix: &str) -> bool {
        steer_canvas::projection::open_sheet_ids(self.host.layout(), self.host.held())
            .iter()
            .any(|sheet| {
                sheet.starts_with(prefix)
                    && sheet
                        .strip_prefix(prefix)
                        .and_then(|rest| rest.strip_prefix(':'))
                        .and_then(|rest| rest.strip_prefix(container_id))
                        .is_some_and(|rest| rest.starts_with(':'))
            })
    }

    /// The container's terminal sheets still open anywhere on its canvas
    /// (shown or parked) — the `/exit` teardown's read.
    pub(crate) fn terminal_sheets(&self, container_id: &str) -> Vec<String> {
        steer_canvas::projection::open_sheet_ids(self.host.layout(), self.host.held())
            .into_iter()
            .filter(|sheet| {
                sheet_kind(sheet) == "terminal"
                    && terminal_resource(sheet)
                        .is_some_and(|resource| resource.container_id == container_id)
            })
            .collect()
    }

    /// Closes sheets without a window (the `/exit` leg): an AutoArchive the
    /// close would ask for is dropped — the caller is tearing the container
    /// down itself.
    pub(crate) fn close_sheets_detached(&mut self, sheets: &[String], cx: &mut Context<Self>) {
        for sheet in sheets {
            if sheet_kind(sheet) == "pi-session" {
                self.pi_views.remove(sheet);
                let session_id = pi_session_of_sheet(sheet).to_owned();
                self.pi_store
                    .update(cx, |store, cx| store.close_session(&session_id, cx));
            }
            if self.host.close_sheet(sheet) == CanvasAction::Applied {
                self.prune_terminals();
                self.after_edit(cx);
            }
        }
        cx.notify();
    }

    /// The card's measured interior size, in points.
    #[cfg(feature = "test-support")]
    pub(crate) fn measured_size(&self) -> (f32, f32) {
        self.canvas_size()
    }

    /// The sheet being dragged, if any (the source-fade read).
    #[cfg(feature = "test-support")]
    pub(crate) fn dragging_id(&self) -> Option<&str> {
        self.dragging.as_deref()
    }

    /// Stages a drag WITHOUT a pointer (the visual scenes' stand-in for a
    /// held mouse): the same pure hit test a real drag move runs, with the
    /// source tab's fade and the indicator layer rendered from the result.
    /// Nothing commits — the layout is only ever written by a drop.
    #[cfg(feature = "test-support")]
    pub(crate) fn stage_drag(&mut self, sheet: &str, x: f32, y: f32, cx: &mut Context<Self>) {
        let (width, height) = self.canvas_size();
        if width <= 0. || height <= 0. {
            return;
        }
        let layout = self.host.layout().clone();
        let zoomed = self.host.zoomed().map(str::to_string);
        let frame = projection::canvas_frame(&layout, width, height, zoomed.as_deref());
        let boxes = projection::tab_bar_boxes(&layout, width, height, zoomed.as_deref());
        self.dragging = Some(sheet.to_string());
        self.drop = projection::drop_target(&frame, &boxes, Point { x, y });
        cx.notify();
    }

    /// The post-edit faces a host-routed gesture needs (PTY pruning, the
    /// debounced save, repaint).
    #[cfg(feature = "test-support")]
    pub(crate) fn prune_after_edit(&mut self, cx: &mut Context<Self>) {
        self.prune_terminals();
        self.after_edit(cx);
        cx.notify();
    }

    /// Drops the PTY views of sheets no canvas holds any more (closed here, or
    /// on a parked canvas that was evicted). The union is the only judge — a
    /// layout edit that closes a tab while a parked canvas keeps it must not
    /// end the shell.
    fn prune_terminals(&mut self) {
        let alive: std::collections::HashSet<String> =
            steer_canvas::projection::open_sheet_ids(self.host.layout(), self.host.held())
                .into_iter()
                .collect();
        self.terminals.retain(|sheet, _| alive.contains(sheet));
    }

    /// Keeps the canvas on the workspace's foreground container: the container
    /// view IS the canvas, so every attach / sidebar click lands here and the
    /// previous canvas parks (its layout is kept, its PTYs stay alive).
    pub(crate) fn sync_foreground(
        &mut self,
        container_id: Option<&str>,
        session_id: Option<&str>,
        cwd: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        match container_id {
            Some(container_id) if self.host.active_container() != Some(container_id) => {
                self.open_container(container_id, session_id, cwd, cx);
            }
            None if self.host.active_container().is_some() => {
                self.show_scratch(cx);
            }
            _ => {}
        }
    }

    /// A container was just minted: when the canvas is still the scratch one,
    /// its sheets move into the new container.
    pub(crate) fn mint_container(
        &mut self,
        container_id: &str,
        cwd: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.host.active_container().is_some() {
            return;
        }
        self.cwd = cwd;
        self.host.migrate_scratch(container_id);
        self.prune_terminals();
        self.after_edit(cx);
        cx.notify();
    }

    /// Shows the scratch canvas (the shell's pre-container surface).
    pub(crate) fn show_scratch(&mut self, cx: &mut Context<Self>) {
        self.host.show_scratch();
        cx.notify();
    }

    /// Schedules the debounced layout write (the newest edit wins).
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let Some((generation, container_id, snapshot)) = self.host.pending_save() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::canvas_host::SAVE_DEBOUNCE)
                .await;
            let _ = this.update(cx, |view, _| {
                if view.host.save_generation() != generation {
                    return;
                }
                let file: persist::CanvasFile =
                    persist::CanvasFile::new(container_id, snapshot.layout, snapshot.zoom);
                if let Err(error) = persist::write_canvas(&file) {
                    tracing::warn!(%error, "canvas layout write failed");
                }
            });
        })
        .detach();
    }
}

impl CanvasView {
    /// The canvas page: the dock regions, the center's groups, the drag bands,
    /// the live drop indicator and the tab menu.
    pub(crate) fn render_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let size = self.size.get().unwrap_or_else(|| window.bounds().size);
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let layout = self.host.layout().clone();
        let zoomed = self.host.zoomed().map(str::to_string);
        let frame = projection::canvas_frame(&layout, width, height, zoomed.as_deref());
        let boxes = projection::tab_bar_boxes(&layout, width, height, zoomed.as_deref());
        let groups = projection::group_frames(&boxes, TAB_STRIP_H);
        let scratch = self.host.active_container().is_none();

        let mut root = v_flex()
            .size_full()
            .relative()
            .bg(theme.background)
            .track_focus(&cx.focus_handle())
            .on_action(cx.listener(|this, _: &CanvasToggleZoom, _, cx| {
                this.zoom_foreground(cx);
            }))
            // The move-drag's hit test: the pure projection resolves the
            // pointer against the frame; the layout stays untouched until the
            // drop commits.
            .on_drag_move::<DraggedSheet>(cx.listener(
                |this, ev: &DragMoveEvent<DraggedSheet>, _, cx| {
                    let drag = ev.drag(cx);
                    let Some((pointer, _origin)) = this.canvas_pointer(ev.bounds, &ev.event) else {
                        return;
                    };
                    let Some(preview) = this.resolve_drop(pointer) else {
                        if this.drop.take().is_some() || this.dragging.take().is_some() {
                            cx.notify();
                        }
                        return;
                    };
                    let changed = this.drop.as_ref() != Some(&preview)
                        || this.dragging.as_deref() != Some(drag.id.as_str());
                    this.drop = Some(preview);
                    this.dragging = Some(drag.id.clone());
                    if changed {
                        cx.notify();
                    }
                },
            ))
            // The drop commits once: the whole drag wrote nothing before it.
            .on_drop::<DraggedSheet>(cx.listener(|this, dragged: &DraggedSheet, window, cx| {
                let target = this.drop.take().map(|preview| preview.target);
                this.dragging = None;
                let Some(target) = target else {
                    return;
                };
                this.commit_drop(&dragged.id, target, window, cx);
            }))
            // A split handle's drag: one ticket per move (real geometry), the
            // save debounce converges behind it.
            .on_drag_move::<DraggedSplit>(cx.listener(
                |this, ev: &DragMoveEvent<DraggedSplit>, _, cx| {
                    let path = ev.drag(cx).path.clone();
                    let Some((pointer, _)) = this.canvas_pointer(ev.bounds, &ev.event) else {
                        return;
                    };
                    this.drag_split(&path, pointer, cx);
                },
            ))
            // A dock edge's drag: sizes (and, below the floor, collapses) the
            // region on `side`.
            .on_drag_move::<DraggedDock>(cx.listener(
                |this, ev: &DragMoveEvent<DraggedDock>, _, cx| {
                    let payload = ev.drag(cx);
                    let Some((pointer, _origin)) = this.canvas_pointer(ev.bounds, &ev.event) else {
                        return;
                    };
                    this.drag_dock(payload.side, pointer, cx);
                },
            ));

        for (box_, group) in boxes.iter().zip(groups.iter()) {
            root = root.child(self.render_group(box_, group, scratch, &theme, window, cx));
        }
        if groups.is_empty() {
            // No sheet anywhere: the canvas shows its own start page so the
            // surface is never a dead grey area.
            root = root.child(
                v_flex()
                    .absolute()
                    .inset_0()
                    .items_center()
                    .justify_center()
                    .child(crate::i18n::t("canvas-empty")),
            );
        }

        // The invisible drag bands: center splits first, then each dock edge.
        for handle in &frame.handles {
            root = root.child(split_band(handle, cx));
        }
        for handle in &frame.dock_handles {
            root = root.child(dock_band(handle, cx));
        }

        // The drop indicator, over the pages (its rects sit between tabs or
        // inside the content, so nothing it covers is missed).
        if self.dragging.is_some()
            && let Some(preview) = &self.drop
        {
            root = root.child(indicator_layer(preview, &theme));
        }

        // The tab context menu: an anchored floating layer the menu itself
        // dismisses (outside click / item click both fire DismissEvent).
        if let Some((sheet, menu)) = self.tab_menu.clone() {
            root = root.child(
                gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::TopRight)
                        .position(self.tab_menu_anchor.unwrap_or_default())
                        .offset(gpui::point(px(0.), px(2.)))
                        .child(
                            div()
                                .id(SharedString::from(format!("canvas-tab-menu-{sheet}")))
                                .occlude()
                                .child(menu),
                        ),
                )
                .with_priority(1),
            );
        }

        // The displayed sheets, for the pane store's visibility face (the
        // webviews' OS windows follow it).
        let shown: std::collections::HashSet<String> = boxes
            .iter()
            .filter(|box_| box_.region.is_none())
            .filter_map(|box_| box_.panels.get(box_.active).cloned())
            .collect();
        self.panes
            .update(cx, |panes, cx| panes.note_shown(shown, cx));

        let size_cell = self.size.clone();
        root.on_prepaint(move |bounds, window, _| {
            let next = Some(bounds.size);
            if size_cell.get() != next {
                size_cell.set(next);
                window.refresh();
            }
        })
        .into_any_element()
    }

    /// The card's measured size, in points.
    fn canvas_size(&self) -> (f32, f32) {
        self.size
            .get()
            .map(|s| (f32::from(s.width), f32::from(s.height)))
            .unwrap_or((0., 0.))
    }

    /// The pointer in canvas coordinates (window position minus the canvas
    /// root's origin) and the root's window-space origin.
    ///
    /// `bounds` is the ROOT's — the drag-move handlers mount on it — so its
    /// origin is the canvas's own origin in window space.
    fn canvas_pointer(
        &self,
        bounds: Bounds<Pixels>,
        event: &MouseMoveEvent,
    ) -> Option<(Point, gpui::Point<Pixels>)> {
        Some((
            Point {
                x: f32::from(event.position.x - bounds.left()),
                y: f32::from(event.position.y - bounds.top()),
            },
            gpui::point(bounds.left(), bounds.top()),
        ))
    }

    /// The pure placement: where would this pointer land a drop?
    fn resolve_drop(&self, pointer: Point) -> Option<projection::DropPreview> {
        let (width, height) = self.canvas_size();
        if width <= 0. || height <= 0. {
            return None;
        }
        let layout = self.host.layout().clone();
        let zoomed = self.host.zoomed().map(str::to_string);
        let frame = projection::canvas_frame(&layout, width, height, zoomed.as_deref());
        let boxes = projection::tab_bar_boxes(&layout, width, height, zoomed.as_deref());
        projection::drop_target(&frame, &boxes, pointer)
    }

    /// Commits a drag's drop through the one writer, then runs the standard
    /// post-edit faces (PTY pruning, debounced save, repaint).
    fn commit_drop(
        &mut self,
        sheet: &str,
        target: DropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.host.drop_sheet(sheet, target) {
            CanvasAction::Applied => {
                self.prune_terminals();
                self.prune_panes(cx);
                self.after_edit(cx);
                cx.notify();
            }
            CanvasAction::Stale => {}
            CanvasAction::AutoArchive(container_id) => {
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.canvas_auto_archive(&container_id, window, cx);
                    });
                }
            }
        }
    }

    /// Live split sizing: the pointer's coordinate along the split's axis is
    /// the first slot's size (clamped), the second slot takes the rest.
    fn drag_split(&mut self, path: &[usize], pointer: Point, cx: &mut Context<Self>) {
        let (width, height) = self.canvas_size();
        if width <= 0. {
            return;
        }
        let layout = self.host.layout().clone();
        let zoomed = self.host.zoomed().map(str::to_string);
        let frame = projection::canvas_frame(&layout, width, height, zoomed.as_deref());
        let Some(handle) = frame.handles.iter().find(|h| h.path == path) else {
            return;
        };
        let horizontal = handle.axis == steer_canvas::layout::Axis::Horizontal;
        let total = if horizontal {
            handle.span.width
        } else {
            handle.span.height
        };
        let at = if horizontal {
            pointer.x - handle.span.x
        } else {
            pointer.y - handle.span.y
        };
        let first = at.clamp(
            SPLIT_MIN,
            (total - SPLIT_HANDLE_GAP - SPLIT_MIN).max(SPLIT_MIN),
        );
        let sizes = vec![first, (total - SPLIT_HANDLE_GAP - first).max(SPLIT_MIN)];
        if self.host.set_split_sizes(path, Some(sizes)) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// Live dock sizing: the pointer's distance from the dock's outer edge is
    /// the region's cross-edge size; below the floor the drag collapses the
    /// dock (collapsible semantics), past the floor a closed dock reopens.
    fn drag_dock(&mut self, side: DockSide, pointer: Point, cx: &mut Context<Self>) {
        let (width, height) = self.canvas_size();
        if width <= 0. {
            return;
        }
        let size = match side {
            DockSide::Left => pointer.x,
            DockSide::Right => width - pointer.x,
            DockSide::Bottom => height - pointer.y,
        };
        let was_open = self
            .host
            .layout()
            .region(side)
            .is_some_and(|region| region.open);
        let action = if size < steer_canvas::layout::DOCK_MIN_SIZE {
            if was_open {
                self.host.set_dock_open(side, false)
            } else {
                CanvasAction::Stale
            }
        } else if !was_open {
            let opened = self.host.set_dock_open(side, true);
            if opened == CanvasAction::Applied {
                self.host.set_dock_size(side, size)
            } else {
                opened
            }
        } else {
            self.host.set_dock_size(side, size)
        };
        if action == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// Zooms the foreground center group (the `⌘⇧↵` action's face): the
    /// center's first group's displayed sheet.
    pub(crate) fn zoom_foreground(&mut self, cx: &mut Context<Self>) {
        let Some(sheet) = steer_canvas::layout::first_center_active(self.host.layout()) else {
            return;
        };
        if self.host.toggle_zoom(&sheet) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }
}

/// The 1pt seam a split band straddles.
const SPLIT_HANDLE_GAP: f32 = projection::SPLIT_HANDLE;

/// The tab strip's content inset: the pill floats 2pt inside the track.
const SWITCHER_INSET: f32 = 2.;

impl CanvasView {
    /// One group: its switcher track (the single traveling pill), its
    /// persistent pages (shown + near slide; far stay empty frames), and the
    /// sheet bodies.
    fn render_group(
        &mut self,
        box_: &TabBarBox,
        group: &GroupFrame,
        scratch: bool,
        theme: &gpui_component::Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rect = box_.rect;
        let track = self.render_track(box_, theme, cx);
        let pages = self.render_pages(box_, group, window, cx);

        v_flex()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .overflow_hidden()
            .child(pages)
            .child(track)
            .when(scratch && box_.region.is_none(), |this| this)
            .into_any_element()
    }

    /// The switcher: a track of equal-share cells (one per tab) and ONE pill
    /// that travels under the displayed tab — a sibling of the cells inside a
    /// relative wrapper, so gpui's child-then-border paint order cannot cover
    /// it (the same rule the pane's indicator obeys).
    fn render_track(
        &mut self,
        box_: &TabBarBox,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = box_.panels.len().max(1) as f32;
        let track_width = box_.rect.width;
        let share = track_width / count;

        // The pill's target: the displayed tab's share. A one-place turn
        // travels (the 504/38 spring as duration + quint easing); a jump or a
        // first frame lands directly.
        let active_id = box_.panels.get(box_.active).cloned();
        let target_left = box_.active as f32 * share + SWITCHER_INSET;
        let target_width = (share - SWITCHER_INSET * 2.).max(0.);
        let from_left = self
            .last_active
            .get(&box_.key)
            .and_then(|last| box_.panels.iter().position(|panel| panel == last))
            .map(|last_index| last_index as f32 * share + SWITCHER_INSET);
        let travel = match from_left {
            Some(from) if (from - target_left).abs() > share * 1.5 => None,
            Some(from) => Some((from, target_left)),
            None => None,
        };
        if let Some(id) = &active_id {
            self.last_active.insert(box_.key.clone(), id.clone());
        }

        let mut cells = h_flex().w_full().h_full();
        for (index, sheet) in box_.panels.iter().enumerate() {
            cells = cells.child(self.render_tab_cell(box_, index, sheet, theme, cx));
        }

        let pill_id = format!(
            "canvas-pill-{}-{}",
            box_.key,
            active_id.clone().unwrap_or_default()
        );
        let pill_base = || {
            div()
                .absolute()
                .top(px((TAB_STRIP_H - 26.) / 2.))
                .h(px(26.))
                .rounded(px(13.))
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .shadow_sm()
        };
        let pill = match travel {
            Some((from, to)) => pill_base()
                .with_animation(
                    ElementId::Name(pill_id.into()),
                    gpui::Animation::new(SLIDE_DURATION).with_easing(ease_out_quint()),
                    move |this, delta| {
                        this.left(px(from + (to - from) * delta))
                            .w(px(target_width))
                    },
                )
                .into_any_element(),
            None => pill_base()
                .left(px(target_left))
                .w(px(target_width))
                .into_any_element(),
        };

        // The wrapper exists so the pill can be a SIBLING of the cells (gpui
        // paints a parent's border after its children; a nested pill would be
        // half-covered by the strip's own bottom hairline).
        div()
            .id(ElementId::Name(format!("canvas-strip-{}", box_.key).into()))
            .debug_selector({
                let key = box_.key.clone();
                move || format!("canvas-strip-{key}")
            })
            .relative()
            .w_full()
            .h(px(TAB_STRIP_H))
            .flex_shrink_0()
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.border)
            // A double-click on the track's EMPTY area zooms the group; the
            // tab cells swallow their own double-clicks (the split entry).
            .on_double_click(cx.listener({
                let active = active_id.clone();
                move |this, _: &ClickEvent, _, cx| {
                    if let Some(sheet) = active.clone() {
                        this.toggle_zoom(&sheet, cx);
                    }
                }
            }))
            .child(cells)
            .child(pill)
            // The action cluster floats above the track's right end (the same
            // "+ overlay above the bar" layering nsh-pi uses): its own wash
            // keeps it legible over a long tab row.
            .child(self.render_track_actions(theme, cx))
            .into_any_element()
    }

    /// One switcher cell: the tab's label (equal share, the pill paints the
    /// active state), its drag face, click/double-click faces and its close.
    fn render_tab_cell(
        &mut self,
        box_: &TabBarBox,
        index: usize,
        sheet: &str,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = index == box_.active;
        let label = self
            .host
            .registry()
            .tab_title(sheet, cx)
            .unwrap_or_else(|| SharedString::from(sheet.to_string()));
        let glyph = self.host.registry().glyph(sheet);
        let closable = self.host.registry().closable(sheet);
        let id = sheet.to_string();

        let ghost = DraggedSheet {
            id: id.clone(),
            label: label.to_string(),
            glyph: glyph.unwrap_or("file"),
        };
        let menu_id = id.clone();
        let faded = self.dragging.as_deref() == Some(sheet);

        let mut cell = h_flex()
            .id(ElementId::Name(format!("canvas-tab-{id}").into()))
            .debug_selector({
                let id = id.clone();
                move || format!("canvas-tab-{id}")
            })
            .flex_1()
            .min_w_0()
            .h_full()
            .px_2()
            .gap_1()
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .text_color(if active {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .when(faded, |this| this.opacity(SOURCE_FADE))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                // A double-click opens the start page in a new group beside
                // this tab (the 04 split entry, now a placement drop); the
                // propagation stops so the strip's zoom gesture never fires.
                if event.click_count() >= 2 {
                    cx.stop_propagation();
                    this.split_home(&id, cx);
                    return;
                }
                if this.host.activate_sheet(&id) == CanvasAction::Applied {
                    this.after_edit(cx);
                    cx.notify();
                }
            }))
            .on_drag(ghost, |payload: &DraggedSheet, _, _, cx| {
                cx.new(|_| payload.clone())
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_tab_menu(&menu_id, event.position, window, cx);
                }),
            );

        if let Some(glyph) = glyph {
            cell = cell.child(Icon::from(glyph_icon(glyph)).small());
        }
        cell = cell.child(gpui::div().text_sm().truncate().child(label));

        if closable {
            let close_id = sheet.to_string();
            cell = cell.child(
                gpui::div()
                    .id(ElementId::Name(
                        format!("canvas-tab-close-{close_id}").into(),
                    ))
                    .debug_selector({
                        let close_id = close_id.clone();
                        move || format!("canvas-tab-close-{close_id}")
                    })
                    .px_1()
                    .rounded(theme.radius)
                    .hover(|this| this.bg(theme.muted))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_sheet(&close_id, window, cx);
                    }))
                    .child(gpui_component::Icon::from(gpui_kit_assets::IconName::Close).xsmall()),
            );
        }
        cell.into_any_element()
    }

    /// The strip's floating action cluster: terminal, pi session, start page.
    fn render_track_actions(
        &mut self,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let in_container = self.host.active_container().is_some();
        let mut actions = h_flex()
            .absolute()
            .right_0()
            .top_0()
            .h_full()
            .items_center()
            .gap_1()
            .px_2()
            .bg(theme.title_bar);
        if in_container {
            actions = actions
                .child(
                    gpui::div()
                        .id("canvas-open-terminal")
                        .debug_selector(|| "canvas-open-terminal".to_string())
                        .px_1()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.muted))
                        .on_click(cx.listener(|this, _, _, cx| this.open_terminal(cx)))
                        .child(Icon::from(gpui_kit_assets::IconName::SquareTerminal).xsmall()),
                )
                .child(
                    gpui::div()
                        .id("canvas-open-agent")
                        .debug_selector(|| "canvas-open-agent".to_string())
                        .px_1()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.muted))
                        .on_click(cx.listener(|this, _, _, cx| this.open_agent_sheet(cx)))
                        .child(Icon::from(gpui_kit_assets::IconName::Bot).xsmall()),
                )
                .child(
                    gpui::div()
                        .id("canvas-open-pi")
                        .debug_selector(|| "canvas-open-pi".to_string())
                        .px_1()
                        .rounded(theme.radius)
                        .cursor_pointer()
                        .hover(|this| this.bg(theme.muted))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let cwd = this.cwd.clone();
                            this.open_pi_session(cwd, cx);
                        }))
                        .child(Icon::from(gpui_kit_assets::IconName::Sparkles).xsmall()),
                );
        }
        if let Some(container) = self.host.active_container().map(str::to_string) {
            let home = home_sheet_id(&container);
            actions = actions.child(
                gpui::div()
                    .id("canvas-open-home")
                    .debug_selector(|| "canvas-open-home".to_string())
                    .px_1()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.muted))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.host.open_sheet(&home, None) == CanvasAction::Applied {
                            this.after_edit(cx);
                            cx.notify();
                        }
                    }))
                    .child(Icon::from(gpui_kit_assets::IconName::Plus).xsmall()),
            );
        }
        actions.into_any_element()
    }

    /// The group's persistent pages: shown and near (±1) render real bodies
    /// (a webview or a diff is not built for a tab two places away), the far
    /// ones empty frames; the row translates so the displayed sheet sits at
    /// rest, and a ONE-place turn slides (the animation's reachable set and
    /// the body build set stay equal — a far jump would slide over blanks).
    fn render_pages(
        &mut self,
        box_: &TabBarBox,
        group: &GroupFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let width = group.rect.width;
        let count = box_.panels.len().max(1);
        let rest = -(box_.active as f32 * width);

        let mut pages = h_flex().h_full();
        for (index, sheet) in box_.panels.iter().enumerate() {
            let place = index as i32 - box_.active as i32;
            // near(±1) keeps the slide honest — except the OS-window kinds:
            // a webview ignores CSS clipping, so it exists only on its shown
            // page (a CLI picker, same rule — it is an interactive surface,
            // not a page neighbor).
            let os_window_kind = matches!(sheet_kind(sheet), "webview" | "agent" | "subagent");
            let body = if place.abs() <= 1 && !(os_window_kind && place != 0) {
                self.render_sheet_body(sheet, window, cx)
            } else {
                gpui::div().into_any_element()
            };
            pages = pages.child(
                div()
                    .w(px(width))
                    .h_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(body),
            );
        }
        pages = pages.w(px(width * count as f32));

        // The slide: from the previously displayed sheet's place, only when
        // it is exactly one place away.
        let slide = self
            .last_active
            .get(&box_.key)
            .and_then(|last| box_.panels.iter().position(|panel| panel == last))
            .map(|last_index| -(last_index as i32 - box_.active as i32))
            .filter(|from_place| from_place.abs() == 1)
            .map(|from_place| from_place as f32 * width);
        if let Some(id) = active_id_of(box_) {
            self.last_active.insert(box_.key.clone(), id);
        }

        let row = match slide {
            Some(from) => {
                let to = rest;
                pages
                    .absolute()
                    .top_0()
                    .left(px(from))
                    .h_full()
                    .with_animation(
                        ElementId::Name(
                            format!(
                                "canvas-pages-{}-{}",
                                box_.key,
                                active_id_of(box_).unwrap_or_default()
                            )
                            .into(),
                        ),
                        gpui::Animation::new(SLIDE_DURATION).with_easing(ease_out_quint()),
                        move |this, delta| this.left(px(from + (to - from) * delta)),
                    )
                    .into_any_element()
            }
            None => pages
                .absolute()
                .top_0()
                .left(px(rest))
                .h_full()
                .into_any_element(),
        };

        // Anchored top AND bottom: an absolutely positioned container without
        // a bottom edge has no height to flex into, and the column inside
        // collapses to its content height (the footer rides up with it).
        div()
            .absolute()
            .left_0()
            .top(px(TAB_STRIP_H))
            .bottom_0()
            .w_full()
            .overflow_hidden()
            .child(row)
            .into_any_element()
    }
}

/// A box's displayed sheet id, if it has one.
fn active_id_of(box_: &TabBarBox) -> Option<String> {
    box_.panels.get(box_.active).cloned()
}

impl CanvasView {
    /// One sheet's content, produced by the entity that owns it: a
    /// kind-owned body (the registry's factory) first, then the host's own
    /// faces (the conversation column, the PTY map, the pi store).
    fn render_sheet_body(
        &mut self,
        sheet: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(view) = self.host.registry().body(sheet, window, cx) {
            return div()
                .size_full()
                .min_h_0()
                .overflow_hidden()
                .child(view)
                .into_any_element();
        }
        match sheet_kind(sheet) {
            "session" => match &self.column {
                Some(column) => column.update(cx, |view, cx| view.render_page(window, cx)),
                None => gpui::div().into_any_element(),
            },
            "terminal" => self
                .terminals
                .get(sheet)
                .map(|view| view.clone().into_any_element())
                .unwrap_or_else(|| gpui::div().into_any_element()),
            "pi-session" => self.pi_view(sheet, window, cx),
            _ => {
                // The start page (and any kind the shell has no live body for
                // yet) renders the container's own hint, never another
                // entity's page.
                let text = if sheet_kind(sheet) == "home" {
                    crate::i18n::t("canvas-home-hint")
                } else {
                    crate::i18n::t("canvas-sheet-unavailable")
                };
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(centered(gpui::div().child(text)))
                    .into_any_element()
            }
        }
    }

    /// The pi sheet's body entity, created on first show and kept for the
    /// sheet's life (a tab switch must not rebuild a running conversation).
    fn pi_view(&mut self, sheet: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(view) = self.pi_views.get(sheet) {
            return view.clone().into_any_element();
        }
        let session_id = pi_session_of_sheet(sheet).to_owned();
        let store = self.pi_store.clone();
        let view = cx.new(|cx| PiSessionView::new(store, session_id, window, cx));
        self.pi_views.insert(sheet.to_owned(), view.clone());
        view.into_any_element()
    }

    /// The v1 split gesture: the container's start page opens in a new group
    /// beside `beside` — a placement drop, like every other move.
    pub(crate) fn split_home(&mut self, beside: &str, cx: &mut Context<Self>) {
        let Some(container) = self.host.active_container().map(str::to_string) else {
            return;
        };
        let home = home_sheet_id(&container);
        let target = DropTarget::Split {
            anchor: beside.to_string(),
            side: SplitEdge::Right,
        };
        if self.host.drop_sheet(&home, target) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// Zooms (or unzooms) one sheet — the track's empty-area double-click and
    /// the context menu's item.
    pub(crate) fn toggle_zoom(&mut self, sheet: &str, cx: &mut Context<Self>) {
        if self.host.toggle_zoom(sheet) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
        }
    }

    /// The tab's right-click menu: zoom toggle, split right, close.
    fn open_tab_menu(
        &mut self,
        sheet: &str,
        anchor: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};

        let zoomed = self.host.zoomed() == Some(sheet);
        let closable = self.host.registry().closable(sheet);
        let this = cx.entity();
        let zoom_sheet = sheet.to_string();
        let zoom_label = if zoomed {
            crate::i18n::t("canvas-tab-unzoom")
        } else {
            crate::i18n::t("canvas-tab-zoom")
        };
        let split_sheet = sheet.to_string();
        let close_sheet_id = sheet.to_string();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let zoom_this = this.clone();
            let split_this = this.clone();
            let close_this = this.clone();
            let mut built = menu
                .item(PopupMenuItem::new(zoom_label).on_click(move |_, _, cx| {
                    zoom_this.update(cx, |this, cx| {
                        this.toggle_zoom(&zoom_sheet, cx);
                    });
                }))
                .item(
                    PopupMenuItem::new(crate::i18n::t("canvas-split-right")).on_click(
                        move |_, _, cx| {
                            split_this.update(cx, |this, cx| {
                                this.split_home(&split_sheet, cx);
                            });
                        },
                    ),
                );
            if closable {
                built = built.item(
                    PopupMenuItem::new(crate::i18n::t("canvas-close-sheet")).on_click(
                        move |_, window, cx| {
                            close_this.update(cx, |this, cx| {
                                this.close_sheet(&close_sheet_id, window, cx);
                            });
                        },
                    ),
                );
            }
            built
        });
        let sub = cx.subscribe(&menu, |this, _menu, _: &gpui::DismissEvent, cx| {
            this.tab_menu = None;
            this._tab_menu_sub = None;
            cx.notify();
        });
        self._tab_menu_sub = Some(sub);
        self.tab_menu_anchor = Some(anchor);
        self.tab_menu = Some((sheet.to_string(), menu));
        cx.notify();
    }

    fn close_sheet(&mut self, sheet: &str, window: &mut Window, cx: &mut Context<Self>) {
        if sheet_kind(sheet) == "pi-session" {
            self.pi_views.remove(sheet);
            let session_id = pi_session_of_sheet(sheet).to_owned();
            self.pi_store
                .update(cx, |store, cx| store.close_session(&session_id, cx));
        }
        self.panes.update(cx, |panes, cx| panes.reclaim(sheet, cx));
        match self.host.close_sheet(sheet) {
            CanvasAction::Applied => {
                self.prune_terminals();
                self.prune_panes(cx);
                self.after_edit(cx);
                cx.notify();
            }
            CanvasAction::Stale => {}
            CanvasAction::AutoArchive(container_id) => {
                // The archive itself is the workspace's write (the canvas never
                // tears down another entity); it lands on the effect boundary.
                if let Some(workspace) = self.workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.canvas_auto_archive(&container_id, window, cx);
                    });
                }
            }
        }
    }
}

/// The chrome-free resize appearance: nothing. The seams are the card gaps
/// themselves; the band is a pure hit strip.
fn invisible_appearance() -> gpui_base::ResizeHandleRenderer {
    std::rc::Rc::new(|_, _, _| Some(gpui::Empty.into_any_element()))
}

/// A center split's invisible drag band, with the double-click equal-share
/// reset.
fn split_band(handle: &SplitHandle, cx: &mut Context<CanvasView>) -> AnyElement {
    let axis = match handle.axis {
        steer_canvas::layout::Axis::Horizontal => gpui::Axis::Horizontal,
        steer_canvas::layout::Axis::Vertical => gpui::Axis::Vertical,
    };
    let joined = handle
        .path
        .iter()
        .map(|step| step.to_string())
        .collect::<Vec<_>>()
        .join("-");
    let name = if joined.is_empty() {
        "root".to_string()
    } else {
        joined
    };
    let id = SharedString::from(format!("canvas-split-band-{name}"));
    div()
        .absolute()
        .left(px(handle.rect.x))
        .top(px(handle.rect.y))
        .w(px(handle.rect.width))
        .h(px(handle.rect.height))
        .debug_selector({
            let id = id.clone();
            move || id.to_string()
        })
        .child(
            gpui_base::resize_handle::<DraggedSplit, DraggedSplit>(id, axis)
                .with_appearance(invisible_appearance())
                .on_drag(
                    DraggedSplit {
                        path: handle.path.clone(),
                    },
                    |payload: &DraggedSplit, _, _, cx| {
                        cx.new(|_| DraggedSplit {
                            path: payload.path.clone(),
                        })
                    },
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_double_click(cx.listener({
                    let path = handle.path.clone();
                    move |this, _: &ClickEvent, _, cx| {
                        if this.host.set_split_sizes(&path, None) == CanvasAction::Applied {
                            this.after_edit(cx);
                            cx.notify();
                        }
                    }
                })),
        )
        .into_any_element()
}

/// A dock edge's invisible drag band: sizes the region (collapsing below the
/// floor, reopening a closed one past it); double-click restores the default.
fn dock_band(handle: &projection::DockHandle, cx: &mut Context<CanvasView>) -> AnyElement {
    let axis = match handle.side {
        DockSide::Left | DockSide::Right => gpui::Axis::Horizontal,
        DockSide::Bottom => gpui::Axis::Vertical,
    };
    let side = handle.side;
    div()
        .absolute()
        .left(px(handle.rect.x))
        .top(px(handle.rect.y))
        .w(px(handle.rect.width))
        .h(px(handle.rect.height))
        .debug_selector({
            let name = side.name();
            move || format!("canvas-dock-band-{name}")
        })
        .child(
            gpui_base::resize_handle::<DraggedDock, DraggedDock>(
                SharedString::from(format!("canvas-dock-band-{}", side.name())),
                axis,
            )
            .with_appearance(invisible_appearance())
            .on_drag(DraggedDock { side }, |payload: &DraggedDock, _, _, cx| {
                cx.new(|_| *payload)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_double_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let opened = this.host.set_dock_open(side, true);
                if opened == CanvasAction::Applied
                    && this
                        .host
                        .set_dock_size(side, steer_canvas::layout::DOCK_DEFAULT_SIZE)
                        == CanvasAction::Applied
                {
                    this.after_edit(cx);
                    cx.notify();
                }
            })),
        )
        .into_any_element()
}

/// The live drop indicator: a 2pt brand line for a tab slot, a brand-tinted
/// block with a 1pt border for everything else (DESIGN §3.7).
fn indicator_layer(preview: &projection::DropPreview, theme: &gpui_component::Theme) -> AnyElement {
    let (rect, slot) = match preview.indicator {
        Indicator::Slot(rect) => (rect, true),
        Indicator::Block(rect) => (rect, false),
    };
    let mut el = div()
        .absolute()
        .left(px(rect.x))
        .top(px(rect.y))
        .w(px(rect.width))
        .h(px(rect.height))
        .debug_selector(|| "canvas-drop-indicator".to_string());
    el = if slot {
        el.rounded(px(1.)).bg(theme.accent)
    } else {
        el.rounded(px(8.))
            .bg(theme.accent.opacity(0.15))
            .border_1()
            .border_color(theme.accent)
    };
    el.into_any_element()
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_canvas(window, cx)
    }
}

/// The v1 sheet registry, wired with the host's title source.
pub(crate) fn registry(
    titles: SheetTitles,
    pi_store: Entity<AcpStore>,
    panes: &Entity<crate::canvas_kinds::CanvasPaneStore>,
) -> steer_canvas::SheetRegistry {
    v1_registry(titles, pi_store, panes)
}

/// The pi session kind: existence, title and closability; the body is painted
/// by the canvas from the store's view (`body: None`, like the conversation
/// column).
fn pi_kind(pi_store: Entity<AcpStore>) -> steer_canvas::SheetKindSpec {
    use std::sync::Arc;

    steer_canvas::SheetKindSpec {
        kind: "pi-session",
        title: Arc::new(move |sheet: &str, cx: &App| {
            let session = pi_session_of_sheet(sheet);
            let store = pi_store.read(cx);
            store
                .title(session)
                .unwrap_or_else(|| crate::i18n::t("canvas-tab-pi"))
        }),
        tab_title: None,
        glyph: "pi",
        closable: true,
        // A pi session is a sheet like any conversation; it does not name the
        // container.
        names_container: false,
        restore: None,
        parse: None,
        body: None,
    }
}

/// The session a `pi-session:` sheet id names.
fn pi_session_of_sheet(sheet: &str) -> &str {
    sheet.strip_prefix("pi-session:").unwrap_or(sheet)
}
