//! Dock badge — the count on the app's Dock icon (macOS).
//!
//! The number is the sessions currently owing the user attention (`Workspace::attention_count`):
//! a parked ask / tool-approval card, a plan review awaiting a verdict, an
//! errored thread, or an unseen settle on a non-focused thread. It drops the
//! same way the sidebar's attention marks do — the user answers the card,
//! rules on the plan, or focuses the thread.
//!
//! The count is not pushed from any single state edge: the client-owned
//! unread mirror rises through paths that only notify the leaf entity, so a
//! change-fed badge would need observers on every live leaf. A foreground
//! poll (the tray pump's pattern) reads the aggregate instead and only
//! crosses into AppKit when the value actually changed — badge latency of one
//! poll period is imperceptible for this affordance.

use std::time::Duration;

use gpui::App;

/// Foreground poll period. Badge updates are advisory; a fraction of a
/// second keeps the dock icon honest without any state-edge plumbing.
const POLL: Duration = Duration::from_millis(500);

/// Cache of the last applied count so an unchanged aggregate never reaches
/// AppKit. Starts at 0: an empty badge needs no clearing at startup.
static LAST_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Spawn the foreground pump that mirrors the workspace's attention count
/// onto the dock badge. Call once after the main window opened (the
/// workspace global must exist for the pump to observe anything); a second
/// call panics.
pub fn spawn_pump(cx: &mut App) {
    static PUMP_SPAWNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    PUMP_SPAWNED
        .set(())
        .expect("badge::spawn_pump called more than once");
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL).await;
            // The workspace is process-lifetime once registered; holding the
            // clone across the read keeps it alive against the read itself.
            let Some(workspace) = agent_ui::dispatch::workspace_global() else {
                continue;
            };
            let count = workspace.read_with(cx, |ws, cx| ws.attention_count(cx));
            set_count(count);
        }
    })
    .detach();
}

/// Apply `count` to the dock badge, skipping redundant AppKit round-trips.
/// A count that could not be applied (off-main caller) is deliberately not
/// cached, so the next poll retries.
pub fn set_count(count: usize) {
    if LAST_COUNT.load(std::sync::atomic::Ordering::Relaxed) == count {
        return;
    }
    if backend::apply(count) {
        LAST_COUNT.store(count, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The badge string for `count`: `None` clears the badge; above 99 the
/// standard "99+" ceiling keeps the glyph inside the icon's corner.
fn badge_label(count: usize) -> Option<String> {
    match count {
        0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".into()),
    }
}

/// macOS backend: `NSDockTile.setBadgeLabel` — the platform's native dock
/// count. AppKit requires the main thread; the pump runs on the gpui main
/// thread, and `MainThreadMarker::new` re-verifies instead of trusting it.
#[cfg(target_os = "macos")]
mod backend {
    use super::badge_label;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::NSString;

    pub(super) fn apply(count: usize) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("dock badge skipped: not on the main thread");
            return false;
        };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        match badge_label(count) {
            Some(label) => tile.setBadgeLabel(Some(&NSString::from_str(&label))),
            None => tile.setBadgeLabel(None),
        }
        true
    }
}

/// Fallback for platforms with no dock badge backend: Windows would carry
/// the count as an `ITaskbarList3` overlay icon and Linux has no
/// cross-desktop launcher-badge contract, so they keep a clean no-op until
/// a backend lands (the pump stays cheap and the API surface unchanged).
#[cfg(not(target_os = "macos"))]
mod backend {
    pub(super) fn apply(_count: usize) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::badge_label;

    #[test]
    fn badge_label_clears_at_zero_and_ceilings_at_99() {
        assert_eq!(badge_label(0), None);
        assert_eq!(badge_label(1).as_deref(), Some("1"));
        assert_eq!(badge_label(99).as_deref(), Some("99"));
        assert_eq!(badge_label(100).as_deref(), Some("99+"));
    }
}
