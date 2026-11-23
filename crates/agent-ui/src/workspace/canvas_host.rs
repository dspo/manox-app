//! The canvas host: the app-side owner of the canvas model (`steer-canvas`) and
//! the sheet-kind registry.
//!
//! `CanvasState` is the geometry state machine — who is shown, what is parked,
//! which ticket is current. This module is the **host** around it: it mints
//! tickets for gestures, routes every layout write through the one writer, and
//! hands the caller a pending save to debounce. The renderer
//! (`canvas_view.rs`) only calls these commands and reads the layout back; it
//! never writes.
//!
//! Two rules keep the seam honest:
//!
//! - **one writer**: every gesture here takes a ticket first and commits with
//!   it; a stale commit (an older canvas's reply) is dropped by the state, not
//!   by the caller remembering to check.
//! - **the host owns no views**: kinds register in the [`SheetRegistry`]; a
//!   body's state lives with its kind, so switching tabs or parking a canvas
//!   never destroys it.

use gpui::App;
use steer_canvas::state::{CanvasSnapshot, CanvasState, SetLayoutOutcome, SwitchOutcome};
use steer_canvas::{DockLayout, SheetRegistry, layout};

/// How long a layout edit waits before it is written (the save pump's quiet
/// window). Bursts of tab switches collapse into one write.
pub const SAVE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(400);

/// What a gesture did, for the caller to react to (archive teardown, chrome
/// projection).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasAction {
    /// The layout moved.
    Applied,
    /// The commit was stale and was dropped.
    Stale,
    /// The container lost its last sheet: the caller archives it through the
    /// workspace's teardown face.
    AutoArchive(String),
}

/// The canvas host.
pub struct CanvasHost {
    state: CanvasState,
    registry: SheetRegistry,
    /// The generation of the newest edit: a scheduled save only writes when no
    /// newer edit landed while it waited.
    save_generation: u64,
}

impl CanvasHost {
    /// A host over a populated registry.
    pub fn new(registry: SheetRegistry) -> Self {
        Self {
            state: CanvasState::new(),
            registry,
            save_generation: 0,
        }
    }

    /// The registry (kind lookups, titles, glyphs, closability).
    pub fn registry(&self) -> &SheetRegistry {
        &self.registry
    }

    /// The shown canvas's layout.
    pub fn layout(&self) -> &DockLayout {
        self.state.layout()
    }

    /// The shown container, or `None` for the scratch canvas.
    pub fn active_container(&self) -> Option<&str> {
        self.state.active_container()
    }

    /// A container's title projection (the earliest surviving sheet whose kind
    /// names the container). `None` when nothing names it — the caller falls
    /// back to the session title / cwd tail.
    ///
    /// A kind with no title yet answers its tab's localized placeholder; the
    /// placeholder names a tab, never a container, so the projection skips it
    /// and keeps walking. The tab itself still shows the placeholder — this
    /// filter lives on the container projection only.
    pub fn container_title(&self, container_id: &str, cx: &App) -> Option<String> {
        self.state.container_title(container_id, |sheet| {
            let title = self.registry.container_title_of(sheet, cx)?;
            (title != crate::i18n::t("canvas-tab-new-chat")).then_some(title)
        })
    }

    /// The parked canvases (the LRU the shell's park gesture fills).
    pub fn held(&self) -> &steer_canvas::state::HeldLayouts {
        self.state.held()
    }

    /// A container's remembered sheet order (creation order), for evaluating
    /// restore verdicts ahead of a switch.
    pub fn sheet_order(&self, container_id: &str) -> &[String] {
        self.state.sheet_order(container_id).unwrap_or(&[])
    }

    /// The zoomed sheet, if any.
    pub fn zoomed(&self) -> Option<&str> {
        self.state.zoomed()
    }

    /// Opens (or displays) a sheet: appended to the group of `prefer`, else the
    /// first group.
    pub fn open_sheet(&mut self, sheet: &str, prefer: Option<&str>) -> CanvasAction {
        self.note(sheet);
        let container = self.state.canvas_key().to_string();
        let next = layout::append_sheet(self.state.layout(), sheet, prefer);
        self.commit(&container, next)
    }

    /// Closes a sheet.
    pub fn close_sheet(&mut self, sheet: &str) -> CanvasAction {
        let container = self.state.canvas_key().to_string();
        let next = layout::remove_sheet(self.state.layout(), sheet);
        self.commit(&container, next)
    }

    /// Opens `sheet` in a new group beside `beside`.
    pub fn split_right(&mut self, beside: &str, sheet: &str) -> CanvasAction {
        self.note(sheet);
        let container = self.state.canvas_key().to_string();
        let next = layout::split_right(self.state.layout(), beside, sheet);
        self.commit(&container, next)
    }

    /// Displays an already-open sheet.
    pub fn activate_sheet(&mut self, sheet: &str) -> CanvasAction {
        let container = self.state.canvas_key().to_string();
        let next = layout::activate_sheet(self.state.layout(), sheet);
        self.commit(&container, next)
    }

    /// Shows `container_id`'s canvas, restoring the parked copy or `saved`.
    pub fn switch_container(
        &mut self,
        container_id: &str,
        saved: Option<CanvasSnapshot>,
        alive: impl Fn(&str) -> bool,
    ) -> SwitchOutcome {
        let outcome = self.state.switch_container(container_id, saved, alive);
        if outcome == SwitchOutcome::Switched {
            self.save_generation += 1;
        }
        outcome
    }

    /// Shows the scratch canvas.
    pub fn show_scratch(&mut self) -> SwitchOutcome {
        self.state.show_scratch()
    }

    /// Turns the scratch canvas into `container_id`'s canvas: its sheets move
    /// over (the scratch start page becomes the container's own), so minting a
    /// container from the launcher never loses the tab the user was on.
    pub fn migrate_scratch(&mut self, container_id: &str) {
        self.state.migrate_scratch(container_id);
        self.save_generation += 1;
    }

    /// Drops everything kept for a gone container. Answers whether it was shown
    /// (the caller then switches somewhere else).
    ///
    /// The generation bump retires every save scheduled before the forget:
    /// an archive request opens the start page and schedules a write, so a
    /// confirmed archive must not leave that container a home-page-only
    /// layout file behind (it would haunt the container's revival).
    pub fn forget_container(&mut self, container_id: &str) -> bool {
        let shown = self.state.forget_container(container_id);
        self.save_generation += 1;
        shown
    }

    /// The shown canvas as it would be saved.
    pub fn snapshot(&self) -> CanvasSnapshot {
        self.state.snapshot()
    }

    /// The newest edit's generation.
    pub fn save_generation(&self) -> u64 {
        self.save_generation
    }

    /// The shown canvas as it should be written, stamped with its generation.
    /// `None` on the scratch canvas (never persisted).
    pub fn pending_save(&self) -> Option<(u64, String, CanvasSnapshot)> {
        let container_id = self.state.active_container()?.to_string();
        Some((self.save_generation, container_id, self.snapshot()))
    }

    fn note(&mut self, sheet: &str) {
        let container = self.state.canvas_key().to_string();
        self.state.note_sheet_opened(&container, sheet);
    }

    /// The single write path: takes a ticket, commits, and reports what
    /// happened.
    fn commit(&mut self, container_id: &str, next: DockLayout) -> CanvasAction {
        let ticket = self.state.take_ticket();
        match self.state.set_layout(container_id, next, ticket) {
            SetLayoutOutcome::Applied => {
                self.save_generation += 1;
                CanvasAction::Applied
            }
            SetLayoutOutcome::Stale => CanvasAction::Stale,
            SetLayoutOutcome::AutoArchive(container_id) => CanvasAction::AutoArchive(container_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> CanvasHost {
        let mut host = CanvasHost::new(SheetRegistry::new());
        host.switch_container("c1", None, |_| true);
        host
    }

    #[test]
    fn opening_a_sheet_lands_in_the_shown_layout() {
        let mut host = host();
        assert_eq!(host.open_sheet("session:s1", None), CanvasAction::Applied);
        assert_eq!(layout::panel_ids(host.layout()), ["home:c1", "session:s1"]);
    }

    #[test]
    fn opening_the_same_sheet_twice_keeps_one_tab() {
        let mut host = host();
        host.open_sheet("session:s1", None);
        host.open_sheet("session:s1", None);
        assert_eq!(layout::panel_ids(host.layout()), ["home:c1", "session:s1"]);
    }

    #[test]
    fn closing_the_last_sheet_asks_for_archive() {
        let mut host = host();
        assert_eq!(
            host.close_sheet("home:c1"),
            CanvasAction::AutoArchive("c1".to_string())
        );
        assert_eq!(layout::sheet_count(host.layout()), 0);
    }

    #[test]
    fn a_split_opens_beside_the_shown_sheet() {
        let mut host = host();
        assert_eq!(
            host.split_right("home:c1", "terminal:c1:1"),
            CanvasAction::Applied
        );
        assert_eq!(
            layout::active_panel_ids(host.layout(), None),
            ["home:c1", "terminal:c1:1"]
        );
    }

    #[test]
    fn switching_away_parks_the_canvas_and_coming_back_restores_it() {
        let mut host = host();
        host.open_sheet("session:s1", None);
        host.switch_container("c2", None, |_| true);
        assert!(host.held().contains("c1"));
        host.switch_container("c1", None, |_| true);
        assert_eq!(layout::panel_ids(host.layout()), ["home:c1", "session:s1"]);
    }

    #[test]
    fn an_archived_container_loses_its_bookkeeping() {
        let mut host = host();
        host.open_sheet("session:s1", None);
        host.switch_container("c2", None, |_| true);
        assert!(host.held().contains("c1"));
        assert!(!host.forget_container("c1"));
        assert!(!host.held().contains("c1"));
        assert!(host.forget_container("c2"));
        assert_eq!(host.active_container(), None);
    }

    #[test]
    fn every_edit_bumps_the_save_generation() {
        let mut host = host();
        let before = host.save_generation();
        host.open_sheet("session:s1", None);
        assert!(host.save_generation() > before);
        // The scratch canvas has nothing to write.
        host.show_scratch();
        assert_eq!(host.pending_save(), None);
    }

    /// The container-title projection skips a kind's untitled placeholder:
    /// the localized placeholder names a tab, never a container, so the
    /// projection answers None (the row keeps its catalogue title) while the
    /// registry's own title face — the tab's — still shows it. A real title
    /// names the container again.
    #[gpui::test]
    fn an_untitled_session_names_no_container(cx: &mut gpui::TestAppContext) {
        use steer_canvas::SheetKindSpec;

        let untitled = SheetKindSpec {
            kind: "session",
            title: std::sync::Arc::new(|_, _| crate::i18n::t("canvas-tab-new-chat")),
            tab_title: None,
            glyph: "chat",
            closable: true,
            names_container: true,
            restore: None,
            parse: None,
            body: None,
        };
        let titled = SheetKindSpec {
            title: std::sync::Arc::new(|_, _| gpui::SharedString::from("Steady")),
            ..untitled.clone()
        };
        let mut host = CanvasHost::new(SheetRegistry::new().with(untitled));
        host.switch_container("c1", None, |_| true);
        host.open_sheet("session:s1", None);
        cx.update(|cx| {
            assert_eq!(host.container_title("c1", cx), None);
            assert_eq!(
                host.registry().title("session:s1", cx).as_deref(),
                Some(crate::i18n::t("canvas-tab-new-chat").as_ref())
            );
        });

        let mut host = CanvasHost::new(SheetRegistry::new().with(titled));
        host.switch_container("c1", None, |_| true);
        host.open_sheet("session:s1", None);
        cx.update(|cx| assert_eq!(host.container_title("c1", cx).as_deref(), Some("Steady")));
    }
}
