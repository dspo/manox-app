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
pub fn ensure_tool_card(
    entry: &mut ToolCallItem,
    subs: &mut Vec<gpui::Subscription>,
    cx: &mut Context<crate::views::message::MessageItem>,
) -> Option<Entity<ToolState>> {
    let family = card_family(&entry.name)?;
    let _ = family;
    if entry.card.is_none() {
        let state = cx.new(|cx| ToolState::new(cx).no_auto_fold());
        // The copy flash lives in the card's state; a flash must repaint the
        // message, so the mirror is observed for the item's lifetime.
        subs.push(cx.observe(&state, |_, _, cx| cx.notify()));
        entry.card = Some(state);
    }
    let state = entry.card.clone()?;
    let (status, open) = (
        card_status(entry.status),
        entry.streaming || !entry.collapsed,
    );
    state.update(cx, |state, cx| {
        state.set_status(status, cx);
        state.set_open(open, cx);
    });
    Some(state)
}

/// One tool call as a card. `on_toggle` routes the header click back into the
/// conversation's own fold (the card's state only mirrors it).
pub fn build_tool_card(
    entry: &ToolCallItem,
    state: &Entity<ToolState>,
    cwd: Option<SharedString>,
    theme: &Theme,
    on_toggle: Option<CardToggle>,
    cx: &App,
) -> AnyElement {
    let family = card_family(&entry.name).expect("card family");
    let mut card = Tool::new(gpui::ElementId::Name(entry.id.clone().into()), state)
        .icon(card_icon(family))
        .name(entry.name.as_str())
        .args(card_args(family, entry, theme))
        .status(status_chip(entry, theme))
        .copy_input(SharedString::from(entry.input.to_string()))
        .content(card_body(family, entry, cwd, cx));
    // The edit family's quotable shape is the diff itself; every other
    // family quotes the raw output.
    if family == CardFamily::Edit {
        card = card.copy_diff(SharedString::from(entry.output.clone()));
    } else {
        card = card.copy_output(SharedString::from(entry.output.clone()));
    }
    if let Some(on_toggle) = on_toggle {
        card = card.on_toggle(move |window, cx| on_toggle(window, cx));
    }
    card.into_any_element()
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

/// The header's argument summary, from the structured input: the file for
/// file tools, the command for `Bash`, the pattern for the search tools.
fn card_args(family: CardFamily, entry: &ToolCallItem, theme: &Theme) -> AnyElement {
    let mono = theme.mono_font_family.clone();
    let muted = theme.muted_foreground;
    let summary: SharedString = match family {
        CardFamily::Read | CardFamily::Write | CardFamily::Edit => {
            input_str(entry, "path").unwrap_or_else(|| entry.title.clone().into())
        }
        CardFamily::Bash => input_str(entry, "command").unwrap_or_default(),
        CardFamily::BashOutput => {
            let shell = input_str(entry, "shell_id").unwrap_or_default();
            SharedString::from(format!("{shell}"))
        }
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
    gpui::div()
        .min_w_0()
        .overflow_hidden()
        .font_family(mono)
        .text_xs()
        .text_color(muted)
        .child(summary)
        .into_any_element()
}

fn input_str(entry: &ToolCallItem, key: &str) -> Option<SharedString> {
    entry
        .input
        .get(key)
        .and_then(|v| v.as_str())
        .map(SharedString::from)
}

/// The localized status chip — the same label set the step rows use, so the
/// card and the strip speak one language.
fn status_chip(entry: &ToolCallItem, theme: &Theme) -> AnyElement {
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

/// The card body per family. Streaming bodies read the live tail; settled
/// bodies cap at [`LINE_CAP`] lines with a truncation note.
fn card_body(
    family: CardFamily,
    entry: &ToolCallItem,
    cwd: Option<SharedString>,
    _cx: &App,
) -> AnyElement {
    let id: gpui::ElementId = gpui::ElementId::Name(format!("{}-body", entry.id).into());
    let output = if entry.streaming {
        crate::views::message::live_tail(&entry.output)
    } else {
        entry.output.clone()
    };
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
            let mut paths = output.lines().map(str::trim_end).filter(|l| !l.is_empty());
            let (count, truncated) = {
                let mut count = 0usize;
                let mut truncated = false;
                for line in paths.by_ref() {
                    if count >= LINE_CAP {
                        truncated = true;
                        break;
                    }
                    shell = shell.line(ShellLine::spans(vec![(
                        ShellSpanKind::Path,
                        SharedString::from(line),
                    )]));
                    count += 1;
                }
                (count, truncated)
            };
            let _ = count;
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
        } else if let Some((n, text)) = line.split_once(": ") {
            if let Ok(n) = n.trim().parse::<u32>() {
                lines.push(CodeLine::plain(n, text));
                continue;
            }
            lines.push(CodeLine::plain(0, line));
        } else {
            lines.push(CodeLine {
                old: None,
                new: None,
                kind: CodeLineKind::Context,
                text: SharedString::from(line),
            });
        }
    }
    lines
}

/// A unified diff into editor lines: `@@` hunks carry both counters, ` ` /
/// `-` / `+` walk them, and the `---` / `+++` headers drop.
fn parse_diff(output: &str) -> Vec<CodeLine> {
    let mut lines = Vec::new();
    let (mut old, mut new) = (None, None);
    for line in output.lines() {
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
