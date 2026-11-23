//! Visual acceptance for the canvas surface: the real
//! chrome assembly mounted in an offscreen window, its main column showing one
//! container's canvas — two tabs (start page + session) and, with
//! `CANVAS_SPLIT=1`, the split gesture's two groups.
//!
//! Run with `CHROME_SHOT=/tmp/canvas.png CHROME_CANVAS=1 cargo test -p agent-ui
//! --features test-support --test visual_canvas`; without `CHROME_CANVAS` the
//! binary prints a skip line and exits green, so the test-support gate stays
//! quiet in CI runs that only compile it. The scene binds a detached AHP store
//! (the offscreen harness has no host to attach a session), seeds the session
//! titles the canvas's title source reads, and drives the production gestures
//! (`focus_conversation`, the store binding, the tab split) — no view is
//! rebuilt for the shot.
//!
//! macOS-only: the readback is a Metal texture (`capture_screenshot`).

#[cfg(any(not(target_os = "macos"), not(feature = "test-support")))]
fn main() {
    eprintln!("visual_canvas: skipped (needs macOS Metal readback + test-support)");
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
fn main() {
    macos::main();
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
mod macos {
    use std::sync::Arc;

    use agent_ui::ahp_store::AhpStore;
    use agent_ui::assets::ExtrasAssetSource;
    use gpui::{AppContext as _, VisualTestAppContext, px, size};

    pub fn main() {
        if std::env::var("CHROME_CANVAS").is_err() {
            eprintln!("visual_canvas: CHROME_CANVAS not set, skipping");
            return;
        }
        let Ok(path) = std::env::var("CHROME_SHOT") else {
            eprintln!("visual_canvas: CHROME_SHOT not set, skipping");
            return;
        };
        let split = std::env::var("CANVAS_SPLIT").is_ok();

        manox_agent::init();
        // render_to_image only exists on non-headless platforms (Metal
        // texture readback); the window renders offscreen at (-10000,-10000),
        // never flashing on any display.
        let platform = gpui_platform::current_platform(false);
        let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(ExtrasAssetSource));
        cx.update(|cx| {
            gpui_component::init(cx);
            steer_i18n::init();
            steer_agent_chrome_ui::register_fonts(cx);
        });

        // A per-run container id: the scene must not read or leave a real
        // layout file under the user's `~/.steer/layouts`.
        let container = format!("canvas-shot-{}", std::process::id());
        let session_sheet = steer_canvas::id::session_sheet_id(&container);

        let handle = cx
            .open_offscreen_window(size(px(1280.), px(820.)), |window, cx| {
                let shell = agent_ui::chrome_assembly::mount(window, cx);
                let Some(ws) = agent_ui::dispatch::workspace_global() else {
                    panic!("the chrome assembly publishes its workspace");
                };
                // The container's session: a detached store with the
                // catalogue row the title source reads (native container ids
                // are the session id verbatim).
                let store = cx.new(|_| AhpStore::detached());
                store.update(cx, |store, _| {
                    let summary: ahp_types::state::SessionSummary =
                        serde_json::from_value(serde_json::json!({
                            "provider": "pi",
                            "title": "修复登录跳转",
                            "status": 0,
                            "resource": format!("ahp-session:/{container}"),
                            "createdAt": "2026-01-01T00:00:00Z",
                            "modifiedAt": "2026-01-01T00:00:00Z",
                        }))
                        .expect("a well-formed session summary");
                    store.book.summaries.insert(container.clone(), summary);
                });
                ws.update(cx, |ws, cx| {
                    ws.focus_conversation(cx);
                    ws.diagnostic_bind_store(store, container.clone(), cx);
                });
                agent_ui::chrome_assembly::root(shell, window, cx)
            })
            .expect("offscreen window");

        // The canvas syncs on the column's render: one settle pass opens the
        // container (start page + session sheet).
        settle(&mut cx);
        if split {
            cx.update(|cx| {
                if let Some(ws) = agent_ui::dispatch::workspace_global() {
                    ws.update(cx, |ws, cx| {
                        ws.diagnostic_canvas_split_home(&session_sheet, cx)
                    });
                }
            });
            settle(&mut cx);
        }

        let shot = cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot");
        shot.save(&path).expect("save png");
        println!(
            "visual_canvas: wrote {path} ({}x{})",
            shot.width(),
            shot.height()
        );
        // The shot's container id is per-run, but the save pump may have landed
        // its layout under the real state root during the settle passes: it is
        // this harness's only trace, so remove it.
        if let Some(layout) = steer_canvas::persist::layout_path(&container) {
            let _ = std::fs::remove_file(layout);
        }
    }

    fn settle(cx: &mut VisualTestAppContext) {
        for _ in 0..6 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
