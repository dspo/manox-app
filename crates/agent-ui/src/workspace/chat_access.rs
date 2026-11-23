//! The conversation's state accessors and its chat-local derivations, shared by
//! the app frame and the column that renders itself.
//!
//! The column split gives one piece of state two readers: `Workspace` (which
//! frames the column and owns everything around it) and `ChatColumnView` (which
//! renders the column). The wrong answers are to duplicate each helper on both
//! entities or to route session state through the effect port:
//!
//! - a duplicate forks the definition the first time one side is edited;
//! - the port is for **app** effects and environment only — putting the
//!   composer's "can I submit" or the model identity behind it would let session
//!   state cross the boundary and turn the port into a second workspace.
//!
//! So the shared half lives here, as free functions over the state entity. The
//! dependency runs **entity → conversation state**, never entity → other entity,
//! and each side keeps a one-line method so call sites stay put.

use super::*;

/// The column's composer input state.
pub(crate) fn input(chat: &Entity<ChatColumn>, cx: &App) -> Entity<TextareaState> {
    chat.read(cx).input_state.clone()
}

/// The column's context rail (the right-edge usage/observations rail).
pub(crate) fn rail(
    chat: &Entity<ChatColumn>,
    cx: &App,
) -> Entity<crate::views::context_rail::ContextRail> {
    chat.read(cx).context_rail.clone()
}

/// The column's conversation projection (the message list's source).
pub(crate) fn conversation(chat: &Entity<ChatColumn>, cx: &App) -> Entity<ConversationState> {
    chat.read(cx).conversation.clone()
}

/// Whether the composer may submit right now: a running turn always may (the
/// submit steers it); otherwise the input, a pending ask or an attachment is
/// required.
pub(crate) fn composer_can_submit(chat: &Entity<ChatColumn>, running: bool, cx: &App) -> bool {
    if running {
        return true;
    }
    let input_empty = input(chat, cx).read(cx).value().trim().is_empty();
    if chat.read(cx).pending_ask.is_some() {
        // The card is answerable without typing anything: its pager's check
        // settles whatever is selected, and an untouched question settles as
        // an explicit skip.
        true
    } else {
        !input_empty || !chat.read(cx).pending_attachments.is_empty()
    }
}

/// Applies a completion item at `ix` to the composer input.
pub(crate) fn completion_confirm<T: 'static>(
    chat: &Entity<ChatColumn>,
    ix: usize,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    let Some(state) = chat.update(cx, |chat, cc| {
        let v = chat.completion.take();
        cc.notify();
        v
    }) else {
        return;
    };
    let Some(item) = state.items.get(ix) else {
        chat.update(cx, |chat, cx| {
            chat.completion = Some(state);
            cx.notify();
        });
        return;
    };
    let name = item.name.to_string();
    let trigger = state.trigger;
    let token_start = state.token_start;
    let (value, cursor) = {
        let s = input(chat, cx).read(cx);
        (s.value().to_string(), s.selected_range().end)
    };
    if cursor > value.len() || token_start > cursor {
        return;
    }
    // The inserted name rides as an atomic inline token (chip render, whole
    // unit delete); a rejected token op degrades to the plain-text insert.
    let (new_value, caret): (InputContent, usize) =
        match build_replacement_content(trigger, &name, &name, &value, token_start, cursor) {
            Ok((content, caret)) => (content, caret),
            Err(_) => {
                let (text, caret) = build_replacement(trigger, &name, &value, token_start, cursor);
                (InputContent::from(text), caret)
            }
        };
    input(chat, cx).update(cx, |s, cx| {
        s.set_value(new_value, window, cx);
        let pos = RopeExt::offset_to_position(s.text(), caret.min(s.text().len()));
        s.set_cursor_position(pos, window, cx);
    });
    cx.notify();
}

/// The foreground session's effective [`AccessMode`], read from the fold
/// (§J11): the seated value, the host-declared schema default, or the enum
/// default — never a local guess. `Sandbox` when no store is bound.
pub(crate) fn access_mode(chat: &Entity<ChatColumn>, cx: &App) -> AccessMode {
    chat.read(cx)
        .store
        .as_ref()
        .map(|(store, sid)| {
            crate::ahp_store::leaf(&store.read(cx).book, sid).access_mode_or_default()
        })
        .unwrap_or_default()
}

/// The foreground session's model identity (`provider`, `id`): the seated pick,
/// else the registry default — the honest "what would run" for a session whose
/// config seat is still empty.
pub(crate) fn model_identity(chat: &Entity<ChatColumn>, cx: &App) -> Option<(String, String)> {
    let identity = chat
        .read(cx)
        .store
        .as_ref()
        .and_then(|(store, sid)| {
            let view = store.read(cx);
            crate::ahp_store::leaf(&view.book, sid)
                .model_id()
                .map(str::to_string)
        })
        .or_else(|| {
            manox_agent::provider_glue::default_model().map(|m| format!("{}/{}", m.provider, m.id))
        });
    let identity = identity?;
    let (provider, id) = identity.split_once('/')?;
    (!provider.is_empty() && !id.is_empty()).then(|| (provider.to_string(), id.to_string()))
}

/// Exact registration match of a canonical model identity against the
/// gateway's model-registry snapshot (U2 display resolution): the chip and the
/// picker menu read the wire `ModelInfo` the server projects, so a remote
/// server's registry drives the desktop display. A stale id resolves to `None`
/// and the caller renders the raw identity, never a fuzzy look-alike.
pub(crate) fn model_display(
    provider: &str,
    id: &str,
) -> Option<(String, crate::model_catalog::ModelRow)> {
    // Display resolution rides the in-process provider registry (the same
    // source the streaming side matches against); the AHP root catalogue
    // serves remote clients.
    crate::model_catalog::resolve(provider, id).map(|row| (row.provider_display.clone(), row))
}
