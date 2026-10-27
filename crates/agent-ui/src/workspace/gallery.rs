//! The send-mode gallery (`cargo run -p agent-ui --features test-support
//! --example send_mode_gallery`): a self-driving eyeball surface for the
//! composer's mid-turn interactions — no host, no model, no wire. The
//! workspace mounts for real (the chrome assembly, the real composer, the
//! real queue), then its store pair is swapped for a detached
//! [`AhpStore::detached`] one whose book holds a fabricated active turn, so
//! `running()` reads true and every write parks instead of dialing. The
//! driver strip on top fires the same methods the live pump would
//! ([`Workspace::settle_follow_ups`], [`Workspace::retire_steer_card`]);
//! Enter in the composer rides the real `send_user_turn` mode routing.
//! Nothing here ships: the whole module is `test-support`-gated, and the
//! strip's labels are gallery-only literals (not app-chrome copy).
//!
//! NOTE: the public driver surface below (gallery_*) is ALSO the API the
//! `send_mode_lanes` regression test drives — deleting this module deletes
//! that test's scaffolding with it.

use super::*;
use gpui::{App, Entity, IntoElement, div, size};
use gpui_component::Root;
use steer_agent_chat_ui::ahp_store::{AhpStore, chat_uri};
use steer_agent_chrome_ui::shell::Shell;

/// The chat fold a fabricated mid-turn scene rides: one completed turn for
/// scenery plus an active turn only while the scene is "running".
fn fabricated_chat(chat_id: &str, running: bool) -> ahp_types::state::ChatState {
    let mut value = serde_json::json!({
        "resource": chat_uri(chat_id),
        "title": "send-mode gallery",
        "status": 0,
        "modifiedAt": "2026-10-22T00:00:00Z",
        "turns": [{
            "id": "t-1",
            "startedAt": "2026-10-22T00:00:00Z",
            "message": { "text": "帮我梳理这个仓库的构建脚本", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": "p-1", "content": "构建脚本在 `script/` 下，入口是 `check.sh`。" },
            ],
            "state": "complete",
        }],
    });
    if running {
        value["activeTurn"] = serde_json::json!({
            "id": "t-live",
            "startedAt": "2026-10-22T00:00:01Z",
            "message": { "text": "继续，把测试也过一遍", "origin": { "kind": "user" } },
            "responseParts": [
                { "kind": "markdown", "id": "p-2", "content": "正在跑 `cargo test`…" },
            ],
        });
    }
    serde_json::from_value(value).expect("the fabricated chat parses")
}

/// Swap the mounted workspace's foreground store for a detached one holding
/// the fabricated scene, then seed the transcript and the queue so every
/// surface has something to show on the first frame. Also the test binary's
/// fixture (see `tests/send_mode_lanes.rs`).
fn install_scene(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    let sid = ws
        .chat
        .read(cx)
        .store
        .as_ref()
        .map(|(_, sid)| sid.clone())
        .expect("the assembly binds a foreground session");
    let mut store = AhpStore::detached();
    store.diagnostic_set_chat(&sid, fabricated_chat(&sid, true));
    // A real store's session catalogue owns the chat; bucket routing
    // (`session_of_chat`) resolves through it, so the detached fixture
    // seeds the owning row too.
    store.diagnostic_seed_session_for_chat(&sid, &sid);
    let store = cx.new(|_| store);
    ws.chat.update(cx, |chat, cc| {
        chat.store = Some((store, sid.clone()));
        chat.turn_active = true;
        cc.notify();
    });
    set_running(ws, true, cx);
    seed_transcript(ws, cx);
    preset_queue(ws, cx);
    tracing::info!(session_id = %sid, "gallery scene installed");
}

/// One finished exchange through the REAL apply path, so the transcript
/// rows are exactly what a settled turn renders.
fn seed_transcript(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    let role = ws.model_label(cx);
    let meta = ws.user_turn_meta(cx);
    let host = ws.chat.read(cx).host.clone();
    let cwd = thread_cwd(&ws.chat.read(cx).thread, &ws.chat.read(cx).store, cx);
    let text = "帮我梳理这个仓库的构建脚本".to_string();
    ws.chat_conversation(cx).update(cx, |c, cx| {
        c.push_user(text, Vec::new(), meta, host.clone(), cx);
        let ctx = crate::conversation::ApplyCtx {
            host: host.clone(),
            cwd: cwd.clone(),
            fork_source: None,
        };
        c.apply(&manox_agent::ThreadEvent::TurnStarted, &role, None, ctx, cx);
        let ctx = crate::conversation::ApplyCtx {
            host,
            cwd,
            fork_source: None,
        };
        c.apply(
            &manox_agent::ThreadEvent::AgentText(
                "构建脚本在 `script/` 下，入口是 `check.sh`。".to_string(),
            ),
            &role,
            None,
            ctx,
            cx,
        );
    });
    ws.sync_list_count(cx);
    ws.follow_message_tail(cx);
}

/// Fabricate (or clear) the active turn in the detached book — the same
/// edge the live `chat/turnStarted` / `turnFinished` echoes drive.
fn set_running(ws: &mut Workspace, on: bool, cx: &mut Context<Workspace>) {
    let Some((store, sid)) = ws.chat.read(cx).store.clone() else {
        return;
    };
    store.update(cx, |store, _| {
        store.diagnostic_set_chat(&sid, fabricated_chat(&sid, on));
    });
    ws.chat.update(cx, |chat, cc| {
        chat.turn_active = on;
        cc.notify();
    });
    if on {
        ws.spawn_thinking_ticker(cx);
    }
    cx.notify();
}

/// Queue one card per [`FollowUpState`] so every badge and button cluster
/// is on screen at once.
fn preset_queue(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    let card = |text: &str, state| QueuedFollowUp {
        turn: DeferredUserTurn {
            text: text.to_string(),
            images: Vec::new(),
            meta: crate::conversation::UserTurnMeta::new(0, "gallery".to_string(), None),
            user_images: Vec::new(),
        },
        state,
    };
    ws.chat.update(cx, |chat, cc| {
        chat.queued_follow_ups = [
            card(
                "先只看失败的测试",
                FollowUpState::SteerPending {
                    message_id: "gallery-steer-1".to_string(),
                },
            ),
            card("这条发送没能上线，可重试", FollowUpState::Failed),
            card("把错误日志再贴一遍", FollowUpState::Queued),
        ]
        .into_iter()
        .collect();
        cc.notify();
    });
    cx.notify();
}

/// The strip's one-click verbs. Each fires the same Workspace methods the
/// live event pump would, so what the gallery shows is the real machinery.
impl Workspace {
    fn settle_normally(&mut self, cx: &mut Context<Self>) {
        set_running(self, false, cx);
        self.settle_follow_ups(false, cx);
    }

    fn settle_cancelled(&mut self, cx: &mut Context<Self>) {
        set_running(self, false, cx);
        self.settle_follow_ups(true, cx);
    }

    fn consume_first_steer(&mut self, cx: &mut Context<Self>) {
        let id = self
            .chat
            .read(cx)
            .queued_follow_ups
            .iter()
            .find_map(|item| match &item.state {
                FollowUpState::SteerPending { message_id } => Some(message_id.clone()),
                _ => None,
            });
        if let Some(id) = id {
            self.retire_steer_card(&id, cx);
        }
    }

    fn toggle_running(&mut self, cx: &mut Context<Self>) {
        set_running(self, !self.foreground_running(cx), cx);
    }
}

/// The gallery's public driver surface — and the send-mode integration
/// test's API (`tests/send_mode_lanes.rs`): the tests drive the REAL
/// routing through these wrappers because the workspace methods they
/// exercise sit below `pub(crate)`.
impl Workspace {
    /// Fabricate (or clear) the running turn, detached-store edition.
    pub fn gallery_set_running(&mut self, on: bool, cx: &mut Context<Self>) {
        set_running(self, on, cx);
    }

    /// Submit a draft with one of the three streaming behaviors.
    pub fn gallery_submit(&mut self, text: &str, behavior: SubmitBehavior, cx: &mut Context<Self>) {
        self.send_user_turn(text.to_string(), Vec::new(), behavior, cx);
    }

    /// Settle with (or without) the cancelled bit — the pump's
    /// `TurnFinished` arm, minus the sidebar refresh.
    pub fn gallery_settle(&mut self, cancelled: bool, cx: &mut Context<Self>) {
        self.settle_follow_ups(cancelled, cx);
    }

    /// Replay the parked bucket (the re-attach reconcile path).
    pub fn gallery_replay_parked(&mut self, cx: &mut Context<Self>) {
        self.replay_parked_chat_events(cx);
    }

    /// Route one raw wire envelope through the store's production dispatch
    /// point (`route`), exactly as the host's broadcast lands — the true
    /// wire-shape edition of the consumption tests (a removal may precede
    /// its `pendingMessageSet` echo; the pre-derived event injection skips
    /// the fold gate that ordering exposes).
    pub fn gallery_route_raw_envelope(
        &mut self,
        action: ahp_types::actions::StateAction,
        cx: &mut Context<Self>,
    ) {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            return;
        };
        let channel = chat_uri(&sid);
        store.update(cx, |store, cx| {
            store.diagnostic_route_event(
                ahp::ClientEvent {
                    channel: channel.clone(),
                    event: ahp::SubscriptionEvent::Action(ahp_types::actions::ActionEnvelope {
                        channel,
                        action,
                        server_seq: 1,
                        origin: None,
                        rejection_reason: None,
                    }),
                },
                cx,
            );
        });
    }

    /// Inject a wire consumption receipt into the store's event bucket, as
    /// the backend's injection echo would land while the thread is parked.
    pub fn gallery_inject_steer_consumed(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            return;
        };
        store.update(cx, |store, _| {
            store.push_chat_event_for_test(
                &sid,
                crate::chat_fold::ChatEvent::SteerConsumed {
                    message_id: message_id.to_string(),
                },
            );
        });
    }

    /// Queue strip snapshot: (state name, message id or "", first line).
    pub fn gallery_queue_snapshot(&self, cx: &App) -> Vec<(String, String)> {
        self.chat
            .read(cx)
            .queued_follow_ups
            .iter()
            .map(|item| {
                let lane = match &item.state {
                    FollowUpState::Queued => "queued",
                    FollowUpState::SteerPending { message_id: _ } => "steer",
                    FollowUpState::Failed => "failed",
                };
                (
                    lane.to_string(),
                    item.turn.text.lines().next().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    pub fn gallery_outbox_len(&self, cx: &App) -> usize {
        self.chat.read(cx).interrupt_outbox.len()
    }

    /// Last user bubble text in the transcript (the flush/steer-retire
    /// landing spot).
    pub fn gallery_last_user_bubble(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .conversation
            .read(cx)
            .items()
            .iter()
            .filter_map(|item| match item.read(cx).kind() {
                ConvItem::User { text, .. } => Some(text.trim().to_string()),
                _ => None,
            })
            .last()
    }

    /// What the detached store would have ridden: the parked dispatch tags.
    pub fn gallery_dispatch_log(&self, cx: &App) -> Vec<(String, String)> {
        self.chat
            .read(cx)
            .store
            .as_ref()
            .map(|(store, _)| store.read(cx).diagnostic_dispatch_log())
            .unwrap_or_default()
    }

    /// A steer card's pending id (the correlate for consumption injection).
    pub fn gallery_first_steer_id(&self, cx: &App) -> Option<String> {
        self.chat
            .read(cx)
            .queued_follow_ups
            .iter()
            .find_map(|item| match &item.state {
                FollowUpState::SteerPending { message_id } => Some(message_id.clone()),
                _ => None,
            })
    }

    /// 进入引导 on the idx-th card (the lane move).
    pub fn gallery_steer_card(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.steer_follow_up(idx, cx);
    }

    /// 重新排队 on the idx-th card (withdraw + demote).
    pub fn gallery_requeue_card(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.requeue_follow_up(idx, cx);
    }

    /// 立即打断 on the idx-th card.
    pub fn gallery_interrupt_card(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.interrupt_from_card(idx, cx);
    }

    /// 编辑 on the idx-th card (withdraw + back into the composer).
    pub fn gallery_edit_card(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_follow_up(idx, window, cx);
    }

    /// 删除 on the idx-th card.
    pub fn gallery_delete_card(&mut self, idx: usize, cx: &mut Context<Self>) {
        self.delete_follow_up(idx, cx);
    }

    /// Inject a parked turn-start whose opening row IS a steer: the
    /// idle-steer wake path's receipt. `steer_id` is the id the row threads
    /// (`None` when the host threads none); `opening_text` is the row's text
    /// fallback (`None` for an empty row). The retire matches by id first,
    /// then by text.
    pub fn gallery_inject_parked_turn_start(
        &mut self,
        steer_id: Option<&str>,
        opening_text: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            return;
        };
        store.update(cx, |store, _| {
            store.push_chat_event_for_test(
                &sid,
                crate::chat_fold::ChatEvent::TurnStarted {
                    steer_id: steer_id.map(str::to_string),
                    opening_text: opening_text.map(str::to_string),
                },
            );
        });
    }

    /// The local mirror of the running edge (drives the thinking ticker).
    pub fn gallery_turn_active(&self, cx: &App) -> bool {
        self.chat.read(cx).turn_active
    }

    /// The composer draft (the edit action's landing spot).
    pub fn gallery_composer_text(&self, cx: &App) -> String {
        self.chat.read(cx).input_state.read(cx).value().to_string()
    }
}

/// Fixture entry for the test binary: bind a detached store under a fixed
/// session id, fabricate the running turn, seed the transcript — but a
/// CLEAN queue/outbox, so lane assertions start from zero (the eyeball
/// scene presets demo cards; the tests don't).
pub fn seed_scene(ws: &Entity<Workspace>, cx: &mut App) {
    const SID: &str = "send-mode-lanes";
    ws.update(cx, |ws, cx| {
        let store = cx.new(|_| AhpStore::detached());
        ws.diagnostic_bind_store(store, SID, cx);
    });
    ws.update(cx, |ws, cx| {
        install_scene(ws, cx);
        ws.chat.update(cx, |chat, cx| {
            chat.queued_follow_ups.clear();
            chat.interrupt_outbox.clear();
            cx.notify();
        });
    });
}

/// The driver bar docked above the shell. Gallery-only literals, never
/// shipped chrome.
struct GalleryStrip {
    ws: WeakEntity<Workspace>,
}

impl GalleryStrip {
    fn fire(
        &self,
        act: fn(&mut Workspace, &mut Context<Workspace>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.ws.upgrade() else {
            return;
        };
        ws.update(cx, act);
        window.refresh();
    }
}

impl Render for GalleryStrip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let btn = |id: &'static str,
                   label: &'static str,
                   act: fn(&mut Workspace, &mut Context<Workspace>),
                   cx: &mut Context<Self>| {
            Button::new(id)
                .secondary()
                .xsmall()
                .label(label)
                .on_click(cx.listener(move |this, _, window, cx| this.fire(act, window, cx)))
        };
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.5))
            .child(
                gpui::div()
                    .text_xs()
                    .text_color(theme.danger)
                    .child("发送模式演示（无真实会话）"),
            )
            .child(btn(
                "gallery-run",
                "运行 开/关",
                |ws, cx| ws.toggle_running(cx),
                cx,
            ))
            .child(btn(
                "gallery-settle",
                "正常结算",
                |ws, cx| ws.settle_normally(cx),
                cx,
            ))
            .child(btn(
                "gallery-cancel",
                "取消结算",
                |ws, cx| ws.settle_cancelled(cx),
                cx,
            ))
            .child(btn(
                "gallery-consume",
                "引导送达",
                |ws, cx| ws.consume_first_steer(cx),
                cx,
            ))
            .child(btn("gallery-preset", "预设队列", preset_queue, cx))
            .child(
                gpui::div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("↩ 排队 · ⌘↩ 引导 · ⇧⌘↩ 打断；卡片上可打断/重排队/编辑/删除"),
            )
            .into_any_element()
    }
}

/// The strip + shell pair the gallery window mounts (an entity, because
/// `Root::new` takes an `Into<AnyView>`).
struct GalleryRoot {
    strip: Entity<GalleryStrip>,
    shell: Entity<Shell>,
}

impl Render for GalleryRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(self.strip.clone())
            .child(div().flex_1().min_h_0().child(self.shell.clone()))
    }
}

/// Build the gallery window: the real chrome assembly with the driver bar
/// docked above it. The application comes from the example binary (the
/// platform constructor lives in a dev-dependency).
pub fn launch(app: gpui::Application) {
    app.with_assets(crate::assets::ExtrasAssetSource::new())
        .run(|cx| {
            gpui_component::init(cx);
            manox_agent::host::set_host(manox_agent::host::Host::ManoxApp);
            manox_agent::runtime::init_hermetic_for_test();
            manox_agent::provider_glue::init();
            manox_agent::thread_store::init();
            manox_agent::object_store::init();
            steer_i18n::init();
            gpui_component::set_locale(steer_i18n::current().token());
            crate::slash_command::init(cx);
            cx.bind_keys(crate::composer_send_key_bindings(cx));

            let options = gpui::WindowOptions {
                window_min_size: Some(size(px(980.), px(640.))),
                ..Default::default()
            };
            let window = cx
                .open_window(options, |window, cx| {
                    window.activate_window();
                    window.set_window_title("Steer — 发送模式演示");
                    gpui_component::Theme::change(
                        gpui_component::ThemeMode::Light,
                        Some(window),
                        cx,
                    );
                    steer_agent_chrome_ui::register_fonts(cx);
                    let shell = crate::chrome_assembly::mount(window, cx);
                    let ws = crate::dispatch::workspace_global().expect("assembly workspace");
                    ws.update(cx, install_scene);
                    let strip = cx.new(|_| GalleryStrip { ws: ws.downgrade() });
                    let body = cx.new(|_| GalleryRoot { strip, shell });
                    cx.new(|cx| Root::new(body, window, cx))
                })
                .expect("gallery window opens");
            crate::dispatch::set_window(window);
        });
}
