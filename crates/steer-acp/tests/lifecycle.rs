//! The ACP lifecycle against a stub agent: handshake, host-minted sessions,
//! streamed updates, the request bridge, replay ordering, a frame that lands
//! after the load response, cancellation, coded refusals, the oversized-frame
//! hangup, and both ways a process ends.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use steer_acp::{
    AcpEnv, AcpError, AcpOptions, AcpProcess, HostRequest, SessionProxy, SessionUpdate,
    mint_session_id,
};
use tokio::sync::mpsc::UnboundedReceiver;

const TIMEOUT: Duration = Duration::from_secs(10);

fn stub() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("stub-agent.sh")
}

struct Harness {
    process: AcpProcess,
    updates: UnboundedReceiver<SessionUpdate>,
    requests: UnboundedReceiver<HostRequest>,
}

impl Harness {
    fn start() -> Self {
        Self::start_with(AcpEnv::inherit())
    }

    fn start_with(env: AcpEnv) -> Self {
        let process = AcpProcess::spawn(AcpOptions::new(stub(), std::env::temp_dir(), env))
            .expect("the stub agent starts");
        let updates = process.updates().expect("updates are taken once");
        let requests = process.requests().expect("requests are taken once");
        Self {
            process,
            updates,
            requests,
        }
    }

    async fn handshake(&self) {
        let initialized =
            tokio::time::timeout(TIMEOUT, self.process.initialize("steer-acp-test", "0.0.0"))
                .await
                .expect("initialize answers")
                .expect("initialize succeeds");
        assert_eq!(initialized.protocol_version, 1);
        assert_eq!(initialized.agent_name.as_deref(), Some("stub-agent"));
        assert_eq!(initialized.agent_version.as_deref(), Some("9.9.9"));
    }

    async fn open_session(&mut self, id: &str) -> SessionProxy {
        self.handshake().await;
        let session = tokio::time::timeout(
            TIMEOUT,
            self.process.new_session(Some(id), &std::env::temp_dir()),
        )
        .await
        .expect("session/new answers")
        .expect("session/new succeeds");
        assert_eq!(session.id(), id);
        // The harness announces its command table before the response.
        let announced = self.next_update().await;
        assert_eq!(announced.kind(), Some("available_commands_update"));
        session
    }

    async fn next_update(&mut self) -> SessionUpdate {
        tokio::time::timeout(TIMEOUT, self.updates.recv())
            .await
            .expect("an update arrives")
            .expect("the update channel stays open")
    }

    async fn next_request(&mut self) -> HostRequest {
        tokio::time::timeout(TIMEOUT, self.requests.recv())
            .await
            .expect("a request arrives")
            .expect("the request channel stays open")
    }
}

#[tokio::test]
async fn handshake_mints_the_host_id_and_closes_on_stdin_eof() {
    let mut harness = Harness::start();
    let id = mint_session_id();
    let session = harness.open_session(&id).await;
    assert!(session.info().config_options.is_array());
    assert!(session.info().modes.get("availableModes").is_some());

    tokio::time::timeout(TIMEOUT, harness.process.close())
        .await
        .expect("close settles");
    let exit = harness
        .process
        .exit()
        .expect("the process reported its exit");
    assert_eq!(exit.code, Some(0), "stdin EOF is the graceful exit");
}

#[tokio::test]
async fn prompt_streams_the_transcript_and_settles() {
    let mut harness = Harness::start();
    let session = harness.open_session(&mint_session_id()).await;

    let prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("hello").await }
    });
    let user = harness.next_update().await;
    assert_eq!(user.kind(), Some("user_message_chunk"));
    assert_eq!(user.update["content"]["text"], "hello");
    let agent = harness.next_update().await;
    assert_eq!(agent.kind(), Some("agent_message_chunk"));
    assert_eq!(agent.update["content"]["text"], "reply to hello");

    let stop = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .expect("the turn settles")
        .expect("the task does not panic")
        .expect("the turn succeeds");
    assert_eq!(stop, "end_turn");
}

#[tokio::test]
async fn permission_request_bridges_to_the_host() {
    let mut harness = Harness::start();
    let session = harness.open_session(&mint_session_id()).await;

    let prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("permission").await }
    });
    let request = harness.next_request().await;
    assert_eq!(request.method, "session/request_permission");
    assert_eq!(request.params["options"][0]["optionId"], "allow");
    harness
        .process
        .respond(&request.id, json!({ "optionId": "allow" }))
        .expect("the answer is written");

    let agent = harness.next_update().await;
    assert_eq!(agent.update["_meta"]["stub"]["answer"], "allow");
    let stop = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .expect("the turn settles")
        .expect("the task does not panic")
        .expect("the turn succeeds");
    assert_eq!(stop, "end_turn");
}

#[tokio::test]
async fn load_returns_the_replay_with_the_response() {
    let mut harness = Harness::start();
    harness.handshake().await;

    let (session, replay) = tokio::time::timeout(
        TIMEOUT,
        harness.process.load_session(
            "11111111-1111-4111-8111-111111111111",
            &std::env::temp_dir(),
        ),
    )
    .await
    .expect("session/load answers")
    .expect("session/load succeeds");
    assert_eq!(session.id(), "11111111-1111-4111-8111-111111111111");
    let kinds: Vec<_> = replay.iter().filter_map(SessionUpdate::kind).collect();
    assert_eq!(kinds, ["user_message_chunk", "agent_message_chunk"]);

    // The replay was captured, not raced onto the live channel.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), harness.updates.recv())
            .await
            .is_err(),
        "replay frames must not also arrive as live updates"
    );
}

#[tokio::test]
async fn resume_of_a_missing_session_keeps_the_connection_usable() {
    let harness = Harness::start();
    harness.handshake().await;

    let error = harness
        .process
        .resume_session("missing", &std::env::temp_dir())
        .await
        .expect_err("an unknown session is refused");
    assert!(error.is_session_not_found(), "unexpected error: {error}");
    assert_eq!(error.data_code(), Some("session_not_found"));
    assert!(
        !error.is_fatal(),
        "a coded refusal is the harness answering"
    );

    // The same connection mints the rebuilt session under the same id.
    let rebuilt = tokio::time::timeout(
        TIMEOUT,
        harness.process.new_session(None, &std::env::temp_dir()),
    )
    .await
    .expect("session/new answers")
    .expect("session/new succeeds");
    assert_eq!(rebuilt.id(), "stub-session-1");
}

#[tokio::test]
async fn cancel_settles_the_pending_turn() {
    let mut harness = Harness::start();
    let session = harness.open_session(&mint_session_id()).await;

    let prompt = tokio::spawn({
        let session = session.clone();
        async move { session.prompt("wait").await }
    });
    let streaming = harness.next_update().await;
    assert_eq!(streaming.update["content"]["text"], "working");
    session.cancel().expect("cancel is written");

    let stop = tokio::time::timeout(TIMEOUT, prompt)
        .await
        .expect("the cancelled turn settles")
        .expect("the task does not panic")
        .expect("cancellation is not an error");
    assert_eq!(stop, "cancelled");
}

#[tokio::test]
async fn crash_fails_the_pending_turn_and_closes_the_channels() {
    let mut harness = Harness::start();
    let session = harness.open_session(&mint_session_id()).await;

    let error = tokio::time::timeout(TIMEOUT, session.prompt("crash"))
        .await
        .expect("the turn settles")
        .expect_err("a crash fails the turn");
    assert!(error.is_fatal(), "unexpected error: {error}");
    assert!(
        error.to_string().contains("exit 7"),
        "unexpected error: {error}"
    );

    let leftover = tokio::time::timeout(TIMEOUT, harness.updates.recv())
        .await
        .expect("the update channel settles");
    assert!(leftover.is_none(), "the channel closes after the crash");
    let exit = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(exit) = harness.process.exit() {
                return exit;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the exit is reported");
    assert_eq!(exit.code, Some(7));
}

#[tokio::test]
async fn an_oversized_frame_hangs_up_and_fails_the_pending_turn() {
    let mut harness = Harness::start();
    let session = harness.open_session(&mint_session_id()).await;

    let error = tokio::time::timeout(TIMEOUT, session.prompt("flood"))
        .await
        .expect("the turn settles instead of hanging past the test timeout")
        .expect_err("a framing failure fails the turn");
    assert!(error.is_fatal(), "unexpected error: {error}");
    assert!(
        error.to_string().contains("framing"),
        "the failure must carry the framing diagnosis, got: {error}"
    );

    let leftover = tokio::time::timeout(TIMEOUT, harness.updates.recv())
        .await
        .expect("the update channel settles");
    assert!(
        leftover.is_none(),
        "the update channel closes after the framing failure"
    );
    let leftover = tokio::time::timeout(TIMEOUT, harness.requests.recv())
        .await
        .expect("the request channel settles");
    assert!(
        leftover.is_none(),
        "the request channel closes after the framing failure"
    );

    tokio::time::timeout(TIMEOUT, async {
        loop {
            if !harness.process.is_alive() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the flood child is hung up");
}

#[tokio::test]
async fn initialize_failure_hangs_up_the_child() {
    let harness = Harness::start_with(AcpEnv::inherit().with_var("PI_STUB_PROTOCOL", "2"));
    let error = tokio::time::timeout(
        TIMEOUT,
        harness.process.initialize("steer-acp-test", "0.0.0"),
    )
    .await
    .expect("initialize answers")
    .expect_err("a foreign protocol version is refused");
    assert!(
        matches!(error, AcpError::Protocol(_)),
        "unexpected error: {error}"
    );

    let exit = tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Some(exit) = harness.process.exit() {
                return exit;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the refused child is hung up");
    assert!(
        exit.signal.is_some(),
        "a refused handshake kills the process, got {exit:?}"
    );
}

#[tokio::test]
async fn load_captures_only_the_loaded_session() {
    let mut harness = Harness::start();
    harness.handshake().await;

    let (session, replay) = tokio::time::timeout(
        TIMEOUT,
        harness.process.load_session("cross", &std::env::temp_dir()),
    )
    .await
    .expect("session/load answers")
    .expect("session/load succeeds");
    assert_eq!(session.id(), "cross");
    assert_eq!(replay.len(), 2);
    assert!(
        replay.iter().all(|update| update.session_id == "cross"),
        "another session's frame leaked into the replay: {replay:?}"
    );

    // The other session's update belonged to the live channel all along.
    let live = harness.next_update().await;
    assert_eq!(live.session_id, "stub-other");
}

#[tokio::test]
async fn a_frame_after_the_load_response_is_live_not_replay() {
    let mut harness = Harness::start();
    harness.handshake().await;

    let (session, replay) = tokio::time::timeout(
        TIMEOUT,
        harness.process.load_session("burst", &std::env::temp_dir()),
    )
    .await
    .expect("session/load answers")
    .expect("session/load succeeds");
    assert_eq!(session.id(), "burst");
    let kinds: Vec<_> = replay.iter().filter_map(SessionUpdate::kind).collect();
    assert_eq!(
        kinds,
        ["user_message_chunk", "agent_message_chunk"],
        "the frame that follows the response is not replay: {replay:?}"
    );

    let live = harness.next_update().await;
    assert_eq!(live.session_id, "burst");
    assert_eq!(live.update["content"]["text"], "live after load");
}

#[tokio::test]
async fn cancelled_load_releases_the_capture() {
    let harness = Harness::start();
    harness.handshake().await;

    // The stub holds this load for two seconds; the caller gives up first.
    let cancelled = tokio::time::timeout(
        Duration::from_millis(150),
        harness.process.load_session("slow", &std::env::temp_dir()),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the slow load should not settle in time"
    );

    // The capture slot must be free again: the next load gets its own replay.
    let (session, replay) = tokio::time::timeout(
        TIMEOUT,
        harness
            .process
            .load_session("normal", &std::env::temp_dir()),
    )
    .await
    .expect("the follow-up load answers")
    .expect("the follow-up load succeeds");
    assert_eq!(session.id(), "normal");
    assert_eq!(replay.len(), 2);
    assert!(replay.iter().all(|update| update.session_id == "normal"));
}

#[tokio::test]
async fn custom_requests_carry_the_session_id() {
    let mut harness = Harness::start();
    let session = harness.open_session(&mint_session_id()).await;

    let error = session
        .request_scoped("_zu/not_served", json!({}))
        .await
        .expect_err("the stub does not serve an unknown extension method");
    // The stub answers method-not-found, which proves the request reached it
    // (the panel methods `_zu/todos/list` and `_zu/subagents/list` are served,
    // so this probe stays on a name the stub deliberately lacks).
    assert_eq!(error.data_code(), Some("method_not_found"));
}
