//! The pi transcript: ACP `session/update` payloads in, conversation rows out.
//!
//! Live deltas and a `session/load` replay feed the same fold, so a rebuilt
//! transcript and a streamed one agree row for row. There is no second
//! translation layer — tool states stay ACP `ToolCallStatus`, message identity
//! is the ACP id (`messageId`, `toolCallId`, or the settle marker's
//! `_meta.pi.entryId`), and the distro's `_meta.pi` / `_meta.zu` facts ride
//! their entries verbatim.
//!
//! Three identity rules the row list depends on:
//!
//! 1. a live reply drafts under its wire `messageId` and is re-keyed onto pi's
//!    entry id by the settle marker, in place;
//! 2. an optimistic user row (`pending:{n}`) is adopted by the entry chunk
//!    with the same words, wherever a reply sits between them;
//! 3. everything the turn produces is inserted ahead of the optimistic rows
//!    the user queued while it ran — a follow-up reads after the answer it
//!    interrupted, never before it.
//!
//! The reducer is pure and gpui-free so the fold is testable without a view.

use serde_json::Value;

/// The optimistic row's id prefix: minted locally, re-keyed on the real id.
pub const PENDING_PREFIX: &str = "pending:";

/// The distro's `_meta.pi` payload (pi-harness `mapping.ts`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PiMeta {
    /// Wall-clock milliseconds the message was written.
    pub at: Option<i64>,
    pub model: Option<PiModel>,
    /// The pi session entry this chunk belongs to (stable across replay).
    pub entry_id: Option<String>,
    /// The message entry landed in the session file.
    pub settled: bool,
    /// Milliseconds from the first delta to the settled entry.
    pub duration_ms: Option<i64>,
    /// pi's message stop reason.
    pub stop_reason: Option<String>,
    /// pi's provider error message when the turn ended in one.
    pub error: Option<String>,
}

impl PiMeta {
    fn from_json(value: &Value) -> Self {
        Self {
            at: value.get("at").and_then(number),
            model: value.get("model").and_then(PiModel::from_json),
            entry_id: string(value.get("entryId")).map(str::to_owned),
            settled: value
                .get("settled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            duration_ms: value.get("durationMs").and_then(number),
            stop_reason: string(value.get("stopReason")).map(str::to_owned),
            error: string(value.get("error")).map(str::to_owned),
        }
    }
}

/// A model identity as the wire names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiModel {
    pub provider: String,
    pub id: String,
    pub name: Option<String>,
}

impl PiModel {
    pub(crate) fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            provider: string(value.get("provider"))?.to_owned(),
            id: string(value.get("id"))?.to_owned(),
            name: string(value.get("name")).map(str::to_owned),
        })
    }
}

/// The distro's `_meta.zu` payload on a tool end frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolMeta {
    pub duration_ms: Option<i64>,
    /// The bash tool's exit code.
    pub exit_code: Option<i64>,
    pub truncated: Option<bool>,
    pub total_lines: Option<i64>,
    /// Temp file holding the untruncated output.
    pub full_output_path: Option<String>,
    /// The live result's structured details; a replay carries none.
    pub details: Option<Value>,
}

impl ToolMeta {
    fn from_json(value: &Value) -> Self {
        Self {
            duration_ms: value.get("durationMs").and_then(number),
            exit_code: value.get("exitCode").and_then(number),
            truncated: value.get("truncated").and_then(Value::as_bool),
            total_lines: value.get("totalLines").and_then(number),
            full_output_path: string(value.get("fullOutputPath")).map(str::to_owned),
            details: value.get("details").cloned(),
        }
    }
}

/// ACP `ToolCallStatus`, verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    fn from_wire(value: Option<&Value>) -> Option<Self> {
        match string(value)? {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One `ToolCallContent.diff`: the exact replacement an edit made.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffBlock {
    pub path: Option<String>,
    /// `None` covers both the absent and the explicit-null oldText: either
    /// way the file was created by this replacement.
    pub old_text: Option<String>,
    pub new_text: String,
}

/// The user's own message.
#[derive(Clone, Debug, PartialEq)]
pub struct UserEntry {
    pub id: String,
    pub text: String,
    pub pi: Option<PiMeta>,
}

/// An assistant reply: its text, its thinking, and the settle facts.
#[derive(Clone, Debug, PartialEq)]
pub struct AssistantEntry {
    pub id: String,
    pub text: String,
    /// The thinking deltas that preceded the text, folded for the collapse.
    pub thoughts: Option<String>,
    pub pi: Option<PiMeta>,
}

/// One tool call: the card, keyed by `toolCallId`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolEntry {
    pub id: String,
    pub tool_call_id: String,
    pub title: String,
    pub name: Option<String>,
    /// ACP `ToolKind` (read/search/edit/execute/…), verbatim.
    pub tool_kind: Option<String>,
    pub status: ToolStatus,
    pub raw_input: Option<Value>,
    /// The joined text of the result's content blocks (the output lane).
    pub output: String,
    /// The diff blocks the result carried (an edit's changes).
    pub diffs: Vec<DiffBlock>,
    pub locations: Vec<String>,
    /// The end frame's `_meta.zu`; a replay has none (a known gap).
    pub meta: Option<ToolMeta>,
}

/// An extension notice (the `notice` update's transcript lane).
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEntry {
    pub id: String,
    pub severity: Option<String>,
    pub text: String,
}

/// One transcript row.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User(UserEntry),
    Assistant(AssistantEntry),
    Tool(ToolEntry),
    Note(NoteEntry),
}

impl Entry {
    /// The entry's stable identity (the row key the list reconciles by).
    pub fn id(&self) -> &str {
        match self {
            Entry::User(entry) => &entry.id,
            Entry::Assistant(entry) => &entry.id,
            Entry::Tool(entry) => &entry.id,
            Entry::Note(entry) => &entry.id,
        }
    }

    /// The row's plain display text.
    pub fn text(&self) -> &str {
        match self {
            Entry::User(entry) => &entry.text,
            Entry::Assistant(entry) => &entry.text,
            Entry::Tool(entry) => &entry.title,
            Entry::Note(entry) => &entry.text,
        }
    }
}

/// The assistant message that is still streaming.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub id: String,
    pub text: String,
    pub thoughts: Option<String>,
    pub pi: Option<PiMeta>,
}

impl Draft {
    fn entry(&self) -> AssistantEntry {
        AssistantEntry {
            id: self.id.clone(),
            text: self.text.clone(),
            thoughts: self.thoughts.clone(),
            pi: self.pi.clone(),
        }
    }
}

/// One session's transcript.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    entries: Vec<Entry>,
    draft: Option<Draft>,
    /// The live message id the open deltas belong to: a live turn streams
    /// under its own `mN` id while the settle marker carries pi's entry id.
    active_id: Option<String>,
    /// Private id minting for identity-less payloads (notices).
    seq: u64,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    /// The settled rows, in order (the streaming draft is not one of them).
    pub fn settled(&self) -> &[Entry] {
        &self.entries
    }

    /// The open draft, when a reply is still writing.
    pub fn draft(&self) -> Option<&Draft> {
        self.draft.as_ref()
    }

    /// The live message id the open deltas belong to.
    pub fn active_id(&self) -> Option<&str> {
        self.active_id.as_deref()
    }

    /// The rows a list renders: settled entries plus the streaming draft,
    /// placed ahead of any optimistic row the user queued mid-turn.
    pub fn rows(&self) -> Vec<Entry> {
        let Some(draft) = &self.draft else {
            return self.entries.clone();
        };
        let at = self.append_at();
        let mut rows = Vec::with_capacity(self.entries.len() + 1);
        rows.extend_from_slice(&self.entries[..at]);
        rows.push(Entry::Assistant(draft.entry()));
        rows.extend_from_slice(&self.entries[at..]);
        rows
    }

    /// Adds the user's optimistic row and returns its id. The reducer re-keys
    /// it onto the real entry id when the harness echoes the message.
    pub fn push_optimistic_user(&mut self, text: &str) -> String {
        self.seq += 1;
        let id = format!("{PENDING_PREFIX}{}", self.seq);
        self.entries.push(Entry::User(UserEntry {
            id: id.clone(),
            text: text.to_owned(),
            pi: None,
        }));
        id
    }

    /// Folds one ACP `session/update` payload. Unknown updates are the
    /// host's projection (config/mode/usage/commands/banner), not rows.
    pub fn apply(&mut self, update: &Value) {
        match string(update.get("sessionUpdate")) {
            Some("user_message_chunk") => self.reduce_user_chunk(update),
            Some("agent_message_chunk") => self.reduce_agent_chunk(update),
            Some("agent_thought_chunk") => self.reduce_thought_chunk(update),
            Some("tool_call") => self.reduce_tool_call(update),
            Some("tool_call_update") => self.reduce_tool_call_update(update),
            Some("notice") => self.reduce_notice(update),
            _ => {}
        }
    }

    /// Folds a whole replay (or any update list) through the reducer.
    pub fn apply_all(&mut self, updates: &[Value]) {
        for update in updates {
            self.apply(update);
        }
    }

    /// Whether the row id is a locally minted optimistic one.
    pub fn is_pending_id(id: &str) -> bool {
        id.starts_with(PENDING_PREFIX)
    }

    // --- chunks -------------------------------------------------------------

    /// Lands the open draft into `entries` as it stands. Called whenever
    /// another row must precede the reply's settle (a tool card inside the
    /// same pi message, the next user prompt after an interruption); the
    /// settle marker later finds the entry by id and adds the facts it
    /// lacked. `except` spares the draft the incoming chunk is still growing.
    fn flush_draft(&mut self, except: Option<&str>) {
        let Some(draft) = self.draft.as_ref() else {
            return;
        };
        if Some(draft.id.as_str()) == except {
            return;
        }
        let entry = Entry::Assistant(draft.entry());
        let at = self.append_at();
        self.entries.insert(at, entry);
        self.draft = None;
    }

    /// Where a new row lands: before the optimistic rows the user queued
    /// while this turn was still running.
    fn append_at(&self) -> usize {
        let mut index = self.entries.len();
        while index > 0 {
            let entry = &self.entries[index - 1];
            match entry {
                Entry::User(user) if Self::is_pending_id(&user.id) => index -= 1,
                _ => break,
            }
        }
        index
    }

    fn reduce_user_chunk(&mut self, update: &Value) {
        let meta = pi_meta(update);
        let text = text_of(update.get("content"));
        let Some(id) = meta
            .as_ref()
            .and_then(|meta| meta.entry_id.clone())
            .or_else(|| id_of(update.get("messageId")))
        else {
            // Benign shapes land here too (non-text content blocks, harness
            // quirks), so this is diagnosability at debug level, not a warning.
            tracing::debug!("dropping a user chunk without an id");
            return;
        };
        if text.is_empty() {
            tracing::debug!(id = %id, "dropping an empty user chunk");
            return;
        }
        self.flush_draft(None);
        if let Some(Entry::User(user)) = self.entries.last_mut()
            && user.id == id
        {
            // One chunk per user entry: an identical repeat is a duplicate
            // frame, same rule the same-id guard below applies mid-list.
            if user.text != text {
                user.text.push_str(&text);
            }
            return;
        }
        // One chunk per user entry: the same id arriving again (a repeated
        // frame) changes nothing. This runs before the optimistic scan, so a
        // duplicate cannot re-key a same-text queued row onto an id that is
        // already in the transcript.
        if self
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::User(user) if user.id == id))
        {
            return;
        }
        // The optimistic row is ours: when the real chunk arrives with the
        // same words, the row re-keys onto the ACP id in place. It need not
        // be the last row — a message queued during the previous turn already
        // has that turn's reply below it by the time the entry chunk lands.
        for entry in self.entries.iter_mut().rev() {
            if let Entry::User(user) = entry
                && Self::is_pending_id(&user.id)
                && user.text == text
            {
                user.id = id;
                if meta.is_some() {
                    user.pi = meta;
                }
                return;
            }
        }
        // The words can differ: pi expands `/` input before persisting (a
        // skill becomes a `<skill …>` block, a template its body). The oldest
        // pending row is still that message — adopt the entry's authoritative
        // text in place rather than minting a second row the replay will
        // never show.
        let at = self.append_at();
        if let Some(Entry::User(pending)) = self.entries.get_mut(at) {
            pending.id = id;
            pending.text = text;
            if meta.is_some() {
                pending.pi = meta;
            }
            return;
        }
        self.entries
            .insert(at, Entry::User(UserEntry { id, text, pi: meta }));
    }

    fn reduce_agent_chunk(&mut self, update: &Value) {
        let meta = pi_meta(update);
        if meta.as_ref().is_some_and(|meta| meta.settled) {
            self.settle_draft(meta.unwrap_or_default());
            return;
        }
        let text = text_of(update.get("content"));
        if text.is_empty() {
            return;
        }
        let target = meta
            .as_ref()
            .and_then(|meta| meta.entry_id.clone())
            .or_else(|| id_of(update.get("messageId")));
        let Some(target) = target else {
            return;
        };
        let is_replay = meta
            .as_ref()
            .and_then(|meta| meta.entry_id.as_ref())
            .is_some();
        self.flush_draft((!is_replay).then_some(target.as_str()));
        if let Some(Entry::Assistant(last)) = self.entries.last_mut()
            && last.id == target
        {
            last.text.push_str(&text);
            return;
        }
        if is_replay {
            // A replay's chunks are born under their entry's id: they build
            // the entry directly; the marker adds the settle facts.
            let at = self.append_at();
            self.entries.insert(
                at,
                Entry::Assistant(AssistantEntry {
                    id: target,
                    text,
                    thoughts: None,
                    pi: meta,
                }),
            );
            return;
        }
        if let Some(draft) = self.draft.as_mut()
            && draft.id == target
        {
            draft.text.push_str(&text);
            self.active_id = Some(target);
            return;
        }
        self.active_id = Some(target.clone());
        self.draft = Some(Draft {
            id: target,
            text,
            thoughts: None,
            pi: meta,
        });
    }

    fn reduce_thought_chunk(&mut self, update: &Value) {
        let meta = pi_meta(update);
        let text = text_of(update.get("content"));
        if text.is_empty() {
            return;
        }
        let target = meta
            .as_ref()
            .and_then(|meta| meta.entry_id.clone())
            .or_else(|| id_of(update.get("messageId")));
        let Some(target) = target else {
            return;
        };
        let is_replay = meta
            .as_ref()
            .and_then(|meta| meta.entry_id.as_ref())
            .is_some();
        self.flush_draft((!is_replay).then_some(target.as_str()));
        if let Some(Entry::Assistant(last)) = self.entries.last_mut()
            && last.id == target
        {
            last.thoughts
                .get_or_insert_with(String::new)
                .push_str(&text);
            return;
        }
        if is_replay {
            let at = self.append_at();
            self.entries.insert(
                at,
                Entry::Assistant(AssistantEntry {
                    id: target,
                    text: String::new(),
                    thoughts: Some(text),
                    pi: meta,
                }),
            );
            return;
        }
        if let Some(draft) = self.draft.as_mut()
            && draft.id == target
        {
            draft
                .thoughts
                .get_or_insert_with(String::new)
                .push_str(&text);
        } else {
            // Thinking deltas can precede the message's first text delta.
            self.draft = Some(Draft {
                id: target.clone(),
                text: String::new(),
                thoughts: Some(text),
                pi: meta,
            });
        }
        self.active_id = Some(target);
    }

    // --- tool calls ---------------------------------------------------------

    fn reduce_tool_call(&mut self, update: &Value) {
        let Some(tool_call_id) = id_of(update.get("toolCallId")) else {
            return;
        };
        self.flush_draft(None);
        let (output, diffs) = update
            .get("content")
            .and_then(Value::as_array)
            .map(|content| content_of(content))
            .unwrap_or_default();
        let card = ToolEntry {
            id: tool_call_id.clone(),
            tool_call_id: tool_call_id.clone(),
            // Empty when the frame named nothing; the view localizes the
            // fallback at render time, so the reducer stays gpui-free.
            title: string(update.get("title"))
                .or_else(|| string(update.get("name")))
                .unwrap_or_default()
                .to_owned(),
            name: string(update.get("name")).map(str::to_owned),
            tool_kind: string(update.get("kind")).map(str::to_owned),
            status: ToolStatus::from_wire(update.get("status")).unwrap_or(ToolStatus::InProgress),
            raw_input: update.get("rawInput").cloned(),
            output,
            diffs,
            locations: update
                .get("locations")
                .and_then(Value::as_array)
                .map(|locations| {
                    locations
                        .iter()
                        .filter_map(|location| string(location.get("path")).map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            meta: None,
        };
        if let Some(index) = self.find_tool(&tool_call_id) {
            if let Entry::Tool(existing) = &mut self.entries[index] {
                let id = existing.id.clone();
                *existing = ToolEntry { id, ..card };
            }
        } else {
            let at = self.append_at();
            self.entries.insert(at, Entry::Tool(card));
        }
    }

    fn reduce_tool_call_update(&mut self, update: &Value) {
        let Some(tool_call_id) = id_of(update.get("toolCallId")) else {
            return;
        };
        let Some(index) = self.find_tool(&tool_call_id) else {
            // A settlement with no open card (a replayed result whose call
            // was in an unloaded branch): keep the result as its own card.
            let (output, diffs) = update
                .get("content")
                .and_then(Value::as_array)
                .map(|content| content_of(content))
                .unwrap_or_default();
            let card = ToolEntry {
                id: tool_call_id.clone(),
                tool_call_id,
                // Same render-time fallback as `reduce_tool_call`.
                title: string(update.get("title")).unwrap_or_default().to_owned(),
                name: None,
                tool_kind: None,
                status: ToolStatus::from_wire(update.get("status"))
                    .unwrap_or(ToolStatus::Completed),
                raw_input: None,
                output,
                diffs,
                locations: Vec::new(),
                meta: None,
            };
            let at = self.append_at();
            self.entries.insert(at, Entry::Tool(card));
            return;
        };
        let Entry::Tool(card) = &mut self.entries[index] else {
            return;
        };
        if let Some(status) = ToolStatus::from_wire(update.get("status")) {
            card.status = status;
        }
        if let Some(title) = string(update.get("title")) {
            card.title = title.to_owned();
        }
        if let Some(content) = update.get("content").and_then(Value::as_array) {
            // A partial frame's content is a cumulative snapshot: replace,
            // not append, or a streaming output would double every chunk.
            let (output, diffs) = content_of(content);
            card.output = output;
            card.diffs = diffs;
        }
        if let Some(meta) = zu_meta(update) {
            card.meta = Some(meta);
        }
    }

    // --- notices ------------------------------------------------------------

    fn reduce_notice(&mut self, update: &Value) {
        let notice = update.get("notice");
        let title = notice.and_then(|notice| string(notice.get("title")));
        let description = notice.and_then(|notice| string(notice.get("description")));
        let text = [title, description]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" — ");
        if text.is_empty() {
            return;
        }
        self.flush_draft(None);
        self.seq += 1;
        let entry = Entry::Note(NoteEntry {
            id: format!("notice:{}", self.seq),
            severity: notice
                .and_then(|notice| string(notice.get("severity")))
                .map(str::to_owned),
            text,
        });
        let at = self.append_at();
        self.entries.insert(at, entry);
    }

    // --- settle -------------------------------------------------------------

    /// The settle marker: an empty chunk whose `_meta.pi` carries the facts.
    fn settle_draft(&mut self, meta: PiMeta) {
        let entry_id = meta.entry_id.clone().or_else(|| {
            self.draft
                .as_ref()
                .map(|draft| draft.id.clone())
                .or_else(|| self.active_id.clone())
        });
        let Some(entry_id) = entry_id else {
            return;
        };
        self.settle_into(&entry_id, meta);
    }

    /// Folds the streaming draft (or a bare marker) into `entries` for good.
    fn settle_into(&mut self, entry_id: &str, meta: PiMeta) {
        let mut settle_pi = meta;
        settle_pi.settled = true;
        settle_pi.entry_id = Some(entry_id.to_owned());
        let existing = self.entries.iter().position(
            |entry| matches!(entry, Entry::Assistant(assistant) if assistant.id == entry_id),
        );
        if let Some(index) = existing {
            // A replay's entry already exists from its chunks; the marker adds
            // the facts it lacked.
            if let Entry::Assistant(entry) = &mut self.entries[index] {
                let mut pi = entry.pi.clone().unwrap_or_default();
                merge_meta(&mut pi, &settle_pi);
                entry.pi = Some(pi);
            }
            self.active_id = None;
            return;
        }
        if let Some(draft) = self.draft.take() {
            let mut pi = draft.pi.clone().unwrap_or_default();
            merge_meta(&mut pi, &settle_pi);
            let at = self.append_at();
            self.entries.insert(
                at,
                Entry::Assistant(AssistantEntry {
                    id: entry_id.to_owned(),
                    text: draft.text,
                    thoughts: draft.thoughts,
                    pi: Some(pi),
                }),
            );
            self.active_id = None;
            return;
        }
        // A tool card inside the same pi message flushes the live draft under
        // its `mN` id before the settle marker (which carries pi's entry id)
        // arrives. Re-key that row, so the reply keeps its place and no empty
        // second row closes the turn.
        if let Some(live_id) = self.active_id.take() {
            let live = self.entries.iter().position(|entry| {
                matches!(entry, Entry::Assistant(assistant)
                    if assistant.id == live_id && !assistant.pi.as_ref().is_some_and(|pi| pi.settled))
            });
            if let Some(index) = live {
                if let Entry::Assistant(entry) = &mut self.entries[index] {
                    entry.id = entry_id.to_owned();
                    let mut pi = entry.pi.clone().unwrap_or_default();
                    merge_meta(&mut pi, &settle_pi);
                    entry.pi = Some(pi);
                }
                return;
            }
        }
        // No streamed text reached the client (an error entry, a fast settle):
        // the marker itself is the record.
        let at = self.append_at();
        self.entries.insert(
            at,
            Entry::Assistant(AssistantEntry {
                id: entry_id.to_owned(),
                text: String::new(),
                thoughts: None,
                pi: Some(settle_pi),
            }),
        );
        self.active_id = None;
    }

    fn find_tool(&self, tool_call_id: &str) -> Option<usize> {
        self.entries.iter().rposition(
            |entry| matches!(entry, Entry::Tool(tool) if tool.tool_call_id == tool_call_id),
        )
    }
}

/// The distro's `_meta.pi`, when the update carries one.
pub fn pi_meta(update: &Value) -> Option<PiMeta> {
    let meta = update.get("_meta")?.get("pi")?;
    meta.is_object().then(|| PiMeta::from_json(meta))
}

/// The distro's `_meta.zu` (a tool end frame), when it carries one.
pub fn zu_meta(update: &Value) -> Option<ToolMeta> {
    let meta = update.get("_meta")?.get("zu")?;
    meta.is_object().then(|| ToolMeta::from_json(meta))
}

/// Drops the absent fields of `incoming` into `target` (the settle merge).
fn merge_meta(target: &mut PiMeta, incoming: &PiMeta) {
    if incoming.at.is_some() {
        target.at = incoming.at;
    }
    if incoming.model.is_some() {
        target.model = incoming.model.clone();
    }
    if incoming.entry_id.is_some() {
        target.entry_id = incoming.entry_id.clone();
    }
    target.settled |= incoming.settled;
    if incoming.duration_ms.is_some() {
        target.duration_ms = incoming.duration_ms;
    }
    if incoming.stop_reason.is_some() {
        target.stop_reason = incoming.stop_reason.clone();
    }
    if incoming.error.is_some() {
        target.error = incoming.error.clone();
    }
}

/// The displayable text of a content payload: a bare string, a text block,
/// or an array of text blocks.
fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                (string(block.get("type")) == Some("text"))
                    .then(|| string(block.get("text")))
                    .flatten()
            })
            .collect(),
        Some(Value::Object(object)) => {
            if string(object.get("type")) == Some("text") {
                string(object.get("text")).unwrap_or_default().to_owned()
            } else {
                String::new()
            }
        }
        _ => String::new(),
    }
}

/// Splits a frame's `ToolCallContent` into the entry's two lanes.
fn content_of(content: &[Value]) -> (String, Vec<DiffBlock>) {
    let mut output = String::new();
    let mut diffs = Vec::new();
    for block in content {
        match string(block.get("type")) {
            Some("content") => {
                let inner = block.get("content");
                if inner.is_some_and(|inner| string(inner.get("type")) == Some("text")) {
                    output.push_str(
                        string(inner.and_then(|inner| inner.get("text"))).unwrap_or_default(),
                    );
                }
            }
            Some("diff") => {
                let Some(new_text) = string(block.get("newText")) else {
                    continue;
                };
                diffs.push(DiffBlock {
                    path: string(block.get("path")).map(str::to_owned),
                    old_text: string(block.get("oldText")).map(str::to_owned),
                    new_text: new_text.to_owned(),
                });
            }
            _ => {}
        }
    }
    // Tool output often ends with blank lines; they only add dead rows to the
    // card (and are not what the model sees after its own trim).
    while output.ends_with([' ', '\t', '\r', '\n']) {
        output.pop();
    }
    (output, diffs)
}

fn id_of(value: Option<&Value>) -> Option<String> {
    let text = match value? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

fn string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn number(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number.round() as i64))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn state(updates: &[Value]) -> Transcript {
        let mut transcript = Transcript::new();
        transcript.apply_all(updates);
        transcript
    }

    #[test]
    fn keeps_acp_vocabulary_and_identity() {
        let mut transcript = Transcript::new();
        transcript.apply(&json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm"}, "messageId": "m1"}));
        transcript.apply(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "he"}, "messageId": "m1"}));
        transcript.apply(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "llo"}, "messageId": "m1"}));
        assert_eq!(
            transcript.draft().map(|draft| draft.id.as_str()),
            Some("m1")
        );
        assert_eq!(transcript.draft().unwrap().text, "hello");
        assert_eq!(transcript.draft().unwrap().thoughts.as_deref(), Some("hmm"));

        // A tool card opens after the reply's text: the draft lands first, in
        // order, and the settle marker later adds its facts.
        transcript.apply(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "name": "read",
            "title": "read x",
            "kind": "read",
            "status": "in_progress",
        }));
        assert!(transcript.draft().is_none());
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id())
            .collect();
        assert_eq!(ids, ["m1", "t1"]);
        assert!(matches!(
            &transcript.settled()[1],
            Entry::Tool(tool) if tool.status == ToolStatus::InProgress
        ));
        transcript.apply(
            &json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "failed"}),
        );
        assert!(matches!(
            &transcript.settled()[1],
            Entry::Tool(tool) if tool.status == ToolStatus::Failed
        ));

        // Protocol state is not transcript.
        let before = transcript.clone();
        transcript.apply(&json!({"sessionUpdate": "current_mode_update"}));
        assert_eq!(transcript, before);
    }

    #[test]
    fn thoughts_accumulate_and_a_settle_rekeys_a_flushed_reply() {
        let mut transcript = Transcript::new();
        transcript.apply(&json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "first "}, "messageId": "m1"}));
        transcript.apply(&json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "second"}, "messageId": "m1"}));
        assert_eq!(
            transcript.draft().unwrap().thoughts.as_deref(),
            Some("first second")
        );
        transcript.apply(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "name": "bash",
            "title": "bash ls",
            "status": "in_progress",
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id())
            .collect();
        assert_eq!(ids, ["m1", "t1"]);
        transcript.apply(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ""},
            "messageId": "e1",
            "_meta": {"pi": {"settled": true, "entryId": "e1", "at": 3, "durationMs": 700, "stopReason": "end_turn"}},
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id())
            .collect();
        assert_eq!(ids, ["e1", "t1"]);
        match &transcript.settled()[0] {
            Entry::Assistant(reply) => {
                assert_eq!(reply.thoughts.as_deref(), Some("first second"));
                let pi = reply.pi.as_ref().unwrap();
                assert!(pi.settled);
                assert_eq!(pi.duration_ms, Some(700));
            }
            other => panic!("expected an assistant row, got {other:?}"),
        }
    }

    #[test]
    fn the_settle_marker_folds_the_draft_under_the_entry_id() {
        let transcript = state(&[
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hi"}, "messageId": "m1"}),
            json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": ""},
                "messageId": "e1",
                "_meta": {"pi": {"settled": true, "entryId": "e1", "at": 2000, "durationMs": 900, "stopReason": "end_turn"}},
            }),
        ]);
        assert!(transcript.draft().is_none());
        assert_eq!(transcript.settled().len(), 1);
        let Entry::Assistant(reply) = &transcript.settled()[0] else {
            panic!("expected an assistant row");
        };
        assert_eq!(reply.id, "e1");
        assert_eq!(reply.text, "hi");
        let pi = reply.pi.as_ref().unwrap();
        assert!(pi.settled);
        assert_eq!(pi.at, Some(2000));
        assert_eq!(pi.duration_ms, Some(900));
        assert_eq!(pi.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn a_replay_builds_the_same_entries_as_the_live_turn() {
        let live = state(&[
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "he"}, "messageId": "m1", "_meta": {"pi": {"at": 1, "model": {"provider": "p", "id": "m"}}}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "llo"}, "messageId": "m1", "_meta": {"pi": {"at": 1}}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": ""}, "messageId": "e1", "_meta": {"pi": {"settled": true, "entryId": "e1", "at": 2, "durationMs": 900, "stopReason": "end_turn"}}}),
        ]);
        let replay = state(&[
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "he"}, "messageId": "e1", "_meta": {"pi": {"at": 1, "entryId": "e1", "model": {"provider": "p", "id": "m"}}}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "llo"}, "messageId": "e1", "_meta": {"pi": {"at": 1, "entryId": "e1"}}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": ""}, "messageId": "e1", "_meta": {"pi": {"settled": true, "entryId": "e1", "at": 2, "durationMs": 900, "stopReason": "end_turn"}}}),
        ]);
        assert_eq!(live.rows(), replay.rows());
        let rows = replay.rows();
        assert_eq!(rows.len(), 1);
        let Entry::Assistant(reply) = &rows[0] else {
            panic!("expected an assistant row");
        };
        assert_eq!(reply.text, "hello");
        assert_eq!(reply.pi.as_ref().unwrap().duration_ms, Some(900));
    }

    #[test]
    fn a_full_replay_keeps_order_and_the_marker_settles() {
        let transcript = state(&[
            json!({"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "q"}, "messageId": "e0", "_meta": {"pi": {"at": 1, "entryId": "e0"}}}),
            json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "h"}, "messageId": "e1", "_meta": {"pi": {"at": 2, "entryId": "e1"}}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "a"}, "messageId": "e1", "_meta": {"pi": {"at": 2, "entryId": "e1"}}}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "b"}, "messageId": "e1", "_meta": {"pi": {"at": 2, "entryId": "e1"}}}),
            json!({"sessionUpdate": "tool_call", "toolCallId": "t9", "name": "bash", "title": "bash ls", "status": "in_progress"}),
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": ""}, "messageId": "e1", "_meta": {"pi": {"settled": true, "entryId": "e1", "stopReason": "end_turn"}}}),
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t9", "status": "completed"}),
        ]);
        let kinds: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| match entry {
                Entry::User(_) => "user",
                Entry::Assistant(_) => "assistant",
                Entry::Tool(_) => "tool",
                Entry::Note(_) => "note",
            })
            .collect();
        assert_eq!(kinds, ["user", "assistant", "tool"]);
        assert_eq!(transcript.settled()[0].id(), "e0");
        let Entry::Assistant(reply) = &transcript.settled()[1] else {
            panic!("expected an assistant row");
        };
        assert_eq!(reply.text, "ab");
        assert_eq!(reply.thoughts.as_deref(), Some("h"));
        assert!(matches!(
            &transcript.settled()[2],
            Entry::Tool(tool) if tool.status == ToolStatus::Completed
        ));
        assert!(transcript.draft().is_none());
    }

    #[test]
    fn the_optimistic_row_rekeys_in_place() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("count slowly");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "count slowly"},
            "messageId": "e7",
            "_meta": {"pi": {"at": 42, "entryId": "e7"}},
        }));
        assert_eq!(transcript.settled().len(), 1);
        assert_eq!(transcript.settled()[0].id(), "e7");
        let Entry::User(user) = &transcript.settled()[0] else {
            panic!("expected a user row");
        };
        assert_eq!(user.pi.as_ref().unwrap().at, Some(42));
    }

    #[test]
    fn a_follow_up_queued_mid_turn_reads_after_the_reply() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("first");
        // The first message's entry lands, then a reply streams and the user
        // queues a follow-up while it runs.
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "first"},
            "messageId": "e0",
            "_meta": {"pi": {"entryId": "e0"}},
        }));
        transcript.apply(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "answering the first"}, "messageId": "m1"}));
        transcript.push_optimistic_user("follow up");
        let ids: Vec<_> = transcript
            .rows()
            .iter()
            .map(|row| row.id().to_owned())
            .collect();
        assert_eq!(ids, ["e0", "m1", "pending:2"]);
        transcript.apply(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ""},
            "messageId": "e1",
            "_meta": {"pi": {"settled": true, "entryId": "e1", "stopReason": "end_turn"}},
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect();
        assert_eq!(ids, ["e0", "e1", "pending:2"]);
        assert!(transcript.draft().is_none());
        // The queued text's own entry chunk re-keys that row in place, though
        // a reply sits between it and the tail.
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "follow up"},
            "messageId": "e2",
            "_meta": {"pi": {"at": 43, "entryId": "e2"}},
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect();
        assert_eq!(ids, ["e0", "e1", "e2"]);
    }

    #[test]
    fn a_tool_card_inside_a_streaming_reply_lands_ahead_of_a_queued_row() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("first");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "first"},
            "messageId": "e0",
            "_meta": {"pi": {"entryId": "e0"}},
        }));
        transcript.apply(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "thinking"}, "messageId": "m1"}));
        transcript.push_optimistic_user("queued");
        transcript.apply(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "t1",
            "name": "bash",
            "title": "bash ls",
            "status": "in_progress",
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect();
        assert_eq!(ids, ["e0", "m1", "t1", "pending:2"]);
    }

    #[test]
    fn an_expanded_slash_input_adopts_the_entry_text() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("/skill:review");
        let chunk = json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "<skill name=\"review\">…</skill>\ncheck the diff"},
            "messageId": "e9",
            "_meta": {"pi": {"at": 44, "entryId": "e9"}},
        });
        transcript.apply(&chunk);
        assert_eq!(transcript.settled().len(), 1);
        assert_eq!(transcript.settled()[0].id(), "e9");
        assert!(
            transcript.settled()[0]
                .text()
                .contains("<skill name=\"review\">")
        );
        // The same chunk again (a repeated frame) changes nothing — the row
        // sits last here, so the repeat hits the tail branch.
        transcript.apply(&chunk);
        assert_eq!(transcript.settled().len(), 1);
        assert_eq!(
            transcript.settled()[0].text(),
            "<skill name=\"review\">…</skill>\ncheck the diff"
        );
    }

    #[test]
    fn a_repeated_frame_spares_the_queued_rows_behind_it() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("expanded");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "expanded"},
            "messageId": "e9",
            "_meta": {"pi": {"entryId": "e9"}},
        }));
        transcript.push_optimistic_user("queued");
        // The re-keyed row is not last anymore: the duplicate must no-op, not
        // consume the queued row.
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "expanded"},
            "messageId": "e9",
            "_meta": {"pi": {"entryId": "e9"}},
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect();
        assert_eq!(ids, ["e9", "pending:2"]);
    }

    #[test]
    fn a_repeated_frame_does_not_rekey_a_same_text_pending_row_at_the_tail() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("expanded");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "expanded"},
            "messageId": "e9",
            "_meta": {"pi": {"entryId": "e9"}},
        }));
        // The queued row happens to carry the same words as the settled entry
        // and sits last: the duplicate must no-op, not re-key it onto `e9`.
        transcript.push_optimistic_user("expanded");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "expanded"},
            "messageId": "e9",
            "_meta": {"pi": {"entryId": "e9"}},
        }));
        let ids: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| entry.id().to_owned())
            .collect();
        assert_eq!(ids, ["e9", "pending:2"]);
    }

    #[test]
    fn queued_messages_keep_their_order_as_each_entry_lands() {
        let mut transcript = Transcript::new();
        transcript.push_optimistic_user("first");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "first"},
            "messageId": "e0",
            "_meta": {"pi": {"entryId": "e0"}},
        }));
        transcript.push_optimistic_user("/skill:a");
        transcript.push_optimistic_user("/skill:b");
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "expanded a"},
            "messageId": "e1",
            "_meta": {"pi": {"at": 1, "entryId": "e1"}},
        }));
        transcript.apply(&json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "expanded b"},
            "messageId": "e2",
            "_meta": {"pi": {"at": 2, "entryId": "e2"}},
        }));
        let rows: Vec<_> = transcript
            .settled()
            .iter()
            .map(|entry| (entry.id().to_owned(), entry.text().to_owned()))
            .collect();
        assert_eq!(
            rows,
            [
                ("e0".to_owned(), "first".to_owned()),
                ("e1".to_owned(), "expanded a".to_owned()),
                ("e2".to_owned(), "expanded b".to_owned()),
            ]
        );
    }

    #[test]
    fn a_tool_partial_replaces_its_snapshot_and_the_end_adds_stats() {
        let transcript = state(&[
            json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "name": "bash", "title": "bash bun test", "kind": "execute", "status": "in_progress", "rawInput": {"command": "bun test"}}),
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress", "content": [{"type": "content", "content": {"type": "text", "text": "one"}}]}),
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress", "content": [{"type": "content", "content": {"type": "text", "text": "onetwo"}}]}),
        ]);
        assert!(matches!(&transcript.settled()[0], Entry::Tool(tool) if tool.output == "onetwo"));

        let transcript = state(&[
            json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "name": "bash", "title": "bash bun test", "kind": "execute", "status": "in_progress"}),
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "t1",
                "status": "failed",
                "content": [
                    {"type": "content", "content": {"type": "text", "text": "onetwo three\n\n"}},
                    {"type": "diff", "path": "/a.ts", "oldText": "x", "newText": "y"},
                ],
                "_meta": {"zu": {"durationMs": 1800, "exitCode": 1, "truncated": true, "totalLines": 36}},
            }),
        ]);
        let Entry::Tool(tool) = &transcript.settled()[0] else {
            panic!("expected a tool row");
        };
        assert_eq!(tool.status, ToolStatus::Failed);
        assert_eq!(tool.output, "onetwo three");
        assert_eq!(
            tool.diffs,
            [DiffBlock {
                path: Some("/a.ts".to_owned()),
                old_text: Some("x".to_owned()),
                new_text: "y".to_owned(),
            }]
        );
        let meta = tool.meta.as_ref().unwrap();
        assert_eq!(meta.duration_ms, Some(1800));
        assert_eq!(meta.exit_code, Some(1));
        assert_eq!(meta.truncated, Some(true));
        assert_eq!(meta.total_lines, Some(36));
    }

    #[test]
    fn a_provider_error_settles_as_the_entry_error() {
        let transcript = state(&[json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ""},
            "messageId": "e9",
            "_meta": {"pi": {"settled": true, "entryId": "e9", "at": 5, "stopReason": "end_turn", "error": "rate limited"}},
        })]);
        assert_eq!(transcript.settled().len(), 1);
        let Entry::Assistant(reply) = &transcript.settled()[0] else {
            panic!("expected an assistant row");
        };
        assert_eq!(
            reply.pi.as_ref().unwrap().error.as_deref(),
            Some("rate limited")
        );
        assert_eq!(reply.text, "");
    }

    #[test]
    fn notices_mint_their_own_stable_ids() {
        let transcript = state(&[json!({
            "sessionUpdate": "notice",
            "notice": {"severity": "warning", "title": "extension ui", "description": "not supported"},
        })]);
        let Entry::Note(note) = &transcript.settled()[0] else {
            panic!("expected a note row");
        };
        assert_eq!(note.id, "notice:1");
        assert_eq!(note.severity.as_deref(), Some("warning"));
        assert_eq!(note.text, "extension ui — not supported");
    }
}
