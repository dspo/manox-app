//! The plan-mode chip surface (U9b cluster 2): the plan-mode mirror read and
//! the plan-mode toggle. Split from `workspace.rs` — `super` is the workspace
//! module; private methods are lifted `pub(super)` so the parent (render
//! handlers) and its `tests` child keep calling them.
//!
//! B2-PR-5: the plan-review VERDICT surface is gone. A proposed plan now
//! reaches the user as a single-question `AskUserQuestion` whose `intent.kind`
//! is `plan-review` (see the ask card + `chips.rs`), so the four-choice verdict
//! drawer, the `PlanVerdict` reply/cancel legs, and the `ExecuteFresh`
//! create-and-reseed path were retired with the server's `PlanVerdict` call.

use super::*;

impl Workspace {
    /// Plan mode active on the current thread (drives the composer chip).
    pub(crate) fn thread_plan_mode(&self, cx: &mut Context<Self>) -> bool {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.plan_mode)
            .expect("foreground store present")
    }

    /// Toggle plan mode on the current thread (persisted by the engine).
    pub(crate) fn set_thread_plan_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let _ = self.send_note(cx, |sid| manox_protocol::ClientNote::SetPlanMode {
            session_id: sid.into(),
            enabled,
        });
    }
}
