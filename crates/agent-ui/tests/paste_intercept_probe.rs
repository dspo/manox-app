//! The composer's paste intercept riding the input's `on_paste` hook
//! (gpui-component 0.8): a clipboard image stages as a pending attachment
//! and nothing reaches the rope (the alt-text regression class), and a text
//! paste falls through to the engine — the text lands at the caret and the
//! `InputEvent::Change` it emits drives the completion re-sync (asserted via
//! a planted popover snapshot that must be replaced).
//!
//! Own test binary: shared harness singletons (see `tests/common/mod.rs`).
#![cfg(feature = "test-support")]

mod common;

use common::{init_harness, open_workspace};
use gpui::{TestAppContext, VisualTestContext};
use steer_agent_chat_ui::column::PendingAttachment;

const PNG_BYTES: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

#[gpui::test]
async fn a_clipboard_image_stages_as_an_attachment_and_the_rope_stays_clean(
    cx: &mut TestAppContext,
) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    window
        .update(&mut visual.cx, |_, window, cx| {
            workspace
                .read(cx)
                .diagnostic_chat_input(cx)
                .update(cx, |s, cx| s.focus(window, cx));
            // Image + text entries, as many producers write both flavors: the
            // image must win (the old capture intercept's precedence) and the
            // text must never land in the rope.
            let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, PNG_BYTES.to_vec());
            cx.write_to_clipboard(gpui::ClipboardItem {
                entries: vec![
                    gpui::ClipboardEntry::Image(image),
                    gpui::ClipboardEntry::String(gpui::ClipboardString::new(
                        "alt-text".to_string(),
                    )),
                ],
            });
        })
        .expect("window");

    // The production route is the input's cmd-v binding; the probe dispatches
    // the same action the binding maps to, at the focused input.
    visual.dispatch_action(gpui_component::input::Paste);
    visual.run_until_parked();

    window
        .update(&mut visual.cx, |_, _, cx| {
            let attachments = workspace.read(cx).diagnostic_pending_attachments(cx);
            assert_eq!(
                attachments.len(),
                1,
                "the clipboard image staged as one pending attachment"
            );
            assert!(
                matches!(attachments[0], PendingAttachment::ClipboardImage(_)),
                "staged as a clipboard image, got {:?}",
                attachments[0]
            );
            let input = workspace.read(cx).diagnostic_chat_input(cx);
            assert_eq!(
                input.read(cx).value().to_string(),
                "",
                "no alt-text may reach the rope"
            );
        })
        .expect("window");
}

#[gpui::test]
async fn a_text_paste_lands_in_the_rope_and_resyncs_the_completion(cx: &mut TestAppContext) {
    init_harness(cx);
    let (window, workspace) = open_workspace(cx);
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    let (seeded, caret) = window
        .update(&mut visual.cx, |_, window, cx| {
            let seeded = workspace
                .read(cx)
                .diagnostic_chat_input(cx)
                .update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.set_value("pre-", window, cx);
                    (s.value().to_string(), s.selected_range().start)
                });
            workspace.update(cx, |ws, cx| ws.diagnostic_seed_completion(cx));
            seeded
        })
        .expect("window");

    window
        .update(&mut visual.cx, |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("hello".to_string()));
        })
        .expect("window");

    visual.dispatch_action(gpui_component::input::Paste);
    visual.run_until_parked();

    window
        .update(&mut visual.cx, |_, _, cx| {
            let input = workspace.read(cx).diagnostic_chat_input(cx);
            let expected = format!("{}hello{}", &seeded[..caret], &seeded[caret..]);
            assert_eq!(
                input.read(cx).value().to_string(),
                expected,
                "the text fell through to the engine and landed at the caret"
            );
            // The popover sources are empty in tests, so the planted snapshot
            // being replaced (no trigger left → closed) is the observable of
            // the Change-driven `sync_completion` run: without the re-sync the
            // snapshot would survive the paste untouched.
            assert!(
                workspace.read(cx).diagnostic_completion(cx).is_none(),
                "the planted completion was replaced — the Change-driven re-sync ran"
            );
        })
        .expect("window");
}
