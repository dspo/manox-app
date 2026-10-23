//! Send-mode lane regression, full wiring: the production workspace against
//! a detached store (fabricated running turn, every write parks). Exercises
//! the routing/settle/strand/retire paths — submit routing, settle flush
//! split, interrupt outbox, cancelled-settle stranding, consumption retire,
//! parked-bucket replay.
//!
//! One `#[gpui::test]` per binary: the tests initialize process-global
//! singletons whose store entities are app-affine (see
//! `tests/common/mod.rs`); scenarios run sequentially against one window.

use agent_ui::workspace::SubmitBehavior;
use agent_ui::workspace::gallery as driver;
use common::open_workspace;
use gpui::{TestAppContext, VisualTestContext};

mod common;

fn lanes(snapshot: &[(String, String)]) -> Vec<&str> {
    snapshot.iter().map(|(lane, _)| lane.as_str()).collect()
}

fn wire_tags(ws: &agent_ui::Workspace, cx: &gpui::App) -> Vec<String> {
    ws.gallery_dispatch_log(cx)
        .into_iter()
        .map(|(_, tag)| tag)
        .collect()
}

#[gpui::test]
async fn send_mode_lanes(cx: &mut TestAppContext) {
    common::init_harness(cx);
    cx.update(|cx| {
        steer_i18n::init();
        agent_ui::slash_command::init(cx);
        cx.bind_keys(agent_ui::composer_send_key_bindings());
    });
    let (window, ws) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    // Fixture: detached store + fabricated running turn + a CLEAN queue (the
    // gallery scene, test-facing).
    visual.update(|_window, cx| {
        driver::seed_scene(&ws, cx);
    });

    // ── routing: one submit per behavior while running ─────────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_submit("引导一：先只看失败的测试", SubmitBehavior::Steer, cx);
            ws.gallery_submit("排队一：把错误日志再贴一遍", SubmitBehavior::Queue, cx);
            ws.gallery_submit("打断一：换 release 再跑", SubmitBehavior::Interrupt, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["steer", "queued"],
                "interrupt leaves no card; steer and queue land in their lanes"
            );
            assert_eq!(
                ws.gallery_outbox_len(cx),
                1,
                "the interrupt rides the outbox"
            );
            let wire = wire_tags(ws, cx);
            assert!(
                wire.contains(&"chat/pendingMessageSet".to_string()),
                "steer sets the pending message"
            );
            assert!(
                wire.contains(&"chat/turnCancelled".to_string()),
                "interrupt cancels the turn"
            );
            assert!(
                !wire.contains(&"chat/turnStarted".to_string()),
                "no turn starts under a running turn"
            );
        });
    });

    // ── consumption while running: the removal echo retires the card ───────
    let steer_id = visual
        .update(|_window, cx| ws.update(cx, |ws, cx| ws.gallery_first_steer_id(cx)))
        .expect("steer card parked");
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_inject_steer_consumed(&steer_id, cx);
            ws.gallery_replay_parked(cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["queued"],
                "the consumed steer card retires"
            );
            assert_eq!(
                ws.gallery_last_user_bubble(cx).as_deref(),
                Some("引导一：先只看失败的测试"),
                "the steered bubble lands in the transcript"
            );
        });
    });

    // ── settle split: the outbox flushes even on a cancelled settle; the
    // plain queue stays parked behind a stop ───────────────────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| ws.gallery_settle(true, cx));
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                ws.gallery_last_user_bubble(cx).as_deref(),
                Some("打断一：换 release 再跑"),
                "the outbox flushes across a cancelled settle"
            );
            assert_eq!(ws.gallery_outbox_len(cx), 0);
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["queued"],
                "a stop never fires the plain queue"
            );
        });
    });

    // ── normal settle drains the queue as ONE combined turn ────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("排队二：跑 clippy", SubmitBehavior::Queue, cx);
            ws.gallery_set_running(false, cx);
            ws.gallery_settle(false, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(ws.gallery_queue_snapshot(cx).is_empty(), "the queue drains");
            let last = ws.gallery_last_user_bubble(cx).unwrap_or_default();
            assert!(
                last.contains("把错误日志再贴一遍") && last.contains("跑 clippy"),
                "the queue coalesces into one combined turn, got: {last}"
            );
        });
    });

    // ── strand: a steer card across a cancelled settle turns Failed ────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("引导二：会话内再试验证", SubmitBehavior::Steer, cx);
            ws.gallery_settle(true, cx); // the engine retracts on abort
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["failed"],
                "a cancelled settle strands the steer (positive retraction signal)"
            );
        });
    });

    // ── 进入引导 on the failed card: re-send under a fresh id ───────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_steer_card(0, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["steer"],
                "the failed card re-arms as 待引导"
            );
            // 引导一 + 引导二 + the retry: every set keeps its own log entry
            // (引导二's retraction was engine-side; the parked write remains
            // what was sent).
            assert_eq!(
                wire_tags(ws, cx)
                    .iter()
                    .filter(|t| *t == "chat/pendingMessageSet")
                    .count(),
                3,
                "the retry sets a fresh pending message"
            );
        });
    });

    // ── 重新排队: withdraw the steer and demote to the plain queue ──────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            let _id = ws.gallery_first_steer_id(cx).expect("steer parked");
            ws.gallery_requeue_card(0, cx);
            assert!(
                wire_tags(ws, cx).contains(&"chat/pendingMessageRemoved".to_string()),
                "requeue withdraws the steer"
            );
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["queued"],
                "the requeued card joins the plain queue"
            );
        });
    });

    // ── 立即打断 from a card: cancel + outbox, no card state ────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_interrupt_card(0, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.gallery_queue_snapshot(cx).is_empty(),
                "the card left the strip"
            );
            assert_eq!(ws.gallery_outbox_len(cx), 1, "the message rides the outbox");
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| ws.gallery_settle(true, cx));
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(
                ws.gallery_outbox_len(cx),
                0,
                "the interrupted message flushes at the settle"
            );
        });
    });

    // ── one submit per settle: an outbox flush leaves the queue for the
    // NEXT settle (no back-to-back mid-run submit) ──────────────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("排队四：稍后一起", SubmitBehavior::Queue, cx);
            ws.gallery_submit("打断二：现在就要答案", SubmitBehavior::Interrupt, cx);
            ws.gallery_set_running(false, cx);
            ws.gallery_settle(false, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert_eq!(ws.gallery_outbox_len(cx), 0, "the outbox flushed");
            assert_eq!(
                lanes(&ws.gallery_queue_snapshot(cx)),
                vec!["queued"],
                "the queue waits — flushing it now would submit mid-run"
            );
            assert_eq!(
                ws.gallery_last_user_bubble(cx).as_deref(),
                Some("打断二：现在就要答案"),
                "only the outbox turn went out"
            );
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| ws.gallery_settle(false, cx));
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.gallery_queue_snapshot(cx).is_empty(),
                "the queue flushes at the next settle"
            );
            let last = ws.gallery_last_user_bubble(cx).unwrap_or_default();
            assert!(
                last.contains("稍后一起"),
                "the queued batch went out: {last}"
            );
        });
    });

    // ── idle degradation: ⇧⌘↩ with nothing running is an ordinary send ─────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(false, cx);
            ws.gallery_submit("空闲打断退化：直接发", SubmitBehavior::Interrupt, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.gallery_queue_snapshot(cx).is_empty() && ws.gallery_outbox_len(cx) == 0,
                "idle interrupt parks nothing"
            );
            assert_eq!(
                ws.gallery_last_user_bubble(cx).as_deref(),
                Some("空闲打断退化：直接发"),
                "it degrades to an ordinary turn"
            );
        });
    });

    // ── card 编辑 / 删除: a steer card withdraws from the wire first ────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("引导四：会被编辑掉", SubmitBehavior::Steer, cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            // Consumed steers carry no CLIENT removal (the host published
            // that one), so assert the DELTA: the edit withdraws exactly the
            // steer this card had set.
            let removed_before = wire_tags(ws, cx)
                .iter()
                .filter(|t| *t == "chat/pendingMessageRemoved")
                .count();
            ws.gallery_edit_card(0, _window, cx);
            let removed_after = wire_tags(ws, cx)
                .iter()
                .filter(|t| *t == "chat/pendingMessageRemoved")
                .count();
            assert_eq!(
                removed_after,
                removed_before + 1,
                "the edit withdraws the steer it had set"
            );
            assert!(
                ws.gallery_queue_snapshot(cx).is_empty(),
                "the edited card leaves the strip"
            );
            assert!(
                ws.gallery_composer_text(cx).contains("会被编辑掉"),
                "the text returns to the composer"
            );
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("引导五：会被删除", SubmitBehavior::Steer, cx);
            let before = ws.gallery_queue_snapshot(cx).len();
            ws.gallery_delete_card(0, cx);
            assert_eq!(
                ws.gallery_queue_snapshot(cx).len(),
                before - 1,
                "the deleted card leaves the strip"
            );
            assert!(
                wire_tags(ws, cx).contains(&"chat/pendingMessageRemoved".to_string()),
                "the delete withdraws the steer"
            );
            ws.gallery_set_running(false, cx);
        });
    });

    // ── parked replay: consumed-while-away retires AND lands the bubble —
    // nothing turns Failed on a guess ──────────────────────────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("引导三：注入消费回执", SubmitBehavior::Steer, cx);
        });
    });
    let steer_id = visual
        .update(|_window, cx| ws.update(cx, |ws, cx| ws.gallery_first_steer_id(cx)))
        .expect("third steer parked");
    visual.update(|_window, cx| {
        // Park-and-return simulation: the echo lands in the bucket while no
        // transcript drains it; the replay (the re-attach reconcile) runs.
        ws.update(cx, |ws, cx| {
            ws.gallery_inject_steer_consumed(&steer_id, cx);
            ws.gallery_replay_parked(cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            let snapshot = ws.gallery_queue_snapshot(cx);
            assert!(
                !lanes(&snapshot).contains(&"failed"),
                "no card fails on a guess: {snapshot:?}"
            );
            assert_eq!(
                ws.gallery_last_user_bubble(cx).as_deref(),
                Some("引导三：注入消费回执"),
                "the replayed consumption lands the steered bubble"
            );
        });
    });

    // ── parked replay, idle-steer-wake edition: the receipt rides
    // TurnStarted{steer_id} — the card retires, the bubble lands, AND the
    // lifecycle event still flips the running edge (M7's swallowed-event
    // path, end to end) ───────────────────────────────────────────────────
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            // The card parks on the RUNNING turn; the turn then settles with
            // the steer still parked — the engine's settle-drain wake is what
            // starts the next run, and its TurnStarted carries the id.
            ws.gallery_set_running(true, cx);
            ws.gallery_submit("引导四：空闲唤醒", SubmitBehavior::Steer, cx);
            ws.gallery_set_running(false, cx);
        });
    });
    let steer_id = visual
        .update(|_window, cx| ws.update(cx, |ws, cx| ws.gallery_first_steer_id(cx)))
        .expect("steer parked across the settle");
    assert!(
        !ws.read_with(&visual.cx, |ws, cx| ws.gallery_turn_active(cx)),
        "idle before the wake"
    );
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            ws.gallery_inject_parked_turn_start(&steer_id, cx);
            ws.gallery_replay_parked(cx);
        });
    });
    visual.update(|_window, cx| {
        ws.update(cx, |ws, cx| {
            assert!(
                ws.gallery_turn_active(cx),
                "the wake's TurnStarted flips the running edge"
            );
            assert!(
                ws.gallery_queue_snapshot(cx).is_empty(),
                "the woken steer's card retired"
            );
            assert_eq!(
                ws.gallery_last_user_bubble(cx).as_deref(),
                Some("引导四：空闲唤醒"),
                "the steered bubble lands"
            );
        });
    });
}
