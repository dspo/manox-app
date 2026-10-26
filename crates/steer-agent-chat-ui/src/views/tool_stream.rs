//! The tool card body's published snapshot: a batched, append-only view of a
//! tool call's output while it streams.
//!
//! A card body is a keyed rich-text document: the `TextView` state is looked
//! up by element id, and the block highlighter hands the parsed text a run
//! list built for this frame. When the text changes discontinuously under a
//! committed parse — stream batching, the live-tail head drop past 12 KiB, a
//! retry rewriting the output, the settle-time truncation — the runs and the
//! text can disagree and the rich-text core aborts in `StyledText::with_runs`
//! ("invalid text run"). This snapshot makes the rendered text a value the
//! card controls:
//!
//! - while streaming, only a longer prefix of the published text is an
//!   *extension*; anything else is a *replace* (a new generation, hence a
//!   fresh document state);
//! - extensions are coalesced to one publish per [`PUBLISH_TICK`];
//! - a card open-state change holds extensions for [`REVEAL_HOLD`]: the
//!   reveal animation re-lays the body out as root every frame, and that
//!   co-occurrence is where the production abort landed;
//! - the streaming snapshot is bounded by [`PUBLISH_CAP`]; past it the window
//!   resets to the source's live tail (a replace).
//!
//! The type is pure state: the caller supplies the clock, so tests are
//! deterministic. Authoritative data (copy actions, diff tabs, status) keeps
//! reading the entry's full output; only the rendered body reads the
//! snapshot.

use std::time::{Duration, Instant};

use crate::views::message::live_tail;

/// The most text the snapshot holds while streaming before resetting the
/// window to the source's live tail.
pub const PUBLISH_CAP: usize = 24 * 1024;

/// Coalescing interval for append-only publishes.
pub const PUBLISH_TICK: Duration = Duration::from_millis(100);

/// How long a card open-state change suppresses tick-driven publishes.
pub const REVEAL_HOLD: Duration = Duration::from_millis(400);

/// What a call to [`ToolStreamSnapshot::observe`] did to the published text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publish {
    None,
    /// The published text grew by a suffix; the document state stays valid.
    Extended,
    /// The published text was replaced; the generation advanced and the
    /// renderer must key a fresh document state.
    Replaced,
}

/// Batched, append-only publication of a tool call's streamed output.
#[derive(Clone, Debug, Default)]
pub struct ToolStreamSnapshot {
    published: String,
    generation: u64,
    last_publish: Option<Instant>,
    hold_until: Option<Instant>,
    /// Source length at the last window reset. Another reset needs that much
    /// new growth, so the snapshot never re-windows once per chunk.
    reset_mark: usize,
}

impl ToolStreamSnapshot {
    /// The text the card body renders.
    pub fn published(&self) -> &str {
        &self.published
    }

    /// Bumped on every non-append replace. The element id carries it, so a
    /// replace cannot be paired with the previous document's parse.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Observe the source's current text. The first non-empty text publishes
    /// immediately so a card never starts blank for a tick; afterwards an
    /// append is coalesced to one publish per [`PUBLISH_TICK`] and suppressed
    /// during a reveal hold. A non-append change publishes at once unless the
    /// derived (windowed) text is unchanged.
    pub fn observe(&mut self, source: &str, now: Instant) -> Publish {
        if source == self.published {
            return Publish::None;
        }
        if !self.is_append(source) {
            return self.replace(source, now);
        }
        let first = self.published.is_empty();
        if source.len() > PUBLISH_CAP
            && (first || source.len() >= self.reset_mark.saturating_add(PUBLISH_CAP))
        {
            self.window(source, now);
            return Publish::Replaced;
        }
        if first {
            self.published.push_str(source);
            self.last_publish = Some(now);
            return Publish::Extended;
        }
        if self.hold_until.is_some_and(|until| now < until) {
            return Publish::None;
        }
        if self
            .last_publish
            .is_some_and(|last| now.saturating_duration_since(last) < PUBLISH_TICK)
        {
            return Publish::None;
        }
        self.published.push_str(&source[self.published.len()..]);
        self.last_publish = Some(now);
        Publish::Extended
    }

    /// A card open-state change: hold tick-driven publishes for
    /// [`REVEAL_HOLD`]. Replacements and [`Self::flush`] are never held back.
    pub fn hold_reveal(&mut self, now: Instant) {
        self.hold_until = Some(now + REVEAL_HOLD);
    }

    /// Publish the source unconditionally (settle / forced). A settled body is
    /// the authoritative one, so it bypasses the cap, the tick and the reveal
    /// hold; a non-append change still advances the generation.
    pub fn flush(&mut self, source: &str) -> Publish {
        self.hold_until = None;
        self.last_publish = None;
        if source == self.published {
            return Publish::None;
        }
        let append = self.is_append(source);
        self.reset_mark = if source.len() > PUBLISH_CAP {
            source.len()
        } else {
            0
        };
        self.published.clear();
        self.published.push_str(source);
        if append {
            Publish::Extended
        } else {
            self.generation += 1;
            Publish::Replaced
        }
    }

    /// The source extends the published text with a suffix.
    fn is_append(&self, source: &str) -> bool {
        source.len() > self.published.len() && source.starts_with(self.published.as_str())
    }

    /// A non-append change: publish immediately, from the live tail when the
    /// text outgrows the streaming cap.
    fn replace(&mut self, source: &str, now: Instant) -> Publish {
        let windowed = source.len() > PUBLISH_CAP;
        let next = if windowed {
            live_tail(source)
        } else {
            source.to_string()
        };
        // The derived window can be identical while the source grew in a
        // region the tail already covered. Nothing to paint, and repainting
        // would mint a fresh document state for the same text.
        if next == self.published {
            return Publish::None;
        }
        self.generation += 1;
        self.last_publish = Some(now);
        self.reset_mark = if windowed { source.len() } else { 0 };
        self.published = next;
        Publish::Replaced
    }

    /// Reset the append window to the source's live tail.
    fn window(&mut self, source: &str, now: Instant) {
        self.generation += 1;
        self.reset_mark = source.len();
        self.last_publish = Some(now);
        self.published = live_tail(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: u64) -> Instant {
        Instant::now() + Duration::from_millis(millis)
    }

    #[test]
    fn the_first_non_empty_text_publishes_immediately() {
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe("hello", at(0)), Publish::Extended);
        assert_eq!(snapshot.published(), "hello");
        assert_eq!(snapshot.generation(), 0);
    }

    #[test]
    fn an_empty_source_publishes_nothing() {
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe("", at(0)), Publish::None);
        assert_eq!(snapshot.published(), "");
        assert_eq!(snapshot.generation(), 0);
    }

    #[test]
    fn appends_coalesce_to_one_publish_per_tick() {
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe("a", at(0)), Publish::Extended);

        assert_eq!(snapshot.observe("ab", at(99)), Publish::None);
        assert_eq!(snapshot.published(), "a");

        assert_eq!(snapshot.observe("abc", at(100)), Publish::Extended);
        assert_eq!(snapshot.published(), "abc");
        assert_eq!(snapshot.generation(), 0);
    }

    #[test]
    fn a_reveal_hold_suppresses_tick_publishes_but_not_replaces() {
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe("a", at(0)), Publish::Extended);

        snapshot.hold_reveal(at(10));
        assert_eq!(snapshot.observe("ab", at(200)), Publish::None);
        assert_eq!(snapshot.published(), "a");

        // The hold expires at 410ms; the tick has long elapsed.
        assert_eq!(snapshot.observe("abc", at(410)), Publish::Extended);
        assert_eq!(snapshot.published(), "abc");

        // A rewrite is a replace and is not held back.
        snapshot.hold_reveal(at(500));
        assert_eq!(snapshot.observe("rewritten", at(510)), Publish::Replaced);
        assert_eq!(snapshot.published(), "rewritten");
        assert_eq!(snapshot.generation(), 1);
    }

    #[test]
    fn a_flush_is_not_held_back() {
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe("a", at(0)), Publish::Extended);
        snapshot.hold_reveal(at(1));

        assert_eq!(snapshot.flush("abc"), Publish::Extended);
        assert_eq!(snapshot.published(), "abc");
        assert_eq!(snapshot.generation(), 0);

        assert_eq!(snapshot.flush("final"), Publish::Replaced);
        assert_eq!(snapshot.published(), "final");
        assert_eq!(snapshot.generation(), 1);
        assert_eq!(snapshot.flush("final"), Publish::None);
    }

    #[test]
    fn a_rewrite_is_a_replace_and_bumps_the_generation() {
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe("first attempt", at(0)), Publish::Extended);

        assert_eq!(
            snapshot.observe("different output", at(1)),
            Publish::Replaced
        );
        assert_eq!(snapshot.published(), "different output");
        assert_eq!(snapshot.generation(), 1);

        // A shorter text is never an extension, even when the published text
        // was a prefix of the previous source.
        assert_eq!(snapshot.observe("diff", at(2)), Publish::Replaced);
        assert_eq!(snapshot.published(), "diff");
        assert_eq!(snapshot.generation(), 2);
    }

    #[test]
    fn a_live_tail_head_drop_is_a_replace() {
        let mut snapshot = ToolStreamSnapshot::default();
        // Cross PUBLISH_CAP in one append: the window resets to the live tail.
        let long = "line of output\n".repeat(PUBLISH_CAP / 8);
        assert_eq!(snapshot.observe(&long, at(0)), Publish::Replaced);
        assert_eq!(snapshot.published(), live_tail(&long));
        assert_eq!(snapshot.generation(), 1);

        // Growing the source changes the tail's head, so it cannot be an
        // extension of the published text.
        let longer = format!("{long}another line\n");
        assert_eq!(snapshot.observe(&longer, at(200)), Publish::Replaced);
        assert_eq!(snapshot.published(), live_tail(&longer));
        assert_eq!(snapshot.generation(), 2);
    }

    /// Render passes re-observe the same source after a windowed replace; the
    /// derived tail is unchanged, so no parse is burnt and no generation moves.
    #[test]
    fn an_unchanged_windowed_source_does_not_replace_again() {
        let long = "line of output\n".repeat(PUBLISH_CAP / 8);
        let mut snapshot = ToolStreamSnapshot::default();
        assert_eq!(snapshot.observe(&long, at(0)), Publish::Replaced);
        let generation = snapshot.generation();

        assert_eq!(snapshot.observe(&long, at(200)), Publish::None);
        assert_eq!(snapshot.generation(), generation);
        assert_eq!(snapshot.published(), live_tail(&long));
    }

    #[test]
    fn growth_past_the_cap_resets_the_window_once_per_cap() {
        let mut snapshot = ToolStreamSnapshot::default();
        let unit = "line of output\n";
        let near_cap: String = unit.repeat(PUBLISH_CAP / unit.len());
        assert_eq!(snapshot.observe(&near_cap, at(0)), Publish::Extended);
        assert_eq!(snapshot.generation(), 0);

        // Cross the cap: the published text resets to the live tail.
        let crossed = format!("{near_cap}{unit}");
        assert!(crossed.len() > PUBLISH_CAP);
        assert_eq!(snapshot.observe(&crossed, at(200)), Publish::Replaced);
        assert_eq!(snapshot.published(), live_tail(&crossed));
        assert_eq!(snapshot.generation(), 1);

        // Settle forces the full text back in; the reset mark moves with it.
        assert_eq!(snapshot.flush(&crossed), Publish::Replaced);
        assert_eq!(snapshot.published(), crossed);
        assert_eq!(snapshot.generation(), 2);

        // Less than another cap of growth stays a plain extension.
        let warmed = format!("{crossed}{unit}");
        assert_eq!(snapshot.observe(&warmed, at(400)), Publish::Extended);
        assert_eq!(snapshot.generation(), 2);

        // A full cap of growth past the mark resets again.
        let regrown = format!("{warmed}{}", unit.repeat(PUBLISH_CAP / unit.len()));
        assert_eq!(snapshot.observe(&regrown, at(600)), Publish::Replaced);
        assert_eq!(snapshot.published(), live_tail(&regrown));
        assert_eq!(snapshot.generation(), 3);
    }
}
