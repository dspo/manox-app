//! Visual acceptance for the Settings split layout (harness=false so it runs
//! on the main thread; MacPlatform requires it for offscreen rendering; the
//! Metal readback is macOS-only, everything else prints a skip line).
//!
//! Run with `SETTINGS_SHOT=/tmp/settings.png cargo test -p agent-ui
//! --features test-support --test visual_settings`; `SETTINGS_SHOT_2` (any
//! value) also captures the second page (Integrations) after a nav switch.
//! Mounts the REAL chrome assembly (Shell + Workspace through
//! `chrome_assembly::mount`), enters Settings through the production
//! `enter_settings` path — the chrome sidebar must swap its session list for
//! the settings nav while the main column carries the selected page's detail
//! — then switches pages through the production nav state (`settings_nav`).
//!
//! Without `SETTINGS_SHOT` the binary prints a skip line and exits green, so
//! the test-support gate stays quiet in CI runs that only compile it.

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

    use agent_ui::assets::ExtrasAssetSource;
    use gpui::{VisualTestAppContext, px, size};

    pub fn main() {
        let Ok(path) = std::env::var("SETTINGS_SHOT") else {
            eprintln!("visual_settings: SETTINGS_SHOT not set, skipping");
            return;
        };
        let second = std::env::var("SETTINGS_SHOT_2").is_ok();

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

        let shell_slot: std::cell::RefCell<Option<gpui::Entity<steer_agent_chrome_ui::Shell>>> =
            std::cell::RefCell::new(None);
        let capture_shell = &shell_slot;
        let handle = cx
            .open_offscreen_window(size(px(1280.), px(820.)), |window, cx| {
                let shell = agent_ui::chrome_assembly::mount(window, cx);
                *capture_shell.borrow_mut() = Some(shell.clone());
                if let Some(ws) = agent_ui::dispatch::workspace_global() {
                    ws.update(cx, |ws, cx| ws.enter_settings(window, cx));
                }
                agent_ui::chrome_assembly::root(shell, window, cx)
            })
            .expect("offscreen window");
        // Let the fonts/layout/store pumps settle before the capture.
        for _ in 0..5 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let shell = shell_slot.into_inner().expect("shell captured");
        cx.read(|cx| {
            assert!(
                shell.read(cx).sidebar_surface.is_some(),
                "the settings nav must own the chrome sidebar slot"
            );
        });

        let shot = cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot");
        shot.save(&path).expect("save png");
        println!(
            "visual_settings: wrote {path} ({}x{})",
            shot.width(),
            shot.height()
        );

        if second {
            cx.update_window(handle.into(), |_root, _window, cx| {
                if let Some(ws) = agent_ui::dispatch::workspace_global() {
                    ws.update(cx, |ws, cx| ws.select_settings_page(1, cx));
                }
            })
            .expect("window");
            for _ in 0..3 {
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let path2 = format!("{path}.p2.png");
            let shot2 = cx
                .capture_screenshot(handle.into())
                .expect("capture page 2");
            shot2.save(&path2).expect("save png 2");
            println!(
                "visual_settings: wrote {path2} ({}x{})",
                shot2.width(),
                shot2.height()
            );
        }
    }
}
