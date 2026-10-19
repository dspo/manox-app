//! Component gallery for `ai-elements`.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p ai-elements --example gallery
//! ```
//!
//! Left rail lists the components this crate ships; the main pane shows the
//! selected one's behaviour demos, each with the controls that drive it. Start a
//! scripted stream and watch the body open itself, the trigger shimmer, and the
//! block fold a second after the stream ends; toggle by hand and watch the
//! automatic behaviour stand down. This is the components' acceptance surface —
//! the chat panel wires the same states in from `ThreadEvent`s.

use std::time::Duration;

use ai_elements::{
    AskBlock, AskOption, AskState, AskUserQuestion, CodeLine, CodeView, FRAME_NAMES, Meerkat,
    PixelSpinner, PixelSpinnerVariant, Reasoning, ReasoningState, ShellLine, ShellSpanKind,
    ShellView, Tool, ToolState, ToolStatus,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, px, size,
};
use gpui_component::{Icon, Root, Sizable as _, Theme, button::Button, h_flex, v_flex};
use gpui_kit_assets::IconName;
use steer_components::markdown::{
    TextViewState, message_document, static_document, stream_fade_motion,
};

/// The scripted reasoning text. A scripted stream feeds it a few characters at a
/// time — the cadence a provider actually delivers, and the thing that makes a
/// stream read as progressive instead of line-by-line.
const SCRIPT: &str = concat!(
    "Let me look at what the repository already does here. ",
    "The `Reasoning` block mirrors upstream's `Reasoning` element — same props, same\n",
    "auto-open and auto-fold behaviour — but the state lives in an entity instead of a\n",
    "React context. That means the host drives it explicitly:\n\n",
    "- `set_streaming(true)` on the rising edge of a thinking stream,\n",
    "- `set_streaming(false)` when the stream ends.\n\n",
    "The fold, the duration clock, and the user-toggle pin all follow from those two calls.",
);

/// How often a scripted delta lands. A real stream arrives continuously; at
/// this cadence the body grows on essentially every frame.
const DELTA_CADENCE: Duration = Duration::from_millis(16);

/// Characters per scripted delta — token-sized, not sentence-sized.
const DELTA_CHARS: usize = 3;

/// Cut `SCRIPT` into the deltas a scripted stream feeds, on character
/// boundaries so multi-byte text never splits.
fn scripted_deltas() -> Vec<&'static str> {
    let mut deltas = Vec::new();
    let mut start = 0;
    let mut taken = 0;
    for (ix, ch) in SCRIPT.char_indices() {
        taken += 1;
        if taken == DELTA_CHARS {
            let end = ix + ch.len_utf8();
            deltas.push(&SCRIPT[start..end]);
            start = end;
            taken = 0;
        }
    }
    if start < SCRIPT.len() {
        deltas.push(&SCRIPT[start..]);
    }
    deltas
}
/// A component this crate ships. Every variant appears in the rail; the selected
/// one's demos fill the pane.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Component {
    Reasoning,
    PixelSpinner,
    HistoryLoading,
    ToolText,
    ToolShell,
    AskQuestion,
}

impl Component {
    const ALL: &'static [Component] = &[
        Component::Reasoning,
        Component::PixelSpinner,
        Component::HistoryLoading,
        Component::ToolText,
        Component::ToolShell,
        Component::AskQuestion,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Reasoning => "Reasoning",
            Self::PixelSpinner => "Pixel Spinner",
            Self::HistoryLoading => "History Loading",
            Self::ToolText => "Tools — Text",
            Self::ToolShell => "Tools — Shell",
            Self::AskQuestion => "AskUserQuestion",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Reasoning => {
                "A collapsible block for one round of thinking: opens itself while the stream is \
                 live, folds one second after it ends."
            }
            Self::PixelSpinner => {
                "VS Code's session-status glyph on gpui primitives: six 2px dots that drop \
                 through a 16px frame in a quantized cascade while working, or light up in a \
                 clockwise sweep while waiting for input."
            }
            Self::HistoryLoading => {
                "The pixel meerkat the app plays while a reopened thread's chat snapshot is in \
                 flight — same one-clock approach as the pixel spinner: cycle time picks the \
                 slot, the slot picks the frame, only the active frame renders."
            }
            Self::ToolText => {
                "The file tools (`Read` / `Write` / `Edit`) as tool cards: an icon-name-args \
                 header over an in-band editor — gutter, diff markers, naive coloring, no \
                 surface tint beyond the diff. Drag-select copies what you see; the hover \
                 `in`/`out` chips quote the raw call. Settling folds the card a second \
                 later; a hand toggle pins it."
            }
            Self::ToolShell => {
                "The command family (`Bash`, `BashOutput` — and `Grep`, `Glob`, `Ls` rendered \
                 in the same shell vocabulary, not as shell tools) as tool cards: the header \
                 keeps the tool's real identity, the body is a fake terminal that opens with \
                 the host's zsh prompt lines — path, git branch and dirt, toolchains — then \
                 the ❯ command, colored output, and an exit footer. Selecting the body \
                 copies the transcript you see."
            }
            Self::AskQuestion => {
                "The AskUserQuestion takeover card as a paged form: radio circles and \
                 checkboxes loud enough to read at a glance, a note field per question, the \
                 `< 1/3 >` pager whose next button becomes the submit check on the last \
                 page, and — once submitted — the host's markdown output standing in for \
                 the form."
            }
        }
    }
}

struct Demo {
    title: &'static str,
    note: &'static str,
    state: Entity<ReasoningState>,
    /// The reasoning body. Rebuilt per scripted run, exactly as the
    /// conversation mounts one per reasoning round: a streaming document is
    /// created mid-stream and finalized when the round ends.
    markdown: Entity<TextViewState>,
    /// The in-flight scripted feed. Holding it here is what cancels a run the
    /// user restarts, instead of two feeds interleaving into one body.
    stream_task: Option<gpui::Task<()>>,
    /// Whether the block mounts mid-stream, and whether its message differs
    /// from the default.
    mounts_streaming: bool,
    custom_message: bool,
}

impl Demo {
    fn is_streaming(&self, cx: &App) -> bool {
        self.state.read(cx).is_streaming()
    }
}

/// The tool cards' states, one entity per card so header clicks and the
/// settle-fold reach the screen. Static demos mount settled and open; the
/// lifecycle demo mounts running and is driven by its buttons.
struct ToolDemos {
    read: Entity<ToolState>,
    ranged: Entity<ToolState>,
    edit: Entity<ToolState>,
    write: Entity<ToolState>,
    failed: Entity<ToolState>,
    lifecycle: Entity<ToolState>,
    bash_ok: Entity<ToolState>,
    bash_fail: Entity<ToolState>,
    bash_run: Entity<ToolState>,
    bash_output: Entity<ToolState>,
    grep: Entity<ToolState>,
    glob: Entity<ToolState>,
    ls: Entity<ToolState>,
}

struct Gallery {
    selected: Component,
    reasoning: Vec<Demo>,
    tools: ToolDemos,
    /// The interactive wizard's live state: page, selections, notes,
    /// submitted.
    ask: Entity<AskState>,
    /// The pre-settled card's state — what a replayed transcript mounts.
    ask_answered: Entity<AskState>,
    /// The scripted stream's delta sequence, cut once.
    deltas: Vec<&'static str>,
    /// Keeps every demo's state observed, so a fold the block performs on its
    /// own (the delayed one) reaches the screen without a click.
    _subscriptions: Vec<Subscription>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let (reasoning, mut subscriptions) = Self::build_reasoning(cx);
        let tools = Self::build_tools(cx, &mut subscriptions);
        // The live wizard. Questions: single, multi, single — one flag per
        // block, in order.
        let ask = cx.new(|_| AskState::new(vec![false, true, false], "补充说明…（可选）"));
        subscriptions.push(cx.observe(&ask, |_, _, cx| cx.notify()));
        // The answered card: settled at mount with one choice on the first
        // question and nothing on the second — the skipped-question shape the
        // output markdown has to carry.
        let ask_answered = cx.new(|cx| {
            let mut state = AskState::new(vec![false], "补充说明…（可选）");
            state.toggle_option(0, 0, cx);
            state.submit(cx);
            state
        });
        subscriptions.push(cx.observe(&ask_answered, |_, _, cx| cx.notify()));
        Self {
            selected: Component::Reasoning,
            reasoning,
            tools,
            ask,
            ask_answered,
            deltas: scripted_deltas(),
            _subscriptions: subscriptions,
        }
    }

    /// One settled-and-open card state: what a replayed transcript mounts.
    fn settled_card(
        cx: &mut Context<Self>,
        status: ToolStatus,
        duration: Option<Duration>,
    ) -> Entity<ToolState> {
        let state = cx.new(|cx| ToolState::new(cx).with_status(status, cx));
        state.update(cx, |state, cx| {
            state.set_duration(duration, cx);
            state.set_open(true, cx);
        });
        state
    }

    fn build_tools(cx: &mut Context<Self>, subscriptions: &mut Vec<Subscription>) -> ToolDemos {
        let read = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(120)));
        let ranged = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(90)));
        let edit = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(210)));
        let write = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(340)));
        let failed = Self::settled_card(cx, ToolStatus::Failed, Some(Duration::from_millis(420)));
        // The lifecycle demo mounts mid-run, the way a live conversation
        // first renders a tool it just dispatched.
        let lifecycle = cx.new(|cx| ToolState::new(cx).with_status(ToolStatus::Running, cx));
        let bash_ok =
            Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(4210)));
        let bash_fail =
            Self::settled_card(cx, ToolStatus::Failed, Some(Duration::from_millis(2870)));
        let bash_run = cx.new(|cx| ToolState::new(cx).with_status(ToolStatus::Running, cx));
        let bash_output = cx.new(|cx| ToolState::new(cx).with_status(ToolStatus::Running, cx));
        let grep = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(120)));
        let glob = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(60)));
        let ls = Self::settled_card(cx, ToolStatus::Succeeded, Some(Duration::from_millis(40)));
        for state in [
            &read,
            &ranged,
            &edit,
            &write,
            &failed,
            &lifecycle,
            &bash_ok,
            &bash_fail,
            &bash_run,
            &bash_output,
            &grep,
            &glob,
            &ls,
        ] {
            subscriptions.push(cx.observe(state, |_, _, cx| cx.notify()));
        }
        ToolDemos {
            read,
            ranged,
            edit,
            write,
            failed,
            lifecycle,
            bash_ok,
            bash_fail,
            bash_run,
            bash_output,
            grep,
            glob,
            ls,
        }
    }

    fn build_reasoning(cx: &mut Context<Self>) -> (Vec<Demo>, Vec<Subscription>) {
        #[allow(clippy::type_complexity)]
        let specs: &[(&'static str, &'static str, bool, bool, Option<bool>)] = &[
            (
                "Streaming",
                "Born idle. Press Start: the block opens itself and the trigger shimmers while \
                 deltas arrive. It folds one second after the stream ends, and the trigger then \
                 reports the elapsed seconds.",
                false,
                false,
                None,
            ),
            (
                "Streaming, mounted mid-run",
                "The block is constructed with `streaming(true)`, so it is already open — how a \
                 conversation mounts a round the page just discovered.",
                true,
                false,
                None,
            ),
            (
                "Explicitly closed while streaming",
                "`default_open(false)` pins the block shut: a stream can neither open it nor fold \
                 it. Press Start and watch it stay closed.",
                false,
                false,
                Some(false),
            ),
            (
                "Settled, duration supplied",
                "`set_duration(Some(12))` — upstream's `duration` prop. A host that keeps its own \
                 timing reports it instead of letting the block measure.",
                false,
                false,
                Some(true),
            ),
            (
                "Settled, zero-second stream",
                "A stream that ended within its first second reads as `0`, and the trigger falls \
                 back to the live shimmer — upstream's `duration === 0` case.",
                false,
                false,
                Some(true),
            ),
            (
                "Settled, never streamed",
                "No duration was ever measured: the trigger reads \"Thought for a few seconds\" \
                 rather than inventing one.",
                false,
                false,
                Some(true),
            ),
            (
                "Custom thinking message",
                "The default label is untranslated English. A host that localizes its chrome \
                 passes its own `get_thinking_message` — exactly upstream's \
                 `getThinkingMessage`.",
                false,
                true,
                None,
            ),
        ];

        let mut demos = Vec::new();
        let mut subscriptions = Vec::new();
        for (title, note, mounts_streaming, custom_message, default_open) in specs.iter() {
            let mounts_streaming = *mounts_streaming;
            let state = cx.new(|cx| {
                let state = match *default_open {
                    Some(open) => ReasoningState::new(cx).default_open(open),
                    None => ReasoningState::new(cx),
                };
                if mounts_streaming {
                    state.streaming(true, cx)
                } else {
                    state
                }
            });
            subscriptions.push(cx.observe(&state, |_, _, cx| cx.notify()));
            // The settled demos open with a body in place, so a folded block
            // has something to show before any control is touched.
            let seeded = *default_open == Some(true);
            let markdown = mount_body(cx, if seeded { SCRIPT } else { "" });
            demos.push(Demo {
                title,
                note,
                state,
                markdown,
                stream_task: None,
                mounts_streaming,
                custom_message: *custom_message,
            });
        }
        demos[3].state.update(cx, |state, cx| {
            state.set_duration(Some(12), cx);
        });
        demos[4].state.update(cx, |state, cx| {
            state.set_duration(Some(0), cx);
        });
        (demos, subscriptions)
    }

    /// Start the scripted stream: the block is told a stream began, then fed
    /// deltas, then told it ended — the same three calls the chat panel makes.
    fn start_stream(&mut self, ix: usize, cx: &mut Context<Self>) {
        let mounts_streaming = self.reasoning[ix].mounts_streaming;
        // A fresh streaming document per run, as the conversation mounts one per
        // reasoning round: the streaming flag is a mount-time property, and it is
        // what puts the trailing cursor at the end of the growing body.
        self.reasoning[ix].markdown = mount_body(cx, "");
        self.reasoning[ix].state.update(cx, |state, cx| {
            state.set_duration(None, cx);
            // A block mounted mid-run is already streaming; feeding it another
            // rising edge would be a no-op, so only idle demos get one.
            if !mounts_streaming {
                state.set_streaming(true, cx);
            }
        });
        cx.notify();

        let deltas = self.deltas.clone();
        // Assigning over the previous handle cancels a run this one replaces.
        self.reasoning[ix].stream_task = Some(cx.spawn(async move |this, cx| {
            for delta in deltas {
                cx.background_executor().timer(DELTA_CADENCE).await;
                let live = this
                    .update(cx, |gallery, cx| gallery.feed_delta(ix, delta, cx))
                    .is_ok();
                if !live {
                    return;
                }
            }
            let _ = this.update(cx, |gallery, cx| gallery.finish_stream(ix, cx));
        }));
    }

    /// Append one delta. The body notifies itself, so the block repaints without
    /// the gallery re-rendering every demo row on every delta.
    fn feed_delta(&mut self, ix: usize, delta: &str, cx: &mut Context<Self>) {
        self.reasoning[ix]
            .markdown
            .update(cx, |md, cx| md.push_str(delta, cx));
    }

    fn finish_stream(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.reasoning[ix].stream_task = None;
        self.reasoning[ix]
            .state
            .update(cx, |state, cx| state.set_streaming(false, cx));
        cx.notify();
    }

    fn toggle_by_hand(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.reasoning[ix]
            .state
            .update(cx, |state, cx| state.toggle(cx));
        cx.notify();
    }

    fn demo_block(&self, ix: usize, cx: &App) -> impl IntoElement {
        let demo = &self.reasoning[ix];
        let mut block = Reasoning::new(("reasoning", ix), &demo.state).content(
            message_document(&demo.markdown, Theme::global(cx), px(13.))
                .motion(stream_fade_motion())
                .into_any_element(),
        );
        if demo.custom_message {
            block = block.get_thinking_message(|streaming, duration, _window, _cx| {
                if streaming {
                    SharedString::from("思考中…").into_any_element()
                } else {
                    match duration {
                        Some(secs) => {
                            SharedString::from(format!("已思考 {secs} 秒")).into_any_element()
                        }
                        None => SharedString::from("已思考").into_any_element(),
                    }
                }
            });
        }
        block
    }

    fn demo_row(
        &self,
        ix: usize,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, note) = (self.reasoning[ix].title, self.reasoning[ix].note);
        let is_streaming = self.reasoning[ix].is_streaming(cx);
        v_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(border)
            .rounded(px(10.))
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(foreground)
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(note)),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new(("start", ix))
                            .xsmall()
                            .outline()
                            .label(if is_streaming { "Restart" } else { "Start" })
                            .on_click(
                                cx.listener(move |this, _, _window, cx| this.start_stream(ix, cx)),
                            ),
                    )
                    .child(
                        Button::new(("toggle", ix))
                            .xsmall()
                            .outline()
                            .label("Toggle by hand")
                            .on_click(
                                cx.listener(move |this, _, _window, cx| {
                                    this.toggle_by_hand(ix, cx)
                                }),
                            ),
                    ),
            )
            .child(self.demo_block(ix, cx))
            .into_any_element()
    }

    /// The pixel spinner's demos: the two variants at native size and zoomed
    /// in, then a slow-motion pass and the linear-timing contrast arm. These
    /// are time-only demos — nothing to click, the clock runs from mount.
    fn pixel_spinner_rows(
        &self,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
    ) -> Vec<AnyElement> {
        // The two states a session status renders in, in VS Code's palette:
        // link blue for working, warning yellow for waiting.
        let accent: gpui::Hsla = gpui::rgb(0x3794ff).into();
        let warning: gpui::Hsla = gpui::rgb(0xcca700).into();

        vec![
            demo_card(
                muted,
                border,
                foreground,
                "Grid — the falling cascade",
                "Six 2px dots on a 1820 ms cycle, staggered 520/650/260/390/0/130 ms; each \
                 rises from −4 px, holds lit in its cell, and falls +7 px out of the clipped \
                 frame. Every moving segment snaps through steps(6, jump-none). The 16px tile \
                 is the size it runs at in a session list.",
                h_flex()
                    .gap_10()
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new().color(accent),
                        "1× (16 px)",
                    ))
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new().color(accent).zoom(8.),
                        "8× · steps(6)",
                    ))
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new().color(accent).zoom(8.).sharp(),
                        "8× · square dots",
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Ring — waiting for input",
                "The dots hold their grid positions at a quarter opacity while a highlight \
                 steps up to full and decays — a clockwise sweep, one dot every 200 ms around \
                 a 1200 ms cycle, quantized by steps(4, jump-none).",
                h_flex()
                    .gap_10()
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new()
                            .variant(PixelSpinnerVariant::Ring)
                            .color(warning)
                            .zoom(8.),
                        "8× · clockwise",
                    ))
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new()
                            .variant(PixelSpinnerVariant::Ring)
                            .color(warning),
                        "1× (16 px)",
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Quarter speed, and what linear timing costs",
                "Slowing the clock to a quarter lets you count each discrete hop — including \
                 the long/short hold variants that keep the cascade from reading as \
                 mechanical. The right one is the same keyframes on linear timing: smooth, \
                 and generic. The steps snap is the pixel.",
                h_flex()
                    .gap_10()
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new().color(accent).zoom(6.).speed(0.25),
                        "grid ×6 · speed ¼",
                    ))
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new()
                            .variant(PixelSpinnerVariant::Ring)
                            .color(warning)
                            .zoom(6.)
                            .speed(0.25),
                        "ring ×6 · speed ¼",
                    ))
                    .child(spinner_cell(
                        muted,
                        PixelSpinner::new().color(muted).zoom(8.).smooth(),
                        "8× · linear",
                    )),
            )
            .into_any_element(),
        ]
    }

    /// The history-loading page's demos: the sprite at its native scale, a
    /// half-speed pass for reading the loop, and the four frames frozen
    /// side by side. Time-only again — the clock runs from mount.
    fn history_loading_rows(
        &self,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
    ) -> Vec<AnyElement> {
        vec![
            demo_card(
                muted,
                border,
                foreground,
                "The loading sprite",
                "12×14 cells, 4 distinct frames in a 6-slot loop (idle, bob, blink, bob, idle, \
                 tail flick) at 3 slots per second — the face the main column shows while a \
                 reopened thread's snapshot is in flight. Half speed lets you see each slot.",
                h_flex()
                    .gap_10()
                    .items_center()
                    .child(spinner_cell(muted, Meerkat::new(), "cell 10 · 3 slots/s"))
                    .child(spinner_cell(
                        muted,
                        Meerkat::new().speed(0.5),
                        "cell 10 · speed ½",
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "The four frames, frozen",
                "Frame 0 is the master grid; the others are hand-derived variants — the bob \
                 drops the feet row off the grid, the blink collapses the eye patches to an \
                 outline one row below the brow, the tail flick lifts the tail to head height.",
                h_flex().gap_10().items_center().children(
                    FRAME_NAMES
                        .iter()
                        .enumerate()
                        .map(|(ix, name)| spinner_cell(muted, Meerkat::new().freeze(ix), name)),
                ),
            )
            .into_any_element(),
        ]
    }

    /// The text tools' demos: the three file shapes a `CodeView` renders
    /// (`Read` window, `Read` range with a gap, `Edit` hunk with the old/new
    /// gutter pair, `Write` whole file), a failed edit, and the interactive
    /// lifecycle card. Bodies carry copy controls — `in`/`out` quote the
    /// call's raw JSON and raw result; selecting the body copies the text
    /// you see, gutter numbers included.
    fn tool_text_rows(
        &self,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let tools = &self.tools;
        vec![
            demo_card(
                muted,
                border,
                foreground,
                "Read — a whole file",
                "The header carries the invocation's identity: icon, tool name, the file in the \
                 UI-weight mono, the path muted. The body is the mock editor — sequential line \
                 numbers, naive token coloring, no surface tint. Click the header to fold; the \
                 status chip keeps the outcome visible either way.",
                Tool::new("demo-read", &tools.read)
                    .icon(Icon::new(IconName::FileText).xsmall())
                    .name("Read")
                    .args(file_args(foreground, "reasoning.rs", "crates/ai-elements/src"))
                    .copy_input(READ_RAW_INPUT)
                    .copy_output(READ_RAW_OUTPUT)
                    .content(CodeView::source("demo-read-code", READ_SAMPLE)),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Read — a range with a gap",
                "A ranged read keeps the file's own line numbers (the window starts where the \
                 file does) and marks the skipped span with a `...` row — the shape the model \
                 actually sees, minus the `N:` prefixes.",
                Tool::new("demo-ranged", &tools.ranged)
                    .icon(Icon::new(IconName::FileText).xsmall())
                    .name("Read")
                    .args(file_args(foreground, "chat_fold.rs", "crates/steer-agent-chat-ui/src"))
                    .copy_input(RANGED_RAW_INPUT)
                    .content(CodeView::new(
                        "demo-ranged-code",
                        vec![
                            CodeLine::plain(
                                138,
                                "fn find_tool_call<'a>(chat: &'a ChatState, id: &str) -> Option<&'a ToolCallState> {",
                            ),
                            CodeLine::plain(
                                139,
                                "    chat.turns.iter().find_map(|turn| find_in(&turn.parts, id))",
                            ),
                            CodeLine::plain(140, "}"),
                            CodeLine::gap(),
                            CodeLine::plain(
                                168,
                                "/// The live turn stays in `active_turn` until the engine settles the",
                            ),
                            CodeLine::plain(
                                169,
                                "/// round; the reducer folds finished turns into `chat.turns`.",
                            ),
                            CodeLine::plain(170, "fn tool_call_id_of(call: &ToolCallState) -> &str {"),
                            CodeLine::plain(171, "    &call.id"),
                            CodeLine::plain(172, "}"),
                        ],
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Edit — a unified hunk",
                "Any line carrying a pre-image number widens the gutter to the old/new pair. \
                 Removed lines tint danger with the `-` marker, added lines success with `+` — \
                 the diff tint is the one surface tint the editor keeps.",
                Tool::new("demo-edit", &tools.edit)
                    .icon(Icon::new(IconName::Pencil).xsmall())
                    .name("Edit")
                    .args(file_args(foreground, "reasoning.rs", "crates/ai-elements/src"))
                    .copy_diff(EDIT_DIFF)
                    .copy_output("The file crates/ai-elements/src/reasoning.rs has been edited. (2 lines changed)")
                    .content(CodeView::new(
                        "demo-edit-code",
                        vec![
                            CodeLine::context(
                                132,
                                "    pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {",
                            ),
                            CodeLine::removed(133, "        if self.is_streaming == streaming {"),
                            CodeLine::removed(134, "            return;"),
                            CodeLine::added(
                                133,
                                "        if self.is_streaming == streaming && !streaming {",
                            ),
                            CodeLine::added(
                                134,
                                "            // a no-op falling edge collapses early; the rising edge opens",
                            ),
                            CodeLine::context(135, "        }"),
                            CodeLine::context(136, "        if streaming {"),
                        ],
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Write — a new file",
                "Same sequential gutter as a whole-file read: the content is new, but it numbers \
                 from 1 because that is where the file will open. The card's identity — icon, \
                 name, the created path — is all the header says.",
                Tool::new("demo-write", &tools.write)
                    .icon(Icon::new(IconName::FilePlus).xsmall())
                    .name("Write")
                    .args(file_args(foreground, "code_view.rs", "crates/ai-elements/src"))
                    .copy_input(WRITE_RAW_INPUT)
                    .content(CodeView::source(
                        "demo-write-code",
                        "//! A text-editor mock for file-shaped tool bodies.\n\nuse gpui::{Hsla, IntoElement, Render};\n\npub struct CodeView {\n    lines: Vec<CodeLine>,\n}\n\nimpl CodeView {\n    pub fn source(text: &str) -> Self {\n        Self { lines: split_and_number(text) }\n    }\n}\n",
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Edit — failed, then the lifecycle",
                "The failed card shows the failure chip (cross, danger tint, duration). The \
                 right card is the live contract: mounted mid-run, it opens itself; settling \
                 it folds it a second later — unless you toggle it by hand first, which pins \
                 it for good. Restart re-opens it.",
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Tool::new("demo-failed", &tools.failed)
                                    .icon(Icon::new(IconName::Pencil).xsmall())
                                    .name("Edit")
                                    .args(file_args(
                                        foreground,
                                        "workspace.rs",
                                        "crates/agent-ui/src",
                                    ))
                                    .content(CodeView::new(
                                        "demo-failed-code",
                                        vec![
                                            CodeLine::removed(1804, "    let Some((store, _)) = live.take() else {"),
                                            CodeLine::added(
                                                1804,
                                                "    let Some((store, session_id)) = live.take() else {",
                                            ),
                                        ],
                                    )),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("lifecycle-restart")
                                            .xsmall()
                                            .outline()
                                            .label("Restart")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.tools.lifecycle.update(cx, |state, cx| {
                                                    state.set_status(ToolStatus::Running, cx)
                                                })
                                            })),
                                    )
                                    .child(
                                        Button::new("lifecycle-succeed")
                                            .xsmall()
                                            .outline()
                                            .label("Succeed")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.tools.lifecycle.update(cx, |state, cx| {
                                                    state.set_status(ToolStatus::Succeeded, cx)
                                                })
                                            })),
                                    )
                                    .child(
                                        Button::new("lifecycle-fail")
                                            .xsmall()
                                            .outline()
                                            .label("Fail")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.tools.lifecycle.update(cx, |state, cx| {
                                                    state.set_status(ToolStatus::Failed, cx)
                                                })
                                            })),
                                    ),
                            )
                            .child(
                                Tool::new("demo-lifecycle", &tools.lifecycle)
                                    .icon(Icon::new(IconName::FileText).xsmall())
                                    .name("Read")
                                    .args(file_args(
                                        foreground,
                                        "ahp_store.rs",
                                        "crates/steer-agent-chat-ui/src",
                                    ))
                                    .content(CodeView::new(
                                        "demo-lifecycle-code",
                                        vec![
                                            CodeLine::plain(1170, "let event = chat_events.pop_front();"),
                                            CodeLine::plain(1171, "match event {"),
                                            CodeLine::plain(1172, "    Some((sid, ev)) if sid == owner => apply(ev),"),
                                            CodeLine::plain(1173, "    _ => {} // not ours: drop it"),
                                            CodeLine::plain(1174, "}"),
                                        ],
                                    )),
                            ),
                    ),
            )
            .into_any_element(),
        ]
    }

    /// The command family's demos: every body is the same fake terminal —
    /// the host's zsh prompt lines, `❯` command, colored output, exit
    /// footer — while the header keeps each tool's real identity. `Grep` /
    /// `Glob` / `Ls` are rendered in shell vocabulary (their command
    /// equivalent and file-shaped output), not claimed as shell tools.
    fn tool_shell_rows(
        &self,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
    ) -> Vec<AnyElement> {
        let tools = &self.tools;
        // The prompt this machine prints: path + git branch + dirt, then the
        // toolchain line. In the app the host derives these from the user's
        // shell configuration; the gallery pins the dev box's.
        let prompt = |shell: ShellView| {
            shell
                .context(ShellLine::spans(vec![
                    (ShellSpanKind::Path, "~/projects/dspo/manox-app".into()),
                    (ShellSpanKind::Plain, " ".into()),
                    (ShellSpanKind::Git, "git:main".into()),
                    (ShellSpanKind::Plain, " ".into()),
                    (ShellSpanKind::Hit, "*2!4?11".into()),
                ]))
                .context(ShellLine::dim("node v26.0.0 rs v1.95.0"))
        };
        vec![
            demo_card(
                muted,
                border,
                foreground,
                "Bash — passing",
                "The settled shape: header chip with the duration, body with the exit footer. \\
                 The prompt area renders the host's zsh configuration — path, `git:main` with \\
                 its dirt markers, the toolchain line. Selecting the body copies the \\
                 transcript you see; `in`/`out` quote the raw call.",
                Tool::new("demo-bash-ok", &tools.bash_ok)
                    .icon(Icon::new(IconName::SquareTerminal).xsmall())
                    .name("Bash")
                    .args(cmd_args(foreground, "cargo test -p ai-elements"))
                    .copy_input(BASH_RAW_INPUT)
                    .copy_output(BASH_RAW_OUTPUT)
                    .content(prompt(
                        ShellView::new("demo-bash-ok-shell", "cargo test -p ai-elements")
                            .line(ShellLine::dim("   Compiling ai-elements v0.1.0"))
                            .line(ShellLine::plain("running 40 tests"))
                            .line(ShellLine::plain(
                                "test views::trigger_label_follows_upstream ... ok",
                            ))
                            .line(ShellLine::plain("test views::reasoning_stream ... ok"))
                            .line(ShellLine::plain(
                                "test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured",
                            ))
                            .exit(0)
                            .duration(Duration::from_millis(4210)),
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Bash — failing",
                "Error lines render in the terminal's error red; the footer carries the real \\
                 exit code and the failure tint. The header chip mirrors the outcome so a \\
                 folded card still tells you it broke.",
                Tool::new("demo-bash-fail", &tools.bash_fail)
                    .icon(Icon::new(IconName::SquareTerminal).xsmall())
                    .name("Bash")
                    .args(cmd_args(foreground, "cargo build -p steer-app"))
                    .copy_input(r#"{"command":"cargo build -p steer-app"}"#)
                    .copy_output("error[E0432]: unresolved import `agent_ui::chrome`\nerror: could not compile `steer-app` (bin \"steer\") due to 1 previous error")
                    .content(prompt(
                        ShellView::new("demo-bash-fail-shell", "cargo build -p steer-app")
                            .line(ShellLine::dim("   Compiling steer-app v0.1.0"))
                            .line(ShellLine::error(
                                "error[E0432]: unresolved import `agent_ui::chrome`",
                            ))
                            .line(ShellLine::dim(" --> crates/steer-app/src/main.rs:12:5"))
                            .line(ShellLine::dim("   |"))
                            .line(ShellLine::dim(
                                "12 |     use agent_ui::chrome_assembly::ChromeShell;",
                            ))
                            .line(ShellLine::error(
                                "error: could not compile `steer-app` (bin \"steer\") due to 1 previous error",
                            ))
                            .exit(101)
                            .duration(Duration::from_millis(2870)),
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Bash — running, and BashOutput",
                "An unsettled command: spinner in the footer, `running` instead of an exit \\
                 line. `BashOutput` is the same surface reading a background shell — its \\
                 command line is the read, its footer stays live because the stream is open.",
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div().flex_1().min_w_0().child(
                            Tool::new("demo-bash-run", &tools.bash_run)
                                .icon(Icon::new(IconName::SquareTerminal).xsmall())
                                .name("Bash")
                                .args(cmd_args(foreground, "cargo test --workspace --all-targets"))
                                .copy_input(r#"{"command":"cargo test --workspace --all-targets"}"#)
                                .content(prompt(
                                    ShellView::new(
                                        "demo-bash-run-shell",
                                        "cargo test --workspace --all-targets",
                                    )
                                    .line(ShellLine::plain("running 612 tests"))
                                    .line(ShellLine::plain("test views::reasoning_stream ... ok"))
                                    .line(ShellLine::plain("test views::ask_card_render ... "))
                                    .line(ShellLine::plain("test views::turn_rail ... "))
                                    .running(),
                                )),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Tool::new("demo-bash-output", &tools.bash_output)
                                .icon(Icon::new(IconName::Terminal).xsmall())
                                .name("BashOutput")
                                .args(cmd_args(foreground, "tail -n 3 /tmp/steer-shell-1.out"))
                                .copy_input(r#"{"shell_id":"shell_1"}"#)
                                .content(prompt(
                                    ShellView::new(
                                        "demo-bash-output-shell",
                                        "tail -n 3 /tmp/steer-shell-1.out",
                                    )
                                    .line(ShellLine::dim(
                                        "[10:02:11] watch: crates/ai-elements/src/** changed",
                                    ))
                                    .line(ShellLine::dim("[10:02:11] cargo check -p ai-elements"))
                                    .line(ShellLine::plain(
                                        "[10:02:12] Finished `dev` profile [unoptimized] in 0.61s",
                                    ))
                                    .running(),
                                )),
                        ),
                    ),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Grep — hits, located and highlighted",
                "The header keeps `Grep`'s identity; the body speaks shell. Hit lines are \\
                 `path:line:content` with the path and line number tinted and the matched \\
                 needle raised — the grep view a terminal actually gives you, plus a summary.",
                Tool::new("demo-grep", &tools.grep)
                    .icon(Icon::new(IconName::TextSearch).xsmall())
                    .name("Grep")
                    .args(cmd_args(
                        foreground,
                        "find_tool_call · crates/steer-agent-chat-ui/src",
                    ))
                    .copy_input(GREP_RAW_INPUT)
                    .copy_output("crates/steer-agent-chat-ui/src/chat_fold.rs:138: fn find_tool_call<'a>(...)\ncrates/steer-agent-chat-ui/src/message.rs:2317: let Some(review) = ...\n\n3 matches\n")
                    .content(prompt(
                        ShellView::new(
                            "demo-grep-shell",
                            "grep -rn \"find_tool_call\" crates/steer-agent-chat-ui/src",
                        )
                        .line(ShellLine::hit(
                            "crates/steer-agent-chat-ui/src/chat_fold.rs",
                            138,
                            "fn find_tool_call<'a>(chat: &'a ChatState, id: &str) -> Option<&'a ToolCallState> {",
                            "find_tool_call",
                        ))
                        .line(ShellLine::hit(
                            "crates/steer-agent-chat-ui/src/message.rs",
                            2317,
                            "    let Some(review) = fold_input_request(&turn, id) else {",
                            "find_tool_call",
                        ))
                        .line(ShellLine::hit(
                            "crates/steer-agent-chat-ui/src/chat_fold.rs",
                            258,
                            "fn tool_result_text(content: Option<&[ToolResultContent]>) -> String {",
                            "find_tool_call",
                        ))
                        .line(ShellLine::blank())
                        .line(ShellLine::dim("3 matches · 2 files"))
                        .exit(0)
                        .duration(Duration::from_millis(120)),
                    )),
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Glob and Ls — file-shaped output",
                "`Glob`'s body is its shell equivalent (`find -name`); `Ls` is the listing \\
                 itself, directories tinted as paths, permissions dim. One visual series, \\
                 honest tool identities.",
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        div().flex_1().min_w_0().child(
                            Tool::new("demo-glob", &tools.glob)
                                .icon(Icon::new(IconName::FileSearch).xsmall())
                                .name("Glob")
                                .args(cmd_args(foreground, "**/*.rs · crates/ai-elements/src"))
                                .copy_input(GLOB_RAW_INPUT)
                                .content(prompt(
                                    ShellView::new(
                                        "demo-glob-shell",
                                        "find crates/ai-elements/src -name \"*.rs\"",
                                    )
                                    .line(path_line("crates/ai-elements/src/lib.rs"))
                                    .line(path_line("crates/ai-elements/src/animation.rs"))
                                    .line(path_line("crates/ai-elements/src/ask.rs"))
                                    .line(path_line("crates/ai-elements/src/code_view.rs"))
                                    .line(path_line("crates/ai-elements/src/reasoning.rs"))
                                    .line(path_line("crates/ai-elements/src/shell.rs"))
                                    .line(path_line("crates/ai-elements/src/tool.rs"))
                                    .line(ShellLine::blank())
                                    .line(ShellLine::dim("7 files"))
                                    .exit(0)
                                    .duration(Duration::from_millis(60)),
                                )),
                        ),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Tool::new("demo-ls", &tools.ls)
                                .icon(Icon::new(IconName::FolderTree).xsmall())
                                .name("Ls")
                                .args(cmd_args(foreground, "crates/ai-elements"))
                                .copy_input(LS_RAW_INPUT)
                                .content(prompt(
                                    ShellView::new("demo-ls-shell", "ls -la crates/ai-elements")
                                        .line(ShellLine::dim("total 0"))
                                        .line(ls_line(
                                            "drwxr-xr-x   9 czr  staff   288 Oct 16 10:02 ",
                                            ".",
                                        ))
                                        .line(ls_line(
                                            "drwxr-xr-x@  4 czr  staff   128 Oct 15 18:44 ",
                                            "..",
                                        ))
                                        .line(ls_line(
                                            "drwxr-xr-x   5 czr  staff   160 Oct 15 18:44 ",
                                            "examples",
                                        ))
                                        .line(ls_line(
                                            "drwxr-xr-x   4 czr  staff   128 Oct 16 09:58 ",
                                            "src",
                                        ))
                                        .line(ls_line(
                                            "drwxr-xr-x   3 czr  staff    96 Oct 15 18:44 ",
                                            "tests",
                                        ))
                                        .line(ls_line(
                                            "-rw-r--r--   1 czr  staff   512 Oct 15 18:44 ",
                                            "Cargo.toml",
                                        ))
                                        .exit(0)
                                        .duration(Duration::from_millis(40)),
                                )),
                        ),
                    ),
            )
            .into_any_element(),
        ]
    }

    /// The ask card's demos: the live wizard (paged form, note field,
    /// pager, submit) and the pre-settled card whose output markdown is what
    /// a replayed transcript shows.
    fn ask_rows(
        &mut self,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::global(cx);
        let submitted = self.ask.read(cx).submitted();

        // The wizard card. Submitted, it swaps the form for the output
        // markdown composed from the live state — the host's summary step.
        let mut wizard = AskUserQuestion::new("ask-live", &self.ask)
            .icon(Icon::new(IconName::MessageCircleQuestionMark).xsmall())
            .blocks(ask_blocks());
        if submitted {
            let markdown = ask_output_markdown(&self.ask, cx);
            wizard = wizard
                .status(ask_answered_chip())
                .output(static_document("ask-live-output", markdown, theme))
                .footnote("已提交 · 答案已回传工具");
        } else {
            wizard = wizard.status(ask_waiting_chip());
        }

        // The settled card: mounted answered, one choice made, the second
        // question skipped — the output markdown has to carry both.
        let answered_markdown = format!(
            "### Question 1: {}\n\n{}\n\n### Question 2: {}\n\n*(skipped)*\n",
            ASK_ANSWERED_Q1, ASK_ANSWERED_A1, ASK_ANSWERED_Q2,
        );
        let answered = AskUserQuestion::new("ask-answered", &self.ask_answered)
            .icon(Icon::new(IconName::MessageCircleQuestionMark).xsmall())
            .blocks(ask_answered_blocks())
            .status(ask_answered_chip())
            .output(static_document(
                "ask-answered-output",
                answered_markdown,
                theme,
            ))
            .footnote("已提交 · gpui-base TextView · Question 2 skipped");

        vec![
            demo_card(
                muted,
                border,
                foreground,
                if submitted {
                    "The wizard — submitted"
                } else {
                    "The wizard — one question per page"
                },
                "Radio circles for single-select blocks, checkboxes for multi-select, a note \\
                 field per question, and the bottom-left pager: `< 1/3 >` turns pages; on \\
                 the last page the next button becomes the accent check that submits. Work \\
                 it — answers and notes land in the output markdown the moment it settles.",
                wizard,
            )
            .into_any_element(),
            demo_card(
                muted,
                border,
                foreground,
                "Submitted — the output rendering",
                "What a replayed transcript mounts: the form is gone, the host's markdown \\
                 summary stands in its place — one heading per question, the chosen answers, \\
                 and an explicit *(skipped)* for the ones left unanswered.",
                answered,
            )
            .into_any_element(),
        ]
    }

    /// The left rail: one row per component, the selected one tinted.
    fn rail(
        &mut self,
        muted: gpui::Hsla,
        border: gpui::Hsla,
        foreground: gpui::Hsla,
        secondary: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.selected;
        v_flex()
            .w(px(220.))
            .h_full()
            .flex_shrink_0()
            .gap_1()
            .p_3()
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from("Components")),
            )
            .children(Component::ALL.iter().enumerate().map(|(ix, component)| {
                let component = *component;
                let is_selected = component == selected;
                h_flex()
                    .id(("component", ix))
                    .w_full()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_sm()
                    .when(is_selected, |row| row.bg(secondary).text_color(foreground))
                    .when(!is_selected, |row| {
                        row.text_color(muted)
                            .hover(move |row| row.bg(secondary.opacity(0.5)).text_color(foreground))
                    })
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.selected = component;
                        cx.notify();
                    }))
                    .child(SharedString::from(component.name()))
            }))
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);
        let (muted, border, background, foreground, secondary) = (
            theme.muted_foreground,
            theme.border,
            theme.background,
            theme.foreground,
            theme.secondary,
        );

        let rows: Vec<AnyElement> = match self.selected {
            Component::Reasoning => (0..self.reasoning.len())
                .map(|ix| self.demo_row(ix, muted, border, foreground, cx))
                .collect(),
            Component::PixelSpinner => self.pixel_spinner_rows(muted, border, foreground),
            Component::HistoryLoading => self.history_loading_rows(muted, border, foreground),
            Component::ToolText => self.tool_text_rows(muted, border, foreground, cx),
            Component::ToolShell => self.tool_shell_rows(muted, border, foreground),
            Component::AskQuestion => self.ask_rows(muted, border, foreground, cx),
        };

        let pane = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_base()
                            .text_color(foreground)
                            .child(SharedString::from(self.selected.name())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(SharedString::from(self.selected.blurb())),
                    ),
            )
            .child(
                div()
                    .id("gallery-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(v_flex().w_full().gap_4().p_6().children(rows)),
            )
            .child(
                div()
                    .w_full()
                    .px_6()
                    .py_3()
                    .border_t_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .child(SharedString::from(footer_note(self.selected))),
            );

        h_flex()
            .size_full()
            .bg(background)
            .child(self.rail(muted, border, foreground, secondary, cx))
            .child(pane)
    }
}

/// Mount a body the way the conversation mounts one for a reasoning round: a
/// persistent document entity that `feed_delta` grows through the incremental
/// `push_str` path.
fn mount_body(cx: &mut Context<Gallery>, source: &'static str) -> Entity<TextViewState> {
    cx.new(|cx| TextViewState::markdown(source, cx))
}

/// The pane footer states the contract the selected component's demos are
/// about: the reasoning blocks' automation contract, or the spinner's
/// clock-only one.
fn footer_note(selected: Component) -> &'static str {
    match selected {
        Component::PixelSpinner => {
            "Clock-only demos: one gpui Animation folds cycle time through the per-dot \
             delays and keyframe tables. The steps() snap is the pixel; speed is a rate \
             multiplier on the same clock."
        }
        Component::HistoryLoading => {
            "Clock-only demos: one gpui Animation maps cycle time to the loop's six slots, \
             and only the active frame's cells render — no stacked layers. Freeze pins a \
             frame; speed multiplies the same clock."
        }
        Component::ToolText => {
            "Tool cards: a Running edge opens the body, a settled edge folds it one second \
             later (AUTO_FOLD_DELAY), and a hand toggle pins the card — the Reasoning \
             contract on tool vocabulary. The editor renders in-band through the rich-text \
             core, so selection copy is the window's own; the tokenizer is line-local by \
             design, and anything it can't classify renders plain."
        }
        Component::ToolShell => {
            "One fake terminal for the whole command family: the host's zsh prompt lines, ❯ \
             command, span-tinted output, exit footer — one in-band document, so selection \
             copies the transcript. The dark palette is the component's own; a terminal \
             surface stays dark under either app theme."
        }
        Component::AskQuestion => {
            "All form state lives in AskState — page, selections, notes, submitted — driven \
             by clicks and typing, observed by the host. Submit emits AskSubmitted; the \
             host then hands the composed markdown back through .output(), which is the \
             shape a replayed transcript mounts."
        }
        _ => {
            "Auto-open while streaming, auto-fold one second after the stream ends \
             (AUTO_CLOSE_DELAY). A hand toggle pins the block: nothing automatic moves \
             it afterwards."
        }
    }
}

/// The whole-file `Read` sample — chosen to exercise the mock tokenizer:
/// keywords, attributes, doc comments, and a type-ful enum.
const READ_SAMPLE: &str = "use std::time::Duration;\n\n/// How long a block stays open after its stream ends before folding.\npub const AUTO_CLOSE_DELAY: Duration = Duration::from_millis(1000);\n\n#[derive(Clone, Debug, PartialEq)]\npub enum ThinkingLabel {\n    Shimmer,\n    Seconds(u64),\n}\n";

/// The header's argument summary for file tools: the file name in the
/// foreground, the path muted beside it.
fn file_args(foreground: gpui::Hsla, file: &'static str, path: &'static str) -> impl IntoElement {
    h_flex()
        .gap_1p5()
        .min_w_0()
        .overflow_hidden()
        .child(div().text_color(foreground).child(file))
        .child(div().opacity(0.75).child(path))
}

/// The header's argument summary for command tools: the command itself.
fn cmd_args(foreground: gpui::Hsla, cmd: &'static str) -> impl IntoElement {
    div().text_color(foreground).child(cmd)
}

/// One tinted path line — the shape `Glob` output takes.
fn path_line(path: &'static str) -> ShellLine {
    ShellLine::spans(vec![(ShellSpanKind::Path, SharedString::from(path))])
}

/// One `ls -la` row: dim permissions/size/date prefix, the name tinted as a
/// path when it is a directory.
fn ls_line(prefix: &'static str, name: &'static str) -> ShellLine {
    ShellLine::spans(vec![
        (ShellSpanKind::Dim, SharedString::from(prefix)),
        (
            if name.ends_with(".toml") {
                ShellSpanKind::Plain
            } else {
                ShellSpanKind::Path
            },
            SharedString::from(name),
        ),
    ])
}

/// Raw tool-call structures the copy controls quote — `in` copies the input
/// JSON, `out` the raw result, exactly as the machine saw them.
const READ_RAW_INPUT: &str = r#"{"file_path":{"relative_path":"crates/ai-elements/src/reasoning.rs"},"offset":1,"limit":60}"#;
const READ_RAW_OUTPUT: &str = "[crates/ai-elements/src/reasoning.rs#T4K2]\n  1: use std::time::Duration;\n  2:\n  3: /// How long a block stays open after its stream ends before folding.\n …";
/// The edit family's quotable shape: not the input JSON, the unified diff.
const EDIT_DIFF: &str = "--- a/crates/ai-elements/src/reasoning.rs\n+++ b/crates/ai-elements/src/reasoning.rs\n@@ -132,6 +132,7 @@\n     pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {\n-        if self.is_streaming == streaming {\n-            return;\n+        if self.is_streaming == streaming && !streaming {\n+            // a no-op falling edge collapses early; the rising edge opens\n         }\n";
const WRITE_RAW_INPUT: &str = r#"{"file_path":{"relative_path":"crates/ai-elements/src/code_view.rs"},"content":"//! A text-editor mock for file-shaped tool bodies.\n…"}"#;
const BASH_RAW_INPUT: &str = r#"{"command":"cargo test -p ai-elements","timeout_ms":120000,"description":"Test the gallery's crate"}"#;
const BASH_RAW_OUTPUT: &str = "   Compiling ai-elements v0.1.0\nrunning 40 tests\ntest result: ok. 40 passed; 0 failed; 0 ignored; 0 measured\n\nfinished in 4.21s";
const GLOB_RAW_INPUT: &str = r#"{"pattern":"crates/ai-elements/src/**/*.rs"}"#;
const LS_RAW_INPUT: &str = r#"{"path":{"relative_path":"crates/ai-elements"}}"#;
const RANGED_RAW_INPUT: &str = r#"{"file_path":{"relative_path":"crates/steer-agent-chat-ui/src/chat_fold.rs"},"offset":138,"limit":40}"#;
const GREP_RAW_INPUT: &str = r#"{"pattern":"find_tool_call","path":{"relative_path":"crates/steer-agent-chat-ui/src"},"output_mode":"content","head_limit":100}"#;

/// The wizard's three questions as the card renders them.
fn ask_blocks() -> Vec<AskBlock> {
    vec![
        AskBlock::new("Which rich-text core should the chat standardize on?")
            .header("Libraries")
            .option(
                AskOption::new("gpui-base TextView")
                    .badge("Recommended")
                    .description(
                        "The stack markdown already renders through — incremental push_str, \
                         selection, highlight channels.",
                    ),
            )
            .option(
                AskOption::new("gpui-component Markdown").description(
                    "Higher-level, but a full re-parse per update; no incremental path.",
                ),
            )
            .option(
                AskOption::new("hand-rolled markdown")
                    .description("Retired in #145 — −4.4k lines say no."),
            ),
        AskBlock::new("Which surfaces get the tool cards first?")
            .header("Scope")
            .multi_select()
            .option(AskOption::new("Conversation column"))
            .option(AskOption::new("Right rail ToolTab"))
            .option(AskOption::new("Exported transcripts")),
        AskBlock::new("Should settled tool bodies fold by default?")
            .header("Default state")
            .option(AskOption::new("Fold after a second").badge("Recommended"))
            .option(AskOption::new("Stay open until clicked")),
    ]
}

/// The settled demo's questions — mirrored by [`ASK_ANSWERED_Q1`] and friends
/// so the markdown and the state agree.
fn ask_answered_blocks() -> Vec<AskBlock> {
    vec![
        AskBlock::new(ASK_ANSWERED_Q1)
            .header("Libraries")
            .option(
                AskOption::new(ASK_ANSWERED_A1)
                    .badge("Recommended")
                    .description(
                        "The stack markdown already renders through — incremental push_str, \
                         selection, highlight channels.",
                    ),
            )
            .option(
                AskOption::new("gpui-component Markdown").description(
                    "Higher-level, but a full re-parse per update; no incremental path.",
                ),
            ),
        AskBlock::new(ASK_ANSWERED_Q2)
            .header("Default state")
            .option(AskOption::new("Fold after a second").badge("Recommended"))
            .option(AskOption::new("Stay open until clicked")),
    ]
}

const ASK_ANSWERED_Q1: &str = "Which rich-text core should the chat standardize on?";
const ASK_ANSWERED_A1: &str = "gpui-base TextView";
const ASK_ANSWERED_Q2: &str = "Should settled tool bodies fold by default?";

/// Compose the submit output's markdown from the live state: one section per
/// question, chosen labels joined, the note appended, *(skipped)* when a
/// block carries no answer.
fn ask_output_markdown(state: &Entity<AskState>, cx: &App) -> String {
    let blocks = ask_blocks();
    let mut out = String::new();
    for (qix, block) in blocks.iter().enumerate() {
        let _ = block;
        let labels: Vec<SharedString> = {
            let selections = state.read(cx).selection(qix).to_vec();
            let opts = options_of(qix);
            selections
                .iter()
                .filter_map(|oix| opts.get(*oix).cloned())
                .collect()
        };
        out.push_str(&format!(
            "### Question {}: {}\n\n",
            qix + 1,
            questions_of(qix)
        ));
        if labels.is_empty() {
            out.push_str("*(skipped)*\n\n");
        } else {
            out.push_str(
                &labels
                    .iter()
                    .map(|l| l.as_ref())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push('\n');
            let note = state.read(cx).note(qix, cx);
            if !note.is_empty() {
                out.push_str(&format!("\n（补充：{note}）\n"));
            }
            out.push('\n');
        }
    }
    out
}

/// The questions' plain text, indexed — the markdown composer's source of
/// truth alongside [`ask_blocks`].
fn questions_of(qix: usize) -> &'static str {
    const QUESTIONS: &[&str] = &[
        "Which rich-text core should the chat standardize on?",
        "Which surfaces get the tool cards first?",
        "Should settled tool bodies fold by default?",
    ];
    QUESTIONS[qix]
}

/// The questions' option labels, indexed.
fn options_of(qix: usize) -> Vec<SharedString> {
    const OPTIONS: &[&[&str]] = &[
        &[
            "gpui-base TextView",
            "gpui-component Markdown",
            "hand-rolled markdown",
        ],
        &[
            "Conversation column",
            "Right rail ToolTab",
            "Exported transcripts",
        ],
        &["Fold after a second", "Stay open until clicked"],
    ];
    OPTIONS[qix]
        .iter()
        .map(|s| SharedString::from(*s))
        .collect()
}

/// The ask header's waiting chip: the pending-auth blue dot the sidebar
/// already uses, plus the label.
fn ask_waiting_chip() -> AnyElement {
    let blue: gpui::Hsla = gpui::rgb(0x3794ff).into();
    h_flex()
        .gap_1p5()
        .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(blue))
        .child(div().text_xs().child("waiting for your answer"))
        .into_any_element()
}

/// The ask header's settled chip.
fn ask_answered_chip() -> AnyElement {
    h_flex()
        .gap_1p5()
        .child(
            gpui_component::Icon::new(gpui_component::IconName::CircleCheck)
                .xsmall()
                .into_any_element(),
        )
        .child(div().text_xs().child("answered"))
        .into_any_element()
}

fn main() {
    let app = gpui_platform::application().with_assets(gpui_kit_assets::AllAssets);
    app.run(|cx| {
        gpui_component::init(cx);
        let bounds = gpui::WindowBounds::centered(size(px(1040.), px(760.)), cx);
        let _ = cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(bounds),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title("ai-elements — gallery");
                let gallery = cx.new(Gallery::new);
                cx.new(|cx| Root::new(gallery, window, cx))
            },
        );
    });
}

/// One demo cell: the glyph centered in a fixed stage, its label under it —
/// same shape as the reference page's zoomed stages.
fn spinner_cell(
    muted: gpui::Hsla,
    stage: impl IntoElement,
    label: &'static str,
) -> impl IntoElement {
    v_flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex()
                .w(px(170.))
                .h(px(150.))
                .items_center()
                .justify_center()
                .child(stage),
        )
        .child(div().text_xs().text_color(muted).child(label))
}

/// One demo card: a title, a note, and the component under test.
fn demo_card(
    muted: gpui::Hsla,
    border: gpui::Hsla,
    foreground: gpui::Hsla,
    title: &'static str,
    note: &'static str,
    body: impl IntoElement,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .min_w_0()
        .gap_3()
        .p_4()
        .border_1()
        .border_color(border)
        .rounded(px(10.))
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(foreground)
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(note)),
                ),
        )
        .child(body)
}
