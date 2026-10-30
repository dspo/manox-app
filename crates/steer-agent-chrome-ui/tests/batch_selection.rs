//! The sidebar's batch-selection mode at the shell level: 多选 enters it with
//! the row checked, clicks toggle instead of switching, 全选 checks every
//! thread row (never an external one), 归档 commits the whole batch through
//! one host hook and drops the rows, 取消 leaves with nothing archived.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AppContext as _, Context, IntoElement, Modifiers, MouseButton, Render, TestAppContext,
    VisualTestContext, div, px, size,
};
use gpui_component::Root;
use steer_agent_chrome_ui::session_list::{SessionRowKind, SessionStatus};
use steer_agent_chrome_ui::shell::SessionRow;
use steer_agent_chrome_ui::{HostHooks, MainSurface, Shell, ShellConfig, register_fonts};

struct StubView;

impl Render for StubView {
    fn render(&mut self, _w: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct StubMain {
    view: gpui::AnyView,
}

impl MainSurface for StubMain {
    fn view(&self) -> gpui::AnyView {
        self.view.clone()
    }

    fn title(&self, _cx: &gpui::App) -> gpui::SharedString {
        "Test".into()
    }
}

type BatchLog = Rc<RefCell<Vec<Vec<String>>>>;
type SelectLog = Rc<RefCell<Vec<String>>>;

fn thread_row(id: &str) -> SessionRow {
    SessionRow {
        id: id.into(),
        short_id: "abcd1234".into(),
        title: format!("thread {id}"),
        workspace: "Chats".into(),
        project: None,
        status: SessionStatus::Idle,
        kind: SessionRowKind::Thread,
        updated_at: 0,
        sort_stamp: 0,
        rank: 0,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

fn external_row(id: &str) -> SessionRow {
    SessionRow {
        kind: SessionRowKind::External {
            icon: steer_agent_chrome_ui::theme::icons::IconAsset("icons/claude.svg"),
        },
        status: SessionStatus::Running,
        rank: i64::MAX,
        ..thread_row(id)
    }
}

/// The hooks under test: the batch-archive and select callbacks log.
fn logging_hooks(batch_log: BatchLog, select_log: SelectLog) -> HostHooks {
    HostHooks {
        on_batch_archive: Some(Box::new(move |ids, _w, _cx| {
            batch_log.borrow_mut().push(ids.to_vec());
        })),
        on_select: Some(Box::new(move |id, _w, _cx| {
            select_log.borrow_mut().push(id.to_string());
        })),
        ..Default::default()
    }
}

/// Mount a shell over the rows; returns (visual, shell).
fn mount(
    cx: &mut TestAppContext,
    hooks: HostHooks,
    rows: Vec<SessionRow>,
) -> (VisualTestContext, gpui::Entity<Shell>) {
    cx.update(gpui_component::init);
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        steer_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        let config = ShellConfig {
            main: Arc::new(StubMain { view: main }),
            tool_kinds: vec![],
            panel_surface: None,
            brand: None,
            fixed_rows: vec![],
            customizations: vec![],
            hooks,
        };
        let shell = cx.new(|cx| Shell::new(config, window, cx));
        shell.update(cx, |s, _| s.set_sessions(rows));
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.into(), cx);
    let shell = slot.borrow().clone().expect("shell captured");
    (visual, shell)
}

fn picked(shell: &gpui::Entity<Shell>, cx: &mut TestAppContext) -> Option<Vec<String>> {
    shell.read_with(cx, |s, _| s.selection().map(<[String]>::to_vec))
}

/// 多选 checks the row it was invoked on; toggles add and remove; 全选 covers
/// every thread row and skips the external one; 归档 commits the batch
/// through ONE hook call and leaves selection mode. Since the container
/// lifecycle (plan A6) the ROW DROP is the host teardown path's own
/// optimistic half — the shell does not pre-drop (the path may confirm
/// first), so the test replays the path's drop face (`set_archived`) and
/// asserts the rows leave through it.
#[gpui::test]
fn selection_toggles_and_commits_the_batch_through_one_hook(cx: &mut TestAppContext) {
    let batch_log: BatchLog = Rc::new(RefCell::new(Vec::new()));
    let (mut visual, shell) = mount(
        cx,
        logging_hooks(batch_log.clone(), Rc::new(RefCell::new(Vec::new()))),
        vec![
            thread_row("t-1"),
            thread_row("t-2"),
            thread_row("t-3"),
            external_row("ext-1"),
        ],
    );

    assert_eq!(picked(&shell, cx), None, "not selecting until 多选");
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    assert_eq!(picked(&shell, cx), Some(vec!["t-1".to_string()]));

    shell.update(cx, |s, cx| s.toggle_row_selection("t-2", cx));
    shell.update(cx, |s, cx| s.toggle_row_selection("t-1", cx));
    assert_eq!(picked(&shell, cx), Some(vec!["t-2".to_string()]));

    // An external row is not selectable: the toggle is inert.
    shell.update(cx, |s, cx| s.toggle_row_selection("ext-1", cx));
    assert_eq!(picked(&shell, cx), Some(vec!["t-2".to_string()]));

    shell.update(cx, |s, cx| s.select_all_rows(cx));
    let mut all = picked(&shell, cx).expect("still selecting");
    all.sort();
    assert_eq!(
        all,
        vec!["t-1", "t-2", "t-3"],
        "全选 covers the thread rows only"
    );

    visual.run_until_parked();
    assert!(
        visual.debug_bounds("chrome-session-row-t-2").is_some(),
        "the rows paint while selecting"
    );

    shell.update_in(&mut visual, |s, window, cx| {
        s.commit_selection_archive(window, cx)
    });
    visual.run_until_parked();

    let log = batch_log.borrow();
    assert_eq!(
        log.len(),
        1,
        "one hook call carries the whole batch: {log:?}"
    );
    let mut committed = log[0].clone();
    committed.sort();
    assert_eq!(committed, vec!["t-1", "t-2", "t-3"]);
    drop(log);
    assert_eq!(picked(&shell, cx), None, "the commit leaves selection mode");
    // The shell did NOT pre-drop: the confirmed archive's row drop is the
    // host path's optimistic half (which may confirm with the dependents
    // first). Replaying that drop face (what the app's `drop_sidebar_row`
    // calls) removes the batch's rows.
    assert!(
        visual.debug_bounds("chrome-session-row-t-2").is_some(),
        "the rows stay until the host path drops them"
    );
    shell.update(cx, |s, cx| {
        for id in ["t-1", "t-2", "t-3"] {
            s.set_archived(id, true, cx);
        }
        cx.notify();
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("chrome-session-row-t-2").is_none(),
        "the host path's row drop removes the archived rows"
    );
    assert!(
        visual.debug_bounds("chrome-session-row-ext-1").is_some(),
        "the external row was never part of the batch"
    );
}

/// While selecting, a left click on a row toggles its check instead of
/// switching sessions — the host's select hook never fires.
#[gpui::test]
fn a_row_click_while_selecting_toggles_instead_of_switching(cx: &mut TestAppContext) {
    let select_log: SelectLog = Rc::new(RefCell::new(Vec::new()));
    let (mut visual, shell) = mount(
        cx,
        logging_hooks(Rc::new(RefCell::new(Vec::new())), select_log.clone()),
        vec![thread_row("t-1"), thread_row("t-2")],
    );
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    visual.run_until_parked();

    let row = visual
        .debug_bounds("chrome-session-row-t-2")
        .expect("the row paints");
    let at = row.center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();

    let mut now = picked(&shell, cx).expect("still selecting");
    now.sort();
    assert_eq!(now, vec!["t-1", "t-2"], "the click checked the row");
    assert!(
        select_log.borrow().is_empty(),
        "no session switch rides a selection-mode click: {:?}",
        select_log.borrow()
    );
}

/// 取消 leaves selection mode with every row still in place and no hook
/// call; an empty batch's 归档 commits nothing and stays in selection mode.
#[gpui::test]
fn cancel_and_an_empty_commit_archive_nothing(cx: &mut TestAppContext) {
    let batch_log: BatchLog = Rc::new(RefCell::new(Vec::new()));
    let (mut visual, shell) = mount(
        cx,
        logging_hooks(batch_log.clone(), Rc::new(RefCell::new(Vec::new()))),
        vec![thread_row("t-1"), thread_row("t-2")],
    );
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    shell.update(cx, |s, cx| s.toggle_row_selection("t-1", cx));
    assert_eq!(picked(&shell, cx), Some(vec![]));

    shell.update_in(&mut visual, |s, window, cx| {
        s.commit_selection_archive(window, cx)
    });
    assert!(
        batch_log.borrow().is_empty(),
        "an empty batch writes nothing"
    );
    assert_eq!(
        picked(&shell, cx),
        Some(vec![]),
        "and stays in selection mode"
    );

    shell.update(cx, |s, cx| s.exit_selection(cx));
    visual.run_until_parked();
    assert_eq!(picked(&shell, cx), None);
    assert!(batch_log.borrow().is_empty());
    assert!(visual.debug_bounds("chrome-session-row-t-1").is_some());
    assert!(visual.debug_bounds("chrome-session-row-t-2").is_some());
}

/// The selection is a set of live rows: a row that leaves the snapshot
/// (archived by another client, deleted) leaves the checked set too, so the
/// count and the commit never speak about a row that no longer exists.
#[gpui::test]
fn a_row_that_leaves_the_list_leaves_the_selection(cx: &mut TestAppContext) {
    let (visual, shell) = mount(
        cx,
        logging_hooks(
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        ),
        vec![thread_row("t-1"), thread_row("t-2")],
    );
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    shell.update(cx, |s, cx| s.toggle_row_selection("t-2", cx));

    shell.update(cx, |s, _| s.set_sessions(vec![thread_row("t-2")]));
    visual.run_until_parked();
    assert_eq!(
        picked(&shell, cx),
        Some(vec!["t-2".to_string()]),
        "the departed row is no longer checked"
    );
}

/// A host that offers selection but no batch-archive hook has no write path:
/// committing is inert (no optimistic drop, selection stays), so the rows do
/// not vanish until the next snapshot puts them back.
#[gpui::test]
fn a_host_without_the_batch_hook_commits_nothing(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(cx, HostHooks::default(), vec![thread_row("t-1")]);
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    shell.update_in(&mut visual, |s, window, cx| {
        s.commit_selection_archive(window, cx)
    });
    visual.run_until_parked();

    assert_eq!(
        picked(&shell, cx),
        Some(vec!["t-1".to_string()]),
        "the commit leaves selection mode untouched"
    );
    assert!(
        visual.debug_bounds("chrome-session-row-t-1").is_some(),
        "the row was never optimistically dropped"
    );
}

/// 全选 covers what the sidebar SHOWS: a collapsed group contributes nothing,
/// so its members cannot be archived unseen.
#[gpui::test]
fn select_all_skips_collapsed_groups(cx: &mut TestAppContext) {
    let (visual, shell) = mount(
        cx,
        logging_hooks(
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        ),
        vec![thread_row("t-1"), thread_row("t-2")],
    );
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    shell.update(cx, |s, _| s.toggle_group("Chats"));
    visual.run_until_parked();

    shell.update(cx, |s, cx| s.select_all_rows(cx));
    assert_eq!(
        picked(&shell, cx),
        Some(vec![]),
        "a collapsed group's rows are not swept into the batch"
    );
}

/// 全选 rides the same projection the list renders: an active search term
/// narrows the checked set, so 归档 cannot sweep a filtered-out row.
#[gpui::test]
fn select_all_respects_the_search_filter(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(
        cx,
        logging_hooks(
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        ),
        vec![thread_row("t-1"), thread_row("t-2")],
    );
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    shell.update_in(&mut visual, |s, window, cx| {
        s.toggle_sidebar_search(window, cx)
    });
    visual.simulate_input("t-2");

    shell.update(cx, |s, cx| s.select_all_rows(cx));
    assert_eq!(
        picked(&shell, cx),
        Some(vec!["t-2".to_string()]),
        "only the row the filter shows is checked"
    );
}

/// Escape leaves selection mode: the key bubbles from a focusable row up to
/// the sidebar root, so 取消 has a keyboard equivalent.
#[gpui::test]
fn escape_leaves_selection_mode(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(
        cx,
        logging_hooks(
            Rc::new(RefCell::new(Vec::new())),
            Rc::new(RefCell::new(Vec::new())),
        ),
        vec![thread_row("t-1"), thread_row("t-2")],
    );
    shell.update(cx, |s, cx| s.begin_selection("t-1", cx));
    visual.run_until_parked();
    let at = visual
        .debug_bounds("chrome-session-row-t-2")
        .expect("the row paints")
        .center();
    visual.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    visual.simulate_keystrokes("escape");

    assert_eq!(
        picked(&shell, cx),
        None,
        "escape cancels the selection like 取消 does"
    );
}
