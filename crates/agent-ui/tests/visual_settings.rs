//! Visual acceptance for the gpui-shell Settings floating dialog (harness=false
//! so it runs on the main thread; MacPlatform requires it for offscreen
//! rendering; the Metal readback is macOS-only, everything else prints a skip
//! line).
//!
//! Run with `SETTINGS_SHOT=/tmp/settings.png cargo test -p agent-ui
//! --features test-support --test visual_settings`. Mounts the REAL wiring the
//! app uses — `settings_shell::init` (the `settingsNative` HostModule + the
//! script runtime) and `wrap_root` over a real `Workspace` — then opens the
//! dialog through the production `open()` path and captures the native
//! floating dialog: backdrop, focus-trapped card, script-driven settings
//! pages. Falsifiable assertions: the dialog is mounted on the ShellRoot, and
//! the pixels show a bright card over a dimmed backdrop.
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

    use agent_ui::{Workspace, assets::ExtrasAssetSource, settings_shell};
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
            settings_shell::init(cx);
        });

        let handle = cx
            .open_offscreen_window(size(px(1280.), px(820.)), |window, cx| {
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                let shell_root = settings_shell::wrap_root(workspace, window, cx);
                cx.new(|cx| gpui_component::Root::new(shell_root, window, cx))
            })
            .expect("offscreen window");
        cx.run_until_parked();

        // Open through the production path (the Settings launcher routes
        // here), then pump the dialog's entrance to its settled state —
        // offscreen there is no platform frame loop, and animation frames
        // fire only through `simulate_next_frame`.
        cx.update_window(handle.into(), |_root, window, cx| {
            settings_shell::open(window, cx);
        })
        .expect("window");
        for _ in 0..40 {
            cx.update_window(handle.into(), |_root, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("window");
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(12));
        }

        cx.read(|cx| {
            assert!(
                settings_shell::dialog_open(cx),
                "the settings dialog must be mounted on the ShellRoot"
            );
        });

        let shot = cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot");

        // Pixels: a bright centered card over a dimmed backdrop (the dialog's
        // own backdrop + the workspace underneath).
        let luminance = |x: u32, y: u32| -> f32 {
            let p = shot.get_pixel(x, y);
            0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
        };
        let (w, h) = (shot.width(), shot.height());
        let center = luminance(w / 2, h / 2);
        let corner = (luminance(30, 30) + luminance(w - 30, 30)) / 2.;
        assert!(
            center - corner > 30.,
            "the floating card must sit bright over the dimmed backdrop: center={center} corner={corner}"
        );
        assert!(
            corner < 210.,
            "the backdrop must dim what is underneath: corner={corner}"
        );

        shot.save(&path).expect("save png");
        println!(
            "visual_settings: wrote {path} ({}x{})",
            shot.width(),
            shot.height()
        );
        // Drop the shell host while the thread is alive; TLS-stage drops of
        // the QuickJS runtime abort the process.
        settings_shell::teardown();
    }
}
