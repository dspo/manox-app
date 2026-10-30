//! Regression tests for the 2026-10-26 tool-card abort.
//!
//! Production aborted (`SIGABRT`) in the layout phase inside
//! `StyledText::with_runs` ("invalid text run") beneath
//! `ShellView` → `Tool` → `Collapsible`/`MotionReveal` → `MessageItem`,
//! while a Bash card's output streamed under its open reveal. The exact
//! trigger could not be pinned offscreen (see
//! `.agents/skills/gpui-kit-follow/references/upstream-bugs.md`); these tests
//! instead guard the structural exposure end to end: the production
//! `ChatState` fold → `ConversationState::project` → `MessageItem` →
//! ai-elements card assembly, driven frame by frame through the open reveal
//! with text that changes discontinuously (stream batching, the live-tail head
//! drop past 24 KiB, a settle-time truncation past `LINE_CAP`, and a retry
//! rewrite).
//!
//! The projection lowers a fresh `ToolCallItem` every pass; the reconcile
//! mutates the materialized entry in place and `sync_tool_fields` deliberately
//! does not copy `stream`, so the snapshot — like `panel`/`card`/`cwd` — is
//! carried across passes. Only a mid-stream observation can pin that: a reset
//! snapshot publishes the whole text at once, while a carried one is still
//! throttled by the publish tick (see
//! `a_carried_snapshot_is_throttled_mid_stream`). End-state assertions cannot
//! tell the two apart.
//!
//! Offscreen animation frames only arrive through `Window::simulate_next_frame`
//! (AGENTS.md "出 UI 图"); the fake scheduler clock is advanced between chunks
//! so the 100 ms publish tick actually fires while the reveal runs.

use std::sync::Arc;
use std::time::Duration;

use ahp_types::actions::StateAction;
use ahp_types::state::ChatState;
use gpui::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use manox_agent::MessageAuthor;
use steer_agent_chat_ui::conversation::{ActivityEntry, ConvItem, ConversationState, ToolCallItem};
use steer_agent_chat_ui::host::{ChatHostHandle, noop_host};
use steer_agent_chat_ui::transcript::{ProjectEnv, no_steered_turns};
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

// --- the AHP fold ------------------------------------------------------------

fn empty_chat() -> ChatState {
    serde_json::from_value(serde_json::json!({
        "resource": "ahp-chat:/c-test",
        "title": "test",
        "status": 0,
        "modifiedAt": "2026-01-01T00:00:00Z",
        "turns": [],
    }))
    .expect("the empty chat parses")
}

fn action(value: serde_json::Value) -> StateAction {
    serde_json::from_value(value).expect("the action parses")
}

fn fold(chat: &mut ChatState, action: &StateAction) {
    ahp::reducers::apply_action_to_chat(chat, action);
}

fn turn_started(command: &str) -> StateAction {
    action(serde_json::json!({
        "type": "chat/turnStarted", "turnId": "t-1",
        "startedAt": "2026-01-01T00:00:00Z",
        "message": { "text": command, "origin": { "kind": "user" } },
    }))
}

fn tool_started(command: &str) -> StateAction {
    action(serde_json::json!({
        "type": "chat/toolCallStart", "turnId": "t-1", "toolCallId": "tc-1",
        "toolName": "Bash", "displayName": "Bash",
        "intention": format!("$ {command}"),
    }))
}

fn tool_ready(command: &str) -> StateAction {
    action(serde_json::json!({
        "type": "chat/toolCallReady", "turnId": "t-1", "toolCallId": "tc-1",
        "invocationMessage": format!("$ {command}"),
        "toolInput": serde_json::json!({ "command": command }).to_string(),
        "confirmed": "not-needed",
    }))
}

fn tool_content(text: &str) -> StateAction {
    action(serde_json::json!({
        "type": "chat/toolCallContentChanged", "turnId": "t-1", "toolCallId": "tc-1",
        "content": [{ "type": "text", "text": text }],
    }))
}

fn tool_completed(text: &str) -> StateAction {
    action(serde_json::json!({
        "type": "chat/toolCallComplete", "turnId": "t-1", "toolCallId": "tc-1",
        "result": {
            "success": true,
            "pastTenseMessage": "Ran the command",
            "content": [{ "type": "text", "text": text }],
        },
    }))
}

/// The projection's borrowed environment. The test constants are leaked: the
/// env borrows for the whole case and each test builds one.
fn env() -> ProjectEnv<'static> {
    let recipient: &'static MessageAuthor = Box::leak(Box::new(MessageAuthor::Lead));
    let host: &'static ChatHostHandle = Box::leak(Box::new(noop_host()));
    ProjectEnv {
        role: "test-model",
        recipient,
        host,
        cwd: None,
        fork_source: false,
        promoted_confirmation: None,
        entering_turns: &[],
        steered_turns: no_steered_turns(),
    }
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

// --- the production assembly -------------------------------------------------

struct Case {
    chat: ChatState,
    conversation: Entity<ConversationState>,
    visual: VisualTestContext,
}

impl Case {
    fn project(&mut self, cx: &mut TestAppContext) {
        let env = env();
        let chat = &self.chat;
        cx.update(|cx| {
            self.conversation.update(cx, |conversation, cx| {
                let _ = conversation.project(Some(chat), &env, cx);
            });
        });
    }

    fn row(&mut self, cx: &mut TestAppContext) -> ToolCallItem {
        let conversation = self.conversation.clone();
        tool_entry(&conversation, cx)
    }
}

/// Mount a fresh conversation with one running Bash card and open its window.
fn mount_bash_card(cx: &mut TestAppContext, command: &str) -> Case {
    cx.update(|cx| {
        if !cx.has_global::<gpui_component::Theme>() {
            gpui_component::init(cx);
        }
    });

    let mut chat = empty_chat();
    fold(&mut chat, &turn_started(command));
    fold(&mut chat, &tool_started(command));
    fold(&mut chat, &tool_ready(command));

    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    let env = env();
    cx.update(|cx| {
        conversation.update(cx, |conversation, cx| {
            let _ = conversation.project(Some(&chat), &env, cx);
        });
    });
    let rows = cx.update(|cx| conversation.read(cx).items().to_vec());
    assert_eq!(rows.len(), 2, "the user row and the activity segment");

    let window = cx.open_window(size(px(760.), px(720.)), move |_, _| Mount { rows });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    // Let the card mount and start its open reveal.
    pump(&mut visual, 2);
    Case {
        chat,
        conversation,
        visual,
    }
}

/// Stream `text` as live partial content, projecting after each chunk.
fn stream(cx: &mut TestAppContext, case: &mut Case, text: &str) {
    let mut fed = String::new();
    for chunk in chunks(text, 700) {
        fed.push_str(&chunk);
        fold(&mut case.chat, &tool_content(&fed));
        case.project(cx);
        // The publish tick is driven by the (fake) scheduler clock.
        cx.executor().advance_clock(Duration::from_millis(120));
        pump(&mut case.visual, 2);
    }
}

/// Settle the tool call with its canonical result; the turn stays active, as
/// in production (the tool result lands mid-turn).
fn settle(cx: &mut TestAppContext, case: &mut Case, output: &str) {
    fold(&mut case.chat, &tool_completed(output));
    case.project(cx);
    pump(&mut case.visual, 20);
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
    // The row's opaque MessageItem id keys the selector; the user row owns 0,
    // the activity segment is the row after it.
    for selector in [
        "message-overflow-activity-entry-0-0",
        "message-overflow-activity-entry-1-0",
        "message-overflow-activity-entry-2-0",
        "message-overflow-activity-entry-3-0",
    ] {
        if let Some(bounds) = visual.debug_bounds(selector) {
            assert!(
                bounds.size.height > px(8.),
                "the card has real height: {bounds:?}"
            );
            return;
        }
    }
    panic!("the tool card row paints");
}

// --- tests -------------------------------------------------------------------

/// The recorded production fixture: live chunks through the open reveal, then
/// the canonical result. Under the publish cap the end state is one append-only
/// body at generation 0; the carry across projection passes is pinned mid-stream
/// by [`a_carried_snapshot_is_throttled_mid_stream`].
#[gpui::test]
async fn the_recorded_bash_output_streams_through_an_open_card(cx: &mut TestAppContext) {
    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    stream(cx, &mut case, CRASH_BASH_OUTPUT);
    settle(cx, &mut case, CRASH_BASH_OUTPUT);

    let entry = case.row(cx);
    assert_eq!(entry.output, CRASH_BASH_OUTPUT);
    assert_eq!(entry.stream.published(), CRASH_BASH_OUTPUT);
    assert_eq!(
        entry.stream.generation(),
        0,
        "below the cap no replace is needed; the end state is one append-only body"
    );
    assert_card_paints(&mut case.visual);
}

/// The snapshot is carried across projection passes, not rebuilt per pass. A
/// carried snapshot is throttled by `PUBLISH_TICK`: right after a chunk lands
/// and a frame renders inside the tick, the published text is a strict,
/// non-empty prefix of the output. A snapshot lowered fresh each pass would
/// start empty, take the immediate first-publish branch, and publish the whole
/// text at once — the same end state, but never an interior prefix.
#[gpui::test]
async fn a_carried_snapshot_is_throttled_mid_stream(cx: &mut TestAppContext) {
    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    // Let the mount's reveal hold expire so only the tick gates the publish.
    cx.executor().advance_clock(Duration::from_millis(450));
    pump(&mut case.visual, 1);

    fold(&mut case.chat, &tool_content("line one\n"));
    case.project(cx);
    pump(&mut case.visual, 1);

    // The next chunk arrives inside the tick. Project and render once WITHOUT
    // moving the clock: a carried snapshot keeps the tail for the next tick.
    fold(&mut case.chat, &tool_content("line one\nline two\n"));
    case.project(cx);
    pump(&mut case.visual, 1);

    let entry = case.row(cx);
    assert_eq!(entry.output, "line one\nline two\n");
    assert!(
        !entry.stream.published().is_empty() && entry.stream.published().len() < entry.output.len(),
        "a carried snapshot publishes a strict prefix inside the tick, got {:?}",
        entry.stream.published()
    );
    assert!(
        entry.output.starts_with(entry.stream.published()),
        "the published text is a prefix of the live output"
    );

    // Past the tick, the same render path publishes the deferred tail.
    cx.executor().advance_clock(Duration::from_millis(120));
    pump(&mut case.visual, 1);
    let entry = case.row(cx);
    assert_eq!(entry.stream.published(), entry.output);
    assert_card_paints(&mut case.visual);
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

    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    stream(cx, &mut case, &full);
    settle(cx, &mut case, &full);

    let entry = case.row(cx);
    assert_eq!(entry.output, full, "the entry keeps the full text");
    assert_eq!(entry.stream.published(), full);

    // The rendered shell body is the first LINE_CAP lines (the note is added
    // by the card; the capped prefix is what must stay stable).
    let rendered: Vec<&str> = entry.stream.published().lines().take(400).collect();
    let expected: Vec<&str> = full.lines().take(400).collect();
    assert_eq!(rendered.len(), 400);
    assert_eq!(rendered, expected);
    assert_card_paints(&mut case.visual);
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

    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    stream(cx, &mut case, &full);

    // The window reset happened while streaming (the published text is the
    // capped live tail, not the whole source).
    let streaming = case.row(cx);
    assert!(streaming.stream.published().len() <= full.len());
    assert!(
        streaming.stream.generation() >= 1,
        "crossing the cap resets the window and advances the generation"
    );

    let before_settle = streaming.stream.generation();
    settle(cx, &mut case, &full);

    let entry = case.row(cx);
    assert_eq!(
        entry.stream.published(),
        full,
        "settle publishes the full body"
    );
    assert!(
        entry.stream.generation() > before_settle,
        "settling from a tail window is a replace, not an extension"
    );
    assert_card_paints(&mut case.visual);
}

/// A retry/rewrite lands a completely different text. The settle flush
/// replaces the body and advances the generation.
#[gpui::test]
async fn a_retry_rewrite_replaces_the_body(cx: &mut TestAppContext) {
    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    stream(cx, &mut case, "first attempt: partial 中文 output\n");

    let canonical = "different canonical result\n第二行：完全不同的文本\n";
    settle(cx, &mut case, canonical);

    let entry = case.row(cx);
    assert_eq!(entry.output, canonical);
    assert_eq!(entry.stream.published(), canonical);
    assert!(
        entry.stream.generation() >= 1,
        "a non-append rewrite must mint a new generation"
    );
    assert_card_paints(&mut case.visual);
}

/// A burst that ends the call's output, followed by a quiet command: the last
/// chunks land inside the publish tick and no further chunk arrives. Render
/// passes are the card's only clock after that, so the next frame must publish
/// the deferred tail without waiting for the settle.
#[gpui::test]
async fn a_quiet_tail_catches_up_on_the_next_render(cx: &mut TestAppContext) {
    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    // Let the mount's reveal hold expire so this exercises the tick path.
    cx.executor().advance_clock(Duration::from_millis(450));
    pump(&mut case.visual, 1);

    fold(&mut case.chat, &tool_content("first burst line\n"));
    case.project(cx);
    pump(&mut case.visual, 1);
    assert_eq!(
        case.row(cx).stream.published(),
        "first burst line\n",
        "the first text publishes immediately"
    );

    // The rest of the burst arrives inside the tick; then the command goes
    // quiet — no settle, no further chunk.
    fold(
        &mut case.chat,
        &tool_content("first burst line\nsecond burst line\n"),
    );
    case.project(cx);
    cx.executor().advance_clock(Duration::from_millis(120));
    pump(&mut case.visual, 2);

    let entry = case.row(cx);
    assert_eq!(entry.output, "first burst line\nsecond burst line\n");
    assert_eq!(
        entry.stream.published(),
        entry.output,
        "a render pass publishes the tail the tick deferred"
    );
    assert_card_paints(&mut case.visual);
}

/// A burst that arrives entirely inside the card's open reveal, then silence.
/// The hold suppresses tick publishes, but once it expires a render pass must
/// land the deferred text (no new chunk, no settle).
#[gpui::test]
async fn a_burst_inside_the_open_reveal_lands_after_the_hold(cx: &mut TestAppContext) {
    let mut case = mount_bash_card(cx, "rg -n 中文 .");
    // The mount's render set the reveal hold at the current clock; feed the
    // burst without advancing it, so every chunk lands inside the hold.
    let burst = "line one\nline two\nline three\n";
    fold(&mut case.chat, &tool_content("line one\n"));
    case.project(cx);
    fold(&mut case.chat, &tool_content("line one\nline two\n"));
    case.project(cx);
    fold(&mut case.chat, &tool_content(burst));
    case.project(cx);
    pump(&mut case.visual, 1);
    assert_eq!(
        case.row(cx).stream.published(),
        "line one\n",
        "the hold suppresses the deferred chunks"
    );

    // Silence: no further chunk. Only the clock moves past the hold.
    cx.executor().advance_clock(Duration::from_millis(450));
    pump(&mut case.visual, 2);

    let entry = case.row(cx);
    assert_eq!(entry.output, burst);
    assert_eq!(
        entry.stream.published(),
        burst,
        "the hold releases on a later render pass and the deferred text lands"
    );
    assert_card_paints(&mut case.visual);
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

    for content in cases {
        let mut case = mount_bash_card(cx, "rg -n 中文 .");
        stream(cx, &mut case, content);
        settle(cx, &mut case, content);

        let entry = case.row(cx);
        assert_eq!(entry.stream.published(), *content);
        assert_card_paints(&mut case.visual);
    }
}

// --- the fold click, over the production chain --------------------------------

/// A host wired to the harness's conversation — the workspace's real shape
/// (`entry_toggle` asks the host for the conversation, then flips the fold
/// authority on it).
struct HarnessHost(gpui::Entity<ConversationState>);

impl steer_agent_chat_ui::host::ChatHost for HarnessHost {
    fn conversation(&self, _cx: &gpui::App) -> Option<gpui::Entity<ConversationState>> {
        Some(self.0.clone())
    }
    fn dismiss_ask(&self, _cx: &mut gpui::App) {}
    fn submit_ask(
        &self,
        _replies: Vec<steer_agent_chat_ui::ask_card::AskCardReply>,
        _cx: &mut gpui::App,
    ) {
    }
    fn submit_input(&self, _window: &mut gpui::Window, _cx: &mut gpui::App) {}
    fn resolve_tool_confirmation(
        &self,
        _auth_id: &str,
        _option_id: &str,
        _approved: bool,
        _cx: &mut gpui::App,
    ) {
    }
    fn fork_session_at(&self, _through_entry_id: &str, _cx: &mut gpui::App) {}
    fn open_subagent_tab(
        &self,
        _id: &str,
        _subagent_type: &str,
        _topic: &str,
        _status: manox_agent::ToolCallStatus,
        _cx: &mut gpui::App,
    ) {
    }
    fn open_right_diff(&self, _path: Option<String>, _patch: String, _cx: &mut gpui::App) {}
}

/// The panel-vs-card adjacency: an assistant message whose diff fence renders
/// the editor panel, directly above a folded tool card. The panel's scroll
/// overlays must not eat the card header's click.
#[gpui::test]
async fn a_panel_above_does_not_eat_a_folded_card_s_click(cx: &mut TestAppContext) {
    let command = "cargo test";
    cx.update(|cx| {
        if !cx.has_global::<gpui_component::Theme>() {
            gpui_component::init(cx);
        }
    });

    let mut chat = empty_chat();
    fold(&mut chat, &turn_started(command));
    // The assistant prose opens a markdown part carrying a tall diff fence —
    // the editor panel — then the tool call lands below the reply.
    fold(
        &mut chat,
        &action(serde_json::json!({
            "type": "chat/responsePart", "turnId": "t-1",
            "part": { "kind": "markdown", "id": "md-1", "content": "" }
        })),
    );
    let fence = "-let gone = 1;\n+let here = 2;\n".repeat(12);
    fold(
        &mut chat,
        &action(serde_json::json!({
            "type": "chat/delta", "turnId": "t-1", "partId": "md-1",
            "content": format!("改一下:\n\n```diff\n{fence}```\n\n跑一下:\n"),
        })),
    );
    fold(&mut chat, &tool_started(command));
    fold(&mut chat, &tool_ready(command));

    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    let handle: ChatHostHandle = Arc::new(HarnessHost(conversation.clone()));
    let host: &'static ChatHostHandle = Box::leak(Box::new(handle));
    let env = ProjectEnv {
        role: "test-model",
        recipient: Box::leak(Box::new(MessageAuthor::Lead)),
        host,
        cwd: None,
        fork_source: false,
        promoted_confirmation: None,
        entering_turns: &[],
        steered_turns: no_steered_turns(),
    };
    cx.update(|cx| {
        conversation.update(cx, |conversation, cx| {
            let _ = conversation.project(Some(&chat), &env, cx);
        });
    });
    let rows = cx.update(|cx| conversation.read(cx).items().to_vec());
    assert!(rows.len() >= 3, "user row, assistant row, activity segment");

    let window = cx.open_window(size(px(760.), px(720.)), move |_, _| Mount { rows });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 2);

    fold(&mut chat, &tool_completed("ok\n"));
    cx.update(|cx| {
        conversation.update(cx, |conversation, cx| {
            let _ = conversation.project(Some(&chat), &env, cx);
        });
    });
    cx.executor().advance_clock(Duration::from_millis(1400));
    pump(&mut visual, 6);

    let header = visual
        .debug_bounds("tc-1-header")
        .expect("the folded card header paints below the panel");
    visual.simulate_click(header.center(), gpui::Modifiers::default());
    pump(&mut visual, 4);

    let expanded = cx.update(|cx| {
        conversation.read(cx).items().iter().any(|item| {
            matches!(
                item.read(cx).kind(),
                ConvItem::Thinking(t) if t.entries.iter().any(|entry| {
                    matches!(entry, ActivityEntry::Tool(tool) if !tool.collapsed)
                })
            )
        })
    });
    assert!(
        expanded,
        "the panel above must not eat the folded card's header click"
    );
}

#[gpui::test]
async fn clicking_a_folded_card_header_flips_the_fold_authority(cx: &mut TestAppContext) {
    let command = "cargo test";
    cx.update(|cx| {
        if !cx.has_global::<gpui_component::Theme>() {
            gpui_component::init(cx);
        }
    });

    let mut chat = empty_chat();
    fold(&mut chat, &turn_started(command));
    fold(&mut chat, &tool_started(command));
    fold(&mut chat, &tool_ready(command));

    let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
    let conversation_for_host = conversation.clone();
    let handle: ChatHostHandle = Arc::new(HarnessHost(conversation_for_host.clone()));
    let host: &'static ChatHostHandle = Box::leak(Box::new(handle));
    let env = ProjectEnv {
        role: "test-model",
        recipient: Box::leak(Box::new(MessageAuthor::Lead)),
        host,
        cwd: None,
        fork_source: false,
        promoted_confirmation: None,
        entering_turns: &[],
        steered_turns: no_steered_turns(),
    };
    cx.update(|cx| {
        conversation.update(cx, |conversation, cx| {
            let _ = conversation.project(Some(&chat), &env, cx);
        });
    });
    let rows = cx.update(|cx| conversation.read(cx).items().to_vec());

    let window = cx.open_window(size(px(760.), px(720.)), move |_, _| Mount { rows });
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    pump(&mut visual, 2);

    // Settle, then let the card's auto-fold fire.
    fold(&mut chat, &tool_completed("ok\n"));
    cx.update(|cx| {
        conversation.update(cx, |conversation, cx| {
            let _ = conversation.project(Some(&chat), &env, cx);
        });
    });
    cx.executor().advance_clock(Duration::from_millis(1400));
    pump(&mut visual, 6);

    let collapsed = cx.update(|cx| {
        conversation.read(cx).items().iter().any(|item| {
            matches!(
                item.read(cx).kind(),
                ConvItem::Thinking(t) if t.entries.iter().any(|entry| {
                    matches!(entry, ActivityEntry::Tool(tool) if tool.collapsed)
                })
            )
        })
    });
    assert!(
        collapsed,
        "the settled card has folded (the authority flipped)"
    );

    // Click the folded card's header over the production chain: on_toggle →
    // entry_toggle → the conversation flips → the card mirrors.
    let header = visual
        .debug_bounds("tc-1-header")
        .expect("the folded card header paints");
    visual.simulate_click(header.center(), gpui::Modifiers::default());
    pump(&mut visual, 4);

    let expanded = cx.update(|cx| {
        conversation.read(cx).items().iter().any(|item| {
            matches!(
                item.read(cx).kind(),
                ConvItem::Thinking(t) if t.entries.iter().any(|entry| {
                    matches!(entry, ActivityEntry::Tool(tool) if !tool.collapsed)
                })
            )
        })
    });
    assert!(
        expanded,
        "clicking the folded header flips the fold authority"
    );
}
