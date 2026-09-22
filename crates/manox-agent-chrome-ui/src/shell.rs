//! The shell root view — the app window's chrome. Owns layout state (sidebar
//! width/visibility, bottom dock, right pane) and the sidebar's *interaction*
//! state (selection, collapse, group order, row menu, session picker); the
//! session rows themselves are host-pushed snapshots ([`Shell::set_sessions`])
//! and every state-changing action is mirrored to the host through
//! [`HostHooks`] — the chrome never touches a data source.

use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, actions, div, px,
};

use crate::main_surface::MainSurfaceHandle;
use crate::panel::{PanelSlot, PanelSurface};
use crate::right_pane::{RightPane, ToolTabFactory};
use crate::session_list::{
    CustomizationRow, FixedRow, SessionGroup, SessionList, SessionRowData, SessionStatus,
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
    pub time: String,
    pub status: SessionStatus,
    /// `ThreadStore`-style update stamp (unix seconds) for local re-sorting
    /// after a pin flip.
    pub updated_at: i64,
    pub pinned: bool,
    pub unread: bool,
    /// D2 columns: the user tag chip, team-nesting depth, leader mark.
    pub tag: Option<String>,
    pub indent: u8,
    pub team_leader: bool,
}

impl SessionRow {
    /// Lift one projected group (see agent-ui's `sidebar_projection`) into
    /// the shell's row carrier, deriving each row's sort stamp from its
    /// position (the projection emits recency order already).
    pub fn from_group(group: crate::session_list::SessionGroup) -> Vec<SessionRow> {
        group
            .rows
            .into_iter()
            .enumerate()
            .map(|(ix, r)| SessionRow {
                id: r.id,
                title: r.title,
                workspace: group.name.clone(),
                time: r.time,
                status: r.status,
                updated_at: -(ix as i64),
                pinned: r.pinned,
                unread: r.unread,
                tag: r.tag,
                indent: r.indent,
                team_leader: r.team_leader,
            })
            .collect()
    }

    fn row_data(&self) -> SessionRowData {
        SessionRowData {
            id: self.id.clone(),
            title: self.title.clone(),
            time: self.time.clone(),
            status: self.status,
            pinned: self.pinned,
            unread: self.unread,
            tag: self.tag.clone(),
            indent: self.indent,
            team_leader: self.team_leader,
        }
    }
}

/// Host action carrying a row id.
pub type HookOnId = Box<dyn Fn(&str, &mut Window, &mut App)>;
/// Host action with no payload.
pub type HookOnUnit = Box<dyn Fn(&mut Window, &mut App)>;

/// Host-side actions the shell mirrors to. All optional; an absent hook
/// leaves the shell's local behavior only (e.g. `on_pin` flips the local row
/// and the next `set_sessions` snapshot reconciles).
#[derive(Default)]
pub struct HostHooks {
    pub on_pin: Option<HookOnId>,
    pub on_archive: Option<HookOnId>,
    /// New-session request (the New button / ⌘N). Absent → the shell just
    /// clears the active session.
    pub on_new_session: Option<HookOnUnit>,
    /// Selection-change notification (the future chat-column swap hooks
    /// here).
    pub on_select: Option<HookOnId>,
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
    pub hooks: HostHooks,
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
    // ── layout ──
    pub show_sidebar: bool,
    pub show_panel: bool,
    /// The right pane (the shell view; tab content is decoupled through
    /// ToolTab — see `right_pane.rs`).
    pub right: Entity<RightPane>,
    /// Sidebar width (invisible-handle drag, 180–460, double-click reset).
    pub sidebar_width: Pixels,
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
            fixed_rows: config.fixed_rows,
            customizations: config.customizations,
            row_menu: None,
            row_menu_sub: None,
            show_sidebar: true,
            show_panel: false,
            sidebar_width: px(divider::SIDEBAR_DEFAULT),
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
    /// marker.
    pub fn move_group(&mut self, dragged: &str, target: &str, before: bool) {
        self.group_drag_marker = None;
        if dragged == target {
            return;
        }
        // Materialize the current display order into `group_order` first
        // (unrecorded groups trail in default order), then move, so a first
        // drag never loses the existing relative order.
        let current: Vec<String> = self.sidebar_props().1.into_iter().map(|g| g.name).collect();
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

    /// Row menu (kebab / right-click): copy Thread ID, pin/unpin, archive.
    pub fn open_row_menu(
        &mut self,
        id: &str,
        anchor: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_component::menu::{PopupMenu, PopupMenuItem};

        let Some(sess) = self.sessions.iter().find(|s| s.id == id) else {
            return;
        };
        let pinned = sess.pinned;
        let this = cx.entity();

        let id_copy = id.to_string();
        let id_pin = id.to_string();
        let id_archive = id.to_string();
        let id_pin2 = id_pin.clone();
        let this_pin = this.clone();
        let this_archive = this.clone();

        let menu = PopupMenu::build(window, cx, move |menu, _w, _cx| {
            menu.max_w(gpui::px(220.))
                .item(
                    PopupMenuItem::new(manox_i18n::t("chrome-row-copy-id")).on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(id_copy.clone()));
                        },
                    ),
                )
                .item(
                    PopupMenuItem::new(if pinned {
                        manox_i18n::t("chrome-row-unpin")
                    } else {
                        manox_i18n::t("chrome-row-pin")
                    })
                    .on_click(move |_, _, cx| {
                        let id = id_pin2.clone();
                        this_pin.update(cx, |this, cx| {
                            this.toggle_pin(&id, cx);
                            this.row_menu = None;
                            cx.notify();
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new(manox_i18n::t("chrome-row-archive")).on_click(
                        move |_, _, cx| {
                            let id = id_archive.clone();
                            this_archive.update(cx, |this, cx| {
                                this.archive_session(&id, cx);
                                this.row_menu = None;
                                cx.notify();
                            });
                        },
                    ),
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

    pub fn toggle_group(&mut self, name: &str) {
        if let Some(i) = self.collapsed.iter().position(|k| k == name) {
            self.collapsed.remove(i);
        } else {
            self.collapsed.push(name.to_string());
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

    /// Archive: the hook performs the store write; the local row disappears
    /// (and the active selection clears if it was archived).
    pub fn archive_session(&mut self, id: &str, cx: &mut App) {
        self.sessions.retain(|s| s.id != id);
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
        let _ = cx;
    }

    /// Pinned first, then by update time descending.
    fn sort_sessions(&mut self) {
        self.sessions.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
    }

    /// Sidebar props (fixed rows + workspace groups + Customizations).
    pub fn sidebar_props(&self) -> (Vec<FixedRow>, Vec<SessionGroup>, Vec<CustomizationRow>) {
        let mut groups: Vec<SessionGroup> = Vec::new();
        for s in &self.sessions {
            let collapsed = self.collapsed.iter().any(|k| k == &s.workspace);
            if let Some(g) = groups.iter_mut().find(|g| g.name == s.workspace) {
                g.rows.push(s.row_data());
                g.collapsed = collapsed;
            } else {
                groups.push(SessionGroup {
                    name: s.workspace.clone(),
                    collapsed,
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
        (self.fixed_rows.clone(), groups, self.customizations.clone())
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
        let (fixed, groups, customizations) = self.sidebar_props();
        let selected = self.active.clone();
        let has_chats = !self.sessions.is_empty();
        let marker = self.group_drag_marker.clone();

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
        // Pin/archive: run the host hook (the real store write) first, then
        // flip the local row — the next snapshot push reconciles.
        let on_pin = cx.listener(|this, id: &String, w, cx| {
            if let Some(hook) = &this.hooks.on_pin {
                let id = id.clone();
                hook(&id, w, cx);
            }
            let id = id.clone();
            this.toggle_pin(&id, cx);
            cx.notify();
        });
        let on_archive = cx.listener(|this, id: &String, w, cx| {
            if let Some(hook) = &this.hooks.on_archive {
                let id = id.clone();
                hook(&id, w, cx);
            }
            let id = id.clone();
            this.archive_session(&id, cx);
            cx.notify();
        });
        let on_row_menu = cx.listener(|this, (id, pos): &(String, gpui::Point<Pixels>), w, cx| {
            this.open_row_menu(id, *pos, w, cx);
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
                on_select: std::rc::Rc::new(move |id, w, cx| on_select(id, w, cx)),
                on_toggle_group: std::rc::Rc::new(move |name, w, cx| on_toggle_group(name, w, cx)),
                on_new: std::rc::Rc::new(move |e, w, cx| on_new(e, w, cx)),
                on_pin: Some(std::rc::Rc::new(move |id, w, cx| on_pin(id, w, cx))),
                on_archive: Some(std::rc::Rc::new(move |id, w, cx| on_archive(id, w, cx))),
                on_row_menu: std::rc::Rc::new(move |id, pos, w, cx| {
                    on_row_menu(&(id.clone(), pos), w, cx)
                }),
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
