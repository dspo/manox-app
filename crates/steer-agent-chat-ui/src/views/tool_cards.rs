//! The ai-elements tool cards' mount point in the conversation column.
//!
//! The card families (`Read` / `Write` / `Edit` / `Bash` / `BashOutput` /
//! `Grep` / `Glob`) render as `ai_elements::Tool` cards — icon, name, args in
//! the header; an in-band `CodeView` or `ShellView` body; copy chips quoting
//! the raw call; the localized status chip. Everything else keeps the
//! `TerminalPanel` step.
//!
//! Ownership stays where it was: the conversation's `ToolCallItem` remains
//! the authority for status, output, and the fold (`collapsed` /
//! `user_toggled` and its delayed auto-collapse). The card's `ToolState` is a
//! pure mirror — mounted here, re-synced on every render pass, its header
//! click routed back through the conversation's own toggle, and its own
//! auto-fold disabled so the two clocks never race. The one state the card
//! keeps for itself is the copy flash.
//!
//! Body caps: code bodies render at most [`LINE_CAP`] lines and shell bodies
//! at most [`LINE_CAP`] output lines, with a muted truncation note — the
//! `TerminalPanel` paginates, the cards cap, and the full text is always one
//! `out` copy away.

use ai_elements::{
    CodeLine, CodeLineKind, CodeView, ShellLine, ShellSpanKind, ShellView, Tool, ToolState,
    ToolStatus,
};
use gpui::{AnyElement, App, Context, Entity, IntoElement, SharedString, Window, div, prelude::*};
use gpui_component::{Icon, Sizable as _, Theme, h_flex};
use gpui_kit_assets::IconName;
use manox_agent::ToolCallStatus;

/// The card header's click, routed back into the conversation.
pub type CardToggle = Box<dyn Fn(&mut Window, &mut App)>;

use crate::conversation::{ActivityEntry, ConvItem, ToolCallItem};
use crate::host::ChatHostHandle;
use crate::i18n;
use crate::text::one_line;

/// Lines a card body renders before the truncation note takes over.
const LINE_CAP: usize = 400;

/// The tool families that render as ai-elements cards. Everything else stays
/// on the `TerminalPanel` step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardFamily {
    Read,
    Write,
    Edit,
    Bash,
    BashOutput,
    Grep,
    Glob,
}

pub fn card_family(name: &str) -> Option<CardFamily> {
    use manox_agent::tools as wire;
    match name {
        x if x == wire::READ => Some(CardFamily::Read),
        x if x == wire::WRITE => Some(CardFamily::Write),
        x if x == wire::EDIT => Some(CardFamily::Edit),
        x if x == wire::BASH => Some(CardFamily::Bash),
        x if x == wire::BASH_OUTPUT => Some(CardFamily::BashOutput),
        x if x == wire::GREP => Some(CardFamily::Grep),
        x if x == wire::GLOB => Some(CardFamily::Glob),
        _ => None,
    }
}

fn card_status(status: ToolCallStatus) -> ToolStatus {
    match status {
        ToolCallStatus::PendingApproval => ToolStatus::Pending,
        ToolCallStatus::Running | ToolCallStatus::Continued => ToolStatus::Running,
        ToolCallStatus::Success => ToolStatus::Succeeded,
        ToolCallStatus::Error | ToolCallStatus::Denied | ToolCallStatus::Cancelled => {
            ToolStatus::Failed
        }
    }
}

/// Mount (lazily) the card's mirrored `ToolState` and re-sync it from the
/// entry: status first, then the fold — the conversation is the authority, so
/// its mirror lands last. The card's own auto-fold is off; the conversation's
/// delayed collapse drives the fold through this same mirror.
///
/// A write happens only when the mirror actually disagrees with the entry: the
/// caller runs this on every render pass, and an unconditional `set_status` /
/// `set_open` would notify the card (and through its observer the message) on
/// every frame — a repaint loop with no state change behind it.
pub fn ensure_tool_card(
    entry: &mut ToolCallItem,
    subs: &mut Vec<gpui::Subscription>,
    cx: &mut Context<crate::views::message::MessageItem>,
) -> Option<Entity<ToolState>> {
    card_family(&entry.name)?;
    if entry.card.is_none() {
        let state = cx.new(|cx| ToolState::new(cx).no_auto_fold());
        // The copy flash lives in the card's state; a flash must repaint the
        // message, so the mirror is observed for the item's lifetime.
        subs.push(cx.observe(&state, |_, _, cx| cx.notify()));
        entry.card = Some(state);
    }
    let state = entry.card.clone()?;
    let now = cx.background_executor().now();
    if entry.streaming {
        // Render passes are the only clock the card gets after a burst ends:
        // publish the tail the tick or a reveal hold deferred. The rules are
        // unchanged — an unchanged text is a no-op, appends still wait for the
        // tick and for the hold to expire.
        entry.stream.observe(&entry.output, now);
    } else {
        // A settled entry's body is authoritative and static; seed the
        // snapshot here so restored history (which never sees a `ToolResult`
        // event) renders the full output. A no-op once the two agree.
        entry.stream.flush(&entry.output);
    }
    let (status, open) = (
        card_status(entry.status),
        entry.streaming || !entry.collapsed,
    );
    let open_flipped = state.read(cx).is_open() != open;
    state.update(cx, |state, cx| {
        if state.status() != &status {
            state.set_status(status, cx);
        }
        if state.is_open() != open {
            state.set_open(open, cx);
        }
    });
    // The reveal re-lays the body out as root on every animation frame;
    // holding tick-driven publishes across that window keeps the text stable
    // while the layout is in motion (replacements still land).
    if open_flipped {
        entry.stream.hold_reveal(now);
    }
    Some(state)
}

/// One tool call as a card. `on_toggle` routes the header click back into the
/// conversation's own fold (the card's state only mirrors it); `host` feeds
/// the edit family's full-diff entry (the right pane's diff tab).
pub fn build_tool_card(
    entry: &ToolCallItem,
    state: &Entity<ToolState>,
    cwd: Option<SharedString>,
    theme: &Theme,
    on_toggle: Option<CardToggle>,
    host: Option<ChatHostHandle>,
) -> AnyElement {
    let family = card_family(&entry.name).expect("card family");
    let mut card = Tool::new(gpui::ElementId::Name(entry.id.clone().into()), state)
        .icon(card_icon(family))
        .name(entry.name.as_str())
        .args(card_args(family, entry, theme))
        .status(status_chip(entry, theme))
        .content(card_body(family, entry, cwd));
    if let Some(raw_input) = copy_input_text(family, entry) {
        card = card.copy_input(raw_input);
    }
    // The edit family's quotable shape is the diff itself; every other
    // family quotes the raw output.
    if family == CardFamily::Edit {
        card = card.copy_diff(SharedString::from(entry.output.clone()));
    } else {
        card = card.copy_output(SharedString::from(entry.output.clone()));
    }
    // The edit family's full-diff entry: the in-band body caps at LINE_CAP,
    // the right pane's diff tab renders the whole patch. Attached only when
    // the output could actually be a diff — an error text or an empty one
    // has nothing to open.
    if family == CardFamily::Edit && !entry.is_error && !entry.output.is_empty() {
        card = card.open_diff(
            i18n::t("tool-diff-open-in-pane"),
            entry_open_diff(host, entry.id.clone()),
        );
    }
    if let Some(on_toggle) = on_toggle {
        card = card.on_toggle(move |window, cx| on_toggle(window, cx));
    }
    card.into_any_element()
}

/// The text the card's "in" chip quotes. `Bash` quotes the command itself —
/// the shape a user re-runs — and the rest quote the raw call. An input-less
/// call quotes nothing: the chip serializes the structured input, and a
/// missing one would land the literal string `null` on the clipboard.
fn copy_input_text(family: CardFamily, entry: &ToolCallItem) -> Option<SharedString> {
    match family {
        // A Bash call whose command is absent or not a string still has a
        // machine shape worth quoting.
        CardFamily::Bash => input_str(entry, "command").or_else(|| raw_input(entry)),
        _ => raw_input(entry),
    }
}

/// The call's structured input as the machine wrote it. `Value::Null` means
/// no parameters reached the fold; serializing it would put the word `null`
/// on the clipboard, so there is nothing to quote.
fn raw_input(entry: &ToolCallItem) -> Option<SharedString> {
    (!entry.input.is_null()).then(|| SharedString::from(entry.input.to_string()))
}

fn card_icon(family: CardFamily) -> AnyElement {
    let name = match family {
        CardFamily::Read => IconName::FileText,
        CardFamily::Write => IconName::FilePlus,
        CardFamily::Edit => IconName::Pencil,
        CardFamily::Bash => IconName::SquareTerminal,
        CardFamily::BashOutput => IconName::Terminal,
        CardFamily::Grep => IconName::TextSearch,
        CardFamily::Glob => IconName::FileSearch,
    };
    Icon::new(name).xsmall().into_any_element()
}

/// The header's argument summary as it renders: one clipped row of mono
/// text. [`header_summary`] flattens the value first; `.truncate()` only
/// ellipsizes a line that is already one line.
fn card_args(family: CardFamily, entry: &ToolCallItem, theme: &Theme) -> AnyElement {
    let mono = theme.mono_font_family.clone();
    let muted = theme.muted_foreground;
    let summary = header_summary(family, entry);
    gpui::div()
        .min_w_0()
        .truncate()
        .font_family(mono)
        .text_xs()
        .text_color(muted)
        .child(summary)
        .into_any_element()
}

/// The header's argument summary, from the structured input: the file for
/// file tools, the command for `Bash`, the pattern for the search tools —
/// flattened onto one line so a multi-line Bash command cannot ride its
/// literal newlines into the header and stretch the row. The "in" chip
/// ([`copy_input_text`]) still quotes the raw call.
fn header_summary(family: CardFamily, entry: &ToolCallItem) -> String {
    let summary: SharedString = match family {
        CardFamily::Read | CardFamily::Write | CardFamily::Edit => {
            input_str(entry, "path").unwrap_or_else(|| entry.title.clone().into())
        }
        CardFamily::Bash => input_str(entry, "command").unwrap_or_default(),
        CardFamily::BashOutput => input_str(entry, "shell_id").unwrap_or_default(),
        CardFamily::Grep => {
            let pattern = input_str(entry, "pattern").unwrap_or_default();
            match input_str(entry, "path") {
                Some(path) => SharedString::from(format!("{pattern} · {path}")),
                None => pattern,
            }
        }
        CardFamily::Glob => {
            let pattern = input_str(entry, "pattern").unwrap_or_default();
            match input_str(entry, "path") {
                Some(path) => SharedString::from(format!("{pattern} · {path}")),
                None => pattern,
            }
        }
    };
    one_line(&summary)
}

fn input_str(entry: &ToolCallItem, key: &str) -> Option<SharedString> {
    entry
        .input
        .get(key)
        .and_then(|v| v.as_str())
        .map(SharedString::from)
}

/// The localized status chip — the same label set the step rows use, so the
/// card, the strip and the settled ask card speak one language.
pub(crate) fn status_chip(entry: &ToolCallItem, theme: &Theme) -> AnyElement {
    use manox_agent::ToolCallStatus as S;
    let (color, label, glyph) = match entry.status {
        S::PendingApproval => (theme.muted_foreground, i18n::t("status-pending"), None),
        S::Running | S::Continued => (theme.muted_foreground, i18n::t("status-running"), None),
        S::Success => (
            theme.success,
            i18n::t("status-success"),
            Some(IconName::CircleCheck),
        ),
        S::Error => (
            theme.danger,
            i18n::t("status-error"),
            Some(IconName::CircleX),
        ),
        S::Denied => (
            theme.danger,
            i18n::t("status-denied"),
            Some(IconName::CircleX),
        ),
        S::Cancelled => (
            theme.muted_foreground,
            i18n::t("status-cancelled"),
            Some(IconName::CircleX),
        ),
    };
    h_flex()
        .gap_1()
        .text_color(color)
        .children(glyph.map(|g| div().child(Icon::new(g).xsmall())))
        .child(gpui::div().text_xs().child(label))
        .into_any_element()
}

/// The card body per family, from the snapshot the stream published (settle
/// flushes the authoritative output). Bodies cap at [`LINE_CAP`] lines with a
/// truncation note.
///
/// The element id carries the snapshot's generation: a replace (retry, the
/// live-tail head drop, the settle-time truncation) allocates a fresh
/// `TextViewState`, so the parse and the highlight runs are born from the
/// same text instead of a committed document from the previous stream.
fn card_body(family: CardFamily, entry: &ToolCallItem, cwd: Option<SharedString>) -> AnyElement {
    let id: gpui::ElementId =
        gpui::ElementId::Name(format!("{}-body-g{}", entry.id, entry.stream.generation()).into());
    let output = entry.stream.published().to_string();
    match family {
        CardFamily::Read => {
            let (lines, truncated) = cap(parse_read_output(&output), LINE_CAP);
            code_body(id, lines, truncated)
        }
        CardFamily::Write => {
            // The written content rides in the input; the output is a
            // one-line confirmation. On failure the output carries the error
            // and is what there is to show.
            let body = if entry.is_error {
                output
            } else {
                input_str(entry, "content").unwrap_or_default().to_string()
            };
            let (lines, truncated) = cap(
                body.lines()
                    .enumerate()
                    .map(|(ix, line)| CodeLine::plain(ix as u32 + 1, line))
                    .collect(),
                LINE_CAP,
            );
            code_body(id, lines, truncated)
        }
        CardFamily::Edit => {
            let (lines, truncated) = cap(parse_diff(&output), LINE_CAP);
            code_body(id, lines, truncated)
        }
        CardFamily::Bash => shell_body(
            id,
            entry,
            input_str(entry, "command")
                .map(|c| c.to_string())
                .unwrap_or_default(),
            cwd,
            &output,
        ),
        CardFamily::BashOutput => shell_body(
            id,
            entry,
            format!(
                "BashOutput({})",
                input_str(entry, "shell_id").unwrap_or_default()
            ),
            None,
            &output,
        ),
        CardFamily::Grep => {
            let pattern = input_str(entry, "pattern").unwrap_or_default().to_string();
            let path = input_str(entry, "path").unwrap_or_default().to_string();
            let mut shell = ShellView::new(id, format!("grep -n \"{pattern}\" {path}"))
                .context(path_context(cwd));
            for (shown, line) in output.lines().enumerate() {
                if shown >= LINE_CAP {
                    shell = shell.line(ShellLine::dim(i18n::t("tool-output-truncated")));
                    break;
                }
                shell = shell.line(grep_line(line, &pattern));
            }
            finish_shell(shell, entry)
        }
        CardFamily::Glob => {
            let pattern = input_str(entry, "pattern").unwrap_or_default().to_string();
            let path = input_str(entry, "path").unwrap_or_default().to_string();
            let mut shell = ShellView::new(id, format!("find {path} -name \"{pattern}\""))
                .context(path_context(cwd));
            let paths: Vec<&str> = output
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty())
                .collect();
            let truncated = paths.len() > LINE_CAP;
            for line in paths.into_iter().take(LINE_CAP) {
                shell = shell.line(ShellLine::spans(vec![(
                    ShellSpanKind::Path,
                    SharedString::from(line),
                )]));
            }
            if truncated {
                shell = shell.line(ShellLine::dim(i18n::t("tool-output-truncated")));
            }
            finish_shell(shell, entry)
        }
    }
}

/// A `CodeView` body with the truncation note appended when lines were cut.
fn code_body(id: gpui::ElementId, mut lines: Vec<CodeLine>, truncated: bool) -> AnyElement {
    if truncated {
        lines.push(CodeLine {
            old: None,
            new: None,
            kind: CodeLineKind::Gap,
            text: i18n::t("tool-output-truncated"),
        });
    }
    CodeView::new(id, lines).into_any_element()
}

fn shell_body(
    id: gpui::ElementId,
    entry: &ToolCallItem,
    command: String,
    cwd: Option<SharedString>,
    output: &str,
) -> AnyElement {
    let mut shell = ShellView::new(id, command).context(path_context(cwd));
    for (shown, line) in output.lines().enumerate() {
        if shown >= LINE_CAP {
            shell = shell.line(ShellLine::dim(i18n::t("tool-output-truncated")));
            break;
        }
        let kind = if entry.is_error {
            ShellSpanKind::Error
        } else {
            ShellSpanKind::Plain
        };
        shell = shell.line(ShellLine::spans(vec![(kind, SharedString::from(line))]));
    }
    finish_shell(shell, entry)
}

/// The exit footer: success is exit 0; failures carry no invented code — the
/// cross glyph alone says it; a streaming or still-open call runs.
fn finish_shell(shell: ShellView, entry: &ToolCallItem) -> AnyElement {
    let shell = if entry.streaming {
        shell.running()
    } else {
        match entry.status {
            ToolCallStatus::Success => shell.exit(0),
            ToolCallStatus::Error => shell.exit(1),
            _ => shell,
        }
    };
    shell.into_any_element()
}

/// The zsh prompt's working-directory line — the path alone for now; the git
/// segment needs the workdir probe and lands with it.
fn path_context(cwd: Option<SharedString>) -> ShellLine {
    ShellLine::spans(vec![(ShellSpanKind::Path, cwd.unwrap_or_default())])
}

/// One grep output line: `path:line:content` shapes locate and highlight;
/// anything else (summaries, truncation notices) renders plain.
fn grep_line(line: &str, pattern: &str) -> ShellLine {
    let mut parts = line.splitn(3, ':');
    if let (Some(path), Some(lineno), Some(content)) = (parts.next(), parts.next(), parts.next())
        && let Ok(n) = lineno.trim().parse::<u32>()
        && !path.is_empty()
    {
        ShellLine::hit(path, n, content.trim_start(), pattern)
    } else {
        ShellLine::plain(line)
    }
}

/// The read tool's model-facing shape back to editor lines: the `[path#TAG]`
/// header drops, `N:` prefixes become the gutter, bare `...` marks a gap.
fn parse_read_output(output: &str) -> Vec<CodeLine> {
    let mut lines = Vec::new();
    for line in output.lines() {
        if lines.is_empty() && line.starts_with('[') && line.ends_with(']') {
            continue;
        }
        if line == "..." {
            lines.push(CodeLine::gap());
            continue;
        }
        // The harness prefixes every row `N:TEXT` with no space after the
        // colon (`manox-harness` hashline), so the split is on the first
        // colon and the digits parse or the row is gutter-less — the
        // runtime's bracketed notes (`[no snapshot: …]`) must never print a
        // gutter number.
        if let Some((n, text)) = line.split_once(':')
            && let Ok(n) = n.parse::<u32>()
        {
            lines.push(CodeLine::plain(n, text));
        } else {
            lines.push(gutterless(line));
        }
    }
    lines
}

/// A row that carries no gutter number: runtime notes, foreign shapes, the
/// truncated tail.
fn gutterless(line: &str) -> CodeLine {
    CodeLine {
        old: None,
        new: None,
        kind: CodeLineKind::Context,
        text: SharedString::from(line),
    }
}

/// A unified diff into editor lines: `@@` hunks carry both counters, ` ` /
/// `-` / `+` walk them, and the `---` / `+++` headers drop.
fn parse_diff(output: &str) -> Vec<CodeLine> {
    let mut lines = Vec::new();
    let (mut old, mut new) = (None, None);
    for line in output.lines() {
        if lines.is_empty() && line.starts_with('[') && line.ends_with(']') {
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        if line.starts_with("@@") {
            old = hunk_start(line, '-');
            new = hunk_start(line, '+');
            continue;
        }
        match line.as_bytes().first() {
            Some(b' ') => {
                // Context lines carry both counters — after edits the old and
                // new numbers have drifted apart.
                lines.push(CodeLine {
                    old,
                    new,
                    kind: CodeLineKind::Context,
                    text: SharedString::from(&line[1..]),
                });
                old = old.map(|o| o + 1);
                new = new.map(|n| n + 1);
            }
            Some(b'-') => {
                if let Some(o) = old {
                    lines.push(CodeLine::removed(o, &line[1..]));
                }
                old = old.map(|o| o + 1);
            }
            Some(b'+') => {
                if let Some(n) = new {
                    lines.push(CodeLine::added(n, &line[1..]));
                }
                new = new.map(|n| n + 1);
            }
            _ => lines.push(CodeLine {
                old: None,
                new: None,
                kind: CodeLineKind::Context,
                text: SharedString::from(line),
            }),
        }
    }
    lines
}

/// The `-a,b` / `+c,d` side of a `@@` hunk header, as its starting line.
fn hunk_start(hunk: &str, side: char) -> Option<u32> {
    let at = hunk.find(side)? + 1;
    let rest = &hunk[at..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Cap a line vector at `cap`, reporting whether anything was cut.
fn cap(mut lines: Vec<CodeLine>, limit: usize) -> (Vec<CodeLine>, bool) {
    if lines.len() <= limit {
        return (lines, false);
    }
    lines.truncate(limit);
    (lines, true)
}

/// The card header's toggle routed back into the conversation — the same
/// body the step rows' disclosure runs, because the conversation's
/// `collapsed` / `user_toggled` remains the fold's authority and the card's
/// `ToolState` only mirrors it.
pub fn entry_toggle(
    host: Option<ChatHostHandle>,
    tool_id: String,
) -> impl Fn(&mut Window, &mut App) + Clone {
    move |_window, cx| {
        let Some(host) = host.clone() else {
            return;
        };
        let Some(conv) = host.conversation(cx) else {
            return;
        };
        conv.update(cx, |c, cx| {
            let id = tool_id.clone();
            if let Some((cix, eix)) = c.find_thinking_entry(&id, &*cx)
                && let Some(item) = c.items().get(cix)
            {
                item.update(cx, |item, cx| {
                    if let ConvItem::Thinking(t) = item.kind_mut()
                        && let Some(ActivityEntry::Tool(entry)) = t.entries.get_mut(eix)
                    {
                        entry.collapsed = !entry.collapsed;
                        entry.user_toggled = true;
                    }
                    cx.notify();
                });
            }
            cx.notify();
        });
    }
}

/// The edit card's full-diff entry: the click re-reads the entry by id (the
/// copy chips' build-time capture would freeze the patch of one render pass;
/// patches grow while a call streams) and hands the patch plus the target
/// path to the host — the workspace owns where it opens (the right pane's
/// diff tab).
fn entry_open_diff(
    host: Option<ChatHostHandle>,
    tool_id: String,
) -> impl Fn(&mut Window, &mut App) + Clone {
    move |_window, cx| {
        let Some(host) = host.clone() else {
            return;
        };
        let Some(conv) = host.conversation(cx) else {
            return;
        };
        let payload = conv.update(cx, |c, cx| {
            let (cix, eix) = c.find_thinking_entry(&tool_id, &*cx)?;
            let item = c.items().get(cix)?.clone();
            item.update(cx, |item, _| {
                let ConvItem::Thinking(t) = item.kind_mut() else {
                    return None;
                };
                match t.entries.get_mut(eix) {
                    Some(ActivityEntry::Tool(entry)) => {
                        let path = entry
                            .input
                            .get("path")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        Some((path, entry.output.clone()))
                    }
                    _ => None,
                }
            })
        });
        if let Some((path, patch)) = payload {
            host.open_right_diff(path, patch, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, input: serde_json::Value) -> ToolCallItem {
        ToolCallItem {
            id: "t-1".into(),
            name: name.into(),
            title: String::new(),
            status: ToolCallStatus::Success,
            output: String::new(),
            is_error: false,
            input,
            streaming: false,
            collapsed: false,
            user_toggled: false,
            panel: None,
            card: None,
            cwd: None,
            stream: Default::default(),
        }
    }

    /// The "in" chip quotes the command for `Bash` (the shape a user re-runs),
    /// the raw call for the rest, and nothing for an input-less call: the
    /// structured input serializes, and a missing one serialized to the
    /// literal `null` on the clipboard.
    #[test]
    fn the_in_chip_quotes_the_command_and_never_the_word_null() {
        let bash = entry("Bash", serde_json::json!({"command": "cargo test"}));
        assert_eq!(
            copy_input_text(CardFamily::Bash, &bash).as_deref(),
            Some("cargo test")
        );
        let bare = entry("Bash", serde_json::Value::Null);
        assert_eq!(copy_input_text(CardFamily::Bash, &bare), None);
        let commandless = entry("Bash", serde_json::json!({"description": "run"}));
        assert_eq!(
            copy_input_text(CardFamily::Bash, &commandless).as_deref(),
            Some(r#"{"description":"run"}"#),
            "a Bash call without a command still quotes its machine shape"
        );
        let read = entry("Read", serde_json::json!({"path": "src/lib.rs"}));
        assert_eq!(
            copy_input_text(CardFamily::Read, &read).as_deref(),
            Some(r#"{"path":"src/lib.rs"}"#)
        );
        let bare = entry("Read", serde_json::Value::Null);
        assert_eq!(copy_input_text(CardFamily::Read, &bare), None);
    }

    /// The harness's real read envelope (`manox-harness` hashline): a
    /// `[path#TAG]` header, `N:TEXT` rows with NO space after the colon, and
    /// bare `...` between disjoint windows. Column-0 rows (`use …`,
    /// `fn main`), blank rows, and indented rows must all land their own
    /// gutter number.
    #[test]
    fn read_envelope_restores_the_files_own_numbers() {
        let out =
            "[src/lib.rs#T4K2]\n1:use std::fmt;\n2:\n3:    let x: u32 = 1;\n...\n10:fn main() {}\n";
        let lines = parse_read_output(out);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0].new, Some(1));
        assert_eq!(lines[0].text, "use std::fmt;");
        assert_eq!(lines[1].new, Some(2));
        assert_eq!(lines[1].text, "");
        assert_eq!(lines[2].new, Some(3));
        assert_eq!(lines[2].text, "    let x: u32 = 1;");
        assert_eq!(lines[3].kind, CodeLineKind::Gap);
        assert_eq!(lines[4].new, Some(10));
        assert_eq!(lines[4].text, "fn main() {}");
    }

    /// The runtime's bracketed notes are not numbered rows: a gutter-less
    /// context line, never a `0` in the gutter.
    #[test]
    fn read_runtime_notes_render_gutterless() {
        let out = "[src/lib.rs#T4K2]\n1:a\n[no snapshot: file changed since read]\n[read: output truncated]\n";
        let lines = parse_read_output(out);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].new, None);
        assert_eq!(lines[1].text, "[no snapshot: file changed since read]");
        assert_eq!(lines[2].new, None);
    }

    /// The edit output rides behind the same `[path#TAG]` header; the diff
    /// body walks both counters, context rows carrying both numbers.
    #[test]
    fn diff_envelope_strips_header_and_walks_counters() {
        let out = "[src/lib.rs#T4K2]\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -3,2 +3,3 @@\n ctx\n-old\n+new\n+added\n";
        let lines = parse_diff(out);
        assert_eq!(lines.len(), 4);
        assert_eq!((lines[0].old, lines[0].new), (Some(3), Some(3)));
        assert_eq!(lines[1].kind, CodeLineKind::Removed);
        assert_eq!(lines[1].old, Some(4));
        assert_eq!(lines[2].kind, CodeLineKind::Added);
        assert_eq!(lines[2].new, Some(4));
        assert_eq!(lines[3].new, Some(5));
    }

    /// The header summary flattens a multi-line Bash command to one
    /// single-spaced row — hard `\n`, `\r\n`, blank and indented rows all
    /// normalize — while the "in" chip keeps the raw command for re-running.
    #[test]
    fn bash_header_summary_flattens_but_the_copy_stays_raw() {
        let command = "echo first\n\r\n  echo second\r\n\techo third";
        let bash = entry("Bash", serde_json::json!({ "command": command }));
        let summary = header_summary(CardFamily::Bash, &bash);
        assert_eq!(summary, "echo first echo second echo third");
        assert!(
            !summary.contains(['\n', '\r']),
            "the header renders a single line"
        );
        assert_eq!(
            copy_input_text(CardFamily::Bash, &bash).as_deref(),
            Some(command),
            "the quoted command stays verbatim so a user can re-run it"
        );
    }

    /// A lone carriage return is a break too: `str::lines` alone leaves it
    /// inside the row, where it can still split the header.
    #[test]
    fn bash_header_summary_flattens_a_lone_carriage_return() {
        let bash = entry(
            "Bash",
            serde_json::json!({ "command": "echo first\recho second" }),
        );
        assert_eq!(
            header_summary(CardFamily::Bash, &bash),
            "echo first echo second"
        );
    }

    /// A single-line command passes through the summary untouched.
    #[test]
    fn single_line_header_summary_passes_through() {
        let bash = entry(
            "Bash",
            serde_json::json!({ "command": "cargo test --all --features test-support" }),
        );
        assert_eq!(
            header_summary(CardFamily::Bash, &bash),
            "cargo test --all --features test-support"
        );
    }

    /// The search families keep their `pattern · path` shape after flattening.
    #[test]
    fn grep_header_summary_joins_pattern_and_path() {
        let grep = entry(
            "Grep",
            serde_json::json!({ "pattern": "fn main", "path": "src/main.rs" }),
        );
        assert_eq!(
            header_summary(CardFamily::Grep, &grep),
            "fn main · src/main.rs"
        );
    }

    /// A file card with no `path` falls back to its title, flattened too.
    #[test]
    fn read_header_summary_flattens_the_title_fallback() {
        let mut read = entry("Read", serde_json::json!({}));
        read.title = "line one\nline two".into();
        assert_eq!(header_summary(CardFamily::Read, &read), "line one line two");
    }
}
