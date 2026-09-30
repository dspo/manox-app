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
            .map(|(store, sid)| {
                let view = store.read(cx);
                manox_agent_chat_ui::ahp_store::plan_mode_of(&view.book, sid)
            })
            .unwrap_or(false)
    }

    /// Toggle plan mode on the current thread (persisted by the engine).
    pub(crate) fn set_thread_plan_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let (store, sid) = match self.chat.read(cx).store.clone() {
            Some(pair) => pair,
            None => return,
        };
        store.update(cx, |store, _| {
            store.dispatch(
                format!("{}{sid}", manox_ahp::ext::channels::PLAN),
                ahp_types::actions::StateAction::Unknown(serde_json::json!({
                    "type": manox_ahp::ext::actions::PLAN_MODE_CHANGED,
                    "enabled": enabled,
                })),
            );
        });
    }
}
