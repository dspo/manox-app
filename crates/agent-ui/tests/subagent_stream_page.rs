//! The Captain status block's sub-agent lines and the main-column page behind
//! them: the block lists the parent chat's live runs, and a line's click opens
//! that child's own transcript in the main column (the child chat is the only
//! carrier of a sub-agent's text — the parent's chat carries the run's status).
//!
//! Every tick this test feeds is PRODUCTION-SHAPED: the parent's background-work
//! entry is byte-identical throughout (the host writes it once per run and never
//! refreshes it), and only the CHILD's chat grows. What the page must follow is
//! the child's fold, and what it must fetch it with is its own re-subscribe poll
//! (a child chat is served snapshot-only), which the queue count below pins.
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{AppContext as _, TestAppContext, VisualTestContext};
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};

const RUN: &str = "sub-ns-3";

/// The parent chat's fold: one running sub-agent, entry written once.
fn parent() -> ahp_types::state::ChatState {
    serde_json::from_value(serde_json::json!({
        "resource": chat_uri("parent-1"),
        "title": "the captain's chat",
        "status": 0,
        "modifiedAt": "2026-10-15T00:00:00.000Z",
        "turns": [],
        "backgroundWork": [{
            "kind": "subagent",
            "id": RUN,
            "label": "Explore",
            "startedAt": "2026-10-15T00:00:00.000Z",
            "_meta": { "agentType": "Explore", "latestActivity": "fix the login redirect" },
            "chat": format!("ahp-chat:/{RUN}"),
        }],
    }))
    .expect("the parent chat snapshot parses")
}

/// The parent chat with the run settled — the host removes the entry.
fn parent_after_settle() -> ahp_types::state::ChatState {
    serde_json::from_value(serde_json::json!({
        "resource": chat_uri("parent-1"),
        "title": "the captain's chat",
        "status": 0,
        "modifiedAt": "2026-10-15T00:00:09.000Z",
        "turns": [],
    }))
    .expect("the settled parent chat snapshot parses")
}

fn part(id: &str, content: &str) -> serde_json::Value {
    serde_json::json!({ "kind": "markdown", "id": id, "content": content })
}

fn child(
    resource_id: &str,
    modified_at: &str,
    body: serde_json::Value,
) -> ahp_types::state::ChatState {
    let mut value = serde_json::json!({
        "resource": chat_uri(resource_id),
        "title": "the child's chat",
        "status": 0,
        "modifiedAt": modified_at,
    });
    let map = value.as_object_mut().expect("object");
    for (key, field) in body.as_object().expect("body object") {
        map.insert(key.clone(), field.clone());
    }
    serde_json::from_value(value).expect("the child chat snapshot parses")
}

/// The child mid-run: the answer streams INSIDE the active turn.
fn child_active(parts: &[&str]) -> ahp_types::state::ChatState {
    child(
        RUN,
        "2026-10-15T00:00:02.000Z",
        serde_json::json!({
            "turns": [],
            "activeTurn": {
                "id": "turn-1",
                "startedAt": "2026-10-15T00:00:01.000Z",
                "message": { "text": "fix the login redirect", "origin": { "kind": "user" } },
                "responseParts": parts
                    .iter()
                    .enumerate()
                    .map(|(ix, content)| part(&format!("turn-1-p{ix}"), content))
                    .collect::<Vec<_>>(),
            },
        }),
    )
}

/// The child's turn settled: the same content lands in `turns`.
fn child_settled(parts: &[&str]) -> ahp_types::state::ChatState {
    child(
        RUN,
        "2026-10-15T00:00:03.000Z",
        serde_json::json!({
            "turns": [{
                "id": "turn-1",
                "message": { "text": "fix the login redirect", "origin": { "kind": "user" } },
                "responseParts": parts
                    .iter()
                    .enumerate()
                    .map(|(ix, content)| part(&format!("turn-1-p{ix}"), content))
                    .collect::<Vec<_>>(),
                "state": "complete",
            }],
        }),
    )
}

/// A PTY source that never starts a process: the external session this test
/// hands the main column to takes the REAL `spawn_external_session` /
/// `open_external_session` paths.
#[derive(Clone, Default)]
struct FakePty {
    event_tx: std::sync::Arc<
        std::sync::Mutex<Option<async_channel::Sender<manox_terminal::event::TerminalEvent>>>,
    >,
}

impl manox_terminal::pty_source::PtySource for FakePty {
    fn start(&mut self, event_tx: async_channel::Sender<manox_terminal::event::TerminalEvent>) {
        *self.event_tx.lock().unwrap() = Some(event_tx);
    }
    fn write(&self, _bytes: &[u8]) -> std::io::Result<()> {
        Ok(())
    }
    fn resize(&self, _cols: u16, _rows: u16) -> std::io::Result<()> {
        Ok(())
    }
}

/// Let the page's own poll run: the test executor's clock is deterministic, so
/// time moves explicitly (no real sleeping) and each advance fires at most one
/// poll tick.
fn pump(cx: &mut VisualTestContext, by: std::time::Duration) {
    cx.cx.executor().advance_clock(by);
    cx.run_until_parked();
}

/// The block's lines come from the parent chat's live background work, and a
/// line's click puts THAT child's transcript in the main column (never the
/// parent's rows); the page then follows the child's own fold, and its back
/// control returns the conversation.
#[gpui::test]
async fn a_subagent_line_opens_that_childs_transcript(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "parent-1", cx);
    });
    store.update(&mut visual.cx, |s, _| {
        s.book.chats.insert("parent-1".into(), parent());
        s.book
            .chats
            .insert(RUN.into(), child_active(&["the child's answer"]));
    });

    // The block lists the live run (id, agent type, the run's task label).
    let rows = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_running_subagents(cx));
    assert_eq!(rows.len(), 1, "the block lists the live run");
    assert_eq!(rows[0].id, RUN);
    assert_eq!(rows[0].label, "Explore");
    assert_eq!(rows[0].activity.as_deref(), Some("fix the login redirect"));

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_open_subagent_stream(RUN, cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream(cx)),
        Some(RUN.to_string()),
        "the clicked run owns the main column"
    );
    let texts = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream_texts(cx));
    assert!(
        texts.iter().any(|text| text.contains("the child's answer")),
        "the page opens on the click-time snapshot, got {texts:?}"
    );
    for _ in 0..3 {
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
    assert!(
        visual.debug_bounds("subagent-stream-main").is_some(),
        "the sub-agent page paints in the main column"
    );

    // The run advances INSIDE its active turn. The parent's background-work
    // entry is byte-identical — the only shape the real host emits — so the
    // page must key on the CHILD's fold, which is what this asserts.
    store.update(&mut visual.cx, |s, cx| {
        s.book.chats.insert(
            RUN.into(),
            child_active(&["the child's answer", "the child's follow-up"]),
        );
        s.book.chats.insert("parent-1".into(), parent());
        cx.notify();
    });
    pump(&mut visual, std::time::Duration::from_millis(50));
    let texts = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream_texts(cx));
    assert!(
        texts
            .iter()
            .any(|text| text.contains("the child's follow-up")),
        "a growing ACTIVE turn must reach the page, got {texts:?}"
    );

    // The turn settles: the same content lands in `turns`, plus the final
    // answer. Parent entry still byte-identical.
    store.update(&mut visual.cx, |s, cx| {
        s.book.chats.insert(
            RUN.into(),
            child_settled(&[
                "the child's answer",
                "the child's follow-up",
                "the child's final answer",
            ]),
        );
        s.book.chats.insert("parent-1".into(), parent());
        cx.notify();
    });
    pump(&mut visual, std::time::Duration::from_millis(50));
    let texts = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream_texts(cx));
    assert!(
        texts
            .iter()
            .any(|text| text.contains("the child's final answer")),
        "the settled turn must reach the page, got {texts:?}"
    );

    // The fetch half: the host serves a child chat snapshot-only, so the page
    // re-subscribes on its own poll while the run is live (the detached store
    // queues each subscribe, which is what this counts).
    let before = store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes());
    pump(&mut visual, std::time::Duration::from_millis(2_500));
    let after = store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes());
    assert!(
        after > before,
        "the open page must keep fetching while its run is live ({before} -> {after})"
    );

    // The invariant candidate the review found broken: the page lives only
    // while ITS mode owns the column. An external session taking the column
    // (the project menu's launch, the sidebar's parked-session row, and any
    // later mode flip) must drop the page — a page nobody sees would keep
    // polling its child chat, and a re-open that kept it would take the
    // early-return arm with a dead poll.
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let terminal = manox_terminal::Terminal::spawn(
                "probe-ext-pty".into(),
                std::path::PathBuf::from("/tmp"),
                80,
                24,
                Box::new(FakePty::default()),
            )
            .expect("fake terminal spawns");
            let proxy = cx.new(|cx| terminal_ui::terminal_proxy::TerminalProxy::new(terminal, cx));
            let view = terminal_ui::TerminalView::new(proxy, cx);
            ws.spawn_external_session(
                agent_ui::workspace::external_sessions::ExternalSessionLaunch {
                    agent_id: "claude",
                    cx_session_id: "probe-ext".into(),
                    label: "Claude Code".into(),
                    svg: "icons/claude.svg",
                    project: None,
                },
                view,
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream(cx)),
        None,
        "a launched external session drops the sub-agent page"
    );

    // Park that session and re-open the run: the page comes back WITH a
    // fetch loop (the defect this pins: the dropped poll never returned).
    workspace.update(&mut visual.cx, |ws, _| ws.leave_external_session());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_open_subagent_stream(RUN, cx);
    });
    let reopened = store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes());
    pump(&mut visual, std::time::Duration::from_millis(2_500));
    assert!(
        store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes()) > reopened,
        "a re-opened page on a live run must still fetch"
    );

    // The parked session's own row is the SECOND hand-off site the review
    // named: its click must drop the page as well.
    let external_id = workspace
        .read_with(&visual.cx, |ws, _| ws.diagnostic_external_session_ids())
        .into_iter()
        .next()
        .expect("the external session registered");
    let any = window.into();
    cx.update_window(any, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.open_external_session(&external_id, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream(cx)),
        None,
        "an external session row's click drops the sub-agent page too"
    );
    // Back to the page for the remaining phases (the run is still live).
    workspace.update(&mut visual.cx, |ws, _| ws.leave_external_session());
    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_open_subagent_stream(RUN, cx);
    });

    // Settle: the run leaves the live set, which earns one last fetch (the
    // child's journal has ended, so that snapshot is final) and then stops the
    // poll — a page nobody needs to advance must not keep subscribing.
    store.update(&mut visual.cx, |s, cx| {
        s.book
            .chats
            .insert("parent-1".into(), parent_after_settle());
        cx.notify();
    });
    let at_settle = store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes());
    pump(&mut visual, std::time::Duration::from_millis(5_000));
    let after_settle = store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes());
    assert_eq!(
        after_settle,
        at_settle + 1,
        "the settle edge fetches once and stops the poll ({at_settle} -> {after_settle})"
    );
    pump(&mut visual, std::time::Duration::from_millis(2_500));
    assert_eq!(
        store.read_with(&visual.cx, |s, _| s.diagnostic_queued_subscribes()),
        after_settle,
        "a stopped poll stays stopped"
    );

    workspace.update(&mut visual.cx, |ws, cx| {
        ws.diagnostic_subagent_stream_back(cx);
    });
    assert_eq!(
        workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_subagent_stream(cx)),
        None,
        "the back control returns the conversation"
    );
    agent_ui::dispatch::clear_globals_for_test();
}
