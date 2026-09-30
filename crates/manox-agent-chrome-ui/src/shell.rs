//! The shell root view — the app window's chrome. Owns layout state (sidebar
//! width/visibility, bottom dock, right pane) and the sidebar's *interaction*
//! state (selection, collapse, group order, row menu, session picker); the
//! session rows themselves are host-pushed snapshots ([`Shell::set_sessions`])
//! and every state-changing action is mirrored to the host through
//! [`HostHooks`] — the chrome never touches a data source.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, actions, div,
    px,
};

use crate::main_surface::MainSurfaceHandle;
use crate::panel::{PanelSlot, PanelSurface};
use crate::right_pane::{RightPane, ToolTabFactory};
use crate::session_list::{
    CustomizationRow, FixedRow, SessionGroup, SessionList, SessionRowData, SessionRowKind,
    SessionStatus, SidebarGrouping,
};
use crate::theme::{
    CARD_BG, CARD_BORDER, FG_DIM, FG_STRONG, FLOAT_GAP, PANEL_BG, TABBAR_BG, icon, icons,
};
use crate::{divider, titlebar};

actions!(manox_agent_chrome_ui, [NewSession]);

/// One session as pushed by the host. Grouping, ordering and interaction
/// state are the chrome's; interpreting sessions is not.
#[derive(Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    /// Workspace (project) display name — the grouping key.
    pub workspace: String,
    /// The group's project directory (the host's launch target for the
    /// project menu); `None` on no-project rows — the menu falls back to the
    /// host's cwd rules and hides the remove-project row.
    pub project: Option<String>,
    pub status: SessionStatus,
    /// What the row is (thread vs live external session) — the leading slot
    /// and the row menu ride it.
    pub kind: SessionRowKind,
    /// Last-active unix seconds — the info line's display source.
    pub updated_at: i64,
    /// The re-sort stamp: the row's own `updated_at`, except team members
    /// arrive with their leader's (the projection owns the team structure),
    /// so a team sorts as one unit and stays contiguous through the
    /// pinned-first re-order. Note the store pins per thread: pinning a
    /// member floats that member alone — unit-wide pinning is the store's
    /// concern when teams land.
    pub sort_stamp: i64,
    pub pinned: bool,
    /// See [`SessionRowData::archived`] — always false on today's wire.
    pub archived: bool,
    /// D2 columns: the user tag chip and the team-leader mark (rows do not
    /// indent — hierarchy lives in the group header and the leader chevron).
    pub tag: Option<String>,
    pub team_leader: bool,
}

impl SessionRow {
    /// Lift one projected group (see agent-ui's `sidebar_projection`) into
    /// the shell's row carrier. The projection emits wire order (recency)
    /// already, carries each row's real `updated_at`, and owns the team
    /// structure: members arrive with their leader's `sort_stamp`.
    pub fn from_group(group: crate::session_list::SessionGroup) -> Vec<SessionRow> {
        group
            .rows
            .into_iter()
            .map(|r| SessionRow {
                id: r.id,
                title: r.title,
                workspace: group.name.clone(),
                project: group.project.clone(),
                status: r.status,
                kind: r.kind,
                updated_at: r.updated_at,
                sort_stamp: r.sort_stamp,
                pinned: r.pinned,
                archived: r.archived,
                tag: r.tag,
                team_leader: r.team_leader,
            })
            .collect()
    }

    fn row_data(&self) -> SessionRowData {
        SessionRowData {
            id: self.id.clone(),
            title: self.title.clone(),
            updated_at: self.updated_at,
            sort_stamp: self.sort_stamp,
            status: self.status,
            kind: self.kind,
            pinned: self.pinned,
            archived: self.archived,
            tag: self.tag.clone(),
            team_leader: self.team_leader,
        }
    }
}

/// Host action carrying a row id.
pub type HookOnId = Box<dyn Fn(&str, &mut Window, &mut App)>;
/// Host action with no payload.
pub type HookOnUnit = Box<dyn Fn(&mut Window, &mut App)>;
/// Host tag write (row id, `Some(tag)` to set / `None` to clear).
pub type HookOnSetTag = Box<dyn Fn(&str, Option<String>, &mut Window, &mut App)>;
/// Host-built group menu (state key, the group's project path when it is a
/// workspace group, open position) → the menu entity to mount, or `None` to
/// open nothing. The chrome supplies the surface (anchor + dismissal); the
/// content is host semantics — the project actions are not the chrome's to
/// invent.
pub type HookOnGroupMenu = Box<
    dyn Fn(&str, Option<&str>, gpui::Point<Pixels>, &mut Window, &mut App) -> Option<MenuEntity>,
>;
/// The popup menu entity the host builds and the shell mounts.
pub type MenuEntity = Entity<gpui_component::menu::PopupMenu>;
/// Host nav move: opens the previous/next thread in the host's history and
/// returns the thread id it landed on (`None` when there is nowhere to go).
pub type HookOnNav = Box<dyn Fn(&mut Window, &mut App) -> Option<String>>;
/// Host state query face (render-time read, no mutation).
pub type HookQuery<T> = Box<dyn Fn(&App) -> T>;

/// Which nav moves have an edge to land on right now (named because two
/// anonymous booleans carry their meaning only in positional order).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavAvail {
    pub back: bool,
    pub forward: bool,
}

/// Host-side actions the shell mirrors to. All optional; an absent hook
/// leaves the shell's local behavior only (e.g. `on_pin` flips the local row
/// and the next `set_sessions` snapshot reconciles). One asymmetry: the nav
/// moves REQUIRE their `nav_avail` face — an absent face reads as "no edge
/// in either direction", so `on_nav_back`/`on_nav_forward` without it are
/// inert by design.

#[derive(Default)]
pub struct HostHooks {
    pub on_pin: Option<HookOnId>,
    pub on_archive: Option<HookOnId>,
    /// Thread-tag write-back (the same store seam the sidebar's
    /// `SetThreadTag` uses). `None` clears the tag.
    pub on_set_tag: Option<HookOnSetTag>,
    /// New-session request (the New button / ⌘N). Absent → the shell just
    /// clears the active session.
    pub on_new_session: Option<HookOnUnit>,
    /// Selection-change notification (the full production switch path; the
    /// row menu has no open action — its five items are pin / archive /
    /// tag / clear-tag / copy id).
    pub on_select: Option<HookOnId>,
    /// ←/→ session-history navigation. The hook performs the host-side move
    /// and reports the landed thread id so the shell can move its own
    /// selection without re-deriving host state.
    pub on_nav_back: Option<HookOnNav>,
    pub on_nav_forward: Option<HookOnNav>,
    /// Availability of the two nav moves, queried at render time AND
    /// enforced by [`Shell::nav_back`] / [`Shell::nav_forward`]. ABSENT =
    /// neither move may fire: a host that ships `on_nav_back` without this
    /// face gets permanently dead arrows BY CONTRACT (the shell cannot
    /// invent the history edges it does not own). Provide the face.
    pub nav_avail: Option<HookQuery<NavAvail>>,
    /// Open the foreground session's workspace in the user's editor.
    pub on_open_editor: Option<HookOnUnit>,
    /// Group-menu builder (the project actions: launch agents / terminal /
    /// editor, remove project). Absent → group headers carry no menu surface.
    pub on_group_menu: Option<HookOnGroupMenu>,
    /// Close a live external session (the external row menu's 关闭会话):
    /// the host closes its tab AND reaps the sidebar row. Absent → the menu
    /// item fires nothing (a host that surfaces external rows owes this).
    pub on_close_external: Option<HookOnId>,
}

/// Everything the shell needs from its host at construction.
pub struct ShellConfig {
    pub main: MainSurfaceHandle,
    pub tool_kinds: Vec<Arc<dyn ToolTabFactory>>,
    pub panel_surface: Option<Arc<dyn PanelSurface>>,
    /// Fixed sidebar rows (Automations / Chats …) — labels resolved by the
    /// host.
    pub fixed_rows: Vec<FixedRow>,
    /// Customizations-block rows (Overview / Plugins / MCP / Skills).
    pub customizations: Vec<CustomizationRow>,
    /// The app brand mark for the titlebar's avatar slot (a per-call element
    /// factory — elements rebuild each frame). `None` falls back to the
    /// generic code glyph.
    pub brand: Option<Arc<dyn Fn() -> gpui::AnyElement>>,
    pub hooks: HostHooks,
}

/// Thread-tag character ceiling (the sidebar editor's rule).
const MAX_THREAD_TAG_CHARS: usize = 10;

/// The inline tag editor in flight (one at a time); the subscription commits
/// on Enter/blur and clamps on Change.
struct TagEdit {
    id: String,
    input: Entity<gpui_component::input::InputState>,
    _sub: gpui::Subscription,
}

/// The app-shell root view.
pub struct Shell {
    // ── sessions (host-pushed) ──
    pub sessions: Vec<SessionRow>,
    pub active: Option<String>,
    collapsed: Vec<String>,
    /// Group (project) display order: the persisted result of drag
    /// reordering; unlisted groups trail in default order.
    group_order: Vec<String>,
    /// Live group-drag drop marker (dragged, target, before-edge?); drives
    /// the insertion line, cleared when the drag ends.
    group_drag_marker: Option<(String, String, bool)>,
    /// The hovered row id (marquee + title-weight trigger), mirrored from
    /// the list's hover events.
    hovered_row: Option<String>,
    /// Per-row focus handles, rebuilt against the visible id set on every
    /// sidebar render (keyboard focus ring + up/down row navigation).
    row_focus: HashMap<String, FocusHandle>,
    /// Per-row title clip-box width, written by the rows' `on_prepaint` and
    /// read by the marquee's truncation test (last painted frame's value).
    title_box_w: Rc<RefCell<HashMap<String, Pixels>>>,
    fixed_rows: Vec<FixedRow>,
    customizations: Vec<CustomizationRow>,
    /// The open row menu (id, anchor, the PopupMenu entity).
    row_menu: Option<(
        String,
        gpui::Point<Pixels>,
        Entity<gpui_component::menu::PopupMenu>,
    )>,
    /// Row-menu DismissEvent subscription (the menu closes itself on outside
    /// click; the event drives the host side shut).
    row_menu_sub: Option<gpui::Subscription>,
    /// The open group menu (state key, anchor, the host-built menu entity).
    group_menu: Option<(String, gpui::Point<Pixels>, MenuEntity)>,
    /// Group-menu dismissal subscription (same shape as the row menu's).
    group_menu_sub: Option<gpui::Subscription>,
    /// The inline tag editor (row + input); Escape cancels, Enter/blur
    /// commits, an empty value is silently discarded.
    tag_edit: Option<TagEdit>,
    // ── layout ──
    pub show_sidebar: bool,
    pub show_panel: bool,
    /// The right pane (the shell view; tab content is decoupled through
    /// ToolTab — see `right_pane.rs`).
    pub right: Entity<RightPane>,
    /// Sidebar width (invisible-handle drag, 180–460, double-click reset).
    pub sidebar_width: Pixels,
    /// The app brand mark factory (the titlebar avatar slot's content).
    brand: Option<Arc<dyn Fn() -> gpui::AnyElement>>,
    /// Sidebar grouping mode (the sort button's toggle).
    grouping: SidebarGrouping,
    // ── sidebar filter ──
    /// Whether the filter input row is expanded (the search button's toggle).
    pub sidebar_filter_open: bool,
    /// The filter term (created on open; an InputState needs a Window, so it
    /// is built on the event path).
    filter_query: Option<Entity<gpui_component::input::InputState>>,
    _filter_sub: Option<gpui::Subscription>,
    // ── bottom dock ──
    panel: Option<PanelSlot>,
    // ── titlebar session picker ──
    pub session_picker_open: bool,
    /// Picker anchor (window space, the triggering click position).
    picker_anchor: gpui::Point<Pixels>,
    /// Picker search term (created on open; an InputState needs a Window, so
    /// it is built on the event path).
    pub(crate) picker_query: Option<Entity<gpui_component::input::InputState>>,
    _picker_sub: Option<gpui::Subscription>,
    // ── host faces ──
    pub(crate) main: MainSurfaceHandle,
    hooks: HostHooks,
}

impl Shell {
    pub fn new(config: ShellConfig, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let right = cx.new(|_| RightPane::new(config.tool_kinds));
        let panel = config.panel_surface.map(|surface| PanelSlot {
            surface,
            view: None,
            error: None,
        });
        Self {
            right,
            sessions: Vec::new(),
            active: None,
            collapsed: Vec::new(),
            group_order: Vec::new(),
            group_drag_marker: None,
            hovered_row: None,
            row_focus: HashMap::new(),
            title_box_w: Rc::new(RefCell::new(HashMap::new())),
            fixed_rows: config.fixed_rows,
            customizations: config.customizations,
            row_menu: None,
            row_menu_sub: None,
            group_menu: None,
            group_menu_sub: None,
            tag_edit: None,
            show_sidebar: true,
            show_panel: false,
            sidebar_width: px(divider::SIDEBAR_DEFAULT),
            brand: config.brand,
            grouping: SidebarGrouping::default(),
            sidebar_filter_open: false,
            filter_query: None,
            _filter_sub: None,
            panel,
            session_picker_open: false,
            picker_anchor: gpui::Point::default(),
            picker_query: None,
            _picker_sub: None,
            main: config.main,
            hooks: config.hooks,
        }
    }

    // ── sessions ─────────────────────────────────────────────────────

    /// Replace the session snapshot (a ThreadStore-style push). The active
    /// selection survives by id.
    pub fn set_sessions(&mut self, rows: Vec<SessionRow>) {
        self.sessions = rows;
    }

    pub fn select(&mut self, id: &str, window: &mut Window, cx: &mut App) {
        self.active = Some(id.to_string());
        if let Some(on_select) = &self.hooks.on_select {
            on_select(id, window, cx);
        }
    }

    /// The New button / ⌘N: default behavior is the empty state (no active
    /// session); hosts with a real new-session path hook `on_new_session`.
    pub fn new_session(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(on_new) = &self.hooks.on_new_session {
            on_new(window, cx);
        } else {
            self.active = None;
        }
    }

    /// ←: one step back through the host's session history. The move fires
    /// only when the host reports a live back edge ([`Self::nav_avail`]) —
    /// the same contract the dimmed button paints, so a programmatic move
    /// at a dead edge is as inert as the click.
    pub fn nav_back(&mut self, window: &mut Window, cx: &mut App) {
        if !self.nav_avail(cx).back {
            return;
        }
        let Some(hook) = &self.hooks.on_nav_back else {
            return;
        };
        if let Some(id) = hook(window, cx) {
            self.active = Some(id);
        }
    }

    /// →: one step forward through the host's session history (live-edge
    /// gated, see [`Self::nav_back`]).
    pub fn nav_forward(&mut self, window: &mut Window, cx: &mut App) {
        if !self.nav_avail(cx).forward {
            return;
        }
        let Some(hook) = &self.hooks.on_nav_forward else {
            return;
        };
        if let Some(id) = hook(window, cx) {
            self.active = Some(id);
        }
    }

    /// Whether the two nav moves can fire right now (the host's history
    /// edges; all-false with no host face).
    pub fn nav_avail(&self, cx: &App) -> NavAvail {
        self.hooks
            .nav_avail
            .as_ref()
            .map(|q| q(cx))
            .unwrap_or_default()
    }

    /// The titlebar's editor button: open the foreground session's workspace
    /// in the user's editor. No-op without a host face.
    pub fn open_editor(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(hook) = &self.hooks.on_open_editor {
            hook(window, cx);
        }
    }

    /// The brand-mark element for the titlebar's avatar slot, rebuilt for
    /// this frame (`None` → the titlebar renders its generic fallback).
    pub fn brand_element(&self) -> Option<gpui::AnyElement> {
        self.brand.as_ref().map(|f| f())
    }

    /// The sidebar sort button: workspace grouping ↔ time buckets. Drops a
    /// drag marker left by an abandoned drag (dropped outside every slot,
    /// where `on_drop` never fired) so it cannot resurface on the way back.
    pub fn toggle_grouping(&mut self) {
        self.group_drag_marker = None;
        self.grouping = if self.grouping.is_time() {
            SidebarGrouping::Workspace
        } else {
            SidebarGrouping::Time
        };
    }

    pub fn grouping(&self) -> SidebarGrouping {
        self.grouping
    }

    /// The sidebar search button: expand/collapse the filter row. Expanding
    /// builds the input on this event path (an InputState needs a Window;
    /// the row always starts collapsed, so the input is built exactly once
    /// per expansion), clears the term and focuses it; collapsing drops both.
    pub fn toggle_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_filter_open = !self.sidebar_filter_open;
        if self.sidebar_filter_open {
            let query = cx.new(|cx| {
                gpui_component::input::InputState::new(window, cx)
                    .placeholder(manox_i18n::t("chrome-sidebar-search-placeholder"))
            });
            let sub = cx.subscribe_in(
                &query,
                window,
                |_, _, _: &gpui_component::input::InputEvent, _w, cx| cx.notify(),
            );
            self._filter_sub = Some(sub);
            query.update(cx, |s, cx| {
                s.set_value("", window, cx);
                let handle = s.presentation().focus_handle().clone();
                window.focus(&handle, cx);
            });
            self.filter_query = Some(query);
        } else {
            self._filter_sub = None;
            self.filter_query = None;
        }
        cx.notify();
    }

    /// The filter row's input, if mounted (read face for hosts/tests that
    /// need to drive the term).
    pub fn filter_input(&self) -> Option<Entity<gpui_component::input::InputState>> {
        self.filter_query.clone()
    }

    /// The current filter term (lowercased; `None` while the row is closed
    /// or the term is empty — an empty term means no filtering).
    pub fn sidebar_filter(&self, cx: &App) -> Option<String> {
        if !self.sidebar_filter_open {
            return None;
        }
        let value = self
            .filter_query
            .as_ref()?
            .read(cx)
            .value()
            .trim()
            .to_string();
        (!value.is_empty()).then(|| value.to_lowercase())
    }

    /// Group-drag marker update (drag-move path; the insertion line follows
    /// the pointer).
    pub fn set_group_drag_marker(&mut self, dragged: String, target: String, before: bool) {
        let marker = Some((dragged, target, before));
        if self.group_drag_marker != marker {
            self.group_drag_marker = marker;
        }
    }

    /// Group reorder: insert `dragged` before/after `target`'s edge
    /// (same-name / same-place drops are ignored). Committing clears the
    /// marker. A no-op in time grouping — drag order is a workspace-mode
    /// concept only.
    pub fn move_group(&mut self, dragged: &str, target: &str, before: bool) {
        self.group_drag_marker = None;
        if dragged == target || self.grouping.is_time() {
            return;
        }
        // Materialize the current display order into `group_order` first
        // (unrecorded groups trail in default order), then move, so a first
        // drag never loses the existing relative order.
        let all: Vec<&SessionRow> = self.sessions.iter().collect();
        let current: Vec<String> = self
            .workspace_groups(&all)
            .iter()
            .map(|g| g.name.clone())
            .collect();
        if self.group_order.is_empty() {
            self.group_order = current.clone();
        }
        let mut order: Vec<String> = self
            .group_order
            .iter()
            .cloned()
            .chain(
                current
                    .into_iter()
                    .filter(|n| !self.group_order.contains(n)),
            )
            .collect();
        order.retain(|n| n != dragged);
        let insert_at = order
            .iter()
            .position(|n| n == target)
            .map(|pos| if before { pos } else { pos + 1 })
            .unwrap_or(order.len());
        order.insert(insert_at, dragged.to_string());
        self.group_order = order;
    }

    /// Row menu (right-click on a row): the ONLY action surface. Threads:
    /// pin/unpin, archive/unarchive, tag add/rename/remove, copy id. A live
    /// external session (a launched CLI agent / terminal): 关闭会话 + copy
    /// id — its teardown rides [`HostHooks::on_close_external`], the
    /// legacy sidebar's ArchiveExternalSession semantics. Every action
    /// closes the menu; toggles read the row's current flags so the label
    /// names the action it will perform.
    pub fn open_row_menu(
        &mut self,
        id: &str,
        anchor: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};

        // One menu at a time: an open group menu's anchor would lie under
        // this menu (the mirror of open_group_menu's close_row_menu).
        self.close_group_menu(cx);
        let Some(sess) = self.sessions.iter().find(|s| s.id == id) else {
            return;
        };
        // The external-session branch: 关闭会话 (the hook reaps the row AND
        // closes its tab) + copy id. No thread semantics on a PTY.
        if let crate::session_list::SessionRowKind::External { .. } = sess.kind {
            let this = cx.entity();
            let id_close = id.to_string();
            let id_copy = id.to_string();
            let menu = PopupMenu::build(window, cx, move |menu, _w, _cx| {
                menu.max_w(gpui::px(220.))
                    .item(
                        PopupMenuItem::new(manox_i18n::t("sidebar-close-external"))
                            .icon(crate::theme::icons::CLOSE)
                            .on_click(move |_, window, cx| {
                                this.update(cx, |this, cx| {
                                    if let Some(hook) = &this.hooks.on_close_external {
                                        hook(&id_close, window, cx);
                                    }
                                    this.close_row_menu(cx);
                                });
                            }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new(manox_i18n::t("chrome-row-copy-id"))
                            .icon(crate::theme::icons::COPY)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    id_copy.clone(),
                                ));
                            }),
                    )
            });
            let sub = cx.subscribe(&menu, |this, _menu, _: &gpui::DismissEvent, cx| {
                this.close_row_menu(cx);
            });
            self.row_menu_sub = Some(sub);
            self.row_menu = Some((id.to_string(), anchor, menu));
            cx.notify();
            return;
        }
        let pinned = sess.pinned;
        let archived = sess.archived;
        let has_tag = sess.tag.is_some();
        let this = cx.entity();

        let id_pin = id.to_string();
        let id_pin2 = id_pin.clone();
        let id_archive = id.to_string();
        let id_arch2 = id_archive.clone();
        let id_tag_edit = id.to_string();
        let id_tag_clear = id.to_string();
        let id_copy = id.to_string();
        let this_pin = this.clone();
        let this_archive = this.clone();
        let this_tag = this.clone();
        let this_tag2 = this.clone();

        let menu = PopupMenu::build(window, cx, move |menu, _w, _cx| {
            let menu = menu
                .max_w(gpui::px(220.))
                .item(
                    PopupMenuItem::new(if pinned {
                        manox_i18n::t("chrome-row-unpin")
                    } else {
                        manox_i18n::t("chrome-row-pin")
                    })
                    .icon(crate::theme::icons::PIN)
                    .on_click(move |_, window, cx| {
                        this_pin.update(cx, |this, cx| {
                            if let Some(hook) = &this.hooks.on_pin {
                                hook(&id_pin2, window, cx);
                            }
                            this.toggle_pin(&id_pin2, cx);
                            this.close_row_menu(cx);
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new(if archived {
                        manox_i18n::t("sidebar-unarchive")
                    } else {
                        manox_i18n::t("sidebar-archive")
                    })
                    .icon(if archived {
                        crate::theme::icons::ARCHIVE_RESTORE
                    } else {
                        crate::theme::icons::ARCHIVE
                    })
                    .on_click(move |_, window, cx| {
                        this_archive.update(cx, |this, cx| {
                            if let Some(hook) = &this.hooks.on_archive {
                                hook(&id_arch2, window, cx);
                            }
                            this.set_archived(&id_arch2, !archived, cx);
                            this.close_row_menu(cx);
                        });
                    }),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if has_tag {
                        manox_i18n::t("sidebar-thread-tag-rename")
                    } else {
                        manox_i18n::t("sidebar-thread-tag-add")
                    })
                    .icon(crate::theme::icons::TAG)
                    .on_click(move |_, window, cx| {
                        this_tag.update(cx, |this, cx| {
                            this.close_row_menu(cx);
                            this.begin_tag_edit(id_tag_edit.clone(), has_tag, window, cx);
                        });
                    }),
                );
            // 移除标签 only exists while a tag does.
            let menu = if has_tag {
                menu.item(
                    PopupMenuItem::new(manox_i18n::t("sidebar-thread-tag-clear"))
                        .icon(crate::theme::icons::TRASH)
                        .on_click(move |_, window, cx| {
                            this_tag2.update(cx, |this, cx| {
                                this.set_tag(&id_tag_clear, None, window, cx);
                                this.close_row_menu(cx);
                            });
                        }),
                )
            } else {
                menu
            };
            menu.separator().item(
                PopupMenuItem::new(manox_i18n::t("chrome-row-copy-id"))
                    .icon(crate::theme::icons::COPY)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(id_copy.clone()));
                    }),
            )
        });
        // DismissEvent → the host closes (the menu handles outside clicks
        // itself).
        let sub = cx.subscribe(&menu, |this, _menu, _: &gpui::DismissEvent, cx| {
            this.close_row_menu(cx);
        });
        self.row_menu_sub = Some(sub);
        self.row_menu = Some((id.to_string(), anchor, menu));
        cx.notify();
    }

    pub fn close_row_menu(&mut self, cx: &mut Context<Self>) {
        self.row_menu_sub = None;
        if self.row_menu.take().is_some() {
            cx.notify();
        }
    }

    /// Whether the row menu is open (read face for hosts/tests).
    pub fn row_menu_open(&self) -> bool {
        self.row_menu.is_some()
    }

    /// Group menu (the header's ellipsis button / right-click): the HOST
    /// builds the menu through [`HostHooks::on_group_menu`] — the project
    /// actions are host semantics — and the shell mounts it anchored at the
    /// trigger, handling dismissal. One group menu at a time; opening one
    /// replaces the previous (and the row menu, whose anchor would now lie).
    pub fn open_group_menu(
        &mut self,
        key: &str,
        project: Option<&str>,
        anchor: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hook) = &self.hooks.on_group_menu else {
            return;
        };
        let Some(menu) = hook(key, project, anchor, window, cx) else {
            return;
        };
        self.close_row_menu(cx);
        let sub = cx.subscribe(&menu, |this, _menu, _: &gpui::DismissEvent, cx| {
            this.close_group_menu(cx);
        });
        self.group_menu_sub = Some(sub);
        self.group_menu = Some((key.to_string(), anchor, menu));
        cx.notify();
    }

    pub fn close_group_menu(&mut self, cx: &mut Context<Self>) {
        self.group_menu_sub = None;
        if self.group_menu.take().is_some() {
            cx.notify();
        }
    }

    /// Whether the group menu is open (read face for hosts/tests).
    pub fn group_menu_open(&self) -> bool {
        self.group_menu.is_some()
    }

    /// The hovered row id, if any (read face for hosts/tests).
    pub fn hovered_row(&self) -> Option<&str> {
        self.hovered_row.as_deref()
    }

    /// The in-flight inline tag editor, if any — the row id and its input
    /// (read face for hosts/tests that need to drive the value).
    pub fn tag_edit_input(&self) -> Option<(String, Entity<gpui_component::input::InputState>)> {
        self.tag_edit
            .as_ref()
            .map(|e| (e.id.clone(), e.input.clone()))
    }

    pub fn toggle_group(&mut self, key: &str) {
        if let Some(i) = self.collapsed.iter().position(|k| k == key) {
            self.collapsed.remove(i);
        } else {
            self.collapsed.push(key.to_string());
        }
    }

    /// Pin/unpin: the hook performs the real store write; the local row
    /// flips immediately (the next snapshot reconciles).
    pub fn toggle_pin(&mut self, id: &str, cx: &mut App) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.pinned = !s.pinned;
        }
        self.sort_sessions();
        let _ = cx;
    }

    /// Archive/unarchive local reconciliation (the hook performs the store
    /// write): archiving drops the row (and clears the selection if it was
    /// active); unarchiving flips the row's partition flag so the next
    /// snapshot only confirms it.
    pub fn set_archived(&mut self, id: &str, archived: bool, cx: &mut App) {
        if archived {
            self.sessions.retain(|s| s.id != id);
            if self.active.as_deref() == Some(id) {
                self.active = None;
            }
        } else if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.archived = false;
        }
        let _ = cx;
    }

    /// Thread-tag write-back: the hook (the store seam) then the local row,
    /// so the chip updates without waiting for the next snapshot.
    pub fn set_tag(&mut self, id: &str, tag: Option<String>, window: &mut Window, cx: &mut App) {
        if let Some(hook) = &self.hooks.on_set_tag {
            hook(id, tag.clone(), window, cx);
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.tag = tag;
        }
    }

    /// The inline tag editor on a row's tag line. Rename mode prefills the
    /// current tag; the input is focused immediately and clamped to
    /// [`MAX_THREAD_TAG_CHARS`] on every edit.
    pub fn begin_tag_edit(
        &mut self,
        id: String,
        rename: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefill = rename.then(|| {
            self.sessions
                .iter()
                .find(|s| s.id == id)
                .and_then(|s| s.tag.clone())
        });
        let placeholder = manox_i18n::t("sidebar-thread-tag-placeholder");
        let input = cx.new(|cx| {
            let mut state =
                gpui_component::input::InputState::new(window, cx).placeholder(placeholder);
            if let Some(Some(value)) = prefill {
                state.set_value(value, window, cx);
            }
            state
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &gpui_component::input::InputEvent, window, cx| match event {
                // The pinned gpui-component input has no max-length support;
                // clamp every edit down to the tag ceiling.
                gpui_component::input::InputEvent::Change => {
                    input.update(cx, |state, cx| {
                        let value = state.value();
                        if value.chars().count() > MAX_THREAD_TAG_CHARS {
                            let truncated: String =
                                value.chars().take(MAX_THREAD_TAG_CHARS).collect();
                            state.set_value(truncated, window, cx);
                        }
                    });
                }
                gpui_component::input::InputEvent::PressEnter { .. }
                | gpui_component::input::InputEvent::Blur => {
                    this.commit_tag_edit(window, cx);
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

    /// Commit the in-flight tag edit: a non-empty value rides
    /// [`Self::set_tag`] (hook + local row), an empty one is discarded
    /// silently. Either way the editor unmounts. Idempotent — a blur may
    /// race in after an Enter commit.
    pub fn commit_tag_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.tag_edit.take() else {
            return;
        };
        let value = edit.input.read(cx).value().trim().to_string();
        if !value.is_empty() {
            self.set_tag(&edit.id, Some(value), window, cx);
        }
        cx.notify();
    }

    /// Escape in the editor: unmount without writing.
    pub fn cancel_tag_edit(&mut self, cx: &mut Context<Self>) {
        if self.tag_edit.take().is_some() {
            cx.notify();
        }
    }

    /// The recorded drag order (read face for hosts/tests): a test can
    /// assert the RECORDING was not polluted — the display order alone
    /// cannot show it (time grouping ignores `group_order`, and a
    /// workspace relist only surfaces recorded names that match real
    /// groups).
    pub fn group_order(&self) -> &[String] {
        &self.group_order
    }

    /// Pinned first, then by the team-unit sort stamp descending (see
    /// `SessionRow::sort_stamp`).
    fn sort_sessions(&mut self) {
        self.sessions.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.sort_stamp.cmp(&a.sort_stamp))
        });
    }

    /// Sidebar props (fixed rows + groups + Customizations). Groups follow
    /// the sidebar's grouping mode; the filter term (the expanded search
    /// row's input) narrows the rows and force-expands every surviving group.
    pub fn sidebar_props(
        &self,
        cx: &App,
    ) -> (Vec<FixedRow>, Vec<SessionGroup>, Vec<CustomizationRow>) {
        let filter = self.sidebar_filter(cx);
        let visible: Vec<&SessionRow> = self
            .sessions
            .iter()
            .filter(|s| match &filter {
                None => true,
                Some(f) => {
                    s.title.to_lowercase().contains(f)
                        || s.workspace.to_lowercase().contains(f)
                        || s.tag
                            .as_deref()
                            .is_some_and(|t| t.to_lowercase().contains(f))
                }
            })
            .collect();
        let mut groups: Vec<SessionGroup> = if self.grouping.is_time() {
            self.time_groups(&visible, today())
        } else {
            self.workspace_groups(&visible)
        };
        // While filtering, a group that matched hides nothing: the term is
        // the visible structure, not the collapse toggles.
        if filter.is_some() {
            for g in &mut groups {
                g.collapsed = false;
            }
        }
        (self.fixed_rows.clone(), groups, self.customizations.clone())
    }

    /// Workspace grouping (the default): one group per project display name,
    /// ordered by the recorded drag order, unrecorded groups trailing. The
    /// group's project path rides the rows (the host stamps every row of a
    /// group with it).
    fn workspace_groups(&self, sessions: &[&SessionRow]) -> Vec<SessionGroup> {
        let mut groups: Vec<SessionGroup> = Vec::new();
        for s in sessions {
            let collapsed = self.collapsed.iter().any(|k| k == &s.workspace);
            if let Some(g) = groups.iter_mut().find(|g| g.name == s.workspace) {
                g.rows.push(s.row_data());
                g.collapsed = collapsed;
            } else {
                groups.push(SessionGroup {
                    name: s.workspace.clone(),
                    key: s.workspace.clone(),
                    collapsed,
                    project: s.project.clone(),
                    rows: vec![s.row_data()],
                });
            }
        }
        // Drag order: recorded groups first in their recorded order,
        // unrecorded groups trail in default relative order.
        let seq: std::collections::HashMap<&str, usize> = self
            .group_order
            .iter()
            .enumerate()
            .map(|(i, n)| (n.as_str(), i))
            .collect();
        groups.sort_by(|a, b| {
            let ka = seq.get(a.name.as_str());
            let kb = seq.get(b.name.as_str());
            match (ka, kb) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        groups
    }

    /// Time grouping: four recency buckets by local natural day (today /
    /// yesterday / last 7 days / earlier). Rows bucket AND order by their
    /// `sort_stamp` — the projection stamps team members with their leader's
    /// stamp, so a team lands in ONE bucket and the bucket sort keeps it
    /// contiguous (per-row keys would split it / reorder it under the
    /// chevron; a member pinned individually can still float up — pin is
    /// per-row, pre-existing). Unknown stamps (0) fall into "earlier".
    /// Empty buckets are not rendered.
    fn time_groups(&self, sessions: &[&SessionRow], today: chrono::NaiveDate) -> Vec<SessionGroup> {
        const BUCKETS: [TimeBucket; 4] = [
            TimeBucket::Today,
            TimeBucket::Yesterday,
            TimeBucket::Week,
            TimeBucket::Earlier,
        ];
        let mut groups: Vec<SessionGroup> = BUCKETS
            .iter()
            .map(|bucket| {
                let key = bucket.state_key();
                SessionGroup {
                    name: manox_i18n::t(key).to_string(),
                    key: key.to_string(),
                    collapsed: self.collapsed.iter().any(|k| k == key),
                    project: None,
                    rows: Vec::new(),
                }
            })
            .collect();
        for s in sessions {
            let bucket = time_bucket(s.sort_stamp, today);
            groups[bucket as usize].rows.push(s.row_data());
        }
        for g in &mut groups {
            // sort_stamp is the ONLY time key: team members share their
            // leader's, so equal stamps keep the wire order (stable sort)
            // and the team stays contiguous under its chevron.
            g.rows.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then(b.sort_stamp.cmp(&a.sort_stamp))
            });
        }
        groups.retain(|g| !g.rows.is_empty());
        groups
    }

    /// Session-picker open/close: records the anchor (the triggering click
    /// position), builds/clears the search term on open and focuses it;
    /// closing only collapses.
    pub fn toggle_session_picker(
        &mut self,
        anchor: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_picker_open = !self.session_picker_open;
        if !self.session_picker_open {
            return;
        }
        self.picker_anchor = anchor;
        if self.picker_query.is_none() {
            let query = cx.new(|cx| {
                gpui_component::input::InputState::new(window, cx)
                    .placeholder(manox_i18n::t("chrome-picker-search"))
            });
            let sub = cx.subscribe_in(
                &query,
                window,
                |_, _, _: &gpui_component::input::InputEvent, _w, cx| cx.notify(),
            );
            self._picker_sub = Some(sub);
            self.picker_query = Some(query);
        }
        if let Some(query) = self.picker_query.clone() {
            query.update(cx, |s, cx| {
                s.set_value("", window, cx);
                let handle = s.presentation().focus_handle().clone();
                window.focus(&handle, cx);
            });
        }
    }

    // ── layout ───────────────────────────────────────────────────────

    pub fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = px(width.clamp(divider::SIDEBAR_MIN, divider::SIDEBAR_MAX));
    }

    /// Detach the live panel view WITHOUT teardown — the host takes
    /// ownership (per-thread dock stashing: the view keeps running and is
    /// re-installed on switch-back). `None` when the slot is empty/errored.
    pub fn take_panel_view(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyView> {
        let slot = self.panel.as_mut()?;
        let view = slot.view.take();
        if view.is_some() {
            cx.notify();
        }
        view
    }

    /// Install a view into the panel slot (the per-thread restore half of
    /// [`Self::take_panel_view`]); clears any error state. Does not change
    /// visibility — a collapsed dock stays collapsed.
    pub fn set_panel_view(&mut self, view: gpui::AnyView, cx: &mut Context<Self>) {
        if let Some(slot) = self.panel.as_mut() {
            slot.error = None;
            slot.view = Some(view);
            cx.notify();
        }
    }

    /// Detach the right pane's whole session (per-thread stashing — see
    /// [`crate::right_pane::RightPaneSession`]).
    pub fn stash_right_session(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<crate::right_pane::RightPaneSession> {
        Some(self.right.update(cx, |pane, cx| pane.stash_session(cx)))
    }

    /// Resume a stashed right-pane session.
    pub fn restore_right_session(
        &mut self,
        session: crate::right_pane::RightPaneSession,
        cx: &mut Context<Self>,
    ) {
        self.right
            .update(cx, |pane, cx| pane.restore_session(session, cx));
        cx.notify();
    }

    /// Open (or focus) a right-pane tool kind.
    pub fn open_right(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.right
            .update(cx, |pane, cx| pane.open_kind(kind, window, cx));
        cx.notify();
    }

    /// Titlebar panel toggle: expand runs the surface's `open` (mount equals
    /// launch), collapse runs `close` (drop → teardown, e.g. a PTY's process
    /// tree). Spawning happens on the event path.
    pub fn toggle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_panel = !self.show_panel;
        if self.show_panel {
            self.open_panel_content(window, cx);
        } else {
            self.clear_panel_content(cx);
        }
    }

    fn open_panel_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(slot) = &mut self.panel {
            slot.error = None;
            if slot.view.is_some() {
                return;
            }
            let surface = slot.surface.clone();
            match surface.open(window, cx) {
                Ok(view) => slot.view = Some(view),
                Err(e) => slot.error = Some(e),
            }
        }
    }

    /// Teardown the panel content but keep the dock expanded (the trash
    /// glyph): the next expand or recycle rebuilds it.
    fn clear_panel_content(&mut self, cx: &mut Context<Self>) {
        if let Some(slot) = &mut self.panel {
            if let Some(view) = slot.view.take() {
                let surface = slot.surface.clone();
                surface.close(view, cx);
            }
            slot.error = None;
        }
        cx.notify();
    }

    /// Fresh content (the + glyph): teardown then rebuild — for a terminal
    /// surface this is "kill and spawn a new one".
    fn recycle_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel.is_some() {
            self.clear_panel_content(cx);
            self.open_panel_content(window, cx);
            cx.notify();
        }
    }
}

impl gpui::Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picker_open = self.session_picker_open;
        // Row-menu anchor snapshot (mounted on the deferred layer in window
        // space).
        let row_menu = self
            .row_menu
            .as_ref()
            .map(|(id, anchor, menu)| (id.clone(), *anchor, menu.clone()));
        // Group-menu anchor snapshot (same deferred layer, one menu at a
        // time).
        let group_menu = self
            .group_menu
            .as_ref()
            .map(|(key, anchor, menu)| (key.clone(), *anchor, menu.clone()));
        let titlebar = titlebar::render(self, window, cx);
        let content = self.render_content(cx);
        let close_picker = cx.listener(|this, _: &ClickEvent, _w, cx| {
            if this.session_picker_open {
                this.session_picker_open = false;
                cx.notify();
            }
        });
        div()
            .id("shell")
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(crate::theme::SHELL_BG)
            .text_color(crate::theme::FG)
            .text_size(px(13.))
            .font_family(crate::theme::FONT_UI)
            // The freya/cosmic-text natural line box (SF Pro ≈1.19em, CJK
            // larger) ≈ 1.28em; gpui's default phi(1.618) systematically
            // inflates row cards and group headers.
            .line_height(gpui::relative(1.28))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                this.new_session(window, cx);
                cx.notify();
            }))
            // Panel resizing: drag-move hangs on the root (bounds = the
            // whole window); the handle only initiates the drag — mounted on
            // the handle itself, ev.bounds would be the 6px strip and the
            // width math collapses.
            .on_drag_move(cx.listener(
                |this, ev: &gpui::DragMoveEvent<divider::DraggedSidebarDivider>, _w, cx| {
                    // The sidebar hugs the window's left edge: width =
                    // pointer x − root left edge.
                    let w: f32 = f32::from(ev.event.position.x - ev.bounds.left());
                    this.set_sidebar_width(w.clamp(divider::SIDEBAR_MIN, divider::SIDEBAR_MAX));
                    cx.notify();
                },
            ))
            .on_drag_move(cx.listener(
                |this, ev: &gpui::DragMoveEvent<divider::DraggedRightDivider>, _w, cx| {
                    // The right pane's right edge = root right edge − the
                    // main column's FLOAT_GAP right pad: width = right edge −
                    // pointer.
                    let w: f32 = f32::from(ev.bounds.right() - ev.event.position.x) - FLOAT_GAP;
                    this.right.update(cx, |pane, _cx| {
                        pane.set_width(w.clamp(divider::RIGHT_MIN, divider::RIGHT_MAX));
                    });
                    cx.notify();
                },
            ))
            // Group-drag fallback: capture-phase dispatch runs the parent
            // before the child — the root clears the marker first and a hit
            // group header re-sets it; a pointer outside every group clears
            // the line.
            .on_drag_move::<crate::session_list::DraggedGroup>(cx.listener(
                |this, _: &gpui::DragMoveEvent<crate::session_list::DraggedGroup>, _w, cx| {
                    if this.group_drag_marker.take().is_some() {
                        cx.notify();
                    }
                },
            ))
            .on_drop::<crate::session_list::DraggedGroup>(cx.listener(
                |this, _: &crate::session_list::DraggedGroup, _w, cx| {
                    if this.group_drag_marker.take().is_some() {
                        cx.notify();
                    }
                },
            ))
            .child(titlebar)
            .child(content)
            // Picker open: a full-window transparent capture layer; any
            // click collapses it (mounted after content so it covers it;
            // the picker panel mounts later still and stays clickable).
            .when(picker_open, |this| {
                this.child(
                    div()
                        .id("picker-click-catcher")
                        .on_click(move |e, w, cx| close_picker(e, w, cx))
                        .absolute()
                        .inset_0(),
                )
            })
            // The picker panel rides the root paint layer through
            // deferred(anchored) — a plain child would be covered by the
            // content sibling layer and stay click-through; occlude() takes
            // over hit testing.
            .when(picker_open, |this| {
                let anchor = self.picker_anchor;
                this.child(gpui::deferred(
                    gpui::anchored()
                        .anchor(gpui::Anchor::TopLeft)
                        .position(anchor)
                        .offset(gpui::point(px(0.), px(30.)))
                        .child(titlebar::picker_panel(self, cx)),
                ))
            })
            // Row menu (kebab / right-click): an anchored floating layer;
            // PopupMenu handles its own focus and outside-click dismissal.
            .when_some(row_menu, |this, (id, anchor, menu)| {
                this.child(
                    gpui::deferred(
                        gpui::anchored()
                            .anchor(gpui::Anchor::TopRight)
                            .position(anchor)
                            .offset(gpui::point(px(0.), px(2.)))
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("row-menu-{id}")))
                                    .debug_selector(|| "chrome-row-menu".into())
                                    .occlude()
                                    .child(menu),
                            ),
                    )
                    .with_priority(1),
                )
            })
            // Group menu (the project actions): the same anchored floating
            // layer, fed by the host-built menu entity.
            .when_some(group_menu, |this, (key, anchor, menu)| {
                this.child(
                    gpui::deferred(
                        gpui::anchored()
                            .anchor(gpui::Anchor::TopRight)
                            .position(anchor)
                            .offset(gpui::point(px(0.), px(2.)))
                            .child(
                                div()
                                    .id(gpui::SharedString::from(format!("group-menu-{key}")))
                                    .debug_selector(|| "chrome-group-menu".into())
                                    .occlude()
                                    .child(menu),
                            ),
                    )
                    .with_priority(1),
                )
            })
    }
}

impl Shell {
    /// Content assembly — [sidebar | main area [[main card | right pane] /
    /// bottom dock]]. Card gaps are 6px (the seam); resize handles are
    /// invisible absolute layers centered on the seams (see `divider.rs`),
    /// taking no layout width.
    fn render_content(&mut self, cx: &mut Context<Shell>) -> gpui::AnyElement {
        let show_sidebar = self.show_sidebar;
        let show_panel = self.show_panel;
        let right = self.right.clone();
        let right_visible = right.read(cx).visible;
        let main = self.main.view();

        // The handle follows the width, so the content row must be relative;
        // mounting the handle last keeps its hit priority.
        let sidebar_handle =
            show_sidebar.then(|| divider::sidebar_handle(self, cx).into_any_element());

        div()
            .relative()
            .flex()
            .w_full()
            .flex_1()
            .min_h_0()
            .gap(px(PANE_GAP))
            .when(show_sidebar, |this| this.child(self.render_sidebar(cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pt_0()
                    .pr(px(FLOAT_GAP))
                    .pb(px(FLOAT_GAP))
                    // The sidebar hugs the window's left edge (it carries
                    // its own inset); once collapsed, the main column pads
                    // its left edge to keep the window margins symmetric
                    // with the right/bottom.
                    .pl(px(if show_sidebar { 0. } else { FLOAT_GAP }))
                    .gap(px(FLOAT_GAP))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            // The main card hosts the injected main surface.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .bg(CARD_BG)
                                    .border_1()
                                    .border_color(CARD_BORDER)
                                    .rounded(px(crate::theme::CARD_RADIUS))
                                    .overflow_hidden()
                                    .child(main),
                            )
                            // main card | right pane: the 6px sash is the
                            // seam (an in-layout element).
                            .when(right_visible, |this| {
                                this.child(divider::chat_right_sash(cx)).child(right)
                            }),
                    )
                    .when(show_panel, |this| this.child(self.render_panel(cx))),
            )
            .children(sidebar_handle)
            .into_any_element()
    }

    /// The sidebar view: shell session state → SessionList props.
    fn render_sidebar(&mut self, cx: &mut Context<Shell>) -> impl IntoElement {
        let (fixed, groups, customizations) = self.sidebar_props(cx);
        let selected = self.active.clone();
        let hovered = self.hovered_row.clone();
        let has_chats = !self.sessions.is_empty();
        let marker = self.group_drag_marker.clone();
        let grouping = self.grouping;
        let filter_open = self.sidebar_filter_open;
        let filter_active = self.sidebar_filter(cx).is_some();
        let filter_input = self.filter_query.clone();
        let tag_edit = self
            .tag_edit
            .as_ref()
            .map(|e| (e.id.clone(), e.input.clone()));
        let title_box_w = Rc::clone(&self.title_box_w);

        // Focus handles track the VISIBLE row set: prune the stale ids, then
        // create on demand so every painted row is focusable and the
        // up/down walk has stable handles across frames.
        let visible: std::collections::HashSet<String> = groups
            .iter()
            .flat_map(|g| (!g.collapsed).then(|| g.rows.iter().map(|r| r.id.clone())))
            .flatten()
            .collect();
        let mut focus = std::mem::take(&mut self.row_focus);
        focus.retain(|k, _| visible.contains(k));
        for id in &visible {
            focus.entry(id.clone()).or_insert_with(|| cx.focus_handle());
        }
        self.row_focus = focus;
        let row_focus = Rc::new(self.row_focus.clone());
        // Title clip widths of rows that left the list stop accumulating.
        self.title_box_w
            .borrow_mut()
            .retain(|k, _| visible.contains(k));

        let on_select = cx.listener(|this, id: &String, w, cx| {
            let id = id.clone();
            this.select(&id, w, cx);
            cx.notify();
        });
        let on_toggle_group = cx.listener(|this, name: &String, _w, cx| {
            this.toggle_group(name);
            cx.notify();
        });
        let on_new = cx.listener(|this, _: &ClickEvent, w, cx| {
            this.new_session(w, cx);
            cx.notify();
        });
        let on_hover_row = cx.listener(|this, (id, entered): &(String, bool), _w, cx| {
            // A leave retracts only its OWN row: crossing directly from one
            // row into the next delivers the new row's enter and the old
            // row's leave within one mouse-move dispatch (either order), and
            // a stale leave must not strand the hover on None — the marquee
            // would never start until the pointer moved again.
            let next = if *entered {
                Some(id.clone())
            } else if this.hovered_row.as_deref() == Some(id.as_str()) {
                None
            } else {
                return;
            };
            if this.hovered_row != next {
                this.hovered_row = next;
                cx.notify();
            }
        });
        let on_row_menu = cx.listener(|this, (id, pos): &(String, gpui::Point<Pixels>), w, cx| {
            this.open_row_menu(id, *pos, w, cx);
        });
        let on_group_menu = cx.listener(
            |this, (key, project, pos): &(String, Option<String>, gpui::Point<Pixels>), w, cx| {
                this.open_group_menu(key, project.as_deref(), *pos, w, cx);
            },
        );
        let on_tag_rename = cx.listener(|this, id: &String, w, cx| {
            let id = id.clone();
            this.begin_tag_edit(id, true, w, cx);
        });
        let this = cx.entity();
        let on_tag_edit_cancel: crate::session_list::OnWindowApp = Rc::new(move |_w, cx| {
            this.update(cx, |this, cx| this.cancel_tag_edit(cx));
        });
        let on_move_group = cx.listener(
            |this, (dragged, target, before): &(String, String, bool), _w, cx| {
                this.move_group(dragged, target, *before);
                cx.notify();
            },
        );
        let on_drag_move_group = cx.listener(
            |this, (dragged, target, before): &(String, String, bool), _w, cx| {
                this.set_group_drag_marker(dragged.clone(), target.clone(), *before);
                cx.notify();
            },
        );
        let on_toggle_grouping = cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.toggle_grouping();
            cx.notify();
        });
        let on_toggle_search = cx.listener(|this, _: &ClickEvent, window, cx| {
            this.toggle_sidebar_search(window, cx);
        });
        let on_filter_clear: crate::session_list::OnWindowApp = {
            let this = cx.entity();
            Rc::new(move |window, cx| {
                this.update(cx, |this, cx| this.toggle_sidebar_search(window, cx));
            })
        };

        let width = self.sidebar_width;

        div()
            .w(width)
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .child(SessionList {
                fixed_rows: fixed,
                groups,
                customizations,
                selected,
                no_chats_hint: !has_chats,
                hovered,
                tag_edit,
                title_box_w,
                row_focus,
                grouping,
                filter_open,
                filter_active,
                filter_input,
                on_select: std::rc::Rc::new(move |id, w, cx| on_select(id, w, cx)),
                on_toggle_group: std::rc::Rc::new(move |name, w, cx| on_toggle_group(name, w, cx)),
                on_new: std::rc::Rc::new(move |e, w, cx| on_new(e, w, cx)),
                on_hover_row: std::rc::Rc::new(move |id, entered, w, cx| {
                    on_hover_row(&(id.clone(), entered), w, cx)
                }),
                on_row_menu: std::rc::Rc::new(move |id, pos, w, cx| {
                    on_row_menu(&(id.clone(), pos), w, cx)
                }),
                on_group_menu: if grouping.is_time() {
                    None
                } else {
                    let cb: crate::session_list::OnGroupMenu = std::rc::Rc::new(
                        move |key: &str,
                              project: Option<&str>,
                              pos: gpui::Point<gpui::Pixels>,
                              w: &mut gpui::Window,
                              cx: &mut gpui::App| {
                            on_group_menu(
                                &(key.to_string(), project.map(str::to_string), pos),
                                w,
                                cx,
                            )
                        },
                    );
                    Some(cb)
                },
                on_tag_edit_cancel: Some(on_tag_edit_cancel),
                on_tag_rename: std::rc::Rc::new(move |id, w, cx| on_tag_rename(id, w, cx)),
                on_toggle_grouping: std::rc::Rc::new(move |e, w, cx| on_toggle_grouping(e, w, cx)),
                on_toggle_search: std::rc::Rc::new(move |e, w, cx| on_toggle_search(e, w, cx)),
                on_filter_clear: Some(on_filter_clear),
                on_move_group: std::rc::Rc::new(move |dragged, target, before, w, cx| {
                    on_move_group(&(dragged.clone(), target.clone(), before), w, cx)
                }),
                on_drag_move_group: std::rc::Rc::new(move |dragged, target, before, w, cx| {
                    on_drag_move_group(&(dragged.clone(), target.clone(), before), w, cx)
                }),
                group_drag_marker: marker,
            })
    }

    /// The bottom dock: the shell card + tab strip; the body hosts the
    /// surface's view, its open failure, or the cleared-state hint.
    fn render_panel(&mut self, cx: &mut Context<Shell>) -> impl IntoElement {
        const HEIGHT: f32 = 220.0;

        let Some(slot) = &self.panel else {
            return div().into_any_element();
        };
        let surface = slot.surface.clone();

        let body: gpui::AnyElement = match (&slot.view, slot.error.as_ref()) {
            (Some(view), _) => div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .py(px(4.))
                .px(px(8.))
                .child(view.clone())
                .into_any_element(),
            (None, Some(err)) => div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .text_color(FG_DIM)
                .child(err.clone())
                .into_any_element(),
            (None, None) => div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.5))
                .text_color(FG_DIM)
                .child(manox_i18n::t("chrome-panel-empty"))
                .into_any_element(),
        };

        let on_new_content = cx.listener(|this, _: &ClickEvent, w, cx| {
            this.recycle_panel(w, cx);
        });
        let on_clear = cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.clear_panel_content(cx);
        });
        let on_collapse = cx.listener(|this, _: &ClickEvent, w, cx| {
            this.toggle_panel(w, cx);
            cx.notify();
        });

        div()
            .w_full()
            .h(px(HEIGHT))
            .flex_shrink_0()
            .bg(PANEL_BG)
            .border_1()
            .border_color(CARD_BORDER)
            .rounded(px(crate::theme::CARD_RADIUS))
            .overflow_hidden()
            .text_color(FG_STRONG)
            .flex()
            .flex_col()
            // Tab strip: the surface label + new/clear/collapse.
            .child(
                div()
                    .w_full()
                    .h(px(32.))
                    .flex_shrink_0()
                    .bg(TABBAR_BG)
                    .items_center()
                    .px(px(10.))
                    .gap(px(10.))
                    .text_size(px(12.5))
                    .flex()
                    .child(
                        div()
                            .gap(px(6.))
                            .items_center()
                            .text_color(FG_STRONG)
                            .flex()
                            .child(icon(surface.icon(), 14.))
                            .child(surface.title()),
                    )
                    .child(div().flex_1().min_w_0())
                    .child(
                        div()
                            .id("panel-new")
                            .on_click(move |e, w, cx| on_new_content(e, w, cx))
                            .child(icon(icons::ADD, 14.)),
                    )
                    .child(
                        div()
                            .id("panel-clear")
                            .on_click(move |e, w, cx| on_clear(e, w, cx))
                            .child(icon(icons::TRASH, 14.)),
                    )
                    .child(
                        div()
                            .id("panel-collapse")
                            .on_click(move |e, w, cx| on_collapse(e, w, cx))
                            .child(icon(icons::CHEVRON_DOWN, 14.)),
                    ),
            )
            .child(body)
            .into_any_element()
    }
}

/// sidebar|main seam width (the left handle is absolutely centered on it).
const PANE_GAP: f32 = 6.;

/// The four recency buckets, in display order. `state_key` supplies BOTH
/// the collapse-state key and the ftl label key (one string, so they cannot
/// drift); the explicit discriminant is the index into the render-order
/// array — reordering `BUCKETS` without renumbering these is the one
/// mismatch the compiler cannot catch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimeBucket {
    Today = 0,
    Yesterday = 1,
    Week = 2,
    Earlier = 3,
}

impl TimeBucket {
    /// The collapse-state key AND the ftl label key — one string serving
    /// both, so a bucket's state and its label cannot drift apart.
    fn state_key(self) -> &'static str {
        match self {
            TimeBucket::Today => "chrome-group-today",
            TimeBucket::Yesterday => "chrome-group-yesterday",
            TimeBucket::Week => "chrome-group-week",
            TimeBucket::Earlier => "chrome-group-earlier",
        }
    }
}

/// The time-grouping bucket for a stamp, by local natural day: unknown
/// stamps (≤ 0 unix secs) and undecodable ones land in "earlier".
fn time_bucket(sort_stamp: i64, today: chrono::NaiveDate) -> TimeBucket {
    use chrono::TimeZone as _;
    if sort_stamp <= 0 {
        return TimeBucket::Earlier;
    }
    let Some(t) = chrono::Local.timestamp_opt(sort_stamp, 0).single() else {
        return TimeBucket::Earlier;
    };
    match today.signed_duration_since(t.date_naive()).num_days() {
        d if d <= 0 => TimeBucket::Today,
        1 => TimeBucket::Yesterday,
        d if d < 7 => TimeBucket::Week,
        _ => TimeBucket::Earlier,
    }
}

/// The render-time "today" anchor for the bucket math. Determinism comes
/// from the injection seams downstream (`time_bucket` /
/// `time_groups` both take the anchor as a parameter), not from here.
fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(offset: i64) -> chrono::NaiveDate {
        chrono::Local::now().date_naive() + chrono::Duration::days(offset)
    }

    /// Unix seconds for local noon on `day` (noon survives DST shifts).
    fn stamp_on(day: chrono::NaiveDate) -> i64 {
        use chrono::TimeZone as _;
        chrono::Local
            .from_local_datetime(&day.and_hms_opt(12, 0, 0).expect("noon exists"))
            .single()
            .expect("local noon resolves")
            .timestamp()
    }

    #[test]
    fn time_bucket_respects_the_local_day_boundaries() {
        let today = day(0);
        assert_eq!(time_bucket(stamp_on(today), today), TimeBucket::Today);
        // Future stamps (clock skew) read as today.
        assert_eq!(time_bucket(stamp_on(day(1)), today), TimeBucket::Today);
        assert_eq!(time_bucket(stamp_on(day(-1)), today), TimeBucket::Yesterday);
        assert_eq!(time_bucket(stamp_on(day(-6)), today), TimeBucket::Week);
        assert_eq!(time_bucket(stamp_on(day(-7)), today), TimeBucket::Earlier);
        assert_eq!(time_bucket(stamp_on(day(-30)), today), TimeBucket::Earlier);
        // Unknown stamps land in "earlier".
        assert_eq!(time_bucket(0, today), TimeBucket::Earlier);
        assert_eq!(time_bucket(-5, today), TimeBucket::Earlier);
    }

    #[test]
    fn time_bucket_state_keys_are_unique() {
        // The enum's state keys are one-per-bucket: a duplicate or missing
        // key breaks the collapse mapping long before a user sees it.
        let keys: Vec<&'static str> = [
            TimeBucket::Today,
            TimeBucket::Yesterday,
            TimeBucket::Week,
            TimeBucket::Earlier,
        ]
        .iter()
        .map(|b| b.state_key())
        .collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(keys.len(), sorted.len(), "bucket state keys must be unique");
    }
}
