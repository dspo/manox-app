//! Attach-time catch-up for the live-only thread state a parked thread drops.
//!
//! Retired in v3: the AHP chat state is a complete fold of the journal, so a
//! re-attach rebuilds the whole transcript from the snapshot — there is no
//! live-only window to replay, and the observation surfaces re-derive from
//! the same fold. The entry point stays so the attach path is unchanged.

use super::*;

impl Workspace {
    /// Formerly the parked-window replay. v3 folds everything, so this is a
    /// no-op kept for the attach path's shape.
    pub(super) fn catch_up_live_only_state(&mut self, _cx: &mut Context<Self>) {}
}
