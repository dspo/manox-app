//! The close-semantics pin: a ghostty view's process dies with the view.
//! Closing a tab drops the view — dropping the view must kill the process
//! (the group sweep itself is pty-level and platform-bound; this pins the
//! view → kill edge the close path relies on).

#![cfg(feature = "test-support")]

use gpui::TestAppContext;
use steer_ghostty::test_support::test_process;
use steer_ghostty::{GhosttyView, TerminalProcess, Theme};

#[gpui::test]
async fn dropping_the_view_kills_the_process(cx: &mut TestAppContext) {
    let (process, probe) = test_process();
    let process = std::sync::Arc::new(process) as std::sync::Arc<dyn TerminalProcess>;
    let view = cx
        .update(|cx| GhosttyView::spawn_with(process, Theme::LIGHT, cx))
        .expect("the fixture view spawns");
    assert!(!probe.killed(), "a live view keeps its process");

    // Closing the tab drops the view (the store reset); the view's `Drop`
    // forwards to `TerminalProcess::kill` on a detached thread, so the flag
    // is polled rather than assumed.
    cx.update(|_| drop(view));
    for _ in 0..250 {
        cx.run_until_parked();
        if probe.killed() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        probe.killed(),
        "dropping the view kills the backing process"
    );
}
