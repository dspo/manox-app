//! The per-container canvas state machine: who is shown, what each canvas
//! holds, what is parked, and which intents are still current.
//!
//! This is the only writer of a stored layout ([`CanvasState::set_layout`]) —
//! the geometry-layer counterpart of the chrome boundary's "one writer, one
//! queue" rule. Every commit carries a ticket taken *before* the intent was
//! formed, and a ticket commits at most once: a commit that finds its ticket
//! superseded or already spent is dropped silently instead of rolling a newer
//! intent back.
//!
//! Two derived facts live here as projections, never as stored truth:
//!
//! - a container's title is the earliest surviving sheet's title
//!   ([`CanvasState::container_title`]) — closing the first tab promotes the
//!   second;
//! - the parked set is bounded, so a parked web page or PTY cannot keep
//!   running behind a container nobody can open.

use std::collections::{HashMap, HashSet};

use crate::id::{home_sheet_id, sheet_kind};
use crate::layout::{self, DockLayout};

/// The scratch canvas's key: it is a canvas without a container record, so it
/// has no id of its own. It is never parked and never persisted — its layout
/// lives in memory next to the canvas.
pub const SCRATCH_CONTAINER: &str = "scratch";

/// How many put-aside canvases stay mounted.
///
/// Each holds live bodies (a web page keeps running, a terminal keeps reading
/// its PTY), so the set is bounded: past this many, the least recently shown
/// canvas is dropped and reloads from its stores on the way back.
pub const PARKED_CANVASES: usize = 4;

/// A canvas as it was put away: its layout plus the zoomed panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CanvasSnapshot {
    pub layout: DockLayout,
    pub zoom: Option<String>,
}

/// The canvases put aside, by canvas key, in the order they were last shown
/// (oldest first).
#[derive(Clone, Debug, Default)]
pub struct HeldLayouts {
    canvases: HashMap<String, CanvasSnapshot>,
    order: Vec<String>,
}

impl HeldLayouts {
    /// How many canvases stay parked ([`PARKED_CANVASES`]).
    pub const CAPACITY: usize = PARKED_CANVASES;

    /// How many canvases are parked now.
    pub fn len(&self) -> usize {
        self.canvases.len()
    }

    /// Whether nothing is parked.
    pub fn is_empty(&self) -> bool {
        self.canvases.is_empty()
    }

    /// Whether `container_id`'s canvas is parked.
    pub fn contains(&self, container_id: &str) -> bool {
        self.canvases.contains_key(container_id)
    }

    /// A parked canvas, without taking it out.
    pub fn get(&self, container_id: &str) -> Option<&CanvasSnapshot> {
        self.canvases.get(container_id)
    }

    /// The parked canvases, least recently shown first.
    pub fn ids(&self) -> &[String] {
        &self.order
    }

    /// Puts a canvas aside, most recently shown last, evicting the oldest
    /// beyond the capacity.
    fn park(&mut self, container_id: &str, snapshot: CanvasSnapshot) {
        self.order.retain(|id| id != container_id);
        self.order.push(container_id.to_string());
        self.canvases.insert(container_id.to_string(), snapshot);
        while self.order.len() > Self::CAPACITY {
            let oldest = self.order.remove(0);
            self.canvases.remove(&oldest);
        }
    }

    /// Takes a canvas out of the parked set (it is being shown).
    fn take(&mut self, container_id: &str) -> Option<CanvasSnapshot> {
        self.order.retain(|id| id != container_id);
        self.canvases.remove(container_id)
    }

    /// Drops a canvas for good (its container is gone).
    fn remove(&mut self, container_id: &str) {
        self.order.retain(|id| id != container_id);
        self.canvases.remove(container_id);
    }
}

/// The result of one layout commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetLayoutOutcome {
    /// The layout landed.
    Applied,
    /// The ticket was superseded or already spent (or the canvas is no longer
    /// shown): the edit was dropped, not merged.
    Stale,
    /// The layout landed and left the container with no sheet at all. The
    /// caller archives the container through the workspace's teardown face —
    /// this state never does a cross-entity write on its own.
    AutoArchive(String),
}

/// The result of switching the shown canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// The canvas changed.
    Switched,
    /// The requested canvas was already shown; nothing moved.
    AlreadyShown,
}

/// Every container's canvas bookkeeping, and the one canvas shown now.
#[derive(Clone, Debug)]
pub struct CanvasState {
    /// The shown container; `None` is the scratch canvas.
    active_container: Option<String>,
    /// The shown canvas's layout while a container is shown; `None` exactly
    /// when `active_container` is. The pair moves together — it is set by the
    /// paths that show a container and cleared by the ones that leave one — so
    /// the shown canvas always has a layout to read ([`CanvasState::layout`]
    /// falls back to the scratch canvas while this is `None`).
    shown: Option<DockLayout>,
    /// The scratch canvas's layout, kept in memory whether it is shown or not
    /// (it is never parked and never persisted).
    scratch: DockLayout,
    /// The canvases put aside, bounded.
    held: HeldLayouts,
    /// The zoomed panel id, if any (the zoom state sits outside the layout).
    zoomed: Option<String>,
    /// The monotone canvas-intent ticket.
    ticket: u64,
    /// Each container's sheets in creation order — the container title's
    /// input. Scratch's own key is in the map while it holds sheets.
    sheet_order: HashMap<String, Vec<String>>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            active_container: None,
            shown: None,
            scratch: DockLayout::for_sheets([home_sheet_id(SCRATCH_CONTAINER)]),
            held: HeldLayouts::default(),
            zoomed: None,
            ticket: 0,
            sheet_order: HashMap::new(),
        }
    }
}

impl CanvasState {
    /// A canvas state showing the scratch canvas's start page.
    pub fn new() -> Self {
        Self::default()
    }

    /// The shown container, or `None` for the scratch canvas.
    pub fn active_container(&self) -> Option<&str> {
        self.active_container.as_deref()
    }

    /// The shown canvas's identity: its container id, or
    /// [`SCRATCH_CONTAINER`] for the scratch canvas.
    pub fn canvas_key(&self) -> &str {
        self.active_container
            .as_deref()
            .unwrap_or(SCRATCH_CONTAINER)
    }

    /// The shown canvas's layout. Total by construction: a container's canvas
    /// is held in [`CanvasState::shown`] exactly while it is shown, and the
    /// scratch canvas's layout never leaves [`CanvasState::scratch`].
    pub fn layout(&self) -> &DockLayout {
        match &self.shown {
            Some(layout) => layout,
            None => &self.scratch,
        }
    }

    /// The zoomed panel, if any.
    pub fn zoomed(&self) -> Option<&str> {
        self.zoomed.as_deref()
    }

    /// Zooms a panel of the shown layout, or leaves zoom (`None`). A panel the
    /// layout does not hold clears the zoom instead of blanking the canvas.
    pub fn set_zoom(&mut self, panel: Option<String>) {
        let panel = panel.filter(|panel| layout::has_sheet(self.layout(), panel));
        self.zoomed = panel;
    }

    /// The sheets a user can see now (the throttle's input).
    pub fn visible_panels(&self) -> Vec<&str> {
        layout::active_panel_ids(self.layout(), self.zoomed.as_deref())
    }

    /// The parked canvases.
    pub fn held(&self) -> &HeldLayouts {
        &self.held
    }

    /// A container's sheets in creation order.
    pub fn sheet_order(&self, container_id: &str) -> Option<&[String]> {
        self.sheet_order.get(container_id).map(Vec::as_slice)
    }

    /// The shown canvas as it would be saved.
    pub fn snapshot(&self) -> CanvasSnapshot {
        CanvasSnapshot {
            layout: self.layout().clone(),
            zoom: self.zoomed.clone(),
        }
    }

    /// Takes the next canvas-intent ticket. Every gesture takes one before it
    /// forms its edit and commits with it. A ticket is single-use: its one
    /// successful commit spends it (see [`CanvasState::set_layout`]).
    pub fn take_ticket(&mut self) -> u64 {
        self.ticket += 1;
        self.ticket
    }

    /// Whether `ticket` is still the newest intent and unspent — a successful
    /// commit spends the current one, so the same ticket cannot land twice.
    pub fn ticket_current(&self, ticket: u64) -> bool {
        ticket == self.ticket
    }

    /// The one layout write entry.
    ///
    /// Applies `next` to `container_id`'s canvas when the ticket is current
    /// and that canvas is the one shown; otherwise the edit is dropped as
    /// stale. A successful commit ([`SetLayoutOutcome::Applied`] and
    /// [`SetLayoutOutcome::AutoArchive`] alike) spends the ticket: committing
    /// the same ticket again — even with a different layout — is stale, so one
    /// intent lands at most once. The sheet creation order is pruned to the
    /// new layout first, so the title projection never names a closed sheet.
    pub fn set_layout(
        &mut self,
        container_id: &str,
        next: DockLayout,
        ticket: u64,
    ) -> SetLayoutOutcome {
        if !self.ticket_current(ticket) || container_id != self.canvas_key() {
            return SetLayoutOutcome::Stale;
        }
        // Spending the ticket here makes a second commit with it stale
        // instead of letting one intent land twice.
        self.ticket += 1;
        let alive: HashSet<String> = layout::panel_ids(&next)
            .into_iter()
            .map(str::to_string)
            .collect();
        if let Some(order) = self.sheet_order.get_mut(container_id) {
            order.retain(|id| alive.contains(id));
            if order.is_empty() {
                self.sheet_order.remove(container_id);
            }
        }
        if self
            .zoomed
            .as_deref()
            .is_some_and(|zoomed| !layout::has_sheet(&next, zoomed))
        {
            self.zoomed = None;
        }
        let empty = layout::sheet_count(&next) == 0;
        if container_id == SCRATCH_CONTAINER {
            self.scratch = next;
        } else {
            self.shown = Some(next);
        }
        // The last sheet gone leaves no place to work — the start page's own
        // close counts, which is why `home` is closable at all.
        if empty && container_id != SCRATCH_CONTAINER {
            return SetLayoutOutcome::AutoArchive(container_id.to_string());
        }
        SetLayoutOutcome::Applied
    }

    /// Shows `container_id`'s canvas: parks the one being left, restores this
    /// one, and activates it.
    ///
    /// `saved` is the canvas read from disk (`None` when there is no file);
    /// a parked copy wins over it, because the parked copy is the live truth
    /// and the file may lag behind the save pump's debounce. `alive` answers
    /// whether a sheet's resource still exists — the layout is pruned against
    /// it *before* activation, so a dead id is never shown even for a frame.
    ///
    /// The scratch id is not a container: it routes to
    /// [`CanvasState::show_scratch`], ignoring `saved` and `alive` — the
    /// scratch canvas lives in memory and is never restored from disk.
    pub fn switch_container(
        &mut self,
        container_id: &str,
        saved: Option<CanvasSnapshot>,
        alive: impl Fn(&str) -> bool,
    ) -> SwitchOutcome {
        if container_id == SCRATCH_CONTAINER {
            return self.show_scratch();
        }
        if self.active_container.as_deref() == Some(container_id) {
            return SwitchOutcome::AlreadyShown;
        }
        self.stash_current();
        let restored = self.held.take(container_id).or(saved);
        let (next, zoom) = match restored {
            Some(snapshot) => {
                let keep: HashSet<String> = layout::panel_ids(&snapshot.layout)
                    .into_iter()
                    .filter(|id| alive(id))
                    .map(str::to_string)
                    .collect();
                let pruned = layout::prune_layout(&snapshot.layout, &keep);
                if layout::sheet_count(&pruned) == 0 {
                    (self.default_layout(container_id, &alive), None)
                } else {
                    (pruned, snapshot.zoom)
                }
            }
            None => (self.default_layout(container_id, &alive), None),
        };
        self.active_container = Some(container_id.to_string());
        self.zoomed = zoom.filter(|zoomed| layout::has_sheet(&next, zoomed));
        self.shown = Some(next);
        self.prune_sheet_order(container_id);
        SwitchOutcome::Switched
    }

    /// Shows the scratch canvas. Its layout is kept in memory (never parked,
    /// never persisted) so leaving it and coming back does not reload it.
    pub fn show_scratch(&mut self) -> SwitchOutcome {
        if self.active_container.is_none() {
            return SwitchOutcome::AlreadyShown;
        }
        // Parking the outgoing container clears it as the shown one; the
        // scratch canvas's own layout is already in place (it never left).
        self.stash_current();
        SwitchOutcome::Switched
    }

    /// Notes a sheet as newly opened on `container_id`'s canvas. Repeats are
    /// ignored (the creation order records first appearances only).
    pub fn note_sheet_opened(&mut self, container_id: &str, sheet: &str) {
        let order = self
            .sheet_order
            .entry(container_id.to_string())
            .or_default();
        if !order.iter().any(|id| id == sheet) {
            order.push(sheet.to_string());
        }
    }

    /// A container's title: the earliest created surviving sheet's title.
    ///
    /// `title_of` answers a sheet's title, or `None` when its kind declines to
    /// name a container (`names_container = false`) — the registry is the
    /// authority there, so this stays gpui-free. Start pages are skipped by
    /// kind: every one would read the same. `None` means there is nothing to
    /// project and the caller falls back to the cwd's last segment.
    pub fn container_title(
        &self,
        container_id: &str,
        title_of: impl Fn(&str) -> Option<String>,
    ) -> Option<String> {
        for sheet in self.sheet_order.get(container_id)? {
            if sheet_kind(sheet) == "home" {
                continue;
            }
            let Some(title) = title_of(sheet) else {
                continue;
            };
            let title = title.trim();
            if !title.is_empty() && title != sheet {
                return Some(title.to_string());
            }
        }
        None
    }

    /// Turns the scratch canvas into `container_id`'s canvas: its sheets move
    /// over (the scratch start page becomes the container's), the new
    /// container becomes the shown canvas, and the scratch canvas restarts
    /// with its own start page. Migrating onto the scratch id itself is a
    /// no-op — the scratch canvas is not a container.
    pub fn migrate_scratch(&mut self, container_id: &str) {
        if container_id == SCRATCH_CONTAINER {
            return;
        }
        let from = home_sheet_id(SCRATCH_CONTAINER);
        let to = home_sheet_id(container_id);
        // The scratch canvas restarts with its own start page; its sheets move
        // into the new container.
        let scratch = std::mem::replace(&mut self.scratch, DockLayout::for_sheets([from.clone()]));
        let next = layout::replace_sheet(&scratch, &from, &to);
        if let Some(mut order) = self.sheet_order.remove(SCRATCH_CONTAINER) {
            for id in &mut order {
                if *id == from {
                    *id = to.clone();
                }
            }
            order.retain(|id| layout::has_sheet(&next, id));
            if !order.is_empty() {
                self.sheet_order.insert(container_id.to_string(), order);
            }
        }
        self.active_container = Some(container_id.to_string());
        self.zoomed = None;
        self.shown = Some(next);
    }

    /// Drops everything the state keeps for a container that is gone: its
    /// parked canvas, its creation order, and — when it is the shown one —
    /// its shown layout (the caller then switches somewhere else). Answers
    /// whether the container was the shown canvas.
    ///
    /// The scratch id is not a container: forgetting it does nothing.
    pub fn forget_container(&mut self, container_id: &str) -> bool {
        if container_id == SCRATCH_CONTAINER {
            return false;
        }
        self.sheet_order.remove(container_id);
        self.held.remove(container_id);
        if self.active_container.as_deref() != Some(container_id) {
            return false;
        }
        self.active_container = None;
        self.zoomed = None;
        // The shown canvas is gone; `layout()` now reads the scratch canvas.
        self.shown = None;
        true
    }

    /// Parks the shown container's canvas and clears it as the shown one. The
    /// scratch canvas has nothing to stash: its layout never leaves
    /// [`CanvasState::scratch`].
    fn stash_current(&mut self) {
        let Some(container_id) = self.active_container.take() else {
            return;
        };
        let zoom = self.zoomed.take();
        if let Some(layout) = self.shown.take() {
            let snapshot = CanvasSnapshot { layout, zoom };
            self.held.park(&container_id, snapshot);
        }
    }

    /// The canvas for a container with no usable stored layout: its recorded
    /// sheets (surviving ones only), else its start page.
    fn default_layout(&self, container_id: &str, alive: &impl Fn(&str) -> bool) -> DockLayout {
        let recorded: Vec<String> = self
            .sheet_order
            .get(container_id)
            .map(|order| order.iter().filter(|id| alive(id)).cloned().collect())
            .unwrap_or_default();
        if recorded.is_empty() {
            DockLayout::for_sheets([home_sheet_id(container_id)])
        } else {
            DockLayout::for_sheets(recorded)
        }
    }

    /// Drops creation-order entries the shown layout no longer holds.
    fn prune_sheet_order(&mut self, container_id: &str) {
        let alive: Option<HashSet<String>> = self.shown.as_ref().map(|layout| {
            layout::panel_ids(layout)
                .into_iter()
                .map(str::to_string)
                .collect()
        });
        let Some(order) = self.sheet_order.get_mut(container_id) else {
            return;
        };
        match alive {
            Some(alive) => order.retain(|id| alive.contains(id)),
            None => order.clear(),
        }
        if order.is_empty() {
            self.sheet_order.remove(container_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::{session_sheet_id, terminal_sheet_id};

    fn sheets(ids: &[&str]) -> DockLayout {
        DockLayout::for_sheets(ids.iter().map(|id| (*id).to_string()))
    }

    fn open(state: &mut CanvasState, sheet: &str) {
        let container = state.canvas_key().to_string();
        let next = layout::append_sheet(state.layout(), sheet, None);
        let ticket = state.take_ticket();
        assert_eq!(
            state.set_layout(&container, next, ticket),
            SetLayoutOutcome::Applied
        );
        state.note_sheet_opened(&container, sheet);
    }

    fn switch(state: &mut CanvasState, container: &str) {
        assert_eq!(
            state.switch_container(container, None, |_| true),
            SwitchOutcome::Switched
        );
    }

    #[test]
    fn a_fresh_state_shows_the_scratch_start_page() {
        let state = CanvasState::new();
        assert_eq!(state.active_container(), None);
        assert_eq!(state.canvas_key(), SCRATCH_CONTAINER);
        assert_eq!(layout::panel_ids(state.layout()), ["home:scratch"]);
        assert!(state.held().is_empty());
    }

    #[test]
    fn parking_evicts_the_least_recently_shown_canvas() {
        let mut state = CanvasState::new();
        for index in 0..6 {
            switch(&mut state, &format!("c{index}"));
        }
        // c0..c4 were parked in order; the capacity dropped c0.
        assert_eq!(state.held().ids(), ["c1", "c2", "c3", "c4"]);
        assert!(!state.held().contains("c0"));
        assert!(!state.held().contains("c5"));
        assert_eq!(state.held().len(), HeldLayouts::CAPACITY);
    }

    #[test]
    fn the_shown_canvas_is_never_parked() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        switch(&mut state, "b");
        assert!(state.held().contains("a"));
        switch(&mut state, "a");
        assert!(!state.held().contains("a"));
        assert_eq!(state.held().ids(), ["b"]);
    }

    #[test]
    fn the_scratch_canvas_is_never_parked_and_keeps_its_layout() {
        let mut state = CanvasState::new();
        open(&mut state, &session_sheet_id("s1"));
        switch(&mut state, "a");
        // Leaving the scratch canvas parks nothing: scratch is not a container.
        assert!(state.held().is_empty());
        state.show_scratch();
        assert_eq!(state.active_container(), None);
        assert_eq!(
            layout::panel_ids(state.layout()),
            ["home:scratch", "session:s1"]
        );
        // Scratch never enters the parked set, from either direction.
        assert_eq!(state.held().ids(), ["a"]);
    }

    #[test]
    fn a_parked_canvas_wins_over_a_stale_file_and_is_pruned_by_alive() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        open(&mut state, &session_sheet_id("s1"));
        switch(&mut state, "b");

        // The file lags the parked copy (the save pump debounces), and the
        // session behind `s2` died while parked.
        let saved = CanvasSnapshot {
            layout: sheets(&["home:a", "session:s2"]),
            zoom: None,
        };
        assert_eq!(
            state.switch_container("a", Some(saved), |id| id != "session:s2"),
            SwitchOutcome::Switched
        );
        assert_eq!(layout::panel_ids(state.layout()), ["home:a", "session:s1"]);
    }

    #[test]
    fn a_restored_layout_dies_back_to_the_default_canvas() {
        let mut state = CanvasState::new();
        let saved = CanvasSnapshot {
            layout: sheets(&["session:dead"]),
            zoom: None,
        };
        state.switch_container("a", Some(saved), |_| false);
        assert_eq!(layout::panel_ids(state.layout()), ["home:a"]);
        assert_eq!(state.zoomed(), None);
    }

    #[test]
    fn a_saved_zoom_that_survived_restore_is_kept() {
        let mut state = CanvasState::new();
        let saved = CanvasSnapshot {
            layout: sheets(&["home:a", "session:s1"]),
            zoom: Some("session:s1".to_string()),
        };
        state.switch_container("a", Some(saved), |_| true);
        assert_eq!(state.zoomed(), Some("session:s1"));
        assert_eq!(state.visible_panels(), ["session:s1"]);
    }

    #[test]
    fn a_stale_ticket_drops_the_edit() {
        let mut state = CanvasState::new();
        let stale = state.take_ticket();
        let fresh = state.take_ticket();
        assert!(!state.ticket_current(stale));

        let next = sheets(&["home:scratch", "session:s1"]);
        assert_eq!(
            state.set_layout(SCRATCH_CONTAINER, next.clone(), stale),
            SetLayoutOutcome::Stale
        );
        assert_eq!(layout::panel_ids(state.layout()), ["home:scratch"]);
        assert_eq!(
            state.set_layout(SCRATCH_CONTAINER, next, fresh),
            SetLayoutOutcome::Applied
        );
        assert_eq!(
            layout::panel_ids(state.layout()),
            ["home:scratch", "session:s1"]
        );
    }

    #[test]
    fn an_edit_for_a_canvas_that_is_no_longer_shown_is_stale() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        switch(&mut state, "b");
        let ticket = state.take_ticket();
        // A late gesture from container `a`'s canvas must not land on `b`'s.
        assert_eq!(
            state.set_layout("a", sheets(&["home:a", "session:s1"]), ticket),
            SetLayoutOutcome::Stale
        );
        assert_eq!(layout::panel_ids(state.layout()), ["home:b"]);
    }

    #[test]
    fn a_ticket_commits_once() {
        let mut state = CanvasState::new();
        let ticket = state.take_ticket();
        let next = sheets(&["home:scratch", "session:s1"]);
        assert_eq!(
            state.set_layout(SCRATCH_CONTAINER, next, ticket),
            SetLayoutOutcome::Applied
        );
        // The spent ticket cannot land a second time — not even a different
        // layout on the same canvas.
        assert_eq!(
            state.set_layout(SCRATCH_CONTAINER, sheets(&["home:scratch"]), ticket),
            SetLayoutOutcome::Stale
        );
        assert_eq!(
            layout::panel_ids(state.layout()),
            ["home:scratch", "session:s1"]
        );
        assert!(!state.ticket_current(ticket));
    }

    #[test]
    fn an_auto_archive_commit_spends_the_ticket_too() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        let ticket = state.take_ticket();
        assert_eq!(
            state.set_layout("a", DockLayout::default(), ticket),
            SetLayoutOutcome::AutoArchive("a".to_string())
        );
        assert!(!state.ticket_current(ticket));
    }

    #[test]
    fn closing_the_last_sheet_asks_for_auto_archive() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        open(&mut state, &session_sheet_id("s1"));
        let ticket = state.take_ticket();
        assert_eq!(
            state.set_layout("a", DockLayout::default(), ticket),
            SetLayoutOutcome::AutoArchive("a".to_string())
        );
        assert_eq!(layout::sheet_count(state.layout()), 0);
        assert_eq!(state.sheet_order("a"), None);
    }

    #[test]
    fn the_scratch_canvas_is_never_auto_archived() {
        let mut state = CanvasState::new();
        let ticket = state.take_ticket();
        assert_eq!(
            state.set_layout(SCRATCH_CONTAINER, DockLayout::default(), ticket),
            SetLayoutOutcome::Applied
        );
    }

    #[test]
    fn the_creation_order_records_first_appearance_only() {
        let mut state = CanvasState::new();
        state.note_sheet_opened("a", "home:a");
        state.note_sheet_opened("a", "session:s1");
        state.note_sheet_opened("a", "session:s1");
        assert_eq!(state.sheet_order("a").unwrap(), ["home:a", "session:s1"]);
        assert_eq!(state.sheet_order("nobody"), None);
    }

    #[test]
    fn the_title_is_the_earliest_surviving_sheet_and_skips_start_pages() {
        let mut state = CanvasState::new();
        state.note_sheet_opened("a", "home:a");
        state.note_sheet_opened("a", "session:s1");
        state.note_sheet_opened("a", "session:s2");
        let titled = |id: &str| Some(format!("title {id}"));
        assert_eq!(
            state.container_title("a", titled).as_deref(),
            Some("title session:s1")
        );
        // A sheet may decline to name the container; the next one names it.
        assert_eq!(
            state
                .container_title("a", |id| (id == "session:s2").then(|| "second".to_string()))
                .as_deref(),
            Some("second")
        );
        // A title equal to the id says nothing; the next naming sheet is
        // asked instead.
        assert_eq!(
            state
                .container_title("a", |id| Some(if id == "session:s1" {
                    id.to_string()
                } else {
                    "terminal".to_string()
                }))
                .as_deref(),
            Some("terminal")
        );
        assert_eq!(state.container_title("a", |id| Some(id.to_string())), None);
        assert_eq!(state.container_title("a", |_| None), None);
        assert_eq!(state.container_title("nobody", titled), None);
    }

    #[test]
    fn closing_the_first_sheet_promotes_the_second_title() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        open(&mut state, &session_sheet_id("s1"));
        open(&mut state, &terminal_sheet_id("a", 1));
        let ticket = state.take_ticket();
        let next = layout::remove_sheet(state.layout(), &session_sheet_id("s1"));
        state.set_layout("a", next, ticket);
        assert_eq!(
            state
                .container_title("a", |id| Some(id.to_string()))
                .as_deref(),
            None,
            "the terminal's title equals its id here, so nothing is projected"
        );
        assert_eq!(
            state
                .container_title("a", |id| Some(format!("title {id}")))
                .as_deref(),
            Some("title terminal:a:1")
        );
    }

    #[test]
    fn migrate_scratch_moves_its_sheets_into_the_new_container() {
        let mut state = CanvasState::new();
        open(&mut state, &session_sheet_id("s1"));
        state.migrate_scratch("c1");

        assert_eq!(state.active_container(), Some("c1"));
        assert_eq!(layout::panel_ids(state.layout()), ["home:c1", "session:s1"]);
        // The creation order carries what a gesture opened; the start page
        // itself is never noted (every container would read the same).
        assert_eq!(state.sheet_order("c1").unwrap(), ["session:s1"]);

        // The scratch canvas restarts with its own start page.
        state.show_scratch();
        assert_eq!(layout::panel_ids(state.layout()), ["home:scratch"]);
        assert_eq!(state.sheet_order(SCRATCH_CONTAINER), None);
    }

    #[test]
    fn forgetting_a_container_takes_its_parked_canvas_and_order() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        open(&mut state, &session_sheet_id("s1"));
        switch(&mut state, "b");
        assert!(state.held().contains("a"));

        assert!(!state.forget_container("a"));
        assert!(!state.held().contains("a"));
        assert_eq!(state.sheet_order("a"), None);

        // Forgetting the shown container clears the canvas for the caller.
        assert!(state.forget_container("b"));
        assert_eq!(state.active_container(), None);
        assert_eq!(layout::panel_ids(state.layout()), ["home:scratch"]);
    }

    #[test]
    fn a_snapshot_carries_the_layout_and_the_zoom() {
        let mut state = CanvasState::new();
        open(&mut state, &session_sheet_id("s1"));
        state.set_zoom(Some("session:s1".to_string()));
        let snapshot = state.snapshot();
        assert_eq!(snapshot.layout, *state.layout());
        assert_eq!(snapshot.zoom.as_deref(), Some("session:s1"));
        // A panel the layout does not hold cannot be zoomed.
        state.set_zoom(Some("session:ghost".to_string()));
        assert_eq!(state.zoomed(), None);
    }

    #[test]
    fn switching_through_the_scratch_id_matches_show_scratch() {
        let mut state = CanvasState::new();
        switch(&mut state, "a");
        // The generic switch path must not treat `scratch` as a container: no
        // parked copy under that key, no `saved` snapshot honored, and the
        // scratch canvas stays the keyless one.
        let saved = CanvasSnapshot {
            layout: sheets(&["home:a"]),
            zoom: None,
        };
        assert_eq!(
            state.switch_container(SCRATCH_CONTAINER, Some(saved), |_| true),
            SwitchOutcome::Switched
        );
        assert_eq!(state.active_container(), None);
        assert_eq!(state.canvas_key(), SCRATCH_CONTAINER);
        assert_eq!(layout::panel_ids(state.layout()), ["home:scratch"]);
        assert_eq!(state.held().ids(), ["a"]);
        assert!(!state.held().contains(SCRATCH_CONTAINER));
        // Both entry points agree on what "already shown" means.
        assert_eq!(
            state.switch_container(SCRATCH_CONTAINER, None, |_| true),
            SwitchOutcome::AlreadyShown
        );
        assert_eq!(state.show_scratch(), SwitchOutcome::AlreadyShown);
    }

    #[test]
    fn forgetting_the_scratch_id_leaves_the_shown_canvas_intact() {
        let mut state = CanvasState::new();
        open(&mut state, &session_sheet_id("s1"));
        // The scratch canvas is shown here; forgetting it must not clear the
        // shown layout (the panic this once reached).
        assert!(!state.forget_container(SCRATCH_CONTAINER));
        assert_eq!(state.active_container(), None);
        assert_eq!(
            layout::panel_ids(state.layout()),
            ["home:scratch", "session:s1"]
        );
    }
}
