//! The `AskUserQuestion` takeover card: a paged form — one question per
//! page, radio or checkbox options, a free-text note field per question, and
//! a bottom-left pager (`< 1/3 >`) whose next button becomes the submit check
//! on the last page (`< 3/3 ✓`).
//!
//! The mutable state lives in [`AskState`], the entity the host drives —
//! the same arrangement `Reasoning` and `Tool` use. The card renders from it:
//! options toggle, pages turn, notes type into real input fields, and
//! `advance` past the last page submits, flipping the card to its settled
//! rendering where the host's output element replaces the form — the
//! transcript a later reader sees. A host that owns the answer elsewhere (a
//! conversation writing the ask's answer onto the wire, say) can reroute the
//! submit instead: [`AskUserQuestion::on_submit`] replaces what the pager's
//! check does and keeps the card a working form until that host drops it, and
//! [`AskUserQuestion::on_close`] adds the header's dismissal control.
//!
//! The strings are the host's. Headers, badges ("Recommended"), the note
//! placeholder, the footnote, and the output's markdown all arrive localized;
//! this crate holds no copy of its own. The option indicator's shape follows
//! the block: a radio circle for single-select, a checkbox for multi-select.
//! Indicators are deliberately loud — 18px cells, solid borders in the muted
//! foreground, filled selected states — because a form the user cannot read
//! at a glance is a form they answer wrongly.

use std::rc::Rc;

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, Entity, EventEmitter,
    FontWeight, IntoElement, RenderOnce, SharedString, Window, div, ease_in_out, prelude::*, px,
    radians,
};
use gpui_component::collapsible::Collapsible;
use gpui_component::input::{Input, InputState};
use gpui_component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};

/// The output reveal's motion — the same cadence the tool cards fold with.
const TOGGLE_MOTION: std::time::Duration = std::time::Duration::from_millis(160);

/// Emitted when the ask settles — the host's cue to answer the tool call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AskSubmitted;

/// One selectable answer of one question block.
pub struct AskOption {
    label: SharedString,
    badge: Option<SharedString>,
    description: Option<SharedString>,
}

impl AskOption {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            badge: None,
            description: None,
        }
    }

    /// A short annotation after the label — "Recommended" in the host's
    /// language.
    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// One question: its header, its text, its option set. The block's
/// `multi_select` flag picks both the indicator's shape and the toggle
/// semantics in [`AskState`].
pub struct AskBlock {
    header: Option<SharedString>,
    question: SharedString,
    /// Host-rendered support text under the question — a plan body, a file
    /// excerpt, whatever the ask hangs off the question. The crate renders no
    /// markdown of its own, so the host hands over the element.
    detail: Option<AnyElement>,
    multi_select: bool,
    options: Vec<AskOption>,
}

impl AskBlock {
    pub fn new(question: impl Into<SharedString>) -> Self {
        Self {
            header: None,
            question: question.into(),
            detail: None,
            multi_select: false,
            options: Vec::new(),
        }
    }

    pub fn header(mut self, header: impl Into<SharedString>) -> Self {
        self.header = Some(header.into());
        self
    }

    /// Support text beneath the question (the host's element).
    pub fn detail(mut self, detail: impl IntoElement) -> Self {
        self.detail = Some(detail.into_any_element());
        self
    }

    pub fn multi_select(mut self) -> Self {
        self.multi_select = true;
        self
    }

    pub fn option(mut self, option: AskOption) -> Self {
        self.options.push(option);
        self
    }

    fn multi(&self) -> bool {
        self.multi_select
    }
}

/// Page position, selections, note editors, and the submitted flag for one
/// ask. Construct inside `cx.new` with one `multi` flag per question block,
/// then hand it to [`AskUserQuestion`].
///
/// Note editors are created lazily on first render (an input state needs a
/// window), which is why the placeholder is supplied here rather than on the
/// card.
pub struct AskState {
    page: usize,
    multi: Vec<bool>,
    selections: Vec<Vec<usize>>,
    notes: Vec<Option<Entity<InputState>>>,
    submitted: bool,
    note_placeholder: SharedString,
    /// Whether the settled card's output stands open. The submitted header
    /// toggles it — a settled ask is still a card the reader may fold.
    output_open: bool,
    /// Bumped on every output toggle; the chevron's rotation keys on it.
    output_gen: u64,
}

impl AskState {
    /// `multi` carries one flag per question block, in order.
    pub fn new(multi: Vec<bool>, note_placeholder: impl Into<SharedString>) -> Self {
        let blocks = multi.len();
        Self {
            page: 0,
            multi,
            selections: vec![Vec::new(); blocks],
            notes: vec![None; blocks],
            submitted: false,
            note_placeholder: note_placeholder.into(),
            output_open: true,
            output_gen: 0,
        }
    }

    /// Work one option: single-select blocks replace their selection (or
    /// clear it, if the chosen option is clicked again); multi-select blocks
    /// toggle membership.
    pub fn toggle_option(&mut self, qix: usize, oix: usize, cx: &mut Context<Self>) {
        let selection = &mut self.selections[qix];
        if self.multi[qix] {
            match selection.iter().position(|o| *o == oix) {
                Some(at) => {
                    selection.remove(at);
                }
                None => selection.push(oix),
            }
        } else if selection.as_slice() == [oix] {
            selection.clear();
        } else {
            selection.clear();
            selection.push(oix);
        }
        cx.notify();
    }

    pub fn set_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let page = page.min(self.multi.len().saturating_sub(1));
        if self.page != page {
            self.page = page;
            cx.notify();
        }
    }

    /// The pager's forward edge: turns the page, or reports `true` when this
    /// WAS the last page — the caller then settles the card however it owns the
    /// submit. The click and a host's own keyboard binding share this one rule;
    /// [`AskState::advance`] is this plus the card's own settle.
    pub fn forward(&mut self, cx: &mut Context<Self>) -> bool {
        if self.page + 1 < self.multi.len() {
            self.set_page(self.page + 1, cx);
            false
        } else {
            true
        }
    }

    /// The pager's forward edge, settling the card itself on the last page.
    pub fn advance(&mut self, cx: &mut Context<Self>) {
        if self.forward(cx) {
            self.submit(cx);
        }
    }

    /// Settle the ask from the host side — the pre-settled path a replayed
    /// transcript mounts. The pager's last-page check calls the same thing.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.submitted {
            self.submitted = true;
            cx.emit(AskSubmitted);
            cx.notify();
        }
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        self.set_page(self.page.saturating_sub(1), cx);
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn pages(&self) -> usize {
        self.multi.len()
    }

    pub fn is_selected(&self, qix: usize, oix: usize) -> bool {
        self.selections[qix].contains(&oix)
    }

    pub fn selection(&self, qix: usize) -> &[usize] {
        &self.selections[qix]
    }

    pub fn submitted(&self) -> bool {
        self.submitted
    }

    /// Fold or unfold the settled card's output.
    pub fn toggle_output(&mut self, cx: &mut Context<Self>) {
        self.output_open = !self.output_open;
        self.output_gen = self.output_gen.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn output_open(&self) -> bool {
        self.output_open
    }

    /// One question's note text, as typed.
    pub fn note(&self, qix: usize, cx: &App) -> SharedString {
        self.notes[qix]
            .as_ref()
            .map(|note| note.read(cx).value())
            .unwrap_or_default()
    }

    /// The whole form as the host reads it back at submit time: one
    /// `(selected option indices, note text)` per question, in question order.
    /// A question the user never touched reports an empty pair — the host
    /// settles that as an explicit skip.
    pub fn answers(&self, cx: &App) -> Vec<(Vec<usize>, String)> {
        (0..self.multi.len())
            .map(|qix| (self.selections[qix].clone(), self.note(qix, cx).to_string()))
            .collect()
    }

    /// The question's note editor, mounted on first use. Input state needs a
    /// window, so the card materializes editors during its render — this
    /// method is the crate's internal seam for that, not a host API.
    pub(crate) fn note_editor(
        &mut self,
        qix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(note) = &self.notes[qix] {
            return note.clone();
        }
        let placeholder = self.note_placeholder.clone();
        let note = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        self.notes[qix] = Some(note.clone());
        note
    }
}

impl EventEmitter<AskSubmitted> for AskState {}

/// A click that settles the ask card, as the component takes it.
pub type AskAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// The ask card: header row, the current question's form (or the submitted
/// output), and the pager bar.
#[derive(IntoElement)]
pub struct AskUserQuestion {
    id: SharedString,
    state: Entity<AskState>,
    icon: Option<AnyElement>,
    name: Option<SharedString>,
    status: Option<AnyElement>,
    blocks: Vec<AskBlock>,
    footnote: Option<SharedString>,
    output: Option<AnyElement>,
    on_submit: Option<AskAction>,
    on_close: Option<AskAction>,
}

impl AskUserQuestion {
    pub fn new(id: impl Into<SharedString>, state: &Entity<AskState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            icon: None,
            name: None,
            status: None,
            blocks: Vec::new(),
            footnote: None,
            output: None,
            on_submit: None,
            on_close: None,
        }
    }

    /// The header's glyph. No default: a host that omits it starts the
    /// header row at the name.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// The header row's tool name.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The header row's right-hand slot — the "waiting for your answer"
    /// chip, or whatever the host's ask state says.
    pub fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    pub fn block(mut self, block: AskBlock) -> Self {
        self.blocks.push(block);
        self
    }

    /// Every block at once, in page order.
    pub fn blocks(mut self, blocks: Vec<AskBlock>) -> Self {
        self.blocks = blocks;
        self
    }

    /// The pager bar's right-hand hint.
    pub fn footnote(mut self, footnote: impl Into<SharedString>) -> Self {
        self.footnote = Some(footnote.into());
        self
    }

    /// The settled rendering — the host's summary of the answers (typically
    /// a markdown element), shown in place of the form once submitted.
    pub fn output(mut self, output: impl IntoElement) -> Self {
        self.output = Some(output.into_any_element());
        self
    }

    /// Replaces what the pager's last-page check does. The default settles the
    /// card's own [`AskState`] (`advance` → `submit`); a host that owns the
    /// answer elsewhere routes the click to itself and settles the card by
    /// dropping it — the state stays a working form until then.
    pub fn on_submit(mut self, on_submit: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_submit = Some(Rc::new(on_submit));
        self
    }

    /// The header's dismissal control — rendered while the card is
    /// unsettled, because a settled card is a transcript row rather than a
    /// surface the user can leave. No default: a host that never lets the ask
    /// go simply omits it.
    pub fn on_close(mut self, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }

    /// The option indicator. Loud by design: an 18px cell with a solid
    /// border in the muted foreground (not a washed accent), a filled accent
    /// dot for a chosen radio, an accent-filled checkbox with a light check
    /// for a chosen multi option.
    fn indicator(selected: bool, multi: bool, accent: gpui::Hsla, muted: gpui::Hsla) -> AnyElement {
        let frame = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .w(px(18.))
            .h(px(18.))
            .border_1()
            .border_color(if selected { accent } else { muted })
            .when(multi, |frame| frame.rounded(px(4.)))
            .when(!multi, |frame| frame.rounded_full());
        if multi {
            frame
                .when(selected, |frame| {
                    frame.bg(accent).child(
                        div()
                            .text_color(gpui::white())
                            .child(Icon::new(IconName::Check).xsmall()),
                    )
                })
                .into_any_element()
        } else {
            frame
                .when(selected, |frame| {
                    frame.child(div().w(px(10.)).h(px(10.)).rounded_full().bg(accent))
                })
                .into_any_element()
        }
    }
}

impl RenderOnce for AskUserQuestion {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (background, muted, foreground, border) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.muted_foreground,
                theme.foreground,
                theme.border,
            )
        };
        let block_count = self.blocks.len();
        let AskUserQuestion {
            id,
            state,
            icon,
            name,
            status,
            blocks,
            footnote,
            output,
            on_submit,
            on_close,
        } = self;
        // `AnyElement` is not `Clone`, so the output moves into the settled
        // branch and the form never sees it; a block's detail element is
        // likewise consumed by whichever body renders it.
        let has_output = output.is_some();
        let mut ask = AskUserQuestion {
            id: id.clone(),
            state: state.clone(),
            icon: None,
            name: None,
            status: None,
            blocks,
            footnote: footnote.clone(),
            output: None,
            on_submit: None,
            on_close: None,
        };
        let state_ref = state.read(cx);
        // The state's page is clamped against the blocks this card received:
        // a host can construct a state with more pages than it mounts blocks
        // for, and a published component must not index on faith.
        let page = state_ref.page().min(block_count.saturating_sub(1));
        let (pages, submitted, output_open, output_gen) = (
            state_ref.pages(),
            state_ref.submitted(),
            state_ref.output_open(),
            state_ref.output_gen,
        );
        debug_assert!(
            pages <= block_count,
            "AskState has {pages} pages but the card mounts {block_count} blocks"
        );
        let settled = submitted && has_output;

        let base_header = h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .items_center()
            .children(icon)
            .child(
                div()
                    .text_sm()
                    .text_color(foreground)
                    .child(name.unwrap_or_else(|| SharedString::from("AskUserQuestion"))),
            )
            .child(div().flex_1())
            .children(status);
        // The dismissal control rides the header's trailing edge, level with
        // the status chip it sits beside.
        let close: Option<AnyElement> = on_close.map(|on_close| {
            pager_button(
                (ElementId::Name(id.clone()), "close"),
                Icon::new(IconName::Close).xsmall().into_any_element(),
                muted,
                true,
                move |_, window, cx| on_close(window, cx),
            )
            .into_any_element()
        });
        // The settled header owns the output's fold: clicking it (or working
        // it on the keyboard) toggles, and the trailing chevron says so.
        let header: AnyElement = if settled {
            let state_for_toggle = state.clone();
            let header_id = (ElementId::Name(id.clone()), "header");
            let chevron = Icon::new(IconName::ChevronDown).xsmall().with_animation(
                ElementId::Name(format!("{id:?}-chevron-g{output_gen}").into()),
                Animation::new(TOGGLE_MOTION).with_easing(ease_in_out),
                move |icon, t| {
                    let sweep = if output_open { t } else { 1.0 - t };
                    icon.rotate(radians(std::f32::consts::PI * sweep))
                },
            );
            base_header
                .id(header_id)
                .cursor_pointer()
                .hover(move |row| row.text_color(foreground))
                .on_click(move |_, _window, cx| {
                    state_for_toggle.update(cx, |state, cx| state.toggle_output(cx))
                })
                .text_color(muted)
                .child(chevron)
                .into_any_element()
        } else {
            // A settled card is a transcript row, so the dismissal control
            // belongs to the form alone (see `on_close`).
            base_header.children(close).into_any_element()
        };

        let body: AnyElement = if submitted {
            // The settled card: the host's output replaces the form. A card
            // without an output slot keeps the last page's form, frozen.
            match output {
                Some(output) => output,
                None => frozen_form(&mut ask, page, cx),
            }
        } else {
            form_page(&mut ask, page, window, cx)
        };

        // The pager: prev, `n/N`, then next — the submit check on the last
        // page, the same plain glyph control as the turn chevrons. Not shown
        // on the settled card; the output is not a form. The check settles
        // through `on_submit` when the host owns the answer, and through the
        // card's own state otherwise.
        let pager = (!submitted && pages > 0).then(|| {
            let state_for_prev = state.clone();
            let submit = page + 1 == pages;
            let next = {
                let state_for_next = state.clone();
                let on_submit = on_submit.clone();
                let glyph = if submit {
                    Icon::new(IconName::Check).xsmall().into_any_element()
                } else {
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .into_any_element()
                };
                pager_button(
                    (ElementId::Name(id.clone()), "next"),
                    glyph,
                    muted,
                    true,
                    move |_, window, cx| {
                        if submit && let Some(on_submit) = on_submit.as_ref() {
                            on_submit(window, cx);
                            return;
                        }
                        state_for_next.update(cx, |state, cx| state.advance(cx));
                    },
                )
            };
            h_flex()
                .gap_1()
                .items_center()
                .child(pager_button(
                    (ElementId::Name(id.clone()), "prev"),
                    Icon::new(IconName::ChevronLeft).xsmall().into_any_element(),
                    muted,
                    page > 0,
                    move |_, _window, cx| state_for_prev.update(cx, |state, cx| state.go_back(cx)),
                ))
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::from(format!("{}/{}", page + 1, pages))),
                )
                .child(next)
        });

        let footnote_row = |footnote: SharedString| {
            div()
                .text_xs()
                .text_color(muted)
                .pt_2()
                .border_t_1()
                .border_color(border.opacity(0.5))
                .child(footnote)
        };

        if settled {
            // One collapsible: the header is the trigger, the divider, the
            // output, and the footnote fold with it.
            let card = Collapsible::new()
                .open(output_open)
                .motion_id((ElementId::Name(id.clone()), "output"))
                .child(header)
                .content(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .child(div().h_px().w_full().bg(border.opacity(0.5)))
                        .child(body)
                        .children(footnote.map(footnote_row)),
                );
            return v_flex()
                .w_full()
                .min_w_0()
                .p_3()
                .bg(background)
                .border_1()
                .border_color(border)
                .rounded(px(8.))
                .child(card);
        }

        v_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .p_3()
            .bg(background)
            .border_1()
            .border_color(border)
            .rounded(px(8.))
            .child(header)
            .child(div().h_px().w_full().bg(border.opacity(0.5)))
            .child(body)
            .when_some(pager, |card, pager| {
                card.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .pt_2()
                        .border_t_1()
                        .border_color(border.opacity(0.5))
                        .child(pager)
                        .child(div().flex_1())
                        .children(
                            footnote
                                .clone()
                                .map(|footnote| div().text_xs().text_color(muted).child(footnote)),
                        ),
                )
            })
            .when(submitted, |card| {
                card.children(footnote.clone().map(footnote_row))
            })
    }
}

/// One pager control: a plain glyph, styled exactly like its siblings — no
/// hover wash, no fill, the press dim is the only feedback. The submit check
/// is the same control with a check glyph; a disabled arm dims out.
fn pager_button(
    id: impl Into<ElementId>,
    glyph: AnyElement,
    muted: gpui::Hsla,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .px_1()
        .py_0p5()
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .on_click(on_click)
                .active(move |button| button.opacity(0.55))
        })
        .when(!enabled, |button| button.opacity(0.35))
        .child(div().text_color(muted).child(glyph))
}

/// The current page's form: the question, its options, and the note field.
fn form_page(
    ask: &mut AskUserQuestion,
    page: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if ask.blocks.is_empty() {
        return v_flex().w_full().min_w_0().into_any_element();
    }
    let theme = cx.theme();
    let (muted, foreground, secondary, accent) = (
        theme.muted_foreground,
        theme.foreground,
        theme.secondary,
        theme.accent,
    );
    let block = &mut ask.blocks[page];
    let multi = block.multi();

    let note = ask
        .state
        .update(cx, |state, cx| state.note_editor(page, window, cx));

    let options =
        block
            .options
            .iter()
            .enumerate()
            .map(|(oix, option)| {
                let selected = ask.state.read(cx).is_selected(page, oix);
                let ask_state = ask.state.clone();
                h_flex()
                    .id((ask.id.clone(), page * 64 + oix))
                    .w_full()
                    .min_w_0()
                    .gap_2p5()
                    .items_start()
                    .px_2()
                    .py_2()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(if selected {
                        accent
                    } else {
                        gpui::transparent_black()
                    })
                    .when(selected, |row| row.bg(accent.opacity(0.06)))
                    .hover(move |row| row.bg(secondary))
                    .on_click(move |_, _window, cx| {
                        ask_state.update(cx, |state, cx| state.toggle_option(page, oix, cx));
                    })
                    .child(AskUserQuestion::indicator(selected, multi, accent, muted))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(option.label.clone()),
                                    )
                                    .children(option.badge.clone().map(|badge| {
                                        div().text_xs().text_color(muted).child(badge)
                                    })),
                            )
                            .children(option.description.clone().map(|description| {
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .pl(px(30.))
                                    .child(description)
                            })),
                    )
            })
            .collect::<Vec<_>>();

    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .children(block.header.clone().map(|header| {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(header)
        }))
        .child(
            div()
                .text_base()
                .text_color(foreground)
                .child(block.question.clone()),
        )
        .children(block.detail.take())
        .children(options)
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .pl(px(30.))
                .child(Input::new(&note)),
        )
        .into_any_element()
}

/// The frozen last page — the fallback for a submitted card whose host gave
/// no output element.
fn frozen_form(ask: &mut AskUserQuestion, page: usize, cx: &App) -> AnyElement {
    if ask.blocks.is_empty() {
        return v_flex().w_full().min_w_0().into_any_element();
    }
    let theme = cx.theme();
    let (muted, foreground, accent) = (theme.muted_foreground, theme.foreground, theme.accent);
    let state = ask.state.read(cx);
    let block = &mut ask.blocks[page];
    v_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .children(block.header.clone().map(|header| {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(muted)
                .child(header)
        }))
        .child(
            div()
                .text_base()
                .text_color(foreground)
                .child(block.question.clone()),
        )
        .children(block.detail.take())
        .children(block.options.iter().enumerate().map(|(oix, option)| {
            let selected = state.is_selected(page, oix);
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2p5()
                .items_start()
                .px_2()
                .py_2()
                .rounded(px(6.))
                .border_1()
                .border_color(if selected {
                    accent
                } else {
                    gpui::transparent_black()
                })
                .when(selected, |row| row.bg(accent.opacity(0.06)))
                .when(!selected, |row| row.opacity(0.55))
                .child(AskUserQuestion::indicator(
                    selected,
                    block.multi(),
                    accent,
                    muted,
                ))
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_baseline()
                        .child(
                            div()
                                .text_sm()
                                .text_color(foreground)
                                .child(option.label.clone()),
                        )
                        .children(
                            option
                                .badge
                                .clone()
                                .map(|badge| div().text_xs().text_color(muted).child(badge)),
                        ),
                )
        }))
        .into_any_element()
}
