//! The workspace test suite (U9a: extracted from the inline `mod tests`
//! — `super` remains the workspace module, so every import/visibility
//! resolves exactly as before).
/// Serializes tests that init/replace the process-wide `thread_store`
/// global; without it parallel tests clobber each other's store.
static GLOBALS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
use super::{
    ComposerPlacement, RecallDirection, RecallStep, Workspace, composer_key_context,
    composer_placement, editor_can_submit,
};
use gpui::InteractiveElement as _;
use gpui::prelude::*;

/// Drives the pure recall step. `applied` is the text the composer holds
/// afterwards — `None` means the step left it untouched, `Some("")` means
/// it cleared the input.
fn step(
    direction: RecallDirection,
    value: &str,
    index: i64,
    draft: Option<&str>,
    turns: &[&str],
) -> (i64, Option<String>, Option<String>) {
    let owned: Vec<String> = turns.iter().map(|t| t.to_string()).collect();
    let (index, draft, step) = Workspace::recall_step(direction, value, index, draft, &owned);
    let applied = match step {
        RecallStep::None => None,
        RecallStep::Recall(text) => Some(text),
        RecallStep::Clear => Some(String::new()),
    };
    (index, draft, applied)
}

fn assert_recall(step: &(i64, Option<String>, Option<String>), index: i64, text: &str) {
    assert_eq!(step.0, index);
    assert_eq!(step.2.as_deref(), Some(text));
}

#[test]
fn recall_up_from_an_empty_composer_starts_at_the_newest_turn() {
    let turns = ["newest", "oldest"];
    let out = step(RecallDirection::Up, "", -1, None, &turns);
    assert_recall(&out, 0, "newest");
    // Nothing was left behind, so the walk has no draft to return to.
    assert_eq!(out.1, None);
}

#[test]
fn recall_up_from_a_draft_keeps_it_as_the_working_line() {
    let turns = ["newest", "oldest"];
    let out = step(RecallDirection::Up, "typed draft", -1, None, &turns);
    assert_recall(&out, 0, "newest");
    assert_eq!(out.1.as_deref(), Some("typed draft"));
}

#[test]
fn recall_up_walks_further_back_and_clamps_at_the_oldest() {
    let turns = ["newest", "middle", "oldest"];
    assert_recall(
        &step(RecallDirection::Up, "newest", 0, None, &turns),
        1,
        "middle",
    );
    assert_recall(
        &step(RecallDirection::Up, "middle", 1, None, &turns),
        2,
        "oldest",
    );
    let at_oldest = step(RecallDirection::Up, "oldest", 2, None, &turns);
    assert_eq!(at_oldest.0, 2);
    assert_eq!(at_oldest.2, None, "already at the oldest turn");
}

#[test]
fn recall_up_without_history_does_nothing() {
    let out = step(RecallDirection::Up, "", -1, None, &[]);
    assert_eq!(out.0, -1);
    assert_eq!(out.2, None);
}

#[test]
fn recall_down_walks_toward_newer_and_restores_the_working_line() {
    let turns = ["newest", "middle", "oldest"];
    assert_recall(
        &step(RecallDirection::Down, "oldest", 2, Some("draft"), &turns),
        1,
        "middle",
    );
    assert_recall(
        &step(RecallDirection::Down, "middle", 1, Some("draft"), &turns),
        0,
        "newest",
    );
    let at_newest = step(RecallDirection::Down, "newest", 0, Some("draft"), &turns);
    assert_eq!(at_newest.0, -1, "the walk ends past the newest turn");
    assert_eq!(at_newest.1, None, "and its draft is spent");
    assert_eq!(at_newest.2.as_deref(), Some("draft"));
}

#[test]
fn recall_down_at_the_newest_clears_an_empty_working_line() {
    let turns = ["newest"];
    let out = step(RecallDirection::Down, "newest", 0, None, &turns);
    assert_eq!(out.0, -1);
    assert_eq!(out.2.as_deref(), Some(""), "started from an empty input");
}

#[test]
fn recall_down_outside_a_walk_does_nothing() {
    let turns = ["newest"];
    let out = step(RecallDirection::Down, "", -1, None, &turns);
    assert_eq!(out.0, -1);
    assert_eq!(out.2, None);
}

#[test]
fn an_edited_recalled_turn_becomes_the_working_line() {
    let turns = ["newest", "middle"];
    // The walk keeps stepping after an edit, and the edited text is what
    // comes back when the walk runs off the newest end.
    let stepped = step(RecallDirection::Up, "newest edited", 0, None, &turns);
    assert_recall(&stepped, 1, "middle");
    assert_eq!(stepped.1.as_deref(), Some("newest edited"));
    let back = step(
        RecallDirection::Down,
        "middle",
        1,
        Some("newest edited"),
        &turns,
    );
    assert_recall(&back, 0, "newest");
    assert_eq!(back.1.as_deref(), Some("newest edited"));
    let home = step(
        RecallDirection::Down,
        "newest",
        0,
        Some("newest edited"),
        &turns,
    );
    assert_eq!(home.0, -1);
    assert_eq!(home.2.as_deref(), Some("newest edited"));
}

#[test]
fn a_stale_walk_index_starts_a_fresh_walk() {
    let turns = ["newest", "oldest"];
    let out = step(RecallDirection::Up, "whatever", 5, None, &turns);
    assert_recall(&out, 0, "newest");
    assert_eq!(out.1.as_deref(), Some("whatever"));
}

#[test]
fn history_restore_keeps_composer_mounted() {
    assert_eq!(composer_placement(false, true), ComposerPlacement::Hero);
    assert_eq!(composer_placement(false, false), ComposerPlacement::Footer);
}

#[test]
fn editor_remains_the_only_composer_exclusion() {
    assert_eq!(composer_placement(true, true), ComposerPlacement::Hidden);
    assert_eq!(composer_placement(true, false), ComposerPlacement::Hidden);
}

#[test]
fn editor_submission_waits_for_authoritative_history() {
    assert!(!editor_can_submit(true, false, false, "draft"));
    assert!(editor_can_submit(false, false, false, "draft"));
    assert!(!editor_can_submit(false, true, false, "draft"));
    assert!(!editor_can_submit(false, false, true, "draft"));
    assert!(!editor_can_submit(false, false, false, "   "));
}

#[test]
fn composer_key_context_is_the_popover_or_the_composer() {
    // The open popover owns every key the composer subtree sees.
    assert_eq!(composer_key_context(true), "completion = open");
    // Closed, the wrapper is plain `composer`: the recall bindings match
    // its alt-arrows, and nothing else shadows the Input's own keys.
    assert_eq!(composer_key_context(false), "composer");
}

/// Minimal composer harness for the recall keybinding tests: a wrapper
/// carrying a configurable key context around a live input. With
/// `consume_recall` set it also registers recall listeners like the
/// workspace does; an action listener that fires consumes its keystroke
/// even when it does nothing.
struct RecallTestComposer {
    input: gpui::Entity<gpui_component::input::InputState>,
    context: &'static str,
    consume_recall: bool,
}

impl gpui::Render for RecallTestComposer {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let mut wrap = gpui::div().key_context(self.context);
        if self.consume_recall {
            wrap = wrap
                .on_action(|_: &crate::ComposerRecallUp, _window, _cx| {})
                .on_action(|_: &crate::ComposerRecallDown, _window, _cx| {});
        }
        wrap.child(gpui_component::input::Input::new(&self.input))
    }
}

/// Live key routing for the composer subtree: the bare arrows (and
/// Shift+arrows) stay with the Input's caret bindings even though the
/// recall listeners are registered on the same wrapper, while `alt-up` /
/// `alt-down` belong to recall and never touch the caret.
#[gpui::test]
fn bare_arrows_move_caret_and_alt_arrows_recall(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    cx.update(gpui_component::init);
    cx.update(|cx| cx.bind_keys(crate::composer_recall_key_bindings()));
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot_for_window = slot.clone();
    let (_root, cx) = cx.add_window_view(move |window, cx| {
        let view = cx.new(|cx| RecallTestComposer {
            input: cx.new(|cx| gpui_component::input::InputState::new(window, cx).multi_line(true)),
            context: "composer",
            consume_recall: true,
        });
        let input = view.read(cx).input.clone();
        *slot_for_window.borrow_mut() = Some(input);
        gpui_component::Root::new(view, window, cx)
    });
    cx.simulate_resize(gpui::size(gpui::px(640.), gpui::px(480.)));
    let input = slot.borrow().as_ref().expect("input initialized").clone();
    cx.update(|window, cx| input.update(cx, |state, cx| state.focus(window, cx)));

    // A multi-line draft: Up/Down move between its lines. Recall binds
    // neither arrow, so the listeners below never see this keystroke.
    cx.simulate_input("hello\nworld");
    assert_eq!(
        input.read_with(cx, |state, _| state.selected_range().end),
        11
    );
    cx.simulate_keystrokes("up");
    let (row, end) = input.read_with(cx, |state, _| {
        let end = state.selected_range().end;
        (
            gpui_component::input::RopeExt::offset_to_position(state.text(), end).line,
            end,
        )
    });
    assert_eq!(row, 0, "native MoveUp moved the caret to the first line");
    assert!(end < 11);
    cx.simulate_keystrokes("down");
    let row = input.read_with(cx, |state, _| {
        gpui_component::input::RopeExt::offset_to_position(state.text(), state.selected_range().end)
            .line
    });
    assert_eq!(row, 1, "native MoveDown moved the caret back down");
    // Shift+Down extends the selection, untouched by the recall bindings.
    cx.simulate_keystrokes("shift-down");
    let selected = input.read_with(cx, |state, _| state.selected_range());
    assert_ne!(selected.start, selected.end);
    // Alt+Up is recall's: the wrapper's listener consumes it, so the caret
    // and selection do not move at all.
    cx.simulate_keystrokes("alt-up");
    assert_eq!(
        input.read_with(cx, |state, _| state.selected_range()),
        selected
    );
}
#[test]
fn cap_tab_label_caps_with_ellipsis() {
    let long = "a very long tab label that must be capped";
    let capped = super::cap_tab_label(long);
    assert_eq!(capped.chars().count(), super::RIGHT_TAB_LABEL_CAP + 1);
    assert!(capped.ends_with('\u{2026}'), "{capped}");
    // Short labels pass through untouched; unicode caps on char bounds.
    assert_eq!(super::cap_tab_label("short"), "short");
    let unicode = "\u{4e2d}".repeat(super::RIGHT_TAB_LABEL_CAP + 4);
    assert_eq!(
        super::cap_tab_label(&unicode).chars().count(),
        super::RIGHT_TAB_LABEL_CAP + 1
    );
}
/// Right-pane state machine coverage: persisted-snapshot remap, the
/// stash/restore round trip, orphan-tab drops, and db re-materialization
/// across a simulated restart. Runs on an in-memory store so the real
/// `~/.manox/threads.db` is never touched.
#[gpui::test]
async fn right_pane_state_machine(cx: &mut gpui::TestAppContext) {
    use super::{PersistedRightTab, RightTab};
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-right-pane-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // ── persisted_right_pane: subagent filter + active remap ──────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.right_tabs = vec![
                RightTab::Editor,
                RightTab::Subagent("s1".into()),
                RightTab::Launcher,
            ];
            ws.active_right_tab = 2;
            ws.right_pane_visible = true;
            let p = ws.persisted_right_pane(cx);
            assert_eq!(p.tabs.len(), 2, "subagent tab must drop out");
            assert!(matches!(p.tabs[0], PersistedRightTab::Editor));
            assert!(matches!(p.tabs[1], PersistedRightTab::Launcher));
            assert_eq!(p.active, 1, "active remaps into the filtered list");
            assert!(p.visible);

            // An active subagent tab falls back to the head.
            ws.active_right_tab = 1;
            let p = ws.persisted_right_pane(cx);
            assert_eq!(p.active, 0);
        });
    });

    // ── stash → restore round trip (in-session) ────────────────────────
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.right_tabs = vec![RightTab::Editor, RightTab::Launcher];
            ws.active_right_tab = 1;
            ws.right_pane_visible = true;
            ws.stash_right_pane("threadA".into(), cx);
            assert!(ws.right_tabs.is_empty());
            assert!(!ws.right_pane_visible);
            ws.restore_right_pane("threadA", window, cx);
            assert_eq!(ws.right_tabs.len(), 2);
            assert!(matches!(ws.right_tabs[0], RightTab::Editor));
            assert!(matches!(ws.right_tabs[1], RightTab::Launcher));
            assert_eq!(ws.active_right_tab, 1);
            assert!(ws.right_pane_visible);
        });
    });
    // The stash wrote the db row keyed by thread.
    assert!(
        db.load_right_pane("threadA")
            .expect("db readable")
            .is_some()
    );

    // ── orphaned tabs drop on restore ──────────────────────────────────
    // A browser view closed and a session exited while another thread was
    // foreground: the stashed tabs referencing them must not come back.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.right_tabs = vec![
                RightTab::Browser(987_654),
                RightTab::Session("external:claude:dead".into()),
                RightTab::Editor,
            ];
            ws.active_right_tab = 2;
            ws.right_pane_visible = true;
            ws.stash_right_pane("threadB".into(), cx);
            ws.restore_right_pane("threadB", window, cx);
            assert_eq!(ws.right_tabs.len(), 1, "orphans dropped");
            assert!(matches!(ws.right_tabs[0], RightTab::Editor));
            assert_eq!(ws.active_right_tab, 0, "active remaps onto the survivor");
        });
    });

    // ── db re-materialization across a simulated restart ───────────────
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.right_tabs = vec![RightTab::Editor, RightTab::Launcher];
            ws.active_right_tab = 0;
            ws.right_pane_visible = true;
            ws.stash_right_pane("threadC".into(), cx);
            // Restart: the in-session stash is gone, only the db row
            // survives.
            ws.right_pane_by_thread.clear();
            ws.restore_right_pane("threadC", window, cx);
            assert_eq!(ws.right_tabs.len(), 2);
            assert!(matches!(ws.right_tabs[0], RightTab::Editor));
            assert!(matches!(ws.right_tabs[1], RightTab::Launcher));
            assert!(ws.right_pane_visible);
        });
    });

    // ── toggle on an empty pane lands on a fresh Launcher ──────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.right_tabs.clear();
            ws.right_pane_visible = false;
            ws.editor_open = true; // stale on purpose
            ws.toggle_right_pane(cx);
            assert!(ws.right_pane_visible);
            assert_eq!(ws.right_tabs.len(), 1);
            assert!(matches!(ws.right_tabs[0], RightTab::Launcher));
            assert!(!ws.editor_open, "a Launcher is never the editor");
        });
    });

    // Release the process-global store override so the gpui leak
    // detector doesn't trip on it at teardown.
    manox_agent::thread_store::drop_for_test();
    let _ = std::fs::remove_file(&db_path);
}

/// `thread_store::init_for_test` swaps a process-global, and a gpui test
/// runs on its own scheduler thread, so every test that builds a real
/// `Workspace` against a temp db holds this for its whole body.
static STORE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Borrow the store lock and the temp db for one `Workspace` test. The
/// poisoned case recovers rather than cascading a failed assertion into
/// the other store-backed test.
fn store_test_guard() -> std::sync::MutexGuard<'static, ()> {
    // P0-2 (review round 3): every scaffold that reaches `runtime::init()`
    // passes through here first — redirect HOME to a throwaway dir so the
    // single-instance flock cannot contend a developer's live app (which
    // used to `exit(1)` the whole test binary mid-suite with zero
    // diagnostics). The realdata boot test opts out through its
    // MANOX_REALDATA_HOME gate (it wants a real home).
    if std::env::var_os("MANOX_REALDATA_HOME").is_none() {
        manox_agent::runtime::hermetic_home_for_test();
    }
    STORE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The P0-1 stub (review round 3): a deterministic one-model registry
/// installed over the process global. Desktop tests must not read the
/// developer's real `cx.providers.config.yaml` (absent on CI ⇒
/// `expect("a model exists")` was a deterministic red) nor race
/// `provider_glue::init()`'s background registration (a late Arc swap);
/// `install_for_test` freezes the global so later scaffolds' `init()`
/// calls cannot clobber it.
fn install_stub_provider_registry() {
    use manox_harness::core::{
        Api, Cost, InputModality, ProviderConfig, ProviderModelConfig, ProviderRegistry,
    };
    let model_cfg = |id: &str| ProviderModelConfig {
        id: id.into(),
        name: id.into(),
        reasoning: false,
        input: vec![InputModality::Text],
        context_window: 131_072,
        max_tokens: 8_192,
        cost: Cost::default(),
        api: None,
        base_url: None,
        metadata: std::collections::HashMap::new(),
    };
    let registry = ProviderRegistry::new();
    // "anthropic" with model "m" is what the switch test's v3 fixture
    // projects on restore (`no provider registered for "anthropic"` was
    // the silent fresh-fallback root cause); "p" covers the fixture's
    // model_change provider id; "stub-model" gives the intent/SetModel
    // test a distinct target. No network is ever dialed (no turn runs).
    let provider = |models: Vec<ProviderModelConfig>| ProviderConfig {
        name: Some("Stub".into()),
        base_url: Some("https://stub.example".into()),
        api_key: Some("sk-literal".into()),
        api: Some(Api::AnthropicMessages),
        headers: None,
        auth_header: true,
        models,
    };
    registry
        .register_provider(
            "anthropic",
            provider(vec![model_cfg("m"), model_cfg("stub-model")]),
        )
        .unwrap();
    registry
        .register_provider("p", provider(vec![model_cfg("m")]))
        .unwrap();
    manox_agent::provider_glue::install_for_test(std::sync::Arc::new(registry));
}

/// The chip's display resolution is an EXACT registration match — a stale
/// identity (provider/model the current catalog no longer registers)
/// resolves to `None` and renders raw, never a fuzzy look-alike. The
/// registry is constructed inline (the global builds on a background
/// thread, too racy for a sync test, and this must not contend the
/// process-wide runtime.lock either).
#[test]
fn resolve_model_identity_exact_match_only() {
    use manox_harness::core::{
        Api, Cost, InputModality, ProviderConfig, ProviderModelConfig, ProviderRegistry,
    };
    let model_cfg = |id: &str| ProviderModelConfig {
        id: id.into(),
        name: id.into(),
        reasoning: false,
        input: vec![InputModality::Text],
        context_window: 131_072,
        max_tokens: 8_192,
        cost: Cost::default(),
        api: None,
        base_url: None,
        metadata: std::collections::HashMap::new(),
    };
    let registry = ProviderRegistry::new();
    registry
        .register_provider(
            "Test-anthropic",
            ProviderConfig {
                name: Some("Test".into()),
                base_url: Some("https://test.example".into()),
                api_key: Some("sk-literal".into()),
                api: Some(Api::AnthropicMessages),
                headers: None,
                auth_header: true,
                models: vec![model_cfg("m-1")],
            },
        )
        .unwrap();
    assert!(Workspace::resolve_model_identity_in(&registry, "Test-anthropic", "m-1").is_some());
    assert!(
        Workspace::resolve_model_identity_in(&registry, "Test-anthropic", "no-such-model-id")
            .is_none(),
        "a stale id must not fuzzy-resolve to a look-alike"
    );
    assert!(
        Workspace::resolve_model_identity_in(&registry, "Other-anthropic", "m-1").is_none(),
        "the provider registration must match exactly too"
    );
}

/// Unique-ish id for temp files and test session ids without pulling in a
/// uuid dependency. The shape stays inside the wire-id charset (ASCII
/// alphanumeric plus `-`/`_` — the B5 gate on `persisted_session_file`),
/// because these ids double as session ids in the switch/attach tests and
/// must survive the gateway's cold-read path: nanos for time uniqueness,
/// the pid for cross-binary uniqueness (cargo runs test binaries in
/// parallel), a process-local counter for same-nanos collisions. The old
/// `{:?}` thread suffix printed `ThreadId(..)` — parens and spaces the
/// gate rejects.
fn uuid_like_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "{nanos}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
/// A walk entered from a non-empty composer hands the user's own text
/// back after the whole round trip: the draft is carried through every
/// step, not just the first one.
#[test]
fn draft_survives_a_long_walk() {
    let turns = ["newest", "middle", "older", "oldest"];
    let mut value = "my draft".to_string();
    let mut index = -1i64;
    let mut draft: Option<String> = None;
    for expected in ["newest", "middle", "older", "oldest"] {
        let (next, kept, applied) =
            step(RecallDirection::Up, &value, index, draft.as_deref(), &turns);
        assert_eq!(applied.as_deref(), Some(expected));
        assert_eq!(
            kept.as_deref(),
            Some("my draft"),
            "draft held at {expected}"
        );
        index = next;
        draft = kept;
        value = applied.unwrap();
    }
    for expected in ["older", "middle", "newest"] {
        let (next, kept, applied) = step(
            RecallDirection::Down,
            &value,
            index,
            draft.as_deref(),
            &turns,
        );
        assert_eq!(applied.as_deref(), Some(expected));
        assert_eq!(kept.as_deref(), Some("my draft"));
        index = next;
        draft = kept;
        value = applied.unwrap();
    }
    let (ended, spent, applied) = step(
        RecallDirection::Down,
        &value,
        index,
        draft.as_deref(),
        &turns,
    );
    assert_eq!(ended, -1, "the walk ends once the draft is restored");
    assert_eq!(spent, None);
    assert_eq!(applied.as_deref(), Some("my draft"));
}

/// End to end: a navigator fill lands the recall walk on the turn it
/// filled, and `alt-down` off the walk's newest end hands back the draft
/// the fill displaced. The composer's own undo history cannot do this —
/// `set_value` clears it — so the working line is the only copy.
#[gpui::test]
async fn navigator_fill_lands_the_walk_and_hands_the_draft_back(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-fill-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace");

    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            let weak = cx.entity().downgrade();
            let meta = || crate::conversation::UserTurnMeta::new(0, String::new(), None);
            ws.conversation.update(cx, |conv, cx| {
                conv.push_user("older turn".into(), vec![], meta(), weak.clone(), cx);
                conv.push_user("newest turn".into(), vec![], meta(), weak, cx);
            });
            ws.input_state.update(cx, |s, cx| {
                s.set_value("half a sentence", window, cx);
            });

            // ⌘↵ on the older of the two turns.
            ws.fill_composer_from_turn("older turn".into(), window, cx);
            assert_eq!(ws.input_state.read(cx).value().as_ref(), "older turn");
            assert_eq!(ws.recall_index, 1, "newest-first puts it in slot 1");
            assert_eq!(
                ws.recall_draft.as_deref(),
                Some("half a sentence"),
                "the displaced draft is the walk's working line"
            );

            // ⌥↓ to the newest turn, ⌥↓ again past it hands the draft back.
            ws.apply_recall_step(RecallDirection::Down, window, cx);
            assert_eq!(ws.input_state.read(cx).value().as_ref(), "newest turn");
            ws.apply_recall_step(RecallDirection::Down, window, cx);
            assert_eq!(ws.recall_index, -1, "the walk ends at its newest end");
            assert_eq!(ws.recall_draft, None, "and its draft is spent");
            assert_eq!(
                ws.input_state.read(cx).value().as_ref(),
                "half a sentence",
                "the user's own text is back in the composer"
            );
        });
    });

    manox_agent::thread_store::drop_for_test();
    let _ = std::fs::remove_file(&db_path);
}

#[gpui::test]
fn attach_thread_rebinds_store_to_new_session(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-attach-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    // The AgentServer runs on the real tokio runtime and replies to the
    // gpui `ClientStoreHandle` pump across threads. That cross-thread wake
    // is legitimate production behavior, but the deterministic test
    // scheduler flags it unless parking is allowed; without this the test
    // is flaky (fails on Linux CI where the tokio reply lands while the
    // pump is parked on `server_rx.recv()`).
    cx.background_executor.allow_parking();

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");
    let landing_id = ws.read_with(&visual, |ws, _| ws.thread.read(|t| t.id.0.clone()));

    // The landing thread's session is bound to its own id.
    let session_id = ws.read_with(&visual, |ws, _| ws.session_id.clone());
    assert_eq!(session_id, Some(landing_id.clone()), "landing session id");

    // Attach a fresh thread: a new session is created for it, so the
    // foreground store now mirrors the new thread (not the old one).
    let new_id = "t-attach-2".to_string();
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            let new =
                manox_agent::Thread::new_fresh(manox_agent::ThreadId(new_id.clone()), "/".into());
            ws.attach_thread(new, false, _window, cx);
        });
    });
    cx.run_until_parked();
    // Give the new session's pump a beat to deliver ThreadInfo.
    cx.run_until_parked();
    let rebound = ws.read_with(&visual, |ws, cx| {
        let store_id = ws.store.as_ref().map(|s| s.read(cx).store.id.0.clone());
        (
            ws.session_id.clone(),
            store_id,
            ws.thread.read(|t| t.id.0.clone()),
        )
    });
    cx.run_until_parked();
    // Give the new session's pump a beat to deliver ThreadInfo.
    assert_eq!(
        rebound.0.as_deref(),
        Some(new_id.as_str()),
        "session rebound"
    );
    assert_eq!(
        rebound.1.as_deref(),
        Some(new_id.as_str()),
        "store mirrors new thread"
    );
    assert_eq!(rebound.2, new_id, "foreground thread swapped");
    drop(ws);
    drop(visual);
    manox_agent::thread_store::drop_global_for_test();
}

/// U2 end-to-end through the real in-process gateway: the C1 handshake
/// `Ready` lands on the multiplexer and fires the first list pull (the
/// server's command snapshot always carries the built-ins, so a
/// non-empty commands array proves the round trip); then an in-process
/// store change rides the dual-track bridge — the store event drives the
/// workspace pump, which pushes the decoration columns to the sidebar
/// and re-pulls `ListThreads`, so the sidebar's rows arrive as the
/// U1-flush: the parked follow-up flush rides the gateway wire — all
/// but the last item as `AppendUserMessage` notes, the last as the v2
/// `Submit` (origin_rpc riding for the entry correlation) — and the
/// non-`Queued` cards stay parked. The pre-migration flush wrote the
/// facade directly (`insert_user_message` + `run_turn`), bypassing K5
/// accept-time persistence and the server queue merge.
#[gpui::test]
fn parked_follow_up_flush_rides_the_gateway_wire(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-u1-flush-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");
    // Spy client: the flush's wire frames land on a raw pair this test
    // reads directly (the multiplexer keeps its own server connection).
    let (client_conn, server_conn) = manox_protocol::in_process_pair();
    ws.update(cx, |ws, _| {
        ws.client = std::sync::Arc::new(manox_session_core::agent_client::AgentClient::from_conn(
            client_conn,
        ));
    });
    // Seed the parked stash: two Queued drains + one Failed card that
    // must stay parked.
    ws.update(cx, |ws, cx| {
        let turn = |text: &str, cx: &mut Context<Workspace>| -> super::DeferredUserTurn {
            let meta = ws.user_turn_meta(cx);
            super::DeferredUserTurn {
                text: text.to_string(),
                images: vec![],
                user_images: vec![],
                meta,
            }
        };
        let mut q = std::collections::VecDeque::new();
        q.push_back(super::QueuedFollowUp {
            turn: turn("first", cx),
            state: super::FollowUpState::Queued,
        });
        q.push_back(super::QueuedFollowUp {
            turn: turn("second", cx),
            state: super::FollowUpState::Queued,
        });
        q.push_back(super::QueuedFollowUp {
            turn: turn("failed-card", cx),
            state: super::FollowUpState::Failed,
        });
        ws.queued_follow_ups_by_thread.insert("s-parked".into(), q);
    });
    ws.update(cx, |ws, cx| ws.flush_parked_follow_ups("s-parked", cx));
    // The two wire frames: the Append note first, the Submit last.
    use manox_protocol::RpcConnection as _;
    let rx = server_conn.client_rx();
    let mut frames = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while frames.len() < 2 {
        cx.run_until_parked();
        while let Ok(m) = rx.try_recv() {
            frames.push(m);
        }
        if frames.len() >= 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the parked flush never reached the wire ({} frames)",
            frames.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    match &frames[0] {
        manox_protocol::FromClient::Notification {
            note:
                manox_protocol::ClientNote::AppendUserMessage {
                    session_id,
                    text,
                    images,
                },
        } => {
            assert_eq!(session_id, "s-parked");
            assert_eq!(text, "first");
            assert!(images.is_empty());
        }
        other => panic!("expected the AppendUserMessage note first, got {other:?}"),
    }
    match &frames[1] {
        manox_protocol::FromClient::Request {
            call:
                manox_protocol::ClientCall::Submit {
                    session_id,
                    text,
                    origin_rpc,
                    ..
                },
            ..
        } => {
            assert_eq!(session_id, "s-parked");
            assert_eq!(text, "second");
            assert!(
                origin_rpc.is_some(),
                "the Submit rides an origin_rpc for the entry correlation"
            );
        }
        other => panic!("expected the v2 Submit last, got {other:?}"),
    }
    // The Failed card stays parked; the Queued drains are gone.
    ws.read_with(&visual, |ws, _| {
        let q = ws
            .queued_follow_ups_by_thread
            .get("s-parked")
            .expect("the Failed card keeps the stash alive");
        assert_eq!(q.len(), 1, "{:?}", q.len());
        assert!(matches!(q[0].state, super::FollowUpState::Failed));
    });
    std::fs::remove_file(&db_path).ok();
}

/// #5: the pure insert rule that keeps the composer queue's invariant
/// `[SteerPending|Failed …] ++ [Queued …]`. A promoted steer lands at the head
/// of the queued group (== the end of the steer group), i.e. right before the
/// first plain `Queued` item, or at the tail when there is none.
#[test]
fn steer_group_insert_index_keeps_steers_before_the_queue() {
    use crate::conversation::{UserImage, UserTurnMeta};
    let turn = |text: &str| super::DeferredUserTurn {
        text: text.to_string(),
        images: vec![],
        meta: UserTurnMeta::new(1, "m".into(), None),
        user_images: Vec::<UserImage>::new(),
    };
    let q = |state| super::QueuedFollowUp {
        turn: turn("x"),
        state,
    };
    use super::FollowUpState as S;

    let empty = std::collections::VecDeque::new();
    assert_eq!(super::Workspace::steer_group_insert_index(&empty), 0);

    let all_queued: std::collections::VecDeque<_> =
        [q(S::Queued), q(S::Queued)].into_iter().collect();
    assert_eq!(super::Workspace::steer_group_insert_index(&all_queued), 0);

    let all_steers: std::collections::VecDeque<_> = [
        q(S::SteerPending {
            message_id: "k".into(),
        }),
        q(S::Failed),
    ]
    .into_iter()
    .collect();
    assert_eq!(super::Workspace::steer_group_insert_index(&all_steers), 2);

    let mixed: std::collections::VecDeque<_> = [
        q(S::SteerPending {
            message_id: "k".into(),
        }),
        q(S::Failed),
        q(S::Queued),
        q(S::Queued),
    ]
    .into_iter()
    .collect();
    assert_eq!(super::Workspace::steer_group_insert_index(&mixed), 2);
}

/// The queue drag's commit rule: reorder inside the `Queued` tail only. A
/// committed row (steer/failed) never moves and is never a crossing
/// destination — the group invariant holds even when the pointer hovers the
/// status rows mid-drag.
#[test]
fn queue_move_index_reorders_only_the_queued_tail() {
    use super::composer_render::{QueueDragEdge as E, QueueRowDrag as D};
    let turn = |text: &str| super::DeferredUserTurn {
        text: text.to_string(),
        images: vec![],
        meta: crate::conversation::UserTurnMeta::new(1, "m".into(), None),
        user_images: Vec::new(),
    };
    let q = |state| super::QueuedFollowUp {
        turn: turn("x"),
        state,
    };
    use super::FollowUpState as S;

    let queue: std::collections::VecDeque<_> = [
        q(S::SteerPending {
            message_id: "k".into(),
        }),
        q(S::Queued),
        q(S::Queued),
        q(S::Queued),
    ]
    .into_iter()
    .collect();

    // Forward move: row 1 dropped below row 3 → lands at the tail.
    assert_eq!(
        super::Workspace::queue_move_index(
            &queue,
            D {
                dragged: 1,
                line_on: 3,
                edge: E::Bottom
            }
        ),
        Some(3)
    );
    // Backward move: row 3 dropped above row 1 → lands at 1.
    assert_eq!(
        super::Workspace::queue_move_index(
            &queue,
            D {
                dragged: 3,
                line_on: 1,
                edge: E::Top
            }
        ),
        Some(1)
    );
    // Self drops no-op (own row, and the adjacent slot that re-inserts in place).
    assert_eq!(
        super::Workspace::queue_move_index(
            &queue,
            D {
                dragged: 2,
                line_on: 2,
                edge: E::Top
            }
        ),
        None
    );
    assert_eq!(
        super::Workspace::queue_move_index(
            &queue,
            D {
                dragged: 2,
                line_on: 1,
                edge: E::Bottom
            }
        ),
        None
    );
    // Landing inside the committed head is rejected.
    assert_eq!(
        super::Workspace::queue_move_index(
            &queue,
            D {
                dragged: 3,
                line_on: 0,
                edge: E::Top
            }
        ),
        None
    );
    // A committed source never moves: SteerPending and Failed alike.
    assert_eq!(
        super::Workspace::queue_move_index(
            &queue,
            D {
                dragged: 0,
                line_on: 3,
                edge: E::Bottom
            }
        ),
        None
    );
    let with_failed: std::collections::VecDeque<_> = [q(S::Failed), q(S::Queued), q(S::Queued)]
        .into_iter()
        .collect();
    assert_eq!(
        super::Workspace::queue_move_index(
            &with_failed,
            D {
                dragged: 0,
                line_on: 2,
                edge: E::Bottom
            }
        ),
        None
    );
    // All-Queued still reorders freely to the end.
    let all_queued: std::collections::VecDeque<_> =
        [q(S::Queued), q(S::Queued)].into_iter().collect();
    assert_eq!(
        super::Workspace::queue_move_index(
            &all_queued,
            D {
                dragged: 0,
                line_on: 1,
                edge: E::Bottom
            }
        ),
        Some(1)
    );
}

/// A foreground workspace bound to a spy client with a running store, ready to
/// exercise the composer steer state machine without a real agent turn. Returns
/// the workspace handle and the server half of the spy connection.
fn running_foreground_with_spy(
    cx: &mut gpui::TestAppContext,
    tag: &'static str,
    session_id: &str,
) -> (gpui::Entity<Workspace>, manox_protocol::InProcessConnection) {
    use gpui::AppContext as _;
    let db_path = std::env::temp_dir().join(format!("manox-{tag}-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let _ = window;
    let ws = captured.borrow().clone().expect("workspace captured");
    let (client_conn, server_conn) = manox_protocol::in_process_pair();
    ws.update(cx, |ws, cx| {
        ws.client = std::sync::Arc::new(manox_session_core::agent_client::AgentClient::from_conn(
            client_conn,
        ));
        ws.session_id = Some(session_id.to_string());
        let store = ws.store.as_ref().expect("landing store bound");
        store.update(cx, |h, _| h.store.running = true);
    });
    (ws, server_conn)
}

/// #5: clicking 「引导」 on a parked follow-up while a turn runs sends the REAL
/// online `ClientCall::Steer` and keeps the card parked (promoted to
/// `SteerPending`) — it must NOT push a message-list bubble. That bubble only
/// appears once the turn settles (see the settle tests).
#[gpui::test]
fn steer_click_wires_online_steer_and_parks_the_card(cx: &mut gpui::TestAppContext) {
    use manox_protocol::RpcConnection as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let (ws, server_conn) = running_foreground_with_spy(cx, "steer-wire", "s-steer");
    cx.run_until_parked();

    let bubbles_before = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    ws.update(cx, |ws, cx| {
        let meta = ws.user_turn_meta(cx);
        ws.queued_follow_ups.push_back(super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: "steer me".into(),
                images: vec![],
                meta,
                user_images: vec![],
            },
            state: super::FollowUpState::Queued,
        });
        ws.steer_follow_up(0, cx);
    });

    // ① card stays parked, promoted to SteerPending.
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.queued_follow_ups.len(), 1);
        assert!(
            matches!(
                ws.queued_follow_ups[0].state,
                super::FollowUpState::SteerPending { .. }
            ),
            "a running steer parks the card as SteerPending"
        );
    });
    // ② no message-list bubble was pushed.
    let bubbles_after = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    assert_eq!(
        bubbles_before, bubbles_after,
        "steering must not surface a bubble before the turn settles"
    );
    // ③ the online steer reached the wire.
    let rx = server_conn.client_rx();
    let mut saw_steer = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while saw_steer.is_none() && std::time::Instant::now() < deadline {
        cx.run_until_parked();
        while let Ok(m) = rx.try_recv() {
            if let manox_protocol::FromClient::Request {
                call:
                    manox_protocol::ClientCall::Steer {
                        session_id, text, ..
                    },
                ..
            } = m
            {
                saw_steer = Some((session_id, text));
            }
        }
    }
    let (sid, text) = saw_steer.expect("a running steer must send ClientCall::Steer");
    assert_eq!(sid, "s-steer");
    assert_eq!(text, "steer me");
}

/// #5 (settle, success path): a normally-settled turn moves every
/// `SteerPending` card out of the queue and into the message list as a
/// `steered` user bubble, leaving the plain `Queued` cards parked for the flush
/// that follows — so the final list order equals the real delivery order.
#[gpui::test]
fn settle_promotes_pending_steers_into_the_list(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let ws = captured.borrow().clone().expect("workspace captured");
    let mk = |text: &str, state, ws: &mut Workspace, cx: &mut Context<Workspace>| {
        super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: text.into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state,
        }
    };
    ws.update(cx, |ws, cx| {
        let steer = mk(
            "the steer",
            super::FollowUpState::SteerPending {
                message_id: "steer-1".into(),
            },
            ws,
            cx,
        );
        ws.queued_follow_ups.push_back(steer);
        let plain = mk("plain queue", super::FollowUpState::Queued, ws, cx);
        ws.queued_follow_ups.push_back(plain);
    });
    let before = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());

    ws.update(cx, |ws, cx| ws.promote_settled_steers(cx));

    // Steer card left the queue; the plain Queued card stays for the flush.
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.queued_follow_ups.len(), 1, "only the plain queue stays");
        assert!(matches!(
            ws.queued_follow_ups[0].state,
            super::FollowUpState::Queued
        ));
    });
    // The steered bubble entered the list.
    let after = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    assert_eq!(after, before + 1, "settled steer pushes exactly one bubble");
    ws.read_with(cx, |ws, cx| {
        let items = ws.conversation.read(cx).items();
        let last = items.last().expect("steered bubble appended");
        match last.read(cx).kind() {
            crate::conversation::ConvItem::User { text, meta, .. } => {
                assert_eq!(text, "the steer");
                assert!(
                    meta.as_ref().is_some_and(|m| m.steered),
                    "the promoted bubble carries the steered flag"
                );
            }
            other => panic!("expected a user bubble, got {other:?}"),
        }
    });
}

/// #5 (settle routing, per-id verdict): the server retracts only the
/// not-yet-injected FIFO tail of the steer group — exactly those cards turn
/// `Failed` (retryable, no bubble); the injected head promotes with its
/// `steered` bubble. The old "whole group strands" reading would fake-fail
/// an already-delivered message and double-deliver on retry.
#[gpui::test]
fn cancelled_settle_strands_steers_without_a_bubble(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let ws = captured.borrow().clone().expect("workspace captured");
    let mk = |text: &str, message_id: &str, ws: &mut Workspace, cx: &mut Context<Workspace>| {
        super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: text.into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state: super::FollowUpState::SteerPending {
                message_id: message_id.into(),
            },
        }
    };
    ws.update(cx, |ws, cx| {
        let injected = mk("injected steer", "steer-injected", ws, cx);
        ws.queued_follow_ups.push_back(injected);
        let retracted = mk("retracted steer", "steer-retracted", ws, cx);
        ws.queued_follow_ups.push_back(retracted);
        let plain = super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: "plain queue".into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state: super::FollowUpState::Queued,
        };
        ws.queued_follow_ups.push_back(plain);
    });
    let before = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());

    // The server's per-id verdict: one retracted id (the FIFO tail card).
    ws.update(cx, |ws, cx| ws.settle_steer_group(1, cx));

    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.queued_follow_ups.len(), 2, "injected card promoted out");
        assert!(
            matches!(ws.queued_follow_ups[0].state, super::FollowUpState::Failed),
            "the retracted tail lands Failed (retryable)"
        );
        assert!(
            matches!(ws.queued_follow_ups[1].state, super::FollowUpState::Queued),
            "the plain queue stays for the flush"
        );
    });
    let after = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    assert_eq!(
        after,
        before + 1,
        "the injected steer promotes exactly one bubble; the retracted one adds none"
    );
    ws.read_with(cx, |ws, cx| {
        let items = ws.conversation.read(cx).items();
        let last = items.last().expect("promoted bubble appended");
        match last.read(cx).kind() {
            crate::conversation::ConvItem::User { text, meta, .. } => {
                assert_eq!(text, "injected steer");
                assert!(meta.as_ref().is_some_and(|m| m.steered));
            }
            other => panic!("expected a user bubble, got {other:?}"),
        }
    });
}

/// The settle ROUTING arm itself: a `TurnFinished` fed through the
/// workspace's live subscription must derive `stranded` from
/// `stranded_steer_ids` (the forwarding arm, not just `settle_steer_group`)
/// — foreground here; the parked twin is covered by
/// `parked_steer_group_settles_by_the_per_id_tail`.
#[gpui::test]
fn turn_finished_subscription_routes_the_per_id_stranded_verdict(cx: &mut gpui::TestAppContext) {
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
    });
    cx.background_executor.allow_parking();
    let (ws, _server_conn) = running_foreground_with_spy(cx, "turnfin-route", "s-route");
    let mk = |text: &str, message_id: &str, ws: &mut Workspace, cx: &mut Context<Workspace>| {
        super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: text.into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state: super::FollowUpState::SteerPending {
                message_id: message_id.into(),
            },
        }
    };
    ws.update(cx, |ws, cx| {
        let a = mk("injected steer", "s-a", ws, cx);
        ws.queued_follow_ups.push_back(a);
        let b = mk("retracted steer", "s-b", ws, cx);
        ws.queued_follow_ups.push_back(b);
    });
    let before = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    let store = ws
        .read_with(cx, |ws, _| ws.store.clone())
        .expect("foreground store");
    // Drive the subscription, not the method: a cancelled turn whose wire
    // verdict retracted exactly one id (the FIFO tail).
    store.update(cx, |_handle, cx| {
        cx.emit(manox_agent::thread::ThreadEvent::TurnFinished {
            cancelled: true,
            failed: false,
            stranded_steer_ids: vec!["s-b".to_string()],
        });
    });
    cx.run_until_parked();
    ws.read_with(cx, |ws, _| {
        assert_eq!(
            ws.queued_follow_ups.len(),
            1,
            "the injected head promoted out"
        );
        assert!(
            matches!(ws.queued_follow_ups[0].state, super::FollowUpState::Failed),
            "the retracted tail lands Failed"
        );
    });
    let after = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    assert_eq!(
        after,
        before + 1,
        "the subscription routed exactly one promote and one strand"
    );
}

/// Parked twin of the per-id settle: only the retracted tail fails, the
/// injected head drops (the stash has no live list).
#[gpui::test]
fn parked_steer_group_settles_by_the_per_id_tail(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let ws = captured.borrow().clone().expect("workspace captured");
    ws.update(cx, |ws, cx| {
        let mut q = std::collections::VecDeque::new();
        for (text, id) in [("head", "p-a"), ("tail", "p-b")] {
            q.push_back(super::QueuedFollowUp {
                turn: super::DeferredUserTurn {
                    text: text.into(),
                    images: vec![],
                    meta: ws.user_turn_meta(cx),
                    user_images: vec![],
                },
                state: super::FollowUpState::SteerPending {
                    message_id: id.into(),
                },
            });
        }
        q.push_back(super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: "plain".into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state: super::FollowUpState::Queued,
        });
        ws.queued_follow_ups_by_thread
            .insert("s-parked-settle".into(), q);
    });
    ws.update(cx, |ws, _| {
        ws.settle_parked_steer_group("s-parked-settle", 1)
    });
    ws.read_with(cx, |ws, _| {
        let q = ws
            .queued_follow_ups_by_thread
            .get("s-parked-settle")
            .expect("the stash keeps the Failed + Queued cards");
        assert_eq!(q.len(), 2, "the injected head dropped");
        assert!(matches!(q[0].state, super::FollowUpState::Failed));
        assert!(matches!(q[1].state, super::FollowUpState::Queued));
    });
}

/// `undo_last_queued` (the ⌘⌥/ undo): only a tail `Queued` card pops;
/// `SteerPending` (not withdrawable), `Failed` (explicit retry/remove) and an
/// empty queue are all no-ops.
#[gpui::test]
fn undo_last_queued_pops_only_a_queued_tail(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let ws = captured.borrow().clone().expect("workspace captured");
    let mk = |text: &str, state, ws: &mut Workspace, cx: &mut Context<Workspace>| {
        super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: text.into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state,
        }
    };
    // Empty: a no-op.
    ws.update(cx, |ws, cx| ws.undo_last_queued(cx));
    ws.read_with(cx, |ws, _| assert!(ws.queued_follow_ups.is_empty()));

    // [Failed, Queued]: the Queued tail pops; the Failed head stays (it is
    // kept for the explicit retry/remove path).
    ws.update(cx, |ws, cx| {
        let failed = mk("failed head", super::FollowUpState::Failed, ws, cx);
        ws.queued_follow_ups.push_back(failed);
        let queued = mk("undo me", super::FollowUpState::Queued, ws, cx);
        ws.queued_follow_ups.push_back(queued);
    });
    ws.update(cx, |ws, cx| ws.undo_last_queued(cx));
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.queued_follow_ups.len(), 1);
        assert!(matches!(
            ws.queued_follow_ups[0].state,
            super::FollowUpState::Failed
        ));
    });

    // A Failed tail alone: the walk stops (no pop).
    ws.update(cx, |ws, cx| ws.undo_last_queued(cx));
    ws.read_with(cx, |ws, _| assert_eq!(ws.queued_follow_ups.len(), 1));

    // A SteerPending tail: not withdrawable — unchanged.
    ws.update(cx, |ws, cx| {
        let steer = mk(
            "steer tail",
            super::FollowUpState::SteerPending {
                message_id: "undo-s".into(),
            },
            ws,
            cx,
        );
        ws.queued_follow_ups.push_back(steer);
    });
    ws.update(cx, |ws, cx| ws.undo_last_queued(cx));
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.queued_follow_ups.len(), 2);
        assert!(matches!(
            ws.queued_follow_ups[1].state,
            super::FollowUpState::SteerPending { .. }
        ));
    });
}

/// Retire-on-injection (dsh `claimed`): the card leaves the queue the moment
/// its injected row lands — long before the turn boundary — and an unmatched
/// id (any ordinary prompt row reports the same event) is a no-op.
#[gpui::test]
fn injected_row_landing_retires_the_matching_steer_card(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let ws = captured.borrow().clone().expect("workspace captured");
    let mk = |text: &str, state, ws: &mut Workspace, cx: &mut Context<Workspace>| {
        super::QueuedFollowUp {
            turn: super::DeferredUserTurn {
                text: text.into(),
                images: vec![],
                meta: ws.user_turn_meta(cx),
                user_images: vec![],
            },
            state,
        }
    };
    ws.update(cx, |ws, cx| {
        let first = mk(
            "first steer",
            super::FollowUpState::SteerPending {
                message_id: "steer-a".into(),
            },
            ws,
            cx,
        );
        ws.queued_follow_ups.push_back(first);
        let second = mk(
            "second steer",
            super::FollowUpState::SteerPending {
                message_id: "steer-b".into(),
            },
            ws,
            cx,
        );
        ws.queued_follow_ups.push_back(second);
        let plain = mk("plain queue", super::FollowUpState::Queued, ws, cx);
        ws.queued_follow_ups.push_back(plain);
    });
    let before = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());

    // An ordinary prompt row (id carries no card) is inert.
    ws.update(cx, |ws, cx| ws.retire_injected_steer("prompt-row", cx));
    ws.read_with(cx, |ws, _| {
        assert_eq!(
            ws.queued_follow_ups.len(),
            3,
            "unmatched id changes nothing"
        );
    });

    // The injected row for steer-b retires exactly that card, immediately.
    ws.update(cx, |ws, cx| ws.retire_injected_steer("steer-b", cx));
    ws.read_with(cx, |ws, _| {
        assert_eq!(ws.queued_follow_ups.len(), 2, "the injected card left");
        assert!(matches!(
            ws.queued_follow_ups[0].state,
            super::FollowUpState::SteerPending { .. }
        ));
        assert!(matches!(
            ws.queued_follow_ups[1].state,
            super::FollowUpState::Queued
        ));
    });
    let after = ws.read_with(cx, |ws, cx| ws.conversation.read(cx).items().len());
    assert_eq!(after, before + 1, "exactly one steered bubble appears");
    ws.read_with(cx, |ws, cx| {
        let items = ws.conversation.read(cx).items();
        let last = items.last().expect("steered bubble appended");
        match last.read(cx).kind() {
            crate::conversation::ConvItem::User { text, meta, .. } => {
                assert_eq!(text, "second steer");
                assert!(
                    meta.as_ref().is_some_and(|m| m.steered),
                    "the injected bubble carries the steered flag"
                );
            }
            other => panic!("expected a user bubble, got {other:?}"),
        }
    });
}

/// server's wire projection instead of a kernel read.
#[gpui::test]
fn u2_thread_list_flows_through_the_gateway(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-u2-list-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    // The AgentServer answers across threads (real tokio runtime); see
    // the sibling test's parking note.
    cx.background_executor.allow_parking();

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // (a) Ready(epoch) + the first pull: the commands snapshot is never
    // empty (the server always projects the built-in commands).
    let mut pulled = false;
    for _ in 0..300 {
        cx.run_until_parked();
        let (epoch, commands) = ws.read_with(&visual, |ws, cx| {
            let m = ws.multiplexer.read(cx);
            (
                m.ready_epoch(),
                m.commands().as_array().map(|a| a.len()).unwrap_or(0),
            )
        });
        if epoch == Some(manox_protocol::PROTOCOL_EPOCH) && commands > 0 {
            pulled = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        pulled,
        "the handshake Ready + first ListCommands pull must land on the multiplexer"
    );

    // (b) A store change: seed a summary row with a pending-auth badge
    // and register a project folder, then drive the wire leg. U6a: in
    // production the SERVER's store watcher broadcasts the list refresh
    // on this write; in the suite the desktop's global-singleton server
    // was constructed by the first workspace test (its watcher rode
    // that test's store), so this test pulls the list explicitly — the
    // same in-memory snapshot the broadcast would carry. The broadcast
    // mechanism is pinned by store_change_broadcasts_the_list_refresh
    // (session-core) and the mux's ThreadsUpdated fold by the host
    // list-mirror tests.
    cx.update(|_cx| {
        manox_agent::thread_store_global().with_mut(|s| {
            s.register_project("/p/u2".to_string());
            s.insert_summary_for_test("t-u2-row", None);
            s.mark_pending_auth("t-u2-row", true);
        });
    });
    ws.update(cx, |ws, cx| {
        ws.multiplexer.update(cx, |m, _| m.fetch_thread_list());
    });
    let mut row = None;
    for _ in 0..300 {
        cx.run_until_parked();
        let found = ws.read_with(&visual, |ws, cx| {
            ws.multiplexer
                .read(cx)
                .thread_list()
                .iter()
                .find(|r| r.id == "t-u2-row")
                .cloned()
        });
        if found.is_some() {
            row = found;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let row = row.expect("the list refresh must flow through the gateway");
    assert!(
        row.pending_auth,
        "the wire row carries the server's projection of the store flag"
    );

    // (c) The registry rides the wire: the server pushed the Projects
    // snapshot with the ListThreads answer, and the multiplexer's
    // notify fed the workspace's chip-menu cache (the sidebar reads
    // the mux directly — nothing reads the kernel at render time any
    // more).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut ws_known: Vec<String> = ws.read_with(&visual, |ws, _| ws.known_projects.clone());
    loop {
        if ws_known.iter().any(|p| p == "/p/u2") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the chip-menu project cache never rode the mux feed: {ws_known:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        cx.run_until_parked();
        ws_known = ws.read_with(&visual, |ws, _| ws.known_projects.clone());
    }

    drop(ws);
    drop(visual);
    let _ = std::fs::remove_file(&db_path);
    manox_agent::thread_store::drop_global_for_test();
}

/// Diagnostic (PR #765 verification): the full real chain — project-intent
/// creation → follow stream → Snapshot projection baseline materializes
/// project+model in the bound store; then SetModel updates the model
/// projection. Each stage has its own bounded wait so a failure names
/// the broken link.
#[gpui::test]
fn new_thread_intent_lands_projections_and_set_model_updates(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-projdiag-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    let project = std::env::temp_dir().join(format!("manox-projdir-{}", uuid_like_id()));
    std::fs::create_dir_all(&project).unwrap();
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    install_stub_provider_registry();
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // 1. Create with a project intent.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.start_new_thread(Some(project.clone()), window, cx)
        });
    });

    // 2. Bounded wait: session bound off the landing id.
    let mut bound = None;
    for _ in 0..400 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let sid = ws.read_with(&visual, |ws, _| ws.session_id.clone());
        if let Some(sid) = sid {
            bound = Some(sid);
            break;
        }
    }
    let sid = bound.expect("session bound after create receipt");

    // 3. Bounded wait: the follow Snapshot's projection baseline
    //    materializes project + model_id in the bound store.
    let mut projected = None;
    for _ in 0..400 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let got = ws.read_with(&visual, |ws, cx| {
            ws.store.as_ref().map(|s| {
                s.read(cx).store.with(|st| {
                    (
                        st.project.clone(),
                        st.model_id.clone(),
                        st.projections.len(),
                    )
                })
            })
        });
        match got {
            Some((p, Some(m), _)) if p.as_deref() == Some(project.to_str().unwrap()) => {
                projected = Some((p, Some(m)));
                break;
            }
            Some((p, m, _)) => projected = Some((p, m)), // diagnostics
            None => {}
        }
    }
    let (p, m) = projected.expect("projection baseline lands with the project intent");
    assert_eq!(
        p.as_deref(),
        Some(project.to_str().unwrap()),
        "project chip source"
    );

    // 4. SetModel through the same note the picker sends, then bounded
    //    wait for the model projection to update.
    let registry = manox_agent::provider_glue::global();
    let chosen = registry
        .models()
        .into_iter()
        .next()
        .expect("a model exists");
    let target = chosen.id.clone();
    // L8: the picker sends the registration-qualified `{provider}/{model}`
    // ref, not a bare id — mirror that wire shape here so this is a real
    // contract lock for the switch.
    let qualified = format!("{}/{}", chosen.provider, chosen.id);
    visual.update(|_window, cx| {
        ws.update(cx, |ws, _| {
            let _ = ws.send_note(|sid| manox_protocol::ClientNote::SetModel {
                session_id: sid.into(),
                id: qualified.clone(),
            });
        });
    });
    let mut updated = false;
    for _ in 0..400 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .and_then(|s| s.read(cx).store.with(|st| st.model_id.clone()))
        });
        if now.as_deref() == Some(target.as_str()) {
            updated = true;
            break;
        }
    }
    assert!(
        updated || m.as_deref() == Some(target.as_str()),
        "SetModel must update the model projection (was {m:?}, target {target})"
    );
    let _ = sid;
}

/// U6b⑤: the park decision reads the foreground LEAF's wire running
/// mirror (the server-pump-fed truth) — not the facade `is_running`,
/// a dead flag on the detached mirrors the attach path builds since
/// U6b②. A switch away from a running turn parks the thread (its
/// session stays attached — no `DetachSession`), and the switch back
/// reclaims the park in place: the same leaf entity, no reopen.
#[gpui::test]
fn park_rides_the_leaf_running_mirror_and_reclaim_skips_reopen(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-u6b5-park-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // Attach thread A (the create leg registers its leaf), then raise
    // the leaf's wire running mirror — the truth the park reads.
    let a_id = format!("u6b5-a-{}", uuid_like_id());
    visual.update(|window, cx| {
        ws.update(cx, |this, cx| {
            let a = manox_agent::Thread::landing_with_id(
                manox_agent::ThreadId(a_id.clone()),
                this.cwd.clone(),
            );
            this.attach_thread(a, false, window, cx);
        });
    });
    cx.run_until_parked();
    ws.update(cx, |this, cx| {
        let leaf = this.store.as_ref().expect("A attached with a leaf");
        leaf.update(cx, |h, _cx| {
            h.store
                .apply_session_status(Some(true), None, None, None, None, None);
        });
    });

    // Switch to B while A's turn runs: A must park, not detach.
    let b_id = format!("u6b5-b-{}", uuid_like_id());
    visual.update(|window, cx| {
        ws.update(cx, |this, cx| {
            let b = manox_agent::Thread::landing_with_id(
                manox_agent::ThreadId(b_id.clone()),
                this.cwd.clone(),
            );
            this.attach_thread(b, false, window, cx);
        });
    });
    cx.run_until_parked();
    let (parked_len, parked_id, fg_sid) = ws.read_with(&visual, |this, _| {
        (
            this.background_threads.len(),
            this.background_threads.first().map(|b| b.id.clone()),
            this.session_id.clone(),
        )
    });
    assert_eq!(parked_len, 1, "the running thread A parks on the switch");
    assert_eq!(parked_id.as_deref(), Some(a_id.as_str()));
    assert_eq!(
        fg_sid.as_deref(),
        Some(b_id.as_str()),
        "B is the foreground session"
    );
    let parked_leaf_id = ws.read_with(&visual, |this, _| {
        this.background_threads
            .first()
            .and_then(|b| b.store.as_ref())
            .map(|s| s.entity_id())
    });

    // Switch back to A: the reclaim restores the parked leaf + session
    // in place (no reopen — the parked session never detached).
    visual.update(|window, cx| {
        ws.update(cx, |this, cx| {
            let a2 = manox_agent::Thread::landing_with_id(
                manox_agent::ThreadId(a_id.clone()),
                this.cwd.clone(),
            );
            this.attach_thread(a2, true, window, cx);
        });
    });
    cx.run_until_parked();
    let (after_len, after_sid, after_leaf) = ws.read_with(&visual, |this, _| {
        (
            this.background_threads.len(),
            this.session_id.clone(),
            this.store.as_ref().map(|s| s.entity_id()),
        )
    });
    assert_eq!(after_len, 0, "the park is reclaimed, not double-held");
    assert_eq!(after_sid.as_deref(), Some(a_id.as_str()));
    assert_eq!(
        after_leaf, parked_leaf_id,
        "the reclaim restores the parked leaf in place (no reopen)"
    );
    let _ = std::fs::remove_file(&db_path);
}

/// Regression lock (#765 symptom 2): clicking a sidebar thread loads the
/// persisted transcript. Real-shaped session files (header, a model row,
/// a user and an assistant turn, and — critically — the wire-opaque
/// `custom` / `active_tools_change` rows legacy sessions carry) are
/// seeded on disk, discovered by a store `refresh`, then switched to /
/// away / back. Each cold restore drives the real engine's
/// `open_existing` + follow-stream `Snapshot` on the tokio runtime; the
/// assertion is that the client fold ends up with the transcript, and —
/// critically — that it lands inside the bounded wait (the #765 bug
/// parked the actor behind a store-wide scan so no frame ever arrived;
/// the round-2 bug holed the snapshot page at the wire-less rows so the
/// fold looped snapshot → Resync and nothing ever rendered).
#[gpui::test]
fn sidebar_thread_switch_restores_transcript(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-switch-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    install_stub_provider_registry();
    cx.background_executor.allow_parking();

    // Seed two persisted transcripts in the store's scan directory.
    let sessions = manox_agent::thread_store::global_sessions_dir();
    std::fs::create_dir_all(&sessions).unwrap();
    let id_a = format!("switch-a-{}", uuid_like_id());
    let id_b = format!("switch-b-{}", uuid_like_id());
    let transcript = |id: &str, text: &str| {
        format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"cwd\":\"/tmp\",\"metadata\":{{\"host\":\"manox\"}}}}\n\
                 {{\"type\":\"model_change\",\"id\":\"{id}-m0\",\"parentId\":null,\"timestamp\":\"2026-01-01T00:00:00Z\",\"provider\":\"p\",\"modelId\":\"m\"}}\n\
                 {{\"type\":\"message\",\"id\":\"{id}-u1\",\"parentId\":\"{id}-m0\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}],\"timestamp\":1767225601000}}}}\n\
                 {{\"type\":\"custom\",\"id\":\"{id}-c1\",\"parentId\":\"{id}-u1\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"customType\":\"manox_ui_note\",\"data\":{{\"kind\":\"notice\",\"data\":{{\"text\":\"note\"}}}}}}\n\
                 {{\"type\":\"active_tools_change\",\"id\":\"{id}-t1\",\"parentId\":\"{id}-c1\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"activeToolNames\":[\"Bash\"]}}\n\
                 {{\"type\":\"message\",\"id\":\"{id}-a1\",\"parentId\":\"{id}-t1\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"reply\"}}],\"model\":\"m\",\"provider\":\"p\",\"api\":\"anthropic\",\"stopReason\":\"stop\",\"timestamp\":1767225602000}}}}\n"
        )
    };
    std::fs::write(
        sessions.join(format!("{id_a}.jsonl")),
        transcript(&id_a, "alpha-turn"),
    )
    .unwrap();
    std::fs::write(
        sessions.join(format!("{id_b}.jsonl")),
        transcript(&id_b, "beta-turn"),
    )
    .unwrap();
    manox_agent::thread_store::global().refresh();

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // Both rows appear in the sidebar after the scan lands.
    let mut ids = Vec::new();
    for _ in 0..1000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
        ids = manox_agent::thread_store::global()
            .read(|s| s.summaries().iter().map(|r| r.id.clone()).collect());
        if ids.contains(&id_a) && ids.contains(&id_b) {
            break;
        }
    }
    assert!(
        ids.contains(&id_a) && ids.contains(&id_b),
        "both threads listed"
    );

    // The count of folded messages currently mirrored in the client store.
    let folded = |ws: &gpui::Entity<Workspace>, visual: &mut gpui::VisualTestContext| {
        ws.read_with(visual, |ws, cx| {
            ws.store
                .as_ref()
                .map(|s| s.read(cx).store.with(|st| st.display.len()))
                .unwrap_or(0)
        })
    };

    // Open thread A → bounded wait: A's transcript folds into the store.
    // The bound covers the cold engine boot (provider + LSP preflight are
    // bounded but can take several seconds) plus the follow-stream
    // snapshot round-trip; the regression itself never loads a frame, so
    // the wait is a true pass/fail boundary, not a tuning knob.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.open_thread(id_a.clone(), window, cx));
    });
    let mut a_loaded = false;
    for _ in 0..1000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
        if folded(&ws, &mut visual) >= 2 {
            a_loaded = true;
            break;
        }
    }
    assert!(
        a_loaded,
        "thread A's persisted transcript restores on switch"
    );

    // Switch away to B (A parks to the background), then back to A.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.open_thread(id_b.clone(), window, cx));
    });
    let mut b_loaded = false;
    for _ in 0..1000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
        if folded(&ws, &mut visual) >= 2 {
            b_loaded = true;
            break;
        }
    }
    assert!(
        b_loaded,
        "thread B's persisted transcript restores on switch"
    );

    // Switch back to A: the transcript is present again (non-empty).
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.open_thread(id_a.clone(), window, cx));
    });
    let mut a_back = false;
    for _ in 0..1000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
        if folded(&ws, &mut visual) >= 2 {
            a_back = true;
            break;
        }
    }
    assert!(a_back, "thread A's transcript is non-empty on switch-back");

    let _ = std::fs::remove_file(&db_path);
}

/// #765 round-2 live repro harness: runs the whole open/restore/model
/// contract against the user's REAL sessions tree. `HOME` is redirected
/// for the test process (a sanitized copy — keychain sources replaced
/// with literals, sockets dropped), so the real store scan, the real
/// provider catalog, and the real v3 files all exercise the production
/// paths. Run in isolation:
/// `cargo test -p agent-ui --lib realdata --no-run`, then execute the
/// printed test binary with `HOME=<copy> … realdata -- --nocapture
/// --test-threads=1`.
#[gpui::test]
fn realdata_open_thread_restores_and_set_model_lands(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    // This test only runs in its dedicated process (HOME redirected to the
    // sanitized copy): the shared suite would otherwise point the real
    // `init()` store at the developer's actual `~/.manox` and write to it.
    let Ok(home) = std::env::var("MANOX_REALDATA_HOME") else {
        eprintln!("REALDATA: MANOX_REALDATA_HOME unset — skipping (dedicated-process test)");
        return;
    };
    assert!(
        std::env::var_os("HOME")
            .map(|h| h.to_string_lossy() == home)
            .unwrap_or(false),
        "MANOX_REALDATA_HOME must equal the redirected HOME"
    );
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        // Real init (not init_for_test): sessions_dir and threads.db come
        // from $HOME/.manox — the redirected real-data copy. A prior
        // test's TEST_OVERRIDE store would shadow it, so clear the slot
        // first.
        manox_agent::thread_store::drop_for_test();
        manox_agent::thread_store::init();
    });
    cx.background_executor.allow_parking();

    let target = std::env::var("MANOX_REALDATA_THREAD")
        .unwrap_or_else(|_| "e6d4b2e5-02bb-4bac-aae1-cfa6e5f18750".to_string());

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(1200.), gpui::px(800.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // 1. The real scan must surface the user's thread (386 files; the
    //    bound covers the multi-second full-tree pass).
    let mut listed = false;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let hit = manox_agent::thread_store::global().read(|s| {
            s.summaries().iter().any(|r| r.id == target)
                || s.archived_summaries().iter().any(|r| r.id == target)
        });
        if hit {
            listed = true;
            break;
        }
    }
    let n_summaries = manox_agent::thread_store::global()
        .read(|s| s.summaries().len() + s.archived_summaries().len());
    assert!(
        listed,
        "real scan lists the target thread ({target}); summaries={n_summaries}"
    );
    eprintln!("REALDATA: scan listed {n_summaries} summaries");

    let folded = |ws: &gpui::Entity<Workspace>, visual: &mut gpui::VisualTestContext| {
        ws.read_with(visual, |ws, cx| {
            ws.store
                .as_ref()
                .map(|s| s.read(cx).store.with(|st| st.display.len()))
                .unwrap_or(0)
        })
    };

    // 2. The user's exact gesture: click the row → open_thread. The
    //    functional assertion is the transcript fold (the reported bug:
    //    nothing renders, not even a loading state).
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.open_thread(target.clone(), window, cx));
    });
    let mut restored = 0usize;
    let mut bound_sid: Option<String> = None;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let n = folded(&ws, &mut visual);
        if n > 0 {
            restored = n;
            bound_sid = ws.read_with(&visual, |ws, _| ws.session_id.clone());
            break;
        }
    }
    assert!(
        restored > 0,
        "real thread {target} must restore its transcript (display=0, sid={bound_sid:?})"
    );
    eprintln!("REALDATA: restored {restored} display entries (sid={bound_sid:?})");

    // 3. The model chip contract on real data: the file's persisted
    //    model_change must be visible in the projection.
    let model_before = ws.read_with(&visual, |ws, cx| {
        ws.store
            .as_ref()
            .and_then(|s| s.read(cx).store.with(|st| st.model_id.clone()))
    });
    eprintln!("REALDATA: model projection after restore = {model_before:?}");

    // 4. The picker's exact wire: a registration-qualified ref of a real
    //    catalog model, sent through the same note path.
    let registry = manox_agent::provider_glue::global();
    let chosen = registry
        .models()
        .into_iter()
        .find(|m| m.id.contains("qwen3.8-flash"))
        .or_else(|| registry.models().into_iter().next())
        .expect("a real catalog model exists");
    let target_id = chosen.id.clone();
    let qualified = format!("{}/{}", chosen.provider, chosen.id);
    eprintln!("REALDATA: SetModel ref = {qualified}");
    visual.update(|_window, cx| {
        ws.update(cx, |ws, _| {
            let _ = ws.send_note(|sid| manox_protocol::ClientNote::SetModel {
                session_id: sid.into(),
                id: qualified.clone(),
            });
        });
    });
    let mut model_updated = false;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .and_then(|s| s.read(cx).store.with(|st| st.model_id.clone()))
        });
        if now.as_deref() == Some(target_id.as_str()) {
            model_updated = true;
            break;
        }
    }
    assert!(
        model_updated,
        "SetModel({qualified}) must move the model projection (before={model_before:?})"
    );
    eprintln!("REALDATA: SetModel projection update landed");

    // 5. The inheritance contract: a new thread started from this state
    //    must carry the model into the new session's store.
    let project_before = ws.read_with(&visual, |ws, cx| {
        ws.store
            .as_ref()
            .and_then(|s| s.read(cx).store.with(|st| st.project.clone()))
    });
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.start_new_thread(None, window, cx));
    });
    let mut new_sid: Option<String> = None;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let sid = ws.read_with(&visual, |ws, _| ws.session_id.clone());
        if sid.as_deref() != bound_sid.as_deref() {
            new_sid = sid;
            break;
        }
    }
    assert!(new_sid.is_some(), "new thread must bind a fresh session");
    // The create receipt binds the id, but the model/project only land
    // with the new session's follow snapshot — bounded wait for the
    // projection baseline.
    let mut new_model: Option<String> = None;
    for _ in 0..600 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .and_then(|s| s.read(cx).store.with(|st| st.model_id.clone()))
        });
        if now.is_some() {
            new_model = now;
            break;
        }
    }
    let new_project = ws.read_with(&visual, |ws, cx| {
        ws.store
            .as_ref()
            .and_then(|s| s.read(cx).store.with(|st| st.project.clone()))
    });
    eprintln!(
        "REALDATA: new thread {new_sid:?} model={new_model:?} project={new_project:?} \
             (source project was {project_before:?})"
    );
    assert!(
        new_model.is_some(),
        "new thread must inherit a model (source model {target_id}, project {project_before:?})"
    );
}

/// T6: `start_new_thread` deletes the local pre-build — the session is
/// created server-side via the §D.2 `CreateSession` intent, and when the
/// `{session_id}` receipt lands the workspace binds to that server-minted
/// id (store + thread + session all agree, and it is NOT the landing id).
#[gpui::test]
fn start_new_thread_creates_via_intent_and_binds_server_id(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-intent-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");
    let landing_id = ws.read_with(&visual, |ws, _| ws.thread.read(|t| t.id.0.clone()));

    // Fire the intent-driven new-thread creation.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.start_new_thread(None, window, cx));
    });

    // The receipt + bind is async (server task → gpui pump); park until the
    // session id moves off the landing id (bounded wait).
    for i in 0..400 {
        cx.run_until_parked();
        if i % 40 == 0 {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let rebound = ws.read_with(&visual, |ws, _| ws.session_id.clone());
        if rebound.as_deref() != Some(landing_id.as_str()) && rebound.is_some() {
            break;
        }
    }
    let (session, store_id, thread_id) = ws.read_with(&visual, |ws, cx| {
        (
            ws.session_id.clone(),
            ws.store.as_ref().map(|s| s.read(cx).store.id.0.clone()),
            ws.thread.read(|t| t.id.0.clone()),
        )
    });
    let new_id = session.expect("session bound after the create receipt");
    assert_ne!(
        new_id.as_str(),
        landing_id.as_str(),
        "server minted a new id"
    );
    assert_eq!(
        store_id.as_deref(),
        Some(new_id.as_str()),
        "store follows the new id"
    );
    assert_eq!(
        thread_id, new_id,
        "foreground mirror binds to the server id"
    );
    drop(ws);
    drop(visual);
    let _ = std::fs::remove_file(&db_path);
    manox_agent::thread_store::drop_global_for_test();
}

/// #765 round-3 repro (same dedicated-process regime as the sibling
/// realdata test): two residual live symptoms against the real sessions
/// tree —
/// (a) a model picked in the picker immediately after boot (the engine
///     still materializing) must still land in the projection, and
/// (b) ONE `SidebarEvent::OpenThread` (the exact event a row click
///     emits) must select the row and restore the transcript.
#[gpui::test]
fn realdata_boot_set_model_and_single_event_open(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let Ok(home) = std::env::var("MANOX_REALDATA_HOME") else {
        eprintln!("REALDATA: MANOX_REALDATA_HOME unset — skipping (dedicated-process test)");
        return;
    };
    assert!(
        std::env::var_os("HOME")
            .map(|h| h.to_string_lossy() == home)
            .unwrap_or(false),
        "MANOX_REALDATA_HOME must equal the redirected HOME"
    );
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::drop_for_test();
        manox_agent::thread_store::init();
    });
    cx.background_executor.allow_parking();

    let target = std::env::var("MANOX_REALDATA_THREAD")
        .unwrap_or_else(|_| "e6d4b2e5-02bb-4bac-aae1-cfa6e5f18750".to_string());

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(1200.), gpui::px(800.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // (a) The picker's exact gesture, fired IMMEDIATELY after boot with
    // no waits — the engine is still materializing. Pick a model that
    // differs from the boot fallback so the change is observable. (The
    // registry builds on a background thread — with a real HOME the
    // keychain reads take a beat, so wait for the first registration.)
    let mut chosen = None;
    for _ in 0..300 {
        let registry = manox_agent::provider_glue::global();
        if let Some(m) = registry
            .models()
            .into_iter()
            .find(|m| m.id.contains("qwen3.8-max"))
            .or_else(|| registry.models().into_iter().next())
        {
            chosen = Some(m);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let chosen = chosen.expect("a real catalog model exists (registry built within 30s)");
    let target_id = chosen.id.clone();
    let qualified = format!("{}/{}", chosen.provider, chosen.id);
    eprintln!("REALDATA-BOOT: SetModel ref = {qualified} (fired with no settle wait)");
    visual.update(|_window, cx| {
        ws.update(cx, |ws, _| {
            let _ = ws.send_note(|sid| manox_protocol::ClientNote::SetModel {
                session_id: sid.into(),
                id: qualified.clone(),
            });
        });
    });
    let mut landed = false;
    for _ in 0..900 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .and_then(|s| s.read(cx).store.with(|st| st.model_id.clone()))
        });
        if now.as_deref() == Some(target_id.as_str()) {
            landed = true;
            break;
        }
    }
    assert!(
        landed,
        "an immediate post-boot SetModel({qualified}) must land in the model projection"
    );
    eprintln!("REALDATA-BOOT: immediate SetModel landed");
    // The chip's render inputs (the #765 "pick a model, nothing happens"
    // repro: the journal had the change but the chip deserialized the
    // identity projection into a full Model blob and always failed).
    let (chip_provider, chip_id) = ws
        .read_with(&visual, |ws, cx| ws.foreground_model_identity(cx))
        .expect("model identity present after SetModel");
    assert_eq!(
        (chip_provider.as_str(), chip_id.as_str()),
        (chosen.provider.as_str(), target_id.as_str()),
        "the chip identity equals the picked model"
    );
    assert!(
        Workspace::resolve_model_identity(&chip_provider, &chip_id).is_some(),
        "the chip display resolves against the live registry"
    );

    // (b) Wait for the scan to list the target, then ONE OpenThread
    // event — the exact payload a row click emits.
    let mut listed = false;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        if manox_agent::thread_store::global()
            .read(|s| s.summaries().iter().any(|r| r.id == target))
        {
            listed = true;
            break;
        }
    }
    assert!(listed, "real scan lists the target thread");
    let sidebar = ws.read_with(&visual, |ws, _| ws.sidebar.clone());
    visual.update(|_window, cx| {
        sidebar.update(cx, |_, cx| {
            cx.emit(crate::views::sidebar::SidebarEvent::OpenThread(
                target.clone(),
            ))
        });
    });
    // The selection must move on that single event (the sidebar's
    // selected id is set synchronously inside open_thread → attach).
    let mut selected_now = false;
    for _ in 0..50 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let sel = ws.read_with(&visual, |ws, cx| {
            ws.sidebar.read(cx).selected_id().map(str::to_string)
        });
        if sel.as_deref() == Some(target.as_str()) {
            selected_now = true;
            break;
        }
    }
    assert!(
        selected_now,
        "one OpenThread event must select the row (no second click needed)"
    );
    // And the transcript restores on that same single event.
    let mut restored = 0usize;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let n = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .map(|s| s.read(cx).store.with(|st| st.display.len()))
                .unwrap_or(0)
        });
        if n > 0 {
            restored = n;
            break;
        }
    }
    assert!(
        restored > 0,
        "one OpenThread event must restore the transcript"
    );
    eprintln!("REALDATA-BOOT: single OpenThread event selected + restored {restored} entries");

    // 6b. Round-7 repro — the user's exact flow tonight: submit on the
    //     REOPENED thread. The durable user row (and the live turn it
    //     starts) must render into the foreground display fold.
    let reopen_probe = "realdata round-7 reopened-thread probe".to_string();
    let reopen_text = reopen_probe.clone();
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| ws.send_user_turn(reopen_text, Vec::new(), cx));
    });
    let mut reopen_row = false;
    for _ in 0..600 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let hit = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .map(|s| {
                    s.read(cx).store.with(|st| {
                        st.display.iter().any(|e| {
                            matches!(
                                e,
                                manox_agent::db::HistoryEntry::Message(m)
                                    if m.content
                                        .iter()
                                        .any(|c| c.to_str() == Some(reopen_probe.as_str()))
                            )
                        })
                    })
                })
                .unwrap_or(false)
        });
        if hit {
            reopen_row = true;
            break;
        }
    }
    assert!(
        reopen_row,
        "a submit on the reopened thread must render its user row"
    );
    eprintln!("REALDATA-BOOT: reopened-thread submit renders its user row");
    // The conversation ENTITY is what paints — the store's display fold
    // alone is not render. The durable user row is an Append (no live
    // ThreadEvent, no structural rebuild), so this is the exact seam the
    // round-7 "no reaction" repro lives or dies on.
    let mut conv_row = false;
    for _ in 0..100 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let hit = ws.read_with(&visual, |ws, cx| {
            ws.conversation.read(cx).items().iter().any(|item| {
                matches!(
                    item.read(cx).kind(),
                    crate::conversation::ConvItem::User { text, .. }
                        if text.contains(reopen_probe.as_str())
                )
            })
        });
        if hit {
            conv_row = true;
            break;
        }
    }
    eprintln!("REALDATA-BOOT: conversation entity carries the user row: {conv_row}");

    // 6c. The streaming contract (round-9): assistant text reaches the
    //     conversation through its DELTA rows — the durable settled
    //     message that follows only finalizes what already rendered.
    //     Round-7 fired a full conversation rebuild on every settled
    //     row; with live journaling (round-8) the typed rows land live,
    //     and the rebuild became a main-thread rebuild storm that
    //     destroyed streaming state on long tool turns. Push the real
    //     sequence through the leaf exactly as the follow stream would:
    //     turn boundary + one text delta must grow the conversation.
    let (before_items, tail_seq) = ws.read_with(&visual, |ws, cx| {
        let n = ws.conversation.read(cx).items().len();
        let tail = ws
            .store
            .as_ref()
            .map(|s| s.read(cx).store.window.last().map(|e| e.seq).unwrap_or(0))
            .unwrap_or(0);
        (n, tail)
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            if let Some(store) = ws.store.clone() {
                store.update(cx, |h, cx| {
                    h.apply_from_server(
                        manox_protocol::FromServer::StreamItem {
                            stream_id: manox_protocol::StreamId::new("probe-stream"),
                            frame: manox_protocol::StreamFrame::Entry {
                                seq: tail_seq + 1,
                                id: format!("w{}", tail_seq + 1),
                                parent_id: None,
                                timestamp: String::new(),
                                event: manox_protocol::JournalWireEvent::TurnStart,
                            },
                        },
                        cx,
                    );
                    h.apply_from_server(
                        manox_protocol::FromServer::StreamItem {
                            stream_id: manox_protocol::StreamId::new("probe-stream"),
                            frame: manox_protocol::StreamFrame::Entry {
                                seq: tail_seq + 2,
                                id: format!("w{}", tail_seq + 2),
                                parent_id: None,
                                timestamp: String::new(),
                                event: manox_protocol::JournalWireEvent::AgentTextDelta {
                                    s: "settled row probe".into(),
                                },
                            },
                        },
                        cx,
                    );
                });
            }
        });
    });
    let mut settled_row_renders = false;
    for _ in 0..100 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let n = ws.read_with(&visual, |ws, cx| ws.conversation.read(cx).items().len());
        if n > before_items {
            settled_row_renders = true;
            break;
        }
    }
    assert!(
        settled_row_renders,
        "a streamed text delta must grow the conversation \
             ({before_items} items before, tail seq {tail_seq})"
    );
    eprintln!("REALDATA-BOOT: text delta streams into the conversation");

    // 7. A NEW thread via the §D.2 intent must have a LIVE transcript:
    //    the real submit path's durable user row arrives through the
    //    follow stream (the round-5 repro: an intent-created session's
    //    transcript stayed dead after submit while the journal recorded
    //    everything — the user message landed in the new session but the
    //    window never showed it).
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.start_new_thread(None, window, cx));
    });
    let mut new_bound: Option<String> = None;
    for _ in 0..3000 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let sid = ws.read_with(&visual, |ws, _| ws.session_id.clone());
        if let Some(sid) = sid
            && sid != target
        {
            new_bound = Some(sid);
            break;
        }
    }
    assert!(
        new_bound.is_some(),
        "the intent thread must bind a fresh session"
    );
    let probe_text = "realdata round-5 transcript probe".to_string();
    let probe = probe_text.clone();
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.send_user_turn(probe.clone(), Vec::new(), cx)
        });
    });
    let mut user_row = false;
    for _ in 0..900 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let hit = ws.read_with(&visual, |ws, cx| {
            ws.store
                .as_ref()
                .map(|s| {
                    s.read(cx).store.with(|st| {
                        st.display.iter().any(|e| {
                            matches!(
                                e,
                                manox_agent::db::HistoryEntry::Message(m)
                                    if m.content
                                        .iter()
                                        .any(|c| c.to_str() == Some(probe.as_str()))
                            )
                        })
                    })
                })
                .unwrap_or(false)
        });
        if hit {
            user_row = true;
            break;
        }
    }
    assert!(
        user_row,
        "the submitted user turn must appear in the intent thread's transcript"
    );
    eprintln!("REALDATA-BOOT: submitted user turn is live in the intent thread's transcript");

    // 8. Turn-outcome probe (diagnostic, env-dependent — never asserts):
    // the engine's turn must reach a terminal state. Against the
    // sanitized copy the provider rejects the dummy key quickly; against
    // a real HOME this is the transport-failure reproducer — the
    // journaled error now carries the full reqwest source chain.
    let journal_sid = new_bound.clone().unwrap_or_default();
    let journal_path =
        manox_agent::thread_store::global_sessions_dir().join(format!("{journal_sid}.jsonl"));
    let mut outcome = String::new();
    for _ in 0..1500 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(text) = std::fs::read_to_string(&journal_path) {
            for line in text.lines().rev() {
                let Ok(d) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                match d.get("type").and_then(|t| t.as_str()) {
                    Some("error") => {
                        outcome = format!(
                            "ERROR: {}",
                            d.get("message").and_then(|m| m.as_str()).unwrap_or("")
                        );
                        break;
                    }
                    Some("turn_finish") => {
                        outcome = "TURN_FINISHED".to_string();
                        break;
                    }
                    _ => {}
                }
            }
            if !outcome.is_empty() {
                break;
            }
        }
    }
    eprintln!(
        "REALDATA-BOOT: turn outcome for {journal_sid} = {}",
        if outcome.is_empty() {
            "(no terminal entry within 150s)"
        } else {
            &outcome
        }
    );
    // No `drop_global_for_test` here: this test runs in a dedicated
    // process (the MANOX_REALDATA_HOME gate) and the agent-runtime
    // background tasks (session-list refresh) outlive the test — dropping
    // the slot makes their `global()` panic after the asserts.
}

/// Rail-freeze regression (the visual-acceptance report): the
/// ContextRail reads its status row (`store.running`) and its usage
/// face (`per_model_usage` / `cumulative_*`) from the store leaf it is
/// bound to. The binding used to happen once at construction, so after
/// a thread switch the rail kept watching the outgoing leaf — the
/// live session's SessionStatus deltas miss its id filter and its
/// committed count never advances (follow frames route to the attached
/// leaf), so the status row and the usage sections never moved while
/// the conversation ran. The attach flow must re-bind the rail to the
/// incoming leaf.
#[gpui::test]
fn rail_store_rebinds_on_thread_attach(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-rail-rebind-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // Construction: the rail is bound to the ctor leaf.
    let ctor = ws.read_with(&visual, |this, cx| {
        (
            this.store.as_ref().map(|s| s.entity_id()),
            this.context_rail.read(cx).diagnostic_store_id(),
        )
    });
    assert!(ctor.0.is_some(), "the ctor workspace has a leaf");
    assert_eq!(ctor.0, ctor.1, "ctor rail binds the ctor leaf");

    // Attach thread B: the workspace swaps in B's leaf; the rail must
    // follow (before the fix it kept the ctor leaf and froze).
    let b_id = format!("rail-b-{}", uuid_like_id());
    visual.update(|window, cx| {
        ws.update(cx, |this, cx| {
            let b = manox_agent::Thread::landing_with_id(
                manox_agent::ThreadId(b_id.clone()),
                this.cwd.clone(),
            );
            this.attach_thread(b, false, window, cx);
        });
    });
    cx.run_until_parked();
    let after = ws.read_with(&visual, |this, cx| {
        (
            this.store.as_ref().map(|s| s.entity_id()),
            this.context_rail.read(cx).diagnostic_store_id(),
        )
    });
    assert!(after.0.is_some(), "B attached with a leaf");
    assert_ne!(
        after.0, ctor.0,
        "the attach swapped the workspace leaf (test precondition)"
    );
    assert_eq!(
        after.0, after.1,
        "the rail must re-bind to the attached thread's leaf (the rail-freeze regression)"
    );
    let _ = std::fs::remove_file(&db_path);
}

/// The launcher cascade regression (2026-09-10 abort): clicking a
/// CLI-agent row runs `launcher_pick` inside the click handler's
/// `ws.update` lease, and `open_launcher_cascade`'s `PopupMenu::build`
/// closure used to read the Workspace entity from inside that lease.
/// gpui's double-lease panic cannot unwind past `handle_view_event`
/// (`extern "C"`), so the app aborted on the spot — every click of a
/// Claude Code / Codex / Copilot launcher row was a hard crash. The
/// sidebar twin of this bug pinned its fix the same way
/// (`new_session_menu_builds_inside_sidebar_update`): the models are
/// read BEFORE the eager build, so opening the cascade inside an
/// update must build cleanly.
#[gpui::test]
fn launcher_cascade_builds_inside_workspace_update(cx: &mut gpui::TestAppContext) {
    use super::{LauncherPick, RightTab};
    use gpui::AppContext as _;

    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path =
        std::env::temp_dir().join(format!("manox-launcher-cascade-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.right_tabs = vec![RightTab::Launcher];
            ws.active_right_tab = 0;
            // The crash shape: the very lease the launcher's on_pick
            // click handler holds when it routes into the cascade.
            ws.launcher_pick(
                LauncherPick::Agent(crate::external_session::SessionKind::ClaudeCode),
                0,
                window,
                cx,
            );
            assert!(
                ws.launcher_menu.is_some(),
                "the cascade menu must be built (not crashed)"
            );
            assert_eq!(
                ws.launcher_menu_kind,
                Some(crate::external_session::SessionKind::ClaudeCode)
            );
        });
    });
    let _ = std::fs::remove_file(&db_path);
}

/// The right-pane spawn cwd source: `launcher_thread_cwd` tracks the
/// foreground store's `cwd` projection exactly — seeded value wins, an
/// unseeded (empty) projection and a missing store both yield `None` so
/// the spawn paths fall back to the workspace default. All three branches
/// are load-bearing: the terminal pane and every launcher row key off
/// this one helper.
#[gpui::test]
fn launcher_thread_cwd_tracks_the_foreground_projection(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;

    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path =
        std::env::temp_dir().join(format!("manox-launcher-cwd-test-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });

    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(960.), gpui::px(640.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    // Seeded projection wins.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            let handle = ws.store.clone().expect("the ctor workspace has a leaf");
            handle.update(cx, |h, _| {
                h.store
                    .merge_projection("cwd", serde_json::json!("/seeded/project"), 5);
            });
            assert_eq!(
                ws.launcher_thread_cwd(cx),
                Some(std::path::PathBuf::from("/seeded/project")),
                "a seeded cwd projection is the spawn source"
            );

            // Unseeded (empty) projection: fall back to the workspace default.
            handle.update(cx, |h, _| {
                h.store.merge_projection("cwd", serde_json::json!(""), 6);
            });
            assert_eq!(
                ws.launcher_thread_cwd(cx),
                None,
                "an empty cwd projection yields None"
            );

            // Missing foreground store: None (warned), never a panic.
            let saved = ws.store.take();
            assert_eq!(
                ws.launcher_thread_cwd(cx),
                None,
                "a missing foreground store yields None instead of panicking"
            );
            ws.store = saved;
        });
    });
    let _ = std::fs::remove_file(&db_path);
}

/// `turn_navigator_layout` matrix: the gutter/border compensation must keep
/// the overlay centered over the message column's card interior at every
/// combination of sidebar collapse, right pane, and context rail.
#[test]
fn turn_navigator_layout_compensates_shell_gutter_and_card_border() {
    use super::{CARD_BORDER, SHELL_PAD_EDGE, SHELL_PAD_LEFT, turn_navigator_layout};
    use gpui::px;

    let rail_inset = px(crate::views::context_rail::ENV_CONTENT_INSET);
    let half_border = px(CARD_BORDER / 2.);

    // Default: expanded sidebar (260), no right pane, no rail. The insets are
    // gutter + card border on each side; the wide leftover clamps to 480.
    let l = turn_navigator_layout(px(1200.), px(260.), None, false);
    assert_eq!(l.left_inset, px(SHELL_PAD_LEFT) + px(260.) + half_border);
    assert_eq!(l.right_inset, px(SHELL_PAD_EDGE) + half_border);
    assert_eq!(l.panel_width, px(480.));

    // Collapsed sidebar: the left inset is just the gutter + border.
    let l = turn_navigator_layout(px(1200.), px(0.), None, false);
    assert_eq!(l.left_inset, px(SHELL_PAD_LEFT) + half_border);
    assert_eq!(l.panel_width, px(480.));

    // Right pane open: its width + editor divider join the right inset and
    // the available span shrinks below the 480 cap.
    let l = turn_navigator_layout(px(1200.), px(260.), Some(px(640.)), false);
    assert_eq!(
        l.right_inset,
        px(SHELL_PAD_EDGE) + half_border + px(640.) + px(super::EDITOR_DIVIDER_WIDTH)
    );
    assert!(l.panel_width < px(480.) && l.panel_width > px(0.));

    // Context rail shown: its content inset joins the right side instead.
    let l = turn_navigator_layout(px(1200.), px(260.), None, true);
    assert_eq!(l.right_inset, px(SHELL_PAD_EDGE) + half_border + rail_inset);

    // Narrow window: the panel takes whatever fits, then floors at zero —
    // never negative (a negative width would poison the overlay layout).
    let l = turn_navigator_layout(px(400.), px(260.), None, false);
    assert_eq!(
        l.panel_width,
        px(400.) - l.left_inset - l.right_inset - px(24.)
    );
    let l = turn_navigator_layout(px(290.), px(260.), None, true);
    assert_eq!(l.panel_width, px(0.));
}

/// Absolute cancel priority (the deadlock class): while the leaf runs, the
/// send/stop control dispatches `cancel_turn` whatever interaction cards are
/// surfaced — the regression swapped it into an empty-input-disabled send,
/// physically unreachable exactly when the user most needed to interrupt.
/// And when not running, the same control keeps the ask supplement path
/// (`submit_input` answers the card via Enter-semantics).
#[gpui::test]
fn send_control_cancels_while_running_with_pending_cards(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-cancel-priority-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    // The AgentServer runs on the real tokio runtime and replies to the
    // gpui store pump across threads. That cross-thread wake is legitimate
    // production behavior, but the deterministic test scheduler flags it
    // unless parking is allowed.
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(1_120.), gpui::px(780.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    let ask_payload = serde_json::json!({
        "questions": [
            { "question": "Which one?", "header": "Pick",
              "options": [{ "label": "A" }, { "label": "B" }] }
        ]
    });
    // The running edge + a pending ask + a pending generic approval + text in
    // the composer: the worst-case shape of the repro.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| {
            ws.pending_ask = super::parse_pending_ask("ask1".into(), ask_payload.clone());
            assert!(ws.pending_ask.is_some(), "ask payload must parse");
            ws.pending_auth = Some(super::PendingAuth {
                id: "call_9".into(),
                tool_name: "Edit".into(),
                summary: "escalate sandbox to danger-full-access".into(),
            });
            ws.input_state
                .update(cx, |s, cx| s.replace("hello", window, cx));
            let store = ws.store.as_ref().expect("landing store bound");
            store.update(cx, |h, _| h.store.running = true);
        });
    });

    // Enabled stop form: `running` alone drives the control, so the card +
    // empty-input disables from the regression can never gate it.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.composer_can_submit(true, cx),
                "running ⟹ the stop control is never disabled"
            );
        });
    });

    // Click: cancel, not answer — and since B2-PR-3 the cancel dismisses the
    // parked ask on the way through (the `{"dismissed": true}` /
    // `AskUserQuestionDismissed` leg that converges the server waterfall on
    // the interrupt). The proof it cancelled rather than answered: the
    // composer text survives untouched (`submit_input`/
    // `resolve_ask_with_response` would have consumed it), and the generic
    // approval card — no party to the dismissal — stays parked for the next
    // verdict.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.send_button_clicked(window, cx));
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.pending_ask.is_none(),
                "the interrupt retires the parked ask via dismissal"
            );
            assert!(
                ws.pending_auth.is_some(),
                "cancel dismisses the ask only; the approval card stays"
            );
            assert_eq!(
                ws.input_state.read(cx).value(),
                "hello",
                "cancel consumes no composer input"
            );
            // Re-surface the ask so the idle edge below keeps the
            // supplement-path contract against a live card.
            let ask_payload = serde_json::json!({
                "questions": [
                    { "question": "Which one?", "header": "Pick",
                      "options": [{ "label": "A" }, { "label": "B" }] }
                ]
            });
            ws.pending_ask = super::parse_pending_ask("ask1".into(), ask_payload);
            let store = ws.store.as_ref().expect("landing store bound");
            store.update(cx, |h, _| h.store.running = false);
        });
    });

    // Idle: the same control keeps the supplement path — it resolves the ask
    // with the composer text (Enter-semantics, cleared input, card gone).
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.send_button_clicked(window, cx));
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.pending_ask.is_none(),
                "idle dispatch keeps the ask-supplement path"
            );
            assert!(
                ws.input_state.read(cx).value().is_empty(),
                "the supplement was submitted, so the input cleared"
            );
        });
    });
    let _ = std::fs::remove_file(&db_path);
}

/// W2 (the deadlocked-conversation live edge): a `ToolCallAuthorization` must
/// surface the interactive question card on the same event edge — it can never
/// wait for the journal's `tool_use` fold to arrive through the stream (that
/// lag is what left the ask unrenderable and the turn deadlocked). The gateway
/// re-delivers a parked question on re-own (§D.6 replay), so a re-delivery of
/// the SAME id must not churn the walk the user is mid-way through answering,
/// while a new id is a fresh adjudication that restarts it.
#[gpui::test]
#[cfg(feature = "test-support")]
fn live_tool_call_authorization_synthesizes_card_and_redelivery_is_idempotent(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-live-ask-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    // Cross-thread AgentServer replies need the scheduler's parking
    // allowance (see the gateway flow tests' note).
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(1_120.), gpui::px(780.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    let tid = ws.read_with(&visual.cx, |ws, _| ws.thread.read(|t| t.id.0.clone()));
    let ask_payload = |q: &str| {
        serde_json::json!({
            "questions": [{ "question": q, "header": "Pick",
                             "options": [{ "label": "A" }, { "label": "B" }] }]
        })
    };
    let emit_auth = |ws: &gpui::Entity<Workspace>,
                     visual: &mut gpui::VisualTestContext,
                     id: &str,
                     summary: &str,
                     q: &str| {
        ws.update(&mut visual.cx, |ws, cx| {
            ws.diagnostic_emit_event(
                &tid,
                manox_agent::ThreadEvent::ToolCallAuthorization {
                    id: id.into(),
                    tool_name: "AskUserQuestion".into(),
                    summary: summary.into(),
                    input: ask_payload(q),
                },
                cx,
            );
        });
    };

    // The live edge on a fresh conversation (no ask `ToolCall` row yet): the
    // pending state AND a synthesized interactive card land on this event, and
    // the walk starts.
    emit_auth(&ws, &mut visual, "ask1", "clarify the target", "Which one?");
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask1".to_string()),
        "the live event surfaces the question card"
    );
    assert_eq!(
        ws.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_tool_call_count("ask1", cx)),
        1,
        "the card row is synthesized on the same edge, without the journal fold"
    );
    assert!(
        ws.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_ask_card_interactive("ask1", cx)),
        "the synthesized card carries the interactive snapshot"
    );
    let walk_gen = ws.read_with(&visual.cx, |ws, _| ws.diagnostic_ask_transition_gen());
    assert!(walk_gen > 0, "the first request starts the walk");

    // §D.6 re-delivery of the same id (a re-own replay): pending state and the
    // walk survive untouched — only the card row is re-adoption-safe to ensure.
    emit_auth(&ws, &mut visual, "ask1", "clarify the target", "Which one?");
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask1".to_string())
    );
    assert_eq!(
        ws.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_tool_call_count("ask1", cx)),
        1,
        "re-delivery must not stack a second card"
    );
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_ask_transition_gen()),
        walk_gen,
        "re-delivery must not restart the walk the user is answering"
    );

    // The fold confirms ask1 — the card arms in the reconcile. An armed flag
    // is a fact about THIS card's lifetime, not workspace-wide state.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_merge_projection(
                "pending_auth",
                serde_json::json!({ "ask1": true }),
                10,
                cx,
            );
            ws.diagnostic_reconcile_pending_with_projections(cx);
        });
    });
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask1".to_string()),
        "an armed, still-pending card stays"
    );

    // A different id is a new adjudication: pending replaces, the walk
    // restarts, and the new card is synthesized too.
    emit_auth(&ws, &mut visual, "ask2", "second question", "And now?");
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask2".to_string())
    );
    assert!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_ask_transition_gen()) > walk_gen,
        "a new id churns the walk"
    );
    assert_eq!(
        ws.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_tool_call_count("ask2", cx)),
        1
    );

    // ask2's Request can outrun its own fold: the projection still names only
    // ask1, yet the freshly-installed card must not be retired for mere
    // absence — the previous card's armed flag must not carry over.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_reconcile_pending_with_projections(cx)
        });
    });
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask2".to_string()),
        "a new card never inherits the previous card's armed flag"
    );
    let _ = std::fs::remove_file(&db_path);
}

/// W1 (the remote-settle half of the repro): a card parked with no local way
/// out — the gateway settled it elsewhere and the wire's `pending_auth`
/// projection moved on. The leaf projection is the authoritative pending view:
/// an id confirmed in it and then vanished must retire the local card, while
/// an id never yet confirmed (the `Request` frame can outrun the projection
/// fold) must never be cleared for mere absence.
#[gpui::test]
#[cfg(feature = "test-support")]
fn armed_then_gone_projection_retires_the_local_card(cx: &mut gpui::TestAppContext) {
    use gpui::AppContext as _;
    let _g = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path =
        std::env::temp_dir().join(format!("manox-projection-reconcile-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    // Cross-thread AgentServer replies need the scheduler's parking
    // allowance (see the gateway flow tests' note).
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(1_120.), gpui::px(780.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");

    let ask_payload = serde_json::json!({
        "questions": [{ "question": "Which one?", "header": "Pick",
                         "options": [{ "label": "A" }, { "label": "B" }] }]
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_seed_ask("ask1", ask_payload.clone(), cx);
            // The MsgId the live `Request` frame registered for the reply leg.
            ws.diagnostic_seed_store_pending_auth("ask1", "q1", cx);
        });
    });

    // Startup race: the projection has not folded the park yet — reconcile must
    // not mistake "never confirmed" for "settled".
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_reconcile_pending_with_projections(cx)
        });
    });
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask1".to_string()),
        "an id never yet confirmed in the projection must not be cleared"
    );

    // The fold confirms the park: the id arms the reconcile; a confirmed-pending
    // card stays.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_merge_projection(
                "pending_auth",
                serde_json::json!({ "ask1": true }),
                10,
                cx,
            );
            ws.diagnostic_reconcile_pending_with_projections(cx);
        });
    });
    let walk_gen = ws.read_with(&visual.cx, |ws, _| ws.diagnostic_ask_transition_gen());
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        Some("ask1".to_string()),
        "a confirmed-pending card stays"
    );

    // Remote settle: the server's fold REMOVES the key on decision (a real
    // settle frame carries an empty map, never `{id: false}`) — the id is gone
    // from the live set. The local card retires with it, and the dead call's
    // MsgId leaves the store so a stale click cannot reply to it.
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.diagnostic_merge_projection("pending_auth", serde_json::json!({}), 11, cx);
            ws.diagnostic_reconcile_pending_with_projections(cx);
        });
    });
    assert_eq!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_ask_id()),
        None,
        "armed-then-gone settles the dead interaction out of the composer"
    );
    assert!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_pending_auth())
            .is_none(),
        "the generic approval card retires alongside the ask"
    );
    assert_eq!(
        ws.read_with(&visual.cx, |ws, cx| ws
            .diagnostic_store_pending_auth_ids(cx)),
        Vec::<String>::new(),
        "the retired call's MsgId must not linger in the leaf store"
    );
    assert!(
        ws.read_with(&visual.cx, |ws, _| ws.diagnostic_ask_transition_gen()) > walk_gen,
        "retiring churns the card state once"
    );
    let _ = std::fs::remove_file(&db_path);
}

/// B2-PR-3: shared scaffold for the dismissal-path regressions. A landing
/// Workspace whose `client` is a raw `in_process_pair` spy (every frame the
/// workspace sends on the wire lands on the returned receiver), with a
/// parsed ask card seeded AND the MsgId a live `Request` frame would have
/// registered for its reply leg. The lock guards ride in the fixture: the
/// process-global store is swapped for exactly one test body.
#[cfg(feature = "test-support")]
struct AskWireSpyFixture {
    _globals: std::sync::MutexGuard<'static, ()>,
    _store: std::sync::MutexGuard<'static, ()>,
    visual: gpui::VisualTestContext,
    ws: gpui::Entity<Workspace>,
    rx: async_channel::Receiver<manox_protocol::FromClient>,
    db_path: std::path::PathBuf,
}

#[cfg(feature = "test-support")]
fn seeded_ask_wire_spy(cx: &mut gpui::TestAppContext) -> AskWireSpyFixture {
    use gpui::AppContext as _;
    let _globals = GLOBALS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _store = store_test_guard();
    cx.update(gpui_component::init);
    let db_path = std::env::temp_dir().join(format!("manox-ask-dismiss-{}.db", uuid_like_id()));
    let db = std::sync::Arc::new(
        manox_agent::db::ThreadsDatabase::open(&db_path).expect("open temp threads db"),
    );
    cx.update(|_cx| {
        manox_agent::runtime::init();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init_for_test(db.clone());
    });
    // Cross-thread AgentServer replies need the scheduler's parking
    // allowance (see the gateway flow tests' note).
    cx.background_executor.allow_parking();
    let captured: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = captured.clone();
    let window = cx.open_window(
        gpui::size(gpui::px(1_120.), gpui::px(780.)),
        move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx));
            *slot.borrow_mut() = Some(workspace.clone());
            gpui_component::Root::new(workspace, window, cx)
        },
    );
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let ws = captured.borrow().clone().expect("workspace captured");
    // Spy client: the dismissal/cancel frames land on a raw pair this test
    // reads directly (the multiplexer keeps its own server connection).
    let (client_conn, server_conn) = manox_protocol::in_process_pair();
    use manox_protocol::RpcConnection as _;
    let rx = server_conn.client_rx();
    ws.update(&mut visual, |ws, _| {
        ws.client = std::sync::Arc::new(manox_session_core::agent_client::AgentClient::from_conn(
            client_conn,
        ));
    });
    let ask_payload = serde_json::json!({
        "questions": [{ "question": "Which one?", "header": "Pick",
                         "options": [{ "label": "A" }, { "label": "B" }] }]
    });
    ws.update(&mut visual, |ws, cx| {
        ws.diagnostic_seed_ask("ask1", ask_payload, cx);
        assert!(ws.pending_ask.is_some(), "ask payload must parse");
        // The MsgId the live `Request` frame registered for the reply leg.
        ws.diagnostic_seed_store_pending_auth("ask1", "q1", cx);
    });
    AskWireSpyFixture {
        _globals,
        _store,
        visual,
        ws,
        rx,
        db_path,
    }
}

/// Collect `count` frames off the spy with a real-clock deadline (the
/// tokio-backed cross-thread wake needs scheduler pumps the deterministic
/// test loop alone may not provide).
#[cfg(feature = "test-support")]
fn spy_frames(
    cx: &mut gpui::TestAppContext,
    rx: &async_channel::Receiver<manox_protocol::FromClient>,
    count: usize,
    what: &str,
) -> Vec<manox_protocol::FromClient> {
    let mut frames = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while frames.len() < count {
        cx.run_until_parked();
        while let Ok(m) = rx.try_recv() {
            frames.push(m);
        }
        if frames.len() >= count {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{what}: only {} frames reached the wire",
            frames.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    frames
}

/// B2-PR-3 ① (close ⇒ dismissal): closing the question card without
/// answering — the card's X leg, `dismiss_ask` — replies on the wire with
/// the Batch-1 `{"dismissed": true}` marker against the live call's MsgId
/// and retires the card. It must NOT send the allow/deny shape: a close is
/// "the user left to speak", not a rejection (server `apply_ask_reply`
/// maps the marker to `AskUserQuestionDismissed`).
#[gpui::test]
#[cfg(feature = "test-support")]
fn closing_the_ask_card_sends_the_dismissal_marker(cx: &mut gpui::TestAppContext) {
    let f = seeded_ask_wire_spy(cx);
    let mut visual = f.visual;
    let ws = f.ws.clone();
    ws.update(&mut visual, |ws, cx| ws.dismiss_ask(cx));
    let frames = spy_frames(cx, &f.rx, 1, "the close leg must reach the wire");
    match &frames[0] {
        manox_protocol::FromClient::Reply { id, outcome } => {
            assert_eq!(
                id,
                &manox_protocol::MsgId::new("q1"),
                "reply to the live ask"
            );
            assert_eq!(
                outcome.as_ref().ok(),
                Some(&serde_json::json!({ "dismissed": true })),
                "the close carries the dismissal marker, never an allow/deny shape"
            );
        }
        other => panic!("expected the dismissal Reply, got {other:?}"),
    }
    // The card is retired locally: a stale second click replies to nothing.
    // (The dead call's MsgId leaves the leaf store with the settle
    // reconcile — same as the answer leg, which never deletes it either.)
    let pending = ws.read_with(&mut visual, |ws, _| ws.diagnostic_pending_ask_id());
    assert_eq!(pending, None, "the close retires the card");
    // A repeat close with nothing pending: no second frame, no panic.
    ws.update(&mut visual, |ws, cx| ws.dismiss_ask(cx));
    cx.run_until_parked();
    assert!(
        f.rx.try_recv().is_err(),
        "a dismissal with no parked card sends nothing"
    );
    let _ = std::fs::remove_file(&f.db_path);
}

/// B2-PR-3 ③ (turn interrupt ⇒ convergence): hitting stop while the ask
/// card is parked must converge the server's waterfall immediately — the
/// dismissal marker goes out on the ask's MsgId first, then the
/// `CancelTurn` note. The Batch-1 pump awaits the parked adjudication
/// inline: without the marker the cancel leaves the pump stalled until
/// disconnect, exactly the practical risk this PR removes.
#[gpui::test]
#[cfg(feature = "test-support")]
fn turn_interrupt_dismisses_the_parked_ask_before_cancel(cx: &mut gpui::TestAppContext) {
    let f = seeded_ask_wire_spy(cx);
    let mut visual = f.visual;
    let ws = f.ws.clone();
    let session_id = ws
        .read_with(&mut visual, |ws, _| ws.session_id.clone())
        .expect("landing session bound");
    ws.update(&mut visual, |ws, cx| ws.cancel_turn(cx));
    let frames = spy_frames(cx, &f.rx, 2, "interrupt: dismissal + cancel");
    match &frames[0] {
        manox_protocol::FromClient::Reply { id, outcome } => {
            assert_eq!(id, &manox_protocol::MsgId::new("q1"));
            assert_eq!(
                outcome.as_ref().ok(),
                Some(&serde_json::json!({ "dismissed": true })),
                "the interrupt converges the parked waterfall with the marker"
            );
        }
        other => panic!("expected the dismissal Reply first, got {other:?}"),
    }
    match &frames[1] {
        manox_protocol::FromClient::Notification {
            note: manox_protocol::ClientNote::CancelTurn { session_id: sid },
        } => {
            assert_eq!(sid, &session_id, "the turn cancel still rides the note");
        }
        other => panic!("expected the CancelTurn note, got {other:?}"),
    }
    ws.read_with(&mut visual, |ws, _| {
        assert!(
            ws.diagnostic_pending_ask_id().is_none(),
            "the interrupt retires the parked card locally"
        );
    });
    let _ = std::fs::remove_file(&f.db_path);
}

/// B2-PR-3 ② (deny stays the Decision leg): the generic approval card's
/// verdict must still settle as allow/deny — `{"allow": …}` on the wire —
/// and must never gain the dismissal marker. Close and reject now run
/// through disjoint exits; this test pins the reject side of the split
/// (it also guards that `resolve_auth` no longer absorbs an ask id).
#[gpui::test]
#[cfg(feature = "test-support")]
fn approval_denial_stays_on_the_decision_leg(cx: &mut gpui::TestAppContext) {
    let f = seeded_ask_wire_spy(cx);
    let mut visual = f.visual;
    let ws = f.ws.clone();
    // Close the ask first (dismissal leg), then raise the generic approval
    // card exactly as a gate escalation would.
    ws.update(&mut visual, |ws, cx| ws.dismiss_ask(cx));
    let frames = spy_frames(cx, &f.rx, 1, "close leg reply");
    assert!(
        matches!(&frames[0], manox_protocol::FromClient::Reply { .. }),
        "the ask close replies first: {frames:?}"
    );
    ws.update(&mut visual, |ws, cx| {
        ws.diagnostic_seed_auth("call_9", "Edit", "escalate sandbox", cx);
        ws.diagnostic_seed_store_pending_auth("call_9", "q9", cx);
    });
    ws.update(&mut visual, |ws, cx| {
        ws.resolve_auth_for_test(manox_agent::PermissionDecision::Deny, cx)
    });
    let frames = spy_frames(cx, &f.rx, 1, "deny reply");
    match &frames[0] {
        manox_protocol::FromClient::Reply { id, outcome } => {
            assert_eq!(
                id,
                &manox_protocol::MsgId::new("q9"),
                "the verdict answers the approval call"
            );
            let v = outcome.as_ref().expect("the deny replies Ok");
            assert_eq!(v, &serde_json::json!({ "allow": false }));
            assert!(
                !serde_json::to_string(v).unwrap().contains("dismissed"),
                "the denial leg never smuggles the dismissal marker"
            );
        }
        other => panic!("expected the deny Reply, got {other:?}"),
    }
    ws.read_with(&mut visual, |ws, _| {
        assert!(ws.diagnostic_pending_auth().is_none(), "the card retires");
    });
    let _ = std::fs::remove_file(&f.db_path);
}
