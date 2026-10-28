//! The Settings split layout's sidebar nav — the view the chrome shell's
//! sidebar slot renders instead of the session list while Settings is open
//! (see `chrome_assembly::enter_settings_layout`). ZCode-pattern nav: a back
//! control above one grouped list of 30px icon rows, chrome-native metrics
//! (30px rows / 6px radius / 13px labels) with a NEUTRAL active wash — the
//! highlight is the surface, not an accent color. Page selection lives on
//! the Workspace (`settings_nav`); the main column renders the selected
//! page's detail. Row order stays aligned with `views::settings::build_pages`
//! — both sides read `views::settings::NAV`.

use gpui::{App, Pixels, Render, Subscription, Window, prelude::*, px};
use gpui_component::{ActiveTheme as _, Icon, IconName, h_flex, v_flex};

use crate::i18n;
use crate::views::settings::NAV;

/// Nav metrics, chrome-session-row flavored (32px fixed rows, 6px radius,
/// 13px labels — see session_list.rs).
const ROW_H: Pixels = px(30.);
const RADIUS: Pixels = px(6.);

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

        let back_label = i18n::t("settings-back");
        let back = h_flex()
            .id("settings-nav-back")
            .w_full()
            .h(ROW_H)
            .px_2()
            .gap_2()
            .items_center()
            .rounded(RADIUS)
            .text_color(theme.muted_foreground)
            .hover(|this| this.bg(theme.secondary).text_color(theme.foreground))
            .on_click(move |_ev, window, cx| {
                if let Some(ws) = crate::dispatch::workspace_global() {
                    ws.update(cx, |ws, cx| ws.leave_settings(window, cx));
                }
            })
            .child(Icon::new(IconName::ArrowLeft).size_4())
            .child(gpui::div().text_size(px(13.)).child(back_label));

        // The group label doubles as the surface title: one list, one label.
        let mut list = v_flex()
            .w_full()
            .px_2()
            .pb_3()
            .gap_0p5()
            .child(
                gpui::div()
                    .px_2()
                    .pt_4()
                    .pb_1()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.muted_foreground)
                    .child(i18n::t("settings-nav-title")),
            )
            .child(back);
        for (ix, (key, icon)) in NAV.iter().enumerate() {
            let active = ix == selected;
            let row_ws = ws.clone();
            list = list.child(
                h_flex()
                    .id(("settings-nav-row", ix))
                    .w_full()
                    .h(ROW_H)
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(RADIUS)
                    .when(active, |this| {
                        this.bg(theme.secondary).text_color(theme.foreground)
                    })
                    .when(!active, |this| {
                        this.text_color(theme.muted_foreground)
                            .hover(|this| this.bg(theme.secondary).text_color(theme.foreground))
                    })
                    .on_click(move |_ev, _window, cx: &mut App| {
                        row_ws.update(cx, |ws, cx| ws.select_settings_page(ix, cx));
                    })
                    .child(Icon::new(icon.to_owned()).size_3p5().text_color(if active {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    }))
                    .child(
                        gpui::div()
                            .text_size(px(13.))
                            .when(active, |this| this.font_weight(gpui::FontWeight::MEDIUM))
                            .child(i18n::t(key)),
                    ),
            );
        }
        list
    }
}
