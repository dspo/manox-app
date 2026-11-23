//! The canvas renderer: the container's layout tree, its tab strips and the
//! sheet bodies placed on it.
//!
//! Layout geometry comes from `steer_canvas::projection` (pure); this module
//! only draws what that reports and turns pointer gestures into [`CanvasHost`]
//! commands. Every gesture commits through the host's single write path, so a
//! stale reply from a canvas that is no longer shown is dropped by the state,
//! not by the renderer remembering to check.
//!
//! Bodies: a sheet's content is produced by the entity that owns it — the
//! conversation column's own view for `session:` sheets, the launcher's for
//! `home:`. The canvas never builds another entity's page, so it can never take
//! a second lease on the workspace while the shell is rendering.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::{ElementExt as _, Icon, Sizable as _, h_flex, v_flex};

use steer_canvas::id::{home_sheet_id, sheet_kind, terminal_resource, terminal_sheet_id};
use steer_canvas::projection::{self, GroupFrame};
use steer_canvas::state::CanvasSnapshot;

use crate::views::centered;
use crate::workspace::canvas_host::{CanvasAction, CanvasHost};
use crate::workspace::chat_column::ChatColumn;
use crate::workspace::column_view::ChatColumnView;
use steer_agent_chat_ui::pi::AcpStore;
use steer_agent_chat_ui::views::pi::PiSessionView;
use steer_canvas::persist;

/// The tab strip's height (the sheet frames start below it).
pub const TAB_STRIP_H: f32 = 34.;

/// The canvas's cross-entity requests. Every gesture runs inside the canvas's
/// own lease, so a request the workspace must serve — one that re-enters the
/// canvas for the teardown — crosses the boundary as an event: the
/// subscription runs when the canvas is no longer leased.
pub(crate) enum CanvasEvent {
    /// The shown canvas lost its last sheet: the container must go through
    /// the workspace's archive entry (confirm-or-direct).
    AutoArchive(String),
}

impl gpui::EventEmitter<CanvasEvent> for CanvasView {}

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
    /// The shown container's working directory: a terminal sheet spawns there.
    /// Injected by the workspace when the canvas opens a container (the canvas
    /// never reads the workspace itself — that would take a second lease while
    /// the shell is updating it).
    cwd: std::path::PathBuf,
    /// The newest edit not known to be on disk: the host's pending save,
    /// kept here so the debounce is not the only writer. A container leaving
    /// the foreground lands it before its canvas parks, an archive retires
    /// it, and the view's release lands whatever is left.
    pending_save: Option<(u64, String, CanvasSnapshot)>,
    /// The release face of [`Self::pending_save`] — dropping the entity must
    /// not drop its last unsaved edit. Held so the listener stays live.
    _release_flush: gpui::Subscription,
    /// The card's interior size, measured at prepaint (the first frame of a
    /// fresh window falls back to the window's).
    size: Rc<std::cell::Cell<Option<Size<Pixels>>>>,
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
fn sheet_titles(chat: Entity<ChatColumn>) -> SheetTitles {
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
    chat: Entity<ChatColumn>,
    pi_store: Entity<AcpStore>,
) -> steer_canvas::SheetRegistry {
    use std::sync::Arc;
    use steer_canvas::{ParsedSheet, SheetKindSpec};

    let titles = sheet_titles(chat.clone());
    let session_restore: steer_canvas::registry::RestoreFn = {
        let chat = chat.clone();
        Arc::new(move |sheet, cx| session_sheet_resource_alive(&chat, sheet, cx))
    };
    let kind = |kind: &'static str,
                glyph: &'static str,
                names_container: bool,
                title: steer_canvas::registry::TitleFn,
                tab_title: Option<steer_canvas::registry::TabTitleFn>,
                restore: Option<steer_canvas::registry::RestoreFn>,
                parse: Option<steer_canvas::registry::ParseFn>|
     -> SheetKindSpec {
        SheetKindSpec {
            kind,
            title,
            tab_title,
            glyph,
            closable: true,
            names_container,
            restore,
            parse,
            // The canvas renders these bodies inline from the entities it
            // holds; the kind still owns existence, title and closability.
            body: None,
        }
    };
    steer_canvas::SheetRegistry::new()
        .with(pi_kind(pi_store))
        .with(kind(
            "home",
            "home",
            false,
            static_title("canvas-tab-home"),
            None,
            None,
            None,
        ))
        .with(kind(
            "terminal",
            "terminal",
            false,
            static_title("canvas-tab-terminal"),
            None,
            // A terminal's PTY is re-attached on restore (`open_container`
            // re-spawns a fresh shell per sheet), so the layout is the fact.
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
            // The tab-only projection: a program's OSC title renames its tab,
            // never the container.
            Some(Arc::new(|id, _| {
                let session = steer_canvas::id::session_of_sheet(id)?;
                crate::title_store::TitleStore::global()
                    .program_title(session)
                    .map(SharedString::from)
            })),
            Some(session_restore),
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
        _ => IconName::File,
    }
}

impl CanvasView {
    pub(crate) fn new(
        host: CanvasHost,
        pi_store: Entity<AcpStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        // The entity's last flush: when it is released (app exit, workspace
        // teardown) the debounced task dies with it, so the pending save
        // lands here instead of never.
        let release_flush = cx.on_release(|view, _| view.flush_pending_save());
        Self {
            host,
            column: None,
            terminals: HashMap::new(),
            pi_views: HashMap::new(),
            pi_store,
            cwd: std::env::current_dir().unwrap_or_default(),
            pending_save: None,
            _release_flush: release_flush,
            size: Rc::new(std::cell::Cell::new(None)),
        }
    }

    pub(crate) fn set_column(&mut self, column: Entity<ChatColumnView>, cx: &mut Context<Self>) {
        self.column = Some(column);
        cx.notify();
    }

    /// Shows `container_id`'s canvas, seeding its start page and — when the
    /// container is a session — that session's sheet.
    ///
    /// The restore verdicts come from the registry's restore face, evaluated
    /// up front as plain data (the predicate must not borrow the host
    /// mid-switch); a sheet the registry cannot check reads alive — the
    /// layout is the fact. After the switch, the restored layout's resources
    /// come back before the first render: a terminal sheet re-attaches a
    /// fresh shell (the old one died with the process), and a pi sheet
    /// re-opens its session in the store so the tab never paints the
    /// closed-session empty state.
    pub(crate) fn open_container(
        &mut self,
        container_id: &str,
        session_id: Option<&str>,
        cwd: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.cwd = cwd;
        // The container leaving the foreground lands its newest edit before
        // its canvas parks: the switch's generation bump would otherwise
        // retire the debounced write and lose the edit.
        self.flush_pending_save();
        let saved = persist::read_canvas(container_id).map(|file| file.snapshot());
        let verdicts: HashMap<String, bool> = saved
            .as_ref()
            .map(|snapshot| {
                steer_canvas::layout::panel_ids(&snapshot.layout)
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default()
            .into_iter()
            .chain(self.host.sheet_order(container_id).iter().cloned())
            .map(|sheet| {
                let alive = self.host.registry().restore(&sheet, cx);
                (sheet, alive)
            })
            .collect();
        let alive = move |sheet: &str| verdicts.get(sheet).copied().unwrap_or(true);
        self.host.switch_container(container_id, saved, alive);
        // Both re-attachments are idempotent, so a canvas returning from its
        // park (its resources never left) does no work here.
        let pi_cwd = self.cwd.clone();
        let sheets: Vec<String> = steer_canvas::layout::panel_ids(self.host.layout())
            .into_iter()
            .map(str::to_string)
            .collect();
        for sheet in &sheets {
            match sheet_kind(sheet) {
                "terminal" => self.ensure_terminal(sheet, cx),
                "pi-session" => {
                    let session_id = pi_session_of_sheet(sheet).to_owned();
                    // The store keeps no per-session cwd; the container's
                    // stands in for the re-open (a session the harness lost
                    // is rebuilt under the same id, living here).
                    self.pi_store.update(cx, |store, cx| {
                        store.open_existing(&session_id, pi_cwd.clone(), true, cx);
                    });
                }
                _ => {}
            }
        }
        self.host.open_sheet(&home_sheet_id(container_id), None);
        if let Some(session_id) = session_id {
            let sheet = steer_canvas::id::session_sheet_id(session_id);
            self.host.open_sheet(&sheet, None);
        }
        cx.notify();
    }

    /// Opens the container's next terminal sheet. A terminal's PTY view
    /// exists only while its sheet does: `ensure_terminal` spawns it at the
    /// sheet's first appearance — a fresh tab here, or a restored layout
    /// re-attaching a fresh shell to a tab that came back from disk.
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
        // The container leaving the foreground parks here; its newest edit
        // lands before the park (the switch retires the debounced write).
        self.flush_pending_save();
        self.host.show_scratch();
        cx.notify();
    }

    /// Schedules the debounced layout write (the newest edit wins). The
    /// pending save is kept on the view so the newest edit can land even when
    /// its debounce window is cut short — a container switch flushes it
    /// before parking, an archive retires it, and the view's release flushes
    /// whatever is left.
    fn after_edit(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.host.pending_save() else {
            return;
        };
        let generation = pending.0;
        self.pending_save = Some(pending);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(super::canvas_host::SAVE_DEBOUNCE)
                .await;
            let _ = this.update(cx, |view, _| {
                // A newer edit replaced this one's pending save (its own task
                // lands it); the switch / forget / release faces own the rest.
                if view.host.save_generation() != generation {
                    return;
                }
                view.flush_pending_save();
            });
        })
        .detach();
    }

    /// Writes the pending save now. The generation guard keeps a superseded
    /// snapshot from ever overwriting a newer write — the newest edit for a
    /// container wins, whichever face landed it.
    fn flush_pending_save(&mut self) {
        let Some((generation, container_id, snapshot)) = self.pending_save.take() else {
            return;
        };
        if generation != self.host.save_generation() {
            return;
        }
        write_layout_snapshot(&container_id, &snapshot);
    }

    /// The canvas page: every group's tab strip and content area.
    pub(crate) fn render_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let size = self.size.get().unwrap_or_else(|| window.bounds().size);
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        let layout = self.host.layout().clone();
        let boxes = projection::tab_bar_boxes(&layout, width, height, self.host.zoomed());
        let groups = projection::group_frames(&boxes, TAB_STRIP_H);
        let sheet_id = self.host.active_container().map(str::to_string);
        let scratch = sheet_id.is_none();

        let mut root = v_flex()
            .size_full()
            .relative()
            .bg(theme.background)
            .track_focus(&cx.focus_handle());

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

    /// One group: its tab strip and the displayed sheet's body.
    fn render_group(
        &mut self,
        box_: &projection::TabBarBox,
        group: &GroupFrame,
        scratch: bool,
        theme: &gpui_component::Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rect = group.rect;
        // The strip is positioned inside the group's own box (which is itself
        // absolute), so its origin is the group's, not the canvas's.
        let mut strip = h_flex()
            .id(ElementId::Name(
                format!("canvas-strip-{}", group.key).into(),
            ))
            .debug_selector({
                let key = group.key.clone();
                move || format!("canvas-strip-{key}")
            })
            .absolute()
            .left_0()
            .top_0()
            .w_full()
            .h(px(TAB_STRIP_H))
            .items_center()
            .gap_1()
            .px_2()
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.border);

        for (index, sheet) in box_.panels.iter().enumerate() {
            let active = index == box_.active;
            // The tab's label is the kind's tab title (a program-renamed tab,
            // else the sheet's title); an unowned id shows its raw id (the
            // shell never strands a tab it cannot name).
            let label = self
                .host
                .registry()
                .tab_title(sheet, cx)
                .unwrap_or_else(|| SharedString::from(sheet.clone()));
            let id = sheet.clone();
            let glyph = self.host.registry().glyph(sheet);
            let closable = self.host.registry().closable(sheet);

            let mut tab = h_flex()
                .id(ElementId::Name(format!("canvas-tab-{id}").into()))
                .debug_selector({
                    let id = id.clone();
                    move || format!("canvas-tab-{id}")
                })
                .h(px(TAB_STRIP_H - 8.))
                .px_2()
                .gap_1()
                .items_center()
                .rounded(theme.radius)
                .cursor_pointer()
                .when(active, |this| {
                    this.bg(theme.background).text_color(theme.foreground)
                })
                .when(!active, |this| this.text_color(theme.muted_foreground))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                    // A double-click opens the start page in a new group
                    // beside this tab (the v1 split entry).
                    if event.click_count() >= 2 {
                        this.split_home(&id, cx);
                        return;
                    }
                    if this.host.activate_sheet(&id) == CanvasAction::Applied {
                        this.after_edit(cx);
                        cx.notify();
                    }
                }));

            if let Some(glyph) = glyph {
                tab = tab.child(Icon::from(glyph_icon(glyph)).small());
            }
            tab = tab.child(gpui::div().text_sm().child(label));

            if closable {
                let close_id = sheet.clone();
                tab = tab.child(
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_sheet(&close_id, cx);
                        }))
                        .child(
                            gpui_component::Icon::from(gpui_kit_assets::IconName::Close).xsmall(),
                        ),
                );
            }
            strip = strip.child(tab);
        }

        // The group's terminal "+": opens the container's next terminal sheet.
        if self.host.active_container().is_some() {
            strip = strip.child(
                gpui::div()
                    .id(ElementId::Name(
                        format!("canvas-open-terminal-{}", group.key).into(),
                    ))
                    .debug_selector(|| "canvas-open-terminal".to_string())
                    .px_1()
                    .rounded(theme.radius)
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.muted))
                    .on_click(cx.listener(|this, _, _, cx| this.open_terminal(cx)))
                    .child(Icon::from(gpui_kit_assets::IconName::SquareTerminal).xsmall()),
            );
            strip = strip.child(
                gpui::div()
                    .id(ElementId::Name(
                        format!("canvas-open-pi-{}", group.key).into(),
                    ))
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

        // The group's own "+": the container's start page.
        if let Some(container) = self.host.active_container().map(str::to_string) {
            let home = home_sheet_id(&container);
            strip = strip.child(
                gpui::div()
                    .id(ElementId::Name(
                        format!("canvas-open-home-{}", group.key).into(),
                    ))
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
        let _ = scratch;

        let active_sheet = box_.panels.get(box_.active).cloned();
        let body = active_sheet
            .as_deref()
            .map(|sheet| self.render_sheet_body(sheet, window, cx))
            .unwrap_or_else(|| gpui::div().into_any_element());

        v_flex()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .overflow_hidden()
            .child(gpui::div().flex_1().min_h_0().w_full().h_full().child(body))
            .child(strip)
            .into_any_element()
    }

    /// One sheet's content, produced by the entity that owns it.
    fn render_sheet_body(
        &mut self,
        sheet: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    /// The v1 split gesture: the container's start page opens in a new group
    /// beside `beside`.
    pub(crate) fn split_home(&mut self, beside: &str, cx: &mut Context<Self>) {
        let Some(container) = self.host.active_container().map(str::to_string) else {
            return;
        };
        let home = home_sheet_id(&container);
        if self.host.split_right(beside, &home) == CanvasAction::Applied {
            self.after_edit(cx);
            cx.notify();
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

    fn close_sheet(&mut self, sheet: &str, cx: &mut Context<Self>) {
        let is_pi = sheet_kind(sheet) == "pi-session";
        match self.host.close_sheet(sheet) {
            CanvasAction::Applied => {
                // The layout commit landed: the sheet is gone, so a pi sheet's
                // session closes with it. On a dropped commit (`Stale`) the
                // sheet stays shown — closing its session would strand it on a
                // dead store.
                if is_pi {
                    self.close_pi_sheet(sheet, cx);
                }
                self.prune_terminals();
                self.after_edit(cx);
                cx.notify();
            }
            CanvasAction::Stale => {}
            CanvasAction::AutoArchive(container_id) => {
                if is_pi {
                    self.close_pi_sheet(sheet, cx);
                }
                // The canvas is never empty: the start page reopens in place,
                // so a refused archive leaves the container on its home page.
                if self.host.open_sheet(&home_sheet_id(&container_id), None)
                    == CanvasAction::Applied
                {
                    self.after_edit(cx);
                }
                // The archive itself is the workspace's write, and its landing
                // re-enters this canvas (`archive_containers` forgets the
                // container) — the request crosses on the event face, because
                // this gesture runs inside the canvas's own lease.
                cx.emit(CanvasEvent::AutoArchive(container_id));
                cx.notify();
            }
        }
    }

    /// Closes a pi sheet's body and its session: the view is dropped and the
    /// store tears the session and its process down.
    fn close_pi_sheet(&mut self, sheet: &str, cx: &mut Context<Self>) {
        self.pi_views.remove(sheet);
        let session_id = pi_session_of_sheet(sheet).to_owned();
        self.pi_store
            .update(cx, |store, cx| store.close_session(&session_id, cx));
    }

    /// The pi-session sheets a container's canvas holds: its shown layout (if
    /// this is the shown canvas) plus its parked copy.
    fn container_pi_sheets(&self, container_id: &str) -> Vec<String> {
        let mut sheets = Vec::new();
        if self.host.active_container() == Some(container_id) {
            sheets.extend(
                steer_canvas::layout::panel_ids(self.host.layout())
                    .into_iter()
                    .map(str::to_string),
            );
        }
        if let Some(snapshot) = self.host.held().get(container_id) {
            sheets.extend(
                steer_canvas::layout::panel_ids(&snapshot.layout)
                    .into_iter()
                    .map(str::to_string),
            );
        }
        sheets.retain(|sheet| sheet_kind(sheet) == "pi-session");
        sheets.sort();
        sheets.dedup();
        sheets
    }

    /// Drops everything the canvas keeps for a gone container: its layout
    /// (shown or parked), its terminal PTY views and its pi sessions. The
    /// ONE archive landing (`Workspace::archive_containers`) is the caller,
    /// so a container forgotten here can never come back through a gesture.
    pub(crate) fn forget_container(&mut self, container_id: &str, cx: &mut Context<Self>) {
        // A pending save naming the archived container retires — an archive
        // must not leave its file behind. One naming a survivor (a batch
        // archive of parked containers while the shown canvas is dirty) lands
        // now: the forget's generation bump below would otherwise retire it
        // unlanded.
        let pending_container = self
            .pending_save
            .as_ref()
            .map(|(_, pending, _)| pending.clone());
        match pending_container.as_deref() {
            Some(pending) if pending == container_id => self.pending_save = None,
            Some(_) => self.flush_pending_save(),
            None => {}
        }
        // The container's pi sessions must be read before the state forgets
        // the layouts that name them.
        let pi_sheets = self.container_pi_sheets(container_id);
        let shown = self.host.forget_container(container_id);
        self.prune_terminals();
        for sheet in pi_sheets {
            self.close_pi_sheet(&sheet, cx);
        }
        if shown {
            // The archived container owned the shown canvas; the shell falls
            // back to the scratch surface until the workspace lands elsewhere.
            self.show_scratch(cx);
        } else {
            cx.notify();
        }
    }

    /// Whether the canvas holds any state for `container_id` — its shown
    /// canvas or a parked copy. The workspace's dead-foreground sync consults
    /// this before forgetting, so an idle frame does no work.
    pub(crate) fn holds_container(&self, container_id: &str) -> bool {
        self.host.active_container() == Some(container_id)
            || self.host.held().contains(container_id)
    }

    /// The terminal sheets' live PTY views (test-support read face).
    #[cfg(feature = "test-support")]
    pub(crate) fn host_terminals(&self) -> &HashMap<String, Entity<steer_ghostty::GhosttyView>> {
        &self.terminals
    }

    /// The pi sheets' live body views (test-support read face).
    #[cfg(feature = "test-support")]
    pub(crate) fn host_pi_views(&self) -> &HashMap<String, Entity<PiSessionView>> {
        &self.pi_views
    }
}

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_canvas(window, cx)
    }
}

/// The v1 sheet registry, wired with the conversation entity (session titles
/// and the session restore check read its folded book) and the pi store.
pub(crate) fn registry(
    chat: Entity<ChatColumn>,
    pi_store: Entity<AcpStore>,
) -> steer_canvas::SheetRegistry {
    v1_registry(chat, pi_store)
}

/// The session kind's restore face: a session sheet survives a layout restore
/// unless its session is POSITIVELY known retired. The read mirrors the
/// container death predicate's fold half, deliberately conservative — a
/// session the fold, the catalogue and the host's thread row have all not met
/// yet (a restore racing the catalogue's first walk) reads alive, so a
/// not-yet-loaded book never prunes a live session's sheet.
fn session_sheet_resource_alive(chat: &Entity<ChatColumn>, sheet: &str, cx: &App) -> bool {
    use ahp_types::state::SessionStatus;

    let Some(session) = steer_canvas::id::session_of_sheet(sheet) else {
        return true;
    };
    let archived = |status: u32| status & SessionStatus::IsArchived.bits() != 0;
    let chat = chat.read(cx);
    if let Some((store, _)) = chat.store.as_ref() {
        let book = &store.read(cx).book;
        if let Some(state) = book.sessions.get(session)
            && state.status != 0
        {
            return !archived(state.status);
        }
        if let Some(summary) = book.summaries.get(session)
            && summary.status != 0
        {
            return !archived(summary.status);
        }
    }
    // The host's own thread row (the catalogue's local mirror): a session
    // archived in a row this client wrote is retired even before the fold
    // meets it again.
    !manox_agent::thread_store::try_global()
        .and_then(|store| store.read(|state| state.summary_by_id(session).map(|row| row.archived)))
        .unwrap_or(false)
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

/// Writes one canvas snapshot to its container's layout file (the save
/// pump's only write, debounced or flushed).
fn write_layout_snapshot(container_id: &str, snapshot: &CanvasSnapshot) {
    let file: persist::CanvasFile =
        persist::CanvasFile::new(container_id, snapshot.layout.clone(), snapshot.zoom.clone());
    if let Err(error) = persist::write_canvas(&file) {
        tracing::warn!(%error, "canvas layout write failed");
    }
}
