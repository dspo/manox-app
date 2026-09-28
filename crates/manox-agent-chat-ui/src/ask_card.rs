//! The ask-card snapshot types the message column renders. Built by the
//! workspace (which owns the pending-ask state) and consumed by the message
//! views.

#[derive(Clone, PartialEq)]
pub struct AskCardSnapshot {
    pub id: String,
    pub step: usize,
    pub total: usize,
    pub transition_gen: u64,
    pub question: AskCardQuestion,
    pub selections: Vec<bool>,
    /// Current step's free-text custom answer (the per-question input's live
    /// value), so the card can reflect it and gate the skip affordance.
    pub custom: String,
}

#[derive(Clone, PartialEq)]
pub struct AskCardQuestion {
    pub question: String,
    pub header: String,
    /// Optional markdown support text beneath the question.
    pub detail: String,
    /// Optional specialised-surface intent (`kind` + the `approve` option
    /// label). Empty `kind` means a plain ask.
    pub intent: Option<AskCardIntent>,
    pub multi_select: bool,
    pub options: Vec<AskCardOption>,
}

#[derive(Clone, PartialEq)]
pub struct AskCardIntent {
    pub kind: String,
    pub approve: String,
}

#[derive(Clone, PartialEq)]
pub struct AskCardOption {
    pub label: String,
    pub description: String,
    pub recommended: bool,
}
