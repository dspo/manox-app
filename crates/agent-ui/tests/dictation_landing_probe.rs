//! The dictation transcript's landing: a finished transcript inserts at the
//! composer's caret through the public edit path (so the input's `Change`
//! fires and the completion popover re-syncs), the composer takes focus back,
//! and an empty transcript (the recognizer heard nothing) is a no-op. The
//! recognizer itself is upstream's (gpui-component `speech`); this pins OUR
//! half — where the text lands, where the caret ends, what holds focus.
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{Focusable as _, TestAppContext, VisualTestContext};

/// The seeded draft is ASCII so byte offsets, chars and UTF-16 units agree —
/// the assertions slice the value at the caret's offset without caring which
/// unit `selected_range` reports.
const DRAFT: &str = "hello world";
const TRANSCRIPT: &str = "beautiful";

#[gpui::test]
async fn a_final_transcript_lands_at_the_caret_and_refocuses(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let (seeded, caret) = window
        .update(&mut visual.cx, |_, window, cx| {
            let input = workspace.read(cx).diagnostic_chat_input(cx);
            input.update(cx, |s, cx| {
                s.set_value(DRAFT, window, cx);
                (s.value().to_string(), s.selected_range().start)
            })
        })
        .expect("window");

    // The production route is the `SpeechEvent::Final` subscription; the
    // diagnostic entry drives the same `land_dictation` directly.
    window
        .update(&mut visual.cx, |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                ws.diagnostic_land_dictation(TRANSCRIPT, window, cx)
            });
        })
        .expect("window");

    window
        .update(&mut visual.cx, |_, window, cx| {
            let input = workspace.read(cx).diagnostic_chat_input(cx);
            input.update(cx, |s, cx| {
                let expected = format!("{}{TRANSCRIPT}{}", &seeded[..caret], &seeded[caret..]);
                assert_eq!(s.value().to_string(), expected, "inserted at the caret");
                let range = s.selected_range();
                assert!(
                    range.start == range.end && range.start == caret + TRANSCRIPT.len(),
                    "the caret collapsed past the inserted text, got {range:?}"
                );
                assert!(
                    s.focus_handle(cx).is_focused(window),
                    "the composer took focus back for the next keystroke"
                );
            });
        })
        .expect("window");
}

#[gpui::test]
async fn an_empty_transcript_is_a_noop(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let seeded = window
        .update(&mut visual.cx, |_, window, cx| {
            let input = workspace.read(cx).diagnostic_chat_input(cx);
            input.update(cx, |s, cx| {
                s.set_value(DRAFT, window, cx);
                s.value().to_string()
            })
        })
        .expect("window");

    window
        .update(&mut visual.cx, |_, window, cx| {
            workspace.update(cx, |ws, cx| ws.diagnostic_land_dictation("", window, cx));
        })
        .expect("window");

    window
        .update(&mut visual.cx, |_, _, cx| {
            let input = workspace.read(cx).diagnostic_chat_input(cx);
            input.update(cx, |s, _| {
                assert_eq!(s.value().to_string(), seeded, "nothing was inserted");
            });
        })
        .expect("window");
}
