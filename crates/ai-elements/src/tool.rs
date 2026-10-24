//! One tool invocation as a card: an always-visible header row (icon, tool
//! name, argument summary, status) over a collapsible body.
//!
//! Mirrors Vercel AI Elements' `Tool` / `ToolHeader` / `ToolContent` triple:
//! the frame is generic — every tool family (file editors, shells, prompts)
//! supplies its own body — while the header keeps the invocation's identity
//! and outcome on screen whether the body is folded or not. Upstream's
//! `ToolContent` is a styled body wrapper; here its framing (the card's
//! content rule and debug selector) lives inside [`Tool::content`], so hosts
//! hand over the body element itself. As with [`crate::Reasoning`], the
//! mutable state lives in an entity ([`ToolState`]) the host drives; the
//! elements are stateless.
//!
//! Lifecycle contract: a tool mounts closed and idle. `set_status(Running)`
//! opens the body; settling it into `Succeeded`/`Failed` pins the elapsed
//! time and — unless the host disabled it with [`ToolState::no_auto_fold`] —
//! folds the body one second later, the way a transcript keeps finished work
//! compact. A hand toggle (mouse or keyboard) pins the card open or shut:
//! nothing automatic moves it afterwards.

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, Entity, EventEmitter,
    FocusHandle, FontWeight, Hsla, IntoElement, RenderOnce, SharedString, Task, Window, div,
    ease_in_out, ease_out_quint, prelude::*, px, radians, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, ThemeStyled as _, collapsible::Collapsible,
    h_flex, v_flex,
};

use crate::animation::toggle_key;
use crate::spinner::BrailleSpinner;

/// How long a settled card stays open before folding itself.
pub const AUTO_FOLD_DELAY: Duration = Duration::from_millis(1000);

/// Chevron rotation and body fade/slide timing — the same cadence
/// [`crate::Reasoning`] toggles with, so the two card kinds read as one family.
const TOGGLE_MOTION: Duration = Duration::from_millis(160);

/// How long a copy chip shows its check before falling back to the copy
/// glyph — the transcript-wide copy-feedback cadence.
const COPY_FLASH: Duration = Duration::from_millis(1200);

/// Which copy chip of a card flashed. Ordered as the flash state's slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopySide {
    In,
    Out,
    Diff,
}

impl CopySide {
    fn as_index(self) -> usize {
        match self {
            Self::In => 0,
            Self::Out => 1,
            Self::Diff => 2,
        }
    }
}

/// The slide distance of [`crate::Reasoning`]'s body reveal.
const BODY_SLIDE_REMS: f32 = 0.5;

/// Emitted when the open state or the status changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEvent {
    OpenChanged { open: bool },
    StatusChanged(ToolStatus),
}

/// Where an invocation stands. The header renders each arm differently;
/// `Running` also owns the duration clock inside [`ToolState`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ToolStatus {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
}

/// Open state, status, duration clock, and delayed fold for one tool card.
///
/// Construct inside `cx.new`, hand it to [`Tool`], and drive it with
/// [`ToolState::set_status`] as the tool call progresses. A host that keeps
/// its own timing reports it with [`ToolState::set_duration`] instead of
/// letting the card measure.
pub struct ToolState {
    status: ToolStatus,
    is_open: bool,
    explicitly_closed: bool,
    user_toggled: bool,
    auto_fold: Option<Duration>,
    has_auto_folded: bool,
    started_at: Option<Instant>,
    duration: Option<Duration>,
    toggle_gen: u64,
    /// Per copy chip: whether its check is up (the 1.2 s flash after a
    /// copy lands on the clipboard).
    copy_flashed: [bool; 3],
    focus_handle: FocusHandle,
    _fold_task: Option<Task<()>>,
    _copy_flash_tasks: [Option<Task<()>>; 3],
}

impl ToolState {
    /// A closed, idle card.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            status: ToolStatus::Pending,
            is_open: false,
            explicitly_closed: false,
            user_toggled: false,
            auto_fold: Some(AUTO_FOLD_DELAY),
            has_auto_folded: false,
            started_at: None,
            duration: None,
            toggle_gen: 0,
            copy_flashed: [false; 3],
            focus_handle: cx.focus_handle(),
            _fold_task: None,
            _copy_flash_tasks: [None, None, None],
        }
    }

    /// The status the card mounts with. Mounting `Running` starts the clock
    /// and opens the body.
    pub fn with_status(mut self, status: ToolStatus, cx: &mut Context<Self>) -> Self {
        self.apply_status(status, cx);
        self
    }

    /// Disables the settle-fold: a card whose body the host wants to leave
    /// open (an editor preview the user is reading, say).
    pub fn no_auto_fold(mut self) -> Self {
        self.auto_fold = None;
        self
    }

    pub fn set_status(&mut self, status: ToolStatus, cx: &mut Context<Self>) {
        self.apply_status(status, cx);
        cx.notify();
    }

    fn apply_status(&mut self, status: ToolStatus, cx: &mut Context<Self>) {
        if self.status == status {
            return;
        }
        self.status = status.clone();
        match status {
            ToolStatus::Running => {
                // A fold an earlier settle scheduled is called off by the
                // tool running again — dropping the task is the cancellation.
                self._fold_task = None;
                self.has_auto_folded = false;
                if self.started_at.is_none() {
                    self.started_at = Some(cx.background_executor().now());
                }
                if !self.is_open && !self.explicitly_closed && !self.user_toggled {
                    self.set_open_inner(true, cx);
                }
            }
            ToolStatus::Succeeded | ToolStatus::Failed => {
                if let Some(started) = self.started_at.take() {
                    self.duration = Some(
                        cx.background_executor()
                            .now()
                            .saturating_duration_since(started),
                    );
                }
                if self.is_open
                    && !self.user_toggled
                    && !self.has_auto_folded
                    && let Some(delay) = self.auto_fold
                {
                    self._fold_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(delay).await;
                        let _ = this.update(cx, |state, cx| state.fold_after_settle(cx));
                    }));
                }
            }
            ToolStatus::Pending => {}
        }
        cx.emit(ToolEvent::StatusChanged(status));
    }

    fn fold_after_settle(&mut self, cx: &mut Context<Self>) {
        if !self.user_toggled && self.is_open {
            self.has_auto_folded = true;
            self.set_open_inner(false, cx);
        }
        cx.notify();
    }

    /// A host-measured elapsed time, overriding the one the card clocks from
    /// its own running edges.
    pub fn set_duration(&mut self, duration: Option<Duration>, cx: &mut Context<Self>) {
        self.duration = duration;
        cx.notify();
    }

    /// Closes (or opens) the card programmatically. Closing pins it shut: a
    /// later run cannot re-open a card the host deliberately closed.
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.explicitly_closed = !open;
        self.set_open_inner(open, cx);
    }

    /// The trigger's toggle, as driven by a click or key: the card becomes
    /// user-owned, and nothing automatic moves it afterwards.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.user_toggled = true;
        let open = !self.is_open;
        self.explicitly_closed = !open;
        self.set_open_inner(open, cx);
    }

    fn set_open_inner(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.is_open == open {
            return;
        }
        self.is_open = open;
        self.toggle_gen = self.toggle_gen.wrapping_add(1);
        cx.emit(ToolEvent::OpenChanged { open });
        cx.notify();
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    pub fn status(&self) -> &ToolStatus {
        &self.status
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Record a landed copy: the chip's check rises for [`COPY_FLASH`]. A
    /// repeat copy inside the window restarts it — replacing the task is the
    /// cancellation.
    pub(crate) fn flash_copy(&mut self, side: CopySide, cx: &mut Context<Self>) {
        let ix = side.as_index();
        self.copy_flashed[ix] = true;
        self._copy_flash_tasks[ix] = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPY_FLASH).await;
            let _ = this.update(cx, |state, cx| {
                state.copy_flashed[ix] = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn is_copy_flashed(&self, side: CopySide) -> bool {
        self.copy_flashed[side.as_index()]
    }
}

impl EventEmitter<ToolEvent> for ToolState {}

/// A duration as the header renders it: tenths under a minute, minutes and
/// seconds above.
pub fn format_tool_duration(duration: Duration) -> SharedString {
    let secs = duration.as_secs_f32();
    if secs < 60. {
        SharedString::from(format!("{secs:.1}s"))
    } else {
        SharedString::from(format!(
            "{}m{:02}s",
            duration.as_secs() / 60,
            duration.as_secs() % 60
        ))
    }
}

/// The clickable header row: icon, tool name, argument summary, status chip,
/// disclosure chevron. Also the default header under [`Tool`]; public so a
/// host can place it standalone. The debug selectors it exposes are
/// `{id}-header` on the row.
/// The header's toggle, as the host drives it. Hosts that own the fold
/// elsewhere (a conversation mirroring its own collapse state into
/// [`ToolState`]) override the click with this; `None` — the default —
/// toggles the card's own state.
pub type ToolToggle = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ToolHeader {
    id: ElementId,
    state: Entity<ToolState>,
    icon: Option<AnyElement>,
    name: Option<SharedString>,
    args: Option<AnyElement>,
    status: Option<AnyElement>,
    on_toggle: Option<ToolToggle>,
}

impl ToolHeader {
    pub fn new(id: impl Into<ElementId>, state: &Entity<ToolState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            icon: None,
            name: None,
            args: None,
            status: None,
            on_toggle: None,
        }
    }

    /// Replaces what a header click does. The card's own `ToolState` stays
    /// the render source; this only reroutes the gesture.
    pub fn on_toggle(mut self, on_toggle: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(on_toggle));
        self
    }

    /// The leading glyph — the tool family's mark. No default: a host that
    /// omits it starts the row at the name.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// The tool's name, in the UI font at medium weight.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The invocation summary — file name, path, arguments. Rendered in the
    /// mono font, muted, and clipped to what remains of the row.
    pub fn args(mut self, args: impl IntoElement) -> Self {
        self.args = Some(args.into_any_element());
        self
    }

    /// Replaces the default status chip.
    pub fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    /// The default status chip: a muted ring while pending, the crate's
    /// braille spinner while running, a check with the duration on success,
    /// a cross with the duration on failure.
    fn default_status(status: &ToolStatus, duration: Option<Duration>, tint: Hsla) -> AnyElement {
        match status {
            ToolStatus::Pending => div()
                .size_2()
                .rounded_full()
                .border_1()
                .border_color(tint.opacity(0.5))
                .into_any_element(),
            ToolStatus::Running => BrailleSpinner::new()
                .xsmall()
                .color(tint)
                .into_any_element(),
            ToolStatus::Succeeded => h_flex()
                .gap_1()
                .text_color(tint)
                .child(Icon::new(IconName::CircleCheck).xsmall())
                .child(
                    div().text_xs().child(
                        duration
                            .map(format_tool_duration)
                            .unwrap_or_else(|| "done".into()),
                    ),
                )
                .into_any_element(),
            ToolStatus::Failed => h_flex()
                .gap_1()
                .text_color(tint)
                .child(Icon::new(IconName::CircleX).xsmall())
                .child(
                    div().text_xs().child(
                        duration
                            .map(format_tool_duration)
                            .unwrap_or_else(|| "failed".into()),
                    ),
                )
                .into_any_element(),
        }
    }
}

impl RenderOnce for ToolHeader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.read(cx);
        let (is_open, status, duration, toggle_gen) = (
            state.is_open,
            &state.status,
            state.duration,
            state.toggle_gen,
        );
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);

        let tint = match status {
            ToolStatus::Pending => muted,
            ToolStatus::Running => muted,
            ToolStatus::Succeeded => theme.success,
            ToolStatus::Failed => theme.danger,
        };
        let status = self
            .status
            .unwrap_or_else(|| Self::default_status(status, duration, tint));

        let click_state = self.state.clone();
        let on_toggle = self.on_toggle.clone();
        let chevron = Icon::new(IconName::ChevronDown)
            .xsmall()
            .with_animation(
                toggle_key(&self.id, "chevron", toggle_gen),
                Animation::new(TOGGLE_MOTION).with_easing(ease_in_out),
                move |icon, t| {
                    let sweep = if is_open { t } else { 1.0 - t };
                    icon.rotate(radians(std::f32::consts::PI * sweep))
                },
            )
            .into_any_element();

        let focus_handle = self.state.read(cx).focus_handle.clone();

        h_flex()
            .id(self.id.clone())
            .debug_selector({
                let id = self.id.clone();
                move || format!("{id}-header")
            })
            .track_focus(&focus_handle)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .gap_2()
            .px_3()
            .py_2()
            .items_center()
            .text_sm()
            .text_color(muted)
            .cursor_pointer()
            .hover(move |row| row.text_color(foreground))
            .when(focus_handle.is_focused(window), |row| {
                row.focus_ring_style(window, cx)
            })
            .on_click(move |_, window, cx| match &on_toggle {
                Some(on_toggle) => on_toggle(window, cx),
                None => click_state.update(cx, |state, cx| state.toggle(cx)),
            })
            .children(self.icon)
            .when_some(self.name, |row, name| {
                row.child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(foreground)
                        .child(name),
                )
            })
            .child(
                // The argument summary flexes and clips; the caller's element
                // is expected to be a single row of mono text.
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .overflow_hidden()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .children(self.args),
            )
            .child(status)
            .child(chevron)
    }
}

/// One tool invocation: a [`ToolHeader`] over a collapsible body. The body is
/// whatever the host renders — the crate ships editor and shell surfaces as
/// siblings of this frame.
///
/// The body may carry copy controls, always visible at its top-right corner:
/// `in` copies the call's quotable input — the raw argument structure, or the
/// host's chosen shape where re-running matters (the app quotes a Bash
/// command) — and `out` its raw output: the machine shape, as opposed to
/// selecting the rendered text, which copies what the card shows. An
/// `open_diff` chip rides the same row: a card whose body is a capped view of
/// something the host can show in full hands the expansion to a callback (the
/// app opens the right pane's diff tab) instead of growing the transcript.
#[derive(IntoElement)]
pub struct Tool {
    id: ElementId,
    state: Entity<ToolState>,
    header: Option<AnyElement>,
    icon: Option<AnyElement>,
    name: Option<SharedString>,
    args: Option<AnyElement>,
    status: Option<AnyElement>,
    content: Option<AnyElement>,
    copy_input: Option<SharedString>,
    copy_output: Option<SharedString>,
    copy_diff: Option<SharedString>,
    open_diff: Option<(SharedString, ToolToggle)>,
    on_toggle: Option<ToolToggle>,
    animated: bool,
}

impl Tool {
    pub fn new(id: impl Into<ElementId>, state: &Entity<ToolState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            header: None,
            icon: None,
            name: None,
            args: None,
            status: None,
            content: None,
            copy_input: None,
            copy_output: None,
            copy_diff: None,
            open_diff: None,
            on_toggle: None,
            animated: true,
        }
    }

    /// Replaces the whole header row.
    pub fn header(mut self, header: impl IntoElement) -> Self {
        self.header = Some(header.into_any_element());
        self
    }

    /// The default header's leading glyph.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// The default header's tool name.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The default header's argument summary.
    pub fn args(mut self, args: impl IntoElement) -> Self {
        self.args = Some(args.into_any_element());
        self
    }

    /// The default header's status chip.
    pub fn status(mut self, status: impl IntoElement) -> Self {
        self.status = Some(status.into_any_element());
        self
    }

    /// The body. Rendered only while the card is open (or animating shut).
    pub fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    /// The text the body's `in` copy control quotes: the call's raw argument
    /// structure, or the host's chosen shape where re-running matters (the
    /// app quotes a Bash command). The rendered body is for reading; this is
    /// for quoting the call.
    pub fn copy_input(mut self, raw: impl Into<SharedString>) -> Self {
        self.copy_input = Some(raw.into());
        self
    }

    /// The tool call's raw output, offered by the body's `out` copy control.
    pub fn copy_output(mut self, raw: impl Into<SharedString>) -> Self {
        self.copy_output = Some(raw.into());
        self
    }

    /// The invocation's unified diff, offered by the body's `diff` copy
    /// control — the edit family's quotable shape, instead of quoting the
    /// input JSON.
    pub fn copy_diff(mut self, raw: impl Into<SharedString>) -> Self {
        self.copy_diff = Some(raw.into());
        self
    }

    /// An extra body chip next to the copy controls: `label` (host-localized
    /// vocabulary — the crate injects no copy) on a chip whose click hands
    /// `on_open` the window and app. The full-view counterpart to the copy
    /// chips: the body shows the capped shape, the callback the whole thing.
    pub fn open_diff(
        mut self,
        label: impl Into<SharedString>,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.open_diff = Some((label.into(), std::rc::Rc::new(on_open)));
        self
    }

    /// The default header's toggle override — see [`ToolHeader::on_toggle`].
    pub fn on_toggle(mut self, on_toggle: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(on_toggle));
        self
    }

    /// Whether opening and closing animate. An animated body stays mounted
    /// while closed — that is what makes the reveal reversible — so a host
    /// that virtualizes long transcripts may prefer `animated(false)`, which
    /// drops a closed body entirely.
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }
}

impl RenderOnce for Tool {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Tool {
            id,
            state,
            header,
            icon,
            name,
            args,
            status,
            content,
            copy_input,
            copy_output,
            copy_diff,
            open_diff,
            on_toggle,
            animated,
        } = self;

        let (is_open, toggle_gen) = {
            let state = state.read(cx);
            (state.is_open, state.toggle_gen)
        };

        let header = header.unwrap_or_else(|| {
            let mut header = ToolHeader::new(id.clone(), &state);
            if let Some(icon) = icon {
                header = header.icon(icon);
            }
            if let Some(name) = name {
                header = header.name(name);
            }
            if let Some(args) = args {
                header = header.args(args);
            }
            if let Some(status) = status {
                header = header.status(status);
            }
            if let Some(on_toggle) = on_toggle {
                header = header.on_toggle(move |window, cx| on_toggle(window, cx));
            }
            header.into_any_element()
        });

        let body = content.map(|body| {
            // The copy controls float over the body's top-right corner, always
            // visible — same chrome, no hover reveal; a discovery affordance
            // that hides is one a failing command's user cannot find. A click
            // presses, lands on the clipboard, and the glyph flashes to a
            // check for COPY_FLASH.
            let controls = (copy_input.is_some()
                || copy_output.is_some()
                || copy_diff.is_some()
                || open_diff.is_some())
            .then(|| {
                h_flex()
                    .absolute()
                    .top_1()
                    .right_1()
                    .gap_1()
                    .children(copy_input.map(|raw| {
                        copy_chip((id.clone(), "copy-in"), "in", CopySide::In, raw, &state, cx)
                    }))
                    .children(copy_output.map(|raw| {
                        copy_chip(
                            (id.clone(), "copy-out"),
                            "out",
                            CopySide::Out,
                            raw,
                            &state,
                            cx,
                        )
                    }))
                    .children(copy_diff.map(|raw| {
                        copy_chip(
                            (id.clone(), "copy-diff"),
                            "diff",
                            CopySide::Diff,
                            raw,
                            &state,
                            cx,
                        )
                    }))
                    .children(open_diff.map(|(label, on_open)| {
                        open_chip((id.clone(), "open-diff"), label, on_open, cx)
                    }))
            });
            let wrapper = div()
                .w_full()
                .min_w_0()
                .relative()
                .border_t_1()
                .border_color(cx.theme().border)
                .debug_selector({
                    let id = id.clone();
                    move || format!("{id}-content")
                })
                .child(body)
                .children(controls);
            if animated {
                wrapper
                    .with_animation(
                        toggle_key(&id, "fade", toggle_gen),
                        Animation::new(TOGGLE_MOTION).with_easing(ease_out_quint()),
                        move |body, t| {
                            let t = if is_open { t } else { 1.0 - t };
                            body.opacity(t)
                                .relative()
                                .top(rems(-BODY_SLIDE_REMS * (1.0 - t)))
                        },
                    )
                    .into_any_element()
            } else {
                wrapper.into_any_element()
            }
        });

        let mut collapsible = Collapsible::new().open(is_open).child(header);
        if animated {
            collapsible = collapsible.motion_id((id.clone(), "reveal"));
        }
        let card = match body {
            Some(body) => collapsible.content(body).into_any_element(),
            None => collapsible.into_any_element(),
        };

        // The card frame: the sketch's rules above and below the header. The
        // body surfaces (editor, shell) carry their own backgrounds and clip
        // to the frame's corners through `overflow_hidden`.
        v_flex()
            .w_full()
            .min_w_0()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(8.))
            .overflow_hidden()
            .child(card)
    }
}

/// One raw-structure copy chip: the copy glyph and which side of the call it
/// quotes. The look is static — no hover restyle, the chip is chrome, not a
/// moving part. Clicking presses, writes `raw` to the system clipboard, and
/// the glyph flashes to a check for [`COPY_FLASH`] — the copy-feedback
/// cadence every other copy control in the transcript follows.
fn copy_chip(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    side: CopySide,
    raw: SharedString,
    state: &Entity<ToolState>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let (muted, border, background, success, mono) = (
        theme.muted_foreground,
        theme.border,
        theme.background,
        theme.success,
        theme.mono_font_family.clone(),
    );
    let flashed = state.read(cx).is_copy_flashed(side);
    let glyph_color = if flashed { success } else { muted };
    let glyph = if flashed {
        Icon::new(IconName::Check).xsmall()
    } else {
        Icon::new(IconName::Copy).xsmall()
    };
    let state_for_click = state.clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded(px(5.))
        .bg(background.opacity(0.85))
        .border_1()
        .border_color(border)
        .cursor_pointer()
        .active(move |chip| chip.opacity(0.7))
        .on_click(move |_, _window, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(raw.to_string()));
            state_for_click.update(cx, |state, cx| state.flash_copy(side, cx));
        })
        .child(div().text_color(glyph_color).child(glyph))
        .child(
            div()
                .text_xs()
                .font_family(mono)
                .text_color(muted)
                .child(label),
        )
}

/// The full-view chip: the copy chips' chrome (no flash — nothing landed), the
/// click routed to the host's open callback instead of the clipboard. `label`
/// is host-localized text; the glyph names the destination, not the action.
fn open_chip(
    id: impl Into<gpui::ElementId>,
    label: SharedString,
    on_open: ToolToggle,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded(px(5.))
        .bg(theme.background.opacity(0.85))
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .active(|chip| chip.opacity(0.7))
        .on_click(move |_, window, cx| on_open(window, cx))
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::PanelRight).xsmall()),
        )
        .child(
            div()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(label),
        )
}
