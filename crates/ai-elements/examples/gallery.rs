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

use ai_elements::{Reasoning, ReasoningState};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, prelude::*, px, size,
};
use gpui_component::{Root, Sizable as _, Theme, button::Button, h_flex, v_flex};
use manox_components::markdown::Markdown;

/// Gap between scripted reasoning deltas: slow enough to read the shimmer,
/// short enough that a run finishes without waiting.
const CHUNK_DELAY: Duration = Duration::from_millis(220);

/// The deltas a scripted stream appends, in order.
const CHUNKS: &[&str] = &[
    "Let me look at what the repository already does here. ",
    "The `Reasoning` block mirrors upstream's `Reasoning` element — same props, same\n",
    "auto-open and auto-fold behaviour — but the state lives in an entity instead of a\n",
    "React context. That means the host drives it explicitly:\n\n",
    "- `set_streaming(true)` on the rising edge of a thinking stream,\n",
    "- `set_streaming(false)` when the stream ends.\n\n",
    "The fold, the duration clock, and the user-toggle pin all follow from those two calls.",
];

/// A component this crate ships. Every variant appears in the rail; the selected
/// one's demos fill the pane.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Component {
    Reasoning,
}

impl Component {
    const ALL: &'static [Component] = &[Component::Reasoning];

    fn name(self) -> &'static str {
        match self {
            Self::Reasoning => "Reasoning",
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Self::Reasoning => {
                "A collapsible block for one round of thinking: opens itself while the stream is \
                 live, folds one second after it ends."
            }
        }
    }
}

struct Demo {
    title: &'static str,
    note: &'static str,
    state: Entity<ReasoningState>,
    /// The reasoning body. Its document outlives the frame, so a scripted
    /// stream grows one parse instead of re-parsing the whole text per frame.
    markdown: Entity<Markdown>,
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
        for (ix, (title, note, mounts_streaming, custom_message, default_open)) in
            specs.iter().enumerate()
        {
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
            let markdown = cx.new(|cx| {
                Markdown::new(
                    ("reasoning-body", ix),
                    if seeded {
                        CHUNKS.concat()
                    } else {
                        String::new()
                    },
                )
                // The renderer paints nothing without the theme's style table,
                // so a body that only looks mounted would show as blank space
                // — the one thing this gallery must not demonstrate.
                .theme(Theme::global(cx))
            });
            demos.push(Demo {
                title,
                note,
                state,
                markdown,
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
        self.reasoning[ix]
            .markdown
            .update(cx, |md, cx| md.replace("", cx));
        self.reasoning[ix].state.update(cx, |state, cx| {
            state.set_duration(None, cx);
            // A block mounted mid-run is already streaming; feeding it another
            // rising edge would be a no-op, so only idle demos get one.
            if !mounts_streaming {
                state.set_streaming(true, cx);
            }
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            for chunk in CHUNKS {
                cx.background_executor().timer(CHUNK_DELAY).await;
                let live = this
                    .update(cx, |gallery, cx| gallery.append_chunk(ix, chunk, cx))
                    .is_ok();
                if !live {
                    return;
                }
            }
            let _ = this.update(cx, |gallery, cx| gallery.finish_stream(ix, cx));
        })
        .detach();
    }

    fn append_chunk(&mut self, ix: usize, chunk: &str, cx: &mut Context<Self>) {
        self.reasoning[ix]
            .markdown
            .update(cx, |md, cx| md.append(chunk, cx));
        cx.notify();
    }

    fn finish_stream(&mut self, ix: usize, cx: &mut Context<Self>) {
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

    fn demo_block(&self, ix: usize) -> impl IntoElement {
        let demo = &self.reasoning[ix];
        let mut block =
            Reasoning::new(("reasoning", ix), &demo.state).content(demo.markdown.clone());
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
            .child(self.demo_block(ix))
            .into_any_element()
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

        let rows: Vec<AnyElement> = (0..self.reasoning.len())
            .map(|ix| self.demo_row(ix, muted, border, foreground, cx))
            .collect();

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
                    .child(SharedString::from(
                        "Auto-open while streaming, auto-fold one second after the stream ends \
                         (AUTO_CLOSE_DELAY). A hand toggle pins the block: nothing automatic moves \
                         it afterwards.",
                    )),
            );

        h_flex()
            .size_full()
            .bg(background)
            .child(self.rail(muted, border, foreground, secondary, cx))
            .child(pane)
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
