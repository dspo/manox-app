//! The plan's manual smoke, automated: a headless canvas walks its whole
//! lifecycle through the public surface — a scratch canvas is minted into a
//! container, sheets open, the canvas is parked and restored, and closing the
//! last sheet asks for an archive — with the layout files written and read
//! back at every step.

use std::path::Path;

use steer_canvas::id::{home_sheet_id, session_sheet_id, terminal_sheet_id};
use steer_canvas::layout::{self, DockLayout};
use steer_canvas::persist::{self, CanvasFile};
use steer_canvas::{CanvasState, SetLayoutOutcome, SwitchOutcome};

/// The save pump's one write: the shown container's canvas.
fn save(state: &CanvasState, dir: &Path) {
    let Some(container_id) = state.active_container() else {
        panic!("the scratch canvas is never persisted");
    };
    let file = CanvasFile::new(
        container_id,
        state.layout().clone(),
        state.zoomed().map(str::to_string),
    );
    persist::write_to(dir, &file).expect("the layout writes");
}

/// One "open" gesture: append, commit under a fresh ticket, note the sheet.
fn open(state: &mut CanvasState, sheet: &str) {
    let container_id = state.canvas_key().to_string();
    let next = layout::append_sheet(state.layout(), sheet, None);
    let ticket = state.take_ticket();
    assert_eq!(
        state.set_layout(&container_id, next, ticket),
        SetLayoutOutcome::Applied
    );
    state.note_sheet_opened(&container_id, sheet);
}

/// One "close" gesture.
fn close(state: &mut CanvasState, sheet: &str) -> SetLayoutOutcome {
    let container_id = state.canvas_key().to_string();
    let next = layout::remove_sheet(state.layout(), sheet);
    let ticket = state.take_ticket();
    state.set_layout(&container_id, next, ticket)
}

/// A sidebar click: flush the outgoing canvas, then show `container_id` from
/// whatever the file has (the state adds the parked copy on top).
fn switch(state: &mut CanvasState, dir: &Path, container_id: &str) -> SwitchOutcome {
    if state.active_container().is_some() {
        save(state, dir);
    }
    let saved = persist::read_from(dir, container_id).map(|file| file.snapshot());
    state.switch_container(container_id, saved, |_| true)
}

#[test]
fn a_canvas_moves_through_its_whole_lifecycle() {
    let root = tempfile::tempdir().expect("a temp state root");
    let dir = root.path().join("layouts");

    // 1. A session starts on the scratch canvas; minting the container carries
    //    its sheets over (the start page becomes the container's own).
    let mut state = CanvasState::new();
    open(&mut state, &session_sheet_id("s1"));
    state.migrate_scratch("c1");
    assert_eq!(state.active_container(), Some("c1"));
    assert_eq!(layout::panel_ids(state.layout()), ["home:c1", "session:s1"]);

    // 2. Two more sheets, and the save pump keeps the file current.
    open(&mut state, &terminal_sheet_id("c1", 1));
    open(&mut state, &session_sheet_id("s2"));
    assert_eq!(
        layout::panel_ids(state.layout()),
        ["home:c1", "session:s1", "terminal:c1:1", "session:s2"]
    );
    save(&state, &dir);
    assert_eq!(
        persist::read_from(&dir, "c1")
            .expect("the canvas is on disk")
            .layout,
        *state.layout()
    );

    // The title is the earliest surviving non-start-page sheet.
    assert_eq!(
        state
            .container_title("c1", |id| Some(format!("title {id}")))
            .as_deref(),
        Some("title session:s1")
    );

    // 3. Switching away parks c1 and opens c2 on its default canvas.
    assert_eq!(switch(&mut state, &dir, "c2"), SwitchOutcome::Switched);
    assert!(state.held().contains("c1"));
    assert_eq!(layout::panel_ids(state.layout()), ["home:c2"]);

    // 4. Switching back restores the parked canvas, not the (older or equal)
    //    file, and the shown canvas leaves the parked set.
    assert_eq!(switch(&mut state, &dir, "c1"), SwitchOutcome::Switched);
    assert!(!state.held().contains("c1"));
    assert_eq!(
        layout::panel_ids(state.layout()),
        ["home:c1", "session:s1", "terminal:c1:1", "session:s2"]
    );

    // 5. A fresh process restores the same canvas from the file alone.
    let mut restarted = CanvasState::new();
    let saved = persist::read_from(&dir, "c1").expect("the canvas is on disk");
    restarted.switch_container("c1", Some(saved.snapshot()), |_| true);
    assert_eq!(
        layout::panel_ids(restarted.layout()),
        ["home:c1", "session:s1", "terminal:c1:1", "session:s2"]
    );

    // 6. Closing every sheet is fine until the last one, which asks for the
    //    archive — the workspace owns that write, the state only says when.
    assert_eq!(
        close(&mut state, &home_sheet_id("c1")),
        SetLayoutOutcome::Applied
    );
    assert_eq!(
        close(&mut state, &session_sheet_id("s1")),
        SetLayoutOutcome::Applied
    );
    assert_eq!(
        close(&mut state, &terminal_sheet_id("c1", 1)),
        SetLayoutOutcome::Applied
    );
    assert_eq!(
        close(&mut state, &session_sheet_id("s2")),
        SetLayoutOutcome::AutoArchive("c1".to_string())
    );
    assert_eq!(layout::sheet_count(state.layout()), 0);

    // The archive drops what the state kept; the file stays (the canvas is
    // what the container was left as).
    assert!(state.forget_container("c1"));
    assert_eq!(state.active_container(), None);
    assert_eq!(state.sheet_order("c1"), None);
    assert!(persist::read_from(&dir, "c1").is_some());
}

#[test]
fn a_container_with_no_file_and_no_history_opens_its_start_page() {
    let root = tempfile::tempdir().expect("a temp state root");
    let dir = root.path().join("layouts");
    let mut state = CanvasState::new();
    assert_eq!(switch(&mut state, &dir, "c1"), SwitchOutcome::Switched);
    assert_eq!(layout::panel_ids(state.layout()), ["home:c1"]);

    // The same switch again is not a switch.
    assert_eq!(switch(&mut state, &dir, "c1"), SwitchOutcome::AlreadyShown);
    assert_eq!(
        state.layout(),
        &DockLayout::for_sheets([home_sheet_id("c1")])
    );
}
