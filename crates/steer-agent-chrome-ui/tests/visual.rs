//! Visual acceptance (harness=false, main thread): gpui's official
//! offscreen render — `VisualTestAppContext` + Metal texture readback
//! (`capture_screenshot`), no screen-recording TCC permission needed.
//!
//! Run with `CHROME_SHOT=/tmp/chrome.png cargo test -p steer-agent-chrome-ui
//! --test visual`; `CHROME_RIGHT=1` expands the right pane with a dummy tab
//! (no PTY/webview — those need a real window), `CHROME_SELECT=1` the
//! sidebar's batch-selection mode with two rows checked, `CHROME_PANEL=1` the bottom
//! dock, `CHROME_PICKER=1` the titlebar session picker with its dropdown
//! open. macOS-only: the readback is a Metal texture and the harness's
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
    use steer_agent_chat_ui::ask_card::{AskCardOption, AskCardQuestion, AskCardSnapshot};
    use steer_agent_chat_ui::conversation::{ConvItem, ToolCallItem};
    use steer_agent_chat_ui::host::noop_host;
    use steer_agent_chat_ui::views::message::MessageItem;
    use steer_agent_chrome_ui::right_pane::TabStore;
    use steer_agent_chrome_ui::session_list::{SessionRowKind, SessionStatus};
    use steer_agent_chrome_ui::shell::SessionRow;
    use steer_agent_chrome_ui::theme::IconAsset;
    use steer_agent_chrome_ui::{
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
        let mut cx =
            VisualTestAppContext::with_asset_source(platform, Arc::new(gpui_kit_assets::AllAssets));
        cx.update(|cx| {
            gpui_component::init(cx);
            steer_i18n::init();
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
                if std::env::var("CHROME_PICKER").is_ok() {
                    shell.update(cx, |shell, cx| {
                        shell.toggle_session_picker(window, cx);
                        cx.notify();
                    });
                }
                // CHROME_SELECT=1: the sidebar's batch-selection mode with the
                // first two thread rows checked — the check circles, the
                // accent stroke and the bottom action bar.
                if std::env::var("CHROME_SELECT").is_ok() {
                    shell.update(cx, |shell, cx| {
                        let ids: Vec<String> = shell
                            .sessions
                            .iter()
                            .filter(|row| matches!(row.kind, SessionRowKind::Thread))
                            .map(|row| row.id.clone())
                            .take(2)
                            .collect();
                        for id in ids {
                            shell.begin_selection(&id, cx);
                        }
                        cx.notify();
                    });
                }
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
                    label: steer_i18n::t("chrome-sidebar-automations"),
                    badge: None,
                },
                FixedRow {
                    icon: icons::COMMENT_DISCUSSION,
                    label: steer_i18n::t("chrome-sidebar-chats"),
                    badge: None,
                },
            ],
            customizations: vec![
                CustomizationRow {
                    icon: icons::HOME,
                    label: steer_i18n::t("chrome-sidebar-overview"),
                    count: None,
                },
                CustomizationRow {
                    icon: icons::EXTENSIONS,
                    label: steer_i18n::t("chrome-sidebar-plugins"),
                    count: None,
                },
                CustomizationRow {
                    icon: icons::SETTINGS_GEAR,
                    label: steer_i18n::t("chrome-sidebar-mcp"),
                    count: None,
                },
                CustomizationRow {
                    icon: icons::WAND,
                    label: steer_i18n::t("chrome-sidebar-skills"),
                    count: None,
                },
            ],
            hooks: HostHooks {
                // Live nav edges: the screenshot is the only acceptance face
                // for the titlebar's enabled-arrow state.
                nav_avail: Some(Box::new(|_| steer_agent_chrome_ui::shell::NavAvail {
                    back: true,
                    forward: false,
                })),
                ..Default::default()
            },
        }
    }

    /// Static session snapshot from the real store, seeded before the first
    /// render (the pump-less harness never gets a second push). The status
    /// mapping mirrors the host ladder for the faces the legacy store
    /// carries — running wins over unread wins over idle. `Pending` has no
    /// store face (it lives on the live fold), so the second row is
    /// synthesized to keep the amber question mark in the shot.
    fn snapshot_rows() -> Vec<SessionRow> {
        let store = manox_agent::thread_store::global();
        let mut rows: Vec<SessionRow> = store.read(|st| {
            st.summaries()
                .iter()
                .filter(|t| !t.archived && t.depth == 0)
                .enumerate()
                .take(12)
                .map(|(ix, t)| SessionRow {
                    id: t.id.clone(),
                    short_id: "abcd1234".into(),
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
                    project: (!t.project.is_empty()).then(|| t.project.clone()),
                    kind: steer_agent_chrome_ui::session_list::SessionRowKind::Thread,
                    status: if st.is_running(&t.id) {
                        SessionStatus::Running
                    } else if t.has_unread {
                        SessionStatus::Unread
                    } else {
                        SessionStatus::Idle
                    },
                    tag: None,
                    team_leader: false,
                    updated_at: t.updated_at,
                    sort_stamp: t.updated_at,
                    rank: ix as i64,
                    pinned: t.pinned,
                    archived: t.archived,
                })
                .collect()
        });

        // The synthetic pending row: an ask parked on the user, the one
        // rung the legacy store snapshot can never produce.
        if let Some(row) = rows.get_mut(1) {
            row.status =
                SessionStatus::Pending(steer_agent_chrome_ui::session_list::PendingKind::Ask);
        }
        rows
    }

    // ── main-surface seat ─────────────────────────────────────────────────────

    /// The demo ask: three questions covering single-select, multi-select and
    /// a free-text note.
    fn seed_snapshot() -> AskCardSnapshot {
        let question = |header: &str,
                        question: &str,
                        multi_select: bool,
                        options: Vec<(&str, &str, bool)>| AskCardQuestion {
            question: question.to_string(),
            header: header.to_string(),
            detail: String::new(),
            multi_select,
            options: options
                .into_iter()
                .map(|(label, description, recommended)| AskCardOption {
                    label: label.to_string(),
                    description: description.to_string(),
                    recommended,
                })
                .collect(),
        };
        AskCardSnapshot {
            id: "ask-seed".into(),
            questions: vec![
                question(
                    "存储",
                    "这个服务用哪个数据库？",
                    false,
                    vec![
                        ("PostgreSQL", "事务 + JSONB，跟现有部署一致", true),
                        ("MySQL", "团队更熟悉，但需要补迁移脚本", false),
                    ],
                ),
                question(
                    "功能",
                    "这一期先启用哪些能力？",
                    true,
                    vec![
                        ("查询缓存", "命中率可观测", false),
                        ("全文检索", "先把接口留好", false),
                        ("指标上报", "", false),
                    ],
                ),
                question("补充", "还有什么需要我注意的吗？", false, Vec::new()),
            ],
        }
    }

    /// A plan review, as the built-in `/plan` skill raises one: a plain ask
    /// whose `detail` carries the plan body (the request-level `message` on
    /// the wire) and whose options are the three verdicts.
    /// One card face — no separate decision surface.
    fn plan_review_snapshot() -> AskCardSnapshot {
        AskCardSnapshot {
            id: "plan-review-seed".into(),
            questions: vec![AskCardQuestion {
                question: "批准这个计划吗？".into(),
                header: "Plan 评审".into(),
                detail: "## 帖子服务拆分\n\n1. 建 `posts` 表与迁移脚本\n2. 查询缓存接一层\n3. 全文检索先留接口\n\n> 预计两轮迭代".into(),
                multi_select: false,
                options: vec![
                    AskCardOption {
                        label: "Approve".into(),
                        description: "开始执行".into(),
                        recommended: true,
                    },
                    AskCardOption {
                        label: "Approve & compact".into(),
                        description: "先压缩上下文再执行".into(),
                        recommended: false,
                    },
                    AskCardOption {
                        label: "Request changes".into(),
                        description: "继续讨论".into(),
                        recommended: false,
                    },
                ],
            }],
        }
    }

    /// A minimal seeded message list (the chat pipeline's real `MessageItem`s),
    /// so the acceptance capture shows chat content inside the shell.
    struct ChatPreviewStub {
        items: Vec<gpui::Entity<MessageItem>>,
    }

    impl ChatPreviewStub {
        fn new(cx: &mut gpui::Context<Self>) -> Self {
            let mk = |cx: &mut gpui::Context<Self>, item: ConvItem, role: &str| {
                cx.new(|_| MessageItem::new(item, role.to_string(), 0, noop_host()))
            };
            // `CHROME_ASK=<page>` swaps the seeded transcript for a pending
            // `AskUserQuestion` card on that page (0-based), `CHROME_ASK=answered`
            // for the same card in its answered phase.
            if let Ok(mode) = std::env::var("CHROME_ASK") {
                return match mode.as_str() {
                    "answered" => Self::answered_ask_card(cx),
                    "plan" => Self::ask_card_with(cx, 0, plan_review_snapshot()),
                    page => Self::ask_card(cx, page.parse::<usize>().unwrap_or(0)),
                };
            }
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
                    card: None,
                    cwd: None,
                }),
                "",
            ),
            mk(
                cx,
                ConvItem::Assistant {
                    text: "主栏由 **steer-agent-chat-ui** 的消息管线渲染。\n\n- markdown 列表\n- `代码` 与代码块\n\n```rust\nlet shell = chrome::Shell::new(config, window, cx);\n```".into(),
                    streaming: false,
                    token_usage: None,
                    activity_header: true,
                    entry_id: None,
                    fork_unavailable: None,
                },
                "GLM-5.3",
            ),
        ];
            Self { items }
        }

        /// The pending ask, as the conversation mounts it: the synthesized
        /// `ToolCall` row carrying the snapshot and the card's own state. The
        /// state is seeded by hand here (the workspace's `ensure_ask_state`
        /// does it live), including the answers a reader would see mid-form.
        fn ask_card(cx: &mut gpui::Context<Self>, page: usize) -> Self {
            Self::ask_card_with(cx, page, seed_snapshot())
        }

        /// The same card over any snapshot — the plan-review mode reuses it.
        fn ask_card_with(
            cx: &mut gpui::Context<Self>,
            page: usize,
            snapshot: AskCardSnapshot,
        ) -> Self {
            let multi: Vec<bool> = snapshot.questions.iter().map(|q| q.multi_select).collect();
            let state = cx.new(|cx| ai_elements::AskState::new(multi, "添加可选补充说明", cx));
            state.update(cx, |state, cx| {
                state.toggle_option(0, 0, cx);
                state.set_page(page, cx);
            });
            let ask = cx.new(|_| {
                MessageItem::new(
                    ConvItem::ToolCall(ToolCallItem {
                        id: "ask-seed".into(),
                        name: manox_agent::tools::ASK_USER_QUESTION.to_string(),
                        title: "存储 · 功能 · 补充".into(),
                        status: manox_agent::ToolCallStatus::PendingApproval,
                        output: String::new(),
                        is_error: false,
                        input: serde_json::Value::Null,
                        streaming: false,
                        collapsed: false,
                        user_toggled: false,
                        panel: None,
                        card: None,
                        cwd: None,
                    }),
                    "GLM-5.3".into(),
                    0,
                    noop_host(),
                )
            });
            ask.update(cx, |item, _| {
                item.ask_snapshot = Some(snapshot);
                item.ask_state = Some(state);
            });
            let items = vec![
                cx.new(|_| {
                    MessageItem::new(
                        ConvItem::User {
                            text: "帮我起一个帖子服务".into(),
                            images: Vec::new(),
                            meta: None,
                        },
                        String::new(),
                        0,
                        noop_host(),
                    )
                }),
                ask,
            ];
            Self { items }
        }

        /// The answered ask, as the transcript keeps it: the synthesized
        /// `ToolCall` row carrying the call's own input and the server's
        /// canonical answer payload — the row mounts the card's settled phase
        /// itself (no `AskState` is seeded here; the row owns it).
        fn answered_ask_card(cx: &mut gpui::Context<Self>) -> Self {
            let input = serde_json::json!({
                "questions": [
                    { "id": "one", "question": "这个服务用哪个数据库？",
                      "options": [{"label": "PostgreSQL"}, {"label": "MySQL"}] },
                    { "id": "many", "question": "这一期先启用哪些能力？", "multiSelect": true,
                      "options": [{"label": "查询缓存"}, {"label": "全文检索"}, {"label": "指标上报"}] },
                    { "id": "free", "question": "还有什么需要我注意的吗？" }
                ]
            });
            let output = r#"{"answers":[
                {"id":"one","selected":["PostgreSQL"]},
                {"id":"many","selected":["查询缓存","指标上报"],"custom":"全文检索先留接口"},
                {"id":"free","selected":[]}
            ]}"#;
            let ask = cx.new(|_| {
                MessageItem::new(
                    ConvItem::ToolCall(ToolCallItem {
                        id: "ask-seed".into(),
                        name: manox_agent::tools::ASK_USER_QUESTION.to_string(),
                        title: "存储 · 功能 · 补充".into(),
                        status: manox_agent::ToolCallStatus::Success,
                        output: output.to_string(),
                        is_error: false,
                        input,
                        streaming: false,
                        collapsed: false,
                        user_toggled: false,
                        panel: None,
                        card: None,
                        cwd: None,
                    }),
                    "GLM-5.3".into(),
                    0,
                    noop_host(),
                )
            });
            let items = vec![
                cx.new(|_| {
                    MessageItem::new(
                        ConvItem::User {
                            text: "帮我起一个帖子服务".into(),
                            images: Vec::new(),
                            meta: None,
                        },
                        String::new(),
                        1,
                        noop_host(),
                    )
                }),
                ask,
            ];
            Self { items }
        }
    }

    impl gpui::Render for ChatPreviewStub {
        // A plain column, NOT `gpui::list`: the virtualized list never
        // resolves its rows' heights under the offscreen harness, so the
        // acceptance capture came out with an empty main column. The rows here
        // are the same real `MessageItem`s — the list is what the live shell
        // adds on top, not what the shot is about.
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            let items = self.items.clone();
            let rows: Vec<gpui::AnyElement> = (0..items.len())
                .map(|ix| {
                    items
                        .get(ix)
                        .cloned()
                        .map(|item| {
                            gpui::div()
                                .w_full()
                                .pt_1()
                                .pb_4()
                                .flex_shrink_0()
                                .min_w_0()
                                .child(item)
                                .into_any_element()
                        })
                        .unwrap_or_else(|| gpui::div().into_any_element())
                })
                .collect();
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
                        .children(rows)
                        .into_any_element(),
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
            "Steer".into()
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
            steer_agent_chrome_ui::theme::icon(icons::TOOLS, 15.).into_any_element()
        }

        fn open(
            &self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::App,
            _store: &mut TabStore,
            _pane: &gpui::WeakEntity<steer_agent_chrome_ui::RightPane>,
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
            steer_agent_chrome_ui::theme::icon(icons::GLOBE, 15.).into_any_element()
        }
        fn open(
            &self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::App,
            _store: &mut TabStore,
            _pane: &gpui::WeakEntity<steer_agent_chrome_ui::RightPane>,
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

        fn icon(&self) -> IconAsset {
            icons::TOOLS
        }

        fn open(&self, _window: &mut gpui::Window, cx: &mut gpui::App) -> Result<AnyView, String> {
            Ok(AnyView::from(cx.new(|_| DummyPanelView)))
        }
    }
}
