//! Visual acceptance for an ```svg fence in a message body: the diagram paints
//! in the transcript, through the production row path (a real
//! `ConversationState` fed an assistant message, mounted as a `MessageItem`).
//!
//! Run with `SVG_SHOT=/tmp/inline-svg.png cargo test -p agent-ui --features
//! test-support --test visual_inline_svg`. The sample is the shape a model
//! writes: a labelled diagram, a fence the rasterizer rejects (it must stay a
//! code block), and a plain code block for contrast. A diagram taller than the
//! display cap is pinned by `tests/inline_svg_block.rs` instead — the offscreen
//! window stops at the display's own height, which the cap plus this message
//! would exceed.
//!
//! macOS only: the readback is a Metal texture; other platforms print a skip
//! line.

#[cfg(any(not(target_os = "macos"), not(feature = "test-support")))]
fn main() {
    eprintln!("visual_inline_svg: skipped (needs macOS Metal readback + test-support)");
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
fn main() {
    macos::main();
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
mod macos {
    use std::sync::Arc;

    use agent_ui::assets::ExtrasAssetSource;
    use agent_ui::conversation::{ConvItem, ConversationState};
    use agent_ui::views::message::MessageItem;
    use gpui::{
        AppContext as _, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        VisualTestAppContext, Window, div, px, size,
    };
    use gpui_component::Root;
    use manox_agent::MessageAuthor;

    const MODEL: &str = "bigmodel-anthropic/glm-5.3";

    /// A labelled diagram — the rasterizer has to resolve fonts for it, so the
    /// shot also covers text inside the svg.
    const DIAGRAM: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="260" viewBox="0 0 640 260">
  <rect width="640" height="260" fill="#f8fafc" rx="12"/>
  <rect x="40" y="90" width="160" height="80" rx="10" fill="#3b82f6"/>
  <text x="120" y="136" font-family="sans-serif" font-size="20" fill="#ffffff" text-anchor="middle">prompt</text>
  <rect x="440" y="90" width="160" height="80" rx="10" fill="#10b981"/>
  <text x="520" y="136" font-family="sans-serif" font-size="20" fill="#ffffff" text-anchor="middle">reply</text>
  <line x1="204" y1="130" x2="436" y2="130" stroke="#334155" stroke-width="4"/>
  <polygon points="436,130 416,120 416,140" fill="#334155"/>
  <text x="320" y="110" font-family="sans-serif" font-size="18" fill="#334155" text-anchor="middle">agent turn</text>
</svg>"##;

    /// The assistant message the shot renders: the diagram, a fence the
    /// rasterizer rejects (it stays a code block), and a plain code block for
    /// contrast. A diagram taller than the cap is not in the shot — the window
    /// is shorter than the cap plus the rest of the message, so the tall case
    /// is pinned by `tests/inline_svg_block.rs` instead.
    fn message() -> String {
        format!(
            "Here is the round trip:\n\n```svg\n{DIAGRAM}\n```\n\n\
             A fence the rasterizer rejects stays code:\n\n```svg\n<svg><rect\n```\n\n\
             And a normal code block is untouched:\n\n```rust\nfn main() {{}}\n```\n"
        )
    }

    struct Transcript {
        row: Entity<MessageItem>,
    }

    impl Render for Transcript {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .size_full()
                .p_6()
                .child(div().w_full().max_w(px(760.)).child(self.row.clone()))
        }
    }

    pub fn main() {
        let Ok(path) = std::env::var("SVG_SHOT") else {
            eprintln!("visual_inline_svg: SVG_SHOT not set, skipping");
            return;
        };

        manox_agent::init();
        let platform = gpui_platform::current_platform(false);
        let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(ExtrasAssetSource));
        cx.update(|cx| {
            gpui_component::init(cx);
            steer_i18n::init();
            steer_agent_chrome_ui::register_fonts(cx);
        });

        let conversation = cx.update(|cx| cx.new(|_| ConversationState::new(MessageAuthor::Lead)));
        cx.update(|cx| {
            // The same fold the live stream runs: one markdown part carrying
            // the whole message, applied through the published reducer.
            let mut chat: ahp_types::state::ChatState = serde_json::from_value(serde_json::json!({
                "resource": "ahp-chat:/c-shot",
                "title": "shot",
                "status": 0,
                "modifiedAt": "2026-01-01T00:00:00Z",
                "turns": [],
            }))
            .expect("the empty chat parses");
            let start: ahp_types::actions::StateAction =
                serde_json::from_value(serde_json::json!({
                    "type": "chat/turnStarted", "turnId": "t-1",
                    "startedAt": "2026-01-01T00:00:00Z",
                    "message": { "text": "shot", "origin": { "kind": "user" } },
                }))
                .expect("the start action parses");
            let open: ahp_types::actions::StateAction = serde_json::from_value(serde_json::json!({
                "type": "chat/responsePart", "turnId": "t-1",
                "part": { "kind": "markdown", "id": "p-1", "content": "" },
            }))
            .expect("the open action parses");
            let delta: ahp_types::actions::StateAction =
                serde_json::from_value(serde_json::json!({
                    "type": "chat/delta", "turnId": "t-1", "partId": "p-1",
                    "content": message(),
                }))
                .expect("the delta action parses");
            ahp::reducers::apply_action_to_chat(&mut chat, &start);
            ahp::reducers::apply_action_to_chat(&mut chat, &open);
            ahp::reducers::apply_action_to_chat(&mut chat, &delta);
            let host = steer_agent_chat_ui::host::noop_host();
            let env = steer_agent_chat_ui::transcript::ProjectEnv {
                role: MODEL,
                recipient: &MessageAuthor::Lead,
                host: &host,
                cwd: None,
                fork_source: false,
                promoted_confirmation: None,

                entering_turns: &[],
                steered_turns: steer_agent_chat_ui::transcript::no_steered_turns(),
            };
            conversation.update(cx, |c, cx| {
                let nodes = c.stage(Some(&chat), &env);
                c.reconcile(nodes, &env, cx);
            });
        });
        let rows: Vec<Entity<MessageItem>> = cx.update(|cx| {
            conversation
                .read(cx)
                .items()
                .iter()
                .filter(|item| matches!(item.read(cx).kind(), ConvItem::Assistant { .. }))
                .cloned()
                .collect()
        });

        let handle = cx
            .open_offscreen_window(size(px(1000.), px(1400.)), move |window, cx| {
                let transcript = cx.new(|_| Transcript {
                    row: rows[0].clone(),
                });
                cx.new(|cx| Root::new(transcript, window, cx))
            })
            .expect("offscreen window");

        for _ in 0..5 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let shot = cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot");
        shot.save(&path).expect("save png");
        println!(
            "visual_inline_svg: wrote {path} ({}x{})",
            shot.width(),
            shot.height()
        );
    }
}
