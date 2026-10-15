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
    FRAME_NAMES, Meerkat, PixelSpinner, PixelSpinnerVariant, Reasoning, ReasoningState,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, px, size,
};
use gpui_component::{Root, Sizable as _, Theme, button::Button, h_flex, v_flex};
use steer_components::markdown::{TextViewState, message_document, stream_fade_motion};

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
}

impl Component {
    const ALL: &'static [Component] = &[
        Component::Reasoning,
        Component::PixelSpinner,
        Component::HistoryLoading,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Reasoning => "Reasoning",
            Self::PixelSpinner => "Pixel Spinner",
            Self::HistoryLoading => "History Loading",
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

struct Gallery {
    selected: Component,
    reasoning: Vec<Demo>,
    /// The scripted stream's delta sequence, cut once.
    deltas: Vec<&'static str>,
    /// Keeps every demo's state observed, so a fold the block performs on its
    /// own (the delayed one) reaches the screen without a click.
    _subscriptions: Vec<Subscription>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>) -> Self {
        let (reasoning, subscriptions) = Self::build_reasoning(cx);
        Self {
            selected: Component::Reasoning,
            reasoning,
            deltas: scripted_deltas(),
            _subscriptions: subscriptions,
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
        _ => {
            "Auto-open while streaming, auto-fold one second after the stream ends \
             (AUTO_CLOSE_DELAY). A hand toggle pins the block: nothing automatic moves \
             it afterwards."
        }
    }
}

fn main() {
    let app = gpui_platform::application().with_assets(gpui_kit_assets::Assets);
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
