//! The right dock's tool kinds (PLAN-ABSORB-08B): webview / editor / diff /
//! agent / subagent sheets — one store keyed by sheet id, and the
//! [`SheetKindSpec`]s the canvas registry registers.
//!
//! The state rule is the one the canvas model already set: **a body is a
//! view, the store owns the state.** Switching tabs (or parking a canvas)
//! unmounts a body; whatever a tool remembers lives here, keyed by sheet
//! id, and each body factory mints-or-reuses per id. The persistence story
//! collapses with it: the container's canvas layout file is the only
//! durable record — a restored layout names its sheets and each `restore`
//! face answers whether the resource behind the id still exists (the
//! in-memory store's presence, a file on disk). The pane snapshots and
//! stashes this store replaces are gone, not migrated.
//!
//! Visibility: a webview is an OS-level child window — it ignores CSS
//! clipping, so its window exists ONLY while its sheet is displayed
//! ([`CanvasPaneStore::note_shown`] hides the rest). An editor, a CLI
//! picker or a diff can sit on a `near` page harmlessly.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
use std::sync::Mutex;

use gpui::prelude::*;
use gpui::{App, Entity, SharedString, WeakEntity, Window};
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
use steer_canvas::id::webview_resource;
use steer_canvas::id::{agent_resource, diff_resource, editor_resource};
use steer_canvas::registry::{ParsedSheet, SheetKindSpec};

use crate::Workspace;

/// The right dock's per-sheet state. One entity the canvas holds; every
/// body factory mints-or-reuses through it, so a body rebuild is never a
/// state reset.
pub struct CanvasPaneStore {
    ws: WeakEntity<Workspace>,
    mux: Entity<crate::multiplexer::SessionMultiplexer>,
    /// The canvas view: the webview ticker's repaint target (its page
    /// `<title>` lands on a tab strip; browser-platform-only).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    notify: Option<WeakEntity<super::workspace::canvas_view::CanvasView>>,
    webviews: HashMap<String, WebviewSheet>,
    editors: HashMap<String, Entity<gpui_component::input::EditorState>>,
    diffs: HashMap<String, Entity<DiffPaneView>>,
    subagents: HashMap<String, SubagentPane>,
    agents: HashMap<String, Entity<super::tool_tabs::AgentPicker>>,
}

/// The webview slot's state — browser-platform-only (the embedded browser
/// compiles out on Linux/FreeBSD with the kind, so the twin there is the
/// unit type and the slot map stays empty by construction).
#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
struct WebviewSheet {
    /// The app-side browser tab id, minted at the body's first build (the
    /// host routes IPC by it); `None` until then.
    tab_id: Arc<Mutex<Option<manox_agent::thread_engine::BrowserTabId>>>,
    /// The URL the sheet opened at, kept in step with navigation (the next
    /// launch's restore).
    url: Arc<Mutex<String>>,
    /// The page's `<title>` (the ticker mirrors it onto the tab).
    title: Arc<Mutex<String>>,
    ticker_alive: Arc<std::sync::atomic::AtomicBool>,
    ticker_visible: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct WebviewSheet;

struct SubagentPane {
    panel: Entity<crate::views::subagent_panel::SubagentPanel>,
    label: String,
}

impl CanvasPaneStore {
    pub fn new(
        ws: WeakEntity<Workspace>,
        mux: Entity<crate::multiplexer::SessionMultiplexer>,
    ) -> Self {
        Self {
            ws,
            mux,
            notify: None,
            webviews: HashMap::new(),
            editors: HashMap::new(),
            diffs: HashMap::new(),
            subagents: HashMap::new(),
            agents: HashMap::new(),
        }
    }

    /// The repaint target for the faces that change what a tab strip reads
    /// (a page title). Set by the canvas at mount.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn set_notify(
        &mut self,
        notify: WeakEntity<super::workspace::canvas_view::CanvasView>,
    ) {
        self.notify = Some(notify);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub(crate) fn set_notify(
        &mut self,
        _notify: WeakEntity<super::workspace::canvas_view::CanvasView>,
    ) {
    }

    /// The webview slot for `sheet`, minted at `url` when absent (the
    /// browser-platform faces' helper; the Linux twin has no slots).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn webview_slot(&mut self, sheet: &str, url: &str) -> &mut WebviewSheet {
        self.webviews
            .entry(sheet.to_string())
            .or_insert_with(|| WebviewSheet {
                tab_id: Arc::new(Mutex::new(None)),
                url: Arc::new(Mutex::new(url.to_string())),
                title: Arc::new(Mutex::new(String::new())),
                ticker_alive: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                ticker_visible: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            })
    }

    /// The webview's body view: mint-or-reuse the browser tab behind the
    /// sheet, routed through the workspace's own `browser_views` map. A
    /// refused embed (no usable webview) answers `None` — the caller keeps
    /// the sheet frame empty rather than minting a dead tab.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn webview_view(
        &mut self,
        sheet: &str,
        url: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<crate::views::browser_view::BrowserView>> {
        let ws = self.ws.upgrade()?;
        let preset = self
            .webviews
            .get(sheet)
            .and_then(|slot| *slot.tab_id.lock().expect("webview tab id lock"));
        let built = ws.update(cx, |ws, cx| {
            let tab_id = match preset {
                Some(tab_id) => Some(tab_id),
                None => ws.restore_browser_tab(url, window, cx),
            }?;
            let view = ws
                .browser_views
                .get(&tab_id)
                .cloned()
                .expect("the tab's view is registered in the workspace");
            Some((tab_id, view))
        });
        let (tab_id, view) = built?;
        if let Some(slot) = self.webviews.get_mut(sheet) {
            *slot.tab_id.lock().expect("webview tab id lock") = Some(tab_id);
        }
        Some(view)
    }

    /// Opens (or adopts) a browser tab as a webview sheet: the host already
    /// minted and routed the tab (the agent's browser-open path); the
    /// sheet's slot records the tab id and the URL the next launch
    /// restores.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn adopt_webview(
        &mut self,
        sheet: &str,
        url: &str,
        tab_id: manox_agent::thread_engine::BrowserTabId,
    ) {
        let slot = self.webview_slot(sheet, url);
        *slot.tab_id.lock().expect("webview tab id lock") = Some(tab_id);
        *slot.url.lock().expect("webview url lock") = url.to_string();
    }

    /// The browser tab a sheet displays, when its body was built.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn webview_tab_id(&self, sheet: &str) -> Option<manox_agent::thread_engine::BrowserTabId> {
        *self
            .webviews
            .get(sheet)?
            .tab_id
            .lock()
            .expect("webview tab id lock")
    }

    /// A webview sheet's URL (the open face names the next sheet's start
    /// page from it).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn webview_url(&self, sheet: &str) -> Option<String> {
        self.webviews
            .get(sheet)
            .map(|slot| slot.url.lock().expect("webview url lock").clone())
    }

    /// A webview sheet's live page title (the ticker's write).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub(crate) fn webview_page_title(&self, sheet: &str) -> Option<String> {
        let slot = self.webviews.get(sheet)?;
        let title = slot.title.lock().expect("webview title lock").clone();
        (!title.is_empty()).then_some(title)
    }

    /// Records the displayed sheet set: the webviews' OS windows follow it —
    /// a hidden tab's window would float above everything otherwise. (The
    /// OS-window faces ride the browser platform; on Linux/FreeBSD the
    /// webview kind is not registered, so this loop never has slots.)
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn note_shown(&mut self, shown: HashSet<String>, cx: &mut App) {
        for (sheet, slot) in &self.webviews {
            let visible = shown.contains(sheet);
            slot.ticker_visible
                .store(visible, std::sync::atomic::Ordering::Relaxed);
            if let (Some(tab_id), Some(ws)) = (
                *slot.tab_id.lock().expect("webview tab id lock"),
                self.ws.upgrade(),
            ) {
                ws.update(cx, |ws, cx| {
                    if let Some(view) = ws.browser_views.get(&tab_id)
                        && let Some(webview) = view.read(cx).webview().cloned()
                    {
                        webview.update(cx, |w, _| {
                            if visible {
                                w.show();
                            } else {
                                w.hide();
                            }
                        });
                    }
                });
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn note_shown(&mut self, _shown: HashSet<String>, _cx: &mut App) {}

    /// The webview ticker for a sheet (the page `<title>` / URL mirror):
    /// started when the sheet's body is first built; it dies with the slot.
    /// The tab id it polls is the slot's own — re-read each tick, because
    /// the first build may have minted it after the ticker started.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn start_webview_ticker(&mut self, sheet: &str, cx: &mut App) {
        let Some(slot) = self.webviews.get(sheet) else {
            return;
        };
        let alive = slot.ticker_alive.clone();
        let visible = slot.ticker_visible.clone();
        let title_slot = slot.title.clone();
        let url_slot = slot.url.clone();
        let tab_slot = slot.tab_id.clone();
        let ws = self.ws.clone();
        let Some(notify) = self.notify.clone() else {
            return;
        };
        cx.spawn(async move |cx| {
            while alive.load(std::sync::atomic::Ordering::Relaxed) {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                if !alive.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                if !visible.load(std::sync::atomic::Ordering::Relaxed) {
                    continue;
                }
                let Some(host) = crate::browser_host::WorkspaceBrowserHost::concrete() else {
                    continue;
                };
                let Some(tab_id) = *tab_slot.lock().expect("webview tab id lock") else {
                    continue;
                };
                let Ok(title) = cx.update(|cx| host.page_title(tab_id, cx)).await else {
                    continue;
                };
                let changed = {
                    let mut slot = title_slot.lock().expect("webview title lock");
                    if *slot == title {
                        false
                    } else {
                        *slot = title;
                        true
                    }
                };
                let Ok(Some(url)) = ws.read_with(cx, |ws, cx| {
                    ws.browser_views
                        .get(&tab_id)
                        .map(|view| view.read(cx).url().to_string())
                }) else {
                    continue;
                };
                *url_slot.lock().expect("webview url lock") = url;
                if changed && let Some(canvas) = notify.upgrade() {
                    // The canvas entity's notify is the repaint signal: the
                    // tab strip reads the title each frame.
                    canvas.update(cx, |_, cx| cx.notify());
                }
            }
        })
        .detach();
    }

    /// The editor sheet's state: mint-or-reuse (a tab switch never resets a
    /// draft).
    #[cfg_attr(any(target_os = "linux", target_os = "freebsd"), allow(dead_code))]
    pub(crate) fn editor_view(
        &mut self,
        sheet: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<gpui_component::input::EditorState> {
        self.editors
            .entry(sheet.to_string())
            .or_insert_with(|| {
                cx.new(|cx| {
                    gpui_component::input::EditorState::new(window, cx)
                        .language("markdown")
                        .line_number(true)
                        .folding(false)
                        .soft_wrap(true)
                        .submit_on_enter(false)
                        .placeholder(crate::i18n::t("chrome-editor-placeholder"))
                })
            })
            .clone()
    }

    /// The CLI sheet's title: the picked agent's display name once picked.
    pub(crate) fn agent_title(&self, sheet: &str, cx: &App) -> Option<String> {
        let picker = self.agents.get(sheet)?;
        Some(match picker.read(cx).picked_display() {
            Some(display) => display.to_string(),
            None => crate::i18n::t("canvas-tab-agent").to_string(),
        })
    }

    /// The sub-agent sheet's label (the run's task topic).
    pub(crate) fn subagent_label(&self, sheet: &str) -> Option<String> {
        self.subagents.get(sheet).map(|pane| pane.label.clone())
    }

    /// Mounts a sub-agent observation panel under its sheet (the workspace's
    /// child-event face owns the panel; the store only remembers it).
    pub fn put_subagent(
        &mut self,
        sheet: &str,
        label: &str,
        panel: Entity<crate::views::subagent_panel::SubagentPanel>,
    ) {
        self.subagents.insert(
            sheet.to_string(),
            SubagentPane {
                panel,
                label: label.to_string(),
            },
        );
    }

    /// The observation panel a sheet holds, if any.
    pub fn subagent_panel(
        &self,
        sheet: &str,
    ) -> Option<Entity<crate::views::subagent_panel::SubagentPanel>> {
        self.subagents.get(sheet).map(|pane| pane.panel.clone())
    }

    /// Frees a closed sheet's state: the webview's browser tab closes (its
    /// IPC route and webview go with it), the CLI's kill handle fires, the
    /// editor/panel entities drop.
    pub fn reclaim(&mut self, sheet: &str, cx: &mut App) {
        if let Some(slot) = self.webviews.remove(sheet) {
            slot.ticker_alive
                .store(false, std::sync::atomic::Ordering::Relaxed);
            self.close_webview_tab(&slot, cx);
        }
        if self.agents.remove(sheet).is_some() {
            // The picker may have spawned: the kill handle rides the
            // registry under the sheet id either way.
            if let Some(ws) = self.ws.upgrade() {
                ws.update(cx, |ws, _| {
                    ws.kill_handle_for_tab(sheet);
                });
            }
        }
        self.editors.remove(sheet);
        self.diffs.remove(sheet);
        self.subagents.remove(sheet);
    }

    /// Frees every sheet of a dead container: the kill handles fire (a CLI
    /// child outliving its container is the bug the registry exists to end)
    /// and the webviews close through their host routes.
    pub fn reclaim_container(&mut self, container_id: &str, cx: &mut App) {
        let doomed: Vec<String> = self
            .webviews
            .keys()
            .chain(self.agents.keys())
            .filter(|sheet| {
                steer_canvas::id::sheet_container(sheet).as_deref() == Some(container_id)
            })
            .cloned()
            .collect();
        for sheet in doomed {
            self.reclaim(&sheet, cx);
        }
    }

    /// Prunes the store against the sheets the canvases still hold (the
    /// same union the terminal PTYs prune against): a closed sheet's state
    /// ends, a parked canvas's state survives.
    pub fn prune(&mut self, kept: &HashSet<String>, cx: &mut App) {
        let doomed: Vec<String> = self
            .webviews
            .keys()
            .chain(self.editors.keys())
            .chain(self.diffs.keys())
            .chain(self.subagents.keys())
            .chain(self.agents.keys())
            .filter(|sheet| !kept.contains(*sheet))
            .cloned()
            .collect();
        for sheet in doomed {
            self.reclaim(&sheet, cx);
        }
    }

    /// The webview sheet's body view: mint-or-reuse behind it, and the
    /// ticker on first build. A refused embed (no usable webview) answers
    /// `None` — the sheet frame stays empty.
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn webview_body(
        &mut self,
        sheet: &str,
        url: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui::AnyView> {
        let fresh = self
            .webviews
            .get(sheet)
            .map(|slot| slot.tab_id.lock().expect("webview tab id lock").is_none())
            .unwrap_or(false);
        let view = self.webview_view(sheet, url, window, cx)?;
        if fresh {
            self.start_webview_ticker(sheet, cx);
        }
        Some(view.into())
    }

    /// The CLI sheet's pane (mint-or-reuse behind it); `None` without a
    /// multiplexer (no wire models to pick from).
    fn agent_body(&mut self, sheet: &str, cx: &mut App) -> Entity<super::tool_tabs::AgentPicker> {
        self.agent_pane(sheet, self.mux.clone(), cx)
    }

    /// The diff sheet's pane: the open face hands the parsed state in (the
    /// view holds the state entity, never the workspace lease).
    pub(crate) fn put_diff(
        &mut self,
        sheet: &str,
        title: SharedString,
        state: Result<Entity<gpui_component::diff::DiffState>, SharedString>,
        cx: &mut App,
    ) {
        let view = cx.new(|_| DiffPaneView { title, state });
        self.diffs.insert(sheet.to_string(), view);
    }

    /// The diff sheet's pane view, when the open face minted it.
    pub(crate) fn diff_pane(&self, sheet: &str) -> Option<Entity<DiffPaneView>> {
        self.diffs.get(sheet).cloned()
    }

    /// The diff sheet keys this store holds (the diagnostic probe).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_diff_keys(&self) -> Vec<String> {
        self.diffs.keys().cloned().collect()
    }

    /// The agent sheet keys this store holds (the diagnostic probe).
    #[cfg(feature = "test-support")]
    pub fn diagnostic_agent_keys(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }

    /// The diff sheet's title + parsed file count (the diagnostic read).
    #[cfg(feature = "test-support")]
    pub(crate) fn diagnostic_diff(&self, sheet: &str, cx: &App) -> Option<(String, usize)> {
        let view = self.diffs.get(sheet)?;
        let files = view.read(cx).state.as_ref().ok()?.read(cx).files().len();
        Some((view.read(cx).title.to_string(), files))
    }

    /// The diff sheet's pane title (the patch's own).
    pub(crate) fn diff_title(&self, sheet: &str, cx: &App) -> Option<String> {
        self.diffs
            .get(sheet)
            .map(|view| view.read(cx).title.to_string())
    }

    /// Whether any sheet of `prefix` holds state — kept for symmetry with
    /// the other kinds; the diff sheets ride `diffs`.
    pub fn has_kind(&self, prefix: &str) -> bool {
        match prefix {
            "webview" => !self.webviews.is_empty(),
            "editor" => !self.editors.is_empty(),
            "subagent" => !self.subagents.is_empty(),
            "agent" => !self.agents.is_empty(),
            "diff" => !self.diffs.is_empty(),
            _ => false,
        }
    }

    /// Drops a webview slot's browser tab (the platform-gated half of
    /// [`Self::reclaim`] — the browser face compiles out on
    /// Linux/FreeBSD, where no webview slot can exist).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    fn close_webview_tab(&self, slot: &WebviewSheet, cx: &mut App) {
        if let (Some(tab_id), Some(ws)) = (
            slot.tab_id.lock().expect("webview tab id lock").take(),
            self.ws.upgrade(),
        ) {
            ws.update(cx, |ws, cx| ws.close_browser_tab(tab_id, cx));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn close_webview_tab(&self, _slot: &WebviewSheet, _cx: &mut App) {}

    /// The agent picker a sheet rides (minted on first show): the picker IS
    /// the CLI sheet's record — the spawn's view and kill handle live
    /// inside it.
    fn agent_pane(
        &mut self,
        sheet: &str,
        mux: Entity<crate::multiplexer::SessionMultiplexer>,
        cx: &mut App,
    ) -> Entity<super::tool_tabs::AgentPicker> {
        self.agents
            .entry(sheet.to_string())
            .or_insert_with(|| cx.new(|_| super::tool_tabs::AgentPicker::for_sheet(sheet, mux)))
            .clone()
    }
}

/// The five right-dock kinds' specs. The store owns every body's state (the
/// diff's parsed patch rides the open gesture into its sheet's view).
pub fn pane_kinds(store: Entity<CanvasPaneStore>) -> Vec<SheetKindSpec> {
    // The specs borrow the store for their whole life: the Arc'd closures
    // outlive the function call, so they take their own handles.
    // The webview kind is browser-platform-only: the embedded browser
    // compiles out on Linux/FreeBSD with the rest of the browser face, so
    // the kind is NOT REGISTERED there at all (no entry — the same
    // clean-refusal shape the capability face keeps).
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let webview = Some({
        let webview_title_store = store.clone();
        let webview_body_store = webview_title_store.clone();
        SheetKindSpec {
            kind: "webview",
            // The page's `<title>` (the ticker mirrors it); the i18n
            // fallback names a not-yet-titled page.
            title: Arc::new(move |sheet, cx| {
                webview_title_store
                    .read(cx)
                    .webview_page_title(sheet)
                    .map(SharedString::from)
                    .unwrap_or_else(|| crate::i18n::t("chrome-tab-browser"))
            }),
            tab_title: None,
            glyph: "frame",
            closable: true,
            names_container: false,
            // The layout is the fact: a webview sheet re-opens at its URL.
            restore: None,
            parse: Some(Arc::new(|id| {
                let resource = webview_resource(id);
                ParsedSheet {
                    container_id: resource.as_ref().map(|r| r.container_id.clone()),
                    instance: resource.map(|r| r.index.to_string()).unwrap_or_default(),
                }
            })),
            body: Some(Arc::new(move |sheet, window, cx| {
                // One update: the URL read and the body build share it.
                let url = crate::views::browser_view::DEFAULT_URL.to_string();
                match webview_body_store.update(cx, |store, cx| {
                    let url = store.webview_url(sheet).unwrap_or(url);
                    store.webview_body(sheet, &url, window, cx)
                }) {
                    Some(view) => view,
                    None => empty_body(cx),
                }
            })),
        }
    });
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let webview = Option::<SheetKindSpec>::None;

    let editor_store = store.clone();
    let editor = SheetKindSpec {
        kind: "editor",
        title: Arc::new(|_, _| crate::i18n::t("chrome-tab-editor")),
        tab_title: None,
        glyph: "file",
        closable: true,
        names_container: false,
        // The resource is the FILE: a restored editor sheet survives only
        // while its path exists.
        restore: Some(Arc::new(|sheet, _| {
            editor_resource(sheet)
                .map(|resource| std::path::Path::new(&resource.path).exists())
                .unwrap_or(false)
        })),
        parse: Some(Arc::new(|id| {
            let resource = editor_resource(id);
            ParsedSheet {
                container_id: resource.as_ref().map(|r| r.container_id.clone()),
                instance: resource.map(|r| r.path.clone()).unwrap_or_default(),
            }
        })),
        body: Some(Arc::new(move |sheet, window, cx| {
            let view = editor_store.update(cx, |store, cx| store.editor_view(sheet, window, cx));
            view.into()
        })),
    };

    let diff_title_store = store.clone();
    let diff_pane_store = diff_title_store.clone();
    let diff = SheetKindSpec {
        kind: "diff",
        // The parsed patch lives with the sheet (the open face hands it
        // in); the title is the patch's own.
        title: Arc::new(move |sheet, cx| {
            diff_title_store
                .read(cx)
                .diff_title(sheet, cx)
                .map(SharedString::from)
                .unwrap_or_else(|| crate::i18n::t("chrome-tab-diff"))
        }),
        tab_title: None,
        glyph: "code",
        closable: true,
        names_container: false,
        // The patch is the fact (its content rides the open gesture).
        restore: None,
        parse: Some(Arc::new(|id| {
            let resource = diff_resource(id);
            ParsedSheet {
                container_id: resource.as_ref().map(|r| r.container_id.clone()),
                instance: resource
                    .map(|r| format!("{}~{}", r.old, r.new))
                    .unwrap_or_default(),
            }
        })),
        body: Some(Arc::new(move |sheet, _, cx| {
            match diff_pane_store.read(cx).diff_pane(sheet) {
                Some(view) => view.into(),
                None => empty_body(cx),
            }
        })),
    };

    let agent_title_store = store.clone();
    let agent_restore_store = agent_title_store.clone();
    let agent_body_store = agent_restore_store.clone();
    let agent = SheetKindSpec {
        kind: "agent",
        // The picker's heading names the CLI once picked; before that the
        // sheet says what it is.
        title: Arc::new(move |sheet, cx| {
            agent_title_store
                .read(cx)
                .agent_title(sheet, cx)
                .map(SharedString::from)
                .unwrap_or_else(|| crate::i18n::t("canvas-tab-agent"))
        }),
        tab_title: None,
        glyph: "terminal",
        closable: true,
        names_container: false,
        // The CLI child is in-memory state: a layout naming an agent sheet
        // survives a restart only if the store still holds the child (it
        // does not) — the sheet prunes away.
        restore: Some(Arc::new(move |sheet, cx| {
            agent_restore_store.read(cx).holds(sheet)
        })),
        parse: Some(Arc::new(|id| {
            let resource = agent_resource(id);
            ParsedSheet {
                container_id: resource.as_ref().map(|r| r.container_id.clone()),
                instance: resource.map(|r| r.index.to_string()).unwrap_or_default(),
            }
        })),
        body: Some(Arc::new(move |sheet, _, cx| {
            agent_body_store
                .update(cx, |store, cx| store.agent_body(sheet, cx))
                .into()
        })),
    };

    let subagent_title_store = store.clone();
    let subagent_restore_store = subagent_title_store.clone();
    let subagent_body_store = subagent_restore_store.clone();
    let subagent = SheetKindSpec {
        kind: "subagent",
        // The label is the run's task topic (the panel's banner shows it
        // too).
        title: Arc::new(move |sheet, cx| {
            subagent_title_store
                .read(cx)
                .subagent_label(sheet)
                .map(SharedString::from)
                .unwrap_or_else(|| crate::i18n::t("canvas-tab-subagent"))
        }),
        tab_title: None,
        glyph: "bot",
        closable: true,
        names_container: false,
        // A child transcript does not survive the process.
        restore: Some(Arc::new(move |sheet, cx| {
            subagent_restore_store.read(cx).holds(sheet)
        })),
        parse: None,
        body: Some(Arc::new(move |sheet, _, cx| {
            match subagent_body_store.read(cx).subagent_panel(sheet) {
                Some(panel) => panel.into(),
                None => empty_body(cx),
            }
        })),
    };

    // The webview kind joins only where the embedded browser compiles in
    // (the other four are platform-neutral).
    let mut kinds = vec![editor, diff, agent, subagent];
    if let Some(webview) = webview {
        kinds.insert(0, webview);
    }
    kinds
}

impl CanvasPaneStore {
    /// Whether the store holds any state for `sheet` (the agent/subagent
    /// restore face: the in-memory presence IS the resource).
    fn holds(&self, sheet: &str) -> bool {
        self.webviews.contains_key(sheet)
            || self.editors.contains_key(sheet)
            || self.diffs.contains_key(sheet)
            || self.subagents.contains_key(sheet)
            || self.agents.contains_key(sheet)
    }
}

/// The body a sheet shows while its own view is absent (a CLI before its
/// multiplexer, a webview on the platforms without one).
fn empty_body(cx: &mut App) -> gpui::AnyView {
    cx.new(|_| EmptyPane).into()
}

/// The empty pane body.
struct EmptyPane;

impl gpui::Render for EmptyPane {
    fn render(
        &mut self,
        _window: &mut Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div().size_full()
    }
}

/// The diff body: a view holding the parsed patch state directly (the open
/// face replaces the entity when a new patch arrives — no workspace lease).
pub(crate) struct DiffPaneView {
    title: SharedString,
    state: Result<Entity<gpui_component::diff::DiffState>, SharedString>,
}

impl gpui::Render for DiffPaneView {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::IntoElement as _;
        use gpui::{ParentElement as _, Styled as _, div, px};
        use gpui_component::ActiveTheme as _;
        match &self.state {
            Ok(state) => div()
                .id("canvas-diff-pane")
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .debug_selector(|| "canvas-diff-pane".into())
                .child(gpui_component::diff::Diff::new(state))
                .into_any_element(),
            Err(e) => div()
                .w_full()
                .p(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(e.clone())
                .into_any_element(),
        }
    }
}
