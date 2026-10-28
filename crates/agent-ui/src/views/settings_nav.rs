//! The Settings split layout's sidebar nav — the view the chrome shell's
//! sidebar slot renders instead of the session list while Settings is open
//! (see `chrome_assembly::enter_settings_layout`). The nav IS the settings
//! navigation: page selection lives on the Workspace (`settings_nav`), the
//! main column renders the selected page's detail. Row order stays aligned
//! with `views::settings::build_pages` — both sides read `NAV_KEYS`.

use gpui::{App, Render, Subscription, Window, prelude::*};
use gpui_component::{ActiveTheme as _, h_flex, v_flex};

use crate::i18n;
use crate::views::management_shell::back_control;
use crate::views::settings::NAV_KEYS;

pub(crate) struct SettingsNavView {
    /// Selection lives on the Workspace; the nav re-renders on every
    /// workspace notification (chatty, but a settings-page nav is cheap).
    _ws_sub: Subscription,
}

impl SettingsNavView {
    pub(crate) fn new(cx: &mut gpui::Context<Self>) -> Self {
        let ws =
            crate::dispatch::workspace_global().expect("settings nav mounts after the workspace");
        let _ws_sub = cx.observe(&ws, |_, _, cx| cx.notify());
        Self { _ws_sub }
    }
}

impl Render for SettingsNavView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let Some(ws) = crate::dispatch::workspace_global() else {
            return v_flex();
        };
        let selected = ws.read(cx).settings_nav;

        let back = back_control(&theme, i18n::t("settings-back"), move |_ev, window, cx| {
            if let Some(ws) = crate::dispatch::workspace_global() {
                ws.update(cx, |ws, cx| ws.leave_settings(window, cx));
            }
        });

        let mut list = v_flex().w_full().p_1p5().gap_0p5().child(back);
        for (ix, key) in NAV_KEYS.iter().enumerate() {
            let active = ix == selected;
            let row_ws = ws.clone();
            list = list.child(
                h_flex()
                    .id(("settings-nav-row", ix))
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .rounded(theme.radius)
                    .items_center()
                    .when(active, |this| this.bg(theme.accent.opacity(0.08)))
                    .on_click(move |_ev, _window, cx: &mut App| {
                        row_ws.update(cx, |ws, cx| {
                            ws.settings_nav = ix;
                            cx.notify();
                        });
                    })
                    .child(
                        gpui::div()
                            .text_size(gpui::px(13.))
                            .when(active, |this| {
                                this.font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.accent)
                            })
                            .child(i18n::t(key)),
                    ),
            );
        }
        list
    }
}
