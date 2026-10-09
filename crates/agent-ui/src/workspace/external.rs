//! External-app launches and the browser-tab lifecycle.
//!
//! A launch rides the Tools (工具) menu through an App-level action: it runs
//! on a background thread and reports through a notification — the launched
//! app detaches and keeps running on its own, so nothing about it belongs to
//! the shell.
//!
//! Browser tabs are the workspace's webview registry: `restore_browser_tab`
//! builds a view and routes it in the process-wide browser host; the
//! right-pane tab (see `tool_tabs`) owns the label ticker and the show/hide of
//! the native subview.

use super::*;

impl Workspace {
    /// Launch ChatGPT.app through cx's injection path with the provider + model
    /// picked in the macOS Tools (工具) → ChatGPT.app menu cascade. The launch blocks
    /// (config load, model catalog build, CDP injection — up to ~20s), so it runs
    /// on a background thread and reports the outcome as a notification; the app
    /// itself detaches and keeps running independently of manox.
    pub fn launch_chatgpt_app(
        &mut self,
        provider: String,
        model: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let launch_provider = provider.clone();
            let launch_model = model.clone();
            let result = cx
                .background_spawn(async move {
                    steer_ext_agents::launch_chatgpt_app(&launch_provider, &launch_model)
                })
                .await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(()) => {
                    window.push_notification(
                        Notification::success(i18n::t_str(
                            "chatgpt-app-launched",
                            &[("provider", &provider), ("model", &model)],
                        )),
                        cx,
                    );
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        provider = %provider,
                        model = %model,
                        "ChatGPT.app launch failed"
                    );
                    window.push_notification(
                        Notification::error(format!(
                            "{}: {e}",
                            i18n::t("chatgpt-app-launch-failed")
                        )),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Launch VS Code with injections resolved from the persisted
    /// `vscode_app:` settings (Settings → External Tools (外部工具) → Visual Studio Code.app):
    /// Claude Code Extension block → ANTHROPIC_* env; Codex Extension block →
    /// CODEX_HOME + config.toml; both off → plain open. `folder` is `Some` when
    /// the launch should open a directory. Same background-spawn + notification
    /// shape as [`Self::launch_chatgpt_app`]; the cx injection path may block on
    /// login-shell env resolution and — when VS Code is already running — on the
    /// restart confirmation + graceful quit wait.
    pub fn launch_vscode_app(
        &mut self,
        folder: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    steer_ext_agents::launch_vscode_app_from_settings(folder.as_deref())
                })
                .await;
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(()) => {
                    window.push_notification(
                        Notification::success(i18n::t("vscode-app-launched")),
                        cx,
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "VS Code launch failed");
                    window.push_notification(
                        Notification::error(format!(
                            "{}: {e}",
                            i18n::t("vscode-app-launch-failed")
                        )),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// Build a browser tab's webview (app-restart path and the pane's own
    /// "new browser tab"): the view goes into `browser_views` and its routing
    /// entry into the process-wide host; the caller places the tab.
    pub fn restore_browser_tab(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BrowserTabId {
        let url = if url.is_empty() {
            crate::views::browser_view::DEFAULT_URL
        } else {
            url
        };
        let tab_id = crate::views::browser_view::allocate_tab_id();
        let view =
            cx.new(|cx| crate::views::browser_view::BrowserView::new(tab_id, url, window, cx));
        self.browser_views.insert(tab_id, view);
        if let Some(host) = crate::browser_host::WorkspaceBrowserHost::concrete() {
            host.register_ui_tab(tab_id);
        }
        tab_id
    }

    /// Open a browser tab navigated to `url` (the host's own open path — the
    /// agent's web tooling). The webview is built and routed here, then
    /// surfaced as a right-pane tab; a build with no live pane still leaves
    /// the tab drivable, so the tool call never depends on the UI. The
    /// webview is built untrusted: no Tauri command surface, only the
    /// notify/inbound bridges.
    pub fn open_browser_tab(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> BrowserTabId {
        let url = if url.is_empty() {
            crate::views::browser_view::DEFAULT_URL
        } else {
            url
        };
        let tab_id = self.restore_browser_tab(url, window, cx);
        let tab = crate::tool_tabs::adopt_browser_tab(cx.entity(), tab_id, url);
        // The pane open runs on the event path: this call arrives inside a
        // Workspace update (the host's dispatch), and the tab's own `open`
        // reaches the workspace again.
        let logged_url = url.to_string();
        cx.defer(move |cx| {
            // No live pane: the call still works for the agent (the webview
            // exists and the host routes to it), but nothing shows the tab —
            // log it so "the agent says it opened a browser" has a trail.
            if !crate::chrome_assembly::open_tool_tab(tab, cx) {
                tracing::debug!(
                    tab = ?tab_id,
                    url = %logged_url,
                    "browser tab opened with no live right pane"
                );
            }
        });
        tab_id
    }

    /// Drop a browser tab: the view (whose `Drop` hides and detaches the
    /// native subview) and the host's routing entry. No-op if the id is not
    /// live.
    pub fn close_browser_tab(&mut self, tab_id: BrowserTabId, cx: &mut Context<Self>) {
        if self.browser_views.remove(&tab_id).is_none() {
            return;
        }
        // Reclaim the host's routing entry so a late notify for this tab finds
        // no route (no orphaned oneshot). The host's own close path reclaims
        // first then calls us — in that direction `reclaim_routes` is a no-op;
        // this call covers the UI-close direction.
        if let Some(host) = crate::browser_host::WorkspaceBrowserHost::concrete() {
            host.reclaim_routes(tab_id);
        }
        cx.notify();
    }
}
