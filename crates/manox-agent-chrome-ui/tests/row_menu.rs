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
use manox_agent_chrome_ui::session_list::{SessionGroup, SessionRowData, SessionStatus};
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
        brand: None,
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

/// A projection-shaped row (post-`project_forest`: members carry their
/// leader's stamp, everyone else their own clock).
fn row_data(id: &str, updated_at: i64, sort_stamp: i64, team_leader: bool) -> SessionRowData {
    SessionRowData {
        id: id.into(),
        title: "把 sidebar 的 thread 行改成三行布局".into(),
        updated_at,
        sort_stamp,
        status: SessionStatus::Idle,
        kind: manox_agent_chrome_ui::session_list::SessionRowKind::Thread,
        pinned: false,
        archived: false,
        tag: None,
        team_leader,
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
    // Built through `from_group` — the stamping under test lives there (via
    // the projection's `sort_stamp`), not in hand-written rows.
    let rows = SessionRow::from_group(SessionGroup {
        name: "Chats".into(),
        key: "Chats".into(),
        collapsed: false,
        project: None,
        rows: vec![
            row_data("leader-1", 300, 300, true),
            // Post-projection shape: the member carries its leader's stamp
            // (project_forest re-stamps it).
            row_data("member-1", 150, 300, false),
            // A standalone session trailing the team: it must keep its OWN
            // stamp, not inherit the team's.
            row_data("standalone", 200, 200, false),
        ],
    });
    // The projection stamps the member with its leader; the standalone
    // stays on its own clock.
    assert_eq!(
        rows.iter().map(|r| r.sort_stamp).collect::<Vec<_>>(),
        [300, 300, 200],
        "from_group must not leak the team stamp onto the standalone row"
    );

    let (_visual, shell) = mount(cx, Rc::new(RefCell::new(Vec::new())), rows);

    shell.update(cx, |s, cx| s.toggle_pin("leader-1", cx));
    let order: Vec<String> =
        shell.read_with(cx, |s, _| s.sessions.iter().map(|r| r.id.clone()).collect());
    assert_eq!(
        order,
        ["leader-1", "member-1", "standalone"],
        "the pinned team stays a unit; the standalone keeps its own recency"
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

/// Time grouping: rows bucket by sort_stamp, collapse rides the STABLE
/// state key (not the translated display name — the P0 regression where a
/// collapsed bucket re-expanded the very next frame), and drag reorder is
/// inert in time mode.
#[gpui::test]
fn time_grouping_buckets_collapse_and_reject_reorder(cx: &mut TestAppContext) {
    let rows = SessionRow::from_group(SessionGroup {
        name: "Chats".into(),
        key: "Chats".into(),
        collapsed: false,
        project: None,
        rows: vec![
            // Team sharing the leader's stamp; the member is NEWER on its
            // own clock (the classic "member just finished, leader idle")
            // — the shared sort_stamp must keep it below the leader in the
            // bucket order, not float it above the chevron. And a member
            // with a much OLDER own-clock stamp must still land in the
            // leader's bucket (bucketing by per-row updated_at was the
            // first-round defect this second row pins).
            row_data("leader", stamp_days_ago(0), stamp_days_ago(0), true),
            row_data("member", stamp_days_ago(0) + 60, stamp_days_ago(0), false),
            row_data("member-old", stamp_days_ago(40), stamp_days_ago(0), false),
            row_data("old", stamp_days_ago(30), stamp_days_ago(30), false),
        ],
    });
    let (mut visual, shell) = mount(cx, Rc::new(RefCell::new(Vec::new())), rows);

    shell.update(cx, |s, _cx| s.toggle_grouping());
    visual.run_until_parked();
    shell.update(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert_eq!(groups.len(), 2, "today + earlier");
        let today = &groups[0];
        assert_eq!(
            today.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["leader", "member", "member-old"],
            "the team buckets together regardless of each row's own clock, and the newer member stays under the chevron"
        );
    });

    // Collapse through a REAL CLICK on the rendered header — the write side
    // (which identity the header's on_click passes) is the defect this test
    // pins; calling toggle_group directly would bypass it.
    // The mode flip notified the shell; give the window its redraw so the
    // time-mode headers exist in the rendered frame.
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    let header_bounds = visual
        .debug_bounds("chrome-group-header-chrome-group-today")
        .expect("the today header paints");
    visual.simulate_click(header_bounds.center(), gpui::Modifiers::default());
    visual.run_until_parked();
    shell.update(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        let today = groups
            .iter()
            .find(|g| g.key == "chrome-group-today")
            .expect("today bucket survives");
        assert!(
            today.collapsed,
            "a clicked-header collapse must survive the next props build"
        );
        // The other bucket is untouched.
        assert!(!groups[1].collapsed);
    });

    // Reorder is inert in time mode: not only does the display order stay
    // (trivially true — time_groups never reads group_order), the RECORDING
    // must come out untouched — a dropped edge (the mode early-return) must
    // not leak the pseudo name into the persisted drag order.
    shell.update(cx, |s, _cx| {
        s.move_group("chrome-group-earlier", "chrome-group-today", true);
        assert!(
            s.group_order().is_empty(),
            "a time-mode drag must not record any order"
        );
        s.toggle_grouping();
    });
    shell.update(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert_eq!(
            groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(),
            ["Chats"],
            "the workspace order is exactly as it was before the time-mode drag"
        );
    });
}

/// The sidebar filter narrows rows case-insensitively over title / tag,
/// force-expands surviving groups, and its empty result reads as no groups.
#[gpui::test]
fn sidebar_filter_narrows_and_force_expands(cx: &mut TestAppContext) {
    let titled = |id: &str, title: &str, stamp: i64| SessionRowData {
        id: id.into(),
        title: title.into(),
        updated_at: stamp,
        sort_stamp: stamp,
        status: SessionStatus::Idle,
        kind: manox_agent_chrome_ui::session_list::SessionRowKind::Thread,
        pinned: false,
        archived: false,
        tag: None,
        team_leader: false,
    };
    let rows = SessionRow::from_group(SessionGroup {
        name: "proj".into(),
        key: "proj".into(),
        collapsed: false,
        project: None,
        rows: vec![
            titled("hit", "alpha design", stamp_days_ago(0)),
            titled("miss", "unrelated", stamp_days_ago(1)),
        ],
    });
    let (mut visual, shell) = mount(cx, Rc::new(RefCell::new(Vec::new())), rows);
    // Pre-collapse the group: an active filter must force it open.
    shell.update(cx, |s, _cx| s.toggle_group("proj"));

    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.toggle_sidebar_search(window, cx));
    });
    let input = shell
        .read_with(cx, |s, _| s.filter_input())
        .expect("the filter row mounts an input");
    visual.update(|window, cx| {
        input.update(cx, |state, cx| state.set_value("ALPHA", window, cx));
    });
    shell.update(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rows.len(), 1);
        assert_eq!(groups[0].rows[0].id, "hit");
        assert!(
            !groups[0].collapsed,
            "a matching group is force-expanded while filtering"
        );
    });
    // A term matching nothing leaves no groups.
    visual.update(|window, cx| {
        input.update(cx, |state, cx| state.set_value("zzz", window, cx));
    });
    shell.update(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        assert!(groups.is_empty());
    });
}

/// The shell's nav moves call the host hooks only when the move has an
/// edge (the dimmed-inert contract), and the landed id drives the
/// selection.
#[gpui::test]
fn nav_moves_call_hooks_only_at_live_edges(cx: &mut TestAppContext) {
    use manox_agent_chrome_ui::shell::NavAvail;
    let back_log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let fwd_log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let log = back_log.clone();
    let fwd = fwd_log.clone();
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    cx.update(gpui_component::init);
    let window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        manox_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        let mut config = shell_config(main, Rc::new(RefCell::new(Vec::new())));
        config.hooks.nav_avail = Some(Box::new(move |_| NavAvail {
            back: true,
            forward: false,
        }));
        let log = log.clone();
        config.hooks.on_nav_back = Some(Box::new(move |_w, _cx| {
            log.borrow_mut().push("back".into());
            Some("landed".into())
        }));
        config.hooks.on_nav_forward = Some(Box::new(move |_w, _cx| {
            fwd.borrow_mut().push("fwd".into());
            Some("fwd-landed".into())
        }));
        let shell = cx.new(|cx| Shell::new(config, window, cx));
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    let shell = slot.borrow().clone().expect("shell captured");

    // forward has no edge (nav_avail says so, and the hook would log):
    // the move must not fire it.
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.nav_forward(window, cx));
    });
    assert!(back_log.borrow().is_empty());
    assert!(
        fwd_log.borrow().is_empty(),
        "an edge-less forward must not call its hook"
    );
    // back is live: the hook fires and the landed id becomes the selection.
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.nav_back(window, cx));
    });
    assert_eq!(back_log.borrow().len(), 1);
    shell.read_with(cx, |s, _| assert_eq!(s.active.as_deref(), Some("landed")));

    // The RENDERED buttons carry their glyphs in BOTH tones at the same
    // geometry (the round-two regression: the enabled branch dropped the
    // inner element, so live arrows were empty boxes). The glyph underlay
    // has its own debug selector — the outer box exists in both tones, so
    // its bounds alone cannot see the difference.
    let back_glyph = visual
        .debug_bounds("tb-back-glyph")
        .expect("the enabled arrow draws its glyph");
    let fwd_glyph = visual
        .debug_bounds("tb-fwd-glyph")
        .expect("the disabled arrow still draws its glyph");
    assert_eq!(
        [back_glyph.size.width, back_glyph.size.height],
        [fwd_glyph.size.width, fwd_glyph.size.height],
        "both tones share the flat-button geometry"
    );

    // And the Disabled tone is inert at the click surface too: clicking the
    // rendered forward button must not dispatch its hook.
    visual.update(|window, cx| {
        shell.update(cx, |s, cx| s.nav_forward(window, cx));
    });
    assert!(
        fwd_log.borrow().is_empty(),
        "an edge-less forward stays inert through the rendered path"
    );
}

/// Unix seconds for local noon N days ago (noon survives DST shifts).
fn stamp_days_ago(days: usize) -> i64 {
    use chrono::TimeZone as _;
    let day = chrono::Local::now().date_naive() - chrono::Duration::days(days as i64);
    chrono::Local
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).expect("noon exists"))
        .single()
        .expect("local noon resolves")
        .timestamp()
}
