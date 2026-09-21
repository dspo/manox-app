//! Transient "copied" feedback for copy controls: the control's icon flips to
//! a check for a beat after its click, then reverts.
//!
//! Copy buttons are rebuilt every frame from their owning view's render, so
//! the feedback state cannot live in the button. It lives instead in a
//! [`CopiedRegistry`] owned by the entity that renders the buttons, keyed by
//! each control's `ElementId`; the owner exposes it through
//! [`CopyFeedbackHost`]. `fire_copied` marks the click and schedules the
//! revert redraw; [`copy_button`] is the standard ghost icon control that
//! reads `is_active` to pick its glyph and routes its click back through the
//! owner's weak handle.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{App, ClipboardItem, Context, ElementId, WeakEntity};
use gpui_component::{IconName, Sizable, button::Button, button::ButtonVariants};

/// How long a copy control shows its copied state after a click.
pub const COPY_FEEDBACK: Duration = Duration::from_millis(1200);

/// Which copy controls recently fired, keyed by the control's `ElementId`.
/// Owned by the entity that renders the controls; entries expire on their own
/// and a re-fire while active replaces the generation so the icon stays lit.
#[derive(Default)]
pub struct CopiedRegistry {
    fired: HashMap<ElementId, Instant>,
}

impl CopiedRegistry {
    /// Whether `id` was clicked within [`COPY_FEEDBACK`].
    pub fn is_active(&self, id: &ElementId) -> bool {
        self.fired
            .get(id)
            .is_some_and(|t| t.elapsed() < COPY_FEEDBACK)
    }

    /// Record a click on `id` and return the generation stamped for it. A
    /// re-fire while active replaces the entry, so the older revert timer is
    /// a no-op against the new generation.
    pub fn fire(&mut self, id: &ElementId) -> Instant {
        self.fired.retain(|_, t| t.elapsed() < COPY_FEEDBACK);
        let now = Instant::now();
        self.fired.insert(id.clone(), now);
        now
    }

    /// Drop the entry for `id` if it is still the given generation.
    fn clear_if_current(&mut self, id: &ElementId, generation: Instant) {
        if self.fired.get(id) == Some(&generation) {
            self.fired.remove(id);
        }
    }
}

/// An entity that hosts copy controls and their feedback state.
pub trait CopyFeedbackHost: 'static {
    fn copied(&mut self) -> &mut CopiedRegistry;
}

/// Record a click on `id`: mark it active now and schedule the revert redraw
/// one [`COPY_FEEDBACK`] later. A re-fire replaces the generation, so only the
/// newest timer's revert lands.
pub fn fire_copied<T: CopyFeedbackHost>(host: &mut T, id: ElementId, cx: &mut Context<T>) {
    let generation = host.copied().fire(&id);
    cx.notify();
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(COPY_FEEDBACK).await;
        let _ = this.update(cx, |host, cx| {
            host.copied().clear_if_current(&id, generation);
            cx.notify();
        });
    })
    .detach();
}

/// The standard copy control: a ghost xsmall icon button that writes `text` to
/// the clipboard. Shows a check instead of the copy glyph while the control's
/// id is active in its owner's [`CopiedRegistry`].
pub fn copy_button<T: CopyFeedbackHost>(
    id: impl Into<ElementId>,
    text: String,
    active: bool,
    owner: WeakEntity<T>,
) -> Button {
    let id = id.into();
    Button::new(id.clone())
        .ghost()
        .xsmall()
        .icon(if active {
            IconName::Check
        } else {
            IconName::Copy
        })
        .on_click(move |_, _, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            let _ = owner.update(cx, |host, cx| fire_copied(host, id.clone(), cx));
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::ElementId;

    #[test]
    fn fire_activates_until_feedback_elapses() {
        let mut registry = CopiedRegistry::default();
        let id: ElementId = ("copy", 1usize).into();
        assert!(!registry.is_active(&id));
        registry.fire(&id);
        assert!(registry.is_active(&id));
    }

    #[test]
    fn expired_entries_are_inactive() {
        let mut registry = CopiedRegistry::default();
        let id: ElementId = ("copy", 1usize).into();
        let stale = Instant::now() - COPY_FEEDBACK - Duration::from_millis(1);
        registry.fired.insert(id.clone(), stale);
        assert!(!registry.is_active(&id));
    }

    #[test]
    fn fire_prunes_expired_entries() {
        let mut registry = CopiedRegistry::default();
        let stale_id: ElementId = ("copy", 1usize).into();
        let fresh_id: ElementId = ("copy", 2usize).into();
        let stale = Instant::now() - COPY_FEEDBACK - Duration::from_millis(1);
        registry.fired.insert(stale_id.clone(), stale);
        registry.fire(&fresh_id);
        assert!(!registry.fired.contains_key(&stale_id));
        assert!(registry.fired.contains_key(&fresh_id));
    }

    #[test]
    fn re_fire_replaces_generation_so_old_revert_is_a_noop() {
        let mut registry = CopiedRegistry::default();
        let id: ElementId = ("copy", 1usize).into();
        let first = registry.fire(&id);
        let second = registry.fire(&id);
        registry.clear_if_current(&id, first);
        assert!(registry.is_active(&id), "old generation must not revert");
        registry.clear_if_current(&id, second);
        assert!(!registry.is_active(&id), "current generation reverts");
    }
}
