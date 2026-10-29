//! About window: centered floating dialog with the app icon, the headline
//! `Steer.app v<version>+<short commit>`, and the provenance block — every
//! row is a label/value pair laid out on one continuous vertical axis (the
//! labels right-align onto it, the values left-align off it): the OS (with
//! the chip's marketing name where the platform reports one), the rustc the
//! binary was compiled by, and the pinned stacks (manox runtime,
//! gpui-component, gpui-pre). The OK / Copy action row sits at the bottom;
//! Copy writes the structured info block (`version::structured_about`), the
//! full app commit, and one `label: value` line per row to the clipboard and
//! closes the window; Escape closes as well. Duplicate window detection
//! keeps a single instance.

use std::sync::Arc;

use gpui::{prelude::*, *};
use gpui_component::{
    StyledExt as _, Theme,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use manox_agent::version;
use steer_i18n as i18n;

use crate::pins;

/// About dialog size. The height has to fit the icon, the headline and the
/// provenance axis without clipping; `rows_fit_in_window` pins that.
const WINDOW_WIDTH: f32 = 440.;
const WINDOW_HEIGHT: f32 = 440.;

/// One provenance row: a stack identifier label and its build-time value.
/// Labels are package/product identifiers, so they stay untranslated.
struct InfoRow {
    label: &'static str,
    value: SharedString,
}

struct AboutWindow {
    focus_handle: FocusHandle,
    app_icon: Arc<Image>,
    headline: SharedString,
    rows: Vec<InfoRow>,
}

/// `Steer.app v<version>+<short commit>` — the app's own identity line; the
/// short commit is the only face the old `steer desktop` row's value survives
/// in. A build without git metadata stays on the bare version.
fn headline() -> SharedString {
    match pins::app_commit() {
        Some(commit) => format!(
            "Steer.app v{}+{}",
            env!("CARGO_PKG_VERSION"),
            pins::short_commit(commit)
        )
        .into(),
        None => format!("Steer.app v{}", env!("CARGO_PKG_VERSION")).into(),
    }
}

impl AboutWindow {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            app_icon: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../resources/app-icon.png").to_vec(),
            )),
            headline: headline(),
            rows: provenance_rows(),
        }
    }

    fn copy_details(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(details_block(&self.rows)));
        window.remove_window();
    }
}

/// The provenance rows the window renders, in display order. A row whose value
/// could not be resolved is dropped rather than shown blank.
fn provenance_rows() -> Vec<InfoRow> {
    let mut rows = Vec::new();
    let mut push = |label, value: Option<SharedString>| {
        if let Some(value) = value {
            rows.push(InfoRow { label, value });
        }
    };
    let short = |commit| SharedString::from(pins::short_commit(commit).to_string());
    push("OS", Some(os_label()));
    push("rustc", pins::rustc_version().map(SharedString::from));
    push("manox runtime", pins::manox_commit().map(short));
    push(
        "gpui-component",
        pins::gpui_kit_pin().map(SharedString::from),
    );
    push("gpui-pre", pins::gpui_pin().map(SharedString::from));
    rows
}

/// The OS row's value: the platform and, where the hardware names itself, its
/// marketing name — `macOS · Apple M4 Pro`. A platform without a brand string
/// falls back to its arch.
fn os_label() -> SharedString {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    match platform_cpu() {
        Some(cpu) => format!("{os} · {cpu}"),
        None => format!("{os} {}", std::env::consts::ARCH),
    }
    .into()
}

/// The running machine's marketing chip name (`Apple M4 Pro`), where the
/// platform exposes one.
#[cfg(target_os = "macos")]
fn platform_cpu() -> Option<String> {
    let mut value = [0u8; 128];
    let mut len = value.len();
    let rc = unsafe {
        libc::sysctlbyname(
            c"machdep.cpu.brand_string".as_ptr(),
            value.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let end = value.iter().position(|&b| b == 0)?;
    let name = std::str::from_utf8(&value[..end]).ok()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(not(target_os = "macos"))]
fn platform_cpu() -> Option<String> {
    None
}

/// The clipboard block: the runtime's own structured block, whose first lines
/// are a stable format, then the full app commit the headline only
/// abbreviates, then one line per provenance row so a pasted report carries
/// every identity the window shows.
fn details_block(rows: &[InfoRow]) -> String {
    let mut block = version::structured_about();
    if let Some(commit) = pins::app_commit() {
        block.push_str(&format!("\nsteer-app commit: {commit}"));
    }
    for row in rows {
        block.push_str(&format!("\n{}: {}", row.label, row.value));
    }
    block
}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);

        // The provenance block: labels right-align onto one continuous
        // vertical rule, values left-align off it — every row sits on the
        // same axis, so a scan down either column stays straight. Both
        // columns re-pin their text alignment: the dialog root centers text
        // (headline, buttons), and the inheritance would otherwise center
        // every cell inside its column and float it off the axis.
        let keys = v_flex()
            .gap_1p5()
            .items_end()
            .text_right()
            .children(self.rows.iter().map(|row| {
                gpui::div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::new_static(row.label))
            }));
        let values = v_flex()
            .gap_1p5()
            .text_left()
            .children(self.rows.iter().map(|row| {
                gpui::div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .child(row.value.clone())
            }));
        let axis = h_flex()
            .debug_selector(|| "about-axis".into())
            .items_stretch()
            .child(keys.pr_3())
            .child(gpui::div().w(px(1.)).bg(theme.border))
            .child(values.pl_3());

        let details = div()
            .id("about-details")
            .debug_selector(|| "about-details".into())
            .v_flex()
            .w_full()
            .gap_5()
            .items_center()
            .child(gpui::img(self.app_icon.clone()).size_16().flex_none())
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.headline.clone()),
            )
            .child(axis);

        div()
            .id("about-window")
            .debug_selector(|| "about-window".into())
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, window, _cx| {
                if ev.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .p_4()
            .pt_10()
            .gap_4()
            .text_center()
            .justify_between()
            .child(details)
            .child(
                div()
                    .id("about-buttons")
                    .debug_selector(|| "about-buttons".into())
                    .h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        Button::new("about-ok")
                            .label(i18n::t("about-ok"))
                            .ghost()
                            .flex_1()
                            .on_click(|_ev, window, _cx| {
                                window.remove_window();
                            }),
                    )
                    .child(
                        Button::new("about-copy")
                            .label(i18n::t("about-copy"))
                            .primary()
                            .flex_1()
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.copy_details(window, cx);
                            })),
                    ),
            )
    }
}

pub fn open_about_window(cx: &mut App) {
    // Don't open a second About window.
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<AboutWindow>())
    {
        let _ = existing.update(cx, |_, window, _cx| {
            window.activate_window();
        });
        return;
    }

    // Compute bounds before spawning so we can use &App.
    let bounds = WindowBounds::centered(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);

    cx.spawn(async move |cx| {
        let options = WindowOptions {
            window_bounds: Some(bounds),
            is_resizable: false,
            is_minimizable: false,
            kind: WindowKind::Floating,
            ..Default::default()
        };
        let _handle = cx
            .open_window(options, |window, cx| {
                window.set_window_title(i18n::t("about-title").as_str());
                let about = cx.new(AboutWindow::new);
                let focus = about.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                about
            })
            .expect("failed to open about window");
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // Imported by name, not via `super::*`: the file glob-imports gpui, whose
    // re-exported `test` attribute macro would shadow the built-in `#[test]`.
    use manox_agent::version;

    use gpui::{TestAppContext, VisualTestContext, px, size};

    use super::{AboutWindow, WINDOW_HEIGHT, WINDOW_WIDTH, details_block, provenance_rows};
    use crate::pins;

    /// The window must name the platform, the toolchain, and every pinned
    /// stack, in display order; a wrong label or a dropped row would otherwise
    /// only be visible to a human eyeballing the dialog. Rows whose value this
    /// build cannot resolve are absent by design, so the expectation follows
    /// the pins that need git metadata or a lockfile.
    #[test]
    fn provenance_rows_name_the_platform_toolchain_and_pins() {
        let rows = provenance_rows();
        let labels: Vec<_> = rows.iter().map(|row| row.label).collect();
        let mut expected = vec!["OS"];
        if pins::rustc_version().is_some() {
            expected.push("rustc");
        }
        if pins::manox_commit().is_some() {
            expected.push("manox runtime");
        }
        expected.extend(["gpui-component", "gpui-pre"]);
        assert_eq!(labels, expected);
    }

    /// The OS row names the running platform and carries the chip's marketing
    /// name where the platform reports one — never an empty value.
    #[test]
    fn the_os_row_names_the_platform() {
        let rows = provenance_rows();
        let os = &rows
            .iter()
            .find(|row| row.label == "OS")
            .expect("the OS row is unconditional")
            .value;
        assert!(!os.is_empty());
        assert!(
            os.starts_with("macOS") || os.starts_with("Linux") || os.starts_with("windows"),
            "the OS row names the running platform: {os}"
        );
    }

    /// A copied report must stay greppable by the runtime's format (its own
    /// lines first, untouched) while carrying the app commit and the rows
    /// unique to this build.
    #[test]
    fn details_block_appends_the_app_commit_and_every_row_to_the_runtime_block() {
        let rows = provenance_rows();
        let block = details_block(&rows);

        let runtime = version::structured_about();
        assert!(
            block.starts_with(&runtime),
            "runtime block must lead: {block}"
        );
        if let Some(commit) = pins::app_commit() {
            assert!(
                block.contains(&format!("\nsteer-app commit: {commit}")),
                "the full app commit rides the report: {block}"
            );
        }
        for row in &rows {
            assert!(
                block.contains(&format!("\n{}: {}", row.label, row.value)),
                "missing {}: {block}",
                row.label
            );
        }
    }

    /// The headline names the app, its version, and — where the build has git
    /// metadata — the short commit it was built from.
    #[test]
    fn the_headline_names_the_app_version_and_commit() {
        let headline = super::headline();
        assert!(
            headline.starts_with("Steer.app v"),
            "the headline names the app and version: {headline}"
        );
        if let Some(commit) = pins::app_commit() {
            assert!(
                headline.ends_with(&format!("+{}", pins::short_commit(commit))),
                "the short commit rides the headline: {headline}"
            );
        }
    }

    /// The dialog is sized once and never resizable, so one more row or a taller
    /// text style would push the action row off the bottom edge. Measure the
    /// rendered frame instead of trusting the pixel arithmetic; a provenance
    /// value too wide for the row shows up here as well, since it wraps and
    /// makes the block taller.
    #[gpui::test]
    fn rows_fit_in_window(cx: &mut TestAppContext) {
        let window_size = size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT));
        // The dialog reads its colors from the global component theme, which the
        // real binary installs at startup.
        cx.update(gpui_component::init);
        let window = cx.open_window(window_size, |_, cx| AboutWindow::new(cx));
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let details = cx
            .debug_bounds("about-details")
            .expect("the provenance block must be laid out");
        let buttons = cx
            .debug_bounds("about-buttons")
            .expect("the action row must be laid out");
        let dialog = cx
            .debug_bounds("about-window")
            .expect("the dialog must be laid out");
        let axis = cx
            .debug_bounds("about-axis")
            .expect("the provenance axis must be laid out");

        assert!(
            details.bottom() <= buttons.top(),
            "provenance rows overlap the actions: {details:?} vs {buttons:?}"
        );
        // The axis is the dialog's spine: every row hangs on one continuous
        // rule, so it renders taller than one row and narrower than the
        // content box it centers in.
        assert!(
            axis.size.height > px(40.),
            "the axis spans the provenance rows: {axis:?}"
        );
        // The action row is pinned to the content box's bottom edge, so this
        // only fails once the rows above have pushed it through the padding.
        assert!(
            buttons.bottom() <= dialog.bottom() - px(16.),
            "the action row is pushed out of the padded content box: {buttons:?} vs {dialog:?}"
        );
    }
}
