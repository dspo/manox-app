//! The pi runtime against the ACP stub agent: spawn, bind, replay/live
//! ordering, a streamed turn folded into the book, and the graceful close.
//!
//! The stub is the same fixture `steer-acp`'s lifecycle suite drives; using it
//! here keeps this test model-free and the two seams in agreement about the
//! wire.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_channel::Receiver;
use serde_json::json;
use steer_agent_chat_ui::pi::{OpenMode, PiBook, PiRuntime, PiSessionState, StoreEvent};

const TIMEOUT: Duration = Duration::from_secs(10);
const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn stub() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../steer-acp/tests/fixtures/stub-agent.sh")
}

fn state_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "steer-pi-runtime-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&root).expect("the temporary state root is created");
    root
}

async fn next_event(events: &Receiver<StoreEvent>) -> StoreEvent {
    tokio::time::timeout(TIMEOUT, events.recv())
        .await
        .expect("an event arrives")
        .expect("the channel stays open")
}

fn open_book(root: &Path, session: &str) -> PiBook {
    let mut book = PiBook::new();
    book.open(session, root);
    book
}

#[tokio::test]
async fn a_new_session_streams_a_turn_through_the_book() {
    // The exit-reason assertion below matches the English wording; the UI
    // language is process-global, so pin it for this test.
    steer_i18n::set_ui_language(steer_i18n::Language::En);
    let root = state_root();
    let (events, receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some(SESSION), &root, OpenMode::New, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session opens");
    assert_eq!(connection.session_id(), SESSION);

    let mut book = open_book(&root, SESSION);
    let ready = next_event(&receiver).await;
    assert!(matches!(ready, StoreEvent::Ready { .. }));
    book.apply(&ready);
    assert!(
        !book.view(SESSION).unwrap().config_options.is_empty(),
        "session/new announces its selectors"
    );

    // The command table lands as a live update after the bind.
    let commands = next_event(&receiver).await;
    book.apply(&commands);

    book.prompt_started(SESSION, "hello");
    let prompt = tokio::spawn({
        let connection = connection.clone();
        async move { connection.prompt("hello").await }
    });
    // Fold the turn's frames until the assistant's entry settles.
    let mut settled = false;
    while !settled {
        let event = next_event(&receiver).await;
        if let StoreEvent::Update { update, .. } = &event {
            settled = update["sessionUpdate"] == "agent_message_chunk"
                && update["_meta"]["pi"]["settled"] == true;
        }
        book.apply(&event);
    }
    let stop = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .expect("the turn settles")
        .expect("the task does not panic")
        .expect("the prompt succeeds");
    assert_eq!(stop, "end_turn");
    book.turn_finished(SESSION);

    let rows = book.rows(SESSION);
    let kinds: Vec<_> = rows
        .iter()
        .map(|row| match row {
            steer_agent_chat_ui::pi::Entry::User(_) => "user",
            steer_agent_chat_ui::pi::Entry::Assistant(_) => "assistant",
            steer_agent_chat_ui::pi::Entry::Tool(_) => "tool",
            steer_agent_chat_ui::pi::Entry::Note(_) => "note",
        })
        .collect();
    assert_eq!(kinds, ["user", "assistant"], "rows: {rows:?}");
    assert_eq!(book.view(SESSION).unwrap().state, PiSessionState::Ready);

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
    let exit = next_event(&receiver).await;
    match exit {
        StoreEvent::Exited { reason, .. } => {
            assert!(reason.contains("exit 0"), "unexpected exit: {reason}");
        }
        other => panic!("expected the exit event, got {other:?}"),
    }
}

#[tokio::test]
async fn a_restored_session_replays_before_any_live_frame() {
    let root = state_root();
    let (events, receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some("cross"), &root, OpenMode::History, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session restores");

    let mut book = open_book(&root, "cross");
    let ready = next_event(&receiver).await;
    assert!(matches!(ready, StoreEvent::Ready { .. }), "got {ready:?}");
    book.apply(&ready);
    // The replay is a single event, already in the harness's order.
    let replay = next_event(&receiver).await;
    assert!(
        matches!(replay, StoreEvent::Replay { .. }),
        "got {replay:?}"
    );
    book.apply(&replay);
    assert_eq!(book.rows("cross").len(), 2);
    assert!(!book.view("cross").unwrap().loading);
    assert_eq!(book.view("cross").unwrap().state, PiSessionState::Ready);

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
}

#[tokio::test]
async fn a_missing_session_is_rebuilt_under_the_same_id() {
    let root = state_root();
    let (events, receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some("missing"), &root, OpenMode::History, &events),
    )
    .await
    .expect("the bind answers")
    .expect("an empty conversation replaces the lost one");
    assert_eq!(connection.session_id(), "missing");

    let mut book = open_book(&root, "missing");
    let ready = next_event(&receiver).await;
    book.apply(&ready);
    assert_eq!(book.view("missing").unwrap().state, PiSessionState::Ready);
    assert!(book.rows("missing").is_empty());

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
}

#[tokio::test]
async fn a_permission_request_reaches_the_host_and_the_answer_returns() {
    let root = state_root();
    let (events, receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some(SESSION), &root, OpenMode::New, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session opens");
    let mut book = open_book(&root, SESSION);
    book.apply(&next_event(&receiver).await);

    book.prompt_started(SESSION, "permission");
    let prompt = tokio::spawn({
        let connection = connection.clone();
        async move { connection.prompt("permission").await }
    });
    // Drain the bind's command table and the turn's first frame until the
    // request lands.
    let event = loop {
        let event = next_event(&receiver).await;
        if let StoreEvent::Request { request, .. } = &event {
            assert_eq!(request.method, "session/request_permission");
            break event;
        }
        book.apply(&event);
    };
    let StoreEvent::Request { request, .. } = &event else {
        unreachable!("the loop breaks only on a request");
    };
    let request = request.clone();
    book.apply(&event);
    assert_eq!(
        book.view(SESSION).unwrap().state,
        PiSessionState::WaitingUser
    );
    let pending = book
        .take_request(SESSION, &request.id)
        .expect("the request is pending");
    connection
        .respond(&pending.id, json!({"optionId": "allow"}))
        .expect("the answer is written");

    let stop = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .expect("the turn settles")
        .expect("the task does not panic")
        .expect("the prompt succeeds");
    assert_eq!(stop, "end_turn");
    book.turn_finished(SESSION);
    assert_eq!(book.view(SESSION).unwrap().state, PiSessionState::Ready);

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
}

#[tokio::test]
async fn an_ask_card_reaches_the_host_and_the_answer_returns() {
    let root = state_root();
    let (events, receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some(SESSION), &root, OpenMode::New, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session opens");
    let mut book = open_book(&root, SESSION);
    book.apply(&next_event(&receiver).await);

    book.prompt_started(SESSION, "ask");
    let prompt = tokio::spawn({
        let connection = connection.clone();
        async move { connection.prompt("ask").await }
    });
    let event = loop {
        let event = next_event(&receiver).await;
        if let StoreEvent::Request { request, .. } = &event {
            assert_eq!(request.method, "_zu/ask/create");
            break event;
        }
        book.apply(&event);
    };
    let StoreEvent::Request { request, .. } = &event else {
        unreachable!("the loop breaks only on a request");
    };
    let request = request.clone();
    book.apply(&event);
    assert_eq!(
        book.view(SESSION).unwrap().state,
        PiSessionState::WaitingUser
    );
    assert_eq!(book.pending_asks().count(), 1);

    let pending = book
        .take_request(SESSION, &request.id)
        .expect("the request is pending");
    connection
        .respond(
            &pending.id,
            json!({"action": "accept", "answers": [{"kind": "option", "labels": ["A"]}]}),
        )
        .expect("the answer is written");

    let stop = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .expect("the turn settles")
        .expect("the task does not panic")
        .expect("the prompt succeeds");
    assert_eq!(stop, "end_turn");
    book.turn_finished(SESSION);
    assert_eq!(book.view(SESSION).unwrap().state, PiSessionState::Ready);

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
}

#[tokio::test]
async fn the_panel_requests_answer_and_fold_into_the_view() {
    use steer_agent_chat_ui::pi::session::{
        parse_plan, parse_subagent_runs, parse_subagent_transcript,
    };

    let root = state_root();
    let (events, _receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some(SESSION), &root, OpenMode::New, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session opens");

    let plan = parse_plan(
        &connection
            .request("_zu/todos/list", json!({}))
            .await
            .expect("the plan answers"),
    );
    assert!(plan.enabled);
    assert_eq!(plan.progress(), (0, 1));

    let written = parse_plan(
        &connection
            .request(
                "_zu/todos/write",
                json!({"op": {"kind": "update", "id": "t-1", "status": "done"}}),
            )
            .await
            .expect("the write answers"),
    );
    assert_eq!(written.progress(), (1, 1));

    let runs = parse_subagent_runs(
        &connection
            .request("_zu/subagents/list", json!({}))
            .await
            .expect("the run list answers"),
    );
    assert_eq!(runs.len(), 1);
    assert!(runs[0].status.is_live());

    let messages = parse_subagent_transcript(
        &connection
            .request("_zu/subagents/transcript", json!({"runId": "r-1"}))
            .await
            .expect("the transcript answers"),
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text, "looking");

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
}

#[tokio::test]
async fn the_mailbox_restores_asks_and_takes_a_late_answer() {
    use steer_agent_chat_ui::pi::PiSessionView;
    use steer_agent_chat_ui::pi::session::parse_restored_asks;

    let root = state_root();
    let (events, _receiver) = async_channel::unbounded();
    let runtime = PiRuntime::new(stub(), &root, &root);
    let connection = tokio::time::timeout(
        TIMEOUT,
        runtime.open(Some(SESSION), &root, OpenMode::New, &events),
    )
    .await
    .expect("the bind answers")
    .expect("the session opens");

    let asks = parse_restored_asks(
        &connection
            .request("_zu/ask/pending", json!({}))
            .await
            .expect("the mailbox answers"),
    );
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].tool_call_id, "ask-restored");
    assert!(asks[0].params["questions"].is_array());

    // The card folds into the view strictly (a repeat is a no-op).
    let mut view = PiSessionView::new(SESSION, &root);
    assert!(view.set_restored_asks(asks));
    assert!(!view.set_restored_asks(Vec::new()) || view.restored_asks.is_empty());
    assert!(view.restored_asks.is_empty());

    // The answer goes through the mailbox, not a JSON-RPC response.
    let delivered = connection
        .request(
            "_zu/ask/answer",
            json!({"toolCallId": "ask-restored", "response": {"action": "accept"}}),
        )
        .await
        .expect("the answer is delivered");
    assert_eq!(delivered["delivered"], "late");

    tokio::time::timeout(TIMEOUT, connection.close())
        .await
        .expect("close settles");
}
