//! The composer interrupt chord must DISPATCH end-to-end: the registered
//! binding parses, the focused input's `composer > Input` context matches,
//! and the `ComposerSubmitInterrupt` action reaches the workspace handler.
//! `secondary` no longer parses at build time (gpui-pre 0.3.10+), so a typo'd
//! chord would otherwise fail silently — this test is that guard. The lanes
//! suite covers the handler logic by calling it directly; this one covers
//! the binding itself.

use common::open_workspace;
use gpui::{TestAppContext, VisualTestContext};

mod common;

#[gpui::test]
async fn interrupt_chord_dispatches(cx: &mut TestAppContext) {
    common::init_harness(cx);
    cx.update(|cx| {
        steer_i18n::init();
        agent_ui::slash_command::init(cx);
        cx.bind_keys(agent_ui::composer_send_key_bindings());
    });
    let (window, ws) = open_workspace(cx);
    let handle = window.into();
    let mut visual = VisualTestContext::from_window(handle, cx);

    // Fixture: detached store + fabricated running turn — the same scene the
    // lanes suite uses, so `chat/turnCancelled` is the crisp "the chord
    // fired" signal.
    visual.update(|_window, cx| {
        agent_ui::workspace::gallery::seed_scene(&ws, cx);
    });

    // The binding's predicate is `composer > Input`: the input must hold
    // focus for the keystroke to route anywhere.
    visual.update(|window, cx| {
        ws.update(cx, |ws, cx| ws.gallery_focus_composer(window, cx));
    });

    // One real platform-primary press (⌘⇧↩ on macOS, ctrl⇧↩ elsewhere).
    let chord = if cfg!(target_os = "macos") {
        "cmd-shift-enter"
    } else {
        "ctrl-shift-enter"
    };
    visual.simulate_keystrokes(chord);

    visual.update(|_window, cx| {
        assert!(
            ws.read(cx)
                .gallery_dispatch_log(cx)
                .iter()
                .any(|(_, tag)| tag.as_str() == "chat/turnCancelled"),
            "the interrupt chord must reach the workspace handler"
        );
    });
}
