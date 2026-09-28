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
    fn cap_tab_label_caps_with_ellipsis() {
        let long = "a very long tab label that must be capped";
        let capped = crate::workspace::cap_tab_label(long);
        assert_eq!(
            capped.chars().count(),
            crate::workspace::RIGHT_TAB_LABEL_CAP + 1
        );
        assert!(capped.ends_with('\u{2026}'), "{capped}");
        // Short labels pass through untouched; unicode caps on char bounds.
        assert_eq!(crate::workspace::cap_tab_label("short"), "short");
        let unicode = "\u{4e2d}".repeat(crate::workspace::RIGHT_TAB_LABEL_CAP + 4);
        assert_eq!(
            crate::workspace::cap_tab_label(&unicode).chars().count(),
            crate::workspace::RIGHT_TAB_LABEL_CAP + 1
        );
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
    fn turn_navigator_layout_compensates_shell_gutter_and_card_border() {
        use crate::workspace::{
            CARD_BORDER, SHELL_PAD_EDGE, SHELL_PAD_LEFT, turn_navigator_layout,
        };
        use gpui::px;

        let rail_inset = px(crate::views::context_rail::ENV_CONTENT_INSET);
        let half_border = px(CARD_BORDER / 2.);

        // Default: expanded sidebar (260), no right pane, no rail. The insets are
        // gutter + card border on each side; the wide leftover clamps to 480.
        let l = turn_navigator_layout(px(1200.), px(260.), None, false);
        assert_eq!(l.left_inset, px(SHELL_PAD_LEFT) + px(260.) + half_border);
        assert_eq!(l.right_inset, px(SHELL_PAD_EDGE) + half_border);
        assert_eq!(l.panel_width, px(480.));

        // Collapsed sidebar: the left inset is just the gutter + border.
        let l = turn_navigator_layout(px(1200.), px(0.), None, false);
        assert_eq!(l.left_inset, px(SHELL_PAD_LEFT) + half_border);
        assert_eq!(l.panel_width, px(480.));

        // Right pane open: its width + editor divider join the right inset and
        // the available span shrinks below the 480 cap.
        let l = turn_navigator_layout(px(1200.), px(260.), Some(px(640.)), false);
        assert_eq!(
            l.right_inset,
            px(SHELL_PAD_EDGE)
                + half_border
                + px(640.)
                + px(crate::workspace::EDITOR_DIVIDER_WIDTH)
        );
        assert!(l.panel_width < px(480.) && l.panel_width > px(0.));

        // Context rail shown: its content inset joins the right side instead.
        let l = turn_navigator_layout(px(1200.), px(260.), None, true);
        assert_eq!(l.right_inset, px(SHELL_PAD_EDGE) + half_border + rail_inset);

        // Narrow window: the panel takes whatever fits, then floors at zero —
        // never negative (a negative width would poison the overlay layout).
        let l = turn_navigator_layout(px(400.), px(260.), None, false);
        assert_eq!(
            l.panel_width,
            px(400.) - l.left_inset - l.right_inset - px(24.)
        );
        let l = turn_navigator_layout(px(290.), px(260.), None, true);
        assert_eq!(l.panel_width, px(0.));
    }

    #[test]
    fn plan_review_ask_parses_intent_detail_and_approve() {
        let payload = serde_json::json!({
            "questions": [{
                "id": "plan-review",
                "question": "Review the proposed plan?",
                "header": "Plan",
                "detail": "# the plan\n\n- do the thing",
                "intent": {"kind": "plan-review", "approve": "Approve"},
                "options": [
                    {"label": "Approve"},
                    {"label": "Approve & compact"},
                    {"label": "Request changes"}
                ]
            }]
        });
        let ask = crate::workspace::parse_pending_ask("ask1".into(), payload)
            .expect("plan-review ask parses");
        assert_eq!(ask.questions.len(), 1, "a plan-review is one question");
        let q = &ask.questions[0];
        assert_eq!(q.id, "plan-review", "the server-minted id is preserved");
        assert_eq!(
            q.detail, "# the plan\n\n- do the thing",
            "the plan body rides detail"
        );
        let intent = q.intent.as_ref().expect("intent present");
        assert_eq!(
            intent.kind, "plan-review",
            "the derivation keys on this kind"
        );
        assert_eq!(
            intent.approve, "Approve",
            "the affirmative label is captured"
        );
        assert!(
            q.options.iter().any(|o| o.label == intent.approve),
            "approve must name one of this question's own options (the highlight invariant)"
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
