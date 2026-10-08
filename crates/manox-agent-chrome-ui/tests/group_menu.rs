//! Chrome group-header menu regressions: the project-menu surface opens from
//! BOTH its triggers (the header's ellipsis button and a header right-click),
//! hands the host the group key AND the project path, one menu open at a
//! time, and disappears in time grouping — a time bucket is not a launch
//! target.

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

/// What the host hook saw: (group key, project path, open position captured).
type MenuLog = Rc<RefCell<Vec<(String, Option<String>)>>>;

fn project_row(id: &str, workspace: &str, project: &str) -> SessionRow {
    SessionRow {
        id: id.into(),
        title: "把 sidebar 的 thread 行改成三行布局".into(),
        workspace: workspace.into(),
        project: Some(project.into()),
        status: SessionStatus::Idle,
        kind: manox_agent_chrome_ui::session_list::SessionRowKind::Thread,
        updated_at: 0,
        sort_stamp: 0,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

fn chats_row(id: &str) -> SessionRow {
    SessionRow {
        id: id.into(),
        title: "把 sidebar 的 thread 行改成三行布局".into(),
        workspace: "Chats".into(),
        project: None,
        status: SessionStatus::Idle,
        kind: manox_agent_chrome_ui::session_list::SessionRowKind::Thread,
        updated_at: 0,
        sort_stamp: 0,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

fn shell_config(menu_log: MenuLog, main: gpui::AnyView) -> ShellConfig {
    ShellConfig {
        main: Arc::new(StubMain { view: main }),
        tool_kinds: vec![],
        panel_surface: None,
        brand: None,
        fixed_rows: vec![],
        customizations: vec![],
        hooks: HostHooks {
            on_group_menu: Some(Box::new(move |key, project, _anchor, window, cx| {
                menu_log
                    .borrow_mut()
                    .push((key.to_string(), project.map(str::to_string)));
                Some(gpui_component::menu::PopupMenu::build(
                    window,
                    cx,
                    |menu, _, _| menu.item(gpui_component::menu::PopupMenuItem::new("stub")),
                ))
            })),
            ..Default::default()
        },
    }
}

/// Mount a shell with the given rows; returns (visual, shell, menu log).
fn mount(
    cx: &mut TestAppContext,
    rows: Vec<SessionRow>,
) -> (VisualTestContext, gpui::Entity<Shell>, MenuLog) {
    let log: MenuLog = Rc::new(RefCell::new(Vec::new()));
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let log_for_build = log.clone();
    cx.update(gpui_component::init);
    let window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        manox_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        let shell = cx.new(|cx| Shell::new(shell_config(log_for_build, main), window, cx));
        shell.update(cx, |s, _| s.set_sessions(rows));
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let visual = VisualTestContext::from_window(window.into(), cx);
    let shell = slot.borrow().clone().expect("shell captured");
    (visual, shell, log)
}

/// The header right-click opens the host-built menu, carrying the group key
/// and the REAL project path (the menu's launch target), and the menu paints
/// anchored below the click.
#[gpui::test]
fn header_right_click_opens_the_group_menu(cx: &mut TestAppContext) {
    let (mut visual, shell, log) = mount(
        cx,
        vec![project_row("thread-1", "manox", "/home/u/projs/manox")],
    );

    let header = visual
        .debug_bounds("chrome-group-header-manox")
        .expect("the group header paints");
    assert!(
        visual.debug_bounds("chrome-group-menu").is_none(),
        "the menu only exists after the trigger"
    );

    visual.simulate_mouse_down(header.center(), MouseButton::Right, Modifiers::default());
    visual.run_until_parked();

    assert!(
        shell.read_with(cx, |s, _| s.group_menu_open()),
        "the right click must open the group menu"
    );
    visual
        .debug_bounds("chrome-group-menu")
        .expect("the menu paints");
    assert_eq!(
        log.borrow().as_slice(),
        [("manox".to_string(), Some("/home/u/projs/manox".to_string()))],
        "the host receives the key and the project path"
    );
}

/// The ellipsis button is a second trigger (the legacy header's `...`), and
/// it must not ALSO toggle the group's collapse — the click stops there.
#[gpui::test]
fn ellipsis_button_opens_without_toggling_collapse(cx: &mut TestAppContext) {
    let (mut visual, shell, log) = mount(
        cx,
        vec![project_row("thread-1", "manox", "/home/u/projs/manox")],
    );

    let button = visual
        .debug_bounds("chrome-group-menu-btn-manox")
        .expect("the ellipsis button paints");
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();

    assert!(
        shell.read_with(cx, |s, _| s.group_menu_open()),
        "the button opens the group menu"
    );
    assert_eq!(
        log.borrow().as_slice(),
        [("manox".to_string(), Some("/home/u/projs/manox".to_string()))],
    );
    // The rows still paint: the click never reached the header's collapse
    // toggle.
    assert!(
        visual.debug_bounds("chrome-session-row-thread-1").is_some(),
        "the row still paints — the group did not collapse"
    );
}

/// The no-project bucket keeps the menu (the flat launch surface: session /
/// terminal / editor at the fallback cwd) but the host sees no path — that
/// is the menu's remove-project absence signal.
#[gpui::test]
fn chats_group_menu_carries_no_project(cx: &mut TestAppContext) {
    let (mut visual, _shell, log) = mount(cx, vec![chats_row("thread-1")]);

    let button = visual
        .debug_bounds("chrome-group-menu-btn-Chats")
        .expect("the Chats header still carries the button");
    visual.simulate_click(button.center(), Modifiers::default());
    visual.run_until_parked();

    assert_eq!(
        log.borrow().as_slice(),
        [("Chats".to_string(), None)],
        "the no-project group opens the menu with no path"
    );
}

/// Time grouping has no menu surface anywhere: a time bucket is not a launch
/// target, so neither the button nor the right-click may fire. (Every row's
/// stamp is 0 → the "earlier" bucket; the bucket's state key doubles as the
/// group key, so the selector repeats it.)
#[gpui::test]
fn time_grouping_hides_the_group_menu(cx: &mut TestAppContext) {
    let (mut visual, shell, log) = mount(
        cx,
        vec![project_row("thread-1", "manox", "/home/u/projs/manox")],
    );
    shell.update(cx, |s, cx| {
        s.toggle_grouping();
        cx.notify();
    });
    visual.run_until_parked();

    let header = visual
        .debug_bounds("chrome-group-header-chrome-group-earlier")
        .expect("the earlier bucket header paints");
    assert!(
        visual
            .debug_bounds("chrome-group-menu-btn-chrome-group-earlier")
            .is_none(),
        "no ellipsis button in time grouping"
    );

    visual.simulate_mouse_down(header.center(), MouseButton::Right, Modifiers::default());
    visual.run_until_parked();
    assert!(
        !shell.read_with(cx, |s, _| s.group_menu_open()),
        "a header right-click in time grouping opens nothing"
    );
    assert!(log.borrow().is_empty());
}

/// The external-session open path's exact shape: a row click's select hook
/// runs INSIDE the window's dispatch, and the pane open round-trips through
/// the window handle (`handle.update` → `shell.update`). This test pins what
/// that nested round-trip does when dispatched from a live click — the
/// reason the external open is a pure workspace update
/// (`Workspace::open_external_session`) instead of a window-handle
/// round-trip.
#[gpui::test]
fn nested_window_update_from_a_select_hook(cx: &mut TestAppContext) {
    let nested_ran = Rc::new(RefCell::new(false));
    let nested_ok = Rc::new(RefCell::new(None));
    let shell_slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let window_slot: Rc<RefCell<Option<gpui::AnyWindowHandle>>> = Rc::new(RefCell::new(None));

    let (nested_for_hook, ok_for_hook, shell_for_hook, window_for_hook) = (
        nested_ran.clone(),
        nested_ok.clone(),
        shell_slot.clone(),
        window_slot.clone(),
    );
    let on_select: manox_agent_chrome_ui::shell::HookOnId = Box::new(move |_id, _w, cx| {
        let Some(handle) = *window_for_hook.borrow() else {
            return;
        };
        let Some(shell) = shell_for_hook.borrow().clone() else {
            return;
        };
        // The open_tool_tab round-trip, verbatim.
        let result = handle.update(cx, |_root, _window, cx| {
            shell.update(cx, |_shell, cx| cx.notify());
        });
        *nested_for_hook.borrow_mut() = true;
        *ok_for_hook.borrow_mut() = Some(result.is_ok());
    });

    let shell_for_build = shell_slot.clone();
    let window_for_build = window_slot.clone();

    cx.update(gpui_component::init);
    let window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        manox_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        *window_for_build.borrow_mut() = Some(window.window_handle());
        let shell = cx.new(|cx| {
            Shell::new(
                ShellConfig {
                    main: Arc::new(StubMain { view: main }),
                    tool_kinds: vec![],
                    panel_surface: None,
                    brand: None,
                    fixed_rows: vec![],
                    customizations: vec![],
                    hooks: HostHooks {
                        on_select: Some(on_select),
                        ..Default::default()
                    },
                },
                window,
                cx,
            )
        });
        shell.update(cx, |s, _| {
            s.set_sessions(vec![project_row("thread-1", "manox", "/p")])
        });
        *shell_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let shell = shell_slot.borrow().clone().expect("shell captured");

    let row = visual
        .debug_bounds("chrome-session-row-thread-1")
        .expect("the row paints");
    visual.simulate_click(row.center(), Modifiers::default());
    visual.run_until_parked();

    assert!(*nested_ran.borrow(), "the select hook ran on the click");
    // The nested round-trip FAILS while the window is dispatching (the
    // shell.update re-enters the mid-update window): a window-handle
    // round-trip from a click path is a silent no-op. That is the semantics
    // that forced the external open's shape — a pure workspace update, no
    // handle round-trip; this assertion pins it.
    assert_eq!(
        *nested_ok.borrow(),
        Some(false),
        "a nested window update from a live click is rejected"
    );
    let active = shell.read_with(cx, |s, _| s.active.clone());
    assert_eq!(active.as_deref(), Some("thread-1"), "the shell survived");
}

/// The external row's menu is the legacy close-session semantics: 关闭会话
/// rides `on_close_external` (the host closes the tab AND reaps the row) —
/// no thread verbs (pin/archive/tag) on a PTY.
#[gpui::test]
fn external_row_menu_closes_the_session(cx: &mut TestAppContext) {
    let closed: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let closed_for_build = closed.clone();
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();

    cx.update(gpui_component::init);
    let window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        manox_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        let shell = cx.new(|cx| {
            Shell::new(
                ShellConfig {
                    main: Arc::new(StubMain { view: main }),
                    tool_kinds: vec![],
                    panel_surface: None,
                    brand: None,
                    fixed_rows: vec![],
                    customizations: vec![],
                    hooks: HostHooks {
                        on_close_external: Some(Box::new(move |id, _w, _cx| {
                            closed_for_build.borrow_mut().push(id.to_string());
                        })),
                        ..Default::default()
                    },
                },
                window,
                cx,
            )
        });
        shell.update(cx, |s, _| {
            s.set_sessions(vec![SessionRow {
                id: "ext-0001".into(),
                title: "Claude Code".into(),
                workspace: "manox".into(),
                project: Some("/p/manox".into()),
                status: SessionStatus::Running,
                kind: manox_agent_chrome_ui::session_list::SessionRowKind::External {
                    icon: manox_agent_chrome_ui::theme::icons::IconAsset("icons/claude.svg"),
                },
                updated_at: 100,
                sort_stamp: 100,
                pinned: false,
                archived: false,
                tag: None,
                team_leader: false,
            }])
        });
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let shell = slot.borrow().clone().expect("shell captured");

    let row = visual
        .debug_bounds("chrome-session-row-ext-0001")
        .expect("the external row paints");
    visual.simulate_mouse_down(row.center(), MouseButton::Right, Modifiers::default());
    visual.run_until_parked();

    assert!(
        shell.read_with(cx, |s, _| s.row_menu_open()),
        "the external row opens its menu"
    );
    let menu = visual
        .debug_bounds("chrome-row-menu")
        .expect("the menu paints");

    // The first item is 关闭会话 — clicking it fires the close hook.
    visual.simulate_click(
        gpui::point(menu.center().x, menu.top() + px(14.)),
        Modifiers::default(),
    );
    visual.run_until_parked();

    assert_eq!(
        closed.borrow().as_slice(),
        ["ext-0001"],
        "关闭会话 routes the row id through the host hook"
    );
    assert!(
        shell.read_with(cx, |s, _| !s.row_menu_open()),
        "the menu dismissed after the action"
    );
}
