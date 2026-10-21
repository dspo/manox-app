//! The sidebar group fold: a group renders its first `GROUP_ROW_LIMIT` (5)
//! rows and hides the rest behind a "show more (N)" footer; the footer and
//! the group's collapse chevron are the two reset paths, and a fold hiding
//! the ACTIVE row lifts itself once per active row (a manual show-less
//! after that sticks until the foreground thread moves again).

use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Context, IntoElement, Render, TestAppContext, div, px, size};
use gpui_component::Root;
use steer_agent_chrome_ui::shell::{GROUP_ROW_LIMIT, SessionRow, Shell, ShellConfig};
use steer_agent_chrome_ui::{HostHooks, MainSurface, register_fonts};

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

/// One project-bound thread row; stamps are handed out increasing and the
/// host ranks newer rows better, so the sort surfaces them NEWEST first
/// (n is the wire push index, not the display position).
fn row(id: &str, stamp: i64) -> SessionRow {
    SessionRow {
        id: id.into(),
        short_id: id.chars().take(8).collect(),
        title: id.into(),
        workspace: "proj".into(),
        project: Some("/p/proj".into()),
        status: steer_agent_chrome_ui::session_list::SessionStatus::Idle,
        kind: steer_agent_chrome_ui::session_list::SessionRowKind::Thread,
        updated_at: stamp,
        sort_stamp: stamp,
        rank: i64::MAX - stamp,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

fn mount(
    cx: &mut TestAppContext,
    rows: Vec<SessionRow>,
) -> (gpui::VisualTestContext, gpui::Entity<Shell>) {
    cx.update(gpui_component::init);
    let slot: Rc<std::cell::RefCell<Option<gpui::Entity<Shell>>>> = Rc::default();
    let slot_for_build = slot.clone();
    // Tall enough that the expanded group (7 rows + footer) stays inside the
    // scroll viewport — a footer clipped by the overflow edge cannot be
    // clicked through the window coordinate space.
    let window = cx.open_window(size(px(900.), px(780.)), move |window, cx| {
        steer_i18n::init();
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
                    hooks: HostHooks::default(),
                },
                window,
                cx,
            )
        });
        shell.update(cx, |s, _| s.set_sessions(rows.clone()));
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let shell = slot.borrow().clone().expect("shell captured");
    (
        gpui::VisualTestContext::from_window(window.into(), cx),
        shell,
    )
}

/// Seven rows in one project: the group folds at the limit, the footer
/// carries the hidden count, the hidden rows are the group's tail (oldest
/// stamps) — in BOTH grouping modes.
#[gpui::test]
fn a_long_group_folds_at_the_row_limit(cx: &mut TestAppContext) {
    let rows: Vec<SessionRow> = (1..=7).map(|n| row(&format!("r{n}"), 100 * n)).collect();
    let (_visual, shell) = mount(cx, rows);

    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.rows.len(), GROUP_ROW_LIMIT, "the visible five");
        assert_eq!(g.hidden, 2);
        assert!(!g.expanded);
        // The sort is newest-first, so the hidden tail is the OLDEST pair.
        assert_eq!(g.rows.last().map(|r| r.id.as_str()), Some("r3"));
    });

    // Time grouping folds the same way (the fold pass is mode-uniform).
    shell.update(cx, |s, _| s.toggle_grouping());
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        let g = &groups[0];
        assert_eq!(g.rows.len(), GROUP_ROW_LIMIT);
        assert_eq!(g.hidden, 2);
    });
}

/// The footer is a real control: clicking "show more" lifts the fold and the
/// footer flips to "show less"; clicking that lays the fold back down.
#[gpui::test]
fn the_footer_toggles_the_fold(cx: &mut TestAppContext) {
    let rows: Vec<SessionRow> = (1..=7).map(|n| row(&format!("r{n}"), 100 * n)).collect();
    let (mut visual, _shell) = mount(cx, rows);

    let more = visual
        .debug_bounds("chrome-group-fold-more-/p/proj")
        .expect("the show-more footer paints over the fold");
    assert!(
        visual.debug_bounds("chrome-session-row-r1").is_none(),
        "the hidden tail does not paint"
    );

    visual.simulate_click(more.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("chrome-session-row-r1").is_some(),
        "show more lifts the fold"
    );
    let less = visual
        .debug_bounds("chrome-group-fold-less-/p/proj")
        .expect("the footer flips to show less");

    visual.simulate_click(less.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("chrome-session-row-r1").is_none(),
        "show less lays the fold back down"
    );
    assert!(
        visual
            .debug_bounds("chrome-group-fold-more-/p/proj")
            .is_some(),
        "the footer flips back to show more"
    );
}

/// Collapsing the group resets the fold: expanding, collapsing (the chevron),
/// and expanding again lands back at the row limit — the lifted fold does not
/// survive a group collapse.
#[gpui::test]
fn collapsing_the_group_resets_the_fold(cx: &mut TestAppContext) {
    let rows: Vec<SessionRow> = (1..=7).map(|n| row(&format!("r{n}"), 100 * n)).collect();
    let (_visual, shell) = mount(cx, rows);

    shell.update(cx, |s, _| s.toggle_show_more("/p/proj"));
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert_eq!(groups[0].rows.len(), 7, "expanded");
    });

    shell.update(cx, |s, _| s.toggle_group("/p/proj"));
    shell.update(cx, |s, _| s.toggle_group("/p/proj"));
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert_eq!(
            groups[0].rows.len(),
            GROUP_ROW_LIMIT,
            "the fold resets with the group collapse"
        );
        assert_eq!(groups[0].hidden, 2);
    });
}

/// While a filter is active the fold is lifted entirely: every surviving
/// row renders (a match beyond the 5-row limit must not hide behind a
/// "show more" footer) and no footer paints.
#[gpui::test]
fn an_active_filter_lifts_the_fold(cx: &mut TestAppContext) {
    let rows: Vec<SessionRow> = (1..=7).map(|n| row(&format!("r{n}"), 100 * n)).collect();
    let (mut visual, shell) = mount(cx, rows);

    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.toggle_sidebar_search(window, cx));
    });
    let input = shell
        .read_with(cx, |s, _| s.filter_input())
        .expect("the filter row mounts an input");
    visual.update(|window, cx| {
        input.update(cx, |state, cx| state.set_value("r7", window, cx));
    });
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        let g = &groups[0];
        assert_eq!(g.rows.len(), 1, "the tail match survives");
        assert_eq!(g.rows[0].id, "r7");
        assert_eq!(g.hidden, 0, "nothing hides behind the fold while filtering");
        assert!(!g.collapsed, "the group force-expands");
    });
    assert!(
        visual
            .debug_bounds("chrome-group-fold-more-/p/proj")
            .is_none(),
        "no footer paints while filtering"
    );
}

/// A fold hiding the ACTIVE row lifts itself once per active row: the reveal
/// fires on arrival, a manual show-less sticks while the foreground thread
/// stays put, and a NEW hidden foreground row lifts it again.
#[gpui::test]
fn a_fold_hiding_the_active_row_reveals_once_per_active_row(cx: &mut TestAppContext) {
    let rows: Vec<SessionRow> = (1..=7).map(|n| row(&format!("r{n}"), 100 * n)).collect();
    let (_visual, shell) = mount(cx, rows);

    // The foreground thread sits in the hidden tail (r1 is the oldest row).
    shell.update(cx, |s, _| s.active = Some("r1".into()));
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert!(groups[0].expanded, "the fold hiding the active row lifts");
        assert_eq!(groups[0].rows.len(), 7);
    });

    // The user lays the fold back down: it must NOT bounce up while r1
    // stays the foreground thread.
    shell.update(cx, |s, _| s.toggle_show_more("/p/proj"));
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert!(!groups[0].expanded, "the manual show-less sticks");
        assert_eq!(groups[0].hidden, 2);
    });

    // A different hidden row becomes the foreground: the reveal fires again.
    shell.update(cx, |s, _| s.active = Some("r2".into()));
    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert!(groups[0].expanded, "a new active row re-arms the reveal");
    });
}
