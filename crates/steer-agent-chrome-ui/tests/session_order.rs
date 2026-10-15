//! Session-list ordering: a freshly created session (the newest sort stamp)
//! must lead its group, pinned rows above it — in BOTH grouping modes. The
//! regression class: `set_sessions` used to keep the push order verbatim, so
//! the assembly's external rows (appended at the snapshot's tail — a just
//! launched Claude Code session) rendered at the group's bottom while the
//! time buckets already sorted by stamp.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{AppContext as _, Context, IntoElement, Render, TestAppContext, div, px, size};
use gpui_component::Root;
use steer_agent_chrome_ui::session_list::{SessionRowKind, SessionStatus};
use steer_agent_chrome_ui::shell::{SessionRow, Shell, ShellConfig};
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

fn row(id: &str, stamp: i64, pinned: bool, project: Option<&str>) -> SessionRow {
    let workspace = project
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.to_string())
        })
        .unwrap_or_else(|| "Chats".into());
    SessionRow {
        id: id.into(),
        short_id: id.chars().take(8).collect(),
        title: id.into(),
        workspace,
        project: project.map(str::to_string),
        status: SessionStatus::Running,
        kind: SessionRowKind::External {
            icon: steer_agent_chrome_ui::theme::icons::IconAsset("icons/claude.svg"),
        },
        updated_at: stamp,
        sort_stamp: stamp,
        pinned,
        archived: false,
        tag: None,
        team_leader: false,
    }
}

/// The push order is the assembly's merge order: existing threads first, the
/// freshly spawned external session LAST. The workspace grouping (the
/// default) must still render it first — pinned rows above everything.
#[gpui::test]
fn newest_session_leads_its_group_in_the_workspace_grouping(cx: &mut TestAppContext) {
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    cx.update(gpui_component::init);
    let _window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
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
        shell.update(cx, |s, _| {
            // The stale thread leads the push; the pinned relic is older
            // still; the just-created external session trails the snapshot.
            s.set_sessions(vec![
                row("stale", 100, false, Some("/p/proj")),
                row("relic", 10, true, Some("/p/proj")),
                row("fresh", 500, false, Some("/p/proj")),
            ])
        });
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let shell = slot.borrow().clone().expect("shell captured");

    let (_, groups, _) = shell.read_with(cx, |s, cx| s.sidebar_props(cx));
    let group = groups
        .iter()
        .find(|g| g.project.as_deref() == Some("/p/proj"))
        .expect("the project group renders");
    let ids: Vec<&str> = group.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["relic", "fresh", "stale"],
        "pinned first, then the newest stamp — the push order does not survive"
    );

    // The time grouping keeps the same discipline per bucket.
    shell.update(cx, |s, _| s.toggle_grouping());
    let (_, groups, _) = shell.read_with(cx, |s, cx| s.sidebar_props(cx));
    let ids: Vec<&str> = groups
        .iter()
        .flat_map(|g| g.rows.iter().map(|r| r.id.as_str()))
        .collect();
    assert_eq!(
        ids,
        vec!["relic", "fresh", "stale"],
        "the time buckets order pinned-first then newest-first"
    );
}

/// The workspace group's state key is its PROJECT PATH (not the display
/// name), a host-seeded order is respected verbatim, and a drag hands the
/// full new key list to `on_group_order` for the host to persist.
#[gpui::test]
fn group_order_seeds_keys_and_drag_mirrors_to_the_host(cx: &mut TestAppContext) {
    let slot: Rc<RefCell<Option<gpui::Entity<Shell>>>> = Rc::new(RefCell::new(None));
    let slot_for_build = slot.clone();
    let recorded: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
    let recorded_for_assert = recorded.clone();
    cx.update(gpui_component::init);
    let _window = cx.open_window(size(px(900.), px(600.)), move |window, cx| {
        steer_i18n::init();
        register_fonts(cx);
        let main: gpui::AnyView = cx.new(|_| StubView).into();
        let recorded_for_build = recorded.clone();
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
                        on_group_order: Some(Box::new(move |keys, _cx| {
                            recorded_for_build.borrow_mut().push(keys.to_vec());
                        })),
                        ..HostHooks::default()
                    },
                },
                window,
                cx,
            )
        });
        shell.update(cx, |s, _| {
            s.set_sessions(vec![
                row("a", 100, false, Some("/p/one")),
                row("b", 200, false, Some("/p/two")),
                row("c", 300, false, None),
            ]);
            // The host seeds the persisted order: two on top, then one;
            // the loose bucket trails unlisted.
            s.set_group_order(vec!["/p/two".into(), "/p/one".into()]);
        });
        *slot_for_build.borrow_mut() = Some(shell.clone());
        Root::new(shell, window, cx)
    });
    cx.run_until_parked();
    let shell = slot.borrow().clone().expect("shell captured");

    shell.read_with(cx, |s, cx| {
        let (_, groups, _) = s.sidebar_props(cx);
        let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
        assert_eq!(keys, vec!["/p/two", "/p/one", "Chats"]);
    });

    // A drag inserts before the target and hands the FULL key list to the
    // host — the list the host persists and re-seeds.
    shell.update(cx, |s, cx| {
        s.move_group("/p/one", "Chats", false, cx);
    });
    assert_eq!(
        recorded_for_assert.borrow().as_slice(),
        &[vec![
            "/p/two".to_string(),
            "Chats".to_string(),
            "/p/one".to_string()
        ]],
        "the drag mirrors the whole order, keyed by project path"
    );
}
