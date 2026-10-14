//! FULL-WIRING regression for the project group menu's Steer row: clicking
//! 新建会话 → Steer must land a NEW foreground session on the REAL
//! workspace + AHP host.
//!
//! The regression class: the row's click closure used to rebuild the
//! `&mut Window` through `dispatch::window_global().update(...)` — inside a
//! click dispatch the window is already leased, so the re-entrant update
//! fails as "window not found" (swallowed by `let _ =`), and the row did
//! NOTHING: no session created, no conversation landing. The fix routes the
//! click's own `&mut Window` through a plain workspace update, exactly like
//! the sibling terminal/agent rows.
//!
//! Own test binary: the harness initializes process-global singletons (see
//! `tests/common/mod.rs` — one `#[gpui::test]` per binary), and this one
//! dials the REAL in-proc host, so the store's state root must be hermetic.
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use gpui::{AppContext as _, TestAppContext, VisualTestContext, px, size};
use gpui_component::Root;
use steer_agent_chrome_ui::register_fonts;

/// The real-host handshake and the create/attach round-trip complete on the
/// tokio runtime and only then wake the gpui side; poll instead of a single
/// `run_until_parked`.
const PARK_ROUNDS: usize = 100;
const PARK_STEP: std::time::Duration = std::time::Duration::from_millis(50);

#[gpui::test]
async fn steer_row_click_lands_a_new_thread(cx: &mut TestAppContext) {
    // Hermetic HOME: the real host boots against a throwaway state root, not
    // the developer's ~/.manox.
    cx.update(|_cx| {
        manox_agent::runtime::init_hermetic_for_test();
        manox_agent::provider_glue::init();
        manox_agent::thread_store::init();
    });
    cx.background_executor.allow_parking();

    let ws_cell: std::rc::Rc<std::cell::RefCell<Option<gpui::Entity<Workspace>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let shell_cell: std::rc::Rc<
        std::cell::RefCell<Option<gpui::Entity<steer_agent_chrome_ui::Shell>>>,
    > = std::rc::Rc::new(std::cell::RefCell::new(None));
    let (ws_for_build, shell_for_build) = (ws_cell.clone(), shell_cell.clone());
    cx.update(gpui_component::init);
    cx.update(|_cx| {
        steer_i18n::init();
    });
    let window = cx.open_window(size(px(1_280.), px(820.)), move |window, cx| {
        register_fonts(cx);
        let shell = agent_ui::chrome_assembly::mount(window, cx);
        let ws = agent_ui::dispatch::workspace_global().expect("assembly workspace");
        *ws_for_build.borrow_mut() = Some(ws);
        *shell_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let ws = ws_cell.borrow().clone().expect("workspace captured");
    let shell = shell_cell.borrow().clone().expect("shell captured");
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        if cx.update(|cx| ws.read(cx).diagnostic_ahp_connected(cx)) {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    cx.run_until_parked();

    let before = cx.update(|cx| {
        ws.read(cx)
            .diagnostic_foreground_session(cx)
            .expect("landing session bound")
    });

    // Open the production group menu on the real shell.
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        shell.update(cx, |s, cx| {
            s.open_group_menu(
                "regression",
                Some("/tmp/steer-regression-proj"),
                gpui::point(px(400.), px(200.)),
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    let menu = visual
        .debug_bounds("chrome-group-menu")
        .expect("the group menu paints");

    // The mouse flow the menu is built for: hover the 新建会话 row (the
    // first row; panel p_1 padding + half a 26px row) to open its submenu,
    // then click the submenu's first row — Steer — anchored to the row's
    // right.
    let row0 = gpui::point(
        menu.origin.x + menu.size.width / 2.,
        menu.origin.y + px(17.),
    );
    visual.simulate_mouse_move(row0, None, gpui::Modifiers::default());
    for _ in 0..3 {
        cx.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    let steer = gpui::point(menu.origin.x + menu.size.width + px(60.), row0.y);
    visual.simulate_click(steer, gpui::Modifiers::default());
    for _ in 0..3 {
        cx.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    let mut after = None;
    for _ in 0..PARK_ROUNDS {
        cx.run_until_parked();
        after = cx.update(|cx| ws.read(cx).diagnostic_foreground_session(cx));
        if after.as_deref() != Some(before.as_str()) {
            break;
        }
        std::thread::sleep(PARK_STEP);
    }
    assert_ne!(
        Some(before),
        after,
        "the Steer menu row must start a new foreground thread"
    );
    manox_agent::thread_store::drop_global_for_test();
    agent_ui::dispatch::clear_globals_for_test();
}
