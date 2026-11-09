//! The launcher's workspace-side gestures (the entry + the three submit
//! hands): this module is a member of the workspace family (`super::*` —
//! `ViewMode`, the composer's submit face and the workspace's private fields
//! resolve unchanged). [`crate::launcher::LauncherState`] is the page's own
//! state machine; everything that MINTS a container or flips the workspace's
//! mode lives here.

use super::*;
use crate::launcher::{LauncherAgentLaunchPayload, LauncherState};
use crate::workspace::external_sessions::ExternalSessionLaunch;

impl crate::Workspace {
    /// Enter the launcher — the New gestures' landing (⌘N, the New button,
    /// the group menu's 新建对话 with a preset). Nothing is minted here. The
    /// entry yields the main column the way every other mode flip does (an
    /// on-screen external session parks, the sub-agent page drops, the
    /// settings nav unmounts); already on the launcher, it is idempotent —
    /// the composer takes focus. `preset` preselects the target (the project
    /// item gestures' project id, an inherited ad-hoc folder) and `locked`
    /// fixes it (the project-item entries: the target IS the entry's intent,
    /// the chip renders display-only — a bare entry is always unlocked). A
    /// fresh bare entry follows the no-project default (the target is an
    /// explicit creation-time pick, never a memory).
    pub fn enter_launcher(
        &mut self,
        preset: Option<crate::launcher::LauncherTarget>,
        locked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let already = matches!(self.view_mode, ViewMode::Launcher);
        if !already {
            self.leave_external_session();
            self.leave_subagent_stream(cx);
            if matches!(self.view_mode, ViewMode::Settings) {
                crate::chrome_assembly::leave_settings_layout(cx);
            }
            self.view_mode = ViewMode::Launcher;
        }
        let launcher = match self.launcher.clone() {
            Some(launcher) => launcher,
            None => {
                let ws = cx.weak_entity();
                let launcher = cx.new(|cx| LauncherState::new(ws, window, cx));
                self.launcher = Some(launcher.clone());
                launcher
            }
        };
        launcher.update(cx, |state, cx| {
            match preset {
                Some(target) => state.preset_target(target, locked, cx),
                // A preset entry keeps the current target; a bare fresh
                // entry resets to the no-project default.
                None if !already => state.apply_default_target(cx),
                None => {}
            }
            state.focus(window, cx);
        });
        // The launcher owns the main column: `chrome_selection` reads None
        // for this mode, so no row wears the highlight.
        cx.notify();
    }

    /// Whether a launcher send is staged but its container not yet attached —
    /// the ⏎-to-attach window the submit gesture's single-flight peek guards.
    pub(crate) fn first_turn_in_flight(&self) -> bool {
        self.pending_first_turn.is_some()
    }

    /// ⏎ on the launcher composer: mint the Manox container (the target's
    /// folder rides the create intent; the picks land in its config), stage
    /// the text, and let the attach completion flush it. The mode stays
    /// Launcher until the attach lands — the attach tail flips the column and
    /// [`Self::flush_pending_first_turn`] sends. The payload arrives
    /// pre-extracted (see `LauncherSubmitPayload`): this gesture runs in the
    /// PressEnter handler's wake, where re-entering the launcher entity
    /// panics the borrow. The handler peeked first: an in-flight or blank
    /// send never arrives here with composer text to lose.
    pub(crate) fn submit_launcher(
        &mut self,
        payload: crate::launcher::LauncherSubmitPayload,
        cx: &mut Context<Self>,
    ) {
        // Single flight: the staged text already names the container being
        // minted, and the flush is keyed to its attach. The gesture peeks
        // before consuming its composer, so an in-flight second send never
        // reaches here with text to lose.
        if self.pending_first_turn.is_some() {
            tracing::warn!("launcher send ignored: a first turn is already in flight");
            return;
        }
        let crate::launcher::LauncherSubmitPayload {
            text,
            target,
            model,
            access,
            effort,
        } = payload;
        if text.trim().is_empty() {
            return;
        }
        let roots = target.roots();
        let project_id = target.project_id().map(str::to_string);
        self.create_container(
            crate::workspace::container::ContainerSeed::Manox(
                crate::workspace::container::NativeContainerSeed {
                    roots,
                    project_id,
                    model,
                    access,
                    effort,
                },
            ),
            cx,
        );
        // The grouping fact lands at the attach tail (the seed carries it
        // there — a refused create must not leave a dead key); the launcher
        // itself remembers nothing across conversations.
        self.pending_first_turn = Some(text);
        cx.notify();
    }

    /// The launcher send's flush point (design §3.2): called from the attach
    /// completion, where the foreground store is bound to the fresh session.
    /// The send is CHAINED onto the fresh chat's own subscribe answer
    /// (`subscribe_then`) — a strict wire order. The racing alternative
    /// (dispatching on a separate tokio task, "one executor turn late")
    /// carried no order guarantee: a submit that lands before the host
    /// opened the chat is answered not-found and silently lost — the journal
    /// never materializes and the message is gone. The text rides the
    /// ordinary submit path — optimistic bubble and the sidebar's seed row
    /// included. A failed subscribe (either this direct leg or the
    /// connect-replay leg) answers the hook with `Failed`: the first turn
    /// refunds through [`Self::refund_first_turn`] instead of firing into a
    /// channel that never opened.
    pub(crate) fn flush_pending_first_turn(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.pending_first_turn.take() else {
            return;
        };
        let Some((store, sid)) = self.chat.read(cx).store.clone() else {
            // The unbound-store failure leg (design §6): the staged text
            // refunds, never swallowed.
            tracing::warn!("launcher send refunds: the foreground store is unbound");
            self.refund_first_turn(text, window, cx);
            return;
        };
        let ws = cx.weak_entity();
        store.update(cx, |store, cx| {
            store.subscribe_then(
                steer_agent_chat_ui::ahp_store::chat_uri(&sid),
                move |outcome, cx| {
                    let _ = ws.update_in(cx, |ws, window, cx| match outcome {
                        steer_agent_chat_ui::ahp_store::SubscribeThenOutcome::Answered => {
                            ws.send_user_turn(text, Vec::new(), SubmitBehavior::Send, cx);
                        }
                        steer_agent_chat_ui::ahp_store::SubscribeThenOutcome::Failed(err) => {
                            tracing::warn!(error = %err, "first-turn subscribe failed; refund");
                            ws.refund_first_turn(text, window, cx);
                        }
                    });
                },
                cx,
            );
        });
    }

    /// The failure legs' ONE refund face (design §6): re-open the launcher
    /// and put `text` back into its composer. Every failure point funnels
    /// here — the createSession refusal, a failed first-turn subscribe on
    /// either leg, an unbound foreground store, a failed agent spawn — a
    /// user-initiated send is never swallowed. `restore_text` fills only an
    /// empty composer, so a refund never clobbers input submitted during
    /// the flight. The re-entry carries the launcher's own target when the
    /// attach tail already flipped the column away: a bare re-entry there
    /// would land the no-project default OVER the user's pick (the submit
    /// never cleared the launcher's target). Already on the launcher, the
    /// bare entry is the no-op branch — the preset write would clear a
    /// locked page's lock bit.
    pub(crate) fn refund_first_turn(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let already = matches!(self.view_mode, ViewMode::Launcher);
        let preset = if already {
            None
        } else {
            self.launcher
                .as_ref()
                .map(|launcher| launcher.read(cx).snapshot().0)
        };
        self.enter_launcher(preset, false, window, cx);
        if let Some(launcher) = &self.launcher {
            launcher.update(cx, |state, cx| state.restore_text(text, window, cx));
        }
        cx.notify();
    }

    /// The staged-slot refund — the createSession refusal feeds from
    /// [`Self::pending_first_turn`]; the face is [`Self::refund_first_turn`].
    pub(crate) fn refund_launcher_first_turn(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = self.pending_first_turn.take() {
            self.refund_first_turn(text, window, cx);
        }
    }

    /// One agent cascade pick: spawn the CLI as the container-to-be's 0 号,
    /// rooted at the target (the primary root or ad-hoc folder; the
    /// foreground-container fallback the old menu kept). A non-empty composer
    /// text rides the argv as the CLI's initial prompt; the membership lands
    /// with the mint.
    pub(crate) fn launch_agent_from_launcher(
        &mut self,
        launch: LauncherAgentLaunchPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(launcher) = self.launcher.clone() else {
            return;
        };
        let (target, _, _, _) = launcher.read(cx).snapshot();
        let prompt = launcher.update(cx, |state, cx| state.take_text_if_present(window, cx));
        let project_path = target.folder();
        let project_id = target.project_id().map(str::to_string);
        let cwd = project_path
            .clone()
            .unwrap_or_else(|| crate::tool_tabs::thread_cwd_or_home(cx));
        match crate::tool_tabs::spawn_agent_terminal(
            launch.agent_id,
            &cwd,
            &launch.provider,
            &launch.model,
            launch.wire,
            prompt.as_deref(),
            cx,
        ) {
            Ok(spawned) => {
                let id = self.spawn_external_session(
                    ExternalSessionLaunch {
                        agent_id: launch.agent_id,
                        cx_session_id: spawned.cx_session_id,
                        label: launch.display.to_string(),
                        svg: launch.svg,
                        cwd,
                        handle: Some(spawned.handle),
                    },
                    spawned.view,
                    window,
                    cx,
                );
                if let Some(project_id) = project_id {
                    crate::projects::set_membership(&id, &project_id);
                }
            }
            Err(e) => {
                spawn_failed_notification(launch.display, &e, window, cx);
                // The spawn-failure leg (design §6): the composer text the
                // gesture consumed as the initial prompt refunds through the
                // one face — already on the launcher, so the refund is the
                // refill plus the composer focus.
                if let Some(prompt) = prompt {
                    self.refund_first_turn(prompt, window, cx);
                }
            }
        }
    }

    /// The terminal pill: no intermediate step — the existing standalone-PTY
    /// path (the retired group menu's 新建终端), rooted at the target. The
    /// composer text is left alone — the terminal takes no initial prompt,
    /// and the draft stays in the launcher for the user's return (never
    /// silently dropped).
    pub(crate) fn launch_terminal_from_launcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(launcher) = self.launcher.clone() else {
            return;
        };
        let (target, _, _, _) = launcher.read(cx).snapshot();
        let project_path = target.folder();
        let project_id = target.project_id().map(str::to_string);
        let cwd = project_path
            .clone()
            .unwrap_or_else(|| crate::tool_tabs::thread_cwd_or_home(cx));
        // The shell PTY IS the new container's 0 号 — no container env to
        // inherit; its own attrs snapshot is captured at the registration.
        let no_attrs = crate::workspace::container::ContainerAttrs {
            cwd: cwd.clone(),
            env: Default::default(),
        };
        match crate::tool_tabs::spawn_standalone_terminal(&cwd, &no_attrs, cx) {
            Ok(view) => {
                let id = self.spawn_external_session(
                    ExternalSessionLaunch {
                        agent_id: "terminal",
                        cx_session_id: String::new(),
                        label: steer_i18n::t("chrome-tab-ghostty").to_string(),
                        svg: "icons/terminal.svg",
                        cwd,
                        handle: None,
                    },
                    view,
                    window,
                    cx,
                );
                if let Some(project_id) = project_id {
                    crate::projects::set_membership(&id, &project_id);
                }
            }
            Err(e) => {
                spawn_failed_notification(&steer_i18n::t("chrome-tab-ghostty"), &e, window, cx)
            }
        }
    }
}

/// The failure notice names the program that failed to start (the project
/// menu's own shape — a spawn failure must not open anything silently).
fn spawn_failed_notification(prog: &str, error: &str, window: &mut Window, cx: &mut App) {
    use gpui_component::WindowExt as _;
    window.push_notification(
        gpui_component::notification::Notification::error(steer_i18n::t_str(
            "chrome-spawn-failed",
            &[("prog", prog), ("err", error)],
        )),
        cx,
    );
}
