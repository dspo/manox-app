//! About window: centered floating dialog with the app icon, a headline
//! app-version line, one muted-label provenance row per pinned stack
//! (manox-app commit, dspo/manox commit, gpui-component / gpui-pre pins), and
//! an OK / Copy action row. Copy writes the structured info block
//! (`version::structured_about`) followed by those rows to the clipboard and
//! closes the window; Escape closes as well. Duplicate window detection keeps a
//! single instance.

use std::sync::Arc;

use gpui::{prelude::*, *};
use gpui_component::{
    StyledExt as _, Theme,
    button::{Button, ButtonVariants as _},
};
use manox_agent::{i18n, version};

use crate::pins;

/// About dialog size. The height has to fit the icon, the headline and one
/// label/value pair per provenance row without clipping; `rows_fit_in_window`
/// pins that.
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
    message: SharedString,
    rows: Vec<InfoRow>,
}

impl AboutWindow {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            app_icon: Arc::new(Image::from_bytes(
                ImageFormat::Png,
                include_bytes!("../resources/app-icon.png").to_vec(),
            )),
            message: SharedString::from(format!(
                "Manox {} ({})",
                env!("CARGO_PKG_VERSION"),
                version::build_type()
            )),
            rows: provenance_rows(),
        }
    }

    fn copy_details(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(details_block(&self.rows)));
        window.remove_window();
    }
}

/// The provenance rows the window renders, in display order. A row whose value
/// could not be resolved at build time is dropped rather than shown blank.
fn provenance_rows() -> Vec<InfoRow> {
    [
        ("manox-app", pins::app_commit()),
        ("dspo/manox", pins::manox_commit()),
        ("gpui-component", pins::gpui_kit_version()),
        ("gpui-pre", pins::gpui_version()),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        Some(InfoRow {
            label,
            value: SharedString::new_static(value?),
        })
    })
    .collect()
}

/// The clipboard block: the runtime's own structured block, whose first lines
/// are a stable format, followed by one line per provenance row so a pasted
/// report carries every identity the window shows.
fn details_block(rows: &[InfoRow]) -> String {
    let mut block = version::structured_about();
    for row in rows {
        block.push_str(&format!("\n{}: {}", row.label, row.value));
    }
    block
}

impl Render for AboutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx);
        let muted = theme.muted_foreground;

        let mut details = div()
            .id("about-details")
            .debug_selector(|| "about-details".into())
            .v_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(gpui::img(self.app_icon.clone()).size_16().flex_none())
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.message.clone()),
            );
        for row in &self.rows {
            details = details
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(SharedString::new_static(row.label)),
                )
                .child(div().text_sm().child(row.value.clone()));
        }

        div()
            .id("about-window")
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

    /// The window must name every pinned stack, in display order; a wrong label
    /// or a dropped row would otherwise only be visible to a human eyeballing
    /// the dialog. Rows whose value this build cannot resolve are absent by
    /// design, so the expectation follows `app_commit` (the one value that needs
    /// git metadata in the manox-app checkout).
    #[test]
    fn provenance_rows_name_every_pinned_stack() {
        let rows = provenance_rows();
        let labels: Vec<_> = rows.iter().map(|row| row.label).collect();
        let mut expected = vec!["dspo/manox", "gpui-component", "gpui-pre"];
        if pins::app_commit().is_some() {
            expected.insert(0, "manox-app");
        }
        assert_eq!(labels, expected);
        assert!(rows.iter().all(|row| !row.value.is_empty()));
    }

    /// A copied report must stay greppable by the runtime's format (its own
    /// lines first, untouched) while carrying the rows unique to this build.
    #[test]
    fn details_block_appends_every_row_to_the_runtime_block() {
        let rows = provenance_rows();
        let block = details_block(&rows);

        let runtime = version::structured_about();
        assert!(
            block.starts_with(&runtime),
            "runtime block must lead: {block}"
        );
        for row in &rows {
            assert!(
                block.contains(&format!("\n{}: {}", row.label, row.value)),
                "missing {}: {block}",
                row.label
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

        assert!(
            details.bottom() <= buttons.top(),
            "provenance rows overlap the actions: {details:?} vs {buttons:?}"
        );
        assert!(
            buttons.bottom() <= window_size.height,
            "the action row is clipped: {buttons:?} in {window_size:?}"
        );
    }
}
