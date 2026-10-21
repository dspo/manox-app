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

/// Mount a shell over the rows with the batch-archive and select hooks
/// logging; returns (visual, shell).
fn mount(
    cx: &mut TestAppContext,
    batch_log: BatchLog,
    select_log: SelectLog,
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
            hooks: HostHooks {
                on_batch_archive: Some(Box::new(move |ids, _w, _cx| {
                    batch_log.borrow_mut().push(ids.to_vec());
                })),
                on_select: Some(Box::new(move |id, _w, _cx| {
                    select_log.borrow_mut().push(id.to_string());
                })),
                ..Default::default()
            },
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
/// through ONE hook call, drops the rows and leaves selection mode.
#[gpui::test]
fn selection_toggles_and_commits_the_batch_through_one_hook(cx: &mut TestAppContext) {
    let batch_log: BatchLog = Rc::new(RefCell::new(Vec::new()));
    let (mut visual, shell) = mount(
        cx,
        batch_log.clone(),
        Rc::new(RefCell::new(Vec::new())),
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
    assert!(
        visual.debug_bounds("chrome-session-row-t-2").is_none(),
        "the archived rows leave the sidebar at the commit"
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
        Rc::new(RefCell::new(Vec::new())),
        select_log.clone(),
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
        batch_log.clone(),
        Rc::new(RefCell::new(Vec::new())),
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
