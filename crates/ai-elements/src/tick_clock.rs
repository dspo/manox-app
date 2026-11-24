//! The one shared status-glyph clock.
//!
//! Every animated spinner on screen (`BrailleSpinner`, `PixelSpinner`) mounts
//! as an entity view observing this single ticker. The OLD per-instance
//! `with_animation` timers notified the *containing* view — with the heavy
//! content inlined there, every beat re-ran the whole ancestor render chain,
//! so N running conversations meant hundreds of full-window rebuilds per
//! second (#320). The fix is the CLOCK CONVERGENCE, not isolation: a tick
//! still marks the glyph's ancestor views dirty (`mark_view_dirty` walks the
//! whole chain — no gpui API spares them), but the beat count is now a
//! constant ~30/s regardless of N, and the ancestors' renders are cheap
//! (they emit cached child entities; see `SidebarLens` and
//! `ActivityStepView`). One tick costs the glyph itself plus those cheap
//! re-emits — the structural floor, not a vanished cost.
//!
//! The clock runs while at least one glyph entity is alive: glyph entities
//! live in window-scoped keyed state, so unmounting the last spinner (an
//! idle conversation, a scrolled-out row) drops the global's weak handle and
//! the loop exits — no timers behind a spinner-free UI.
//!
//! Ticks are gated on the App too: no watched window visible, or
//! `reduce_motion` set (the system's "reduce motion"), and the loop keeps
//! its period but notifies nothing — the glyphs then render their static
//! frames, exactly what the retired `with_animation` path did.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, App, AppContext as _, AsyncApp, Entity, Global, WeakEntity, Window};

/// The shared glyph frame rate: 30 fps, the pixel spinner's native hop
/// cadence (~34 ms per stop — the sidebar's old `.max_fps(30.)` requirement,
/// which a 12 fps fleet beat aliased into visible skipped hops). Braille is
/// unaffected by the higher rate: its frames are time-driven (80 ms each),
/// so finer sampling only removes the repeats a 12 fps beat introduced.
/// 30 beats/s is still a constant, versus the hundreds of full-window
/// rebuilds per second this replaced (see #320).
pub const GLYPH_FPS: f64 = 30.;

fn tick_period() -> Duration {
    Duration::from_secs_f64(1. / GLYPH_FPS)
}

/// Diagnostic line for `MANOX_FRAME_DIAG=1`: one row per interval with the
/// clock's real pacing and the live glyph count — the regression tripwire
/// for "clock count grows with conversations" (#320 P2).
const DIAG_ENV: &str = "MANOX_FRAME_DIAG";
const DIAG_INTERVAL: Duration = Duration::from_secs(2);

/// Live glyph entities, for the diagnostic line and tests.
static LIVE_GLYPHS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn glyph_mounted() {
    LIVE_GLYPHS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn glyph_released() {
    LIVE_GLYPHS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
}

/// The number of glyph entities currently alive.
pub fn live_glyphs() -> usize {
    LIVE_GLYPHS.load(std::sync::atomic::Ordering::Relaxed)
}

struct SharedClock(Option<WeakEntity<TickClock>>);

impl Global for SharedClock {}

/// The App-wide status-glyph ticker.
pub struct TickClock {
    started: Instant,
    /// Windows showing at least one glyph; a tick notifies only while at
    /// least one of them is visible. Plain ids — they die with their window.
    watched: RefCell<Vec<AnyWindowHandle>>,
}

impl TickClock {
    /// The shared clock, creating (and starting) it on first use. Cheap to
    /// call from every glyph mount.
    pub fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(clock) = cx
            .try_global::<SharedClock>()
            .and_then(|g| g.0.clone())
            .and_then(|w| w.upgrade())
        {
            return clock;
        }
        let clock = cx.new(|_| Self {
            started: Instant::now(),
            watched: RefCell::new(Vec::new()),
        });
        cx.set_global(SharedClock(Some(clock.downgrade())));
        spawn_tick_loop(clock.downgrade(), cx);
        clock
    }

    /// Milliseconds since the clock's epoch — the phase every glyph derives
    /// its frame from. Time-based (not tick-count-based), so a dropped tick
    /// shifts nothing. f64 on purpose: an f32 mantissa would start losing
    /// milliseconds after ~22 h of uptime, inside one braille frame.
    pub fn ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.
    }

    /// Register a window whose visibility gates ticking.
    pub fn watch(&self, window: &Window) {
        let handle = window.window_handle();
        let mut watched = self.watched.borrow_mut();
        if !watched.contains(&handle) {
            watched.push(handle);
        }
    }

    fn watched_windows(&self) -> Vec<AnyWindowHandle> {
        self.watched.borrow().clone()
    }
}

fn spawn_tick_loop(clock: WeakEntity<TickClock>, cx: &mut App) {
    let diag = std::env::var(DIAG_ENV).is_ok_and(|v| v != "0");
    let started = Instant::now();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let period = tick_period();
        let mut ticks: u64 = 0;
        let mut drift_total = Duration::ZERO;
        loop {
            let wake = Instant::now() + period;
            cx.background_executor().timer(period).await;
            // Exit once the last glyph unmounted (the global holds only a
            // weak handle).
            let Some(clock) = clock.upgrade() else {
                break;
            };
            let watched = cx.read_entity(&clock, |clock, _| clock.watched_windows());
            // One fold classifies every watched window: ALIVE (open, maybe
            // hidden) stays registered — a hidden window may come back, and
            // dropping it would freeze its glyphs forever (nothing re-runs
            // `use_keyed_state` to re-register); CLOSED drops out.
            let (alive, any_visible) = watched.iter().copied().fold(
                (Vec::new(), false),
                |(mut alive, any_visible), handle| {
                    match handle.update(cx, |_, window, _| {
                        window.visibility() == gpui::WindowVisibility::Visible
                    }) {
                        Ok(is_visible) => {
                            alive.push(handle);
                            (alive, any_visible || is_visible)
                        }
                        Err(_) => (alive, any_visible),
                    }
                },
            );
            let reduce_motion = cx.update(|cx| cx.reduce_motion());
            // No watched window at all keeps the beat: glyphs mounted
            // outside a watched window must not freeze the clock for
            // everyone.
            let beat = !reduce_motion && (alive.is_empty() || any_visible);
            // The strong handle pins the entity; the update cannot fail.
            // One update prunes to the alive set and fires the beat.
            clock.update(cx, |clock, cx| {
                if clock.watched.borrow().as_slice() != alive.as_slice() {
                    *clock.watched.borrow_mut() = alive.clone();
                }
                if beat {
                    cx.notify();
                }
            });
            ticks += 1;
            if diag {
                drift_total += Instant::now().duration_since(wake);
                let every =
                    (DIAG_INTERVAL.as_millis() as u64 / period.as_millis() as u64).max(1);
                if ticks.is_multiple_of(every) {
                    let elapsed = started.elapsed().as_secs_f32();
                    log::info!(
                        "frame-diag: {:.1} tick/s (target {:.1}), mean drift {:.2} ms, {} live glyphs",
                        ticks as f32 / elapsed,
                        GLYPH_FPS,
                        drift_total.as_secs_f32() * 1000. / ticks.max(1) as f32,
                        live_glyphs(),
                    );
                }
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::IntoElement;

    #[gpui::test]
    fn shared_clock_is_a_single_app_wide_entity(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let a = TickClock::shared(cx);
            let b = TickClock::shared(cx);
            assert_eq!(a.entity_id(), b.entity_id());
        });
    }

    #[gpui::test]
    fn clock_ms_is_monotonic(cx: &mut gpui::TestAppContext) {
        // The clock phases off `Instant`, not the test scheduler's fake
        // clock — the invariant worth pinning here is monotonicity; frame
        // math is pinned by the pure phase tests in `pixel_spinner`.
        let clock = cx.update(TickClock::shared);
        let t0 = clock.read_with(cx, |clock, _| clock.ms());
        std::thread::sleep(Duration::from_millis(15));
        let t1 = clock.read_with(cx, |clock, _| clock.ms());
        assert!(t1 >= t0, "{t0} -> {t1}");
        assert!(t0 < f64::MAX);
    }

    /// The P1 lifecycle claim: the loop notifies on its beat while a
    /// consumer holds the clock, and goes quiet once the last strong handle
    /// drops (the global only keeps a weak one).
    #[gpui::test]
    async fn tick_loop_stops_once_the_last_consumer_drops(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        struct Probe;
        impl gpui::Render for Probe {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                gpui::div()
            }
        }
        let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let other = std::rc::Rc::new(std::cell::Cell::new(0u32));
        // Probes must outlive the beat window: an entity's subscriptions die
        // with it.
        let (_probe, _probe2, clock) = cx.update(|cx| {
            let clock = TickClock::shared(cx);
            let sink = ticks.clone();
            let probe = cx.new(|cx| {
                cx.observe(&clock, move |_, _, _| {
                    sink.set(sink.get() + 1);
                })
                .detach();
                Probe
            });
            // A second observer on the SAME clock: the whole fleet advances
            // on one beat (no glyph left behind on its own timer).
            let sink2 = other.clone();
            let probe2 = cx.new(|cx| {
                cx.observe(&clock, move |_, _, _| {
                    sink2.set(sink2.get() + 1);
                })
                .detach();
                Probe
            });
            (probe, probe2, clock)
        });
        // Park first so the loop reaches its timer await; only then does a
        // virtual-time jump release the beats.
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        let while_alive = ticks.get();
        assert!(
            while_alive >= 2,
            "expected beats while alive, got {while_alive}"
        );
        assert_eq!(other.get(), while_alive, "the fleet beats together");

        // Drop the last strong handle; the loop's weak upgrade must fail and
        // the beat must go silent.
        drop(clock);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert_eq!(
            ticks.get(),
            while_alive,
            "loop kept ticking after the last consumer dropped",
        );
        assert_eq!(other.get(), while_alive, "second observer stopped too");
    }
}
