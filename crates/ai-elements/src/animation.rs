//! Shared conventions for the components' toggle animations.

use gpui::ElementId;

/// The identity a toggle animation is keyed on.
///
/// gpui replays an animation when its key changes and only then, so the key has
/// to carry the block's identity *and* which way it is going. `generation` is
/// that second half: a block with a state entity bumps its own counter, a
/// controlled block derives one from the frames it has seen (see
/// `chain_of_thought::toggle_generation`). Passing an unchanging generation
/// gives an animation that plays once per mount, which is what a step's
/// entrance wants.
pub(crate) fn toggle_key(id: &ElementId, role: &'static str, generation: u64) -> ElementId {
    let role_id: ElementId = (id.clone(), role).into();
    (role_id, format!("g{generation}")).into()
}
