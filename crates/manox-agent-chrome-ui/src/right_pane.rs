//! The right pane **shell** — the container and tab mechanics of the
//! tool-panel column. Fully decoupled from tab content: content arrives via
//! the [`ToolTab`] trait, kinds register via [`ToolTabFactory`]; hosts decide
//! what lives in the pane and how.
//!
//! Shell responsibilities: the floating rounded card, the tab strip (active
//! tab joins the body with top corners, the new-tab tab, +/split/external),
//! the new-tab empty page (quick actions generated from the registry),
//! open/activate/close lifecycles (closing the last tab collapses the pane),
//! and the width.
//!
//! Invariants:
//! - entity creation / process spawning only happens in `open`/`close` (the
//!   event path); `render` only reads the content store ([`TabStore`]);
//! - ids are **instance** ids: one factory kind may be open as many instances
//!   as the host allows (multiple browser tabs), each minting its own id.
//!
//! Per-session tab sets (suspend/resume across a session switch) are a
//! deliberate gap at this stage — the assembly stage owns that contract.

use std::collections::HashMap;
use std::sync::Arc;

use crate::primitives::{icon_button, small_icon_button};
use crate::theme::{
    BORDER, CARD_BG, CARD_BORDER, FG, FG_DIM, FG_FAINT, FG_STRONG, LIST_HOVER, icon, icons,
};
use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, div, px,
};

/// Tab content store: `open` writes entities (or failure text), `render`
/// only reads. Dropping the entity is the resource teardown (e.g. PTY).
#[derive(Default)]
pub struct TabStore {
    slots: HashMap<String, Slot>,
}

enum Slot {
    Entity(Box<dyn std::any::Any>),
    Error(String),
}

impl TabStore {
    pub fn put<T: 'static>(&mut self, id: &str, entity: Entity<T>) {
        self.slots
            .insert(id.to_string(), Slot::Entity(Box::new(entity)));
    }

    pub fn get<T: 'static>(&self, id: &str) -> Option<Entity<T>> {
        match self.slots.get(id) {
            Some(Slot::Entity(any)) => any.downcast_ref::<Entity<T>>().cloned(),
            _ => None,
        }
    }

    pub fn set_error(&mut self, id: &str, msg: String) {
        self.slots.insert(id.to_string(), Slot::Error(msg));
    }

    /// The most recent error, if any (`render` then shows the error bar with
    /// a retry entry).
    pub fn error(&self, id: &str) -> Option<&str> {
        match self.slots.get(id) {
            Some(Slot::Error(msg)) => Some(msg.as_str()),
            _ => None,
        }
    }

    /// Clear before a retry (event path): removing the entry lets `open`
    /// build again.
    pub fn reset(&mut self, id: &str) {
        self.slots.remove(id);
    }
}

/// One open tab instance. Lifecycle: `open` (first open, side effects) →
/// `render` (read-only) → `close` (default: drop the store entry, dropping
/// the entity reclaims resources).
pub trait ToolTab: 'static {
    /// The factory kind this instance belongs to (how the shell resolves
    /// "another one of these" for the pane's "+").
    fn kind(&self) -> &'static str;
    /// Stable instance id — the open-set key and the store key. Minted by
    /// the factory, unique among open tabs.
    fn id(&self) -> &str;
    /// The tab label at frame time (takes `cx` so a tab may mirror live
    /// state — the browser mirrors the page's `<title>`).
    fn title(&self, cx: &App) -> SharedString;
    /// Tab-strip / quick-action glyph (~15px box).
    fn icon(&self, cx: &App) -> AnyElement;
    /// First open: create the entity / spawn the process. The shell
    /// guarantees idempotence — if the store already holds an entity or an
    /// error for this id, `open` is not called. `pane` is the shell handle a
    /// tab may hold for async repaints (the browser title ticker).
    fn open(
        &self,
        window: &mut Window,
        cx: &mut App,
        store: &mut TabStore,
        pane: &gpui::WeakEntity<RightPane>,
    );
    /// Render the tab body (reads the store only). When `open` failed the
    /// shell takes over (error bar + retry) and never calls this.
    fn render(&self, _window: &mut Window, cx: &App, store: &TabStore) -> AnyElement;
    /// Close: default drops the store entry (dropping the entity reclaims
    /// resources). Hosts owning external resources override this to run
    /// their teardown (the `window`/`cx` are for exactly that).
    fn close(&self, _window: &mut Window, _cx: &mut App, store: &mut TabStore) {
        store.reset(self.id());
    }
    /// Tab-activation / pane-visibility notification (e.g. the browser's OS
    /// subview must hide or it floats above the other tabs).
    fn on_active(&self, _visible: bool, _cx: &mut App, _store: &TabStore) {}

    /// Opaque persistence payload for this instance (the host owns the
    /// schema; `None` = not persistable, the tab is dropped on restore —
    /// the live-session kinds, whose process cannot be resurrected).
    fn persist(&self, _cx: &App, _store: &TabStore) -> Option<String> {
        None
    }
}

/// A registered tool **kind** — the source of the new-tab page's quick
/// actions and the minter of instances. `create` may mint a fresh instance id
/// on every call (that is how one kind goes multi-instance).
pub trait ToolTabFactory: 'static {
    /// Stable kind id (registry lookup key, e.g. `"browser"`).
    fn kind(&self) -> &'static str;
    fn create(&self) -> Arc<dyn ToolTab>;
    /// Rebuild an instance from a persisted payload (see
    /// [`ToolTab::persist`]); `None` skips the entry.
    fn restore(&self, _spec: &str) -> Option<Arc<dyn ToolTab>> {
        None
    }
    /// Quick-action label on the new-tab page; `None` keeps the kind off the
    /// quick-action list (registry-only).
    fn quick_action(&self) -> Option<SharedString>;
    fn icon(&self, cx: &App) -> AnyElement;
}

/// A detached right-pane session: the open tabs, their content store, the
/// active tab, and the pane's visibility — everything needed to suspend one
/// thread's pane and resume it later. Stashed tabs keep their entities (a
/// browser's OS subview is hidden via `on_active(false)`; a terminal keeps
/// running); only an explicit `close_tab` tears content down.
pub struct RightPaneSession {
    pub open: Vec<Arc<dyn ToolTab>>,
    pub store: TabStore,
    pub active_id: Option<String>,
    pub visible: bool,
}

/// The right pane view: a registry-driven shell.
pub struct RightPane {
    registry: Vec<Arc<dyn ToolTabFactory>>,
    open: Vec<Arc<dyn ToolTab>>,
    active: Option<Arc<dyn ToolTab>>,
    pub visible: bool,
    pub width: Pixels,
    store: TabStore,
}

impl RightPane {
    /// `registry` is the full openable set (the source of both tab kinds and
    /// the new-tab page's quick actions).
    pub fn new(registry: Vec<Arc<dyn ToolTabFactory>>) -> Self {
        Self {
            registry,
            open: Vec::new(),
            active: None,
            visible: false,
            width: px(460.),
            store: TabStore::default(),
        }
    }

    pub fn set_width(&mut self, w: f32) {
        self.width = px(w.clamp(320., 900.));
    }

    pub fn find_kind(&self, kind: &str) -> Option<Arc<dyn ToolTabFactory>> {
        self.registry.iter().find(|f| f.kind() == kind).cloned()
    }

    pub fn is_kind_open(&self, kind: &str) -> bool {
        self.open.iter().any(|t| t.id().starts_with(kind))
    }

    /// Open a fresh instance of `kind` (minting a new id) and focus it. Side
    /// effects complete on the event path.
    pub fn open_kind(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(factory) = self.find_kind(kind) else {
            return;
        };
        let tab = factory.create();
        let id = tab.id().to_string();
        let pane = cx.entity().downgrade();
        if !self.open.iter().any(|t| t.id() == id) {
            tab.open(window, cx, &mut self.store, &pane);
            self.open.push(tab.clone());
        }
        self.activate(tab, cx);
    }

    /// Open an already-minted tab instance (host-built) and focus it.
    pub fn open_tab(&mut self, tab: Arc<dyn ToolTab>, window: &mut Window, cx: &mut Context<Self>) {
        let id = tab.id().to_string();
        if !self.open.iter().any(|t| t.id() == id) {
            let pane = cx.entity().downgrade();
            tab.open(window, cx, &mut self.store, &pane);
            self.open.push(tab.clone());
        }
        self.activate(tab, cx);
    }

    /// The live entity behind an open tab, for callers that know its type
    /// (the host that put it there does) — how a host streams into a tab it
    /// opened itself, without owning the tab's lifetime.
    pub fn tab_entity<T: 'static>(&self, id: &str) -> Option<Entity<T>> {
        self.store.get::<T>(id)
    }

    /// The pane's persistable shape: tab kinds + their payloads, in order,
    /// with the active index (the host owns the final on-disk schema).
    pub fn persisted(&self, cx: &App) -> (bool, usize, Vec<(String, String)>) {
        let mut tabs = Vec::new();
        let mut active = 0usize;
        for tab in self.open.iter() {
            if let Some(spec) = tab.persist(cx, &self.store) {
                if self.active.as_ref().is_some_and(|a| a.id() == tab.id()) {
                    active = tabs.len();
                }
                tabs.push((tab.kind().to_string(), spec));
            }
        }
        (self.visible, active, tabs)
    }

    /// Rebuild a pane from a persisted shape (host-driven, on thread entry):
    /// restorable kinds are re-created through their factories; kinds
    /// without a payload are dropped (a dead session cannot be
    /// resurrected). Ignored when the host already holds a live in-session
    /// session for the thread.
    pub fn restore_persisted(
        &mut self,
        tabs: Vec<(String, String)>,
        visible: bool,
        active: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (kind, spec) in tabs {
            let Some(factory) = self.find_kind(&kind) else {
                continue;
            };
            if let Some(tab) = factory.restore(&spec) {
                let id = tab.id().to_string();
                if !self.open.iter().any(|t| t.id() == id) {
                    let pane = cx.entity().downgrade();
                    tab.open(window, cx, &mut self.store, &pane);
                    self.open.push(tab);
                }
            }
        }
        self.active = self.open.get(active).cloned();
        self.visible = visible && !self.open.is_empty();
        if let Some(tab) = self.active.clone() {
            tab.on_active(self.visible, cx, &self.store);
        }
        cx.notify();
    }

    /// Activate an already-open tab (no re-open).
    pub fn activate_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(tab) = self.open.iter().find(|t| t.id() == id).cloned() {
            self.activate(tab, cx);
        }
    }

    fn activate(&mut self, tab: Arc<dyn ToolTab>, cx: &mut Context<Self>) {
        if let Some(prev) = self.active.replace(tab.clone())
            && prev.id() != tab.id()
        {
            prev.on_active(false, cx, &self.store);
        }
        self.visible = true;
        tab.on_active(true, cx, &self.store);
        cx.notify();
    }

    /// Close a tab: reclaim the content (entity drop); closing the last tab
    /// collapses the pane.
    pub fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.open.iter().find(|t| t.id() == id).cloned() else {
            return;
        };
        tab.close(window, cx, &mut self.store);
        self.open.retain(|t| t.id() != id);
        if self.active.as_ref().is_some_and(|t| t.id() == id) {
            if let Some(last) = self.open.last().cloned() {
                self.activate(last, cx);
            } else {
                self.active = None;
            }
        }
        if self.open.is_empty() {
            self.visible = false;
        }
        cx.notify();
    }

    /// Close every open tab of `kind`, in open order; returns how many went
    /// away. For kinds whose content cannot outlive its thread, the host
    /// retires them before stashing the pane instead of carrying dead content
    /// across the switch.
    pub fn close_kind(&mut self, kind: &str, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let ids: Vec<String> = self
            .open
            .iter()
            .filter(|t| t.kind() == kind)
            .map(|t| t.id().to_string())
            .collect();
        let closed = ids.len();
        for id in ids {
            self.close_tab(&id, window, cx);
        }
        closed
    }

    /// "+"/new-tab page: back to the empty page (open tabs stay).
    pub fn new_tab_page(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.active.take() {
            prev.on_active(false, cx, &self.store);
        }
        self.visible = true;
        cx.notify();
    }

    /// Detach the whole session (per-thread stashing): the active tab gets
    /// `on_active(false)` (a browser subview hides), the open set / store /
    /// visibility move out, and the pane resets to a fresh empty state.
    pub fn stash_session(&mut self, cx: &mut Context<Self>) -> RightPaneSession {
        if let Some(active) = &self.active {
            active.on_active(false, cx, &self.store);
        }
        let session = RightPaneSession {
            open: std::mem::take(&mut self.open),
            store: std::mem::take(&mut self.store),
            active_id: self.active.as_ref().map(|t| t.id().to_string()),
            visible: self.visible,
        };
        self.active = None;
        self.visible = false;
        cx.notify();
        session
    }

    /// Resume a stashed session: the open set, store, active tab, and
    /// visibility return as one unit; the active tab is re-armed
    /// (`on_active(visible)`).
    pub fn restore_session(&mut self, session: RightPaneSession, cx: &mut Context<Self>) {
        self.open = session.open;
        self.store = session.store;
        self.active = session
            .active_id
            .as_ref()
            .and_then(|id| self.open.iter().find(|t| t.id() == id).cloned());
        self.visible = session.visible;
        if let Some(tab) = &self.active {
            tab.on_active(self.visible, cx, &self.store);
        }
        cx.notify();
    }

    /// Titlebar toggle: expand/collapse; expanding with no tabs lands on the
    /// new-tab empty page.
    pub fn toggle_visible(&mut self, cx: &mut Context<Self>) {
        self.visible = !self.visible;
        if self.visible && self.open.is_empty() {
            self.active = None;
        }
        let visible = self.visible;
        if let Some(tab) = self.active.clone() {
            tab.on_active(visible, cx, &self.store);
        }
        cx.notify();
    }
}

impl gpui::Render for RightPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.width;
        let active = self.active.clone();

        // Tab strip (closures in the loop go through the entity handle).
        let entity = cx.entity();
        let pills: Vec<AnyElement> = self
            .open
            .clone()
            .iter()
            .map(|tab| {
                let t = tab.clone();
                let ent = entity.clone();
                let activate = move |_: &ClickEvent, _w: &mut Window, cx: &mut App| {
                    let id = t.id().to_string();
                    ent.update(cx, |pane, cx| pane.activate_tab(&id, cx));
                };
                let t2 = tab.clone();
                let ent2 = entity.clone();
                let close = move |_: &ClickEvent, w: &mut Window, cx: &mut App| {
                    let id = t2.id().to_string();
                    ent2.update(cx, |pane, cx| pane.close_tab(&id, w, cx));
                };
                tab_pill(
                    tab,
                    cx,
                    active.as_ref().map(|a| a.id()) == Some(tab.id()),
                    tab.icon(cx),
                    activate,
                    close,
                )
                .into_any_element()
            })
            .collect();

        let on_new_tab = cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.new_tab_page(cx);
        });
        // The pane "+" opens another instance of the active tab's kind —
        // one click demonstrates the multi-instance contract.
        let on_plus = cx.listener(|this, _: &ClickEvent, w, cx| {
            if let Some(kind) = this.active.as_ref().map(|t| t.kind()) {
                this.open_kind(kind, w, cx);
            }
        });

        div()
            .w(width)
            .h_full()
            .flex_shrink_0()
            .bg(CARD_BG)
            .border_1()
            .border_color(CARD_BORDER)
            .rounded(px(crate::theme::CARD_RADIUS))
            .overflow_hidden()
            .text_color(FG)
            .text_size(px(13.))
            .flex()
            .flex_col()
            // Tab strip: tool tabs + the new-tab tab + right-side
            // +/split/external.
            .child(
                div()
                    .w_full()
                    .h(px(36.))
                    .flex_shrink_0()
                    .pl(px(4.))
                    .pr(px(6.))
                    .gap(px(2.))
                    .items_end()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_end()
                            .gap(px(2.))
                            .children(pills)
                            .child(new_tab_pill(active.is_none(), on_new_tab)),
                    )
                    .child(
                        div()
                            .h_full()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .child(icon_button(
                                "rp-add",
                                icons::ADD,
                                14.,
                                false,
                                move |e, w, cx| on_plus(e, w, cx),
                            ))
                            .child(icon_button(
                                "rp-split",
                                icons::SPLIT_HORIZONTAL,
                                14.,
                                false,
                                |_, _, _| {},
                            ))
                            .child(icon_button(
                                "rp-external",
                                icons::LINK_EXTERNAL,
                                14.,
                                false,
                                |_, _, _| {},
                            )),
                    ),
            )
            .child(div().w_full().h(px(1.)).bg(BORDER).flex_shrink_0())
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_body(window, cx, &active)),
            )
    }
}

impl RightPane {
    fn render_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        active: &Option<Arc<dyn ToolTab>>,
    ) -> AnyElement {
        match active {
            None => {
                let entity = cx.entity();
                let actions: Vec<AnyElement> = self
                    .registry
                    .clone()
                    .iter()
                    .filter_map(|factory| {
                        let label = factory.quick_action()?;
                        let f = factory.clone();
                        let ent = entity.clone();
                        let open = move |_: &ClickEvent, w: &mut Window, cx: &mut App| {
                            let kind = f.kind();
                            ent.update(cx, |pane, cx| pane.open_kind(kind, w, cx));
                        };
                        Some(quick_action(factory.icon(cx), label, open).into_any_element())
                    })
                    .collect();
                div()
                    .w_full()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(div().w_full().flex_1())
                    .child(
                        div()
                            .w_full()
                            .pl(px(10.))
                            .pr(px(10.))
                            .pb(px(12.))
                            .gap(px(1.))
                            .flex()
                            .flex_col()
                            .children(actions),
                    )
                    .into_any_element()
            }
            Some(tab) => {
                // A failed open → the shell's uniform error bar + retry
                // (retrying re-opens on the event path).
                if let Some(err) = self.store.error(tab.id()).map(str::to_string) {
                    let t = tab.clone();
                    let retry = cx.listener(move |this, _e: &ClickEvent, w, cx| {
                        let id = t.id().to_string();
                        this.store.reset(&id);
                        let pane = cx.entity().downgrade();
                        t.open(w, cx, &mut this.store, &pane);
                        cx.notify();
                    });
                    return error_body(&err, retry);
                }
                let store = &self.store;
                tab.render(window, cx, store)
            }
        }
    }
}

/// Error bar + retry button (the shell's uniform fallback).
fn error_body(
    msg: &str,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .w_full()
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .child(div().text_color(FG_FAINT).child(msg.to_string()))
        .child(
            div()
                .id("tool-tab-retry")
                .on_click(move |e, w, cx| retry(e, w, cx))
                .py(px(6.))
                .px(px(10.))
                .rounded(px(5.))
                .border_1()
                .border_color(BORDER)
                .text_color(FG)
                .hover(|style| style.bg(LIST_HOVER))
                .child(manox_i18n::t("chrome-tab-retry")),
        )
        .into_any_element()
}

/// Top-corner tab pill: the active state is 31px tall with a stroke and
/// joins the body (no bottom border); inactive pills are ghosted.
fn tab_pill(
    tab: &Arc<dyn ToolTab>,
    cx: &App,
    active: bool,
    icon_el: AnyElement,
    activate: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let pill = div()
        .id(SharedString::from(format!("tool-tab-{}", tab.id())))
        .on_click(move |e, w, cx| activate(e, w, cx))
        .h(px(if active { 31. } else { 28. }))
        .pl(px(10.))
        .pr(px(8.))
        .gap(px(6.))
        .items_center()
        .flex_shrink_0()
        .rounded_tl(px(7.))
        .rounded_tr(px(7.))
        .flex()
        .text_color(if active { FG_STRONG } else { FG_DIM });
    let pill = if active {
        pill.bg(CARD_BG)
            .border_1()
            .border_color(BORDER)
            .border_b_0()
    } else {
        pill.hover(|style| style.bg(LIST_HOVER))
    };
    pill.child(icon_el)
        .child(
            div()
                .min_w_0()
                .max_w(px(120.))
                .truncate()
                .text_size(px(12.))
                .child(tab.title(cx)),
        )
        .child(small_icon_button(
            SharedString::from(format!("tool-tab-close-{}", tab.id())),
            icons::CLOSE,
            10.,
            FG_FAINT,
            FG,
            close,
        ))
}

fn new_tab_pill(
    active: bool,
    on_new_tab: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    let pill = div()
        .id("tool-new-tab")
        .on_click(move |e, w, cx| on_new_tab(e, w, cx))
        .h(px(if active { 31. } else { 28. }))
        .pl(px(10.))
        .pr(px(8.))
        .gap(px(6.))
        .items_center()
        .flex_shrink_0()
        .rounded_tl(px(7.))
        .rounded_tr(px(7.))
        .flex()
        .text_color(if active { FG_STRONG } else { FG_DIM });
    let pill = if active {
        pill.bg(CARD_BG)
            .border_1()
            .border_color(BORDER)
            .border_b_0()
    } else {
        pill.hover(|style| style.bg(LIST_HOVER))
    };
    pill.child(icon(icons::ADD, 12.)).child(
        div()
            .truncate()
            .text_size(px(12.))
            .child(manox_i18n::t("chrome-tab-new-tab")),
    )
}

/// Quick-action row: icon + label, whole row clickable, hover wash.
fn quick_action(
    icon_el: AnyElement,
    label: SharedString,
    open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("qa-{label}")))
        .on_click(move |e, w, cx| open(e, w, cx))
        .w_full()
        .py(px(8.))
        .px(px(10.))
        .gap(px(10.))
        .items_center()
        .rounded(px(6.))
        .flex()
        .hover(|style| style.bg(LIST_HOVER))
        .child(icon_el)
        .child(div().text_size(px(13.)).child(label))
}
