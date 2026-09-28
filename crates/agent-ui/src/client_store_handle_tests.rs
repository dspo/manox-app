//! Integration tests for the chat store handle — kept in agent-ui because
//! they drive the handle through `SessionMultiplexer` (the agent-ui side of
//! the boundary; the handle itself now lives in manox-agent-chat-ui).

#[cfg(test)]
mod tests {
    use crate::multiplexer::SessionMultiplexer;
    use manox_agent::ThreadEvent;
    use manox_agent_chat_ui::client_store_handle::*;
    use manox_protocol::{
        FromClient, FromServer, MsgId, RpcConnection as _, ServerNote, StreamFrame, StreamId,
        in_process_pair, journal::ThreadHeader,
    };
    use manox_session_core::agent_client::AgentClient;
    use std::sync::Arc;

    use gpui::{AppContext as _, Entity, TestAppContext};

    /// §二.3: the reopen budget — exponential backoff, terminal past the
    /// cap. The pre-fix reopen was immediate and unbounded: a stream that
    /// kept failing (corrupt journal, an engine that never materialized)
    /// spun reopens forever with no backoff.
    #[test]
    fn reopen_backoff_is_bounded_and_terminal() {
        let d = ClientStoreHandle::reopen_backoff;
        assert_eq!(d(1), Some(std::time::Duration::from_millis(500)));
        assert_eq!(d(2), Some(std::time::Duration::from_millis(1000)));
        assert_eq!(d(3), Some(std::time::Duration::from_millis(2000)));
        assert_eq!(d(4), Some(std::time::Duration::from_millis(4000)));
        assert_eq!(d(5), Some(std::time::Duration::from_millis(8000)));
        assert_eq!(d(6), None, "past the cap: terminal, no more reopens");
        assert_eq!(d(0), None, "attempt 0 is not a reopen");
    }

    /// A multiplexer backed by a raw connection pair so a test can inject
    /// `FromServer` frames from the server side without a live AgentServer.
    fn test_mux(
        cx: &mut TestAppContext,
    ) -> (
        Entity<SessionMultiplexer>,
        manox_protocol::InProcessConnection,
    ) {
        let (client_conn, server_conn) = in_process_pair();
        let client = Arc::new(AgentClient::from_conn(client_conn));
        let mux = cx.new(|cx| SessionMultiplexer::with_client(client, cx));
        (mux, server_conn)
    }

    /// GW3 capture: an adjudication Request deposits BOTH correlations on the
    /// leaf — the MsgId (a `Reply` answers by it) and the delivery_id (a
    /// `CancelDelivery` withdraws by it, and a PR-4 `DeliveryCancelled` note
    /// retires the card against it). AskUserQuestion answers the model; the
    /// delivery identity is what lets a settled-on-another-client card retire
    /// at once instead of hanging to the 300s expire.
    #[gpui::test]
    fn adjudication_requests_capture_reply_and_delivery_correlations(cx: &mut TestAppContext) {
        let (mux, server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", false, cx));
        cx.run_until_parked();
        server_conn.send_to_client(manox_protocol::FromServer::Request {
            id: manox_protocol::MsgId::new("req-1"),
            call: manox_protocol::ServerCall::Approve {
                delivery_id: "dlv-s1-1".into(),
                session_id: "s1".into(),
                auth_id: "auth-1".into(),
                tool_name: "Bash".into(),
                summary: "run ls".into(),
                input: serde_json::json!({}),
            },
        });
        server_conn.send_to_client(manox_protocol::FromServer::Request {
            id: manox_protocol::MsgId::new("req-2"),
            call: manox_protocol::ServerCall::AskUserQuestion {
                delivery_id: "dlv-s1-2".into(),
                session_id: "s1".into(),
                auth_id: "auth-2".into(),
                input: serde_json::json!({"questions": []}),
            },
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let got = handle.read_with(cx, |h, _| {
                (
                    h.store.pending_auth.get("auth-1").cloned(),
                    h.store.pending_auth_delivery.get("auth-1").cloned(),
                    h.store.pending_auth.get("auth-2").cloned(),
                    h.store.pending_auth_delivery.get("auth-2").cloned(),
                )
            });
            let landed = matches!(&got.0, Some(id) if *id == manox_protocol::MsgId::new("req-1"))
                && matches!(&got.1, Some(d) if d == "dlv-s1-1")
                && matches!(&got.2, Some(id) if *id == manox_protocol::MsgId::new("req-2"))
                && matches!(&got.3, Some(d) if d == "dlv-s1-2");
            if landed {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the adjudication correlations never landed on the leaf"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// PR-4 end-to-end at the leaf: an `AskUserQuestion` delivery captured on
    /// the leaf is retired by the matching `DeliveryCancelled` note routed
    /// through `apply_from_server` — the reply + withdrawal correlations drop,
    /// the projection-set membership clears (so the card reconciles away), and
    /// the auth id is armed for the "handled elsewhere" notice.
    #[gpui::test]
    async fn delivery_cancelled_note_retires_a_parked_ask(cx: &mut TestAppContext) {
        let (mux, server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", false, cx));
        cx.run_until_parked();
        server_conn.send_to_client(manox_protocol::FromServer::Request {
            id: manox_protocol::MsgId::new("req-ask"),
            call: manox_protocol::ServerCall::AskUserQuestion {
                delivery_id: "dlv-ask".into(),
                session_id: "s1".into(),
                auth_id: "auth-q".into(),
                input: serde_json::json!({"questions": []}),
            },
        });
        // Wait for the capture, then arm the projection-set membership the
        // reconcile guard keys on (a real settle delta also clears it; here we
        // seed it to prove the note's own removal path).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let captured = handle.read_with(cx, |h, _| {
                h.store.pending_auth.contains_key("auth-q")
                    && h.store.pending_auth_delivery.contains_key("auth-q")
            });
            if captured {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the ask delivery never captured"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        handle.update(cx, |h, _| h.store.pending_auth_set.insert("auth-q".into()));

        server_conn.send_to_client(manox_protocol::FromServer::Notification {
            note: manox_protocol::ServerNote::DeliveryCancelled {
                delivery_id: "dlv-ask".into(),
            },
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let retired = handle.read_with(cx, |h, _| {
                !h.store.pending_auth.contains_key("auth-q")
                    && !h.store.pending_auth_delivery.contains_key("auth-q")
                    && !h.store.pending_auth_set.contains("auth-q")
                    && h.store.settled_elsewhere.contains("auth-q")
            });
            if retired {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the DeliveryCancelled note never retired the parked ask"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// §E.3 Q face wiring: a message row landing in the window (the
    /// committed edge) triggers a `GetConversationInfo` request whose
    /// Response fills the usage panel fields (per-model rows + totals).
    #[gpui::test]
    async fn conversation_info_fills_usage_panel_on_committed_edge(cx: &mut TestAppContext) {
        use manox_protocol::journal::JournalWireEntry;
        let (mux, server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", false, cx));
        cx.run_until_parked();

        // A snapshot carrying one user + one assistant message row (the
        // assistant with a usage payload) lands: committed = 2.
        let entry = |seq: u64, role: &str, usage: Option<manox_protocol::journal::UsagePayload>| {
            let mut value = serde_json::json!({
                "seq": seq,
                "id": format!("e-{seq}"),
                "parentId": if seq == 0 { serde_json::Value::Null } else { serde_json::json!(format!("e-{}", seq - 1)) },
                "timestamp": "2026-09-05T00:00:00Z",
                "type": "message",
                "role": role,
                "content": [],
                "usage": usage,
                "originRpc": serde_json::Value::Null,
            });
            serde_json::from_value::<JournalWireEntry>(value.take()).unwrap()
        };
        let usage = manox_protocol::journal::UsagePayload {
            input: 100,
            output: 40,
            cache_read: 10,
            cache_write: 5,
            reasoning: 0,
        };
        let frame =
            manox_protocol::StreamFrame::Snapshot(manox_protocol::stream::SessionSnapshot {
                session_id: "s1".into(),
                header: ThreadHeader {
                    id: "s1".into(),
                    cwd: "/w".into(),
                    parent_session: None,
                    metadata: None,
                    created_at: "2026-09-05T00:00:00Z".into(),
                },
                cursor: 1,
                records: vec![entry(0, "user", None), entry(1, "assistant", Some(usage))],
                has_more: false,
                projections: Default::default(),
                projections_as_of_seq: 1,
            });
        // Drive the snapshot directly through the leaf (the sibling stream
        // tests' pattern); the outbound LeafRequest channel still runs
        // through the real multiplexer to the raw pair.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                FromServer::StreamItem {
                    stream_id: StreamId::new("s1"),
                    frame,
                },
                cx,
            )
        });
        cx.run_until_parked();
        // §E.3 debounce: the fetch fires at the trailing edge of the window.
        cx.executor()
            .advance_clock(INFO_DEBOUNCE + std::time::Duration::from_millis(10));
        cx.run_until_parked();

        // The committed edge fired: the server side of the pair must have
        // received a GetConversationInfo Request for s1.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut info_id = None;
        let rx = server_conn.client_rx();
        while info_id.is_none() && std::time::Instant::now() < deadline {
            while let Ok(msg) = rx.try_recv() {
                if let FromClient::Request { id, call } = msg
                    && matches!(call, manox_protocol::ClientCall::GetConversationInfo { .. })
                {
                    info_id = Some(id);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let info_id = info_id.expect("committed edge must issue GetConversationInfo");

        // The Q-face answer fills the panel fields (mechanical fold).
        server_conn.send_to_client(FromServer::Response {
            id: info_id,
            outcome: Ok(serde_json::json!({
                "cumulativeUsage": {"input": 100, "output": 40, "cacheWrite": 5, "cacheRead": 10},
                "cumulativeCost": 0.42,
                "models": [
                    // Real §E.3 payload shape: the server already sends the
                    // canonical `{provider}/{model}` identity in `model`; the
                    // `provider` field is redundant and must NOT be re-prefixed
                    // onto the group key (a regression would yield `P/P/m`).
                    {"provider": "P", "model": "P/m", "input": 100, "output": 40,
                     "cacheRead": 10, "cacheWrite": 5}
                ],
                "perModelCost": {"P/m": 0.42},
            })),
        });
        cx.run_until_parked();
        handle.update(cx, |h, _| {
            let st = &h.store;
            let cumulative = st.cumulative_usage.as_ref().expect("cumulative filled");
            assert_eq!((cumulative.input, cumulative.output), (100, 40));
            assert_eq!(st.per_model_usage.len(), 1);
            // The per-model key is the canonical identity taken verbatim from
            // `model`, so it lines up with both `perModelCost` and the rail's
            // `split_once('/')` resolution.
            let row = st
                .per_model_usage
                .get("P/m")
                .expect("per-model usage keyed by canonical `model`");
            assert_eq!((row.input, row.output), (100, 40));
            assert!((st.cumulative_cost - 0.42).abs() < 1e-9);
            assert_eq!(st.per_model_cost.get("P/m"), Some(&0.42));
        });
    }

    /// #1 regression guard at the display boundary: the canonical per-model
    /// key `{provider}/{model}` (what the store now keys by) splits into the
    /// exact registration name + model id, resolves against the registry, and
    /// renders as `百炼/qwen3.8-max` — not the double-prefixed
    /// `百炼-anthropic/百炼-anthropic/qwen3.8-max` the old store key produced,
    /// which failed `split_once`/`resolve_model` and fell through to the raw
    /// verbatim branch (also silently killing the cost + context-budget rows).
    #[test]
    fn canonical_model_key_resolves_to_provider_and_display_pair() {
        use manox_harness::core::{
            Api, Cost, InputModality, ProviderConfig, ProviderModelConfig, ProviderRegistry,
        };
        // The writer itself (the review's falsification): the store must key
        // per-model usage by the row's verbatim `model` — re-introducing the
        // `{provider}/{model}` re-prefix turns this red.
        let mut store = crate::client_store::ClientStore::default();
        store.apply_conversation_info(&serde_json::json!({
            "models": [{
                "provider": "百炼-anthropic",
                "model": "百炼-anthropic/qwen3.8-max",
                "input": 10,
                "output": 2,
                "cacheWrite": 0,
                "cacheRead": 0,
            }],
        }));
        assert!(
            store
                .per_model_usage
                .contains_key("百炼-anthropic/qwen3.8-max"),
            "the store key is the verbatim canonical identity"
        );
        assert!(
            !store
                .per_model_usage
                .contains_key("百炼-anthropic/百炼-anthropic/qwen3.8-max"),
            "the provider field must not be prefixed a second time"
        );
        let registry = ProviderRegistry::new();
        registry
            .register_provider(
                // Registration name is `{display}-{wire}` (provider.rs
                // `provider_registration_name`): `百炼` over the anthropic
                // endpoint registers as `百炼-anthropic`.
                "百炼-anthropic",
                ProviderConfig {
                    name: Some("百炼".into()),
                    base_url: Some("https://bailian.example".into()),
                    api_key: Some("sk-literal".into()),
                    api: Some(Api::AnthropicMessages),
                    headers: None,
                    auth_header: true,
                    models: vec![ProviderModelConfig {
                        id: "qwen3.8-max".into(),
                        name: "qwen3.8-max".into(),
                        reasoning: false,
                        input: vec![InputModality::Text],
                        context_window: 131_072,
                        max_tokens: 8_192,
                        cost: Cost::default(),
                        api: None,
                        base_url: None,
                        metadata: std::collections::HashMap::new(),
                    }],
                },
            )
            .unwrap();

        // The store key is the verbatim `model` field — the canonical identity.
        let key = "百炼-anthropic/qwen3.8-max";
        let (provider, id) = key.split_once('/').expect("composite key splits");
        assert_eq!(provider, "百炼-anthropic");
        assert_eq!(id, "qwen3.8-max");
        let model = registry
            .resolve_model(provider, id)
            .expect("canonical key must resolve");
        assert_eq!(
            manox_agent::provider_glue::display_provider_name(&model),
            "百炼"
        );
        assert_eq!(
            manox_agent::provider_glue::display_name(&model),
            "qwen3.8-max"
        );
    }

    /// U7 (§E.3): the committed-message counter is maintained
    /// incrementally — an Append contributes exactly its own row, and only
    /// structural window changes (snapshot Replace, gap-repair merge)
    /// recount. Behavior is identical to the former full-window scan:
    /// requests fire only on committed edges with `info-<session>-<count>`
    /// ids, non-message appends fire nothing, and the counter equals a
    /// full recount at every boundary.
    #[gpui::test]
    async fn q_face_committed_counter_is_incremental_and_exact(cx: &mut TestAppContext) {
        let (mux, server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", false, cx));
        cx.run_until_parked();
        let rx = server_conn.client_rx();
        // Drain GetConversationInfo request ids; other frames (the follow
        // StreamOpen) are discarded — the raw pair's server side answers
        // nothing in this test.
        let drain_info = |rx: &async_channel::Receiver<manox_protocol::FromClient>| -> Vec<String> {
            let mut ids = Vec::new();
            while let Ok(m) = rx.try_recv() {
                if let manox_protocol::FromClient::Request { id, call } = m
                    && matches!(call, manox_protocol::ClientCall::GetConversationInfo { .. })
                {
                    ids.push(id.0.clone());
                }
            }
            ids
        };
        let msg_ev = |role: &str| JournalWireEvent::Message {
            role: role.into(),
            content: vec![serde_json::json!({"type": "text", "text": "x"})],
            usage: None,
            origin_rpc: None,
            display: None,
        };

        // Snapshot: two message rows plus one delta → Replace → committed
        // recounts to 2 and fires one request.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    snapshot(
                        "s1",
                        2,
                        vec![
                            wire(0, msg_ev("user")),
                            wire(1, JournalWireEvent::AgentTextDelta { s: "d".into() }),
                            wire(2, msg_ev("assistant")),
                        ],
                    ),
                ),
                cx,
            )
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(INFO_DEBOUNCE + std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(drain_info(&rx), vec!["info-s1-2".to_string()]);

        // Delta and tool appends: window rows land but the committed edge
        // does not move — no request (the hot path the former code rescanned
        // in full on every frame).
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 3,
                        id: "w3".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: JournalWireEvent::AgentTextDelta { s: "a".into() },
                    },
                ),
                cx,
            )
        });
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 4,
                        id: "w4".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: JournalWireEvent::ToolCall {
                            call_id: "c1".into(),
                            name: "Bash".into(),
                            title: "t".into(),
                            status: "running".into(),
                            input: serde_json::json!({}),
                        },
                    },
                ),
                cx,
            )
        });
        cx.run_until_parked();
        assert!(
            drain_info(&rx).is_empty(),
            "non-message appends must not fire the Q face"
        );

        // A message append moves the edge to 3 — exactly one request.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 5,
                        id: "w5".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: msg_ev("user"),
                    },
                ),
                cx,
            )
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(INFO_DEBOUNCE + std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(drain_info(&rx), vec!["info-s1-3".to_string()]);

        // U5: the live frame's durable envelope flows through verbatim —
        // the fold no longer synthesizes `e-{seq}` ids, which drifted from
        // the snapshot records' real uuids at every Replace (usage keys,
        // bubble identity, list keys).
        handle.update(cx, |h, _| {
            let row = h
                .store
                .window
                .iter()
                .find(|e| e.seq == 5)
                .expect("seq 5 lands in the window");
            assert_eq!(row.id, "w5", "the frame's durable id flows through");
        });

        // Structural boundary: a gap (seq 7 after tail 5) buffers the entry
        // and requests the missing page; answering it merges into a Replace
        // whose recount must be exact (messages 0, 2, 5, 6, 7 → 5).
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 7,
                        id: "w7".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: msg_ev("assistant"),
                    },
                ),
                cx,
            )
        });
        cx.run_until_parked();
        let mut page_id: Option<MsgId> = None;
        while let Ok(m) = rx.try_recv() {
            if let manox_protocol::FromClient::Request { id, call } = m
                && matches!(call, manox_protocol::ClientCall::PageHistory { .. })
            {
                page_id = Some(id);
            }
        }
        let page_id = page_id.expect("the gap must request a page");
        // The repair page follows the production PageHistory contract: an
        // unbounded read from the chain start through `through_seq` (the
        // offending entry's own seq) — the engine publishes the repair page
        // AS the whole window ("exactly the repair page plus the queued
        // entries", journal_stream replaceThrough), so a bounded page would
        // truncate the transcript. The queued seq-7 entry merges as a stale
        // duplicate (the page already contains it).
        let full_chain: Vec<serde_json::Value> = vec![
            serde_json::to_value(wire(0, msg_ev("user"))).unwrap(),
            serde_json::to_value(wire(1, JournalWireEvent::AgentTextDelta { s: "d".into() }))
                .unwrap(),
            serde_json::to_value(wire(2, msg_ev("assistant"))).unwrap(),
            serde_json::to_value(wire(3, JournalWireEvent::AgentTextDelta { s: "a".into() }))
                .unwrap(),
            serde_json::to_value(wire(
                4,
                JournalWireEvent::ToolCall {
                    call_id: "c1".into(),
                    name: "Bash".into(),
                    title: "t".into(),
                    status: "running".into(),
                    input: serde_json::json!({}),
                },
            ))
            .unwrap(),
            serde_json::to_value(wire(5, msg_ev("user"))).unwrap(),
            serde_json::to_value(wire(6, msg_ev("user"))).unwrap(),
            serde_json::to_value(wire(7, msg_ev("assistant"))).unwrap(),
        ];
        handle.update(cx, |h, cx| {
            h.apply_page_response(
                page_id,
                Ok(serde_json::json!({ "records": full_chain })),
                cx,
            )
        });
        cx.run_until_parked();
        cx.executor()
            .advance_clock(INFO_DEBOUNCE + std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(drain_info(&rx), vec!["info-s1-5".to_string()]);

        // §E.3 debounce: two committed rows inside one window coalesce into
        // a single trailing fetch keyed by the latest count (messages are
        // now {0,2,5,6,7,8,9} = 7).
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 8,
                        id: "w8".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: msg_ev("user"),
                    },
                ),
                cx,
            )
        });
        // NB: a "user" row, not "assistant" — an appended assistant row
        // fires the materialization edge (sidebar refresh through the
        // process-global ThreadStore), which this hermetic leaf test must
        // not touch; the Q-face count is role-independent.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 9,
                        id: "w9".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: msg_ev("user"),
                    },
                ),
                cx,
            )
        });
        // Inside the debounce window nothing has been sent yet — the
        // trailing-edge fetch is the only request the burst produces.
        cx.run_until_parked();
        assert!(
            drain_info(&rx).is_empty(),
            "the debounce window must hold the trailing fetch"
        );
        cx.executor()
            .advance_clock(INFO_DEBOUNCE + std::time::Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(drain_info(&rx), vec!["info-s1-7".to_string()]);

        handle.update(cx, |h, _| {
            let exact = h
                .store
                .window
                .iter()
                .filter(|e| matches!(&e.event, JournalWireEvent::Message { .. }))
                .count();
            assert_eq!(
                h.info_committed, exact,
                "the incremental counter equals the full recount"
            );
            assert_eq!(exact, 7);
        });
    }

    #[gpui::test]
    async fn pump_feeds_retained_notes_to_store(cx: &mut TestAppContext) {
        let (mux, server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", false, cx));
        server_conn.send_to_client(FromServer::Notification {
            note: ServerNote::SessionCreated {
                session_id: "s1".into(),
            },
        });
        cx.run_until_parked();
        assert_eq!(
            handle.update(cx, |h, _| h.store.id.0.clone()),
            "s1",
            "the multiplexer should route SessionCreated → store.id"
        );
        // A global note must not touch the mirror's session fields.
        server_conn.send_to_client(FromServer::Notification {
            note: ServerNote::Ready,
        });
        cx.run_until_parked();
        assert_eq!(handle.update(cx, |h, _| h.store.id.0.clone()), "s1");
    }

    /// C4a: the leaf normalizes the control HOST frames into its note path
    /// (the same host-event subscription shape the deleted webui host used) —
    /// SessionCreated binds the empty
    /// store id and a scoped Error emits ThreadEvent::Error — so C4b can
    /// retire the wire notes with zero leaf-side change. A foreign
    /// session's frames stay silent (the fan-out reaches every leaf).
    #[gpui::test]
    fn host_control_frames_normalize_into_the_leaf(cx: &mut TestAppContext) {
        let (mux, server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", false, cx));
        let errors: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = errors.clone();
        let subscribed = handle.clone();
        let _sub = handle.update(cx, move |_, cx| {
            cx.subscribe(&subscribed, move |_, _, ev: &ThreadEvent, _| {
                if let ThreadEvent::Error(err) = ev {
                    sink.borrow_mut().push(err.to_string());
                }
            })
        });
        // The compat create flow leaves store.id empty until SessionCreated
        // lands; the HOST frame binds it now (the v1 note did before C4a).
        server_conn.send_to_client(FromServer::Host {
            host: manox_protocol::stream::HostEvent::SessionCreated {
                session_id: "s1".into(),
                header: manox_protocol::journal::ThreadHeader {
                    id: "s1".into(),
                    cwd: "/w".into(),
                    parent_session: None,
                    metadata: None,
                    created_at: "2026-09-05T00:00:00Z".into(),
                },
            },
        });
        server_conn.send_to_client(FromServer::Host {
            host: manox_protocol::stream::HostEvent::Error {
                message: "boom-s1".into(),
                session_id: Some("s1".into()),
            },
        });
        // A foreign session's error must stay silent on this leaf.
        server_conn.send_to_client(FromServer::Host {
            host: manox_protocol::stream::HostEvent::Error {
                message: "boom-s2".into(),
                session_id: Some("s2".into()),
            },
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let bound = handle.update(cx, |h, _| h.store.id.0.clone());
            let seen = errors.borrow().clone();
            if bound == "s1" && seen.iter().any(|m| m.contains("boom-s1")) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the host control frames never normalized (id={bound:?}, errors={seen:?})"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let seen = errors.borrow().clone();
        assert_eq!(
            seen.len(),
            1,
            "the foreign session's error stays silent: {seen:?}"
        );
    }

    /// T10c e2e restore regression (the gap the v1-fold deletion exposed):
    /// a reopen's follow-stream `Snapshot` carrying multiple history rows
    /// must land in the leaf's `display` fold (the sole render source),
    /// transcribe to `derived_messages`, and re-arm the conversation
    /// rebuild via `HistoryRestored` — the v1 `ThreadHistory` note's old
    /// job. At HEAD this rendered an empty transcript (the restore reads
    /// still pointed at the note-fed `display_entries` field).
    #[gpui::test]
    async fn reopen_snapshot_restores_transcript_and_rearms_rebuild(cx: &mut TestAppContext) {
        use std::cell::Cell;
        let (mux, _server_conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/w", true, cx));
        let rearmed = std::rc::Rc::new(Cell::new(false));
        let sink = rearmed.clone();
        let subscribed = handle.clone();
        let _sub = handle.update(cx, move |_, cx| {
            cx.subscribe(&subscribed, move |_, _, ev: &ThreadEvent, _| {
                if matches!(ev, ThreadEvent::HistoryRestored) {
                    sink.set(true);
                }
            })
        });
        let records = vec![
            wire(
                0,
                JournalWireEvent::Message {
                    role: "user".into(),
                    content: vec![serde_json::json!({"type": "text", "text": "one"})],
                    usage: None,
                    origin_rpc: None,
                    display: None,
                },
            ),
            wire(
                1,
                JournalWireEvent::Message {
                    role: "assistant".into(),
                    content: vec![serde_json::json!({"type": "text", "text": "two"})],
                    usage: None,
                    origin_rpc: None,
                    display: None,
                },
            ),
            wire(
                2,
                JournalWireEvent::Message {
                    role: "user".into(),
                    content: vec![serde_json::json!({"type": "text", "text": "three"})],
                    usage: None,
                    origin_rpc: Some("rpc-9".into()),
                    display: None,
                },
            ),
        ];
        handle.update(cx, |h, cx| {
            h.apply_from_server(item("s1", snapshot("s1", 2, records)), cx)
        });
        cx.run_until_parked();
        handle.update(cx, |h, _| {
            assert_eq!(h.store.window.len(), 3, "the window holds the chain");
            assert_eq!(
                h.store.display.len(),
                3,
                "the restored transcript must be non-empty and complete"
            );
            let msgs = h.store.derived_messages();
            assert_eq!(msgs.len(), 3, "display rows transcribe to messages");
            assert_eq!(
                msgs.iter().map(|m| m.role).collect::<Vec<_>>(),
                vec![
                    manox_agent::language_model::Role::User,
                    manox_agent::language_model::Role::Assistant,
                    manox_agent::language_model::Role::User,
                ]
            );
        });
        assert!(
            rearmed.get(),
            "the snapshot Replace must re-arm the rebuild (HistoryRestored)"
        );
    }

    // ── v2 stream fold / echo / resync / status (spec T6-6) ────────────────

    use manox_protocol::journal::{JournalWireEntry, JournalWireEvent};
    use manox_protocol::stream::{HostEvent, ProjectionsFrame, SessionSnapshot};
    use std::collections::BTreeMap;

    fn wire(seq: u64, event: JournalWireEvent) -> JournalWireEntry {
        JournalWireEntry {
            seq,
            id: format!("w{seq}"),
            parent_id: None,
            timestamp: "2026-09-04T00:00:00.000Z".into(),
            event,
        }
    }

    fn snapshot(session_id: &str, cursor: u64, records: Vec<JournalWireEntry>) -> StreamFrame {
        StreamFrame::Snapshot(SessionSnapshot {
            session_id: session_id.into(),
            header: ThreadHeader {
                id: session_id.into(),
                cwd: "/p".into(),
                parent_session: None,
                metadata: None,
                created_at: "2026-09-04T00:00:00.000Z".into(),
            },
            cursor,
            records,
            has_more: false,
            projections: BTreeMap::new(),
            projections_as_of_seq: cursor,
        })
    }

    fn item(session_id: &str, frame: StreamFrame) -> FromServer {
        FromServer::StreamItem {
            stream_id: StreamId::new(session_id),
            frame,
        }
    }

    #[gpui::test]
    async fn stream_snapshot_entry_projections_fold_store(cx: &mut TestAppContext) {
        let (mux, _conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/p", true, cx));
        let user = wire(
            0,
            JournalWireEvent::Message {
                role: "user".into(),
                content: vec![serde_json::json!({"type": "text", "text": "hi"})],
                usage: None,
                origin_rpc: None,
                display: None,
            },
        );
        handle.update(cx, |h, cx| {
            h.apply_from_server(item("s1", snapshot("s1", 0, vec![user.clone()])), cx)
        });
        cx.run_until_parked();
        // Snapshot folded into window + display.
        handle.update(cx, |h, _| {
            assert_eq!(h.store.window.len(), 1);
            assert_eq!(
                h.store.display.len(),
                1,
                "user message projected to display"
            );
        });
        // A live assistant delta appends to the window (no display item).
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 1,
                        id: "w1".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: JournalWireEvent::AgentTextDelta { s: "yo".into() },
                    },
                ),
                cx,
            )
        });
        handle.update(cx, |h, _| assert_eq!(h.store.window.len(), 2));
        // A Projections frame merges (higher-seq-wins) and materializes.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Projections(ProjectionsFrame {
                        session_id: "s1".into(),
                        as_of_seq: 1,
                        values: BTreeMap::from([(
                            "title".to_string(),
                            serde_json::json!("Renamed"),
                        )]),
                    }),
                ),
                cx,
            )
        });
        handle.update(cx, |h, _| {
            assert_eq!(h.store.display_title, "Renamed");
            assert_eq!(
                h.store.projection("title").unwrap().value,
                serde_json::json!("Renamed")
            );
        });
    }

    #[gpui::test]
    async fn stream_resync_reopens_from_snapshot(cx: &mut TestAppContext) {
        let (mux, _conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/p", true, cx));
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    snapshot("s1", 0, vec![wire(0, JournalWireEvent::TurnStart)]),
                ),
                cx,
            )
        });
        // A gap opens (seq 5 after tail 0) with no page source wired (the raw
        // pair's server never replies), so the leaf stays repairing; instead
        // drive the resync terminal frame directly.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                FromServer::StreamEnd {
                    stream_id: StreamId::new("s1"),
                    reason: manox_protocol::StreamEndReason::Resync,
                },
                cx,
            )
        });
        cx.run_until_parked();
        // After the re-open, a fresh contiguous snapshot replaces the window
        // seamlessly (cursor >= tail).
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    snapshot(
                        "s1",
                        2,
                        vec![
                            wire(0, JournalWireEvent::TurnStart),
                            wire(
                                1,
                                JournalWireEvent::TurnFinish {
                                    cancelled: false,
                                    failed: false,
                                    stranded_steer_ids: vec![],
                                },
                            ),
                            wire(2, JournalWireEvent::TurnStart),
                        ],
                    ),
                ),
                cx,
            )
        });
        handle.update(cx, |h, _| {
            assert_eq!(
                h.store.window.len(),
                3,
                "re-open converged to the full chain"
            );
        });
    }

    #[gpui::test]
    async fn echo_retires_on_durable_origin_rpc(cx: &mut TestAppContext) {
        let (mux, _conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/p", true, cx));
        handle.update(cx, |h, _| h.store.push_echo("rpc-42", "hello"));
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    snapshot(
                        "s1",
                        0,
                        vec![wire(
                            0,
                            JournalWireEvent::Message {
                                role: "user".into(),
                                content: vec![serde_json::json!({"type": "text", "text": "hello"})],
                                usage: None,
                                origin_rpc: Some("rpc-42".into()),
                                display: None,
                            },
                        )],
                    ),
                ),
                cx,
            )
        });
        // Snapshot (Replace) does not run the per-entry echo retirement; only
        // Append does, matching §F.2 (a durable row arriving live). Feed the
        // same row as an Append to exercise the retire.
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                item(
                    "s1",
                    StreamFrame::Entry {
                        seq: 1,
                        id: "w1".to_string(),
                        parent_id: None,
                        timestamp: String::new(),
                        event: JournalWireEvent::Message {
                            role: "user".into(),
                            content: vec![serde_json::json!({"type": "text", "text": "hello"})],
                            usage: None,
                            origin_rpc: Some("rpc-42".into()),
                            display: None,
                        },
                    },
                ),
                cx,
            )
        });
        handle.update(cx, |h, _| {
            assert!(
                h.store.echo.is_empty(),
                "durable originRpc retired the echo"
            );
        });
    }

    #[gpui::test]
    async fn session_status_mirrors_monotonically(cx: &mut TestAppContext) {
        let (mux, _conn) = test_mux(cx);
        let handle = mux.update(cx, |m, cx| m.open_or_create("s1", "/p", true, cx));
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                FromServer::Host {
                    host: HostEvent::SessionStatus {
                        session_id: "s1".into(),
                        running: None,
                        errored: None,
                        unread: Some(true),
                        pending_auth: None,
                        pending_plan: None,
                        background_work: None,
                    },
                },
                cx,
            )
        });
        handle.update(cx, |h, _| assert!(h.store.unread));
        // A later `unread=false` delta does not clear it (only focus does).
        handle.update(cx, |h, cx| {
            h.apply_from_server(
                FromServer::Host {
                    host: HostEvent::SessionStatus {
                        session_id: "s1".into(),
                        running: Some(true),
                        errored: None,
                        unread: Some(false),
                        pending_auth: None,
                        pending_plan: None,
                        background_work: None,
                    },
                },
                cx,
            )
        });
        handle.update(cx, |h, _| {
            assert!(h.store.unread, "unread survives until focus");
            assert!(h.store.running, "running takes the latest value");
        });
    }
    /// GW5 regression: the client owns unread (§F.2) — the focused leaf
    /// never lights up, activation clears the monotonic mirrors, and the
    /// local-knowledge rise (parked error / background task) obeys the
    /// same gate.
    #[gpui::test]
    fn active_leaf_suppresses_unread_and_focus_clears(cx: &mut TestAppContext) {
        let handle = cx.new(|cx| ClientStoreHandle::leaf("s1", cx));
        let delta = |unread: Option<bool>| FromServer::Host {
            host: HostEvent::SessionStatus {
                session_id: "s1".into(),
                running: None,
                errored: None,
                unread,
                pending_auth: None,
                pending_plan: None,
                background_work: None,
            },
        };
        handle.update(cx, |h, cx| h.apply_from_server(delta(Some(true)), cx));
        assert!(
            handle.read_with(cx, |h, _| h.store.unread),
            "an unfocused leaf lights up"
        );
        handle.update(cx, |h, cx| {
            h.set_active(true, cx);
        });
        assert!(
            !handle.read_with(cx, |h, _| h.store.unread),
            "activation clears the unread mirror (focus_cleared)"
        );
        handle.update(cx, |h, cx| h.apply_from_server(delta(Some(true)), cx));
        assert!(
            !handle.read_with(cx, |h, _| h.store.unread),
            "GW5: a focused leaf never lights up"
        );
        handle.update(cx, |h, cx| {
            h.set_active(false, cx);
        });
        handle.update(cx, |h, cx| h.apply_from_server(delta(Some(true)), cx));
        assert!(
            handle.read_with(cx, |h, _| h.store.unread),
            "the unfocused leaf lights up again"
        );
        handle.update(cx, |h, cx| {
            h.set_active(true, cx);
        });
        handle.update(cx, |h, cx| h.note_local_unread(cx));
        assert!(
            !handle.read_with(cx, |h, _| h.store.unread),
            "the local rise is suppressed while focused"
        );
    }

    /// GW5 regression: multiplexer focus transitions drive the leaves'
    /// active gates; `unread_map` is the sidebar badge source; a leaf
    /// created while its session is focused starts active.
    #[gpui::test]
    fn multiplexer_focus_transitions_gate_the_leaf_mirrors(cx: &mut TestAppContext) {
        let (mux, server_conn) = test_mux(cx);
        let leaf_a = mux.update(cx, |m, cx| m.open_or_create("s-a", "/p", false, cx));
        let leaf_b = mux.update(cx, |m, cx| m.open_or_create("s-b", "/p", false, cx));
        let delta = |sid: &str| FromServer::Host {
            host: HostEvent::SessionStatus {
                session_id: sid.into(),
                running: None,
                errored: None,
                unread: Some(true),
                pending_auth: None,
                pending_plan: None,
                background_work: None,
            },
        };
        mux.update(cx, |m, cx| m.set_focused(Some("s-a"), cx));
        server_conn.send_to_client(delta("s-a"));
        server_conn.send_to_client(delta("s-b"));
        cx.run_until_parked();
        assert!(
            !leaf_a.read_with(cx, |h, _| h.store.unread),
            "the focused leaf stays dark"
        );
        assert!(
            leaf_b.read_with(cx, |h, _| h.store.unread),
            "the parked leaf lights up"
        );
        let (a, b) = mux.read_with(cx, |m, cx| {
            let map = m.unread_map(cx);
            (map.get("s-a").copied(), map.get("s-b").copied())
        });
        assert_eq!(a, Some(false), "unread_map is the sidebar badge source");
        assert_eq!(b, Some(true));
        // Switching focus clears the new foreground and re-arms the old.
        mux.update(cx, |m, cx| m.set_focused(Some("s-b"), cx));
        assert!(
            !leaf_b.read_with(cx, |h, _| h.store.unread),
            "activation clears the mirror"
        );
        server_conn.send_to_client(delta("s-a"));
        cx.run_until_parked();
        assert!(
            leaf_a.read_with(cx, |h, _| h.store.unread),
            "the deprioritized leaf re-arms"
        );
        // Local-knowledge rise: parked lights, focused is a no-op.
        mux.update(cx, |m, cx| m.note_unread("s-a", cx));
        assert!(leaf_a.read_with(cx, |h, _| h.store.unread));
        mux.update(cx, |m, cx| m.note_unread("s-b", cx));
        assert!(
            !leaf_b.read_with(cx, |h, _| h.store.unread),
            "note_unread on the focused leaf is suppressed"
        );
        // A leaf created while its session is focused starts active.
        mux.update(cx, |m, cx| m.set_focused(Some("s-c"), cx));
        let leaf_c = mux.update(cx, |m, cx| m.open_or_create("s-c", "/p", false, cx));
        server_conn.send_to_client(delta("s-c"));
        cx.run_until_parked();
        assert!(
            !leaf_c.read_with(cx, |h, _| h.store.unread),
            "ensure_leaf auto-activates the focused session's leaf"
        );
    }

    /// §二.3 user-visible stop: the sixth terminal failure sets a
    /// `FollowStop` the session chrome renders; a dismissal is remembered
    /// for this session and later stops cannot re-show it; a good snapshot
    /// withdraws the notice and refills the budget.
    #[gpui::test]
    fn follow_stop_lifecycle(cx: &mut TestAppContext) {
        let handle = cx.update(|cx| cx.new(|cx| ClientStoreHandle::leaf("s1", cx)));
        let resync = FromServer::StreamEnd {
            stream_id: StreamId::new("s1"),
            reason: manox_protocol::StreamEndReason::Resync,
        };
        let fail = |cx: &mut TestAppContext, n: u32| {
            for _ in 0..n {
                handle.update(cx, |h, cx| h.apply_from_server(resync.clone(), cx));
            }
        };
        // While the budget lasts, failures stay in backoff — not stopped.
        fail(cx, 5);
        assert_eq!(handle.read_with(cx, |h, _| h.follow_stop()), None);
        // The sixth is terminal: the notice is raised, undismissed.
        fail(cx, 1);
        assert_eq!(
            handle.read_with(cx, |h, _| h.follow_stop()),
            Some(FollowStop {
                reason: FollowStopReason::StreamFailing,
                dismissed: false,
            })
        );
        // Dismissed sticks: further (automatic) terminal failures never
        // bring the banner back.
        handle.update(cx, |h, cx| h.dismiss_follow_stop(cx));
        fail(cx, 3);
        assert_eq!(
            handle.read_with(cx, |h, _| h.follow_stop().map(|s| s.dismissed)),
            Some(true)
        );
        // A snapshot resumes following: notice withdrawn, budget refilled.
        handle.update(cx, |h, cx| {
            h.apply_from_server(item("s1", snapshot("s1", 0, vec![])), cx)
        });
        assert_eq!(handle.read_with(cx, |h, _| h.follow_stop()), None);
        fail(cx, 5);
        assert_eq!(
            handle.read_with(cx, |h, _| h.follow_stop()),
            None,
            "the budget restarted from zero"
        );
        fail(cx, 1);
        assert_eq!(
            handle.read_with(cx, |h, _| h.follow_stop()),
            Some(FollowStop {
                reason: FollowStopReason::StreamFailing,
                dismissed: false,
            }),
            "a fresh stop is a fresh notice, even after a dismissed one"
        );
    }

    /// The notice's retry entry: `retry_follow` clears the notice, re-arms
    /// the budget, and rides the outbound channel — its own request carries
    /// `reattach` (the multiplexer turns it into `OpenSession` +
    /// `StreamOpen`), the automatic path's do not.
    #[gpui::test]
    fn retry_follow_rearms_the_budget_and_reopens(cx: &mut TestAppContext) {
        let handle = cx.update(|cx| cx.new(|cx| ClientStoreHandle::leaf("s1", cx)));
        let (tx, rx) = async_channel::unbounded::<LeafRequest>();
        handle.update(cx, |h, _| h.set_outbound(tx));
        let resync = FromServer::StreamEnd {
            stream_id: StreamId::new("s1"),
            reason: manox_protocol::StreamEndReason::Resync,
        };
        let fail = |cx: &mut TestAppContext, n: u32| {
            for _ in 0..n {
                handle.update(cx, |h, cx| h.apply_from_server(resync.clone(), cx));
            }
        };
        fail(cx, 6);
        handle.update(cx, |h, cx| {
            assert!(h.follow_stop().is_some());
            h.dismiss_follow_stop(cx);
            h.retry_follow(cx);
        });
        assert_eq!(handle.read_with(cx, |h, _| h.follow_stop()), None);
        // Budget re-armed: the retry spent attempt 1, four more failures
        // stay in backoff...
        fail(cx, 4);
        assert_eq!(handle.read_with(cx, |h, _| h.follow_stop()), None);
        // ...and the next one stops again — undismissed, because the retry
        // was an explicit user action whose outcome must be visible.
        fail(cx, 1);
        assert_eq!(
            handle.read_with(cx, |h, _| h.follow_stop()),
            Some(FollowStop {
                reason: FollowStopReason::StreamFailing,
                dismissed: false,
            })
        );
        // Every non-terminal attempt scheduled exactly one backoff reopen
        // (attempts 1-5 before the retry, 1-5 after); the clock past the
        // whole ladder drains them all onto the channel.
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(9000));
        cx.run_until_parked();
        let mut reopens = 0;
        let mut reattached = 0;
        while let Ok(req) = rx.try_recv() {
            match req {
                LeafRequest::Reopen {
                    session_id,
                    reattach,
                    ..
                } => {
                    assert_eq!(
                        session_id.as_str(),
                        "s1",
                        "only s1 reopens ride the channel"
                    );
                    reopens += 1;
                    reattached += u32::from(reattach);
                }
                other => panic!("only Reopen requests ride the channel, got {other:?}"),
            }
        }
        assert_eq!(reopens, 10, "five pre-retry + five post-retry reopens");
        assert_eq!(
            reattached, 1,
            "only the user-facing retry re-attaches; automatic reopens stay pure StreamOpen"
        );
    }

    /// The retry contract with nothing to re-open: with no multiplexer wired
    /// the click must leave the notice it cannot act on, and must not spend an
    /// attempt on it.
    #[gpui::test]
    fn retry_without_an_outbound_leaves_the_notice_up(cx: &mut TestAppContext) {
        let handle = cx.update(|cx| cx.new(|cx| ClientStoreHandle::leaf("s1", cx)));
        let resync = FromServer::StreamEnd {
            stream_id: StreamId::new("s1"),
            reason: manox_protocol::StreamEndReason::Resync,
        };
        for _ in 0..6 {
            handle.update(cx, |h, cx| h.apply_from_server(resync.clone(), cx));
        }
        assert!(handle.read_with(cx, |h, _| h.follow_stop()).is_some());
        handle.update(cx, |h, cx| h.retry_follow(cx));
        assert_eq!(
            handle.read_with(cx, |h, _| h.follow_stop()),
            Some(FollowStop {
                reason: FollowStopReason::StreamFailing,
                dismissed: false,
            }),
            "a retry with nothing to re-open must leave the notice up"
        );
        assert_eq!(
            handle.read_with(cx, |h, _| h.reopen_attempts),
            6,
            "and must not spend an attempt on it"
        );
    }

    /// The manox-i18n scan gate resolves whatever these helpers return, but a
    /// rename would silently point them at an unregistered key and the gate
    /// would stay green — pin the literals here, next to the match that owns
    /// them.
    #[test]
    fn follow_stop_reason_keys_are_the_registered_literals() {
        assert_eq!(
            FollowStopReason::StreamFailing.notice_key(),
            "follow-stop-stream-failing"
        );
        assert_eq!(
            FollowStopReason::StreamFailing.indicator_key(),
            "follow-stop-indicator-stream-failing"
        );
    }
}
