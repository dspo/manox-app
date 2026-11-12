//! Durable memory of user turns that have not reached the journal yet.
//!
//! Optimistic bubbles and parked queue cards live in memory: a process death
//! between submit and the fold's echo silently dropped the user's text. This
//! append-only JSONL (one line per undelivered turn) survives that death.
//! Recovery is a draft, never a replay — the text is handed to its session's
//! composer the next time the session opens, and the row leaves with it.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One undelivered turn, keyed by the client's own id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UnsentTurn {
    /// Schema version; the file is append-only and never migrated.
    pub v: u32,
    /// The client-minted turn id (the adoption identity) for a submit, or a
    /// client id for a parked turn.
    pub id: String,
    pub session: String,
    pub text: String,
    #[serde(default)]
    pub images: Vec<serde_json::Value>,
    pub created_at: String,
}

impl UnsentTurn {
    pub fn new(id: String, session: String, text: String, images: Vec<serde_json::Value>) -> Self {
        Self {
            v: 1,
            id,
            session,
            text,
            images,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// The app-owned unsent ledger at `~/.manox/steer/unsent.jsonl`.
///
/// Failures are logged, never fatal: a send must not be blocked by a disk
/// problem, and the in-memory path still delivers the turn.
pub struct UnsentOutbox {
    path: PathBuf,
}

impl Default for UnsentOutbox {
    fn default() -> Self {
        Self::new()
    }
}

impl UnsentOutbox {
    pub fn new() -> Self {
        // Tests (unit and `test-support` binaries) share one temp file per
        // process: the ledger must never write the developer's real state.
        #[cfg(any(test, feature = "test-support"))]
        let path = std::env::temp_dir().join(format!("steer-unsent-{}.jsonl", std::process::id()));
        #[cfg(not(any(test, feature = "test-support")))]
        let path = manox_agent::paths::manox_config_dir()
            .map(|root| root.join("steer").join("unsent.jsonl"))
            .unwrap_or_else(|_| PathBuf::from("unsent.jsonl"));
        Self { path }
    }

    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Record a turn before its dispatch, so a crash cannot lose the text.
    pub fn push(&self, turn: &UnsentTurn) {
        if let Err(err) = self.append(turn) {
            tracing::warn!(id = %turn.id, error = %err, "unsent turn was not persisted");
        }
    }

    /// Forget a delivered (or deliberately abandoned) turn.
    pub fn remove(&self, id: &str) {
        if let Err(err) = self.rewrite(|turn| turn.id != id) {
            tracing::warn!(id, error = %err, "unsent turn was not removed");
        }
    }

    /// Take every undelivered turn for `session`, leaving the rest in place.
    /// The caller hands them back as drafts; after this call they are no
    /// longer recoverable from the file.
    pub fn take_for_session(&self, session: &str) -> Vec<UnsentTurn> {
        let mut taken = Vec::new();
        if let Err(err) = self.rewrite(|turn| {
            if turn.session == session {
                taken.push(turn.clone());
                false
            } else {
                true
            }
        }) {
            tracing::warn!(session, error = %err, "unsent turns were not read");
        }
        taken
    }

    fn append(&self, turn: &UnsentTurn) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        let line = serde_json::to_string(turn).map_err(std::io::Error::other)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_data()
    }

    fn rewrite(&self, mut keep: impl FnMut(&UnsentTurn) -> bool) -> std::io::Result<()> {
        // A no-op must not touch the disk: `remove` fires on every landed
        // turn's adoption, and most calls find nothing recorded (or a row
        // that retired earlier) — creating or rewriting the file there
        // would spend fsyncs on nothing.
        if !self.path.exists() {
            return Ok(());
        }
        let turns = self.read()?;
        let before = turns.len();
        let kept: Vec<UnsentTurn> = turns.into_iter().filter(|turn| keep(turn)).collect();
        // Nothing matched: the file already holds exactly the kept rows —
        // rewriting it would only spend two fsyncs re-writing the bytes.
        if kept.len() == before {
            return Ok(());
        }
        let mut out = String::new();
        for turn in &kept {
            out.push_str(&serde_json::to_string(turn).map_err(std::io::Error::other)?);
            out.push('\n');
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = File::create(&tmp)?;
            file.write_all(out.as_bytes())?;
            file.sync_data()?;
        }
        fs::rename(&tmp, &self.path)
    }

    fn read(&self) -> std::io::Result<Vec<UnsentTurn>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut turns = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                // A torn tail is a crash artifact, not a reason to lose the
                // rows before it.
                Err(err) => tracing::warn!(error = %err, "skipping a malformed unsent row"),
                Ok(turn) => turns.push(turn),
            }
        }
        Ok(turns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox(dir: &tempfile::TempDir) -> UnsentOutbox {
        UnsentOutbox::at(dir.path().join("steer/unsent.jsonl"))
    }

    #[test]
    fn a_turn_survives_a_new_outbox_and_leaves_when_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let store = outbox(&dir);
        store.push(&UnsentTurn::new(
            "t-1".into(),
            "s-1".into(),
            "hello".into(),
            Vec::new(),
        ));
        store.push(&UnsentTurn::new(
            "t-2".into(),
            "s-2".into(),
            "other".into(),
            Vec::new(),
        ));

        // A fresh instance (the restart) still sees both rows.
        let reopened = outbox(&dir);
        assert_eq!(reopened.take_for_session("s-1").len(), 1);

        // The taken row is gone; the other session's row stays.
        let reopened = outbox(&dir);
        assert!(reopened.take_for_session("s-1").is_empty());
        assert_eq!(reopened.take_for_session("s-2").len(), 1);
    }

    #[test]
    fn remove_forgets_one_turn_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = outbox(&dir);
        store.push(&UnsentTurn::new(
            "t-1".into(),
            "s-1".into(),
            "one".into(),
            Vec::new(),
        ));
        store.push(&UnsentTurn::new(
            "t-2".into(),
            "s-1".into(),
            "two".into(),
            Vec::new(),
        ));
        store.remove("t-1");
        let left = outbox(&dir).take_for_session("s-1");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "t-2");
    }
}
