//! The ask-card types the message column renders. Built by the workspace
//! (which owns the pending-ask state) and consumed by the message views.
//!
//! The snapshot is the ask's *content* — every question, in page order. The
//! interactive half (which page is up, what is selected, what each note says)
//! lives in the `ai_elements::AskState` entity the workspace owns beside the
//! snapshot, so a card that reports its answers back reports indices and note
//! text while the label→wire fold stays at the one place that mints the
//! questions.

/// The pending ask as its card renders it.
#[derive(Clone, PartialEq)]
pub struct AskCardSnapshot {
    pub id: String,
    /// Every question of the ask, in page order.
    pub questions: Vec<AskCardQuestion>,
}

/// One question's answer as the card reports it back: the chosen options by
/// index (positional against this question's own options) plus the note the
/// user typed. An empty selection with an empty note is the question the user
/// never touched — the host settles it as an explicit skip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AskCardReply {
    pub selected: Vec<usize>,
    pub note: String,
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

/// Read the card's interactive state back into replies, in question order —
/// the shape both submit paths (the pager's check and the composer's Enter)
/// hand the host.
pub fn replies_of(
    state: &gpui::Entity<ai_elements::AskState>,
    cx: &gpui::App,
) -> Vec<AskCardReply> {
    state
        .read(cx)
        .answers(cx)
        .into_iter()
        .map(|(selected, note)| AskCardReply { selected, note })
        .collect()
}
