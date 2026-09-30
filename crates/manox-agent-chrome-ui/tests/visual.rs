//! Visual acceptance (harness=false, main thread): gpui's official
//! offscreen render — `VisualTestAppContext` + Metal texture readback
//! (`capture_screenshot`), no screen-recording TCC permission needed.
//!
//! Run with `CHROME_SHOT=/tmp/chrome.png cargo test -p manox-agent-chrome-ui
//! --test visual`; `CHROME_RIGHT=1` expands the right pane with a dummy tab
//! (no PTY/webview — those need a real window), `CHROME_PANEL=1` the bottom
//! dock. macOS-only: the readback is a Metal texture and the harness's
//! imports (`VisualTestAppContext`) exist only on that platform.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("visual: the chrome harness is macOS-only (Metal readback), skipping");
}

#[cfg(target_os = "macos")]
fn main() {
    macos::main();
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Arc;

    use gpui::{
        AnyView, AppContext as _, IntoElement, ParentElement, Styled, VisualTestAppContext, px,
        size,
    };
    use gpui_component::Root;
    use manox_agent_chat_ui::conversation::{ConvItem, ToolCallItem};
    use manox_agent_chat_ui::host::noop_host;
    use manox_agent_chat_ui::views::message::MessageItem;
    use manox_agent_chrome_ui::right_pane::TabStore;
    use manox_agent_chrome_ui::session_list::SessionStatus;
    use manox_agent_chrome_ui::shell::SessionRow;
    use manox_agent_chrome_ui::theme::Icon;
    use manox_agent_chrome_ui::{
        CustomizationRow, FixedRow, HostHooks, MainSurface, PanelSurface, Shell, ShellConfig,
        ToolTab, ToolTabFactory, icons, register_fonts,
    };

    pub fn main() {
        let Ok(path) = std::env::var("CHROME_SHOT") else {
            eprintln!("visual: CHROME_SHOT not set, skipping");
            return;
        };
        let right = std::env::var("CHROME_RIGHT").is_ok();
        let panel = std::env::var("CHROME_PANEL").is_ok();

        manox_agent::init();
        // render_to_image only exists on non-headless platforms (Metal texture
        // readback); the window renders offscreen at (-10000,-10000), never
        // flashing on any display.
        let platform = gpui_platform::current_platform(false);
        let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(EmptyAssets));
        cx.update(|cx| {
            gpui_component::init(cx);
            manox_i18n::init();
            register_fonts(cx);
            // Real threads for a populated sidebar (one synchronous scan; no
            // event pump — a static frame is the point).
            manox_agent::thread_store::refresh_thread_list();
        });

        let mut shell_slot = None;
        let handle = cx
            .open_offscreen_window(size(px(1280.), px(820.)), |window, cx| {
                let main: AnyView = cx.new(ChatPreviewStub::new).into();
                let shell = cx.new(|cx| Shell::new(shell_config(main), window, cx));
                shell_slot = Some(shell.clone());
                shell.update(cx, |shell, _cx| {
                    shell.set_sessions(snapshot_rows());
                });
                if right || panel {
                    shell.update(cx, |shell, cx| {
                        if right {
                            shell.open_right("dummy", window, cx);
                            if std::env::var("CHROME_SWITCH").is_ok() {
                                shell.open_right("second", window, cx);
                            }
                        }
                        if panel {
                            shell.toggle_panel(window, cx);
                        }
                        cx.notify();
                    });
                }
                cx.new(|cx| Root::new(shell, window, cx))
            })
            .expect("offscreen window");

        // Let the font/layout pass settle before the capture. The extra
        // `simulate_next_frame` calls matter: tests have no platform frame
        // loop, so a component that asks for one more frame (the tab
        // indicator does, to read its prepaint bounds) would otherwise never
        // get it and the capture would show a half-resolved first frame.
        for i in 0..5 {
            if i > 0 {
                let _ = cx.update_window(handle.into(), |_view, window, cx| {
                    window.simulate_next_frame(cx);
                });
            }
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        if std::env::var("CHROME_SWITCH").is_ok() {
            // Now that the layout has settled, switch back to the FIRST tab —
            // the long travel the animation exists for.
            let shell = shell_slot.clone().expect("shell");
            cx.update(|cx| {
                shell.update(cx, |shell, cx| {
                    shell.right.update(cx, |pane, cx| {
                        pane.activate_tab("dummy-0", cx);
                    });
                    cx.notify();
                });
            });
            // Tests have no platform frame loop: `request_animation_frame`
            // schedules via on_next_frame, which only fires through
            // `simulate_next_frame`. Sleeping alone advances no animation.
            for _ in 0..40 {
                cx.update_window(handle.into(), |_view, window, cx| {
                    window.simulate_next_frame(cx);
                })
                .expect("window");
                cx.run_until_parked();
                std::thread::sleep(std::time::Duration::from_millis(12));
            }
        }

        let shot = cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot");
        shot.save(&path).expect("save png");
        println!("visual: wrote {path} ({}x{})", shot.width(), shot.height());
    }

    fn shell_config(main: AnyView) -> ShellConfig {
        ShellConfig {
            main: Arc::new(MainSeat { view: main }),
            tool_kinds: {
                let mut kinds: Vec<Arc<dyn ToolTabFactory>> = vec![Arc::new(DummyKind)];
                // Only under CHROME_SWITCH, so the baseline shot's quick-action
                // list is unchanged by a diagnostic-only kind.
                if std::env::var("CHROME_SWITCH").is_ok() {
                    kinds.push(Arc::new(SecondKind));
                }
                kinds
            },
            panel_surface: Some(Arc::new(DummyPanel)),
            brand: None,
            fixed_rows: vec![
                FixedRow {
                    icon: icons::CALENDAR,
                    label: manox_i18n::t("chrome-sidebar-automations"),
                    badge: None,
                },
                FixedRow {
                    icon: icons::COMMENT_DISCUSSION,
                    label: manox_i18n::t("chrome-sidebar-chats"),
                    badge: None,
                },
            ],
            // Count badges are visual fixtures of the replica, not live data.
            customizations: vec![
                CustomizationRow {
                    icon: icons::HOME,
                    label: manox_i18n::t("chrome-sidebar-overview"),
                    count: None,
                },
                CustomizationRow {
                    icon: icons::EXTENSIONS,
                    label: manox_i18n::t("chrome-sidebar-plugins"),
                    count: None,
                },
                CustomizationRow {
                    icon: icons::SETTINGS_GEAR,
                    label: manox_i18n::t("chrome-sidebar-mcp"),
                    count: None,
                },
                CustomizationRow {
                    icon: icons::WAND,
                    label: manox_i18n::t("chrome-sidebar-skills"),
                    count: None,
                },
            ],
            hooks: HostHooks {
                // Live nav edges: the screenshot is the only acceptance face
                // for the titlebar's enabled-arrow state.
                nav_avail: Some(Box::new(|_| manox_agent_chrome_ui::shell::NavAvail {
                    back: true,
                    forward: false,
                })),
                ..Default::default()
            },
        }
    }

    /// Static session snapshot from the real store, seeded before the first
    /// render (the pump-less harness never gets a second push).
    fn snapshot_rows() -> Vec<SessionRow> {
        let store = manox_agent::thread_store::global();
        store
            .read(|st| {
                st.summaries()
                    .iter()
                    .filter(|t| !t.archived && t.depth == 0 && t.superseded_by.is_none())
                    .map(|t| (t.clone(), st.is_running(&t.id)))
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .map(|(t, running)| SessionRow {
                id: t.id.clone(),
                title: t
                    .title_override
                    .clone()
                    .or_else(|| t.title.clone())
                    .unwrap_or_else(|| t.summary.clone()),
                workspace: t
                    .project
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Chats")
                    .to_string(),
                status: if t.errored {
                    SessionStatus::Errored
                } else if running {
                    SessionStatus::Running
                } else if t.has_unread {
                    SessionStatus::Unread
                } else {
                    SessionStatus::Idle
                },
                tag: None,
                team_leader: false,
                updated_at: t.updated_at,
                // Team rows are filtered out above, so every row is its own
                // sort unit (production stamping lives in project_forest).
                sort_stamp: t.updated_at,
                pinned: t.pinned,
                archived: t.archived,
            })
            .take(12)
            .collect()
    }

    // ── main-surface seat ─────────────────────────────────────────────────────

    /// A minimal seeded message list (the chat pipeline's real `MessageItem`s),
    /// so the acceptance capture shows chat content inside the shell.
    struct ChatPreviewStub {
        items: Vec<gpui::Entity<MessageItem>>,
        list_state: gpui::ListState,
    }

    impl ChatPreviewStub {
        fn new(cx: &mut gpui::Context<Self>) -> Self {
            let mk = |cx: &mut gpui::Context<Self>, item: ConvItem, role: &str| {
                cx.new(|_| MessageItem::new(item, role.to_string(), 0, noop_host()))
            };
            let items = vec![
            mk(
                cx,
                ConvItem::User {
                    text: "聊聊这个仓库的结构".into(),
                    images: Vec::new(),
                    meta: None,
                },
                "",
            ),
            mk(
                cx,
                ConvItem::ToolCall(ToolCallItem {
                    id: "seed-read".into(),
                    name: "read_file".into(),
                    title: "read_file(PLAN-CHROME-CHAT-SPLIT.md)".into(),
                    status: manox_agent::ToolCallStatus::Success,
                    output: "# 拆分计划…".into(),
                    is_error: false,
                    input: serde_json::Value::Null,
                    streaming: false,
                    collapsed: false,
                    user_toggled: true,
                    panel: None,
                }),
                "",
            ),
            mk(
                cx,
                ConvItem::Assistant {
                    text: "主栏由 **manox-agent-chat-ui** 的消息管线渲染。\n\n- markdown 列表\n- `代码` 与代码块\n\n```rust\nlet shell = chrome::Shell::new(config, window, cx);\n```".into(),
                    streaming: false,
                    token_usage: None,
                    activity_header: true,
                    entry_id: None,
                    fork_unavailable: None,
                },
                "GLM-5.3",
            ),
        ];
            let list_state =
                gpui::ListState::new(items.len(), gpui::ListAlignment::Bottom, px(400.));
            Self { items, list_state }
        }
    }

    impl gpui::Render for ChatPreviewStub {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let items = self.items.clone();
            let processor = move |ix: usize, _w: &mut gpui::Window, _cx: &mut gpui::App| match items
                .get(ix)
                .cloned()
            {
                Some(item) => gpui::div()
                    .w_full()
                    .pt_1()
                    .pb_4()
                    .flex_shrink_0()
                    .min_w_0()
                    .child(item)
                    .into_any_element(),
                None => gpui::div().into_any_element(),
            };
            gpui::div()
                .size_full()
                .flex()
                .flex_col()
                .px_4()
                .py_4()
                .child(
                    gpui::div()
                        .w_full()
                        .flex_1()
                        .min_h_0()
                        .child(gpui::list(self.list_state.clone(), processor)),
                )
        }
    }

    struct MainSeat {
        view: AnyView,
    }

    impl MainSurface for MainSeat {
        fn view(&self) -> AnyView {
            self.view.clone()
        }

        fn title(&self, _cx: &gpui::App) -> gpui::SharedString {
            "Manox".into()
        }
    }

    // ── dummy dock/tab surfaces (no PTY, no webview) ──────────────────────────
    //
    // A PTY in the harness trips gpui's leak detection and a wry subview never
    // reads back offscreen, so the shot exercises the tab chrome only.

    struct DummyKind;

    impl ToolTabFactory for DummyKind {
        fn kind(&self) -> &'static str {
            "dummy"
        }

        fn create(&self) -> Arc<dyn ToolTab> {
            Arc::new(DummyTab)
        }

        fn quick_action(&self) -> Option<gpui::SharedString> {
            Some("Dummy".into())
        }

        fn icon(&self, cx: &gpui::App) -> gpui::AnyElement {
            DummyTab.icon(cx)
        }
    }

    struct DummyTab;

    impl ToolTab for DummyTab {
        fn kind(&self) -> &'static str {
            "dummy"
        }

        fn id(&self) -> &str {
            "dummy-0"
        }

        fn title(&self, _cx: &gpui::App) -> gpui::SharedString {
            "Dummy".into()
        }

        fn icon(&self, _cx: &gpui::App) -> gpui::AnyElement {
            manox_agent_chrome_ui::theme::icon(icons::TOOLS, 15.).into_any_element()
        }

        fn open(
            &self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::App,
            _store: &mut TabStore,
            _pane: &gpui::WeakEntity<manox_agent_chrome_ui::RightPane>,
        ) {
        }

        fn render(
            &self,
            _window: &mut gpui::Window,
            _cx: &gpui::App,
            _store: &TabStore,
        ) -> gpui::AnyElement {
            gpui::div()
                .w_full()
                .h_full()
                .p(px(12.))
                .child("dummy tab body")
                .into_any_element()
        }
    }

    /// Diagnostic-only: a second tab kind, so `CHROME_SWITCH=1` can switch
    /// between two tabs of different widths and exercise the indicator slide.
    struct SecondKind;

    impl ToolTabFactory for SecondKind {
        fn kind(&self) -> &'static str {
            "second"
        }
        fn create(&self) -> Arc<dyn ToolTab> {
            Arc::new(SecondTab)
        }
        fn quick_action(&self) -> Option<gpui::SharedString> {
            Some("Second".into())
        }
        fn icon(&self, cx: &gpui::App) -> gpui::AnyElement {
            SecondTab.icon(cx)
        }
    }

    struct SecondTab;

    impl ToolTab for SecondTab {
        fn kind(&self) -> &'static str {
            "second"
        }
        fn id(&self) -> &str {
            "second-0"
        }
        fn title(&self, _cx: &gpui::App) -> gpui::SharedString {
            "A wider second tab".into()
        }
        fn icon(&self, _cx: &gpui::App) -> gpui::AnyElement {
            manox_agent_chrome_ui::theme::icon(icons::GLOBE, 15.).into_any_element()
        }
        fn open(
            &self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::App,
            _store: &mut TabStore,
            _pane: &gpui::WeakEntity<manox_agent_chrome_ui::RightPane>,
        ) {
        }
        fn render(
            &self,
            _window: &mut gpui::Window,
            _cx: &gpui::App,
            _store: &TabStore,
        ) -> gpui::AnyElement {
            gpui::div().into_any_element()
        }
    }

    struct DummyPanel;

    struct DummyPanelView;

    impl gpui::Render for DummyPanelView {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
                .w_full()
                .h_full()
                .p(px(12.))
                .child("dummy panel body")
        }
    }

    impl PanelSurface for DummyPanel {
        fn title(&self) -> gpui::SharedString {
            "Dummy".into()
        }

        fn icon(&self) -> Icon {
            icons::TOOLS
        }

        fn open(&self, _window: &mut gpui::Window, cx: &mut gpui::App) -> Result<AnyView, String> {
            Ok(AnyView::from(cx.new(|_| DummyPanelView)))
        }
    }

    // ── assets ────────────────────────────────────────────────────────────────

    struct EmptyAssets;

    impl gpui::AssetSource for EmptyAssets {
        fn load(&self, _path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
            Ok(None)
        }

        fn list(&self, _path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
            Ok(Vec::new())
        }
    }
}
