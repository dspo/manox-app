//! The conversation column as its own entity — the step that lets a session be
//! a sheet body.
//!
//! The canvas model's invariant is "a sheet exists because its kind is
//! registered, and the layout only says where it is placed". For that to hold
//! for the dominant kind (`manox-session`), the column must be a view the
//! registry can hand out — not a stretch of `Workspace::render`. This module is
//! where that view lives while the migration lands:
//!
//! 1. **the entity and the transcript** (here): the message band, its turn
//!    rail, the history gate and the body facts the rest of the frame reads;
//! 2. the composer and its attachments (moves next);
//! 3. the captain strip and the column frame (then `Workspace::render` mounts
//!    the whole view as its main column);
//! 4. the canvas (`steer-canvas` layouts + `SheetKindSpec`) mounts one view per
//!    session sheet.
//!
//! Two rules keep the stages honest:
//!
//! - **State stays in the column.** `ChatColumn` (the chat crate's entity) owns
//!   the conversation, drafts, asks, the list state and the body flags; this
//!   view owns presentation and derives per-frame facts ([`ColumnBodyState`])
//!   once, so the frame around it never re-derives them from a second path.
//! - **Effects cross through a port.** Anything that belongs to the app —
//!   scrolling a row into view today; right-pane tabs, navigation and settings
//!   as they migrate — goes through [`ChatEnv`]. The view never holds a
//!   `WeakEntity<Workspace>` of its own, so a later canvas host swaps the port
//!   instead of rewriting the column.

use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::{ElementExt as _, h_flex, v_flex};
use steer_agent_chat_ui::column::ChatColumn;

use crate::Workspace;

/// The app-side port the column's effects go through.
///
/// Deliberately narrow: a method lands here only when the column needs an
/// effect the app owns. Session-scoped work stays in the column (its own state
/// plus the chat crate's host port); app-scoped work comes here.
pub trait ChatEnv: Send + Sync + 'static {
    /// Scroll a transcript row into view (the turn rail's jump and the
    /// navigator's landing).
    fn reveal_message(&self, item_ix: usize, cx: &mut App);
}

/// The production [`ChatEnv`]: a weak handle onto the workspace, which owns
/// the scroll/index machinery today.
pub struct WorkspaceChatEnv(WeakEntity<Workspace>);

impl WorkspaceChatEnv {
    pub fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self(workspace)
    }
}

impl ChatEnv for WorkspaceChatEnv {
    fn reveal_message(&self, item_ix: usize, cx: &mut App) {
        if let Some(workspace) = self.0.upgrade() {
            workspace.update(cx, |workspace, cx| workspace.reveal_message(item_ix, cx));
        }
    }
}

/// The body facts the column derives once per frame: whether the history gate
/// is up, whether a turn runs, and whether the conversation is still on its
/// first screen. The frame around the column reads these instead of reaching
/// into the chat entity itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ColumnBodyState {
    pub(crate) history_loading: bool,
    /// The thread whose history is loading (empty unless the gate is up).
    pub(crate) thread_id: String,
    pub(crate) running: bool,
    /// No message yet and nothing running: the composer takes the hero slot.
    pub(crate) first_screen: bool,
}

/// One conversation column.
pub struct ChatColumnView {
    pub(crate) chat: Entity<ChatColumn>,
    env: Arc<dyn ChatEnv>,
}

impl ChatColumnView {
    pub fn new(chat: Entity<ChatColumn>, env: Arc<dyn ChatEnv>) -> Self {
        Self { chat, env }
    }

    /// The column's body facts, derived once: the history gate, the running
    /// bit and the composer's placement input.
    pub(crate) fn body_state(&self, cx: &App) -> ColumnBodyState {
        let history_loading = self.chat.read(cx).awaiting_history.is_some()
            && self
                .chat
                .read(cx)
                .store
                .as_ref()
                .map(|(store, sid)| {
                    crate::ahp_store::leaf(&store.read(cx).book, sid.as_str())
                        .chat
                        .is_none()
                })
                .unwrap_or(false);
        // The gate names the thread it waits for; a landed snapshot clears it.
        let thread_id = if history_loading {
            self.chat.read(cx).thread.read(|thread| thread.id.0.clone())
        } else {
            String::new()
        };
        let running = self
            .chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, sid)| {
                let view = store.read(cx);
                crate::ahp_store::leaf(&view.book, sid).running()
            })
            .unwrap_or(false);
        let first_screen = self.chat.read(cx).conversation.read(cx).is_empty(cx) && !running;
        ColumnBodyState {
            history_loading,
            thread_id,
            running,
            first_screen,
        }
    }

    /// A reopened thread whose snapshot has not landed keeps the loading page
    /// only while the wait is young: past the timeout the gate is pruned and
    /// the page degrades to the hero screen instead of pinning forever.
    fn prune_history_gate(&self, cx: &mut App) {
        prune_history_gate(&self.chat, cx);
    }

    /// The transcript band: the message list, its rubber-band, its press /
    /// release bookkeeping and the turn rail over it.
    ///
    /// The band owns nothing — every piece of state it reads or writes lives on
    /// the chat entity, so a switch of tabs or containers never rebuilds it.
    fn transcript(&self, window: &mut Window, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        self.prune_history_gate(cx);
        let state = self.body_state(cx);
        if state.first_screen || state.history_loading {
            return gpui::div().into_any_element();
        }
        let main_body_w = self
            .chat
            .read(cx)
            .card_width
            .get()
            .unwrap_or_else(|| window.bounds().size.width);

        // Left-edge turn rail: the user-turn anchors are re-derived every
        // frame (the same projection the ⌘M navigator reads). The width gate
        // short-circuits before the projection; the turn-count gate lives
        // inside `render_turn_rail`, whose single result feeds both the rail
        // and the gutter so the two can never fork.
        let turn_rail = if main_body_w >= gpui::px(crate::views::turn_rail::MIN_CARD_WIDTH) {
            let turns = crate::views::turn_rail::collect_rail_turns(
                self.chat
                    .read(cx)
                    .conversation
                    .read(cx)
                    .items()
                    .iter()
                    .enumerate()
                    .map(|(ix, item)| (ix, item.read(cx).kind())),
            );
            let env = self.env.clone();
            let on_jump: crate::views::turn_rail::JumpFn =
                std::rc::Rc::new(move |item_ix, _window, cx| {
                    env.reveal_message(item_ix, cx);
                });
            crate::views::turn_rail::render_turn_rail(&theme, &self.chat, turns, on_jump, cx)
        } else {
            None
        };
        let show_turn_rail = turn_rail.is_some();

        // The row factory is a pure read-only projection over the conversation.
        let conversation = self.chat.read(cx).conversation.clone();
        let diag_enabled = crate::overlap_diag::enabled();
        let processor = move |ix: usize, _window: &mut Window, cx: &mut App| {
            // A folded whole-turn process head hides the turn's segments (the
            // reply is exempt) — the row renders empty instead.
            if conversation.read(cx).hidden_by_turn_process(ix, cx) {
                return v_flex().w_full().min_w_0().into_any_element();
            }
            let item = conversation.read(cx).items().get(ix).cloned();
            match item {
                Some(item) => {
                    if diag_enabled {
                        crate::overlap_diag::record_mapping(ix, item.read(cx).diagnostic_id());
                    }
                    // A reply immediately followed by an activity segment
                    // tightens its bottom gap: the segment's own header
                    // padding breathes, so the generic 16px reads as a hole
                    // (the reverse direction keeps it — a reply needs its air).
                    let tight_gap = conversation.read(cx).tight_gap_after(ix, cx);
                    v_flex()
                        .w_full()
                        .pt_1()
                        .when(tight_gap, |this| this.pb_1())
                        .when(!tight_gap, |this| this.pb_4())
                        .flex_shrink_0()
                        .min_w_0()
                        .debug_selector(move || format!("workspace-message-row-{ix}"))
                        .when(diag_enabled, |this| {
                            this.on_prepaint(move |bounds, _window, _cx| {
                                crate::overlap_diag::record_row(ix, bounds);
                            })
                        })
                        .child(item)
                        .into_any_element()
                }
                None => gpui::div().into_any_element(),
            }
        };
        let list_state = self.chat.read(cx).list_state.clone();
        let bounce_state = self.chat.read(cx).list_state.clone();
        let chat_for_bounce = self.chat.clone();
        let chat_for_press = self.chat.clone();
        let chat_for_release = self.chat.clone();
        let width_state = self.chat.read(cx).list_state.clone();
        let message_list_width = self.chat.read(cx).message_list_width.clone();
        let diag_state = self.chat.read(cx).list_state.clone();
        let mono_family = theme.mono_font_family.clone();

        let list_el = gpui::list(list_state, processor)
            .w_full()
            .h_full()
            .min_h_0()
            .min_w_0();
        // macOS-native rubber-band on precise scrolling, via gpui-kit 0.8's
        // shared overscroll engine. The bounce scrolls the ListState itself,
        // past the list's own scroll handler, so notify the chat here as that
        // handler would — render-time scroll consumers (the turn rail's active
        // mark) read `logical_scroll_top` and must keep up. Tail follow is
        // native ListState state (`FollowMode::Tail`), so the bounce's scroll
        // path keeps its disengage/re-arm semantics.
        let list_el = gpui_base::ScrollBounce::new("chat-list-bounce", &bounce_state, list_el)
            .on_scroll(move |_, cx| {
                chat_for_bounce.update(cx, |_, cx| cx.notify());
            });
        let list_wrap = v_flex()
            .debug_selector(|| "workspace-message-list".into())
            .flex_1()
            .h_full()
            .min_h_0()
            .min_w_0()
            .font_family(mono_family.clone())
            .font_weight(gpui::FontWeight::LIGHT)
            .child(list_el)
            // Grabbing the transcript pauses tail-follow so the next streamed
            // chunk cannot yank the column away mid-drag. A release that
            // produced no text selection and left the view where the press
            // found it restores the press-time follow state (plain clicks keep
            // live-reading); a real selection or a moved view (scrollbar drag)
            // stays paused until the user wheels back to the bottom or submits
            // — the pause keeps Tail mode, so both re-arm it.
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                chat_for_press.update(cx, |chat, cx| {
                    chat.transcript_press.started(&chat.list_state);
                    cx.notify();
                });
            })
            .on_mouse_up(gpui::MouseButton::Left, move |_, window, cx| {
                // The bubble-phase handler runs before the engine's
                // window-level mouse-up, but the selection snapshot is already
                // readable.
                let has_selection = !gpui_base::TextSelection::selected_text(window, cx).is_empty();
                chat_for_release.update(cx, |chat, cx| {
                    chat.transcript_press.ended(&chat.list_state, has_selection);
                    cx.notify();
                });
            })
            .on_prepaint(move |bounds, window, _app| {
                if message_list_width.update(bounds.size.width, &width_state) {
                    window.refresh();
                }
                if crate::overlap_diag::enabled() {
                    crate::overlap_diag::check_completed_frame(bounds, diag_state.item_count());
                }
            });

        h_flex()
            .flex_1()
            .w_full()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .relative()
            // h_flex presets items_center, which centers the children
            // vertically and collapses the list's h_full to zero — the
            // transcript band must stretch them instead.
            .items_stretch()
            // The gutter pads the list itself, not this band: the rail's
            // absolute anchor is the band, so padding the band would drag the
            // rail right along with the text it must clear.
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .items_stretch()
                    .when(show_turn_rail, |this| {
                        this.pl(gpui::px(crate::views::turn_rail::GUTTER))
                    })
                    .child(list_wrap),
            )
            // The rail paints after the list so its marks and preview float
            // over the transcript band.
            .children(turn_rail)
            .into_any_element()
    }
}

impl Render for ChatColumnView {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // Stage 1: the transcript. The composer, the captain strip and the
        // column frame join it next; the frame around it is still the
        // workspace's, and reads its facts through `body_state`.
        self.transcript(window, cx)
    }
}

/// Drops an expired history gate (see [`ChatColumnView::body_state`]).
pub(crate) fn prune_history_gate(chat: &Entity<ChatColumn>, cx: &mut App) {
    let expired = chat
        .read(cx)
        .awaiting_history
        .is_some_and(|since| since.elapsed() > crate::views::history_loading::HISTORY_TIMEOUT);
    if expired {
        chat.update(cx, |chat, cx| {
            chat.awaiting_history = None;
            cx.notify();
        });
    }
}
