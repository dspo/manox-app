//! The session-title bridge (manox leg).
//!
//! manox ≥ v0.13.0 retired model-side automatic titles: the model emits a
//! plain OSC 0/2 sequence inside its Bash output (`printf '\033]2;t\007'`,
//! no redirection, no bespoke tool) and the embedder owns capture and
//! interpretation. The chain is
//!
//! ```text
//! raw bash chunk tap (manox-harness, process singleton)
//!   → one headless `steer_ghostty::Terminal` per session (the ONLY
//!     interpreter — no bespoke scanner, no policy table)
//!   → a program title the titlebar's session-picker trigger reads.
//! ```
//!
//! The projection is deliberately **tab-only**: a program title renames the
//! titlebar trigger (this UI's tab equivalent) and nothing else. Sidebar
//! rows, notifications and container names keep the session's own title — a
//! program title flowing into the container name renames the whole container.
//!
//! State is session-scoped: an entry lives as long as the session id does.
//! Closing a tab prunes nothing (background work that keeps running while the
//! tab is closed still titles the session), and only a deleted session drops
//! its entry (`prune`, driven by the store fold's deletion events).
//!
//! Sanitizing lives HERE, not in the kernel: trim, whitespace-only =
//! no title (the caller falls back to the session name), reuse of the
//! kernel's own byte cap and C1-terminator semantics. The kernel only
//! interprets the byte stream.
//!
//! AHP exposes ONE merged session title — a user rename and the mechanical
//! first-prompt prefix land in the same field (upstream `ThreadStore::
//! journal_title`: “a reader cannot tell them apart, which is correct: both
//! are the session's title”), and v0.13.0 ships no rename surface at all.
//! The projection therefore reads `program_title` first and falls back to
//! that merged title; the two cannot be ordered against each other from the
//! app side.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use manox_harness::{BashOutputTapId, InstallError, install_bash_output_tap};

/// The headless interpreter's surface. Titles are a single-line string; the
/// grid only has to exist so the VT parser runs with sane geometry.
const COLS: u16 = 80;
const ROWS: u16 = 24;

type Waker = Arc<dyn Fn() + Send + Sync>;

/// The process-wide program-title store (the tap and the UI share one).
pub struct TitleStore {
    sessions: Mutex<HashMap<String, Arc<Mutex<SessionInner>>>>,
    /// Session ids whose title moved since the last projection pass: wakes
    /// coalesce instead of queueing a repaint per retitle.
    dirty: Mutex<HashSet<String>>,
    /// Tells the UI side a title landed (set by the workspace; `None` in
    /// headless tests).
    waker: Mutex<Option<Waker>>,
}

struct SessionInner {
    /// The one interpreter for this session's byte stream.
    term: steer_ghostty::Terminal,
    /// The last non-empty title the program set; `None` until one lands and
    /// again when a program clears it (the caller then falls back to the
    /// session name).
    program_title: Option<String>,
    /// Set by the terminal's title effect callback. The callback runs inside
    /// `Terminal::write` while the session lock is held, so it only flips this
    /// flag — it never touches the store's locks (that would self-deadlock).
    title_changed: Arc<AtomicBool>,
}

impl Default for TitleStore {
    fn default() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            dirty: Mutex::new(HashSet::new()),
            waker: Mutex::new(None),
        }
    }
}

impl TitleStore {
    pub fn global() -> &'static TitleStore {
        static STORE: OnceLock<TitleStore> = OnceLock::new();
        STORE.get_or_init(TitleStore::default)
    }

    /// Install the process-singleton bash output tap. Must precede the first
    /// session start; the harness refuses a second install (a different
    /// embedder already owns the bytes), which is logged, never fatal.
    pub fn install() {
        let tap = Arc::new(|id: BashOutputTapId, chunk: &[u8]| {
            TitleStore::global().observe(&id, chunk);
        });
        match install_bash_output_tap(tap) {
            Ok(()) => {}
            Err(InstallError::AlreadyInstalled) => {
                tracing::warn!(
                    "bash output tap already installed; session program titles are disabled"
                )
            }
        }
    }

    /// Route one raw chunk by its session id. Bytes from a harness-standalone
    /// assembly (`session_id: None`) have no owning session and are dropped —
    /// `exec_id` is advisory and collides across sessions, so it never routes.
    pub fn observe(&self, id: &BashOutputTapId, chunk: &[u8]) {
        if let Some(session_id) = &id.session_id {
            self.feed(session_id, chunk);
        }
    }

    /// Feed one raw bash chunk into the session's interpreter.
    pub fn feed(&self, session_id: &str, chunk: &[u8]) {
        let Some(session) = self.session(session_id) else {
            // No interpreter (construction failed): titles stay off for this
            // session. Decorative feature — never panic the chunk path.
            return;
        };
        let mut inner = lock(&session);
        inner.title_changed.store(false, Ordering::Release);
        inner.term.write(chunk);
        if !inner.title_changed.swap(false, Ordering::Acquire) {
            return;
        }
        let title = inner
            .term
            .title()
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty());
        if inner.program_title == title {
            return;
        }
        inner.program_title = title;
        drop(inner);
        self.mark_dirty(session_id);
    }

    /// The session's program title, or `None` before one lands (and after a
    /// program clears it).
    pub fn program_title(&self, session_id: &str) -> Option<String> {
        let session = lock(&self.sessions).get(session_id)?.clone();
        lock(&session).program_title.clone()
    }

    /// Drop one session's interpreter and title (a deleted session). The
    /// deletion is an EVENT, not an inference: the store fold reports the
    /// removals that carry no archive mark (an archived session is revivable
    /// and keeps its entry), and the projection pump drives this — never a
    /// per-frame sweep over catalogue membership, which would drop titles
    /// during the catalogue's landing window and on archive exits.
    pub fn prune(&self, session_id: &str) {
        lock(&self.sessions).remove(session_id);
        lock(&self.dirty).remove(session_id);
    }

    /// The session ids whose title moved since the last call.
    pub fn take_dirty(&self) -> Vec<String> {
        lock(&self.dirty).drain().collect()
    }

    /// Register the UI-side wake (a capacity-one channel send at the call
    /// site: bursts collapse into one repaint).
    pub fn set_waker(&self, waker: Waker) {
        *lock(&self.waker) = Some(waker);
    }

    /// The session's interpreter, created on first byte. `None` when the
    /// headless terminal refuses to build.
    fn session(&self, session_id: &str) -> Option<Arc<Mutex<SessionInner>>> {
        if let Some(existing) = lock(&self.sessions).get(session_id) {
            return Some(existing.clone());
        }
        let mut term = steer_ghostty::Terminal::new(COLS, ROWS).ok()?;
        let title_changed = Arc::new(AtomicBool::new(false));
        term.set_on_title({
            let title_changed = title_changed.clone();
            Box::new(move || title_changed.store(true, Ordering::Release))
        });
        let created = Arc::new(Mutex::new(SessionInner {
            term,
            program_title: None,
            title_changed,
        }));
        // Two threads can reach here for one session; the first interpreter
        // wins so exactly one parses that session's byte stream.
        Some(
            lock(&self.sessions)
                .entry(session_id.to_string())
                .or_insert(created)
                .clone(),
        )
    }

    fn mark_dirty(&self, session_id: &str) {
        lock(&self.dirty).insert(session_id.to_string());
        let waker = lock(&self.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// A poisoned lock still holds a consistent map (every critical section ends
/// in a plain insert/remove), so recover instead of propagating the panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap_id(session_id: Option<&str>) -> BashOutputTapId {
        BashOutputTapId {
            session_id: session_id.map(str::to_string),
            exec_id: "call-1".to_string(),
        }
    }

    #[test]
    fn routes_by_session_and_drops_standalone_chunks() {
        let store = TitleStore::default();
        store.observe(&tap_id(None), b"\x1b]2;orphan\x07");
        assert!(store.program_title("s-1").is_none());

        store.observe(&tap_id(Some("s-1")), b"\x1b]2;alpha\x07");
        assert_eq!(store.program_title("s-1").as_deref(), Some("alpha"));
        // Another session's bytes never land on this one.
        assert!(store.program_title("s-2").is_none());
    }

    #[test]
    fn chunks_assemble_across_feed_boundaries() {
        let store = TitleStore::default();
        store.feed("s-1", b"\x1b]2;split ");
        store.feed("s-1", b"title\x07");
        assert_eq!(store.program_title("s-1").as_deref(), Some("split title"));
    }

    #[test]
    fn whitespace_title_clears_the_program_title() {
        let store = TitleStore::default();
        store.feed("s-1", b"\x1b]2;alpha\x07");
        assert_eq!(store.program_title("s-1").as_deref(), Some("alpha"));
        store.feed("s-1", b"\x1b]2;   \x07");
        assert!(store.program_title("s-1").is_none());
    }

    #[test]
    fn dirty_marks_only_real_changes_and_drains_once() {
        let store = TitleStore::default();
        store.feed("s-1", b"\x1b]2;alpha\x07");
        assert_eq!(store.take_dirty(), vec!["s-1".to_string()]);
        assert!(store.take_dirty().is_empty());

        // The same title re-emitted is not a change.
        store.feed("s-1", b"\x1b]2;alpha\x07");
        assert!(store.take_dirty().is_empty());

        store.feed("s-1", b"\x1b]2;beta\x07");
        assert_eq!(store.take_dirty(), vec!["s-1".to_string()]);
    }

    #[test]
    fn prune_drops_only_the_named_session() {
        let store = TitleStore::default();
        store.feed("s-1", b"\x1b]2;alpha\x07");
        store.feed("s-2", b"\x1b]2;beta\x07");
        store.prune("s-1");
        assert!(store.program_title("s-1").is_none());
        assert_eq!(store.program_title("s-2").as_deref(), Some("beta"));
        assert!(!store.take_dirty().iter().any(|id| id == "s-1"));
        // Pruning is an event, not a sweep: an entry no catalogue names
        // survives until its own deletion lands.
        store.feed("s-2", b"\x1b]2;gamma\x07");
        assert_eq!(store.program_title("s-2").as_deref(), Some("gamma"));
    }

    #[test]
    fn a_programs_clear_falls_back_to_the_session_name() {
        let store = TitleStore::default();
        store.feed("s-1", b"\x1b]2;alpha\x07");
        store.feed("s-1", b"\x1b]2;\x07");
        assert!(store.program_title("s-1").is_none());
    }
}
