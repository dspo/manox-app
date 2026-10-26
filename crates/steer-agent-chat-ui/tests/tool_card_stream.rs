//! Regression tests for the 2026-10-26 tool-card abort.
//!
//! Production aborted (`SIGABRT`) in the layout phase inside
//! `StyledText::with_runs` ("invalid text run") beneath
//! `ShellView` → `Tool` → `Collapsible`/`MotionReveal` → `MessageItem`,
//! while a Bash card's output streamed under its open reveal. The exact
//! trigger could not be pinned offscreen (see
//! `.agents/skills/gpui-kit-follow/references/upstream-bugs.md`); these tests
//! instead guard the structural exposure end to end: the production
//! `ConversationState` -> `MessageItem` -> ai-elements card assembly, driven
//! frame by frame through the open reveal with text that changes
//! discontinuously (stream batching, the live-tail head drop past 24 KiB, a
//! settle-time truncation past `LINE_CAP`, and a retry rewrite).
//!
//! Offscreen animation frames only arrive through `Window::simulate_next_frame`
//! (AGENTS.md "出 UI 图"); the fake scheduler clock is advanced between chunks
//! so the 100 ms publish tick actually fires while the reveal runs.

use std::time::Duration;

use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use manox_agent::{MessageAuthor, ThreadEvent, ToolCallStatus};
use steer_agent_chat_ui::conversation::{
    ActivityEntry, ApplyCtx, ConvItem, ConversationState, ToolCallItem,
};
use steer_agent_chat_ui::host::noop_host;
use steer_agent_chat_ui::views::message::MessageItem;
use steer_agent_chat_ui::views::tool_stream::PUBLISH_CAP;

/// The recorded output of the crashed session's Bash call
/// (`toolu_d255c1cb2f9f4c44b1b2ff9a`, `2026-10-26T06:11:42.336Z`), byte for
/// byte as the journal streamed it: dense CJK plus escaped JSON plus backtick
/// runs.
const CRASH_BASH_OUTPUT: &str = include_str!("fixtures/crash_bash_output.txt");

/// The message column that mounts the production rows.
struct Mount {
    rows: Vec<Entity<MessageItem>>,
}

impl Render for Mount {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().min_w_0();
        for row in &self.rows {
            root = root.child(row.clone());
        }
        root
    }
}

fn pump(visual: &mut VisualTestContext, frames: usize) {
    for _ in 0..frames {
        visual.update(|window, cx| {
            window.simulate_next_frame(cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }
}

fn apply(cx: &mut TestAppContext, conversation: &Entity<ConversationState>, event: ThreadEvent) {
    let ctx = ApplyCtx {
        host: noop_host(),
        cwd: None,
        fork_source: None,
    };
    cx.update(|cx| {
        conversation.update(cx, |conversation, cx| {
            let _ = conversation.apply(&event, "test-model", None, ctx, cx);
        });
    });
}

fn tool_call(id: &str, name: &str, command: &str) -> ThreadEvent {
    ThreadEvent::ToolCall {
        id: id.into(),
        name: name.into(),
        title: format!("{name} {command}"),
        status: ToolCallStatus::Running,
        input: Some(serde_json::json!({ "command": command })),
    }
}

/// Mount a fresh conversation with one running Bash card and open its window.
fn mount_bash_card(
    cx: &mut TestAppContext,
    id: &str,
) -> (Entity<ConversationState>, VisualTestContext) {
    cx.update(|cx| {
        // The content battery mounts several cards in one test; `init` is
        // only safe to run once per app.
        if !cx.has_global::<gpui_component::Theme>() {
            gpui_component::init(cx);
        }
    });
    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    apply(cx, &conversation, tool_call(id, "Bash", "rg -n 中文 ."));
    let rows = cx.update(|cx| conversation.read(cx).items().to_vec());
    assert_eq!(rows.len(), 1, "the tool call folds into one activity row");

    let window = cx.open_window(size(px(760.), px(720.)), move |_, _| Mount { rows });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    // Let the card mount and start its open reveal.
    pump(&mut visual, 2);
    (conversation, visual)
}

/// Byte chunks that never split a character.
fn chunks(text: &str, size: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + size).min(text.len());
        while end < text.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        out.push(text[start..end].to_string());
        start = end;
    }
    out
}

/// Stream `text` as live output chunks while pumping frames.
fn stream(
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
    conversation: &Entity<ConversationState>,
    id: &str,
    text: &str,
) {
    for chunk in chunks(text, 700) {
        apply(
            cx,
            conversation,
            ThreadEvent::ToolOutput {
                id: id.into(),
                chunk,
            },
        );
        // The publish tick is driven by the (fake) scheduler clock.
        cx.executor().advance_clock(Duration::from_millis(120));
        pump(visual, 2);
    }
}

fn settle(
    cx: &mut TestAppContext,
    visual: &mut VisualTestContext,
    conversation: &Entity<ConversationState>,
    id: &str,
    output: &str,
) {
    apply(
        cx,
        conversation,
        ThreadEvent::ToolResult {
            id: id.into(),
            output: output.into(),
            is_error: false,
        },
    );
    pump(visual, 20);
}

fn tool_entry(conversation: &Entity<ConversationState>, cx: &mut TestAppContext) -> ToolCallItem {
    cx.update(|cx| {
        conversation
            .read(cx)
            .items()
            .iter()
            .find_map(|item| match item.read(cx).kind() {
                ConvItem::Thinking(t) => t.entries.iter().find_map(|entry| match entry {
                    ActivityEntry::Tool(tool) => Some(tool.clone()),
                    _ => None,
                }),
                ConvItem::ToolCall(tool) => Some(tool.clone()),
                _ => None,
            })
            .expect("the conversation holds a tool entry")
    })
}

fn assert_card_paints(visual: &mut VisualTestContext) {
    let bounds = visual
        .debug_bounds("message-overflow-activity-entry-0-0")
        .expect("the tool card row paints");
    assert!(
        bounds.size.height > px(8.),
        "the card has real height: {bounds:?}"
    );
}

/// The recorded production fixture: live chunks through the open reveal, then
/// the canonical result. Under the publish cap this stays append-only, so no
/// document generation is burnt.
#[gpui::test]
async fn the_recorded_bash_output_streams_through_an_open_card(cx: &mut TestAppContext) {
    let (conversation, mut visual) = mount_bash_card(cx, "toolu_d255c1cb2f9f4c44b1b2ff9a");
    stream(
        cx,
        &mut visual,
        &conversation,
        "toolu_d255c1cb2f9f4c44b1b2ff9a",
        CRASH_BASH_OUTPUT,
    );
    settle(
        cx,
        &mut visual,
        &conversation,
        "toolu_d255c1cb2f9f4c44b1b2ff9a",
        CRASH_BASH_OUTPUT,
    );

    let entry = tool_entry(&conversation, cx);
    assert_eq!(entry.output, CRASH_BASH_OUTPUT);
    assert_eq!(entry.stream.published(), CRASH_BASH_OUTPUT);
    assert_eq!(
        entry.stream.generation(),
        0,
        "below the cap the body extends in place; no replace is needed"
    );
    assert_card_paints(&mut visual);
}

/// A settled body past `LINE_CAP` (400 lines): the card keeps the first 400
/// lines and appends the truncation note; the full text stays authoritative on
/// the entry (copy/diff paths).
#[gpui::test]
async fn an_output_past_line_cap_settles_truncated(cx: &mut TestAppContext) {
    let full: String = (0..450)
        .map(|ix| format!("line {ix}: 中文输出 with `ticks` and \"quotes\"\n"))
        .collect();
    assert!(full.lines().count() > 400);

    let (conversation, mut visual) = mount_bash_card(cx, "toolu-long");
    stream(cx, &mut visual, &conversation, "toolu-long", &full);
    settle(cx, &mut visual, &conversation, "toolu-long", &full);

    let entry = tool_entry(&conversation, cx);
    assert_eq!(entry.output, full, "the entry keeps the full text");
    assert_eq!(entry.stream.published(), full);

    // The rendered shell body is the first LINE_CAP lines (the note is added
    // by the card; the capped prefix is what must stay stable).
    let rendered: Vec<&str> = entry.stream.published().lines().take(400).collect();
    let expected: Vec<&str> = full.lines().take(400).collect();
    assert_eq!(rendered.len(), 400);
    assert_eq!(rendered, expected);
    assert_card_paints(&mut visual);
}

/// Output past the snapshot cap forces the live-tail window and then a settle
/// replace: the generation moves, so the body state is never paired with the
/// previous parse.
#[gpui::test]
async fn a_live_tail_head_drop_settles_into_a_fresh_generation(cx: &mut TestAppContext) {
    let mut full = String::new();
    while full.len() <= PUBLISH_CAP + 4096 {
        full.push_str("output line with 中文 and `backticks`\n");
    }

    let (conversation, mut visual) = mount_bash_card(cx, "toolu-window");
    stream(cx, &mut visual, &conversation, "toolu-window", &full);

    // The window reset happened while streaming (the published text is the
    // capped live tail, not the whole source).
    let streaming = tool_entry(&conversation, cx);
    assert!(streaming.stream.published().len() <= full.len());
    assert!(
        streaming.stream.generation() >= 1,
        "crossing the cap resets the window and advances the generation"
    );

    let before_settle = streaming.stream.generation();
    settle(cx, &mut visual, &conversation, "toolu-window", &full);

    let entry = tool_entry(&conversation, cx);
    assert_eq!(
        entry.stream.published(),
        full,
        "settle publishes the full body"
    );
    assert!(
        entry.stream.generation() > before_settle,
        "settling from a tail window is a replace, not an extension"
    );
    assert_card_paints(&mut visual);
}

/// A retry/rewrite lands a completely different text. The settle flush
/// replaces the body and advances the generation.
#[gpui::test]
async fn a_retry_rewrite_replaces_the_body(cx: &mut TestAppContext) {
    let (conversation, mut visual) = mount_bash_card(cx, "toolu-retry");
    stream(
        cx,
        &mut visual,
        &conversation,
        "toolu-retry",
        "first attempt: partial 中文 output\n",
    );

    let canonical = "different canonical result\n第二行：完全不同的文本\n";
    settle(cx, &mut visual, &conversation, "toolu-retry", canonical);

    let entry = tool_entry(&conversation, cx);
    assert_eq!(entry.output, canonical);
    assert_eq!(entry.stream.published(), canonical);
    assert!(
        entry.stream.generation() >= 1,
        "a non-append rewrite must mint a new generation"
    );
    assert_card_paints(&mut visual);
}

/// A burst that ends the call's output, followed by a quiet command: the last
/// chunks land inside the publish tick and no further chunk arrives. Render
/// passes are the card's only clock after that, so the next frame must publish
/// the deferred tail without waiting for the settle.
#[gpui::test]
async fn a_quiet_tail_catches_up_on_the_next_render(cx: &mut TestAppContext) {
    let (conversation, mut visual) = mount_bash_card(cx, "toolu-quiet");
    // Let the mount's reveal hold expire so this exercises the tick path.
    cx.executor().advance_clock(Duration::from_millis(450));
    pump(&mut visual, 1);

    apply(
        cx,
        &conversation,
        ThreadEvent::ToolOutput {
            id: "toolu-quiet".into(),
            chunk: "first burst line\n".into(),
        },
    );
    pump(&mut visual, 1);
    assert_eq!(
        tool_entry(&conversation, cx).stream.published(),
        "first burst line\n"
    );

    // The rest of the burst arrives inside the tick; then the command goes
    // quiet — no settle, no further chunk.
    apply(
        cx,
        &conversation,
        ThreadEvent::ToolOutput {
            id: "toolu-quiet".into(),
            chunk: "second burst line\n".into(),
        },
    );
    cx.executor().advance_clock(Duration::from_millis(120));
    pump(&mut visual, 2);

    let entry = tool_entry(&conversation, cx);
    assert_eq!(entry.output, "first burst line\nsecond burst line\n");
    assert_eq!(
        entry.stream.published(),
        entry.output,
        "a render pass publishes the tail the tick deferred"
    );
    assert_card_paints(&mut visual);
}

/// A burst that arrives entirely inside the card's open reveal, then silence.
/// The hold suppresses tick publishes, but once it expires a render pass must
/// land the deferred text (no new chunk, no settle).
#[gpui::test]
async fn a_burst_inside_the_open_reveal_lands_after_the_hold(cx: &mut TestAppContext) {
    let (conversation, mut visual) = mount_bash_card(cx, "toolu-reveal");
    // The mount's render set the reveal hold at the current clock; feed the
    // burst without advancing it, so every chunk lands inside the hold.
    let burst = "line one\nline two\nline three\n";
    for chunk in ["line one\n", "line two\n", "line three\n"] {
        apply(
            cx,
            &conversation,
            ThreadEvent::ToolOutput {
                id: "toolu-reveal".into(),
                chunk: chunk.into(),
            },
        );
    }
    pump(&mut visual, 1);
    assert_eq!(
        tool_entry(&conversation, cx).stream.published(),
        "line one\n",
        "the hold suppresses the deferred chunks"
    );

    // Silence: no further chunk. Only the clock moves past the hold.
    cx.executor().advance_clock(Duration::from_millis(450));
    pump(&mut visual, 2);

    let entry = tool_entry(&conversation, cx);
    assert_eq!(entry.output, burst);
    assert_eq!(
        entry.stream.published(),
        burst,
        "the hold releases on a later render pass and the deferred text lands"
    );
    assert_card_paints(&mut visual);
}

/// The hostile content classes harvested from the exploratory sweep: CRLF,
/// lone CR, ANSI escapes, backtick-fence boundaries, CJK, emoji, and a line
/// wider than the card. Each streams through the open reveal and settles.
#[gpui::test]
async fn hostile_content_streams_through_the_card(cx: &mut TestAppContext) {
    let cases: &[&str] = &[
        "crlf line one\r\ncrlf line two\r\n",
        "lone\rcr line\nafter\n",
        "\x1b[32mgreen中文\x1b[0m plain\n\x1b[1;31mbold red\x1b[0m\n",
        "backtick `inline` line\n```\nfence-ish line\n```\n",
        "~~~~\ntilde block\n~~~~\n````\nfour backticks\n",
        "中文输出第一行\n第二行：结果如下 emoji 👨‍👩‍👧 😀\n",
        "发\r\n中\r文\r混排 \x1b[32m绿色\r\n中文\x1b[0m\n",
        "```\u{200b}zero width\nafter\n",
        "src/lib.rs:12:    let s = \"中文\";\nsrc/中/lib.rs:3:匹配 中文 两次 中文\n",
        "pub fn long_line() { pub fn long_line() { pub fn long_line() { pub fn long_line() { pub fn long_line() {}\n",
    ];

    for (ix, content) in cases.iter().enumerate() {
        let id = format!("toolu-hostile-{ix}");
        let (conversation, mut visual) = mount_bash_card(cx, &id);
        stream(cx, &mut visual, &conversation, &id, content);
        settle(cx, &mut visual, &conversation, &id, content);

        let entry = tool_entry(&conversation, cx);
        assert_eq!(entry.stream.published(), *content, "case {ix}");
        assert_card_paints(&mut visual);
    }
}
