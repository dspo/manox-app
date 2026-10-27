//! Visual acceptance for the Settings floating overlay (harness=false so it
//! runs on the main thread; MacPlatform requires it for offscreen rendering;
//! the Metal readback is macOS-only, everything else prints a skip line).
//!
//! Run with `SETTINGS_SHOT=/tmp/settings.png cargo test -p agent-ui
//! --features test-support --test visual_settings`. Mounts the REAL
//! `Workspace` (empty hero column underneath), enters Settings through the
//! production `enter_settings` path and captures the settled overlay: the
//! dim mask, the centered rounded card and the upstream settings shell
//! inside it. Beyond the pixels, the falsifiable assertions are luminance
//! ones — the card reads bright at the window center while the dimmed
//! column stays visibly darker at the corners (the 32%-black mask's work).
//!
//! Without `SETTINGS_SHOT` the binary prints a skip line and exits green,
//! so the test-support gate stays quiet in CI runs that only compile it.

#[cfg(any(not(target_os = "macos"), not(feature = "test-support")))]
fn main() {
    eprintln!("visual_settings: skipped (needs macOS Metal readback + test-support)");
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
fn main() {
    macos::main();
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
mod macos {
    use std::sync::Arc;

    use agent_ui::{Workspace, assets::ExtrasAssetSource};
    use gpui::{AppContext as _, VisualTestAppContext, px, size};

    pub fn main() {
        let Ok(path) = std::env::var("SETTINGS_SHOT") else {
            eprintln!("visual_settings: SETTINGS_SHOT not set, skipping");
            return;
        };

        manox_agent::init();
        // render_to_image only exists on non-headless platforms (Metal
        // texture readback); the window renders offscreen at
        // (-10000,-10000), never flashing on any display.
        let platform = gpui_platform::current_platform(false);
        let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(ExtrasAssetSource));
        cx.update(|cx| {
            gpui_component::init(cx);
            steer_i18n::init();
            steer_agent_chrome_ui::register_fonts(cx);
        });

        let handle = cx
            .open_offscreen_window(size(px(1280.), px(820.)), |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                workspace.update(cx, |ws, cx| ws.enter_settings(window, cx));
                cx.new(|cx| gpui_component::Root::new(workspace, window, cx))
            })
            .expect("offscreen window");
        cx.run_until_parked();

        // Let the font/layout pass settle, then pump the entrance tween to
        // its settled state: offscreen there is no platform frame loop, and
        // `request_animation_frame` callbacks fire only through
        // `simulate_next_frame` — sleeping alone would capture the first
        // frame (delta=0, the card fully faded out).
        for _ in 0..3 {
            cx.update_window(handle.into(), |_view, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("window");
            cx.run_until_parked();
        }
        for _ in 0..40 {
            cx.update_window(handle.into(), |_view, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("window");
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(12));
        }

        let shot = cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot");

        // Geometry through pixels (VisualTestAppContext exposes no
        // debug_bounds): the card is a bright rounded panel on a dimmed
        // column, so the window center must be clearly brighter than the
        // dimmed corners, and the card must not swallow the window — the
        // corner column stays visible through the 32%-black mask.
        let luminance = |x: u32, y: u32| -> f32 {
            let p = shot.get_pixel(x, y);
            0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
        };
        let (w, h) = (shot.width(), shot.height());
        let center = luminance(w / 2, h / 2);
        let corner = (luminance(30, 30) + luminance(w - 30, 30)) / 2.;
        assert!(
            center - corner > 30.,
            "the floating card must sit bright over a dimmed column: center={center} corner={corner}"
        );
        assert!(
            corner < 200.,
            "the mask must dim the column underneath: corner={corner}"
        );

        shot.save(&path).expect("save png");
        println!(
            "visual_settings: wrote {path} ({}x{})",
            shot.width(),
            shot.height()
        );
    }
}
