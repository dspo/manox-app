//! Shared sub-agent observation data.
//!
//! The rail's agents section renders these as observe-only rows; the retired
//! manox harness additionally drilled into a per-child panel, which the pi
//! harness has no child-thread entities for (the panel was retired with the
//! manox harness).

use gpui::AnyElement;
use gpui::prelude::*;
use gpui_component::{Icon, IconName, Sizable as _, Theme};
use manox_agent::ToolCallStatus;

use ai_elements::BrailleSpinner;

/// The main agent's literal label. App chrome is localized, but this name is
/// product vocabulary rendered verbatim in every language — style it through
/// this constant so no surface can drift into a translation.
pub const CAPTAIN_LABEL: &str = "Captain";

#[derive(Clone, Debug)]
pub struct SubagentInfo {
    pub id: String,
    pub subagent_type: String,
    pub description: String,
    pub status: ToolCallStatus,
    /// The watchdog's one-line health verdict (working / tool running /
    /// stalled / looping) while the run is live; `None` for settled rows.
    pub health: Option<String>,
}

pub fn subagent_display_title(info: &SubagentInfo) -> String {
    task_display_title(&info.subagent_type, &info.description).unwrap_or_default()
}

/// `{type}({topic})` with graceful one-sided fallbacks; `None` when both
/// sides are empty (callers pick their own last resort, e.g. the call id).
pub fn task_display_title(subagent_type: &str, description: &str) -> Option<String> {
    if description.is_empty() {
        (!subagent_type.is_empty()).then(|| subagent_type.to_string())
    } else if subagent_type.is_empty() {
        Some(description.to_string())
    } else {
        Some(format!("{subagent_type}({description})"))
    }
}

pub fn status_indicator(status: ToolCallStatus, theme: &Theme) -> AnyElement {
    match status {
        ToolCallStatus::PendingApproval | ToolCallStatus::Running => BrailleSpinner::new()
            .xsmall()
            .color(theme.accent_foreground)
            .into_any_element(),
        ToolCallStatus::Success | ToolCallStatus::Continued => Icon::default()
            .path("icons/circle-check-big.svg")
            .xsmall()
            .text_color(theme.success)
            .into_any_element(),
        ToolCallStatus::Error | ToolCallStatus::Denied => Icon::new(IconName::CircleX)
            .xsmall()
            .text_color(theme.danger)
            .into_any_element(),
        ToolCallStatus::Cancelled => Icon::new(IconName::Minus)
            .xsmall()
            .text_color(theme.muted_foreground)
            .into_any_element(),
    }
}

/// The Claude subagent display-color vocabulary mapped to fixed RGB. Runtime
/// values render verbatim: a name outside the eight-color set is not an
/// error — the caller falls back to its neutral treatment.
pub fn agent_color(name: &str) -> Option<gpui::Hsla> {
    let rgb: u32 = match name {
        "red" => 0xE5484D,
        "blue" => 0x3E63DD,
        "green" => 0x30A46C,
        "yellow" => 0xFFB224,
        "purple" => 0x8E4EC6,
        "orange" => 0xF76B15,
        "pink" => 0xD6409F,
        "cyan" => 0x00A2C7,
        _ => return None,
    };
    Some(gpui::rgb(rgb).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_color_maps_the_eight_color_vocabulary_and_degrades_to_none() {
        assert_eq!(
            agent_color("pink").map(|c| c == gpui::rgb(0xD6409F).into()),
            Some(true)
        );
        assert!(agent_color("cyan").is_some());
        assert!(
            agent_color("magenta").is_none() && agent_color("").is_none(),
            "a name outside the vocabulary is uncolored, never an error"
        );
    }

    #[test]
    fn task_display_title_combines_type_and_topic() {
        assert_eq!(
            task_display_title("Explore", "find the auth module").as_deref(),
            Some("Explore(find the auth module)")
        );
        assert_eq!(
            task_display_title("Explore", "").as_deref(),
            Some("Explore")
        );
        assert_eq!(
            task_display_title("", "topic only").as_deref(),
            Some("topic only")
        );
        assert_eq!(task_display_title("", ""), None);
    }
}
