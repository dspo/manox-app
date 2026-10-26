//! The composer + overlay render helpers (U9b cluster 8): the composer
//! element (and its queued-follow-ups / access-switch / send-button /
//! completion-overlay / attachments / project-chip faces), the browser-suite
//! toggles, the blank-project family, and the project registration leg.
//! Split from `workspace.rs` — `super` is the workspace module; the
//! bare-private methods lift `pub(super)` for the parent's render face and
//! the `tests` child.

use super::*;
use crate::DismissContextBubble;
use crate::cockpit::{context_budget_pct, format_tokens_pi};
use crate::views::context_rail::{BUBBLE_MAX_W, BUBBLE_MIN_W, CONTEXT_NEAR_FULL_PCT, ContextRail};
use crate::views::popup_menu::MAX_LIST_HEIGHT;
use gpui::{
    Anchor, Focusable as _, Hsla, PathBuilder, PathStyle, Point, StrokeOptions, canvas, point,
};
use gpui_component::ThemeStyled as _;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::popover::Popover;

/// The context pill as a [`Popover`] trigger. `Selectable` is the library's
/// hook for pushing open-state styling into its own components; the pill has
/// no open-state styling of its own, so the impl is a no-op.
struct ContextPillTrigger(gpui::AnyElement);

impl gpui::IntoElement for ContextPillTrigger {
    type Element = gpui::AnyElement;
    fn into_element(self) -> Self::Element {
        self.0
    }
}

impl gpui_component::Selectable for ContextPillTrigger {
    fn selected(self, _: bool) -> Self {
        self
    }
    fn is_selected(&self) -> bool {
        false
    }
}
use lyon::tessellation::LineCap;
pub use steer_agent_chat_ui::column::{QueueDragEdge, QueueRowDrag};

// ── Context-usage ring geometry ───────────────────────────────────────────
// The dsh ContextMeter's numbers verbatim: a 14px box, r 5.5, 2px stroke —
// `r + stroke/2 = 6.5` exactly fills the 7px half-box, so the ring kisses
// the canvas edge like the reference SVG.

/// Canvas (and ring) box edge, in px.
const CONTEXT_RING_SIZE: f32 = 14.0;
/// Stroke centerline radius inside the 14px box.
const CONTEXT_RING_RADIUS: f32 = 5.5;
/// Ring stroke width; round-capped like the reference's `stroke-linecap`.
const CONTEXT_RING_STROKE: f32 = 2.0;

/// Drag payload for a queued follow-up row. Rows are transient session
/// state, so the live queue position is the identity (mirrors the sidebar's
/// id-carrying `DraggedGroup`, of which this is the index-addressed twin);
/// the text rides along only to paint the ghost pill.
#[derive(Clone, PartialEq)]
pub(super) struct DraggedQueueRow {
    idx: usize,
    text: SharedString,
}

impl Render for DraggedQueueRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The sidebar's drag ghost: a small bordered pill tracking the
        // cursor, so the dragged row's content is visible at the pointer.
        gpui::div()
            .max_w(px(280.))
            .px(px(6.))
            .py(px(2.))
            .rounded(px(4.))
            .bg(gpui::black().opacity(0.75))
            .text_size(px(12.))
            .text_color(gpui::white())
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.text.clone())
    }
}

/// The send cluster chip's color register: primary = the default action
/// (accent tint), danger = the stop (red), neutral = plain chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendChipTone {
    Neutral,
    Primary,
    Danger,
}

/// One action row of the send menu: icon + localized name + the
/// platform-honest shortcut hint pinned to the row's right edge in
/// `secondary_foreground`.
fn send_menu_row(
    ws: &gpui::WeakEntity<Workspace>,
    theme: &Theme,
    icon: gpui_kit_assets::IconName,
    name_key: &'static str,
    keys: String,
    act: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
) -> PopupMenuItem {
    let muted = theme.muted_foreground;
    let secondary = theme.secondary_foreground;
    let foreground = theme.foreground;
    PopupMenuItem::element(move |_, _| {
        h_flex()
            .items_center()
            .gap_2()
            .child(Icon::from(icon).small().text_color(muted))
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(foreground)
                    .child(i18n::t(name_key)),
            )
            .child(
                gpui::div()
                    .ml_auto()
                    .text_xs()
                    .text_color(secondary)
                    .child(keys.clone()),
            )
    })
    .on_click({
        let ws = ws.clone();
        move |_, window, cx| {
            let _ = ws.update(cx, |this, cx| act(this, window, cx));
        }
    })
}

/// What one cluster chip renders.
struct SendChipSpec {
    icon: gpui_kit_assets::IconName,
    label_key: Option<&'static str>,
    tooltip_key: Option<&'static str>,
    tone: SendChipTone,
    enabled: bool,
}

/// The render-time snapshot of one queued row: the render borrows `cx`
/// mutably, so the shared `&QueuedFollowUp` can't cross into the row builder.
struct QueueRowSpec {
    idx: usize,
    state: FollowUpState,
    line: String,
}

/// Resolve the pointer's boundary on one row: the insertion line hugs the
/// half of the row the pointer is in (the sidebar's `drag_boundary`, so the
/// two drag affordances behave identically). Outside the row's own bounds
/// there is no boundary — `on_drag_move` fires for every move of a live
/// drag, not only the hovered element.
fn queue_drag_boundary(
    bounds_top: gpui::Pixels,
    height: gpui::Pixels,
    pointer_y: gpui::Pixels,
) -> Option<QueueDragEdge> {
    let bottom = bounds_top + height;
    if pointer_y < bounds_top || pointer_y > bottom {
        return None;
    }
    Some(if pointer_y < bounds_top + height / 2.0 {
        QueueDragEdge::Top
    } else {
        QueueDragEdge::Bottom
    })
}

/// Accent insertion bar for a live queue drag: absolutely positioned on the
/// hovered row's top/bottom edge (the sidebar's drop marker), so showing or
/// flipping it never shifts the rows — an in-flow hairline here pumped every
/// row down by 2px on each edge flip, which read as layout jitter.
fn insertion_line(before: bool, theme: &Theme) -> AnyElement {
    let line = gpui::div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(2.))
        .rounded_full()
        .bg(theme.accent);
    if before {
        line.top_0()
    } else {
        line.bottom_0()
    }
    .into_any_element()
}

/// The queue row's one-line summary: newlines collapse to spaces, and the
/// width-based `text_ellipsis` render does the truncation.
fn queue_row_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Workspace {
    /// Rendered bare — no card border, fill, or rounding — so it shares the
    /// page background with the message list and reads as the same layer.
    /// The `Input` has no appearance of its own; the only visual separator
    /// from the messages above is the hairline injected by the footer caller.
    pub(super) fn render_composer(
        &mut self,
        running: bool,
        window: &mut Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Flip the composer placeholder only on mode transitions, so render
        // doesn't churn the InputState every frame.
        let followup_mode = running
            && self.chat.read(cx).pending_ask.is_none()
            && self.chat.read(cx).pending_confirmation.is_none();
        let placeholder_mode = if followup_mode {
            ComposerPlaceholderMode::FollowUp
        } else {
            ComposerPlaceholderMode::Normal
        };
        if placeholder_mode != self.chat.read(cx).composer_placeholder_mode {
            self.chat.update(cx, |chat, cx| {
                chat.composer_placeholder_mode = placeholder_mode;
                cx.notify();
            });
            let key = match placeholder_mode {
                ComposerPlaceholderMode::Normal => "workspace-input-placeholder",
                ComposerPlaceholderMode::FollowUp => "composer-placeholder-followup",
            };
            self.chat_input(cx).update(cx, |state, cx| {
                state.set_placeholder(i18n::t(key), window, cx);
            });
        }
        let queue = self.render_queued_follow_ups(theme, cx);
        let project_chip = self.render_project_chip_pi(theme, cx);
        let goal_chip = self.render_goal_chip(theme, cx);
        let follow_stop_chip = self.render_follow_stop_chip(theme, cx);
        let access = self.render_access_switch(theme, cx);
        let model = self.render_model_selector_pi(theme, cx);
        // Absolute cancel priority: the stop form is driven by the raw
        // running edge alone — a pending ask/approve card never swaps
        // the control into a send glyph the user cannot fire (the deadlock
        // class: stop became an empty-input-disabled send exactly when
        // the user most needed to interrupt). Enter still answers a pending
        // ask card, whose own controls are the only answer surface.
        let send = self.render_send_cluster(running, theme, cx);
        // Dictation entry (macOS first): sits left of the send cluster and
        // renders nothing where dictation cannot work, so unsupported
        // platforms reserve no space and never see a dead button.
        let dictation = self.render_dictation_button(cx);
        // Context-occupancy pill (ring + percent) sits between the model and
        // the action it quantifies: session-state cluster, then the send
        // control. Hidden entirely until usage + a resolvable window exist.
        let context_ring = self.render_context_usage_ring(theme, window, cx);
        // The completion popover overlays the composer; anchoring it on the
        // composer's own v_flex keeps it glued to the input bar in both hero
        // and footer, with a single mount point and ElementId.
        let completion_overlay = self.render_completion_overlay(window, cx);
        let mention_tip = self.render_mention_hover_tip(cx);

        v_flex()
            .w_full()
            .gap_2()
            .relative()
            .children(queue)
            .children(completion_overlay)
            .children(mention_tip)
            .when_some(
                self.chat
                    .read(cx)
                    .store
                    .as_ref()
                    .and_then(|(store, _)| store.read(cx).delivery_failure()),
                |el, failure| el.child(self.render_delivery_error_strip(&failure, cx)),
            )
            .child(
                // Composer input is message content in the mono family (Lilex)
                // at Light weight — the message-list body typeface. The `Input`
                // component forces `text_sm()` internally (its default
                // `Size::Medium` maps through `input_text_size`), so the host
                // pins the body size back with an instance-level
                // `.text_size(MESSAGE_BODY_SIZE)` (13px, one step below chrome
                // `text_base`); family + weight are applied from the wrapper
                // context.
                {
                    let wrap = gpui::div()
                        .font_family(theme.mono_font_family.clone())
                        .font_weight(gpui::FontWeight::LIGHT)
                        // The wrapper carries exactly one key context: the open
                        // completion popover takes `completion = open` (its
                        // `completion == open > Input` bindings shadow the
                        // Input's own up/down/enter/tab/escape), otherwise plain
                        // `composer` — which is what the recall bindings
                        // (`composer > Input` on alt-up / alt-down) hang off.
                        // The bare arrows stay with the Input's native
                        // MoveUp/MoveDown in every composer state.
                        .key_context(composer_key_context(
                            self.chat.read(cx).completion.is_some(),
                        ));
                    wrap.child(
                        Textarea::new(&self.chat.read(cx).input_state)
                            .appearance(false)
                            .text_size(crate::views::message::MESSAGE_BODY_SIZE)
                            // The input's own paste hook (gpui-component 0.8)
                            // replaces the capture-phase `Paste` interception
                            // this v_flex used to carry. Images stage as
                            // pending attachments and `true` consumes the
                            // paste, so the image's alt-text never reaches the
                            // rope. A clipboard with no text entry is copied
                            // files: `ClipboardItem::text()` would fall back
                            // to their path string and the engine would paste
                            // it as text, so the paths stage through the
                            // pending-attachment pipeline instead. Plain text
                            // returns `false`: the action falls through to the
                            // engine, which inserts it at the caret as one
                            // atomic edit and emits `InputEvent::Change` — the
                            // workspace subscription re-syncs the completion
                            // popover from it (the same contract the old
                            // public-`replace` + explicit `sync_completion`
                            // pair provided). The closure rides the same
                            // weak-entity bridge as the hover listener below:
                            // the hook runs on `&mut App`, not the workspace
                            // context.
                            .on_paste({
                                let ws = cx.entity().downgrade();
                                move |item, _, cx| {
                                    let entries = item.entries();
                                    // Images win over text, as the old capture
                                    // intercept did: an image+text clipboard
                                    // stages the images and nothing is
                                    // inserted.
                                    if entries
                                        .iter()
                                        .any(|e| matches!(e, gpui::ClipboardEntry::Image(_)))
                                    {
                                        let _ = ws.update(cx, |this, cx| {
                                            for entry in entries {
                                                if let gpui::ClipboardEntry::Image(image) = entry {
                                                    this.handle_pasted_image(image.clone(), cx);
                                                }
                                            }
                                            cx.notify();
                                        });
                                        // Consumed: the image's alt-text must
                                        // never reach the rope.
                                        return true;
                                    }
                                    // A clipboard with no text entry makes
                                    // `ClipboardItem::text()` fall back to the
                                    // file paths' display string — the engine
                                    // would paste it as raw path text. Copied
                                    // files stage through the pending-attachment
                                    // pipeline instead (the `File` variant the
                                    // submit path inlines via `load_attachment`).
                                    if !entries
                                        .iter()
                                        .any(|e| matches!(e, gpui::ClipboardEntry::String(_)))
                                    {
                                        let _ = ws.update(cx, |this, cx| {
                                            for entry in entries {
                                                if let gpui::ClipboardEntry::ExternalPaths(paths) =
                                                    entry
                                                {
                                                    for path in paths.paths() {
                                                        this.chat.update(cx, |chat, cx| {
                                                            chat.pending_attachments.push(
                                                                PendingAttachment::new(
                                                                    path.clone(),
                                                                ),
                                                            );
                                                            cx.notify();
                                                        });
                                                    }
                                                }
                                            }
                                            cx.notify();
                                        });
                                        return true;
                                    }
                                    // Plain text: `false` lets the action fall
                                    // through to the engine, which inserts it at
                                    // the caret as one atomic edit and emits
                                    // `InputEvent::Change` — the workspace
                                    // subscription re-syncs the completion
                                    // popover from it (the same contract the old
                                    // public-`replace` + explicit
                                    // `sync_completion` pair provided).
                                    false
                                }
                            })
                            .on_token_hover({
                                let chat = self.chat.clone();
                                let ws = cx.entity().downgrade();
                                move |event, _, cx| {
                                    let changed = chat.update(cx, |chat, _| {
                                        let next = if event.is_hovered() {
                                            // The chip already shows its short name;
                                            // the tooltip earns its place only when
                                            // the registry key differs.
                                            (event.token().id() != event.token().text()).then(
                                                || (event.token().id().clone(), event.bounds()),
                                            )
                                        } else if chat
                                            .mention_hover
                                            .as_ref()
                                            .is_some_and(|(id, _)| id == event.token().id())
                                        {
                                            // GPUI delivers a token's exit after the
                                            // next token's entry, so an exit only
                                            // clears the chip it belongs to.
                                            None
                                        } else {
                                            chat.mention_hover.clone()
                                        };
                                        if chat.mention_hover == next {
                                            return false;
                                        }
                                        chat.mention_hover = next;
                                        true
                                    });
                                    if changed {
                                        // Chat notifies have no observer bridging
                                        // them to a frame from this `&mut App`
                                        // context (the context rail needed the same
                                        // bridge at construction), so the tooltip's
                                        // mount/unmount rides on a workspace
                                        // repaint. Weak because the installed
                                        // listener outlives the frame inside the
                                        // input state.
                                        let _ = ws.update(cx, |_, cx| cx.notify());
                                    }
                                }
                            }),
                    )
                },
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        // `min_w_0` lets this group flex-shrink when the row is
                        // narrow; `overflow_hidden` is deliberately NOT set so
                        // the project picker's popover can overflow upward.
                        // `MIN_WINDOW_W` keeps the row wide enough that the
                        // chips themselves never overflow.
                        h_flex()
                            .items_center()
                            .gap_1()
                            .min_w_0()
                            .child(project_chip)
                            .when_some(goal_chip, |el, chip| el.child(chip))
                            .child(access)
                            // The stopped-follow projection rides at the
                            // group's tail: its arrival must never shift
                            // the stable controls, only extend the row.
                            .when_some(follow_stop_chip, |el, chip| el.child(chip)),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .flex_shrink_0()
                            .child(model)
                            .when_some(context_ring, |el, ring| el.child(ring))
                            .children(dictation)
                            .child(send),
                    ),
            )
            .into_any_element()
    }

    /// Render the compact queue above the composer. Every follow-up parked
    /// during the running turn stays visible here until it settles. Row
    /// layout (left to right): the one-line ellipsized summary, a lane
    /// badge (待引导 / 排队中), then the row's actions —
    /// 立即打断, the lane move, 编辑, 删除 (see
    /// [`Self::render_queue_row`]). The drag follows the sidebar's group
    /// gesture: whole row as the source, a ghost pill at the cursor, an
    /// absolute accent bar marking the insertion edge, and the strip
    /// container clearing the marker whenever the pointer leaves a row.
    /// Commits stay inside the contiguous `Queued` tail — the
    /// `[SteerPending|Failed …] ++ [Queued …]` invariant is never
    /// negotiable ([`Workspace::commit_queue_drag`]).
    pub(super) fn render_queued_follow_ups(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.chat.read(cx).queued_follow_ups.is_empty() {
            return None;
        }
        let marker = self.chat.read(cx).queue_drag;
        // Snapshot the fields the row renders (the render borrows cx
        // mutably, so the shared `&QueuedFollowUp` can't cross into it).
        let snapshot: Vec<QueueRowSpec> = self
            .chat
            .read(cx)
            .queued_follow_ups
            .iter()
            .enumerate()
            .map(|(idx, item)| QueueRowSpec {
                idx,
                state: item.state.clone(),
                line: queue_row_line(&item.turn.text),
            })
            .collect();
        let mut rows = Vec::with_capacity(snapshot.len());
        for row in snapshot.into_iter() {
            rows.push(self.render_queue_row(&row, marker, theme, cx));
        }
        // The strip owns the marker's liveness the way the shell root does
        // for sidebar drags: capture-phase dispatch runs this container
        // before the rows, so a pointer between rows or off the strip clears
        // the line a previous row set, and a drop that lands on no row
        // abandons the gesture instead of committing a stale edge.
        let strip = v_flex()
            .id("queue-strip")
            .w_full()
            .on_drag_move::<DraggedQueueRow>(cx.listener(|this, _, _, cx| {
                if this.chat.read(cx).queue_drag.is_some() {
                    this.chat.update(cx, |chat, cx| {
                        chat.queue_drag = None;
                        cx.notify();
                    });
                }
            }))
            .on_drop::<DraggedQueueRow>(cx.listener(|this, _, _, cx| {
                if this.chat.read(cx).queue_drag.is_some() {
                    this.chat.update(cx, |chat, cx| {
                        chat.queue_drag = None;
                        cx.notify();
                    });
                }
            }))
            .children(rows);
        Some(strip.into_any_element())
    }

    /// One parked follow-up row. Layout (left to right): the single-line
    /// ellipsized summary, the lane badge (待引导 / 排队中), then the row's
    /// actions — 立即打断, the lane move (重新排队 on a steer card, 进入引导
    /// otherwise — on a failed card it re-sends under a fresh id), 编辑,
    /// 删除. The whole row is the drag source (the sidebar's gesture:
    /// no grip, a ghost pill at the cursor), and the insertion marker is an
    /// absolutely-positioned accent bar on the row's top/bottom edge — zero
    /// layout shift while the pointer flips edges.
    fn render_queue_row(
        &self,
        row: &QueueRowSpec,
        marker: Option<QueueRowDrag>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let QueueRowSpec { idx, state, line } = row;
        let idx = *idx;
        let danger = matches!(state, FollowUpState::Failed);
        let line = line.clone();

        let icon_btn =
            |id: String,
             icon: gpui_kit_assets::IconName,
             tip: &'static str,
             danger_tint: bool,
             cx: &mut Context<Self>,
             on_click: fn(&mut Workspace, usize, &mut Window, &mut Context<Workspace>)|
             -> AnyElement {
                let mut btn = Button::new(SharedString::from(id))
                    .ghost()
                    .xsmall()
                    .icon(Icon::from(icon))
                    .tooltip(i18n::t(tip));
                if danger_tint {
                    btn = btn.custom(
                        ButtonCustomVariant::new(cx)
                            .color(theme.danger)
                            .foreground(theme.danger),
                    );
                }
                btn.on_click(
                    cx.listener(move |this, _, window, cx| on_click(this, idx, window, cx)),
                )
                .into_any_element()
            };

        // Lane-move + 编辑/删除 share one cluster shape across the states.
        let lane_btn = match state {
            FollowUpState::SteerPending { .. } => icon_btn(
                format!("queue-requeue-{idx}"),
                gpui_kit_assets::IconName::Undo2,
                "queue-requeue-action",
                false,
                cx,
                |this, idx, _window, cx| this.requeue_follow_up(idx, cx),
            ),
            FollowUpState::Queued => icon_btn(
                format!("queue-steer-{idx}"),
                gpui_kit_assets::IconName::CornerDownRight,
                "queued-steer-now-action",
                false,
                cx,
                |this, idx, _window, cx| this.steer_follow_up(idx, cx),
            ),
            // A failed row renders exactly like a queued one — same four
            // actions, the summary text in red as the only distinction. The
            // 进入引导 click re-sends the online steer under a fresh id (the
            // old "立即重试" labeled button; same path, converged face).
            FollowUpState::Failed => icon_btn(
                format!("queue-steer-{idx}"),
                gpui_kit_assets::IconName::CornerDownRight,
                "queued-steer-now-action",
                false,
                cx,
                |this, idx, _window, cx| this.steer_follow_up(idx, cx),
            ),
        };
        let interrupt_btn = icon_btn(
            format!("queue-interrupt-{idx}"),
            gpui_kit_assets::IconName::Zap,
            "queue-row-interrupt-action",
            true,
            cx,
            |this, idx, _window, cx| this.interrupt_from_card(idx, cx),
        );
        let edit_btn = icon_btn(
            format!("queue-edit-{idx}"),
            gpui_kit_assets::IconName::Pencil,
            "queued-edit-action",
            false,
            cx,
            |this, idx, window, cx| this.edit_follow_up(idx, window, cx),
        );
        let delete_btn = icon_btn(
            format!("queue-delete-{idx}"),
            gpui_kit_assets::IconName::Trash,
            "queued-delete-action",
            false,
            cx,
            |this, idx, _window, cx| this.delete_follow_up(idx, cx),
        );

        let badge = match state {
            FollowUpState::SteerPending { .. } => Some(
                gpui::div()
                    .px_1()
                    .py_0p5()
                    .rounded(theme.radius)
                    .bg(theme.accent.opacity(0.15))
                    .text_sm()
                    .text_color(theme.accent_foreground)
                    .child(i18n::t("message-steer-pending-badge")),
            ),
            FollowUpState::Queued => Some(
                gpui::div()
                    .px_1()
                    .py_0p5()
                    .rounded(theme.radius)
                    .bg(theme.muted.opacity(0.6))
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(i18n::t("message-queued-badge")),
            ),
            FollowUpState::Failed => None,
        };

        // The whole row drags — sidebar gesture — but only a plain `Queued`
        // row carries the payload (the steer group's order is the wire's,
        // and a failed row has nowhere to move into).
        let mut row = h_flex()
            .id(SharedString::from(format!("queue-row-{idx}")))
            .w_full()
            .relative()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border.opacity(0.6))
            .when_some(
                marker.filter(|m| m.line_on == idx),
                |row, m: QueueRowDrag| {
                    row.child(insertion_line(matches!(m.edge, QueueDragEdge::Top), theme))
                },
            );
        if matches!(state, FollowUpState::Queued) {
            let payload = DraggedQueueRow {
                idx,
                text: SharedString::from(line.to_owned()),
            };
            let ghost = payload.clone();
            row = row
                .on_drag(payload, move |_, _, _, cx| cx.new(|_| ghost.clone()))
                .on_drag_move::<DraggedQueueRow>(cx.listener(
                    move |this, e: &gpui::DragMoveEvent<DraggedQueueRow>, _, cx| {
                        let Some(edge) = queue_drag_boundary(
                            e.bounds.origin.y,
                            e.bounds.size.height,
                            e.event.position.y,
                        ) else {
                            return;
                        };
                        let next = QueueRowDrag {
                            dragged: e.drag(cx).idx,
                            line_on: idx,
                            edge,
                        };
                        if this.chat.read(cx).queue_drag != Some(next) {
                            this.chat.update(cx, |chat, cx| {
                                chat.queue_drag = Some(next);
                                cx.notify();
                            });
                            cx.notify();
                        }
                    },
                ))
                .on_drop::<DraggedQueueRow>(cx.listener(
                    move |this, _row: &DraggedQueueRow, _, cx| {
                        this.commit_queue_drag(cx);
                    },
                ));
        }
        row.child(
            h_flex().items_center().gap_2().min_w_0().flex_1().child(
                gpui::div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    // The summary wears the transcript's own body size — the
                    // row IS the message the model will read next, not chrome.
                    // Ellipsis + min_w_0 keep it one adaptive line that never
                    // covers the action cluster.
                    .text_size(crate::views::message::MESSAGE_BODY_SIZE)
                    .text_color(if danger {
                        theme.danger
                    } else {
                        theme.foreground
                    })
                    .child(line),
            ),
        )
        .children(badge)
        .child(
            h_flex()
                .items_center()
                .gap_0p5()
                .flex_shrink_0()
                .child(interrupt_btn)
                .child(lane_btn)
                .child(edit_btn)
                .child(delete_btn),
        )
        .into_any_element()
    }

    /// Persistent projection of the §二.3 follow-stop state: a compact
    /// footer chip that lives in the composer's own chip group — the
    /// place the user reaches to resend/retry — and NEVER disappears on
    /// its own. Unlike the dismissible banner (which hides only the
    /// broadcast), the chip's condition reads `follow_stop()` and
    /// deliberately ignores `dismissed`: the state outlives the
    /// dismissal, so a stuck lease or an exhausted budget always leaves a
    /// visible status and a live retry entry; the frozen transcript is
    /// never left signal-less. The chip IS the retry action: clicking it
    /// fires the same `retry_follow` the banner's Retry button wires (a
    /// no-op with no multiplexer, keeping the contract that the entry
    /// never vanishes on a click), and its tooltip previews that
    /// consequence. Geometry: `flex_shrink_0` at the chip-group tail so
    /// the chip's arrival or departure can never squeeze or shift the
    /// pinned model/send controls. Copy is keyed per reason via
    /// `indicator_key()`.
    fn render_follow_stop_chip(
        &self,
        _theme: &Theme,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        // The retry chip retired with the v2 follow stream: a failed turn
        // surfaces as the chat's error part now.
        None
    }

    /// The composer footer's inline access-mode switch (see
    /// [`build_access_switch`] for the control's anatomy). This face only
    /// reads: the seated mode from the fold (§J11), then hands the workspace
    /// handle to the pure builder.
    pub(super) fn render_access_switch(
        &self,
        _theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Selector read face (§J11): the composer's switch derives its
        // access mode from the fold — the seated value, the declared schema
        // default, or the host/enum default, never a local guess. The
        // network switch is not read here: both tiers run with the network
        // on (seeded at attach), so the control carries no network segment.
        let mode = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).access_mode_or_default()
            })
            .unwrap_or_default();
        build_access_switch(cx.entity().downgrade(), mode, cx).into_any_element()
    }

    /// Deactivate a browser tool suite on the bound thread. The chip is
    /// derived state: it rides the `BrowserSuitesChanged` echo of the
    /// facade mirror, so a landing thread's pre-engine toggle survives until
    /// the engine materializes.
    pub(super) fn deactivate_browser_tool_suite(
        &mut self,
        suite: manox_agent::engine::BrowserSuite,
        cx: &mut Context<Self>,
    ) {
        // U6b①: the toggle rides the gateway (the setter-note family, like
        // SetPlanMode/SetModel); the landing fallback parks in the facade
        // mirror.
        self.chat
            .read(cx)
            .thread
            .with_mut(|t| t.set_browser_suite(suite, false));
    }

    /// One chip-style control in the send cluster — the footer's shared chip
    /// anatomy (px_2/py_1, text_sm label, small icon, accent hover). The
    /// footer's type tier is unified at 14px so these chips match the access
    /// TabBar (whose `Size::Small` maps to `text_sm`); the cluster used to
    /// be 24px pill buttons whose label size towered over the footer's chip
    /// row.
    fn send_cluster_chip(
        id: &'static str,
        spec: SendChipSpec,
        theme: &Theme,
        cx: &mut Context<Self>,
        act: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
    ) -> AnyElement {
        let SendChipSpec {
            icon,
            label_key,
            tooltip_key,
            tone,
            enabled,
        } = spec;
        let (fg, hover) = match tone {
            SendChipTone::Neutral => (theme.muted_foreground, theme.accent.opacity(0.08)),
            SendChipTone::Primary => (theme.accent_foreground, theme.accent.opacity(0.3)),
            SendChipTone::Danger => (theme.danger, theme.danger.opacity(0.08)),
        };
        let chip = h_flex()
            .id(SharedString::from(id))
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .cursor_pointer()
            .when(tone == SendChipTone::Primary, |c| {
                c.bg(theme.accent.opacity(0.12))
            })
            .when(enabled, |c| c.hover(|h| h.bg(hover)))
            .child(Icon::from(icon).small().text_color(fg));
        let chip = match label_key {
            Some(key) => chip.child(gpui::div().text_sm().text_color(fg).child(i18n::t(key))),
            None => chip,
        };
        let chip = match tooltip_key {
            Some(key) => {
                chip.tooltip(move |window, cx| Tooltip::new(i18n::t(key)).build(window, cx))
            }
            None => chip,
        };
        if !enabled {
            return chip.opacity(0.45).into_any_element();
        }
        chip.on_click(cx.listener(move |this, _, window, cx| act(this, window, cx)))
            .into_any_element()
    }

    /// The composer's dictation entry: the upstream `SpeechButton` over the
    /// workspace's speech state. A click starts a session, the next click
    /// stops it, and the final transcript lands at the caret
    /// ([`Workspace::land_dictation`]). `None` wherever dictation cannot
    /// work — no system recognizer (Linux), or a process without the
    /// Info.plist usage keys — so the row reserves no space and never shows
    /// a dead button on an unsupported platform.
    fn render_dictation_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.speech
            .read(cx)
            .has_recognizer()
            .then(|| SpeechButton::new(&self.speech).small().into_any_element())
    }

    /// The composer's action cluster (footer right), dressed exactly like
    /// the footer's other chips. Idle: one 发送 chip. Running: an icon-only
    /// danger 停止 chip — absolute cancel priority, never gated by a draft
    /// or a card — plus the default face (排队: Enter parks the draft for
    /// the settle flush) and the chevron menu (排队 ↩ / 引导 ⌘↩ / 打断
    /// ⇧⌘↩, most-passive first).
    pub(super) fn render_send_cluster(
        &self,
        running: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !running {
            let disabled = !self.composer_can_submit(false, cx);
            return Self::send_cluster_chip(
                "send-btn",
                SendChipSpec {
                    icon: gpui_kit_assets::IconName::ArrowUp,
                    label_key: Some("send-pill-send"),
                    tooltip_key: None,
                    tone: SendChipTone::Primary,
                    enabled: !disabled,
                },
                theme,
                cx,
                |this, window, cx| this.submit_input(window, cx),
            );
        }
        let stop = Self::send_cluster_chip(
            "stop-btn",
            SendChipSpec {
                icon: gpui_kit_assets::IconName::Pause,
                label_key: None,
                tooltip_key: Some("send-pill-stop"),
                tone: SendChipTone::Danger,
                enabled: true,
            },
            theme,
            cx,
            |this, _window, cx| this.cancel_turn(cx),
        );
        // The default face carries the draft gate; the stop chip above never
        // does (the deadlock class: stop became an empty-input-disabled send
        // exactly when the user most needed to interrupt).
        let default_disabled = !self.composer_can_submit(false, cx);
        let default_face = Self::send_cluster_chip(
            "send-btn",
            SendChipSpec {
                icon: gpui_kit_assets::IconName::ListPlus,
                label_key: Some("send-pill-queue"),
                tooltip_key: None,
                tone: SendChipTone::Primary,
                enabled: !default_disabled,
            },
            theme,
            cx,
            |this, window, cx| this.submit_input(window, cx),
        );
        // The menu is the project chip's menu, whole-cloth: `PopupMenu`
        // built open-time, mounted in the cluster's deferred slot, dismissed
        // by outside click / Escape / row click through its own
        // `DismissEvent`. The rows read most-passive first — 排队 then 引导
        // then 打断 — mirroring the keys' escalation (↩ / ⌘↩ / ⇧⌘↩), and
        // stay enabled on an empty draft so the shortcuts stay discoverable
        // (the composer guards swallow the empty submit).
        let chevron = Self::send_cluster_chip(
            "send-menu-btn",
            SendChipSpec {
                icon: gpui_kit_assets::IconName::ChevronDown,
                label_key: None,
                tooltip_key: Some("send-menu-tooltip"),
                tone: SendChipTone::Neutral,
                enabled: true,
            },
            theme,
            cx,
            |this, window, cx| this.open_send_menu(window, cx),
        );
        let menu_open = self.chat.read(cx).send_menu.is_some();
        let chevron_slot = if !menu_open {
            chevron.into_any_element()
        } else {
            let menu = self
                .chat
                .read(cx)
                .send_menu
                .clone()
                .expect("send_menu exists when open");
            gpui::div()
                .relative()
                .child(chevron)
                .child(
                    deferred(
                        gpui::div()
                            .id("send-menu")
                            .absolute()
                            .bottom_full()
                            .right_0()
                            .occlude()
                            .child(menu),
                    )
                    .with_priority(1),
                )
                .into_any_element()
        };
        h_flex()
            .items_center()
            .gap_1()
            .child(stop)
            .child(default_face)
            .child(chevron_slot)
            .into_any_element()
    }

    /// Build the send menu: three ACTION rows, not a sticky mode — each
    /// fires the current draft with that behavior immediately (thinkrail's
    /// streaming send menu; a persistent mode would make the next plain
    /// Enter silently obey hidden state).
    fn open_send_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let theme = cx.theme().clone();
        let secondary = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        let steer_keys = format!("{secondary}↩");
        let interrupt_keys = format!("{secondary}⇧↩");
        let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
            menu.item(send_menu_row(
                &ws,
                &theme,
                gpui_kit_assets::IconName::ListPlus,
                "send-menu-queue",
                "↩".to_string(),
                |this, window, cx| this.submit_input(window, cx),
            ))
            .item(send_menu_row(
                &ws,
                &theme,
                gpui_kit_assets::IconName::CornerDownRight,
                "send-menu-steer",
                steer_keys,
                |this, window, cx| this.submit_steer(window, cx),
            ))
            .item(send_menu_row(
                &ws,
                &theme,
                gpui_kit_assets::IconName::Zap,
                "send-menu-interrupt",
                interrupt_keys,
                |this, window, cx| this.submit_interrupt(window, cx),
            ))
        });
        let sub = cx.subscribe(&menu, |this, _menu, _: &DismissEvent, cx| {
            this.chat.update(cx, |chat, cx| {
                chat.send_menu = None;
                cx.notify();
            });
        });
        self.chat.update(cx, |chat, cx| {
            chat.send_menu_sub = Some(sub);
            chat.send_menu = Some(menu);
            cx.notify();
        });
    }

    /// Composer context-occupancy pill (the dsh ContextMeter port): a 14px
    /// stroke ring plus the integer percent, fed by the latest request's
    /// input-side tokens against the foreground model's context window —
    /// the same active-token formula (input + cache write + cache read) the
    /// context rail's budget row uses. Renders nothing until a usage row has
    /// landed AND the model's window resolves against the registry (the dsh
    /// meter's contract: no capacity → no meter). The fill and the percent
    /// flip to the warning color past the rail's ≥90% near-full line.
    ///
    /// The pill is the conversation info bubble's trigger: click toggles,
    /// hover only tints — never opens. While open, the bubble floats above
    /// the pill on a plain relative/absolute mount: the zero-height slot
    /// below pins the surface's bottom a fixed apron above the pill top,
    /// right-aligns it to the pill, and the tail bridges the rest of the
    /// gap down to the ring. Outside click and Escape are wired by hand:
    /// `on_mouse_down_out` records the dismissal (the ring's click fires
    /// right after and must not re-open), and the surface takes focus so
    /// `Escape` lands on its `ContextBubble` key context. Thread switches
    /// close via the rail's reset.
    pub(super) fn render_context_usage_ring(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (store, sid) = self.chat.read(cx).store.clone()?;
        let usage = {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, &sid)
                .last_usage()
                .map(|u| crate::ahp_store::UsageSnapshot {
                    input: u.input_tokens.unwrap_or(0).max(0) as u64,
                    output: u.output_tokens.unwrap_or(0).max(0) as u64,
                    cache_creation: 0,
                    cache_read: u.cache_read_tokens.unwrap_or(0).max(0) as u64,
                })?
        };
        let (provider, id) = self.foreground_model_identity(cx)?;
        let window_tokens = manox_agent::provider_glue::global()
            .resolve_model(&provider, &id)
            .map(|m| m.context_window as u64)?;
        let active = usage
            .input
            .saturating_add(usage.cache_creation)
            .saturating_add(usage.cache_read);
        let budget = context_budget_pct(window_tokens, active)?;

        let near_full = budget.used_pct >= CONTEXT_NEAR_FULL_PCT;
        let fill_color = if near_full {
            theme.warning
        } else {
            theme.muted_foreground
        };
        let percent = format!("{:.0}", budget.used_pct);
        let tooltip = i18n::t_str(
            "composer-context-usage-tooltip",
            &[
                ("pct", percent.as_str()),
                ("used", &format_tokens_pi(budget.active_tokens)),
                ("cap", &format_tokens_pi(budget.cap_tokens)),
            ],
        );

        let rail = self.chat_rail(cx);
        let bubble_open = rail.read(cx).bubble_open;

        let pill = h_flex()
            .id("context-usage-pill")
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .cursor_pointer()
            .hover(|s| s.bg(theme.accent.opacity(0.08)))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            // The ring sits in its own relative wrapper so the open-state
            // tail can anchor to the RING's box: its horizontal center IS
            // the ring center, no text-width measurement involved.
            .child(gpui::div().relative().flex_none().child(context_usage_ring(
                theme.border,
                fill_color,
                budget.used_pct / 100.0,
            )))
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(fill_color)
                    .child(SharedString::from(format!("{percent}%"))),
            );

        // Width clamps are window-derived: the bubble never stretches past
        // 360 (or a narrow window's own width). Height caps at 640 — the
        // overlay renders at the window's Root layer, so the in-column
        // clipping the old measured clearance defended against no longer
        // applies.
        let viewport = window.viewport_size();
        let avail_w = f32::from(viewport.width) - 32.0;
        let max_w = px(BUBBLE_MAX_W.min(avail_w.max(BUBBLE_MIN_W)));
        let focus = rail.read(cx).bubble_focus.clone();
        let ws = cx.entity();
        let input = self.chat_input(cx);
        let rail_entity = rail.clone();
        let bubble_focus = focus.clone();

        Some(
            Popover::new("conversation-info-bubble")
                .trigger(ContextPillTrigger(pill.into_any_element()))
                .anchor(Anchor::BottomRight)
                .arrow(true)
                .open(bubble_open)
                .track_focus(&focus)
                .on_open_change({
                    let rail_entity = rail_entity.clone();
                    move |open, window, cx| {
                        rail_entity.update(cx, |rail, cx| rail.set_bubble_open(*open, cx));
                        // Hand the keyboard back to the composer when the bubble
                        // held it (Escape or outside click); a click on another
                        // focusable target keeps its own focus.
                        if !open && bubble_focus.is_focused(window) {
                            let handle = input.read(cx).focus_handle(cx);
                            window.focus(&handle, cx);
                        }
                    }
                })
                .content(move |_, _, cx| {
                    v_flex()
                        .key_context("ContextBubble")
                        .track_focus(&focus)
                        .on_action({
                            let ws = ws.clone();
                            move |_: &DismissContextBubble, window, cx| {
                                ws.update(cx, |this, cx| {
                                    this.chat_rail(cx)
                                        .update(cx, |rail, cx| rail.set_bubble_open(false, cx));
                                    this.chat_input(cx)
                                        .update(cx, |state, cx| state.focus(window, cx));
                                });
                            }
                        })
                        .max_w(max_w)
                        .max_h(px(640.))
                        .child(ContextRail::render_bubble(
                            &rail_entity,
                            max_w,
                            px(640.),
                            cx,
                        ))
                })
                .into_any_element(),
        )
    }

    /// The delivery-error strip: the composer's face for a submit pipeline
    /// that cannot currently deliver — a dead handshake, a failing
    /// transport, or a pre-connect queue past its budget. The failure
    /// states used to be tracing-only, so a user typing into a dead
    /// composer had no signal at all. Retryable faces carry a redial
    /// button; the strip reads the store's [`DeliveryFailure`] so the
    /// reason is always the pipeline's own.
    pub(super) fn render_delivery_error_strip(
        &self,
        failure: &steer_agent_chat_ui::ahp_store::DeliveryFailure,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let retryable = matches!(
            failure,
            steer_agent_chat_ui::ahp_store::DeliveryFailure::Handshake(_)
        );
        h_flex()
            .id("delivery-error-strip")
            .debug_selector(|| "delivery-error-strip".to_string())
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .bg(theme.danger.opacity(0.08))
            .child(
                Icon::new(IconName::TriangleAlert)
                    .xsmall()
                    .text_color(theme.danger),
            )
            .child(
                gpui::div()
                    .flex_1()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(i18n::t("composer-delivery-error")),
            )
            .when(retryable, |el| {
                el.child(
                    Button::new("delivery-error-retry")
                        .xsmall()
                        .label(i18n::t("composer-delivery-retry"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some((store, _)) = this.chat.read(cx).store.clone() {
                                store.update(cx, |store, cx| store.retry_handshake(cx));
                            }
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    /// The completion popover overlaid above the composer while a trigger token
    /// (`/` or `@`) is active at the caret. Uses [`gpui::anchored`] (the same
    /// mechanism gpui-component's `Popover` uses for its floating menus) so the
    /// popover escapes ancestor `overflow_hidden` clipping and avoids window-edge
    /// overflow — `div().absolute().bottom_full()` inside `deferred` does not
    /// position correctly and gets clipped by the body wrapper's `overflow_hidden`.
    /// A click on a row confirms it.
    ///
    /// The anchor point is the caret, not the input box: the textarea stashes
    /// fresh caret geometry at prepaint (gpui-base 0.8's
    /// `prepaint_caret_geometry`), so the popover stays glued to the typing
    /// position through soft wraps and multi-line growth. Render reads the
    /// last prepaint, so the anchor trails the caret by at most one frame;
    /// before the first prepaint there is no geometry yet and the overlay
    /// falls back to the input-box corner it used before.
    pub(super) fn render_completion_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.chat.read(cx).completion.as_ref()?;
        let theme = cx.theme().clone();
        let on_select = cx.listener(|this, ix: &usize, window, cx| {
            this.completion_confirm(*ix, window, cx);
        });
        let on_select: SelectHandler =
            std::rc::Rc::new(move |ix, window, cx| on_select(&ix, window, cx));
        let anchor = match self.chat_input(cx).read(cx).prepaint_caret_geometry() {
            Some(geo) => {
                let origin = caret_anchor_origin(
                    geo.cursor_bounds(),
                    geo.line_height(),
                    geo.scroll_offset().y,
                    window.viewport_size(),
                    MAX_LIST_HEIGHT + px(16.),
                    px(8.),
                );
                anchored().anchor(Anchor::TopLeft).position(origin)
            }
            None => anchored().anchor(Anchor::BottomLeft),
        };
        Some(
            deferred(
                anchor.snap_to_window_with_margin(px(8.)).child(
                    gpui::div()
                        .id("completion-dropdown")
                        .occlude()
                        .child(render_completion(state, &theme, on_select)),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// The hovered `@`-mention chip's tooltip (the column's `mention_hover`):
    /// the chip's full registry key in a small popover card pinned right under
    /// the chip's laid-out rect — the hover event's window-space bounds, so
    /// soft wraps and multi-line growth ride for free. `snap_to_window` keeps
    /// the card inside the window when the chip sits near an edge. `None`
    /// renders nothing.
    pub(super) fn render_mention_hover_tip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (key, chip) = self.chat.read(cx).mention_hover.clone()?;
        let theme = cx.theme().clone();
        let origin = point(chip.origin.x, chip.origin.y + chip.size.height + px(4.));
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(origin)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        gpui::div()
                            .id("mention-hover-tip")
                            .debug_selector(|| "mention-hover-tip".to_string())
                            .occlude()
                            .max_w(px(360.))
                            .popover_style(cx)
                            .px_2()
                            .py_1()
                            .rounded(theme.radius)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .font_family(theme.mono_font_family.clone())
                            .child(key),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// Attachment + browser-suite chips shown above the composer, each
    /// removable. File attachments clear on submit; browser suites persist.
    pub(super) fn render_attachments(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.chat.read(cx).pending_attachments.is_empty()
            && self.chat.read(cx).active_browser_suites.is_empty()
        {
            return None;
        }
        let mut col = v_flex().w_full().gap_1();
        if !self.chat.read(cx).pending_attachments.is_empty() {
            let on_remove = cx.listener(|this, ix: &usize, _window, cx| {
                if *ix < this.chat.read(cx).pending_attachments.len() {
                    this.chat.update(cx, |chat, cx| {
                        chat.pending_attachments.remove(*ix);
                        cx.notify();
                    });
                    cx.notify();
                }
            });
            col = col.child(centered(render_attachment_chips(
                &self.chat.read(cx).pending_attachments,
                theme,
                move |ix, window, cx| on_remove(&ix, window, cx),
            )));
        }
        if !self.chat.read(cx).active_browser_suites.is_empty() {
            let on_remove_suite = cx.listener(|this, ix: &usize, _window, cx| {
                if let Some(suite) = this.chat.read(cx).active_browser_suites.get(*ix).copied() {
                    this.deactivate_browser_tool_suite(suite, cx);
                }
            });
            col = col.child(centered(render_browser_chips(
                &self.chat.read(cx).active_browser_suites,
                theme,
                move |ix, window, cx| on_remove_suite(&ix, window, cx),
            )));
        }
        Some(col.into_any_element())
    }

    /// The pi-harness project chip: bound-project indicator + dropdown with
    /// recent projects (store `known_projects` plus session-cwd backfill),
    /// blank-project creation and folder selection. Selection is only
    /// allowed on empty threads (same guard as the manox chip). Data source
    /// is the pi thread store only.
    pub(super) fn render_project_chip_pi(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The chip names the conversation's *project*, read from its object
        // row — not its working directory. The two are disjoint: a
        // project-less conversation runs in a temporary workspace of its own,
        // and showing that path would name a directory the user never chose.
        let project = self.chat.read(cx).store.clone().and_then(|(_, sid)| {
            manox_agent::object_store::ObjectStore::try_global()
                .and_then(|objects| objects.project_of(&sid))
                .map(std::path::PathBuf::from)
        });
        let open = self.chat.read(cx).project_chip_open;
        let workspace = cx.entity().downgrade();

        let (icon, label): (Option<IconName>, SharedString) = match &project {
            Some(dir) => {
                let name = dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("project")
                    .to_string();
                (Some(IconName::FolderOpen), name.into())
            }
            // No project: the special project, which reads as the plain
            // conversation folder it is grouped under.
            None => (
                Some(IconName::FolderOpen),
                i18n::t("workspace-project-unfiled"),
            ),
        };

        let trigger = h_flex()
            .id("project-chip")
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .hover(|s| s.bg(theme.accent.opacity(0.08)))
            .cursor_pointer()
            .when_some(icon.clone(), |el, ic| {
                el.child(Icon::new(ic).small().text_color(theme.muted_foreground))
            })
            .child(
                gpui::div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child(label),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .small()
                .text_color(theme.muted_foreground),
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if this.chat.read(cx).project_chip_open {
                    this.close_project_chip_menu(cx);
                    cx.notify();
                    return;
                }
                // Only allow project selection on empty threads.
                let can_set = this
                    .chat
                    .read(cx)
                    .store
                    .as_ref()
                    .map(|(store, sid)| {
                        let view = store.read(cx);
                        crate::ahp_store::leaf(&view.book, sid)
                            .chat
                            .is_none_or(|c| c.turns.is_empty())
                    })
                    .unwrap_or(true);
                if !can_set {
                    return;
                }
                this.chat.update(cx, |chat, cx| {
                    chat.project_chip_open = true;
                    cx.notify();
                });

                let ws = workspace.clone();
                let theme = cx.theme().clone();
                let ws_blank = ws.clone();
                let ws_folder = ws.clone();
                // The workspace registry (`WorkspaceUpdate` stream) is the
                // recency + identity source; rows keep host display order.
                let rows: Vec<(String, String)> = this
                    .multiplexer
                    .read(cx)
                    .workspaces(cx)
                    .and_then(|state| {
                        state
                            .get("workspaces")
                            .and_then(serde_json::Value::as_array)
                            .map(|rows| {
                                rows.iter()
                                    .filter_map(|row| {
                                        let p = row.get("path")?.as_str()?;
                                        // The alias is the project's own
                                        // display name; the directory segment
                                        // is the fallback.
                                        let label = row
                                            .get("alias")
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_string)
                                            .unwrap_or_else(|| {
                                                crate::sidebar_projection::project_label(p)
                                            });
                                        Some((p.to_string(), label))
                                    })
                                    .collect()
                            })
                    })
                    .unwrap_or_default();

                let menu = PopupMenu::build(window, cx, move |menu, _window, _cx| {
                    let mut menu = menu.max_w(gpui::px(320.)).scrollable(true);
                    menu = menu.label(i18n::t("sidebar-section-projects"));

                    // Recent projects, newest first: the host's display
                    // order is creation order (oldest first), so the menu
                    // reverses it and caps at 20 — dropping the OLDEST rows,
                    // like the pre-registry list did (review #39 [issue] 3).
                    let recent_projects: Vec<(String, String)> =
                        rows.iter().rev().take(20).cloned().collect();

                    let ws_recent = ws.clone();
                    let theme_recent = theme.clone();
                    for (ix, (path_str, name)) in recent_projects.iter().enumerate() {
                        let name = name.clone();
                        let display_path = path_str.clone();
                        let click_path = path_str.clone();
                        let ws_sel = ws_recent.clone();
                        let themed = theme_recent.clone();
                        menu = menu.item(
                            PopupMenuItem::element(move |_window, _cx| {
                                let tooltip = display_path.clone();
                                h_flex()
                                    .id(("project-menu-row", ix))
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::FolderOpen)
                                            .xsmall()
                                            .text_color(themed.muted_foreground),
                                    )
                                    .child(
                                        gpui::div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .whitespace_nowrap()
                                            .text_sm()
                                            .text_color(themed.foreground)
                                            .child(name.clone()),
                                    )
                                    .tooltip(move |window, cx| {
                                        gpui_component::tooltip::Tooltip::new(tooltip.clone())
                                            .build(window, cx)
                                    })
                            })
                            .on_click(
                                move |_, _, cx: &mut gpui::App| {
                                    let p = std::path::PathBuf::from(&click_path);
                                    let _ = ws_sel.update(cx, |this, cx| {
                                        this.close_project_chip_menu(cx);
                                        this.with_foreground_store(cx, |store, sid| {
                                            store.set_cwd(&sid, p.to_str().unwrap_or_default());
                                        });
                                        Self::register_project_in_store(&p, cx);
                                        cx.notify();
                                    });
                                },
                            ),
                        );
                    }

                    menu = menu.separator();
                    menu = menu.label(i18n::t("workspace-project-new"));

                    let themed_blank = theme.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::Plus)
                                        .xsmall()
                                        .text_color(themed_blank.muted_foreground),
                                )
                                .child(
                                    gpui::div()
                                        .text_sm()
                                        .text_color(themed_blank.foreground)
                                        .child(i18n::t("workspace-project-blank")),
                                )
                        })
                        .on_click(move |_, _, cx: &mut gpui::App| {
                            let _ = ws_blank.update(cx, |this, cx| {
                                this.close_project_chip_menu(cx);
                                this.open_blank_project(cx);
                            });
                        }),
                    );

                    let themed_folder = theme.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_window, _cx| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::FolderOpen)
                                        .xsmall()
                                        .text_color(themed_folder.muted_foreground),
                                )
                                .child(
                                    gpui::div()
                                        .text_sm()
                                        .text_color(themed_folder.foreground)
                                        .child(i18n::t("workspace-project-select-folder")),
                                )
                        })
                        .on_click(move |_, _, cx: &mut gpui::App| {
                            let _ = ws_folder.update(cx, |this, cx| {
                                this.close_project_chip_menu(cx);
                                this.choose_project_inner(cx);
                            });
                        }),
                    );
                    menu
                });
                let sub = cx.subscribe(
                    &menu,
                    |this: &mut Workspace,
                     _menu: Entity<PopupMenu>,
                     _: &DismissEvent,
                     cx: &mut Context<Workspace>| {
                        this.close_project_chip_menu(cx);
                        cx.notify();
                    },
                );
                this.chat.update(cx, |chat, cx| {
                    chat.project_chip_menu = Some(menu);
                    cx.notify();
                });
                this.chat.update(cx, |chat, cx| {
                    chat.project_chip_menu_sub = Some(sub);
                    cx.notify();
                });
                cx.notify();
            }));

        if !open {
            return trigger.into_any_element();
        }

        let menu = self
            .chat
            .read(cx)
            .project_chip_menu
            .clone()
            .expect("project_chip_menu exists when open");

        gpui::div()
            .relative()
            .child(trigger)
            .child(
                deferred(
                    gpui::div()
                        .id("project-chip-dropdown")
                        .absolute()
                        .bottom_full()
                        .right_0()
                        .occlude()
                        .child(menu),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    /// Register a bound project on the active variant's thread store so the
    /// sidebar keeps its folder (persisted; survives restarts and archives).
    /// Every bind path funnels here, so it is also where a removed-project
    /// overlay entry clears: launching into a folder is the re-registration.
    pub(super) fn register_project_in_store(path: &std::path::Path, _cx: &mut Context<Self>) {
        let path = path.to_string_lossy().to_string();
        crate::project_registry::register_project(&path);
        manox_agent::thread_store_global().with_mut(|s| s.register_project(path));
    }

    /// Open the blank-project flow: pick a parent directory, then prompt for name.
    pub(super) fn open_blank_project(&mut self, cx: &mut Context<Self>) {
        if self.chat.read(cx).project_picker_pending {
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.project_picker_pending = true;
            cx.notify();
        });
        let dir = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let result = dir.await;
            this.update(cx, |this, cx| {
                this.chat.update(cx, |chat, cx| {
                    chat.project_picker_pending = false;
                    cx.notify();
                });
                if let Ok(Ok(Some(paths))) = result
                    && let Some(parent) = paths.into_iter().next()
                {
                    this.chat.update(cx, |chat, cx| {
                        chat.blank_project_parent = Some(parent);
                        cx.notify();
                    });
                    this.chat.update(cx, |chat, cx| {
                        chat.blank_project_name_input = None;
                        cx.notify();
                    });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Lazily create the blank-project name input (needs a Window).
    pub(super) fn ensure_blank_project_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.chat.read(cx).blank_project_parent.is_none() {
            return;
        }
        if self.chat.read(cx).blank_project_name_input.is_some() {
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.blank_project_name_input = Some(cx.new(|cx| InputState::new(window, cx)));
            cx.notify();
        });
    }

    /// Submit the blank project: create the directory and bind it.
    pub(super) fn confirm_blank_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self.chat.update(cx, |chat, cc| {
            let v = chat.blank_project_parent.take();
            cc.notify();
            v
        }) else {
            return;
        };
        let name = self
            .chat
            .read(cx)
            .blank_project_name_input
            .as_ref()
            .map(|s| s.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            self.chat.update(cx, |chat, cx| {
                chat.blank_project_parent = Some(parent);
                cx.notify();
            });
            return;
        }
        let new_path = parent.join(&name);
        if let Err(e) = std::fs::create_dir_all(&new_path) {
            tracing::warn!(error = %e, "failed to create project directory");
            cx.notify();
            return;
        }
        self.with_foreground_store(cx, |store, sid| {
            store.set_cwd(&sid, new_path.to_str().unwrap_or_default());
        });
        Self::register_project_in_store(&new_path, cx);
        self.chat.update(cx, |chat, cx| {
            chat.blank_project_name_input = None;
            cx.notify();
        });
        cx.notify();
    }

    /// Cancel the blank project overlay.
    pub(super) fn cancel_blank_project(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.chat.update(cx, |chat, cx| {
            chat.blank_project_parent = None;
            cx.notify();
        });
        self.chat.update(cx, |chat, cx| {
            chat.blank_project_name_input = None;
            cx.notify();
        });
        cx.notify();
    }

    /// Shared inner logic for "Select folder" (directory picker → bind project).
    pub(super) fn choose_project_inner(&mut self, cx: &mut Context<Self>) {
        if self.chat.read(cx).project_picker_pending {
            return;
        }
        self.chat.update(cx, |chat, cx| {
            chat.project_picker_pending = true;
            cx.notify();
        });
        let dir = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let result = dir.await;
            this.update(cx, |this, cx| {
                tracing::info!(?result, "project picker completed");
                this.chat.update(cx, |chat, cx| {
                    chat.project_picker_pending = false;
                    cx.notify();
                });
                if let Ok(Ok(Some(paths))) = result
                    && let Some(path) = paths.into_iter().next()
                {
                    this.with_foreground_store(cx, |store, sid| {
                        store.set_cwd(&sid, path.to_str().unwrap_or_default());
                    });
                    tracing::info!(path = %path.display(), "project pick SetCwd");
                    Self::register_project_in_store(&path, cx);
                    // Belonging is the attach decision: the row owns it and
                    // the sidebar groups by it. The SetCwd above only moves
                    // a not-yet-interacted session's working directory.
                    let sid = this
                        .chat
                        .read(cx)
                        .store
                        .as_ref()
                        .map(|(_, sid)| sid.clone());
                    if let Some(sid) = sid
                        && let Some(objects) = manox_agent::object_store::ObjectStore::try_global()
                    {
                        objects.attach(&sid, &path.to_string_lossy());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn render_blank_project_overlay(
        &self,
        _window: &mut Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.chat.read(cx).pending_ask.is_some()
            || self.chat.read(cx).pending_confirmation.is_some()
        {
            return None;
        }
        self.chat.read(cx).blank_project_parent.as_ref()?;
        let input = self.chat.read(cx).blank_project_name_input.as_ref()?;
        let parent_name = self
            .chat
            .read(cx)
            .blank_project_parent
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("…")
            .to_string();

        Some(
            gpui::div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                // Scrim must use the dark foreground, not `background`. A white
                // veil over a white conversation does not dim, so the page shows
                // through and the modal reads as transparent.
                .bg(theme.foreground.opacity(0.6))
                .child(
                    v_flex()
                        .w(px(480.))
                        .p_4()
                        .gap_3()
                        .rounded(theme.radius)
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .shadow_lg()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Icon::new(IconName::FolderOpen)
                                        .small()
                                        .text_color(theme.accent_foreground),
                                )
                                .child(
                                    gpui::div()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(i18n::t("workspace-project-blank")),
                                ),
                        )
                        .child(
                            gpui::div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(format!(
                                    "{}: {}",
                                    i18n::t("workspace-project-name-prompt"),
                                    parent_name
                                )),
                        )
                        .child(Input::new(input))
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(
                                    Button::new("blank-project-cancel")
                                        .ghost()
                                        .small()
                                        .label(i18n::t("workspace-cancel"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.cancel_blank_project(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("blank-project-confirm")
                                        .primary()
                                        .small()
                                        .label(i18n::t("workspace-rename-confirm"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.confirm_blank_project(window, cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// The 14×14 occupancy ring canvas: a border-tone track circle plus the
/// occupancy arc sweeping clockwise from 12 o'clock (muted, warning when
/// near full). The stroke treatment replicates the reference SVG: 2px
/// round-capped circle strokes, dash origin rotated to the top.
fn context_usage_ring(track: Hsla, fill: Hsla, pct: f64) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let center = bounds.center();
            let radius = px(CONTEXT_RING_RADIUS);
            if let Ok(track_path) = ring_circle_path(center, radius) {
                window.paint_path(track_path, track);
            }
            // A full turn redraws the closed circle (a 360° endpoint arc
            // collapses to nothing); a zero turn strokes nothing at all.
            let clamped = pct.clamp(0.0, 1.0);
            let fill_path = if clamped >= 1.0 {
                ring_circle_path(center, radius).ok()
            } else {
                occupancy_arc_end(CONTEXT_RING_RADIUS, clamped).and_then(|(dx, dy, large)| {
                    ring_arc_path(center, radius, px(dx), px(dy), large)
                })
            };
            if let Some(fill_path) = fill_path {
                window.paint_path(fill_path, fill);
            }
        },
    )
    .size(px(CONTEXT_RING_SIZE))
}

/// Stroke builder shared by every ring path: the reference SVG's 2px
/// round-capped line treatment.
fn ring_path_builder() -> PathBuilder {
    PathBuilder::default().with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(CONTEXT_RING_STROKE)
            .with_line_cap(LineCap::Round),
    ))
}

/// Full circle centered at `center`: two half arcs, since the SVG endpoint
/// parameterization collapses a 360° arc into a zero-length segment.
fn ring_circle_path(
    center: Point<Pixels>,
    radius: Pixels,
) -> Result<gpui::Path<Pixels>, anyhow::Error> {
    let mut builder = ring_path_builder();
    builder.move_to(point(center.x + radius, center.y));
    builder.arc_to(
        point(radius, radius),
        px(0.),
        true,
        true,
        point(center.x - radius, center.y),
    );
    builder.arc_to(
        point(radius, radius),
        px(0.),
        true,
        true,
        point(center.x + radius, center.y),
    );
    builder.build()
}

/// Open occupancy arc: from 12 o'clock clockwise to the point at offset
/// (`dx`, `dy`) from the center.
fn ring_arc_path(
    center: Point<Pixels>,
    radius: Pixels,
    dx: Pixels,
    dy: Pixels,
    large_arc: bool,
) -> Option<gpui::Path<Pixels>> {
    let mut builder = ring_path_builder();
    builder.move_to(point(center.x, center.y - radius));
    builder.arc_to(
        point(radius, radius),
        px(0.),
        large_arc,
        true,
        point(center.x + dx, center.y + dy),
    );
    builder.build().ok()
}

/// Endpoint of the occupancy sweep, relative to the ring center, plus the
/// SVG large-arc flag. The sweep starts at 12 o'clock (0, −r) and runs
/// clockwise — the `rotate(-90 7 7)` dash origin of the reference SVG;
/// screen y grows downward, so the positive-angle direction is clockwise.
/// Degenerate sweeps return `None`: 0% has nothing to stroke, and 100%
/// belongs to the closed-circle path.
fn occupancy_arc_end(radius: f32, pct: f64) -> Option<(f32, f32, bool)> {
    let alpha = pct.clamp(0.0, 1.0) * std::f64::consts::TAU;
    if alpha <= 0.0 || alpha >= std::f64::consts::TAU {
        return None;
    }
    let phi = -std::f64::consts::FRAC_PI_2 + alpha;
    Some((
        (radius as f64 * phi.cos()) as f32,
        (radius as f64 * phi.sin()) as f32,
        alpha > std::f64::consts::PI,
    ))
}

#[cfg(test)]
mod tests {
    use super::occupancy_arc_end;

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-4, "{actual} != {expected}");
    }

    #[test]
    fn occupancy_arc_quarters() {
        // 25%: 12 o'clock → 3 o'clock.
        let (dx, dy, large) = occupancy_arc_end(5.5, 0.25).expect("quarter arc");
        assert_close(dx, 5.5);
        assert_close(dy, 0.0);
        assert!(!large);
        // 50%: → 6 o'clock; exactly half is still the small-arc flag.
        let (dx, dy, large) = occupancy_arc_end(5.5, 0.5).expect("half arc");
        assert_close(dx, 0.0);
        assert_close(dy, 5.5);
        assert!(!large);
        // 75%: → 9 o'clock, first sweep past the halfway mark.
        let (dx, dy, large) = occupancy_arc_end(5.5, 0.75).expect("three-quarter arc");
        assert_close(dx, -5.5);
        assert_close(dy, 0.0);
        assert!(large);
    }

    #[test]
    fn occupancy_arc_degenerate_sweeps() {
        assert_eq!(occupancy_arc_end(5.5, 0.0), None);
        assert_eq!(occupancy_arc_end(5.5, 1.0), None);
        // Out-of-range percents clamp before the degeneracy check.
        assert_eq!(occupancy_arc_end(5.5, -0.5), None);
        assert_eq!(occupancy_arc_end(5.5, 1.5), None);
    }

    #[test]
    fn occupancy_arc_intermediate_point() {
        // 1/12 of the circle: 12 o'clock rotated 30° clockwise lands at
        // screen angle φ = −60°: (r·cos60°, −r·sin60°).
        let (dx, dy, large) = occupancy_arc_end(5.5, 1.0 / 12.0).expect("30° arc");
        assert_close(dx, 2.75);
        assert_close(dy, -4.7631);
        assert!(!large);
    }
}
