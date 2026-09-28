//! The sub-agent observation face: the per-thread child-transcript state the
//! panels render from, plus the right-pane tab that surfaces one panel per
//! sub-agent address. The tab instance is built here and handed to the live
//! chrome pane through the assembly's opener — the shell owns tab placement,
//! the workspace owns the data a panel displays.

use super::*;

impl Workspace {
    /// Open (or focus) a sub-agent observation panel in the right pane. The
    /// tab label is the subagent's address (`id`); the panel's banner shows
    /// `topic` — the shared `subagent_topic` derivation the rail and
    /// conversation rows use.
    pub(crate) fn open_subagent_tab(
        &mut self,
        id: &str,
        subagent_type: &str,
        topic: &str,
        status: manox_agent::ToolCallStatus,
        cx: &mut Context<Self>,
    ) {
        let backfill = self
            .subagent_transcripts
            .get(id)
            .cloned()
            .unwrap_or_default();
        let final_text = if backfill.is_empty() {
            self.agent_final_text(id, cx)
                .or_else(|| self.subagent_final_text.get(id).cloned())
        } else {
            None
        };
        let banner = if topic.is_empty() {
            id.to_string()
        } else {
            topic.to_string()
        };
        let recipient = if subagent_type.is_empty() {
            "sub-agent".to_string()
        } else {
            subagent_type.to_string()
        };
        // Transcript rows name the model that runs the child: the child reports
        // its resolved model at dispatch (`SubagentChildEvent::Model`), and the
        // parent's live label stands in until one is known.
        let role = backfill
            .iter()
            .find_map(|event| match event {
                manox_agent::SubagentChildEvent::Model(model) => Some(model.clone()),
                _ => None,
            })
            .unwrap_or_else(|| self.model_label(cx));
        let prompt = self.subagent_prompts.get(id).cloned();
        let panel = crate::views::subagent_panel::SubagentPanel::new(
            banner,
            role,
            recipient,
            status,
            &backfill,
            prompt.map(|p| (p.text, p.dispatched_at)),
            final_text,
            self.chat.read(cx).host.clone(),
            cx,
        );
        let tab = crate::tool_tabs::subagent_tab(id, panel);
        // The pane open runs on the event path: opening it from inside this
        // entity's update would re-enter the Workspace (the tab's own `open`
        // reaches the workspace store) while it is leased.
        let address = id.to_string();
        cx.defer(move |cx| {
            if !crate::chrome_assembly::open_tool_tab(tab, cx) {
                tracing::debug!(
                    subagent = %address,
                    "subagent panel requested with no live right pane"
                );
            }
        });
    }

    /// Final answer of a finished Agent call, for panels opened after a
    /// reload when no live transcript was accumulated. T10c: reads the v2
    /// display fold (the message rows ARE the transcript).
    pub(super) fn agent_final_text(&self, id: &str, cx: &App) -> Option<String> {
        use manox_agent::language_model::MessageContent;
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|s| s.read(cx).store.derived_messages())
            .expect("foreground store present")
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|c| match c {
                MessageContent::ToolResult(r) if r.tool_use_id == id => Some(r.content.clone()),
                _ => None,
            })
    }

    /// Drop the per-thread sub-agent observation state: the accumulated child
    /// transcripts, and every observation panel mounted in the live pane.
    ///
    /// The panels must go with it. A panel renders the transcript accumulated
    /// up to its open, and the shell's per-thread stash would carry it across
    /// the switch — it would then show the thread's history only as of the
    /// moment it was left, while the catch-up refills `subagent_transcripts`
    /// behind it. Retiring them here keeps "a panel shows the transcript as of
    /// its open" true for the reopened one (the rail row reopens from the
    /// fresh transcript).
    pub(super) fn clear_subagent_observation(&mut self, cx: &mut Context<Self>) {
        self.subagent_transcripts.clear();
        crate::chrome_assembly::close_tool_tabs_of_kind("subagent", cx);
    }

    /// Rebuild the per-thread sub-agent observation state (rail rows + panel
    /// prompt/final-text) from the restored transcript. Live rows are fed by
    /// `SubagentProgress` events, which die with the process; this recovers
    /// the settled rows after a restart or a thread switch-back so the rail
    /// and panels are not left empty. Idempotent: rows upsert by address.
    /// Takes precomputed rows (derived inside the store-read closure) so the
    /// callers never clone the full transcript just for this scan.
    pub(super) fn apply_subagent_rows(
        &mut self,
        rows: Vec<manox_agent::subagent_restore::RestoredSubagent>,
        cx: &mut Context<Self>,
    ) {
        for row in rows {
            let first_line = crate::conversation::first_line(&row.prompt);
            self.subagent_prompts.insert(
                row.address.clone(),
                SubagentPrompt {
                    text: row.prompt.clone(),
                    dispatched_at: row.dispatched_at,
                },
            );
            if let Some(text) = &row.final_text {
                self.subagent_final_text
                    .insert(row.address.clone(), text.clone());
            }
            self.chat_rail(cx).update(cx, |r, cx| {
                r.apply_subagent_progress(
                    &row.address,
                    &row.subagent_type,
                    first_line.as_deref(),
                    row.status,
                    None,
                    cx,
                );
            });
        }
    }
}
