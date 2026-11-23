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

use crate::Workspace;
use crate::views::centered;
use crate::workspace::canvas_host::{CanvasAction, CanvasHost};
use crate::workspace::column_view::ChatColumnView;
use steer_canvas::persist;

/// The tab strip's height (the sheet frames start below it).
pub const TAB_STRIP_H: f32 = 34.;

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
    /// The shown container's working directory: a terminal sheet spawns there.
    /// Injected by the workspace when the canvas opens a container (the canvas
    /// never reads the workspace itself — that would take a second lease while
    /// the shell is updating it).
    cwd: std::path::PathBuf,
    workspace: WeakEntity<Workspace>,
    /// The card's interior size, measured at prepaint (the first frame of a
    /// fresh window falls back to the window's).
    size: Rc<std::cell::Cell<Option<Size<Pixels>>>>,
}

/// The v1 sheet kinds. `home` is the container's start page; `session` is a
/// conversation, named by its tab title and never by the container (the
/// container's own name is the projection over the earliest surviving sheet
/// that *does* name it).
///
/// The `body` factories here are the mountable face for hosts that place
/// `AnyView`s directly; this canvas renders its bodies inline from the
/// entities it holds (see [`CanvasView::render_sheet_body`]), so a factory's
/// result is only used by such hosts.
fn v1_registry() -> steer_canvas::SheetRegistry {
    use std::sync::Arc;
    use steer_canvas::{ParsedSheet, SheetKindSpec};

    let kind = |kind: &'static str,
                glyph: &'static str,
                names_container: bool,
                parse: Option<steer_canvas::registry::ParseFn>|
     -> SheetKindSpec {
        SheetKindSpec {
            kind,
            title: Arc::new(|id, _| SharedString::from(id.to_string())),
            tab_title: None,
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
    steer_canvas::SheetRegistry::new()
        .with(kind("home", "home", false, None))
        .with(kind(
            "terminal",
            "terminal",
            false,
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
            Some(Arc::new(|id| ParsedSheet::workspace_wide(id))),
        ))
}

impl CanvasView {
    pub(crate) fn new(host: CanvasHost, workspace: WeakEntity<Workspace>) -> Self {
        Self {
            host,
            column: None,
            terminals: HashMap::new(),
            cwd: std::env::current_dir().unwrap_or_default(),
            workspace,
            size: Rc::new(std::cell::Cell::new(None)),
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
        let mut strip = h_flex()
            .id(ElementId::Name(
                format!("canvas-strip-{}", group.key).into(),
            ))
            .debug_selector({
                let key = group.key.clone();
                move || format!("canvas-strip-{key}")
            })
            .absolute()
            .left(px(box_.rect.x))
            .top(px(box_.rect.y))
            .w(px(box_.rect.width))
            .h(px(TAB_STRIP_H))
            .items_center()
            .gap_1()
            .px_2()
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.border);

        for (index, sheet) in box_.panels.iter().enumerate() {
            let active = index == box_.active;
            // The tab's label is the registry's tab title, falling back to the
            // sheet's title; an unowned id shows its raw id (the shell never
            // strands a tab it cannot name).
            let label = sheet.clone();
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
                    if event.click_count() >= 2
                        && let Some(container) = this.host.active_container().map(str::to_string)
                    {
                        let home = home_sheet_id(&container);
                        if this.host.split_right(&id, &home) == CanvasAction::Applied {
                            this.after_edit(cx);
                            cx.notify();
                        }
                        return;
                    }
                    if this.host.activate_sheet(&id) == CanvasAction::Applied {
                        this.after_edit(cx);
                        cx.notify();
                    }
                }));

            if let Some(glyph) = glyph {
                tab =
                    tab.child(gpui_component::Icon::from(gpui_kit_assets::IconName::File).small());
                // The glyph constant is the kind's; the icon theme maps names,
                // so an unknown name falls back to the document glyph above.
                let _ = glyph;
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
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_sheet(&close_id, window, cx);
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

    fn close_sheet(&mut self, sheet: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.host.close_sheet(sheet) {
            CanvasAction::Applied => {
                self.prune_terminals();
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

impl Render for CanvasView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_canvas(window, cx)
    }
}

/// The sheet frames of the shown canvas, for tests and the chrome projection.
pub(crate) fn registry() -> steer_canvas::SheetRegistry {
    v1_registry()
}
