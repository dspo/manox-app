//! Render-level contract for the composer's delivery-error strip: a latched
//! submit-pipeline failure paints the strip (the failure states used to be
//! tracing-only, so a user typing into a dead composer had no signal at
//! all), the workspace wiring reads the same face the composer reads, and
//! clearing the latch removes the strip again.
//!
//! Own test binary on purpose, exactly one `#[gpui::test]` per binary: the
//! probes drive `window.draw` under the gpui deterministic scheduler, and
//! the harness's process-global singletons make two tests in one process
//! overwrite each other's store.
#![cfg(feature = "test-support")]

mod common;

use agent_ui::Workspace;
use common::{init_harness, open_workspace};
use gpui::{
    AppContext as _, Entity, IntoElement, Render, TestAppContext, VisualTestContext, Window, px,
    size,
};
use steer_agent_chat_ui::ahp_store::{AhpStore, DeliveryFailure};

struct StripProbe {
    ws: Entity<Workspace>,
}

impl Render for StripProbe {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        self.ws
            .update(cx, |ws, cx| ws.diagnostic_delivery_error_element(cx))
            .unwrap_or_else(|| gpui::div().into_any_element())
    }
}

#[gpui::test]
async fn delivery_error_strip_paints_while_latched_and_clears_with_it(cx: &mut TestAppContext) {
    init_harness(cx);
    let (_window, workspace) = open_workspace(cx);

    let store = cx.update(|cx| cx.new(|_| AhpStore::detached()));
    workspace.update(cx, |ws, cx| {
        ws.diagnostic_bind_store(store.clone(), "sess-1", cx);
    });
    assert_eq!(
        workspace.read_with(cx, |ws, cx| ws.diagnostic_delivery_failure(cx)),
        None,
        "a healthy store reads as no failure"
    );

    let probe = cx.open_window(size(px(480.), px(120.)), {
        let ws = workspace.clone();
        move |_, _| StripProbe { ws }
    });
    let mut visual = VisualTestContext::from_window(probe.into(), cx);
    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    assert!(
        visual.debug_bounds("delivery-error-strip").is_none(),
        "no latch, no strip"
    );

    // The failure latches (what the pump does on a dead transport) and the
    // workspace wiring plus the paint follow it.
    store.update(&mut visual.cx, |s, _| s.diagnostic_latch_delivery_failure());
    let latched = workspace.read_with(&visual.cx, |ws, cx| ws.diagnostic_delivery_failure(cx));
    assert_eq!(
        latched,
        Some(DeliveryFailure::Handshake("test: dead transport".into())),
        "the workspace reads the store's failure face"
    );
    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    let bounds = visual
        .debug_bounds("delivery-error-strip")
        .expect("the strip paints while the failure is latched");
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));

    // The latch clears (a successful redial lands here) and the strip goes.
    store.update(&mut visual.cx, |s, _| s.diagnostic_clear_delivery_failure());
    for _ in 0..3 {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
    assert!(
        visual.debug_bounds("delivery-error-strip").is_none(),
        "a cleared latch removes the strip"
    );
    manox_agent::thread_store::drop_global_for_test();
}
