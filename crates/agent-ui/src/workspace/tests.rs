//! The workspace test suite (v3): the pure-function slice. The v2 wire-
//! behavioural tests retired with the wire itself — their replacements drive
//! the AhpStore fold (tracked as the W3 coverage backlog).

use super::{RecallDirection, RecallStep, Workspace};

#[cfg(test)]
mod suite {
    use super::*;

    fn step(
        direction: RecallDirection,
        value: &str,
        index: i64,
        draft: Option<&str>,
        turns: &[&str],
    ) -> (i64, Option<String>, Option<String>) {
        let owned: Vec<String> = turns.iter().map(|t| t.to_string()).collect();
        let (index, draft, step) = Workspace::recall_step(direction, value, index, draft, &owned);
        let applied = match step {
            RecallStep::None => None,
            RecallStep::Recall(text) => Some(text),
            RecallStep::Clear => Some(String::new()),
        };
        (index, draft, applied)
    }

    fn assert_recall(step: &(i64, Option<String>, Option<String>), index: i64, text: &str) {
        assert_eq!(step.0, index);
        assert_eq!(step.2.as_deref(), Some(text));
    }

    #[test]
    fn recall_up_from_an_empty_composer_starts_at_the_newest_turn() {
        let turns = ["newest", "oldest"];
        let out = step(RecallDirection::Up, "", -1, None, &turns);
        assert_recall(&out, 0, "newest");
        // Nothing was left behind, so the walk has no draft to return to.
        assert_eq!(out.1, None);
    }

    #[test]
    fn recall_up_from_a_draft_keeps_it_as_the_working_line() {
        let turns = ["newest", "oldest"];
        let out = step(RecallDirection::Up, "typed draft", -1, None, &turns);
        assert_recall(&out, 0, "newest");
        assert_eq!(out.1.as_deref(), Some("typed draft"));
    }

    #[test]
    fn recall_up_walks_further_back_and_clamps_at_the_oldest() {
        let turns = ["newest", "middle", "oldest"];
        assert_recall(
            &step(RecallDirection::Up, "newest", 0, None, &turns),
            1,
            "middle",
        );
        assert_recall(
            &step(RecallDirection::Up, "middle", 1, None, &turns),
            2,
            "oldest",
        );
        let at_oldest = step(RecallDirection::Up, "oldest", 2, None, &turns);
        assert_eq!(at_oldest.0, 2);
        assert_eq!(at_oldest.2, None, "already at the oldest turn");
    }

    #[test]
    fn recall_up_without_history_does_nothing() {
        let out = step(RecallDirection::Up, "", -1, None, &[]);
        assert_eq!(out.0, -1);
        assert_eq!(out.2, None);
    }

    #[test]
    fn recall_down_walks_toward_newer_and_restores_the_working_line() {
        let turns = ["newest", "middle", "oldest"];
        assert_recall(
            &step(RecallDirection::Down, "oldest", 2, Some("draft"), &turns),
            1,
            "middle",
        );
        assert_recall(
            &step(RecallDirection::Down, "middle", 1, Some("draft"), &turns),
            0,
            "newest",
        );
        let at_newest = step(RecallDirection::Down, "newest", 0, Some("draft"), &turns);
        assert_eq!(at_newest.0, -1, "the walk ends past the newest turn");
        assert_eq!(at_newest.1, None, "and its draft is spent");
        assert_eq!(at_newest.2.as_deref(), Some("draft"));
    }

    #[test]
    fn recall_down_at_the_newest_clears_an_empty_working_line() {
        let turns = ["newest"];
        let out = step(RecallDirection::Down, "newest", 0, None, &turns);
        assert_eq!(out.0, -1);
        assert_eq!(out.2.as_deref(), Some(""), "started from an empty input");
    }

    #[test]
    fn recall_down_outside_a_walk_does_nothing() {
        let turns = ["newest"];
        let out = step(RecallDirection::Down, "", -1, None, &turns);
        assert_eq!(out.0, -1);
        assert_eq!(out.2, None);
    }

    #[test]
    fn an_edited_recalled_turn_becomes_the_working_line() {
        let turns = ["newest", "middle"];
        // The walk keeps stepping after an edit, and the edited text is what
        // comes back when the walk runs off the newest end.
        let stepped = step(RecallDirection::Up, "newest edited", 0, None, &turns);
        assert_recall(&stepped, 1, "middle");
        assert_eq!(stepped.1.as_deref(), Some("newest edited"));
        let back = step(
            RecallDirection::Down,
            "middle",
            1,
            Some("newest edited"),
            &turns,
        );
        assert_recall(&back, 0, "newest");
        assert_eq!(back.1.as_deref(), Some("newest edited"));
        let home = step(
            RecallDirection::Down,
            "newest",
            0,
            Some("newest edited"),
            &turns,
        );
        assert_eq!(home.0, -1);
        assert_eq!(home.2.as_deref(), Some("newest edited"));
    }

    #[test]
    fn a_stale_walk_index_starts_a_fresh_walk() {
        let turns = ["newest", "oldest"];
        let out = step(RecallDirection::Up, "whatever", 5, None, &turns);
        assert_recall(&out, 0, "newest");
        assert_eq!(out.1.as_deref(), Some("whatever"));
    }
    #[test]
    fn draft_survives_a_long_walk() {
        let turns = ["newest", "middle", "older", "oldest"];
        let mut value = "my draft".to_string();
        let mut index = -1i64;
        let mut draft: Option<String> = None;
        for expected in ["newest", "middle", "older", "oldest"] {
            let (next, kept, applied) =
                step(RecallDirection::Up, &value, index, draft.as_deref(), &turns);
            assert_eq!(applied.as_deref(), Some(expected));
            assert_eq!(
                kept.as_deref(),
                Some("my draft"),
                "draft held at {expected}"
            );
            index = next;
            draft = kept;
            value = applied.unwrap();
        }
        for expected in ["older", "middle", "newest"] {
            let (next, kept, applied) = step(
                RecallDirection::Down,
                &value,
                index,
                draft.as_deref(),
                &turns,
            );
            assert_eq!(applied.as_deref(), Some(expected));
            assert_eq!(kept.as_deref(), Some("my draft"));
            index = next;
            draft = kept;
            value = applied.unwrap();
        }
        let (ended, spent, applied) = step(
            RecallDirection::Down,
            &value,
            index,
            draft.as_deref(),
            &turns,
        );
        assert_eq!(ended, -1, "the walk ends once the draft is restored");
        assert_eq!(spent, None);
        assert_eq!(applied.as_deref(), Some("my draft"));
    }

    #[test]
    fn steer_group_insert_index_keeps_steers_before_the_queue() {
        use crate::conversation::{UserImage, UserTurnMeta};
        let turn = |text: &str| crate::workspace::DeferredUserTurn {
            text: text.to_string(),
            images: vec![],
            meta: UserTurnMeta::new(1, "m".into(), None),
            user_images: Vec::<UserImage>::new(),
        };
        let q = |state| crate::workspace::QueuedFollowUp {
            turn: turn("x"),
            state,
        };
        use crate::workspace::FollowUpState as S;

        let empty = std::collections::VecDeque::new();
        assert_eq!(super::Workspace::steer_group_insert_index(&empty), 0);

        let all_queued: std::collections::VecDeque<_> =
            [q(S::Queued), q(S::Queued)].into_iter().collect();
        assert_eq!(super::Workspace::steer_group_insert_index(&all_queued), 0);

        let all_steers: std::collections::VecDeque<_> = [
            q(S::SteerPending {
                message_id: "k".into(),
            }),
            q(S::Failed),
        ]
        .into_iter()
        .collect();
        assert_eq!(super::Workspace::steer_group_insert_index(&all_steers), 2);

        let mixed: std::collections::VecDeque<_> = [
            q(S::SteerPending {
                message_id: "k".into(),
            }),
            q(S::Failed),
            q(S::Queued),
            q(S::Queued),
        ]
        .into_iter()
        .collect();
        assert_eq!(super::Workspace::steer_group_insert_index(&mixed), 2);
    }

    #[test]
    fn queue_move_index_reorders_only_the_queued_tail() {
        use crate::workspace::composer_render::{QueueDragEdge as E, QueueRowDrag as D};
        let turn = |text: &str| crate::workspace::DeferredUserTurn {
            text: text.to_string(),
            images: vec![],
            meta: crate::conversation::UserTurnMeta::new(1, "m".into(), None),
            user_images: Vec::new(),
        };
        let q = |state| crate::workspace::QueuedFollowUp {
            turn: turn("x"),
            state,
        };
        use crate::workspace::FollowUpState as S;

        let queue: std::collections::VecDeque<_> = [
            q(S::SteerPending {
                message_id: "k".into(),
            }),
            q(S::Queued),
            q(S::Queued),
            q(S::Queued),
        ]
        .into_iter()
        .collect();

        // Forward move: row 1 dropped below row 3 → lands at the tail.
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &queue,
                D {
                    dragged: 1,
                    line_on: 3,
                    edge: E::Bottom
                }
            ),
            Some(3)
        );
        // Backward move: row 3 dropped above row 1 → lands at 1.
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &queue,
                D {
                    dragged: 3,
                    line_on: 1,
                    edge: E::Top
                }
            ),
            Some(1)
        );
        // Self drops no-op (own row, and the adjacent slot that re-inserts in place).
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &queue,
                D {
                    dragged: 2,
                    line_on: 2,
                    edge: E::Top
                }
            ),
            None
        );
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &queue,
                D {
                    dragged: 2,
                    line_on: 1,
                    edge: E::Bottom
                }
            ),
            None
        );
        // Landing inside the committed head is rejected.
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &queue,
                D {
                    dragged: 3,
                    line_on: 0,
                    edge: E::Top
                }
            ),
            None
        );
        // A committed source never moves: SteerPending and Failed alike.
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &queue,
                D {
                    dragged: 0,
                    line_on: 3,
                    edge: E::Bottom
                }
            ),
            None
        );
        let with_failed: std::collections::VecDeque<_> = [q(S::Failed), q(S::Queued), q(S::Queued)]
            .into_iter()
            .collect();
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &with_failed,
                D {
                    dragged: 0,
                    line_on: 2,
                    edge: E::Bottom
                }
            ),
            None
        );
        // All-Queued still reorders freely to the end.
        let all_queued: std::collections::VecDeque<_> =
            [q(S::Queued), q(S::Queued)].into_iter().collect();
        assert_eq!(
            crate::workspace::ChatColumn::queue_move_index_in(
                &all_queued,
                D {
                    dragged: 0,
                    line_on: 1,
                    edge: E::Bottom
                }
            ),
            Some(1)
        );
    }

    #[test]
    fn a_plain_ask_carries_no_intent() {
        let payload = serde_json::json!({
            "questions": [{"question": "Which color?", "options": [{"label": "Red"}]}]
        });
        let ask =
            crate::workspace::parse_pending_ask("ask1".into(), payload).expect("plain ask parses");
        assert!(
            ask.questions[0].intent.is_none(),
            "no intent → generic ask card"
        );
        assert!(
            ask.questions[0].detail.is_empty(),
            "no detail → no markdown block"
        );
    }
}

/// The ←/→ history invariants (see `NavHistory`): these pin the state
/// machine's dedup / truncation / cap / edge semantics without a Window.
#[cfg(test)]
mod nav_history {
    use super::super::NavHistory;

    fn recorded(ids: &[&str]) -> NavHistory {
        let mut nav = NavHistory::default();
        for id in ids {
            nav.record(id);
        }
        nav
    }

    #[test]
    fn reopening_the_current_entry_after_a_back_step_is_a_noop() {
        let mut nav = recorded(&["a", "b", "c"]);
        nav.step_back();
        // The pointer sits on b; clicking b's row again must not duplicate
        // it (an entry dup here made ← land on the same b twice).
        nav.record("b");
        assert_eq!(nav.step_back().as_deref(), Some("a"));
        assert_eq!(nav.step_forward().as_deref(), Some("b"));
    }

    #[test]
    fn a_new_open_truncates_the_forward_tail() {
        let mut nav = recorded(&["a", "b", "c"]);
        nav.step_back();
        nav.record("d");
        assert_eq!(nav.step_forward(), None, "the tail after d must be gone");
        assert_eq!(nav.step_back().as_deref(), Some("b"));
    }

    #[test]
    fn the_cap_drains_the_front_and_keeps_the_index_at_the_tail() {
        let mut nav = NavHistory::default();
        for i in 0..110 {
            nav.record(&format!("t{i}"));
        }
        assert_eq!(nav.step_back().as_deref(), Some("t108"));
        // The front drained: exactly the last 100 survive.
        assert_eq!(nav.step_back().map(|_| ()), Some(()));
    }

    #[test]
    fn edges_report_no_move_and_the_front_is_not_a_panic() {
        let mut nav = recorded(&["a", "b"]);
        assert_eq!(nav.step_back().as_deref(), Some("a"));
        assert_eq!(nav.step_back(), None, "the front is inert");
        assert!(!nav.avail().back);
        assert!(nav.avail().forward);
        assert_eq!(nav.step_forward().as_deref(), Some("b"));
        assert_eq!(nav.step_forward(), None, "the tail is inert");
    }

    #[test]
    fn an_empty_history_is_unavailable_in_both_directions() {
        let nav = NavHistory::default();
        assert!(!nav.avail().back);
        assert!(!nav.avail().forward);
    }

    #[test]
    fn the_successor_handoff_rewrites_the_current_entry_in_place() {
        let mut nav = recorded(&["a"]);
        // A 换代成 a'：同一场会话的新身份，原地改写而非追加。
        nav.replace_current("a2");
        assert_eq!(nav.step_back(), None, "rewrite must not grow the stack");
        nav.record("b");
        assert_eq!(
            nav.step_back().as_deref(),
            Some("a2"),
            "← lands on the SUCCESSOR id, never the retired predecessor"
        );
    }
}
