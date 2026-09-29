//! Chrome session-row interaction regressions: the right-click row menu
//! opens on the FIRST click (the first-click dead-trigger class the legacy
//! sidebar once had), and the inline tag editor's commit / cancel / empty /
//! clamp semantics hold at the shell level.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AppContext as _, Context, IntoElement, Modifiers, MouseButton, Render, TestAppContext,
    VisualTestContext, div, px, size,
};
use gpui_component::Root;
use manox_agent_chrome_ui::session_list::SessionStatus;
use manox_agent_chrome_ui::shell::SessionRow;
use manox_agent_chrome_ui::{HostHooks, MainSurface, Shell, ShellConfig, register_fonts};

/// A placeholder main-surface view (the shell always mounts one).
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

type TagLog = Rc<RefCell<Vec<(String, Option<String>)>>>;

fn shell_config(main: gpui::AnyView, set_tag_log: TagLog) -> ShellConfig {
    ShellConfig {
        main: Arc::new(StubMain { view: main }),
        tool_kinds: vec![],
        panel_surface: None,
        fixed_rows: vec![],
        customizations: vec![],
        hooks: HostHooks {
            on_set_tag: Some(Box::new(move |id, tag, _w, _cx| {
                set_tag_log.borrow_mut().push((id.to_string(), tag));
            })),
            ..Default::default()
        },
    }
}

fn sample_row(id: &str) -> SessionRow {
    SessionRow {
        id: id.into(),
        title: "把 sidebar 的 thread 行改成三行布局".into(),
        workspace: "Chats".into(),
        status: SessionStatus::Idle,
        updated_at: 0,
        sort_stamp: 0,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

/// Mount a shell with the given rows; returns (visual, shell).
fn mount(
    cx: &mut TestAppContext,
    set_tag_log: TagLog,
    rows: Vec<SessionRow>,
) -> (VisualTestContext, gpui::Entity<Shell>) {
    cx.update(gpui_component::init);
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        manox_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        let shell = cx.new(|cx| Shell::new(shell_config(main, set_tag_log), window, cx));
        shell.update(cx, |s, _| s.set_sessions(rows));
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.into(), cx);
    let shell = slot.borrow().clone().expect("shell captured");
    (visual, shell)
}

/// The row menu opens on the FIRST right-click and paints (the old-shell
/// regression class: an anchor written only for an already-open menu made
/// the trigger permanently dead).
#[gpui::test]
fn row_menu_opens_on_the_first_right_click(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        vec![sample_row("thread-1")],
    );

    let row_bounds = visual
        .debug_bounds("chrome-session-row-thread-1")
        .expect("the row paints");
    assert!(
        visual.debug_bounds("chrome-row-menu").is_none(),
        "the menu only exists after the click"
    );

    // The anchor is the click position itself (captured from the event).
    let click_at = row_bounds.center();
    visual.simulate_mouse_down(click_at, MouseButton::Right, Modifiers::default());
    visual.run_until_parked();

    assert!(
        shell.read_with(cx, |s, _| s.row_menu_open()),
        "the first right click must open the row menu"
    );
    let menu_bounds = visual
        .debug_bounds("chrome-row-menu")
        .expect("the menu paints on the first right click");
    // The anchored dropdown hangs BELOW the cursor (TopRight + the 2px
    // offset, bottom-flip tolerance included).
    let click_y = f32::from(click_at.y);
    assert!(
        f32::from(menu_bounds.top()) >= click_y && f32::from(menu_bounds.top()) <= click_y + 4.,
        "menu opens just below the cursor, top={:?} click_y={click_y}",
        f32::from(menu_bounds.top())
    );
}

/// Tag edit semantics: Enter-path commit (trimmed, hook + local row),
/// rename prefill, the 10-char clamp on real typing, empty-value discard,
/// and Escape cancel.
#[gpui::test]
fn tag_edit_commit_cancel_and_empty_semantics(cx: &mut TestAppContext) {
    let log: TagLog = Rc::new(RefCell::new(Vec::new()));
    let (mut visual, shell) = mount(cx, log.clone(), vec![sample_row("thread-1")]);

    // Begin (add mode) + commit a value: trimmed, rides the hook, lands on
    // the local row, editor unmounts.
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| {
            s.begin_tag_edit("thread-1".into(), false, window, cx)
        });
    });
    let (_id, input) = shell
        .read_with(cx, |s, _| s.tag_edit_input())
        .expect("the editor is mounted");
    visual.update(|window, cx| {
        input.update(cx, |state, cx| state.set_value("  hello  ", window, cx));
    });
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.commit_tag_edit(window, cx));
    });
    assert_eq!(
        log.borrow().as_slice(),
        [("thread-1".to_string(), Some("hello".to_string()))],
        "commit writes the trimmed tag through the hook"
    );
    assert!(
        shell.read_with(cx, |s, _| s.tag_edit_input().is_none()
            && s.sessions[0].tag.as_deref() == Some("hello")),
        "the editor unmounts and the row carries the tag"
    );

    // Rename mode prefills the current tag.
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| {
            s.begin_tag_edit("thread-1".into(), true, window, cx)
        });
    });
    let (_id, input) = shell
        .read_with(cx, |s, _| s.tag_edit_input())
        .expect("the editor is mounted");
    let prefill = visual.update(|_w, cx| input.read(cx).value().to_string());
    assert_eq!(prefill, "hello", "rename prefills the current tag");

    // Real typing clamps to the 10-char ceiling (the Change subscription).
    visual.simulate_input("abcdefghijklmn");
    let clamped = visual.update(|_w, cx| input.read(cx).value().to_string());
    assert_eq!(
        clamped.chars().count(),
        10,
        "typing clamps to MAX_THREAD_TAG_CHARS"
    );

    // A blank commit is silently discarded: no hook call, editor unmounts.
    visual.update(|window, cx| {
        input.update(cx, |state, cx| state.set_value("   ", window, cx));
    });
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.commit_tag_edit(window, cx));
    });
    assert_eq!(
        log.borrow().len(),
        1,
        "an empty value never reaches the hook"
    );
    assert!(shell.read_with(cx, |s, _| s.tag_edit_input().is_none()));

    // Escape cancels: editor unmounts, nothing written.
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| {
            s.begin_tag_edit("thread-1".into(), false, window, cx)
        });
    });
    visual.update(|_w, cx| {
        shell.update(cx, |s, cx| s.cancel_tag_edit(cx));
    });
    assert!(shell.read_with(cx, |s, _| s.tag_edit_input().is_none()));
    assert_eq!(log.borrow().len(), 1, "cancel writes nothing");
}

/// Hover must fire regardless of the approach direction: entering a row from
/// ABOVE (out of the group header) and from BELOW (out of the next row) both
/// flip the shell's hovered-row state.
#[gpui::test]
fn hover_fires_from_any_direction(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        vec![sample_row("thread-1")],
    );
    let row = visual
        .debug_bounds("chrome-session-row-thread-1")
        .expect("the row paints");

    // Park OUTSIDE the row, then enter from above.
    let above = gpui::point(row.center().x, row.top() - px(12.));
    visual.simulate_mouse_move(above, None, Modifiers::default());
    visual.run_until_parked();
    visual.simulate_mouse_move(row.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.hovered_row().map(str::to_string)),
        Some("thread-1".to_string()),
        "entering from above must hover the row"
    );

    // Park below the row (still inside the window), then enter from below.
    let below = gpui::point(row.center().x, row.bottom() + px(12.));
    visual.simulate_mouse_move(below, None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.hovered_row().map(str::to_string)),
        None,
        "leaving downward must clear the hover"
    );
    visual.simulate_mouse_move(row.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.hovered_row().map(str::to_string)),
        Some("thread-1".to_string()),
        "entering from below must hover the row"
    );
}

/// Crossing DIRECTLY from one row into the next must land the hover on the
/// row the pointer is in. One mouse-move dispatches the old row's leave AND
/// the new row's enter; whichever order they arrive in, a stale leave must
/// not clobber the fresh enter (the real-window crossing used to strand the
/// hover on None — no marquee until the pointer moved again).
#[gpui::test]
fn crossing_rows_lands_the_hover_on_the_row_under_the_pointer(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        vec![sample_row("thread-1"), sample_row("thread-2")],
    );
    let row1 = visual
        .debug_bounds("chrome-session-row-thread-1")
        .expect("row 1 paints");
    let row2 = visual
        .debug_bounds("chrome-session-row-thread-2")
        .expect("row 2 paints");

    visual.simulate_mouse_move(row1.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.hovered_row().map(str::to_string)),
        Some("thread-1".to_string()),
        "sanity: the first row hovers"
    );

    // THE crossing under test: row 1 -> row 2 in one move.
    visual.simulate_mouse_move(row2.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.hovered_row().map(str::to_string)),
        Some("thread-2".to_string()),
        "crossing into row 2 must hover row 2"
    );

    // And back up: row 2 -> row 1.
    visual.simulate_mouse_move(row1.center(), None, Modifiers::default());
    visual.run_until_parked();
    assert_eq!(
        shell.read_with(cx, |s, _| s.hovered_row().map(str::to_string)),
        Some("thread-1".to_string()),
        "crossing back into row 1 must hover row 1"
    );
}

/// Pinning a team leader must keep the team contiguous: members carry their
/// leader's sort stamp, so the pinned-first re-order cannot strand a member
/// behind the next team (the chevron is the only hierarchy marker).
#[gpui::test]
fn pinning_a_leader_keeps_its_members_contiguous(cx: &mut TestAppContext) {
    let (mut visual, shell) = mount(
        cx,
        Rc::new(RefCell::new(Vec::new())),
        vec![
            SessionRow {
                id: "leader-1".into(),
                team_leader: true,
                updated_at: 300,
                sort_stamp: 300,
                ..sample_row("leader-1")
            },
            SessionRow {
                id: "member-1".into(),
                updated_at: 150,
                sort_stamp: 300,
                ..sample_row("member-1")
            },
            SessionRow {
                id: "leader-2".into(),
                updated_at: 200,
                sort_stamp: 200,
                ..sample_row("leader-2")
            },
        ],
    );
    let _ = &mut visual;

    shell.update(cx, |s, cx| s.toggle_pin("leader-1", cx));
    let order: Vec<String> =
        shell.read_with(cx, |s, _| s.sessions.iter().map(|r| r.id.clone()).collect());
    assert_eq!(
        order,
        ["leader-1", "member-1", "leader-2"],
        "the pinned team stays a unit; a member must not sink behind leader-2"
    );
}

/// The REAL editor wiring: typing + Enter reaches the commit subscription,
/// Escape reaches the cancel action through the input's propagation — not
/// just the shell methods called directly.
#[gpui::test]
fn tag_edit_wires_enter_and_escape_keystrokes(cx: &mut TestAppContext) {
    let log: TagLog = Rc::new(RefCell::new(Vec::new()));
    let (mut visual, shell) = mount(cx, log.clone(), vec![sample_row("thread-1")]);

    visual.update(|window, cx| {
        shell.update(cx, |s, cx| {
            s.begin_tag_edit("thread-1".into(), false, window, cx)
        });
    });
    visual.simulate_input("typed");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    assert_eq!(
        log.borrow().as_slice(),
        [("thread-1".to_string(), Some("typed".to_string()))],
        "enter commits through the input subscription"
    );
    assert!(shell.read_with(cx, |s, _| s.tag_edit_input().is_none()));

    visual.update(|window, cx| {
        shell.update(cx, |s, cx| {
            s.begin_tag_edit("thread-1".into(), false, window, cx)
        });
    });
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(
        shell.read_with(cx, |s, _| s.tag_edit_input().is_none()),
        "escape cancels through the propagated action"
    );
    assert_eq!(log.borrow().len(), 1, "escape writes nothing");
}
